/// White Screen – fill any monitor with a solid color.

use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::{gdk, gio, glib, prelude::*, subclass::prelude::*};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[cfg(feature = "gamma")]
mod gamma;
#[cfg(feature = "gamma")]
use gamma::GammaListener;

const APP_ID:      &str = "io.github.SergeGris.WhiteScreen";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Preset {
    name: &'static str,
    rgba: gdk::RGBA,
}

const PRESETS: &[Preset] = &[
    Preset { name: "White",    rgba: gdk::RGBA::new(1.0, 1.0, 1.0, 1.0) },
    Preset { name: "Black",    rgba: gdk::RGBA::new(0.0, 0.0, 0.0, 1.0) },
    Preset { name: "Red",      rgba: gdk::RGBA::new(1.0, 0.0, 0.0, 1.0) },
    Preset { name: "Green",    rgba: gdk::RGBA::new(0.0, 1.0, 0.0, 1.0) },
    Preset { name: "Blue",     rgba: gdk::RGBA::new(0.0, 0.0, 1.0, 1.0) },
];

mod css_class {
    // Prefix every class with the app-id slug to avoid collisions
    // with the compositor's own GTK stylesheet.
    pub const BACKGROUND: &str = "whitescreen-background";
    pub const NOTICE: &str = "whitescreen-notice";
    pub const NOTICE_BOX: &str = "whitescreen-notice-box";
}

// ScreenOverlay  –  gtk-layer-shell fullscreen color window
mod screen_overlay {
    use super::*;

    mod imp {
        use super::*;
        use glib::Properties;

        #[derive(Properties)]
        #[properties(wrapper_type = super::ScreenOverlay)]
        pub struct ScreenOverlay {
            /// Current background color.  Setting this property immediately
            /// repaints the CSS — no separate "apply" call needed.
            #[property(get, set = Self::set_color, explicit_notify)]
            pub color: RefCell<gdk::RGBA>,

            /// CSS provider for all `.wsov-*` rules.
            pub css_provider: gtk::CssProvider,

            /// "Press ESC" toast wrapped in a Revealer for a smooth
            /// cross-fade animation.
            pub notice_revealer: gtk::Revealer,

            /// Handle for the pending auto-hide timeout.
            /// `None` when the timer has already fired or been cancelled.
            pub notice_timeout: RefCell<Option<glib::SourceId>>,
        }

        impl Default for ScreenOverlay {
            fn default() -> Self {
                // Build the notice widget hierarchy here so we can store
                // the Revealer directly in the struct.
                let notice_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                notice_box.add_css_class(css_class::NOTICE_BOX);
                let lbl = gtk::Label::new(Some("Press ESC to exit"));
                lbl.add_css_class(css_class::NOTICE);
                notice_box.append(&lbl);

                let notice_revealer = gtk::Revealer::builder()
                    .transition_type(gtk::RevealerTransitionType::Crossfade)
                    .transition_duration(200)
                    .reveal_child(false)
                    .child(&notice_box)
                    .build();

                Self {
                    color:          RefCell::new(gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)),
                    css_provider:   gtk::CssProvider::new(),
                    notice_revealer,
                    notice_timeout: RefCell::new(None),
                }
            }
        }

        impl ScreenOverlay {
            // ── Property setter ────────────────────────────────────────────
            fn set_color(&self, rgba: gdk::RGBA) {
                if *self.color.borrow() == rgba { return; }
                *self.color.borrow_mut() = rgba;
                self.reapply_css();
                self.obj().notify_color();
            }

