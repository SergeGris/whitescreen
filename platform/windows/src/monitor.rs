//! Which screens there are, and what to call them.
//!
//! The names matter as much as the geometry: `--monitor` takes a pattern and
//! matches it against these, so they have to be stable and recognisable. On
//! Windows the stable one is the display device (`\\.\DISPLAY1`), and the
//! recognisable one is the adapter's own string ("Dell U2415"), so a monitor
//! carries both.

use windows::Win32::Foundation::{LPARAM, RECT, TRUE};
use windows::core::BOOL;
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW, EnumDisplayMonitors,
    EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

use whitescreen_core::glob;

/// One attached screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// What `--monitor` matches and the settings file stores: the display
    /// device name, `\\.\DISPLAY1` and friends. Stable across reboots and
    /// unique, which is what a key has to be.
    pub key: String,
    /// What a person would call it, from the adapter: "Dell U2415". Empty
    /// when Windows does not say.
    pub name: String,
    /// Position and size in the virtual desktop, in physical pixels.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Scale factor, 1.0 at 96 dpi. The overlay is sized in physical pixels,
    /// so this is only needed where a length is specified in logical ones --
    /// the grid's pitch, for instance.
    pub scale: f32,
    /// Refresh rate in millihertz, to match what the GTK build prints. Zero
    /// when Windows does not say.
    pub refresh_mhz: u32,
    /// Whether Windows considers this the primary display.
    pub primary: bool,
}

impl Monitor {
    /// The rectangle, for the window that will cover it.
    pub(crate) fn rect(&self) -> RECT {
        RECT {
            left: self.x,
            top: self.y,
            right: self.x + self.width,
            bottom: self.y + self.height,
        }
    }
}

/// Every attached screen, in the order Windows enumerates them.
pub fn monitors() -> Vec<Monitor> {
    let mut found: Vec<Monitor> = Vec::new();
    // The closure cannot be passed across the FFI boundary, so the vector
    // travels as the LPARAM the callback is handed back.
    let data = LPARAM(&raw mut found as isize);
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), data);
    }
    found
}

unsafe extern "system" fn collect(
    monitor: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // Safety: the pointer is the one monitors() just passed, and
    // EnumDisplayMonitors calls back synchronously on this thread.
    let found = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };

    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let ok = unsafe {
        GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>())
    };
    if !ok.as_bool() {
        // One screen that will not describe itself is no reason to abandon
        // the others.
        return TRUE;
    }

    let rect = info.monitorInfo.rcMonitor;
    let key = wide_to_string(&info.szDevice);
    found.push(Monitor {
        name: friendly_name(&info.szDevice),
        key,
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
        scale: dpi_scale(monitor),
        refresh_mhz: refresh_mhz(&info.szDevice),
        // MONITORINFOF_PRIMARY, the only flag the structure defines.
        primary: info.monitorInfo.dwFlags & 1 != 0,
    });
    TRUE
}

/// The adapter's name for the display, or empty when it has none to give.
fn friendly_name(device: &[u16; 32]) -> String {
    let mut adapter = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    let ok = unsafe {
        EnumDisplayDevicesW(
            windows::core::PCWSTR(device.as_ptr()),
            0,
            &raw mut adapter,
            0,
        )
    };
    if ok.as_bool() {
        wide_to_string(&adapter.DeviceString)
    } else {
        String::new()
    }
}

/// The mode's refresh rate, in millihertz.
///
/// Windows reports whole hertz; the GTK build prints millihertz because
/// that is what GDK reports, and the two listings should not need different
/// parsing.
fn refresh_mhz(device: &[u16; 32]) -> u32 {
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe {
        EnumDisplaySettingsW(
            windows::core::PCWSTR(device.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &raw mut mode,
        )
    };
    if ok.as_bool() { mode.dmDisplayFrequency.saturating_mul(1000) } else { 0 }
}

/// How much larger than 96 dpi this screen is.
fn dpi_scale(monitor: HMONITOR) -> f32 {
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    // Failure here means "assume 96", which is what the variables already
    // say; a wrong scale costs a hairline's width, not correctness.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) };
    dpi_x as f32 / 96.0
}

/// A NUL-terminated fixed-size wide field as a `String`.
fn wide_to_string(field: &[u16]) -> String {
    let len = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf16_lossy(&field[..len])
}

/// The monitors matching these shell-style patterns, and the patterns that
/// matched nothing.
///
/// The same bargain the GTK build makes, through the same matcher: a pattern
/// is matched against both the device name and the friendly one, because a
/// person reading `--list-monitors` will try whichever of the two they
/// recognise.
pub fn select_matching<'a>(
    monitors: &'a [Monitor],
    patterns: &[String],
) -> (Vec<&'a Monitor>, Vec<String>) {
    let mut hits: Vec<&Monitor> = Vec::new();
    let mut unmatched = Vec::new();

    for pattern in patterns {
        let matched: Vec<&Monitor> = monitors
            .iter()
            .filter(|m| glob::matches(pattern, &m.key) || glob::matches(pattern, &m.name))
            .collect();

        if matched.is_empty() {
            // A plain name may be a screen that is not plugged in, which the
            // caller keeps; a pattern that matches nothing is a typo.
            if glob::is_pattern(pattern) {
                unmatched.push(pattern.clone());
            }
            continue;
        }
        for m in matched {
            if !hits.iter().any(|h| h.key == m.key) {
                hits.push(m);
            }
        }
    }
    (hits, unmatched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(key: &str, name: &str) -> Monitor {
        Monitor {
            key: key.to_string(),
            name: name.to_string(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            refresh_mhz: 60_000,
            primary: false,
        }
    }

    #[test]
    fn a_pattern_matches_the_device_or_the_friendly_name() {
        let all = vec![screen(r"\\.\DISPLAY1", "Dell U2415"), screen(r"\\.\DISPLAY2", "LG HDR 4K")];

        let (hits, bad) = select_matching(&all, &[r"*DISPLAY1".to_string()]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Dell U2415");
        assert!(bad.is_empty());

        let (hits, _) = select_matching(&all, &["Dell*".to_string()]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, r"\\.\DISPLAY1");
    }

    #[test]
    fn a_monitor_named_twice_is_listed_once() {
        let all = vec![screen(r"\\.\DISPLAY1", "Dell U2415")];
        let (hits, _) = select_matching(&all, &["Dell*".to_string(), "*DISPLAY1".to_string()]);
        assert_eq!(hits.len(), 1, "two patterns hitting one screen must not double it");
    }

    #[test]
    fn a_pattern_matching_nothing_is_reported_and_a_plain_name_is_not() {
        let all = vec![screen(r"\\.\DISPLAY1", "Dell U2415")];

        let (hits, bad) = select_matching(&all, &["LG*".to_string()]);
        assert!(hits.is_empty());
        assert_eq!(bad, vec!["LG*".to_string()]);

        // A plain name is kept for a screen that may yet be plugged in.
        let (hits, bad) = select_matching(&all, &[r"\\.\DISPLAY9".to_string()]);
        assert!(hits.is_empty());
        assert!(bad.is_empty(), "a plain name is not a typo");
    }

    #[test]
    fn the_rect_is_the_monitor_in_the_virtual_desktop() {
        let mut m = screen(r"\\.\DISPLAY2", "LG");
        m.x = 1920;
        m.y = -200;
        let r = m.rect();
        assert_eq!((r.left, r.top, r.right, r.bottom), (1920, -200, 3840, 880));
    }
}
