//! A world-target move the body's graphs start (`STUStatescriptStateMoveTo`):
//! the client's mover behaviour of kind 1 (vtable `0x7FF78B56A2F0`, made by
//! `0x7FF789B516C0`), which drives the body's velocity toward a point while
//! it runs.
//!
//! The state hands the behaviour its block once at its begin (slot 57
//! `0x7FF789C43A60`, called from its update `0x7FF789C4CA20`; a state with
//! `m_84D349FB` would hand it again each update, which no modelled use
//! needs): the target (below), the speed (`m_speed`, or the distance over
//! `m_5BEFF040` seconds; at least 1.19e-4), the direction to the target as
//! it began, and the numbers of [`MoveTo`]. The target is the destination
//! `m_865F3D01` stood off its surface by the body's size
//! (`0x7FF789C4A7A0`: the normal `m_7183E9CE`'s horizontal part times its
//! horizontal length times the radius, and down by `(0.5 - n.y / 2)` of the
//! height), and with `m_EA99616F` the free spot a capsule of the body's
//! size finds moving from the destination toward that point
//! (`0x7FF789C4A8A0`: three moves of a capsule of a third, two thirds and
//! the whole size standing on the destination, each from where the last
//! stopped, see `free_spot`); without it the target is the destination
//! itself.
//!
//! While the behaviour runs (phases 1 and 3, mover `+0xDF4`), the mover's
//! tick (`0x7FF789B4B1D0`) skips its ground probe and transitions (slot 8
//! `0x7FF789B58F10`), the pogo spring (slot 10 `0x7FF789B58410`), its
//! velocity integration with gravity (slot 11 `0x7FF789B52F30`) and the
//! velocity mods 32..35 (slot 13 `0x7FF789B533B0`); slot 11 sets the
//! velocity instead:
//!
//! 1. the direction to the target from the feet (`0x7FF789B59A50`; none
//!    gives +z);
//! 2. arrival and blocking (`0x7FF789B5F7C0`): the feet within
//!    sqrt(1.19e-5) of the target, or a frame after the exact landing
//!    below (phase 3), arrive (phase 4) with the velocity held to the exit
//!    limits; else the feet moving `m_86EA271E` from the last checkpoint
//!    make a new checkpoint, and `m_725EB364` seconds without that arrive
//!    within that distance of the target, else block (phase 2);
//! 3. the velocity across the direction goes over `m_7F3716A6` seconds, the
//!    same share of what is left each frame (`0x7FF789B5ADE0`: frame `n` of
//!    `N = m_7F3716A6 / dt` takes `1 / (N - n)` of it; all of it when `N`
//!    or `N - n` is none);
//! 4. the speed along the direction goes toward the speed by speed times
//!    `m_717EAFE7` a second, eased within `m_187E2318` of the target toward
//!    the exit speed; a frame that would reach the target lands on it
//!    exactly, with the exit-limited rest of the velocity (phase 3,
//!    `0x7FF789B590F0`);
//! 5. Widowmaker's Grappling Hook only (`005A`, the block's `+153` tests the
//!    script's graph): a direction against the one it began with blocks
//!    (slot 12 `0x7FF789B573C0`).
//!
//! The exit limits (`0x7FF789B5D690`) hold the velocity's vertical part to
//! `m_03B764BE`, its horizontal part to `m_C1E1F78C` and the whole to
//! `m_6AC330A6`; an unset limit is 0. The behaviour's begin (slot 4
//! `0x7FF789B53CC0`) takes a body on the ground off it (`0x7FF789AD0370`)
//! and sets the checkpoint to the feet; with `m_AE1CEDB5` its end (slot 5
//! `0x7FF789B539B0`, when the state lets it go) holds the velocity to the
//! exit limits. A frame that ends the behaviour runs the velocity mods
//! after it; later frames move as usual until the state ends. The state's
//! begin aborts at once when the target is behind the body as seen from
//! the destination and the destination is within 2 m (`0x7FF789C41BC0`).
//! The state ends when its update sees phase 4 (finished) or 2 (aborted,
//! `0x7FF789C425F0`).
//!
//! Assumptions: the client's mover position here is the feet; the body's
//! scale (mover `+0xD5C`, which the stand-off and the free spot's capsule
//! multiply by) is 1; the up the exit limits split by (mover `+0xB0`) is
//! +y; a driven frame moves the capsule by the velocity alone, the pogo
//! spring held.
//!
//! Not modelled: a destination entity or bone (the target moves with it),
//! a moving platform under the body (`+152`), and the block's `+144`,
//! `+148`..`+150` and `+154` (`m_1866524C`, `m_B769BC47`, `m_000B044B`,
//! `m_5E3A7FC5`, `m_6D6BA94D`), whose readers were not traced.

