//! The control window.
//!
//! The Windows counterpart of the GTK build's window: the preview, the
//! colour chips, the key-light temperatures, brightness, the fill modes,
//! the cycle and the monitor list.
//!
//! Three files: this one is the window itself -- its state, its controls,
//! and what every message means -- while `layout` works out where things go
//! and `draw` paints them. The reasoning for owning the pixels is in `draw`,
//! and for computing the geometry rather than loading a dialog resource in
//! `layout`.
//!
//! The controls are real `BUTTON` and `COMBOBOX` windows, so tab, space, the
//! focus ring and the screen reader all work the way Windows users expect.
//! The brightness slider is a real trackbar; only its pixels are ours.
//!
//! Nothing here decides anything about colour. Every control reads and
//! writes `Session`, which is shared with the command line, so the two
//! cannot disagree about what a button means.

mod draw;
mod layout;

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, HBRUSH, HDC, InvalidateRect, PAINTSTRUCT,
    RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow, SetBkMode, SetTextColor, TRANSPARENT,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, MEASUREITEMSTRUCT, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, SetWindowTheme,
    TBM_SETPOS, TBM_SETRANGE, TBS_NOTICKS, TBS_TRANSPARENTBKGND, TRACKBAR_CLASSW, WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_OWNERDRAW, CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL,
    CB_SETITEMHEIGHT, CBN_SELCHANGE, CBS_DROPDOWNLIST, CBS_HASSTRINGS, CBS_OWNERDRAWFIXED,
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, GetWindowRect, HMENU, IDC_ARROW, IDCANCEL, IDOK, LoadCursorW, MINMAXINFO,
    PostQuitMessage, RegisterClassW, SPI_SETHIGHCONTRAST, SPI_SETNONCLIENTMETRICS, SW_SHOW,
    SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_COMMAND, WM_CREATE, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC,
    WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_DRAWITEM, WM_ERASEBKGND, WM_GETMINMAXINFO,
    WM_HSCROLL, WM_MEASUREITEM, WM_MOUSEMOVE, WM_NCDESTROY, WM_NOTIFY, WM_PAINT, WM_PRINTCLIENT,
    WM_SETFONT, WM_SETTINGCHANGE, WM_SIZE, WM_THEMECHANGED, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use whitescreen_core::{Fill, Pattern, Rgba, Side, kelvin, listing, options};

use crate::icon::Icons;
use crate::paint::colorref;
use crate::session::Session;
use crate::theme::{Fonts, Palette, apply_window_chrome};
use draw::{draw_item, draw_slider, paint_background, paint_foreground};
use layout::{Chrome, Role, metric};

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

/// The slider's range, in whole percent. The bounds are the core's, where
/// `--brightness` reads them too: a slider that went somewhere the command
/// line refused to go would be two different apps.
const BRIGHTNESS_MIN: i32 = options::BRIGHTNESS_MIN_PERCENT as i32;

const BRIGHTNESS_MAX: i32 = options::BRIGHTNESS_MAX_PERCENT as i32;

/// The preset colours, in the order the GTK build shows them.
const CHIPS: &[(&str, Rgba)] = &[
    ("White", Rgba::new(255, 255, 255)),
    ("Black", Rgba::new(0, 0, 0)),
    ("Red", Rgba::new(255, 0, 0)),
    ("Green", Rgba::new(0, 255, 0)),
    ("Blue", Rgba::new(0, 0, 255)),
];

/// The fill buttons, and the mode each one stands for. `Flat` has no button
/// of its own: it is all of them unchecked.
const FILLS: &[(&str, Pattern)] = &[
    ("Region", Pattern::Region),
    ("Exerciser", Pattern::Exerciser),
    ("Wedge", Pattern::Wedge),
    ("Grid", Pattern::Grid),
];

const CLASS_NAME: PCWSTR = w!("WhiteScreenControl");

/// What a control is, which decides how it draws and what a click means.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A colour chip: the swatch is the whole control.
    Swatch(Rgba),
    /// A command button. The accented one is the window's default action.
    Push { accent: bool },
    /// A checkbox. Its state is the session's, not the control's.
    Check,
    /// A monitor row: a checkbox carrying that screen's colour.
    Monitor,
    /// The one drop-down.
    Combo,
    /// The brightness trackbar.
    Slider,
}

