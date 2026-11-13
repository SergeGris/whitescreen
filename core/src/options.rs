//! The parts of the command line that are nobody's toolkit's business.
//!
//! A flag's *spelling* belongs to the front end -- GTK declares its options
//! on the application so that `--help` comes out translated, and the
//! portable build parses its own -- but what a value is allowed to be does
//! not. Both builds accept `--brightness` and `--monitor-color`, and both
//! must accept exactly the same arguments: a script written against one of
//! them is written against the other.
//!
//! They had not. `--brightness 500` was refused on Windows and silently
//! clamped on Linux, and `--monitor-color "DP-1= red"` worked on Linux
//! alone, because only one of the two copies trimmed the colour.

/// What `--brightness` and the window's slider both allow.
///
/// The floor is not zero on purpose: a black screen is what the black chip
/// is for, and a brightness that can blank an overlay looks like a fault
/// rather than a setting.
pub const BRIGHTNESS_MIN_PERCENT: f64 = 5.0;
pub const BRIGHTNESS_MAX_PERCENT: f64 = 100.0;

/// Whether a percentage is one the app will take.
///
/// Refused rather than clamped, so that whoever typed it hears about it:
/// everything else either build rejects says so, and a brightness quietly
/// becoming something other than what was asked for is the one that would
/// be found out later, on a measurement.
pub fn brightness_in_range(percent: f64) -> bool {
    // NaN fails this, as it must: it compares false against everything, so
    // a clamp would pass it straight through to the renderer.
    (BRIGHTNESS_MIN_PERCENT..=BRIGHTNESS_MAX_PERCENT).contains(&percent)
}

/// Split a `--monitor-color` argument into a monitor pattern and a colour.
///
/// Split at the *last* `=`, not the first: a monitor that reports no
/// connector name is keyed by its EDID text, and a Windows one by its
/// device name, and either can contain any character at all, `=` included.
/// A colour cannot.
///
/// Both halves come back trimmed, and the name may not be empty. The colour
/// is handed back as it was written, however little sense it makes: what
/// counts as a colour differs between the builds -- the GTK side takes any
/// CSS one, having GDK to parse them -- so judging it is the caller's, and
/// "not a color" is a better answer than "malformed pair" for an argument
/// whose name half is perfectly good.
pub fn split_monitor_color(spec: &str) -> Option<(&str, &str)> {
    let (name, color) = spec.rsplit_once('=')?;
    let (name, color) = (name.trim(), color.trim());
    (!name.is_empty()).then_some((name, color))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_is_a_percentage_within_the_bounds() {
        assert!(brightness_in_range(5.0));
        assert!(brightness_in_range(100.0));
        assert!(brightness_in_range(42.5));
        assert!(!brightness_in_range(4.9));
        assert!(!brightness_in_range(101.0));
        assert!(!brightness_in_range(-0.0));
        assert!(!brightness_in_range(f64::NAN), "a NaN is not a percentage");
    }

    #[test]
    fn a_monitor_colour_splits_at_the_last_equals() {
        assert_eq!(split_monitor_color("DP-1=red"), Some(("DP-1", "red")));
        // A key with an = in it: the colour is still the part after the
        // last one.
        assert_eq!(
            split_monitor_color("ACME=Display 27=#ff00ff"),
            Some(("ACME=Display 27", "#ff00ff")),
        );
        assert_eq!(
            split_monitor_color(r"\\.\DISPLAY1=#001122"),
            Some((r"\\.\DISPLAY1", "#001122")),
        );
    }

    #[test]
    fn both_halves_are_trimmed_and_the_name_may_not_be_empty() {
        assert_eq!(split_monitor_color("  DP-1 =  red "), Some(("DP-1", "red")));
        assert_eq!(split_monitor_color("DP-1"), None);
        assert_eq!(split_monitor_color("=red"), None);
        assert_eq!(split_monitor_color("  =red"), None);
        assert_eq!(split_monitor_color(""), None);
    }

    #[test]
    fn an_empty_colour_is_left_for_the_caller_to_refuse() {
        // The name half is good, so this is a bad colour rather than a bad
        // pair, and both builds say so.
        assert_eq!(split_monitor_color("DP-1="), Some(("DP-1", "")));
        assert_eq!(split_monitor_color("DP-1=   "), Some(("DP-1", "")));
    }
}
