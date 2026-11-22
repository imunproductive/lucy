//! Meteor Strike's flight over the ground (`STU_6154B909`, Doomfist's `0C51`
//! st36): the client's mover behaviour of kind 13 (vtable
//! `0x7FF78B56AB70`, made by `0x7FF789B533F0`).
//!
//! The state (begin `0x7FF789C41180`, update `0x7FF789C4CA20`, end
//! `0x7FF789C41480`) builds a new behaviour at every update with a fresh
//! block (slot 57 `0x7FF789C3CDA0`: `+24` `m_4999D53A`, `+28`
//! `m_62F6795F`, `+32` `m_2A7E6E17`, −100000 when unset) and queues it from
//! the next frame, so each tick runs on the block of the frame before. Its
//! begin and end (slots 4, 5) do nothing and it sets no flag (slot 6).
//!
//! Of the tick's steps it replaces three:
//!
//! - the ground probe and the transitions (slot 8 `0x7FF789B58E70`): nothing
//!   lands, falls or counts air ticks; flag 0x20000 is cleared, the body
//!   stands on no platform (`0x7FF789B4ADA0` with none);
//! - the pogo spring (slot 10 `0x7FF789B583A0`): the offset is the stance's
//!   pogo below the feet (`0x7FF789ACF0C0`: the pogo × min(scale, 1)) and
//!   the speed 0, so the capsule's lowest point is the feet;
//! - the move (slot 16 `0x7FF789B54A30`, from the move `0x7FF789B4D9C0`):
//!   [`Rise::search`] gives a point, the capsule is put there
//!   (`0x7FF789CCAD10`) and the feet under it; the velocity stays as the
//!   velocity step left it.
//!
//! The velocity step (with the state's movement mods), the velocity mods,
//! the take-off and gravity run as usual.
//!
//! [`Rise::search`] follows `0x7FF789B54A30` in single precision. With the
//! block's `+24` set (Meteor Strike unless secondary fire is held) it keeps
//! the body on the ground under the aim: from the floor up it tries heights
//! one radius apart (the first, then every fifth by the frame, so a frame
//! tries a fifth of them) for the one whose move by the velocity goes
//! furthest along it, and puts the body down on what is under that; without
//! `+24` it tries them from the top down and lifts the body to `height` over
//! the floor, or under a ceiling.
//!
//! Not ported: the move's first step when mover byte `+5344` is set (a move
//! by nothing; its writer is not traced), and the entities the sweeps leave
//! out (`0x7FF789B542C0`'s list, whose test `0x7FF789B51400` is not traced):
//! the server's sweeps meet the static map only.

use std::fmt;

use crate::contact::world::ContactWorld;
use crate::contact::{body::CharacterBody, gjk::PointSetSize, hull::HullContactError};
use crate::mover::landing::sweep;

type Vec3 = [f32; 3];

/// What a `STU_6154B909` state hands the mover each frame (its block).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rise {
    /// `m_4999D53A` (block `+24`): search from the floor up and settle on
    /// the ground, else from the top down and rise.
    pub low: bool,
    /// `m_62F6795F` (`+28`): how far over the floor the body may be.
    pub height: f32,
    /// `m_2A7E6E17` (`+32`): the lowest floor; [`UNSET_FLOOR`] when unset.
    pub floor: f32,
}

/// The block's `+32` when `m_2A7E6E17` is unset (`0xC7C35000`).
pub const UNSET_FLOOR: f32 = -100_000.0;

/// What kills a body that falls into it: the world's kill height and its
/// damage areas (`STUECDamageAreaInstanceData`, class `6E7ED1C2`), as the
/// client's `0x7FF789B2F280` tests them.
pub trait DeathZones: fmt::Debug + Send + Sync {
    /// The highest point where the segment from `start` down to `end` is in
    /// a zone that kills, if any.
    fn entry(&self, start: [f64; 3], end: [f64; 3]) -> Option<[f64; 3]>;
}

/// `0x7FF789B2F280` with its scene ray: a ray from `start` to `end` stops
/// at the first static shape of `category`; where the rest of it is in a
/// zone that kills ([`DeathZones::entry`]), if anywhere. Meteor Strike's
/// floor (`STU_88687E54`, kind 13's first ray) asks it.
pub fn fatal_drop(
    world: &ContactWorld,
    category: u16,
    zones: Option<&dyn DeathZones>,
    start: [f64; 3],
    end: [f64; 3],
) -> Result<Option<[f64; 3]>, PointSetSize> {
    let Some(zones) = zones else {
        return Ok(None);
    };
    let end = match sweep(world, category, start, end, 0.0, None)? {
        Some(hit) => {
            std::array::from_fn(|axis| start[axis] + (end[axis] - start[axis]) * hit.fraction)
        }
        None => end,
    };
    Ok(zones.entry(start, end))
}

/// The client's sphere sweep `0x7FF789B5F920` (radius 0: a ray): the
/// fraction of the way at the first contact.
fn first_contact(
    world: &ContactWorld,
    category: u16,
    start: Vec3,
    end: Vec3,
    radius: f32,
) -> Result<Option<f32>, PointSetSize> {
    let wide = |v: Vec3| v.map(f64::from);
    Ok(sweep(
        world,
        category,
        wide(start),
        wide(end),
        f64::from(radius),
        None,
    )?
    .map(|hit| hit.fraction as f32))
}

