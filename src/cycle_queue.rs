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
use whitescreen_core::queue::{DEFAULT, Steps};

const SWATCH_W: i32 = 34;
const SWATCH_H: i32 = 24;
use crate::i18n::{self, gettext};

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
        self.imp().steps.borrow().colors().iter().map(|c| color::from_core(*c)).collect()
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
        let colors: Vec<_> = colors.iter().map(|c| color::to_core(*c)).collect();
        self.imp().steps.borrow_mut().set_colors(&colors);
        self.rebuild();
        self.emit_by_name::<()>("changed", &[]);
    }

    /// Append a colour. Silently does nothing when the queue is full -- the
    /// button that gets here is insensitive by then, and a dialog for
    /// "sixteen is enough" would be worse than the limit.
    pub fn push(&self, rgba: gdk::RGBA) {
        if !self.imp().steps.borrow_mut().push(color::to_core(rgba)) {
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
        let default: Vec<gdk::RGBA> = DEFAULT.iter().map(|c| color::from_core(*c)).collect();
        self.set_colors(&default);
    }

    /// Replace one step's colour.
    pub fn set_color_at(&self, index: usize, rgba: gdk::RGBA) {
        if !self.imp().steps.borrow_mut().set_color_at(index, color::to_core(rgba)) {
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
        let Some(rgba) = self.imp().steps.borrow().colors().get(index).copied().map(color::from_core)
        else {
            return;
        };

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
        let rgba = self.imp().steps.borrow_mut().start(color::to_core(current)).map(color::from_core);
        self.update_ring();
        rgba
    }

    /// The timer's move to the next step, wrapping at the end.
    pub fn advance(&self) -> Option<gdk::RGBA> {
        let rgba = self.imp().steps.borrow_mut().advance().map(color::from_core);
        self.update_ring();
        rgba
    }

    /// A step taken by hand, in either direction. Unlike `advance()` this
    /// leaves the ring following the colour on screen rather than the timer,
    /// so it works with the timer stopped as well as while it runs.
    pub fn step(&self, forward: bool) -> Option<gdk::RGBA> {
        let rgba = self.imp().steps.borrow_mut().step(forward).map(color::from_core);
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
        self.imp().steps.borrow_mut().highlight(color::to_core(rgba));
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
