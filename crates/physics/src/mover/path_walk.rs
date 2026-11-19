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
//!    [`ledge_path`] from the block's position to its point, then, when it
//!    has more than one point, eight probes down along its last way
//!    ([`Walk::ledge`]); with no path the record's state is 2 and the tick
//!    moves as usual. The path set-up (`0x7FF789B578B0`): state 1, the
//!    seconds, 0 walked, the cap = the path's length over the seconds (100
//!    for 0.004 s or less).
//! 2. A walking tick (`0x7FF789B53FD0`) adds the tick's seconds to the
//!    walked ones and takes the point that far along the path in proportion
//!    (the last point at the end; a path of one point is done at once); the
//!    state is 2 once the walk is done. The velocity is the way from the
//!    feet to that point over the tick, held to the cap (a quarter of it on
//!    the last tick when the jump speed is 0 or less).
//! 3. A tick of a done walk keeps the velocity as it is.
//!
//! Every value is float32 in the client's order of operations
//! (`rsqrtss` estimates correctly rounded, then the client's Newton step).
//!
//! Assumptions: the block's position is the feet (to be checked in the
//! game); the probes' map mask is the mover's category (the client's query
//! takes `0x7FF789ADAA70`'s 8/16/24 from component 25's team bits, the same
//! as the server's mover categories) and the probes count only map
//! triangles; the probe's "start inside" flag (`v93 & 1`) is never set;
//! the body has component 86 (the probes run only then); no platform under
//! the body (the velocity is set in world space).

use orpheus_wire::entity::{NAV_PATH, NAV_PATH_POINTS, NavPathRecord};

use crate::{
    ai_cells::{AiCells, VOXEL_HEIGHT, ledge_path},
    collision::index::TriangleIndex,
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

/// What the path build needs of the body and its world.
pub(super) struct Ground<'a> {
    pub cells: Option<&'a AiCells>,
    pub rays: &'a TriangleIndex,
    pub category: u16,
    /// Degrees (`STUCharacterMoverComponent` `+0x40`).
    pub max_slope: f64,
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

    /// Slot 11 `0x7FF789B53130` for one tick.
    pub(super) fn step(
        &mut self,
        ground: &Ground<'_>,
        feet: [f64; 3],
        jump_speed: f64,
        dt: f64,
    ) -> Velocity {
        let dt = dt as f32;
        if self.state != 1 {
            if self.state == 2 {
                return Velocity::Keep;
            }
            if !self.build(ground) {
                self.state = 2;
                return Velocity::Default;
            }
        }
        let (target, done) = self.walk(dt);
        self.state = if done { 2 } else { 1 };
        let feet = feet.map(|axis| axis as f32);
        let way: Vec3 = std::array::from_fn(|axis| (target[axis] - feet[axis]) / dt);
        let mut cap = self.cap;
        if done && jump_speed as f32 <= 0.0 {
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

    /// `0x7FF789B5EC50`: the path, the ledge probes, the set-up
    /// (`0x7FF789B578B0`). False when there is no path.
    fn build(&mut self, ground: &Ground<'_>) -> bool {
        let Some(cells) = ground.cells else {
            return false;
        };
        let path = ledge_path(cells, self.from, self.to);
        let Ok(mut points) = path.path else {
            return false;
        };
        if points.len() > 1
            && let Some(point) = self.ledge(ground, &points)
        {
            points.push(point);
        }
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

    /// The ledge probes: eight rays 2.5 voxels down, from a quarter metre
    /// before the path's end along its last way (the first point to the
    /// last, flat; else the one before the last to the last), 0.1 m apart;
    /// a hit on ground no steeper than the body's slope limit counts, any
    /// other hit starts the count again, and three in a row (or the last
    /// counted hit after the eight) add that hit's point to the path.
    fn ledge(&self, ground: &Ground<'_>, points: &[Vec3]) -> Option<Vec3> {
        let last = *points.last()?;
        let flat = |from: Vec3| -> Option<Vec3> {
            let way = [last[0] - from[0], 0.0, last[2] - from[2]];
            let squared = (way[0] * way[0] + way[1] * way[1]) + way[2] * way[2];
            if squared < TINY_SQUARED {
                return None;
            }
            let r = estimate(squared);
            let factor = (3.0 - (r * r) * squared) * (0.5 * r);
            (factor * squared > 0.0).then(|| way.map(|axis| axis * factor))
        };
        let way = flat(points[0]).or_else(|| flat(points[points.len() - 2]))?;
        let slope = (ground.max_slope as f32 * 0.017_453_292).clamp(0.0, 1.570_784_4);
        let steepest = f64::from(slope).cos() as f32;
        let drop = VOXEL_HEIGHT * 2.5;
        let mut from: Vec3 = std::array::from_fn(|axis| last[axis] - 0.25 * way[axis]);
        let (mut run, mut found) = (0, None);
        for _ in 0..8 {
            let hit = ground
                .rays
                .cast_ray(
                    from.map(f64::from),
                    [0.0, -1.0, 0.0],
                    f64::from(drop),
                    ground.category,
                )
                .ok()
                .flatten();
            if let Some(hit) = hit {
                if hit.normal[1] as f32 <= steepest {
                    run = 0;
                } else {
                    run += 1;
                    found = Some(hit.point.map(|axis| axis as f32));
                    if run >= 3 {
                        break;
                    }
                }
            }
            from = std::array::from_fn(|axis| from[axis] + 0.1 * way[axis]);
        }
        found
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