/// A control, and what it is for.
struct Control {
    hwnd: HWND,
    id: usize,
    kind: Kind,
    /// Whether it sits on a card or straight on the page, which is the
    /// colour it has to clear itself to.
    on_card: bool,
    /// Kept as well as set on the window, because an owner-drawn control
    /// has to draw its own label.
    label: String,
    /// The second line, where there is one: a monitor row's connector and
    /// mode under its name.
    detail: String,
    /// Whether the pointer is over it. Maintained by `hover_proc`.
    hot: bool,
}

struct State {
    session: Session,
    controls: Vec<Control>,
    /// Which monitor the chips are aimed at, by key; `None` is all of them.
    target: Option<String>,
    scale: f32,
    dpi: u32,
    palette: Palette,
    fonts: Fonts,
    chrome: Chrome,
    /// The client height the layout actually needs, so the window cannot be
    /// dragged small enough to hide the commands at its foot.
    natural: i32,
    /// Handed to the themed children that ask what they are sitting on.
    card_brush: HBRUSH,
}

impl State {
    /// Logical pixels to physical ones.
    fn px(&self, value: f32) -> i32 {
        (value * self.scale).round() as i32
    }

    fn control(&self, id: usize) -> Option<&Control> {
        self.controls.iter().find(|c| c.id == id)
    }

    /// Whether a checkbox is ticked, which is only ever the session's word.
    fn checked(&self, id: usize) -> bool {
        match id {
            ID_IDENTIFY => self.session.identify(),
            ID_CYCLE => self.session.cycle_armed(),
            ID_PER_MONITOR => self.session.per_monitor_colors(),
            id if (ID_FILL_FIRST..=ID_FILL_LAST).contains(&id) => FILLS
                .get(id - ID_FILL_FIRST)
                .is_some_and(|(_, pattern)| *pattern == self.session.fill().pattern()),
            id if id >= ID_MONITOR_FIRST => {
                self.monitor_key(id).is_some_and(|key| selected(self.session.selection(), &key))
            }
            _ => false,
        }
    }

    fn monitor_key(&self, id: usize) -> Option<String> {
        self.session.screens().get(id - ID_MONITOR_FIRST).map(|m| m.key.clone())
    }

    /// The colour a monitor row shows: its own when per-monitor colours are
    /// on, otherwise the shared one, and brightness applied either way so
    /// the swatch matches what the screen will do.
    fn monitor_swatch(&self, id: usize) -> Rgba {
        let shared = self.session.shared_color();
        let color = match self.monitor_key(id) {
            Some(key) if self.session.per_monitor_colors() => {
                self.session.monitor_color(&key).unwrap_or(shared)
            }
            _ => shared,
        };
        color.scaled(self.session.brightness())
    }

    /// The shape of the screen the preview stands for: whichever one
    /// "Applies to" names, or the first attached when it names all of them.
    ///
    /// 16:9 when there is nothing to ask -- no screen is a shape at all,
    /// and a preview has to be some shape.
    fn aimed_aspect(&self) -> (f32, f32) {
        let screens = self.session.screens();
        let screen = match &self.target {
            Some(key) => screens.iter().find(|m| m.key == *key),
            None => screens.first(),
        };
        match screen {
            Some(m) if m.width > 0 && m.height > 0 => (m.width as f32, m.height as f32),
            _ => (16.0, 9.0),
        }
    }

    /// The colour the chips are currently aimed at, so the matching chip can
    /// show itself as the selected one.
    fn aimed_color(&self) -> Rgba {
        match &self.target {
            Some(key) => self.session.monitor_color(key).unwrap_or(self.session.shared_color()),
            None => self.session.shared_color(),
        }
    }
}

/// Whether a monitor is covered by the current selection.
///
/// An empty selection is every screen, which is what the command line means
/// by no `--monitor` at all.
fn selected(selection: &[String], key: &str) -> bool {
    selection.is_empty() || selection.iter().any(|k| k == key)
}

/// The control window.
pub struct ControlWindow {
    hwnd: HWND,
    state: *mut RefCell<State>,
    /// Held so the icons outlive the window that shows them.
    _icons: Icons,
}

