//! The ground walk of a statescript shockwave's hit pass
//! (`STUStatescriptStateShockwave` with `m_7A61C4A9`; Doomfist's Seismic
//! Slam `0B27` st44): whether the wave runs along the ground from the
//! slam to a body it would hit.
//!
//! Client (image base 0x7FF788F10000): the hit pass `0x7FF789BBD0D0` tests
//! each candidate with `0x7FF789BAC810`. Its sight line (`0x7FF789BB3570`)
//! must have seen the body: it keeps the clear ray's origin and end (state
//! `+464`/`+480`, flag `+496`), and without one the walk fails. Then:
//!
//! 1. A sphere of 0.45 is cast from 1 m above the body's position down to
//!    `probe + 1` below it (ray profile `0016.097`); no ground fails the
//!    walk. The ground's height is the sphere's centre at the hit less its
//!    radius.
//! 2. The walk goes from the ray's end on the body toward its origin in
//!    `ceil(length)` equal steps (at most 1 m). At each sample a ray (the
//!    profile of the node's `m_9A8CA980`) is cast down `2 + probe`
//!    (`6 + probe` once a hit surface had flag 672); the last sample casts
//!    a sphere of 0.45 (`0016.097`) in place of the ray. A cast that meets
//!    nothing fails the walk.
//! 3. Each sample's rise over the last ground adds to a running climb;
//!    a rise smaller than the step (1 m, the flat distance from the first
//!    cast for the first sample) first takes that much off the climb,
//!    toward 0. A climb above `climb` or below `-drop` fails the walk; a hit
//!    with flag 672 (or one after it) sets it back to 0.
//!
//! Not modelled: flag 672 of the hit's object (`0x7FF789CC2D40`): the
//! static map is taken not to have it; the casts meet the static map only.

use crate::contact::{gjk::PointSetSize, world::ContactWorld};
use crate::mover::landing::sweep;

/// The sphere of the walk's first and last casts.
const SPHERE: f64 = 0.45;
/// How far below a sample a ray looks, besides `probe`.
const DEPTH: f64 = 2.0;

/// A shockwave's walk to one body, evaluated (`0B27` st44: 1.45 each).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundWalk {
    /// The body's position (its feet).
    pub feet: [f64; 3],
    /// Where the sight line met the body.
    pub from: [f64; 3],
    /// The sight line's origin (the wave's `m_D59188D5`).
    pub to: [f64; 3],
    /// The deepest drop the walk takes (`m_FF3B7DF2`).
    pub drop: f64,
    /// The highest climb the walk takes (`m_E81B2985`).
    pub climb: f64,
    /// How far under a sample the ground may be (`m_525E5A40`).
    pub probe: f64,
}

/// Whether the walk reaches the body: the first and last casts meet the
/// static shapes of `sphere_category`, the samples' rays those of
/// `ray_category`.
pub fn ground_walk(
    world: &ContactWorld,
    sphere_category: u16,
    ray_category: u16,
    walk: &GroundWalk,
) -> Result<bool, PointSetSize> {
    let GroundWalk {
        feet,
        from,
        to,
        drop,
        climb,
        probe,
    } = *walk;
    let start = [feet[0], feet[1] + 1.0, feet[2]];
    let end = [feet[0], feet[1] - (probe + 1.0), feet[2]];
    let Some(first) = sweep(world, sphere_category, start, end, SPHERE, None)? else {
        return Ok(false);
    };
    let mut ground = lerp(start, end, first.fraction)[1] - SPHERE;
    let leg = sub(to, from);
    let length = norm(leg);
    let count = length.ceil();
    if count < 1.0 {
        return Ok(true);
    }
    let count = count as usize;
    let step = scale(leg, 1.0 / count as f64);
    let mut sample = add(from, step);
    let mut climbed = 0.0_f64;
    for index in 1..=count {
        let last = index == count;
        let (radius, category) = if last {
            (SPHERE, sphere_category)
        } else {
            (0.0, ray_category)
        };
        let below = [sample[0], sample[1] - (DEPTH + probe), sample[2]];
        let Some(hit) = sweep(world, category, sample, below, radius, None)? else {
            return Ok(false);
        };
        let rise = hit.point[1] - ground;
        let allowance = if index == 1 {
            (start[0] - sample[0]).hypot(start[2] - sample[2])
        } else {
            1.0
        };
        if rise.abs() < allowance {
            climbed = if climbed <= 0.0 {
                (allowance + climbed).min(0.0)
            } else {
                (climbed - allowance).max(0.0)
            };
        }
        climbed += rise;
        if climbed > climb || climbed < -drop {
            return Ok(false);
        }
        ground = lerp(sample, below, hit.fraction)[1] - radius;
        sample = add(sample, step);
    }
    Ok(true)
}

fn lerp(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * t)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] + b[axis])
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    a.map(|value| value * k)
}

fn norm(a: [f64; 3]) -> f64 {
    a.iter().map(|value| value * value).sum::<f64>().sqrt()
}
