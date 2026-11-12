//! Keeping the screen on.
//!
//! The Windows counterpart of the idle inhibitor: a display used as a key
//! light or left on a test pattern is a screen nobody touches for minutes,
//! which is exactly what the power manager blanks.

use windows::Win32::System::Power::{
    ES_CONTINUOUS, ES_DISPLAY_REQUIRED, EXECUTION_STATE, SetThreadExecutionState,
};

/// Holds the display awake for as long as it is alive.
///
/// A guard rather than a pair of calls because the state is per *thread* and
/// global to it: forgetting to clear it leaves the machine unable to sleep
/// until the process exits, which is the one failure mode a user would never
/// connect back to this app.
#[derive(Debug)]
pub struct KeepAwake {
    /// False when Windows refused, so that dropping does not clear a state
    /// this guard never set.
    held: bool,
}

impl KeepAwake {
    pub fn new() -> Self {
        // ES_CONTINUOUS makes it stick until cleared, rather than counting
        // as one-off activity.
        let previous =
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED) };
        Self { held: previous != EXECUTION_STATE(0) }
    }
}

impl Default for KeepAwake {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        if self.held {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
    }
}
