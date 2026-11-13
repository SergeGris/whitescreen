//! The control window.
//!
//! The Windows counterpart of the GTK build's window: the preview, the
//! colour chips, the key-light temperatures, brightness, the fill modes,
//! the cycle and the monitor list.
//!
//! # Why so much of it is drawn by hand
//!
//! The controls are real `BUTTON` and `COMBOBOX` windows -- so tab, space,
//! the focus ring and the screen reader all work the way Windows users
//! expect -- but almost all of them are `BS_OWNERDRAW` and painted here
//! against the Fluent palette in `theme`. A themed Win32 checkbox draws its
//! own label in the *theme's* text colour, which is black whatever the app
//! does, so an app that follows the system into dark mode and does not own
//! its pixels ends up with black text on a near-black card. Owning the
//! drawing is what makes the two themes both correct, and it is also what
//! gets WinUI's geometry: 4 px corners on a control, 8 px on a card, a
//! 32 px row, and the hover and pressed fills a modern Windows app has.
//!
//! The brightness slider is the exception that keeps its system control: a
//! trackbar is custom-drawn through `NM_CUSTOMDRAW`, which leaves the real
//! control -- and its keyboard handling and accessibility -- in place while
//! the channel and thumb come out Fluent.
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
    BeginPaint, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS,
    DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteObject, Ellipse, EndPaint, FillRect, GetDC,
    HBRUSH, HDC, HFONT, InvalidateRect, PAINTSTRUCT, PS_SOLID, RDW_ALLCHILDREN, RDW_ERASE,
    RDW_INVALIDATE, RedrawWindow, ReleaseDC, SelectClipRgn, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT, UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDIS_SELECTED, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, MEASUREITEMSTRUCT, NM_CUSTOMDRAW, NMCUSTOMDRAW, NMHDR, ODS_COMBOBOXEDIT,
    ODS_FOCUS, ODS_SELECTED, ODT_COMBOBOX, SetWindowTheme, TBCD_CHANNEL, TBCD_THUMB, TBCD_TICS,
    TBM_GETTHUMBRECT, TBM_SETPOS, TBM_SETRANGE, TBS_NOTICKS, TBS_TRANSPARENTBKGND, TRACKBAR_CLASSW,
    WM_MOUSELEAVE,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_OWNERDRAW, CB_ADDSTRING, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN,
    CB_RESETCONTENT, CB_SETCURSEL, CB_SETITEMHEIGHT, CBN_SELCHANGE, CBS_DROPDOWNLIST,
    CBS_OWNERDRAWFIXED, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, GetWindowRect, HMENU, IDC_ARROW, IDCANCEL,
    IDOK, LoadCursorW, MINMAXINFO, MoveWindow, PostQuitMessage, RegisterClassW,
    SPI_SETHIGHCONTRAST, SPI_SETNONCLIENTMETRICS, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER,
    SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_COMMAND, WM_CREATE, WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_DPICHANGED, WM_DRAWITEM, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_HSCROLL, WM_MEASUREITEM,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_NOTIFY, WM_PAINT, WM_PRINTCLIENT, WM_SETFONT, WM_SETTINGCHANGE,
    WM_SIZE, WM_THEMECHANGED, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use whitescreen_core::{Fill, Rgba, Side, kelvin};

use crate::icon::Icons;
use crate::paint::{colorref, paint};
use crate::session::Session;
use crate::theme::{
    Fonts, Palette, RADIUS_CARD, RADIUS_CONTROL, apply_window_chrome, focus_ring, glyph,
    round_rect, text, text_width,
};

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

/// Brightness runs from 5% rather than 0: a black screen is what the black
/// chip is for, and a slider that can blank the overlay looks like a fault.
const BRIGHTNESS_MIN: i32 = 5;
const BRIGHTNESS_MAX: i32 = 100;

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

