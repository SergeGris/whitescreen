/// White Screen – modern GTK4 / Rust rewrite
///
/// Cargo.toml dependencies:
/// ```toml
/// [dependencies]
/// gtk4            = { version = "0.9",  package = "gtk4", features = ["v4_14"] }
/// gtk4-layer-shell = "0.9"
/// glib            = "0.20"
/// ```
use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::{gdk, gio, glib, prelude::*, subclass::prelude::*};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

// ══════════════════════════════════════════════════════════════════════════════
// Colour presets
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
struct Preset {
    name: &'static str,
    rgba: gdk::RGBA,
}

const PRESETS: &[Preset] = &[
    Preset { name: "White",  rgba: gdk::RGBA::new(1.0, 1.0, 1.0, 1.0) },
    Preset { name: "Black",  rgba: gdk::RGBA::new(0.0, 0.0, 0.0, 1.0) },
    Preset { name: "Red",    rgba: gdk::RGBA::new(1.0, 0.0, 0.0, 1.0) },
    Preset { name: "Green",  rgba: gdk::RGBA::new(0.0, 1.0, 0.0, 1.0) },
    Preset { name: "Blue",   rgba: gdk::RGBA::new(0.0, 0.0, 1.0, 1.0) },
];

// Prefix every class with the app-id slug to avoid collisions
// with the compositor's own GTK stylesheet.
const cssScreenBackground: &str = "screen-bg";
const cssScreenNotice: &str = "screen-notice";
const cssScreenNoticeBox: &str = "screen-notice-box";

// ══════════════════════════════════════════════════════════════════════════════
// ScreenOverlay  –  gtk-layer-shell fullscreen colour window
// ══════════════════════════════════════════════════════════════════════════════

mod screen_overlay {
    use super::*;

    // ─── Private GObject implementation ───────────────────────────────────
    mod imp {
        use super::*;
        use glib::Properties;

        #[derive(Properties)]
        #[properties(wrapper_type = super::ScreenOverlay)]
        pub struct ScreenOverlay {
            /// Current background colour.  Setting it immediately repaints CSS;
            /// no separate "apply" call is ever needed.
            #[property(get, set = Self::set_color, explicit_notify)]
            pub color: RefCell<gdk::RGBA>,

            /// Dedicated CSS provider so we never touch the global provider.
            pub css_provider: gtk::CssProvider,

            /// "Press ESC" toast.
            pub notice_box: gtk::Box,

            /// Handle for the pending auto-hide timeout.
            /// `None` means either the timer fired naturally or was cancelled.
            pub notice_timeout: RefCell<Option<glib::SourceId>>,
        }

        impl Default for ScreenOverlay {
            fn default() -> Self {
                Self {
                    color:          RefCell::new(gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)),
                    css_provider:   gtk::CssProvider::new(),
                    notice_box:     gtk::Box::new(gtk::Orientation::Vertical, 0),
                    notice_timeout: RefCell::new(None),
                }
            }
        }

        impl ScreenOverlay {
            // ── Custom property setter ─────────────────────────────────────
            fn set_color(&self, rgba: gdk::RGBA) {
                if *self.color.borrow() == rgba {
                    return; // guard: don't emit spurious notify
                }
                *self.color.borrow_mut() = rgba;
                self.reapply_css();
                self.obj().notify_color();
            }

