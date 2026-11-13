//! The control window.
//!
//! The Windows counterpart of the GTK build's window: the preview, the
//! colour chips, the key-light temperatures, brightness, the fill modes,
//! the cycle and the monitor list. Ordinary Win32 controls, with the
//! swatches owner-drawn through the same painter the overlays use, so a
//! colour looks the same in the chip as it does on the glass.
//!
//! The layout is computed rather than loaded from a dialog resource: every
//! length here is scaled by the window's DPI, and a resource template would
//! have had to be duplicated per scale factor.
//!
//! Nothing here decides anything about colour. Every control reads and
//! writes `Session`, which is shared with the command line, so the two
//! cannot disagree about what a button means.

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, HBRUSH, InvalidateRect,
    PAINTSTRUCT, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::UI::Controls::{
    BST_CHECKED, DRAWITEMSTRUCT, TBM_SETPOS, TBM_SETRANGE, TRACKBAR_CLASSW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_OWNERDRAW, BS_PUSHBUTTON,
    CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, CBN_SELCHANGE,
    CBS_DROPDOWNLIST,
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetClientRect, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MoveWindow, PostQuitMessage,
    RegisterClassW, SW_SHOW, SendMessageW, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
    WINDOW_EX_STYLE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_DRAWITEM, WM_HSCROLL, WM_PAINT,
    WM_DPICHANGED, WM_SIZE, WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use whitescreen_core::{Fill, Rgba, Side, kelvin};

use crate::paint::{colorref, paint};
use crate::session::Session;

/// `TBM_GETPOS`, which the generated bindings do not carry: it is plain
/// `WM_USER`, and the metadata only names the messages that take arguments.
const TBM_GETPOS: u32 = 0x0400;

/// Control ids. Grouped by block so that a range test can say which kind of
/// control a command came from.
const ID_CHIP_FIRST: usize = 100;
const ID_CHIP_LAST: usize = 109;
const ID_KELVIN_FIRST: usize = 110;
const ID_KELVIN_LAST: usize = 119;
const ID_SHOW: usize = 200;
const ID_HIDE: usize = 201;
const ID_IDENTIFY: usize = 202;
const ID_FILL_FIRST: usize = 210;
const ID_FILL_LAST: usize = 214;
const ID_CYCLE: usize = 220;
const ID_PER_MONITOR: usize = 221;
const ID_APPLIES_TO: usize = 230;
const ID_BRIGHTNESS: usize = 240;
const ID_MONITOR_FIRST: usize = 300;

/// The preset colours, in the order the GTK build shows them.
const CHIPS: &[(&str, Rgba)] = &[
    ("White", Rgba::new(255, 255, 255)),
    ("Black", Rgba::new(0, 0, 0)),
    ("Red", Rgba::new(255, 0, 0)),
    ("Green", Rgba::new(0, 255, 0)),
    ("Blue", Rgba::new(0, 0, 255)),
];

/// The fill buttons, and what each one means. `Flat` has no button: it is
/// all of them unchecked.
const FILLS: &[(&str, Fill)] = &[
    ("Region", Fill::Region { side: Side::Left, rest: None }),
    ("Exerciser", Fill::Noise),
    ("Wedge", Fill::Wedge),
    ("Grid", Fill::Grid),
];

const CLASS_NAME: PCWSTR = w!("WhiteScreenControl");

/// A control, and what it is for.
struct Control {
    hwnd: HWND,
    id: usize,
    /// Owner-drawn buttons paint a colour; the rest paint themselves.
    swatch: Option<Rgba>,
}

struct State {
    session: Session,
    controls: Vec<Control>,
    /// Which monitor the chips are aimed at, by key; `None` is all of them.
    target: Option<String>,
    scale: f32,
}

/// The control window.
pub struct ControlWindow {
    hwnd: HWND,
    state: *mut RefCell<State>,
}

impl ControlWindow {
    pub fn new(session: Session) -> windows::core::Result<Self> {
        // Before the first window exists, and harmless if it was already
        // set: without it Windows stretches the whole window on a scaled
        // display, which on a colour-measurement tool is the one thing that
        // must not happen to the preview.
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        register_class()?;

        let state = Box::into_raw(Box::new(RefCell::new(State {
            session,
            controls: Vec::new(),
            target: None,
            scale: 1.0,
        })));

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                w!("White Screen"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                940,
                720,
                None,
                None,
                Some(instance()?),
                Some(state.cast()),
            )?
        };

        let window = Self { hwnd, state };
        // 96 dpi is the unscaled baseline every length here is written in.
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        unsafe { &*state }.borrow_mut().scale = dpi as f32 / 96.0;
        window.build_controls()?;
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }
        Ok(window)
    }

    /// Run until the window is closed, driving the overlays as we go.
    pub fn run(&self) -> windows::core::Result<()> {
        while crate::pump() {
            {
                let cell = unsafe { &*self.state };
                let mut state = cell.borrow_mut();
                // The overlays need a pass of their own: the queue has to
                // advance and a hot-plug has to be noticed even while
                // nobody is touching this window.
                let was = state.session.is_showing();
                state.session.tick()?;
                if was && !state.session.is_showing() {
                    // Dismissed from an overlay; the window stays.
                    drop(state);
                    self.refresh();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }

    fn refresh(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    /// Create every control. Positions are set by `layout`, which runs on
    /// the first `WM_SIZE`.
    fn build_controls(&self) -> windows::core::Result<()> {
        let cell = unsafe { &*self.state };
        let instance = instance()?;

        let mut controls = Vec::new();
        let mut add = |class: PCWSTR,
                       text: PCWSTR,
                       style: u32,
                       id: usize,
                       swatch: Option<Rgba>|
         -> windows::core::Result<()> {
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    class,
                    text,
                    windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                        WS_CHILD.0 | WS_VISIBLE.0 | style,
                    ),
                    0,
                    0,
                    0,
                    0,
                    Some(self.hwnd),
                    Some(windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut _)),
                    Some(instance),
                    None,
                )?
            };
            controls.push(Control { hwnd, id, swatch });
            Ok(())
        };

        // The colour chips and the key-light temperatures: owner-drawn, so
        // they can be the swatch itself rather than a button with a label.
        for (i, (_, color)) in CHIPS.iter().enumerate() {
            add(w!("BUTTON"), w!(""), BS_OWNERDRAW as u32, ID_CHIP_FIRST + i, Some(*color))?;
        }
        add(w!("BUTTON"), w!("Custom…"), BS_PUSHBUTTON as u32, ID_CHIP_LAST, None)?;

        for (i, k) in kelvin::PRESETS.iter().enumerate() {
            add(
                w!("BUTTON"),
                w!(""),
                BS_OWNERDRAW as u32,
                ID_KELVIN_FIRST + i,
                Some(kelvin::rgba(*k)),
            )?;
        }

        add(w!("BUTTON"), w!("Show on selected"), BS_PUSHBUTTON as u32, ID_SHOW, None)?;
        add(w!("BUTTON"), w!("Hide ALL"), BS_PUSHBUTTON as u32, ID_HIDE, None)?;
        add(w!("BUTTON"), w!("Identify"), BS_AUTOCHECKBOX as u32, ID_IDENTIFY, None)?;

        for (i, (name, _)) in FILLS.iter().enumerate() {
            let text: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            add(
                w!("BUTTON"),
                PCWSTR(text.as_ptr()),
                BS_AUTOCHECKBOX as u32,
                ID_FILL_FIRST + i,
                None,
            )?;
            // The text must outlive the call, and CreateWindowExW copies it.
            drop(text);
        }

        add(w!("BUTTON"), w!("Cycle while the overlay is up"), BS_AUTOCHECKBOX as u32, ID_CYCLE, None)?;
        add(w!("BUTTON"), w!("Per-monitor colors"), BS_AUTOCHECKBOX as u32, ID_PER_MONITOR, None)?;
        add(w!("COMBOBOX"), w!(""), CBS_DROPDOWNLIST as u32, ID_APPLIES_TO, None)?;
        add(TRACKBAR_CLASSW, w!(""), 0, ID_BRIGHTNESS, None)?;

        {
            let mut state = cell.borrow_mut();
            state.controls = controls;
        }
        self.fill_monitor_list()?;
        self.sync_controls();
        Ok(())
    }

    /// One checkbox per attached monitor, plus its colour swatch.
    fn fill_monitor_list(&self) -> windows::core::Result<()> {
        let cell = unsafe { &*self.state };
        let instance = instance()?;
        let screens: Vec<_> = cell.borrow().session.screens().to_vec();

        let mut created = Vec::new();
        for (i, screen) in screens.iter().enumerate() {
            let label: Vec<u16> = format!("{}  {}×{}", screen.key, screen.width, screen.height)
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    PCWSTR(label.as_ptr()),
                    windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                        WS_CHILD.0 | WS_VISIBLE.0 | BS_AUTOCHECKBOX as u32,
                    ),
                    0,
                    0,
                    0,
                    0,
                    Some(self.hwnd),
                    Some(windows::Win32::UI::WindowsAndMessaging::HMENU(
                        (ID_MONITOR_FIRST + i) as *mut _,
                    )),
                    Some(instance),
                    None,
                )?
            };
            // Every screen starts ticked, as the GTK build ticks the first
            // one on a fresh profile.
            unsafe {
                SendMessageW(hwnd, BM_SETCHECK, Some(WPARAM(BST_CHECKED.0 as usize)), None);
            }
            created.push(Control { hwnd, id: ID_MONITOR_FIRST + i, swatch: None });
        }

        cell.borrow_mut().controls.extend(created);
        self.sync_applies_to();
        Ok(())
    }

    /// Rebuild "Applies to" from the monitors now attached.
    fn sync_applies_to(&self) {
        let cell = unsafe { &*self.state };
        let state = cell.borrow();
        let Some(combo) = state.controls.iter().find(|c| c.id == ID_APPLIES_TO) else { return };

        unsafe {
            SendMessageW(combo.hwnd, CB_RESETCONTENT, None, None);
        }
        let mut names = vec!["All monitors".to_string()];
        names.extend(state.session.screens().iter().map(|m| m.key.clone()));
        for name in &names {
            let text: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                SendMessageW(
                    combo.hwnd,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(text.as_ptr() as isize)),
                );
            }
        }
        let selected = state
            .target
            .as_ref()
            .and_then(|key| names.iter().position(|n| n == key))
            .unwrap_or(0);
        unsafe {
            SendMessageW(combo.hwnd, CB_SETCURSEL, Some(WPARAM(selected)), None);
        }
    }

    /// Put the controls in step with the session.
    fn sync_controls(&self) {
        let cell = unsafe { &*self.state };
        let state = cell.borrow();

        for control in &state.controls {
            let checked = match control.id {
                ID_IDENTIFY => state.session.identify(),
                ID_CYCLE => state.session.cycle_armed(),
                ID_PER_MONITOR => state.session.per_monitor_colors(),
                id if (ID_FILL_FIRST..=ID_FILL_LAST).contains(&id) => {
                    let which = id - ID_FILL_FIRST;
                    FILLS.get(which).is_some_and(|(_, f)| same_kind(*f, state.session.fill()))
                }
                ID_BRIGHTNESS => {
                    let percent = (state.session.brightness() * 100.0) as usize;
                    unsafe {
                        SendMessageW(
                            control.hwnd,
                            TBM_SETRANGE,
                            Some(WPARAM(1)),
                            Some(LPARAM(((100 << 16) | 5) as isize)),
                        );
                        SendMessageW(control.hwnd, TBM_SETPOS, Some(WPARAM(1)), Some(LPARAM(percent as isize)));
                    }
                    continue;
                }
                _ => continue,
            };
            unsafe {
                SendMessageW(
                    control.hwnd,
                    BM_SETCHECK,
                    Some(WPARAM(usize::from(checked))),
                    None,
                );
            }
        }
    }
}

