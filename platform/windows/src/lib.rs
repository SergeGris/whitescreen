//! The native Windows front end: the control window and the overlays.
//!
//! Windows has no `wlr-layer-shell`, and it does not need one. A borderless
//! `WS_POPUP` window sized to a monitor and kept on `HWND_TOPMOST` is exactly
//! the surface this app wants, and it is an ordinary window: no compositor
//! protocol, no fallback path, no banner telling the user what they cannot
//! have.
//!
//! What is *not* reimplemented here is what to draw. The bands of a wedge,
//! the halves of a region, the colour of a 3200 K key light and the rule for
//! matching `--monitor` patterns all come from `whitescreen_core`, the same
//! crate the GTK build uses, so the two front ends cannot drift apart.
//!
//! # Scope
//!
//! Both halves of the app are here: the overlays, which have to be native
//! because there is no cross-platform way to own a screen, and the control
//! window, which is native because a Windows app should look like one. The
//! window's Fluent palette, type ramp and window chrome live in `theme`, and
//! `window` draws against them.
//!
//! Everything that touches Win32 compiles only on Windows; on any other host
//! those parts are simply absent, so building the workspace on Linux neither
//! needs the Windows bindings nor pretends to offer them. The command line
//! is not among them -- parsing one needs no Win32, and leaving it out here
//! means it is compiled and tested everywhere rather than on the one
//! platform that can run it.

pub mod cli;
pub mod config;

#[cfg(windows)]
mod badge;
#[cfg(windows)]
mod icon;
#[cfg(windows)]
mod message;
#[cfg(windows)]
mod monitor;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod paint;
#[cfg(windows)]
mod power;
#[cfg(windows)]
mod session;
#[cfg(windows)]
mod theme;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use badge::Badge;
#[cfg(windows)]
pub use message::{pump, pump_dialog};
#[cfg(windows)]
pub use monitor::{Monitor, monitors, select_matching};
#[cfg(windows)]
pub use overlay::{Overlay, OverlayEvent};
#[cfg(windows)]
pub use power::KeepAwake;
#[cfg(windows)]
pub use session::Session;
#[cfg(windows)]
pub use window::ControlWindow;

/// Printed by `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