            // ── CSS helpers ────────────────────────────────────────────────
            pub fn reapply_css(&self) {
                let c = self.color.borrow();
                self.css_provider.load_from_data(&format!(
                    ".{cssScreenBackground} {{
                        background-color: rgba({r},{g},{b},{a:.4});
                    }}
                    .{cssScreenNoticeBox} {{
                        background-color: rgba(20, 20, 20, 0.72);
                        border-radius: 16px;
                        padding: 16px 16px;
                    }}
                    .{cssScreenNotice} {{
                        color: white;
                        font-size: 20px;
                        font-weight: 600;
                    }}",
                    r = (c.red()   * 255.0) as u8,
                    g = (c.green() * 255.0) as u8,
                    b = (c.blue()  * 255.0) as u8,
                    a = c.alpha(),
                ));
            }

            /// Show the notice banner and start a 2-second auto-hide timer.
            /// Cancels any previously-armed timer first.
            pub fn arm_notice(&self) {
                self.disarm_notice();
                self.notice_box.set_visible(true);
                // Capture a weak ref to the *imp* struct so the closure can
                // clear `notice_timeout` when it fires naturally.  Without
                // this, notice_timeout would still hold the now-dead SourceId
                // and the next disarm_notice() would panic trying to remove it.
                let source_id = glib::timeout_add_local_once(
                    Duration::from_secs(2),
                    glib::clone!(
                        #[weak(rename_to = notice_box)]
                        self.notice_box,
                        #[weak(rename_to = imp)]
                        self,
                        move || {
                            notice_box.set_visible(false);
                            // Null out the ID: the source has already been
                            // consumed by GLib, so we must NOT call remove().
                            // TODO
                            // if let Some(id) = imp.notice_timeout.borrow_mut().take() {
                            //     id.remove();
                            // }
                            *imp.notice_timeout.borrow_mut() = None;
                        }));
                *self.notice_timeout.borrow_mut() = Some(source_id);
            }

            /// Cancel the pending timer (if any) and immediately hide the banner.
            ///
            /// Uses `take()` so we never call `remove()` on a dead source.
            pub fn disarm_notice(&self) {
                if let Some(id) = self.notice_timeout.borrow_mut().take() {
                    id.remove();
                }
                self.notice_box.set_visible(false);
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ScreenOverlay {
            const NAME: &str = "WhiteScreenOverlay";
            type Type        = super::ScreenOverlay;
            type ParentType  = gtk::ApplicationWindow;
        }

        impl ObjectImpl for ScreenOverlay {
            fn properties() -> &'static [glib::ParamSpec] {
                Self::derived_properties()
            }
            fn set_property(&self, id: usize, v: &glib::Value, pspec: &glib::ParamSpec) {
                self.derived_set_property(id, v, pspec);
            }
            fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
                self.derived_property(id, pspec)
            }

            fn constructed(&self) {
                self.parent_constructed();
                let win = self.obj();

                win.init_layer_shell();
                win.set_namespace(Some("whitescreen-overlay"));
                win.set_layer(Layer::Overlay);
                win.set_keyboard_mode(KeyboardMode::Exclusive);
                win.set_exclusive_zone(-1);
                for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                    win.set_anchor(edge, true);
                }
                win.set_decorated(false);

                if let Some(display) = gdk::Display::default() {
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &self.css_provider,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                }
                self.reapply_css();

                let root_overlay = gtk::Overlay::new();

                let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
                bg.set_hexpand(true);
                bg.set_vexpand(true);
                bg.add_css_class(cssScreenBackground);
                root_overlay.set_child(Some(&bg));

                {
                    let nb = &self.notice_box;
                    nb.set_halign(gtk::Align::Center);
                    nb.set_valign(gtk::Align::End);
                    nb.set_margin_bottom(28);
                    nb.add_css_class(cssScreenNoticeBox);
                    nb.set_visible(false);

                    nb.append(&{
                        let lbl = gtk::Label::new(Some("Press ESC to exit fullscreen"));
                        lbl.add_css_class(cssScreenNotice);
                        lbl
                    });
                    root_overlay.add_overlay(nb);
                }

                win.set_child(Some(&root_overlay));

                // ESC hides; every other key is swallowed.
                let esc = gtk::EventControllerKey::new();
                esc.set_propagation_phase(gtk::PropagationPhase::Capture);
                esc.connect_key_pressed(glib::clone!(
                    #[weak]
                    win,
                    #[upgrade_or]
                    glib::Propagation::Stop,
                    move |_, key, _, _| {
                        if key == gdk::Key::Escape {
                            win.hide_overlay();
                        }
                        glib::Propagation::Stop
                    }
                ));
                win.add_controller(esc);

                // Swallow all pointer clicks so nothing leaks through.
                let swallow = gtk::GestureClick::new();
                swallow.set_propagation_phase(gtk::PropagationPhase::Capture);
                swallow.connect_pressed(|_, _, _, _| {});
                win.add_controller(swallow);
            }
        }

        impl WidgetImpl            for ScreenOverlay {}
        impl WindowImpl            for ScreenOverlay {}
        impl ApplicationWindowImpl for ScreenOverlay {}
    }

    // ─── Public wrapper ────────────────────────────────────────────────────

    glib::wrapper! {
        /// A gtk-layer-shell window that fills the selected monitor with a
        /// solid colour.  Drive it exclusively through the `color` GObject
        /// property; use `show_on_monitor` / `hide_overlay` to control
        /// visibility.
        pub struct ScreenOverlay(ObjectSubclass<imp::ScreenOverlay>)
            @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
    }

    impl ScreenOverlay {
        pub fn new(app: &gtk::Application) -> Self {
            glib::Object::builder()
                .property("application", app)
                .build()
        }

        /// Present the overlay on `monitor`.  Passes `None` to use the
        /// compositor's primary monitor.
        pub fn show_on_monitor(&self, monitor: Option<&gdk::Monitor>) {
            self.imp().disarm_notice();
            self.set_monitor(monitor);
            self.present();
            // Blank cursor: no pointer visible on the fill surface.
            if let Some(surface) = self.surface() {
                surface.set_cursor(gdk::Cursor::from_name("none", None).as_ref());
            }
            self.imp().arm_notice();
        }

        /// Dismiss the overlay and restore the default cursor.
        pub fn hide_overlay(&self) {
            if self.is_visible() {
                if let Some(surface) = self.surface() {
                    surface.set_cursor(None);
                }
                self.set_visible(false);
            }
            self.imp().disarm_notice();
        }
    }
}

