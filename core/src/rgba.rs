//! A colour, as the panel will actually show it.
//!
//! Eight bits a channel and no alpha, because that is what the overlays are:
//! opaque, and quantised to the panel either way. The toolkits deal in their
//! own colour types -- four floats on one side, a packed `COLORREF` on the
//! other -- and both convert at the edge.
//!
//! Being bytes is also what makes two colours comparable. Comparing floats
//! would call a colour picked from the dialog different from the constant it
//! visually matches, which is the bug this type exists to make impossible.

/// An opaque 8-bit colour.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgba {
    pub const BLACK: Self = Self::new(0, 0, 0);
    pub const WHITE: Self = Self::new(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// From the floating-point channels a toolkit hands over. Out-of-range
    /// values are clamped rather than wrapped: a colour slightly outside the
    /// gamut is a rounding error, not a request for a different hue.
    pub fn from_f32(r: f32, g: f32, b: f32) -> Self {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::new(byte(r), byte(g), byte(b))
    }

    /// The channels back as floats in 0..=1.
    pub fn to_f32(self) -> (f32, f32, f32) {
        (
            f32::from(self.r) / 255.0,
            f32::from(self.g) / 255.0,
            f32::from(self.b) / 255.0,
        )
    }

    /// `#RRGGBB`. What the tooltips show, and what the settings file stores:
    /// hex rather than `rgb(...)` keeps the file legible to a human editing
    /// it.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Read `#RGB` or `#RRGGBB` back. Names and `rgb()` are the toolkit's
    /// business; this only has to read what `hex()` wrote.
    pub fn parse_hex(s: &str) -> Option<Self> {
        let digits = s.strip_prefix('#')?;
        let pair = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
        match digits.len() {
            6 => Some(Self::new(pair(0)?, pair(2)?, pair(4)?)),
            // The short form doubles each digit, so #abc is #aabbcc.
            3 => {
                let one = |i: usize| u8::from_str_radix(&digits[i..i + 1], 16).ok().map(|v| v * 17);
                Some(Self::new(one(0)?, one(1)?, one(2)?))
            }
            _ => None,
        }
    }

    /// The names the app's own chips and default queue use.
    ///
    /// Not a CSS colour table: the GTK build hands the string to GDK, which
    /// knows every CSS name there is, and this is the subset a front end
    /// without a toolkit can promise. Anything else has to be hex.
    pub const NAMES: &'static [(&'static str, Self)] = &[
        ("white",   Self::new(255, 255, 255)),
        ("black",   Self::new(0, 0, 0)),
        ("red",     Self::new(255, 0, 0)),
        ("green",   Self::new(0, 255, 0)),
        ("blue",    Self::new(0, 0, 255)),
        ("cyan",    Self::new(0, 255, 255)),
        ("magenta", Self::new(255, 0, 255)),
        ("yellow",  Self::new(255, 255, 0)),
        ("grey",    Self::new(128, 128, 128)),
        ("gray",    Self::new(128, 128, 128)),
    ];

    /// A colour as a person would type it: `#RGB`, `#RRGGBB`, or one of
    /// `NAMES`. Case and surrounding space are ignored.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(c) = Self::parse_hex(s) {
            return Some(c);
        }
        // Hex without the hash is what people type half the time.
        if let Some(c) = Self::parse_hex(&format!("#{s}")) {
            return Some(c);
        }
        let lower = s.to_ascii_lowercase();
        Self::NAMES.iter().find(|(name, _)| *name == lower).map(|&(_, c)| c)
    }

    /// The colour at `fraction` of full output.
    ///
    /// Scaling the three channels together moves the level without moving the
    /// hue, which is what a brightness control has to do.
    pub fn scaled(self, fraction: f32) -> Self {
        let k = fraction.clamp(0.0, 1.0);
        let ch = |v: u8| (f32::from(v) * k).round() as u8;
        Self::new(ch(self.r), ch(self.g), ch(self.b))
    }

    /// Rec. 709 luma, 0..=1.
    ///
    /// Green carries most of the perceived brightness, so a plain average of
    /// the channels would call pure blue "light".
    pub fn luma(self) -> f32 {
        let (r, g, b) = self.to_f32();
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_is_uppercase_and_six_digits() {
        assert_eq!(Rgba::WHITE.hex(), "#FFFFFF");
        assert_eq!(Rgba::BLACK.hex(), "#000000");
        assert_eq!(Rgba::new(0, 255, 255).hex(), "#00FFFF");
    }

    #[test]
    fn hex_round_trips() {
        for c in [Rgba::WHITE, Rgba::BLACK, Rgba::new(18, 52, 86), Rgba::new(1, 2, 3)] {
            assert_eq!(Rgba::parse_hex(&c.hex()), Some(c), "{c:?}");
        }
    }

    #[test]
    fn the_short_hex_form_doubles_each_digit() {
        assert_eq!(Rgba::parse_hex("#abc"), Some(Rgba::new(0xAA, 0xBB, 0xCC)));
        assert_eq!(Rgba::parse_hex("#fff"), Some(Rgba::WHITE));
    }

    #[test]
    fn nonsense_hex_is_rejected_rather_than_guessed() {
        for s in ["", "#", "fff", "#ff", "#fffff", "#gggggg", "#1234567"] {
            assert_eq!(Rgba::parse_hex(s), None, "{s:?} should not parse");
        }
    }

    #[test]
    fn colours_can_be_typed_by_name_or_as_hex() {
        assert_eq!(Rgba::parse("#FF0000"), Some(Rgba::new(255, 0, 0)));
        assert_eq!(Rgba::parse("red"), Some(Rgba::new(255, 0, 0)));
        assert_eq!(Rgba::parse("  RED  "), Some(Rgba::new(255, 0, 0)));
        assert_eq!(Rgba::parse("#abc"), Some(Rgba::new(0xAA, 0xBB, 0xCC)));
        // The hash is what people forget.
        assert_eq!(Rgba::parse("00FF00"), Some(Rgba::new(0, 255, 0)));
    }

    #[test]
    fn a_colour_nobody_could_mean_is_refused() {
        for s in ["", "octarine", "#ff00", "rgb(1,2,3)"] {
            assert_eq!(Rgba::parse(s), None, "{s:?} should not parse");
        }
    }

    #[test]
    fn green_is_the_chip_and_not_the_css_colour() {
        // Worth pinning: CSS `green` is #008000, and the app's Green chip is
        // full green. The names here follow the chips.
        assert_eq!(Rgba::parse("green"), Some(Rgba::new(0, 255, 0)));
    }

    #[test]
    fn floats_off_the_byte_grid_land_on_the_same_colour() {
        // What comparing a picked colour against a constant used to get
        // wrong: half of 255 is not a whole number.
        let picked = Rgba::from_f32(0.5, 0.25, 0.75);
        let shown = Rgba::parse_hex(&picked.hex()).unwrap();
        assert_eq!(picked, shown);
    }

    #[test]
    fn out_of_gamut_floats_are_clamped() {
        assert_eq!(Rgba::from_f32(-1.0, 0.5, 2.0), Rgba::new(0, 128, 255));
    }

    #[test]
    fn scaling_dims_without_moving_the_hue() {
        let half = Rgba::new(255, 128, 0).scaled(0.5);
        assert_eq!(half, Rgba::new(128, 64, 0));
        assert_eq!(Rgba::WHITE.scaled(1.0), Rgba::WHITE);
        assert_eq!(Rgba::WHITE.scaled(0.0), Rgba::BLACK);
        // A fraction outside the slider's range cannot brighten past full.
        assert_eq!(Rgba::WHITE.scaled(4.0), Rgba::WHITE);
    }

    #[test]
    fn luma_weights_green_most() {
        assert!(Rgba::new(0, 255, 0).luma() > Rgba::new(255, 0, 0).luma());
        assert!(Rgba::new(255, 0, 0).luma() > Rgba::new(0, 0, 255).luma());
        assert_eq!(Rgba::WHITE.luma(), 1.0);
        assert_eq!(Rgba::BLACK.luma(), 0.0);
    }
}
