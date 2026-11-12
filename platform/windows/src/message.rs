//! The thread's message queue.
//!
//! Every overlay is an ordinary window, so something has to pump messages
//! for them -- without it they never paint and never see a keystroke. This
//! is the primitive rather than a loop of its own, because the caller has
//! its own idea of when to stop.

use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
};

/// Handle everything Windows has queued for this thread, without blocking.
///
/// Returns false once `WM_QUIT` has been seen, which is the signal to stop.
pub fn pump() -> bool {
    let mut msg = MSG::default();
    // Safety: these are the standard message-loop calls, and `msg` is the
    // storage they are documented to fill.
    unsafe {
        while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
    true
}
