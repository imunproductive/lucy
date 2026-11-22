//! The fixed game tick and packet clock, separate from socket and simulation work.
//!
//! The packet frame stays on real time, like retail's (62.5 per second, OW2
//! traffic): ticks that fell behind are skipped, not run late. The client's
//! command clock runs on real time and only jumps forward to the packet frame
//! (0x7FF7896DB296, 0x7FF789AC7140), so a lagging frame number would leave its
//! commands further ahead with every stall, past its 64 predicted states and
//! 32 kept commands.
//!
//! Each room has its own clock ([`GameClock::opened`]) on the service's tick
//! grid: a tick every [`TICK_PERIOD`] from the moment the service started,
//! the one at time 0 numbered `FIRST_TICK + 1`. So the rooms tick at the same
//! moments and number a moment alike, whichever thread runs them.

use std::time::Duration;

use crate::connection::{HANDOFF_TIME, LEAVE_GRACE};
use lucy_core::tick::{FIRST_TICK, MAX_LEASE_FRAME, TICK_PERIOD};

const CLOCK_DRAIN_TICKS: u32 = (HANDOFF_TIME.as_millis() + LEAVE_GRACE.as_millis())
    .div_ceil(TICK_PERIOD.as_millis()) as u32
    + 1;
pub const MAX_SIMULATION_TICK: u32 = MAX_LEASE_FRAME - CLOCK_DRAIN_TICKS;
/// The grid's frame numbers start again past this many ticks, so a room that
/// opens has at least half the frame range before it drains (about a year).
const GRID_WRAP: u32 = (MAX_SIMULATION_TICK - FIRST_TICK) / 2;

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
    /// The clock of a room that opens at `now` (since the service started),
    /// and when its first tick is due: the grid's next. Until then it reads
    /// the frame of the grid's last tick, as if the room had run it.
    pub fn opened(now: Duration) -> (Self, Duration) {
        let period = TICK_PERIOD.as_nanos();
        let ticks = now.as_nanos() / period;
        let frame = FIRST_TICK + 1 + (ticks % u128::from(GRID_WRAP)) as u32;
        let due = u64::try_from((ticks + 1) * period).map_or(Duration::MAX, Duration::from_nanos);
        let clock = Self {
            frame,
            drain_ticks_left: None,
        };
        (clock, due)
    }

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

    /// The room stops simulating: its next ticks drain, as at the end of the
    /// frame range (a room whose match failed sends its players home).
    pub fn drain(&mut self) {
        self.drain_ticks_left.get_or_insert_with(|| {
            CLOCK_DRAIN_TICKS.min(MAX_LEASE_FRAME.saturating_sub(self.frame))
        });
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
    fn a_room_opens_on_the_services_tick_grid() {
        // The grid's tick at time 0 is FIRST_TICK + 1, the one at 80 ms (the
        // fifth after it) FIRST_TICK + 6: a room that opens between two
        // ticks reads the last one's frame, and its first tick is the next.
        let ms = Duration::from_millis;
        let opened = |now| {
            let (clock, due) = GameClock::opened(now);
            (clock.frame(), due)
        };
        assert_eq!(opened(ms(0)), (FIRST_TICK + 1, ms(16)));
        assert_eq!(opened(ms(80)), (FIRST_TICK + 6, ms(96)));
        assert_eq!(opened(ms(95)), (FIRST_TICK + 6, ms(96)));
        let (mut clock, _) = GameClock::opened(ms(80));
        assert_eq!(
            clock.step(0, true),
            Step::Simulate {
                frame: FIRST_TICK + 7
            }
        );
        // A failed room drains from its next tick on, without the entry
        // a drain at the end of the frame range makes.
        clock.drain();
        assert_eq!(
            clock.step(0, true),
            Step::Drain {
                frame: FIRST_TICK + 8,
                entered: false,
                force: false,
            }
        );
    }

    #[test]
    fn drains_one_tick_at_a_time_until_its_connections_are_gone() {
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
    }
}
