//! The cycle queue: the ordered list of colours the Cycle button plays, and
//! the strip of swatches that edits it.
//!
//! It is its own widget, and its own list, because it used to be neither. The
//! cycle drove the preset chips directly, which meant the sequence could only
//! ever be those five colours -- no cyan, no magenta, no custom pick, and no
//! way to drop a step. The chips answer "what colour do I want on screen";
//! this answers "what sequence do I want to flick through", and the two are
//! not the same question.
//!
//! Playback lives with the owner: this holds the list and the position in it,
//! and hands out the next colour when asked. Nothing here knows about
//! monitors, overlays, or what the colours are for.
//!
//! A step is clicked to put its colour on screen, double-clicked (or
//! right-clicked) to change that colour, and removed with the x revealed on
//! hover.

use std::cell::RefCell;
use std::sync::Once;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};

use crate::{color, swatch};
use crate::i18n::{self, gettext};

/// Colours a fresh queue starts with.
///
/// White and black drive every subpixel fully on and fully off; red, green and
/// blue light one at a time; cyan, magenta and yellow light them in pairs,
/// which is where a single dead subpixel stops hiding behind its neighbours.
const DEFAULT: &[gdk::RGBA] = &[
    gdk::RGBA::WHITE,
    gdk::RGBA::BLACK,
    gdk::RGBA::RED,
    gdk::RGBA::GREEN,
    gdk::RGBA::BLUE,
    gdk::RGBA::new(0.0, 1.0, 1.0, 1.0), // cyan
    gdk::RGBA::new(1.0, 0.0, 1.0, 1.0), // magenta
    gdk::RGBA::new(1.0, 1.0, 0.0, 1.0), // yellow
];

/// Upper bound on the queue. The strip is one row and the window has a width;
/// past this it would either wrap or push the card out of shape.
const MAX_LEN: usize = 16;

const SWATCH_W: i32 = 34;
const SWATCH_H: i32 = 24;

/// The list of colours and the position in it, with no widget attached.
///
/// Split out from the widget because this is the part with the awkward
/// arithmetic -- removing a step shifts every later one down, a queue may hold
/// the same colour twice, and the ring is chosen differently depending on
/// whether the timer or the user put it there -- and none of it needs a
/// display to test. (`dedup_idents` in main_window.rs is split out for the
/// same reason.)
#[derive(Debug, Clone, PartialEq)]
pub struct Steps {
    colors: Vec<gdk::RGBA>,
    /// Step the ring is on.
    index: Option<usize>,
    /// Whether the timer put it there.
    ///
    /// Both modes ring a step, but they choose it differently: playback knows
    /// the index it just moved to, while everything else only knows the colour
    /// now on screen and has to look it up. Keeping the flag is what makes a
    /// queue holding the same colour twice ring the step actually being played
    /// rather than the first one that matches.
    playing: bool,
}

impl Default for Steps {
    fn default() -> Self {
        Self { colors: DEFAULT.to_vec(), index: None, playing: false }
    }
}

impl Steps {
    pub fn colors(&self) -> &[gdk::RGBA] { &self.colors }

    pub fn len(&self) -> usize { self.colors.len() }

    pub fn is_empty(&self) -> bool { self.colors.is_empty() }

    /// Whether another step would fit; what makes "+" insensitive.
    pub fn is_full(&self) -> bool { self.len() >= MAX_LEN }

    pub fn index(&self) -> Option<usize> { self.index }

    /// Replace the list.
    ///
    /// A position into the old list says nothing about the new one, and
    /// keeping it left the ring on whichever step happened to inherit the
    /// index -- at startup, on the default list's step 0 rather than on the
    /// colour actually showing.
    pub fn set_colors(&mut self, colors: &[gdk::RGBA]) {
        self.colors = colors.iter().take(MAX_LEN).copied().collect();
        self.index = None;
    }

