//! Fill every attached screen, until a key is pressed.
//!
//! The smallest thing that uses this crate, and the shape the control
//! window will drive it in:
//!
//! ```text
//! cargo run -p whitescreen-win --example overlay -- "#00FF00" wedge
//! ```

#[cfg(not(windows))]
fn main() {
    eprintln!("this example is for Windows");
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    use std::{thread::sleep, time::Duration};
    use whitescreen_core::{Fill, Rgba, Side};
    use whitescreen_win::{KeepAwake, Overlay, OverlayEvent, monitors, pump};

    let mut args = std::env::args().skip(1);
    let color = args
        .next()
        .as_deref()
        .and_then(Rgba::parse_hex)
        .unwrap_or(Rgba::WHITE);
    let fill = match args.next().as_deref() {
        Some("wedge") => Fill::Wedge,
        Some("grid") => Fill::Grid,
        Some("noise") => Fill::Noise,
        Some("left") => Fill::Region { side: Side::Left, rest: None },
        _ => Fill::default(),
    };

    // Held for as long as the colour is up, like the idle inhibitor on the
    // other platform.
    let _awake = KeepAwake::new();

    let screens = monitors();
    let mut overlays = Vec::new();
    for screen in &screens {
        let mut overlay = Overlay::new(screen)?;
        overlay.set_color(color);
        overlay.set_fill(fill);
        overlay.show(screen)?;
        overlays.push(overlay);
    }

    // Any key or click on any of them takes them all down, which is the
    // same promise the GTK build makes.
    while pump() {
        if overlays.iter().any(|o| o.take_event() == Some(OverlayEvent::Dismissed)) {
            break;
        }
        sleep(Duration::from_millis(10));
    }
    Ok(())
}
