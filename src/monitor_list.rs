//! The sidebar: one row per attached monitor, ticked or not.
//!
//! Owns the selection, because the selection is about monitors and nothing
//! else here is. It is keyed rather than positional, and keys for monitors
//! that are *not* attached are kept: that is what makes unplugging a screen
//! and plugging it back in restore the choice rather than lose it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::Once;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib};

use crate::color_button::ColorButton;
use whitescreen_core::glob;
use crate::i18n::{self, gettext};

/// What a row calls a monitor: its connector name, or a position when the
/// compositor does not report one.
pub fn title(mon: &gdk::Monitor, index: usize) -> String {
    mon.connector()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| i18n::format(&gettext("Monitor {0}"), &[&(index + 1).to_string()]))
}

/// The panel behind the connector, when it says.
pub fn subtitle(mon: &gdk::Monitor) -> String {
    [mon.model(), mon.manufacturer()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(" — ")
}

static CSS_ONCE: Once = Once::new();

const CSS: &str = "
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
";

mod imp {
    use super::*;
    use std::sync::OnceLock;

    use glib::subclass::Signal;

    pub struct MonitorList {
        /// Keys of the ticked monitors, attached or not.
        pub selected: RefCell<HashSet<String>>,
        /// Keys of the monitors currently attached, in display order.
        pub attached: RefCell<Vec<String>>,
        /// The same monitors, kept so the rows can be redrawn without the
        /// window handing them over again.
        pub monitors: RefCell<Vec<(String, gdk::Monitor)>>,
        /// Set while the rows are being driven from the selection rather than
        /// the other way round; see `refresh()`.
        pub updating: Cell<bool>,

        /// The tick box of each attached row, in the same order as
        /// `attached`. Kept rather than walked out of the widget tree,
        /// because the rows grew a second interactive widget.
        pub checks: RefCell<Vec<gtk::CheckButton>>,
        /// Per-monitor colour overrides, kept even while the mode is off so
        /// that switching it back on restores what was set.
        pub colors: RefCell<HashMap<String, gdk::RGBA>>,
        /// The colour chosen in the Choose Color card, which every monitor
        /// without an override is actually showing. Kept so their swatches
        /// can say so instead of inventing a colour of their own.
        pub shared: Cell<gdk::RGBA>,
        /// The rows' colour buttons, by key, so the shared colour can be
        /// pushed into the ones that are following it without rebuilding the
        /// sidebar underneath an open picker.
        pub swatches: RefCell<Vec<(String, ColorButton)>>,
        /// The monitor the Choose Color card is aimed at, so the row can say
        /// so. Mirrors the window's own `target`; the window is the one that
        /// decides.
        pub target:   RefCell<Option<String>>,
        pub per_monitor: gtk::CheckButton,
        pub rows: gtk::Box,
    }

    impl Default for MonitorList {
        fn default() -> Self {
            Self {
                selected: RefCell::new(HashSet::new()),
                attached: RefCell::new(Vec::new()),
                monitors: RefCell::new(Vec::new()),
                updating: Cell::new(false),
                checks:   RefCell::new(Vec::new()),
                colors:   RefCell::new(HashMap::new()),
                shared:   Cell::new(gdk::RGBA::WHITE),
                swatches: RefCell::new(Vec::new()),
                target:   RefCell::new(None),
                per_monitor: gtk::CheckButton::with_label(&gettext("Per-monitor colors")),
                rows: gtk::Box::new(gtk::Orientation::Vertical, 6),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MonitorList {
        const NAME: &'static str = "WhiteScreenMonitorList";
        type Type       = super::MonitorList;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for MonitorList {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("selection-changed").build(),
                    // A per-monitor colour was set, or the mode was switched.
                    Signal::builder("colors-changed").build(),
                    // A row's swatch was clicked: point the Choose Color
                    // card at this monitor. The list does not aim anything
                    // itself -- the window owns where the card is pointed,
                    // and tells the list back through set_target().
                    Signal::builder("target-requested")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(6);
            obj.set_margin_top(12);
            obj.set_margin_bottom(12);
            obj.set_margin_start(12);
            obj.set_margin_end(12);

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

            obj.append(
                &gtk::Label::builder()
                    .label(gettext("Monitors"))
                    .halign(gtk::Align::Start)
                    .css_classes(["title-4"])
                    .build(),
            );
            obj.append(
                &gtk::Label::builder()
                    .label(gettext("Select one or more monitors"))
                    .halign(gtk::Align::Start)
                    .css_classes(["dim-label"])
                    .build(),
            );

            self.per_monitor.set_tooltip_text(Some(&gettext(
                "Give each monitor its own color, for matching panels against one another or telling them apart",
            )));
            self.per_monitor.set_margin_top(6);
            self.per_monitor.connect_toggled(glib::clone!(
                #[weak] obj,
                move |_| {
                    obj.rebuild();
                    obj.emit_by_name::<()>("colors-changed", &[]);
                }
            ));
            obj.append(&self.per_monitor);

            // Rows live in their own box so a hot-plug can clear and
            // repopulate it without touching the headings.
            obj.append(&self.rows);
        }
    }

    impl WidgetImpl for MonitorList {}
    impl BoxImpl    for MonitorList {}
}

glib::wrapper! {
    pub struct MonitorList(ObjectSubclass<imp::MonitorList>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for MonitorList {
    fn default() -> Self { Self::new() }
}

impl MonitorList {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Replace the rows with the monitors now attached, in display order.
    ///
    /// A first run with nothing ticked ticks the first monitor, so the app is
    /// usable without a trip to the sidebar. Past that, "nothing selected" is
    /// the user's decision and is left alone.
    pub fn set_monitors(&self, monitors: &[(String, gdk::Monitor)]) {
        let imp = self.imp();

        imp.attached.replace(monitors.iter().map(|(k, _)| k.clone()).collect());
        imp.monitors.replace(monitors.to_vec());

        let empty = imp.selected.borrow().is_empty();
        if empty
            && let Some((key, _)) = monitors.first()
        {
            imp.selected.borrow_mut().insert(key.clone());
        }

        self.rebuild();
        self.emit_by_name::<()>("selection-changed", &[]);
    }

    /// Redraw the rows from what is attached. Separate from `set_monitors()`
    /// because switching the per-monitor colour mode changes the rows without
    /// changing which monitors there are.
    fn rebuild(&self) {
        let imp = self.imp();
        let monitors = imp.monitors.borrow().clone();

        while let Some(child) = imp.rows.first_child() {
            imp.rows.remove(&child);
        }
        imp.checks.replace(Vec::new());
        imp.swatches.replace(Vec::new());

        if monitors.is_empty() {
            // Zero monitors is a transient state during a hot-plug, not the
            // fatal error it was once treated as.
            imp.rows.append(
                &gtk::Label::builder()
                    .label(gettext("No monitors detected"))
                    .css_classes(["dim-label"])
                    .halign(gtk::Align::Start)
                    .margin_top(12)
                    .build(),
            );
        }
        for (i, (key, mon)) in monitors.iter().enumerate() {
            imp.rows.append(&self.build_row(i, key, mon));
        }
    }

    pub fn is_selected(&self, key: &str) -> bool {
        self.imp().selected.borrow().contains(key)
    }

    /// Whether any *attached* monitor is ticked.
    ///
    /// Intersect, do not just count: keys for unplugged monitors are kept on
    /// purpose, and "Show" must not look clickable when none of the ticked
    /// screens is currently there.
    pub fn any_attached_selected(&self) -> bool {
        let imp = self.imp();
        let selected = imp.selected.borrow();
        imp.attached.borrow().iter().any(|k| selected.contains(k))
    }

    /// Tick exactly these keys. Unknown keys are kept like any other absent
    /// monitor, so naming a screen that is not plugged in yet still works.
    pub fn select_only(&self, keys: impl IntoIterator<Item = String>) {
        self.imp().selected.replace(keys.into_iter().collect());
        self.refresh();
    }

    /// Tick every attached monitor matching one of these shell-style
    /// patterns, and report the patterns that matched nothing.
    ///
    /// A plain name with no wildcard in it is kept whether or not it is
    /// attached, which is the same promise the sidebar makes: naming a screen
    /// that is not plugged in yet is meaningful, and it is ticked the moment
    /// it appears. A *pattern* that matches nothing is a different thing --
    /// there is no key to keep, and it is almost always a typo -- so it comes
    /// back to the caller to report.
    pub fn select_matching(&self, patterns: &[String]) -> Vec<String> {
        let attached = self.imp().attached.borrow().clone();

        let mut keys: HashSet<String> = HashSet::new();
        let mut unmatched = Vec::new();

        for pattern in patterns {
            let hits: Vec<&String> =
                attached.iter().filter(|key| glob::matches(pattern, key)).collect();

            if hits.is_empty() {
                if glob::is_pattern(pattern) {
                    unmatched.push(pattern.clone());
                } else {
                    keys.insert(pattern.clone());
                }
                continue;
            }
            keys.extend(hits.into_iter().cloned());
        }

        self.select_only(keys);
        unmatched
    }

    /// Every ticked key, attached or not, in a stable order so that saving it
    /// twice produces the same file.
    pub fn selected(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.imp().selected.borrow().iter().cloned().collect();
        keys.sort();
        keys
    }

    /// Whether each monitor gets a colour of its own.
    pub fn per_monitor(&self) -> bool {
        self.imp().per_monitor.is_active()
    }

    pub fn set_per_monitor(&self, on: bool) {
        self.imp().per_monitor.set_active(on);
    }

    /// The per-monitor colours that have been set, whether or not the mode is
    /// on -- they are kept across switching it off, and so are saved.
    pub fn colors(&self) -> Vec<(String, gdk::RGBA)> {
        let mut colors: Vec<(String, gdk::RGBA)> =
            self.imp().colors.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect();
        colors.sort_by(|a, b| a.0.cmp(&b.0));
        colors
    }

    /// Restore saved per-monitor colours, without switching the mode on --
    /// that is a separate saved setting.
    pub fn set_colors(&self, colors: &[(String, gdk::RGBA)]) {
        let mut map = self.imp().colors.borrow_mut();
        for (key, rgba) in colors {
            map.insert(key.clone(), *rgba);
        }
    }

    /// The keys of the monitors currently attached, in display order.
    pub fn attached(&self) -> Vec<String> {
        self.imp().attached.borrow().clone()
    }

    pub fn select_all_attached(&self) {
        let all = self.imp().attached.borrow().clone();
        self.select_only(all);
    }

    /// Carry a selection across a key that changed shape -- a connector name
    /// reported late, or a duplicate suffix shifting when its twin was
    /// unplugged.
    pub fn rekey(&self, from: &str, to: &str) {
        let mut selected = self.imp().selected.borrow_mut();
        if selected.remove(from) {
            selected.insert(to.to_string());
        }
    }

    pub fn connect_selection_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "selection-changed",
            false,
            glib::closure_local!(move |list: &Self| f(list)),
        )
    }

    /// Rebuild the rows from the monitors already known, after the selection
    /// changed underneath them.
    fn refresh(&self) {
        let imp = self.imp();

        // Read the whole answer out first. set_active() emits `toggled`
        // synchronously, and that handler borrows the selection mutably --
        // holding a borrow across it is a panic, not a deadlock, because the
        // call goes out through C and comes back in.
        let wanted: Vec<bool> = {
            let selected = imp.selected.borrow();
            imp.attached.borrow().iter().map(|k| selected.contains(k)).collect()
        };

        // And stop the rows from answering back: they would each re-record a
        // selection this method already knows, and emit a signal apiece.
        imp.updating.set(true);
        for (check, &want) in imp.checks.borrow().iter().zip(wanted.iter()) {
            check.set_active(want);
        }
        imp.updating.set(false);

        self.emit_by_name::<()>("selection-changed", &[]);
    }

    /// Say which monitor the Choose Color card is aimed at, so its row can
    /// show it. `None` rings nothing, which is the card aimed at everything.
    pub fn set_target(&self, key: Option<&str>) {
        let imp = self.imp();
        imp.target.replace(key.map(str::to_string));
        for (row_key, swatch) in imp.swatches.borrow().iter() {
            swatch.set_current(Some(row_key.as_str()) == key);
        }
    }

    pub fn connect_target_requested<F: Fn(&Self, String) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_closure(
            "target-requested",
            false,
            glib::closure_local!(move |list: &Self, key: String| f(list, key)),
        )
    }

    /// Tell the rows which colour the Choose Color card is on, so the ones
    /// following it keep saying so.
    ///
    /// Only the swatches that are actually following are touched: a monitor
    /// with a colour of its own keeps showing it, and `ColorButton::set_rgba`
    /// does not report a choice, so none of this reads back as the user
    /// picking anything.
    pub fn set_shared_color(&self, rgba: gdk::RGBA) {
        let imp = self.imp();
        imp.shared.set(rgba);

        let colors = imp.colors.borrow();
        for (key, swatch) in imp.swatches.borrow().iter() {
            if !colors.contains_key(key) {
                swatch.set_rgba(rgba);
            }
        }
    }

    /// The colour this monitor should get, or `None` to use the shared one.
    pub fn color_for(&self, key: &str) -> Option<gdk::RGBA> {
        let imp = self.imp();
        if !imp.per_monitor.is_active() {
            return None;
        }
        imp.colors.borrow().get(key).copied()
    }

    /// Give one monitor a colour of its own, switching the mode on: asking
    /// for a per-monitor colour is asking for per-monitor colours.
    pub fn set_color_for(&self, key: &str, rgba: gdk::RGBA) {
        let imp = self.imp();
        imp.colors.borrow_mut().insert(key.to_string(), rgba);
        if imp.per_monitor.is_active() {
            self.rebuild();
            self.emit_by_name::<()>("colors-changed", &[]);
        } else {
            // The toggle rebuilds and announces for us.
            imp.per_monitor.set_active(true);
        }
    }

    /// Give every attached monitor matching `pattern` a colour of its own,
    /// and say whether anything came of it.
    ///
    /// The same bargain `select_matching()` makes, for the same reason: a
    /// plain name is kept whether or not that screen is plugged in, because
    /// the colour is keyed and waiting for it, while a *pattern* that matches
    /// nothing has no key to keep and is almost always a typo -- so it comes
    /// back to the caller to report.
    pub fn set_color_matching(&self, pattern: &str, rgba: gdk::RGBA) -> bool {
        let hits: Vec<String> = self
            .imp()
            .attached
            .borrow()
            .iter()
            .filter(|key| glob::matches(pattern, key))
            .cloned()
            .collect();

        if hits.is_empty() {
            if glob::is_pattern(pattern) {
                return false;
            }
            self.set_color_for(pattern, rgba);
            return true;
        }
        for key in hits {
            self.set_color_for(&key, rgba);
        }
        true
    }

    pub fn connect_colors_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "colors-changed",
            false,
            glib::closure_local!(move |list: &Self| f(list)),
        )
    }

    fn build_row(&self, index: usize, key: &str, mon: &gdk::Monitor) -> gtk::Widget {
        let geo = mon.geometry();
        let key = key.to_string();

        let icon = gtk::Image::builder()
            .icon_name("video-display-symbolic")
            .pixel_size(44)
            .valign(gtk::Align::Center)
            .build();

        let title_lbl = gtk::Label::builder()
            .label(title(mon, index))
            .halign(gtk::Align::Start)
            .css_classes(["heading"])
            .build();
        let res_lbl = gtk::Label::builder()
            .label(format!("{} × {}", geo.width(), geo.height()))
            .halign(gtk::Align::Start)
            .css_classes(["dim-label"])
            .build();

        // refresh_rate() reports 0 when the compositor does not know it; the
        // old code rendered that as a bogus "0.000 Hz".
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
            .label((index + 1).to_string())
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
            .active(self.is_selected(&key))
            .child(&row)
            .hexpand(true)
            .css_classes(["monitor-check"])
            .build();
        // Without this the name is every label in the row run together,
        // refresh rate and scale factor included.
        check.update_property(&[gtk::accessible::Property::Label(&i18n::format(
            &gettext("{0}, {1} by {2}"),
            &[&title(mon, index), &geo.width().to_string(), &geo.height().to_string()],
        ))]);

        // Decoration either side of the text: the number repeats the position
        // the row already has, and the badge repeats the tick state.
        num_lbl.set_accessible_role(gtk::AccessibleRole::Presentation);
        check_badge.set_accessible_role(gtk::AccessibleRole::Presentation);
        icon.set_accessible_role(gtk::AccessibleRole::Presentation);

        check.connect_toggled(glib::clone!(
            #[weak(rename_to = list)] self,
            #[strong] key,
            move |btn| {
                if list.imp().updating.get() {
                    return;
                }
                {
                    let mut selected = list.imp().selected.borrow_mut();
                    if btn.is_active() {
                        selected.insert(key.clone());
                    } else {
                        selected.remove(&key);
                    }
                }
                list.emit_by_name::<()>("selection-changed", &[]);
            }
        ));
        self.imp().checks.borrow_mut().push(check.clone());

        if !self.imp().per_monitor.is_active() {
            return check.upcast();
        }

        // Beside the tick box rather than inside it: a button nested in a
        // check button is a click the user has to aim, and missing it toggles
        // the monitor instead of opening the picker.
        //
        // A row with no colour of its own shows the shared one, because that
        // is what that screen is displaying. Showing white there -- which is
        // what a bare GtkColorDialogButton defaulted to -- made the swatch
        // disagree with the screen it names, and confirming the picker on it
        // silently set white.
        let picked = self.imp().colors.borrow().get(&key).copied();
        let color = ColorButton::new(
            &i18n::format(&gettext("Color for {0}"), &[&key]),
            picked.unwrap_or(self.imp().shared.get()),
        );
        // A click points the card here rather than opening a picker of its
        // own: one place to choose a colour, and the sidebar says which
        // screen it is for. Double-click still goes straight to the picker.
        color.set_picker_on_click(false);
        color.set_current(self.imp().target.borrow().as_deref() == Some(key.as_str()));
        color.connect_clicked(glib::clone!(
            #[weak(rename_to = list)] self,
            #[strong] key,
            move |_| list.emit_by_name::<()>("target-requested", &[&key])
        ));
        color.connect_color_chosen(glib::clone!(
            #[weak(rename_to = list)] self,
            #[strong] key,
            move |_, rgba| {
                list.imp().colors.borrow_mut().insert(key.clone(), rgba);
                list.emit_by_name::<()>("colors-changed", &[]);
            }
        ));
        self.imp().swatches.borrow_mut().push((key.clone(), color.clone()));

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        row.append(&check);
        row.append(&color);
        row.upcast()
    }
}
