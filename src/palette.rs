//! The colour chips: five presets and a custom pick.
//!
//! Answers one question -- what colour does the user want on screen -- and
//! emits it. What happens to that colour, and whether anything else on the
//! card agrees with it, is the window's business.
//!
//! The chips are one radio group, so at most one is ever marked. None being
//! marked is a real state too: the colour on screen may have come from the
//! cycle queue, and leaving a chip lit would have it claiming a colour that
//! is no longer showing.

use std::cell::{Cell, RefCell};
use std::sync::Once;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, gio, glib};

use crate::{color, swatch};
use whitescreen_core::kelvin;
use crate::i18n::{self, gettext, mark};

struct Preset {
    name: &'static str,
    rgba: gdk::RGBA,
}

const PRESETS: &[Preset] = &[
    Preset { name: mark("White"), rgba: gdk::RGBA::WHITE },
    Preset { name: mark("Black"), rgba: gdk::RGBA::BLACK },
    Preset { name: mark("Red"),   rgba: gdk::RGBA::RED   },
    Preset { name: mark("Green"), rgba: gdk::RGBA::GREEN },
    Preset { name: mark("Blue"),  rgba: gdk::RGBA::BLUE  },
];

/// What each key-light temperature is, in words.
///
/// Here rather than in the core crate because it is UI text: the core knows
/// what 3200 K looks like, this knows what to call it -- and `mark()` is
/// what puts these in front of a translator.
const KELVIN_NAMES: &[(u32, &str)] = &[
    (3_200, mark("Tungsten")),
    (4_300, mark("Warm white")),
    (5_600, mark("Daylight")),
    (6_500, mark("D65 — the sRGB white point")),
];

/// The name for a temperature, or the bare number when it has none: a
/// temperature added to the core's list without a label here is still
/// usable, just unexplained.
fn kelvin_name(k: u32) -> &'static str {
    KELVIN_NAMES.iter().find(|&&(t, _)| t == k).map_or("", |&(_, name)| name)
}

/// Colour the custom chip starts on. Cyan rather than white so the chip is
/// visibly *not* one of the presets, and so selecting it does something even
/// before the picker has ever been opened.
const DEFAULT_CUSTOM: gdk::RGBA = gdk::RGBA::new(0.0, 1.0, 1.0, 1.0);

const SWATCH_W: i32 = 64;
const SWATCH_H: i32 = 40;

/// The temperature chips are a second rank, not a sixth to tenth preset, and
/// a smaller swatch is what says so.
const KELVIN_SWATCH_W: i32 = 52;
const KELVIN_SWATCH_H: i32 = 28;

/// Dimming for a swatch whose row has been made insensitive. GTK greys the
/// labels on its own; a swatch painted with cairo is whatever we paint, and a
/// full-strength colour beside a greyed label reads as a rendering fault.
const DIMMED: f64 = 0.35;

static CSS_ONCE: Once = Once::new();

const CSS: &str = "
/* Hide the native indicator — selection is shown by the border.
   A grouped GtkCheckButton renames its indicator node from \"check\" to
   \"radio\", so the chips (which are one radio group) need both selectors:
   with only \"check\" every chip drew a stray radio circle next to its
   swatch. */
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
   eyedropper floated on top so the chip still reads as \"pick a colour\".
   The badge carries its own background because it sits on an arbitrary
   colour -- a bare symbolic icon disappears against half of them. */
.custom-swatch-badge {
    background: alpha(@window_bg_color, 0.85);
    color: @window_fg_color;
    border-radius: 999px;
    padding: 5px;
}
";

/// Invalidate a widget and everything under it.
///
/// A plain `queue_draw()` on the row is not enough for the swatches: each is
/// a GtkDrawingArea with its own cached render node, and its draw function
/// reads a sensitivity that changed without the child being invalidated.
fn queue_draw_deep(widget: &impl IsA<gtk::Widget>) {
    let widget = widget.as_ref();
    widget.queue_draw();
    let mut child = widget.first_child();
    while let Some(c) = child {
        queue_draw_deep(&c);
        child = c.next_sibling();
    }
}

