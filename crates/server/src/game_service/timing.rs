//! A room's tick timing: the work of each tick by phase
//! (`lucy_core::phase`), and on the real clock how late each tick started
//! and the frames the clock skipped. Always on: a room adds each tick to a
//! window of [`WINDOW`] on its clock, logs the window when it closes
//! (target `lucy::timing`), and keeps the last closed window and the totals
//! since it opened for the snapshot ([`super::MatchSnapshot::timing`]).
//!
//! A tick's work is everything the room's thread did for the match since
//! the tick before: the datagrams it took, the commands it ran, the tick
//! itself. Its lateness is how long after its due time the tick began: the
//! wait's wake-up, and the datagrams taken in the same pass first. The
//! tests' virtual clock runs every tick on time.

use std::{fmt, time::Duration};

use lucy_core::phase::{PHASES, Phase, PhaseTimes};

/// A room logs its timing and starts a new window this often (its clock).
pub const WINDOW: Duration = Duration::from_secs(10);

/// The width of a histogram bucket.
const BUCKET_NS: u64 = 50_000;
/// Buckets to 32 ms; the last also counts everything longer.
const BUCKETS: usize = 640;

/// How a quantity spread over the ticks: a histogram of 50 µs buckets to
/// 32 ms, with the exact sum and maximum.
#[derive(Clone, PartialEq, Eq)]
pub struct Spread {
    counts: Vec<u32>,
    count: u64,
    sum: u64,
    max: u64,
}

impl Default for Spread {
    fn default() -> Self {
        Self {
            counts: vec![0; BUCKETS],
            count: 0,
            sum: 0,
            max: 0,
        }
    }
}

impl fmt::Debug for Spread {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Spread")
            .field("count", &self.count)
            .field("mean", &self.mean())
            .field("p99", &self.quantile(0.99))
            .field("max", &self.max())
            .finish()
    }
}

impl Spread {
    fn record(&mut self, nanoseconds: u64) {
        let bucket = usize::try_from(nanoseconds / BUCKET_NS)
            .map_or(BUCKETS - 1, |bucket| bucket.min(BUCKETS - 1));
        self.counts[bucket] += 1;
        self.count += 1;
        self.sum = self.sum.saturating_add(nanoseconds);
        self.max = self.max.max(nanoseconds);
    }

    fn add(&mut self, other: &Self) {
        for (count, other) in self.counts.iter_mut().zip(&other.counts) {
            *count += other;
        }
        self.count += other.count;
        self.sum = self.sum.saturating_add(other.sum);
        self.max = self.max.max(other.max);
    }

    /// How many ticks it has.
    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn mean(&self) -> Duration {
        Duration::from_nanos(self.sum.checked_div(self.count).unwrap_or(0))
    }

    pub fn max(&self) -> Duration {
        Duration::from_nanos(self.max)
    }

    /// The upper edge of a bucket, at most the maximum; the last bucket has
    /// none: the maximum.
    fn edge(&self, bucket: usize) -> u64 {
        if bucket == BUCKETS - 1 {
            return self.max;
        }
        ((bucket as u64 + 1) * BUCKET_NS).min(self.max)
    }

    /// The upper edge of the bucket that holds the `q` quantile (at most
    /// the maximum).
    pub fn quantile(&self, q: f64) -> Duration {
        if self.count == 0 {
            return Duration::ZERO;
        }
        let rank = ((self.count as f64 * q).ceil() as u64).clamp(1, self.count);
        let mut seen = 0;
        for (bucket, &count) in self.counts.iter().enumerate() {
            seen += u64::from(count);
            if seen >= rank {
                return Duration::from_nanos(self.edge(bucket));
            }
        }
        self.max()
    }

