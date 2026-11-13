//! The command line of the portable Windows build.
//!
//! Deliberately not behind `cfg(windows)`: parsing a command line needs no
//! Win32 at all, and keeping it out here means it is compiled and tested
//! wherever the workspace is, not only on the one platform that can run it.
//!
//! The flags are the subset of the GTK build's that means something without
//! a control window. What is missing is missing because there is nothing yet
//! to drive: `--cycle` needs a queue to play and `--hide` needs an instance
//! already running.

use whitescreen_core::{Fill, Pattern, Rgba, Side, options, queue};

/// What one invocation asks for.
#[derive(Clone, PartialEq, Debug)]
pub struct Options {
    pub color: Rgba,
    /// Shell patterns naming the monitors to cover; empty means every one.
    pub monitors: Vec<String>,
    /// Patterns paired with a colour of their own, from `--monitor-color`.
    pub monitor_colors: Vec<(String, Rgba)>,
    /// Switch per-monitor colours off again, keeping them for next time.
    pub no_monitor_colors: bool,
    pub fill: Fill,
    pub brightness: f32,
    /// Minutes before the overlays take themselves down; `None` for never.
    pub auto_hide: Option<u32>,
    /// Play the queue while the overlays are up.
    pub cycle: bool,
    /// Seconds on each step, clamped to what the GTK build's spinner offers.
    pub interval: f64,
    /// The queue to play, when `--queue` named one.
    pub queue: Option<Vec<Rgba>>,
    /// Flash each monitor's name on the screen itself.
    pub identify: bool,
    pub action: Action,
}

/// Interval bounds, in seconds. One copy, in the core, because the GTK
/// build's spinner offers exactly this range and the floor of it is a
/// safety limit rather than a preference.
pub use whitescreen_core::queue::{INTERVAL_DEFAULT, INTERVAL_MAX, INTERVAL_MIN};

/// What to do, once the options are understood.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Show,
    ListMonitors { json: bool },
    Help,
    Version,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            color: Rgba::WHITE,
            monitors: Vec::new(),
            monitor_colors: Vec::new(),
            no_monitor_colors: false,
            fill: Fill::default(),
            brightness: 1.0,
            auto_hide: None,
            cycle: false,
            interval: INTERVAL_DEFAULT,
            queue: None,
            identify: false,
            action: Action::Show,
        }
    }
}

/// What `--help` prints.
pub const USAGE: &str = "\
whitescreen — fill any monitor with a solid color

Usage:
  whitescreen [options]

Options:
  -c, --color COLOR        A name (white, black, red, green, blue, cyan,
                           magenta, yellow, grey) or #RRGGBB. Default white.
  -m, --monitor NAME       Cover monitors matching NAME, which may be a shell
                           pattern. Matched against the device name
                           (\\\\.\\DISPLAY1) and the model. Repeat for several.
  -a, --all                Every attached monitor. The default.
  -p, --pattern NAME       flat, region, exerciser, wedge or grid.
  -r, --region SIDE        full, left, right, top or bottom; implies
                           --pattern region.
  -S, --second-color COLOR Fill the other half of a region instead of
                           leaving it clear.
  -b, --brightness PERCENT Scale the color on its way to the screens, 5-100.
  -A, --auto-hide MINUTES  Take the overlays down after this long.
  -M, --monitor-color NAME=COLOR
                           Give the monitors matching NAME a color of their
                           own. Repeat for several.
      --no-monitor-colors  Forget those again, so every screen follows
                           --color.
  -y, --cycle              Step through the queue while the overlays are up.
  -i, --interval SECONDS   Seconds on each step, 0.5 to 60. Default 2.
  -q, --queue COLORS       Play this comma-separated list instead of the
                           default dead-pixel sequence.
  -I, --identify           Show each monitor's name on the screen itself.
  -l, --list-monitors      Print the monitor names and sizes, then exit.
  -j, --json               With --list-monitors, print JSON.
  -V, --version            Print the version and exit.
  -h, --help               Print this and exit.

Press any key or click an overlay to dismiss it. While the queue is
playing, Ctrl+Right and Ctrl+Left step it and Ctrl+Space pauses.
";

