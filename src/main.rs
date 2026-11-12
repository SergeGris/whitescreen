// White Screen – fill any monitor with a solid color.

use std::cell::Cell;
use std::sync::OnceLock;

use adw::prelude::*;
use gtk::gio::prelude::ApplicationCommandLineExt;
use gtk::{gdk, gio, glib};

use crate::i18n::{gettext, mark};
use whitescreen_core::text;

mod color;
mod color_button;
mod color_surface;
mod cycle_bar;
mod cycle_queue;
mod fill_bar;
mod i18n;
mod main_window;
mod monitor_label;
mod monitor_list;
mod palette;
mod screen_overlay;
mod settings;
mod swatch;
mod tile;

const APP_ID:        &str = "io.github.SergeGris.WhiteScreen";
const APP_VERSION:   &str = env!("CARGO_PKG_VERSION");
const WEBSITE:       &str = "https://github.com/SergeGris/whitescreen";

#[cfg(feature = "gamma")]
mod gamma;

mod css_class {
    pub const MONLABEL_WINDOW:   &str = "monlabel-window";
    pub const MONLABEL_TITLE:    &str = "monlabel-title";
    pub const MONLABEL_SUBTITLE: &str = "monlabel-subtitle";
}

/// Whether overlays can be layer-shell surfaces on this compositor.
///
/// Answered once and cached, because every overlay and badge window asks, and
/// because the answer flipping mid-session would leave windows half set up:
/// a window that skipped `init_layer_shell()` must never be handed a
/// layer-shell call afterwards.
///
/// `false` puts the app in fallback mode (see `ScreenOverlay`), where overlays
/// are ordinary fullscreen windows. That covers GNOME/Mutter and X11, which
/// refuse `wlr-layer-shell`; what is lost is stacking above everything else
/// and the Identify badges, not the colour itself.
///
/// Must not be called before GTK has opened a display.
pub fn layer_shell_available() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(gtk_layer_shell::is_supported)
}

/// Report a condition that leaves nothing to run, then quit.
///
/// The hold/release pair matters: `AlertDialog` is not a `GtkWindow` the
/// application knows about, so with no application window open the main loop
/// has nothing keeping it alive and `app.run()` would return -- taking the
/// process down before the dialog was ever drawn. That is exactly what the
/// earlier `show()`-and-return version did.
fn fatal(app: &adw::Application, message: &str, detail: &str) {
    // The guard is what keeps the process alive; it is released when the
    // callback drops it, which is the point at which quitting is correct.
    let hold = app.hold();
    let app  = app.clone();

    gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons([gettext("Close")])
        .modal(true)
        .build()
        .choose(gtk::Window::NONE, gio::Cancellable::NONE, move |_| {
            drop(hold);
            app.quit();
        });
}

/// Say once, on stderr, that the overlays will not be layer surfaces.
///
/// The window carries the same notice in a banner; this is for anyone who
/// started the app from a terminal and is wondering why it looks different
/// from the description.
fn warn_no_layer_shell() {
    // Plain stderr, unlike the per-invocation messages: this is said once,
    // by the process that builds the window, which is always the one that
    // started the app.
    eprintln!(
        "whitescreen: {}",
        gettext(
            "this compositor does not support wlr-layer-shell -- overlays will be ordinary fullscreen windows, and Identify is unavailable"
        )
    );
}

