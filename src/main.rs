// White Screen – fill any monitor with a solid color.
//
// Cleaned-up single-file version:
// - scoped CSS with application priority
// - no unsafe object data storage
// - corrected pointer-motion handling
// - no busy idle polling
// - safer shutdown
// - no fake overlay when no monitors exist

use std::{cell::RefCell, cell::Cell, rc::Rc, time::Duration};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

#[cfg(feature = "gamma")]
mod gamma;
#[cfg(feature = "gamma")]
use gamma::GammaListener;

const APP_ID: &str = "io.github.SergeGris.WhiteScreen";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);
const PREVIEW_WIDTH: i32 = 480;

#[derive(Clone)]
struct MonitorInfo {
    index: usize,
    title: String,
    subtitle: String,
    geometry: gdk::Rectangle,
    refresh_rate: i32,
}

impl MonitorInfo {
    fn from_monitor(mon: &gdk::Monitor, index: usize) -> Self {
        let geo = mon.geometry();

        Self {
            index,
            title: monitor_title(mon, index),
            subtitle: format!(
                "{} × {}  •  {}.{:03} Hz",
                geo.width(),
                geo.height(),
                mon.refresh_rate() / 1000,
                mon.refresh_rate() % 1000,
            ),
            geometry: geo,
            refresh_rate: mon.refresh_rate(),
        }
    }
}

mod monitor_object {
    use super::*;
    use glib::subclass::prelude::*;
    use std::cell::{Cell, RefCell};

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct MonitorObject {
            pub title: RefCell<String>,
            pub subtitle: RefCell<String>,
            pub selected: Cell<bool>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MonitorObject {
            const NAME: &'static str = "WhiteScreenMonitorObject";
            type Type = super::MonitorObject;
        }

        impl ObjectImpl for MonitorObject {}
    }

    glib::wrapper! {
        pub struct MonitorObject(ObjectSubclass<imp::MonitorObject>);
    }

    impl MonitorObject {
        pub fn new(title: &str, subtitle: &str) -> Self {
            let obj: Self = glib::Object::new();

            {
                let imp = obj.imp();
                imp.title.replace(title.into());
                imp.subtitle.replace(subtitle.into());
            }

            obj
        }
    }
}

struct Preset {
    name: &'static str,
    rgba: gdk::RGBA,
}

const PRESETS: &[Preset] = &[
    Preset { name: "White", rgba: gdk::RGBA::new(1.0, 1.0, 1.0, 1.0) },
    Preset { name: "Black", rgba: gdk::RGBA::new(0.0, 0.0, 0.0, 1.0) },
    Preset { name: "Red", rgba: gdk::RGBA::new(1.0, 0.0, 0.0, 1.0) },
    Preset { name: "Green", rgba: gdk::RGBA::new(0.0, 1.0, 0.0, 1.0) },
    Preset { name: "Blue", rgba: gdk::RGBA::new(0.0, 0.0, 1.0, 1.0) },
];