/// Read one invocation. `Err` carries the message to show the user.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
    let mut opts = Options::default();
    let mut side: Option<Side> = None;
    let mut second: Option<Rgba> = None;
    let mut region_asked = false;
    let mut json = false;
    let mut listing = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // A value-taking flag reports its own name when the value is absent,
        // which is the only thing the user can act on.
        let mut value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} needs a value"))
        };

        match arg.as_str() {
            "-h" | "--help" => return Ok(Options { action: Action::Help, ..opts }),
            "-V" | "--version" => return Ok(Options { action: Action::Version, ..opts }),
            "-l" | "--list-monitors" => listing = true,
            "-j" | "--json" => json = true,
            "-a" | "--all" => opts.monitors.clear(),
            "-c" | "--color" => {
                let v = value("--color")?;
                opts.color = Rgba::parse(&v).ok_or_else(|| format!("not a color: {v}"))?;
            }
            "-m" | "--monitor" => {
                let v = value("--monitor")?;
                opts.monitors.push(v);
            }
            "-p" | "--pattern" => {
                let v = value("--pattern")?;
                let pattern = Pattern::from_key(&v).ok_or_else(|| {
                    format!("not a pattern: {v} (flat, region, exerciser, wedge, grid)")
                })?;
                // Naming the region mode is asking for the region details,
                // which the loop may not have read yet; the fill is settled
                // once every argument is in.
                region_asked |= pattern == Pattern::Region;
                opts.fill = pattern.fill(Side::Full, None);
            }
            "-r" | "--region" => {
                let v = value("--region")?;
                side = Some(
                    Side::from_key(&v)
                        .ok_or_else(|| format!("not a side: {v} (full, left, right, top, bottom)"))?,
                );
                region_asked = true;
            }
            "-S" | "--second-color" => {
                let v = value("--second-color")?;
                second = Some(Rgba::parse(&v).ok_or_else(|| format!("not a color: {v}"))?);
                region_asked = true;
            }
            "-b" | "--brightness" => {
                let v = value("--brightness")?;
                let percent: f64 = v.parse().map_err(|_| format!("not a number: {v}"))?;
                if !options::brightness_in_range(percent) {
                    return Err(format!(
                        "--brightness is {} to {}, not {v}",
                        options::BRIGHTNESS_MIN_PERCENT, options::BRIGHTNESS_MAX_PERCENT
                    ));
                }
                opts.brightness = (percent / 100.0) as f32;
            }
            "-A" | "--auto-hide" => {
                let v = value("--auto-hide")?;
                let minutes: u32 = v.parse().map_err(|_| format!("not a number of minutes: {v}"))?;
                opts.auto_hide = (minutes > 0).then_some(minutes);
            }
            "-M" | "--monitor-color" => {
                let v = value("--monitor-color")?;
                opts.monitor_colors.push(parse_monitor_color(&v)?);
            }
            "--no-monitor-colors" => opts.no_monitor_colors = true,
            "-y" | "--cycle" => opts.cycle = true,
            "-I" | "--identify" => opts.identify = true,
            "-i" | "--interval" => {
                let v = value("--interval")?;
                let secs: f64 = v.parse().map_err(|_| format!("not a number: {v}"))?;
                // Clamped rather than refused, as the GTK build clamps it to
                // what its spinner offers.
                opts.interval = queue::clamp_interval(secs);
            }
            "-q" | "--queue" => {
                let v = value("--queue")?;
                let mut colors = Vec::new();
                for part in v.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    // All or nothing: a queue missing the step that was
                    // mistyped would play something nobody asked for.
                    colors.push(Rgba::parse(part).ok_or_else(|| format!("not a color: {part}"))?);
                }
                if colors.is_empty() {
                    return Err("--queue needs at least one color".to_string());
                }
                colors.truncate(queue::MAX_LEN);
                opts.queue = Some(colors);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }

    // --region and --second-color are details of the region fill, so naming
    // either of them is asking for it, exactly as in the GTK build.
    if region_asked {
        opts.fill = Fill::Region { side: side.unwrap_or(Side::Full), rest: second };
    }
    if listing {
        opts.action = Action::ListMonitors { json };
    }
    Ok(opts)
}