mod imp {
    use super::*;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    #[derive(Default)]
    pub struct Palette {
        /// Preset chips in `PRESETS` order.
        pub presets:    RefCell<Vec<gtk::CheckButton>>,
        /// Colour-temperature chips, in `kelvin::PRESETS` order. A separate
        /// list because they are a separate row, but the same radio group:
        /// one colour is showing, so one chip is marked.
        pub kelvins:    RefCell<Vec<gtk::CheckButton>>,
        pub custom_btn: RefCell<Option<gtk::CheckButton>>,
        /// Colour behind the custom chip. Kept here rather than in the chip's
        /// closure so the swatch and the picker both read the same value.
        pub custom_rgba: Cell<Option<gdk::RGBA>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Palette {
        const NAME: &'static str = "WhiteScreenPalette";
        type Type       = super::Palette;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for Palette {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![Signal::builder("color-chosen")
                    .param_types([gdk::RGBA::static_type()])
                    .build()]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            self.custom_rgba.set(Some(DEFAULT_CUSTOM));
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(10);
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

            obj.build_chips();

            // The swatches paint their own dimming, so they have to be told
            // when the row they sit in is switched off.
            obj.connect_sensitive_notify(queue_draw_deep);
        }
    }

    impl WidgetImpl for Palette {}
    impl BoxImpl    for Palette {}
}

glib::wrapper! {
    pub struct Palette(ObjectSubclass<imp::Palette>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for Palette {
    fn default() -> Self { Self::new() }
}

impl Palette {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// The colour behind the custom chip.
    pub fn custom_color(&self) -> gdk::RGBA {
        self.imp().custom_rgba.get().unwrap_or(DEFAULT_CUSTOM)
    }

    /// Put a colour behind the custom chip without applying it.
    ///
    /// For the settings file: the chip has to remember its colour even when
    /// the colour on screen came from somewhere else, and restoring it has to
    /// happen before the colour itself so that `select()` can recognise it.
    pub fn set_custom_color(&self, rgba: gdk::RGBA) {
        self.imp().custom_rgba.set(Some(rgba));
        // The swatch's draw function reads custom_color() on every pass, so
        // invalidating it is all that is needed to show the new colour.
        queue_draw_deep(self);
    }

    /// Mark whichever chip holds `rgba`, or none when no chip does.
    ///
    /// This is how the chips stay honest about a colour that arrived from
    /// somewhere else -- a queue step, or the cycle timer.
    pub fn select(&self, rgba: gdk::RGBA) {
        let imp = self.imp();

        if let Some(i) = PRESETS.iter().position(|p| color::same(p.rgba, rgba))
            && let Some(btn) = imp.presets.borrow().get(i)
        {
            btn.set_active(true);
            return;
        }
        if let Some(i) = kelvin::PRESETS
            .iter()
            .position(|&k| color::same(color::from_core(kelvin::rgba(k)), rgba))
            && let Some(btn) = imp.kelvins.borrow().get(i)
        {
            btn.set_active(true);
            return;
        }
        if color::same(self.custom_color(), rgba)
            && let Some(btn) = imp.custom_btn.borrow().as_ref()
        {
            btn.set_active(true);
            return;
        }
        self.clear();
    }

    /// Leave the group with nothing marked. Allowed for a GTK4 check-button
    /// group, and the honest state once the colour came from elsewhere.
    pub fn clear(&self) {
        let imp = self.imp();
        for btn in imp.presets.borrow().iter().chain(imp.kelvins.borrow().iter()) {
            btn.set_active(false);
        }
        if let Some(btn) = imp.custom_btn.borrow().as_ref() {
            btn.set_active(false);
        }
    }

    /// Start on the first preset. Activating the chip is enough to publish
    /// its colour: that is what every chip does when clicked.
    pub fn select_default(&self) {
        if let Some(btn) = self.imp().presets.borrow().first() {
            btn.set_active(true);
        }
    }

    /// A colour was chosen here: by a chip, or from the picker.
    pub fn connect_color_chosen<F: Fn(&Self, gdk::RGBA) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "color-chosen",
            false,
            glib::closure_local!(move |palette: &Self, rgba: gdk::RGBA| f(palette, rgba)),
        )
    }

    fn emit_chosen(&self, rgba: gdk::RGBA) {
        self.emit_by_name::<()>("color-chosen", &[&rgba]);
    }

    // ── Construction ─────────────────────────────────────────────────────

    fn build_chips(&self) {
        let mut group_root: Option<gtk::CheckButton> = None;

        // ── The colours ──────────────────────────────────────────────────
        let colors = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Center)
            .build();

        for &Preset { name, rgba } in PRESETS {
            let area = gtk::DrawingArea::builder()
                .width_request(SWATCH_W)
                .height_request(SWATCH_H)
                .build();
            area.set_draw_func(move |area, cr, w, h| {
                let alpha = if area.is_sensitive() { 1.0 } else { DIMMED };
                swatch::draw(cr, w as f64, h as f64, 12.0, rgba, alpha);
            });

            area.set_accessible_role(gtk::AccessibleRole::Presentation);
            let btn = self.build_chip(&area, &gettext(name), &mut group_root);
            btn.connect_toggled(glib::clone!(
                #[weak(rename_to = palette)] self,
                move |b| if b.is_active() { palette.emit_chosen(rgba); }
            ));

            self.imp().presets.borrow_mut().push(btn.clone());
            colors.append(&btn);
        }

        self.build_custom_chip(&colors, &mut group_root);
        self.append(&colors);

        // ── The temperatures ─────────────────────────────────────────────
        // Their own row, labelled, because "3200 K" only means something once
        // you know it is a lighting temperature and not another colour name.
        let temps = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Center)
            .build();
        temps.append(
            &gtk::Label::builder()
                .label(gettext("Key light"))
                .css_classes(["dim-label"])
                .margin_end(4)
                .build(),
        );