            // ── CSS ────────────────────────────────────────────────────────
            /// Rebuild and load the `.wsov-*` stylesheet.
            ///
            /// The notice box and text colors are **adaptive**: dark
            /// background + light text when the fill is bright; light
            /// background + dark text when it is dark.  Perceived brightness
            /// is computed with the standard NTSC luma coefficients.
            pub fn reapply_css(&self) {
                let c = self.color.borrow();
                let luma =
                    c.red()   * 0.299 +
                    c.green() * 0.587 +
                    c.blue()  * 0.114;

                let (box_bg, text_col) = if luma > 0.5 {
                    ("rgba(12,12,12,0.80)",   "rgba(242,242,242,0.97)")
                } else {
                    ("rgba(243,243,243,0.80)", "rgba(18,18,18,0.97)")
                };

                self.css_provider.load_from_data(&format!(
                    ".{background} {{
                        background-color: rgba({r},{g},{b},{a:.4});
                    }}
                    .{notice_box} {{
                        background-color: {box_bg};
                        border-radius: 12px;
                        padding: 14px 22px;
                    }}
                    .{notice} {{
                        color: {text_col};
                        font-size: 17px;
                        font-weight: 600;
                    }}",
                    r = (c.red()   * 255.0) as u8,
                    g = (c.green() * 255.0) as u8,
                    b = (c.blue()  * 255.0) as u8,
                    a = c.alpha(),
                    background = css_class::BACKGROUND,
                    notice_box = css_class::NOTICE_BOX,
                    notice     = css_class::NOTICE,
                ));
            }

            // ── Notice helpers ─────────────────────────────────────────────

            /// Show the "press ESC" toast and (re)start the 2-second
            /// auto-hide countdown.  Calling this from a motion handler
            /// resets the timer each time the cursor moves, so the toast
            /// stays visible while the mouse is in motion and disappears
            /// 2 s after it stops.
            pub fn arm_notice(&self) {
                self.disarm_notice();
                self.notice_revealer.set_reveal_child(true);

                let source_id = glib::timeout_add_local_once(
                    Duration::from_secs(2),
                    glib::clone!(
                        #[weak(rename_to = rev)] self.notice_revealer,
                        #[weak(rename_to = imp)] self,
                        move || {
                            rev.set_reveal_child(false);
                            // Only take() — never remove() inside a one-shot
                            // callback: GLib removes the source automatically
                            // before invoking it, so calling remove() here
                            // would panic ("Source ID N was not found").
                            imp.notice_timeout.borrow_mut().take();
                        }
                    ),
                );
                *self.notice_timeout.borrow_mut() = Some(source_id);
            }

            /// Cancel the pending timer and hide the toast immediately.
            pub fn disarm_notice(&self) {
                // take() prevents a double-remove on the same ID.
                if let Some(id) = self.notice_timeout.borrow_mut().take() {
                    id.remove(); // safe: source is still pending here
                }
                self.notice_revealer.set_reveal_child(false);
            }
        }

        // ── GObject boilerplate ────────────────────────────────────────────

        #[glib::object_subclass]
        impl ObjectSubclass for ScreenOverlay {
            const NAME: &str = "WhiteScreenOverlay";
            type Type        = super::ScreenOverlay;
            type ParentType  = gtk::ApplicationWindow;
        }

        impl ObjectImpl for ScreenOverlay {
            fn properties() -> &'static [glib::ParamSpec] { Self::derived_properties() }
            fn set_property(&self, id: usize, v: &glib::Value, p: &glib::ParamSpec) {
                self.derived_set_property(id, v, p);
            }
            fn property(&self, id: usize, p: &glib::ParamSpec) -> glib::Value {
                self.derived_property(id, p)
            }

            fn constructed(&self) {
                self.parent_constructed();
                let win = self.obj();

                // ── gtk-layer-shell setup ──────────────────────────────────
                win.init_layer_shell();
                win.set_namespace(Some("whitescreen-overlay"));
                win.set_layer(Layer::Overlay);
                // Exclusive: this window receives ALL keyboard input while
                // visible; no other client can steal keys.
                win.set_keyboard_mode(KeyboardMode::Exclusive);
                win.set_exclusive_zone(-1);
                for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                    win.set_anchor(edge, true);
                }
                win.set_decorated(false);

                // ── Register CSS provider globally ─────────────────────────
                if let Some(display) = gdk::Display::default() {
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &self.css_provider,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                }
                self.reapply_css();

                // ── Widget tree ────────────────────────────────────────────
                let root_overlay = gtk::Overlay::new();

                let bg = gtk::Box::new(gtk::Orientation::Vertical, 0);
                bg.set_hexpand(true);
                bg.set_vexpand(true);
                bg.set_focusable(true); // required so grab_focus() lands here
                bg.add_css_class(css_class::BACKGROUND);
                root_overlay.set_child(Some(&bg));

                // Toast anchored to bottom-centre.
                let rev = &self.notice_revealer;
                rev.set_halign(gtk::Align::Center);
                rev.set_valign(gtk::Align::End);
                rev.set_margin_bottom(32);
                root_overlay.add_overlay(rev);

                win.set_child(Some(&root_overlay));

                // ── Keyboard: ESC closes; swallow everything else ──────────
                let esc = gtk::EventControllerKey::new();
                esc.set_propagation_phase(gtk::PropagationPhase::Capture);
                esc.connect_key_pressed(glib::clone!(
                    #[weak] win,
                    #[upgrade_or] glib::Propagation::Stop,
                    move |_, key, _, _| {
                        if key == gdk::Key::Escape { win.hide_overlay(); }
                        glib::Propagation::Stop
                    }
                ));
                win.add_controller(esc);

                // ── Pointer: swallow all clicks ────────────────────────────
                let swallow = gtk::GestureClick::new();
                swallow.set_propagation_phase(gtk::PropagationPhase::Capture);
                swallow.connect_pressed(|_, _, _, _| {});
                win.add_controller(swallow);

                // ── Motion: show/rearm the toast on every cursor movement ──
                // Each `motion` signal resets the 2-second countdown via
                // arm_notice(), so the toast remains visible while the cursor
                // is moving and fades out 2 s after it stops.
                let motion = gtk::EventControllerMotion::new();
                motion.set_propagation_phase(gtk::PropagationPhase::Capture);
                motion.connect_motion(glib::clone!(
                    #[weak(rename_to = imp)] self,
                    move |_, dx, dy| {
                        // TODO
                        if dx + dy > 3000.0 {
                            imp.arm_notice();
                        }
                    },
                ));
                win.add_controller(motion);
            }
        }

        impl WidgetImpl            for ScreenOverlay {}
        impl WindowImpl            for ScreenOverlay {}
        impl ApplicationWindowImpl for ScreenOverlay {}
    }

    // ─── Public wrapper ────────────────────────────────────────────────────

    glib::wrapper! {
        /// Layer-shell window that fills the selected monitor with a solid
        /// color.  Public API:
        ///   • `color` — GObject property (bind_property-compatible)
        ///   • `show_on_monitor` / `hide_overlay`
        pub struct ScreenOverlay(ObjectSubclass<imp::ScreenOverlay>)
            @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap,
                        gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                        gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl ScreenOverlay {
        pub fn new(app: &gtk::Application) -> Self {
            glib::Object::builder().property("application", app).build()
        }

        /// Present the overlay on `monitor` (primary when `None`).
        /// Blanks the cursor, grabs focus, and arms the ESC toast.
        pub fn show_on_monitor(&self, monitor: Option<&gdk::Monitor>) {
            let imp = self.imp();
            imp.disarm_notice();
            self.set_monitor(monitor);
            self.present();

            // grab_focus() ensures the GTK focus chain is rooted here.
            // KeyboardMode::Exclusive already grabs the Wayland keyboard seat,
            // so together these give us complete input ownership.
            self.grab_focus();

            // Blank cursor — nothing visible over a clean color fill.
            if let Some(surface) = self.surface() {
                surface.set_cursor(gdk::Cursor::from_name("none", None).as_ref());
            }
            imp.arm_notice();
        }

        /// Dismiss the overlay and restore the default cursor.
        ///
        /// Safe to call in any state — before the window has ever been shown
        /// (no live GdkSurface) and during app shutdown.
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

// ══════════════════════════════════════════════════════════════════════════════
// MainWindow
// ══════════════════════════════════════════════════════════════════════════════

mod main_window {
    use super::*;
    use screen_overlay::ScreenOverlay;

    // ─── Monitor sidebar row ───────────────────────────────────────────────
    //
    //   ┌─────────────────────────────────────────────────┐
    //   │  ┌──────────┐   HDMI-A-1                        │
    //   │  │  thumb   │   2560 × 1440                     │
    //   │  └──────────┘   143.999 Hz  ×2  HiDPI           │
    //   └─────────────────────────────────────────────────┘
    //
    fn make_monitor_row(mon: &gdk::Monitor, index: usize) -> gtk::ListBoxRow {
        let geo   = mon.geometry();
        let geo_w = geo.width();
        let geo_h = geo.height();

        // ── Thumbnail ──────────────────────────────────────────────────────
        let thumb = gtk::DrawingArea::new();
        let aspect   = geo_w as f64 / geo_h.max(1) as f64;
        thumb.set_size_request((40.0 * aspect) as i32, 40);
        thumb.set_valign(gtk::Align::Center);
        thumb.set_draw_func(move |_, cr, cw, ch| {
            let (cw, ch) = (cw as f64, ch as f64);
            // Fit the rectangle inside the drawing area preserving aspect ratio.
            let (rw, rh) = if aspect >= cw / ch {
                (cw - 2.0, (cw - 2.0) / aspect)
            } else {
                ((ch - 2.0) * aspect, ch - 2.0)
            };
            let (rx, ry) = ((cw - rw) * 0.5, (ch - rh) * 0.5);

            // Fill
            cr.set_source_rgba(0.40, 0.40, 0.44, 0.28);
            super::rounded_rect(cr, rx, ry, rw, rh, 3.0);
            let _ = cr.fill();

            // Border
            cr.set_source_rgba(0.50, 0.50, 0.55, 0.70);
            super::rounded_rect(cr, rx + 0.5, ry + 0.5, rw - 1.0, rh - 1.0, 3.0);
            cr.set_line_width(1.0);
            let _ = cr.stroke();

            // Monitor number centred in the thumbnail
            cr.set_source_rgba(0.55, 0.55, 0.60, 1.0);
            cr.set_font_size(10.0);
            let s   = (index + 1).to_string();
            let ext = cr.text_extents(&s).unwrap();
            cr.move_to(
                rx + (rw - ext.width())  * 0.5 - ext.x_bearing(),
                ry + (rh + ext.height()) * 0.5,
            );
            let _ = cr.show_text(&s);
        });

        // ── Text labels ────────────────────────────────────────────────────
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_hexpand(true);
        vbox.set_valign(gtk::Align::Center);

        let conn = mon.connector()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Monitor {}", index + 1));

        let name_lbl = gtk::Label::new(None);
        name_lbl.set_markup(&format!("<b>{}</b>", glib::markup_escape_text(&conn)));
        name_lbl.set_halign(gtk::Align::Start);
        name_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let res_lbl = gtk::Label::new(Some(&format!("{geo_w}×{geo_h}")));
        res_lbl.set_halign(gtk::Align::Start);
        res_lbl.add_css_class("dim-label");

        let hz    = mon.refresh_rate();
        let scale = mon.scale_factor();
        let detail = if scale > 1 {
            format!(
                "{}.{:03} Hz  ×{}  HiDPI",
                hz / 1000,
                hz % 1000,
                scale,
            )
        } else {
            format!("{}.{:03} Hz", hz / 1000, hz % 1000)
        };
        let hz_lbl = gtk::Label::new(Some(&detail));
        hz_lbl.set_halign(gtk::Align::Start);
        hz_lbl.add_css_class("dim-label");

        vbox.append(&name_lbl);
        vbox.append(&res_lbl);
        vbox.append(&hz_lbl);

        // ── Row container ──────────────────────────────────────────────────
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_top(8);
        hbox.set_margin_bottom(8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.append(&thumb);
        hbox.append(&vbox);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&hbox));
        row
    }

    // ─── GObject implementation ────────────────────────────────────────────

    mod imp {
        use super::*;
        use std::cell::OnceCell;

        #[derive(Default)]
        pub struct MainWindow {
            /// Set exactly once in `new()`; `unwrap()`-safe for the window's
            /// entire lifetime.
            pub overlay: OnceCell<super::ScreenOverlay>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MainWindow {
            const NAME: &str = "WhiteSpaceMainWindow";
            type Type        = super::MainWindow;
            type ParentType  = gtk::ApplicationWindow;
        }

        impl ObjectImpl            for MainWindow { fn constructed(&self) { self.parent_constructed(); } }
        impl WidgetImpl            for MainWindow {}
        impl WindowImpl            for MainWindow {}
        impl ApplicationWindowImpl for MainWindow {}
    }

    glib::wrapper! {
        pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
            @extends gtk::ApplicationWindow, gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap,
                        gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                        gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl MainWindow {
        pub fn new(app: &gtk::Application, overlay: ScreenOverlay) -> Self {
            let win: Self = glib::Object::builder()
                .property("application", app)
                .property("title", "White Screen")
                .property("default-width",  800i32)
                .property("default-height",  600i32)
                // .property("resizable", false)
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

            let header   = gtk::HeaderBar::new();
            let menu_btn = gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Menu")
                .build();
            let menu = gio::Menu::new();
            menu.append(Some("About White Screen"), Some("win.about"));
            menu_btn.set_menu_model(Some(&menu));
            header.pack_end(&menu_btn);
            self.set_titlebar(Some(&header));

            let about = gio::SimpleAction::new("about", None);
            about.connect_activate(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_, _| {
                    gtk::AboutDialog::builder()
                        .transient_for(&win)
                        .modal(true)
                        .program_name("White Screen")
                        .version(super::APP_VERSION)
                        .comments(
                            "Fill any monitor with a solid color.\n\
                             Useful for photography, video production, and display testing."
                        )
                        .license_type(gtk::License::Gpl30)
                        // TODO
                        .can_focus(false)
                        // .can_target(false)
                        .build()
                        .present();
                }
            ));
            self.add_action(&about);

            let color_dialog = gtk::ColorDialog::builder()
                .title("Choose color")
                .with_alpha(false)
                .modal(true)
                .build();

            let color_btn = gtk::ColorDialogButton::builder()
                .dialog(&color_dialog)
                .rgba(&gdk::RGBA::new(1.0, 1.0, 1.0, 1.0))
                .valign(gtk::Align::Center)
                .build();

            // One-way binding: color_btn.rgba → overlay.color.
            // sync_create initialises the overlay immediately.
            color_btn
                .bind_property("rgba", &overlay, "color")
                .sync_create()
                .build();

            // ── Monitor list ──────────────────────────────────────────────
            let display   = gdk::Display::default().expect("no GDK display");
            let mon_model = display.monitors();

            let monitors: Rc<Vec<gdk::Monitor>> = Rc::new(
                (0..mon_model.n_items())
                    .filter_map(|i| mon_model.item(i)?.downcast::<gdk::Monitor>().ok())
                    .collect(),
            );

            let mon_list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::Single)
                .build();
            mon_list.add_css_class("navigation-sidebar");

            for (i, mon) in monitors.iter().enumerate() {
                mon_list.append(&make_monitor_row(mon, i));
            }
            // Pre-select the first row.
            if let Some(first) = mon_list.row_at_index(0) {
                mon_list.select_row(Some(&first));
            }

            // Helper closure: returns the index of the currently selected row.
            let get_selected_idx = {
                glib::clone!(
                    #[strong]
                    mon_list,
                    move || -> usize {
                        mon_list.selected_row()
                        .map(|r| r.index() as usize)
                        .unwrap_or(0)
                })
            };

            // ── Preview DrawingArea ───────────────────────────────────────
            let preview = gtk::DrawingArea::builder()
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .css_classes(["shadowed"])
                .build();

            {
                preview.set_draw_func(
                    glib::clone!(
                        #[weak]
                        overlay,
                        move |_, cr, w, h| {
                            let c = overlay.color();
                            cr.set_source_rgb(c.red() as f64, c.green() as f64, c.blue() as f64);
                            let _ = cr.paint();
                            // Thin border so a white preview is visible on light themes.
                            cr.set_source_rgba(0.0, 0.0, 0.0, 0.15);
                            cr.rectangle(0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0);
                            cr.set_line_width(1.0);
                            let _ = cr.stroke();
                        }));
            }

            {
                let t = gtk::CssProvider::new();

                t.load_from_data(r#"
                    .shadowed {
                        box-shadow: 0px 8px 16px rgba(0, 0, 0, 0.4);
                        border-radius: 8px;
                    }
                    "#);

                if let Some(display) = gdk::Display::default() {
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &t,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                }

                let click = gtk::GestureClick::new();
                click.connect_pressed(glib::clone!(
                    #[weak] overlay,
                    #[strong] monitors,
                    #[strong] get_selected_idx,
                    move |_, _n_press, _x, _y| {
                        overlay.show_on_monitor(monitors.get(get_selected_idx()));
                    }));
                preview.add_controller(click);
            }

            // Invalidate preview whenever the color changes.
            overlay.connect_color_notify(glib::clone!(
                #[weak] preview,
                move |_| preview.queue_draw()
            ));

            // Keep preview aspect ratio matching the selected monitor.
            let sync_preview_size: Rc<dyn Fn()> = Rc::new({
                glib::clone!(
                    #[weak]
                    preview,
                    #[strong]
                    monitors,
                    #[strong]
                    get_selected_idx,
                    move || {
                        if let Some(mon) = monitors.get(get_selected_idx()) {
                            let geo = mon.geometry();
                            let w   = 480i32;
                            let h   = (w as f64 * geo.height() as f64 / geo.width().max(1) as f64).round() as i32;
                            preview.set_size_request(w, h);
                        }
                    })
            });

            mon_list.connect_row_selected({
                glib::clone!(
                    #[strong]
                    sync_preview_size,
                    move |_, _| sync_preview_size()
            )});
            sync_preview_size();

            // ── Preset buttons ────────────────────────────────────────────
            let preset_row = gtk::Box::builder()
                .halign(gtk::Align::Center)
                .spacing(4)
                .build();

            for &Preset { name, rgba } in PRESETS {
                let swatch = gtk::DrawingArea::new();
                swatch.set_size_request(48, 24);
                swatch.set_draw_func(move |_, cr, w, h| {
                    cr.set_source_rgb(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64);
                    super::rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
                    let _ = cr.fill();
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.20);
                    super::rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
                    cr.set_line_width(0.5);
                    let _ = cr.stroke();
                });

                let lbl = gtk::Label::new(Some(name));
                lbl.set_halign(gtk::Align::Center);

                let btn = gtk::Button::new();
                btn.set_child(Some(&swatch));
                btn.set_tooltip_text(Some(name));
                btn.connect_clicked(glib::clone!(
                    #[weak] color_btn,
                    move |_| color_btn.set_rgba(&rgba)
                ));

                let cell = gtk::Box::new(gtk::Orientation::Vertical, 4);
                cell.set_margin_top(6);
                cell.set_margin_bottom(6);
                cell.set_margin_start(6);
                cell.set_margin_end(6);
                cell.append(&btn);
                cell.append(&lbl);
                preset_row.append(&cell);
            }

            // Custom color picker appended to the preset row.
            {
                let wrap = gtk::Box::new(gtk::Orientation::Vertical, 4);
                wrap.set_margin_top(6);
                wrap.set_margin_bottom(6);
                wrap.set_margin_start(8);
                wrap.set_margin_end(8);
                wrap.append(&color_btn);
                let lbl = gtk::Label::new(Some("Custom"));
                lbl.set_halign(gtk::Align::Center);
                wrap.append(&lbl);
                preset_row.append(&wrap);
            }

            // ── Show / Hide buttons ───────────────────────────────────────
            let show_btn = gtk::Button::with_label("Show Fullscreen");
            show_btn.add_css_class("suggested-action");

            let hide_btn = gtk::Button::builder()
                .label("Hide")
                .tooltip_text("Hide fullscreen overlay  (or press ESC on the overlay)")
                .build();

                show_btn.connect_clicked(glib::clone!(
                    #[weak]
                    overlay,
                    #[strong]
                    monitors,
                    #[strong]
                    get_selected_idx,
                    move |_| {
                    overlay.show_on_monitor(monitors.get(get_selected_idx()));
                }));

            hide_btn.connect_clicked(glib::clone!(#[weak] overlay, move |_| overlay.hide_overlay()));

            //////////
            #[cfg(feature = "gamma")]
            {
                let gamma_label = gtk::Label::new(None);

                let (sender, receiver) = std::sync::mpsc::channel::<bool>();

                // 2. Attach an idle callback that polls the receiver.
                //    We need to keep the label alive; use Rc<RefCell> or clone.
                let label_for_idle = gamma_label.clone();
                let receiver = Rc::new(RefCell::new(receiver)); // mpsc::Receiver is not Sync, so RefCell is fine.

                let idle_id = glib::idle_add_local(move || {
                    // Try to receive any pending messages without blocking.
                    let mut recv = receiver.borrow_mut();

                    while let Ok(enabled) = recv.try_recv() {
                        println!("sdzfgd {enabled}");
                        label_for_idle.set_text(&format!(
                            "Gamma control is {}",
                            if enabled { "ACTIVE" } else { "inactive" }
                        ));
                    }
                    glib::ControlFlow::Continue // keep the idle callback alive
                });

                // 3. Start the gamma listener and send notifications via the sender.
                //    IMPORTANT: store the listener’s handle to keep it alive.
                let listener = GammaListener::new(
                    move |enabled| {
                        // sender.send() is fine; it only blocks if the channel is full.
                        let _ = sender.send(enabled);
                    },
                );

                // Keep the listener from being dropped. You can store it as window data.
                unsafe {
                    self.set_data("gamma-listener", listener);
                }
            }
            //////////

            // // Spin up the listener – every 5 seconds, the label updates.
            // // IMPORTANT: keep the `_listener` alive or it will stop!
            // let _listener = GammaListener::new(
            //     glib::clone!(
            //         // #[weak] gamma_label,
            //         move |enabled| {
            //             // gamma_label.set_text(&format!(
            //             //     "Gamma control is {}",
            //             //     if enabled { "ACTIVE" } else { "inactive" }
            //             // ));
            //         },
            //     ),
            //     5, // poll every 5 seconds
            // );

            // ── Layout ────────────────────────────────────────────────────
            //
            // gtk::Paned (Horizontal)
            //   ├── start: sidebar (Monitors heading + separator + ListBox)
            //   └── end:   content (presets, preview, action buttons)

            // — Sidebar —
            let sidebar_lbl = gtk::Label::new(Some("Monitors"));
            sidebar_lbl.set_halign(gtk::Align::Start);
            sidebar_lbl.set_margin_start(12);
            sidebar_lbl.set_margin_top(10);
            sidebar_lbl.set_margin_bottom(8);
            sidebar_lbl.add_css_class("heading");

            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .vexpand(true)
                .child(&mon_list)
                .build();

            let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
            sidebar.append(&sidebar_lbl);
            sidebar.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            sidebar.append(&scroll);

            // — Content panel —
            let action_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::Center)
                .build();
            action_row.append(&show_btn);
            action_row.append(&hide_btn);

            // let desc = monitors
            //     .get(get_selected_idx()).map(|m| m.description().map(|t| t));
            // let title = gtk::Label::new(if let Some(Some(ref desc)) = desc {
            //     Some(&desc)
            // } else {
            //     None
            // });

            let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
            content.set_margin_start(24);
            content.set_margin_end(24);
            content.set_margin_top(24);
            content.set_margin_bottom(24);
            content.set_hexpand(true);
            content.set_halign(gtk::Align::Center);
            content.append(&preview);
            content.append(&preset_row);
            content.append(&action_row);

            // TODO
            #[cfg(feature = "gamma")]
            content.append(&gamma_label);

            // — Paned —
            let paned = gtk::Paned::builder()
                .orientation(gtk::Orientation::Horizontal)
                .wide_handle(true)
                .position(240)
                .shrink_start_child(false)
                .shrink_end_child(false)
                .resize_start_child(false)   // sidebar keeps its width on resize
                .resize_end_child(true)       // content panel absorbs spare space
                .start_child(&sidebar)
                .end_child(&content)
                .build();

            self.set_child(Some(&paned));

            // ── Teardown ──────────────────────────────────────────────────
            self.connect_destroy(move |_| {
                overlay.show_on_monitor(None);
                overlay.hide_overlay(); // safe even if never shown
                overlay.destroy();
            });
        }
    }

    pub use MainWindow as Window;
}

/// Trace a rounded-rectangle path into `cr`.  Does not stroke or fill.
pub fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -FRAC_PI_2,       0.0         );
    cr.arc(x + w - r, y + h - r, r,  0.0,              FRAC_PI_2  );
    cr.arc(x + r,     y + h - r, r,  FRAC_PI_2,        PI         );
    cr.arc(x + r,     y + r,     r,  PI,       3.0 * FRAC_PI_2    );
    cr.close_path();
}

fn main() {
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(|app| {
        // Single-instance: if our window is already open, just raise it.
        if let Some(win) = app.windows().into_iter().find(|w| w.is::<main_window::Window>()) {
            win.present();
            return;
        }

        if !gtk_layer_shell::is_supported() {
            gtk::AlertDialog::builder()
                // .modal(true)
                .message("Compositor not supported")
                .detail(
                    "White Screen requires a Wayland compositor that supports \
                     the wlr-layer-shell protocol (e.g. Sway, Hyprland, \
                     Wayfire, KDE Plasma ≥ 6)."
                )
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        let overlay = screen_overlay::ScreenOverlay::new(app);
        let win     = main_window::MainWindow::new(app, overlay);
        win.present();
    });

    app.run();
}
