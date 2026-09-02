
use std::cell::RefCell;

use adw::{prelude::*, subclass::prelude::*};
use gtk::{gdk, glib};

// ── ColorSurface – GPU-accelerated solid colour widget ────────────────────────

mod imp {
    use super::*;

    pub struct ColorSurface {
        pub color: RefCell<gdk::RGBA>,
    }

    impl Default for ColorSurface {
        fn default() -> Self {
            Self {
                color: RefCell::new(gdk::RGBA::WHITE),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ColorSurface {
        const NAME: &'static str = "WhiteScreenColorSurface";
        type Type       = super::ColorSurface;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ColorSurface {}

    impl WidgetImpl for ColorSurface {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // Sane minimum in both axes; the actual size is driven by the parent.
            (64, 64, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let w = self.obj();
            snapshot.append_color(
                &self.color.borrow(),
                &gtk::graphene::Rect::new(0.0, 0.0, w.width() as f32, w.height() as f32),
            );
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
    pub fn new() -> Self { glib::Object::new() }

    pub fn set_rgba(&self, rgba: gdk::RGBA) {
        if *self.imp().color.borrow() != rgba {
            *self.imp().color.borrow_mut() = rgba;
            self.queue_draw();
        }
    }
}
