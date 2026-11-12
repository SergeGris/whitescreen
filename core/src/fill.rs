//! What covers a screen, and where.
//!
//! Geometry only: no colour is painted here and no toolkit is named. Both
//! painters -- GSK render nodes on Linux, GDI on Windows -- ask these same
//! functions where the bands and the halves go, so a seam fixed in one is
//! fixed in both.

/// Which part of the surface the colour covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Side {
    #[default]
    Full,
    Left,
    Right,
    Top,
    Bottom,
}

/// A rectangle, in the surface's own coordinates.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn area(self) -> f32 {
        self.w * self.h
    }
}

impl Side {
    /// The name `--region` accepts and the settings file stores.
    pub fn as_key(self) -> &'static str {
        match self {
            Self::Full   => "full",
            Self::Left   => "left",
            Self::Right  => "right",
            Self::Top    => "top",
            Self::Bottom => "bottom",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Full, Self::Left, Self::Right, Self::Top, Self::Bottom]
            .into_iter()
            .find(|s| s.as_key() == key)
    }

    /// The covered rectangle and the rest, in that order.
    pub fn split(self, w: f32, h: f32) -> (Rect, Option<Rect>) {
        // Round the division so the two halves meet on the pixel grid rather
        // than leaving a hairline of background between them.
        let half_w = (w / 2.0).round();
        let half_h = (h / 2.0).round();
        match self {
            Self::Full   => (Rect::new(0.0, 0.0, w, h), None),
            Self::Left   => (
                Rect::new(0.0, 0.0, half_w, h),
                Some(Rect::new(half_w, 0.0, w - half_w, h)),
            ),
            Self::Right  => (
                Rect::new(half_w, 0.0, w - half_w, h),
                Some(Rect::new(0.0, 0.0, half_w, h)),
            ),
            Self::Top    => (
                Rect::new(0.0, 0.0, w, half_h),
                Some(Rect::new(0.0, half_h, w, h - half_h)),
            ),
            Self::Bottom => (
                Rect::new(0.0, half_h, w, h - half_h),
                Some(Rect::new(0.0, 0.0, w, half_h)),
            ),
        }
    }
}

/// What the surface paints.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fill {
    /// The colour over `side`. The rest takes the second colour, or is left
    /// unpainted -- which shows whatever is behind the window, and is the
    /// point of the half modes: a colour judged against the desktop beside
    /// it.
    Region { side: Side, rest: Option<super::Rgba> },
    /// A drifting block of random colour over the flat colour.
    ///
    /// The established way to unstick a subpixel is to drive it hard through
    /// many values for a few minutes. Confining that to a block is not only
    /// how the technique works -- it also keeps a full screen from flashing.
    Noise,
    /// A staircase from black to the colour, in equal vertical bands.
    ///
    /// A flat field finds a pixel that is dead; a ramp finds the faults that
    /// only exist *between* levels -- banding, a mis-set gamma, black crush
    /// at the bottom of the range.
    Wedge,
    /// Hairlines in the colour on black, one device pixel wide: whether one
    /// screen pixel is still one panel pixel.
    Grid,
}

impl Default for Fill {
    fn default() -> Self {
        Self::Region { side: Side::Full, rest: None }
    }
}

/// Bands in the wedge. Sixteen is the usual step count for a visual banding
/// check: enough that each edge is subtle, few enough that a band is wide
/// enough to judge on its own.
pub const WEDGE_BANDS: u32 = 16;

/// Spacing of the grid, in logical pixels.
pub const GRID_PITCH: f32 = 8.0;

/// Side of one noise cell, in pixels. Small enough to hit individual pixels
/// hard, large enough that a screenful is not a million draw calls.
pub const NOISE_CELL: f32 = 6.0;

/// Side of the block the noise is confined to.
pub const NOISE_BLOCK: f32 = 320.0;

/// Where the noise starts. Any non-zero value would do.
pub const NOISE_SEED: u64 = 0x2545_F491_4F6C_DD1D;

/// The next noise seed.
///
/// Shared because both painters re-roll on a timer of their own and would
/// otherwise each carry a copy of this constant. Never returns zero: the
/// painters run an xorshift from it, and xorshift of zero is zero forever --
/// a frozen block of black instead of noise.
pub fn next_noise_seed(seed: u64) -> u64 {
    // An LCG with a full period over u64, which does eventually land on
    // zero; the guard is what keeps that from stopping the animation.
    let next = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    if next == 0 { NOISE_SEED } else { next }
}

