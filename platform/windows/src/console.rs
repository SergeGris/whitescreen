//! Output for a program that is a window first.
//!
//! The executable is built for the `windows` subsystem, not the `console`
//! one: double-clicking a console-subsystem program makes Windows open a
//! terminal to put it in, and `whitescreen.exe` with no arguments is meant
//! to open a window, not a black rectangle with a window behind it.
//!
//! That would ordinarily cost the command line its output -- a windowed
//! program starts with nowhere to print -- so when there *is* a console
//! behind us, we join it. `--help`, `--version` and `--list-monitors` then
//! print where they were typed.
//!
//! One thing cannot be had both ways: the shell does not wait for a
//! windowed program, so the prompt comes back before the output does. The
//! alternative is a terminal flashing up on every double-click, which is
//! worse for the thing this app is mostly used for.
//!
//! The other half of this file is what to do when there is *no* console. A
//! windowed program with nowhere to print cannot report a bad argument, and
//! a panic in one is a program that simply vanishes. Both end up in a
//! message box instead, because a tool that fails silently when it is
//! double-clicked is a tool nobody can diagnose.

use std::os::windows::io::AsRawHandle;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    SetStdHandle,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
use windows::core::{PCWSTR, w};

/// Join the console we were launched from, if we were launched from one.
///
/// Does nothing when there is no console -- started from Explorer, a
/// shortcut or the Start menu -- and nothing when the shell already handed
/// its own handles over, which is the case that needs no help.
pub fn attach() {
    // Safety: both calls are process-wide and take no pointers of ours.
    // A failure means there was no parent console to attach to, or that we
    // are already attached to one; neither is an error here.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        // Attaching gives the process a console but not the handles to it:
        // the ones it started with are the windowed program's, which point
        // nowhere. `CONOUT$` is the console's own output, whatever it is.
        let Ok(out) = std::fs::OpenOptions::new().read(true).write(true).open("CONOUT$") else {
            return;
        };
        let handle = HANDLE(out.as_raw_handle());
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
        let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        // The handle now belongs to the process, and closing the file would
        // take it away again.
        std::mem::forget(out);
    }
}

/// Whether anything printed will be seen.
///
/// True with a console behind us, and true when the caller redirected the
/// output somewhere; false when the program was double-clicked, which is
/// when a message has to go somewhere else.
pub fn can_print() -> bool {
    // Safety: no pointers, and a failure is an answer rather than a problem.
    unsafe {
        GetStdHandle(STD_ERROR_HANDLE)
            .is_ok_and(|handle| !handle.is_invalid() && !handle.0.is_null())
    }
}

/// Say something in the only place a windowed program always has.
pub fn alert(body: &str) {
    let text: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    // Safety: both strings are NUL terminated and outlive the call, which
    // is modal.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("White Screen"),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

/// Make a panic visible when there is nowhere to print it.
///
/// Without this a panic in a windowed program closes it and leaves nothing
/// at all: no window, no message, no exit code anyone sees. The default hook
/// still runs first, so a console or a redirect gets the usual text and the
/// backtrace.
pub fn alert_on_panic() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        previous(panic);
        alert(&format!(
            "White Screen stopped unexpectedly.\n\n{panic}\n\n\
             This is a bug. Running it from a terminal will show more."
        ));
    }));
}