    /// Append a colour. False when the queue is full -- the button that gets
    /// here is insensitive by then, and a dialog for "sixteen is enough"
    /// would be worse than the limit.
    pub fn push(&mut self, rgba: gdk::RGBA) -> bool {
        if self.is_full() {
            return false;
        }
        self.colors.push(rgba);
        true
    }

    /// Drop a step.
    ///
    /// The ring follows the same *step*, not the same position: dropping an
    /// earlier step shifts every later one down by one. Dropping the ringed
    /// step clears the ring, and the caller decides whether some other step
    /// holds the colour still on screen.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.colors.len() {
            return false;
        }
        self.colors.remove(index);
        self.index = match self.index {
            Some(i) if i == index => None,
            Some(i) if i > index  => Some(i - 1),
            other                 => other,
        };
        true
    }

    /// Replace one step's colour, and ring it. False when the index is out of
    /// range, or the colour is the one already there.
    pub fn set_color_at(&mut self, index: usize, rgba: gdk::RGBA) -> bool {
        match self.colors.get_mut(index) {
            Some(slot) if !color::same(*slot, rgba) => *slot = rgba,
            _ => return false,
        }
        self.index = Some(index);
        true
    }

    pub fn position_of(&self, rgba: gdk::RGBA) -> Option<usize> {
        self.colors.iter().position(|c| color::same(*c, rgba))
    }

    /// Ring one step by position, whatever else holds its colour.
    pub fn select(&mut self, index: usize) {
        if index < self.colors.len() {
            self.index = Some(index);
        }
    }

    /// Begin playing, from the step showing `current` if the queue holds it
    /// and from the top otherwise -- so hitting Cycle while black is up
    /// carries on from black rather than jumping.
    pub fn start(&mut self, current: gdk::RGBA) -> Option<gdk::RGBA> {
        let index = self.position_of(current).unwrap_or(0);
        self.playing = true;
        self.seek(index)
    }

    /// The timer's move to the next step.
    pub fn advance(&mut self) -> Option<gdk::RGBA> {
        let next = self.neighbour(true)?;
        self.playing = true;
        self.seek(next)
    }

    /// A step taken by hand, from the overlay or the control window.
    ///
    /// Unlike `advance()` this does not claim the ring for the timer: stepping
    /// with the timer stopped has to leave the chips following the colour on
    /// screen, which is what `highlight()` does while `playing` is false.
    pub fn step(&mut self, forward: bool) -> Option<gdk::RGBA> {
        let next = self.neighbour(forward)?;
        self.seek(next)
    }

    /// Stop playing. The ring goes back to following whatever colour is on
    /// screen, which the caller sets with `highlight()`.
    pub fn stop(&mut self) {
        self.playing = false;
        self.index = None;
    }

    /// Ring the step holding `rgba`, or nothing if no step does.
    ///
    /// Ignored while playing: the timer's position is the authority then.
    ///
    /// A step that was picked out explicitly -- clicked, stepped to, or just
    /// edited -- keeps the ring as long as it still holds this colour. A queue
    /// may hold the same colour twice, and clicking the second red then
    /// resolving the ring by colour would snap it back to the first one.
    pub fn highlight(&mut self, rgba: gdk::RGBA) {
        if self.playing {
            return;
        }
        if self
            .index
            .is_some_and(|i| self.colors.get(i).is_some_and(|c| color::same(*c, rgba)))
        {
            return;
        }
        self.index = self.position_of(rgba);
    }

    /// The step either side of the ring, wrapping at both ends. With no ring
    /// yet, the end the move came from.
    fn neighbour(&self, forward: bool) -> Option<usize> {
        let len = self.colors.len();
        if len == 0 {
            return None;
        }
        Some(match (self.index, forward) {
            (None, true)     => 0,
            (None, false)    => len - 1,
            (Some(i), true)  => (i + 1) % len,
            // Add len before subtracting: 0 - 1 wraps a usize.
            (Some(i), false) => (i + len - 1) % len,
        })
    }

    fn seek(&mut self, index: usize) -> Option<gdk::RGBA> {
        let rgba = self.colors.get(index).copied()?;
        self.index = Some(index);
        Some(rgba)
    }
}