mod main_window {
    use super::*;
    use screen_overlay::ScreenOverlay;

    mod imp {
        use super::*;
        use std::cell::OnceCell;

        #[derive(Default)]
        pub struct MainWindow {
            /// Set exactly once in `new()`; guaranteed non-None for the
            /// window's lifetime so we can `unwrap()` freely.
            pub overlay: OnceCell<super::ScreenOverlay>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MainWindow {
            const NAME: &str = "WhiteScreenControlWindow";
            type Type        = super::MainWindow;
            type ParentType  = gtk::ApplicationWindow;
        }

        impl ObjectImpl            for MainWindow { fn constructed(&self) { self.parent_constructed(); } }
        impl WidgetImpl            for MainWindow { }
        impl WindowImpl            for MainWindow { }
        impl ApplicationWindowImpl for MainWindow { }
    }

    // ─── Public wrapper ────────────────────────────────────────────────────

    glib::wrapper! {
        pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
            @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl MainWindow {
        pub fn new(app: &gtk::Application, overlay: ScreenOverlay) -> Self {
            let win: Self = glib::Object::builder()
                .property("application", app)
                .property("title", "White Screen")
                .property("default-width",  800i32)
                .property("default-height", 600i32)
                .build();

            win.imp().overlay.set(overlay).expect("overlay injected exactly once");
            win.build_ui();
            win
        }

        fn overlay(&self) -> &ScreenOverlay {
            self.imp().overlay.get().unwrap()
        }

        fn build_ui(&self) {
            let overlay = self.overlay().clone();

            self.set_titlebar(Some(&gtk::HeaderBar::new()));

            let color_dialog = gtk::ColorDialog::builder()
                .title("Choose color")
                .with_alpha(false)
                .modal(true)
                .build();

            let color_btn = gtk::ColorDialogButton::builder()
                .dialog(&color_dialog)
                .rgba(&gdk::RGBA::new(1.0, 1.0, 1.0, 1.0))
                .build();

            // One-way binding: color_btn.rgba → overlay.color
            // `sync_create` sets the overlay colour at construction time so
            // both sides start in sync without extra imperative code.
            color_btn
                .bind_property("rgba", &overlay, "color")
                .sync_create()
                .build();

            // ── Monitor list ──────────────────────────────────────────────────
            let display    = gdk::Display::default().expect("no GDK display");
            let mon_model  = display.monitors();
            let mon_labels = gtk::StringList::new(&[]);

            let monitors: Rc<Vec<gdk::Monitor>> = Rc::new(
                (0..mon_model.n_items())
                    .filter_map(|i| mon_model.item(i)?.downcast::<gdk::Monitor>().ok())
                    .inspect(|m| {
let geo = m.geometry();
let desc = m.description()
    .map(|s| s.to_string())
    .unwrap_or_else(|| "Monitor".into());
mon_labels.append(&format!(
    "{desc}  —  {w}×{h}  (mm: {wmm}×{hmm})  scale={scale}  scale_factor={sf}  subpixel={sub}  connector={conn}  display={disp}  manufacturer={man}  model={model}  refresh={hz}.{ms:03} Hz",
    desc = desc,
    w = geo.width(),
    h = geo.height(),
    wmm = m.width_mm(),
    hmm = m.height_mm(),
    scale = m.scale_factor(),
    sf = m.scale_factor(),
    sub = format!("{:?}", m.subpixel_layout()),
    conn = m.connector().map(|s| s.to_string()).unwrap_or_default(),
    disp = m.display(),
    man = m.manufacturer().map(|s| s.to_string()).unwrap_or_default(),
    model = m.model().map(|s| s.to_string()).unwrap_or_default(),
    hz = m.refresh_rate() / 1000,
    ms = m.refresh_rate() % 1000,
));

                        // let geo  = m.geometry();
                        // let desc = m.description()
                        //     .map(|s| s.to_string())
                        //     .unwrap_or_else(|| "Monitor".into());
                        // mon_labels.append(&format!(
                        //     "{desc}  —  {w}×{h}  ×{scale}  {}.{:03} Hz  {}",
                        //     m.refresh_rate() / 1000,
                        //     m.refresh_rate() % 1000,
                        //     m.model().unwrap_or_default(),
                        //     w = geo.width(),
                        //     h = geo.height(),
                        //     scale = m.scale_factor(),
                        // ));
                    })
                    .collect(),
            );

            let mon_dropdown = gtk::DropDown::builder()
                .model(&mon_labels)
                .selected(0)
                .hexpand(true)
                .build();

            // ── Preview drawing area ──────────────────────────────────────────
            let preview = gtk::DrawingArea::new();
            preview.set_halign(gtk::Align::Center);
            preview.set_valign(gtk::Align::Center);

            // Draw the current overlay colour directly; no intermediate state.
            {
                preview.set_draw_func(glib::clone!(#[weak] overlay, move |_, cr, _, _| {
                    let c = overlay.color();
                    cr.set_source_rgb(c.red() as f64, c.green() as f64, c.blue() as f64);
                    let _ = cr.paint();
                }));
            }

            // Invalidate the preview whenever the overlay color changes.
            overlay.connect_color_notify(glib::clone!(
                #[weak] preview, move |_| preview.queue_draw()
            ));

            // Keep the preview's aspect ratio matching the selected monitor.
            // Stored in an Rc<dyn Fn()> so both the initial call and the
            // dropdown-changed handler share the same body without duplication.
            let sync_preview_size: Rc<dyn Fn()> = Rc::new(glib::clone!(
                #[weak]
                preview,
                #[weak]
                mon_dropdown,
                #[strong]
                monitors,
                move || {
                    if let Some(mon) = monitors.get(mon_dropdown.selected() as usize) {
                        let geo = mon.geometry();
                        let w   = 400i32;
                        let h   = (w as f64 * geo.height() as f64 / geo.width() as f64).round() as i32;
                        preview.set_size_request(w, h);
                    }
                }
            ));

            mon_dropdown.connect_selected_notify(glib::clone!(
                #[strong]
                sync_preview_size,
                move |_| sync_preview_size()
            ));

            sync_preview_size();

            // ── Preset colour buttons ─────────────────────────────────────────
            let preset_row = gtk::FlowBox::new();
            preset_row.set_halign(gtk::Align::Center);
            preset_row.set_max_children_per_line(u32::MAX);
            preset_row.set_selection_mode(gtk::SelectionMode::None);
            preset_row.set_row_spacing(4);
            preset_row.set_column_spacing(4);

            for &Preset { name, rgba } in PRESETS {
                // Swatch uses Cairo: no per-button CssProvider allocation.
                let swatch = gtk::DrawingArea::new();
                swatch.set_size_request(48, 24);
                swatch.set_draw_func(move |_, cr, w, h| {
                    cr.set_source_rgb(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64);
                    rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
                    let _ = cr.fill();
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.2);
                    rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
                    cr.set_line_width(0.5);
                    let _ = cr.stroke();
                    // cr.set_source_rgb(
                    //     rgba.red()   as f64,
                    //     rgba.green() as f64,
                    //     rgba.blue()  as f64,
                    // );
                    // rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
                    // let _ = cr.fill();
                });

                let label = gtk::Label::new(Some(name));
                label.set_halign(gtk::Align::Center);

                let btn = gtk::Button::new();
                btn.set_child(Some(&swatch));
                btn.set_tooltip_text(Some(name));
                btn.connect_clicked(glib::clone!(
                    #[weak] color_btn,
                    move |_| color_btn.set_rgba(&rgba)
                ));

                let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
                vbox.set_margin_top(8);
                vbox.set_margin_bottom(8);
                vbox.set_margin_start(8);
                vbox.set_margin_end(8);
                vbox.append(&btn);
                vbox.append(&label);

                preset_row.append(&vbox);
            }

            // Custom colour picker lives in the same row as the presets.
            {
                let wrap = gtk::Box::new(gtk::Orientation::Vertical, 4);
                wrap.set_margin_top(8);
                wrap.set_margin_bottom(8);
                wrap.set_margin_start(8);
                wrap.set_margin_end(8);
                wrap.append(&color_btn);
                let lbl = gtk::Label::new(Some("Custom"));
                lbl.set_halign(gtk::Align::Center);
                wrap.append(&lbl);
                preset_row.append(&wrap);
            }

            let show_btn = gtk::Button::with_label("Show Fullscreen");
            show_btn.add_css_class("suggested-action");

            let hide_btn = gtk::Button::with_label("Hide Fullscreen");

            show_btn.connect_clicked(glib::clone!(
                #[weak]
                overlay,
                #[strong]
                monitors,
                #[weak]
                mon_dropdown,
                move |_| overlay.show_on_monitor(monitors.get(mon_dropdown.selected() as usize))
            ));
            hide_btn.connect_clicked(glib::clone!(
                #[weak]
                overlay,
                move |_| overlay.hide_overlay()
            ));

            // // ── Layout ────────────────────────────────────────────────────────
            // let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
            // root.set_margin_start(24);
            // root.set_margin_end(24);
            // root.set_margin_top(24);
            // root.set_margin_bottom(24);
            // root.set_halign(gtk::Align::Center);

            let mon_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            mon_row.set_halign(gtk::Align::Fill);
            mon_row.append(&gtk::Label::new(Some("Monitor:")));
            mon_row.append(&mon_dropdown);

            let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            action_row.set_halign(gtk::Align::Center);
            action_row.append(&show_btn);
            action_row.append(&hide_btn);

            let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
            root.set_margin_start(24);
            root.set_margin_end(24);
            root.set_margin_top(24);
            root.set_margin_bottom(24);
            root.set_halign(gtk::Align::Center);

            root.append(&mon_row);
            root.append(&preset_row);
            root.append(&preview);
            root.append(&action_row);
            self.set_child(Some(&root));

            // Destroy the layer-shell window cleanly when the main window closes
            // so the compositor removes it from the layer-surface stack.
            self.connect_destroy(move |_| {
                //// TODO
                overlay.set_color(gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
                overlay.set_monitor(None);
                overlay.present(); // TODO
                ////
                overlay.hide_overlay();
                overlay.destroy();
            });
        }
    }

