//! Painting a colour swatch.
//!
//! Three places draw the same thing -- the preset chips, the custom chip and
//! the cycle queue -- and each was repeating the rounded rectangle, the fill
//! and the hairline by hand.

use gtk::gdk;

/// Paint a rounded swatch of `rgba` filling `width` x `height`.
///
/// `alpha` scales the whole thing, for a swatch that has to follow the
/// sensitivity of the row it sits in: GTK dims the labels of an insensitive
/// widget, but a swatch painted with cairo is whatever we paint, and a
/// full-strength colour next to a greyed label reads as a rendering fault.
pub fn draw(
    cr: &gtk::cairo::Context,
    width: f64,
    height: f64,
    radius: f64,
    rgba: gdk::RGBA,
    alpha: f64,
) {
    // Inset by half a pixel so the hairline lands on the pixel grid instead
    // of straddling two rows of it.
    rounded_rect(cr, 0.5, 0.5, width - 1.0, height - 1.0, radius);

    cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, alpha);
    let _ = cr.fill_preserve();

    let (r, g, b, a) = outline(rgba);
    cr.set_source_rgba(r, g, b, a * alpha);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

/// Hairline colour for a swatch: dark on a light fill, light on a dark one.
///
/// A fixed white hairline is invisible around a white swatch on a light card,
/// and a fixed dark one disappears around black in dark mode -- either way the
/// swatch stops being a swatch and becomes a hole.
fn outline(rgba: gdk::RGBA) -> (f64, f64, f64, f64) {
    // Rec. 709 luma: green carries most of the perceived brightness, so a
    // plain average of the channels would call pure blue "light".
    let luma = 0.2126 * rgba.red() + 0.7152 * rgba.green() + 0.0722 * rgba.blue();
    if luma > 0.5 {
        (0.0, 0.0, 0.0, 0.28)
    } else {
        (1.0, 1.0, 1.0, 0.28)
    }
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -FRAC_PI_2,       0.0         );
    cr.arc(x + w - r, y + h - r, r,  0.0,              FRAC_PI_2  );
    cr.arc(x + r,     y + h - r, r,  FRAC_PI_2,        PI         );
    cr.arc(x + r,     y + r,     r,  PI,       3.0 * FRAC_PI_2    );
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outline_contrasts_with_its_fill() {
        let dark = |o: (f64, f64, f64, f64)| o.0 == 0.0;
        // Light fills get a dark hairline...
        assert!(dark(outline(gdk::RGBA::WHITE)));
        assert!(dark(outline(gdk::RGBA::new(1.0, 1.0, 0.0, 1.0))));
        assert!(dark(outline(gdk::RGBA::GREEN)));
        // ...and dark ones a light hairline. Blue is the case a plain average
        // of the channels would get wrong.
        assert!(!dark(outline(gdk::RGBA::BLACK)));
        assert!(!dark(outline(gdk::RGBA::BLUE)));
        assert!(!dark(outline(gdk::RGBA::RED)));
    }
}