/// The command line, for a compositor keybind or a script.
///
/// Declared on the application rather than parsed by hand so that `--help`
/// comes out right, and so a second invocation is forwarded to the instance
/// already running instead of opening a rival window -- which is what makes
/// `whitescreen --color red --all --show` usable as a shortcut.
fn add_options(app: &adw::Application) {
    let none = glib::Char::try_from('\0').unwrap();
    // `help` and `value` arrive marked rather than translated: the lookup
    // happens here, so the table below stays readable and xgettext still sees
    // every string in it.
    let opt = |long: &str, short: char, arg: glib::OptionArg, help: &str, value: Option<&str>| {
        app.add_main_option(
            long,
            glib::Char::try_from(short).unwrap_or(none),
            glib::OptionFlags::NONE,
            arg,
            &gettext(help),
            value.map(gettext).as_deref(),
        );
    };

    opt("color", 'c', glib::OptionArg::String,
        mark("Fill with COLOR: a name, #RRGGBB, or any CSS color"), Some(mark("COLOR")));
    opt("brightness", 'b', glib::OptionArg::Double,
        mark("Scale the color to PERCENT of full output"), Some(mark("PERCENT")));
    opt("monitor", 'm', glib::OptionArg::StringArray,
        mark("Cover monitors matching NAME, which may be a shell pattern; repeat for several"),
        Some(mark("NAME")));
    opt("monitor-color", 'M', glib::OptionArg::StringArray,
        mark("Give the monitors matching NAME a color of their own; repeat for several"),
        Some(mark("NAME=COLOR")));
    opt("no-monitor-colors", '\0', glib::OptionArg::None,
        mark("Switch per-monitor colors off again, keeping them for next time"), None);
    opt("all", 'a', glib::OptionArg::None,
        mark("Cover every attached monitor"), None);
    opt("show", 's', glib::OptionArg::None,
        mark("Show the overlays straight away"), None);
    opt("hide", 'H', glib::OptionArg::None,
        mark("Hide the overlays"), None);
    opt("cycle", 'y', glib::OptionArg::None,
        mark("Arm cycle mode, so showing plays the queue"), None);
    opt("interval", 'i', glib::OptionArg::Double,
        mark("Seconds on each cycle step"), Some(mark("SECONDS")));
    opt("queue", 'q', glib::OptionArg::String,
        mark("Cycle through this comma-separated list of colors"), Some(mark("COLORS")));
    opt("pattern", 'p', glib::OptionArg::String,
        mark("What covers each screen: flat, region, exerciser, wedge or grid"), Some(mark("NAME")));
    opt("exerciser", 'x', glib::OptionArg::None,
        mark("Shorthand for --pattern exerciser"), None);
    opt("region", 'r', glib::OptionArg::String,
        mark("Cover only this half: full, left, right, top or bottom"), Some(mark("SIDE")));
    opt("second-color", 'S', glib::OptionArg::String,
        mark("Fill the other half of a region with COLOR instead of leaving it clear"),
        Some(mark("COLOR")));
    opt("identify", 'I', glib::OptionArg::None,
        mark("Flash each monitor's connector name on the screen itself"), None);
    opt("auto-hide", 'A', glib::OptionArg::Double,
        mark("Take the overlays down after MINUTES; 0 leaves them up"), Some(mark("MINUTES")));
    opt("list-monitors", 'l', glib::OptionArg::None,
        mark("Print the monitor names --monitor accepts, and exit"), None);
    opt("json", 'j', glib::OptionArg::None,
        mark("Print --list-monitors output as JSON"), None);
    opt("version", 'V', glib::OptionArg::None,
        mark("Print the version and exit"), None);
}

