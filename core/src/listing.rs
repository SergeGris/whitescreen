//! The monitor list, in the one shape both builds promise.
//!
//! `--list-monitors` exists to be read by something else: a script that
//! picks a screen, or a person pasting a name straight back into
//! `--monitor`. That makes its output a promise rather than a convenience,
//! and a promise kept in two places is one that drifts -- as it had, the
//! Windows build having grown a `primary` field the GTK build never
//! printed.
//!
//! So the shape lives here and neither front end writes it. What each of
//! them does is answer the questions in `Screen`, which is all the part
//! that genuinely differs between a `gdk::Monitor` and an `HMONITOR`.

use crate::text::json_string;

/// One attached screen, as the listing needs it.
pub struct Screen<'a> {
    /// What `--monitor` matches: the connector or device name.
    pub key: &'a str,
    /// What a person would call it, from the EDID or the adapter. Empty
    /// when the platform does not say.
    pub model: &'a str,
    pub width: i32,
    pub height: i32,
    /// 1.0 at 96 dpi.
    pub scale: f64,
    /// Refresh rate in millihertz; zero when the platform does not say.
    pub refresh_mhz: i64,
    /// Whether the desktop treats this as the primary screen.
    ///
    /// `None` where the platform will not say, which is not the same as
    /// `false`: GDK 4 removed the question, so on Wayland nobody knows.
    pub primary: Option<bool>,
}

/// The plain listing: one screen a line, name first so it can be cut out.
pub fn plain(screens: &[Screen]) -> String {
    let mut out = String::new();
    for screen in screens {
        out.push_str(&format!("{}\t{} × {}\n", screen.key, screen.width, screen.height));
    }
    out
}

/// The same, as JSON.
///
/// Hand-rolled rather than pulling in a serializer for seven fields. The
/// only values that are not numbers are the key and the model, and both come
/// from the display server, so both are escaped.
pub fn json(screens: &[Screen]) -> String {
    let mut out = String::from("[\n");
    for (i, screen) in screens.iter().enumerate() {
        let comma = if i + 1 < screens.len() { "," } else { "" };
        let primary = match screen.primary {
            Some(true) => "true",
            Some(false) => "false",
            // Null rather than a missing key: a reader should not have to
            // know which platform wrote this to know the field exists.
            None => "null",
        };
        out.push_str(&format!(
            "  {{\"name\": {}, \"width\": {}, \"height\": {}, \
             \"scale\": {}, \"refresh_mhz\": {}, \"model\": {}, \"primary\": {primary}}}{comma}\n",
            json_string(screen.key),
            screen.width,
            screen.height,
            screen.scale,
            screen.refresh_mhz,
            json_string(screen.model),
        ));
    }
    out.push_str("]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens() -> Vec<Screen<'static>> {
        vec![
            Screen {
                key: "DP-1",
                model: "Dell U2415",
                width: 1920,
                height: 1200,
                scale: 1.0,
                refresh_mhz: 59950,
                primary: Some(true),
            },
            Screen {
                key: r"\\.\DISPLAY2",
                model: "",
                width: 2560,
                height: 1440,
                scale: 1.5,
                refresh_mhz: 0,
                primary: None,
            },
        ]
    }

    #[test]
    fn the_plain_listing_is_a_name_and_a_size_a_line() {
        assert_eq!(
            plain(&screens()),
            "DP-1\t1920 × 1200\n\\\\.\\DISPLAY2\t2560 × 1440\n",
        );
    }

    /// The exact bytes, because something is parsing them.
    #[test]
    fn the_json_listing_has_every_field_on_every_platform() {
        assert_eq!(
            json(&screens()),
            concat!(
                "[\n",
                "  {\"name\": \"DP-1\", \"width\": 1920, \"height\": 1200, \"scale\": 1, ",
                "\"refresh_mhz\": 59950, \"model\": \"Dell U2415\", \"primary\": true},\n",
                "  {\"name\": \"\\\\\\\\.\\\\DISPLAY2\", \"width\": 2560, \"height\": 1440, ",
                "\"scale\": 1.5, \"refresh_mhz\": 0, \"model\": \"\", \"primary\": null}\n",
                "]\n",
            ),
        );
    }

    #[test]
    fn nothing_attached_is_still_valid_json() {
        assert_eq!(json(&[]), "[\n]\n");
        assert_eq!(plain(&[]), "");
    }
}