/// Split a `--monitor-color` argument into a monitor pattern and a colour.
///
/// The splitting rule is the core's, because the GTK build takes the same
/// argument; only what counts as a colour differs between them.
fn parse_monitor_color(spec: &str) -> Result<(String, Rgba), String> {
    let Some((name, color)) = options::split_monitor_color(spec) else {
        return Err(format!("not a NAME=COLOR pair: {spec}"));
    };
    let parsed = Rgba::parse(color).ok_or_else(|| format!("not a color: {color}"))?;
    Ok((name.to_string(), parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn no_arguments_fills_every_screen_white() {
        let o = parse_args(&[]).unwrap();
        assert_eq!(o.color, Rgba::WHITE);
        assert!(o.monitors.is_empty(), "no --monitor means all of them");
        assert_eq!(o.action, Action::Show);
        assert_eq!(o.fill, Fill::default());
    }

    #[test]
    fn colors_come_in_by_name_or_hex() {
        assert_eq!(parse_args(&["--color", "red"]).unwrap().color, Rgba::new(255, 0, 0));
        assert_eq!(parse_args(&["-c", "#123456"]).unwrap().color, Rgba::new(0x12, 0x34, 0x56));
        assert_eq!(
            parse_args(&["--color", "octarine"]).unwrap_err(),
            "not a color: octarine"
        );
    }

    #[test]
    fn monitors_accumulate_and_all_clears_them() {
        let o = parse_args(&["-m", "DISPLAY1", "--monitor", "LG*"]).unwrap();
        assert_eq!(o.monitors, vec!["DISPLAY1".to_string(), "LG*".to_string()]);

        let o = parse_args(&["-m", "DISPLAY1", "--all"]).unwrap();
        assert!(o.monitors.is_empty(), "--all means every screen, whatever came before");
    }

    #[test]
    fn naming_a_side_or_a_second_colour_asks_for_the_region_fill() {
        let o = parse_args(&["--region", "left"]).unwrap();
        assert_eq!(o.fill, Fill::Region { side: Side::Left, rest: None });

        let o = parse_args(&["--second-color", "grey"]).unwrap();
        assert_eq!(
            o.fill,
            Fill::Region { side: Side::Full, rest: Some(Rgba::new(128, 128, 128)) }
        );

        // Together, and in either order.
        let o = parse_args(&["-S", "black", "-r", "top"]).unwrap();
        assert_eq!(o.fill, Fill::Region { side: Side::Top, rest: Some(Rgba::BLACK) });
    }

    #[test]
    fn the_patterns_are_the_same_names_the_other_build_uses() {
        assert_eq!(parse_args(&["-p", "wedge"]).unwrap().fill, Fill::Wedge);
        assert_eq!(parse_args(&["-p", "grid"]).unwrap().fill, Fill::Grid);
        assert_eq!(parse_args(&["-p", "exerciser"]).unwrap().fill, Fill::Noise);
        assert_eq!(parse_args(&["-p", "flat"]).unwrap().fill, Fill::default());
        assert!(parse_args(&["-p", "spiral"]).unwrap_err().starts_with("not a pattern"));
    }

    #[test]
    fn brightness_is_a_percentage_within_the_range_the_ui_offers() {
        assert_eq!(parse_args(&["-b", "50"]).unwrap().brightness, 0.5);
        assert!(parse_args(&["-b", "0"]).unwrap_err().contains("5 to 100"));
        assert!(parse_args(&["-b", "1000"]).unwrap_err().contains("5 to 100"));
        assert!(parse_args(&["-b", "half"]).unwrap_err().starts_with("not a number"));
    }

    #[test]
    fn auto_hide_of_zero_means_never() {
        assert_eq!(parse_args(&["-A", "10"]).unwrap().auto_hide, Some(10));
        assert_eq!(parse_args(&["-A", "0"]).unwrap().auto_hide, None);
    }

    #[test]
    fn listing_and_the_answers_that_need_no_screens() {
        assert_eq!(
            parse_args(&["--list-monitors"]).unwrap().action,
            Action::ListMonitors { json: false }
        );
        assert_eq!(
            parse_args(&["-l", "-j"]).unwrap().action,
            Action::ListMonitors { json: true }
        );
        assert_eq!(parse_args(&["--help"]).unwrap().action, Action::Help);
        assert_eq!(parse_args(&["-V"]).unwrap().action, Action::Version);
    }

    #[test]
    fn a_flag_with_no_value_says_which_flag() {
        assert_eq!(parse_args(&["--color"]).unwrap_err(), "--color needs a value");
        assert_eq!(parse_args(&["-m"]).unwrap_err(), "--monitor needs a value");
    }

    #[test]
    fn an_unknown_option_is_refused_rather_than_ignored() {
        assert_eq!(parse_args(&["--colour", "red"]).unwrap_err(), "unknown option: --colour");
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn per_monitor_colours_accumulate() {
        let o = parse_args(&["-M", "DISPLAY1=red", "--monitor-color", "LG*=#00FF00"]).unwrap();
        assert_eq!(
            o.monitor_colors,
            vec![
                ("DISPLAY1".to_string(), Rgba::new(255, 0, 0)),
                ("LG*".to_string(), Rgba::new(0, 255, 0)),
            ]
        );
    }

    #[test]
    fn a_monitor_colour_splits_at_the_last_equals() {
        // A device name can contain anything; a colour cannot contain '='.
        let o = parse_args(&["-M", r"\\.\DISPLAY1=blue"]).unwrap();
        assert_eq!(o.monitor_colors[0].0, r"\\.\DISPLAY1");
        let o = parse_args(&["-M", "Acme=1|Model=blue"]).unwrap();
        assert_eq!(o.monitor_colors[0].0, "Acme=1|Model");
    }

    #[test]
    fn a_malformed_pair_says_so() {
        assert_eq!(
            parse_args(&["-M", "DISPLAY1"]).unwrap_err(),
            "not a NAME=COLOR pair: DISPLAY1"
        );
        assert_eq!(parse_args(&["-M", "=red"]).unwrap_err(), "not a NAME=COLOR pair: =red");
        assert_eq!(parse_args(&["-M", "DISPLAY1=octarine"]).unwrap_err(), "not a color: octarine");
    }

    #[test]
    fn the_interval_is_clamped_to_what_the_other_build_offers() {
        assert_eq!(parse_args(&["-i", "1.5"]).unwrap().interval, 1.5);
        assert_eq!(parse_args(&["-i", "0.1"]).unwrap().interval, INTERVAL_MIN);
        assert_eq!(parse_args(&["-i", "600"]).unwrap().interval, INTERVAL_MAX);
        assert_eq!(parse_args(&[]).unwrap().interval, INTERVAL_DEFAULT);
        // "nan" parses as a number and used to survive the clamp, which
        // stopped the queue dead: nothing is ever due after a NaN deadline.
        assert_eq!(parse_args(&["-i", "nan"]).unwrap().interval, INTERVAL_DEFAULT);
    }

    #[test]
    fn a_queue_is_all_or_nothing() {
        let o = parse_args(&["--queue", "#fff,black, red "]).unwrap();
        assert_eq!(o.queue, Some(vec![Rgba::WHITE, Rgba::BLACK, Rgba::new(255, 0, 0)]));

        // One mistyped step and none of it is used.
        assert_eq!(parse_args(&["-q", "white,octarine"]).unwrap_err(), "not a color: octarine");
        assert_eq!(parse_args(&["-q", " , "]).unwrap_err(), "--queue needs at least one color");
    }

    #[test]
    fn a_queue_cannot_exceed_what_the_strip_holds() {
        let long = (0..30).map(|_| "red").collect::<Vec<_>>().join(",");
        let o = parse_args(&["-q", &long]).unwrap();
        assert_eq!(o.queue.unwrap().len(), queue::MAX_LEN);
    }

    #[test]
    fn the_remaining_switches_are_off_by_default() {
        let o = parse_args(&[]).unwrap();
        assert!(!o.cycle && !o.identify && !o.no_monitor_colors);
        assert!(o.queue.is_none() && o.monitor_colors.is_empty());

        let o = parse_args(&["--cycle", "--identify", "--no-monitor-colors"]).unwrap();
        assert!(o.cycle && o.identify && o.no_monitor_colors);
    }
}
