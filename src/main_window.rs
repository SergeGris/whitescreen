use std::{cell::Cell, cell::RefCell, collections::HashSet};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};

use crate::color_surface;
use crate::color;
use crate::color_surface::Fill;
use crate::cycle_bar::CycleBar;
use crate::fill_bar::{FillBar, Mode as FillMode};
use crate::monitor_label;
use crate::monitor_list::{self, MonitorList};
use crate::palette::Palette;
use crate::screen_overlay::ScreenOverlay;
use crate::settings::Settings;
use crate::tile;

#[cfg(feature = "gamma")]
use crate::gamma::GammaListener;
use crate::i18n::{self, gettext};

// Minimum preview size; it expands to fill the pane.
const PREVIEW_W: i32 = 480;
const PREVIEW_H: i32 = 270; // 16:9

/// Shown by whatever is asked to keep the screen on, where it displays a
/// reason -- so it says something the user would recognise.
const IDLE_REASON: &str = "A color overlay is showing";

/// Brightness bounds, as a percentage of the chosen colour.
///
/// The floor is not zero: a slider that can turn the screen black is a slider
/// that can make the app look broken, and Black is a chip already.
const BRIGHTNESS_MIN: f64 = 5.0;
const BRIGHTNESS_MAX: f64 = 100.0;

