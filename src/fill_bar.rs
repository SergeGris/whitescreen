//! The modes that change *what* covers a screen, rather than which colour it
//! is: a partial fill, the stuck-pixel exerciser, a brightness wedge and a
//! pixel grid.
//!
//! All off by default. Filling a screen with a colour is what the app is for,
//! and none of these should be in the way of doing that.
//!
//! They are mutually exclusive, and not merely for tidiness: noise over half
//! a screen exercises half the pixels, and a wedge behind a grid is two
//! patterns competing to answer different questions. One at a time is what
//! each of them is for.

use std::cell::{Cell, RefCell};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib};

use crate::color_button::ColorButton;
use crate::color_surface::{Fill, Side};
use crate::i18n::{gettext, mark};

/// The colour the second half starts on: mid grey, which is what a uniformity
/// comparison usually wants beside a colour.
const DEFAULT_REST: gdk::RGBA = gdk::RGBA::new(0.5, 0.5, 0.5, 1.0);

const SIDES: &[(Side, &str, &str)] = &[
    (Side::Full,   mark("Full"),   mark("Cover the whole screen")),
    (Side::Left,   mark("Left"),   mark("Cover the left half")),
    (Side::Right,  mark("Right"),  mark("Cover the right half")),
    (Side::Top,    mark("Top"),    mark("Cover the top half")),
    (Side::Bottom, mark("Bottom"), mark("Cover the bottom half")),
];

/// Which fill mode is on.
///
/// The mode itself, its name on the command line and in the settings file,
/// and which fill it stands for all live in the core, because the Windows
/// front end has the same five buttons and must not disagree about any of
/// it. What stays here is the part that is a widget: the labels, the
/// tooltips and the strip they sit in.
pub use whitescreen_core::Pattern as Mode;

mod imp {
    use super::*;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    pub struct FillBar {
        pub region:    gtk::ToggleButton,
        pub exerciser: gtk::ToggleButton,
        pub wedge:     gtk::ToggleButton,
        pub grid:      gtk::ToggleButton,
        pub reveal:    gtk::Revealer,

        pub sides:     RefCell<Vec<(Side, gtk::ToggleButton)>>,
        pub side:      Cell<Side>,
        pub use_rest:  gtk::CheckButton,
        pub rest:      ColorButton,

        /// Set while the buttons are being driven from `set_mode()` rather
        /// than by a click. Without it, switching mode would emit `changed`
        /// once for the button turned on and once more for each of the three
        /// turned off.
        pub updating:  Cell<bool>,
    }

