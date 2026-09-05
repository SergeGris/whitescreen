use std::{cell::Cell, cell::RefCell, collections::HashSet};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};

use crate::color_surface;
use crate::monitor_label;
use crate::screen_overlay::ScreenOverlay;
use crate::settings::Settings;

#[cfg(feature = "gamma")]
use crate::gamma::GammaListener;

// Minimum preview size; it expands to fill the pane.
const PREVIEW_W: i32 = 480;
const PREVIEW_H: i32 = 270; // 16:9

/// Colour the custom chip starts on. Cyan rather than white so the chip is
/// visibly *not* one of the presets on a first run, and so clicking it does
/// something even before the picker has ever been opened.
const DEFAULT_CUSTOM: gdk::RGBA = gdk::RGBA::new(0.0, 1.0, 1.0, 1.0);

/// Name stored in the settings file when the custom chip is the selected one.
const CUSTOM_CHIP: &str = "Custom";

/// Shown by whatever is asked to keep the screen on, where it displays a
/// reason -- so it says something the user would recognise.
const IDLE_REASON: &str = "A color overlay is showing";

/// Cycle bounds, in seconds. The floor is 0.5 s deliberately: a full-screen
/// colour flashing faster than that starts to approach the flash rate that
/// photosensitive-epilepsy guidance warns about, and no dead-pixel check
/// needs it.
const CYCLE_MIN:     f64 = 0.5;
const CYCLE_MAX:     f64 = 60.0;
const CYCLE_STEP:    f64 = 0.5;
const CYCLE_DEFAULT: f64 = 2.0;

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
            .label("Press any key to exit the overlay")
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
    dedup_idents(mons.iter().map(monitor_ident))
}

