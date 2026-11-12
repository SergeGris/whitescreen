//! Colour temperature, for using a panel as a key light.
//!
//! A display pressed into service as a soft light has to match the other
//! lights in the room, and those are sold by temperature: a tungsten fixture
//! is 3200 K, a daylight-balanced LED 5600 K. Reaching that through the
//! colour picker means knowing the RGB triple for a temperature, which is
//! exactly the thing nobody knows offhand.
//!
//! The conversion is Tanner Helland's piecewise approximation of the
//! Planckian locus in sRGB. It is an approximation -- the real thing needs a
//! chromaticity table and the panel's own primaries -- but it is the one
//! every lighting tool uses, it is smooth, and it is correct to within a
//! couple of per cent across the range anyone lights with.
//!
//! The brightest channel always comes out at full scale, which is what a key
//! light wants: dimming is the brightness control's job, not this one's.

use crate::Rgba;

/// Range the approximation is defined over.
const MIN_K: f64 = 1_000.0;
const MAX_K: f64 = 40_000.0;

/// Temperatures worth offering as presets.
///
/// Tungsten, a warm LED panel, daylight-balanced studio light, and the
/// nominal white point of sRGB itself -- the four a camera's white-balance
/// presets are named after. Shared so that both front ends offer the same
/// set; what each one is *called* is UI text, and belongs with the UI.
pub const PRESETS: &[u32] = &[3_200, 4_300, 5_600, 6_500];

/// The colour of a black body at `kelvin`, at full output.
///
/// Out-of-range temperatures are clamped rather than refused: this is fed by
/// a command-line number, and the nearest colour it can show is a better
/// answer than an error for a value no display could render anyway.
pub fn rgba(kelvin: u32) -> Rgba {
    let t = f64::from(kelvin).clamp(MIN_K, MAX_K) / 100.0;

    let (r, g, b) = if t <= 66.0 {
        (
            255.0,
            99.470_802_586_1 * t.ln() - 161.119_568_166_1,
            // Below 1900 K there is no blue output at all, and ln() of a
            // non-positive number would not be a number.
            if t <= 19.0 {
                0.0
            } else {
                138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
            },
        )
    } else {
        (
            329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2),
            288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2),
            255.0,
        )
    };

    Rgba::from_f32(channel(r), channel(g), channel(b))
}

fn channel(v: f64) -> f32 {
    (v / 255.0).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset, plus the ends and the seam between the two branches of
    /// the approximation.
    fn sample() -> Vec<u32> {
        let mut v: Vec<u32> = PRESETS.to_vec();
        v.extend([1_000, 1_900, 6_600, 6_700, 10_000, 40_000]);
        v
    }

    #[test]
    fn every_channel_stays_in_range() {
        for k in sample().into_iter().chain([0, 1, u32::MAX]) {
            let c = rgba(k);
            let (r, g, b) = c.to_f32();
            for (name, v) in [("red", r), ("green", g), ("blue", b)] {
                assert!(v.is_finite(), "{k} K: {name} is not a number");
                assert!((0.0..=1.0).contains(&v), "{k} K: {name} out of range at {v}");
            }
        }
    }

    #[test]
    fn a_key_light_is_always_at_full_output() {
        // Something has to be at full scale, or the chip would be dimming the
        // screen as a side effect of choosing a temperature.
        for k in sample() {
            let c = rgba(k);
            let (r, g, b) = c.to_f32();
            let max = r.max(g).max(b);
            assert!((max - 1.0).abs() < 1e-6, "{k} K peaks at {max}, not full scale");
        }
    }

    #[test]
    fn warm_is_red_and_cool_is_blue() {
        let warm = rgba(3_200);
        assert!(warm.r > warm.g && warm.g > warm.b, "{warm:?}");

        let cool = rgba(10_000);
        assert!(cool.b > cool.g && cool.g > cool.r, "{cool:?}");
    }

    #[test]
    fn blue_only_ever_rises_with_temperature() {
        // The property the whole scale rests on, and the one the seam at
        // 6600 K could break.
        let mut last = 0_u8;
        for k in (1_000..=20_000).step_by(100) {
            let blue = rgba(k).b;
            assert!(blue >= last, "{k} K: blue fell from {last} to {blue}");
            last = blue;
        }
    }

    #[test]
    fn d65_is_close_to_white() {
        // sRGB's own white point: a visibly tinted result here would mean the
        // approximation had been typed in wrong.
        let c = rgba(6_500);
        let (r, g, b) = c.to_f32();
        for v in [r, g, b] {
            assert!(v > 0.93, "D65 came out tinted: {c:?}");
        }
    }

    #[test]
    fn out_of_range_temperatures_clamp_to_the_ends() {
        assert_eq!(rgba(0), rgba(1_000));
        assert_eq!(rgba(u32::MAX), rgba(40_000));
        // The warm end has no blue output at all.
        assert_eq!(rgba(1_000).b, 0);
    }
}