/// Print the names `--monitor` accepts.
///
/// The same keys the window uses, duplicate suffixes and all, so what is
/// printed can be pasted straight back in.
fn list_monitors(cmd: &gio::ApplicationCommandLine, as_json: bool) {
    let Some(display) = gdk::Display::default() else { return };
    let model = display.monitors();
    let monitors: Vec<gdk::Monitor> = (0..model.n_items())
        .filter_map(|i| model.item(i)?.downcast::<gdk::Monitor>().ok())
        .filter(|m| m.is_valid())
        .collect();
    let keys = main_window::monitor_keys(&monitors);

    if !as_json {
        for (key, mon) in keys.iter().zip(&monitors) {
            let geo = mon.geometry();
            cmd.print_literal(&format!("{key}\t{} × {}\n", geo.width(), geo.height()));
        }
        return;
    }

    // Hand-rolled rather than pulling in a serializer for four fields. The
    // only values that are not numbers are the key and the model, and both
    // come from the compositor, so both are escaped.
    let mut out = String::from("[\n");
    for (i, (key, mon)) in keys.iter().zip(&monitors).enumerate() {
        let geo = mon.geometry();
        let model = mon.model().map(|s| s.to_string()).unwrap_or_default();
        let comma = if i + 1 < keys.len() { "," } else { "" };
        out.push_str(&format!(
            "  {{\"name\": {}, \"width\": {}, \"height\": {}, \
             \"scale\": {}, \"refresh_mhz\": {}, \"model\": {}}}{comma}\n",
            text::json_string(key),
            geo.width(),
            geo.height(),
            mon.scale(),
            mon.refresh_rate(),
            text::json_string(&model),
        ));
    }
    out.push_str("]\n");
    cmd.print_literal(&out);
}

/// Options that configure or act on the app, as opposed to `--version` and
/// `--list-monitors`, which answer and leave.
const OPTION_KEYS: &[&str] = &[
    "color", "brightness", "monitor", "monitor-color", "no-monitor-colors",
    "all", "show", "hide", "cycle", "interval",
    "queue", "pattern", "exerciser", "region", "second-color", "identify", "auto-hide",
];

fn has_options(options: &glib::VariantDict) -> bool {
    OPTION_KEYS.iter().any(|key| options.contains(key))
}

/// Whether this invocation puts something on the screens, or takes it away,
/// on its own. Those are the ones worth running without a control window.
fn is_scripted(options: &glib::VariantDict) -> bool {
    options.contains("show") || options.contains("hide")
}

fn find_window(app: &adw::Application) -> Option<main_window::Window> {
    app.windows()
        .into_iter()
        .find_map(|w| w.downcast::<main_window::Window>().ok())
}

/// The window, built if this is the first invocation. Not presented: whether
/// it should be seen is the caller's decision.
///
/// `None` means there is no display to build it on, and the caller has been
/// told about it.
fn ensure_window(app: &adw::Application) -> Option<main_window::Window> {
    if let Some(win) = find_window(app) {
        return Some(win);
    }

    if gdk::Display::default().is_none() {
        fatal(
            app,
            &gettext("No display connection"),
            &gettext("White Screen could not connect to a display server."),
        );
        return None;
    }

    // No layer shell is no longer fatal. Refusing to start meant the app was
    // unusable on GNOME and X11 even though filling a screen with a colour
    // needs nothing more than a fullscreen window there.
    if !layer_shell_available() {
        warn_no_layer_shell();
    }

    Some(main_window::Window::new(app))
}
/// What was wrong with a `--monitor-color` argument.
#[derive(Debug, PartialEq)]
enum BadMonitorColor {
    /// No `=`, or nothing in front of it.
    Malformed,
    /// There is a name, but what follows the `=` is not a colour.
    NotAColor(String),
}

/// Split a `--monitor-color` argument into a monitor pattern and a colour.
///
/// Split at the *last* `=`, not the first: a monitor that reports no
/// connector name is keyed by its EDID text (see `monitor_ident()`), which
/// can contain any character at all, `=` included. A colour cannot.
fn parse_monitor_color(spec: &str) -> Result<(String, gdk::RGBA), BadMonitorColor> {
    let Some((name, color)) = spec.rsplit_once('=') else {
        return Err(BadMonitorColor::Malformed);
    };
    if name.is_empty() {
        return Err(BadMonitorColor::Malformed);
    }
    match gdk::RGBA::parse(color.trim()) {
        Ok(rgba) => Ok((name.to_string(), rgba)),
        Err(_)   => Err(BadMonitorColor::NotAColor(color.to_string())),
    }
}

