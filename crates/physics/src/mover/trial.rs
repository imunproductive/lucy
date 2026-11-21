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
//! The velocity form (`m_9A978490` false: Reinhardt's Charge `007D` st3,
//! `1537` st26) sets the trial's velocity slot to the same vector instead;
//! the trial (`0x7FF789B44630`) then runs `max(1, m_81759372)` steps of
//! dt = 1 / steps, and every step writes that velocity into the movement
//! state just before its move (`0x7FF789B76A70`), so the ground and air
//! friction (bytes `+61`/`+62`) and the gravity (`m_7CBA482F` above 0)
//! the later steps apply first are overwritten: the capsule moves by
//! `d` / steps each step. The result is the flat displacement from the
//! start to the last step's position divided by the distance (not made
//! unit), dotted with the direction and clamped.
//!
//! Here either trial is the capsule's moves by `d` (in `steps` parts for
//! the velocity form) in the body's world from where it stands, without
//! the steps' ground probe and spring (the client runs them first; on flat
//! ground they don't change a horizontal move); each velocity-form part
//! ends on the position grid in x and z, as the mover's moves do.

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
    mover::rise::{DeathZones, fatal_drop},
};

/// Which of the wall check's trials runs (`m_9A978490`; see the module
/// notes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialForm {
    /// The desired displacement against the walls the move meets.
    Displacement,
    /// The body's own displacement over `steps` moves.
    Velocity { steps: u32 },
}

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
    zones: Option<Arc<dyn DeathZones>>,
}

impl TrialMove {
    pub fn new(world: Arc<ContactWorld>, pose: CapsulePose, category: u16) -> Self {
        Self {
            world,
            pose,
            category,
            zones: None,
        }
    }

    /// The zones that kill a body that falls into them ([`fatal_drop`]).
    pub fn with_death_zones(mut self, zones: Option<Arc<dyn DeathZones>>) -> Self {
        self.zones = zones;
        self
    }

    /// Where a ray from `start` down to `end`, stopped by the static shapes
    /// the body's moves meet (as Meteor Strike's flight's ray), enters a
    /// zone that kills, if anywhere ([`fatal_drop`]).
    pub fn fatal_drop(
        &self,
        start: [f64; 3],
        end: [f64; 3],
    ) -> Result<Option<[f64; 3]>, PointSetSize> {
        fatal_drop(&self.world, self.category, self.zones.as_deref(), start, end)
    }

    /// The capsule's radius (its stance's).
    pub fn radius(&self) -> f64 {
        f64::from(self.pose.radius)
    }

    /// The trial's cosine in `form`: [`TrialMove::turn`] or
    /// [`TrialMove::travel`].
    pub fn cosine(
        &self,
        direction: [f64; 3],
        distance: f64,
        form: TrialForm,
    ) -> Result<f64, HullContactError> {
        match form {
            TrialForm::Displacement => self.turn(direction, distance),
            TrialForm::Velocity { steps } => self.travel(direction, distance, steps),
        }
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

    /// The velocity form's cosine: how far along the flat unit `direction`
    /// the body gets of a move by `distance` in `steps` equal parts, over
    /// the distance (1 when nothing is in the way, 0 when a wall stops it,
    /// below 1 when it slides along one).
    pub fn travel(
        &self,
        direction: [f64; 3],
        distance: f64,
        steps: u32,
    ) -> Result<f64, HullContactError> {
        let direction = direction.map(|axis| axis as f32);
        let distance = (distance as f32).max(0.01);
        let dt = 1.0 / steps.max(1) as f32;
        let part = scale(direction, distance * dt);
        let mut body = CharacterBody::new(Arc::clone(&self.world), self.pose, self.category);
        let start = body.position();
        for _ in 0..steps.max(1) {
            let [x, y, z] = body.move_capsule(part, dt)?.position;
            body.teleport([on_grid(x), y, on_grid(z)]);
        }
        let end = body.position();
        let moved = [
            (end[0] - start[0]) / distance,
            0.0,
            (end[2] - start[2]) / distance,
        ];
        Ok(f64::from(dot(moved, direction).clamp(-1.0, 1.0)))
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

/// `value` on the position grid (`snap`, in single precision).
fn on_grid(value: f32) -> f32 {
    super::posture::snap(f64::from(value)) as f32
}