use crate::{
    contact::body::CharacterBody, contact::cache::CapsulePose, contact::hull::HullContactError,
    mover::settings::MoverSettings,
};

/// The squared lengths and sizes the behaviour takes as none
/// (`0x7FF78B50B054`).
const TINY: f64 = 1.192_092_9e-5;
/// The client's normalisations treat a squared length below this as zero
/// (`FLT_MIN`).
const FLT_MIN: f64 = 1.175_494_4e-36;
/// The direction when there is none (`0x7FF78C1517A0`).
const FORWARD: [f64; 3] = [0.0, 0.0, 1.0];
/// The state's begin aborts for a destination this close (squared) behind
/// the target.
const BEGIN_ABORT_SQUARED: f64 = 4.0;
/// The rounds of each of the free spot's moves (`0x7FF789C4A8A0`: the
/// request's `+0x2C`).
const FREE_SPOT_ROUNDS: u32 = 5;

/// The speed of a move: metres a second, or the seconds the move takes
/// from where it begins (`m_5BEFF040` over 1.19e-4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Speed {
    MetresPerSecond(f64),
    Seconds(f64),
}

/// What a `MoveTo` state hands the mover (the behaviour's block).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveTo {
    /// `m_865F3D01`, world feet coordinates.
    pub destination: [f64; 3],
    /// `m_7183E9CE`: the surface the destination is on; the body stands off
    /// it by its size (zero: no offset).
    pub surface: [f64; 3],
    /// `m_EA99616F`: the target is the free spot a capsule finds.
    pub free_spot: bool,
    pub speed: Speed,
    /// `m_717EAFE7`: speeds a second.
    pub acceleration: f64,
    /// `m_187E2318`.
    pub slow_radius: f64,
    /// `m_7F3716A6`.
    pub lateral_seconds: f64,
    /// `m_86EA271E`.
    pub stuck_distance: f64,
    /// `m_725EB364`.
    pub stuck_seconds: f64,
    /// `m_C1E1F78C`, `m_03B764BE`, `m_6AC330A6`.
    pub exit_horizontal: f64,
    pub exit_vertical: f64,
    pub exit_total: f64,
    /// `m_AE1CEDB5`: the behaviour's end (slot 5 `0x7FF789B539B0`, as the
    /// state lets it go) holds the velocity to the exit limits.
    pub exit_on_end: bool,
    /// The script is Widowmaker's Grappling Hook (`005A`): a direction
    /// against the first one blocks.
    pub abort_past: bool,
}

/// Which state's move the mover runs: a new key begins a new behaviour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoveKey {
    pub instance: u16,
    pub raw_state: u16,
    pub activation: u16,
}

/// How a move ended: the state finishes or aborts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveEnd {
    Arrived,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Moving,
    Blocked,
    Landing,
    Arrived,
}

/// A running behaviour (the mover's `+0xDE0` record).
#[derive(Clone, Debug)]
pub(super) struct Run {
    pub(super) key: MoveKey,
    params: MoveTo,
    target: [f64; 3],
    direction: [f64; 3],
    speed: f64,
    phase: Phase,
    checkpoint: [f64; 3],
    still_frames: u32,
    lateral_frames: u32,
    /// The end not yet reported.
    unreported: Option<MoveEnd>,
}

/// The body the mover drives: where it stands and how big it is.
pub(super) struct Body<'a> {
    pub feet: [f64; 3],
    pub body: &'a CharacterBody,
    pub base: &'a MoverSettings,
    pub crouched: bool,
}

