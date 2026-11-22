//! A walk along a path through the air the body's graphs start
//! (`STU_1F5ED928`, Widowmaker's ledge pull `005A` st30): the client's mover
//! behaviour of kind 10 (vtable `0x7FF78B56A568`).
//!
//! The state's begin (`0x7FF789C407D0`) queues the behaviour from the next
//! frame with its block (slot 56 `0x7FF789C39370`): `+32` the body's
//! position, `+48` the point `m_03AD31B9`, `+64` the seconds `m_710CF237`.
//! Its begin (slot 4 `0x7FF789B53DE0`) sets the movement state's flag
//! [`NAV_PATH`], takes a body on the ground off it (`0x7FF789AD0370`, unless
//! flags & 0x80000C) and sets the air ticks to 0; its end (slot 5
//! `0x7FF789B53A40`) clears the flag and the record. The state finishes on
//! the update that sees the walk done (slot 14 `0x7FF789C390C0`: the
//! record's state is 2).
//!
//! Unlike kind 1 it skips neither the ground probe, the transitions, the
//! spring nor the velocity mods (slots 8, 9, 10, 13 answer false): only the
//! velocity step (slot 11 `0x7FF789B53130`), each tick:
//!
//! 1. The first tick builds the path (slot 19 `0x7FF789B5EC50`):
//!    [`ledge_path`] from the block's position to its point; with no path
//!    the record's state is 2 and the tick moves as usual. The path set-up
//!    (`0x7FF789B578B0`): state 1, the seconds, 0 walked, the cap = the
//!    path's length over the seconds (100 for 0.004 s or less). The build's
//!    ledge probes (eight rays down from the path's end, whose hit becomes
//!    a last point) run only for a body with component 86
//!    (`STU_15971773`, the AI's); a player's Widowmaker has none (her
//!    client never cast them, `Grapple.md` "DayLedgePull2").
//! 2. A walking tick (`0x7FF789B53FD0`) adds the tick's seconds to the
//!    walked ones and takes the point that far along the path in proportion
//!    (the last point at the end; a path of one point is done at once); the
//!    state is 2 once the walk is done. The velocity is the way from the
//!    feet to that point over the tick, held to the cap; on the last tick
//!    to a quarter of it unless she pushes forward (the step's `+132`: the
//!    tick's forward throttle after mods 17 and 18, which the velocity step
//!    `0x7FF789B49FA0` and `0x7FF789B43960` put there; the session's done
//!    tick without input moved at 3.1855 m/s, a quarter of 12.742).
//! 3. A tick of a done walk keeps the velocity as it is.
//!
//! Every value is float32 in the client's order of operations
//! (`rsqrtss` estimates correctly rounded, then the client's Newton step).
//!
//! The block's position is the feet (her client's `+32` at the build was
//! her server's feet, `Grapple.md` "DayLedgePull2"). Assumptions: an AI
//! player's Widowmaker body has no component 86 either (the AI's
//! components are on the AI entity that possesses it); no platform under
//! the body (the velocity is set in world space).

use lucy_wire::entity::{NAV_PATH, NAV_PATH_POINTS, NavPathRecord};

use crate::{
    ai_cells::{AiCells, ledge_path},
    mover::{MoveEnd, MoveKey},
};

type Vec3 = [f32; 3];

/// What a `STU_1F5ED928` state hands the mover (the behaviour's block).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathWalk {
    /// `m_03AD31B9`: where the walk goes, world coordinates.
    pub point: [f64; 3],
    /// `m_710CF237`: the seconds the walk takes.
    pub seconds: f64,
}

/// A running walk (the mover's `+0xE50` record).
#[derive(Clone, Debug)]
pub(super) struct Walk {
    pub(super) key: MoveKey,
    from: Vec3,
    to: Vec3,
    seconds: f32,
    /// 0 before the path is built, 1 walking, 2 done.
    state: u8,
    elapsed: f32,
    cap: f32,
    points: Vec<Vec3>,
    reported: bool,
}

/// What a tick of the walk does to the velocity.
pub(super) enum Velocity {
    /// The tick moves as usual (no path).
    Default,
    /// The tick moves with this velocity (the usual velocity step skipped).
    Set([f64; 3]),
    /// The tick keeps the velocity it has (the walk is done).
    Keep,
}

impl Walk {
    /// The behaviour's block at its begin, `feet` the body's position then.
    pub(super) fn start(key: MoveKey, params: PathWalk, feet: [f64; 3]) -> Self {
        Self {
            key,
            from: feet.map(|axis| axis as f32),
            to: params.point.map(|axis| axis as f32),
            seconds: params.seconds as f32,
            state: 0,
            elapsed: 0.0,
            cap: 0.0,
            points: Vec::new(),
            reported: false,
        }
    }

