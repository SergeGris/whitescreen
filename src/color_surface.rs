//! The thing that actually paints a colour, used for both the preview and
//! the overlays.
//!
//! One widget for every answer to "what covers this surface": a flat colour,
//! a colour over part of it, the moving block of noise that the stuck-pixel
//! exerciser needs, a staircase from black to the colour, and a grid of
//! hairlines. Keeping them behind one widget is what lets the preview show
//! exactly what the screens will.

use std::cell::{Cell, RefCell};

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib, graphene};

pub use whitescreen_core::{Fill, Side};
use whitescreen_core::{GRID_PITCH, NOISE_BLOCK, NOISE_CELL, WEDGE_BANDS, wedge_band, wedge_level};

/// How often the noise is re-rolled. 20 Hz is fast enough to exercise a
/// subpixel and far below the frame rate, which keeps the GPU cost flat.
const NOISE_PERIOD_US: i64 = 50_000;
/// How fast the block drifts, in pixels per second, so that a few minutes
/// covers the whole panel.
const NOISE_DRIFT: f64 = 90.0;

/// The core's rectangle as the renderer's.
fn rect_of(r: whitescreen_core::Rect) -> graphene::Rect {
    graphene::Rect::new(r.x, r.y, r.w, r.h)
}

mod imp {
    use super::*;

    pub struct ColorSurface {
        pub color: Cell<gdk::RGBA>,
        pub fill:  Cell<Fill>,

        /// Animation state for `Fill::Noise`.
        pub tick:       RefCell<Option<gtk::TickCallbackId>>,
        pub seed:       Cell<u64>,
        pub rolled_at:  Cell<i64>,
        pub block:      Cell<(f64, f64)>,
        pub drift:      Cell<(f64, f64)>,
        pub moved_at:   Cell<i64>,
    }

