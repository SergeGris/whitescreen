// White Screen – fill any monitor with a solid color.

use std::sync::OnceLock;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod screen_overlay;
mod color_surface;
mod monitor_label;
mod main_window;
mod settings;

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
    *SUPPORTED.get_or_init(|| {
        // Escape hatch, in the same spirit as WHITESCREEN_NO_GAMMA: it forces
        // fallback mode on a compositor that does support layer shell, which
        // is the only way to exercise that path without booting GNOME.
        if std::env::var_os("WHITESCREEN_NO_LAYER_SHELL").is_some() {
            return false;
        }
        gtk_layer_shell::is_supported()
    })
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
        .buttons(["Close"])
        .modal(true)
        .build()
        .choose(gtk::Window::NONE, gio::Cancellable::NONE, move |_| {
            drop(hold);
            app.quit();
        });
}

/// Print everything needed to tell "wrong compositor" apart from "GDK fell
/// back to X11 inside the sandbox". The app keeps running either way now, so
/// this is the only place the distinction is recorded.
fn report_no_layer_shell() {
    eprintln!("whitescreen: gtk_layer_is_supported() = false");
    eprintln!(
        "  GDK display         : {}",
        gdk::Display::default()
            .map(|d| d.type_().name().to_string())
            .unwrap_or_else(|| "<none>".to_string()),
    );
    for (label, var) in [
        ("XDG_SESSION_TYPE   ", "XDG_SESSION_TYPE"),
        ("XDG_CURRENT_DESKTOP", "XDG_CURRENT_DESKTOP"),
        ("WAYLAND_DISPLAY    ", "WAYLAND_DISPLAY"),
        ("LD_PRELOAD         ", "LD_PRELOAD"),
    ] {
        eprintln!("  {} : {:?}", label, std::env::var(var).ok());
    }
    eprintln!(
        "  gtk4-layer-shell    : {}.{}.{}",
        gtk_layer_shell::major_version(),
        gtk_layer_shell::minor_version(),
        gtk_layer_shell::micro_version(),
    );
    eprintln!("  falling back to fullscreen windows; Identify is unavailable");
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();

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

    app.connect_activate(|app| {
        // Single-instance: raise the existing window instead of opening another.
        if let Some(win) = app.windows().into_iter()
            .find(|w| w.is::<main_window::Window>())
        {
            win.present();
            return;
        }

        if gdk::Display::default().is_none() {
            fatal(
                app,
                "No display connection",
                "White Screen could not connect to a display server.",
            );
            return;
        }

        // No layer shell is no longer fatal. Refusing to start meant the app
        // was unusable on GNOME and X11 even though filling a screen with a
        // colour needs nothing more than a fullscreen window there; the
        // window says so in a banner, and this records the details.
        if !layer_shell_available() {
            report_no_layer_shell();
        }

        // Monitors are enumerated by the window itself, which also tracks
        // hot-plug. Enumerating here as well used to produce a second, parallel
        // list that the window then indexed into positionally.
        main_window::Window::new(app).present();
    });

    app.run()
}
