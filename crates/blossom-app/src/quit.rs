//! The last chance to save. Closing the window goes through the app (it
//! writes waiting saves, then exits), but Quit in macOS's app menu (⌘Q)
//! ends the process from the system's side without telling the app. A C
//! `atexit` hook still runs then: it asks the worker to write every save
//! it is holding (typing waits [`crate::worker::SAVE_DELAY`] before it is
//! written) and waits for that, a few seconds at most.

// The one place with unsafe code: declaring and calling C's `atexit`.
#![allow(unsafe_code)]

use std::sync::Mutex;
use std::time::Duration;

use crate::worker::Handle;

/// How long the exit hook waits for the worker.
const WAIT: Duration = Duration::from_secs(3);

/// The running worker, for the exit hook.
static WORKER: Mutex<Option<Handle>> = Mutex::new(None);

/// Tells the exit hook which worker holds the saves.
pub fn set_worker(handle: &Handle) {
    if let Ok(mut w) = WORKER.lock() {
        *w = Some(handle.clone());
    }
}

/// Writes the worker's waiting saves now; whether it confirmed.
pub fn flush() -> bool {
    let handle = WORKER.lock().ok().and_then(|w| w.clone());
    handle.is_some_and(|h| h.flush_now(WAIT))
}

#[cfg(unix)]
unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

#[cfg(unix)]
extern "C" fn at_exit() {
    // Nothing here may panic: unwinding out of a C callback aborts.
    let _ = std::panic::catch_unwind(flush);
}

/// Registers the exit hook (once, at start).
pub fn install() {
    #[cfg(unix)]
    // SAFETY: `atexit` only stores the pointer; `at_exit` is a plain
    // `extern "C" fn` that never unwinds.
    unsafe {
        atexit(at_exit);
    }
}
