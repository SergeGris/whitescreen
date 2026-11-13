//! The Windows 11 look: colour, type and window chrome.
//!
//! Win32 gives an app a grey dialog face and a 1990s bitmap font unless it
//! says otherwise. Everything a modern Windows app actually looks like --
//! the Fluent palette, the Segoe UI Variable type ramp, the rounded window,
//! the dark title bar, the accent colour the user picked in Settings -- has
//! to be asked for, and this is where it is asked for.
//!
//! The numbers are WinUI's own tokens rather than invented ones, so a window
//! built from them sits next to Settings or the Store without looking like a
//! different decade. Three things are deliberately taken from the system
//! rather than hardcoded, because a user who changed them meant it:
//!
//! * light or dark, from the Personalize key `AppsUseLightTheme`;
//! * the accent colour, from DWM;
//! * the UI font *size*, from `SPI_GETNONCLIENTMETRICS`, so the
//!   accessibility text-size slider still does something here.
//!
//! High contrast wins over all of it: when it is on, the palette comes
//! straight from `GetSysColor` and none of the Fluent colours are used.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMWA_CAPTION_COLOR, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_TEXT_COLOR,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmGetColorizationColor, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, COLOR_BTNFACE, DEFAULT_CHARSET, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
    COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateFontIndirectW,
    CreatePen, CreateSolidBrush, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DeleteObject, DrawTextW, EnumFontFamiliesExW, FONT_WEIGHT, FW_NORMAL,
    FW_SEMIBOLD, GetDC, GetStockObject, GetSysColor, HBRUSH, HDC, HFONT, LOGFONTW, NULL_BRUSH,
    PS_SOLID, ReleaseDC, RoundRect, SelectObject, SetBkMode, SetTextColor, TEXTMETRICW,
    TRANSPARENT,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RegGetValueW,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::HiDpi::SystemParametersInfoForDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    NONCLIENTMETRICSW, SPI_GETHIGHCONTRAST, SPI_GETNONCLIENTMETRICS,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::{PCWSTR, w};

use whitescreen_core::Rgba;

use crate::paint::colorref;

/// What Windows ships as the accent colour, for when DWM will not say.
const DEFAULT_ACCENT: Rgba = Rgba::new(0, 120, 212);

const WHITE: Rgba = Rgba::new(255, 255, 255);
const BLACK: Rgba = Rgba::new(0, 0, 0);

/// Corner radii, in logical pixels, as WinUI uses them: 8 for a surface, 4
/// for a control, and half the box for a swatch.
pub const RADIUS_CARD: f32 = 8.0;
pub const RADIUS_CONTROL: f32 = 4.0;

/// The Fluent colour set, resolved for the user's current settings.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    /// Whether the system is in high contrast, in which case nothing here
    /// may be second-guessed.
    pub high_contrast: bool,
    /// The page the cards sit on.
    pub base: Rgba,
    pub card: Rgba,
    pub card_stroke: Rgba,
    pub control: Rgba,
    pub control_hover: Rgba,
    pub control_press: Rgba,
    pub control_stroke: Rgba,
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_press: Rgba,
    /// Text and glyphs drawn on top of `accent`.
    pub on_accent: Rgba,
    pub text: Rgba,
    pub text_secondary: Rgba,
    /// The keyboard focus ring.
    pub focus: Rgba,
}

impl Palette {
    /// Read the palette the user is asking for right now.
    pub fn current() -> Self {
        if high_contrast() {
            return Self::from_system_colors();
        }
        let accent = accent();
        if dark_mode() { Self::dark(accent) } else { Self::light(accent) }
    }