impl Rise {
    /// Slot 16: where the capsule (its lowest point at `body`'s position)
    /// goes this tick, moving by `velocity` over `dt` on command `frame`.
    /// `body` is the mover's own and its trial moves (`0x7FF789CCAD10`,
    /// `0x7FF789CCABF0` with 3 rounds) move it; the caller puts it at the
    /// result.
    pub fn search(
        &self,
        body: &mut CharacterBody,
        category: u16,
        zones: Option<&dyn DeathZones>,
        velocity: Vec3,
        dt: f32,
        frame: u32,
    ) -> Result<Vec3, HullContactError> {
        let pose = body.pose();
        let start = pose.position;
        let radius = pose.radius;
        let tall = radius * 2.0 + (pose.top - radius);
        let half = tall * 0.5;
        let probe = radius - 0.015;
        let up = [0.0, 1.0, 0.0];
        let centre = add(start, scale(up, half));
        let world = body.shared_world();
        let ground = fatal_drop(
            &world,
            category,
            zones,
            centre.map(f64::from),
            [centre[0], centre[1] - 50.0, centre[2]].map(f64::from),
        )?
        .map_or(UNSET_FLOOR, |point| point[1] as f32);
        let mut floor = self.floor.max(ground);
        let mut lowest = sub(centre, scale(up, self.height + 1.0));
        let mut level = match first_contact(&world, category, centre, lowest, probe)? {
            Some(fraction) => lerp(centre, lowest, fraction)[1] - probe,
            None => lowest[1],
        };
        if level > start[1] {
            level = start[1];
        }
        if level < floor {
            level = centre[1].min(floor);
        }
        let mut target = add(scale(up, self.height - (start[1] - level)), centre);
        let mut top = match first_contact(&world, category, centre, target, probe)? {
            Some(fraction) => probe + lerp(centre, target, fraction)[1],
            None => target[1],
        };
        target[1] = top;
        if top < start[1] + tall {
            top = start[1] + tall;
            target[1] = top;
        }
        if top < floor {
            floor = top - tall;
        }
        let bottom = level.max(floor);
        lowest[1] = bottom;
        let span = length(sub(lowest, target));
        let mut count = 1;
        if span > tall {
            let steps = (span - tall) / radius;
            let mut whole = steps as i32;
            if steps < whole as f32 {
                whole -= 1;
            }
            count = whole + 1;
        }
        let base = [lowest[0], bottom, lowest[2]];
        let (first, stop, step) = if self.low {
            (0, count, 1)
        } else {
            (count - 1, -1, -1)
        };
        let phase = (frame % 5) as i32;
        let travel = scale(velocity, dt);
        let mut best = start;
        let mut best_score = -f32::MAX;
        let mut i = first;
        while i != stop {
            if i == first || i % 5 == phase {
                let candidate = add(scale(scale(up, radius), i as f32), base);
                let flat = [velocity[0], 0.0, velocity[2]];
                if flat[0] * flat[0] + flat[2] * flat[2] < TINY_SQUARED {
                    best = candidate;
                    break;
                }
                let direction = unit(flat);
                let middle = add(scale(up, half), candidate);
                let ahead = add(scale(direction, half), middle);
                if i == first || first_contact(&world, category, middle, ahead, 0.0)?.is_none() {
                    body.teleport(candidate);
                    let end = body.move_fixed(travel, 3)?;
                    if end[1] >= ground {
                        let score = dot(unit(travel), sub(end, candidate));
                        if score > best_score {
                            best_score = score;
                            best = end;
                            let enough = length(travel) * 0.707;
                            if (score - enough).abs() <= 0.004 || score > enough {
                                break;
                            }
                        }
                    }
                }
            }
            i += step;
        }
        let settle = if self.low {
            centre[1] - (self.height + 1.0)
        } else {
            top + 1.0
        };
        let end = [best[0], settle.max(floor), best[2]];
        let lift = if self.low { radius } else { tall - radius };
        let from = add(scale(up, lift), best);
        if let Some(fraction) = first_contact(&world, category, from, end, probe)? {
            let contact = lerp(from, end, fraction)[1];
            if self.low {
                let y = contact - probe;
                if y < best[1] {
                    best[1] = y;
                }
            } else {
                let y = contact - (tall - probe);
                if y > best[1] {
                    best[1] = y;
                }
            }
        }
        Ok(best)
    }
}

const TINY_SQUARED: f32 = 1.175_494_4e-36;

fn add(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|axis| a[axis] + b[axis])
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn scale(a: Vec3, k: f32) -> Vec3 {
    a.map(|value| value * k)
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    (a[1] * b[1] + a[0] * b[0]) + a[2] * b[2]
}

fn lerp(from: Vec3, to: Vec3, fraction: f32) -> Vec3 {
    add(scale(sub(to, from), fraction), from)
}

/// The correctly rounded `rsqrtps` estimate.
fn estimate(value: f32) -> f32 {
    if value == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / f64::from(value).sqrt()) as f32
    }
}

/// `v` made unit as the client does (`rsqrtps`, one Newton step).
fn unit(v: Vec3) -> Vec3 {
    let squared = dot(v, v);
    let r = estimate(squared);
    scale(v, (3.0 - (r * r) * squared) * (r * 0.5))
}

/// A length as the client takes it (`rsqrtss`, one Newton step; 0 when
/// tiny).
fn length(v: Vec3) -> f32 {
    let squared = dot(v, v);
    if squared > TINY_SQUARED {
        let r = estimate(squared);
        squared * ((3.0 - squared * (r * r)) * (r * 0.5))
    } else {
        0.0
    }
}
