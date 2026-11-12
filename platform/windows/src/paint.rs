//! Painting a fill with GDI.
//!
//! Plain GDI rather than Direct2D: everything here is axis-aligned
//! rectangles in flat colour, which is the one thing GDI is still good at,
//! and it saves the overlay a device-context dance and a dependency.
//!
//! The *geometry* is not decided here. Where the bands of a wedge fall and
//! which half a region covers come from `whitescreen_core`, so this file and
//! the GTK renderer cannot disagree about them.

use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, FillRect, HBRUSH, HDC,
};

use whitescreen_core::{
    Fill, GRID_PITCH, NOISE_BLOCK, NOISE_CELL, Rgba, WEDGE_BANDS, wedge_band, wedge_level,
};

/// The colour a region leaves uncovered when no second colour was chosen.
///
/// Wayland lets the overlay simply not paint there and show the desktop
/// through; a plain Win32 window has no such hole, so the uncovered half is
/// colour-keyed out instead. The key is only in force while that fill is
/// showing -- see `Overlay::apply_transparency`, which is what keeps it from
/// punching a hole through a noise cell that happens to land on this value.
pub const TRANSPARENT_KEY: Rgba = Rgba::new(1, 0, 1);

/// GDI wants blue in the high byte.
pub fn colorref(c: Rgba) -> COLORREF {
    COLORREF(u32::from(c.r) | (u32::from(c.g) << 8) | (u32::from(c.b) << 16))
}

/// Paint `fill` in `color` over the whole of `bounds`.
///
/// `scale` is the monitor's DPI scale: lengths the app specifies in logical
/// pixels -- the grid's pitch, the noise cell -- are multiplied by it so they
/// come out the same physical size as on a 96 dpi screen. The grid's *lines*
/// deliberately do not scale; see below.
///
/// `seed` drives the noise, and is the caller's to advance.
pub fn paint(dc: HDC, bounds: RECT, color: Rgba, fill: Fill, scale: f32, seed: u64) {
    let w = (bounds.right - bounds.left) as f32;
    let h = (bounds.bottom - bounds.top) as f32;

    match fill {
        Fill::Region { side, rest } => {
            let (covered, remainder) = side.split(w, h);
            solid(dc, offset(bounds, covered), color);
            if let Some(rect) = remainder {
                // No second colour means the key colour, which the layered
                // window turns back into a hole.
                solid(dc, offset(bounds, rect), rest.unwrap_or(TRANSPARENT_KEY));
            }
        }
        Fill::Wedge => {
            for i in 0..WEDGE_BANDS {
                let (x0, x1) = wedge_band(i, WEDGE_BANDS, w);
                let band = color.scaled(wedge_level(i, WEDGE_BANDS));
                solid(
                    dc,
                    RECT {
                        left: bounds.left + x0 as i32,
                        top: bounds.top,
                        right: bounds.left + x1 as i32,
                        bottom: bounds.bottom,
                    },
                    band,
                );
            }
        }
        Fill::Grid => {
            // Black behind rather than the chosen colour: the lines are the
            // whole pattern, and anything else lit competes with them.
            solid(dc, bounds, Rgba::BLACK);

            // One *physical* pixel, whatever the scale. A line that scaled
            // with the display would be exactly the resampling the pattern
            // exists to reveal.
            let pitch = (GRID_PITCH * scale).max(2.0) as i32;
            let mut x = bounds.left;
            while x < bounds.right {
                solid(dc, RECT { left: x, top: bounds.top, right: x + 1, bottom: bounds.bottom }, color);
                x += pitch;
            }
            let mut y = bounds.top;
            while y < bounds.bottom {
                solid(dc, RECT { left: bounds.left, top: y, right: bounds.right, bottom: y + 1 }, color);
                y += pitch;
            }
        }
        Fill::Noise => {
            solid(dc, bounds, color);
            noise(dc, bounds, scale, seed);
        }
    }
}

