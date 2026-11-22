//! The room threads' timer resolution: the one place the server crate
//! calls the OS unsafely (`lib.rs` denies `unsafe` everywhere else).
//!
//! A room waits for its next tick with a timeout (`router.rs`
//! `wait_until`). On Windows every timed wait (the channel's park, a
//! sleep, a poll) ends at a tick of the system timer, which runs at
//! 15.625 ms unless a process asks for more: a wait shorter than that
//! sleeps until the next timer interrupt, so the 16 ms ticks started 0-16
//! ms late (5 ms on average) and bunched against the 15.625 ms timer.
//! Each world frame then reached the client at an irregular time, and its
//! interpolation delay (IND, mean + σ of the arrival intervals) read 21-27
//! ms instead of its 20 ms floor (`refs/reports/Latency.md` §4, §5). Since
//! Windows 10 2004 the resolution a process asks for applies to that
//! process only, so another program's `timeBeginPeriod` (the game's,
//! Chrome's) doesn't help the server.
//!
//! `timeBeginPeriod(1)` makes the timer tick every millisecond for the
//! whole process while it holds; the calls are counted, and each must be
//! undone by `timeEndPeriod` with the same period. Every room thread holds
//! one [`Resolution`] while it runs, so the process asks for 1 ms only
//! while a match is on. Measured with it (Latency.md §5): ticks start
//! 0.8-0.9 ms late on average (p90 1.7 ms), no skipped packet frames, IND
//! 20.0 ms. Elsewhere the OS timers are precise already: no call.
//!
//! The virtual-clock tests (`lockstep.rs`) never wait, so they don't need
//! it; only `run_room` takes one.

/// The system timer at 1 ms while it lives (Windows; nothing elsewhere).
pub(super) struct Resolution {
    #[cfg(windows)]
    held: bool,
}

/// The period asked for, in milliseconds: the finest `timeBeginPeriod`
/// gives.
#[cfg(windows)]
const PERIOD_MS: u32 = 1;

impl Resolution {
    /// Asks for the 1 ms timer. If Windows refuses (`TIMERR_NOCANDO`), the
    /// room runs on the default timer and says so.
    #[cfg_attr(windows, allow(unsafe_code))]
    pub(super) fn one_ms() -> Self {
        #[cfg(windows)]
        {
            // SAFETY: `timeBeginPeriod` takes a plain integer and touches no
            // memory of ours; any period is valid to ask for (an
            // unsupported one returns an error).
            let result = unsafe { windows_sys::Win32::Media::timeBeginPeriod(PERIOD_MS) };
            let held = result == windows_sys::Win32::Media::TIMERR_NOERROR;
            if !held {
                tracing::warn!(
                    result,
                    "[game] no 1 ms timer: ticks may start up to 16 ms late"
                );
            }
            Self { held }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }
}

impl Drop for Resolution {
    #[cfg_attr(windows, allow(unsafe_code))]
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.held {
            // SAFETY: as in `one_ms`; it undoes the one successful
            // `timeBeginPeriod(PERIOD_MS)` this value made.
            unsafe {
                windows_sys::Win32::Media::timeEndPeriod(PERIOD_MS);
            }
        }
    }
}