/// Whether two fills are the same *kind*, ignoring their details: a region
/// is a region whichever half it covers.
fn same_kind(a: Fill, b: Fill) -> bool {
    matches!(
        (a, b),
        (Fill::Region { .. }, Fill::Region { .. })
            | (Fill::Noise, Fill::Noise)
            | (Fill::Wedge, Fill::Wedge)
            | (Fill::Grid, Fill::Grid)
    )
}

impl Drop for ControlWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            drop(Box::from_raw(self.state));
        }
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
                hCursor: unsafe { LoadCursorW(None, IDC_ARROW)? },
                hbrBackground: HBRUSH(((windows::Win32::Graphics::Gdi::COLOR_WINDOW.0 + 1) as isize) as *mut _),
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
    if msg == WM_CREATE {
        // The state pointer travels in CREATESTRUCT so that the very first
        // messages can already reach it.
        let create = lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW;
        let state = unsafe { (*create).lpCreateParams };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        }
        return LRESULT(0);
    }

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
                // The preview shows what a screen will, brightness and all.
                let color = s.session.shared_color().scaled(s.session.brightness());
                paint(dc, preview_rect(hwnd, s.scale), color, s.session.fill(), s.scale, 0);
            }
            let _ = unsafe { EndPaint(hwnd, &raw const ps) };
            LRESULT(0)
        }
        WM_SIZE => {
            layout(hwnd, state);
            LRESULT(0)
        }
        // Moved to a screen with a different scale: Windows offers a new
        // size, and every length has to be recomputed against the new dpi.
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as u32;
            state.borrow_mut().scale = dpi.max(96) as f32 / 96.0;
            let suggested = lparam.0 as *const RECT;
            let r = unsafe { *suggested };
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                        | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
                );
            }
            layout(hwnd, state);
            LRESULT(0)
        }
        WM_DRAWITEM => {
            let item = lparam.0 as *const DRAWITEMSTRUCT;
            draw_swatch(state, unsafe { &*item });
            LRESULT(1)
        }
        WM_HSCROLL => {
            // The only slider is brightness.
            let s = &mut *state.borrow_mut();
            if let Some(control) = s.controls.iter().find(|c| c.id == ID_BRIGHTNESS) {
                let pos = unsafe { SendMessageW(control.hwnd, TBM_GETPOS, None, None) }.0;
                s.session.set_brightness(pos as f32 / 100.0);
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let notification = (wparam.0 >> 16) as u32;
            let id = wparam.0 & 0xFFFF;
            if notification == BN_CLICKED {
                command(hwnd, state, id);
            } else if notification == CBN_SELCHANGE && id == ID_APPLIES_TO {
                aim(hwnd, state);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Where the preview sits: the top of the window, full width.
fn preview_rect(hwnd: HWND, scale: f32) -> RECT {
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut client) };
    let pad = (16.0 * scale) as i32;
    RECT {
        left: client.left + pad,
        top: client.top + pad,
        right: client.right - pad,
        bottom: client.top + (220.0 * scale) as i32,
    }
}

/// Place every control. Plain arithmetic rather than a layout engine: the
/// window has one column of rows and the rows are a fixed height.
fn layout(hwnd: HWND, state: &RefCell<State>) {
    let s = state.borrow();
    let scale = s.scale;
    let px = |v: f32| (v * scale) as i32;

    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut client) };

    let pad = px(16.0);
    let row_h = px(30.0);
    let gap = px(8.0);
    let sidebar = px(260.0);

    // The monitor rows run down the left.
    let mut y = preview_rect(hwnd, scale).bottom + gap;
    let left = pad;
    for control in s.controls.iter().filter(|c| c.id >= ID_MONITOR_FIRST) {
        let _ = unsafe { MoveWindow(control.hwnd, left, y, sidebar, row_h, true) };
        y += row_h + gap / 2;
    }
    let per_monitor_y = y;

    // Everything else runs down the right.
    let right = pad + sidebar + pad;
    let width = (client.right - right - pad).max(px(200.0));
    let mut y = preview_rect(hwnd, scale).bottom + gap;

    let chip = px(56.0);
    for control in s.controls.iter() {
        let (x, w, h) = match control.id {
            id if (ID_CHIP_FIRST..ID_CHIP_LAST).contains(&id) => {
                (right + (id - ID_CHIP_FIRST) as i32 * (chip + gap), chip, px(40.0))
            }
            ID_CHIP_LAST => (right + 5 * (chip + gap), px(80.0), px(40.0)),
            id if (ID_KELVIN_FIRST..=ID_KELVIN_LAST).contains(&id) => (
                right + (id - ID_KELVIN_FIRST) as i32 * (chip + gap),
                chip,
                px(28.0),
            ),
            _ => continue,
        };
        let row = if control.id >= ID_KELVIN_FIRST { y + px(48.0) } else { y };
        let _ = unsafe { MoveWindow(control.hwnd, x, row, w, h, true) };
    }
    y += px(96.0);

    let place = |id: usize, w: i32, h: i32, x: i32, row: i32| {
        if let Some(control) = s.controls.iter().find(|c| c.id == id) {
            let _ = unsafe { MoveWindow(control.hwnd, x, row, w, h, true) };
        }
    };

    place(ID_APPLIES_TO, px(200.0), px(200.0), right, y);
    place(ID_BRIGHTNESS, width - px(220.0), row_h, right + px(220.0), y);
    y += row_h + gap;

    place(ID_SHOW, px(160.0), row_h, right, y);
    place(ID_HIDE, px(110.0), row_h, right + px(170.0), y);
    place(ID_IDENTIFY, px(110.0), row_h, right + px(290.0), y);
    y += row_h + gap;

    for (i, _) in FILLS.iter().enumerate() {
        place(ID_FILL_FIRST + i, px(100.0), row_h, right + i as i32 * px(108.0), y);
    }
    y += row_h + gap;

    place(ID_CYCLE, px(240.0), row_h, right, y);
    place(ID_PER_MONITOR, sidebar, row_h, pad, per_monitor_y);
}

