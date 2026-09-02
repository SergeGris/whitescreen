// White Screen – fill any monitor with a solid color.

use adw::prelude::*;
use gtk::{gdk, gio, glib};

mod screen_overlay;
mod color_surface;
mod monitor_label;
mod main_window;

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

        if !gtk_layer_shell::is_supported() {
            gtk::AlertDialog::builder()
                .message("Compositor not supported")
                .detail(
                    "White Screen requires a Wayland compositor that supports \
                     the wlr-layer-shell protocol (e.g. Niri, Sway, Hyprland, \
                     Wayfire, KDE Plasma ≥ 6).\n\n\
                     GNOME/Mutter and X11 are not supported."
                )
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        // Monitors are enumerated by the window itself, which also tracks
        // hot-plug. Enumerating here as well used to produce a second, parallel
        // list that the window then indexed into positionally.
        if gdk::Display::default().is_none() {
            gtk::AlertDialog::builder()
                .message("No display connection")
                .detail("White Screen could not connect to a Wayland display.")
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        main_window::Window::new(app).present();
    });

    app.run()
}
