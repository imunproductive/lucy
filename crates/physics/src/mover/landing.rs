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
//! end is both.
//!
//! The candidate test `0x7FF789BB0E50` walks each candidate back toward the
//! eye in samples (`0x7FF789BAE800`): first the normal candidate, at most
//! twice `step` apart; then the aim candidate every `spacing` (its distance
//! over the nearest whole count), each of these casting a sphere-swept
//! segment to the next sample on the eye's side (the farther one when the
//! eye is below). Each sample casts down by `step` plus the reach the slope
//! allows (`0x7FF789BAC760`), with `radius` (+ 0.015 when the first sweep
//! met nothing), and keeps a ground (normal y above 0.7193, a surface that
//! may be stood on) no higher than the eye (`m_B221D18D`, `m_6E8CA10B`)
//! that the eye sees both ways at `clearance / 2 + 0.1` above it and that
//! has `reach_height` of room above that (`0x7FF789BADA00`). Console
//! variable `0xC0F0180A` (on) cuts the walks short: the normal candidate's
//! stops at its first ground, the aim candidate's two samples after its
//! first. Of the grounds, the client keeps the first nearest on screen to
//! the normal candidate, through a camera 1 m behind the eye looking at
//! that candidate with the world's up (`0x7FF78A987380`, `0x7FF78A987F80`).
//!
//! The ground test (`0x7FF789BB5550`) and the sight lines follow upstream's
//! reading (leafall903/ORPHEUS `collision.py` `landing_point`, checked on
//! its host only). Not ported: the minimum distance `m_00A23E4B` (a
//! candidate nearer retries the search at an aim turned by `m_1B05EA98`, a
//! nearer ground is skipped; the slam sets neither), the test
//! `0x7FF789B2F280` of each ground hit, the entity filters of the casts
//! (the casts meet the static map only).

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
/// height 1, no ground above the eye).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LandingSearch {
    /// The eye.
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
    /// No ground higher than this (`m_6E8CA10B` when `m_B221D18D` holds).
    pub ceiling: Option<f64>,
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
        ceiling,
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
            ceiling.unwrap_or_default(),
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
        None,
    )? {
        Some(hit) => length * hit.fraction,
        None => length,
    };
    let end = add(origin, scale(aim, reach));
    let mut normal_candidate = end;
    let mut aim_candidate = end;
    let hit = sweep(world, category, origin, end, radius + SKIN, None)?;
    if let Some(hit) = &hit {
        let offset = if hit.normal[1] > GROUND_Y {
            [0.0, 1.0, 0.0]
        } else {
            hit.normal
        };
        normal_candidate = add(hit.point, scale(offset, step));
        aim_candidate = sub(hit.point, scale(aim, step));
        let start = add(origin, scale(aim, (hit.fraction * reach - SKIN).max(0.0)));
        let normal_blocked =
            sweep(world, category, start, normal_candidate, radius, None)?.is_some();
        let aim_blocked = sweep(world, category, start, aim_candidate, radius, None)?.is_some();
        match (normal_blocked, aim_blocked) {
            (true, true) => return Ok(None),
            (true, false) => normal_candidate = aim_candidate,
            (false, true) => aim_candidate = normal_candidate,
            (false, false) => {}
        }
    }
    let normal_distance = distance(origin, normal_candidate);
    let aim_distance = distance(origin, aim_candidate);
    if normal_distance <= EPSILON || aim_distance <= EPSILON {
        return Ok(None);
    }
    // `0x7FF789BAC760` bounds the ground cast by the aim's angle and the
    // distance left after stepping back from the candidate.
    let angle = std::f64::consts::PI - slope - (1.0 - aim[1]) * std::f64::consts::FRAC_PI_2;
    let half = clearance * 0.5;
    let samples = Samples {
        world,
        category,
        origin,
        reach,
        step,
        depth_scale: (angle > 0.0).then(|| slope.sin() / angle.sin()),
        radius: if hit.is_some() { radius } else { radius + SKIN },
        ceiling,
        middle_height: if half > 0.004 {
            half + 0.1
        } else {
            reach_height
        },
        upper_height: (half > 0.004).then_some(reach_height),
    };
    let mut grounds = Vec::new();
    let normal_count = ((normal_distance / step + 0.5) as usize).min(2);
    for index in 0..normal_count {
        if let Some(ground) =
            samples.ground(normal_candidate, normal_distance, step, index, false)?
        {
            grounds.push(ground);
            break;
        }
    }
    let aim_count = (aim_distance / spacing + 0.5) as usize;
    let aim_interval = if aim_count > 0 {
        aim_distance / aim_count as f64
    } else {
        spacing
    };
    let mut last = aim_count;
    let mut index = 0;
    while index < last {
        if let Some(ground) =
            samples.ground(aim_candidate, aim_distance, aim_interval, index, true)?
        {
            grounds.push(ground);
            last = last.min(index + 3);
        }
        index += 1;
    }
    // The camera of `0x7FF78A987380`: 1 m behind the eye, looking at the
    // normal candidate, with the world's up as its second row as it is (not
    // made square to the view); a point's view coordinates are its
    // coordinates in that basis (`0x7FF789884260` inverts it). The candidate
    // is at the screen's middle, so the nearest on screen has the least
    // |up / depth|. A point not in front of the camera leaves the last
    // projection's result as it was (`0x7FF78A987F80`).
    let forward = scale(sub(normal_candidate, origin), 1.0 / normal_distance);
    let flat = forward[0] * forward[0] + forward[2] * forward[2];
    let mut best = None;
    let mut best_offset = f64::MAX;
    let mut offset = f64::MAX;
    for ground in grounds {
        let relative = sub(ground, origin);
        let (depth, up) = if flat > 0.0 {
            let depth = (relative[0] * forward[0] + relative[2] * forward[2]) / flat;
            (depth, relative[1] - depth * forward[1])
        } else {
            (relative[1] / forward[1], 0.0)
        };
        if depth + 1.0 >= 1.192_092_9e-6 {
            offset = (up / (depth + 1.0)).abs();
        }
        if offset < best_offset {
            best = Some(ground);
            best_offset = offset;
        }
    }
    Ok(best)
}

