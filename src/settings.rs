//! Persisted UI state.
//!
//! Everything the user chooses in the window -- the colour, the custom
//! colour, the brightness, which monitors are ticked and what colour each of
//! them gets, the fill mode, the auto-hide delay, and the cycle queue and its
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
//!
//! # What is deliberately not saved
//!
//! Two switches: **Cycle** and **Identify**.
//!
//! The rule is that anything which changes the colour you asked for is
//! remembered, and the two switches that would make something *extra* appear
//! the next time you press Show are not: a sequence flashing across every
//! selected screen, and a name badge on each of them. Restoring either would
//! spring it on someone who had forgotten they left it on, with nothing on
//! screen to say why. Both stay on for the session, which is as far as the
//! surprise can reach.
//!
//! Auto-hide is saved even though it also acts on its own, because all it
//! ever does is take away what you asked for, after a delay you chose -- a
//! safety net rather than a surprise.

use std::collections::HashMap;
use std::path::PathBuf;

use gtk::{gdk, glib};

use crate::color;

/// Group holding the scalar settings.
const GROUP: &str = "whitescreen";
/// Group holding the monitor selection; see `set_monitors()`.
const MONITORS: &str = "monitors";
/// Group holding the cycle queue, one colour per numbered entry.
const QUEUE: &str = "queue";
/// Group holding the per-monitor colours, as parallel numbered entries.
const MONITOR_COLORS: &str = "monitor-colors";