/// Where band `i` of `bands` starts and ends across a width of `w`.
///
/// Rounded to the pixel grid, and each band starts where the last one ended:
/// bands laid at exact `w / bands` intervals leave a seam of unpainted
/// background between them wherever the division is not whole.
pub fn wedge_band(i: u32, bands: u32, w: f32) -> (f32, f32) {
    let edge = |n: u32| (w * n as f32 / bands as f32).round();
    (edge(i), edge(i + 1))
}

/// How far up the ramp band `i` of `bands` is, from 0 at the dark end to 1 at
/// the bright one.
pub fn wedge_level(i: u32, bands: u32) -> f32 {
    i as f32 / (bands.max(2) - 1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths that divide the band count evenly and widths that do not, plus
    /// the awkward ones a real display can hand us.
    const WIDTHS: &[f32] = &[1600.0, 1920.0, 2560.0, 3440.0, 1366.0, 1023.0, 17.0];

    #[test]
    fn the_wedge_leaves_no_seam_and_no_overlap() {
        for &w in WIDTHS {
            let mut edge = 0.0;
            for i in 0..WEDGE_BANDS {
                let (x0, x1) = wedge_band(i, WEDGE_BANDS, w);
                assert_eq!(x0, edge, "band {i} of {w} starts away from the last one");
                assert!(x1 >= x0, "band {i} of {w} runs backwards");
                edge = x1;
            }
            assert_eq!(edge, w, "the wedge stops short of {w}");
        }
    }

    #[test]
    fn a_narrow_wedge_still_covers_its_width() {
        // Fewer pixels than bands: some come out empty, which is fine, but
        // between them they still have to cover the surface.
        assert_eq!(wedge_band(0, WEDGE_BANDS, 5.0).0, 0.0);
        assert_eq!(wedge_band(WEDGE_BANDS - 1, WEDGE_BANDS, 5.0).1, 5.0);
    }

    #[test]
    fn the_ramp_runs_from_black_to_full_output() {
        assert_eq!(wedge_level(0, WEDGE_BANDS), 0.0);
        assert_eq!(wedge_level(WEDGE_BANDS - 1, WEDGE_BANDS), 1.0);

        let mut last = -1.0;
        for i in 0..WEDGE_BANDS {
            let level = wedge_level(i, WEDGE_BANDS);
            assert!(level > last, "band {i} is no brighter than the one before");
            last = level;
        }
    }

    #[test]
    fn the_covered_part_and_the_rest_tile_the_surface() {
        // An odd size as well as an even one: half of 1023 is not a whole
        // number, and that is where a hairline gap would appear.
        for (w, h) in [(800.0_f32, 600.0_f32), (1023.0, 767.0)] {
            for side in [Side::Full, Side::Left, Side::Right, Side::Top, Side::Bottom] {
                let (covered, rest) = side.split(w, h);
                let total = covered.area() + rest.map_or(0.0, Rect::area);
                assert_eq!(total, w * h, "{side:?} at {w}x{h} does not tile the surface");
            }
        }
    }

    #[test]
    fn full_covers_everything_and_leaves_no_rest() {
        let (covered, rest) = Side::Full.split(640.0, 480.0);
        assert_eq!(covered, Rect::new(0.0, 0.0, 640.0, 480.0));
        assert!(rest.is_none(), "a full fill has no other half to colour");
    }

    #[test]
    fn the_noise_seed_never_reaches_the_value_that_would_freeze_it() {
        // xorshift of zero is zero forever, so the one value the sequence
        // must never hand back is zero.
        assert_ne!(next_noise_seed(0), 0);
        let mut seed = NOISE_SEED;
        for _ in 0..10_000 {
            seed = next_noise_seed(seed);
            assert_ne!(seed, 0);
        }
    }

    #[test]
    fn the_noise_seed_actually_moves() {
        let seed = NOISE_SEED;
        assert_ne!(next_noise_seed(seed), seed);
    }

    #[test]
    fn every_side_round_trips_through_its_key() {
        for side in [Side::Full, Side::Left, Side::Right, Side::Top, Side::Bottom] {
            assert_eq!(Side::from_key(side.as_key()), Some(side), "{side:?}");
        }
        assert_eq!(Side::from_key("middle"), None);
    }
}
