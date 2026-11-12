//! The portable Windows build.
//!
//! One executable, no installer, no registry and nothing written anywhere:
//! unzip it and run it, from a USB stick if you like. Everything it does is
//! said on the command line, which is also what makes it useful from a
//! shortcut or a script — the same way the GTK build is used from a
//! compositor keybind.
//!
//! The control window is not here yet, so there is no UI state to keep and
//! nothing to save. That is what makes "portable" easy today, and it is a
//! promise worth keeping when the window does arrive: settings should land
//! beside the executable, not in the registry.

#[cfg(not(windows))]
fn main() {
    // The workspace builds on Linux too, where this binary is a stub rather
    // than a compile error: the parts it drives are Win32.
    eprintln!("whitescreen: this executable is the Windows build; use the GTK app here");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use std::{thread::sleep, time::{Duration, Instant}};

    use whitescreen_core::text::json_string;
    use whitescreen_win::{
        KeepAwake, Overlay, OverlayEvent, VERSION, cli, monitors, pump, select_matching,
    };

    let opts = match cli::parse(std::env::args().skip(1)) {
        Ok(opts) => opts,
        Err(message) => {
            eprintln!("whitescreen: {message}");
            eprintln!("Try 'whitescreen --help'.");
            return std::process::ExitCode::FAILURE;
        }
    };

    match opts.action {
        cli::Action::Help => {
            print!("{}", cli::USAGE);
            return std::process::ExitCode::SUCCESS;
        }
        cli::Action::Version => {
            println!("whitescreen {VERSION}");
            return std::process::ExitCode::SUCCESS;
        }
        cli::Action::ListMonitors { json } => {
            list(&monitors(), json, json_string);
            return std::process::ExitCode::SUCCESS;
        }
        cli::Action::Show => {}
    }

    let screens = monitors();
    if screens.is_empty() {
        eprintln!("whitescreen: no monitors");
        return std::process::ExitCode::FAILURE;
    }

    // No --monitor means every screen, as --all does.
    let (chosen, unmatched): (Vec<_>, Vec<String>) = if opts.monitors.is_empty() {
        (screens.iter().collect(), Vec::new())
    } else {
        select_matching(&screens, &opts.monitors)
    };
    for pattern in &unmatched {
        eprintln!("whitescreen: no monitor matches {pattern}");
    }
    if chosen.is_empty() {
        eprintln!(
            "whitescreen: attached: {}",
            screens.iter().map(|m| m.key.as_str()).collect::<Vec<_>>().join(", ")
        );
        return std::process::ExitCode::FAILURE;
    }

    // Held for as long as the colour is up: a screen used as a key light or
    // left on a test pattern is one nobody touches for minutes.
    let _awake = KeepAwake::new();

    let color = opts.color.scaled(opts.brightness);
    let mut overlays = Vec::new();
    for screen in chosen {
        let mut overlay = match Overlay::new(screen) {
            Ok(overlay) => overlay,
            Err(e) => {
                eprintln!("whitescreen: cannot cover {}: {e}", screen.key);
                continue;
            }
        };
        overlay.set_color(color);
        overlay.set_fill(opts.fill);
        if let Err(e) = overlay.show(screen) {
            eprintln!("whitescreen: cannot show on {}: {e}", screen.key);
            continue;
        }
        overlays.push(overlay);
    }
    if overlays.is_empty() {
        return std::process::ExitCode::FAILURE;
    }

    let deadline = opts.auto_hide.map(|m| Instant::now() + Duration::from_secs(u64::from(m) * 60));
    while pump() {
        // Any key or click on any of them takes them all down, which is the
        // promise the other build makes too.
        if overlays.iter().any(|o| o.take_event() == Some(OverlayEvent::Dismissed)) {
            break;
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        // Idle between messages rather than spinning: there is nothing to do
        // until the user acts or the noise timer fires.
        sleep(Duration::from_millis(10));
    }
    std::process::ExitCode::SUCCESS
}

/// Print the monitor list, in the same shape the GTK build prints it.
#[cfg(windows)]
fn list(
    screens: &[whitescreen_win::Monitor],
    json: bool,
    quote: fn(&str) -> String,
) {
    if !json {
        for m in screens {
            println!("{}\t{} × {}", m.key, m.width, m.height);
        }
        return;
    }
    println!("[");
    for (i, m) in screens.iter().enumerate() {
        let comma = if i + 1 < screens.len() { "," } else { "" };
        println!(
            "  {{\"name\": {}, \"width\": {}, \"height\": {}, \"scale\": {}, \
             \"refresh_mhz\": {}, \"model\": {}, \"primary\": {}}}{comma}",
            quote(&m.key),
            m.width,
            m.height,
            m.scale,
            m.refresh_mhz,
            quote(&m.name),
            m.primary,
        );
    }
    println!("]");
}
