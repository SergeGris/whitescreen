//! Cycle mode: the switch, the interval, and the queue it plays.
//!
//! A mode rather than a transport. Arming it starts nothing, because the
//! queue plays *onto the screens* -- so it runs while an overlay is up and not
//! before. The window says when that is by calling `set_playing()`; everything
//! else about cycling lives here.
//!
//! The one thing this cannot know is which colour is on screen right now, and
//! it needs that twice: to append it when "+" is pressed, and to carry on from
//! the matching step when play starts. The window keeps it posted through
//! `set_current_color()`.

use std::cell::{Cell, RefCell};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib};

use crate::cycle_queue::CycleQueue;
use crate::tile;
use crate::i18n::gettext;

/// Interval bounds, in seconds, and the step the spinner moves by. In the
/// core because the Windows build clamps `--interval` to the same range, and
/// the floor of it is a safety limit rather than a preference: see
/// `queue::INTERVAL_MIN`.
use whitescreen_core::queue::{
    INTERVAL_DEFAULT as DEFAULT, INTERVAL_MAX as MAX, INTERVAL_MIN as MIN, INTERVAL_STEP as STEP,
};

/// Fewer steps than this is not a cycle, so the switch stays insensitive.
const MIN_STEPS: usize = 2;

mod imp {
    use super::*;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    pub struct CycleBar {
        /// Whether the queue *should* play; the state of the switch. Distinct
        /// from `playing`, which is whether it is playing right now.
        pub armed:   Cell<bool>,
        pub playing: Cell<bool>,
        /// Whether playback is held, from `toggle_pause()`. Still `playing`:
        /// the queue keeps the colour and the position it was stopped on,
        /// which is the difference between pausing and stopping.
        pub paused:  Cell<bool>,
        pub secs:    Cell<f64>,
        pub source:  RefCell<Option<glib::SourceId>>,
        /// Last colour the window reported as being on screen.
        pub current: Cell<Option<gdk::RGBA>>,
        /// Whether there is an overlay to play onto, as last reported by the
        /// window through `set_playing()`.
        ///
        /// Not derivable from `current`: that is set by every colour change,
        /// including the one at startup, so it is `Some` long before anything
        /// has been put on a screen. Arming used to test it and consequently
        /// started the timer with no overlay up at all, flashing the preview
        /// and greying out the chips.
        pub overlay_up: Cell<bool>,

        pub toggle: gtk::ToggleButton,

        pub spin:   gtk::SpinButton,
        pub queue:  CycleQueue,
    }

