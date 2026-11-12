//! What White Screen knows that has nothing to do with a toolkit.
//!
//! The app is a GTK program on Linux and a Win32 program on Windows, and the
//! two draw with entirely different machinery -- GSK render nodes on one side,
//! GDI on the other. What they must not disagree about is *what* to draw: the
//! bands of a wedge, the halves of a region, the colour of a 3200 K key light.
//! Those live here, with no `gdk::RGBA` and no `HBRUSH` in sight, so that a
//! fix to the wedge's seam arithmetic cannot land on one platform only.
//!
//! Nothing in this crate has dependencies, and all of it is testable without
//! a display.

pub mod glob;
pub mod kelvin;
pub mod text;

mod fill;
mod rgba;

pub use fill::{
    Fill, GRID_PITCH, NOISE_BLOCK, NOISE_CELL, NOISE_SEED, Rect, Side, WEDGE_BANDS, next_noise_seed,
    wedge_band, wedge_level,
};
pub use rgba::Rgba;