        for &k in kelvin::PRESETS {
            let description = kelvin_name(k);
            let rgba = color::from_core(kelvin::rgba(k));

            let area = gtk::DrawingArea::builder()
                .width_request(KELVIN_SWATCH_W)
                .height_request(KELVIN_SWATCH_H)
                .build();
            area.set_draw_func(move |area, cr, w, h| {
                let alpha = if area.is_sensitive() { 1.0 } else { DIMMED };
                swatch::draw(cr, w as f64, h as f64, 10.0, rgba, alpha);
            });
            area.set_accessible_role(gtk::AccessibleRole::Presentation);

            // The unit is part of the translated text: not every language
            // writes the number and the symbol the way English does.
            let btn = self.build_chip(
                &area,
                &i18n::format(&gettext("{0} K"), &[&k.to_string()]),
                &mut group_root,
            );
            btn.set_tooltip_text(Some(&i18n::format(
                &gettext("{0} — {1}"),
                &[&gettext(description), &color::hex(rgba)],
            )));
            btn.connect_toggled(glib::clone!(
                #[weak(rename_to = palette)] self,
                move |b| if b.is_active() { palette.emit_chosen(rgba); }
            ));

            self.imp().kelvins.borrow_mut().push(btn.clone());
            temps.append(&btn);
        }

