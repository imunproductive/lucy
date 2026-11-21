//! The wall check of a statescript state `STU_FBFF5DE7` (Rocket Punch
//! `0D06` st7, its victim's `0D07` st20): a trial move of the body ahead of
//! it, and how far the walls it meets turn that move.
//!
//! Client (image base 0x7FF788F10000): the state's update (slot 14
//! `0x7FF789C3D440`) runs it each update while the mover's byte `+0x4D0` is
//! set (mod 23 above zero, `0x7FF789B4D610`; no hero's mods turn it off) or
//! `m_8A696888` is, and writes `m_26DA7CF8` = `m_87A4D0AD` (degrees) >
//! asin of the cosine `0x7FF789C4B360` returns. That function builds the
//! trial's settings and runs the mover's own step on a copy of the movement
//! state (`0x7FF789B44630`: `m_81759372` steps, default 4, each of
//! dt = 1 / steps, gravity scaled by `m_7CBA482F`). In the displacement form
//! (`m_9A978490` true, the punch's) the trial's velocity is the desired
//! displacement `d` = the flat unit direction × max(0.01, distance) for one
//! second, so the capsule tries to move by `d`. The planes the step's last
//! contact entry holds (528-byte entries of the result, planes `+0`, count
//! `+8`) clip `d` (`0x7FF789CCAD50`, as [`clip_velocity`]); a clipped
//! vector at most a tenth of the distance long is zero, any other is made
//! unit and, when its |y| is below 0.707, flat and unit again; the result
//! is its dot with the direction, clamped to [-1, 1].
//!
//! Here the trial is the capsule's move by `d` in the body's world from
//! where it stands, without the step's ground probe and spring (the client
//! runs them first; on flat ground they don't change a horizontal move).
//! The velocity form (the trial's own displacement over several steps,
//! with ground friction) is not ported.

use std::sync::Arc;

use crate::{
    contact::body::CharacterBody,
    contact::cache::CapsulePose,
    contact::gjk::PointSetSize,
    contact::hull::HullContactError,
    contact::math::{Vec3, dot, scale},
    contact::solver::clip_velocity,
    contact::world::ContactWorld,
    mover::ground_walk::{GroundWalk, ground_walk},
    mover::landing::{LandingSearch, landing_point},
};

/// A clipped move at most this share of the distance long stops the move.
const STOPPED: f32 = 0.1;
/// A clipped move this steep (|y| at least) keeps its vertical part.
const STEEP: f32 = 0.707;

/// A body's capsule where it stands and the world it moves in, for its
/// statescript's wall checks ([`TrialMove::turn`]).
#[derive(Clone)]
pub struct TrialMove {
    world: Arc<ContactWorld>,
    pose: CapsulePose,
    category: u16,
}

impl TrialMove {
    pub fn new(world: Arc<ContactWorld>, pose: CapsulePose, category: u16) -> Self {
        Self {
            world,
            pose,
            category,
        }
    }

    /// The capsule's radius (its stance's).
    pub fn radius(&self) -> f64 {
        f64::from(self.pose.radius)
    }

    /// The cosine of the angle by which the walls turn a move of the body
    /// along the flat unit `direction` for `distance`: 1 when nothing is in
    /// the way, 0 when it stops.
    pub fn turn(&self, direction: [f64; 3], distance: f64) -> Result<f64, HullContactError> {
        let direction = direction.map(|axis| axis as f32);
        let distance = (distance as f32).max(0.01);
        let desired = scale(direction, distance);
        let mut body = CharacterBody::new(Arc::clone(&self.world), self.pose, self.category);
        let moved = body.move_capsule(desired, 1.0)?;
        let clipped = clip_velocity(desired, moved.planes);
        let threshold = distance * STOPPED;
        let turned = if dot(clipped, clipped) <= threshold * threshold {
            [0.0; 3]
        } else {
            let unit = unit_or_zero(clipped);
            if unit[1].abs() < STEEP {
                unit_or_zero([unit[0], 0.0, unit[2]])
            } else {
                unit
            }
        };
        Ok(f64::from(dot(turned, direction).clamp(-1.0, 1.0)))
    }

    /// Where a placement selector's landing search lands in the body's
    /// world, its casts meeting the static shapes of `category`
    /// ([`landing_point`]).
    pub fn landing(
        &self,
        category: u16,
        search: &LandingSearch,
    ) -> Result<Option<[f64; 3]>, PointSetSize> {
        landing_point(&self.world, category, search)
    }

    /// Whether a shockwave's ground walk reaches a body in the body's
    /// world ([`ground_walk`]).
    pub fn ground_walk(
        &self,
        sphere_category: u16,
        ray_category: u16,
        walk: &GroundWalk,
    ) -> Result<bool, PointSetSize> {
        ground_walk(&self.world, sphere_category, ray_category, walk)
    }
}

/// `v` made unit, or zero when it is too short (`xmmword_7FF78C151710`).
fn unit_or_zero(v: Vec3) -> Vec3 {
    let squared = dot(v, v);
    if squared < 1.175_494_4e-36 {
        return [0.0; 3];
    }
    scale(v, squared.sqrt().recip())
}
