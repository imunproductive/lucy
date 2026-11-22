//! The idle animations of server placeables (`STUMirroredIdleAnimComponent`,
//! client component 27): the parts of a model that the client's physics
//! world places by the model's animation (category 0x1000), posed as the
//! client poses them (`game_map_areas_174.json` `idle_animations`, derived
//! by `lucy-assets` `map_areas`).
//!
//! The client (static reading, refs `MapEntities.md` "MapAnim"): each frame
//! its component 27 system (`0x7FF7895811C0`) gives every component 27 the
//! time `t` of the world's interpolation clock (`0x7FF789D287F0`) and takes
//! the loop's phase from it (`0x7FF789AE1690`): `(t - start) · 30 · rate`
//! frames, less the whole loops of the component's frame count `n` (the
//! embedded `STUAnimGameData_Animation` `m_8E7A6FDE`), times `1/n`. `start`
//! and `rate` are 0 and 1 unless the placeable has the instance data
//! `STU_5AD3B86B` (`0x7FF789AE16E0`: start = -frames/30 from its `s32` at
//! +12, rate its `f32` at +8). The model then plays its idle clip
//! (`m_idleAnimation`) at that phase of the clip's duration.
//!
//! A clip of deltas adds each track to its bone's rest transform
//! ([`crate::bot_animation`]'s layers): 37 of the 50 idle clips with
//! animated shapes, the Hanamura cars' `11B3` among them (it keys the body
//! bone 2 at -0.343 m from its rest 0.681 m: the 0.338 m the client's
//! physics world showed). The other 13 replace the keyed channels (the
//! truck `03B4`'s `112F`). A shape keeps its place relative to its bone:
//! it moves by its bone's move from the rest pose (`posed · rest⁻¹`), in
//! the placeable's frame. Every bone of these rigs has scale 1.
//!
//! Checked against the client's physics world on Hanamura (2026-11-22,
//! refs `MapEntities.md` "MapAnim"): the four cars' body bones within
//! 3·10⁻⁵ m and 10⁻⁴ of their rotation at a time in the window the hooks
//! saw; the truck's hulls within 1.2 cm (the replacing clip's body bone 2
//! stands 1.2 cm higher than the client's: not traced).

use std::sync::Arc;

use serde::Deserialize;

use crate::bot_animation::{Clip, RawBone, RawClip, Transform, multiply, rotate};

/// A model's skeleton and its idle clip.
#[derive(Debug)]
pub struct IdleRig {
    parents: Vec<Option<usize>>,
    rest: Vec<Transform>,
    /// Each bone's rest place in the model.
    rest_places: Vec<Transform>,
    clip: Clip,
    additive: bool,
    /// The loop's length in the client's 30 fps frames.
    frames: u32,
}

#[derive(Deserialize)]
pub(crate) struct RawIdleRig {
    bones: Vec<RawBone>,
    clip: RawClip,
    frames: u32,
}

impl RawIdleRig {
    pub(crate) fn rig(self) -> Result<IdleRig, String> {
        let parents = RawBone::parents(&self.bones).map_err(|error| error.to_string())?;
        if self.frames == 0 {
            return Err("an idle loop of 0 frames".into());
        }
        let rest: Vec<Transform> = self.bones.iter().map(RawBone::rest).collect();
        let additive = self.clip.additive;
        let clip = self
            .clip
            .clip(rest.len())
            .map_err(|error| error.to_string())?;
        let rest_places = places(&parents, rest.clone());
        Ok(IdleRig {
            parents,
            rest,
            rest_places,
            clip,
            additive,
            frames: self.frames,
        })
    }
}

/// Each bone's place in the model: the local pose composed, parents first.
fn places(parents: &[Option<usize>], mut pose: Vec<Transform>) -> Vec<Transform> {
    for index in 0..pose.len() {
        if let Some(parent) = parents[index] {
            pose[index] = pose[parent].then(&pose[index]);
        }
    }
    pose
}

/// The conjugate of a unit quaternion.
fn conjugate([x, y, z, w]: [f64; 4]) -> [f64; 4] {
    [-x, -y, -z, w]
}

impl IdleRig {
    /// Its bone count.
    pub fn bones(&self) -> usize {
        self.rest.len()
    }

    /// The client's loop phase at the clock's time `time` for a placeable
    /// whose idle animation starts `start` frames in at `rate` (see the
    /// module notes; `0x7FF789AE1690`, in its precision: the time a float,
    /// the frames a double, truncated to whole frames as a 64-bit integer).
    pub fn phase(&self, time: f32, start: i32, rate: f32) -> f32 {
        let offset = f64::from(-(start as f32 * 0.033_333_335));
        let frame = (f64::from(time) - offset) * 30.0 * f64::from(rate);
        let whole = frame as i64;
        let looped = frame - (whole - whole % i64::from(self.frames)) as f64;
        let count = self.frames as f32;
        let inverse = (2.0 - count * (1.0 / count)) * (1.0 / count);
        looped as f32 * inverse
    }

    /// Each bone's move from its rest place in the model to its place at
    /// `phase` of the loop (`posed · rest⁻¹`; see the module notes).
    pub(crate) fn moves(&self, phase: f32) -> Vec<Transform> {
        let mut pose = self.rest.clone();
        let frame = self.clip.frame(f64::from(phase) * self.clip.duration, true);
        if self.additive {
            self.clip.add(frame, &mut pose);
        } else {
            self.clip.replace(frame, &mut pose);
        }
        places(&self.parents, pose)
            .into_iter()
            .zip(&self.rest_places)
            .map(|(posed, rest)| {
                let rotation = multiply(posed.rotation, conjugate(rest.rotation));
                let turned = rotate(rotation, rest.translation);
                Transform {
                    rotation,
                    scale: [1.0; 3],
                    translation: std::array::from_fn(|k| posed.translation[k] - turned[k]),
                }
            })
            .collect()
    }
}

/// A placeable's idle animation: its rig and the placeable's start and
/// rate.
#[derive(Clone, Debug)]
pub struct IdleMotion {
    pub rig: Arc<IdleRig>,
    pub start: i32,
    pub rate: f32,
}

impl IdleMotion {
    /// Each bone's move at the clock's time `time`.
    pub(crate) fn moves(&self, time: f32) -> Vec<Transform> {
        self.rig.moves(self.rig.phase(time, self.start, self.rate))
    }
}
