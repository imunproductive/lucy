//! The fixed game tick and packet clock, separate from socket and simulation work.
//!
//! The packet frame stays on real time, like retail's (62.5 per second, OW2
//! traffic): ticks that fell behind are skipped, not run late. The client's
//! command clock runs on real time and only jumps forward to the packet frame
//! (0x7FF7896DB296, 0x7FF789AC7140), so a lagging frame number would leave its
//! commands further ahead with every stall, past its 64 predicted states and
//! 32 kept commands.

use std::time::Duration;

use crate::connection::{HANDOFF_TIME, LEAVE_GRACE};
use lucy_core::tick::{FIRST_TICK, MAX_LEASE_FRAME, TICK_PERIOD};

const CLOCK_DRAIN_TICKS: u32 = (HANDOFF_TIME.as_millis() + LEAVE_GRACE.as_millis())
    .div_ceil(TICK_PERIOD.as_millis()) as u32
    + 1;
pub const MAX_SIMULATION_TICK: u32 = MAX_LEASE_FRAME - CLOCK_DRAIN_TICKS;

/// Skip ticks strictly behind the monotonic clock after processing the current one.
pub fn next_tick_after(due: Duration, now: Duration) -> Option<(Duration, u64)> {
    let following = due.checked_add(TICK_PERIOD)?;
    if following >= now {
        return Some((following, 0));
    }
    let late = (now - following).as_nanos();
    let period = TICK_PERIOD.as_nanos();
    let missed = u64::try_from(late / period + 1).ok()?;
    let until_next = Duration::from_nanos((period - late % period) as u64);
    Some((now.checked_add(until_next)?, missed))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    /// A simulated tick; its datagram goes to every client.
    Simulate { frame: u32 },
    /// A drain tick: no simulation; a datagram unless `force`d.
    Drain {
        frame: u32,
        entered: bool,
        force: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameClock {
    frame: u32,
    drain_ticks_left: Option<u32>,
}

impl Default for GameClock {
    fn default() -> Self {
        Self {
            frame: FIRST_TICK,
            drain_ticks_left: None,
        }
    }
}

impl GameClock {
    pub fn frame(self) -> u32 {
        self.frame
    }

    pub fn draining(self) -> bool {
        self.drain_ticks_left.is_some()
    }

    pub fn step(&mut self, missed: u64, connections_pending: bool) -> Step {
        let candidate = u64::from(self.frame)
            .saturating_add(missed)
            .saturating_add(1);
        if self.drain_ticks_left.is_none() && candidate <= u64::from(MAX_SIMULATION_TICK) {
            self.frame = candidate as u32;
            return Step::Simulate { frame: self.frame };
        }

        let entered = self.drain_ticks_left.is_none();
        let ticks = self.drain_ticks_left.get_or_insert_with(|| {
            CLOCK_DRAIN_TICKS.min(MAX_LEASE_FRAME.saturating_sub(self.frame))
        });
        let force = *ticks == 0;
        if !force && connections_pending {
            *ticks -= 1;
            self.frame += 1;
        }
        Step::Drain {
            frame: self.frame,
            entered,
            force,
        }
    }

    /// Reset only when the old connections and their lobby notifications are finished.
    pub fn renew_if_idle(&mut self, connections_pending: bool, reports_pending: bool) -> bool {
        if !self.draining() || connections_pending || reports_pending {
            return false;
        }
        *self = Self::default();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_tick_alignment_across_a_stall() {
        let ms = Duration::from_millis;
        assert_eq!(
            next_tick_after(ms(10_000), ms(10_001)),
            Some((ms(10_016), 0))
        );
        assert_eq!(
            next_tick_after(ms(10_000), ms(10_016)),
            Some((ms(10_016), 0))
        );
        assert_eq!(
            next_tick_after(ms(10_000), ms(10_050)),
            Some((ms(10_064), 3))
        );
        assert_eq!(
            next_tick_after(ms(10_000), ms(10_048)),
            Some((ms(10_064), 3))
        );
    }

    #[test]
    fn skips_the_missed_ticks_and_sends_once_a_step() {
        // Each step runs frame + missed + 1, and each simulated step is one
        // datagram: from 1000, missing 0, 3 and 0 ticks runs (and sends)
        // 1001, 1001 + 3 + 1 = 1005 and 1006.
        let mut clock = GameClock::default();
        let frames: Vec<_> = [0, 3, 0]
            .into_iter()
            .map(|missed| match clock.step(missed, true) {
                Step::Simulate { frame } => frame,
                step => panic!("{step:?}"),
            })
            .collect();
        assert_eq!(frames, [FIRST_TICK + 1, FIRST_TICK + 5, FIRST_TICK + 6]);
    }

    #[test]
    fn drains_one_tick_at_a_time_and_waits_for_the_last_report() {
        let mut clock = GameClock {
            frame: MAX_LEASE_FRAME - 2,
            ..Default::default()
        };
        assert_eq!(
            clock.step(500, true),
            Step::Drain {
                frame: MAX_LEASE_FRAME - 1,
                entered: true,
                force: false,
            }
        );
        assert_eq!(
            clock.step(0, true),
            Step::Drain {
                frame: MAX_LEASE_FRAME,
                entered: false,
                force: false,
            }
        );
        assert_eq!(
            clock.step(100, true),
            Step::Drain {
                frame: MAX_LEASE_FRAME,
                entered: false,
                force: true,
            }
        );
        assert!(!clock.renew_if_idle(true, false));
        assert!(!clock.renew_if_idle(false, true));
        assert!(clock.renew_if_idle(false, false));
        assert_eq!(clock, GameClock::default());
    }
}
