// White Screen – fill any monitor with a solid colour.

use std::{cell::Cell, cell::RefCell, rc::Rc, sync::Once, time::Duration};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[cfg(feature = "gamma")]
mod gamma;
#[cfg(feature = "gamma")]
use gamma::GammaListener;

const APP_ID:        &str = "io.github.SergeGris.WhiteScreen";
const APP_VERSION:   &str = env!("CARGO_PKG_VERSION");
const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);

// Fixed preview dimensions – never changes with monitor selection.
const PREVIEW_W: i32 = 480;
const PREVIEW_H: i32 = 270; // 16:9

// ── Colour presets ────────────────────────────────────────────────────────────

struct Preset {
    name: &'static str,
    rgba: gdk::RGBA,
}

const PRESETS: &[Preset] = &[
    Preset { name: "White",   rgba: gdk::RGBA::WHITE },
    Preset { name: "Black",   rgba: gdk::RGBA::BLACK },
    Preset { name: "Red",     rgba: gdk::RGBA::RED   },
    Preset { name: "Green",   rgba: gdk::RGBA::GREEN },
    Preset { name: "Blue",    rgba: gdk::RGBA::BLUE  },
];

// ── CSS class name constants ──────────────────────────────────────────────────

mod css_class {
    pub const MONLABEL_WINDOW:   &str = "monlabel-window";
    pub const MONLABEL_TITLE:    &str = "monlabel-title";
    pub const MONLABEL_SUBTITLE: &str = "monlabel-subtitle";
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -FRAC_PI_2,       0.0         );
    cr.arc(x + w - r, y + h - r, r,  0.0,              FRAC_PI_2  );
    cr.arc(x + r,     y + h - r, r,  FRAC_PI_2,        PI         );
    cr.arc(x + r,     y + r,     r,  PI,       3.0 * FRAC_PI_2    );
    cr.close_path();
}

fn monitor_title(mon: &gdk::Monitor, index: usize) -> String {
    mon.connector()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Monitor {}", index + 1))
}