impl Run {
    /// The state's begin and the behaviour's first block.
    pub(super) fn start(
        key: MoveKey,
        params: MoveTo,
        at: Body<'_>,
    ) -> Result<Self, HullContactError> {
        let goal = add(
            params.destination,
            stand_off(params.surface, at.base, at.crouched),
        );
        let target = if params.free_spot {
            free_spot(&at, params.destination, goal)?
        } else {
            params.destination
        };
        let to_target = sub(target, at.feet);
        let distance = length(to_target);
        let speed = match params.speed {
            Speed::Seconds(seconds) if seconds > 1.192_092_9e-4 => distance / seconds,
            Speed::Seconds(_) => 0.0,
            Speed::MetresPerSecond(speed) => speed,
        }
        .max(1.192_092_9e-4);
        let behind = dot(to_target, sub(params.destination, at.feet)) < 0.0
            && squared(sub(params.destination, at.feet)) < BEGIN_ABORT_SQUARED;
        Ok(Self {
            key,
            params,
            target,
            direction: unit(to_target),
            speed,
            phase: if behind {
                Phase::Blocked
            } else {
                Phase::Moving
            },
            checkpoint: at.feet,
            still_frames: 0,
            lateral_frames: 0,
            unreported: behind.then_some(MoveEnd::Blocked),
        })
    }

    /// Whether the behaviour drives this frame.
    pub(super) fn drives(&self) -> bool {
        matches!(self.phase, Phase::Moving | Phase::Landing)
    }

    pub(super) fn target(&self) -> [f64; 3] {
        self.target
    }

    /// The end the mover hasn't reported yet, once.
    pub(super) fn take_end(&mut self) -> Option<MoveEnd> {
        self.unreported.take()
    }

    /// Slots 11 and 12 for one frame: the velocity the body moves with.
    pub(super) fn step(&mut self, feet: [f64; 3], velocity: [f64; 3], dt: f64) -> [f64; 3] {
        let to_target = sub(self.target, feet);
        let direction = unit(to_target);
        let near = squared(to_target);
        let stuck = self.params.stuck_distance * self.params.stuck_distance;
        if near < TINY || self.phase == Phase::Landing {
            return self.arrive(velocity);
        }
        if squared(sub(self.checkpoint, feet)) >= stuck {
            self.checkpoint = feet;
            self.still_frames = 0;
        } else {
            self.still_frames += 1;
            if f64::from(self.still_frames) * dt >= self.params.stuck_seconds {
                if near < stuck {
                    return self.arrive(velocity);
                }
                self.end(Phase::Blocked, MoveEnd::Blocked);
                return velocity;
            }
        }
        let velocity = self.drop_lateral(velocity, direction, dt);
        let velocity = self.drive(feet, velocity, direction, dt);
        if self.params.abort_past && dot(self.direction, direction) < 0.0 {
            self.end(Phase::Blocked, MoveEnd::Blocked);
        }
        velocity
    }

    fn arrive(&mut self, velocity: [f64; 3]) -> [f64; 3] {
        self.end(Phase::Arrived, MoveEnd::Arrived);
        self.exit(velocity)
    }

    fn end(&mut self, phase: Phase, end: MoveEnd) {
        self.phase = phase;
        self.unreported = Some(end);
    }

    /// `0x7FF789B5ADE0`.
    fn drop_lateral(&mut self, velocity: [f64; 3], direction: [f64; 3], dt: f64) -> [f64; 3] {
        let along = dot(velocity, direction);
        let mut lateral = sub(velocity, scale(direction, along));
        let frames = self.params.lateral_seconds / dt;
        if frames > TINY {
            let share = 1.0 / frames;
            let done = share * f64::from(self.lateral_frames);
            if 1.0 - done > TINY {
                lateral = scale(lateral, share / (1.0 - done));
            }
        }
        self.lateral_frames += 1;
        sub(velocity, lateral)
    }

    /// `0x7FF789B590F0`.
    fn drive(
        &mut self,
        feet: [f64; 3],
        velocity: [f64; 3],
        direction: [f64; 3],
        dt: f64,
    ) -> [f64; 3] {
        let along = dot(velocity, direction);
        let change = self.speed * self.params.acceleration * dt;
        let mut speed = if along >= self.speed {
            (along - change).max(self.speed)
        } else {
            (change + along).min(self.speed)
        };
        let distance = length(sub(self.target, feet));
        if distance < self.params.slow_radius {
            let exit = length(self.exit(velocity));
            speed = distance / self.params.slow_radius * (speed - exit) + exit;
        }
        let velocity = add(
            sub(velocity, scale(direction, along)),
            scale(direction, speed),
        );
        if dt * dot(direction, velocity) >= distance {
            let landing = scale(sub(self.target, feet), 1.0 / dt);
            self.phase = Phase::Landing;
            return add(self.exit(sub(velocity, landing)), landing);
        }
        velocity
    }