impl ControlWindow {
    pub fn new(session: Session) -> windows::core::Result<Self> {
        // Before the first window exists, and harmless if it was already
        // set -- the manifest says the same thing, and gets there first.
        // Without it Windows stretches the whole window on a scaled
        // display, which on a colour-measurement tool is the one thing that
        // must not happen to the preview.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        // Registers the v6 classes the manifest asked for; the trackbar is
        // not available until this has run.
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_BAR_CLASSES,
        };
        let _ = unsafe { InitCommonControlsEx(&raw const controls) };
        register_class()?;

        let palette = Palette::current();
        let state = Box::into_raw(Box::new(RefCell::new(State {
            session,
            controls: Vec::new(),
            target: None,
            scale: 1.0,
            dpi: 96,
            fonts: Fonts::for_dpi(96),
            card_brush: unsafe { CreateSolidBrush(colorref(palette.card)) },
            palette,
            chrome: Chrome::default(),
            natural: 0,
        })));

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                w!("White Screen"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                980,
                900,
                None,
                None,
                Some(instance()?),
                Some(state.cast()),
            )?
        };

        let icons = Icons::attach(hwnd, palette.accent);
        let window = Self { hwnd, state, _icons: icons };
        window.build_controls()?;
        // Fonts, palette, chrome and layout all depend on the DPI of the
        // monitor the window actually landed on.
        reload(hwnd, unsafe { &*state }, Some(unsafe { GetDpiForWindow(hwnd) }.max(96)));
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }
        Ok(window)
    }

    /// Run until the window is closed, driving the overlays as we go.
    pub fn run(&self) -> windows::core::Result<()> {
        // The dialog-aware pump: tab, shift-tab and the arrow keys move
        // between controls, which is what makes this a Windows app rather
        // than a window with buttons on it.
        while crate::pump_dialog(self.hwnd) {
            let cell = unsafe { &*self.state };
            let pending = {
                let mut state = cell.borrow_mut();
                // The overlays need a pass of their own: the queue has to
                // advance and a hot-plug has to be noticed even while
                // nobody is touching this window.
                let was = state.session.is_showing();
                state.session.tick()?;
                let dismissed = was && !state.session.is_showing();
                let pending = state.session.next_tick();
                drop(state);
                if dismissed {
                    // Dismissed from an overlay; the window stays.
                    self.refresh();
                }
                pending
            };
            // Nothing to do until the queue's next step, or until the user
            // does something.
            crate::wait(pending);
        }

        // Closing the window is this build's "goodbye": whatever was chosen
        // in it has to outlive it, or the settings file would only ever
        // record what the command line did.
        unsafe { &*self.state }.borrow_mut().session.save();
        Ok(())
    }

    fn refresh(&self) {
        unsafe {
            let _ = RedrawWindow(
                Some(self.hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }

    /// Create every control. Positions are set by `layout`.
    fn build_controls(&self) -> windows::core::Result<()> {
        let cell = unsafe { &*self.state };
        let mut controls = Vec::new();

        for (i, (name, color)) in CHIPS.iter().enumerate() {
            controls.push(self.spawn(name, ID_CHIP_FIRST + i, Kind::Swatch(*color))?);
        }
        controls.push(self.spawn("Custom…", ID_CHIP_LAST, Kind::Push { accent: false })?);

        for (i, k) in kelvin::PRESETS.iter().enumerate() {
            let label = format!("{k} K");
            controls.push(self.spawn(
                &label,
                ID_KELVIN_FIRST + i,
                Kind::Swatch(kelvin::rgba(*k)),
            )?);
        }

        controls.push(self.spawn("Show on selected", ID_SHOW, Kind::Push { accent: true })?);
        controls.push(self.spawn("Hide all", ID_HIDE, Kind::Push { accent: false })?);
        controls.push(self.spawn("Identify screens", ID_IDENTIFY, Kind::Check)?);

        for (i, (name, _)) in FILLS.iter().enumerate() {
            controls.push(self.spawn(name, ID_FILL_FIRST + i, Kind::Check)?);
        }

        controls.push(self.spawn("Cycle while an overlay is up", ID_CYCLE, Kind::Check)?);
        controls.push(self.spawn("Per-monitor colors", ID_PER_MONITOR, Kind::Check)?);
        controls.push(self.spawn("", ID_APPLIES_TO, Kind::Combo)?);
        controls.push(self.spawn("Brightness", ID_BRIGHTNESS, Kind::Slider)?);

        cell.borrow_mut().controls = controls;
        fill_monitor_list(self.hwnd, cell)?;
        self.sync_controls();
        Ok(())
    }

    /// Create one control of the given kind.
    fn spawn(&self, label: &str, id: usize, kind: Kind) -> windows::core::Result<Control> {
        spawn(self.hwnd, self.state as usize, label, id, kind)
    }

    /// Put the controls that keep state of their own in step with the
    /// session. Everything else reads the session as it draws.
    fn sync_controls(&self) {
        let cell = unsafe { &*self.state };
        let state = cell.borrow();
        if let Some(slider) = state.control(ID_BRIGHTNESS) {
            let percent = (state.session.brightness() * 100.0).round() as i32;
            unsafe {
                SendMessageW(
                    slider.hwnd,
                    TBM_SETRANGE,
                    Some(WPARAM(1)),
                    Some(LPARAM(((BRIGHTNESS_MAX << 16) | BRIGHTNESS_MIN) as isize)),
                );
                SendMessageW(
                    slider.hwnd,
                    TBM_SETPOS,
                    Some(WPARAM(1)),
                    Some(LPARAM(percent.clamp(BRIGHTNESS_MIN, BRIGHTNESS_MAX) as isize)),
                );
            }
        }
    }
}

/// Create one control, parented to `parent`.
///
/// Free rather than a method because the monitor rows are made twice: once
/// at startup, and again whenever a screen is plugged in or unplugged, which
/// arrives as a message and so has nothing but the window handle to work
/// from.
fn spawn(
    parent: HWND,
    state: usize,
    label: &str,
    id: usize,
    kind: Kind,
) -> windows::core::Result<Control> {
    let (class, style) = match kind {
        Kind::Combo => (
            PCWSTR(w!("COMBOBOX").as_ptr()),
            // CBS_HASSTRINGS is not optional with an owner-drawn combo box:
            // without it the control keeps no strings at all and
            // CB_ADDSTRING's argument is held as opaque item data, so asking
            // for the text back hands you a pointer value rendered as
            // characters.
            CBS_DROPDOWNLIST as u32
                | CBS_OWNERDRAWFIXED as u32
                | CBS_HASSTRINGS as u32
                | WS_VSCROLL.0,
        ),
        Kind::Slider => (
            TRACKBAR_CLASSW,
            // No tick marks, and the parent paints the background, so
            // the trackbar sits on the card rather than on grey.
            TBS_NOTICKS | TBS_TRANSPARENTBKGND,
        ),
        _ => (PCWSTR(w!("BUTTON").as_ptr()), BS_OWNERDRAW as u32),
    };

    // The window text is set even on the owner-drawn controls, which do
    // not use it to paint: it is what a screen reader reads out.
    let wide: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR(wide.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | style),
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as *mut _)),
            Some(instance()?),
            None,
        )?
    };

    // Hot-tracking: a Win32 button does not report it for an
    // owner-drawn face, so each one watches its own pointer.
    if !matches!(kind, Kind::Combo | Kind::Slider) {
        let _ = unsafe { SetWindowSubclass(hwnd, Some(hover_proc), id, state) };
    }
    // Everything lives in a card except the commands at the foot.
    let on_card = !matches!(id, ID_SHOW | ID_HIDE);
    Ok(Control {
        hwnd,
        id,
        kind,
        on_card,
        label: label.to_string(),
        detail: String::new(),
        hot: false,
    })
}