    impl Default for CycleBar {
        fn default() -> Self {
            Self {
                armed:   Cell::new(false),
                playing: Cell::new(false),
                paused:  Cell::new(false),
                secs:    Cell::new(DEFAULT),
                source:  RefCell::new(None),
                current: Cell::new(None),
                overlay_up: Cell::new(false),
                toggle: gtk::ToggleButton::builder()

                    .css_classes(["action-tile"])
                    .build(),
                spin: gtk::SpinButton::with_range(MIN, MAX, STEP),
                queue: CycleQueue::new(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CycleBar {
        const NAME: &'static str = "WhiteScreenCycleBar";
        type Type       = super::CycleBar;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for CycleBar {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // Put this colour on the screens: a step the timer moved
                    // to, or a swatch the user clicked.
                    Signal::builder("color-chosen")
                        .param_types([gdk::RGBA::static_type()])
                        .build(),
                    // The queue started or stopped playing. The window uses
                    // it to hand the colour over and to take it back.
                    Signal::builder("playing-changed")
                        .param_types([bool::static_type()])
                        .build(),
                    // A swatch in the strip was clicked.
                    //
                    // Separate from color-chosen because the two mean
                    // different things to the window: this is the user
                    // choosing a colour in the card, which may be aimed at a
                    // single monitor, while color-chosen is the sequence
                    // playing onto the screens, which always drives the
                    // shared colour. The timer must never pin a monitor.
                    Signal::builder("color-activated")
                        .param_types([gdk::RGBA::static_type()])
                        .build(),
                    // Something worth saving changed: the queue itself, or
                    // the interval. Not the position in the queue, and not
                    // whether it is playing -- neither of those is saved.
                    Signal::builder("state-changed").build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(8);

            self.toggle.set_child(Some(&tile::content(
                "media-playlist-repeat-symbolic",
                "Cycle",
                "While the overlay is up",
            )));
            self.toggle.connect_toggled(glib::clone!(
                #[weak] obj,
                move |btn| obj.arm(btn.is_active())
            ));

            self.spin.set_digits(1);
            self.spin.set_value(DEFAULT);
            self.spin.set_tooltip_text(Some(&gettext("Seconds on each color")));
            self.spin.connect_value_changed(glib::clone!(
                #[weak] obj,
                move |spin| obj.apply_interval(spin.value())
            ));

            let row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::Center)
                .build();
            row.append(&self.toggle);
            row.append(&gtk::Label::builder().label("every").css_classes(["dim-label"]).build());
            row.append(&self.spin);
            row.append(&gtk::Label::builder().label("s").css_classes(["dim-label"]).build());

            self.queue.connect_activated(glib::clone!(
                #[weak] obj,
                move |_, rgba| obj.emit_by_name::<()>("color-activated", &[&rgba])
            ));
            self.queue.connect_add_requested(glib::clone!(
                #[weak] obj,
                move |q| if let Some(rgba) = obj.imp().current.get() { q.push(rgba); }
            ));
            self.queue.connect_changed(glib::clone!(
                #[weak] obj,
                move |q| {
                    // The list moved under the ring: put it back on the colour
                    // actually showing, which may now be at another step or at
                    // none.
                    if let Some(rgba) = obj.imp().current.get() {
                        q.highlight(rgba);
                    }
                    obj.update_sensitivity();
                    obj.emit_by_name::<()>("state-changed", &[]);
                }
            ));

            obj.append(&row);
            obj.append(&self.queue);
            obj.update_sensitivity();
        }
    }

    impl WidgetImpl for CycleBar {}
    impl BoxImpl    for CycleBar {}
}

glib::wrapper! {
    pub struct CycleBar(ObjectSubclass<imp::CycleBar>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for CycleBar {
    fn default() -> Self { Self::new() }
}

impl CycleBar {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Whether the timer is running right now.
    pub fn is_playing(&self) -> bool {
        self.imp().playing.get()
    }

    /// Tell the bar which colour is on the screens.
    ///
    /// Two things need it: "+" appends it, and starting play carries on from
    /// the step that holds it. It also moves the ring, so the strip agrees
    /// with the chips about what is showing.
    pub fn set_current_color(&self, rgba: gdk::RGBA) {
        let imp = self.imp();
        imp.current.set(Some(rgba));
        imp.queue.highlight(rgba);
    }

    /// Say whether there is an overlay to play onto.
    ///
    /// Starting is conditional -- the mode has to be armed and the queue long
    /// enough -- so a `true` here is a permission, not an order. It is also
    /// remembered, because arming the mode later has to know the answer.
    pub fn set_playing(&self, playing: bool) {
        self.imp().overlay_up.set(playing);
        if playing {
            self.start();
        } else {
            self.stop();
        }
    }

    /// Step through the queue by hand, forwards or backwards.
    ///
    /// Works whether or not the timer is running: the point is to sweep a
    /// panel at your own pace while looking at it, which is the one thing the
    /// timer cannot do. Nothing happens with fewer than two steps.
    pub fn step(&self, forward: bool) {
        let imp = self.imp();
        if imp.queue.len() < MIN_STEPS {
            return;
        }
        if let Some(rgba) = imp.queue.step(forward) {
            self.emit_by_name::<()>("color-chosen", &[&rgba]);
        }
        // Give the colour just chosen a full interval rather than whatever
        // was left of the one it interrupted.
        if imp.playing.get() && !imp.paused.get() {
            self.restart_timer();
        }
    }

    /// Hold the timer where it is, or let it go on.
    ///
    /// Only meaningful while playing; pausing keeps the colour and the
    /// position, which is what separates it from stopping.
    pub fn toggle_pause(&self) {
        let imp = self.imp();
        if !imp.playing.get() {
            return;
        }
        if imp.paused.replace(!imp.paused.get()) {
            self.restart_timer();
        } else {
            self.stop_timer();
        }
    }

    /// Arm or disarm from outside -- the command line, say. Goes through the
    /// switch so the button shows the state rather than contradicting it.
    pub fn set_armed(&self, armed: bool) {
        self.imp().toggle.set_active(armed);
    }

    /// Seconds per step, clamped to what the spinner itself offers so a
    /// command line cannot ask for a flash rate the UI refuses to.
    pub fn set_interval(&self, secs: f64) {
        self.imp().spin.set_value(secs.clamp(MIN, MAX));
    }

    /// Kill the timer without telling anyone. For teardown, where restoring a
    /// colour on a window that is going away would be busywork.
    pub fn stop_timer(&self) {
        if let Some(id) = self.imp().source.replace(None) {
            id.remove();
        }
    }

    /// A colour to put on the screens.
    pub fn connect_color_chosen<F: Fn(&Self, gdk::RGBA) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "color-chosen",
            false,
            glib::closure_local!(move |bar: &Self, rgba: gdk::RGBA| f(bar, rgba)),
        )
    }

    /// A swatch was clicked: a colour chosen in the card, not the sequence
    /// playing.
    pub fn connect_color_activated<F: Fn(&Self, gdk::RGBA) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "color-activated",
            false,
            glib::closure_local!(move |bar: &Self, rgba: gdk::RGBA| f(bar, rgba)),
        )
    }