    pub use MainWindow as Window;
}

/// Draw a rounded rectangle path into `cr` (does not stroke/fill).
fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -FRAC_PI_2,       0.0          );
    cr.arc(x + w - r, y + h - r, r,  0.0,              FRAC_PI_2    );
    cr.arc(x + r,     y + h - r, r,  FRAC_PI_2,        PI           );
    cr.arc(x + r,     y + r,     r,  PI,       3.0 * FRAC_PI_2      );
    cr.close_path();
}

fn main() {
    // Single-instance enforcement via GApplication's built-in D-Bus mechanism.
    // The second instance will activate the first and exit immediately.
    let app = gtk::Application::builder()
        // .application_id(APP_ID)
        .flags(gio::ApplicationFlags::FLAGS_NONE)
        .build();

    app.connect_activate(|app| {
        // Guard: if a window is already open (second activate on the same
        // instance), just raise it instead of building a second one.
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }

        if !gtk_layer_shell::is_supported() {
            // Show a proper error dialog instead of a silent eprintln.
            let dialog = gtk::AlertDialog::builder()
                .message("Wayland compositor required")
                .detail(
                    "gtk4-layer-shell is not supported by the current \
                     compositor.\n\nWhite Screen requires a Wayland compositor \
                     that implements the wlr-layer-shell protocol (e.g. Niri, Sway, \
                     Hyprland, GNOME 45+, KDE Plasma 6+).",
                )
                .modal(true)
                .build();
            // `choose` is non-blocking; the app will exit once the dialog is dismissed.
            dialog.choose(gtk::Window::NONE, gio::Cancellable::NONE, |_| {});
            return;
        }
        let overlay = screen_overlay::ScreenOverlay::new(app);
        let win     = main_window::Window::new(app, overlay);
        win.present();
    });

    app.run();
}