    fn light(accent: Rgba) -> Self {
        Self {
            dark: false,
            high_contrast: false,
            base: Rgba::new(0xF3, 0xF3, 0xF3),
            card: WHITE,
            card_stroke: Rgba::new(0xE5, 0xE5, 0xE5),
            control: Rgba::new(0xFD, 0xFD, 0xFD),
            control_hover: Rgba::new(0xF5, 0xF5, 0xF5),
            control_press: Rgba::new(0xEF, 0xEF, 0xEF),
            control_stroke: Rgba::new(0xD6, 0xD6, 0xD6),
            accent,
            // Fluent's hover and pressed accents are the accent at 90% and
            // 80% over the page, which is what mixing toward it comes to.
            accent_hover: blend(accent, Rgba::new(0xF3, 0xF3, 0xF3), 0.10),
            accent_press: blend(accent, Rgba::new(0xF3, 0xF3, 0xF3), 0.20),
            on_accent: on(accent),
            text: Rgba::new(0x1A, 0x1A, 0x1A),
            text_secondary: Rgba::new(0x5F, 0x5F, 0x5F),
            focus: Rgba::new(0x1A, 0x1A, 0x1A),
        }
    }

    fn dark(accent: Rgba) -> Self {
        // Dark mode uses a lighter accent than the one in Settings: the
        // stock blue at full strength does not carry enough contrast
        // against a near-black page. This is WinUI's AccentLight2, near
        // enough for a solid fill.
        let accent = blend(accent, WHITE, 0.30);
        Self {
            dark: true,
            high_contrast: false,
            base: Rgba::new(0x20, 0x20, 0x20),
            card: Rgba::new(0x2B, 0x2B, 0x2B),
            card_stroke: Rgba::new(0x38, 0x38, 0x38),
            control: Rgba::new(0x34, 0x34, 0x34),
            control_hover: Rgba::new(0x3D, 0x3D, 0x3D),
            control_press: Rgba::new(0x30, 0x30, 0x30),
            control_stroke: Rgba::new(0x45, 0x45, 0x45),
            accent,
            accent_hover: blend(accent, Rgba::new(0x20, 0x20, 0x20), 0.10),
            accent_press: blend(accent, Rgba::new(0x20, 0x20, 0x20), 0.20),
            on_accent: on(accent),
            text: WHITE,
            text_secondary: Rgba::new(0xC8, 0xC8, 0xC8),
            focus: WHITE,
        }
    }

    /// High contrast: every colour is the one the theme nominated, and the
    /// cards lose their fill so the borders carry the structure.
    fn from_system_colors() -> Self {
        let sys = |index| {
            let v = unsafe { GetSysColor(index) };
            Rgba::new((v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8)
        };
        let window = sys(COLOR_WINDOW);
        let text = sys(COLOR_WINDOWTEXT);
        let accent = sys(COLOR_HIGHLIGHT);
        Self {
            dark: window.luma() < 0.5,
            high_contrast: true,
            base: window,
            card: window,
            card_stroke: text,
            control: sys(COLOR_BTNFACE),
            control_hover: sys(COLOR_BTNFACE),
            control_press: sys(COLOR_BTNFACE),
            control_stroke: sys(COLOR_BTNTEXT),
            accent,
            accent_hover: sys(COLOR_HOTLIGHT),
            accent_press: sys(COLOR_HOTLIGHT),
            on_accent: sys(COLOR_HIGHLIGHTTEXT),
            text,
            text_secondary: sys(COLOR_GRAYTEXT),
            focus: text,
        }
    }

    /// The fill for a control in one of its three resting states.
    pub fn control_for(&self, hot: bool, pressed: bool) -> Rgba {
        match (pressed, hot) {
            (true, _) => self.control_press,
            (_, true) => self.control_hover,
            _ => self.control,
        }
    }

    /// The same, for the one accented control.
    pub fn accent_for(&self, hot: bool, pressed: bool) -> Rgba {
        match (pressed, hot) {
            (true, _) => self.accent_press,
            (_, true) => self.accent_hover,
            _ => self.accent,
        }
    }
}

/// Mix `t` of `b` into `a`.
pub fn blend(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Rgba::new(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b))
}

/// What reads on top of `color`.
fn on(color: Rgba) -> Rgba {
    if color.luma() > 0.6 { BLACK } else { WHITE }
}

