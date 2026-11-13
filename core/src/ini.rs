//! The settings file.
//!
//! A key file, in the format GLib's `GKeyFile` writes, because that is what
//! the GTK build has always written and an existing `settings.ini` has to
//! keep working. Implemented here rather than called from GLib so that the
//! Windows build reads and writes exactly the same bytes -- a profile
//! copied from one machine to the other should simply work, and that is
//! only true if one implementation serves both.
//!
//! The subset is the one the app uses: groups in square brackets,
//! `key=value`, `#` comments. Values are escaped the way GLib escapes them,
//! which matters more than it looks: a monitor on Windows is keyed
//! `\\.\DISPLAY1`, and backslashes are exactly what the format escapes.

use std::collections::BTreeMap;

/// A parsed key file. Groups and keys keep a stable order so that writing
/// the same settings twice produces the same file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ini {
    groups: BTreeMap<String, BTreeMap<String, String>>,
}

impl Ini {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a key file. Anything unparseable is skipped rather than
    /// rejected: a corrupt or hand-edited file should cost the settings it
    /// mangled, not every setting in it.
    pub fn parse(text: &str) -> Self {
        let mut ini = Self::new();
        let mut group = String::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                group = name.to_string();
                ini.groups.entry(group.clone()).or_default();
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                ini.groups
                    .entry(group.clone())
                    .or_default()
                    .insert(key.trim().to_string(), unescape(value));
            }
        }
        ini
    }

    pub fn get(&self, group: &str, key: &str) -> Option<&str> {
        self.groups.get(group)?.get(key).map(String::as_str)
    }

    pub fn set(&mut self, group: &str, key: &str, value: &str) {
        self.groups
            .entry(group.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }

    pub fn remove(&mut self, group: &str, key: &str) {
        if let Some(entries) = self.groups.get_mut(group) {
            entries.remove(key);
        }
    }

    /// Drop a whole group.
    ///
    /// Used before writing a numbered list: writing fewer entries than last
    /// time would otherwise leave the tail behind for a future reader to
    /// trip over.
    pub fn remove_group(&mut self, group: &str) {
        self.groups.remove(group);
    }

    pub fn get_i64(&self, group: &str, key: &str) -> Option<i64> {
        self.get(group, key)?.trim().parse().ok()
    }

    pub fn get_f64(&self, group: &str, key: &str) -> Option<f64> {
        self.get(group, key)?.trim().parse().ok()
    }

    /// GLib writes booleans as `true` and `false`.
    pub fn get_bool(&self, group: &str, key: &str) -> Option<bool> {
        match self.get(group, key)?.trim() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }

    pub fn set_i64(&mut self, group: &str, key: &str, value: i64) {
        self.set(group, key, &value.to_string());
    }

    pub fn set_f64(&mut self, group: &str, key: &str, value: f64) {
        // GLib prints a double in the shortest round-tripping form, which is
        // what Rust's Display does too.
        self.set(group, key, &value.to_string());
    }

    pub fn set_bool(&mut self, group: &str, key: &str, value: bool) {
        self.set(group, key, if value { "true" } else { "false" });
    }
}

impl std::fmt::Display for Ini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, (group, entries)) in self.groups.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            writeln!(f, "[{group}]")?;
            for (key, value) in entries {
                writeln!(f, "{key}={}", escape(value))?;
            }
        }
        Ok(())
    }
}

/// Escape a value the way GLib does.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (i, c) in value.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Only a leading space needs escaping; one in the middle is
            // ordinary text, and GLib leaves it alone.
            ' ' if i == 0 => out.push_str("\\s"),
            c => out.push(c),
        }
    }
    out
}

