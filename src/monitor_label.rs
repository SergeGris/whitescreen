use std::sync::Once;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::css_class;

// ── MonitorLabel – click-through connector-name badge ────────────────────────

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
        const NAME: &str = "WhiteScreenMonitorLabel";
        type Type       = super::MonitorLabel;
        type ParentType = gtk::Window;
    }

    impl ObjectImpl for MonitorLabel {
        fn constructed(&self) {
            self.parent_constructed();
            let win = self.obj();

            win.init_layer_shell();
            win.set_namespace(Some("whitescreen-monlabel"));
            // Same layer as ScreenOverlay: on Layer::Top the badge would be
            // painted *underneath* a shown overlay and never be visible.
            // Within a layer the compositor stacks by map order, so the badges
            // are re-presented after the overlays (see MainWindow::show_selected).
            win.set_layer(Layer::Overlay);
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
                if !w.is_realized() { return; }
                if let Some(surface) = w.surface() {
                    // Empty input region: clicks fall through to the layers
                    // below, so a badge never intercepts a click meant for the
                    // overlay or the desktop underneath it.
                    //
                    // The opaque region is deliberately left to GTK: the badge
                    // background is translucent (alpha 0.90), so GTK already
                    // computes an empty one and overriding it gains nothing.
                    let empty = gtk::cairo::Region::create();
                    surface.set_input_region(Some(&empty));
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
        obj.set_info(title, subtitle);
        obj
    }

    /// Update the badge text in place.
    ///
    /// Needed because the text is not fixed for the life of the window: the
    /// fallback title is positional ("Monitor 2", so unplugging a screen ahead
    /// of this one renames it), and a compositor may only report the connector
    /// name and EDID strings after the badge has already been built.
    pub fn set_info(&self, title: &str, subtitle: Option<&str>) {
        let imp = self.imp();
        imp.title_lbl.set_text(title);
        match subtitle {
            Some(s) => {
                imp.subtitle_lbl.set_text(s);
                imp.subtitle_lbl.set_visible(true);
            }
            None => {
                imp.subtitle_lbl.set_text("");
                imp.subtitle_lbl.set_visible(false);
            }
        }
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
        self.set_monitor(None);
    }

    /// Point an existing badge at a monitor that has just come back, so a
    /// dock/undock cycle reuses this window instead of leaking a new one.
    ///
    /// The layer-shell monitor is only touched when it actually differs:
    /// gtk_layer_set_monitor() re-creates the surface of a mapped window, so
    /// calling it unconditionally would flicker every badge on any hot-plug.
    pub fn rebind(&self, monitor: &gdk::Monitor, title: &str, subtitle: Option<&str>) {
        if self.monitor().as_ref() != Some(monitor) {
            self.set_monitor(Some(monitor));
        }
        self.set_info(title, subtitle);
    }
}