/// WinUI's metrics, in logical pixels, scaled by the window's DPI.
mod metric {
    /// The margin around the page.
    pub const PAGE: f32 = 20.0;
    /// The margin inside a card.
    pub const CARD: f32 = 16.0;
    /// Between cards.
    pub const GAP: f32 = 12.0;
    /// A control's height, and the gap between two rows of them.
    pub const ROW: f32 = 32.0;
    pub const ROW_GAP: f32 = 8.0;
    /// The live preview.
    pub const PREVIEW: f32 = 164.0;
    /// The monitor list down the left.
    pub const SIDEBAR: f32 = 296.0;
    /// A colour chip, and the shorter key-light chip.
    pub const CHIP_W: f32 = 46.0;
    pub const CHIP_H: f32 = 32.0;
    pub const CHIP_GAP: f32 = 8.0;
    pub const KELVIN_H: f32 = 26.0;
    /// A card's heading, and a line of caption text.
    pub const HEADING: f32 = 24.0;
    pub const CAPTION: f32 = 18.0;
    /// The page title at the top of the window.
    pub const TITLE: f32 = 30.0;
    /// The box of a checkbox.
    pub const CHECKBOX: f32 = 20.0;
    /// The narrowest a command button goes; a long label makes its own.
    pub const BUTTON_NARROW: f32 = 104.0;
    /// Below this the cards start to overlap, so the window will not go:
    /// the sidebar, a gap, and a colour card wide enough for six chips.
    pub const MIN_W: f32 = 800.0;
    pub const MIN_H: f32 = 760.0;
}

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
    /// Whether the pointer is over it. Maintained by `hover_proc`.
    hot: bool,
}

/// Text the window draws for itself, as opposed to a control's own label.
struct Label {
    rect: RECT,
    text: String,
    role: Role,
}

#[derive(Clone, Copy)]
enum Role {
    /// The page title: WinUI's Subtitle.
    Title,
    /// A card heading: Body Strong.
    Heading,
    /// A hint or a field label: Caption, in the secondary colour.
    Caption,
}

/// Where everything the window paints itself has ended up.
///
/// Recomputed by `layout` and used by both the erase and the paint handler,
/// so a themed child asking its parent for a background gets the same card
/// colour it is sitting on.
#[derive(Default)]
struct Chrome {
    preview: RECT,
    cards: Vec<RECT>,
    labels: Vec<Label>,
    /// The brightness readout, whose text comes from the session.
    brightness: RECT,
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
                .is_some_and(|(_, f)| same_kind(*f, self.session.fill())),
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

    /// The colour the chips are currently aimed at, so the matching chip can
    /// show itself as the selected one.
    fn aimed_color(&self) -> Rgba {
        match &self.target {
            Some(key) => self.session.monitor_color(key).unwrap_or(self.session.shared_color()),
            None => self.session.shared_color(),
        }
    }
}

/// Whether an owner-draw state flag is set.
fn has(
    state: windows::Win32::UI::Controls::ODS_FLAGS,
    flag: windows::Win32::UI::Controls::ODS_FLAGS,
) -> bool {
    state.0 & flag.0 != 0
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
                960,
                840,
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
            CBS_DROPDOWNLIST as u32 | CBS_OWNERDRAWFIXED as u32 | WS_VSCROLL.0,
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
    Ok(Control { hwnd, id, kind, on_card, label: label.to_string(), hot: false })
}

/// One row per attached monitor, and the drop-down to match.
fn fill_monitor_list(hwnd: HWND, cell: &RefCell<State>) -> windows::core::Result<()> {
    let screens: Vec<_> = cell.borrow().session.screens().to_vec();

    let mut created = Vec::new();
    for (i, screen) in screens.iter().enumerate() {
        let label = format!("{}   {}×{}", screen.key, screen.width, screen.height);
        created.push(spawn(
            hwnd,
            cell as *const _ as usize,
            &label,
            ID_MONITOR_FIRST + i,
            Kind::Monitor,
        )?);
    }

    // The font comes with the reload that follows; this only has to get the
    // rows into the list.
    cell.borrow_mut().controls.extend(created);
    sync_applies_to(cell);
    Ok(())
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
    names.extend(state.session.screens().iter().map(|m| m.key.clone()));
    for name in &names {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            SendMessageW(combo.hwnd, CB_ADDSTRING, None, Some(LPARAM(wide.as_ptr() as isize)));
        }
    }
    let selected =
        state.target.as_ref().and_then(|key| names.iter().position(|n| n == key)).unwrap_or(0);
    unsafe {
        SendMessageW(combo.hwnd, CB_SETCURSEL, Some(WPARAM(selected)), None);
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

    layout(hwnd, state);
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
            layout(hwnd, state);
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

/// The page and the cards: everything behind the controls.
fn paint_background(hwnd: HWND, state: &RefCell<State>, dc: HDC) {
    let Ok(s) = state.try_borrow() else { return };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut client) };

    unsafe {
        let brush = CreateSolidBrush(colorref(s.palette.base));
        FillRect(dc, &raw const client, brush);
        let _ = DeleteObject(brush.into());
    }

    let radius = s.px(RADIUS_CARD);
    for card in &s.chrome.cards {
        round_rect(dc, *card, radius, Some(s.palette.card), Some(s.palette.card_stroke));
    }
}