/// One row per attached monitor, and the drop-down to match.
fn fill_monitor_list(hwnd: HWND, cell: &RefCell<State>) -> windows::core::Result<()> {
    let screens: Vec<_> = cell.borrow().session.screens().to_vec();
    let labels = screen_labels(&screens);

    let mut created = Vec::new();
    for (i, (screen, label)) in screens.iter().zip(&labels).enumerate() {
        // The window text carries both lines, because that is what a screen
        // reader reads out.
        let detail = screen.detail();
        let mut control = spawn(
            hwnd,
            cell as *const _ as usize,
            &format!("{label} — {detail}"),
            ID_MONITOR_FIRST + i,
            Kind::Monitor,
        )?;
        control.label = label.clone();
        control.detail = detail;
        created.push(control);
    }

    // The font comes with the reload that follows; this only has to get the
    // rows into the list.
    cell.borrow_mut().controls.extend(created);
    sync_applies_to(cell);
    Ok(())
}

/// What to call each screen in the window.
///
/// The name the monitor reports, made unique: two of the same model report
/// the same name, and a list offering "DELL U2415" twice is one nobody can
/// choose from. The *key* is untouched -- it is what the settings file and
/// `--monitor` use, and it is already unique.
fn screen_labels(screens: &[crate::Monitor]) -> Vec<String> {
    listing::unique(screens.iter().map(|m| m.label().to_string()))
}