    impl Default for FillBar {
        fn default() -> Self {
            Self {
                region: gtk::ToggleButton::builder()
                    .label(gettext("Region"))
                    .tooltip_text(gettext("Cover only part of each screen, so the rest stays visible beside it"))
                    .build(),
                exerciser: gtk::ToggleButton::builder()
                    .label(gettext("Exerciser"))
                    .tooltip_text(gettext(
                        "Drift a block of noise across each screen, the usual way to free a stuck subpixel. Give it a few minutes.",
                    ))
                    .build(),
                wedge: gtk::ToggleButton::builder()
                    .label(gettext("Wedge"))
                    .tooltip_text(gettext(
                        "A staircase from black to the color, for spotting banding and a mis-set gamma",
                    ))
                    .build(),
                grid: gtk::ToggleButton::builder()
                    .label(gettext("Grid"))
                    .tooltip_text(gettext(
                        "One-pixel lines on black, for checking that one screen pixel is still one panel pixel",
                    ))
                    .build(),
                reveal: gtk::Revealer::builder()
                    .transition_type(gtk::RevealerTransitionType::SlideDown)
                    .build(),
                sides:    RefCell::new(Vec::new()),
                side:     Cell::new(Side::Full),
                use_rest: gtk::CheckButton::with_label(&gettext("Second color")),
                // The same swatch the chips and the queue use: this control
                // sits inside the Choose Color card, and a stock colour
                // button beside them read as a different kind of thing.
                rest: ColorButton::new(&gettext("Color for the other half"), DEFAULT_REST),
                updating: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FillBar {
        const NAME: &'static str = "WhiteScreenFillBar";
        type Type       = super::FillBar;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for FillBar {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(8);

            let toggles = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .halign(gtk::Align::Center)
                .build();

            for (mode, btn) in obj.mode_buttons() {
                btn.connect_toggled(glib::clone!(
                    #[weak] obj,
                    move |btn| {
                        if obj.imp().updating.get() {
                            return;
                        }
                        // Turning the active one off is a request for the
                        // plain colour, which is what Flat is.
                        obj.set_mode(if btn.is_active() { mode } else { Mode::Flat });
                    }
                ));
                toggles.append(&btn);
            }

            obj.append(&toggles);
            obj.append(&self.reveal);
            self.reveal.set_child(Some(&obj.build_region_controls()));
        }
    }

    impl WidgetImpl for FillBar {}
    impl BoxImpl    for FillBar {}
}

glib::wrapper! {
    pub struct FillBar(ObjectSubclass<imp::FillBar>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for FillBar {
    fn default() -> Self { Self::new() }
}

impl FillBar {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// What should cover a screen, given the mode now switched on.
    pub fn fill(&self) -> Fill {
        let imp = self.imp();
        // The core speaks in the colour a panel is given, not in the
        // toolkit's floats.
        let rest = imp.use_rest.is_active().then(|| crate::color::to_core(imp.rest.rgba()));
        self.mode().fill(imp.side.get(), rest)
    }

    /// The mode now switched on, `Flat` when none is.
    pub fn mode(&self) -> Mode {
        self.mode_buttons()
            .into_iter()
            .find(|(_, btn)| btn.is_active())
            .map_or(Mode::Flat, |(mode, _)| mode)
    }

    /// Switch one mode on and the rest off. Goes through the buttons so they
    /// show the state rather than contradicting it.
    pub fn set_mode(&self, mode: Mode) {
        let imp = self.imp();

        imp.updating.set(true);
        for (m, btn) in self.mode_buttons() {
            btn.set_active(m == mode);
        }
        imp.updating.set(false);

        imp.reveal.set_reveal_child(mode == Mode::Region);
        self.changed();
    }

    /// Which half the region mode covers. Independent of the mode itself, so
    /// that switching Region off and on again comes back to the same half.
    pub fn side(&self) -> Side {
        self.imp().side.get()
    }

    pub fn set_side(&self, side: Side) {
        if let Some((_, btn)) = self.imp().sides.borrow().iter().find(|(s, _)| *s == side) {
            // The buttons are one radio group, so this turns the old one off
            // and its handler records the new side and announces the change.
            btn.set_active(true);
        }
    }

    /// The colour the other half takes, or `None` to leave it transparent --
    /// which shows whatever is behind the overlay.
    pub fn second_color(&self) -> Option<gdk::RGBA> {
        let imp = self.imp();
        imp.use_rest.is_active().then(|| imp.rest.rgba())
    }

    pub fn set_second_color(&self, rgba: Option<gdk::RGBA>) {
        let imp = self.imp();
        if let Some(rgba) = rgba {
            imp.rest.set_rgba(rgba);
        }
        imp.use_rest.set_active(rgba.is_some());
    }

    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "changed",
            false,
            glib::closure_local!(move |bar: &Self| f(bar)),
        )
    }

    fn changed(&self) {
        self.emit_by_name::<()>("changed", &[]);
    }

    /// The mode buttons, paired with the mode each one stands for. `Flat` has
    /// no button of its own: it is every one of these turned off.
    fn mode_buttons(&self) -> Vec<(Mode, gtk::ToggleButton)> {
        let imp = self.imp();
        vec![
            (Mode::Region,    imp.region.clone()),
            (Mode::Exerciser, imp.exerciser.clone()),
            (Mode::Wedge,     imp.wedge.clone()),
            (Mode::Grid,      imp.grid.clone()),
        ]
    }

    fn build_region_controls(&self) -> gtk::Box {
        let imp = self.imp();

        // One linked strip, so the five read as one choice rather than five
        // independent switches.
        let sides = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["linked"])
            .build();

        let mut group: Option<gtk::ToggleButton> = None;
        for &(side, label, tip) in SIDES {
            let btn = gtk::ToggleButton::builder()
                .label(gettext(label))
                .tooltip_text(gettext(tip))
                .active(side == Side::Full)
                .build();
            match &group {
                Some(root) => btn.set_group(Some(root)),
                None       => group = Some(btn.clone()),
            }
            btn.connect_toggled(glib::clone!(
                #[weak(rename_to = bar)] self,
                move |b| {
                    if b.is_active() {
                        bar.imp().side.set(side);
                        bar.changed();
                    }
                }
            ));
            imp.sides.borrow_mut().push((side, btn.clone()));
            sides.append(&btn);
        }

        imp.use_rest.set_tooltip_text(Some(
            "Fill the other half too, instead of leaving what is behind it visible",
        ));
        imp.use_rest.connect_toggled(glib::clone!(
            #[weak(rename_to = bar)] self,
            move |btn| {
                bar.imp().rest.set_sensitive(btn.is_active());
                bar.changed();
            }
        ));
        // The swatch names itself, hex and all; see ColorButton::describe.
        imp.rest.set_sensitive(false);
        imp.rest.connect_color_chosen(glib::clone!(
            #[weak(rename_to = bar)] self,
            move |_, _| if bar.imp().use_rest.is_active() { bar.changed(); }
        ));

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .halign(gtk::Align::Center)
            .build();
        row.append(&sides);
        row.append(&imp.use_rest);
        row.append(&imp.rest);
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_round_trips_through_its_key() {
        for &mode in Mode::ALL {
            assert_eq!(Mode::from_key(mode.as_key()), Some(mode), "{mode:?}");
        }
    }

    #[test]
    fn an_unknown_mode_key_is_rejected_rather_than_guessed() {
        // A settings file from a newer version, or a typed --pattern: the
        // caller has to be able to fall back rather than get Flat silently.
        assert_eq!(Mode::from_key("spiral"), None);
        assert_eq!(Mode::from_key(""), None);
        assert_eq!(Mode::from_key("Flat"), None, "keys are lowercase");
    }

    #[test]
    fn every_side_round_trips_through_its_key() {
        for &(side, _, _) in SIDES {
            assert_eq!(Side::from_key(side.as_key()), Some(side), "{side:?}");
        }
        assert_eq!(Side::from_key("middle"), None);
    }
}