fn monitor_subtitle(mon: &gdk::Monitor) -> String {
    [mon.model(), mon.manufacturer()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(" — ")
}

// ── ColorSurface – GPU-accelerated solid colour widget ────────────────────────

mod color_surface {
    use super::*;

    mod imp {
        use super::*;

        pub struct ColorSurface {
            pub color: RefCell<gdk::RGBA>,
        }

        impl Default for ColorSurface {
            fn default() -> Self {
                Self {
                    color: RefCell::new(gdk::RGBA::BLACK),
                }
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ColorSurface {
            const NAME: &'static str = "WhiteScreenColorSurface";
            type Type       = super::ColorSurface;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for ColorSurface {}

        impl WidgetImpl for ColorSurface {
            fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
                // Sane minimum in both axes; the actual size is driven by the parent.
                (64, 64, -1, -1)
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let w = self.obj();
                snapshot.append_color(
                    &self.color.borrow(),
                    &gtk::graphene::Rect::new(0.0, 0.0, w.width() as f32, w.height() as f32),
                );
            }
        }
    }

    glib::wrapper! {
        pub struct ColorSurface(ObjectSubclass<imp::ColorSurface>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Default for ColorSurface {
        fn default() -> Self { Self::new() }
    }

    impl ColorSurface {
        pub fn new() -> Self { glib::Object::new() }

        pub fn set_rgba(&self, rgba: gdk::RGBA) {
            if *self.imp().color.borrow() != rgba {
                *self.imp().color.borrow_mut() = rgba;
                self.queue_draw();
            }
        }
    }
}

// ── ScreenOverlay – fullscreen layer-shell colour window ──────────────────────

mod screen_overlay {
    use super::*;

    mod imp {
        use super::*;
        use glib::Properties;

        #[derive(Properties)]
        #[properties(wrapper_type = super::ScreenOverlay)]
        pub struct ScreenOverlay {
            #[property(get, set = Self::set_color, explicit_notify)]
            pub color: RefCell<gdk::RGBA>,

            pub color_surface:    color_surface::ColorSurface,
            pub toast_overlay:    adw::ToastOverlay,
            pub notice_timeout:   RefCell<Option<glib::SourceId>>,
            pub notice_visible:   Cell<bool>,
            pub last_pointer_pos: RefCell<Option<(f64, f64)>>,
        }

        impl Default for ScreenOverlay {
            fn default() -> Self {
                Self {
                    color:           RefCell::new(gdk::RGBA::WHITE),
                    color_surface:   color_surface::ColorSurface::new(),
                    toast_overlay:   adw::ToastOverlay::new(),
                    notice_timeout:  RefCell::new(None),
                    notice_visible:  Cell::new(false),
                    last_pointer_pos: RefCell::new(None),
                }
            }
        }

        impl ScreenOverlay {
            fn set_color(&self, rgba: gdk::RGBA) {
                if *self.color.borrow() == rgba { return; }
                *self.color.borrow_mut() = rgba;
                self.color_surface.set_rgba(rgba);
                self.obj().notify_color();
            }

            /// Show the ESC toast.  Guards against re-adding while one is
            /// already live (adw::Toast has no reset-timeout API).
            pub fn arm_notice(&self) {
                if self.notice_visible.get() { return; }
                self.notice_visible.set(true);

                let toast = adw::Toast::builder()
                    .title("Press ESC to exit")
                    .timeout(NOTICE_TIMEOUT.as_secs() as u32)
                    .build();

                toast.connect_dismissed(glib::clone!(
                    #[weak(rename_to = imp)] self,
                    move |_| imp.notice_visible.set(false)
                ));

                self.toast_overlay.add_toast(toast);
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ScreenOverlay {
            const NAME: &'static str = "WhiteScreenOverlay";
            type Type       = super::ScreenOverlay;
            type ParentType = gtk::Window;
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

                // ── Layer shell ────────────────────────────────────────────
                win.init_layer_shell();
                win.set_namespace(Some("whitescreen-overlay"));
                win.set_layer(Layer::Overlay);
                // Exclusive: window owns the full keyboard seat while visible.
                // Without this, ESC is never delivered on most compositors.
                win.set_keyboard_mode(KeyboardMode::Exclusive);
                win.set_exclusive_zone(-1);
                for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                    win.set_anchor(edge, true);
                }
                win.set_decorated(false);

                // ── Widget tree ────────────────────────────────────────────
                self.toast_overlay.set_child(Some(&self.color_surface));
                win.set_child(Some(&self.toast_overlay));

                // Blank cursor on every map (show), not just the first realize.
                // connect_realize would accumulate handlers on repeated show calls.
                win.connect_map(|w| {
                    if let Some(surface) = w.surface() {
                        surface.set_cursor(gdk::Cursor::from_name("none", None).as_ref());
                    }
                });

                // ── Input: ESC closes ──────────────────────────────────────
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

                // ── Input: swallow all clicks ──────────────────────────────
                let swallow = gtk::GestureClick::new();
                swallow.set_propagation_phase(gtk::PropagationPhase::Capture);
                swallow.connect_pressed(|_, _, _, _| {});
                win.add_controller(swallow);

                // ── Input: show toast on mouse movement ────────────────────
                // Only re-arms after 16 px of travel to avoid noise from
                // micro-jitter while the user holds the mouse still.
                let motion = gtk::EventControllerMotion::new();
                motion.set_propagation_phase(gtk::PropagationPhase::Capture);
                motion.connect_motion(glib::clone!(
                    #[weak(rename_to = imp)] self,
                    move |_, x, y| {
                        let moved = imp.last_pointer_pos.borrow()
                            .map(|(lx, ly)| ((x-lx).powi(2) + (y-ly).powi(2)).sqrt())
                            .unwrap_or(0.0);
                        *imp.last_pointer_pos.borrow_mut() = Some((x, y));
                        if moved > 16.0 { imp.arm_notice(); }
                    }
                ));
                win.add_controller(motion);
            }
        }

        impl WidgetImpl for ScreenOverlay {}
        impl WindowImpl for ScreenOverlay {}
    }

    glib::wrapper! {
        pub struct ScreenOverlay(ObjectSubclass<imp::ScreenOverlay>)
            @extends gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap,
                gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl ScreenOverlay {
        pub fn new(app: &adw::Application) -> Self {
            glib::Object::builder().property("application", app).build()
        }

        pub fn show_on_monitor(&self, monitor: Option<&gdk::Monitor>) {
            // Reset motion baseline so the notice re-arms on first move.
            self.imp().last_pointer_pos.borrow_mut().take();
            self.set_monitor(monitor);
            self.present();
            self.grab_focus();
            self.imp().arm_notice();
        }

        /// Dismiss the overlay and restore the default cursor.
        ///
        /// Safe at any lifecycle stage — including before the window has ever
        /// been shown (no live GdkSurface) and during app shutdown.
        pub fn hide_overlay(&self) {
            if !self.is_visible() { return; }
            if let Some(surface) = self.surface() {
                surface.set_cursor(None);
            }
            self.set_visible(false);
        }
    }
}

// ── MonitorLabel – click-through connector-name badge ────────────────────────

mod monitor_label {
    use super::*;

    // CSS rules for the badge are identical across every instance, so register
    // a single provider on the display the first time a label is built.
    static MONLABEL_CSS_ONCE: Once = Once::new();

    mod imp {
        use super::*;

        pub struct MonitorLabel {
            pub title_lbl:    gtk::Label,
            pub subtitle_lbl: gtk::Label,
            pub css_provider: gtk::CssProvider,
        }

        impl Default for MonitorLabel {
            fn default() -> Self {
                Self {
                    title_lbl: gtk::Label::builder()
                        .xalign(0.0)
                        .css_classes([css_class::MONLABEL_TITLE])
                        .build(),
                    subtitle_lbl: gtk::Label::builder()
                        .xalign(0.0)
                        .visible(false)
                        .css_classes([css_class::MONLABEL_SUBTITLE])
                        .build(),
                    css_provider: gtk::CssProvider::new(),
                }
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MonitorLabel {
            const NAME: &'static str = "WhiteScreenMonitorLabel";
            type Type       = super::MonitorLabel;
            type ParentType = gtk::Window;
        }

        impl ObjectImpl for MonitorLabel {
            fn constructed(&self) {
                self.parent_constructed();
                let win = self.obj();

                win.init_layer_shell();
                win.set_namespace(Some("whitescreen-monlabel"));
                win.set_layer(Layer::Top);
                win.set_keyboard_mode(KeyboardMode::None);
                win.set_exclusive_zone(0);
                win.set_anchor(Edge::Bottom, true);
                win.set_anchor(Edge::Right,  true);
                win.set_margin(Edge::Bottom, 24);
                win.set_margin(Edge::Right,  24);
                win.set_decorated(false);
                win.add_css_class(css_class::MONLABEL_WINDOW);

                self.css_provider.load_from_string(&format!(
                    ".{w} {{
                        background: alpha(@window_bg_color, 0.90);
                        border-radius: 20px;
                        padding: 16px 20px;
                    }}
                    .{t} {{
                        color: @window_fg_color;
                        font-size: 56px;
                        font-weight: 400;
                    }}
                    .{s} {{
                        color: alpha(@window_fg_color, 0.65);
                        font-size: 15px;
                        font-weight: 500;
                    }}",
                    w = css_class::MONLABEL_WINDOW,
                    t = css_class::MONLABEL_TITLE,
                    s = css_class::MONLABEL_SUBTITLE,
                ));

                if let Some(display) = gdk::Display::default() {
                    let provider = self.css_provider.clone();
                    super::MONLABEL_CSS_ONCE.call_once(move || {
                        gtk::style_context_add_provider_for_display(
                            &display,
                            &provider,
                            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                        );
                    });
                }

                let vbox = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(8)
                    .build();
                vbox.append(&self.title_lbl);
                vbox.append(&self.subtitle_lbl);
                win.set_child(Some(&vbox));

                // Make the badge click-through (input events pass to layers below).
                win.connect_realize(|w| {
                    if let Some(surface) = w.surface() {
                        let empty = gtk::cairo::Region::create();
                        surface.set_input_region(Some(&empty));
                        // TODO surface.set_opaque_region(Some(&empty));
                    }
                });
            }
        }

        impl WidgetImpl for MonitorLabel {}
        impl WindowImpl for MonitorLabel {}
    }

    glib::wrapper! {
        pub struct MonitorLabel(ObjectSubclass<imp::MonitorLabel>)
            @extends gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap,
                gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl MonitorLabel {
        pub fn new(
            app:      &adw::Application,
            monitor:  &gdk::Monitor,
            title:    &str,
            subtitle: Option<&str>,
        ) -> Self {
            let obj: Self = glib::Object::builder().property("application", app).build();
            obj.set_monitor(Some(monitor));
            let imp = obj.imp();
            imp.title_lbl.set_text(title);
            if let Some(s) = subtitle {
                imp.subtitle_lbl.set_text(s);
                imp.subtitle_lbl.set_visible(true);
            }
            obj
        }
    }
}

// ── Floating HUD ─────────────────────────────────────────────────────────────

fn build_bottom_hud(
    #[cfg(feature = "gamma")] gamma_icon:  &gtk::Image,
    #[cfg(feature = "gamma")] gamma_label: &gtk::Label,
) -> gtk::Widget {
    let hud = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(20)
        .css_classes(["floating-hud"])
        .build();

    hud.append(
        &gtk::Image::builder()
            .icon_name("input-keyboard-symbolic")
            .pixel_size(16)
            .build(),
    );
    hud.append(
        &gtk::Label::builder()
            .label("ESC to exit fullscreen")
            .css_classes(["hud-label"])
            .build(),
    );

    // Gamma status: a sun / moon icon plus a short label, both updated live.
    #[cfg(feature = "gamma")]
    {
        hud.append(&gtk::Separator::builder().orientation(gtk::Orientation::Vertical).build());
        hud.append(gamma_icon);
        gamma_label.add_css_class("hud-label");
        hud.append(gamma_label);
    }

    gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(220)
        .reveal_child(true)
        .child(&hud)
        .build()
        .upcast()
}

// ── MainWindow ────────────────────────────────────────────────────────────────

mod main_window {
    use super::*;
    use gtk::gdk::RGBA;
    use screen_overlay::ScreenOverlay;

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct MainWindow {
            pub overlays: RefCell<Vec<ScreenOverlay>>,
            pub labels:   RefCell<Vec<monitor_label::MonitorLabel>>,
            #[cfg(feature = "gamma")]
            pub gamma_listener: RefCell<Option<GammaListener>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MainWindow {
            const NAME: &'static str = "WhiteScreenMainWindow";
            type Type       = super::MainWindow;
            type ParentType = adw::ApplicationWindow;
        }

        impl ObjectImpl            for MainWindow { fn constructed(&self) { self.parent_constructed(); } }
        impl WidgetImpl            for MainWindow {}
        impl WindowImpl            for MainWindow {}
        impl ApplicationWindowImpl for MainWindow {}
        impl AdwApplicationWindowImpl for MainWindow {}
    }

    glib::wrapper! {
        pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
            @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap,
                gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl MainWindow {
        pub fn new(
            app:     &adw::Application,
            overlays: Vec<ScreenOverlay>,
            labels:   Vec<monitor_label::MonitorLabel>,
        ) -> Self {
            let win: Self = glib::Object::builder()
                .property("application", app)
                .property("title", "White Screen")
                .property("default-width",  860i32)
                .property("default-height", 640i32)
                .build();
            win.imp().overlays.replace(overlays);
            win.imp().labels.replace(labels);
            win.build_ui();
            win
        }

        fn build_ui(&self) {
            // ── Single consolidated CSS provider ──────────────────────────
            let css = gtk::CssProvider::new();
            css.load_from_string(r#"
/* ── Preset chips ──────────────────────────────────────────────────── */

/* Hide the default GTK radio indicator — selection uses border + bg only */
.preset-chip check {
    min-width: 0; min-height: 0; -gtk-icon-size: 0px;
    opacity: 0; padding: 0; margin: 0;
}
.preset-chip {
    border-radius: 12px;
    padding: 4px;
    border: 2px solid transparent;
    background: transparent;
    transition: background-color 140ms ease, border-color 140ms ease, box-shadow 140ms ease, transform 110ms ease;
}
.preset-chip:hover    { background: alpha(@window_fg_color, 0.05); }
.preset-chip:checked  { border-color: @accent_bg_color; background: alpha(@accent_bg_color, 0.12); }
.preset-chip:active   { transform: scale(0.96); }
.color-label          { font-size: 12px; font-weight: 600; opacity: 0.80; }

/* ── Monitor rows ───────────────────────────────────────────────────── */

/* Hide the GTK checkbox indicator — selection is shown via bg + number badge */
.monitor-check check {
    min-width: 0; min-height: 0; -gtk-icon-size: 0px;
    opacity: 0; padding: 0; margin: 0;
}
.monitor-check {
    border-radius: 12px;
    padding: 2px;
    border: 2px solid transparent;
    transition: background-color 180ms ease, border-color 180ms ease;
}
.monitor-check:checked {
    background-color: alpha(@accent_bg_color, 0.10);
    border-color:     alpha(@accent_bg_color, 0.40);
}

/* Number badge: grey → accent when checked */
.monitor-number {
    min-width: 28px; min-height: 28px;
    border-radius: 999px;
    background: alpha(@window_fg_color, 0.10);
    color: @window_fg_color;
    font-weight: 700; font-size: 12px;
}
.monitor-check:checked .monitor-number {
    background: @accent_bg_color;
    color: @accent_fg_color;
}

/* ── Custom colour chip — a real radio CheckButton, indicator shown ──── */

/* Identical base to .preset-chip, but WITHOUT the rule that hides the
   `check` node, so the native radio dot stays visible. */
.custom-chip {
    border-radius: 12px;
    padding: 4px 8px;
    border: 2px solid transparent;
    background: transparent;
    transition: background-color 140ms ease, border-color 140ms ease,
                box-shadow 140ms ease, transform 110ms ease;
}
.custom-chip:hover   { background: alpha(@window_fg_color, 0.05); }
.custom-chip:checked { border-color: @accent_bg_color; background: alpha(@accent_bg_color, 0.12); }
.custom-chip:active  { transform: scale(0.96); }

/* ── Floating HUD ───────────────────────────────────────────────────── */

.floating-hud {
    background: alpha(@window_bg_color, 0.88);
    border-radius: 999px;
    padding: 10px 18px;
    border: 1px solid alpha(@window_fg_color, 0.08);
    box-shadow: 0 8px 24px rgba(0,0,0,0.20), 0 2px 6px rgba(0,0,0,0.10);
}
.hud-label { font-size: 13px; font-weight: 600; opacity: 0.80; }

/* ── Preview ────────────────────────────────────────────────────────── */

.preview-frame {
    border-radius: 12px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.30);
}
"#);
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display, &css,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }

            let overlays = self.imp().overlays.borrow().clone();
            let labels   = self.imp().labels.borrow().clone();

            // ── Header ────────────────────────────────────────────────────
            let header = adw::HeaderBar::new();
            header.set_title_widget(Some(
                &adw::WindowTitle::builder()
                    .title("White Screen")
                    .subtitle("Display colour overlay")
                    .build(),
            ));
            let menu_btn = gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Menu")
                .build();
            let menu = gio::Menu::new();
            menu.append(Some("About White Screen"), Some("win.about"));
            menu_btn.set_menu_model(Some(&menu));
            header.pack_end(&menu_btn);

            // ── About action ──────────────────────────────────────────────
            let about = gio::SimpleAction::new("about", None);
            about.connect_activate(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_, _| {
                    adw::AboutDialog::builder()
                        .application_name("White Screen")
                        .application_icon(super::APP_ID)
                        .developer_name("Serge Gris")
                        .version(super::APP_VERSION)
                        .license_type(gtk::License::Gpl30)
                        .website("https://github.com/SergeGris/whitescreen")
                        .issue_url("https://github.com/SergeGris/whitescreen/issues")
                        .comments("Fill any monitor with a solid colour.")
                        .build()
                        .present(Some(&win));
                }
            ));
            self.add_action(&about);

            // ── Monitor list ──────────────────────────────────────────────
            let display   = gdk::Display::default().expect("no GDK display");
            let mon_model = display.monitors();
            let monitors: Rc<Vec<gdk::Monitor>> = Rc::new(
                (0..mon_model.n_items())
                    .filter_map(|i| mon_model.item(i)?.downcast::<gdk::Monitor>().ok())
                    .collect(),
            );

            // Tracks which monitors are selected.
            // Initialise first entry true so Show Fullscreen starts enabled.
            let mut init_sel = vec![false; monitors.len()];
            if !monitors.is_empty() { init_sel[0] = true; }
            let selected_monitors: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(init_sel));

            let get_selected_indices = {
                let sel = selected_monitors.clone();
                move || -> Vec<usize> {
                    sel.borrow()
                        .iter()
                        .enumerate()
                        .filter_map(|(i, &s)| s.then_some(i))
                        .collect()
                }
            };

            let show_selected = {
                let ovs  = overlays.clone();
                let mons = monitors.clone();
                let get  = get_selected_indices.clone();
                move || {
                    for idx in get() {
                        if let (Some(ov), Some(mon)) = (ovs.get(idx), mons.get(idx)) {
                            ov.show_on_monitor(Some(mon));
                        }
                    }
                }
            };

            // ── Action buttons (built BEFORE the monitor loop so we can
            //    weakly reference show_btn inside connect_toggled) ──────────

            // Show Fullscreen
            let show_btn = {
                let icon = gtk::Image::from_icon_name("view-fullscreen-symbolic");
                icon.set_pixel_size(16);
                let lbl  = gtk::Label::new(Some("Show Fullscreen"));
                let inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                inner.append(&icon);
                inner.append(&lbl);
                gtk::Button::builder()
                    .child(&inner)
                    .css_classes(["suggested-action", "pill"])
                    // Disabled only when nothing is selected.
                    .sensitive(!monitors.is_empty())
                    .build()
            };
            show_btn.connect_clicked(glib::clone!(
                #[strong] show_selected,
                move |_| show_selected()
            ));

            // Hide
            let hide_btn = gtk::Button::builder()
                .label("Hide")
                .css_classes(["pill"])
                .tooltip_text("Hide overlay on all monitors (or press ESC on the overlay)")
                .build();
            hide_btn.connect_clicked(glib::clone!(
                #[strong] overlays,
                move |_| for ov in &overlays { ov.hide_overlay(); }
            ));

            // Identify – ToggleButton that shows/hides connector-name badges.
            // Labels start hidden; the button controls them entirely.
            let ident_btn = gtk::ToggleButton::builder()
                .label("Identify")
                .tooltip_text("Show monitor connector labels on each screen")
                .build();
            ident_btn.connect_toggled(glib::clone!(
                #[strong] labels,
                move |btn| {
                    if btn.is_active() {
                        for lbl in &labels { lbl.present(); }
                    } else {
                        for lbl in &labels { lbl.set_visible(false); }
                    }
                }
            ));

            let action_bar = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::Center)
                .build();
            action_bar.append(&show_btn);
            action_bar.append(&hide_btn);
            action_bar.append(&ident_btn);

            // ── Monitor check list ────────────────────────────────────────
            let mon_heading = gtk::Label::builder()
                .label("Monitors")
                .halign(gtk::Align::Start)
                .css_classes(["title-4"])
                .build();
            let mon_subheading = gtk::Label::builder()
                .label("Select one or more monitors")
                .halign(gtk::Align::Start)
                .css_classes(["dim-label"])
                .build();

            let mon_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .margin_top(12).margin_bottom(12)
                .margin_start(12).margin_end(12)
                .build();
            mon_box.append(&mon_heading);
            mon_box.append(&mon_subheading);

            for (i, mon) in monitors.iter().enumerate() {
                let geo   = mon.geometry();
                let title = monitor_title(mon, i);
                let sub   = format!(
                    "{} × {}  •  {}.{:03} Hz",
                    geo.width(), geo.height(),
                    mon.refresh_rate() / 1000,
                    mon.refresh_rate() % 1000,
                );

                let icon = gtk::Image::builder()
                    .icon_name("video-display-symbolic")
                    .pixel_size(48)
                    .valign(gtk::Align::Center)
                    .build();

                let title_lbl = gtk::Label::builder()
                    .label(&title)
                    .halign(gtk::Align::Start)
                    .css_classes(["heading"])
                    .build();
                let sub_lbl = gtk::Label::builder()
                    .label(&sub)
                    .halign(gtk::Align::Start)
                    .css_classes(["dim-label"])
                    .build();
                let text = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(2)
                    .hexpand(true)
                    .valign(gtk::Align::Center)
                    .build();
                text.append(&title_lbl);
                text.append(&sub_lbl);

                let num_lbl = gtk::Label::builder()
                    .label((i + 1).to_string())
                    .css_classes(["monitor-number"])
                    .halign(gtk::Align::Center)
                    .valign(gtk::Align::Center)
                    .build();

                let row = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(12)
                    .margin_top(10).margin_bottom(10)
                    .margin_start(10).margin_end(10)
                    .build();
                row.append(&num_lbl);
                row.append(&icon);
                row.append(&text);

                let check = gtk::CheckButton::builder()
                    .active(i == 0) // first monitor pre-selected
                    .child(&row)
                    .hexpand(true)
                    .css_classes(["monitor-check"])
                    .build();

                // Keep selected_monitors in sync AND update Show sensitivity.
                check.connect_toggled(glib::clone!(
                    #[strong]  selected_monitors,
                    #[weak]    show_btn,
                    move |btn| {
                        selected_monitors.borrow_mut()[i] = btn.is_active();
                        let any = selected_monitors.borrow().iter().any(|&s| s);
                        // Show Fullscreen is disabled when nothing is checked.
                        show_btn.set_sensitive(any);
                    }
                ));

                mon_box.append(&check);
            }

            let sidebar = gtk::ScrolledWindow::builder()
                .min_content_width(300)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&mon_box)
                .build();

            // ── Colour application ────────────────────────────────────────
            // Single source of truth for the selected colour.  Both the
            // preset chips and the custom picker call this.
            let apply_color = Rc::new({
                let ovs = overlays.clone();
                let preview_ref: Rc<RefCell<Option<color_surface::ColorSurface>>> =
                    Rc::new(RefCell::new(None));
                // We fill preview_ref after the preview widget is created below.
                (preview_ref.clone(), move |rgba: gdk::RGBA| {
                    if let Some(p) = preview_ref.borrow().as_ref() {
                        p.set_rgba(rgba);
                    }
                    for ov in &ovs { ov.set_color(rgba); }
                })
            });
            let (preview_cell, apply_color) = (*apply_color).clone();

            // ── Preview – fixed size, never changes ───────────────────────
            let preview = color_surface::ColorSurface::new();
            // Set the fixed size request once; no sync_preview_ratio needed.
            preview.set_size_request(PREVIEW_W, PREVIEW_H);
            preview.add_css_class("preview-surface");

            // Wire the preview cell so apply_color can reach it.
            *preview_cell.borrow_mut() = Some(preview.clone());

            // Show fullscreen when user clicks the preview.
            let click = gtk::GestureClick::new();
            click.connect_pressed(glib::clone!(
                #[strong] show_selected,
                move |_, _, _, _| show_selected()
            ));
            preview.add_controller(click);

            // Wrap in a frame for rounded corners + drop shadow.
            let preview_frame = gtk::Frame::builder()
                .child(&preview)
                .css_classes(["preview-frame"])
                .build();

            // ── Preset chips ──────────────────────────────────────────────
            let preset_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::Center)
                .build();

            let preset_buttons: Rc<RefCell<Vec<(gtk::CheckButton, gdk::RGBA)>>> =
                Rc::new(RefCell::new(Vec::new()));
            let mut group_root: Option<gtk::CheckButton> = None;

            for &Preset { name, rgba } in PRESETS {
                let swatch = gtk::DrawingArea::builder()
                    .width_request(52).height_request(28)
                    .build();
                swatch.set_draw_func(move |_, cr, w, h| {
                    rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 10.0);
                    cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                });

                let inner = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(6)
                    .margin_top(6).margin_bottom(6)
                    .margin_start(8).margin_end(8)
                    .build();
                inner.append(&swatch);
                inner.append(
                    &gtk::Label::builder()
                        .label(name)
                        .css_classes(["color-label"])
                        .halign(gtk::Align::Center)
                        .build(),
                );

                let btn = gtk::CheckButton::builder()
                    .child(&inner)
                    .css_classes(["preset-chip"])
                    .tooltip_text(name)
                    .build();

                if let Some(root) = &group_root {
                    btn.set_group(Some(root));
                } else {
                    group_root = Some(btn.clone());
                }

                btn.set_cursor_from_name(Some("pointer"));
                btn.connect_toggled(glib::clone!(
                    #[strong] apply_color,
                    move |b| if b.is_active() { apply_color(rgba); }
                ));

                preset_buttons.borrow_mut().push((btn.clone(), rgba));
                preset_row.append(&btn);
            }

            // ── Custom colour chip — a CheckButton in the preset radio group
            // Native radio indicator (the dot) + automatic mutual exclusion
            // with the presets. Selecting it applies the current custom colour
            // and opens the dialog to change it.
            let custom_rgba = Rc::new(Cell::new(gdk::RGBA::WHITE));

            let custom_swatch = gtk::DrawingArea::builder()
                .width_request(52).height_request(28)
                .build();
            custom_swatch.set_draw_func(glib::clone!(
                #[strong] custom_rgba,
                move |_, cr, w, h| {
                    let rgba = custom_rgba.get();
                    rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 10.0);
                    cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, 1.0);
                    let _ = cr.fill_preserve();
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                }
            ));

            let custom_inner = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .margin_top(6).margin_bottom(6)
                .margin_start(8).margin_end(8)
                .build();
            custom_inner.append(&custom_swatch);
            custom_inner.append(
                &gtk::Label::builder()
                    .label("Custom")
                    .css_classes(["color-label"])
                    .halign(gtk::Align::Center)
                    .build(),
            );

            let custom_btn = gtk::CheckButton::builder()
                .child(&custom_inner)
                .css_classes(["custom-chip"])
                .tooltip_text("Pick a custom colour")
                .build();
            custom_btn.set_cursor_from_name(Some("pointer"));

            // Join the preset radio group → selecting custom unchecks presets.
            if let Some(root) = &group_root {
                custom_btn.set_group(Some(root));
            }

            let custom_dialog = gtk::ColorDialog::builder()
                .title("Select custom colour")
                .with_alpha(false)
                .modal(true)
                .build();

            custom_btn.connect_toggled(glib::clone!(
                #[weak(rename_to = win)] self,
                #[strong] custom_dialog,
                #[strong] custom_rgba,
                #[weak]   custom_swatch,
                #[strong] apply_color,
                move |btn| {
                    if !btn.is_active() { return; } // act only when custom becomes selected
                    apply_color(custom_rgba.get()); // apply the current custom colour now…
                    // …then let the user change it.
                    let custom_rgba   = custom_rgba.clone();
                    let custom_swatch = custom_swatch.clone();
                    let apply_color   = apply_color.clone();
                    custom_dialog.choose_rgba(
                        Some(&win),
                        Some(&custom_rgba.get()),
                        gio::Cancellable::NONE,
                        move |res| {
                            if let Ok(rgba) = res {
                                custom_rgba.set(rgba);
                                custom_swatch.queue_draw();
                                apply_color(rgba);
                            }
                        },
                    );
                }
            ));

            // Activate the first preset at startup.
            if let Some((btn, rgba)) = preset_buttons.borrow().iter().find(|p| p.1 == RGBA::BLACK) {
                btn.set_active(true);
                apply_color(*rgba);
            }

            // ── Gamma status indicator (feature-gated) ────────────────────
            // Lives in the floating HUD as a sun / moon icon plus a short
            // label, flipped live by the gamma listener.
            #[cfg(feature = "gamma")]
            let gamma_icon = gtk::Image::builder()
                .icon_name("weather-clear-symbolic") // sun = normal rendering
                .pixel_size(16)
                // .tooltip_text("Gamma correction off")
               .build();
            #[cfg(feature = "gamma")]
            let gamma_label = gtk::Label::new(Some("Gamma correction disabled"));

            #[cfg(feature = "gamma")]
            {
                let (sender, receiver) = async_channel::unbounded::<bool>();
                let icon = gamma_icon.clone();
                let lbl  = gamma_label.clone();
                glib::spawn_future_local(async move {
                    while let Ok(active) = receiver.recv().await {
                        // sun  → no external gamma adjustment
                        // moon → another client holds gamma (night-light / filter)
                        icon.set_icon_name(Some(if active {
                            "weather-clear-night-symbolic"
                        } else {
                            "weather-clear-symbolic"
                        }));
                        // icon.set_tooltip_text(Some(if active {
                        //     "Gamma correction disabled is active (another app holds gamma)"
                        // } else {
                        //     "No colour filter"
                        // }));
                        lbl.set_text(if active { "Gamma correction enabled" } else { "Gamma correction disabled" });
                    }
                });
                self.imp().gamma_listener.replace(Some(GammaListener::new(move |e| {
                    let _ = sender.send_blocking(e);
                })));
            }

            // ── Content layout ────────────────────────────────────────────
            let controls_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(16)
                .build();
            let vbox = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(0)
                .halign(gtk::Align::Center)
                .build();
            vbox.append(&custom_btn);
            controls_box.append(&preset_row);
            controls_box.append(&vbox);
            controls_box.append(&action_bar);

            let main_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(20)
                .build();
            main_box.append(&preview_frame);
            main_box.append(&controls_box);

            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(0)
                .margin_top(24).margin_bottom(24)
                .margin_start(24).margin_end(24)
                .hexpand(true)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Start)
                .build();
            content.append(&main_box);

            // ── Navigation split view ─────────────────────────────────────
            let split_view = adw::NavigationSplitView::builder()
                .sidebar(
                    &adw::NavigationPage::builder()
                        .title("Monitors")
                        .child(&sidebar)
                        .build()
                )
                .content(
                    &adw::NavigationPage::builder()
                        .title("Preview")
                        .child(&content)
                        .build()
                )
                .build();

            let toolbar_view = adw::ToolbarView::new();
            toolbar_view.add_top_bar(&header);
            toolbar_view.set_content(Some(&split_view));

            // ── Root overlay: app + floating HUD hint ─────────────────────
            #[cfg(feature = "gamma")]
            let hud = build_bottom_hud(&gamma_icon, &gamma_label);

            #[cfg(not(feature = "gamma"))]
            let hud = build_bottom_hud();

            let root = gtk::Overlay::new();
            root.set_child(Some(&toolbar_view));
            root.add_overlay(&hud);
            root.set_measure_overlay(&hud, false);
            //root.set_clip_overlay(&hud, false);
            hud.set_can_target(false);
            // Allow click-through to the floating pill itself.
            if let Some(rev) = hud.downcast_ref::<gtk::Revealer>() {
                if let Some(child) = rev.child() { child.set_can_target(true); }
            }

            self.set_content(Some(&root));

            // ── Teardown ──────────────────────────────────────────────────
            // Was calling show_on_monitor(None) before hide — caused a flash
            // of white on all monitors on every app exit.
            self.connect_close_request(glib::clone!(
                #[strong] overlays,
                #[strong] labels,
                move |_| {
                    for ov  in &overlays { ov.show_on_monitor(None); ov.hide_overlay(); ov.destroy(); }
                    for lbl in &labels   { lbl.present(); lbl.destroy(); }//TODO
                    glib::Propagation::Proceed
                }
            ));

            // Labels start hidden; Identify button controls them.
            // (Do NOT call lbl.present() here.)
        }
    }

    pub use MainWindow as Window;
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();

    // One-time setup: keyboard accelerators are registered once per process.
    app.connect_startup(|app| {
        app.set_accels_for_action("win.about",    &["F1"]);
        app.set_accels_for_action("window.close", &["<Ctrl>Q"]);
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
                     Wayfire, KDE Plasma ≥ 6)."
                )
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        let display    = gdk::Display::default().expect("no GDK display");
        let mon_model  = display.monitors();
        let mons: Vec<gdk::Monitor> = (0..mon_model.n_items())
            .filter_map(|i| mon_model.item(i)?.downcast::<gdk::Monitor>().ok())
            .collect();

        if mons.is_empty() {
            gtk::AlertDialog::builder()
                .message("No monitors found")
                .detail("White Screen needs at least one active monitor.")
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        // One overlay window per physical monitor.
        let overlays: Vec<screen_overlay::ScreenOverlay> =
            mons.iter().map(|_| screen_overlay::ScreenOverlay::new(app)).collect();

        // One connector-name badge per physical monitor.
        // Badges start hidden; the Identify button reveals them.
        let labels: Vec<monitor_label::MonitorLabel> = mons
            .iter()
            .enumerate()
            .map(|(i, mon)| {
                let title = monitor_title(mon, i);
                let sub   = monitor_subtitle(mon);
                monitor_label::MonitorLabel::new(
                    app, mon, &title,
                    if sub.is_empty() { None } else { Some(sub.as_str()) },
                )
            })
            .collect();

        let win = main_window::Window::new(app, overlays, labels);
        win.present();
    });

    app.run()
}