    /// What `self` has beyond `earlier`, an older copy of the same running
    /// spread. The maximum is the upper edge of the highest bucket left.
    pub fn since(&self, earlier: &Self) -> Self {
        let counts: Vec<u32> = self
            .counts
            .iter()
            .zip(&earlier.counts)
            .map(|(count, earlier)| count - earlier)
            .collect();
        let max = counts
            .iter()
            .rposition(|&count| count != 0)
            .map_or(0, |bucket| self.edge(bucket));
        Self {
            counts,
            count: self.count - earlier.count,
            sum: self.sum - earlier.sum,
            max,
        }
    }
}

/// The ticks of a span of a room's clock.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickStats {
    /// The room's time they cover.
    pub span: Duration,
    /// The frames its clock skipped: ticks that never ran, as the room
    /// fell behind (each is a frame number its games never get).
    pub skipped: u64,
    /// The time spent in each phase, summed over the ticks, by
    /// [`Phase::index`] (nanoseconds).
    pub phases: PhaseTimes,
    /// The most one tick spent in each phase (nanoseconds).
    pub phase_max: PhaseTimes,
    /// Each tick's work (every phase).
    pub work: Spread,
    /// How late each tick began.
    pub late: Spread,
}

impl TickStats {
    /// The ticks that ran.
    pub fn ticks(&self) -> u64 {
        self.work.count
    }

    /// The mean time a tick spent in `phase`.
    pub fn phase_mean(&self, phase: Phase) -> Duration {
        Duration::from_nanos(
            self.phases[phase.index()]
                .checked_div(self.ticks())
                .unwrap_or(0),
        )
    }

    /// Ticks a second over the span.
    pub fn rate(&self) -> f64 {
        if self.span.is_zero() {
            return 0.0;
        }
        self.ticks() as f64 / self.span.as_secs_f64()
    }

    fn record(&mut self, phases: &PhaseTimes, late: Duration, skipped: u64) {
        for ((sum, max), &spent) in self.phases.iter_mut().zip(&mut self.phase_max).zip(phases) {
            *sum = sum.saturating_add(spent);
            *max = (*max).max(spent);
        }
        self.work.record(phases.iter().sum());
        self.late
            .record(u64::try_from(late.as_nanos()).unwrap_or(u64::MAX));
        self.skipped += skipped;
    }

    fn add(&mut self, other: &Self) {
        self.span += other.span;
        self.skipped += other.skipped;
        for index in 0..PHASES {
            self.phases[index] = self.phases[index].saturating_add(other.phases[index]);
            self.phase_max[index] = self.phase_max[index].max(other.phase_max[index]);
        }
        self.work.add(&other.work);
        self.late.add(&other.late);
    }

    /// What `self` has beyond `earlier`, an older copy of the same running
    /// totals. The phases' maxima stay `self`'s: those of the whole run.
    pub fn since(&self, earlier: &Self) -> Self {
        Self {
            span: self.span.saturating_sub(earlier.span),
            skipped: self.skipped - earlier.skipped,
            phases: std::array::from_fn(|index| self.phases[index] - earlier.phases[index]),
            phase_max: self.phase_max,
            work: self.work.since(&earlier.work),
            late: self.late.since(&earlier.late),
        }
    }
}

impl fmt::Display for TickStats {
    /// One line: the rate, the work, each phase's mean, the lateness.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |time: Duration| time.as_secs_f64() * 1000.0;
        write!(
            formatter,
            "{} ticks in {:.1} s ({:.1}/s, {} skipped); work mean {:.2} ms, p99 {:.2}, max {:.2}; mean ms by phase:",
            self.ticks(),
            self.span.as_secs_f64(),
            self.rate(),
            self.skipped,
            ms(self.work.mean()),
            ms(self.work.quantile(0.99)),
            ms(self.work.max()),
        )?;
        for phase in Phase::ALL {
            write!(
                formatter,
                " {} {:.3}",
                phase.name(),
                ms(self.phase_mean(phase))
            )?;
        }
        write!(
            formatter,
            "; late mean {:.2} ms, p99 {:.2}, max {:.2}",
            ms(self.late.mean()),
            ms(self.late.quantile(0.99)),
            ms(self.late.max()),
        )
    }
}