/// The preview, the headings and the captions.
fn paint_foreground(state: &RefCell<State>, dc: HDC) {
    let Ok(s) = state.try_borrow() else { return };

    // The preview, with the corner radius every other surface has. GDI will
    // not round a fill, so the painter is clipped to the rounded shape
    // instead.
    let preview = s.chrome.preview;
    if preview.right > preview.left && preview.bottom > preview.top {
        let radius = s.px(RADIUS_CARD);
        unsafe {
            let region = CreateRoundRectRgn(
                preview.left,
                preview.top,
                preview.right + 1,
                preview.bottom + 1,
                radius * 2,
                radius * 2,
            );
            SelectClipRgn(dc, Some(region));
            // What the chips are editing, which is the aimed-at monitor's
            // own colour when "Applies to" names one.
            let color = s.aimed_color().scaled(s.session.brightness());
            paint(dc, preview, color, s.session.fill(), s.scale, 0);
            SelectClipRgn(dc, None);
            let _ = DeleteObject(region.into());
        }
        // A hairline so a white preview has an edge on a white card.
        round_rect(dc, preview, s.px(RADIUS_CARD), None, Some(s.palette.card_stroke));
    }

    for label in &s.chrome.labels {
        let (font, color) = match label.role {
            Role::Title => (s.fonts.heading, s.palette.text),
            Role::Heading => (s.fonts.strong, s.palette.text),
            Role::Caption => (s.fonts.caption, s.palette.text_secondary),
        };
        text(
            dc,
            label.rect,
            &label.text,
            font,
            color,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    }

    let percent = format!("{}%", (s.session.brightness() * 100.0).round() as i32);
    text(
        dc,
        s.chrome.brightness,
        &percent,
        s.fonts.caption,
        s.palette.text_secondary,
        DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
    );
}

/// A vertical cursor: the layout is columns of rows, and this hands them out.
struct Stack {
    x: i32,
    y: i32,
    width: i32,
}

impl Stack {
    fn row(&mut self, height: i32, gap: i32) -> RECT {
        let rect =
            RECT { left: self.x, top: self.y, right: self.x + self.width, bottom: self.y + height };
        self.y += height + gap;
        rect
    }
}

/// Split `rect` into a left part of `width` and the rest, `gap` apart.
fn split(rect: RECT, width: i32, gap: i32) -> (RECT, RECT) {
    (RECT { right: rect.left + width, ..rect }, RECT { left: rect.left + width + gap, ..rect })
}

/// Trim a rectangle inwards on every side.
fn inset(rect: RECT, by: i32) -> RECT {
    RECT {
        left: rect.left + by,
        top: rect.top + by,
        right: rect.right - by,
        bottom: rect.bottom - by,
    }
}

/// How wide a checkbox has to be for a label `text` pixels wide: the box,
/// the gap WinUI puts after it, the text, and a little slack so the focus
/// ring has somewhere to go.
fn check_width(s: &State, text: i32) -> i32 {
    s.px(metric::CHECKBOX) + s.px(10.0) + text + s.px(8.0)
}

/// Place every control and work out everything the window draws itself.
fn layout(hwnd: HWND, state: &RefCell<State>) {
    let moves = {
        let Ok(mut s) = state.try_borrow_mut() else {
            return;
        };
        let px = |v: f32| (v * s.scale).round() as i32;

        let mut client = RECT::default();
        let _ = unsafe { GetClientRect(hwnd, &raw mut client) };

        // Control widths are measured rather than guessed: the type ramp
        // follows the user's text-size setting, so a width that fits
        // "Per-monitor colors" at 100% clips it at 125%.
        let dc = unsafe { GetDC(Some(hwnd)) };
        let measure = |label: &str, font: HFONT| {
            if dc.is_invalid() { 0 } else { text_width(dc, label, font) }
        };

        let pad = px(metric::PAGE);
        let gap = px(metric::GAP);
        let card_pad = px(metric::CARD);
        let row = px(metric::ROW);
        let row_gap = px(metric::ROW_GAP);
        let heading = px(metric::HEADING);
        let caption = px(metric::CAPTION);

        let mut chrome = Chrome::default();
        let mut moves: Vec<(HWND, RECT)> = Vec::new();
        let mut page = Stack {
            x: client.left + pad,
            y: client.top + pad,
            width: (client.right - client.left - pad * 2).max(px(400.0)),
        };

        // The page title, the way a WinUI page opens: a subtitle-sized name
        // and one line saying what the page is for.
        chrome.labels.push(Label {
            rect: page.row(px(metric::TITLE), 0),
            text: "White Screen".to_string(),
            role: Role::Title,
        });
        chrome.labels.push(Label {
            rect: page.row(caption, gap),
            text: "Solid colors and test patterns on every attached screen.".to_string(),
            role: Role::Caption,
        });

        chrome.preview = page.row(px(metric::PREVIEW), gap);

        // The two-column row: the screens on the left, colour on the right.
        let screens = s.session.screens().len() as i32;
        let displays_h = card_pad * 2 + heading + caption + screens.max(1) * row;
        let colour_h = card_pad * 2
            + heading
            + px(metric::CHIP_H)
            + px(10.0)
            + caption
            + px(metric::KELVIN_H)
            + gap
            + row
            + row_gap
            + row;
        let columns = page.row(displays_h.max(colour_h), gap);
        let sidebar = px(metric::SIDEBAR).min(page.width / 2);
        let (displays_card, colour_card) = split(columns, sidebar, gap);
        chrome.cards.push(displays_card);
        chrome.cards.push(colour_card);

        // The screens.
        let mut column = Stack {
            x: displays_card.left + card_pad,
            y: displays_card.top + card_pad,
            width: sidebar - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Displays".to_string(),
            role: Role::Heading,
        });
        chrome.labels.push(Label {
            rect: column.row(caption, 0),
            text: "Which screens an overlay covers".to_string(),
            role: Role::Caption,
        });
        for control in s.controls.iter().filter(|c| c.kind == Kind::Monitor) {
            moves.push((control.hwnd, column.row(row, 0)));
        }

        // Colour.
        let mut column = Stack {
            x: colour_card.left + card_pad,
            y: colour_card.top + card_pad,
            width: colour_card.right - colour_card.left - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Color".to_string(),
            role: Role::Heading,
        });

        let chips = column.row(px(metric::CHIP_H), px(10.0));
        let chip_w = px(metric::CHIP_W);
        let chip_gap = px(metric::CHIP_GAP);
        for (i, control) in
            s.controls.iter().filter(|c| (ID_CHIP_FIRST..ID_CHIP_LAST).contains(&c.id)).enumerate()
        {
            let left = chips.left + i as i32 * (chip_w + chip_gap);
            moves.push((control.hwnd, RECT { left, right: left + chip_w, ..chips }));
        }
        if let Some(custom) = s.control(ID_CHIP_LAST) {
            let left = chips.left + CHIPS.len() as i32 * (chip_w + chip_gap);
            moves.push((
                custom.hwnd,
                RECT { left, right: left + px(metric::BUTTON_NARROW), ..chips },
            ));
        }

        chrome.labels.push(Label {
            rect: column.row(caption, 0),
            text: "Key light".to_string(),
            role: Role::Caption,
        });
        let kelvin_row = column.row(px(metric::KELVIN_H), gap);
        for (i, control) in s
            .controls
            .iter()
            .filter(|c| (ID_KELVIN_FIRST..=ID_KELVIN_LAST).contains(&c.id))
            .enumerate()
        {
            let left = kelvin_row.left + i as i32 * (chip_w + chip_gap);
            moves.push((control.hwnd, RECT { left, right: left + chip_w, ..kelvin_row }));
        }

        // "Applies to" and the per-monitor switch share a row.
        let aim_row = column.row(row, row_gap);
        let label_w = measure("Applies to", s.fonts.caption)
            .max(measure("Brightness", s.fonts.caption))
            + px(12.0);
        let (aim_label, rest) = split(aim_row, label_w, px(4.0));
        chrome.labels.push(Label {
            rect: aim_label,
            text: "Applies to".to_string(),
            role: Role::Caption,
        });
        let (combo, per_monitor) = split(rest, px(168.0), gap);
        if let Some(control) = s.control(ID_APPLIES_TO) {
            // A drop-down list clamps itself to the field height; the extra
            // is the room its list is allowed to use.
            moves.push((control.hwnd, RECT { bottom: combo.bottom + px(180.0), ..combo }));
        }
        if let Some(control) = s.control(ID_PER_MONITOR) {
            moves.push((control.hwnd, per_monitor));
        }

        // Brightness: the label, the slider, the readout.
        let bright_row = column.row(row, 0);
        let (bright_label, rest) = split(bright_row, label_w, px(4.0));
        chrome.labels.push(Label {
            rect: bright_label,
            text: "Brightness".to_string(),
            role: Role::Caption,
        });
        let readout = px(44.0);
        let (slider, value) = split(rest, (rest.right - rest.left - readout).max(px(80.0)), 0);
        if let Some(control) = s.control(ID_BRIGHTNESS) {
            moves.push((control.hwnd, slider));
        }
        chrome.brightness = value;

        // Pattern.
        let pattern_card = page.row(card_pad * 2 + heading + row, gap);
        chrome.cards.push(pattern_card);
        let mut column = Stack {
            x: pattern_card.left + card_pad,
            y: pattern_card.top + card_pad,
            width: pattern_card.right - pattern_card.left - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Pattern".to_string(),
            role: Role::Heading,
        });
        let fills = column.row(row, 0);
        let mut left = fills.left;
        for control in s.controls.iter().filter(|c| (ID_FILL_FIRST..=ID_FILL_LAST).contains(&c.id))
        {
            let width = check_width(&s, measure(&control.label, s.fonts.body));
            moves.push((control.hwnd, RECT { left, right: left + width, ..fills }));
            left += width + gap;
        }

        // Session.
        let session_card = page.row(card_pad * 2 + heading + row, gap);
        chrome.cards.push(session_card);
        let mut column = Stack {
            x: session_card.left + card_pad,
            y: session_card.top + card_pad,
            width: session_card.right - session_card.left - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Session".to_string(),
            role: Role::Heading,
        });
        let switches = column.row(row, 0);
        let mut left = switches.left;
        for id in [ID_CYCLE, ID_IDENTIFY] {
            let Some(control) = s.control(id) else {
                continue;
            };
            let width = check_width(&s, measure(&control.label, s.fonts.body));
            moves.push((control.hwnd, RECT { left, right: left + width, ..switches }));
            left += width + gap * 2;
        }

        // The commands go at the foot of the window, right-aligned, with
        // the default action last: the Windows order.
        let actions = RECT {
            left: page.x,
            top: (client.bottom - pad - row).max(page.y),
            right: page.x + page.width,
            bottom: (client.bottom - pad).max(page.y + row),
        };
        let mut right = actions.right;
        for id in [ID_SHOW, ID_HIDE] {
            let Some(control) = s.control(id) else {
                continue;
            };
            // Wide enough for the label, never narrower than WinUI's
            // minimum button, and free to grow for a longer translation.
            let width =
                (measure(&control.label, s.fonts.body) + px(32.0)).max(px(metric::BUTTON_NARROW));
            moves.push((control.hwnd, RECT { left: right - width, right, ..actions }));
            right -= width + row_gap;
        }

        if !dc.is_invalid() {
            unsafe { ReleaseDC(Some(hwnd), dc) };
        }
        // Where the last card ended, plus the row of commands and the
        // margin under it.
        s.natural = page.y + row + pad;
        s.chrome = chrome;
        moves
    };

    for (child, rect) in moves {
        let _ = unsafe {
            MoveWindow(
                child,
                rect.left,
                rect.top,
                (rect.right - rect.left).max(0),
                (rect.bottom - rect.top).max(0),
                true,
            )
        };
    }
}

