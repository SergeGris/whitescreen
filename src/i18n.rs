//! Translation.
//!
//! Every string the user can read goes through `gettext()`, which looks the
//! English text up in the catalogue for the current locale and hands back the
//! English text unchanged when there is no catalogue -- so an untranslated
//! build behaves exactly as it did before, and a missing translation degrades
//! to English one string at a time rather than all at once.
//!
//! # Plurals
//!
//! A sentence whose shape depends on a count uses `ngettext()` with both
//! English forms and the number. Picking between them with an `if` would be
//! picking by *English* rules, and plenty of languages do not have two forms
//! or do not split them where English does.
//!
//! # Placeholders
//!
//! Anything with a value in it uses `{0}`, `{1}` … and `format()` below,
//! never Rust's own `format!`. Two reasons, and the second is the important
//! one: `format!` needs a literal, so a translated template cannot be passed
//! to it at all; and word order differs between languages, so a translator
//! has to be able to move the values around, which numbered placeholders
//! allow and bare `{}` does not.

use std::sync::OnceLock;

pub use gettextrs::{gettext, ngettext};

/// Catalogue name. Matches the `.mo` files Meson installs.
const DOMAIN: &str = "whitescreen";

/// Where the catalogues live.
///
/// Meson passes the configured localedir in; a plain `cargo build` has no
/// build system to ask, so it falls back to the usual place. Getting this
/// wrong costs translations, not correctness.
fn locale_dir() -> &'static str {
    option_env!("WHITESCREEN_LOCALEDIR").unwrap_or("/usr/share/locale")
}

/// Point gettext at our catalogues. Call once, before anything is displayed.
///
/// Every step is best-effort: a locale that cannot be set, or catalogues that
/// are not installed, leaves the app running in English rather than refusing
/// to start.
pub fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
        let _ = gettextrs::bindtextdomain(DOMAIN, locale_dir());
        // The whole of GTK deals in UTF-8, and a catalogue in anything else
        // would arrive as mojibake rather than as an error.
        let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
        let _ = gettextrs::textdomain(DOMAIN);
    });
}

/// Mark a string for translation without looking it up here.
///
/// For the tables of names that are built as constants -- the colour chips,
/// the region sides -- where the literal has to sit in a `const` and the
/// lookup has to happen later, once there is a locale. `xgettext` is told to
/// collect this as a keyword, so the English text is extracted from the table
/// and `gettext()` at the point of use finds the translation.
pub const fn mark(s: &'static str) -> &'static str {
    s
}

/// Fill the numbered placeholders in a translated template.
///
/// The substitution itself is in the core crate, because the native Windows
/// front end needs the same one and neither front end owns it.
pub use whitescreen_core::text::fill as format;