/// Whether the user asked for dark apps.
///
/// `AppsUseLightTheme` is the value Explorer itself watches; a missing key
/// is an older Windows, which only had light.
fn dark_mode() -> bool {
    dword(
        w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
        w!("AppsUseLightTheme"),
    )
    .is_some_and(|v| v == 0)
}

/// The accent colour, as DWM has it.
fn accent() -> Rgba {
    // The DWM key is the accent the user picked; colorization is the same
    // colour blended for the title bar, which is close enough when the key
    // is absent.
    if let Some(abgr) = dword(w!(r"Software\Microsoft\Windows\DWM"), w!("AccentColor")) {
        return Rgba::new(
            (abgr & 0xFF) as u8,
            ((abgr >> 8) & 0xFF) as u8,
            ((abgr >> 16) & 0xFF) as u8,
        );
    }
    let mut argb = 0u32;
    let mut opaque = windows::core::BOOL(0);
    if unsafe { DwmGetColorizationColor(&raw mut argb, &raw mut opaque) }.is_ok() {
        return Rgba::new(
            ((argb >> 16) & 0xFF) as u8,
            ((argb >> 8) & 0xFF) as u8,
            (argb & 0xFF) as u8,
        );
    }
    DEFAULT_ACCENT
}

/// One `DWORD` out of `HKEY_CURRENT_USER`, or nothing.
fn dword(subkey: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let mut kind = REG_VALUE_TYPE::default();
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey,
            name,
            RRF_RT_REG_DWORD,
            Some(&raw mut kind),
            Some((&raw mut value).cast()),
            Some(&raw mut size),
        )
    };
    if status.is_ok() { Some(value) } else { None }
}

/// Whether a high contrast theme is in force.
fn high_contrast() -> bool {
    let mut hc = HIGHCONTRASTW { cbSize: size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            size_of::<HIGHCONTRASTW>() as u32,
            Some((&raw mut hc).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    ok && hc.dwFlags.contains(HCF_HIGHCONTRASTON)
}

/// Make the window frame itself modern: rounded, dark when the apps are
/// dark, and backed by Mica where Windows has it.
///
/// The client area is painted opaque in `base` rather than left transparent
/// for the Mica to show through. Showing it in the client area of a GDI
/// window means the old glass trick -- extend the frame and paint pure black
/// where the material should appear -- and a tool for judging colour on a
/// screen is the last place to risk a window that comes up black. The
/// caption still picks the material up, which is the part that reads as
/// Windows 11.
pub fn apply_window_chrome(hwnd: HWND, palette: &Palette) {
    let dark = windows::core::BOOL(i32::from(palette.dark));
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const dark).cast(),
            size_of_val(&dark) as u32,
        )
    };

    // Windows 11 rounds a window of this kind anyway; saying so means the
    // corner stays round if a future default changes.
    let corner = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corner).cast(),
            size_of_val(&corner) as u32,
        )
    };

    let backdrop = DWMSBT_MAINWINDOW;
    let mica = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&raw const backdrop).cast(),
            size_of_val(&backdrop) as u32,
        )
    }
    .is_ok();

    // No Mica -- Windows 10, or 11 before 22H2 -- so paint the caption to
    // match the page instead, which is the same seamless result by other
    // means. Under high contrast the theme owns the caption; leave it.
    if !mica && !palette.high_contrast {
        for (attribute, color) in
            [(DWMWA_CAPTION_COLOR, palette.base), (DWMWA_TEXT_COLOR, palette.text)]
        {
            let value = colorref(color);
            let _ = unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    attribute,
                    (&raw const value).cast(),
                    size_of_val(&value) as u32,
                )
            };
        }
    }
}

/// The type ramp.
///
/// WinUI's sizes, scaled by two things at once: the monitor's DPI, and
/// however far the user has moved the text-size slider away from the 12 px
/// that `lfMessageFont` is by default.
pub struct Fonts {
    /// Body: labels, buttons, list rows.
    pub body: HFONT,
    /// Body Strong: the heading on a card.
    pub strong: HFONT,
    /// Caption: the hint under a control.
    pub caption: HFONT,
    /// Subtitle: the one line at the top of the window.
    pub heading: HFONT,
    /// Segoe Fluent Icons, for the check and chevron glyphs.
    pub icon: HFONT,
}