/// The drifting block of random colour.
///
/// Confined to a block, as on the other platform: driving a subpixel hard
/// through many values is the technique, and the block is what keeps a whole
/// screen from flashing while it happens.
fn noise(dc: HDC, bounds: RECT, scale: f32, seed: u64) {
    let w = (bounds.right - bounds.left) as f32;
    let h = (bounds.bottom - bounds.top) as f32;
    let side = (NOISE_BLOCK * scale).min(w).min(h);
    let cell = (NOISE_CELL * scale).max(1.0);

    // Where the block sits is derived from the seed too, so that advancing
    // the seed drifts it across the panel without any state of its own.
    let span_x = (w - side).max(0.0);
    let span_y = (h - side).max(0.0);
    let (ox, oy) = drift(seed, span_x, span_y);

    // next_noise_seed() guarantees non-zero, which xorshift requires.
    let mut rng = seed;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };

    let mut y = 0.0;
    while y < side {
        let mut x = 0.0;
        while x < side {
            let bits = next();
            let c = Rgba::new(bits as u8, (bits >> 8) as u8, (bits >> 16) as u8);
            let left = bounds.left + (ox + x) as i32;
            let top = bounds.top + (oy + y) as i32;
            solid(
                dc,
                RECT {
                    left,
                    top,
                    right: left + cell.min(side - x) as i32,
                    bottom: top + cell.min(side - y) as i32,
                },
                c,
            );
            x += cell;
        }
        y += cell;
    }
}

/// Where the noise block sits for this seed: a slow triangle wave across
/// each axis, so it bounces rather than jumping about.
fn drift(seed: u64, span_x: f32, span_y: f32) -> (f32, f32) {
    let wave = |period: u64, span: f32| {
        if span <= 0.0 {
            return 0.0;
        }
        let phase = (seed % period) as f32 / period as f32;
        // 0 → 1 → 0 across the period.
        let t = if phase < 0.5 { phase * 2.0 } else { (1.0 - phase) * 2.0 };
        t * span
    };
    (wave(211, span_x), wave(307, span_y))
}

/// A rectangle from the core's surface coordinates into the window's.
fn offset(bounds: RECT, r: whitescreen_core::Rect) -> RECT {
    RECT {
        left: bounds.left + r.x as i32,
        top: bounds.top + r.y as i32,
        right: bounds.left + (r.x + r.w) as i32,
        bottom: bounds.top + (r.y + r.h) as i32,
    }
}

fn solid(dc: HDC, rect: RECT, color: Rgba) {
    unsafe {
        let brush: HBRUSH = CreateSolidBrush(colorref(color));
        FillRect(dc, &raw const rect, brush);
        let _ = DeleteObject(brush.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_puts_blue_in_the_high_byte() {
        // What GDI expects, and the opposite way round from the hex a user
        // types: #FF0000 is red, and red belongs in the low byte.
        assert_eq!(colorref(Rgba::new(0xFF, 0, 0)).0, 0x0000_00FF);
        assert_eq!(colorref(Rgba::new(0, 0, 0xFF)).0, 0x00FF_0000);
        assert_eq!(colorref(Rgba::new(0x12, 0x34, 0x56)).0, 0x0056_3412);
    }

    #[test]
    fn the_region_rectangles_are_placed_inside_the_window() {
        let bounds = RECT { left: 100, top: 50, right: 1700, bottom: 1050 };
        let (covered, rest) = whitescreen_core::Side::Left.split(1600.0, 1000.0);

        let c = offset(bounds, covered);
        assert_eq!((c.left, c.top, c.right, c.bottom), (100, 50, 900, 1050));

        let r = offset(bounds, rest.unwrap());
        assert_eq!((r.left, r.right), (900, 1700), "the halves have to meet exactly");
    }

    #[test]
    fn the_noise_block_stays_on_the_panel() {
        for seed in [1_u64, 7, 1000, u64::MAX / 3] {
            let (x, y) = drift(seed, 400.0, 300.0);
            assert!((0.0..=400.0).contains(&x), "seed {seed} drifted off to {x}");
            assert!((0.0..=300.0).contains(&y), "seed {seed} drifted off to {y}");
        }
        // A panel smaller than the block has nowhere to drift to.
        assert_eq!(drift(99, 0.0, 0.0), (0.0, 0.0));
    }
}