    /// The movement state's record of the walk (flag [`NAV_PATH`]).
    pub(super) fn record(&self) -> NavPathRecord {
        let mut record = NavPathRecord {
            state: self.state,
            seconds: if self.points.is_empty() {
                0.0
            } else {
                self.seconds
            },
            elapsed: self.elapsed,
            cap: self.cap,
            ..NavPathRecord::default()
        };
        let count = self.points.len().min(NAV_PATH_POINTS);
        record.points[..count].copy_from_slice(&self.points[..count]);
        record.count = count as u8;
        record
    }

    pub(super) fn flag(&self) -> u32 {
        NAV_PATH
    }

    /// The end once the record's state is 2, reported once.
    pub(super) fn take_end(&mut self) -> Option<MoveEnd> {
        (self.state == 2 && !self.reported).then(|| {
            self.reported = true;
            MoveEnd::Arrived
        })
    }

    /// Slot 11 `0x7FF789B53130` for one tick; `cells` the map's AI cells
    /// the path build searches, `forward` the tick's forward throttle.
    pub(super) fn step(
        &mut self,
        cells: Option<&AiCells>,
        feet: [f64; 3],
        forward: f64,
        dt: f64,
    ) -> Velocity {
        let dt = dt as f32;
        if self.state != 1 {
            if self.state == 2 {
                return Velocity::Keep;
            }
            if !self.build(cells) {
                self.state = 2;
                return Velocity::Default;
            }
        }
        let (target, done) = self.walk(dt);
        self.state = if done { 2 } else { 1 };
        let feet = feet.map(|axis| axis as f32);
        let way: Vec3 = std::array::from_fn(|axis| (target[axis] - feet[axis]) / dt);
        let mut cap = self.cap;
        if done && forward as f32 <= 0.0 {
            cap *= 0.25;
        }
        let squared = (way[1] * way[1] + way[0] * way[0]) + way[2] * way[2];
        let velocity = if squared > cap * cap {
            let r = estimate(squared);
            let factor = (3.0 - (r * r) * squared) * (r * 0.5);
            way.map(|axis| (factor * axis) * cap)
        } else {
            way
        };
        Velocity::Set(velocity.map(f64::from))
    }

    /// `0x7FF789B5EC50`: the path and the set-up (`0x7FF789B578B0`). False
    /// when there is no path.
    fn build(&mut self, cells: Option<&AiCells>) -> bool {
        let Some(cells) = cells else {
            return false;
        };
        let Ok(points) = ledge_path(cells, self.from, self.to).path else {
            return false;
        };
        self.points = points;
        self.elapsed = 0.0;
        self.cap = if self.seconds <= 0.004 {
            100.0
        } else {
            length(&self.points) / self.seconds
        };
        self.state = 1;
        true
    }

    /// `0x7FF789B53FD0`: the point the walk has reached after this tick and
    /// whether it is done.
    fn walk(&mut self, dt: f32) -> (Vec3, bool) {
        if self.points.len() == 1 {
            return (self.points[0], true);
        }
        self.elapsed += dt;
        let share = (self.elapsed / self.seconds).clamp(0.0, 1.0);
        let mut along = length(&self.points) * share;
        let done = share >= 1.0;
        let last = self.points.len() - 1;
        let mut end = 1;
        for pair in self.points.windows(2) {
            let piece = segment(pair[0], pair[1]);
            if piece >= along {
                break;
            }
            end += 1;
            along -= piece;
        }
        let end = end.min(last);
        let from = self.points[end - 1];
        if done {
            return (self.points[end], true);
        }
        let way: Vec3 = std::array::from_fn(|axis| self.points[end][axis] - from[axis]);
        let squared = (way[1] * way[1] + way[0] * way[0]) + way[2] * way[2];
        let r = estimate(squared);
        let factor = (3.0 - (r * r) * squared) * (r * 0.5);
        (
            std::array::from_fn(|axis| (factor * way[axis]) * along + from[axis]),
            false,
        )
    }
}

const TINY_SQUARED: f32 = 1.175_494_4e-36;

/// The correctly rounded `rsqrtss` estimate.
fn estimate(value: f32) -> f32 {
    if value == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / f64::from(value).sqrt()) as f32
    }
}

/// A piece's length as `0x7FF789B53FD0` and `0x7FF789B541E0` take it.
fn segment(from: Vec3, to: Vec3) -> f32 {
    let way: Vec3 = std::array::from_fn(|axis| to[axis] - from[axis]);
    let squared = (way[1] * way[1] + way[0] * way[0]) + way[2] * way[2];
    if squared > TINY_SQUARED {
        let r = estimate(squared);
        squared * ((3.0 - squared * (r * r)) * (r * 0.5))
    } else {
        0.0
    }
}

/// `0x7FF789B541E0`: the path's length.
fn length(points: &[Vec3]) -> f32 {
    points
        .windows(2)
        .fold(0.0, |total, pair| total + segment(pair[0], pair[1]))
}
