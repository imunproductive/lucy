//! The landing search of a statescript placement selector `STU_4C96470A`
//! (Seismic Slam in the air, `0B27` st6): where along the aim the body can
//! land, which the slam's `MoveTo` (st9) then flies to.
//!
//! Client (image base 0x7FF788F10000): the selector's compute
//! `0x7FF789BC6BA0` runs the search `0x7FF789B9EB30`. A ray (`0x7FF789B2F280`)
//! from the eye along the aim for `length` shortens the reach to its hit;
//! then a sphere of `radius` + 0.015 is swept (`0x7FF789C717C0`) from the
//! eye to the reach. Where it hits, two candidates: the contact raised
//! along its normal (straight up when the normal's y is above 0.7193) by
//! `step`, and the contact pulled back along the aim by `step`; each is
//! dropped when a sphere of `radius` swept to it from just before the
//! contact is blocked (both blocked: no landing). Without a hit the reach's
//! end is both. The candidate test (`0x7FF789BB0E50` → `0x7FF789BAE800`,
//! `0x7FF789BAC760`, `0x7FF789BADA00`) walks each candidate back toward the
//! eye in samples (the aim candidate twice, `step` apart; the normal one
//! every `spacing`), casts a sphere down from each sample by `step` plus
//! the reach the slope allows, and keeps a ground (normal y above 0.7193,
//! a surface that may be stood on) that the eye sees both ways at
//! `clearance / 2 + 0.1` above it and that has `reach_height` of room
//! above that; of those, the one nearest the normal candidate's height.
//!
//! This follows upstream's reading (leafall903/ORPHEUS `collision.py`
//! `landing_point`, checked on its host only) plus the eye ray above. Not
//! ported: the search's angle retries (`m_1B05EA98`, unset on the slam, so
//! the client's loop repeats the same aim), the client's spreading of the
//! candidate test over frames (3 samples a frame, `word_7FF78BD3EBC8`), the
//! entity filters of the casts (the casts meet the static map only).

use crate::contact::{
    gjk::{PointSetSize, sweep::MeshProbe},
    world::{ContactWorld, StaticGeometry},
};
use crate::mover::ground::NOT_GROUND;

/// The sweep's skin the search adds to the radius of its first cast.
const SKIN: f64 = 0.015;
/// A contact whose normal's y is above this is ground (`0x7FF789B9FEAC`).
const GROUND_Y: f64 = 0.719_340_03;
/// Shorter than this is no distance.
const EPSILON: f64 = 1e-9;

/// The selector's fields, evaluated (`0B27` st6: length `#1648` = 15,
/// radius 0.1, step 0.5, slope 0.15 rad, spacing 3, clearance 1.5, reach
/// height 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandingSearch {
    /// The eye: the body's position at its eye height (`m_6E8CA10B`).
    pub origin: [f64; 3],
    /// The aim (the view's direction); made unit here.
    pub aim: [f64; 3],
    /// How far along the aim (`m_6386E7B4`).
    pub length: f64,
    /// The swept sphere's radius (`m_3656A811`).
    pub radius: f64,
    /// The candidates' offset from a contact (`m_B3094B0E`).
    pub step: f64,
    /// The steepest descent of the ground cast, radians (`m_2ADABA5F`).
    pub slope: f64,
    /// The distance between samples toward the eye (`m_B42B2BEB`).
    pub spacing: f64,
    /// The room the body needs above the ground (`m_60B6AF94`).
    pub clearance: f64,
    /// The height a sight line ends above the ground when there is no
    /// clearance (`m_15F880A4`).
    pub reach_height: f64,
}

struct Sweep {
    fraction: f64,
    point: [f64; 3],
    normal: [f64; 3],
    surface: u16,
}

