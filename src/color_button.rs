//! A colour swatch that opens the picker.
//!
//! Everything in the Choose Color card draws a colour the same way: a rounded
//! swatch with a hairline that contrasts with its own fill, dimmed when the
//! row it sits in is switched off (see `swatch.rs`). This is that swatch with
//! a button around it.
//!
//! It exists because the two controls that pick a colour outside the chips --
//! a monitor's own colour in the sidebar, and the second half of a region
//! fill, which sits *in* the card -- were stock `GtkColorDialogButton`s. Two
//! vocabularies for "this is a colour" in one window is one too many, and the
//! stock button cannot be dimmed to match or carry the hairline that keeps a
//! white swatch from disappearing into a light card.

use std::cell::{Cell, RefCell};
use std::sync::Once;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};

use crate::i18n::{self, gettext};
use crate::{color, swatch};

/// The size the queue strip uses, so a colour reads the same wherever it is.
const SWATCH_W: i32 = 34;
const SWATCH_H: i32 = 24;
const RADIUS: f64 = 8.0;

/// Dimming for a swatch whose row has been made insensitive. GTK greys the
/// labels on its own; a swatch painted with cairo is whatever we paint.
const DIMMED: f64 = 0.35;

static CSS_ONCE: Once = Once::new();

const CSS: &str = "
.color-button {
    padding: 3px;
    min-width: 0; min-height: 0;
    border-radius: 11px;
}
/* The one the card is aimed at. Same ring the cycle queue puts on the step
   that is showing, because it means the same thing: this is the one. */
.color-button-current { box-shadow: 0 0 0 2px @accent_bg_color; }
";

mod imp {
    use super::*;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    pub struct ColorButton {
        pub rgba:  Cell<gdk::RGBA>,
        /// Whether a plain click opens the picker.
        ///
        /// False where the button belongs to something the click should
        /// select first -- a monitor row, where clicking aims the Choose
        /// Color card at that monitor and the picker is a double-click away,
        /// exactly as a swatch in the cycle queue behaves.
        pub picker_on_click: Cell<bool>,
        /// Names the thing being coloured: the dialog's title, and the start
        /// of the tooltip. "Color for DP-1", say.
        pub title: RefCell<String>,
        pub area:  gtk::DrawingArea,
    }