mod css_class {
    pub const BACKGROUND: &str = "whitescreen-background";
    pub const NOTICE: &str = "whitescreen-notice";
    pub const NOTICE_BOX: &str = "whitescreen-notice-box";
    pub const ID_BOX: &str = "whitescreen-id-box";
    pub const ID_LABEL: &str = "whitescreen-id-label";
    pub const MONLABEL_WINDOW: &str = "monlabel-window";
    pub const MONLABEL_TITLE: &str = "monlabel-title";
    pub const MONLABEL_SUBTITLE: &str = "monlabel-subtitle";
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

fn monitor_title(mon: &gdk::Monitor, index: usize) -> String {
    mon.connector()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Monitor {}", index + 1))
}

fn monitor_subtitle(mon: &gdk::Monitor) -> String {
    let mut parts = Vec::new();
    if let Some(model) = mon.model().filter(|s| !s.is_empty()) {
        parts.push(model.to_string());
    }
    if let Some(mfr) = mon.manufacturer().filter(|s| !s.is_empty()) {
        parts.push(mfr.to_string());
    }
    parts.join(" — ")
}

fn build_monitor_check_row(
    info: &MonitorInfo,
) -> (gtk::CheckButton, gtk::Widget) {

    let icon = gtk::Image::builder()
        .icon_name("video-display-symbolic")
        .pixel_size(64)
        .valign(gtk::Align::Center)
        .build();

    let title_lbl = gtk::Label::builder()
        .label(&info.title)
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build();

    let subtitle_lbl = gtk::Label::builder()
        .label(&info.subtitle)
        .halign(gtk::Align::Start)
        .css_classes(["dim-label"])
        .build();

    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();

    labels.append(&title_lbl);
    labels.append(&subtitle_lbl);

    let number = gtk::Label::builder()
        .label(&(info.index + 1).to_string())
        .css_classes(["monitor-number"])
        .build();

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    row.append(&number);
    row.append(&icon);
    row.append(&labels);

    let check = gtk::CheckButton::builder()
        .child(&row)
        .css_classes(["monitor-check"])
        .hexpand(true)
        .build();

    (check, row.upcast())
}

// ScreenOverlay – fullscreen layer-shell color window.
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
            pub color_surface: color_surface::ColorSurface,
            pub toast_overlay: adw::ToastOverlay,
            pub notice_timeout: RefCell<Option<glib::SourceId>>,
            pub last_pointer_pos: RefCell<Option<(f64, f64)>>,
            pub notice_visible: Cell<bool>,
        }

        impl Default for ScreenOverlay {
            fn default() -> Self {
                let notice_box = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(0)
                    .css_classes([css_class::NOTICE_BOX])
                    .build();

                let notice_label = gtk::Label::builder()
                    .label("Press ESC to exit")
                    .css_classes([css_class::NOTICE])
                    .build();

                notice_box.append(&notice_label);

                // let notice_revealer = gtk::Revealer::builder()
                //     .transition_type(gtk::RevealerTransitionType::Crossfade)
                //     .transition_duration(200)
                //     .reveal_child(false)
                //     .child(&notice_box)
                //     .build();

                let toast_overlay = adw::ToastOverlay::new();

                Self {
                    color: RefCell::new(gdk::RGBA::new(1.0, 1.0, 1.0, 1.0)),
                    color_surface: color_surface::ColorSurface::new(),
                    toast_overlay,
                    notice_timeout: RefCell::new(None),
                    last_pointer_pos: RefCell::new(None),
                    notice_visible: Cell::new(false),
                }
            }
        }

        impl ScreenOverlay {
            fn set_color(&self, rgba: gdk::RGBA) {
                if *self.color.borrow() == rgba {
                    return;
                }

                *self.color.borrow_mut() = rgba;
                self.color_surface.set_rgba(rgba);
                self.obj().notify_color();
            }

            fn color(&self) -> gdk::RGBA {
                *self.color.borrow()
            }

            // fn reapply_css(&self) {
            //     let c = self.color.borrow();
            //     let luma = c.red() * 0.299 + c.green() * 0.587 + c.blue() * 0.114;
            //     let (box_bg, text_col) = if luma > 0.5 {
            //         ("rgba(12,12,12,0.80)", "rgba(242,242,242,0.97)")
            //     } else {
            //         ("rgba(243,243,243,0.80)", "rgba(18,18,18,0.97)")
            //     };

            //     self.css_provider.load_from_data(&format!(
            //         ".{background} {{
            //             background-color: rgba({r},{g},{b},{a:.4});
            //         }}
            //         .{notice_box} {{
            //             background-color: {box_bg};
            //             border-radius: 12px;
            //             padding: 14px 22px;
            //         }}
            //         .{notice} {{
            //             color: {text_col};
            //             font-size: 17px;
            //             font-weight: 600;
            //         }}
            //         .{id_box} {{
            //             background-color: {box_bg};
            //             border-radius: 20px;
            //             padding: 32px 56px;
            //         }}
            //         .{id_label} {{
            //             color: {text_col};
            //             font-size: 64px;
            //             font-weight: 800;
            //         }}",
            //         r = (c.red() * 255.0) as u8,
            //         g = (c.green() * 255.0) as u8,
            //         b = (c.blue() * 255.0) as u8,
            //         a = c.alpha(),
            //         background = css_class::BACKGROUND,
            //         notice_box = css_class::NOTICE_BOX,
            //         notice = css_class::NOTICE,
            //         id_box = css_class::ID_BOX,
            //         id_label = css_class::ID_LABEL,
            //     ));
            // }

            pub fn arm_notice(&self) {
                if self.notice_visible.get() {
                    return;
                }

                self.notice_visible.set(true);

                let toast = adw::Toast::builder()
                    .title("Press ESC to exit")
                    .timeout(NOTICE_TIMEOUT.as_secs() as u32)
                    .build();

                toast.connect_dismissed(glib::clone!(
                    #[weak(rename_to = imp)] self,
                    move |_| imp.notice_visible.set(false),
                ));

                self.toast_overlay.add_toast(toast);
            }

            pub fn disarm_notice(&self) {
                // if let Some(id) = self.notice_timeout.borrow_mut().take() {
                //     id.remove();
                // }
                // self.notice_revealer.set_reveal_child(false);
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ScreenOverlay {
            const NAME: &str = "WhiteScreenOverlay";
            type Type = super::ScreenOverlay;
            type ParentType = gtk::Window;
        }

        impl ObjectImpl for ScreenOverlay {
            fn properties() -> &'static [glib::ParamSpec] {
                Self::derived_properties()
            }

            fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
                self.derived_set_property(id, value, pspec);
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
                win.set_keyboard_mode(KeyboardMode::OnDemand);
                win.set_exclusive_zone(-1);
                for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
                    win.set_anchor(edge, true);
                }
                win.set_decorated(false);

                self.toast_overlay.set_child(Some(&self.color_surface));

                win.set_child(Some(&self.toast_overlay));

                // let rev = &self.notice_revealer;
                // rev.set_halign(gtk::Align::Center);
                // rev.set_valign(gtk::Align::End);
                // rev.set_margin_bottom(32);
                // // toast_overlay.add_toast(rev);
                // // root_overlay.add_overlay(rev);

                // win.set_content(Some(&root_overlay));

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

                let swallow = gtk::GestureClick::new();
                swallow.set_propagation_phase(gtk::PropagationPhase::Capture);
                swallow.connect_pressed(|_, _, _, _| {});
                win.add_controller(swallow);

                let motion = gtk::EventControllerMotion::new();
                motion.set_propagation_phase(gtk::PropagationPhase::Capture);
                motion.connect_motion(glib::clone!(
                    #[weak(rename_to = imp)] self,
                    move |_, x, y| {
                        let moved = imp.last_pointer_pos
                            .borrow()
                            .map(|(lx, ly)| {
                                let dx = x - lx;
                                let dy = y - ly;
                                (dx * dx + dy * dy).sqrt()
                            })
                            .unwrap_or(0.0);

                        *imp.last_pointer_pos.borrow_mut() = Some((x, y));

                        if moved > 16.0 {
                            imp.arm_notice();
                        }
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
            let imp = self.imp();
            imp.disarm_notice();
            self.set_monitor(monitor);
            self.present();
            self.grab_focus();

            self.connect_realize(|win| {
                if let Some(surface) = win.surface() {
                    surface.set_cursor(
                        gdk::Cursor::from_name("none", None).as_ref()
                    );
                }
            });
            // if let Some(surface) = self.surface() {
            //     if let Some(cursor) = gdk::Cursor::from_name("none", None) {
            //         surface.set_cursor(Some(&cursor));
            //     }
            // }

            imp.arm_notice();
        }

        pub fn hide_overlay(&self) {
            if let Some(surface) = self.surface() {
                surface.set_cursor(None);
            }
            self.set_visible(false);
            self.imp().disarm_notice();
        }
    }
}

// MonitorLabel – always-visible, click-through connector-name badge.
mod monitor_label {
    use super::*;

    mod imp {
        use super::*;

        pub struct MonitorLabel {
            pub title_label: gtk::Label,
            pub subtitle_label: gtk::Label,
            pub css_provider: gtk::CssProvider,
        }

        impl Default for MonitorLabel {
            fn default() -> Self {
                Self {
                    title_label: gtk::Label::builder()
                        .xalign(0.0)
                        .css_classes([css_class::MONLABEL_TITLE])
                        .build(),
                    subtitle_label: gtk::Label::builder()
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
            type Type = super::MonitorLabel;
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
                win.set_anchor(Edge::Right, true);
                win.set_margin(Edge::Bottom, 24);
                win.set_margin(Edge::Right, 24);
                win.set_decorated(false);
                win.add_css_class(css_class::MONLABEL_WINDOW);

                self.css_provider.load_from_string(&format!(
                    ".{window} {{
                        background: alpha(@window_bg_color, 0.90);;
                        border-radius: 24px;
                        padding: 16px;
                    }}
                    .{title} {{
                        color: @window_fg_color;
                        font-size: 64px;
                        font-weight: 400;
                    }}
                    .{subtitle} {{
                        color: rgb(200,200,200);
                        font-size: 16px;
                        font-weight: 500;
                    }}",
                    window = css_class::MONLABEL_WINDOW,
                    title = css_class::MONLABEL_TITLE,
                    subtitle = css_class::MONLABEL_SUBTITLE,
                ));

                if let Some(display) = gdk::Display::default() {
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &self.css_provider,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                }

                let vbox = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(16)
                    .halign(gtk::Align::Start)
                    .valign(gtk::Align::Start)
                    .build();

                vbox.append(&self.title_label);
                vbox.append(&self.subtitle_label);
                win.set_child(Some(&vbox));

                win.connect_realize(|w| {
                    if let Some(surface) = w.surface() {
                        let empty = gtk::cairo::Region::create();
                        surface.set_input_region(Some(&empty));
                        surface.set_opaque_region(Some(&empty));
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
        pub fn new(app: &adw::Application, monitor: &gdk::Monitor, title: &str, subtitle: Option<&str>) -> Self {
            let obj: Self = glib::Object::builder()
                .property("application", app)
                .build();
            obj.set_monitor(Some(monitor));

            let imp = obj.imp();
            imp.title_label.set_text(title);
            if let Some(s) = subtitle {
                imp.subtitle_label.set_text(s);
                imp.subtitle_label.set_visible(true);
            }
            obj
        }
    }
}

fn build_bottom_hud(
    #[cfg(feature = "gamma")]
    gamma_label: &gtk::Label,
) -> gtk::Widget {

    let hud = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(18)
        .margin_bottom(20)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .css_classes(["floating-hud"])
        .build();

    //
    // ESC block
    //

    let esc_icon = gtk::Image::builder()
        .icon_name("input-keyboard-symbolic")
        .pixel_size(18)
        .build();

    let esc_label = gtk::Label::builder()
        .label("ESC to exit fullscreen")
        .css_classes(["hud-label"])
        .build();

    let esc_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();

    esc_box.append(&esc_icon);
    esc_box.append(&esc_label);

    hud.append(&esc_box);

    //
    // Divider
    //

    #[cfg(feature = "gamma")]
    {
        let separator = gtk::Separator::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        hud.append(&separator);

        let gamma_icon = gtk::Image::builder()
            .icon_name("display-brightness-symbolic")
            .pixel_size(18)
            .build();

        gamma_label.add_css_class("hud-label");

        let gamma_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();

        gamma_box.append(&gamma_icon);
        gamma_box.append(gamma_label);

        hud.append(&gamma_box);
    }

    //
    // Revealer
    //

    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(250)
        .reveal_child(true)
        .child(&hud)
        .build();

    revealer.upcast()
}

mod main_window {
    use super::*;
    use screen_overlay::ScreenOverlay;

    fn make_monitor_row(mon: &gdk::Monitor, index: usize) -> gtk::ListBoxRow {
        let geo = mon.geometry();
        let geo_w = geo.width();
        let geo_h = geo.height();
        let aspect = geo_w as f64 / geo_h.max(1) as f64;

        let thumb = gtk::DrawingArea::new();
        thumb.set_size_request((40.0 * aspect) as i32, 40);
        thumb.set_valign(gtk::Align::Center);
        thumb.set_draw_func(move |_, cr, cw, ch| {
            let (cw, ch) = (cw as f64, ch as f64);
            let (rw, rh) = if aspect >= cw / ch {
                (cw - 2.0, (cw - 2.0) / aspect)
            } else {
                ((ch - 2.0) * aspect, ch - 2.0)
            };
            let (rx, ry) = ((cw - rw) * 0.5, (ch - rh) * 0.5);

            cr.set_source_rgba(0.40, 0.40, 0.44, 0.28);
            rounded_rect(cr, rx, ry, rw, rh, 3.0);
            let _ = cr.fill();

            cr.set_source_rgba(0.50, 0.50, 0.55, 0.70);
            rounded_rect(cr, rx + 0.5, ry + 0.5, rw - 1.0, rh - 1.0, 3.0);
            cr.set_line_width(1.0);
            let _ = cr.stroke();

            // cr.set_source_rgba(0.55, 0.55, 0.60, 1.0);
            // cr.set_font_size(10.0);
            // let s = (index + 1).to_string();
            // if let Ok(ext) = cr.text_extents(&s) {
            //     cr.move_to(
            //         rx + (rw - ext.width()) * 0.5 - ext.x_bearing(),
            //         ry + (rh + ext.height()) * 0.5,
            //     );
            //     let _ = cr.show_text(&s);
            // }
        });

        let thumb_wrap = gtk::Overlay::new();
        thumb_wrap.set_child(Some(&thumb));
        thumb_wrap.add_overlay(&gtk::Label::new(Some(&format!("{}", index + 1))));

        let conn = monitor_title(mon, index);
        let vbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();

        let name_lbl = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name_lbl.set_markup(&format!("<b>{connector}</b>", connector = glib::markup_escape_text(&conn)));

        let res_lbl = gtk::Label::builder()
            .label(&format!("{geo_w}×{geo_h}"))
            .halign(gtk::Align::Start)
            .css_classes(["dim-label"])
            .build();

        let hz = mon.refresh_rate();
        let scale = mon.scale_factor();
        let detail = if scale > 1 {
            format!("{}.{:03} Hz  ×{}  HiDPI", hz / 1000, hz % 1000, scale)
        } else {
            format!("{}.{:03} Hz", hz / 1000, hz % 1000)
        };

        let hz_lbl = gtk::Label::builder()
            .label(&detail)
            .halign(gtk::Align::Start)
            .css_classes(["dim-label"])
            .build();

        vbox.append(&name_lbl);
        vbox.append(&res_lbl);
        vbox.append(&hz_lbl);

        let hbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        hbox.append(&thumb_wrap);
        hbox.append(&vbox);

        gtk::ListBoxRow::builder().child(&hbox).build()
    }

    mod imp {
        use super::*;

        #[derive(Default)]
        pub struct MainWindow {
            pub overlays: RefCell<Vec<ScreenOverlay>>,
            pub labels: RefCell<Vec<monitor_label::MonitorLabel>>,
            #[cfg(feature = "gamma")]
            pub gamma_listener: RefCell<Option<GammaListener>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MainWindow {
            const NAME: &str = "WhiteScreenMainWindow";
            type Type = super::MainWindow;
            type ParentType = adw::ApplicationWindow;
        }

        impl ObjectImpl for MainWindow {
            fn constructed(&self) {
                self.parent_constructed();
            }
        }

        impl WidgetImpl for MainWindow {}
        impl WindowImpl for MainWindow {}
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
        pub fn new(app: &adw::Application, overlays: Vec<ScreenOverlay>, labels: Vec<monitor_label::MonitorLabel>) -> Self {
            let win: Self = glib::Object::builder()
                .property("application", app)
                .property("title", "White Screen")
                .property("default-width", 800i32)
                .property("default-height", 600i32)
                .build();

            win.imp().overlays.replace(overlays);
            win.imp().labels.replace(labels);
            win.build_ui();
            win
        }

        fn build_ui(&self) {
// Global application CSS
let app_css = gtk::CssProvider::new();
app_css.load_from_string(r#"
    /* Monitor check buttons */
    .monitor-check {
        border-radius: 12px;
        padding: 8px;
        transition: background-color 200ms ease;
    }
    .monitor-check:checked {
        background-color: alpha(@accent_bg_color, 0.08);
    }
    .monitor-check:checked > box {
        /* subtle indicator on the content */
        border-left: 3px solid @accent_bg_color;
        padding-left: 9px;
        border-radius: 0 12px 12px 0;
    }
    .monitor-check:not(:checked) > box {
        border-left: 3px solid transparent;
        padding-left: 9px;
    }

    /* Color chip selector */
    .color-selector {
        border-radius: 12px;
        padding: 6px;
    }
    .color-selector:checked {
        border: 2px solid @accent_bg_color;
        background-color: alpha(@accent_bg_color, 0.1);
    }

    /* Preview card */
    .preview-card {
        overflow: hidden;
        border-radius: 16px;
    }
    .preview-wrap {
        border-radius: 16px;
    }

    /* Dim label for subtitle in monitor labels */
    .monlabel-subtitle {
        color: @view_fg_color;
        opacity: 0.65;
    }

    /* Number circle */
    .monitor-number {
        min-width: 24px;
        min-height: 24px;
        border-radius: 999px;
        background: @view_bg_color;
        color: @view_fg_color;
        font-weight: bold;
        font-size: 12px;
        padding: 2px;
    }

    .check-row:checked .monitor-number {
        background: @accent_bg_color;
        color: @accent_fg_color;
    }
"#);

if let Some(display) = gdk::Display::default() {
    gtk::style_context_add_provider_for_display(
        &display,
        &app_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

            let overlays = self.imp().overlays.borrow().clone();
            let labels = self.imp().labels.borrow().clone();

            let primary = match overlays.first() {
                Some(ov) => ov.clone(),
                None => return,
            };

            let header = adw::HeaderBar::new();

            let title = adw::WindowTitle::builder()
                .title("White Screen")
                .subtitle("Display color overlay")
                .build();

            header.set_title_widget(Some(&title));
            let menu_btn = gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Menu")
                .build();
            let menu = gio::Menu::new();
            menu.append(Some("About Whitescreen"), Some("win.about"));
            menu_btn.set_menu_model(Some(&menu));
            header.pack_end(&menu_btn);
            // TODO self.set_titlebar(Some(&header));

            let about = gio::SimpleAction::new("about", None);
            about.connect_activate(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_, _| {
                    let dialog = adw::AboutDialog::builder()
                        .application_name("Whitescreen")
                        .application_icon("video-display")
                        .developer_name("Serge Gris")
                        .version(super::APP_VERSION)
                        .license_type(gtk::License::Gpl30)
                        .comments("Fill any monitor with a solid color.")
                        .build();

                    dialog.present(Some(&win));
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

            for overlay in &overlays {
                color_btn.bind_property("rgba", overlay, "color").sync_create().build();
            }

            let display = gdk::Display::default().expect("no GDK display");
            let mon_model = display.monitors();
            let monitors: Rc<Vec<gdk::Monitor>> = Rc::new(
                (0..mon_model.n_items())
                    .filter_map(|i| mon_model.item(i)?.downcast::<gdk::Monitor>().ok())
                    .collect(),
            );

            // let mon_list = gtk::ListBox::builder()
            //     .selection_mode(gtk::SelectionMode::Multiple)
            //     //.css_classes(["navigation-sidebar"])
            //     .build();

            // for (i, mon) in monitors.iter().enumerate() {
            //     mon_list.append(&make_monitor_row(mon, i));
            // }

            // if let Some(first) = mon_list.row_at_index(0) {
            //     mon_list.select_row(Some(&first));
            // }

let monitor_box = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(12)
    .margin_top(18)
    .margin_bottom(18)
    .margin_start(16)
    .margin_end(16)
    .build();

// for (i, mon) in monitors.iter().enumerate() {

//     let geo = mon.geometry();

//     let title = monitor_title(mon, i);

//     let subtitle = format!(
//         "{} × {}  •  {}.{:03} Hz",
//         geo.width(),
//         geo.height(),
//         mon.refresh_rate() / 1000,
//         mon.refresh_rate() % 1000,
//     );

//     let icon = gtk::Image::builder()
//         .icon_name("video-display-symbolic")
//         //.pixel_size(256)
//         .build();

//     let title_lbl = gtk::Label::builder()
//         .label(&title)
//         .halign(gtk::Align::Start)
//         .css_classes(["heading"])
//         .build();

//     let subtitle_lbl = gtk::Label::builder()
//         .label(&subtitle)
//         .halign(gtk::Align::Start)
//         .css_classes(["dim-label"])
//         .build();

//     let labels = gtk::Box::builder()
//         .orientation(gtk::Orientation::Vertical)
//         .spacing(4)
//         .hexpand(true)
//         .valign(gtk::Align::Center)
//         .build();

//     labels.append(&title_lbl);
//     labels.append(&subtitle_lbl);

//     // let check = gtk::Image::builder()
//     //     .icon_name("object-select-symbolic")
//     //     .pixel_size(18)
//     //     .visible(i == 0)
//     //     .build();
//     let check = gtk::CheckButton::new();

//     let row_box = gtk::Box::builder()
//         .orientation(gtk::Orientation::Horizontal)
//         .spacing(16)
//         .margin_top(16)
//         .margin_bottom(16)
//         .margin_start(16)
//         .margin_end(16)
//         .build();

//     row_box.append(&icon);
//     row_box.append(&labels);
//     row_box.append(&check);

//     let row = gtk::ListBoxRow::builder()
//         .child(&row_box)
//         .css_classes(["monitor-row"])
//         .build();

//     mon_list.append(&row);
            // }

let monitor_infos: Vec<MonitorInfo> = monitors
    .iter()
    .enumerate()
    .map(|(i, m)| MonitorInfo::from_monitor(m, i))
    .collect();

for info in &monitor_infos {
    let (check, _) = build_monitor_check_row(info);

    monitor_box.append(&check);
}


            // let get_selected_indices = {
            //     glib::clone!(
            //         #[strong] mon_list,
            //         move || -> Vec<usize> {
            //             mon_list.selected_rows().iter().map(|r| r.index() as usize).collect()
            //         }
            //     )
            // };

let store = gio::ListStore::new::<monitor_object::MonitorObject>();

for info in &monitor_infos {
    store.append(
        &monitor_object::MonitorObject::new(
            &info.title,
            &info.subtitle,
        )
    );
}
            let selection = gtk::MultiSelection::new(Some(store.clone()));

let factory = gtk::SignalListItemFactory::new();

factory.connect_setup(|_, item| {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);

    let title = gtk::Label::new(None);
    title.set_xalign(0.0);

    row.append(&title);

    item.downcast_ref::<gtk::ListItem>()
        .unwrap()
        .set_child(Some(&row));
});

factory.connect_bind(|_, item| {
    let item = item.downcast_ref::<gtk::ListItem>().unwrap();

    let obj = item
        .item()
        .unwrap()
        .downcast::<monitor_object::MonitorObject>()
        .unwrap();

    let row = item.child().unwrap()
        .downcast::<gtk::Box>()
        .unwrap();

    let label = row.first_child().unwrap()
        .downcast::<gtk::Label>()
        .unwrap();

    label.set_text(&obj.imp().title.borrow());
});

let list = gtk::ListView::new(
    Some(selection.clone()),
    Some(factory.clone()),
);

            let selected_monitors: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(vec![false; monitors.len()]));

            let get_selected_indices = {
                glib::clone!(
                    #[strong] selected_monitors,
                    move || -> Vec<usize> {
                        selected_monitors
                            .borrow()
                            .iter()
                            .enumerate()
                            .filter_map(|(i, selected)| {
                                if *selected {
                                    Some(i)
                                } else {
                                    None
                                }
                            })
                            .collect()
                    }
                )
            };

            let show_selected = {
                let overlays = overlays.clone();
                let monitors = monitors.clone();
                glib::clone!(
                    #[strong] get_selected_indices,
                    move || {
                        for idx in get_selected_indices() {
                            if let (Some(ov), Some(mon)) = (overlays.get(idx), monitors.get(idx)) {
                                ov.show_on_monitor(Some(mon));
                            }
                        }
                    }
                )
            };

            let preview = color_surface::ColorSurface::new();

            // preview.set_hexpand(true);
            // preview.set_vexpand(true);

            preview.add_css_class("preview-surface");

            let preview_frame = gtk::AspectFrame::builder()
                .ratio(16.0 / 9.0)
                .obey_child(false)
                .xalign(0.5)
                .yalign(0.5)
                .child(&preview)
                .css_classes(["preview-frame"])
                .build();

            primary.connect_color_notify(glib::clone!(
                #[weak] preview,
                move |ov| preview.set_rgba(ov.color())
            ));

            let sync_preview_ratio: Rc<dyn Fn()> = Rc::new({
                glib::clone!(
                    #[weak] preview,
                    #[strong] monitors,
                    #[strong] get_selected_indices,
                    move || {
                        let idx = get_selected_indices().first().copied().unwrap_or(0);
                        if let Some(mon) = monitors.get(idx) {
                            let geo = mon.geometry();
                            let w = PREVIEW_WIDTH;
                            let h = (w as f64 * geo.height() as f64 / geo.width().max(1) as f64).round() as i32;
                            preview.set_size_request(w, h);
                        }
                    }
                )
            });

            // let sync_preview_ratio: Rc<dyn Fn()> = Rc::new({
            //     glib::clone!(
            //         #[strong] monitors,
            //         #[strong] get_selected_indices,
            //         #[weak] preview_frame,
            //         move || {
            //             let idx = get_selected_indices().first().copied().unwrap_or(0);

            //             if let Some(mon) = monitors.get(idx) {
            //                 let geo = mon.geometry();

            //                 let ratio =
            //                     geo.width().max(1) as f32 /
            //                     geo.height().max(1) as f32;

            //                 preview_frame.set_ratio(ratio);
            //             }
            //         }
            //     )
            // });

            // let sync_preview_ratio: Rc<dyn Fn()> = Rc::new({
            //     glib::clone!(
            //         #[weak] preview_frame,
            //         #[strong] monitors,
            //         #[strong] get_selected_indices,
            //         move || {
            //             let idx = get_selected_indices().first().copied().unwrap_or(0);

            //             if let Some(mon) = monitors.get(idx) {
            //                 let geo = mon.geometry();

            //                 let ratio =
            //                     geo.width().max(1) as f32 /
            //                     geo.height().max(1) as f32;

            //                 preview_frame.set_ratio(ratio);
            //             }
            //         }
            //     )
            // });

            // mon_list.connect_row_selected({
            //     glib::clone!(
            //         #[strong] sync_preview_ratio,
            //         #[strong] monitors,
            //         #[weak] preview_frame,
            //         #[strong] get_selected_indices,
            //         move |_, _| {
            //             sync_preview_ratio();
            //             let idx = get_selected_indices().first().copied().unwrap_or(0);
            //             if let Some(mon) = monitors.get(idx) {
            //                 let geo = mon.geometry();
            //                 let ratio = geo.width().max(1) as f32 / geo.height().max(1) as f32;
            //                 preview_frame.set_obey_child(false);
            //                 preview_frame.set_ratio(ratio);
            //             }
            //         }
            //     )
            // });
            sync_preview_ratio();

            // let preset_row = gtk::Box::builder()
            //     .halign(gtk::Align::Center)
            //     .spacing(4)
            //     .build();

            // for &Preset { name, rgba } in PRESETS {
            //     let swatch = gtk::DrawingArea::new();
            //     swatch.set_size_request(48, 24);
            //     swatch.set_draw_func(move |_, cr, w, h| {
            //         cr.set_source_rgb(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64);
            //         rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
            //         let _ = cr.fill();
            //         cr.set_source_rgba(0.0, 0.0, 0.0, 0.20);
            //         rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 4.0);
            //         cr.set_line_width(0.5);
            //         let _ = cr.stroke();
            //     });

            //     let lbl = gtk::Label::builder()
            //         .label(name)
            //         .halign(gtk::Align::Center)
            //         .build();

            //     let btn = gtk::ToggleButton::builder()
            //         .child(&swatch)
            //         .tooltip_text(name)
            //        // .css_classes(["color-chip"])
            //         .build();
            //     btn.connect_clicked(glib::clone!(
            //         #[weak] color_btn,
            //         move |_| color_btn.set_rgba(&rgba)
            //     ));

            //     let cell = gtk::Box::builder()
            //         .orientation(gtk::Orientation::Vertical)
            //         .spacing(4)
            //         .margin_top(6)
            //         .margin_bottom(6)
            //         .margin_start(6)
            //         .margin_end(6)
            //         .build();
            //     cell.append(&btn);
            //     cell.append(&lbl);
            //     preset_row.append(&cell);
            // }

/*
let current_color = Rc::new(RefCell::new(gdk::RGBA::WHITE));

let set_color = Rc::new(glib::clone!(
    #[strong] overlays,
    #[weak] preview,
    #[strong] current_color,
    move |rgba: gdk::RGBA| {

        *current_color.borrow_mut() = rgba;

        preview.set_rgba(rgba);

        for overlay in &overlays {
            overlay.set_color(rgba);
        }
    }
));

let preset_row = gtk::Box::builder()
    .orientation(gtk::Orientation::Horizontal)
    .spacing(14)
    .halign(gtk::Align::Center)
    .css_classes(["color-selector-row"])
    .margin_top(6)
    .margin_bottom(6)
    .build();

let chip_group: Rc<RefCell<Vec<gtk::ToggleButton>>> =
    Rc::new(RefCell::new(Vec::new()));

            for &Preset { name, rgba } in PRESETS {
                let swatch = gtk::DrawingArea::builder()
                    .width_request(48)
                    .height_request(24)
                //.css_classes(["color-preview"])
                    .build();
                swatch.set_draw_func(move |_, cr, w, h| {

                    rounded_rect(
                        cr,
                        0.5,
                        0.5,
                        w as f64 - 1.0,
                        h as f64 - 1.0,
                        8.0,
                    );

                    cr.set_source_rgba(
                        rgba.red() as f64,
                        rgba.green() as f64,
                        rgba.blue() as f64,
                        1.0,
                    );

                    let _ = cr.fill_preserve();

                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
                    cr.set_line_width(1.0);
                    let _ = cr.stroke();
                });

                let label = gtk::Label::builder()
                    .label(name)
                    .css_classes(["color-label"])
                    .halign(gtk::Align::Center)
                    .build();

                let inner = gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(8)
                    .valign(gtk::Align::Center)
                    .halign(gtk::Align::Center)
                    .build();

                inner.append(&swatch);
                inner.append(&label);

                let btn = gtk::ToggleButton::builder()
                    .child(&inner)
                    .tooltip_text(name)
                    .css_classes(["color-selector"])
                    .focusable(true)
                    .build();

                btn.set_cursor_from_name(Some("pointer"));

                chip_group.borrow_mut().push(btn.clone());

                btn.connect_clicked(glib::clone!(
                    #[weak] color_btn,
                    #[strong] chip_group,
                    move |clicked| {

                        for btn in chip_group.borrow().iter() {
                            btn.set_active(false);
                        }

                        clicked.set_active(true);
                        color_btn.set_rgba(&rgba);
                    }
                ));

                preset_row.append(&btn);
            }
if let Some(first) = chip_group.borrow().first() {
    first.set_active(true);
}
            // let custom_wrap = gtk::Box::builder()
            //     .orientation(gtk::Orientation::Vertical)
            //     .spacing(4)
            //     .margin_top(6)
            //     .margin_bottom(6)
            //     .margin_start(8)
            //     .margin_end(8)
            //     .build();

            // custom_wrap.append(&color_btn);
            // custom_wrap.append(&gtk::Label::builder().label("Custom").halign(gtk::Align::Center).build());
            // preset_row.append(&custom_wrap);

let custom_dialog = gtk::ColorDialog::builder()
    .title("Select custom color")
    .with_alpha(false)
    .modal(true)
    .build();

let custom_btn = gtk::ColorDialogButton::builder()
    .dialog(&custom_dialog)
    .rgba(&gdk::RGBA::WHITE)
    .css_classes([
        "custom-color-button",
        "color-selector",
    ])
    .build();

custom_btn.set_cursor_from_name(Some("pointer"));

fn rgba_equal(a: &gdk::RGBA, b: &gdk::RGBA) -> bool {
    const EPS: f32 = 0.001;

    (a.red() - b.red()).abs() < EPS
        && (a.green() - b.green()).abs() < EPS
        && (a.blue() - b.blue()).abs() < EPS
}

custom_btn.connect_rgba_notify(glib::clone!(
    #[strong] chip_group,
    move |btn| {

        let rgba = btn.rgba();

        let mut matched = false;

        for (idx, preset) in PRESETS.iter().enumerate() {

            let is_match = rgba_equal(&rgba, &preset.rgba);

            if let Some(chip) = chip_group.borrow().get(idx) {
                chip.set_active(is_match);
            }

            if is_match {
                matched = true;
            }
        }

        if !matched {
            for chip in chip_group.borrow().iter() {
                chip.set_active(false);
            }
        }
    }
));

let custom_label = gtk::Label::builder()
    .label("Custom")
    .css_classes(["color-label"])
    .halign(gtk::Align::Center)
    .build();

let custom_box = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(8)
    .halign(gtk::Align::Center)
    .build();

custom_box.append(&custom_btn);
custom_box.append(&custom_label);

preset_row.append(&custom_box);
             */

let current_color = Rc::new(RefCell::new(gdk::RGBA::WHITE));

let set_color = Rc::new(glib::clone!(
    #[strong] overlays,
    #[weak] preview,
    #[strong] current_color,
    move |rgba: gdk::RGBA| {

        *current_color.borrow_mut() = rgba;

        preview.set_rgba(rgba);

        for overlay in &overlays {
            overlay.set_color(rgba);
        }
    }
));

let preset_row = gtk::Box::builder()
    .orientation(gtk::Orientation::Horizontal)
    .spacing(14)
    .halign(gtk::Align::Center)
    .css_classes(["color-selector-row"])
    .margin_top(6)
    .margin_bottom(6)
    .build();

let chip_group: Rc<RefCell<Vec<gtk::ToggleButton>>> =
    Rc::new(RefCell::new(Vec::new()));

//
// Preset buttons
//

for &Preset { name, rgba } in PRESETS {

    let swatch = gtk::DrawingArea::builder()
        .width_request(52)
        .height_request(28)
        .build();

    swatch.set_draw_func(move |_, cr, w, h| {

        rounded_rect(
            cr,
            0.5,
            0.5,
            w as f64 - 1.0,
            h as f64 - 1.0,
            10.0,
        );

        cr.set_source_rgba(
            rgba.red() as f64,
            rgba.green() as f64,
            rgba.blue() as f64,
            1.0,
        );

        let _ = cr.fill_preserve();

        cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
        cr.set_line_width(1.0);

        let _ = cr.stroke();
    });

    let label = gtk::Label::builder()
        .label(name)
        .css_classes(["color-label"])
        .halign(gtk::Align::Center)
        .build();

    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_top(4)
        .margin_bottom(4)
        .margin_start(6)
        .margin_end(6)
        .build();

    inner.append(&swatch);
    inner.append(&label);

    let btn = gtk::ToggleButton::builder()
        .child(&inner)
        .tooltip_text(name)
        .focusable(true)
        .css_classes(["color-selector"])
        .build();

    btn.set_cursor_from_name(Some("pointer"));

    chip_group.borrow_mut().push(btn.clone());

btn.connect_toggled(glib::clone!(
    #[weak] color_btn,
    #[strong] chip_group,
    #[strong] set_color,
    move |clicked| {

        //
        // Ignore deactivation events
        //

        if !clicked.is_active() {
            return;
        }

        //
        // Enforce single selection
        //

        for btn in chip_group.borrow().iter() {

            if btn != clicked {
                btn.set_active(false);
            }
        }

        //
        // Apply color
        //

        color_btn.set_rgba(&rgba);

        set_color(rgba);
    }
));

    preset_row.append(&btn);
}

//
// Activate first preset by default
//

if let Some(first) = chip_group.borrow().first() {
    first.set_active(true);
}

set_color(gdk::RGBA::WHITE);

//
// Custom color
//

let custom_dialog = gtk::ColorDialog::builder()
    .title("Select custom color")
    .with_alpha(false)
    .modal(true)
    .build();

let custom_btn = gtk::ColorDialogButton::builder()
    .dialog(&custom_dialog)
    .rgba(&gdk::RGBA::WHITE)
    .css_classes([
        "custom-color-button",
    ])
    .build();

custom_btn.set_cursor_from_name(Some("pointer"));

fn rgba_equal(a: &gdk::RGBA, b: &gdk::RGBA) -> bool {
    const EPS: f32 = 0.001;

    (a.red() - b.red()).abs() < EPS
        && (a.green() - b.green()).abs() < EPS
        && (a.blue() - b.blue()).abs() < EPS
}

custom_btn.connect_rgba_notify(glib::clone!(
    #[strong] chip_group,
    #[strong] set_color,
    move |btn| {

        let rgba = btn.rgba();

        //
        // Apply custom color immediately
        //

        set_color(rgba);

        //
        // Sync preset toggle state
        //

        let mut matched = false;

        for (idx, preset) in PRESETS.iter().enumerate() {

            let is_match = rgba_equal(&rgba, &preset.rgba);

            if let Some(chip) = chip_group.borrow().get(idx) {
                chip.set_active(is_match);
            }

            if is_match {
                matched = true;
            }
        }

        //
        // If custom color doesn't match presets,
        // clear preset selection
        //

        if !matched {
            for chip in chip_group.borrow().iter() {
                chip.set_active(false);
            }
        }
    }
));

//
// Custom button presentation
//

let custom_label = gtk::Label::builder()
    .label("Custom")
    .css_classes(["color-label"])
    .halign(gtk::Align::Center)
    .build();

let custom_icon = gtk::Image::builder()
    .icon_name("color-select-symbolic")
    .pixel_size(22)
    .build();

let custom_inner = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(8)
    .halign(gtk::Align::Center)
    .margin_top(8)
    .margin_bottom(8)
    .margin_start(10)
    .margin_end(10)
    .build();

custom_inner.append(&custom_icon);
custom_inner.append(&custom_label);

let custom_wrap = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(8)
    .halign(gtk::Align::Center)
    .build();

custom_wrap.append(&custom_btn);
custom_wrap.append(&custom_inner);

preset_row.append(&custom_wrap);

            let css = gtk::CssProvider::new();
            css.load_from_string(
r".color-selector {
    border-radius: 16px;
    padding: 6px;
    transition:
        background-color 180ms ease,
        border-color 180ms ease,
        transform 120ms ease;
}

.color-selector:hover {
    background: alpha(@window_fg_color, 0.04);
}

.color-selector:checked {
    background: alpha(@accent_bg_color, 0.12);
    border: 2px solid @accent_bg_color;
}

.color-selector:active {
    transform: scale(0.97);
}

.color-label {
    font-size: 13px;
    font-weight: 600;
}

.custom-color-button {
    min-width: 64px;
    min-height: 64px;

    border-radius: 999px;

    padding: 0;

    background:
        linear-gradient(
            135deg,
            #ff5f6d,
            #ffc371,
            #47cacc,
            #845ec2
        );

    border: 2px solid alpha(@window_fg_color, 0.10);
}

.custom-color-button:hover {
    border-color: alpha(@accent_bg_color, 0.60);
}"
            );
if let Some(display) = gdk::Display::default() {
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

            // let show_btn = gtk::Button::builder()
            //     .label("Show Fullscreen")
            //     .css_classes(["suggested-action", "pill"])
            //     .build();
let icon = gtk::Image::from_icon_name("view-fullscreen-symbolic");
icon.set_pixel_size(16);

let lbl = gtk::Label::new(Some("Show Fullscreen"));

let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
content.append(&icon);
content.append(&lbl);

let show_btn = gtk::Button::builder()
    .child(&content)
    .css_classes(["suggested-action", "pill"])
    .build();
            show_btn.connect_clicked(glib::clone!(
                #[strong] show_selected,
                move |_| show_selected()
            ));

            let hide_btn = gtk::Button::builder()
                .label("Hide")
                .tooltip_text("Hide fullscreen overlay (or press ESC on the overlay)")
                .build();
            hide_btn.connect_clicked(glib::clone!(
                #[strong] overlays,
                move |_| {
                    for ov in &overlays {
                        ov.hide_overlay();
                    }
                }
            ));

            #[cfg(feature = "gamma")]
            let gamma_label = gtk::Label::new(None);

            #[cfg(feature = "gamma")]
            {
                let (sender, receiver) = std::sync::mpsc::channel::<bool>();
                let receiver = Rc::new(RefCell::new(receiver));
                let label_for_timer = gamma_label.clone();

                glib::timeout_add_local(Duration::from_millis(200), move || {
                    let mut recv = receiver.borrow_mut();
                    while let Ok(enabled) = recv.try_recv() {
                        label_for_timer.set_text(&format!(
                            "Gamma control is {}",
                            if enabled { "ACTIVE" } else { "inactive" }
                        ));
                    }
                    glib::ControlFlow::Continue
                });

                let listener = GammaListener::new(move |enabled| {
                    let _ = sender.send(enabled);
                });

                self.imp().gamma_listener.replace(Some(listener));
            }

if !monitors.is_empty() {
    selected_monitors.borrow_mut()[0] = true;
}
// Monitor list – native CheckButtons with blue accent
let monitor_box = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(6)
    .margin_top(12)
    .margin_bottom(12)
    .margin_start(12)
    .margin_end(12)
    .build();

let monitors_heading = gtk::Label::builder()
    .label("Monitors")
    .halign(gtk::Align::Start)
    .css_classes(["title-4"])
    .build();
let monitors_subheading = gtk::Label::builder()
    .label("Select one or more monitors")
    .halign(gtk::Align::Start)
    .css_classes(["dim-label"])
    .build();

monitor_box.append(&monitors_heading);
monitor_box.append(&monitors_subheading);

for (i, mon) in monitors.iter().enumerate() {
    let geo = mon.geometry();
    let title = monitor_title(mon, i);
    let subtitle = format!(
        "{} × {}  •  {}.{:03} Hz",
        geo.width(),
        geo.height(),
        mon.refresh_rate() / 1000,
        mon.refresh_rate() % 1000
    );

    let icon = gtk::Image::builder()
        .icon_name("video-display-symbolic")
        .pixel_size(64)
        .valign(gtk::Align::Center)
        .build();

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let title_lbl = gtk::Label::builder()
        .label(&title)
        .halign(gtk::Align::Start)
        .css_classes(["heading"])
        .build();
    let subtitle_lbl = gtk::Label::builder()
        .label(&subtitle)
        .halign(gtk::Align::Start)
        .css_classes(["dim-label"])
        .build();
    text.append(&title_lbl);
    text.append(&subtitle_lbl);

    let number_label = gtk::Label::builder()
        .label(&(i + 1).to_string())
        .css_classes(["monitor-number"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();

    let row_content = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    row_content.append(&number_label);   // ← add this first
    row_content.append(&icon);
    row_content.append(&text);

    let check = gtk::CheckButton::builder()
        .active(i == 0)
        .child(&row_content)
        .hexpand(true)
        .css_classes(["monitor-check", "check-row"])
        .build();

    // Keep selected_monitors in sync
    check.connect_toggled(glib::clone!(
        #[strong] selected_monitors,
        move |btn| {
            selected_monitors.borrow_mut()[i] = btn.is_active();
        }
    ));

    monitor_box.append(&check);
}
let sidebar = gtk::ScrolledWindow::builder()
    .min_content_width(320)
    .hscrollbar_policy(gtk::PolicyType::Never)
    .child(&monitor_box)
    .build();

            // let monitor_group = adw::PreferencesGroup::builder()
//     .title("Monitors")
//     .build();

// for (i, mon) in monitors.iter().enumerate() {
//     let geo = mon.geometry();

//     let title = monitor_title(mon, i);

//     let subtitle = format!(
//         "{}×{}  @  {}.{:03} Hz",
//         geo.width(),
//         geo.height(),
//         mon.refresh_rate() / 1000,
//         mon.refresh_rate() % 1000,
//     );

//     let row = adw::ActionRow::builder()
//         .title(&title)
//         .subtitle(&subtitle)
//         .activatable(true)
//         .build();

//     let icon = gtk::Image::from_icon_name("video-display-symbolic");
//     row.add_prefix(&icon);

//     monitor_group.add(&row);
// }

// let sidebar_page = adw::PreferencesPage::new();
// sidebar_page.add(&monitor_group);

// let sidebar = gtk::ScrolledWindow::builder()
//     .hscrollbar_policy(gtk::PolicyType::Never)
//     .min_content_width(260)
//     .child(&sidebar_page)
//                 .build();

            let action_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::Center)
                .build();
            action_row.append(&show_btn);
            action_row.append(&hide_btn);

            let overlay_btn = gtk::Button::builder()
                .label("Show Fullscreen")
                .icon_name("view-fullscreen-symbolic")
                .css_classes(["suggested-action"])
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .margin_end(16)
                .margin_bottom(16)
    .build();
            overlay_btn.connect_clicked(glib::clone!(
                #[strong] show_selected,
                move |_| show_selected()
            ));

            let preview_wrap = gtk::Overlay::new();
            preview_wrap.set_child(Some(&preview_frame));
            preview_wrap.add_overlay(&overlay_btn);
            preview_wrap.add_css_class("preview-wrap");

            // content.append(&preview_wrap);
            // content.append(&preset_row);
            // content.append(&action_row);

let content = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(0)
    .margin_top(24)
    .margin_bottom(24)
    .margin_start(24)
    .margin_end(24)
    .hexpand(true)
    .vexpand(true)
    .valign(gtk::Align::Start)
    .build();

let preview_card = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(0)
    .css_classes(["card", "preview-card"])
                //.overflow(gtk::Overflow::Hidden)
    .build();

preview_card.append(&preview_wrap);

let main_box = gtk::Box::new(gtk::Orientation::Vertical, 24);

let controls_box = gtk::Box::builder()
    .orientation(gtk::Orientation::Vertical)
    .spacing(24)
    .build();

            controls_box.append(&preset_row);
            controls_box.append(&action_row);

            main_box.append(&preview_card);
            main_box.append(&controls_box);

content.append(&main_box);

            #[cfg(feature = "gamma")]
            content.append(&gamma_label);

let sidebar_page = adw::NavigationPage::builder()
    .title("Monitors")
    .child(&sidebar)
    .build();

let content_page = adw::NavigationPage::builder()
    .title("Preview")
    .child(&content)
    .build();

let split_view = adw::NavigationSplitView::builder()
    .sidebar(&sidebar_page)
    .content(&content_page)
    .build();

            let toolbar_view = adw::ToolbarView::new();
            toolbar_view.add_top_bar(&header);
            toolbar_view.set_content(Some(&split_view));


let hud_css = gtk::CssProvider::new();
hud_css.load_from_string(r#"
.floating-hud {
    background: alpha(@window_bg_color, 0.88);

    border-radius: 999px;

    padding: 12px 20px;

    border: 1px solid alpha(@window_fg_color, 0.08);

    box-shadow:
        0 12px 32px rgba(0,0,0,0.22),
        0 2px 8px rgba(0,0,0,0.12);
}

.hud-label {
    font-size: 13px;
    font-weight: 600;
}

.preview-card {
    border-radius: 18px;
}

.monitor-check {
    border-radius: 14px;
}

.color-selector {
    border-radius: 14px;
    padding: 8px;
}
"#);
if let Some(display) = gdk::Display::default() {
    gtk::style_context_add_provider_for_display(
        &display,
        &hud_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

let root_overlay = gtk::Overlay::new();

//
// Main interactive content
//

root_overlay.set_child(Some(&toolbar_view));

//
// Floating bottom HUD
//

#[cfg(feature = "gamma")]
let hud = build_bottom_hud(&gamma_label);

#[cfg(not(feature = "gamma"))]
let hud = build_bottom_hud();

//
// IMPORTANT:
// Makes overlay ignore pointer events
// outside the actual HUD widget.
//

root_overlay.add_overlay(&hud);
root_overlay.set_measure_overlay(&hud, false);
root_overlay.set_clip_overlay(&hud, false);

//
// Prevent HUD from blocking content.
//

hud.set_can_target(false);

//
// But allow the actual floating panel to receive events.
//

if let Some(revealer) = hud.downcast_ref::<gtk::Revealer>() {
    if let Some(child) = revealer.child() {
        child.set_can_target(true);
    }
}

self.set_content(Some(&root_overlay));

            self.connect_close_request(glib::clone!(
                #[strong] overlays,
                #[strong] labels,
                move |_| {
                    for ov in &overlays {
                        ov.show_on_monitor(None);
                        ov.hide_overlay();
                        ov.close();
                        ov.destroy(); // TODO
                    }
                    for lbl in &labels {
                        lbl.close();
                    }
                    glib::Propagation::Proceed
                }
            ));

            for lbl in &labels {
                lbl.present();
            }
        }
    }

    // Keep gamma listener alive when the feature is enabled.
    #[cfg(feature = "gamma")]
    mod gamma_state {
        use super::*;

        impl MainWindow {
            pub(super) fn gamma_listener(&self) -> &RefCell<Option<GammaListener>> {
                // This helper is never called directly; it exists only to keep the
                // field logically grouped in one place.
                &self.imp().gamma_listener
            }
        }
    }

    #[cfg(feature = "gamma")]
    mod imp_gamma_field {
        use super::*;

        impl imp::MainWindow {
            pub(crate) fn ensure_gamma_field(&self) -> &RefCell<Option<GammaListener>> {
                // If you add the field below, this method is unnecessary.
                unimplemented!()
            }
        }
    }
}

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
                    color: RefCell::new(gdk::RGBA::WHITE)
                }
            }
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ColorSurface {
            const NAME: &'static str = "WhiteScreenColorSurface";
            type Type = super::ColorSurface;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for ColorSurface { }

        impl WidgetImpl for ColorSurface {
    fn measure(
        &self,
        orientation: gtk::Orientation,
        for_size: i32,
    ) -> (i32, i32, i32, i32) {

        match orientation {
            gtk::Orientation::Horizontal => {
                (64, 256, -1, -1)
            }

            gtk::Orientation::Vertical => {
                (64, 256, -1, -1)
            }

            _ => unreachable!(),
        }
    }

    fn snapshot(&self, snapshot: &gtk::Snapshot) {
        let widget = self.obj();

        let rect = gtk::graphene::Rect::new(
            0.0,
            0.0,
            widget.width() as f32,
            widget.height() as f32,
        );

        snapshot.append_color(
            &*self.color.borrow(),
            &rect,
        );
    }
        }

        // TODO
        //     fn snapshot(&self, snapshot: &gtk::Snapshot) {
        //         let widget = self.obj();

        //         let width = widget.width() as f32;
        //         let height = widget.height() as f32;

        //         let rect = gtk::graphene::Rect::new(
        //             0.0,
        //             0.0,
        //             width,
        //             height,
        //         );

        //         snapshot.append_color(
        //             &*self.color.borrow(),
        //             &rect,
        //         );
        //     }
        // }
    }

    glib::wrapper! {
        pub struct ColorSurface(ObjectSubclass<imp::ColorSurface>)
            @extends gtk::Widget,
            @implements gio::ActionGroup, gio::ActionMap,
                gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                gtk::Root, gtk::Native, gtk::ShortcutManager;
    }

    impl ColorSurface {
        pub fn new() -> Self {
            glib::Object::new()
        }

        pub fn set_rgba(&self, rgba: gdk::RGBA) {
            let imp = self.imp();
            if *imp.color.borrow() != rgba {
                *imp.color.borrow_mut() = rgba;
                self.queue_draw();
            }
        }
    }
}

fn main() {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(|app| {

        if let Some(win) = app
            .windows()
            .into_iter()
            .find(|w| w.is::<main_window::MainWindow>())
        {
            win.present();
            return;
        }

        // let display = match gdk::Display::default() {
        //     Some(d) => d,
        //     None => {
        //         eprintln!("No display available");
        //         return;
        //     }
        // };

        // TODO
        // if !display.is::<gdk_wayland::WaylandDisplay>() {
        //     gtk::AlertDialog::builder()
        //         .message("Wayland required")
        //         .detail("Whitescreen requires Wayland.")
        //         .build()
        //         .show(gtk::Window::NONE);

        //     return;
        // }

        // TODO
        app.set_accels_for_action("win.about", &["F1"]);
        app.set_accels_for_action("window.close", &["<Ctrl>Q"]);

        if !gtk_layer_shell::is_supported() {
            gtk::AlertDialog::builder()
                .message("Compositor not supported")
                .detail(
                    "White Screen requires a Wayland compositor that supports the wlr-layer-shell protocol (e.g. Niri, Sway, Hyprland, Wayfire, KDE Plasma ≥ 6).",
                )
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        let display = gdk::Display::default().expect("no GDK display");
        let mon_model = display.monitors();
        let monitors_vec: Vec<gdk::Monitor> = (0..mon_model.n_items())
            .filter_map(|i| mon_model.item(i)?.downcast::<gdk::Monitor>().ok())
            .collect();

        if monitors_vec.is_empty() {
            gtk::AlertDialog::builder()
                .message("No monitors found")
                .detail("White Screen needs at least one active monitor.")
                .build()
                .show(gtk::Window::NONE);
            return;
        }

        let overlays: Vec<screen_overlay::ScreenOverlay> = monitors_vec
            .iter()
            .map(|_| screen_overlay::ScreenOverlay::new(app))
            .collect();

        let labels: Vec<monitor_label::MonitorLabel> = monitors_vec
            .iter()
            .enumerate()
            .map(|(i, mon)| {
                let conn = monitor_title(mon, i);
                let sub = monitor_subtitle(mon);
                let sub_ref = if sub.is_empty() { None } else { Some(sub.as_str()) };
                monitor_label::MonitorLabel::new(app, mon, &conn, sub_ref)
            })
            .collect();

        let win = main_window::MainWindow::new(app, overlays, labels);
        win.present();
    });

    app.run();
}