const KEY_COLOR:      &str = "color";
const KEY_CUSTOM:     &str = "custom-color";
const KEY_BRIGHTNESS: &str = "brightness";
const KEY_INTERVAL:   &str = "cycle-interval";
const KEY_PATTERN:    &str = "pattern";
const KEY_SIDE:       &str = "region-side";
const KEY_SECOND:     &str = "second-color";
const KEY_AUTO_HIDE:  &str = "auto-hide";
const KEY_PER_MON:    &str = "per-monitor-colors";
const KEY_COUNT:      &str = "count";

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
        if let Some(dir) = self.path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("whitescreen: cannot create {}: {e}", dir.display());
            return;
        }
        if let Err(e) = self.file.save_to_file(&self.path) {
            eprintln!("whitescreen: cannot write {}: {e}", self.path.display());
        }
    }

    // ── Colours ──────────────────────────────────────────────────────────

    /// Stored as `#RRGGBB`. `gdk_rgba_parse()` reads that back, and writing
    /// hex rather than `rgb(...)` keeps the file legible to a human editing it.
    fn read_color(&self, group: &str, key: &str) -> Option<gdk::RGBA> {
        let s = self.file.string(group, key).ok()?;
        gdk::RGBA::parse(s.as_str()).ok()
    }

    fn write_color(&self, group: &str, key: &str, rgba: gdk::RGBA) {
        self.file.set_string(group, key, &color::hex(rgba));
    }

    /// The colour that was on the screens.
    pub fn color(&self) -> Option<gdk::RGBA> { self.read_color(GROUP, KEY_COLOR) }
    pub fn set_color(&self, rgba: gdk::RGBA) { self.write_color(GROUP, KEY_COLOR, rgba); }

    /// The colour behind the Custom chip, which is not necessarily the one
    /// showing: the chip remembers its colour while another chip is marked.
    pub fn custom_color(&self) -> Option<gdk::RGBA> { self.read_color(GROUP, KEY_CUSTOM) }
    pub fn set_custom_color(&self, rgba: gdk::RGBA) { self.write_color(GROUP, KEY_CUSTOM, rgba); }

    /// Colour for the unfilled half in region mode, or `None` for transparent.
    pub fn second_color(&self) -> Option<gdk::RGBA> { self.read_color(GROUP, KEY_SECOND) }

    pub fn set_second_color(&self, rgba: Option<gdk::RGBA>) {
        match rgba {
            Some(rgba) => self.write_color(GROUP, KEY_SECOND, rgba),
            // Removed rather than written empty, so that "no second colour"
            // and "a second colour this version cannot parse" stay distinct.
            None => { let _ = self.file.remove_key(GROUP, KEY_SECOND); }
        }
    }

    // ── Scalars ──────────────────────────────────────────────────────────

    pub fn brightness(&self) -> Option<f64> {
        self.file.double(GROUP, KEY_BRIGHTNESS).ok()
    }

    pub fn set_brightness(&self, fraction: f64) {
        self.file.set_double(GROUP, KEY_BRIGHTNESS, fraction);
    }

    pub fn auto_hide(&self) -> Option<u32> {
        // Negative minutes mean nothing; a hand-edited file saying -5 reads
        // as "no value" rather than as an enormous unsigned delay.
        self.file.integer(GROUP, KEY_AUTO_HIDE).ok()?.try_into().ok()
    }

    pub fn set_auto_hide(&self, minutes: u32) {
        self.file.set_integer(GROUP, KEY_AUTO_HIDE, minutes.min(i32::MAX as u32) as i32);
    }

    pub fn cycle_interval(&self) -> Option<f64> {
        self.file.double(GROUP, KEY_INTERVAL).ok()
    }

    pub fn set_cycle_interval(&self, secs: f64) {
        self.file.set_double(GROUP, KEY_INTERVAL, secs);
    }

    // ── Fill mode ────────────────────────────────────────────────────────
    //
    // Stored as the mode's own key rather than as an index, so that adding a
    // pattern in a later version cannot renumber what everyone has saved.

    pub fn pattern(&self) -> Option<String> {
        self.file.string(GROUP, KEY_PATTERN).ok().map(|s| s.to_string())
    }

    pub fn set_pattern(&self, key: &str) {
        self.file.set_string(GROUP, KEY_PATTERN, key);
    }

    pub fn region_side(&self) -> Option<String> {
        self.file.string(GROUP, KEY_SIDE).ok().map(|s| s.to_string())
    }

    pub fn set_region_side(&self, key: &str) {
        self.file.set_string(GROUP, KEY_SIDE, key);
    }

    // ── Monitor selection ────────────────────────────────────────────────

    /// The selected monitor keys, in no particular order.
    pub fn monitors(&self) -> Vec<String> {
        self.numbered(MONITORS, "")
    }

    /// Stored one key per numbered entry rather than as a single list.
    ///
    /// A monitor key is a connector name or, failing that, raw EDID text
    /// (see `monitor_ident()`), so it can contain a semicolon -- the very
    /// character `g_key_file_set_string_list()` uses as its separator, and
    /// which glib's Rust bindings expose no escaping helper for. Numbered
    /// single strings sidestep the escaping question entirely.
    pub fn set_monitors(&self, keys: &[String]) {
        self.set_numbered(MONITORS, "", keys);
    }

    // ── Per-monitor colours ──────────────────────────────────────────────

    pub fn per_monitor_colors(&self) -> bool {
        self.file.boolean(GROUP, KEY_PER_MON).unwrap_or(false)
    }

    pub fn set_per_monitor_colors(&self, on: bool) {
        self.file.set_boolean(GROUP, KEY_PER_MON, on);
    }

    /// The colour each named monitor was given.
    ///
    /// Parallel numbered entries, for the same reason the selection is
    /// numbered: a monitor key may contain any character at all, including
    /// the `=` that a key-file key cannot.
    pub fn monitor_colors(&self) -> HashMap<String, gdk::RGBA> {
        let keys = self.numbered(MONITOR_COLORS, "key");
        keys.into_iter()
            .enumerate()
            .filter_map(|(i, key)| {
                let rgba = self.read_color(MONITOR_COLORS, &format!("color{i}"))?;
                Some((key, rgba))
            })
            .collect()
    }

    pub fn set_monitor_colors(&self, colors: &[(String, gdk::RGBA)]) {
        let names: Vec<String> = colors.iter().map(|(k, _)| k.clone()).collect();
        self.set_numbered(MONITOR_COLORS, "key", &names);
        for (i, (_, rgba)) in colors.iter().enumerate() {
            self.write_color(MONITOR_COLORS, &format!("color{i}"), *rgba);
        }
    }

    // ── Cycle queue ──────────────────────────────────────────────────────

    pub fn queue(&self) -> Vec<gdk::RGBA> {
        self.numbered(QUEUE, "")
            .iter()
            .filter_map(|s| gdk::RGBA::parse(s).ok())
            .collect()
    }

    pub fn set_queue(&self, colors: &[gdk::RGBA]) {
        let hex: Vec<String> = colors.iter().map(|c| color::hex(*c)).collect();
        self.set_numbered(QUEUE, "", &hex);
    }

    // ── Numbered entries ─────────────────────────────────────────────────

    fn numbered(&self, group: &str, prefix: &str) -> Vec<String> {
        let n = self.file.integer(group, KEY_COUNT).unwrap_or(0).max(0);
        (0..n)
            .filter_map(|i| self.file.string(group, &format!("{prefix}{i}")).ok())
            .map(|s| s.to_string())
            .collect()
    }

    fn set_numbered(&self, group: &str, prefix: &str, values: &[String]) {
        // Drop the whole group first: writing fewer entries than last time
        // would otherwise leave the tail behind for `count` to skip today and
        // a future reader to trip over.
        let _ = self.file.remove_group(group);
        self.file.set_integer(group, KEY_COUNT, values.len() as i32);
        for (i, value) in values.iter().enumerate() {
            self.file.set_string(group, &format!("{prefix}{i}"), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYAN: gdk::RGBA = gdk::RGBA::new(0.0, 1.0, 1.0, 1.0);

    /// Build an in-memory Settings pointed at a path that is never written.
    fn scratch() -> Settings {
        Settings { file: glib::KeyFile::new(), path: PathBuf::from("/nonexistent/settings.ini") }
    }

    #[test]
    fn missing_values_read_back_as_none() {
        let s = scratch();
        assert_eq!(s.color(), None);
        assert_eq!(s.custom_color(), None);
        assert_eq!(s.second_color(), None);
        assert_eq!(s.cycle_interval(), None);
        assert_eq!(s.brightness(), None);
        assert_eq!(s.auto_hide(), None);
        assert_eq!(s.pattern(), None);
        assert_eq!(s.region_side(), None);
        assert!(!s.per_monitor_colors());
        assert!(s.monitors().is_empty());
        assert!(s.queue().is_empty());
        assert!(s.monitor_colors().is_empty());
    }

    #[test]
    fn colors_survive_a_round_trip() {
        let s = scratch();
        s.set_custom_color(CYAN);
        s.set_color(gdk::RGBA::RED);
        assert_eq!(s.custom_color(), Some(CYAN));
        assert_eq!(s.color(), Some(gdk::RGBA::RED));
    }

    #[test]
    fn scalars_survive_a_round_trip() {
        let s = scratch();
        s.set_brightness(0.42);
        s.set_cycle_interval(1.5);
        s.set_auto_hide(30);
        s.set_pattern("wedge");
        s.set_region_side("left");
        s.set_per_monitor_colors(true);

        assert_eq!(s.brightness(), Some(0.42));
        assert_eq!(s.cycle_interval(), Some(1.5));
        assert_eq!(s.auto_hide(), Some(30));
        assert_eq!(s.pattern().as_deref(), Some("wedge"));
        assert_eq!(s.region_side().as_deref(), Some("left"));
        assert!(s.per_monitor_colors());
    }

    #[test]
    fn a_negative_auto_hide_is_no_value_rather_than_a_huge_one() {
        let s = scratch();
        s.file.set_integer(GROUP, KEY_AUTO_HIDE, -5);
        assert_eq!(s.auto_hide(), None);
    }

    #[test]
    fn the_second_color_can_be_turned_back_off() {
        let s = scratch();
        s.set_second_color(Some(CYAN));
        assert_eq!(s.second_color(), Some(CYAN));
        s.set_second_color(None);
        assert_eq!(s.second_color(), None);
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

    #[test]
    fn the_queue_survives_a_round_trip() {
        let s = scratch();
        let colors = vec![gdk::RGBA::WHITE, gdk::RGBA::BLACK, CYAN];
        s.set_queue(&colors);
        assert_eq!(s.queue(), colors);

        // And shrinking it does not leave the tail behind.
        s.set_queue(&[gdk::RGBA::RED]);
        assert_eq!(s.queue(), vec![gdk::RGBA::RED]);
    }

    #[test]
    fn per_monitor_colors_survive_a_round_trip() {
        let s = scratch();
        // Including a key with '=' in it, which cannot be a key-file key.
        let colors = vec![
            ("DP-1".to_string(), gdk::RGBA::RED),
            ("Acme=1|Model".to_string(), CYAN),
        ];
        s.set_monitor_colors(&colors);

        let back = s.monitor_colors();
        assert_eq!(back.len(), 2);
        assert_eq!(back.get("DP-1"), Some(&gdk::RGBA::RED));
        assert_eq!(back.get("Acme=1|Model"), Some(&CYAN));
    }

    #[test]
    fn a_half_written_entry_is_skipped_rather_than_guessed() {
        let s = scratch();
        s.set_monitor_colors(&[("DP-1".to_string(), gdk::RGBA::RED)]);
        // Hand-edited: the name is there, the colour is not.
        let _ = s.file.remove_key(MONITOR_COLORS, "color0");
        assert!(s.monitor_colors().is_empty());
    }
}
