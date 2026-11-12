//! The action tiles on the card: an icon beside a title and a subtitle.
//!
//! Its own module because two widgets build them -- the window's Show / Hide /
//! Identify row, and the cycle bar's mode switch -- and a tile that looked
//! subtly different in one of those places would read as a mistake.

use std::sync::Once;

use gtk::prelude::*;

static CSS_ONCE: Once = Once::new();

const CSS: &str = "
.action-tile   { border-radius: 14px; padding: 8px 18px; }
.tile-title    { font-weight: 700; }
.tile-subtitle { font-size: 12px; opacity: 0.80; }
";

/// The inside of a tile: pass it as the child of a button or toggle, and give
/// that button the `action-tile` style class.
pub fn content(icon_name: &str, title: &str, subtitle: &str) -> gtk::Box {
    register_css();

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(20);

    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Start)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["tile-title"])
            .halign(gtk::Align::Start)
            .build(),
    );
    text.append(
        &gtk::Label::builder()
            .label(subtitle)
            .css_classes(["tile-subtitle"])
            .halign(gtk::Align::Start)
            .build(),
    );

    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    inner.append(&icon);
    inner.append(&text);
    inner
}

fn register_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    CSS_ONCE.call_once(move || {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
}