/// Where the body lands, or None when the search finds no ground.
pub fn landing_point(
    world: &ContactWorld,
    category: u16,
    search: &LandingSearch,
) -> Result<Option<[f64; 3]>, PointSetSize> {
    let LandingSearch {
        origin,
        aim,
        length,
        radius,
        step,
        slope,
        spacing,
        clearance,
        reach_height,
    } = *search;
    let finite = origin.iter().chain(&aim).all(|value| value.is_finite())
        && [
            length,
            radius,
            step,
            slope,
            spacing,
            clearance,
            reach_height,
        ]
        .iter()
        .all(|value| value.is_finite());
    if !finite
        || length.min(step).min(spacing) <= 0.0
        || radius.min(clearance).min(reach_height) < 0.0
        || !(0.0 < slope && slope < std::f64::consts::FRAC_PI_2)
    {
        return Ok(None);
    }
    let magnitude = norm(aim);
    if magnitude <= 0.0 || !magnitude.is_finite() {
        return Ok(None);
    }
    let aim = scale(aim, 1.0 / magnitude);
    // The eye ray shortens the reach to what it meets.
    let reach = match sweep(
        world,
        category,
        origin,
        add(origin, scale(aim, length)),
        0.0,
    )? {
        Some(hit) => length * hit.fraction,
        None => length,
    };
    let end = add(origin, scale(aim, reach));
    let mut normal_candidate = end;
    let mut aim_candidate = end;
    let hit = sweep(world, category, origin, end, radius + SKIN)?;
    if let Some(hit) = &hit {
        let offset = if hit.normal[1] > GROUND_Y {
            [0.0, 1.0, 0.0]
        } else {
            hit.normal
        };
        normal_candidate = add(hit.point, scale(offset, step));
        aim_candidate = sub(hit.point, scale(aim, step));
        let start = add(origin, scale(aim, (hit.fraction * reach - SKIN).max(0.0)));
        let normal_blocked = sweep(world, category, start, normal_candidate, radius)?.is_some();
        let aim_blocked = sweep(world, category, start, aim_candidate, radius)?.is_some();
        match (normal_blocked, aim_blocked) {
            (true, true) => return Ok(None),
            (true, false) => normal_candidate = aim_candidate,
            (false, true) => aim_candidate = normal_candidate,
            (false, false) => {}
        }
    }
    // `0x7FF789BAC760` bounds the ground cast by the aim's angle and the
    // distance left after stepping back from the candidate.
    let angle = std::f64::consts::PI - slope - (1.0 - aim[1]) * std::f64::consts::FRAC_PI_2;
    let depth_scale = (angle > 0.0).then(|| slope.sin() / angle.sin());
    let normal_distance = distance(origin, normal_candidate);
    let normal_count = (normal_distance / spacing + 0.5) as usize;
    let normal_radius = if hit.is_some() { radius } else { radius + SKIN };
    let candidates = [
        (aim_candidate, 2, step, 0.0),
        (
            normal_candidate,
            normal_count,
            normal_distance / normal_count.max(1) as f64,
            normal_radius,
        ),
    ];
    let half = clearance * 0.5;
    let has_clearance = half > 0.004;
    let middle_height = if has_clearance {
        half + 0.1
    } else {
        reach_height
    };
    let mut best = None;
    let mut best_height = f64::INFINITY;
    for (candidate, count, interval, ground_radius) in candidates {
        let candidate_distance = distance(origin, candidate);
        if candidate_distance <= EPSILON {
            continue;
        }
        let toward_eye = scale(sub(origin, candidate), 1.0 / candidate_distance);
        for index in 0..count {
            let retreat = index as f64 * interval;
            let remaining = candidate_distance - retreat;
            if remaining <= 0.0 {
                break;
            }
            let sample = add(candidate, scale(toward_eye, retreat));
            let depth = step + depth_scale.map_or(length, |k| length.min(remaining * k));
            let down = sub(sample, [0.0, depth, 0.0]);
            let Some(ground) = sweep(world, category, sample, down, ground_radius)? else {
                continue;
            };
            if ground.normal[1] <= GROUND_Y || ground.surface & NOT_GROUND != 0 {
                continue;
            }
            // `0x7FF789BADA00`: the eye sees the raised point both ways,
            // then the room above it.
            let lifted = add(ground.point, [0.0, SKIN.min(depth * ground.fraction), 0.0]);
            let middle = add(lifted, [0.0, middle_height, 0.0]);
            if sweep(world, category, middle, origin, 0.0)?.is_some()
                || sweep(world, category, origin, middle, 0.0)?.is_some()
            {
                continue;
            }
            if has_clearance {
                let upper = add(lifted, [0.0, reach_height, 0.0]);
                if sweep(world, category, middle, upper, 0.0)?.is_some() {
                    continue;
                }
            }
            let height = (ground.point[1] - normal_candidate[1]).abs();
            if height < best_height {
                best = Some(ground.point);
                best_height = height;
            }
        }
    }
    Ok(best)
}

/// A sphere of `radius` (a ray when 0) swept from `start` to `end` through
/// the static shapes of `category`: the first contact.
fn sweep(
    world: &ContactWorld,
    category: u16,
    start: [f64; 3],
    end: [f64; 3],
    radius: f64,
) -> Result<Option<Sweep>, PointSetSize> {
    if distance(start, end) <= EPSILON {
        return Ok(None);
    }
    let Some(hit) = world.scene_sweep(MeshProbe {
        points: &[[0.0; 3]],
        radius: radius as f32,
        start: start.map(|value| value as f32),
        end: end.map(|value| value as f32),
        category,
    })?
    else {
        return Ok(None);
    };
    let surface = match (world.shapes()[hit.shape as usize].geometry(), hit.triangle) {
        (StaticGeometry::Mesh(mesh), Some(number)) => {
            mesh.triangle(number).map_or(0, |triangle| triangle.surface)
        }
        _ => 0,
    };
    Ok(Some(Sweep {
        fraction: f64::from(hit.hit.fraction),
        point: hit.hit.point.map(f64::from),
        normal: hit.hit.normal.map(f64::from),
        surface,
    }))
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn norm(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    norm(sub(a, b))
}