/// Paint one owner-drawn swatch.
fn draw_swatch(state: &RefCell<State>, item: &DRAWITEMSTRUCT) {
    let s = state.borrow();
    let Some(control) = s.controls.iter().find(|c| c.hwnd == item.hwndItem) else { return };
    let Some(color) = control.swatch else { return };

    unsafe {
        let brush = CreateSolidBrush(colorref(color));
        FillRect(item.hDC, &raw const item.rcItem, brush);
        let _ = DeleteObject(brush.into());

        // A hairline that contrasts with the fill, so a white chip does not
        // vanish into a light window -- the same rule as the GTK swatches.
        let edge = if color.luma() > 0.5 { Rgba::new(0, 0, 0) } else { Rgba::new(255, 255, 255) };
        let pen = CreateSolidBrush(COLORREF(colorref(edge).0));
        let border = RECT { ..item.rcItem };
        FillRect(item.hDC, &raw const border, pen);
        let inner = RECT {
            left: border.left + 1,
            top: border.top + 1,
            right: border.right - 1,
            bottom: border.bottom - 1,
        };
        let fill = CreateSolidBrush(colorref(color));
        FillRect(item.hDC, &raw const inner, fill);
        let _ = DeleteObject(pen.into());
        let _ = DeleteObject(fill.into());
    }
}