/// Longest auto-hide delay offered, in minutes. Four hours is well past any
/// exerciser run; past that, leaving the screen lit is a deliberate choice.
const AUTO_HIDE_MAX: f64 = 240.0;

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
            .label(gettext("Press any key to exit the overlay"))
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
pub fn monitor_keys(mons: &[gdk::Monitor]) -> Vec<String> {
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
        /// Colour currently applied, so overlays created by a later hot-plug
        /// start out matching the ones already on screen.
        ///
        /// The colour as *chosen*, before brightness is applied: it is what
        /// the chips and the queue compare against, and scaling it here would
        /// stop a dimmed white from marking the White chip.
        pub color:    Cell<gdk::RGBA>,
        pub identify: Cell<bool>,

        /// Which monitor the Choose Color card is aimed at, by key; `None`
        /// means the shared colour, which is every monitor that has not been
        /// given one of its own.
        ///
        /// Not saved: it is where you are working, not what you chose.
        pub target:   RefCell<Option<String>>,
        /// Set while the card is being brought in line with the colour it
        /// edits; see `sync_card()`.
        pub syncing:  Cell<bool>,

        /// Brightness every colour is scaled by on its way to a screen, as a
        /// fraction. A panel used as a key light is usually too bright at
        /// full output, and reaching for the picker to dim one is both
        /// awkward and destructive -- it loses the colour you picked.
        pub brightness: Cell<f64>,

        /// Minutes an overlay stays up before taking itself down, 0 for
        /// never. The exerciser wants minutes of running and the room it is
        /// in may not have anyone in it.
        pub auto_hide:  Cell<u32>,
        pub auto_hide_source: RefCell<Option<glib::SourceId>>,

        /// True while show_selected() has overlays up, so that a monitor
        /// plugged in mid-session is covered too instead of staying the
        /// one conspicuously bright screen in the room.
        pub overlays_shown: Cell<bool>,
        /// Set while a sync is queued on the main loop; see schedule_sync().
        pub sync_queued: Cell<bool>,

        /// Saved UI state, read once at startup and written back as it
        /// changes.
        pub settings:    Settings,
        /// Set while a write is queued on the main loop; see queue_save().
        pub save_queued: Cell<bool>,

        /// True when this window was built to serve a command line and never
        /// shown. It still does all the work -- it owns the overlays and the
        /// hot-plug reconcile -- it just stays out of the way.
        pub headless: Cell<bool>,

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

        /// The sidebar, which owns the monitor selection.
        pub monitors:  RefCell<Option<MonitorList>>,
        pub show_btn:  RefCell<Option<gtk::Button>>,
        /// The Identify toggle, so that `--identify` can drive it rather than
        /// setting the state behind its back.
        pub ident_btn: RefCell<Option<gtk::ToggleButton>>,
        pub preview:   RefCell<Option<color_surface::ColorSurface>>,

        /// The colour chips.
        pub palette: RefCell<Option<Palette>>,
        /// The brightness slider and the auto-hide delay, kept so the command
        /// line and the settings file can drive them.
        pub brightness_scale: RefCell<Option<gtk::Scale>>,
        pub auto_hide_spin:   RefCell<Option<gtk::SpinButton>>,
        /// "Applies to", and the keys behind its rows -- `None` first, then
        /// one per attached monitor, in sidebar order.
        pub target_drop: RefCell<Option<gtk::DropDown>>,
        /// The label and the dropdown together, so that one monitor hides
        /// both and never leaves an "Applies to" with nothing after it.
        pub target_aim:  RefCell<Option<gtk::Box>>,
        pub target_keys: RefCell<Vec<Option<String>>>,
        /// Says which monitors are keeping a colour of their own, so that
        /// "all monitors" never silently means "all but that one".
        pub scope_hint:  RefCell<Option<gtk::Label>>,

        /// Cycle mode: the switch, the interval and the queue it plays.
        pub cycle: RefCell<Option<CycleBar>>,
        /// Region and exerciser modes: what covers a screen, as opposed to
        /// which colour it is.
        pub fill_bar: RefCell<Option<FillBar>>,
        /// Colour showing when cycling started, put back when it stops, so
        /// the mode leaves the colour it found rather than whichever step the
        /// timer happened to land on.
        pub pre_cycle_color: Cell<gdk::RGBA>,

        /// Cookie from `gtk_application_inhibit()`; 0 means "not inhibiting".
        pub idle_cookie: Cell<u32>,
        /// Overlay the inhibitor is attached to. Which window it names
        /// matters: the compositor only honours an inhibitor while that
        /// window is actually on screen, so it has to be an overlay and not
        /// the main window, which is behind them (or on another workspace).
        pub idle_window: RefCell<Option<ScreenOverlay>>,

        #[cfg(feature = "gamma")]
        pub gamma_listener: RefCell<Option<GammaListener>>,
        /// Kept so the listener can be started and stopped after build_ui
        /// has returned; see `MainWindow::sync_gamma_listener`.
        #[cfg(feature = "gamma")]
        pub gamma_sender: RefCell<Option<async_channel::Sender<bool>>>,

    }

    // gdk::RGBA has no Default impl, so this cannot be derived.
    impl Default for MainWindow {
        fn default() -> Self {
            Self {
                entries:  RefCell::new(Vec::new()),
                color:    Cell::new(gdk::RGBA::WHITE),
                identify: Cell::new(false),
                target:   RefCell::new(None),
                syncing:  Cell::new(false),
                brightness: Cell::new(1.0),
                auto_hide:  Cell::new(0),
                auto_hide_source: RefCell::new(None),
                overlays_shown: Cell::new(false),
                sync_queued: Cell::new(false),
                settings:    Settings::load(),
                save_queued: Cell::new(false),
                headless:    Cell::new(false),
                graveyard: RefCell::new(Vec::new()),
                monitors: RefCell::new(None),
                show_btn: RefCell::new(None),
                ident_btn: RefCell::new(None),
                preview:  RefCell::new(None),
                palette: RefCell::new(None),
                brightness_scale: RefCell::new(None),
                auto_hide_spin:   RefCell::new(None),
                target_drop: RefCell::new(None),
                target_aim:  RefCell::new(None),
                target_keys: RefCell::new(Vec::new()),
                scope_hint:  RefCell::new(None),
                cycle: RefCell::new(None),
                fill_bar: RefCell::new(None),
                pre_cycle_color: Cell::new(gdk::RGBA::WHITE),
                idle_cookie: Cell::new(0),
                idle_window: RefCell::new(None),
                #[cfg(feature = "gamma")]
                gamma_listener: RefCell::new(None),
                #[cfg(feature = "gamma")]
                gamma_sender: RefCell::new(None),

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
        // After the widgets exist and before the window is seen, so the user
        // never watches it change its mind about what was saved.
        win.restore_settings();
        win
    }

    // ── Saved state ──────────────────────────────────────────────────────

    /// Put back what the last session left.
    ///
    /// Order matters twice over: the custom chip's colour has to be in place
    /// before the colour itself, or `Palette::select()` cannot recognise a
    /// restored custom colour and leaves every chip unmarked; and the fill
    /// mode has to come after the side and the second colour, since switching
    /// the mode on is what publishes them.
    fn restore_settings(&self) {
        let s = &self.imp().settings;

        if let (Some(palette), Some(rgba)) = (self.palette(), s.custom_color()) {
            palette.set_custom_color(rgba);
        }
        if let Some(rgba) = s.color() {
            self.set_color(rgba);
        }
        if let Some(fraction) = s.brightness() {
            self.set_brightness(fraction);
        }
        if let Some(minutes) = s.auto_hide() {
            self.set_auto_hide(minutes);
        }

        if let Some(cycle) = self.cycle() {
            cycle.set_queue_colors(&s.queue());
            if let Some(secs) = s.cycle_interval() {
                cycle.set_interval(secs);
            }
        }

        if let Some(bar) = self.imp().fill_bar.borrow().as_ref() {
            bar.set_second_color(s.second_color());
            if let Some(side) = s.region_side().as_deref().and_then(color_surface::Side::from_key) {
                bar.set_side(side);
            }
            // An unknown name -- a file from a newer version, or a typo in a
            // hand-edited one -- leaves the mode alone rather than resetting
            // it to Flat without saying so.
            if let Some(mode) = s.pattern().as_deref().and_then(FillMode::from_key) {
                bar.set_mode(mode);
            }
        }

        if let Some(list) = self.monitor_list() {
            list.set_colors(&s.monitor_colors().into_iter().collect::<Vec<_>>());
            list.set_per_monitor(s.per_monitor_colors());
            let saved = s.monitors();
            // An empty list means "nothing saved": honouring it would untick
            // the first monitor that MonitorList ticks for a first run.
            if !saved.is_empty() {
                list.select_only(saved);
            }
        }
    }

    /// Queue a write of everything to the settings file.
    ///
    /// Coalesced onto the main loop, so that one user action -- which may
    /// move several controls at once, each announcing itself -- costs one
    /// write rather than one per control.
    fn queue_save(&self) {
        if self.imp().save_queued.replace(true) {
            return;
        }
        glib::idle_add_local_once(glib::clone!(
            #[weak(rename_to = win)] self,
            move || {
                win.imp().save_queued.set(false);
                win.save_settings();
            }
        ));
    }

    /// Write the current state out.
    fn save_settings(&self) {
        let imp = self.imp();
        let s = &imp.settings;

        // The colour the *user* chose. While the queue is playing, the colour
        // on screen belongs to the timer and will be a different one a second
        // from now; what deserves saving is the one it took over from.
        let playing = self.cycle().is_some_and(|c| c.is_playing());
        s.set_color(if playing { imp.pre_cycle_color.get() } else { imp.color.get() });
        s.set_brightness(imp.brightness.get());
        s.set_auto_hide(imp.auto_hide.get());

        if let Some(palette) = self.palette() {
            s.set_custom_color(palette.custom_color());
        }
        if let Some(cycle) = self.cycle() {
            s.set_queue(&cycle.queue_colors());
            s.set_cycle_interval(cycle.interval());
        }
        if let Some(bar) = imp.fill_bar.borrow().as_ref() {
            s.set_pattern(bar.mode().as_key());
            s.set_region_side(bar.side().as_key());
            s.set_second_color(bar.second_color());
        }
        if let Some(list) = self.monitor_list() {
            s.set_monitors(&list.selected());
            s.set_per_monitor_colors(list.per_monitor());
            s.set_monitor_colors(&list.colors());
        }

        s.save();
    }

    fn build_ui(&self) {
        // ── Single consolidated CSS provider ──────────────────────────
        // Only the rules for widgets this window builds itself. The chips
        // (palette.rs), the action tiles (tile.rs) and the monitor rows
        // (monitor_list.rs) each register their own, once per display -- a
        // second copy here would be a second place to edit and a silent
        // disagreement the first time only one of them was.
        let css = gtk::CssProvider::new();
        css.load_from_string(r#"
/* ── Cards ──────────────────────────────────────────────────────────── */

.controls-card {
    background: @card_bg_color;
    border-radius: 16px;
    padding: 16px 20px;
}
.section-title { font-weight: 700; }

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
                .title(gettext("White Screen"))
                .subtitle(gettext("Display color overlay"))
                .build(),
        ));
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text(gettext("Menu"))
            .build();
        let menu = gio::Menu::new();
        menu.append(Some("About White Screen"), Some("win.about"));
        menu_btn.update_property(&[gtk::accessible::Property::Label(&gettext("Main menu"))]);
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
                    .comments(gettext("Fill any monitor with a solid color."))
                    .build()
                    .present(Some(&win));
            }
        ));
        self.add_action(&about);

        // App-scoped so ScreenOverlay's key handler can reach them without
        // holding a reference back to this window.
        if let Some(app) = self.application() {
            let hide = gio::SimpleAction::new("hide-overlays", None);
            hide.connect_activate(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_, _| win.hide_all_overlays()
            ));
            app.add_action(&hide);

            // Stepping and pausing from the overlay. The whole argument for
            // the cycle is that a dead-pixel check should not mean looking
            // away from the panel -- and the overlay covers the window these
            // controls live in, so without these the only way to take the
            // next colour is to look away and click.
            for (name, forward) in [("cycle-next", true), ("cycle-prev", false)] {
                let step = gio::SimpleAction::new(name, None);
                step.connect_activate(glib::clone!(
                    #[weak(rename_to = win)] self,
                    move |_, _| if let Some(cycle) = win.cycle() {
                        cycle.step(forward);
                    }
                ));
                app.add_action(&step);
            }

            let pause = gio::SimpleAction::new("cycle-pause", None);
            pause.connect_activate(glib::clone!(
                #[weak(rename_to = win)] self,
                move |_, _| if let Some(cycle) = win.cycle() {
                    cycle.toggle_pause();
                }
            ));
            app.add_action(&pause);
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

        let show_btn = gtk::Button::builder()
            .child(&tile::content("video-display-symbolic", &gettext("Show on selected"), &gettext("Fill selected monitors")))

            .css_classes(["suggested-action", "action-tile"])
            .sensitive(false) // sync_monitors() enables it once a monitor is selected
            .build();
        show_btn.connect_clicked(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_| win.show_selected()
        ));
        self.imp().show_btn.replace(Some(show_btn.clone()));

        let ident_btn = gtk::ToggleButton::builder()
            .tooltip_text(gettext("Show monitor connector labels on each screen"))
            .css_classes(["action-tile"])
            .child(&tile::content("dialog-information-symbolic", &gettext("Identify"), &gettext("Identify each monitor")))
            .build();
        ident_btn.connect_toggled(glib::clone!(
            #[weak(rename_to = win)] self,
            move |btn| win.set_identify(btn.is_active())
        ));
        self.imp().ident_btn.replace(Some(ident_btn.clone()));
        if !crate::layer_shell_available() {
            // The badge is a window anchored to a monitor corner and made
            // click-through, which is exactly what layer shell is for and
            // what an ordinary toplevel cannot be asked to do.
            ident_btn.set_sensitive(false);
            ident_btn.set_tooltip_text(Some(&gettext(
                "Identify needs wlr-layer-shell, which this compositor does not support",
            )));
        }

        let hide_btn = gtk::Button::builder()
            .child(&tile::content("view-conceal-symbolic", &gettext("Hide ALL"), &gettext("Hide all overlays")))
            .css_classes(["action-tile"])
            .tooltip_text(gettext("Hide the overlay on all monitors (or press ESC on the overlay)"))
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

        // ── Fill modes ────────────────────────────────────────────────
        // Off by default: covering a screen with a colour is what the app is
        // for, and neither of these should stand in front of that.
        let fill_bar = FillBar::new();
        self.imp().fill_bar.replace(Some(fill_bar.clone()));
        fill_bar.connect_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_| {
                win.apply_fill();
                win.queue_save();
            }
        ));

        // ── Cycle mode ────────────────────────────────────────────────
        // The switch, the interval and the queue it plays are one component;
        // see cycle_bar.rs. This window owns only the two ends of it: telling
        // it when there is an overlay to play onto, and handing the colour
        // over to it (and taking it back) when it does.
        let cycle = CycleBar::new();
        self.imp().cycle.replace(Some(cycle.clone()));

        // The sequence playing, or a step taken by hand from an overlay:
        // always the shared colour. sync_card() marks the matching chip.
        cycle.connect_color_chosen(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_, rgba| win.apply_color(rgba)
        ));
        // A swatch clicked in the strip is a colour chosen in the card, and
        // goes wherever the card is aimed.
        cycle.connect_color_activated(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_, rgba| win.choose_color(rgba)
        ));
        cycle.connect_playing_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_, playing| win.on_cycling_changed(playing)
        ));
        cycle.connect_state_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_| win.queue_save()
        ));

        // ── Monitor list ──────────────────────────────────────────────
        // It owns the selection; this window only asks which keys are ticked
        // when it comes to put a colour on a screen.
        let monitors = MonitorList::new();
        self.imp().monitors.replace(Some(monitors.clone()));
        monitors.connect_colors_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_| {
                win.refresh_colors();
                win.queue_save();
            }
        ));
        // Clicking a row's swatch aims the card at that monitor: the sidebar
        // and the card are two views of one choice, not two controls.
        monitors.connect_target_requested(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_, key| win.set_target(Some(key))
        ));
        monitors.connect_selection_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |list| {
                if let Some(btn) = win.imp().show_btn.borrow().as_ref() {
                    btn.set_sensitive(list.any_attached_selected());
                }
                win.queue_save();
            }
        ));

        let scroller = gtk::ScrolledWindow::builder()
            .min_content_width(300)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&monitors)
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

        // The preview is the one colour surface worth announcing, so it
        // opts back out of the presentation role the others take, and its
        // label follows the colour (see apply_color).
        preview.set_accessible_role(gtk::AccessibleRole::Img);

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
            .tooltip_text(gettext("Show on selected monitors"))
            .build();
        expand_btn.update_property(&[gtk::accessible::Property::Label(
            "Show on the selected monitors",
        )]);
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
        let palette = Palette::new();
        self.imp().palette.replace(Some(palette.clone()));
        palette.connect_color_chosen(glib::clone!(
            #[weak(rename_to = win)] self,
            move |_, rgba| win.choose_color(rgba)
        ));
        palette.select_default();

        // ── Gamma status indicator (feature-gated) ────────────────────
        #[cfg(feature = "gamma")]
        let gamma_icon = gtk::Image::builder()
            .icon_name("weather-clear-symbolic") // sun = normal rendering
            .pixel_size(16)
            .tooltip_text(gettext("No color filter"))
            .build();
        #[cfg(feature = "gamma")]
        let gamma_label = gtk::Label::new(Some(&gettext("Gamma control is inactive")));

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
            //
            // Nothing is started here: the thread only exists to drive an
            // indicator in this window, so it waits until there is a window
            // on screen to drive. See sync_gamma_listener().
            if std::env::var_os("WHITESCREEN_NO_GAMMA").is_none() {
                self.imp().gamma_sender.replace(Some(sender));
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
        controls_card.append(&self.build_target_row());
        controls_card.append(&palette);

        // Hidden until it has something to say; see update_scope_hint().
        let scope_hint = gtk::Label::builder()
            .css_classes(["dim-label"])
            .halign(gtk::Align::Center)
            .visible(false)
            .build();
        self.imp().scope_hint.replace(Some(scope_hint.clone()));
        controls_card.append(&scope_hint);
        controls_card.append(&self.build_output_row());
        controls_card.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        controls_card.append(&action_bar);
        controls_card.append(&fill_bar);
        controls_card.append(&cycle);

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
                    .title(gettext("Monitors"))
                    .child(&sidebar)
                    .build()
            )
            .content(
                &adw::NavigationPage::builder()
                    .title(gettext("Preview"))
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
                    .title(gettext("No wlr-layer-shell: overlays are fullscreen windows, and Identify is unavailable"))
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

    /// The card's heading, and what it is aimed at.
    ///
    /// Without this the card was a set of colour chips floating beside a list
    /// of monitors, with nothing anywhere saying which of them a chip would
    /// change -- and no way to aim it at one except hunting for the swatch in
    /// the sidebar.
    fn build_target_row(&self) -> gtk::Box {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        row.append(
            &gtk::Label::builder()
                .label(gettext("Choose Color"))
                .css_classes(["section-title"])
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build(),
        );
        // The label and the dropdown travel together; see `target_aim`.
        //
        // Hidden until rebuild_target_row() has seen the monitors: visible
        // by default would mean an empty "Applies to" on screen for as long
        // as it took to be corrected.
        let aim = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .visible(false)
            .build();
        aim.append(
            &gtk::Label::builder()
                .label(gettext("Applies to"))
                .css_classes(["dim-label"])
                .valign(gtk::Align::Center)
                .build(),
        );

        let drop = gtk::DropDown::from_strings(&[]);
        drop.set_tooltip_text(Some(&gettext(
            "Which monitor the colors below change. Pick one to give it a color of its own, leaving the others alone.",
        )));
        drop.update_property(&[gtk::accessible::Property::Label(
            "Which monitor the colors apply to",
        )]);
        drop.connect_selected_notify(glib::clone!(
            #[weak(rename_to = win)] self,
            move |drop| {
                // Moved by the window rather than by the user; see
                // set_target() and rebuild_target_row().
                if win.imp().syncing.get() {
                    return;
                }
                let key = win
                    .imp()
                    .target_keys
                    .borrow()
                    .get(drop.selected() as usize)
                    .cloned()
                    .flatten();
                win.set_target(key);
            }
        ));
        self.imp().target_drop.replace(Some(drop.clone()));
        aim.append(&drop);
        self.imp().target_aim.replace(Some(aim.clone()));
        row.append(&aim);

        row
    }

    /// Rebuild "Applies to" from the monitors now attached, keeping the aim
    /// where it was if that monitor is still there.
    fn rebuild_target_row(&self) {
        let imp = self.imp();
        let Some(drop) = imp.target_drop.borrow().clone() else { return };

        let keys: Vec<Option<String>> = std::iter::once(None)
            .chain(imp.entries.borrow().iter().map(|e| Some(e.key.clone())))
            .collect();
        let labels: Vec<String> = keys
            .iter()
            .map(|k| k.clone().unwrap_or_else(|| gettext("All monitors")))
            .collect();

        // A monitor that has been unplugged cannot stay the target: the card
        // would be editing a screen that is not there.
        let wanted = self.target().filter(|k| keys.iter().any(|c| c.as_ref() == Some(k)));
        if wanted != self.target() {
            imp.target.replace(wanted.clone());
            // The row that was ringed has gone; nothing should be ringed in
            // its place.
            if let Some(list) = self.monitor_list() {
                list.set_target(wanted.as_deref());
            }
        }
        let selected = keys.iter().position(|k| *k == wanted).unwrap_or(0) as u32;

        imp.target_keys.replace(keys);
        // Setting the model moves the selection, which would report an aim
        // nobody took.
        imp.syncing.set(true);
        drop.set_model(Some(&gtk::StringList::new(
            &labels.iter().map(String::as_str).collect::<Vec<_>>(),
        )));
        drop.set_selected(selected);
        imp.syncing.set(false);

        // One monitor means nothing to choose between, and a row that can
        // only say "All monitors" is a row in the way.
        let several = imp.entries.borrow().len() > 1;
        if let Some(aim) = imp.target_aim.borrow().as_ref() {
            aim.set_visible(several);
        }
        self.sync_card();
    }

    /// Spell out what "All monitors" leaves out.
    fn update_scope_hint(&self) {
        let imp = self.imp();
        let Some(hint) = imp.scope_hint.borrow().clone() else { return };
        let Some(list) = self.monitor_list() else { return };

        // Only worth saying when the card is aimed at everything: aimed at
        // one monitor, the dropdown has already said so.
        let own: Vec<String> = if self.target().is_some() || !list.per_monitor() {
            Vec::new()
        } else {
            imp.entries
                .borrow()
                .iter()
                .filter(|e| list.color_for(&e.key).is_some())
                .map(|e| e.key.clone())
                .collect()
        };

        hint.set_visible(!own.is_empty());
        if !own.is_empty() {
            let template = i18n::ngettext(
                "{0} keeps a color of its own",
                "{0} keep colors of their own",
                own.len() as u32,
            );
            hint.set_text(&i18n::format(&template, &[&own.join(", ")]));
        }
    }

    /// Brightness, and how long the colour stays up.
    ///
    /// Both are about the light coming off the panel rather than its colour,
    /// which is why they share a row below the chips instead of sitting with
    /// the fill modes.
    fn build_output_row(&self) -> gtk::Box {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .halign(gtk::Align::Center)
            .build();

        row.append(
            &gtk::Label::builder()
                .label(gettext("Brightness"))
                .css_classes(["dim-label"])
                .build(),
        );

        let scale = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            BRIGHTNESS_MIN,
            BRIGHTNESS_MAX,
            1.0,
        );
        scale.set_value(BRIGHTNESS_MAX);
        scale.set_width_request(170);
        scale.set_draw_value(true);
        scale.set_value_pos(gtk::PositionType::Right);
        scale.set_tooltip_text(Some(&gettext(
            "Scale the chosen color on its way to the screens, for a panel being used as a light",
        )));
        scale.update_property(&[gtk::accessible::Property::Label(&gettext("Brightness, percent"))]);
        // The default value label is "85" with no unit, which beside a colour
        // reads as anything at all.
        scale.set_format_value_func(|_, value| i18n::format(&gettext("{0}%"), &[&format!("{value:.0}")]));
        scale.connect_value_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |s| win.apply_brightness(s.value() / 100.0)
        ));
        self.imp().brightness_scale.replace(Some(scale.clone()));
        row.append(&scale);

        row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        row.append(
            &gtk::Label::builder()
                .label(gettext("Hide after"))
                .css_classes(["dim-label"])
                .build(),
        );

        let spin = gtk::SpinButton::with_range(0.0, AUTO_HIDE_MAX, 1.0);
        spin.set_digits(0);
        spin.set_value(0.0);
        spin.set_tooltip_text(Some(&gettext(
            "Take the overlays down after this many minutes. Zero leaves them up until something else takes them down.",
        )));
        spin.update_property(&[gtk::accessible::Property::Label(
            "Hide the overlays after this many minutes, zero for never",
        )]);
        spin.connect_value_changed(glib::clone!(
            #[weak(rename_to = win)] self,
            move |s| win.apply_auto_hide(s.value() as u32)
        ));
        self.imp().auto_hide_spin.replace(Some(spin.clone()));
        row.append(&spin);
        row.append(&gtk::Label::builder().label("min").css_classes(["dim-label"]).build());

        row
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
        if let Some(cycle) = self.cycle() {
            cycle.stop_timer();
        }
        self.cancel_auto_hide();
        self.release_idle();

        // Stop the gamma prober thread deterministically rather than leaving
        // it to process exit.
        #[cfg(feature = "gamma")]
        self.imp().gamma_listener.replace(None);

        // Written directly rather than queued: the main loop is on its way
        // out, and an idle callback would never run.
        self.save_settings();
    }

    // ── Driving the window from outside ──────────────────────────────────

    /// Put a colour on screen as though a chip had been clicked.
    ///
    /// Marking the matching chip is `sync_card()`'s job, which `apply_color`
    /// reaches: doing it here as well would briefly mark the shared colour
    /// on a card that is aimed at a single monitor.
    pub fn set_color(&self, rgba: gdk::RGBA) {
        self.apply_color(rgba);
    }

    /// Switch per-monitor colours off, or back on, as the sidebar's tick box
    /// does. Off keeps the colours: they are there again the moment it goes
    /// back on, which is the promise the tick box makes too.
    pub fn set_per_monitor_colors(&self, on: bool) {
        if let Some(list) = self.monitor_list() {
            list.set_per_monitor(on);
        }
    }

    /// Give named monitors colours of their own, for `--monitor-color`, and
    /// report the patterns that matched nothing.
    ///
    /// Selecting a monitor and colouring it are deliberately separate, here
    /// as in the sidebar: setting a row's colour does not tick it either.
    pub fn set_monitor_colors(&self, colors: &[(String, gdk::RGBA)]) -> Vec<String> {
        let Some(list) = self.monitor_list() else { return Vec::new() };
        colors
            .iter()
            .filter(|(pattern, rgba)| !list.set_color_matching(pattern, *rgba))
            .map(|(pattern, _)| pattern.clone())
            .collect()
    }

    /// Run without showing the control window, for `whitescreen --show` and
    /// friends.
    ///
    /// The window is still built: it owns the overlays, the monitor list and
    /// the hot-plug reconcile, and none of that works without it. What
    /// changes is that it is never presented, and that hiding the overlays
    /// ends the process rather than revealing a window nobody asked for.
    pub fn set_headless(&self, headless: bool) {
        self.imp().headless.set(headless);
        #[cfg(feature = "gamma")]
        self.sync_gamma_listener();
    }

    /// Start the gamma prober while this window is on screen, and stop it
    /// while it is not.
    ///
    /// The prober polls the compositor once or twice a second for the life of
    /// the process, and the only thing it feeds is the indicator in the status
    /// bar. A scripted `whitescreen --color red --all --show` never shows that
    /// bar, so it has no reason to be probing at all.
    #[cfg(feature = "gamma")]
    fn sync_gamma_listener(&self) {
        let imp = self.imp();
        // WHITESCREEN_NO_GAMMA drops the sender in build_ui, so a missing one
        // means "never start this".
        let Some(sender) = imp.gamma_sender.borrow().clone() else { return };

        if imp.headless.get() {
            // Dropping the listener joins its thread; see GammaListener::drop.
            imp.gamma_listener.replace(None);
            return;
        }
        if imp.gamma_listener.borrow().is_some() {
            return;
        }
        imp.gamma_listener.replace(Some(GammaListener::new(move |e| {
            let _ = sender.send_blocking(e);
        })));
    }

    /// Whether a colour is on the screens right now.
    pub fn overlays_visible(&self) -> bool {
        self.imp().overlays_shown.get()
    }

    /// Tick the monitors named by these shell-style patterns, and report the
    /// patterns that matched nothing.
    pub fn select_monitors(&self, patterns: &[String]) -> Vec<String> {
        match self.monitor_list() {
            Some(list) => list.select_matching(patterns),
            None       => Vec::new(),
        }
    }

    /// Names of the monitors currently attached.
    pub fn attached_monitors(&self) -> Vec<String> {
        self.monitor_list().map(|l| l.attached()).unwrap_or_default()
    }

    pub fn select_all_monitors(&self) {
        if let Some(list) = self.monitor_list() {
            list.select_all_attached();
        }
    }

    pub fn set_cycle_armed(&self, armed: bool) {
        if let Some(cycle) = self.cycle() {
            cycle.set_armed(armed);
        }
    }

    pub fn set_cycle_interval(&self, secs: f64) {
        if let Some(cycle) = self.cycle() {
            cycle.set_interval(secs);
        }
    }

    /// Replace the cycle queue, for `--queue`.
    pub fn set_queue(&self, colors: &[gdk::RGBA]) {
        if let Some(cycle) = self.cycle() {
            cycle.set_queue_colors(colors);
        }
    }

    /// Which pattern covers the screens, for `--pattern` and `--exerciser`.
    pub fn set_pattern(&self, mode: FillMode) {
        if let Some(bar) = self.imp().fill_bar.borrow().as_ref() {
            bar.set_mode(mode);
        }
    }

    /// Cover half the screen, for `--region`. Implies the region mode: a side
    /// on its own would be a setting with nothing to apply it to.
    pub fn set_region(&self, side: color_surface::Side) {
        if let Some(bar) = self.imp().fill_bar.borrow().as_ref() {
            bar.set_side(side);
            bar.set_mode(FillMode::Region);
        }
    }

    /// Fill the other half as well, for `--second-color`.
    pub fn set_second_color(&self, rgba: Option<gdk::RGBA>) {
        if let Some(bar) = self.imp().fill_bar.borrow().as_ref() {
            bar.set_second_color(rgba);
        }
    }

    /// Brightness as a fraction, for the settings file.
    pub fn brightness(&self) -> f64 {
        self.imp().brightness.get()
    }

    /// Set the brightness from outside. Goes through the slider so it shows
    /// the value rather than contradicting it, and is clamped to the range
    /// the slider offers so a command line cannot ask for a black screen.
    pub fn set_brightness(&self, fraction: f64) {
        let percent = (fraction * 100.0).clamp(BRIGHTNESS_MIN, BRIGHTNESS_MAX);
        match self.imp().brightness_scale.borrow().as_ref() {
            Some(scale) => scale.set_value(percent),
            // No slider yet: still honour the request.
            None => self.apply_brightness(percent / 100.0),
        }
    }

    fn apply_brightness(&self, fraction: f64) {
        let imp = self.imp();
        if imp.brightness.replace(fraction) == fraction {
            return;
        }
        // The chosen colours have not changed; only what reaches a screen
        // has, and everything on its way there goes through scaled().
        self.refresh_colors();
        self.queue_save();
    }

    /// The chosen colour as it should actually reach a panel.
    ///
    /// Scaling the three channels together moves the level without moving the
    /// hue, which is what a brightness control has to do: dimming by blending
    /// towards black is the same operation, and this way round it needs no
    /// second colour.
    fn scaled(&self, rgba: gdk::RGBA) -> gdk::RGBA {
        let k = self.imp().brightness.get() as f32;
        if k >= 1.0 {
            return rgba;
        }
        gdk::RGBA::new(rgba.red() * k, rgba.green() * k, rgba.blue() * k, rgba.alpha())
    }

    /// Minutes before the overlays take themselves down, 0 for never.
    pub fn auto_hide(&self) -> u32 {
        self.imp().auto_hide.get()
    }

    /// Set the auto-hide delay from outside, through the spinner so it shows
    /// the value.
    pub fn set_auto_hide(&self, minutes: u32) {
        match self.imp().auto_hide_spin.borrow().as_ref() {
            Some(spin) => spin.set_value(f64::from(minutes).min(AUTO_HIDE_MAX)),
            None => self.apply_auto_hide(minutes),
        }
    }

    fn apply_auto_hide(&self, minutes: u32) {
        let imp = self.imp();
        if imp.auto_hide.replace(minutes) == minutes {
            return;
        }
        // Changing the delay restarts the countdown, rather than leaving a
        // timer running against the number that was there before.
        self.cancel_auto_hide();
        if imp.overlays_shown.get() {
            self.arm_auto_hide();
        }
        self.queue_save();
    }

    /// Start the countdown, if there is one to start and it is not already
    /// running.
    ///
    /// Idempotent on purpose: `show_selected()` runs again on every hot-plug,
    /// and re-arming there would hand a plugged-in monitor the power to reset
    /// a countdown that is most of the way through.
    fn arm_auto_hide(&self) {
        let imp = self.imp();
        let minutes = imp.auto_hide.get();
        if minutes == 0 || imp.auto_hide_source.borrow().is_some() {
            return;
        }

        let id = glib::timeout_add_seconds_local_once(
            minutes.saturating_mul(60),
            glib::clone!(
                #[weak(rename_to = win)] self,
                move || {
                    // The source has fired, so it must not be removed again.
                    win.imp().auto_hide_source.replace(None);
                    win.hide_all_overlays();
                }
            ),
        );
        imp.auto_hide_source.replace(Some(id));
    }

    fn cancel_auto_hide(&self) {
        if let Some(id) = self.imp().auto_hide_source.replace(None) {
            id.remove();
        }
    }

    // ── Colour ───────────────────────────────────────────────────────────
    //
    // Three jobs, which used to be one method. Keeping them apart is what
    // lets the card be aimed at a single monitor: writing a per-monitor
    // colour makes the sidebar announce a change, and with one method that
    // announcement came back in as "the user chose a colour" and would have
    // been written straight back out again.
    //
    //   choose_color()    the card was used; where it lands depends on what
    //                     the card is aimed at
    //   apply_color()     the shared colour -- what every monitor without one
    //                     of its own shows
    //   refresh_colors()  push whatever is now true out to the screens
    //   sync_card()       make the card show what it is editing

    /// Which monitor the Choose Color card is aimed at; `None` is all of
    /// them, through the shared colour.
    pub fn target(&self) -> Option<String> {
        self.imp().target.borrow().clone()
    }

    /// Aim the card.
    ///
    /// Changes no colour -- it only decides where the next one goes -- but
    /// both views of the aim have to follow it, whichever of them moved it:
    /// the dropdown in the card and the ring on the monitor's row.
    pub fn set_target(&self, key: Option<String>) {
        let imp = self.imp();
        if *imp.target.borrow() == key {
            return;
        }
        imp.target.replace(key.clone());

        // Driving the widgets makes them report a change; the guard is what
        // stops that coming back round as a fresh aim.
        let was = imp.syncing.replace(true);
        if let Some(list) = self.monitor_list() {
            list.set_target(key.as_deref());
        }
        if let Some(drop) = imp.target_drop.borrow().as_ref()
            && let Some(index) = imp.target_keys.borrow().iter().position(|k| *k == key)
        {
            drop.set_selected(index as u32);
        }
        imp.syncing.set(was);

        self.sync_card();
    }

    /// The colour the card is editing: the aimed-at monitor's own colour if
    /// it has one, the shared colour otherwise.
    fn card_color(&self) -> gdk::RGBA {
        let shared = self.imp().color.get();
        match (self.target(), self.monitor_list()) {
            (Some(key), Some(list)) => list.color_for(&key).unwrap_or(shared),
            _ => shared,
        }
    }

    /// A colour was chosen in the card -- a chip, a temperature, the picker,
    /// a queue swatch.
    fn choose_color(&self, rgba: gdk::RGBA) {
        let Some(key) = self.target() else {
            self.apply_color(rgba);
            return;
        };
        // Aimed at one monitor: give that monitor the colour and leave the
        // shared one alone. set_color_for() switches per-monitor colours on
        // and announces the change, which refreshes the screens.
        if let Some(list) = self.monitor_list() {
            list.set_color_for(&key, rgba);
        }
        self.sync_card();
        self.queue_save();
    }

    /// Set the shared colour: what every monitor without one of its own
    /// shows. The cycle, the command line and the settings file all drive
    /// this one, never a single monitor's colour.
    pub fn apply_color(&self, rgba: gdk::RGBA) {
        let imp = self.imp();
        imp.color.set(rgba);

        // Saving every step of a running queue would rewrite the file every
        // interval for a colour nobody chose. save_settings() stores the
        // colour the queue took over from instead.
        if !self.cycle().is_some_and(|c| c.is_playing()) {
            self.queue_save();
        }
        self.refresh_colors();
    }

    /// Push the colours that are now true out to the sidebar and the screens.
    fn refresh_colors(&self) {
        let shared = self.imp().color.get();
        let list = self.monitor_list();

        // The rows that are following the shared colour follow it here, so
        // the sidebar and the card never disagree about what a screen shows.
        if let Some(list) = list.as_ref() {
            list.set_shared_color(shared);
        }
        // A monitor with a colour of its own keeps it; the rest follow the
        // shared one. Brightness applies to both.
        for entry in self.imp().entries.borrow().iter() {
            let own = list.as_ref().and_then(|l| l.color_for(&entry.key));
            entry.overlay.set_color(self.scaled(own.unwrap_or(shared)));
        }
        self.sync_card();
    }

    /// Make the card show the colour it is editing: the preview, the marked
    /// chip, and the colour the queue would append.
    fn sync_card(&self) {
        let imp = self.imp();
        // palette.select() moves a chip, which reports a colour chosen,
        // which lands back here. The guard is what stops that going round.
        if imp.syncing.replace(true) {
            return;
        }

        let editing = self.card_color();
        if let Some(cycle) = self.cycle() {
            cycle.set_current_color(editing);
        }
        if let Some(palette) = self.palette() {
            // Not while the timer owns the chips; on_cycling_changed() has
            // greyed them out and cleared them on purpose.
            if !self.cycle().is_some_and(|c| c.is_playing()) {
                palette.select(editing);
            }
        }
        // The preview shows what a screen will, brightness included, so it
        // is also what the label has to name.
        let shown = self.scaled(editing);
        let preview = imp.preview.borrow().clone();
        if let Some(p) = preview {
            p.set_rgba(shown);
            p.update_property(&[gtk::accessible::Property::Label(&i18n::format(
                &gettext("Color preview, {0}"),
                &[&color::hex(shown)],
            ))]);
        }

        self.update_scope_hint();
        imp.syncing.set(false);
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

        let Some(list) = self.monitor_list() else { return };
        let targets: Vec<(ScreenOverlay, gdk::Monitor)> = imp
            .entries
            .borrow()
            .iter()
            .filter(|e| list.is_selected(&e.key) && e.monitor.is_valid())
            .map(|e| (e.overlay.clone(), e.monitor.clone()))
            .collect();

        // Nothing attached to show on. Leave `overlays_shown` alone: if the
        // last selected monitor was just unplugged the overlays are still
        // logically up, and replugging it must bring the colour back.
        //
        // The inhibitor cannot stay, though. Nothing is on any screen to
        // justify it, and on the session-manager route it would otherwise
        // keep the machine awake until the app quit. Nor can the countdown:
        // it would take down overlays that are already down and, when
        // headless, end the process over a monitor that was merely unplugged.
        if targets.is_empty() {
            self.release_idle();
            self.cancel_auto_hide();
            // Nothing on screen to play onto either.
            if let Some(cycle) = self.cycle() {
                cycle.set_playing(false);
            }
            return;
        }
        imp.overlays_shown.set(true);

        // Whether the notice on each overlay should name the stepping keys.
        let steppable = self.cycle().is_some_and(|c| c.can_step());

        for (overlay, monitor) in targets {
            overlay.set_cycle_hint(steppable);
            overlay.show_on_monitor(Some(&monitor));
        }
        // Overlays and badges share Layer::Overlay, where the compositor
        // stacks by map order — so re-present the badges last, otherwise
        // Identify would be hidden underneath the overlays just shown.
        if imp.identify.get() {
            self.present_labels();
        }

        // There is something to play onto now. Harmless on the hot-plug path,
        // which comes back through here with the queue already running.
        if let Some(cycle) = self.cycle() {
            cycle.set_playing(true);
        }

        self.arm_auto_hide();

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
        if cookie != 0
            && let Some(app) = self.application()
        {
            app.uninhibit(cookie);
        }
    }

    /// Hide every overlay. Bound to the app-scoped `hide-overlays` action,
    /// so one ESC press on any screen clears all of them.
    pub fn hide_all_overlays(&self) {
        self.imp().overlays_shown.set(false);
        self.cancel_auto_hide();
        // Before hiding, so the colour the queue took over from is the one
        // left behind rather than whichever step it stopped on.
        if let Some(cycle) = self.cycle() {
            cycle.set_playing(false);
        }
        self.release_idle();
        for ov in self.overlays() {
            ov.hide_overlay();
        }

        // Nothing left to look at. A window that was never shown has nothing
        // to fall back to, so the process ends here rather than lingering
        // invisibly after a scripted --show.
        if self.imp().headless.get()
            && let Some(app) = self.application()
        {
            app.quit();
        }
    }

    /// Toggle the per-monitor connector-name badges.
    pub fn set_identify(&self, on: bool) {
        self.imp().identify.set(on);
        // Keep the button honest when this came from the command line. A
        // no-op when it came *from* the button, which is what stops the two
        // from calling each other.
        if let Some(btn) = self.imp().ident_btn.borrow().as_ref() {
            btn.set_active(on);
        }
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

    fn palette(&self) -> Option<Palette> {
        self.imp().palette.borrow().clone()
    }

    /// The colour one monitor should show: its own if it has been given one,
    /// otherwise the shared one.
    fn color_for(&self, key: &str, shared: gdk::RGBA) -> gdk::RGBA {
        self.monitor_list()
            .and_then(|list| list.color_for(key))
            .unwrap_or(shared)
    }

    /// What should cover a screen right now.
    fn fill(&self) -> Fill {
        self.imp().fill_bar.borrow().as_ref().map(FillBar::fill).unwrap_or_default()
    }

    /// Push the current fill to the preview and every overlay.
    fn apply_fill(&self) {
        let fill = self.fill();
        if let Some(preview) = self.imp().preview.borrow().as_ref() {
            preview.set_fill(fill);
        }
        for overlay in self.overlays() {
            overlay.set_fill(fill);
        }
    }

    fn cycle(&self) -> Option<CycleBar> {
        self.imp().cycle.borrow().clone()
    }

    fn monitor_list(&self) -> Option<MonitorList> {
        self.imp().monitors.borrow().clone()
    }

    /// The queue took the colour over, or gave it back.
    ///
    /// Handing it over means clearing the chips and greying them: while the
    /// timer owns the colour, a chip that still looked clickable would be
    /// lying about it. Taking it back restores the colour that was showing
    /// when play started -- through `select()` rather than a remembered chip,
    /// which also covers the case where no chip was marked.
    fn on_cycling_changed(&self, playing: bool) {
        let imp = self.imp();

        if playing {
            imp.pre_cycle_color.set(imp.color.get());
            if let Some(palette) = self.palette() {
                palette.clear();
                palette.set_sensitive(false);
            }
            return;
        }

        let restored = imp.pre_cycle_color.get();
        if let Some(palette) = self.palette() {
            // Only handing the chips back to the user; marking the one that
            // holds the restored colour is sync_card()'s job.
            palette.set_sensitive(true);
        }
        self.apply_color(restored);
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
        let fill  = self.fill();
        let mut old       = self.imp().entries.take();
        let mut graveyard = self.imp().graveyard.take();
        let mut next: Vec<MonitorEntry> = Vec::with_capacity(current.len());
        let mut rekeyed: Vec<(String, String)> = Vec::new();

        for (i, (mon, key)) in current.iter().zip(keys.iter()).enumerate() {
            let title = monitor_list::title(mon, i);
            let sub   = monitor_list::subtitle(mon);
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
                e.overlay.set_color(self.scaled(self.color_for(&e.key, color)));
                e.overlay.set_fill(fill);
                e.label.rebind(mon, &title, sub.as_deref());
                next.push(e);
                continue;
            }

            let overlay = ScreenOverlay::new(&app);
            overlay.set_color(self.scaled(self.color_for(key, color)));
            overlay.set_fill(fill);
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

        // Selections for absent monitors are kept on purpose: unplug a screen
        // and plug it back in and the choice is still there. Carrying a
        // rekeyed one across is the same promise for a key that changed
        // shape rather than a monitor that went away.
        if let Some(list) = self.monitor_list() {
            for (from, to) in rekeyed {
                list.rekey(&from, &to);
            }
        }

        let rows: Vec<(String, gdk::Monitor)> =
            next.iter().map(|e| (e.key.clone(), e.monitor.clone())).collect();
        self.imp().entries.replace(next);

        // A monitor that arrives while the overlays are up gets covered
        // too, and one that comes back goes under the colour it left under.
        if self.imp().overlays_shown.get() {
            self.show_selected();
        } else if self.imp().identify.get() {
            // Badges for newly arrived monitors must respect Identify.
            self.present_labels();
        }

        if let Some(list) = self.monitor_list() {
            list.set_monitors(&rows);
        }
        // After the list, so "Applies to" is rebuilt from the monitors the
        // sidebar is now showing.
        self.rebuild_target_row();
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
