//! The thread's message queue.
//!
//! Every overlay is an ordinary window, so something has to pump messages
//! for them -- without it they never paint and never see a keystroke. This
//! is the primitive rather than a loop of its own, because the caller has
//! its own idea of when to stop.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, IsDialogMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
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

/// The same, for a window with controls on it.
///
/// `IsDialogMessageW` is what turns a window full of child controls into one
/// a keyboard can use: Tab and Shift+Tab walk the controls, the arrow keys
/// move inside a group, Space presses what has focus, and Enter and Escape
/// arrive as `IDOK` and `IDCANCEL`. A plain `TranslateMessage` loop gives
/// none of that, and an app without it is one a Windows user would call
/// broken.
pub fn pump_dialog(dialog: HWND) -> bool {
    let mut msg = MSG::default();
    // Safety: as `pump`, with the keyboard pre-empted by the dialog manager
    // -- which consumes a message outright when it handles it, and must not
    // then see it dispatched a second time.
    unsafe {
        while PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            if IsDialogMessageW(dialog, &raw mut msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
    true
}