/// A control was clicked.
fn command(hwnd: HWND, state: &RefCell<State>, id: usize) {
    {
        let s = &mut *state.borrow_mut();
        match id {
            id if (ID_CHIP_FIRST..ID_CHIP_LAST).contains(&id) => {
                if let Some((_, color)) = CHIPS.get(id - ID_CHIP_FIRST) {
                    apply_color(s, *color);
                }
            }
            id if (ID_KELVIN_FIRST..=ID_KELVIN_LAST).contains(&id) => {
                if let Some(k) = kelvin::PRESETS.get(id - ID_KELVIN_FIRST) {
                    apply_color(s, kelvin::rgba(*k));
                }
            }
            ID_CHIP_LAST => {
                // Windows' own colour dialog: the one a person already
                // knows, and the counterpart of GtkColorDialog.
                let current = match s.target.clone() {
                    Some(key) => s.session.monitor_color(&key),
                    None => Some(s.session.shared_color()),
                }
                .unwrap_or(s.session.shared_color());
                if let Some(picked) = choose_color(hwnd, current) {
                    apply_color(s, picked);
                }
            }
            ID_SHOW => {
                let _ = s.session.show();
            }
            ID_HIDE => s.session.hide(),
            ID_IDENTIFY => {
                let on = !s.session.identify();
                let _ = s.session.set_identify(on);
            }
            ID_CYCLE => {
                let armed = !s.session.cycle_armed();
                s.session.set_cycle_armed(armed);
            }
            ID_PER_MONITOR => {
                let on = !s.session.per_monitor_colors();
                s.session.set_per_monitor_colors(on);
            }
            id if (ID_FILL_FIRST..=ID_FILL_LAST).contains(&id) => {
                let which = id - ID_FILL_FIRST;
                if let Some((_, fill)) = FILLS.get(which) {
                    // Clicking the mode that is already on turns it off,
                    // which is the plain colour.
                    let next =
                        if same_kind(*fill, s.session.fill()) { Fill::default() } else { *fill };
                    s.session.set_fill(next);
                }
            }
            id if id >= ID_MONITOR_FIRST => {
                let keys = ticked_monitors(s);
                let _ = s.session.set_selection(keys);
            }
            _ => {}
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// Ask for a colour with the system picker.
///
/// `None` when the user cancelled, which must leave everything as it was.
fn choose_color(owner: HWND, current: Rgba) -> Option<Rgba> {
    // The dialog keeps the sixteen custom slots in whatever the caller
    // hands it, so they live for the process rather than the call.
    static CUSTOM: std::sync::Mutex<[u32; 16]> = std::sync::Mutex::new([0x00FF_FFFF; 16]);
    let mut custom = CUSTOM.lock().ok()?;
    let mut slots: [COLORREF; 16] = std::array::from_fn(|i| COLORREF(custom[i]));

    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: colorref(current),
        lpCustColors: slots.as_mut_ptr(),
        // Open on the full picker: the basic grid cannot express most of
        // what this app is for.
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    let chosen = unsafe { ChooseColorW(&raw mut cc) }.as_bool();
    for (slot, value) in custom.iter_mut().zip(slots.iter()) {
        *slot = value.0;
    }
    if !chosen {
        return None;
    }
    // COLORREF is 0x00BBGGRR.
    let v = cc.rgbResult.0;
    Some(Rgba::new((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8))
}

/// Put a colour where the card is aimed: one monitor, or all of them.
fn apply_color(s: &mut State, color: Rgba) {
    match s.target.clone() {
        Some(key) => s.session.set_monitor_color(&key, color),
        None => s.session.set_shared_color(color),
    }
}

/// Point the chips at whichever monitor "Applies to" now names.
///
/// Row zero is all of them; the rest are the screens in the order the
/// sidebar lists them.
fn aim(hwnd: HWND, state: &RefCell<State>) {
    {
        let s = &mut *state.borrow_mut();
        let Some(combo) = s.controls.iter().find(|c| c.id == ID_APPLIES_TO) else { return };
        let row = unsafe { SendMessageW(combo.hwnd, CB_GETCURSEL, None, None) }.0;
        s.target = if row <= 0 {
            None
        } else {
            s.session.screens().get((row - 1) as usize).map(|m| m.key.clone())
        };
    }
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

/// The keys of the monitors whose boxes are ticked.
fn ticked_monitors(s: &State) -> Vec<String> {
    let screens: Vec<String> = s.session.screens().iter().map(|m| m.key.clone()).collect();
    let mut keys = Vec::new();
    for control in s.controls.iter().filter(|c| c.id >= ID_MONITOR_FIRST) {
        let checked =
            unsafe { SendMessageW(control.hwnd, BM_GETCHECK, None, None) }.0 == BST_CHECKED.0 as isize;
        if checked
            && let Some(key) = screens.get(control.id - ID_MONITOR_FIRST)
        {
            keys.push(key.clone());
        }
    }
    keys
}

/// Not yet wired: the window title is set once and never changes.
#[allow(dead_code)]
fn set_title(hwnd: HWND, title: &str) {
    let text: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let _ = unsafe { SetWindowTextW(hwnd, PCWSTR(text.as_ptr())) };
}