    impl Default for ColorButton {
        fn default() -> Self {
            Self {
                rgba:  Cell::new(gdk::RGBA::WHITE),
                picker_on_click: Cell::new(true),
                title: RefCell::new(String::new()),
                area: gtk::DrawingArea::builder()
                    .width_request(SWATCH_W)
                    .height_request(SWATCH_H)
                    .build(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColorButton {
        const NAME: &'static str = "WhiteScreenColorButton";
        type Type       = super::ColorButton;
        type ParentType = gtk::Button;
    }

    impl ObjectImpl for ColorButton {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // A colour was picked *here*. Not emitted by set_rgba():
                    // the owner setting the swatch must not read back as the
                    // user choosing it.
                    Signal::builder("color-chosen")
                        .param_types([gdk::RGBA::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

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

            self.area.set_draw_func(glib::clone!(
                #[weak] obj,
                move |area, cr, w, h| {
                    let alpha = if area.is_sensitive() { 1.0 } else { DIMMED };
                    swatch::draw(cr, w as f64, h as f64, RADIUS, obj.rgba(), alpha);
                }
            ));
            // The swatch is the button's whole visible content, so a screen
            // reader announcing it as well would be announcing nothing twice.
            self.area.set_accessible_role(gtk::AccessibleRole::Presentation);

            obj.set_child(Some(&self.area));
            obj.add_css_class("flat");
            obj.add_css_class("color-button");
            obj.set_cursor_from_name(Some("pointer"));
            obj.set_valign(gtk::Align::Center);
            obj.connect_clicked(|obj| {
                if obj.imp().picker_on_click.get() {
                    obj.open_picker();
                }
            });

            // Two ways into the picker when a click is doing something else,
            // because neither is guessable on its own: the tooltip names the
            // double-click, and a right-click is what anyone looking for a
            // per-item menu tries first. (The queue's swatches do the same.)
            let edit = gtk::GestureClick::new();
            edit.connect_pressed(glib::clone!(
                #[weak] obj,
                move |gesture, n_press, _, _| {
                    if n_press == 2 {
                        // Claim it so the button does not also report a click
                        // for the second press of the pair.
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        obj.open_picker();
                    }
                }
            ));
            obj.add_controller(edit);

            let context = gtk::GestureClick::new();
            context.set_button(gdk::BUTTON_SECONDARY);
            context.connect_pressed(glib::clone!(
                #[weak] obj,
                move |_, _, _, _| obj.open_picker()
            ));
            obj.add_controller(context);
            // A cairo swatch does not grey itself out with the rest of the
            // row, so it has to be told the row changed.
            obj.connect_sensitive_notify(|obj| obj.imp().area.queue_draw());
        }
    }

    impl WidgetImpl for ColorButton {}
    impl ButtonImpl for ColorButton {}
}

glib::wrapper! {
    pub struct ColorButton(ObjectSubclass<imp::ColorButton>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ColorButton {
    fn default() -> Self { Self::new("", gdk::RGBA::WHITE) }
}

impl ColorButton {
    /// `title` names what is being coloured -- it becomes the picker's title
    /// and the start of the tooltip.
    pub fn new(title: &str, rgba: gdk::RGBA) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().title.replace(title.to_string());
        obj.set_rgba(rgba);
        obj
    }

    pub fn rgba(&self) -> gdk::RGBA {
        self.imp().rgba.get()
    }

    /// Show a colour without claiming the user chose it.
    ///
    /// This is what lets a monitor with no colour of its own show the shared
    /// one: the swatch follows the chips until somebody picks something here.
    pub fn set_rgba(&self, rgba: gdk::RGBA) {
        let imp = self.imp();
        if imp.rgba.replace(rgba) == rgba {
            return;
        }
        self.describe();
        imp.area.queue_draw();
    }

    /// Whether a plain click opens the picker, or leaves the click to the
    /// owner. See `picker_on_click`.
    pub fn set_picker_on_click(&self, opens: bool) {
        self.imp().picker_on_click.set(opens);
        self.describe();
    }

    /// Ring this swatch, or stop ringing it: the Choose Color card is aimed
    /// here.
    pub fn set_current(&self, current: bool) {
        if current {
            self.add_css_class("color-button-current");
        } else {
            self.remove_css_class("color-button-current");
        }
    }


    pub fn connect_color_chosen<F: Fn(&Self, gdk::RGBA) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "color-chosen",
            false,
            glib::closure_local!(move |btn: &Self, rgba: gdk::RGBA| f(btn, rgba)),
        )
    }

    /// Name the colour as well as the thing, the way the queue's swatches do:
    /// a swatch nobody can see the hex of is a swatch nobody can write down.
    fn describe(&self) {
        let name = i18n::format(
            &gettext("{0} — {1}"),
            &[&self.imp().title.borrow(), &color::hex(self.rgba())],
        );
        // Say what a click will do, because it is not the same everywhere:
        // where the click selects, the picker is a double-click away and
        // nothing on screen would otherwise say so.
        let tooltip = if self.imp().picker_on_click.get() {
            name.clone()
        } else {
            i18n::format(
                &gettext("{0}\nClick to point Choose Color here, double-click to pick one"),
                &[&name],
            )
        };
        self.set_tooltip_text(Some(&tooltip));
        // A drawn button has no accessible name of its own.
        self.update_property(&[gtk::accessible::Property::Label(&name)]);
    }

    fn open_picker(&self) {
        let parent = self.root().and_downcast::<gtk::Window>();
        let current = self.rgba();

        gtk::ColorDialog::builder()
            .title(self.imp().title.borrow().clone())
            .with_alpha(false)
            .modal(true)
            .build()
            .choose_rgba(
                parent.as_ref(),
                Some(&current),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[weak(rename_to = btn)] self,
                    move |result| {
                        // Err is the user cancelling, which must leave the
                        // swatch exactly as it was.
                        let Ok(rgba) = result else { return };
                        btn.set_rgba(rgba);
                        btn.emit_by_name::<()>("color-chosen", &[&rgba]);
                    }
                ),
            );
    }
}