/// One invocation being answered, and whether everything in it was accepted.
///
/// The two belong together because every rejection has to do both things:
/// say so to whoever typed the command, and leave the process they typed it
/// into exiting non-zero. Keeping them in one place is what stops a flag
/// being added later that reports a problem and still claims success.
struct Invocation<'a> {
    cmd: &'a gio::ApplicationCommandLine,
    ok:  Cell<bool>,
}

impl Invocation<'_> {
    fn new(cmd: &gio::ApplicationCommandLine) -> Invocation<'_> {
        Invocation { cmd, ok: Cell::new(true) }
    }

    /// Tell whoever typed the command that part of it was no good.
    ///
    /// Through the GApplicationCommandLine rather than stderr: an invocation
    /// forwarded to an instance already running is *handled* over there, and
    /// a plain `eprintln!` would print the complaint into whatever terminal
    /// started that one -- or into nothing at all, for an instance launched
    /// from the desktop entry. This sends it back to the process that typed
    /// it, which is also the one whose exit code reports it.
    fn complain(&self, message: &str) {
        self.ok.set(false);
        self.cmd.printerr_literal(&format!("whitescreen: {message}\n"));
    }

    /// The one message three different flags produce.
    fn not_a_colour(&self, spec: &str) {
        self.complain(&i18n::format(&gettext("not a color: {0}"), &[spec]));
    }

    /// Report patterns that matched no attached monitor, and name the ones
    /// that are there.
    ///
    /// Shared by `--monitor` and `--monitor-color` so that a mistyped name
    /// reads the same whichever flag it was mistyped in.
    fn report_unmatched(&self, win: &main_window::Window, unmatched: &[String]) {
        if unmatched.is_empty() {
            return;
        }
        for pattern in unmatched {
            self.complain(&i18n::format(&gettext("no monitor matches {0}"), &[pattern]));
        }
        // Naming what is there turns a mistyped pattern into a one-line fix
        // instead of a second run with --list-monitors.
        self.complain(&i18n::format(
            &gettext("attached: {0}"),
            &[&win.attached_monitors().join(", ")],
        ));
    }

    fn exit_code(&self) -> glib::ExitCode {
        if self.ok.get() { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE }
    }
}

