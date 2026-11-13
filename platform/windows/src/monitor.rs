//! Which screens there are, and what to call them.
//!
//! The names matter as much as the geometry: `--monitor` takes a pattern and
//! matches it against these, so they have to be stable *and* recognisable,
//! and on Windows those are two different strings. The stable one is the
//! display device, `\\.\DISPLAY1`, which is what the settings file stores.
//! The recognisable one has to be asked for properly.
//!
//! `EnumDisplayDevices` will not give it. Asked about a display device it
//! describes the monitor's *driver* -- "Generic PnP Monitor", the same answer
//! for every screen on the machine -- and asked about an adapter it describes
//! the GPU. What a person would recognise is the name the monitor reports in
//! its EDID, and that comes from the display configuration: see
//! `descriptions`, which also says which connector the screen arrived on.

use std::collections::HashMap;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_COMPONENT_VIDEO,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_COMPOSITE_VIDEO,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_USB_TUNNEL, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DVI,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HD15, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_LVDS, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_MIRACAST,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_SDI, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_SDTVDONGLE,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_SVIDEO, DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED,
    DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EXTERNAL, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

use whitescreen_core::glob;

/// One attached screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// What `--monitor` matches and the settings file stores: the display
    /// device name, `\\.\DISPLAY1` and friends. Stable across reboots and
    /// unique, which is what a key has to be.
    pub key: String,
    /// What a person would call it, as the monitor reports itself: "DELL
    /// U2415". Empty when it reports nothing, which a virtual display or a
    /// capture dongle may well not.
    pub name: String,
    /// Which socket it arrived on: "HDMI", "DisplayPort", "Internal".
    /// Empty when Windows will not say.
    pub connector: String,
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
    /// What to call this screen to a person.
    ///
    /// The name the monitor reports, falling back to the device name: a
    /// screen that will not say what it is still has to be pickable from a
    /// list, and `\\.\DISPLAY2` at least distinguishes it.
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.key } else { &self.name }
    }

    /// The line under the label: where it is plugged in and what mode it is
    /// in. "DisplayPort · 2560×1440", or just the mode when Windows will
    /// not say which socket.
    pub fn detail(&self) -> String {
        let mode = format!("{}×{}", self.width, self.height);
        if self.connector.is_empty() { mode } else { format!("{} · {mode}", self.connector) }
    }

    /// The rectangle, for the window that will cover it.
    pub(crate) fn rect(&self) -> RECT {
        RECT { left: self.x, top: self.y, right: self.x + self.width, bottom: self.y + self.height }
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

    // Asked once for the whole set rather than once per screen: the display
    // configuration is one query, and it is keyed by the same device name
    // the enumeration just handed back.
    let described = descriptions();
    for screen in &mut found {
        if let Some((name, connector)) = described.get(&screen.key) {
            screen.name = name.clone();
            screen.connector = connector.clone();
        }
    }
    found
}

/// What every active screen reports itself to be, by display device name.
///
/// Two calls per screen into the display configuration: the source says
/// which `\\.\DISPLAYn` a path drives, and the target says what is on the
/// end of it. A screen missing from the result keeps whatever it had, which
/// is its device name -- better than claiming a name that is not its own.
fn descriptions() -> HashMap<String, (String, String)> {
    // Asking how large the configuration is and then asking for it are two
    // calls, and it can change in between -- which is precisely what is
    // happening when a screen is being plugged in, and this runs on every
    // display change. Windows says so with ERROR_INSUFFICIENT_BUFFER, and
    // the answer is to ask again. A few attempts is plenty; after that the
    // device names stand, which is no worse than before they were asked for.
    for _ in 0..4 {
        match paths_and_targets() {
            Ok(described) => return described,
            Err(true) => continue,
            Err(false) => break,
        }
    }
    HashMap::new()
}

/// One attempt at reading the display configuration.
///
/// `Err(true)` when it changed under us and asking again is worth it.
fn paths_and_targets() -> Result<HashMap<String, (String, String)>, bool> {
    let mut described = HashMap::new();

    let mut paths = 0u32;
    let mut modes = 0u32;
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &raw mut paths, &raw mut modes) }
        != ERROR_SUCCESS
    {
        return Err(false);
    }
    let mut path_buffer = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
    let mut mode_buffer = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
    let queried = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut paths,
            path_buffer.as_mut_ptr(),
            &raw mut modes,
            mode_buffer.as_mut_ptr(),
            None,
        )
    };
    if queried != ERROR_SUCCESS {
        return Err(queried == ERROR_INSUFFICIENT_BUFFER);
    }

    // The counts come back reduced to what was actually written.
    for path in path_buffer.iter().take(paths as usize) {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        // Safety: each request is a packet whose header says which kind it
        // is and how large it is, which is the whole of this call's
        // contract.
        if unsafe { DisplayConfigGetDeviceInfo(&raw mut source.header) } != ERROR_SUCCESS.0 as i32 {
            continue;
        }

        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };
        if unsafe { DisplayConfigGetDeviceInfo(&raw mut target.header) } != ERROR_SUCCESS.0 as i32 {
            continue;
        }

        described.insert(
            wide_to_string(&source.viewGdiDeviceName),
            (
                wide_to_string(&target.monitorFriendlyDeviceName),
                connector(target.outputTechnology).to_string(),
            ),
        );
    }
    Ok(described)
}

/// What to call a socket, in the words on the back of a monitor.
fn connector(technology: DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY) -> &'static str {
    match technology {
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HD15 => "VGA",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DVI => "DVI",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_HDMI => "HDMI",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EXTERNAL => "DisplayPort",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_USB_TUNNEL => "DisplayPort over USB",
        // The two names for the panel wired to the lid of a laptop.
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_DISPLAYPORT_EMBEDDED
        | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL => "Internal",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_LVDS => "LVDS",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EXTERNAL => "UDI",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_UDI_EMBEDDED => "UDI (internal)",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_SDI => "SDI",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_SVIDEO => "S-Video",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_COMPOSITE_VIDEO => "Composite",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_COMPONENT_VIDEO => "Component",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_SDTVDONGLE => "TV dongle",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_MIRACAST => "Miracast",
        DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED
        | DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_VIRTUAL => "Virtual",
        // Including OTHER, which says no more than saying nothing would.
        _ => "",
    }
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
    let ok = unsafe { GetMonitorInfoW(monitor, (&raw mut info).cast::<MONITORINFO>()) };
    if !ok.as_bool() {
        // One screen that will not describe itself is no reason to abandon
        // the others.
        return TRUE;
    }

    let rect = info.monitorInfo.rcMonitor;
    let key = wide_to_string(&info.szDevice);
    found.push(Monitor {
        // Both are filled in by `monitors()`, which asks the display
        // configuration once for every screen at a time.
        name: String::new(),
        connector: String::new(),
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

/// The mode's refresh rate, in millihertz.
///
/// Windows reports whole hertz; the GTK build prints millihertz because
/// that is what GDK reports, and the two listings should not need different
/// parsing.
fn refresh_mhz(device: &[u16; 32]) -> u32 {
    let mut mode = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
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
            connector: "DisplayPort".to_string(),
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