/// A room's timing as the snapshot shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomTiming {
    /// The last closed window ([`WINDOW`]); empty until the first closes.
    pub window: TickStats,
    /// Every tick since the room opened, the open window's included.
    pub total: TickStats,
}

/// A room's running count ([`RoomState::timing`](super::RoomState)).
#[derive(Default)]
pub(super) struct TickTiming {
    /// The work since the last tick, for the next.
    pending: PhaseTimes,
    /// The open window, and when it opened on the room's clock.
    open: TickStats,
    opened: Option<Duration>,
    /// The windows that closed.
    closed: TickStats,
    last: TickStats,
}

impl TickTiming {
    /// Work done for the room, to count with its next tick.
    pub(super) fn add(&mut self, spent: &PhaseTimes) {
        for (pending, spent) in self.pending.iter_mut().zip(spent) {
            *pending = pending.saturating_add(*spent);
        }
    }

    /// A tick ran at `now`, `late` after it was due, after the clock
    /// skipped `skipped` frames; its work is what [`Self::add`] counted
    /// since the last. Returns the window it closed, if any.
    pub(super) fn tick(
        &mut self,
        now: Duration,
        late: Duration,
        skipped: u64,
    ) -> Option<&TickStats> {
        let pending = std::mem::take(&mut self.pending);
        let opened = *self.opened.get_or_insert(now);
        self.open.record(&pending, late, skipped);
        self.open.span = now.saturating_sub(opened);
        if self.open.span < WINDOW {
            return None;
        }
        self.opened = Some(now);
        self.last = std::mem::take(&mut self.open);
        self.closed.add(&self.last);
        Some(&self.last)
    }

    pub(super) fn snapshot(&self) -> RoomTiming {
        let mut total = self.closed.clone();
        total.add(&self.open);
        RoomTiming {
            window: self.last.clone(),
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phases(simulate_us: u64, send_us: u64) -> PhaseTimes {
        let mut phases = [0; PHASES];
        phases[Phase::Simulate.index()] = simulate_us * 1000;
        phases[Phase::Send.index()] = send_us * 1000;
        phases
    }

    /// Each tick's work is what the room did since the tick before; a
    /// window closes once its ticks span `WINDOW`, and the totals keep
    /// every tick; `since` gives what a later copy added.
    #[test]
    fn ticks_count_their_work_in_windows() {
        let mut timing = TickTiming::default();
        let tick = Duration::from_millis(16);
        let mut now = Duration::ZERO;
        let mut closed = 0;
        let mut early = None;
        for index in 0..1000_u64 {
            // Work between ticks counts with the next one.
            timing.add(&phases(1000, 0));
            timing.add(&phases(index % 3 * 100, 50));
            if index == 99 {
                early = Some(timing.snapshot().total);
            }
            if let Some(window) = timing.tick(now, Duration::from_micros(index % 2 * 500), 0) {
                closed += 1;
                // 10 s of 16 ms ticks.
                assert_eq!(
                    (window.ticks(), window.span),
                    (626, Duration::from_millis(10_000))
                );
            }
            now += tick;
        }
        assert_eq!(closed, 1);
        let RoomTiming { window, total } = timing.snapshot();
        assert_eq!(window.ticks(), 626);
        assert_eq!(total.ticks(), 1000);
        // 1000 + (0, 100 or 200) µs of simulation and 50 µs of sending.
        assert_eq!(total.phase_mean(Phase::Send), Duration::from_micros(50));
        assert_eq!(total.phase_max[Phase::Simulate.index()], 1_200_000);
        assert_eq!(total.work.max(), Duration::from_micros(1250));
        // The median tick took 1150 µs: its bucket's upper edge.
        assert_eq!(total.work.quantile(0.5), Duration::from_micros(1200));
        assert_eq!(total.late.max(), Duration::from_micros(500));
        let later = total.since(&early.unwrap());
        assert_eq!(later.ticks(), 1000 - 99);
        assert_eq!(later.work.max(), Duration::from_micros(1250));
    }
}