/// Apply one invocation's options to the window.
fn apply_options(win: &main_window::Window, inv: &Invocation) {
    let options = inv.cmd.options_dict();

    if let Ok(Some(spec)) = options.lookup::<String>("color") {
        match gdk::RGBA::parse(&spec) {
            Ok(rgba) => win.set_color(rgba),
            Err(_)   => inv.not_a_colour(&spec),
        }
    }

    if let Ok(Some(percent)) = options.lookup::<f64>("brightness") {
        win.set_brightness(percent / 100.0);
    }

    if options.contains("all") {
        win.select_all_monitors();
    } else if let Ok(Some(patterns)) = options.lookup::<Vec<String>>("monitor") {
        inv.report_unmatched(win, &win.select_monitors(&patterns));
    }

    // Before --monitor-color, so that naming both leaves the mode on: the
    // more specific flag is the one to honour, as with --pattern and
    // --region.
    if options.contains("no-monitor-colors") {
        win.set_per_monitor_colors(false);
    }

    // After --color, whose shared colour these override. Colouring a monitor
    // does not select it, exactly as ticking a row and setting its colour are
    // two actions in the sidebar.
    if let Ok(Some(specs)) = options.lookup::<Vec<String>>("monitor-color") {
        let mut pairs = Vec::new();
        for spec in &specs {
            match parse_monitor_color(spec) {
                Ok(pair) => pairs.push(pair),
                Err(BadMonitorColor::Malformed) => inv.complain(&i18n::format(
                    &gettext("not a NAME=COLOR pair: {0}"),
                    &[spec],
                )),
                Err(BadMonitorColor::NotAColor(color)) => inv.not_a_colour(&color),
            }
        }
        inv.report_unmatched(win, &win.set_monitor_colors(&pairs));
    }

    if let Ok(Some(spec)) = options.lookup::<String>("queue") {
        let (colors, bad): (Vec<_>, Vec<_>) = spec
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| (s.to_string(), gdk::RGBA::parse(s)))
            .partition(|(_, parsed)| parsed.is_ok());

        for (spec, _) in &bad {
            inv.not_a_colour(spec);
        }
        // All or nothing: a queue missing the step that was mistyped would
        // play something the user never asked for.
        if bad.is_empty() {
            let colors: Vec<gdk::RGBA> = colors.into_iter().filter_map(|(_, c)| c.ok()).collect();
            if colors.is_empty() {
                inv.complain(&gettext("--queue needs at least one color"));
            } else {
                win.set_queue(&colors);
            }
        }
    }

    // Before --region, so that naming both takes the side rather than the
    // mode: `--pattern wedge --region left` is contradictory, and the more
    // specific flag is the one to honour.
    if let Ok(Some(name)) = options.lookup::<String>("pattern") {
        match fill_bar::Mode::from_key(&name) {
            Some(mode) => win.set_pattern(mode),
            None => inv.complain(&i18n::format(
                &gettext("not a pattern: {0} (flat, region, exerciser, wedge, grid)"),
                &[&name],
            )),
        }
    }
    if options.contains("exerciser") {
        win.set_pattern(fill_bar::Mode::Exerciser);
    }
    if let Ok(Some(spec)) = options.lookup::<String>("second-color") {
        match gdk::RGBA::parse(&spec) {
            Ok(rgba) => win.set_second_color(Some(rgba)),
            Err(_)   => inv.not_a_colour(&spec),
        }
    }
    if let Ok(Some(name)) = options.lookup::<String>("region") {
        match color_surface::Side::from_key(&name) {
            Some(side) => win.set_region(side),
            None => inv.complain(&i18n::format(
                &gettext("not a side: {0} (full, left, right, top, bottom)"),
                &[&name],
            )),
        }
    }

    if options.contains("identify") {
        if layer_shell_available() {
            win.set_identify(true);
        } else {
            // Saying nothing would look like the flag had worked.
            inv.complain(&gettext(
                "--identify needs wlr-layer-shell, which this compositor does not support",
            ));
        }
    }

    if let Ok(Some(minutes)) = options.lookup::<f64>("auto-hide") {
        if minutes < 0.0 {
            inv.complain(&gettext("--auto-hide cannot be negative"));
        } else {
            win.set_auto_hide(minutes.round() as u32);
        }
    }

    if let Ok(Some(secs)) = options.lookup::<f64>("interval") {
        win.set_cycle_interval(secs);
    }
    if options.contains("cycle") {
        win.set_cycle_armed(true);
    }

    // Last, so everything above is in place before anything lights up.
    if options.contains("hide") {
        win.hide_all_overlays();
    }
    if options.contains("show") {
        win.show_selected();
    }
}