    impl Default for ColorSurface {
        fn default() -> Self {
            Self {
                color: Cell::new(gdk::RGBA::WHITE),
                fill:  Cell::new(Fill::default()),
                tick:      RefCell::new(None),
                seed:      Cell::new(whitescreen_core::NOISE_SEED),
                rolled_at: Cell::new(0),
                block:     Cell::new((0.0, 0.0)),
                drift:     Cell::new((NOISE_DRIFT, NOISE_DRIFT * 0.62)),
                moved_at:  Cell::new(0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColorSurface {
        const NAME: &'static str = "WhiteScreenColorSurface";
        type Type       = super::ColorSurface;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ColorSurface {
        fn constructed(&self) {
            self.parent_constructed();
            // Nothing here is text, and a screen reader announcing an unnamed
            // widget is worse than one told to skip it.
            self.obj().set_accessible_role(gtk::AccessibleRole::Presentation);
        }

        fn dispose(&self) {
            if let Some(tick) = self.tick.take() {
                tick.remove();
            }
        }
    }

    impl WidgetImpl for ColorSurface {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // Sane minimum in both axes; the actual size is driven by the parent.
            (64, 64, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let color = self.color.get();

            match self.fill.get() {
                Fill::Region { side, rest } => {
                    let (covered, remainder) = side.split(w, h);
                    snapshot.append_color(&color, &rect_of(covered));
                    // No `rest` colour means no node at all: the window is
                    // transparent there and whatever is behind shows through.
                    if let (Some(rect), Some(rgba)) = (remainder, rest) {
                        snapshot.append_color(&crate::color::from_core(rgba), &rect_of(rect));
                    }
                }
                Fill::Noise => {
                    snapshot.append_color(&color, &graphene::Rect::new(0.0, 0.0, w, h));
                    self.snapshot_noise(snapshot, w, h);
                }
                Fill::Wedge => self.snapshot_wedge(snapshot, w, h),
                Fill::Grid  => self.snapshot_grid(snapshot, w, h),
            }
        }
    }

    impl ColorSurface {
        /// The staircase, dark end first.
        fn snapshot_wedge(&self, snapshot: &gtk::Snapshot, w: f32, h: f32) {
            let color = self.color.get();
            // Two bands is the least that is still a ramp; the constant is
            // well clear of that, but the arithmetic below divides by
            // `bands - 1`.
            let bands = WEDGE_BANDS.max(2);

            for i in 0..bands {
                let (x0, x1) = wedge_band(i, bands, w);
                let level = wedge_level(i, bands);
                let band = gdk::RGBA::new(
                    color.red()   * level,
                    color.green() * level,
                    color.blue()  * level,
                    1.0,
                );
                snapshot.append_color(&band, &graphene::Rect::new(x0, 0.0, x1 - x0, h));
            }
        }

        /// The hairline grid, on black.
        fn snapshot_grid(&self, snapshot: &gtk::Snapshot, w: f32, h: f32) {
            // Black behind rather than the chosen colour: the lines are the
            // whole pattern, and anything else lit competes with them.
            snapshot.append_color(&gdk::RGBA::BLACK, &graphene::Rect::new(0.0, 0.0, w, h));

            let color = self.color.get();
            // One *device* pixel. Snapshot coordinates are logical, so on a
            // scaled output a line of width 1.0 is drawn two pixels wide --
            // which is exactly the resampling the pattern exists to reveal.
            let hair = 1.0 / self.obj().scale_factor().max(1) as f32;

            let mut x = 0.0;
            while x < w {
                snapshot.append_color(&color, &graphene::Rect::new(x, 0.0, hair, h));
                x += GRID_PITCH;
            }
            let mut y = 0.0;
            while y < h {
                snapshot.append_color(&color, &graphene::Rect::new(0.0, y, w, hair));
                y += GRID_PITCH;
            }
        }

        fn snapshot_noise(&self, snapshot: &gtk::Snapshot, w: f32, h: f32) {
            let (bx, by) = self.block.get();
            let (bx, by) = (bx as f32, by as f32);
            let side = NOISE_BLOCK.min(w).min(h);

            // One stream of numbers per frame, walked in the same order every
            // time, so the pattern is stable between the two passes GTK may
            // make over the same snapshot.
            let mut rng = self.seed.get();
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
                    let rgba = gdk::RGBA::new(
                        (bits & 0xFF) as f32 / 255.0,
                        ((bits >> 8) & 0xFF) as f32 / 255.0,
                        ((bits >> 16) & 0xFF) as f32 / 255.0,
                        1.0,
                    );
                    let cw = NOISE_CELL.min(side - x);
                    let ch = NOISE_CELL.min(side - y);
                    snapshot.append_color(&rgba, &graphene::Rect::new(bx + x, by + y, cw, ch));
                    x += NOISE_CELL;
                }
                y += NOISE_CELL;
            }
        }
    }
}

glib::wrapper! {
    pub struct ColorSurface(ObjectSubclass<imp::ColorSurface>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ColorSurface {
    fn default() -> Self { Self::new() }
}

impl ColorSurface {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn set_rgba(&self, rgba: gdk::RGBA) {
        if self.imp().color.replace(rgba) != rgba {
            self.queue_draw();
        }
    }

    pub fn set_fill(&self, fill: Fill) {
        let imp = self.imp();
        if imp.fill.replace(fill) == fill {
            return;
        }

        match fill {
            Fill::Noise => self.start_noise(),
            _           => self.stop_noise(),
        }
        self.queue_draw();
    }

    /// Drive the noise from the frame clock: re-roll the pattern on a timer
    /// of its own, and drift the block so a few minutes covers the panel.
    fn start_noise(&self) {
        if self.imp().tick.borrow().is_some() {
            return;
        }
        let id = self.add_tick_callback(|surface, clock| {
            let imp = surface.imp();
            let now = clock.frame_time();

            let elapsed = now - imp.moved_at.replace(now);
            // A first frame, or a return after the widget was hidden, has no
            // meaningful elapsed time to move by.
            if (0..1_000_000).contains(&elapsed) {
                surface.drift_block(elapsed as f64 / 1_000_000.0);
            }

            if now - imp.rolled_at.get() >= NOISE_PERIOD_US {
                imp.rolled_at.set(now);
                imp.seed.set(whitescreen_core::next_noise_seed(imp.seed.get()));
                surface.queue_draw();
            }
            glib::ControlFlow::Continue
        });
        self.imp().tick.replace(Some(id));
    }

    fn stop_noise(&self) {
        if let Some(tick) = self.imp().tick.take() {
            tick.remove();
        }
    }

    fn drift_block(&self, seconds: f64) {
        let imp = self.imp();
        let side = NOISE_BLOCK.min(self.width() as f32).min(self.height() as f32) as f64;
        let limit_x = (self.width()  as f64 - side).max(0.0);
        let limit_y = (self.height() as f64 - side).max(0.0);

        let (mut x, mut y) = imp.block.get();
        let (mut dx, mut dy) = imp.drift.get();

        x += dx * seconds;
        y += dy * seconds;

        // Bounce, so the block keeps covering fresh ground without ever
        // leaving the screen.
        if x < 0.0 || x > limit_x {
            dx = -dx;
            x = x.clamp(0.0, limit_x);
        }
        if y < 0.0 || y > limit_y {
            dy = -dy;
            y = y.clamp(0.0, limit_y);
        }

        imp.block.set((x, y));
        imp.drift.set((dx, dy));
    }
}
