//! Where a thread's time goes, by phase of the game server's tick.
//!
//! Each thread keeps a running total of the time it spent in each
//! [`Phase`]. The code that does a phase's work marks it with [`enter`]:
//! until the guard drops, the thread's time counts for that phase, and the
//! phase it interrupted resumes after. So each total is exclusive: the
//! statescript a body's movement runs counts as [`Phase::Script`], not
//! also as [`Phase::Simulate`]. A room's loop reads and clears the totals
//! around each of its passes ([`take`]); each room runs on a thread of its
//! own, and the tests' single thread runs one room's pass at a time.
//!
//! A mark costs a read of the monotonic clock (`Instant::now`, tens of
//! nanoseconds) when its phase differs from the current one, and nothing
//! when the thread is already in it.

use std::{cell::Cell, time::Instant};

/// A part of a tick's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Outside every marked phase.
    Other,
    /// A datagram's link and ingress: its acknowledgements, its reliable
    /// messages and the commands it carries, queued.
    Receive,
    /// The match's own work: movement, combat, the bots, the match flow.
    Simulate,
    /// The AI heroes' and dummy bots' decisions (`ai.rs`): what they see,
    /// their paths and their commands.
    Ai,
    /// The statescript VM, with the natives it calls.
    Script,
    /// The Workshop rules.
    Workshop,
    /// What each client is sent: entity frames, movement records,
    /// reliable messages, the link's egress.
    Replicate,
    /// The link's AES-GCM, both ways.
    Crypto,
    /// The socket's sends.
    Send,
}

impl Phase {
    /// Every phase, in the order of [`Phase::index`].
    pub const ALL: [Self; PHASES] = [
        Self::Other,
        Self::Receive,
        Self::Simulate,
        Self::Ai,
        Self::Script,
        Self::Workshop,
        Self::Replicate,
        Self::Crypto,
        Self::Send,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Other => "other",
            Self::Receive => "receive",
            Self::Simulate => "simulate",
            Self::Ai => "ai",
            Self::Script => "script",
            Self::Workshop => "workshop",
            Self::Replicate => "replicate",
            Self::Crypto => "crypto",
            Self::Send => "send",
        }
    }
}

/// How many phases there are.
pub const PHASES: usize = 9;

/// Nanoseconds spent in each phase, by [`Phase::index`].
pub type PhaseTimes = [u64; PHASES];

thread_local! {
    static CURRENT: Cell<Phase> = const { Cell::new(Phase::Other) };
    static SINCE: Cell<Option<Instant>> = const { Cell::new(None) };
    static TOTALS: Cell<PhaseTimes> = const { Cell::new([0; PHASES]) };
}

/// The thread's time counts for `phase` until the guard drops; then for
/// the phase it was in before.
#[must_use = "the phase ends when the guard drops"]
pub fn enter(phase: Phase) -> Guard {
    let previous = CURRENT.get();
    if previous == phase {
        return Guard { previous: None };
    }
    charge(Instant::now());
    CURRENT.set(phase);
    Guard {
        previous: Some(previous),
    }
}

/// The time each phase took on this thread since the last call (or since
/// the thread first marked a phase), and the count starts again from now.
pub fn take() -> PhaseTimes {
    charge(Instant::now());
    TOTALS.replace([0; PHASES])
}

/// Counts the time since the last mark for the current phase.
fn charge(now: Instant) {
    if let Some(since) = SINCE.replace(Some(now)) {
        let elapsed = u64::try_from(now.duration_since(since).as_nanos()).unwrap_or(u64::MAX);
        let mut totals = TOTALS.get();
        let slot = &mut totals[CURRENT.get().index()];
        *slot = slot.saturating_add(elapsed);
        TOTALS.set(totals);
    }
}

/// The mark of a phase ([`enter`]).
pub struct Guard {
    /// The phase to go back to; none when the thread was already in this
    /// one.
    previous: Option<Phase>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            charge(Instant::now());
            CURRENT.set(previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    fn spin(time: Duration) {
        let start = Instant::now();
        while start.elapsed() < time {}
    }

    /// Nested phases count exclusively, and the outer one resumes after the
    /// inner one; marking the phase the thread is in changes nothing.
    #[test]
    fn nested_phases_count_exclusively() {
        thread::spawn(|| {
            take();
            let start = Instant::now();
            {
                let _simulate = enter(Phase::Simulate);
                spin(Duration::from_millis(4));
                {
                    let _script = enter(Phase::Script);
                    spin(Duration::from_millis(6));
                    let _again = enter(Phase::Script);
                    spin(Duration::from_millis(2));
                }
                spin(Duration::from_millis(4));
            }
            let total = start.elapsed().as_nanos() as u64;
            let times = take();
            let (simulate, script) = (times[Phase::Simulate.index()], times[Phase::Script.index()]);
            // Each phase got at least its own spins; counted inclusively,
            // the two would add up to more than the whole block took.
            assert!(simulate >= 8_000_000 && script >= 8_000_000, "{times:?}");
            assert!(simulate + script <= total, "{times:?} in {total} ns");
            assert_eq!(take()[Phase::Script.index()], 0);
        })
        .join()
        .unwrap();
    }
}
