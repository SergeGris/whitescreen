use std::{cell::Cell, cell::RefCell, time::Duration};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::color_surface;

const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether this keystroke should take the overlays down.
///
/// Deliberately broader than just ESC. The overlay holds an exclusive
/// keyboard grab, so every key it does not act on is swallowed and lost
/// anyway -- and between silently eating a keystroke and reading it as "get
/// me out of here", the second is the safer answer for a full-screen colour
/// with no other keyboard function. What that must not cost is the ability to
/// leave deliberately, hence the two exceptions below.
fn dismisses_overlay(key: gdk::Key, state: gdk::ModifierType) -> bool {
    if is_modifier_key(key) {
        return false;
    }
    // A key held with Ctrl/Alt/Super is a shortcut, not an escape attempt.
    // Swallowing those would break the app's own Ctrl+Q -- which is exactly
    // what happened when this first dismissed on *any* key: quitting from an
    // overlay silently turned into dismissing the overlay.
    !state.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK
            | gdk::ModifierType::HYPER_MASK,
    )
}

/// Keys that only ever modify another key. Reaching for Ctrl or Super is not
/// an attempt to leave, and must not tear the overlay down before the rest of
/// the shortcut is even typed.
fn is_modifier_key(key: gdk::Key) -> bool {
    matches!(
        key,
        gdk::Key::Shift_L | gdk::Key::Shift_R | gdk::Key::Shift_Lock
            | gdk::Key::Control_L | gdk::Key::Control_R
            | gdk::Key::Alt_L | gdk::Key::Alt_R
            | gdk::Key::Meta_L | gdk::Key::Meta_R
            | gdk::Key::Super_L | gdk::Key::Super_R
            | gdk::Key::Hyper_L | gdk::Key::Hyper_R
            | gdk::Key::Caps_Lock | gdk::Key::Num_Lock | gdk::Key::Scroll_Lock
            | gdk::Key::ISO_Level3_Shift | gdk::Key::ISO_Level5_Shift
    )
}

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
        /// Monitor this window is currently showing on.
        ///
        /// Tracked here rather than read back from `LayerShell::monitor()`,
        /// because in fallback mode there is no layer-shell surface to ask.
        pub bound:            RefCell<Option<gdk::Monitor>>,
    }

    impl Default for ScreenOverlay {
        fn default() -> Self {
            Self {
                color:           RefCell::new(gdk::RGBA::WHITE),
                color_surface:   color_surface::ColorSurface::new(),
                toast_overlay:   adw::ToastOverlay::new(),
                notice_visible:  Cell::new(false),
                last_pointer_pos: RefCell::new(None),
                bound:           RefCell::new(None),
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
                .title(if crate::layer_shell_available() {
                    "Press any key to exit"
                } else {
                    // Fallback overlays are ordinary windows, so only the
                    // focused one hears the keyboard; clicking works on any
                    // of them.
                    "Press any key or click to exit"
                })
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

            // ── Layer shell, or a plain fullscreen window ──────────────
            //
            // Nothing below the anchoring differs between the two: the
            // colour, the ESC handling and the blank cursor are the same
            // window either way. What fallback mode gives up is stacking
            // above everything else -- a fullscreen window is only above the
            // windows it covers, so a notification or an on-screen keyboard
            // can still appear over the colour.
            if crate::layer_shell_available() {
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

            // ── Input: any key closes every overlay ────────────────────
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(glib::clone!(
                #[weak] win,
                #[upgrade_or] glib::Propagation::Proceed,
                move |_, key, _, state| {
                    if !dismisses_overlay(key, state) {
                        return glib::Propagation::Proceed;
                    }

                    // Hide *all* overlays, not just this one: with one overlay
                    // per monitor the others would keep grabbing the keyboard
                    // and the user would have to press a key once per screen.
                    match win.application() {
                        Some(app) => app.activate_action("hide-overlays", None),
                        None      => win.hide_overlay(),
                    }
                    glib::Propagation::Stop
                }
            ));
            win.add_controller(keys);

            // ── Input: clicks ──────────────────────────────────────────
            let click = gtk::GestureClick::new();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            if crate::layer_shell_available() {
                // Swallow them: the overlay covers the desktop, and a click
                // that fell through would land on whatever is underneath.
                click.connect_pressed(|_, _, _, _| {});
            } else {
                // In fallback mode each overlay is a separate toplevel and
                // only the focused one receives ESC, so a click has to be a
                // way out -- otherwise the overlays on the other screens can
                // only be dismissed by finding the main window again.
                click.connect_pressed(glib::clone!(
                    #[weak] win,
                    move |_, _, _, _| match win.application() {
                        Some(app) => app.activate_action("hide-overlays", None),
                        None      => win.hide_overlay(),
                    }
                ));
            }
            win.add_controller(click);

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

            // Only re-target when the monitor actually changes.
            // gtk_layer_set_monitor() re-creates the surface of a mapped
            // window, and sync_monitors() re-shows every already-visible
            // overlay after each hot-plug -- unconditionally setting it would
            // make every screen flicker whenever any monitor is plugged in.
            let moved = self.imp().bound.borrow().as_ref() != monitor;
            if moved {
                self.imp().bound.replace(monitor.cloned());
                if crate::layer_shell_available() {
                    self.set_monitor(monitor);
                } else if let Some(monitor) = monitor {
                    // Ask for the monitor before mapping: gtk_window_fullscreen_on_monitor()
                    // on an already-mapped window has to unmap and remap it on
                    // some compositors, which is the flicker the guard avoids.
                    self.fullscreen_on_monitor(monitor);
                }
            }
            self.present();
            if self.is_realized() {
                self.grab_focus();
            }
            self.imp().arm_notice();
        }

        /// Detach from the monitor this window was bound to.
        ///
        /// Called when that monitor is unplugged. The window outlives the
        /// GdkMonitor here (see MainWindow::graveyard), and gtk_layer_set_monitor()
        /// keeps hold of what it was given, so an unplug would otherwise leave the
        /// window pointing at a monitor that is on its way out -- read again by the
        /// next map or by MonitorLabel::rebind(). None is the layer-shell default:
        /// "let the compositor choose".
        pub fn unbind_monitor(&self) {
            self.imp().bound.replace(None);
            if crate::layer_shell_available() {
                self.set_monitor(None);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_do_not_dismiss() {
        for key in [
            gdk::Key::Shift_L, gdk::Key::Control_R, gdk::Key::Alt_L,
            gdk::Key::Super_L, gdk::Key::Caps_Lock, gdk::Key::ISO_Level3_Shift,
        ] {
            assert!(is_modifier_key(key), "{key:?} should be treated as a modifier");
        }
    }

    #[test]
    fn ordinary_keys_dismiss() {
        for key in [
            gdk::Key::Escape, gdk::Key::q, gdk::Key::space, gdk::Key::Return,
            gdk::Key::a, gdk::Key::F1, gdk::Key::Left,
        ] {
            assert!(!is_modifier_key(key), "{key:?} should dismiss the overlay");
            assert!(dismisses_overlay(key, gdk::ModifierType::empty()));
        }
    }

    #[test]
    fn shortcuts_pass_through() {
        // Ctrl+Q has to reach the application's quit accelerator.
        assert!(!dismisses_overlay(gdk::Key::q, gdk::ModifierType::CONTROL_MASK));
        assert!(!dismisses_overlay(gdk::Key::Tab, gdk::ModifierType::ALT_MASK));
        assert!(!dismisses_overlay(gdk::Key::l, gdk::ModifierType::SUPER_MASK));
    }

    #[test]
    fn shift_still_dismisses() {
        // Shift+A is an ordinary keystroke, not a shortcut.
        assert!(dismisses_overlay(gdk::Key::A, gdk::ModifierType::SHIFT_MASK));
    }
}