    /// The queue started or stopped playing.
    pub fn connect_playing_changed<F: Fn(&Self, bool) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "playing-changed",
            false,
            glib::closure_local!(move |bar: &Self, playing: bool| f(bar, playing)),
        )
    }

    /// The queue, for the settings file.
    pub fn queue_colors(&self) -> Vec<gdk::RGBA> {
        self.imp().queue.colors()
    }

    /// Restore a saved queue. An empty list is ignored rather than obeyed:
    /// nothing in the UI can produce one, so it means a corrupt file, and the
    /// default sequence is a better answer than an empty strip.
    pub fn set_queue_colors(&self, colors: &[gdk::RGBA]) {
        if colors.is_empty() {
            return;
        }
        self.imp().queue.set_colors(colors);
    }

    /// Seconds on each step.
    pub fn interval(&self) -> f64 {
        self.imp().secs.get()
    }

    /// The queue or the interval changed -- the parts of cycle mode that are
    /// worth saving.
    pub fn connect_state_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "state-changed",
            false,
            glib::closure_local!(move |bar: &Self| f(bar)),
        )
    }

    /// Whether there is a sequence to step through at all.
    pub fn can_step(&self) -> bool {
        self.imp().queue.len() >= MIN_STEPS
    }

    // ── Internals ────────────────────────────────────────────────────────

    /// Arm or disarm. Arming while an overlay is already up starts play there
    /// and then -- waiting for the next Show would mean waiting forever, since
    /// the overlays cover the window this switch lives in.
    fn arm(&self, armed: bool) {
        let imp = self.imp();
        if imp.armed.get() == armed {
            return;
        }
        imp.armed.set(armed);

        if !armed {
            self.stop();
        } else if imp.overlay_up.get() && self.can_play() {
            // Only with an overlay up, which the window reports through
            // set_playing(); `playing` is false here precisely because the
            // mode was not armed when it said so.
            self.start();
        }
    }

    fn can_play(&self) -> bool {
        self.imp().armed.get() && self.imp().queue.len() >= MIN_STEPS
    }

    fn start(&self) {
        let imp = self.imp();
        if imp.playing.get() || !self.can_play() {
            return;
        }
        imp.playing.set(true);
        imp.paused.set(false);
        self.emit_by_name::<()>("playing-changed", &[&true]);

        // Carry on from the step already showing where there is one, so
        // showing black and then cycling continues from black.
        let from = imp.current.get().unwrap_or(gdk::RGBA::WHITE);
        if let Some(rgba) = imp.queue.start(from) {
            self.emit_by_name::<()>("color-chosen", &[&rgba]);
        }
        self.restart_timer();
    }

    fn stop(&self) {
        let imp = self.imp();
        if !imp.playing.get() {
            return;
        }
        imp.playing.set(false);
        imp.paused.set(false);
        self.stop_timer();
        imp.queue.stop();
        self.emit_by_name::<()>("playing-changed", &[&false]);

    }

    fn restart_timer(&self) {
        self.stop_timer();
        let id = glib::timeout_add_local(
            std::time::Duration::from_secs_f64(self.imp().secs.get()),
            glib::clone!(
                #[weak(rename_to = bar)] self,
                #[upgrade_or] glib::ControlFlow::Break,
                move || {
                    if let Some(rgba) = bar.imp().queue.advance() {
                        bar.emit_by_name::<()>("color-chosen", &[&rgba]);
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );
        self.imp().source.replace(Some(id));
    }

    /// Change the seconds per step, restarting a running timer so the new
    /// interval takes effect now rather than after the current step.
    fn apply_interval(&self, secs: f64) {
        let imp = self.imp();
        if imp.secs.get() == secs {
            return;
        }
        imp.secs.set(secs);
        if imp.playing.get() {
            self.restart_timer();
        }
        self.emit_by_name::<()>("state-changed", &[]);
    }

    fn update_sensitivity(&self) {
        let imp = self.imp();
        let enough = imp.queue.len() >= MIN_STEPS;

        imp.toggle.set_sensitive(enough);
        imp.toggle.set_tooltip_text(Some(if enough {
            "Step through the queued colors while the overlay is showing"
        } else {
            "Queue at least two colors to cycle through them"
        }));
        if !enough && imp.toggle.is_active() {
            // Emptying the queue while it is playing stops it.
            imp.toggle.set_active(false);
        }
    }
}