/// The sample test `0x7FF789BAE800` with the search's numbers.
struct Samples<'a> {
    world: &'a ContactWorld,
    category: u16,
    origin: [f64; 3],
    reach: f64,
    step: f64,
    depth_scale: Option<f64>,
    radius: f64,
    ceiling: Option<f64>,
    middle_height: f64,
    upper_height: Option<f64>,
}

impl Samples<'_> {
    /// The ground under the sample `index` intervals from `candidate`
    /// toward the eye; with `segment`, the cast spans to the next sample.
    fn ground(
        &self,
        candidate: [f64; 3],
        candidate_distance: f64,
        interval: f64,
        index: usize,
        segment: bool,
    ) -> Result<Option<[f64; 3]>, PointSetSize> {
        let Self {
            world,
            category,
            origin,
            ..
        } = *self;
        let toward_eye = scale(sub(origin, candidate), 1.0 / candidate_distance);
        let retreat = index as f64 * interval;
        let sample = add(candidate, scale(toward_eye, retreat));
        let depth = self.step
            + self.depth_scale.map_or(self.reach, |k| {
                self.reach.min((candidate_distance - retreat) * k)
            });
        // The segment runs to the next sample up toward the eye, or away
        // from it when the eye is below (none from the first then).
        let next = if toward_eye[1] < 0.0 {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let span = next
            .filter(|_| segment)
            .map(|next| scale(toward_eye, (next as f64 - index as f64) * interval));
        let down = sub(sample, [0.0, depth, 0.0]);
        let Some(ground) = sweep(world, category, sample, down, self.radius, span)? else {
            return Ok(None);
        };
        if ground.normal[1] <= GROUND_Y || ground.surface & NOT_GROUND != 0 {
            return Ok(None);
        }
        if self
            .ceiling
            .is_some_and(|ceiling| ground.point[1] > ceiling)
        {
            return Ok(None);
        }
        // `0x7FF789BADA00`: the eye sees the raised point both ways, then
        // the room above it.
        let lifted = add(ground.point, [0.0, SKIN.min(depth * ground.fraction), 0.0]);
        let middle = add(lifted, [0.0, self.middle_height, 0.0]);
        if sweep(world, category, middle, origin, 0.0, None)?.is_some()
            || sweep(world, category, origin, middle, 0.0, None)?.is_some()
        {
            return Ok(None);
        }
        if let Some(height) = self.upper_height {
            let upper = add(lifted, [0.0, height, 0.0]);
            if sweep(world, category, middle, upper, 0.0, None)?.is_some() {
                return Ok(None);
            }
        }
        Ok(Some(ground.point))
    }
}

/// A sphere of `radius` (a ray when 0), or the segment from it by `span`
/// when given, swept from `start` to `end` through the static shapes of
/// `category`: the first contact.
fn sweep(
    world: &ContactWorld,
    category: u16,
    start: [f64; 3],
    end: [f64; 3],
    radius: f64,
    span: Option<[f64; 3]>,
) -> Result<Option<Sweep>, PointSetSize> {
    if distance(start, end) <= EPSILON {
        return Ok(None);
    }
    let segment;
    let points: &[[f32; 3]] = match span {
        Some(span) => {
            segment = [[0.0; 3], span.map(|value| value as f32)];
            &segment
        }
        None => &[[0.0; 3]],
    };
    let Some(hit) = world.scene_sweep(MeshProbe {
        points,
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