/// Every instance styles its swatches identically, so the provider is
/// registered once per display rather than once per widget.
static CSS_ONCE: Once = Once::new();

const CSS: &str = "
.queue-item {
    padding: 3px;
    min-width: 0; min-height: 0;
    border-radius: 11px;
}
/* The ring marks the step that is on screen right now. */
.queue-current { box-shadow: 0 0 0 2px @accent_bg_color; }

/* Revealed on hover, so eight swatches do not read as eight close buttons. */
.queue-remove {
    min-width: 18px; min-height: 18px;
    padding: 0;
    border-radius: 999px;
    background: @window_bg_color;
    color: @window_fg_color;
    opacity: 0;
    transition: opacity 120ms ease;
}
.queue-slot:hover .queue-remove { opacity: 1; }
";

mod imp {
    use super::*;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    pub struct CycleQueue {
        /// The list and the position in it; see `Steps`, which is where all
        /// of that arithmetic lives.
        pub steps: RefCell<Steps>,

        /// Holds the swatches; rebuilt whenever the list changes.
        pub strip: gtk::Box,
        pub add:   gtk::Button,
        /// The slot widgets, kept so the ring can move without rebuilding
        /// the strip on every tick.
        pub items: RefCell<Vec<gtk::Widget>>,
    }

    impl Default for CycleQueue {
        fn default() -> Self {
            Self {
                steps: RefCell::new(Steps::default()),
                strip: gtk::Box::builder()

                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(4)
                    .build(),
                add: gtk::Button::builder()
                    .icon_name("list-add-symbolic")
                    .css_classes(["flat", "circular"])
                    .tooltip_text(gettext("Add the current color to the end of the queue"))
                    .build(),
                items: RefCell::new(Vec::new()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CycleQueue {
        const NAME: &'static str = "WhiteScreenCycleQueue";
        type Type       = super::CycleQueue;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for CycleQueue {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // The list itself changed: the owner saves it and
                    // re-checks whether there is enough to play.
                    Signal::builder("changed").build(),
                    // A swatch was clicked; the colour is the payload.
                    Signal::builder("activated")
                        .param_types([gdk::RGBA::static_type()])
                        .build(),
                    // "+" was pressed. The queue has no idea what colour is
                    // on screen, so it asks rather than guesses.
                    Signal::builder("add-requested").build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_orientation(gtk::Orientation::Horizontal);
            obj.set_spacing(6);
            obj.set_halign(gtk::Align::Center);

            if let Some(display) = gdk::Display::default() {
                CSS_ONCE.call_once(move || {
                    let provider = gtk::CssProvider::new();
                    provider.load_from_string(super::CSS);
                    gtk::style_context_add_provider_for_display(
                        &display,
                        &provider,
                        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                    );
                });
            }

            self.add.update_property(&[gtk::accessible::Property::Label(
                "Add the current color to the queue",
            )]);
            self.add.connect_clicked(glib::clone!(
                #[weak] obj,
                move |_| obj.emit_by_name::<()>("add-requested", &[])
            ));

            let reset = gtk::Button::builder()
                .icon_name("view-refresh-symbolic")
                .css_classes(["flat", "circular"])
                .tooltip_text(gettext("Reset the queue to the default dead-pixel sequence"))
                .build();
            reset.update_property(&[gtk::accessible::Property::Label(
                "Reset the queue to the default sequence",
            )]);
            reset.connect_clicked(glib::clone!(
                #[weak] obj,
                move |_| obj.reset()
            ));

            obj.append(&self.strip);
            obj.append(&self.add);
            obj.append(&reset);

            obj.rebuild();
        }
    }

    impl WidgetImpl for CycleQueue {}
    impl BoxImpl    for CycleQueue {}
}

glib::wrapper! {
    pub struct CycleQueue(ObjectSubclass<imp::CycleQueue>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for CycleQueue {
    fn default() -> Self { Self::new() }
}

impl CycleQueue {
    pub fn new() -> Self {
        glib::Object::new()
    }

    // ── The list ─────────────────────────────────────────────────────────

    pub fn colors(&self) -> Vec<gdk::RGBA> {
        self.imp().steps.borrow().colors().to_vec()
    }

    pub fn len(&self) -> usize {
        self.imp().steps.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.imp().steps.borrow().is_empty()
    }

    /// Whether another step would fit; what makes "+" insensitive.
    pub fn is_full(&self) -> bool {
        self.imp().steps.borrow().is_full()
    }

    pub fn set_colors(&self, colors: &[gdk::RGBA]) {
        self.imp().steps.borrow_mut().set_colors(colors);
        self.rebuild();
        self.emit_by_name::<()>("changed", &[]);
    }

    /// Append a colour. Silently does nothing when the queue is full -- the
    /// button that gets here is insensitive by then, and a dialog for
    /// "sixteen is enough" would be worse than the limit.
    pub fn push(&self, rgba: gdk::RGBA) {
        if !self.imp().steps.borrow_mut().push(rgba) {
            return;
        }
        self.rebuild();
        self.emit_by_name::<()>("changed", &[]);
    }

    pub fn remove(&self, index: usize) {
        if !self.imp().steps.borrow_mut().remove(index) {
            return;
        }
        self.rebuild();
        self.emit_by_name::<()>("changed", &[]);
    }

    /// Back to the sequence a fresh queue starts with.
    pub fn reset(&self) {
        self.set_colors(DEFAULT);
    }

    /// Replace one step's colour.
    pub fn set_color_at(&self, index: usize, rgba: gdk::RGBA) {
        if !self.imp().steps.borrow_mut().set_color_at(index, rgba) {
            return;
        }
        self.rebuild();

        // `activated` first, and the order matters. It is what puts the new
        // colour on screen, and until that has happened the owner still
        // believes the old one is showing -- so a `changed` handler that
        // re-derives anything from "the current colour" would be working from
        // the colour this edit just replaced. That is how editing the last
        // step to black used to hand the ring to the first black instead.
        self.emit_by_name::<()>("activated", &[&rgba]);
        self.emit_by_name::<()>("changed", &[]);
    }

    /// Open the colour picker on one step.
    ///
    /// The dialog is parented on whatever window this widget ended up in,
    /// rather than being handed one: the queue is a widget, and asking for
    /// its own root keeps the owner out of it.
    fn edit_step(&self, index: usize) {
        let Some(rgba) = self.imp().steps.borrow().colors().get(index).copied() else { return };

        let parent = self.root().and_downcast::<gtk::Window>();

        gtk::ColorDialog::builder()
            .title(i18n::format(&gettext("Color for step {0}"), &[&(index + 1).to_string()]))
            .with_alpha(false)
            .modal(true)
            .build()
            .choose_rgba(
                parent.as_ref(),
                Some(&rgba),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[weak(rename_to = queue)] self,
                    move |result| {
                        // Err is the user cancelling, which must leave the
                        // step exactly as it was.
                        if let Ok(chosen) = result {
                            queue.set_color_at(index, chosen);
                        }
                    }
                ),
            );
    }

    // ── Position ─────────────────────────────────────────────────────────

    /// Begin playing, from the step showing `current` if the queue holds it
    /// and from the top otherwise -- so hitting Cycle while black is up
    /// carries on from black rather than jumping.
    ///
    /// Returns the colour to show, which is `current` itself in the common
    /// case; `None` only when the queue is empty.
    pub fn start(&self, current: gdk::RGBA) -> Option<gdk::RGBA> {
        let rgba = self.imp().steps.borrow_mut().start(current);
        self.update_ring();
        rgba
    }

    /// The timer's move to the next step, wrapping at the end.
    pub fn advance(&self) -> Option<gdk::RGBA> {
        let rgba = self.imp().steps.borrow_mut().advance();
        self.update_ring();
        rgba
    }

    /// A step taken by hand, in either direction. Unlike `advance()` this
    /// leaves the ring following the colour on screen rather than the timer,
    /// so it works with the timer stopped as well as while it runs.
    pub fn step(&self, forward: bool) -> Option<gdk::RGBA> {
        let rgba = self.imp().steps.borrow_mut().step(forward);
        self.update_ring();
        rgba
    }

    /// Stop playing. The ring goes back to following whatever colour is on
    /// screen, which the owner sets with `highlight()`.
    pub fn stop(&self) {
        self.imp().steps.borrow_mut().stop();
        self.update_ring();
    }

    /// Ring the step holding `rgba`, or nothing if no step does.
    ///
    /// Ignored while playing: the timer's position is the authority then.
    pub fn highlight(&self, rgba: gdk::RGBA) {
        self.imp().steps.borrow_mut().highlight(rgba);
        self.update_ring();
    }

    /// Ring one step by position, whatever else holds its colour.
    pub fn select(&self, index: usize) {
        self.imp().steps.borrow_mut().select(index);
        self.update_ring();
    }

    // ── Signals ──────────────────────────────────────────────────────────

    /// The list changed: its length, its order or one of its colours.
    pub fn connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "changed",
            false,
            glib::closure_local!(move |queue: &Self| f(queue)),
        )
    }

    /// A swatch was clicked: put this colour on screen.
    pub fn connect_activated<F: Fn(&Self, gdk::RGBA) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "activated",
            false,
            glib::closure_local!(move |queue: &Self, rgba: gdk::RGBA| f(queue, rgba)),
        )
    }