/// Rebuild the window's idea of what is plugged in.
///
/// `WM_DISPLAYCHANGE` is the only warning a window gets, and the rows carry
/// each screen's own colour, so a stale list is not merely out of date --
/// it attributes a colour to a monitor that is not there.
fn rebuild_monitors(hwnd: HWND, cell: &RefCell<State>) {
    let stale = {
        let Ok(mut s) = cell.try_borrow_mut() else {
            return;
        };
        match s.session.refresh_screens() {
            Ok(false) => return,
            Ok(true) => {}
            // Nothing can be done about a failure to enumerate, and the
            // rows that are up are still the best guess there is.
            Err(_) => return,
        }
        let stale: Vec<HWND> =
            s.controls.iter().filter(|c| c.kind == Kind::Monitor).map(|c| c.hwnd).collect();
        s.controls.retain(|c| c.kind != Kind::Monitor);
        // A screen that has gone cannot stay the target of the chips.
        let keys: Vec<String> = s.session.screens().iter().map(|m| m.key.clone()).collect();
        if s.target.as_ref().is_some_and(|key| !keys.contains(key)) {
            s.target = None;
        }
        stale
    };

    // Destroyed outside the borrow: tearing down a window runs its handlers.
    for row in stale {
        let _ = unsafe { DestroyWindow(row) };
    }
    let _ = fill_monitor_list(hwnd, cell);
    // The new rows have no font and no place yet.
    reload(hwnd, cell, None);
}

/// Rebuild "Applies to" from the monitors now attached.
fn sync_applies_to(cell: &RefCell<State>) {
    let state = cell.borrow();
    let Some(combo) = state.control(ID_APPLIES_TO) else {
        return;
    };

    unsafe {
        SendMessageW(combo.hwnd, CB_RESETCONTENT, None, None);
    }
    let mut names = vec!["All monitors".to_string()];
    names.extend(screen_labels(state.session.screens()));
    for name in &names {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            SendMessageW(combo.hwnd, CB_ADDSTRING, None, Some(LPARAM(wide.as_ptr() as isize)));
        }
    }
    // By position, not by name: the rows are labelled for a person now, and
    // what is aimed at is still a key.
    let selected = state
        .target
        .as_ref()
        .and_then(|key| state.session.screens().iter().position(|m| m.key == *key))
        .map_or(0, |at| at + 1);
    unsafe {
        SendMessageW(combo.hwnd, CB_SETCURSEL, Some(WPARAM(selected)), None);
    }
}

impl Drop for ControlWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let state = Box::from_raw(self.state);
            let brush = state.borrow().card_brush;
            if !brush.is_invalid() {
                let _ = DeleteObject(brush.into());
            }
            drop(state);
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
                // No class brush: the page is painted in the theme's own
                // colour, and letting Windows flash grey first is exactly
                // the flicker a modern app does not have.
                hbrBackground: HBRUSH(std::ptr::null_mut()),
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