/// Give every identity in a list a unique key, appending an occurrence number
/// to repeats. The first occurrence keeps the bare identity, so the ordinary
/// case -- every monitor reporting a connector name -- yields exactly the
/// connector names.
///
/// Split out from `monitor_keys()` because it is the only part of monitor
/// identity that can be tested without a display connection.
fn dedup_idents(idents: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    idents
        .into_iter()
        .map(|base| {
            if used.insert(base.clone()) {
                return base;
            }
            // Climb until the suffix is free rather than trusting a per-identity
            // counter: an EDID string that itself ends in "#2" would otherwise
            // be handed the key generated for a duplicate of the bare name.
            (2..)
                .map(|n| format!("{base}#{n}"))
                .find(|cand| used.insert(cand.clone()))
                .expect("an unbounded range always yields an unused suffix")
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

        /// Preset chips in `PRESETS` order, so the cycle timer can drive the
        /// selection through the same widgets the user clicks.
        pub presets:    RefCell<Vec<gtk::CheckButton>>,
        pub custom_btn: RefCell<Option<gtk::CheckButton>>,
        /// Colour behind the custom chip. Kept here rather than in the chip's
        /// closure so the swatch, the picker and the saved settings all read
        /// the same value.
        pub custom_rgba: Cell<gdk::RGBA>,

        /// Cycle mode: step through the presets on a timer, for spotting a
        /// dead pixel or a stuck subpixel without clicking through by hand.
        pub cycling:       Cell<bool>,
        pub cycle_index:   Cell<usize>,
        pub cycle_secs:    Cell<f64>,
        pub cycle_source:  RefCell<Option<glib::SourceId>>,
        /// Chip that was selected when cycling started, restored when it
        /// stops, so the mode leaves the colour it found rather than
        /// whichever preset the timer happened to land on.
        pub pre_cycle:     RefCell<Option<gtk::CheckButton>>,

        /// Cleared after the first reconcile; see sync_monitors().
        pub first_sync: Cell<bool>,

        /// Persisted colour / selection / interval.
        pub settings: Settings,

        /// Cookie from `gtk_application_inhibit()`; 0 means "not inhibiting".
        pub idle_cookie: Cell<u32>,
        /// Overlay the inhibitor is attached to. Which window it names
        /// matters: the compositor only honours an inhibitor while that
        /// window is actually on screen, so it has to be an overlay and not
        /// the main window, which is behind them (or on another workspace).
        pub idle_window: RefCell<Option<ScreenOverlay>>,

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
                presets:     RefCell::new(Vec::new()),
                custom_btn:  RefCell::new(None),
                custom_rgba: Cell::new(super::DEFAULT_CUSTOM),
                cycling:      Cell::new(false),
                cycle_index:  Cell::new(0),
                cycle_secs:   Cell::new(super::CYCLE_DEFAULT),
                cycle_source: RefCell::new(None),
                pre_cycle:    RefCell::new(None),
                first_sync:   Cell::new(true),
                settings: Settings::load(),
                idle_cookie: Cell::new(0),
                idle_window: RefCell::new(None),
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

/* Hide the native indicator — selection is shown by the border.
   A grouped GtkCheckButton renames its indicator node from "check" to
   "radio", so the chips (which are one radio group) need both selectors:
   with only "check" every chip drew a stray radio circle next to its
   swatch. The monitor rows are ungrouped and keep the "check" node. */
.preset-chip check,
.preset-chip radio {
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

/* Custom chip: the swatch shows the colour it will apply, with the
   eyedropper floated on top so the chip still reads as "pick a colour".
   The badge carries its own background because it sits on an arbitrary
   colour -- a bare symbolic icon disappears against half of them. */
.custom-swatch-badge {
    background: alpha(@window_bg_color, 0.85);
    color: @window_fg_color;
    border-radius: 999px;
    padding: 5px;
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

.controls-card {
    background: @card_bg_color;
    border-radius: 16px;
    padding: 16px 20px;
}
.section-title { font-weight: 700; }

/* ── Action tiles ───────────────────────────────────────────────────── */

.action-tile   { border-radius: 14px; padding: 8px 18px; }
.tile-title    { font-weight: 700; }
.tile-subtitle { font-size: 12px; opacity: 0.80; }

/* ── Preview ────────────────────────────────────────────────────────── */

/* No `overflow: hidden` here: GTK's CSS has no such property, and it was
   logged as a parser error on every start. Clipping is the frame's own job. */
.preview-frame {
    border-radius: 14px;
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

        // ESC on this window hides the overlays too.
        //
        // On a compositor that honours the layer-shell keyboard grab the
        // overlay itself gets the key and this never fires. In fallback mode
        // it is not a grab but ordinary focus, and focus can sit here -- and
        // then the status bar would be telling the user to press a key that
        // nothing was listening for.
        //
        // Bubble phase, and only while overlays are up, so ESC keeps its
        // usual meaning for a popover or a dialog on top of this window.
        let esc = gtk::EventControllerKey::new();
        esc.connect_key_pressed(glib::clone!(
            #[weak(rename_to = win)] self,
            #[upgrade_or] glib::Propagation::Proceed,
            move |_, key, _, _| {
                if key == gdk::Key::Escape && win.imp().overlays_shown.get() {
                    win.hide_all_overlays();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        ));
        self.add_controller(esc);

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
        if !crate::layer_shell_available() {
            // The badge is a window anchored to a monitor corner and made
            // click-through, which is exactly what layer shell is for and
            // what an ordinary toplevel cannot be asked to do.
            ident_btn.set_sensitive(false);
            ident_btn.set_tooltip_text(Some(
                "Identify needs wlr-layer-shell, which this compositor does not support",
            ));
        }

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

        // ── Cycle mode ────────────────────────────────────────────────
        // A dead-pixel check means looking at red, then green, then blue,
        // then white, then black, and a stuck subpixel only shows up on one
        // of them. Doing that by hand means looking away from the screen to
        // click every time; the timer keeps both eyes on the panel.
        let cycle_btn = gtk::ToggleButton::builder()
            .css_classes(["action-tile"])
            .tooltip_text("Step through the preset colors on a timer")
            .child(&innerr("media-playlist-repeat-symbolic", "Cycle", "Step through colors"))
            .build();

        let cycle_secs = self
            .imp()
            .settings
            .cycle_interval()
            // A hand-edited or truncated settings file must not be able to
            // set a flash rate the UI itself refuses to offer.
            .filter(|s| (CYCLE_MIN..=CYCLE_MAX).contains(s))
            .unwrap_or(CYCLE_DEFAULT);
        self.imp().cycle_secs.set(cycle_secs);

        let cycle_spin = gtk::SpinButton::with_range(CYCLE_MIN, CYCLE_MAX, CYCLE_STEP);
        cycle_spin.set_digits(1);
        cycle_spin.set_value(cycle_secs);
        cycle_spin.set_tooltip_text(Some("Seconds on each color"));
        cycle_spin.connect_value_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |spin| win.set_cycle_interval(spin.value())
        ));

        cycle_btn.connect_toggled(glib::clone!(
            #[weak(rename_to = win)] self,
            move |btn| win.set_cycling(btn.is_active())
        ));

        let cycle_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Center)
            .build();
        cycle_row.append(&cycle_btn);
        cycle_row.append(
            &gtk::Label::builder().label("every").css_classes(["dim-label"]).build(),
        );
        cycle_row.append(&cycle_spin);
        cycle_row.append(
            &gtk::Label::builder().label("s").css_classes(["dim-label"]).build(),
        );

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

        let sidebar = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        sidebar.append(&scroller);

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
                move |b| {
                    if !b.is_active() {
                        return;
                    }
                    win.apply_color(rgba);
                    win.remember_chip(name);
                }
            ));

            self.imp().presets.borrow_mut().push(btn.clone());
            preset_row.append(&btn);
        }

        // Custom chip — a swatch of the custom colour with the eyedropper on
        // top, in the same group as the presets. It used to be an eyedropper
        // and nothing else, which made the one chip whose colour is not
        // implied by its name also the only one that never showed it.
        self.imp()
            .custom_rgba
            .set(self.imp().settings.custom_color().unwrap_or(DEFAULT_CUSTOM));

        let custom_swatch = gtk::DrawingArea::builder()
            .width_request(64).height_request(40)
            .build();
        custom_swatch.set_draw_func(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_, cr, w, h| {
                let rgba = win.imp().custom_rgba.get();
                rounded_rect(cr, 0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0, 12.0);
                cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.10);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            }
        ));

        let custom_badge = gtk::Image::builder()
            .icon_name("color-select-symbolic")
            .pixel_size(16)
            .css_classes(["custom-swatch-badge"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        let custom_stack = gtk::Overlay::builder().child(&custom_swatch).build();
        custom_stack.add_overlay(&custom_badge);

        let custom_inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6).margin_bottom(6)
            .margin_start(8).margin_end(8)
            .build();
        custom_inner.append(&custom_stack);
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

        self.imp().custom_btn.replace(Some(custom_btn.clone()));

        // Selecting the chip only re-applies the stored custom colour.
        custom_btn.connect_toggled(glib::clone!(
            #[weak(rename_to = win)] self,
            move |btn| {
                if !btn.is_active() {
                    return;
                }
                win.apply_color(win.imp().custom_rgba.get());
                win.remember_chip(CUSTOM_CHIP);
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
            #[weak] custom_swatch,
            #[strong] custom_dialog,
            move |_, _, _, _| {
                custom_btn.set_active(true);
                custom_dialog.choose_rgba(
                    Some(&win),
                    Some(&win.imp().custom_rgba.get()),
                    gio::Cancellable::NONE,
                    glib::clone!(
                        #[weak] win,
                        #[weak] custom_swatch,
                        move |res| {
                            // Err = the user cancelled; keep the previous
                            // custom colour rather than leaving the chip
                            // selected but showing something stale.
                            let Ok(rgba) = res else { return };
                            win.imp().custom_rgba.set(rgba);
                            custom_swatch.queue_draw();
                            win.apply_color(rgba);
                            // The chip itself was recorded by the
                            // set_active() above; only the colour is new.
                            let settings = &win.imp().settings;
                            settings.set_custom_color(rgba);
                            settings.save();
                        }
                    ),
                );
            }
        ));
        custom_btn.add_controller(custom_click);
        preset_row.append(&custom_btn);

        // Restore the chip from the last session, falling back to the first
        // preset. (That fallback used to search for BLACK despite the comment
        // and despite WHITE being PRESETS[0], so the app called "White Screen"
        // opened on black.)
        //
        // Activating the chip is enough to apply its colour: every chip's
        // `toggled` handler calls apply_color().
        let saved_chip = self.imp().settings.chip();
        let restored = match saved_chip.as_deref() {
            Some(CUSTOM_CHIP) => {
                custom_btn.set_active(true);
                true
            }
            Some(name) => {
                match PRESETS.iter().position(|p| p.name == name) {
                    Some(i) => {
                        self.imp().presets.borrow()[i].set_active(true);
                        true
                    }
                    // A chip that no longer exists, e.g. a settings file
                    // written by a version with a different preset list.
                    None => false,
                }
            }
            None => false,
        };
        if !restored {
            if let Some(btn) = self.imp().presets.borrow().first() {
                btn.set_active(true);
            }
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
        controls_card.append(&cycle_row);

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
        // Say so rather than leaving the user to discover it: the overlays
        // still work here, but they no longer sit above everything else and
        // Identify is switched off.
        if !crate::layer_shell_available() {
            toolbar_view.add_top_bar(
                &adw::Banner::builder()
                    .title("No wlr-layer-shell: overlays are fullscreen windows, and Identify is unavailable")
                    .revealed(true)
                    .build(),
            );
        }
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

        // Restore the monitor selection before the first reconcile, so the
        // rows are built already ticked instead of flickering into place.
        let saved_monitors = self.imp().settings.monitors();
        if !saved_monitors.is_empty() {
            self.imp().selected.replace(saved_monitors.into_iter().collect());
        }

        // Initial population.
        self.sync_monitors();

        // ── Teardown ──────────────────────────────────────────────────
        //
        // Closing the window is only one way out: Ctrl+Q, and anything else
        // that reaches g_application_quit(), never touches this handler. So
        // the work lives in shutdown(), hung off the application's own
        // `shutdown` signal, and this only asks the application to stop.
        //
        // Calling it here as well is not belt and braces for its own sake:
        // returning Proceed destroys this window immediately, so by the time
        // `shutdown` is emitted the weak reference there no longer upgrades.
        // shutdown() is idempotent, so running it twice costs nothing.
        if let Some(app) = self.application() {
            app.connect_shutdown(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_| win.shutdown()
            ));
        }

        self.connect_close_request(glib::clone!(
            #[weak(rename_to = win)] self,
            #[upgrade_or] glib::Propagation::Proceed,
            move |_| {
                win.shutdown();
                if let Some(app) = win.application() {
                    app.quit();
                }
                glib::Propagation::Proceed
            }
        ));
    }

    /// Give everything back before the process ends.
    ///
    /// Idempotent, and safe to call from any exit route.
    ///
    /// Overlays and badges are hidden, never destroyed. They are layer-shell
    /// windows and most have never been realized -- one pair is created per
    /// monitor at startup, but only shown on demand -- and
    /// gtk_window_destroy() on such a window emits a signal whose handler
    /// calls gdk_surface_get_display() on a surface that does not exist:
    ///   Gdk-CRITICAL gdk_surface_get_display:
    ///   assertion 'GDK_IS_SURFACE (surface)' failed
    /// followed by a segfault. (Nor show_on_monitor(None): that call
    /// *presents* the window, so an older version of this flashed every
    /// overlay full-screen on the way out.)
    pub fn shutdown(&self) {
        // The graveyard is included: those windows are already hidden, but
        // "hide everything" is the invariant this is here to guarantee, and
        // it should not depend on retirement having gone to plan.
        for ov in self.all_overlays() {
            ov.hide_overlay();
        }
        for l in self.all_labels() {
            l.set_visible(false);
        }

        // Stop the cycle timer and hand back the idle inhibitor explicitly.
        // Both would go with the process, but the inhibitor is state held on
        // the other side of a socket, and giving it back is cheaper than
        // trusting every session manager to notice a client that vanished
        // still holding one.
        self.stop_cycle_timer();
        self.release_idle();

        // Stop the gamma prober thread deterministically rather than leaving
        // it to process exit.
        #[cfg(feature = "gamma")]
        self.imp().gamma_listener.replace(None);
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

    /// Record which colour chip is selected, for the next launch.
    ///
    /// Does nothing while cycling: the timer drives the same chips the user
    /// clicks, so recording every step would overwrite the chip that was
    /// actually chosen -- and rewrite the file once per tick.
    fn remember_chip(&self, name: &str) {
        let imp = self.imp();
        if imp.cycling.get() {
            return;
        }
        imp.settings.set_chip(name);
        imp.settings.save();
    }

    /// Persist the ticked monitors.
    ///
    /// Sorted because the selection is a `HashSet`: without it the file would
    /// come out in a different order on every save, for no change at all.
    fn save_monitor_selection(&self) {
        let imp = self.imp();
        let mut keys: Vec<String> = imp.selected.borrow().iter().cloned().collect();
        keys.sort();
        imp.settings.set_monitors(&keys);
        imp.settings.save();
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

    /// Every overlay this window owns, retired ones included. Only teardown
    /// wants these: everything else acts on the monitors actually attached.
    fn all_overlays(&self) -> Vec<ScreenOverlay> {
        let imp = self.imp();
        let mut all = self.overlays();
        all.extend(imp.graveyard.borrow().iter().map(|e| e.overlay.clone()));
        all
    }

    fn all_labels(&self) -> Vec<monitor_label::MonitorLabel> {
        let imp = self.imp();
        let mut all = self.labels();
        all.extend(imp.graveyard.borrow().iter().map(|e| e.label.clone()));
        all
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
        //
        // The inhibitor cannot stay, though. Nothing is on any screen to
        // justify it, and on the session-manager route it would otherwise
        // keep the machine awake until the app quit.
        if targets.is_empty() {
            self.release_idle();
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

        // Keep the screen awake for as long as the colour is up: a lighting
        // or camera setup is a screen nobody touches for minutes, which is
        // exactly what the compositor blanks. Run once now and once more from
        // the main loop, because an overlay presented for the first time may
        // not have its surface yet and the Wayland inhibitor needs one.
        // engage() is idempotent, so the second call usually does nothing.
        self.engage_idle();
        glib::idle_add_local_once(glib::clone!(
            #[weak(rename_to = win)] self,
            move || win.engage_idle()
        ));
    }

    /// (Re-)arm idle inhibition against an overlay that is currently up.
    ///
    /// On Wayland GTK implements this with `zwp_idle_inhibit_manager_v1` on
    /// the surface of the window it is given, and falls back to a session
    /// manager or the inhibit portal elsewhere -- so naming an overlay covers
    /// wlroots compositors (which run no session manager) and GNOME/X11 alike.
    ///
    /// Idempotent, so it is safe to call after every hot-plug reconcile: the
    /// inhibitor only moves when the overlay holding it has gone.
    fn engage_idle(&self) {
        let imp = self.imp();
        let Some(app) = self.application() else { return };

        // An unrealized overlay has no surface for the compositor to track.
        // show_selected() queues a second attempt for exactly this case.
        let overlay = imp
            .entries
            .borrow()
            .iter()
            .find(|e| e.overlay.is_visible() && e.overlay.is_realized())
            .map(|e| e.overlay.clone());
        let Some(overlay) = overlay else {
            // Nothing visible to attach to. Not always transient: the monitor
            // holding the overlay may have just been unplugged, and an
            // inhibitor for a window nobody can see has to go.
            self.release_idle();
            return;
        };

        let unchanged = { imp.idle_window.borrow().as_ref() == Some(&overlay) };
        if unchanged && imp.idle_cookie.get() != 0 {
            return;
        }

        self.release_idle();
        let cookie = app.inhibit(
            Some(&overlay),
            gtk::ApplicationInhibitFlags::IDLE,
            Some(IDLE_REASON),
        );
        imp.idle_cookie.set(cookie);
        imp.idle_window.replace(Some(overlay));
    }

    /// Let the screen blank again. Safe to call when nothing is inhibited.
    fn release_idle(&self) {
        let imp = self.imp();
        imp.idle_window.replace(None);
        let cookie = imp.idle_cookie.replace(0);
        if cookie != 0 {
            if let Some(app) = self.application() {
                app.uninhibit(cookie);
            }
        }
    }

    /// Hide every overlay. Bound to the app-scoped `hide-overlays` action,
    /// so one ESC press on any screen clears all of them.
    pub fn hide_all_overlays(&self) {
        self.imp().overlays_shown.set(false);
        self.release_idle();
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
            l.present_badge();
        }
    }

    // ── Cycle mode ───────────────────────────────────────────────────────

    /// Start or stop stepping through the presets on a timer.
    pub fn set_cycling(&self, on: bool) {
        let imp = self.imp();
        if imp.cycling.get() == on {
            return;
        }

        if !on {
            // Clear the flag first so restoring the chip is recorded as the
            // user's choice rather than swallowed by remember_chip().
            imp.cycling.set(false);
            self.stop_cycle_timer();
            if let Some(btn) = imp.pre_cycle.replace(None) {
                btn.set_active(true);
            }
            return;
        }

        let presets = imp.presets.borrow().clone();
        if presets.is_empty() {
            return;
        }
        // Come back to whatever is showing now when the mode is switched off,
        // and start stepping from there so the first tick is a visible change.
        let active = presets.iter().position(|b| b.is_active());
        let pre = match active {
            Some(i) => Some(presets[i].clone()),
            None    => imp.custom_btn.borrow().clone().filter(|b| b.is_active()),
        };
        imp.pre_cycle.replace(pre);
        imp.cycle_index.set(active.unwrap_or(0));
        imp.cycling.set(true);
        self.start_cycle_timer();
    }

    fn start_cycle_timer(&self) {
        self.stop_cycle_timer();
        let id = glib::timeout_add_local(
            std::time::Duration::from_secs_f64(self.imp().cycle_secs.get()),
            glib::clone!(
                #[weak(rename_to = win)] self,
                #[upgrade_or] glib::ControlFlow::Break,
                move || {
                    win.cycle_step();
                    glib::ControlFlow::Continue
                }
            ),
        );
        self.imp().cycle_source.replace(Some(id));
    }

    /// Remove the cycle timer if one is running. Safe to call when none is.
    pub fn stop_cycle_timer(&self) {
        if let Some(id) = self.imp().cycle_source.replace(None) {
            id.remove();
        }
    }

    fn cycle_step(&self) {
        let presets = self.imp().presets.borrow().clone();
        if presets.is_empty() {
            return;
        }
        let next = (self.imp().cycle_index.get() + 1) % presets.len();
        self.imp().cycle_index.set(next);
        // Activating the chip rather than calling apply_color() directly
        // keeps the window showing which colour is currently on the screens.
        presets[next].set_active(true);
    }

    /// Change the seconds per step, restarting a running timer so the new
    /// interval takes effect immediately rather than after the current step.
    fn set_cycle_interval(&self, secs: f64) {
        let imp = self.imp();
        if imp.cycle_secs.get() == secs {
            return;
        }
        imp.cycle_secs.set(secs);
        imp.settings.set_cycle_interval(secs);
        imp.settings.save();
        if imp.cycling.get() {
            self.start_cycle_timer();
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

        let rekeyed_any = !rekeyed.is_empty();
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
            //
            // The restored selection gets one extra chance: a settings file
            // carried to a machine with different monitors names nothing that
            // is plugged in, and leaving every row unticked on startup would
            // look like the app had failed to find the screens.
            let none_attached = !next.iter().any(|e| sel.contains(&e.key));
            if sel.is_empty() || (self.imp().first_sync.get() && none_attached) {
                if let Some(first) = next.first() {
                    sel.insert(first.key.clone());
                }
            }
        }

        self.imp().entries.replace(next);
        self.imp().first_sync.set(false);

        // A key that changed shape (a connector name reported late, or a
        // duplicate suffix shifting when its twin was unplugged) has to reach
        // the settings file too, or the next launch restores the old one.
        if rekeyed_any {
            self.save_monitor_selection();
        }

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
                win.save_monitor_selection();
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::dedup_idents;

    fn keys(v: &[&str]) -> Vec<String> {
        dedup_idents(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn distinct_identities_are_left_alone() {
        assert_eq!(keys(&["DP-1", "DP-2", "HDMI-A-1"]), ["DP-1", "DP-2", "HDMI-A-1"]);
    }

    #[test]
    fn duplicates_are_numbered_from_the_second() {
        // Two identical panels reporting no connector name must not share a
        // key, or ticking one in the sidebar would tick both.
        assert_eq!(keys(&["Dell|U2415", "Dell|U2415"]), ["Dell|U2415", "Dell|U2415#2"]);
    }

    #[test]
    fn numbering_follows_list_order() {
        assert_eq!(keys(&["a", "b", "a", "b", "a"]), ["a", "b", "a#2", "b#2", "a#3"]);
    }

    #[test]
    fn an_identity_shaped_like_a_generated_key_still_gets_a_unique_one() {
        // The naive per-identity counter produced "a#2" twice here.
        assert_eq!(keys(&["a", "a", "a#2"]), ["a", "a#2", "a#2#2"]);
    }

    #[test]
    fn keys_are_always_unique() {
        for case in [
            &["a", "a", "a", "b", "b"][..],
            &["a#2", "a", "a"][..],
            &["", "", ""][..],
        ] {
            let k = keys(case);
            let uniq: HashSet<&String> = k.iter().collect();
            assert_eq!(uniq.len(), k.len(), "duplicate key for {case:?} -> {k:?}");
        }
    }
}