    /// "+" was pressed; answer with `push()` of whatever is showing.
    pub fn connect_add_requested<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "add-requested",
            false,
            glib::closure_local!(move |queue: &Self| f(queue)),
        )
    }

    // ── Strip ────────────────────────────────────────────────────────────

    fn rebuild(&self) {
        let imp = self.imp();

        while let Some(child) = imp.strip.first_child() {
            imp.strip.remove(&child);
        }
        imp.items.replace(Vec::new());

        let colors = self.colors();
        if colors.is_empty() {
            imp.strip.append(
                &gtk::Label::builder()
                    .label(gettext("Queue is empty"))
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        for (i, rgba) in colors.iter().enumerate() {
            let (slot, item) = self.build_slot(i, *rgba);
            imp.strip.append(&slot);
            imp.items.borrow_mut().push(item);
        }

        imp.add.set_sensitive(!self.is_full());
        self.update_ring();
    }

    /// One step: a swatch that applies its colour, with a remove button
    /// revealed on hover. Returns the widget to pack and the inner button,
    /// which is what carries the ring.
    fn build_slot(&self, index: usize, rgba: gdk::RGBA) -> (gtk::Widget, gtk::Widget) {
        let area = gtk::DrawingArea::builder()
            .width_request(SWATCH_W)
            .height_request(SWATCH_H)
            .build();
        area.set_draw_func(move |_, cr, w, h| {
            swatch::draw(cr, w as f64, h as f64, 8.0, rgba, 1.0);
        });
        // The swatch is the button's whole visible content, so without this
        // a screen reader announces the drawing area as well as the button
        // that contains it -- and has nothing to say about either.
        area.set_accessible_role(gtk::AccessibleRole::Presentation);

        let button = gtk::Button::builder()
            .child(&area)
            .css_classes(["flat", "queue-item"])
            .tooltip_text(i18n::format(
                &gettext("Step {0} — {1}\nClick to show it, double-click to change it"),
                &[&(index + 1).to_string(), &color::hex(rgba)],
            ))
            .build();
        button.set_cursor_from_name(Some("pointer"));
        // An icon-only or drawn button has no accessible name of its own; a
        // tooltip becomes its description, which is announced after the name
        // it does not have.
        button.update_property(&[gtk::accessible::Property::Label(&i18n::format(
            &gettext("Step {0}, {1}"),
            &[&(index + 1).to_string(), &color::hex(rgba)],
        ))]);
        button.connect_clicked(glib::clone!(
            #[weak(rename_to = queue)] self,
            move |_| {
                // By position, not by colour: this is the step the user
                // pointed at, even if an earlier one is the same colour.
                queue.select(index);
                queue.emit_by_name::<()>("activated", &[&rgba]);
            }
        ));

        // Two ways into the picker, because neither is guessable on its own:
        // the tooltip names the double-click, and a right-click is what
        // anyone looking for a per-item menu tries first.
        let edit = gtk::GestureClick::new();
        edit.connect_pressed(glib::clone!(
            #[weak(rename_to = queue)] self,
            move |gesture, n_press, _, _| {
                if n_press == 2 {
                    // Claim it so the button does not also report a click for
                    // the second press of the pair.
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    queue.edit_step(index);
                }
            }
        ));
        button.add_controller(edit);

        let context = gtk::GestureClick::new();
        context.set_button(gdk::BUTTON_SECONDARY);
        context.connect_pressed(glib::clone!(
            #[weak(rename_to = queue)] self,
            move |_, _, _, _| queue.edit_step(index)
        ));
        button.add_controller(context);

        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .css_classes(["flat", "circular", "queue-remove"])
            .tooltip_text(gettext("Remove this step"))
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .build();
        remove.update_property(&[gtk::accessible::Property::Label(&i18n::format(
            &gettext("Remove step {0}"),
            &[&(index + 1).to_string()],
        ))]);
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = queue)] self,
            move |_| queue.remove(index)
        ));

        let slot = gtk::Overlay::builder()
            .child(&button)
            .css_classes(["queue-slot"])
            .build();
        slot.add_overlay(&remove);
        // The remove button must not widen the slot: the strip is sized by
        // its swatches.
        slot.set_measure_overlay(&remove, false);

        (slot.upcast(), button.upcast())
    }

    /// Move the ring to the step that is on screen, or clear it when no step
    /// holds that colour.
    ///
    fn update_ring(&self) {
        let imp = self.imp();
        let current = imp.steps.borrow().index();
        for (i, item) in imp.items.borrow().iter().enumerate() {
            if Some(i) == current {
                item.add_css_class("queue-current");
            } else {
                item.remove_css_class("queue-current");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYAN: gdk::RGBA = gdk::RGBA::new(0.0, 1.0, 1.0, 1.0);

    fn steps(colors: &[gdk::RGBA]) -> Steps {
        let mut s = Steps::default();
        s.set_colors(colors);
        s
    }

    #[test]
    fn a_fresh_queue_is_the_default_sequence_with_no_ring() {
        let s = Steps::default();
        assert_eq!(s.colors(), DEFAULT);
        assert_eq!(s.index(), None);
    }

    #[test]
    fn the_queue_stops_at_its_cap() {
        let mut s = steps(&[gdk::RGBA::WHITE; MAX_LEN]);
        assert!(s.is_full());
        assert!(!s.push(gdk::RGBA::RED), "a full queue must refuse another step");
        assert_eq!(s.len(), MAX_LEN);

        // And a longer list handed in is truncated rather than rejected.
        s.set_colors(&[gdk::RGBA::BLACK; MAX_LEN + 4]);
        assert_eq!(s.len(), MAX_LEN);
    }

    #[test]
    fn removing_a_step_moves_the_ring_with_the_step_it_was_on() {
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK, gdk::RGBA::RED]);

        // The ring is on the last step; dropping an earlier one shifts it down.
        s.select(2);
        assert!(s.remove(0));
        assert_eq!(s.index(), Some(1));

        // Dropping the ringed step itself clears the ring.
        s.select(1);
        assert!(s.remove(1));
        assert_eq!(s.index(), None);

        // Out of range changes nothing.
        assert!(!s.remove(9));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn replacing_a_list_clears_the_position_into_the_old_one() {
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK]);
        s.select(1);
        s.set_colors(&[gdk::RGBA::RED, gdk::RGBA::GREEN]);
        assert_eq!(s.index(), None, "a position into the old list means nothing here");
    }

    #[test]
    fn play_carries_on_from_the_colour_already_showing() {
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK, gdk::RGBA::RED]);
        assert_eq!(s.start(gdk::RGBA::BLACK), Some(gdk::RGBA::BLACK));
        assert_eq!(s.index(), Some(1));

        // A colour no step holds starts from the top.
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK]);
        assert_eq!(s.start(CYAN), Some(gdk::RGBA::WHITE));
        assert_eq!(s.index(), Some(0));
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK, gdk::RGBA::RED]);

        // With no ring yet, each direction starts from the end it came from.
        assert_eq!(s.step(true), Some(gdk::RGBA::WHITE));
        assert_eq!(s.step(true), Some(gdk::RGBA::BLACK));
        assert_eq!(s.step(true), Some(gdk::RGBA::RED));
        assert_eq!(s.step(true), Some(gdk::RGBA::WHITE), "forward wraps at the end");

        // Backwards off step 0 must wrap rather than underflow the usize.
        assert_eq!(s.step(false), Some(gdk::RGBA::RED));
        assert_eq!(s.index(), Some(2));

        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK]);
        assert_eq!(s.step(false), Some(gdk::RGBA::BLACK), "backward starts at the end");
    }

    #[test]
    fn an_empty_queue_has_nowhere_to_step() {
        let mut s = steps(&[]);
        assert_eq!(s.step(true), None);
        assert_eq!(s.step(false), None);
        assert_eq!(s.advance(), None);
        assert_eq!(s.start(gdk::RGBA::WHITE), None);
    }

    #[test]
    fn the_timer_owns_the_ring_but_a_manual_step_does_not() {
        let list = [gdk::RGBA::WHITE, gdk::RGBA::BLACK, gdk::RGBA::RED];

        // advance() is the timer's move: highlight() must not fight it.
        let mut s = steps(&list);
        s.advance();
        s.highlight(gdk::RGBA::RED);
        assert_eq!(s.index(), Some(0), "the timer's position is the authority");

        // step() is the user's move, so the ring still follows the colour.
        let mut s = steps(&list);
        s.step(true);
        s.highlight(gdk::RGBA::RED);
        assert_eq!(s.index(), Some(2));
    }

    #[test]
    fn a_duplicated_colour_keeps_the_ring_on_the_step_chosen() {
        // Two reds: picking the second one and then being told "red is
        // showing" must not snap the ring back to the first.
        let mut s = steps(&[gdk::RGBA::RED, gdk::RGBA::WHITE, gdk::RGBA::RED]);
        s.select(2);
        s.highlight(gdk::RGBA::RED);
        assert_eq!(s.index(), Some(2));

        // A colour no step holds clears it.
        s.highlight(CYAN);
        assert_eq!(s.index(), None);
    }

    #[test]
    fn editing_a_step_rings_it_and_rejects_a_no_op() {
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::BLACK]);
        assert!(s.set_color_at(1, gdk::RGBA::RED));
        assert_eq!(s.colors()[1], gdk::RGBA::RED);
        assert_eq!(s.index(), Some(1));

        assert!(!s.set_color_at(1, gdk::RGBA::RED), "the same colour is not a change");
        assert!(!s.set_color_at(7, gdk::RGBA::BLUE), "out of range is not a change");
    }

    #[test]
    fn a_colour_off_the_byte_grid_still_finds_its_step() {
        // What clicking a swatch compares: the float the picker handed back
        // against the constant the step was built from. See color::same.
        let mut s = steps(&[gdk::RGBA::WHITE, gdk::RGBA::new(0.5, 0.25, 0.75, 1.0)]);
        let parsed = gdk::RGBA::parse(crate::color::hex(s.colors()[1])).unwrap();
        assert_eq!(s.position_of(parsed), Some(1));
        s.highlight(parsed);
        assert_eq!(s.index(), Some(1));
    }
}
