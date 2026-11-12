//! Between the toolkit's colour and the one the panel shows.
//!
//! `gdk::RGBA` holds four `f32`s; `whitescreen_core::Rgba` holds the three
//! bytes a screen is actually given. Everything outside the renderer deals in
//! the latter: the tooltips show `#RRGGBB`, the settings file stores it, and
//! two colours count as the same one when they land on the same byte triple.
//!
//! That last part is why the conversion exists rather than comparing floats:
//! a colour picked from the dialog need not be the same float as the constant
//! it visually matches, so comparing the floats directly would call two
//! identical swatches different.

use gtk::gdk;

pub use whitescreen_core::Rgba;

/// The colour as the panel will show it.
pub fn to_core(rgba: gdk::RGBA) -> Rgba {
    Rgba::from_f32(rgba.red(), rgba.green(), rgba.blue())
}

/// Back to the toolkit's colour. Always opaque: the overlays are.
pub fn from_core(c: Rgba) -> gdk::RGBA {
    let (r, g, b) = c.to_f32();
    gdk::RGBA::new(r, g, b, 1.0)
}

/// `#RRGGBB`. Alpha is dropped: the overlays are opaque.
pub fn hex(rgba: gdk::RGBA) -> String {
    to_core(rgba).hex()
}

/// Whether two colours are the same one to the eye.
pub fn same(a: gdk::RGBA, b: gdk::RGBA) -> bool {
    to_core(a) == to_core(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_uppercase_and_six_digits() {
        assert_eq!(hex(gdk::RGBA::WHITE), "#FFFFFF");
        assert_eq!(hex(gdk::RGBA::BLACK), "#000000");
        assert_eq!(hex(gdk::RGBA::new(0.0, 1.0, 1.0, 1.0)), "#00FFFF");
    }

    #[test]
    fn colors_a_byte_apart_are_not_the_same() {
        let a = gdk::RGBA::new(0.5, 0.5, 0.5, 1.0);
        let b = gdk::RGBA::new(0.5 + 1.0 / 255.0, 0.5, 0.5, 1.0);
        assert!(!same(a, b));
    }

    #[test]
    fn a_colour_off_the_byte_grid_still_matches_its_own_hex() {
        // What clicking a queue swatch compares: the float the picker handed
        // back, against the colour a swatch was built from.
        let picked = gdk::RGBA::new(0.5, 0.25, 0.75, 1.0);
        let shown = gdk::RGBA::parse(hex(picked)).unwrap();
        assert!(same(picked, shown));
        // This is the comparison the module exists to avoid relying on.
        assert_ne!(picked, shown);
    }

    #[test]
    fn a_colour_survives_the_round_trip_through_the_core_type() {
        for c in [gdk::RGBA::WHITE, gdk::RGBA::BLACK, gdk::RGBA::RED] {
            assert!(same(from_core(to_core(c)), c));
        }
    }
}
