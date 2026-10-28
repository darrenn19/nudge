//! Fine timers on Windows while screen mirroring runs.
//!
//! Windows wakes sleeping threads every ~16 ms unless a process asks for
//! better (`timeBeginPeriod`). The real-time channel spreads each frame's
//! packets over a few milliseconds; with 16 ms wake-ups that comes out as
//! bursts, and bursts cause loss on Wi-Fi. The core can't ask (it has no
//! unsafe code), so the app does, only while a session runs: finer timers
//! cost battery.

use std::collections::HashSet;

/// Holds 1 ms timers while any session it was told about is running.
#[derive(Default)]
pub(crate) struct Fine {
    sessions: HashSet<u64>,
}

impl Fine {
    pub(crate) fn hold(&mut self, session: u64) {
        if self.sessions.insert(session) && self.sessions.len() == 1 {
            set(true);
        }
    }

    pub(crate) fn release(&mut self, session: u64) {
        if self.sessions.remove(&session) && self.sessions.is_empty() {
            set(false);
        }
    }
}

impl Drop for Fine {
    fn drop(&mut self) {
        if !self.sessions.is_empty() {
            set(false);
        }
    }
}

#[cfg(windows)]
fn set(fine: bool) {
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
        fn timeEndPeriod(period: u32) -> u32;
    }
    // SAFETY: plain calls with no pointers; each end matches a begin.
    let r = unsafe {
        if fine {
            timeBeginPeriod(1)
        } else {
            timeEndPeriod(1)
        }
    };
    log::info!("timer resolution {} (result {r})", if fine { "1 ms" } else { "default" });
}

#[cfg(not(windows))]
fn set(_fine: bool) {}
