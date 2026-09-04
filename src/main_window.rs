
use std::{cell::Cell, cell::RefCell, collections::{HashMap, HashSet}, rc::Rc};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};

use crate::color_surface;

#[cfg(feature = "gamma")]
use crate::gamma::GammaListener;

// Minimum preview size; it expands to fill the pane.
const PREVIEW_W: i32 = 480;
const PREVIEW_H: i32 = 270; // 16:9

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -FRAC_PI_2,       0.0         );
    cr.arc(x + w - r, y + h - r, r,  0.0,              FRAC_PI_2  );
    cr.arc(x + r,     y + h - r, r,  FRAC_PI_2,        PI         );
    cr.arc(x + r,     y + r,     r,  PI,       3.0 * FRAC_PI_2    );
    cr.close_path();
}

pub fn monitor_title(mon: &gdk::Monitor, index: usize) -> String {
    mon.connector()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Monitor {}", index + 1))
}

pub fn monitor_subtitle(mon: &gdk::Monitor) -> String {
    [mon.model(), mon.manufacturer()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(" — ")
}

fn build_status_bar(
    #[cfg(feature = "gamma")] gamma_icon:  &gtk::Image,
    #[cfg(feature = "gamma")] gamma_label: &gtk::Label,
) -> gtk::Widget {
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .css_classes(["status-bar"])
        .build();

    // Left: ESC hint.
    let left = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Start)
        .build();
    left.append(
        &gtk::Image::builder()
            .icon_name("input-keyboard-symbolic")
            .pixel_size(16)
            .css_classes(["dim-label"])
            .build(),
    );
    left.append(
        &gtk::Label::builder()
            .label("Press ESC to exit overlay")
            .css_classes(["dim-label"])
            .build(),
    );
    bar.append(&left);

    // Spacer pushes the gamma group to the far right.
    bar.append(&gtk::Box::builder().hexpand(true).build());

    // Right: gamma status (sun / moon icon + label).
    #[cfg(feature = "gamma")]
    {
        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .build();
        right.append(gamma_icon);
        gamma_label.add_css_class("dim-label");
        right.append(gamma_label);
        bar.append(&right);
    }

    bar.upcast()
}

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

// ── MainWindow ────────────────────────────────────────────────────────────────
use crate::screen_overlay::ScreenOverlay;
use crate::monitor_label;

/// One physical monitor together with the two layer-shell windows that belong
/// to it. Keeping them in a single record removes the old arrangement, where
/// `main.rs` and `MainWindow` each built their own `Vec` from `display.monitors()`
/// and then indexed into the other's list positionally.
pub struct MonitorEntry {
    pub monitor: gdk::Monitor,
    /// See `monitor_keys()`. Stored rather than recomputed so that every
    /// consumer agrees on the identity, duplicate suffix included.
    pub key:     String,
    pub overlay: ScreenOverlay,
    pub label:   monitor_label::MonitorLabel,
    /// Change handlers on `monitor`, disconnected when the entry is retired.
    /// The entry outlives the monitor's presence (see `MainWindow::graveyard`),
    /// so leaving them connected would keep firing for a screen that is gone.
    pub handlers: Vec<glib::SignalHandlerId>,
}

/// Identity of one monitor, used to remember the user's selection across an
/// unplug/replug cycle and to match a returning monitor to the windows it had.
///
/// Deliberately free of geometry. The previous key mixed in the resolution and
/// position, so changing the mode or dragging a screen in the display settings
/// read as "a different monitor": the selection was silently dropped and the
/// overlay stranded.
fn monitor_ident(mon: &gdk::Monitor) -> String {
    if let Some(c) = mon.connector().filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    // No connector name reported: fall back to the EDID strings. Two identical
    // panels then collide, which monitor_keys() resolves.
    let edid = [mon.manufacturer(), mon.model(), mon.description()]
        .into_iter()
        .flatten()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("|");
    if edid.is_empty() { "monitor".to_string() } else { edid }
}