/// Rebuild everything that depends on the system theme or the DPI.
///
/// Called once at startup and then whenever Windows says the user changed
/// their mind: light to dark, a new accent colour, a different text size, or
/// the window dragged to a monitor with another scale factor.
fn reload(hwnd: HWND, state: &RefCell<State>, dpi: Option<u32>) {
    let (palette, children) = {
        let Ok(mut s) = state.try_borrow_mut() else {
            return;
        };
        if let Some(dpi) = dpi {
            s.dpi = dpi;
            s.scale = dpi as f32 / 96.0;
            s.fonts = Fonts::for_dpi(dpi);
        }
        s.palette = Palette::current();

        let old = s.card_brush;
        s.card_brush = unsafe { CreateSolidBrush(colorref(s.palette.card)) };
        if !old.is_invalid() {
            let _ = unsafe { DeleteObject(old.into()) };
        }

        let font = s.fonts.body;
        let field = s.px(metric::ROW - 6.0) as isize;
        let children: Vec<(HWND, Kind)> = s.controls.iter().map(|c| (c.hwnd, c.kind)).collect();
        for (child, kind) in &children {
            unsafe {
                SendMessageW(*child, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
                if *kind == Kind::Combo {
                    // The closed field, in logical rows rather than
                    // whatever the new font happened to measure.
                    SendMessageW(
                        *child,
                        CB_SETITEMHEIGHT,
                        Some(WPARAM(usize::MAX)),
                        Some(LPARAM(field)),
                    );
                }
            }
        }
        (s.palette, children)
    };

    apply_window_chrome(hwnd, &palette);

    // The one control whose frame the system still draws. `DarkMode_CFD` is
    // the class comctl32 uses for a dark combo box; asking for it in a
    // light theme would be asking for the wrong one, so it is only set
    // while the apps are dark.
    for (child, kind) in children {
        if kind == Kind::Combo {
            let class = if palette.dark { w!("DarkMode_CFD") } else { PCWSTR::null() };
            let _ = unsafe { SetWindowTheme(child, class, PCWSTR::null()) };
        }
    }

    layout::place(hwnd, state);
    unsafe {
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
        // The cards are painted on the erase pass as well as the paint one.
        // A themed child asks its parent to draw what it is sitting on, and
        // that request arrives here, with the child's device context.
        WM_ERASEBKGND => {
            paint_background(hwnd, state, HDC(wparam.0 as *mut _));
            LRESULT(1)
        }
        // What the trackbar asks when it wants its background drawn for it.
        WM_PRINTCLIENT => {
            paint_background(hwnd, state, HDC(wparam.0 as *mut _));
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = unsafe { BeginPaint(hwnd, &raw mut ps) };
            if ps.fErase.as_bool() {
                paint_background(hwnd, state, dc);
            }
            paint_foreground(state, dc);
            let _ = unsafe { EndPaint(hwnd, &raw const ps) };
            LRESULT(0)
        }
        WM_SIZE => {
            layout::place(hwnd, state);
            LRESULT(0)
        }
        // A monitor arrived or left.
        WM_DISPLAYCHANGE => {
            rebuild_monitors(hwnd, state);
            LRESULT(0)
        }
        // A window this dense has a floor below which the cards overlap.
        WM_GETMINMAXINFO => {
            let info = lparam.0 as *mut MINMAXINFO;
            let (scale, natural) =
                state.try_borrow().map(|s| (s.scale, s.natural)).unwrap_or((1.0, 0));
            // The floor is whichever is taller: the layout's own minimum,
            // or what this many monitors actually need.
            let mut client = RECT::default();
            let mut frame = RECT::default();
            unsafe {
                let _ = GetClientRect(hwnd, &raw mut client);
                let _ = GetWindowRect(hwnd, &raw mut frame);
            }
            let chrome = ((frame.bottom - frame.top) - (client.bottom - client.top)).max(0);
            unsafe {
                (*info).ptMinTrackSize.x = (metric::MIN_W * scale) as i32;
                (*info).ptMinTrackSize.y = ((metric::MIN_H * scale) as i32).max(natural + chrome);
            }
            LRESULT(0)
        }
        // Moved to a screen with a different scale: Windows offers a new
        // size, and every length -- and every font -- has to be recomputed
        // against the new dpi.
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as u32;
            let r = unsafe { *(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            reload(hwnd, state, Some(dpi.max(96)));
            LRESULT(0)
        }
        // Light to dark, a new accent colour, a new text size. The
        // setting-change message carries a great deal else besides, and
        // reloading the whole window for an environment variable would be
        // wasteful, so it is filtered to the ones that are ours.
        WM_SETTINGCHANGE => {
            if theme_changed(wparam, lparam) {
                reload(hwnd, state, None);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_THEMECHANGED => {
            reload(hwnd, state, None);
            LRESULT(0)
        }
        WM_MEASUREITEM => {
            let item = lparam.0 as *mut MEASUREITEMSTRUCT;
            if let Ok(s) = state.try_borrow() {
                unsafe { (*item).itemHeight = s.px(metric::ROW - 4.0) as u32 };
            }
            LRESULT(1)
        }
        WM_DRAWITEM => {
            let item = lparam.0 as *const DRAWITEMSTRUCT;
            draw_item(state, unsafe { &*item });
            LRESULT(1)
        }
        // The trackbar keeps its control and loses its paint.
        WM_NOTIFY => {
            let header = lparam.0 as *const NMHDR;
            if unsafe { (*header).code } == NM_CUSTOMDRAW
                && unsafe { (*header).idFrom } == ID_BRIGHTNESS
            {
                return LRESULT(draw_slider(state, lparam.0 as *const NMCUSTOMDRAW) as isize);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // Dark mode is not a colour the system knows about: the list half
        // of the combo box has to be told.
        WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC => {
            let Ok(s) = state.try_borrow() else {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            };
            let dc = HDC(wparam.0 as *mut _);
            unsafe {
                SetTextColor(dc, COLORREF(colorref(s.palette.text).0));
                SetBkMode(dc, TRANSPARENT);
            }
            LRESULT(s.card_brush.0 as isize)
        }
        WM_HSCROLL => {
            let (preview, readout, rows) = {
                let Ok(mut s) = state.try_borrow_mut() else {
                    return LRESULT(0);
                };
                if let Some(slider) = s.control(ID_BRIGHTNESS) {
                    let pos = unsafe { SendMessageW(slider.hwnd, TBM_GETPOS, None, None) }.0;
                    s.session.set_brightness(pos as f32 / 100.0);
                }
                let rows: Vec<HWND> =
                    s.controls.iter().filter(|c| c.kind == Kind::Monitor).map(|c| c.hwnd).collect();
                (s.chrome.preview, s.chrome.brightness, rows)
            };
            // What brightness actually changed -- the preview, the readout
            // and the swatch on each monitor row -- rather than the whole
            // window once per mouse move.
            unsafe {
                let _ = InvalidateRect(Some(hwnd), Some(&raw const preview), false);
                let _ = InvalidateRect(Some(hwnd), Some(&raw const readout), true);
                for row in rows {
                    let _ = InvalidateRect(Some(row), None, false);
                }
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let notification = (wparam.0 >> 16) as u32;
            let id = wparam.0 & 0xFFFF;
            if notification == BN_CLICKED {
                // Enter and Escape arrive as the dialog commands, from the
                // dialog-aware message pump.
                let id = match id as u16 as u32 {
                    v if v == IDOK.0 as u32 => ID_SHOW,
                    v if v == IDCANCEL.0 as u32 => ID_HIDE,
                    _ => id,
                };
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

/// Whether a `WM_SETTINGCHANGE` is one this window has to be rebuilt for.
///
/// `ImmersiveColorSet` is how Windows announces light/dark and the accent
/// colour; the two `SPI_` values are the font metrics and high contrast.
fn theme_changed(wparam: WPARAM, lparam: LPARAM) -> bool {
    if wparam.0 as u32 == SPI_SETNONCLIENTMETRICS.0 || wparam.0 as u32 == SPI_SETHIGHCONTRAST.0 {
        return true;
    }
    if lparam.0 == 0 {
        return false;
    }
    // Safety: for the messages that set it, the parameter is a null
    // terminated wide string naming the area that changed.
    let name = unsafe { PCWSTR(lparam.0 as *const u16).to_string() };
    name.is_ok_and(|name| name == "ImmersiveColorSet")
}

/// Hot-tracking for the owner-drawn controls.
///
/// A themed button tracks the pointer itself and a `BS_OWNERDRAW` one does
/// not, so each control watches for the pointer arriving and leaving and
/// repaints itself. Without this the window has no hover state at all,
/// which is the first thing that makes a Win32 app feel dead next to a
/// WinUI one.
unsafe extern "system" fn hover_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            if set_hot(data, hwnd, true) {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = unsafe { TrackMouseEvent(&raw mut track) };
            }
        }
        WM_MOUSELEAVE => {
            set_hot(data, hwnd, false);
        }
        WM_NCDESTROY => {
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(hover_proc), id) };
        }
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Record that the pointer has arrived at or left a control.
///
/// Returns whether this was a change, which is when the pointer has to be
/// tracked for its departure.
fn set_hot(data: usize, hwnd: HWND, hot: bool) -> bool {
    if data == 0 {
        return false;
    }
    let state = unsafe { &*(data as *const RefCell<State>) };
    let Ok(mut s) = state.try_borrow_mut() else {
        return false;
    };
    let Some(control) = s.controls.iter_mut().find(|c| c.hwnd == hwnd) else {
        return false;
    };
    if control.hot == hot {
        return false;
    }
    control.hot = hot;
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
    true
}

/// A control was clicked.
fn command(hwnd: HWND, state: &RefCell<State>, id: usize) {
    // The colour dialog is modal and pumps messages of its own, so no
    // borrow of the state may be held while it is up: a repaint arriving
    // mid-dialog would be the second borrow.
    if id == ID_CHIP_LAST {
        let (current, custom) = {
            let Ok(s) = state.try_borrow() else { return };
            (s.aimed_color(), s.session.custom_color())
        };
        if let Some(picked) = choose_color(hwnd, current, custom)
            && let Ok(mut s) = state.try_borrow_mut()
        {
            // Remembered as the custom colour as well as applied, so the
            // dialog opens on it next time.
            s.session.set_custom_color(picked);
            apply_color(&mut s, picked);
        }
        refresh(hwnd);
        return;
    }

    {
        let Ok(mut s) = state.try_borrow_mut() else {
            return;
        };
        let s = &mut *s;
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
                if let Some((_, pattern)) = FILLS.get(id - ID_FILL_FIRST) {
                    // The region half and its second colour are the region
                    // controls' own state: kept while another mode is on, so
                    // that coming back to Region comes back to the same half
                    // rather than to the default one. The command line is
                    // the only thing that sets them for now.
                    let (side, rest) = match s.session.fill() {
                        Fill::Region { side, rest } if side != Side::Full => (side, rest),
                        Fill::Region { rest, .. } => (Side::Left, rest),
                        _ => (Side::Left, None),
                    };
                    // Clicking the mode that is already on turns it off,
                    // which is the plain colour.
                    let next = if *pattern == s.session.fill().pattern() {
                        Fill::default()
                    } else {
                        pattern.fill(side, rest)
                    };
                    s.session.set_fill(next);
                }
            }
            id if id >= ID_MONITOR_FIRST => toggle_monitor(s, id),
            _ => {}
        }
    }
    refresh(hwnd);
}

/// Tick or untick one screen.
///
/// An empty selection means every screen, so the last ticked box cannot be
/// unticked: there is no way to say "none", and a window whose boxes are all
/// clear while every screen fills would be a lie.
fn toggle_monitor(s: &mut State, id: usize) {
    let Some(key) = s.monitor_key(id) else { return };
    let all: Vec<String> = s.session.screens().iter().map(|m| m.key.clone()).collect();
    let mut keys: Vec<String> = if s.session.selection().is_empty() {
        all
    } else {
        all.into_iter().filter(|k| selected(s.session.selection(), k)).collect()
    };

    match keys.iter().position(|k| *k == key) {
        Some(_) if keys.len() == 1 => return,
        Some(at) => {
            keys.remove(at);
        }
        None => keys.push(key),
    }
    let _ = s.session.set_selection(keys);
}

/// Repaint the window and everything on it.
fn refresh(hwnd: HWND) {
    unsafe {
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN);
    }
}

/// Ask for a colour with the system picker.
///
/// `None` when the user cancelled, which must leave everything as it was.
fn choose_color(owner: HWND, current: Rgba, saved: Option<Rgba>) -> Option<Rgba> {
    // The dialog keeps the sixteen custom slots in whatever the caller
    // hands it, so they live for the process rather than the call.
    static CUSTOM: std::sync::Mutex<[u32; 16]> = std::sync::Mutex::new([0x00FF_FFFF; 16]);
    let mut custom = CUSTOM.lock().ok()?;
    // The colour the picker last produced, from a previous run of the app,
    // goes in the first slot: the dialog is then where it was left.
    if let Some(saved) = saved {
        custom[0] = colorref(saved).0;
    }
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
        let Ok(mut s) = state.try_borrow_mut() else {
            return;
        };
        let s = &mut *s;
        let Some(combo) = s.control(ID_APPLIES_TO) else {
            return;
        };
        let row = unsafe { SendMessageW(combo.hwnd, CB_GETCURSEL, None, None) }.0;
        s.target = if row <= 0 {
            None
        } else {
            s.session.screens().get((row - 1) as usize).map(|m| m.key.clone())
        };
    }
    // Laid out again, not merely repainted: the preview is the shape of the
    // screen it is aimed at, so aiming it somewhere else resizes it.
    layout::place(hwnd, state);
    refresh(hwnd);
}
