//! Persisted UI state.
//!
//! Everything the user chooses in the window -- the colour, the custom
//! colour, which chip is selected, which monitors are ticked and the cycle
//! interval -- is written to `$XDG_CONFIG_HOME/whitescreen/settings.ini` and
//! restored on the next launch.
//!
//! # Why a key file and not GSettings
//!
//! GSettings would be the idiomatic choice for a GTK application, but it
//! needs a compiled schema on `XDG_DATA_DIRS`, and `g_settings_new()` on a
//! missing schema does not fail -- it aborts the process. That turns a plain
//! `./_build/whitescreen` (the normal way to run this during development, and
//! the way anyone testing a build runs it) into a crash unless
//! `GSETTINGS_SCHEMA_DIR` is set by hand. A key file has no such install-time
//! dependency, costs nothing to read, and keeps the settings in a file the
//! user can edit or delete.

use std::path::PathBuf;

use gtk::{gdk, glib};

/// Group holding the scalar settings.
const GROUP: &str = "whitescreen";
/// Group holding the monitor selection; see `set_monitors()`.
const MONITORS: &str = "monitors";

const KEY_CHIP:     &str = "chip";
const KEY_CUSTOM:   &str = "custom-color";
const KEY_INTERVAL: &str = "cycle-interval";
const KEY_COUNT:    &str = "count";

/// Reader/writer for the settings file. Never fails to construct: a missing,
/// unreadable or corrupt file simply yields no values, and every getter
/// returns `None` so the caller applies its own default.
pub struct Settings {
    file: glib::KeyFile,
    path: PathBuf,
}

impl Settings {
    pub fn load() -> Self {
        let path = glib::user_config_dir().join("whitescreen").join("settings.ini");
        let file = glib::KeyFile::new();
        // A first run has no file at all, which is not worth reporting.
        let _ = file.load_from_file(&path, glib::KeyFileFlags::NONE);
        Self { file, path }
    }

    /// Write the file out, creating `~/.config/whitescreen` if needed.
    ///
    /// Failures are reported once on stderr and otherwise ignored: losing the
    /// saved colour is not a reason to interrupt someone who is using the app.
    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("whitescreen: cannot create {}: {e}", dir.display());
                return;
            }
        }
        if let Err(e) = self.file.save_to_file(&self.path) {
            eprintln!("whitescreen: cannot write {}: {e}", self.path.display());
        }
    }

    // ── Colours ──────────────────────────────────────────────────────────

    /// Stored as `#RRGGBB`. `gdk_rgba_parse()` reads that back, and writing
    /// hex rather than `rgb(...)` keeps the file legible to a human editing it.
    fn color(&self, key: &str) -> Option<gdk::RGBA> {
        let s = self.file.string(GROUP, key).ok()?;
        gdk::RGBA::parse(s.as_str()).ok()
    }

    fn set_color(&self, key: &str, rgba: gdk::RGBA) {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.file.set_string(
            GROUP,
            key,
            &format!("#{:02X}{:02X}{:02X}", byte(rgba.red()), byte(rgba.green()), byte(rgba.blue())),
        );
    }

    pub fn custom_color(&self) -> Option<gdk::RGBA> { self.color(KEY_CUSTOM) }
    pub fn set_custom_color(&self, rgba: gdk::RGBA) { self.set_color(KEY_CUSTOM, rgba); }

    // ── Selected chip ────────────────────────────────────────────────────

    /// Name of the selected colour chip: a preset name, or `Custom`.
    pub fn chip(&self) -> Option<String> {
        self.file.string(GROUP, KEY_CHIP).ok().map(|s| s.to_string())
    }

    pub fn set_chip(&self, name: &str) {
        self.file.set_string(GROUP, KEY_CHIP, name);
    }

    // ── Cycle ────────────────────────────────────────────────────────────

    pub fn cycle_interval(&self) -> Option<f64> {
        self.file.double(GROUP, KEY_INTERVAL).ok()
    }

    pub fn set_cycle_interval(&self, secs: f64) {
        self.file.set_double(GROUP, KEY_INTERVAL, secs);
    }

    // ── Monitor selection ────────────────────────────────────────────────

    /// The selected monitor keys, in no particular order.
    pub fn monitors(&self) -> Vec<String> {
        let n = self.file.integer(MONITORS, KEY_COUNT).unwrap_or(0).max(0);
        (0..n)
            .filter_map(|i| self.file.string(MONITORS, &i.to_string()).ok())
            .map(|s| s.to_string())
            .collect()
    }

    /// Stored one key per numbered entry rather than as a single list.
    ///
    /// A monitor key is a connector name or, failing that, raw EDID text
    /// (see `monitor_ident()`), so it can contain a semicolon -- the very
    /// character `g_key_file_set_string_list()` uses as its separator, and
    /// which glib's Rust bindings expose no escaping helper for. Numbered
    /// single strings sidestep the escaping question entirely.
    pub fn set_monitors(&self, keys: &[String]) {
        // Drop the whole group first: writing fewer keys than last time would
        // otherwise leave the tail entries behind for `count` to skip today
        // and a future reader to trip over.
        let _ = self.file.remove_group(MONITORS);
        self.file.set_integer(MONITORS, KEY_COUNT, keys.len() as i32);
        for (i, key) in keys.iter().enumerate() {
            self.file.set_string(MONITORS, &i.to_string(), key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an in-memory Settings pointed at a path that is never written.
    fn scratch() -> Settings {
        Settings { file: glib::KeyFile::new(), path: PathBuf::from("/nonexistent/settings.ini") }
    }

    #[test]
    fn missing_values_read_back_as_none() {
        let s = scratch();
        assert_eq!(s.chip(), None);
        assert_eq!(s.custom_color(), None);
        assert_eq!(s.cycle_interval(), None);
        assert!(s.monitors().is_empty());
    }

    #[test]
    fn colors_survive_a_round_trip() {
        let s = scratch();
        let cyan = gdk::RGBA::new(0.0, 1.0, 1.0, 1.0);
        s.set_custom_color(cyan);
        assert_eq!(s.custom_color(), Some(cyan));
    }

    #[test]
    fn monitor_keys_with_awkward_characters_survive() {
        let s = scratch();
        // A connector-less monitor is keyed by EDID text joined with '|', and
        // ';' is exactly what a key-file string list would split on.
        let keys = vec![
            "DP-1".to_string(),
            "Acme;Inc|Model X|desc".to_string(),
            "with\\backslash".to_string(),
        ];
        s.set_monitors(&keys);
        assert_eq!(s.monitors(), keys);
    }

    #[test]
    fn shrinking_the_selection_leaves_no_stale_entries() {
        let s = scratch();
        s.set_monitors(&["DP-1".to_string(), "DP-2".to_string(), "DP-3".to_string()]);
        s.set_monitors(&["DP-9".to_string()]);
        assert_eq!(s.monitors(), vec!["DP-9".to_string()]);
    }
}
