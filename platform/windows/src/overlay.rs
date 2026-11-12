//! One fullscreen overlay window, on one monitor.
//!
//! A `WS_POPUP` with no frame, sized to the monitor's rectangle and kept on
//! `HWND_TOPMOST`. That is the whole of what `wlr-layer-shell` is needed for
//! on Wayland, and here it is three API calls.
//!
//! Two details are not obvious:
//!
//! * A region fill with no second colour has to leave the desktop visible
//!   underneath, and a plain window has no hole in it. The uncovered half is
//!   painted in a key colour and `WS_EX_LAYERED` turns that back into a
//!   hole -- but only for that one fill. Any other time the window is opaque,
//!   because a colour key is a trap: it would also punch a hole through a
//!   noise cell, or through a colour the user happened to pick, that landed
//!   on exactly the key value.
//! * The overlay *takes focus*, unlike the layer surface on Wayland, which
//!   holds an exclusive keyboard grab instead. Windows delivers keystrokes to
//!   the focused window and nowhere else, so an overlay that refused
//!   activation would never see the key that is supposed to dismiss it. With
//!   several screens covered only one of them can hold focus, so -- exactly
//!   as in the GTK build's fallback mode -- a key works on the focused one
//!   and a click works on any of them.

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, HBRUSH, InvalidateRect, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetWindowLongPtrW, HWND_TOPMOST, IDC_ARROW, KillTimer, LWA_COLORKEY, LoadCursorW,
    LWA_ALPHA, RegisterClassW, SWP_SHOWWINDOW, SetCursor, SetForegroundWindow,
    SetLayeredWindowAttributes,
    SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, SW_HIDE, WM_DESTROY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_PAINT, WM_SETCURSOR, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use whitescreen_core::{Fill, Rgba};

use crate::monitor::Monitor;
use crate::paint::{TRANSPARENT_KEY, colorref, paint};

/// How often the noise is re-rolled, in milliseconds. 20 Hz, as on the other
/// platform: fast enough to exercise a subpixel, far below the refresh rate.
const NOISE_PERIOD_MS: u32 = 50;

/// Timer id for the noise. Any non-zero value will do; it only has to be
/// unique within the window.
const NOISE_TIMER: usize = 1;

/// What an overlay reports back to whoever owns it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OverlayEvent {
    /// A key or a click asked for the overlays to go away. As on Wayland,
    /// *any* key does this and a shortcut combination does not: the overlay
    /// swallows everything it does not act on, so between eating a keystroke
    /// and reading it as "get me out of here", the second is the safer
    /// answer for a full-screen colour with no other keyboard function.
    Dismissed,
}

/// Per-window state, reached from the window procedure through
/// `GWLP_USERDATA`.
struct State {
    color: Rgba,
    fill: Fill,
    scale: f32,
    seed: u64,
    dismissed: bool,
}

/// A fullscreen colour on one monitor.
pub struct Overlay {
    hwnd: HWND,
    /// Kept so the window can be put back on the same screen after it has
    /// been hidden, and so the paint handler knows the scale.
    monitor: Monitor,
    state: *mut RefCell<State>,
}

impl Overlay {
    /// Create the window for `monitor`, hidden.
    pub fn new(monitor: &Monitor) -> windows::core::Result<Self> {
        register_class()?;

        let state = Box::into_raw(Box::new(RefCell::new(State {
            color: Rgba::WHITE,
            fill: Fill::default(),
            scale: monitor.scale,
            seed: whitescreen_core::NOISE_SEED,
            dismissed: false,
        })));

        let rect = monitor.rect();
        let hwnd = unsafe {
            CreateWindowExW(
                // Topmost so nothing covers the colour, and no taskbar
                // button, because an overlay is not a window anyone wants to
                // alt-tab to. It does still take focus; see above.
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                CLASS_NAME,
                w!("White Screen"),
                WS_POPUP,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                None,
                None,
                Some(instance()?),
                None,
            )?
        };

        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }

        let overlay = Self { hwnd, monitor: monitor.clone(), state };
        overlay.apply_transparency(Fill::default())?;
        Ok(overlay)
    }

    /// What this screen shows.
    pub fn set_color(&self, color: Rgba) {
        self.with_state(|s| s.color = color);
        self.redraw();
    }

    pub fn set_fill(&self, fill: Fill) {
        let was_noise = self.with_state(|s| {
            let was = matches!(s.fill, Fill::Noise);
            s.fill = fill;
            was
        });

        // The noise is the only fill that moves, so it is the only one that
        // needs a timer running.
        let is_noise = matches!(fill, Fill::Noise);
        if is_noise != was_noise {
            unsafe {
                if is_noise {
                    SetTimer(Some(self.hwnd), NOISE_TIMER, NOISE_PERIOD_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), NOISE_TIMER);
                }
            }
        }
        let _ = self.apply_transparency(fill);
        self.redraw();
    }

    /// Whether the uncovered half of a region should be a hole in the window.
    ///
    /// Only then: a colour key is in force for every pixel, so leaving it on
    /// would punch a hole through any other fill that happened to produce
    /// exactly the key colour -- one noise cell in sixteen million, or a
    /// colour the user picked.
    fn apply_transparency(&self, fill: Fill) -> windows::core::Result<()> {
        let hole = matches!(fill, Fill::Region { rest: None, .. });
        unsafe {
            if hole {
                SetLayeredWindowAttributes(
                    self.hwnd,
                    colorref(TRANSPARENT_KEY),
                    0,
                    LWA_COLORKEY,
                )
            } else {
                // Opaque, and no key colour in force.
                SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA)
            }
        }
    }

    /// Put it on screen, covering its monitor.
    ///
    /// The position is set on every show rather than once at creation: a
    /// screen can be moved or re-scaled in the display settings while the
    /// overlay is hidden.
    pub fn show(&mut self, monitor: &Monitor) -> windows::core::Result<()> {
        self.monitor = monitor.clone();
        self.with_state(|s| {
            s.scale = monitor.scale;
            s.dismissed = false;
        });

        let rect = monitor.rect();
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_SHOWWINDOW,
            )?;
            // Windows sends keystrokes to the focused window and nowhere
            // else, so an overlay that is meant to be dismissed by any key
            // has to be the one holding focus.
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
        }
        self.redraw();
        Ok(())
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Whether a key or a click has asked for the overlays to go away since
    /// this was last checked.
    pub fn take_event(&self) -> Option<OverlayEvent> {
        self.with_state(|s| {
            s.dismissed.then(|| {
                s.dismissed = false;
                OverlayEvent::Dismissed
            })
        })
    }

    /// The monitor this overlay is currently covering.
    pub fn monitor(&self) -> &Monitor {
        &self.monitor
    }

    fn redraw(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn with_state<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        // Safety: the pointer is created in new() and only freed in Drop,
        // and every caller is on the thread that owns the window.
        let cell = unsafe { &*self.state };
        let mut state = cell.borrow_mut();
        f(&mut state)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), NOISE_TIMER);
            // Clear the back-pointer first: DestroyWindow sends WM_DESTROY,
            // and the handler must not reach a state that is about to go.
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.state));
        }
    }
}

const CLASS_NAME: PCWSTR = w!("WhiteScreenOverlay");

fn instance() -> windows::core::Result<windows::Win32::Foundation::HINSTANCE> {
    Ok(unsafe { GetModuleHandleW(None)? }.into())
}

/// Register the window class, once per process.
fn register_class() -> windows::core::Result<()> {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    let mut result = Ok(());
    ONCE.call_once(|| {
        result = (|| {
            let class = WNDCLASSW {
                // Redraw the whole surface on a resize: every fill here is
                // derived from the width, so a partial repaint would leave
                // bands from the old size behind.
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wndproc),
                hInstance: instance()?,
                lpszClassName: CLASS_NAME,
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
                // No background brush: every pixel is painted by WM_PAINT,
                // and letting Windows erase first is a flash of white.
                hbrBackground: HBRUSH::default(),
                ..Default::default()
            };
            if unsafe { RegisterClassW(&raw const class) } == 0 {
                // Whatever Windows objected to is in the thread's last error.
                return Err(windows::core::Error::from_thread());
            }
            Ok(())
        })();
    });
    result
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<State>;
    if state.is_null() {
        // Before SetWindowLongPtrW, or after Drop cleared it.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &*state };

    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &raw mut ps) };
            {
                let s = state.borrow();
                paint(dc, ps.rcPaint, s.color, s.fill, s.scale, s.seed);
            }
            let _ = unsafe { EndPaint(hwnd, &raw const ps) };
            LRESULT(0)
        }
        WM_TIMER => {
            // Re-roll the noise. The seed is the only animation state there
            // is: the block's position is derived from it.
            //
            // One borrow, released before anything else: taking a shared and
            // a mutable borrow in a single statement would leave both alive
            // until the semicolon, and panic on the first tick.
            {
                let mut s = state.borrow_mut();
                s.seed = whitescreen_core::next_noise_seed(s.seed);
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if dismisses(wparam) {
                state.borrow_mut().dismissed = true;
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            state.borrow_mut().dismissed = true;
            LRESULT(0)
        }
        WM_SETCURSOR => {
            // No cursor in the frame: the point of the app is a clean field
            // of colour for a camera or a pixel check.
            unsafe {
                let _ = SetCursor(None);
            }
            LRESULT(1)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Whether this keystroke should take the overlays down.
///
/// The same rule as the GTK build's `dismisses_overlay()`: any key does,
/// except a modifier on its own and anything held with Ctrl, Alt or Windows
/// -- those are shortcuts, and swallowing them would make it impossible to
/// leave deliberately.
fn dismisses(key: WPARAM) -> bool {
    let vk = key.0 as u16;
    let held = |k| unsafe { GetKeyState(i32::from(k)) } < 0;

    let is_modifier = matches!(
        vk,
        // VK_SHIFT, VK_CONTROL, VK_MENU and the left/right pairs, the
        // Windows keys, and the locks.
        0x10..=0x12 | 0x14 | 0x5B | 0x5C | 0x90 | 0x91 | 0xA0..=0xA5
    );
    if is_modifier {
        return false;
    }
    !(held(VK_CONTROL.0) || held(VK_MENU.0) || held(VK_LWIN.0) || held(VK_RWIN.0))
}