    /// The exit limits (`0x7FF789B5D690`).
    fn exit(&self, velocity: [f64; 3]) -> [f64; 3] {
        let vertical = [0.0, velocity[1], 0.0];
        let horizontal = [velocity[0], 0.0, velocity[2]];
        let held = add(
            limit(horizontal, self.params.exit_horizontal),
            limit(vertical, self.params.exit_vertical),
        );
        limit(held, self.params.exit_total)
    }

    /// The behaviour's end as the state lets it go (slot 5): the velocity
    /// held to the exit limits with `m_AE1CEDB5`, else as it is.
    pub(super) fn let_go(&self, velocity: [f64; 3]) -> [f64; 3] {
        if self.params.exit_on_end {
            self.exit(velocity)
        } else {
            velocity
        }
    }
}

/// `0x7FF789C4A7A0`: the feet offset that keeps a body of this stance off
/// the surface with normal `surface`.
fn stand_off(surface: [f64; 3], base: &MoverSettings, crouched: bool) -> [f64; 3] {
    let (height, radius) = if crouched {
        (base.crouch_height, base.crouch_radius)
    } else {
        (base.stand_height, base.stand_radius)
    };
    let [x, y, z] = surface;
    let flat = x * x + z * z;
    let reach = if flat > FLT_MIN {
        flat.sqrt() * radius
    } else {
        0.0
    };
    [x * reach, -((0.5 - y * 0.5) * height), z * reach]
}

/// `0x7FF789C4A8A0`: a temporary character (`0x7FF789CCAB30`) at the
/// destination makes three moves (`0x7FF789CCABF0`) toward the goal, each
/// from where the last stopped, with a capsule of a third, two thirds and
/// the whole of the stance's size (`0x7FF789CCAC00`): its points (0, r)
/// and (0, H - r) above the character's position, radius r, for the
/// height H (`0x7FF7896AA3B0`, mover `+0x430`, crouched `+0x43C`) and
/// radius r (`0x7FF789B53F40`, `+0x434`, crouched `+0x440`). The capsule's
/// lowest point is the position itself: no pogo under it, as the body's
/// own has. Each move takes up to 5 rounds, from a velocity of zero.
/// Where the last move stops.
fn free_spot(
    at: &Body<'_>,
    destination: [f64; 3],
    goal: [f64; 3],
) -> Result<[f64; 3], HullContactError> {
    let (height, radius) = if at.crouched {
        (at.base.crouch_height, at.base.crouch_radius)
    } else {
        (at.base.stand_height, at.base.stand_radius)
    };
    let (height, radius) = (height as f32, radius as f32);
    let goal = goal.map(|axis| axis as f32);
    // Made at the whole size, then shrunk for the first move.
    let mut probe = at.body.fresh(CapsulePose {
        position: destination.map(|axis| axis as f32),
        radius,
        top: height - radius,
    });
    for third in 1..=3 {
        let share = third as f32 * (1.0 / 3.0);
        probe.reshape(radius * share, (height - radius) * share);
        let from = probe.position();
        probe.move_fixed(std::array::from_fn(|k| goal[k] - from[k]), FREE_SPOT_ROUNDS)?;
    }
    Ok(probe.position().map(f64::from))
}

fn limit(vector: [f64; 3], most: f64) -> [f64; 3] {
    let length = length(vector);
    if length <= most {
        vector
    } else if length <= TINY {
        [0.0; 3]
    } else {
        scale(vector, most / length)
    }
}

fn unit(vector: [f64; 3]) -> [f64; 3] {
    let squared = squared(vector);
    if squared < FLT_MIN {
        FORWARD
    } else {
        scale(vector, 1.0 / squared.sqrt())
    }
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] + b[axis])
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn scale(a: [f64; 3], factor: f64) -> [f64; 3] {
    a.map(|axis| axis * factor)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn squared(a: [f64; 3]) -> f64 {
    dot(a, a)
}

fn length(a: [f64; 3]) -> f64 {
    squared(a).sqrt()
}