/// Keys for a whole monitor list at once, so that indistinguishable monitors
/// (two of the same model, neither reporting a connector) get "...#2", "...#3"
/// suffixes instead of sharing one key and being selected as a single unit.
fn monitor_keys(mons: &[gdk::Monitor]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    mons.iter()
        .map(|m| {
            let base = monitor_ident(m);
            let n = seen.entry(base.clone()).and_modify(|n| *n += 1).or_insert(1);
            if *n == 1 { base } else { format!("{base}#{n}") }
        })
        .collect()
}

    mod imp {
        use super::*;

        pub struct MainWindow {
            /// Current monitors, rebuilt on every hot-plug event.
            pub entries:  RefCell<Vec<MonitorEntry>>,
            /// Selected monitors, keyed by `monitor_keys()` so the choice
            /// survives unplug/replug rather than being tied to a list
            /// position. Keys for absent monitors are kept, not pruned: that
            /// is what makes a replug restore the selection.
            pub selected: RefCell<HashSet<String>>,
            /// Colour currently applied, so overlays created by a later hot-plug
            /// start out matching the ones already on screen.
            pub color:    Cell<gdk::RGBA>,
            pub identify: Cell<bool>,

            /// True while show_selected() has overlays up, so that a monitor
            /// plugged in mid-session is covered too instead of staying the
            /// one conspicuously bright screen in the room.
            pub overlays_shown: Cell<bool>,
            /// Set while a sync is queued on the main loop; see schedule_sync().
            pub sync_queued: Cell<bool>,

            // Widgets that outlive build_ui because hot-plug has to update them.
            /// Windows for monitors that are not currently attached. They are
            /// hidden but deliberately never destroyed and never detached from
            /// the application: both routes go through
            /// gtk_application_remove_window(), whose handler dereferences a
            /// surface these windows may never have had.
            ///
            /// Not purely a leak: sync_monitors() takes windows back out of
            /// here by key, so a dock/undock cycle reuses one pair of windows
            /// instead of stacking up a new pair every time. Growth is bounded
            /// by the number of distinct monitors seen in one session.
            pub graveyard: RefCell<Vec<MonitorEntry>>,

            pub mon_box:   RefCell<Option<gtk::Box>>,
            pub show_btn:  RefCell<Option<gtk::Button>>,
            pub preview:   RefCell<Option<color_surface::ColorSurface>>,

            #[cfg(feature = "gamma")]
            pub gamma_listener: RefCell<Option<GammaListener>>,
        }

        // gdk::RGBA has no Default impl, so this cannot be derived.
        impl Default for MainWindow {
            fn default() -> Self {
                Self {
                    entries:  RefCell::new(Vec::new()),
                    selected: RefCell::new(HashSet::new()),
                    color:    Cell::new(gdk::RGBA::WHITE),
                    identify: Cell::new(false),
                    overlays_shown: Cell::new(false),
                    sync_queued: Cell::new(false),
                    graveyard: RefCell::new(Vec::new()),
                    mon_box:  RefCell::new(None),
                    show_btn: RefCell::new(None),
                    preview:  RefCell::new(None),
                    #[cfg(feature = "gamma")]
                    gamma_listener: RefCell::new(None),
                }
            }
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
        pub fn new(app: &adw::Application) -> Self {
            let win: Self = glib::Object::builder()
                .property("application", app)
                .property("title", "White Screen")
                .property("default-width",  900i32)
                .property("default-height", 660i32)
                .build();
            win.build_ui();
            win
        }

        fn build_ui(&self) {
            // ── Single consolidated CSS provider ──────────────────────────
            let css = gtk::CssProvider::new();
            css.load_from_string(r#"
/* ── Preset / custom chips ──────────────────────────────────────────── */

/* Hide the native indicator — selection is shown by the border. */
.preset-chip check {
    min-width: 0; min-height: 0; -gtk-icon-size: 0px;
    opacity: 0; padding: 0; margin: 0;
}
.preset-chip {
    border-radius: 14px;
    padding: 6px;
    border: 2px solid transparent;
    background: transparent;
    transition: background-color 140ms ease, border-color 140ms ease, transform 110ms ease;
}
.preset-chip:hover   { background: alpha(@window_fg_color, 0.05); }
.preset-chip:checked { border-color: @accent_bg_color; background: alpha(@accent_bg_color, 0.12); }
.preset-chip:active  { transform: scale(0.97); }
.color-label         { font-size: 12px; font-weight: 600; opacity: 0.85; }

/* Custom eyedropper swatch */
.custom-swatch-icon {
    border: 2px solid alpha(@window_fg_color, 0.20);
    border-radius: 12px;
    color: @window_fg_color;
}
.preset-chip:checked .custom-swatch-icon {
    border-color: @accent_bg_color;
    color: @accent_bg_color;
}

/* ── Monitor rows ───────────────────────────────────────────────────── */

.monitor-check check {
    min-width: 0; min-height: 0; -gtk-icon-size: 0px;
    opacity: 0; padding: 0; margin: 0;
}
.monitor-check {
    border-radius: 14px;
    padding: 2px;
    border: 2px solid transparent;
    transition: background-color 180ms ease, border-color 180ms ease;
}
.monitor-check:checked {
    background-color: alpha(@accent_bg_color, 0.10);
    border-color:     alpha(@accent_bg_color, 0.45);
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

/* Right-hand selection badge: empty ring → filled accent check */
.mon-check-badge {
    min-width: 22px; min-height: 22px;
    border-radius: 999px;
    border: 2px solid alpha(@window_fg_color, 0.28);
    color: transparent;
    transition: background-color 160ms ease, border-color 160ms ease, color 160ms ease;
}
.monitor-check:checked .mon-check-badge {
    background: @accent_bg_color;
    border-color: @accent_bg_color;
    color: @accent_fg_color;
}

/* ── Cards ──────────────────────────────────────────────────────────── */

.controls-card, .about-card {
    background: @card_bg_color;
    border-radius: 16px;
}
.controls-card { padding: 16px 20px; }
.about-card    { padding: 14px 16px; }
.section-title { font-weight: 700; }
.about-title   { font-weight: 700; }
.about-link    { padding: 0; min-height: 0; }

/* ── Action tiles ───────────────────────────────────────────────────── */

.action-tile   { border-radius: 14px; padding: 8px 18px; }
.tile-title    { font-weight: 700; }
.tile-subtitle { font-size: 12px; opacity: 0.80; }

/* ── Preview ────────────────────────────────────────────────────────── */

.preview-frame {
    border-radius: 14px;
    overflow: hidden;
    box-shadow: 0 6px 20px rgba(0,0,0,0.30);
}

/* ── Bottom status bar ──────────────────────────────────────────────── */

.status-bar {
    padding: 8px 16px;
    border-top: 1px solid alpha(@window_fg_color, 0.08);
}
"#);
            // main.rs refuses to build the window without a display, so this
            // is the one place the expectation is genuinely upheld.
            let display = gdk::Display::default()
                .expect("MainWindow built without a GDK display");

            gtk::style_context_add_provider_for_display(
                &display, &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );

            // ── Header ────────────────────────────────────────────────────
            let header = adw::HeaderBar::new();
            header.set_title_widget(Some(
                &adw::WindowTitle::builder()
                    .title("White Screen")
                    .subtitle("Display color overlay")
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
                        .version(crate::APP_VERSION)
                        .license_type(gtk::License::Gpl30)
                        .website(crate::WEBSITE)
                        .issue_url("https://github.com/SergeGris/whitescreen/issues")
                        .comments("Fill any monitor with a solid color.")
                        .build()
                        .present(Some(&win));
                }
            ));
            self.add_action(&about);

            // App-scoped so ScreenOverlay's ESC handler can reach it without
            // holding a reference back to this window.
            if let Some(app) = self.application() {
                let hide = gio::SimpleAction::new("hide-overlays", None);
                hide.connect_activate(glib::clone!(
                    #[weak(rename_to = win)] self,
                    move |_, _| win.hide_all_overlays()
                ));
                app.add_action(&hide);
            }

            // ── Monitor list ──────────────────────────────────────────────
            // Enumerated once, here. The rows themselves are built by
            // rebuild_monitor_rows() so that hot-plug can redraw them.
            let mon_model = display.monitors();

            // ── Action tiles (built BEFORE the monitor loop so the monitor
            //    toggles can weakly reference show_btn) ─────────────────────

            fn innerr(icon: &str, titles: impl AsRef<str>, subtitles: impl AsRef<str>) -> gtk::Box {
                let icon = gtk::Image::from_icon_name(icon);
                icon.set_pixel_size(20);
                let title = gtk::Label::builder()
                    .label(titles.as_ref())
                    .css_classes(["tile-title"])
                    .halign(gtk::Align::Start)
                    .build();
                let subtitle = gtk::Label::builder()
                    .label(subtitles.as_ref())
                    .css_classes(["tile-subtitle"])
                    .halign(gtk::Align::Start)
                    .build();
                let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).halign(gtk::Align::Start).build();
                text.append(&title);
                text.append(&subtitle);
                let inner = gtk::Box::builder().orientation(gtk::Orientation::Horizontal).spacing(12).build();
                inner.append(&icon);
                inner.append(&text);
                inner
            }

            let show_btn = gtk::Button::builder()
                .child(&innerr("video-display-symbolic", "Show on selected", "Fill selected monitors"))
                .css_classes(["suggested-action", "action-tile"])
                .sensitive(false) // sync_monitors() enables it once a monitor is selected
                .build();
            show_btn.connect_clicked(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_| win.show_selected()
            ));
            self.imp().show_btn.replace(Some(show_btn.clone()));

            let ident_btn = gtk::ToggleButton::builder()
                .tooltip_text("Show monitor connector labels on each screen")
                .css_classes(["action-tile"])
                .child(&innerr("dialog-information-symbolic", "Identify", "Identify each monitor"))
                .build();
            ident_btn.connect_toggled(glib::clone!(
                #[weak(rename_to = win)] self,
                move |btn| win.set_identify(btn.is_active())
            ));

            let hide_btn = gtk::Button::builder()
                .child(&innerr("view-conceal-symbolic", "Hide ALL", "Hide all overlays"))
                .css_classes(["action-tile"])
                .tooltip_text("Hide the overlay on all monitors (or press ESC on the overlay)")
                .build();
            hide_btn.connect_clicked(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_| win.hide_all_overlays()
            ));

            let action_bar = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(12)
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

            // Rows live in their own box so rebuild_monitor_rows() can clear
            // and repopulate it on hot-plug without touching the headings.
            let rows_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .build();
            mon_box.append(&rows_box);
            self.imp().mon_box.replace(Some(rows_box));

            let scroller = gtk::ScrolledWindow::builder()
                .min_content_width(300)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&mon_box)
                .build();

            // ── About card, pinned to the bottom of the sidebar ───────────
            let about_head = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            about_head.append(
                &gtk::Image::builder().icon_name("dialog-information-symbolic").pixel_size(16).build(),
            );
            about_head.append(
                &gtk::Label::builder().label("About White Screen").css_classes(["about-title"]).halign(gtk::Align::Start).build(),
            );

            let about_card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .css_classes(["about-card"])
                .margin_start(12).margin_end(12).margin_bottom(12).margin_top(6)
                .build();
            about_card.append(&about_head);
            about_card.append(
                &gtk::Label::builder()
                    .label("Fill any monitor with a solid color.")
                    .css_classes(["dim-label"])
                    .halign(gtk::Align::Start).xalign(0.0).wrap(true)
                    .build(),
            );
            about_card.append(
                &gtk::LinkButton::builder()
                    .uri(super::WEBSITE)
                    .label("Learn more")
                    .halign(gtk::Align::Start)
                    .css_classes(["about-link"])
                    .build(),
            );
            about_card.append(
                &gtk::Label::builder()
                    .label(format!("v{}", super::APP_VERSION))
                    .css_classes(["dim-label", "caption"])
                    .halign(gtk::Align::Start)
                    .build(),
            );

            let sidebar = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
            sidebar.append(&scroller);
            // TODO
            // sidebar.append(&about_card);

            // ── Colour application ────────────────────────────────────────
            // Single source of truth is MainWindow::apply_color, which also
            // records the colour so overlays created by a later hot-plug are
            // born with it instead of defaulting to white.

            // ── Preview – expands to fill the pane, with a fullscreen FAB ──
            let preview = color_surface::ColorSurface::new();
            preview.set_size_request(PREVIEW_W, PREVIEW_H); // minimum
            preview.set_halign(gtk::Align::Center);
            preview.set_valign(gtk::Align::Start);
            preview.add_css_class("preview-surface");
            self.imp().preview.replace(Some(preview.clone()));

            let preview_frame = gtk::Frame::builder()
                .child(&preview)
                .css_classes(["preview-frame"])
                .halign(gtk::Align::Center)
                .build();

            let expand_btn = gtk::Button::builder()
                .icon_name("view-fullscreen-symbolic")
                .css_classes(["circular", "suggested-action"])
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .margin_end(16)
                .margin_bottom(16)
                .tooltip_text("Show on selected monitors")
                .build();
            expand_btn.connect_clicked(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_| win.show_selected()
            ));

            let preview_overlay = gtk::Overlay::builder()
                .child(&preview_frame)
                .halign(gtk::Align::Center)
                .build();
            preview_overlay.add_overlay(&expand_btn);

            // ── Preset chips + custom ─────────────────────────────────────
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
                    .width_request(64).height_request(40)
                    .build();
                swatch.set_draw_func(move |_, cr, w, h| {
                    rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 12.0);
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
                    .build();

                if let Some(root) = &group_root {
                    btn.set_group(Some(root));
                } else {
                    group_root = Some(btn.clone());
                }

                btn.set_cursor_from_name(Some("pointer"));
                btn.connect_toggled(glib::clone!(
                    #[weak(rename_to = win)] self,
                    move |b| if b.is_active() { win.apply_color(rgba); }
                ));

                preset_buttons.borrow_mut().push((btn.clone(), rgba));
                preset_row.append(&btn);
            }

            // Custom chip — eyedropper icon; a CheckButton in the preset group.
            let custom_rgba = Rc::new(Cell::new(gdk::RGBA::WHITE));

            let custom_icon = gtk::Image::builder()
                .icon_name("color-select-symbolic")
                .pixel_size(22)
                .css_classes(["custom-swatch-icon"])
                .halign(gtk::Align::Center)
                .build();
            custom_icon.set_size_request(64, 40);

            let custom_inner = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .margin_top(6).margin_bottom(6)
                .margin_start(8).margin_end(8)
                .build();
            custom_inner.append(&custom_icon);
            custom_inner.append(
                &gtk::Label::builder()
                    .label("Custom")
                    .css_classes(["color-label"])
                    .halign(gtk::Align::Center)
                    .build(),
            );

            let custom_btn = gtk::CheckButton::builder()
                .child(&custom_inner)
                .css_classes(["preset-chip"])
                .tooltip_text("Pick a custom color")
                .build();
            custom_btn.set_cursor_from_name(Some("pointer"));
            if let Some(root) = &group_root {
                custom_btn.set_group(Some(root));
            }

            let custom_dialog = gtk::ColorDialog::builder()
                .title("Select custom color")
                .with_alpha(false)
                .modal(true)
                .build();

            // Selecting the chip only re-applies the stored custom colour.
            custom_btn.connect_toggled(glib::clone!(
                #[weak(rename_to = win)] self,
                #[strong] custom_rgba,
                move |btn| {
                    if btn.is_active() { win.apply_color(custom_rgba.get()); }
                }
            ));

            // Opening the picker is driven by the *click*, not by `toggled`.
            // A CheckButton in a radio group emits no `toggled` when it is
            // already active, so the old code could never reopen the dialog:
            // once Custom was selected the user had to pick another preset and
            // come back just to change the colour.
            let custom_click = gtk::GestureClick::new();
            custom_click.connect_released(glib::clone!(
                #[weak(rename_to = win)] self,
                #[weak] custom_btn,
                #[strong] custom_dialog,
                #[strong] custom_rgba,
                move |_, _, _, _| {
                    custom_btn.set_active(true);
                    custom_dialog.choose_rgba(
                        Some(&win),
                        Some(&custom_rgba.get()),
                        gio::Cancellable::NONE,
                        glib::clone!(
                            #[weak] win,
                            #[strong] custom_rgba,
                            move |res| {
                                // Err = the user cancelled; keep the previous
                                // custom colour rather than leaving the chip
                                // selected but showing something stale.
                                if let Ok(rgba) = res {
                                    custom_rgba.set(rgba);
                                    win.apply_color(rgba);
                                }
                            }
                        ),
                    );
                }
            ));
            custom_btn.add_controller(custom_click);
            preset_row.append(&custom_btn);

            // Activate the first preset at startup. This used to search for
            // BLACK despite the comment and despite WHITE being PRESETS[0],
            // so the app called "White Screen" opened on black.
            if let Some((btn, rgba)) = preset_buttons.borrow().first() {
                btn.set_active(true);
                self.apply_color(*rgba);
            }

            // ── Gamma status indicator (feature-gated) ────────────────────
            #[cfg(feature = "gamma")]
            let gamma_icon = gtk::Image::builder()
                .icon_name("weather-clear-symbolic") // sun = normal rendering
                .pixel_size(16)
                .tooltip_text("No color filter")
                .build();
            #[cfg(feature = "gamma")]
            let gamma_label = gtk::Label::new(Some("Gamma control is inactive"));

            #[cfg(feature = "gamma")]
            {
                let (sender, receiver) = async_channel::unbounded::<bool>();
                let icon = gamma_icon.clone();
                let lbl  = gamma_label.clone();
                glib::spawn_future_local(async move {
                    while let Ok(active) = receiver.recv().await {
                        icon.set_icon_name(Some(if active {
                            "weather-clear-night-symbolic"
                        } else {
                            "weather-clear-symbolic"
                        }));
                        icon.set_tooltip_text(Some(if active {
                            "A color filter is active (another app holds gamma)"
                        } else {
                            "No color filter"
                        }));
                        lbl.set_text(if active {
                            "Gamma control is active"
                        } else {
                            "Gamma control is inactive"
                        });
                    }
                });
                // The prober opens its own Wayland connection on a background
                // thread and polls the compositor. Setting WHITESCREEN_NO_GAMMA
                // leaves the indicator at "inactive" and starts nothing, so the
                // feature can be ruled in or out without a rebuild.
                if std::env::var_os("WHITESCREEN_NO_GAMMA").is_none() {
                    self.imp().gamma_listener.replace(Some(GammaListener::new(move |e| {
                        let _ = sender.send_blocking(e);
                    })));
                } else {
                    // Drop the sender so the receiver future above finishes
                    // instead of parking forever.
                    drop(sender);
                }
            }

            // ── "Choose Color" card ───────────────────────────────────────
            let controls_card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(14)
                .css_classes(["controls-card"])
                .build();
            controls_card.append(
                &gtk::Label::builder()
                    .label("Choose Color")
                    .css_classes(["section-title"])
                    .halign(gtk::Align::Start)
                    .build(),
            );
            controls_card.append(&preset_row);
            controls_card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            controls_card.append(&action_bar);

            // ── Content layout (right pane) ───────────────────────────────
            let main_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(16)
                .build();
            main_box.append(&preview_overlay);
            main_box.append(&controls_card);

            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_top(20).margin_bottom(20)
                .margin_start(20).margin_end(20)
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

            // ── Toolbar view: header + content + bottom status bar ────────
            #[cfg(feature = "gamma")]
            let statusbar = build_status_bar(&gamma_icon, &gamma_label);
            #[cfg(not(feature = "gamma"))]
            let statusbar = build_status_bar();

            let toolbar_view = adw::ToolbarView::new();
            toolbar_view.add_top_bar(&header);
            toolbar_view.set_content(Some(&split_view));
            toolbar_view.add_bottom_bar(&statusbar);

            self.set_content(Some(&toolbar_view));

            // ── Monitor hot-plug ──────────────────────────────────────────
            // display.monitors() is a live GListModel. Without this the app
            // kept overlays for unplugged monitors and never noticed new ones.
            mon_model.connect_items_changed(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_, _, _, _| win.schedule_sync()
            ));

            // Initial population.
            self.sync_monitors();

            // ── Teardown ──────────────────────────────────────────────────
            self.connect_close_request(glib::clone!(
                #[weak(rename_to = win)] self,
                #[upgrade_or] glib::Propagation::Proceed,
                move |_| {
                    // Do NOT destroy() the overlays and badges here.
                    //
                    // They are layer-shell windows, and most of them have never
                    // been realized -- one is created per monitor at startup,
                    // but only shown on demand. gtk_window_destroy() on such a
                    // window emits a signal whose handler calls
                    // gdk_surface_get_display() on a surface that does not
                    // exist, giving
                    //   Gdk-CRITICAL gdk_surface_get_display:
                    //   assertion 'GDK_IS_SURFACE (surface)' failed
                    // and then a segfault.
                    //
                    // Hiding them and quitting is also a clearer statement of
                    // intent: the destroy() calls only existed to drop the
                    // application's window count to zero so that closing the
                    // main window ended the program. Saying so directly is
                    // both safer and more obvious.
                    //
                    // NB: no show_on_monitor(None) either -- that call
                    // *presents* the window, so an older version of this
                    // teardown flashed every overlay full-screen on the way out.
                    for ov in win.overlays() {
                        ov.hide_overlay();
                    }
                    for l in win.labels() {
                        l.set_visible(false);
                    }

                    // Stop the gamma prober thread deterministically rather
                    // than leaving it to process exit.
                    #[cfg(feature = "gamma")]
                    win.imp().gamma_listener.replace(None);

                    if let Some(app) = win.application() {
                        app.quit();
                    }
                    glib::Propagation::Proceed
                }
            ));
        }

        // ── Colour ───────────────────────────────────────────────────────────

        /// Apply `rgba` to the preview and every overlay, and remember it so
        /// overlays created by a later hot-plug start out matching.
        pub fn apply_color(&self, rgba: gdk::RGBA) {
            let imp = self.imp();
            imp.color.set(rgba);

            let preview = imp.preview.borrow().clone();
            if let Some(p) = preview {
                p.set_rgba(rgba);
            }
            for ov in self.overlays() {
                ov.set_color(rgba);
            }
        }

        // Snapshot helpers. Every method below calls into GTK while walking the
        // entry list, and GTK can re-enter us through the monitors ListModel —
        // holding a RefCell borrow across those calls risks a BorrowMutError,
        // so the windows are cloned out first (they are refcounted handles).
        fn overlays(&self) -> Vec<ScreenOverlay> {
            self.imp().entries.borrow().iter().map(|e| e.overlay.clone()).collect()
        }

        fn labels(&self) -> Vec<monitor_label::MonitorLabel> {
            self.imp().entries.borrow().iter().map(|e| e.label.clone()).collect()
        }

        // ── Overlay control ──────────────────────────────────────────────────

        /// Fill every selected monitor.
        pub fn show_selected(&self) {
            let imp = self.imp();

            let targets: Vec<(ScreenOverlay, gdk::Monitor)> = {
                let entries  = imp.entries.borrow();
                let selected = imp.selected.borrow();
                entries
                    .iter()
                    .filter(|e| selected.contains(&e.key) && e.monitor.is_valid())
                    .map(|e| (e.overlay.clone(), e.monitor.clone()))
                    .collect()
            };

            // Nothing attached to show on. Leave `overlays_shown` alone: if the
            // last selected monitor was just unplugged the overlays are still
            // logically up, and replugging it must bring the colour back.
            if targets.is_empty() {
                return;
            }
            imp.overlays_shown.set(true);

            for (overlay, monitor) in targets {
                overlay.show_on_monitor(Some(&monitor));
            }
            // Overlays and badges share Layer::Overlay, where the compositor
            // stacks by map order — so re-present the badges last, otherwise
            // Identify would be hidden underneath the overlays just shown.
            if imp.identify.get() {
                self.present_labels();
            }
        }

        /// Hide every overlay. Bound to the app-scoped `hide-overlays` action,
        /// so one ESC press on any screen clears all of them.
        pub fn hide_all_overlays(&self) {
            self.imp().overlays_shown.set(false);
            for ov in self.overlays() {
                ov.hide_overlay();
            }
        }

        /// Toggle the per-monitor connector-name badges.
        pub fn set_identify(&self, on: bool) {
            self.imp().identify.set(on);
            if on {
                self.present_labels();
            } else {
                for l in self.labels() {
                    l.set_visible(false);
                }
            }
        }

        fn present_labels(&self) {
            for l in self.labels() {
                l.present();
            }
        }

        // ── Monitor tracking ─────────────────────────────────────────────────

        /// Queue a `sync_monitors()` for the next main-loop iteration.
        ///
        /// One physical hot-plug is rarely one signal: the display's list model
        /// typically emits items-changed twice (remove, then add) and a mode
        /// switch emits a burst of property notifies. Reconciling on each of
        /// them would rebuild the sidebar several times per event and, worse,
        /// tear an overlay down and put it straight back up mid-plug.
        /// Coalescing collapses the burst into one rebuild once the display has
        /// settled, and keeps the reconcile off GTK's own signal stack.
        pub fn schedule_sync(&self) {
            if self.imp().sync_queued.replace(true) {
                return;
            }
            glib::idle_add_local_once(glib::clone!(
                #[weak(rename_to = win)] self,
                move || {
                    win.imp().sync_queued.set(false);
                    win.sync_monitors();
                }
            ));
        }

        /// Watch one monitor for the changes that never reach the display's
        /// list model: a mode switch, a rescale, a move in the display
        /// arrangement, a connector name that only arrives later, or the
        /// monitor going invalid in place. Without these the sidebar keeps
        /// reporting whatever the screen looked like when the app started.
        fn track_monitor(&self, mon: &gdk::Monitor) -> Vec<glib::SignalHandlerId> {
            vec![
                mon.connect_invalidate(glib::clone!(
                    #[weak(rename_to = win)] self,
                    move |_| win.schedule_sync()
                )),
                // One handler for every property rather than six: geometry,
                // scale, refresh-rate, connector and valid all mean the same
                // thing here -- re-read this monitor -- and the reconcile is
                // coalesced and idempotent anyway.
                mon.connect_notify_local(None, glib::clone!(
                    #[weak(rename_to = win)] self,
                    move |_, _| win.schedule_sync()
                )),
            ]
        }

        /// Reconcile `entries` with the display's current monitor list, then
        /// redraw the sidebar. Idempotent: repeated calls only touch what
        /// actually changed, so it is safe to run on every hot-plug signal.
        pub fn sync_monitors(&self) {
            let Some(display) = gdk::Display::default() else { return };
            let Some(app) = self.application().and_downcast::<adw::Application>() else { return };

            let model = display.monitors();
            let current: Vec<gdk::Monitor> = (0..model.n_items())
                .filter_map(|i| model.item(i)?.downcast::<gdk::Monitor>().ok())
                .filter(|m| m.is_valid())
                .collect();
            let keys = monitor_keys(&current);

            let color = self.imp().color.get();
            let mut old       = self.imp().entries.take();
            let mut graveyard = self.imp().graveyard.take();
            let mut next: Vec<MonitorEntry> = Vec::with_capacity(current.len());
            let mut rekeyed: Vec<(String, String)> = Vec::new();

            for (i, (mon, key)) in current.iter().zip(keys.iter()).enumerate() {
                let title = monitor_title(mon, i);
                let sub   = monitor_subtitle(mon);
                let sub   = (!sub.is_empty()).then_some(sub);

                // Same GdkMonitor object, so this screen never went away: keep
                // its windows. An overlay already on screen must not blink
                // because some unrelated monitor was plugged in.
                if let Some(pos) = old.iter().position(|e| e.monitor == *mon) {
                    let mut e = old.remove(pos);
                    if e.key != *key {
                        // A connector name can show up late, and a duplicate
                        // suffix shifts when a twin is unplugged. Carry the
                        // selection across instead of dropping it.
                        let was = std::mem::replace(&mut e.key, key.clone());
                        rekeyed.push((was, key.clone()));
                    }
                    // The fallback title is positional ("Monitor 2"), so an
                    // unplug ahead of this one renames it.
                    e.label.set_info(&title, sub.as_deref());
                    next.push(e);
                    continue;
                }

                // A monitor seen earlier in this session: take its windows back
                // out of the graveyard. This is what stops a dock/undock cycle
                // from stacking up a new window pair every time round.
                if let Some(pos) = graveyard.iter().position(|e| e.key == *key) {
                    let mut e = graveyard.remove(pos);
                    e.monitor  = mon.clone();
                    e.handlers = self.track_monitor(mon);
                    e.overlay.set_color(color);
                    e.label.rebind(mon, &title, sub.as_deref());
                    next.push(e);
                    continue;
                }

                let overlay = ScreenOverlay::new(&app);
                overlay.set_color(color);
                let label = monitor_label::MonitorLabel::new(&app, mon, &title, sub.as_deref());

                next.push(MonitorEntry {
                    monitor:  mon.clone(),
                    key:      key.clone(),
                    overlay,
                    label,
                    handlers: self.track_monitor(mon),
                });
            }

            // Whatever is left in `old` was unplugged. Hide it and set it
            // aside; see MainWindow::graveyard for why it is not destroyed and
            // why keeping it is not simply a leak.
            for e in &mut old {
                for id in e.handlers.drain(..) {
                    e.monitor.disconnect(id);
                }
                e.overlay.hide_overlay();
                e.label.set_visible(false);
                e.overlay.unbind_monitor();
                e.label.unbind_monitor();
            }
            graveyard.extend(old);
            self.imp().graveyard.replace(graveyard);

            {
                let mut sel = self.imp().selected.borrow_mut();
                for (from, to) in rekeyed {
                    if sel.remove(&from) {
                        sel.insert(to);
                    }
                }
                // Selections for absent monitors are kept on purpose: unplug a
                // screen and plug it back in and the choice is still there.
                // Only a genuinely empty selection (first run, or the display
                // was empty at startup) picks a monitor on its own -- past that
                // point, "nothing selected" is the user's decision to keep.
                if sel.is_empty() {
                    if let Some(first) = next.first() {
                        sel.insert(first.key.clone());
                    }
                }
            }

            self.imp().entries.replace(next);

            // A monitor that arrives while the overlays are up gets covered
            // too, and one that comes back goes under the colour it left under.
            if self.imp().overlays_shown.get() {
                self.show_selected();
            } else if self.imp().identify.get() {
                // Badges for newly arrived monitors must respect Identify.
                self.present_labels();
            }

            self.rebuild_monitor_rows();
        }

        /// Rebuild the sidebar rows from `entries`.
        fn rebuild_monitor_rows(&self) {
            let imp = self.imp();
            // Cloned out of the RefCell so the borrow ends before the row
            // handlers (which touch imp) can run.
            let rows_box = match imp.mon_box.borrow().as_ref() {
                Some(b) => b.clone(),
                None    => return,
            };

            while let Some(child) = rows_box.first_child() {
                rows_box.remove(&child);
            }

            // Snapshot for the same re-entrancy reason as above; the row only
            // ever needs the monitor.
            let monitors: Vec<(String, gdk::Monitor)> =
                imp.entries.borrow().iter().map(|e| (e.key.clone(), e.monitor.clone())).collect();

            if monitors.is_empty() {
                // Zero monitors is a transient state during a hot-plug, not a
                // fatal error as it used to be treated in main.rs.
                rows_box.append(
                    &gtk::Label::builder()
                        .label("No monitors detected")
                        .css_classes(["dim-label"])
                        .halign(gtk::Align::Start)
                        .margin_top(12)
                        .build(),
                );
            }

            for (i, (key, mon)) in monitors.iter().enumerate() {
                rows_box.append(&self.build_monitor_row(i, key, mon));
            }

            self.update_show_sensitivity();
        }

        fn build_monitor_row(&self, i: usize, key: &str, mon: &gdk::Monitor) -> gtk::CheckButton {
            let geo = mon.geometry();
            let key = key.to_string();

            let icon = gtk::Image::builder()
                .icon_name("video-display-symbolic")
                .pixel_size(44)
                .valign(gtk::Align::Center)
                .build();

            let title_lbl = gtk::Label::builder()
                .label(monitor_title(mon, i))
                .halign(gtk::Align::Start)
                .css_classes(["heading"])
                .build();
            let res_lbl = gtk::Label::builder()
                .label(format!("{} × {}", geo.width(), geo.height()))
                .halign(gtk::Align::Start)
                .css_classes(["dim-label"])
                .build();

            // refresh_rate() reports 0 when the compositor does not know it;
            // the old code rendered that as a bogus "0.000 Hz".
            let rate_text = match mon.refresh_rate() {
                0 => format!("×{:.2}", mon.scale()),
                r => format!("{}.{:03} Hz  •  ×{:.2}", r / 1000, r % 1000, mon.scale()),
            };
            let rate_lbl = gtk::Label::builder()
                .label(rate_text)
                .halign(gtk::Align::Start)
                .css_classes(["dim-label"])
                .build();

            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(1)
                .hexpand(true)
                .valign(gtk::Align::Center)
                .build();
            text.append(&title_lbl);
            text.append(&res_lbl);
            text.append(&rate_lbl);

            let num_lbl = gtk::Label::builder()
                .label((i + 1).to_string())
                .css_classes(["monitor-number"])
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build();

            // Right-hand check badge (styling driven purely by :checked).
            let check_badge = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .pixel_size(12)
                .css_classes(["mon-check-badge"])
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build();

            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(12)
                .margin_top(10).margin_bottom(10)
                .margin_start(10).margin_end(12)
                .build();
            row.append(&num_lbl);
            row.append(&icon);
            row.append(&text);
            row.append(&check_badge);

            let check = gtk::CheckButton::builder()
                .active(self.imp().selected.borrow().contains(&key))
                .child(&row)
                .hexpand(true)
                .css_classes(["monitor-check"])
                .build();

            check.connect_toggled(glib::clone!(
                #[weak(rename_to = win)] self,
                #[strong] key,
                move |btn| {
                    {
                        let mut sel = win.imp().selected.borrow_mut();
                        if btn.is_active() { sel.insert(key.clone()); } else { sel.remove(&key); }
                    }
                    win.update_show_sensitivity();
                }
            ));

            check
        }

        fn update_show_sensitivity(&self) {
            let imp = self.imp();
            // Intersect, do not just count: selections for unplugged monitors
            // are kept deliberately, and "Show" must not look clickable when
            // none of the selected screens is currently attached.
            let any = {
                let sel = imp.selected.borrow();
                imp.entries.borrow().iter().any(|e| sel.contains(&e.key))
            };
            if let Some(btn) = imp.show_btn.borrow().as_ref() {
                btn.set_sensitive(any);
            }
        }
    }

    pub use MainWindow as Window;