/// Where an owner-drawn face goes, what it sits on, and which of its states
/// it is in. Bundled because every face needs all six and a function taking
/// them one at a time reads as a puzzle.
struct Face {
    dc: HDC,
    rect: RECT,
    /// The colour of the surface underneath, which the focus ring needs and
    /// the face clears itself to.
    surface: Rgba,
    hot: bool,
    pressed: bool,
    focused: bool,
}

/// Paint one owner-drawn control.
fn draw_item(state: &RefCell<State>, item: &DRAWITEMSTRUCT) {
    let Ok(s) = state.try_borrow() else { return };
    if item.CtlType == ODT_COMBOBOX {
        return draw_combo_item(&s, item);
    }
    let Some(control) = s.controls.iter().find(|c| c.hwnd == item.hwndItem) else {
        return;
    };

    let face = Face {
        dc: item.hDC,
        rect: item.rcItem,
        // Clear to the surface underneath first. A `BS_OWNERDRAW` button is
        // never erased for us, and none of these faces cover their whole
        // rectangle, so without this the ring from the last hover stays put.
        surface: if control.on_card { s.palette.card } else { s.palette.base },
        hot: control.hot,
        pressed: has(item.itemState, ODS_SELECTED),
        focused: has(item.itemState, ODS_FOCUS),
    };
    unsafe {
        let brush = CreateSolidBrush(colorref(face.surface));
        FillRect(face.dc, &raw const face.rect, brush);
        let _ = DeleteObject(brush.into());
    }

    match control.kind {
        Kind::Swatch(color) => draw_swatch(&s, &face, color, s.aimed_color() == color),
        Kind::Push { accent } => draw_push(&s, &face, &control.label, accent),
        Kind::Check => draw_check(&s, &face, &control.label, s.checked(control.id), None),
        Kind::Monitor => draw_check(
            &s,
            &face,
            &control.label,
            s.checked(control.id),
            Some(s.monitor_swatch(control.id)),
        ),
        Kind::Combo | Kind::Slider => {}
    }
}