        self.append(&temps);
    }

    /// The shell every chip shares: swatch over label, inside a check button
    /// whose native indicator the CSS hides.
    fn build_chip(
        &self,
        content: &impl IsA<gtk::Widget>,
        label: &str,
        group_root: &mut Option<gtk::CheckButton>,
    ) -> gtk::CheckButton {
        let inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(6).margin_bottom(6)
            .margin_start(8).margin_end(8)
            .build();
        inner.append(content);
        inner.append(
            &gtk::Label::builder()
                .label(label)
                .css_classes(["color-label"])
                .halign(gtk::Align::Center)
                .build(),
        );

        let btn = gtk::CheckButton::builder()
            .child(&inner)
            .css_classes(["preset-chip"])
            .build();
        btn.set_cursor_from_name(Some("pointer"));

        match group_root {
            Some(root) => btn.set_group(Some(root)),
            None       => *group_root = Some(btn.clone()),
        }
        btn
    }

    /// The custom chip: a swatch of the chosen colour with the eyedropper on
    /// top. It used to be an eyedropper and nothing else, which made the one
    /// chip whose colour is not implied by its name the only one that never
    /// showed it.
    fn build_custom_chip(&self, row: &gtk::Box, group_root: &mut Option<gtk::CheckButton>) {
        let area = gtk::DrawingArea::builder()
            .width_request(SWATCH_W)
            .height_request(SWATCH_H)
            .build();
        area.set_draw_func(glib::clone!(
            #[weak(rename_to = palette)] self,
            move |area, cr, w, h| {
                let alpha = if area.is_sensitive() { 1.0 } else { DIMMED };
                swatch::draw(cr, w as f64, h as f64, 12.0, palette.custom_color(), alpha);
            }
        ));

        let badge = gtk::Image::builder()
            .icon_name("color-select-symbolic")
            .pixel_size(16)
            .css_classes(["custom-swatch-badge"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        area.set_accessible_role(gtk::AccessibleRole::Presentation);
        badge.set_accessible_role(gtk::AccessibleRole::Presentation);

        let stack = gtk::Overlay::builder().child(&area).build();
        stack.add_overlay(&badge);

        let btn = self.build_chip(&stack, &gettext("Custom"), group_root);
        btn.set_tooltip_text(Some(&gettext("Pick a custom color")));
        btn.update_property(&[gtk::accessible::Property::Label(&gettext("Custom color"))]);
        self.imp().custom_btn.replace(Some(btn.clone()));

        // Selecting the chip only re-applies the stored custom colour.
        btn.connect_toggled(glib::clone!(
            #[weak(rename_to = palette)] self,
            move |b| if b.is_active() { palette.emit_chosen(palette.custom_color()); }
        ));

        // Opening the picker is driven by the *click*, not by `toggled`. A
        // check button in a group emits no `toggled` when it is already
        // active, so the old code could never reopen the dialog: once Custom
        // was selected the user had to pick another chip and come back just
        // to change the colour.
        let click = gtk::GestureClick::new();
        click.connect_released(glib::clone!(
            #[weak(rename_to = palette)] self,
            #[weak] btn,
            #[weak] area,
            move |_, _, _, _| {
                btn.set_active(true);
                palette.choose_custom(&area);
            }
        ));
        btn.add_controller(click);

        row.append(&btn);
    }

    fn choose_custom(&self, area: &gtk::DrawingArea) {
        let parent = self.root().and_downcast::<gtk::Window>();

        gtk::ColorDialog::builder()
            .title(gettext("Select custom color"))
            .with_alpha(false)
            .modal(true)
            .build()
            .choose_rgba(
                parent.as_ref(),
                Some(&self.custom_color()),
                gio::Cancellable::NONE,
                glib::clone!(
                    #[weak(rename_to = palette)] self,
                    #[weak] area,
                    move |result| {
                        // Err is the user cancelling; keep the previous custom
                        // colour rather than leaving the chip marked but
                        // showing something stale.
                        let Ok(rgba) = result else { return };
                        palette.imp().custom_rgba.set(Some(rgba));
                        area.queue_draw();
                        palette.emit_chosen(rgba);
                    }
                ),
            );
    }
}
