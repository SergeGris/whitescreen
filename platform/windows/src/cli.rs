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

use whitescreen_core::{Fill, Rgba, Side};

/// What one invocation asks for.
#[derive(Clone, PartialEq, Debug)]
pub struct Options {
    pub color: Rgba,
    /// Shell patterns naming the monitors to cover; empty means every one.
    pub monitors: Vec<String>,
    pub fill: Fill,
    pub brightness: f32,
    /// Minutes before the overlays take themselves down; `None` for never.
    pub auto_hide: Option<u32>,
    pub action: Action,
}

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
            fill: Fill::default(),
            brightness: 1.0,
            auto_hide: None,
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
  -l, --list-monitors      Print the monitor names and sizes, then exit.
  -j, --json               With --list-monitors, print JSON.
  -V, --version            Print the version and exit.
  -h, --help               Print this and exit.

Press any key or click an overlay to dismiss it.
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
                opts.fill = match v.as_str() {
                    "flat" => Fill::Region { side: Side::Full, rest: None },
                    "region" => {
                        region_asked = true;
                        Fill::Region { side: Side::Full, rest: None }
                    }
                    "exerciser" => Fill::Noise,
                    "wedge" => Fill::Wedge,
                    "grid" => Fill::Grid,
                    _ => {
                        return Err(format!(
                            "not a pattern: {v} (flat, region, exerciser, wedge, grid)"
                        ));
                    }
                };
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
                let percent: f32 = v.parse().map_err(|_| format!("not a number: {v}"))?;
                if !(5.0..=100.0).contains(&percent) {
                    return Err(format!("--brightness is 5 to 100, not {v}"));
                }
                opts.brightness = percent / 100.0;
            }
            "-A" | "--auto-hide" => {
                let v = value("--auto-hide")?;
                let minutes: u32 = v.parse().map_err(|_| format!("not a number of minutes: {v}"))?;
                opts.auto_hide = (minutes > 0).then_some(minutes);
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