/// A colour chip.
///
/// The chip is the colour, so the states cannot be shown by tinting it: a
/// hover is a ring, a press is a smaller chip, and the chip that matches the
/// colour now in force wears the accent ring.
fn draw_swatch(s: &State, face: &Face, color: Rgba, current: bool) {
    let Face { dc, rect, .. } = *face;
    let radius = s.px(RADIUS_CONTROL + 2.0);
    let body = if face.pressed { inset(rect, s.px(2.0)) } else { inset(rect, s.px(1.0)) };
    // A hairline that contrasts with the fill, so a white chip does not
    // vanish into a light card -- the same rule as the GTK swatches.
    let edge = if color.luma() > 0.5 { Rgba::new(0, 0, 0) } else { Rgba::new(255, 255, 255) };
    let edge = crate::theme::blend(color, edge, 0.35);
    round_rect(dc, body, radius, Some(color), Some(edge));

    if current {
        round_rect(dc, rect, radius + s.px(1.0), None, Some(s.palette.accent));
        round_rect(dc, inset(rect, s.px(1.0)), radius, None, Some(s.palette.accent));
    } else if face.hot {
        round_rect(dc, rect, radius + s.px(1.0), None, Some(s.palette.control_stroke));
    }
    if face.focused {
        focus_ring(dc, rect, radius + s.px(1.0), &s.palette, face.surface);
    }
}

