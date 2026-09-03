
use std::{cell::Cell, cell::RefCell, time::Duration};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::color_surface;

const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);

// ── ScreenOverlay – fullscreen layer-shell colour window ──────────────────────
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
        pub notice_visible:   Cell<bool>,
        pub last_pointer_pos: RefCell<Option<(f64, f64)>>,
    }

    impl Default for ScreenOverlay {
        fn default() -> Self {
            Self {
                color:           RefCell::new(gdk::RGBA::WHITE),
                color_surface:   color_surface::ColorSurface::new(),
                toast_overlay:   adw::ToastOverlay::new(),
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
                .timeout(super::NOTICE_TIMEOUT.as_secs() as u32)
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
                // gtk_native_get_surface() is only null-checked by the binding,
                // so an unrealized window can hand back a stale pointer and any
                // call on it trips "assertion 'GDK_IS_SURFACE (surface)' failed"
                // and then segfaults. Gate every surface access on is_realized().
                if !w.is_realized() { return; }
                if let Some(surface) = w.surface() {
                    surface.set_cursor(gdk::Cursor::from_name("none", None).as_ref());
                }
            });

            // ── Input: ESC closes every overlay ────────────────────────
            let esc = gtk::EventControllerKey::new();
            esc.set_propagation_phase(gtk::PropagationPhase::Capture);
            esc.connect_key_pressed(glib::clone!(
                #[weak] win,
                #[upgrade_or] glib::Propagation::Proceed,
                move |_, key, _, _| {
                    if key != gdk::Key::Escape {
                        // Do NOT swallow everything else. This window holds an
                        // exclusive keyboard seat, so any key stopped here is
                        // lost outright — including the compositor's own binds.
                        return glib::Propagation::Proceed;
                    }

                    // Hide *all* overlays, not just this one: with one overlay
                    // per monitor the others would keep grabbing the keyboard
                    // and the user would have to press ESC once per screen.
                    match win.application() {
                        Some(app) => app.activate_action("hide-overlays", None),
                        None      => win.hide_overlay(),
                    }
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
            if self.is_realized() {
                self.grab_focus();
            }
            self.imp().arm_notice();
        }

        /// Dismiss the overlay and restore the default cursor.
        ///
        /// Safe at any lifecycle stage — including before the window has ever
        /// been shown (no live GdkSurface) and during app shutdown.
        pub fn hide_overlay(&self) {
            if !self.is_visible() { return; }
            // Restoring the cursor is cosmetic; skip it rather than touch a
            // surface that may already be gone (teardown, or a layer-shell
            // remap triggered by set_monitor()).
            if self.is_realized() {
                if let Some(surface) = self.surface() {
                    surface.set_cursor(None);
                }
            }
            self.set_visible(false);
        }
    }
