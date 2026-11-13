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

use std::os::windows::io::AsRawHandle;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};

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