impl Fonts {
    pub fn for_dpi(dpi: u32) -> Self {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        let size = size_of::<NONCLIENTMETRICSW>() as u32;
        // The per-DPI call is the one that reports the font at the size this
        // window needs; the plain one is the fallback, and then the scaling
        // below has to do the work instead.
        let dpi_aware = unsafe {
            SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                size,
                Some((&raw mut metrics).cast()),
                0,
                dpi,
            )
        }
        .is_ok();
        if !dpi_aware {
            let _ = unsafe {
                SystemParametersInfoW(
                    SPI_GETNONCLIENTMETRICS,
                    size,
                    Some((&raw mut metrics).cast()),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
            };
        }

        let base = metrics.lfMessageFont;
        let scale = dpi as f32 / 96.0;
        // 12 px is the stock message font. Whatever ratio the user has
        // moved it to is applied to the whole ramp.
        let reported = base.lfHeight.unsigned_abs() as f32;
        let stock = 12.0 * if dpi_aware { scale } else { 1.0 };
        let user = if reported > 0.0 { reported / stock.max(1.0) } else { 1.0 };
        let px = |points: f32| -((points * user * scale).round() as i32);

        let text_face = face(w!("Segoe UI Variable Text"));
        let display_face = face(w!("Segoe UI Variable Display"));
        let icon_face = face(w!("Segoe Fluent Icons")).or_else(|| face(w!("Segoe MDL2 Assets")));

        Self {
            body: font(&base, text_face, px(14.0), FW_NORMAL),
            strong: font(&base, text_face, px(14.0), FW_SEMIBOLD),
            caption: font(&base, text_face, px(12.0), FW_NORMAL),
            heading: font(&base, display_face.or(text_face), px(20.0), FW_SEMIBOLD),
            icon: font(&base, icon_face, px(12.0), FW_NORMAL),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for handle in [self.body, self.strong, self.caption, self.heading, self.icon] {
            if !handle.is_invalid() {
                let _ = unsafe { DeleteObject(handle.into()) };
            }
        }
    }
}

/// One font of the ramp, keeping the charset and quality of the system's.
fn font(base: &LOGFONTW, face: Option<PCWSTR>, height: i32, weight: FONT_WEIGHT) -> HFONT {
    let mut lf = *base;
    lf.lfHeight = height;
    lf.lfWidth = 0;
    lf.lfWeight = weight.0 as i32;
    lf.lfQuality = CLEARTYPE_QUALITY;
    if let Some(face) = face {
        lf.lfFaceName = [0; 32];
        let name = unsafe { face.as_wide() };
        let n = name.len().min(31);
        lf.lfFaceName[..n].copy_from_slice(&name[..n]);
    }
    unsafe { CreateFontIndirectW(&raw const lf) }
}

/// The best UI typeface installed, by name.
///
/// `display` asks for the one Windows 11 cuts for large text, which is what
/// a 42 point monitor name on a badge is. Both fall back to Segoe UI, which
/// every supported Windows has.
pub fn ui_face(display: bool) -> PCWSTR {
    let variable =
        if display { w!("Segoe UI Variable Display") } else { w!("Segoe UI Variable Text") };
    face(variable).unwrap_or(w!("Segoe UI"))
}

/// `Some(face)` when that typeface is installed.
///
/// Asking rather than assuming: Segoe UI Variable and Segoe Fluent Icons
/// are Windows 11 fonts, and on Windows 10 GDI would silently substitute
/// something that is not what the glyph codes below mean.
fn face(name: PCWSTR) -> Option<PCWSTR> {
    // The charset has to be DEFAULT_CHARSET or the enumeration is limited to
    // the fonts that cover one particular one, and the answer stops being
    // "is this face installed".
    let mut lf = LOGFONTW { lfCharSet: DEFAULT_CHARSET, ..Default::default() };
    let wide = unsafe { name.as_wide() };
    let n = wide.len().min(31);
    lf.lfFaceName[..n].copy_from_slice(&wide[..n]);

    let mut found = 0i32;
    let dc = unsafe { GetDC(None) };
    unsafe {
        EnumFontFamiliesExW(
            dc,
            &raw const lf,
            Some(found_face),
            LPARAM(&raw mut found as isize),
            0,
        );
        ReleaseDC(None, dc);
    }
    if found != 0 { Some(name) } else { None }
}

unsafe extern "system" fn found_face(
    _font: *const LOGFONTW,
    _metrics: *const TEXTMETRICW,
    _kind: u32,
    param: LPARAM,
) -> i32 {
    unsafe { *(param.0 as *mut i32) = 1 };
    0
}

/// The glyphs taken from the icon font, by code point.
pub mod glyph {
    /// A tick, for a checked box.
    pub const CHECK: &str = "\u{E73E}";
}

/// Fill a rounded rectangle, and optionally outline it.
///
/// GDI does not antialias this, so the corner of a card is a pixel or two
/// blunter than WinUI's. It is the one place a Direct2D device would look
/// better, and not worth one for four rectangles.
pub fn round_rect(dc: HDC, rect: RECT, radius: i32, fill: Option<Rgba>, stroke: Option<Rgba>) {
    let diameter = (radius * 2).max(0);
    unsafe {
        // RoundRect needs both a pen and a brush; a stroke-less shape gets
        // a pen of its own colour, and a fill-less one the null brush.
        let pen_color = stroke.or(fill).unwrap_or(Rgba::new(0, 0, 0));
        let pen = CreatePen(PS_SOLID, 1, colorref(pen_color));
        let brush = match fill {
            Some(color) => CreateSolidBrush(colorref(color)),
            None => HBRUSH(GetStockObject(NULL_BRUSH).0),
        };
        let old_pen = SelectObject(dc, pen.into());
        let old_brush = SelectObject(dc, brush.into());
        let _ = RoundRect(dc, rect.left, rect.top, rect.right, rect.bottom, diameter, diameter);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        let _ = DeleteObject(pen.into());
        if fill.is_some() {
            let _ = DeleteObject(brush.into());
        }
    }
}

/// Draw one line of text, transparently, in the given colour.
pub fn text(dc: HDC, rect: RECT, s: &str, font: HFONT, color: Rgba, format: DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return;
    }
    let mut bounds = rect;
    unsafe {
        let old = SelectObject(dc, font.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(colorref(color).0));
        DrawTextW(dc, &mut wide, &raw mut bounds, format | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

/// How wide one line of text comes out in `font`.
pub fn text_width(dc: HDC, s: &str, font: HFONT) -> i32 {
    let mut wide: Vec<u16> = s.encode_utf16().collect();
    if wide.is_empty() {
        return 0;
    }
    let mut bounds = RECT::default();
    unsafe {
        let old = SelectObject(dc, font.into());
        DrawTextW(
            dc,
            &mut wide,
            &raw mut bounds,
            DT_CALCRECT | DT_SINGLELINE | DT_LEFT | DT_NOPREFIX,
        );
        SelectObject(dc, old);
    }
    bounds.right - bounds.left
}

/// The keyboard focus ring: WinUI draws two, a thick one in the text colour
/// with a thin one inside it in the colour of whatever the control sits on,
/// so it reads on any fill. `surface` is that colour -- a card, or the page.
pub fn focus_ring(dc: HDC, rect: RECT, radius: i32, palette: &Palette, surface: Rgba) {
    let inset = |r: RECT, by: i32| RECT {
        left: r.left + by,
        top: r.top + by,
        right: r.right - by,
        bottom: r.bottom - by,
    };
    round_rect(dc, rect, radius, None, Some(palette.focus));
    round_rect(dc, inset(rect, 1), radius.saturating_sub(1), None, Some(palette.focus));
    round_rect(dc, inset(rect, 2), radius.saturating_sub(2), None, Some(surface));
}
