//! The Identify badge: a monitor's own name, on that monitor.
//!
//! The counterpart of the GTK build's `MonitorLabel`, and it exists for the
//! same reason: once several screens are covered in colour there is nothing
//! left to tell them apart, so the name has to appear on the glass itself.
//!
//! Click-through, so a badge never intercepts a click meant for the overlay
//! underneath it, and anchored to the bottom-right corner with the same
//! margin the GTK build uses.

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DeleteObject,
    DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FillRect, GetTextExtentPoint32W, HDC, HFONT,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    DT_LEFT, DT_SINGLELINE, DT_TOP,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HWND_TOPMOST,
    RegisterClassW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, WM_DESTROY, WM_PAINT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use whitescreen_core::Rgba;

use crate::monitor::Monitor;
use crate::paint::colorref;

/// Distance from the corner of the screen, matching the GTK build.
const MARGIN: i32 = 24;
/// Padding inside the badge.
const PAD: i32 = 16;
/// Point sizes at 96 dpi, as in the GTK build's 56px title and 15px
/// subtitle.
const TITLE_PT: i32 = 42;
const SUBTITLE_PT: i32 = 11;

const BACKGROUND: Rgba = Rgba::new(32, 32, 32);
const FOREGROUND: Rgba = Rgba::new(255, 255, 255);
const DIM: Rgba = Rgba::new(170, 170, 170);

const CLASS_NAME: PCWSTR = w!("WhiteScreenBadge");

struct State {
    title: Vec<u16>,
    subtitle: Vec<u16>,
    scale: f32,
}

/// One monitor's name, shown on that monitor.
pub struct Badge {
    hwnd: HWND,
    state: *mut RefCell<State>,
}

impl Badge {
    /// Build the badge for `monitor`, hidden.
    pub fn new(monitor: &Monitor) -> windows::core::Result<Self> {
        register_class()?;

        let state = Box::into_raw(Box::new(RefCell::new(State {
            title: wide(&monitor.key),
            subtitle: wide(&monitor.name),
            scale: monitor.scale,
        })));

        let hwnd = unsafe {
            CreateWindowExW(
                // Click-through and never activated: a badge must not take
                // the focus away from the overlay that is listening for the
                // key that dismisses everything.
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                CLASS_NAME,
                w!("White Screen"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance()?),
                None,
            )?
        };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
        Ok(Self { hwnd, state })
    }

    /// Place it in the monitor's bottom-right corner and show it.
    ///
    /// Sized to its text every time rather than once: a monitor can be
    /// renamed by a driver update, and the badge is rebuilt on a hot-plug.
    pub fn show(&self, monitor: &Monitor) -> windows::core::Result<()> {
        self.with_state(|s| {
            s.title = wide(&monitor.key);
            s.subtitle = wide(&monitor.name);
            s.scale = monitor.scale;
        });

        let (w, h) = self.measure(monitor.scale);
        let margin = (MARGIN as f32 * monitor.scale) as i32;
        let x = monitor.x + monitor.width - w - margin;
        let y = monitor.y + monitor.height - h - margin;
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            )?;
        }
        Ok(())
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// How large the badge has to be to hold its text.
    fn measure(&self, scale: f32) -> (i32, i32) {
        let dc = unsafe { windows::Win32::Graphics::Gdi::GetDC(None) };
        let (title, subtitle) = self.with_state(|s| (s.title.clone(), s.subtitle.clone()));

        let mut width = 0;
        let mut height = 0;
        for (text, pt) in [(&title, TITLE_PT), (&subtitle, SUBTITLE_PT)] {
            if text.len() <= 1 {
                continue;
            }
            let font = font(pt, scale);
            let old = unsafe { SelectObject(dc, font.into()) };
            let mut size = SIZE::default();
            // The trailing NUL is not part of the text.
            let _ = unsafe { GetTextExtentPoint32W(dc, &text[..text.len() - 1], &raw mut size) };
            unsafe {
                SelectObject(dc, old);
                let _ = DeleteObject(font.into());
            }
            width = width.max(size.cx);
            height += size.cy;
        }
        unsafe {
            windows::Win32::Graphics::Gdi::ReleaseDC(None, dc);
        }

        let pad = (PAD as f32 * scale) as i32;
        (width + pad * 2, height + pad * 2)
    }

    fn with_state<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        // Safety: created in new(), freed only in Drop, and every caller is
        // on the thread that owns the window.
        let cell = unsafe { &*self.state };
        let mut state = cell.borrow_mut();
        f(&mut state)
    }
}

impl Drop for Badge {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.state));
        }
    }
}

/// A NUL-terminated wide string, as every Win32 text call wants.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn font(points: i32, scale: f32) -> HFONT {
    // Negative height asks for a character height rather than a cell height,
    // which is what a point size means.
    let height = -((points as f32 * scale * 96.0 / 72.0) as i32);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            Default::default(),
            Default::default(),
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI"),
        )
    }
}

fn instance() -> windows::core::Result<windows::Win32::Foundation::HINSTANCE> {
    Ok(unsafe { GetModuleHandleW(None)? }.into())
}

fn register_class() -> windows::core::Result<()> {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    let mut result = Ok(());
    ONCE.call_once(|| {
        result = (|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance()?,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            if unsafe { RegisterClassW(&raw const class) } == 0 {
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
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let state = unsafe { &*state };

    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &raw mut ps) };
            {
                let s = state.borrow();
                draw(dc, ps.rcPaint, &s);
            }
            let _ = unsafe { EndPaint(hwnd, &raw const ps) };
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn draw(dc: HDC, bounds: RECT, state: &State) {
    unsafe {
        let brush = CreateSolidBrush(colorref(BACKGROUND));
        FillRect(dc, &raw const bounds, brush);
        let _ = DeleteObject(brush.into());

        SetBkMode(dc, TRANSPARENT);

        let pad = (PAD as f32 * state.scale) as i32;
        let mut y = bounds.top + pad;

        for (text, pt, colour) in [
            (&state.title, TITLE_PT, FOREGROUND),
            (&state.subtitle, SUBTITLE_PT, DIM),
        ] {
            if text.len() <= 1 {
                continue;
            }
            let f = font(pt, state.scale);
            let old = SelectObject(dc, f.into());
            SetTextColor(dc, COLORREF(colorref(colour).0));

            let mut rect = RECT {
                left: bounds.left + pad,
                top: y,
                right: bounds.right - pad,
                bottom: bounds.bottom - pad,
            };
            let mut owned = text.clone();
            let drawn = DrawTextW(
                dc,
                &mut owned[..text.len() - 1],
                &raw mut rect,
                DT_LEFT | DT_TOP | DT_SINGLELINE,
            );
            y += drawn;

            SelectObject(dc, old);
            let _ = DeleteObject(f.into());
        }
    }
}
