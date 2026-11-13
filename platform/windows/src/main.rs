//! The portable Windows build.
//!
//! One executable, no installer, no registry and nothing written anywhere:
//! unzip it and run it, from a USB stick if you like. Everything it does is
//! said on the command line, which is also what makes it useful from a
//! shortcut or a script — the same way the GTK build is used from a
//! compositor keybind.
//!
//! What the window changes is kept in `whitescreen.ini` beside the
//! executable, never in the registry, so the stick carries the settings with
//! it. If that directory is read-only -- a download folder, a share -- the
//! file falls back to the user's own profile rather than failing to save; see
//! `config`.

#[cfg(not(windows))]
fn main() {
    // The workspace builds on Linux too, where this binary is a stub rather
    // than a compile error: the parts it drives are Win32.
    eprintln!("whitescreen: this executable is the Windows build; use the GTK app here");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    use whitescreen_core::text::json_string;
    use whitescreen_win::{ControlWindow, Session, VERSION, cli, monitors};

    // Whether anything was asked for at all. `whitescreen.exe` on its own
    // opens the window; any flag means the caller wants the thing it names.
    let bare = std::env::args().len() <= 1;

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

    let mut session = match Session::new(&opts) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("whitescreen: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // A bare invocation is "show me the app"; one that puts something on the
    // screens does that and nothing else, so a keybind gets the colour and
    // not a control panel. The same rule as the GTK build.
    if bare {
        return match ControlWindow::new(session).and_then(|w| w.run()) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("whitescreen: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    let (chosen, unmatched) = session.chosen();
    for pattern in &unmatched {
        eprintln!("whitescreen: no monitor matches {pattern}");
    }
    if chosen.is_empty() {
        eprintln!("whitescreen: attached: {}", attached());
        return std::process::ExitCode::FAILURE;
    }

    if let Err(e) = session.run() {
        eprintln!("whitescreen: {e}");
        return std::process::ExitCode::FAILURE;
    }
    // Something was wrong with the invocation even though the colour went
    // up: say so in the exit code, as the GTK build does.
    if unmatched.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// The monitor names that are there, for when a pattern matched none.
#[cfg(windows)]
fn attached() -> String {
    whitescreen_win::monitors()
        .iter()
        .map(|m| m.key.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Print the monitor list, in the same shape the GTK build prints it.
#[cfg(windows)]
fn list(screens: &[whitescreen_win::Monitor], json: bool, quote: fn(&str) -> String) {
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