fn main() -> glib::ExitCode {
    // Before anything that could produce a string, --help included.
    i18n::init();

    let app = adw::Application::builder()
        .application_id(APP_ID)
        // Without this the options would be parsed in each invocation and
        // then thrown away; with it they are forwarded to whichever instance
        // owns the window.
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    add_options(&app);

    // --version answers before a display is opened, so it works over ssh and
    // in a build script.
    app.connect_handle_local_options(|_, options| {
        if options.contains("version") {
            println!("whitescreen {APP_VERSION}");
            return std::ops::ControlFlow::Break(glib::ExitCode::SUCCESS);
        }
        std::ops::ControlFlow::Continue(())
    });

    // One-time setup: actions and accelerators are registered once per process.
    app.connect_startup(|app| {
        // Ctrl+Q must quit the application. It used to be bound to
        // "window.close", which targets whichever window has focus — pressing
        // it on an overlay destroyed that overlay and left a dangling entry
        // behind, so that monitor could never be filled again.
        let quit = gio::SimpleAction::new("quit", None);
        quit.connect_activate(glib::clone!(
            #[weak] app,
            move |_, _| app.quit()
        ));
        app.add_action(&quit);

        app.set_accels_for_action("win.about", &["F1"]);
        app.set_accels_for_action("app.quit",  &["<Ctrl>Q"]);
    });

    app.connect_command_line(|app, cmd| {
        let options = cmd.options_dict();

        // Answers that need no window at all.
        if options.contains("list-monitors") {
            list_monitors(cmd, options.contains("json"));
            return glib::ExitCode::SUCCESS;
        }

        let cold = find_window(app).is_none();
        let Some(win) = ensure_window(app) else {
            return glib::ExitCode::FAILURE;
        };

        let inv = Invocation::new(cmd);
        apply_options(&win, &inv);

        if cold {
            // `whitescreen --color red --all --show` from a keybind wants the
            // colour, not a control panel, so the window stays unseen while
            // something is on the screens. `--hide` with nothing running has
            // nothing to do and nothing to show, so it just leaves.
            if win.overlays_visible() {
                win.set_headless(true);
            } else if is_scripted(&options) {
                app.quit();
            } else {
                // Explicitly, rather than relying on the default: it is what
                // starts the parts that only make sense with the window on
                // screen (the gamma prober).
                win.set_headless(false);
                win.present();
            }
        } else if !has_options(&options) {
            // A bare `whitescreen` is "show me the app" -- which is also how
            // the window is summoned back from a headless instance.
            win.set_headless(false);
            win.present();
        }

        // Non-zero when any part of the invocation was rejected, so that a
        // script or a keybind wrapper can tell. The options that *were* good
        // have still been applied: a typo in one flag is not a reason to
        // ignore the others.
        inv.exit_code()
    });

    // Reached by D-Bus activation rather than a command line: someone asked
    // for the app itself, so show it.
    app.connect_activate(|app| {
        if let Some(win) = ensure_window(app) {
            win.set_headless(false);
            win.present();
        }
    });

    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_colour_splits_into_a_pattern_and_a_colour() {
        assert_eq!(
            parse_monitor_color("DP-1=red"),
            Ok(("DP-1".to_string(), gdk::RGBA::RED))
        );
        // The pattern is kept verbatim; matching it is the list's job.
        assert_eq!(
            parse_monitor_color("DP-*=#00FF00"),
            Ok(("DP-*".to_string(), gdk::RGBA::new(0.0, 1.0, 0.0, 1.0)))
        );
    }

    #[test]
    fn the_split_is_at_the_last_equals_sign() {
        // A monitor with no connector name is keyed by EDID text, which can
        // contain anything -- including the separator.
        assert_eq!(
            parse_monitor_color("Acme=1|Model X=blue"),
            Ok(("Acme=1|Model X".to_string(), gdk::RGBA::BLUE))
        );
    }

    #[test]
    fn surrounding_space_in_the_colour_is_ignored() {
        assert_eq!(
            parse_monitor_color("DP-1= white "),
            Ok(("DP-1".to_string(), gdk::RGBA::WHITE))
        );
    }

    #[test]
    fn a_pair_with_no_name_is_malformed() {
        assert_eq!(parse_monitor_color("DP-1"), Err(BadMonitorColor::Malformed));
        assert_eq!(parse_monitor_color("=red"), Err(BadMonitorColor::Malformed));
        assert_eq!(parse_monitor_color(""), Err(BadMonitorColor::Malformed));
    }

    #[test]
    fn a_bad_colour_is_reported_as_a_colour_and_not_as_a_typo_in_the_name() {
        assert_eq!(
            parse_monitor_color("DP-1=octarine"),
            Err(BadMonitorColor::NotAColor("octarine".to_string()))
        );
        assert_eq!(
            parse_monitor_color("DP-1="),
            Err(BadMonitorColor::NotAColor(String::new()))
        );
    }

}