/// A command button.
fn draw_push(s: &State, face: &Face, label: &str, accent: bool) {
    let Face { dc, rect, hot, pressed, .. } = *face;
    let radius = s.px(RADIUS_CONTROL);
    let (fill, stroke, ink) = if accent {
        (s.palette.accent_for(hot, pressed), None, s.palette.on_accent)
    } else {
        (s.palette.control_for(hot, pressed), Some(s.palette.control_stroke), s.palette.text)
    };
    round_rect(dc, rect, radius, Some(fill), stroke);
    text(
        dc,
        rect,
        label,
        s.fonts.body,
        ink,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
    );
    if face.focused {
        focus_ring(dc, rect, radius, &s.palette, face.surface);
    }
}

/// A checkbox, and for a monitor row the screen's colour beside it.
fn draw_check(s: &State, face: &Face, label: &str, checked: bool, swatch: Option<Rgba>) {
    let Face { dc, rect, hot, pressed, .. } = *face;
    let size = s.px(metric::CHECKBOX);
    let middle = (rect.top + rect.bottom) / 2;
    let box_rect = RECT {
        left: rect.left,
        top: middle - size / 2,
        right: rect.left + size,
        bottom: middle + size / 2,
    };
    let radius = s.px(RADIUS_CONTROL);

    if checked {
        round_rect(dc, box_rect, radius, Some(s.palette.accent_for(hot, pressed)), None);
        text(
            dc,
            box_rect,
            glyph::CHECK,
            s.fonts.icon,
            s.palette.on_accent,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    } else {
        round_rect(
            dc,
            box_rect,
            radius,
            Some(s.palette.control_for(hot, pressed)),
            Some(s.palette.control_stroke),
        );
    }

    let mut label_rect = RECT { left: box_rect.right + s.px(10.0), ..rect };
    if let Some(color) = swatch {
        // The screen's colour at the right-hand end of the row.
        let chip = s.px(18.0);
        let chip_rect = RECT {
            left: rect.right - chip,
            top: middle - chip / 2,
            right: rect.right,
            bottom: middle + chip / 2,
        };
        let edge = crate::theme::blend(
            color,
            if color.luma() > 0.5 { Rgba::new(0, 0, 0) } else { Rgba::new(255, 255, 255) },
            0.35,
        );
        round_rect(dc, chip_rect, s.px(RADIUS_CONTROL), Some(color), Some(edge));
        label_rect.right = chip_rect.left - s.px(8.0);
    }
    text(
        dc,
        label_rect,
        label,
        s.fonts.body,
        s.palette.text,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );

    if face.focused {
        focus_ring(dc, rect, radius, &s.palette, face.surface);
    }
}

/// One row of the drop-down, or its closed field.
fn draw_combo_item(s: &State, item: &DRAWITEMSTRUCT) {
    let hwnd = item.hwndItem;
    // -1 is "nothing selected", which only reads as negative once the
    // unsigned field has been cast back.
    let index = item.itemID as i32;
    if index < 0 {
        return;
    }
    // The text belongs to the control, so it has to be asked for it.
    let length =
        unsafe { SendMessageW(hwnd, CB_GETLBTEXTLEN, Some(WPARAM(index as usize)), None) }.0;
    if length <= 0 {
        return;
    }
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe {
        SendMessageW(
            hwnd,
            CB_GETLBTEXT,
            Some(WPARAM(index as usize)),
            Some(LPARAM(buffer.as_mut_ptr() as isize)),
        );
    }
    let label = String::from_utf16_lossy(&buffer[..length as usize]);

    let field = has(item.itemState, ODS_COMBOBOXEDIT);
    let highlighted = !field && has(item.itemState, ODS_SELECTED);
    let (fill, ink) = if highlighted {
        (Some(s.palette.accent), s.palette.on_accent)
    } else if field {
        // The themed frame is still the system's, but the inside of it is
        // ours to clear: leaving it to the theme would leave the last
        // selection's text underneath this one.
        (Some(s.palette.control), s.palette.text)
    } else {
        (Some(s.palette.card), s.palette.text)
    };

    if let Some(fill) = fill {
        unsafe {
            let brush = CreateSolidBrush(colorref(fill));
            FillRect(item.hDC, &raw const item.rcItem, brush);
            let _ = DeleteObject(brush.into());
        }
    }
    let inner = RECT { left: item.rcItem.left + s.px(8.0), ..item.rcItem };
    text(
        item.hDC,
        inner,
        &label,
        s.fonts.body,
        ink,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
}

/// Custom-draw the brightness trackbar.
///
/// The control stays a trackbar -- keyboard, accessibility, the lot -- and
/// only its pixels change: a 4 px rail, accent up to the thumb, and a round
/// thumb with an accent core, which is the Fluent slider.
fn draw_slider(state: &RefCell<State>, draw: *const NMCUSTOMDRAW) -> u32 {
    let draw = unsafe { &*draw };
    if draw.dwDrawStage == CDDS_PREPAINT {
        return CDRF_NOTIFYITEMDRAW;
    }
    if draw.dwDrawStage != CDDS_ITEMPREPAINT {
        return CDRF_DODEFAULT;
    }
    let Ok(s) = state.try_borrow() else {
        return CDRF_DODEFAULT;
    };

    match draw.dwItemSpec as u32 {
        v if v == TBCD_TICS => CDRF_SKIPDEFAULT,
        v if v == TBCD_CHANNEL => {
            let height = s.px(4.0).max(2);
            let middle = (draw.rc.top + draw.rc.bottom) / 2;
            let rail = RECT {
                left: draw.rc.left,
                top: middle - height / 2,
                right: draw.rc.right,
                bottom: middle - height / 2 + height,
            };
            let radius = height / 2;
            round_rect(draw.hdc, rail, radius, Some(s.palette.control_stroke), None);

            // The part behind the thumb carries the accent. The thumb's
            // own rectangle is the honest place to stop: the channel is
            // wider than the distance the thumb's centre travels.
            let mut thumb = RECT::default();
            unsafe {
                SendMessageW(
                    draw.hdr.hwndFrom,
                    TBM_GETTHUMBRECT,
                    None,
                    Some(LPARAM(&raw mut thumb as isize)),
                );
            }
            let edge = if thumb.right > thumb.left && thumb.left >= draw.rc.left {
                (thumb.left + thumb.right) / 2
            } else {
                // No thumb rectangle to be had: fall back to the value.
                let span = (rail.right - rail.left).max(1);
                let fraction = ((s.session.brightness() * 100.0).round() as i32 - BRIGHTNESS_MIN)
                    .clamp(0, BRIGHTNESS_MAX - BRIGHTNESS_MIN)
                    as f32
                    / (BRIGHTNESS_MAX - BRIGHTNESS_MIN) as f32;
                rail.left + (span as f32 * fraction) as i32
            };
            let filled = RECT { right: edge, ..rail };
            if filled.right > filled.left {
                round_rect(draw.hdc, filled, radius, Some(s.palette.accent), None);
            }
            CDRF_SKIPDEFAULT
        }
        v if v == TBCD_THUMB => {
            let pressed = draw.uItemState.contains(CDIS_SELECTED);
            let rect = draw.rc;
            let middle = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            let outer = ((rect.bottom - rect.top).min(rect.right - rect.left) / 2).max(s.px(9.0));
            // WinUI shrinks the core while the thumb is held.
            let core = if pressed { outer / 2 } else { (outer * 2) / 3 };
            unsafe {
                let pen = CreatePen(PS_SOLID, 1, colorref(s.palette.control_stroke));
                let brush = CreateSolidBrush(colorref(s.palette.card));
                let old_pen = SelectObject(draw.hdc, pen.into());
                let old_brush = SelectObject(draw.hdc, brush.into());
                let _ = Ellipse(
                    draw.hdc,
                    middle.0 - outer,
                    middle.1 - outer,
                    middle.0 + outer,
                    middle.1 + outer,
                );
                SelectObject(draw.hdc, old_brush);
                let _ = DeleteObject(brush.into());

                let core_brush = CreateSolidBrush(colorref(s.palette.accent));
                let old_brush = SelectObject(draw.hdc, core_brush.into());
                let core_pen = CreatePen(PS_SOLID, 1, colorref(s.palette.accent));
                let discarded = SelectObject(draw.hdc, core_pen.into());
                let _ = Ellipse(
                    draw.hdc,
                    middle.0 - core,
                    middle.1 - core,
                    middle.0 + core,
                    middle.1 + core,
                );
                SelectObject(draw.hdc, discarded);
                SelectObject(draw.hdc, old_brush);
                SelectObject(draw.hdc, old_pen);
                let _ = DeleteObject(core_pen.into());
                let _ = DeleteObject(core_brush.into());
                let _ = DeleteObject(pen.into());
            }
            CDRF_SKIPDEFAULT
        }
        _ => CDRF_DODEFAULT,
    }
}

/// A control was clicked.
fn command(hwnd: HWND, state: &RefCell<State>, id: usize) {
    // The colour dialog is modal and pumps messages of its own, so no
    // borrow of the state may be held while it is up: a repaint arriving
    // mid-dialog would be the second borrow.
    if id == ID_CHIP_LAST {
        let current = {
            let Ok(s) = state.try_borrow() else { return };
            s.aimed_color()
        };
        if let Some(picked) = choose_color(hwnd, current)
            && let Ok(mut s) = state.try_borrow_mut()
        {
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
                let which = id - ID_FILL_FIRST;
                if let Some((_, fill)) = FILLS.get(which) {
                    // Clicking the mode that is already on turns it off,
                    // which is the plain colour.
                    let next =
                        if same_kind(*fill, s.session.fill()) { Fill::default() } else { *fill };
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
    refresh(hwnd);
}