/// Read a value back, undoing `escape`.
///
/// Leading whitespace is dropped first, as GLib drops it: the space after
/// `key = value` is formatting, not data.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.trim_start().chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            // An escape nobody defined: keep both characters rather than
            // eating one, so a hand-edited path is not silently mangled.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_survive_a_round_trip() {
        let mut ini = Ini::new();
        ini.set("whitescreen", "color", "#FF0000");
        ini.set_f64("whitescreen", "brightness", 0.5);
        ini.set_i64("whitescreen", "auto-hide", 30);
        ini.set_bool("whitescreen", "per-monitor-colors", true);

        let back = Ini::parse(&ini.to_string());
        assert_eq!(back.get("whitescreen", "color"), Some("#FF0000"));
        assert_eq!(back.get_f64("whitescreen", "brightness"), Some(0.5));
        assert_eq!(back.get_i64("whitescreen", "auto-hide"), Some(30));
        assert_eq!(back.get_bool("whitescreen", "per-monitor-colors"), Some(true));
        assert_eq!(back, ini);
    }

    #[test]
    fn a_windows_device_name_survives_its_backslashes() {
        // The case this format's escaping exists for: \\.\DISPLAY1 is what
        // every monitor is keyed by on Windows.
        let mut ini = Ini::new();
        ini.set("monitors", "0", r"\\.\DISPLAY1");
        let text = ini.to_string();
        assert!(text.contains(r"0=\\\\.\\DISPLAY1"), "not escaped as GLib would: {text}");
        assert_eq!(Ini::parse(&text).get("monitors", "0"), Some(r"\\.\DISPLAY1"));
    }

    #[test]
    fn awkward_characters_in_a_monitor_key_survive() {
        for value in [
            "Acme;Inc|Model X|desc",
            "with\\backslash",
            "has = equals",
            " leading space",
            "tab\there",
        ] {
            let mut ini = Ini::new();
            ini.set("monitors", "0", value);
            assert_eq!(
                Ini::parse(&ini.to_string()).get("monitors", "0"),
                Some(value),
                "{value:?} did not survive"
            );
        }
    }

    #[test]
    fn what_glib_wrote_is_read_back_the_same_way() {
        // A file from the GTK build, written by GKeyFile.
        let text = "\
[whitescreen]
color=#00FF00
brightness=0.75
per-monitor-colors=true

[monitors]
count=1
0=DP-1
";
        let ini = Ini::parse(text);
        assert_eq!(ini.get("whitescreen", "color"), Some("#00FF00"));
        assert_eq!(ini.get_f64("whitescreen", "brightness"), Some(0.75));
        assert_eq!(ini.get_bool("whitescreen", "per-monitor-colors"), Some(true));
        assert_eq!(ini.get_i64("monitors", "count"), Some(1));
        assert_eq!(ini.get("monitors", "0"), Some("DP-1"));
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let ini = Ini::parse("# a comment\n\n[g]\n; another\nk=v\n");
        assert_eq!(ini.get("g", "k"), Some("v"));
    }

    #[test]
    fn a_mangled_line_costs_only_itself() {
        let ini = Ini::parse("[g]\nthis line has no equals sign\nk=v\n");
        assert_eq!(ini.get("g", "k"), Some("v"));
    }

    #[test]
    fn a_missing_value_is_none_rather_than_a_default() {
        let ini = Ini::parse("[g]\nk=notanumber\n");
        assert_eq!(ini.get("g", "absent"), None);
        assert_eq!(ini.get_i64("g", "k"), None, "a value that is not a number is not a zero");
        assert_eq!(ini.get_bool("g", "k"), None);
    }

    #[test]
    fn removing_a_group_removes_its_tail() {
        let mut ini = Ini::new();
        ini.set("queue", "0", "#FFFFFF");
        ini.set("queue", "1", "#000000");
        ini.remove_group("queue");
        ini.set("queue", "0", "#FF0000");
        let back = Ini::parse(&ini.to_string());
        assert_eq!(back.get("queue", "0"), Some("#FF0000"));
        assert_eq!(back.get("queue", "1"), None, "the old second step is still there");
    }
}
