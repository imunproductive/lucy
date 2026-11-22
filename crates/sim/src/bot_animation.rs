//! The bots' animation at run time: each tick a bot body's hit shapes are
//! posed by the clips its blend trees play in the branch its state picks
//! (`game_bot_animation_174.json`, derived by `lucy-assets`, whose
//! `bot_animation` module says how the branches and clips are found).
//!
//! New: the Python reference hit every body as one capsule. The rules
//! below are our reading of the data; the client's animation code is not
//! in it, so each is an assumption.
//!
//! **Branch.** A bot that walks its patrol route plays the `moving` branch
//! (the locomotion tree's category `0005`), timed from the tick it set off;
//! otherwise `standing`, timed from the bot's first animated tick. The
//! direction and speed of the walk don't change the pose: the moving
//! branch's directional blend binds the same clip in all four directions
//! and no node of it is driven by speed (the clips play at their own
//! rate). When the bot's script has a Stack state active that pushes
//! category `007F` into variable 1226 (graph `0BA8` state 2, the only such
//! push), the `arms_forward_` branches play instead. When the script runs
//! an `Anim` state, the alias it names plays (`anims` of the file): its
//! base clip replaces the locomotion's base and its layers add to it, timed
//! from the state's start, stretched to the state's play-over duration
//! (`m_86701C3E`) when it has one, and held at its end. The latest started
//! state wins. The one `Anim` state the bots' scripts enter on today's
//! server is `0E8C`'s knockback (st7, alias `0174`), which a splash hit
//! (tag `078D`) starts and which ends 0.25 s later, once the bot's
//! movement sample says it is on the ground (`lucy_script::bot`); the
//! hit flinch (st3) is client-only, `0BA8`'s needs the reset message
//! `0013`, which only the firing behaviour gets, and `0BA3`'s reload names
//! alias `0004`, which the bots' set doesn't bind. The `007F` push sits
//! behind `0BA8`'s variable 7143, which nothing writes.
//!
//! **Clips.** A branch's base clips play in order, those marked `once`
//! one after the other and then the first that isn't, looping; a branch
//! with no looping clip holds its last. A base clip's track replaces its
//! bone's parent-relative rest transform, channel by channel (a channel the
//! track doesn't key keeps the rest). Its layers loop on the bot's clock
//! (the hover bob `391F` is 4 s): each track adds to the bone, the
//! translation summed, the rotation turned by the delta after the base's
//! (`base · delta`), the scale multiplied. Between keys positions and scales
//! interpolate linearly and rotations by normalised linear blend; outside
//! the keys the first and last hold. Switching branches is immediate (the
//! trees' blend times are left out).
//!
//! **Shapes.** Each bone is placed in the body's frame by its parent's
//! place composed with its own transform: the parent's scale scales the
//! child's translation, but the child keeps its own scale (the client's
//! hit-test skeleton, `*(*(model + 0x6F0) + 0x28)`, read in the game
//! during the assembly: bone 34 at scale 0.034, its own track's, not that
//! times its parent bone 31's track's 0.016; refs report `CombatFeel.md`).
//! A shape's points, given in its bone's frame, are placed by its bone, and
//! its radius scaled by the bone's scale (the client tests it in the bone's
//! frame). A bot with a rig is posed once a
//! tick, in [`crate::scene_assets::PreparedScene::step_bots`] after it
//! walks; its target ([`crate::scene_assets::PreparedScene::bot_target`])
//! carries those shapes, so the combat history keeps each tick's pose. A
//! body without a rig keeps the static shapes of `game_hitboxes_174.json`
//! (its idle pose's first frame).

use std::{collections::HashMap, fs, io, path::Path, sync::Arc};

use lucy_core::{guid::parse_guid, tick::TICK_PERIOD};
use lucy_physics::static_ray::Vec3;
use lucy_script::{graph::GraphStore, vm::Snapshot};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::hitboxes::HitShape;

/// The variable a bot's graph pushes its animation category into (`0BA8`
/// state 2, `STUConfigVarDynamic` 1226, remote sync variable `0D8:04CA`).
pub const CATEGORY_VARIABLE: u64 = 1226;
/// The arms-forward category (the Training Bot's tree `06E.020` switches
/// on it).
pub const ARMS_FORWARD: u64 = 0x0280_0000_0000_007F;

/// A bone's transform: rotation (x, y, z, w), scale, translation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Transform {
    pub rotation: [f64; 4],
    pub scale: Vec3,
    pub translation: Vec3,
}

/// q · r, quaternions (x, y, z, w).
pub(crate) fn multiply(q: [f64; 4], r: [f64; 4]) -> [f64; 4] {
    let [x1, y1, z1, w1] = q;
    let [x2, y2, z2, w2] = r;
    [
        w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
        w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
        w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
        w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
    ]
}

fn normalised(q: [f64; 4]) -> [f64; 4] {
    let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm == 0.0 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        q.map(|v| v / norm)
    }
}

/// v turned by the quaternion q.
pub(crate) fn rotate([x, y, z, w]: [f64; 4], [vx, vy, vz]: Vec3) -> Vec3 {
    let (cx, cy, cz) = (y * vz - z * vy, z * vx - x * vz, x * vy - y * vx);
    let (ux, uy, uz) = (w * vx + cx, w * vy + cy, w * vz + cz);
    let (tx, ty, tz) = (y * uz - z * uy, z * ux - x * uz, x * uy - y * ux);
    [vx + 2.0 * tx, vy + 2.0 * ty, vz + 2.0 * tz]
}

impl Transform {
    /// A point of this frame in its parent's: scaled, turned, moved.
    pub(crate) fn place(&self, point: Vec3) -> Vec3 {
        let scaled = std::array::from_fn(|k| point[k] * self.scale[k]);
        let turned = rotate(self.rotation, scaled);
        std::array::from_fn(|k| turned[k] + self.translation[k])
    }

    /// A child's transform relative to this one, placed by this one: the
    /// parent's scale moves the child but doesn't scale it (the child keeps
    /// its own scale, as the client's hit-test skeleton has it, module
    /// notes).
    pub(crate) fn then(&self, child: &Self) -> Self {
        Self {
            rotation: multiply(self.rotation, child.rotation),
            scale: child.scale,
            translation: self.place(child.translation),
        }
    }
}

fn lerp<const N: usize>(a: [f64; N], b: [f64; N], t: f64) -> [f64; N] {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// Normalised linear blend along the shorter arc.
fn blend(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    if t == 0.0 {
        return a;
    }
    let dot: f64 = (0..4).map(|k| a[k] * b[k]).sum();
    let b = if dot < 0.0 { b.map(|v| -v) } else { b };
    normalised(lerp(a, b, t))
}

/// Keys by frame, in frame order.
type Keys<const N: usize> = Vec<(f64, [f64; N])>;

/// The keys around `frame` and how far between them; the first or last key
/// outside them.
fn around<const N: usize>(keys: &Keys<N>, frame: f64) -> Option<([f64; N], [f64; N], f64)> {
    let after = keys.partition_point(|(at, _)| *at <= frame);
    match (
        after.checked_sub(1).map(|index| keys[index]),
        keys.get(after),
    ) {
        (Some((a, va)), Some(&(b, vb))) => Some((va, vb, (frame - a) / (b - a))),
        (Some((_, value)), None) | (None, Some(&(_, value))) => Some((value, value, 0.0)),
        (None, None) => None,
    }
}

/// One animated bone of a clip.
#[derive(Debug)]
struct Track {
    bone: usize,
    positions: Keys<3>,
    rotations: Keys<4>,
    scales: Keys<3>,
}

impl Track {
    fn position(&self, frame: f64) -> Option<Vec3> {
        around(&self.positions, frame).map(|(a, b, t)| lerp(a, b, t))
    }

    fn rotation(&self, frame: f64) -> Option<[f64; 4]> {
        around(&self.rotations, frame).map(|(a, b, t)| blend(a, b, t))
    }

    fn scale(&self, frame: f64) -> Option<Vec3> {
        around(&self.scales, frame).map(|(a, b, t)| lerp(a, b, t))
    }
}

#[derive(Debug)]
pub(crate) struct Clip {
    pub duration: f64,
    fps: f64,
    /// The last frame the keys index (`frames - 1`).
    last: f64,
    tracks: Vec<Track>,
}

impl Clip {
    /// The frame at `time` seconds, looping or held at the ends.
    pub(crate) fn frame(&self, time: f64, looping: bool) -> f64 {
        let time = if looping && self.duration > 0.0 {
            time.rem_euclid(self.duration)
        } else {
            time
        };
        (time * self.fps).clamp(0.0, self.last)
    }

    /// Replace each keyed channel of its bones at `frame`.
    pub(crate) fn replace(&self, frame: f64, pose: &mut [Transform]) {
        for track in &self.tracks {
            let bone = &mut pose[track.bone];
            if let Some(position) = track.position(frame) {
                bone.translation = position;
            }
            if let Some(rotation) = track.rotation(frame) {
                bone.rotation = rotation;
            }
            if let Some(scale) = track.scale(frame) {
                bone.scale = scale;
            }
        }
    }

    /// Add each keyed channel of its bones at `frame` (see the module
    /// notes).
    pub(crate) fn add(&self, frame: f64, pose: &mut [Transform]) {
        for track in &self.tracks {
            let bone = &mut pose[track.bone];
            if let Some(delta) = track.position(frame) {
                bone.translation = std::array::from_fn(|k| bone.translation[k] + delta[k]);
            }
            if let Some(delta) = track.rotation(frame) {
                bone.rotation = normalised(multiply(bone.rotation, delta));
            }
            if let Some(delta) = track.scale(frame) {
                bone.scale = std::array::from_fn(|k| bone.scale[k] * delta[k]);
            }
        }
    }
}

/// The clips a branch plays (indexes into the rig's clips).
#[derive(Debug, Default)]
struct Branch {
    /// Base clips in order, `true` for those played once before the rest.
    base: Vec<(usize, bool)>,
    layers: Vec<usize>,
}

impl Branch {
    fn is_empty(&self) -> bool {
        self.base.is_empty() && self.layers.is_empty()
    }

    /// The base clip at `time` and its frame: the `once` clips in turn,
    /// then the first that isn't, looping or held at its end (`looping`);
    /// held at the end of the last without one. The other clips that
    /// aren't `once` are the alternatives of a blend the walk met after it
    /// (a directional hit flinch's five).
    fn base_at<'a>(
        &self,
        clips: &'a [Clip],
        mut time: f64,
        looping: bool,
    ) -> Option<(&'a Clip, f64)> {
        for &(index, once) in &self.base {
            let clip = &clips[index];
            if !once {
                return Some((clip, clip.frame(time, looping)));
            }
            if time < clip.duration {
                return Some((clip, clip.frame(time, false)));
            }
            time -= clip.duration;
        }
        let &(last, _) = self.base.last()?;
        let clip = &clips[last];
        Some((clip, clip.last))
    }
}

/// The locomotion branches the server chooses from.
const STANDING: usize = 0;
const MOVING: usize = 1;
const ARMS_FORWARD_STANDING: usize = 2;
const ARMS_FORWARD_MOVING: usize = 3;
const BRANCHES: [&str; 4] = [
    "standing",
    "moving",
    "arms_forward_standing",
    "arms_forward_moving",
];

/// A skeleton and the clips its blend trees play.
#[derive(Debug)]
pub struct Rig {
    parents: Vec<Option<usize>>,
    rest: Vec<Transform>,
    branches: [Branch; 4],
    /// Each alias's branch without and with the arms-forward category.
    anims: HashMap<u64, [Branch; 2]>,
    clips: Vec<Clip>,
}

/// A statescript `Anim` state the bot's script runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimRequest {
    pub alias: u64,
    /// Seconds since the state began.
    pub elapsed: f64,
    /// The state's play-over duration in seconds, if it has one.
    pub over: Option<f64>,
}

/// What picks a bot's pose this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoseInput {
    /// Seconds on the bot's clock: its layers' time and the standing base's.
    pub age: f64,
    /// Seconds since the bot set off, while it walks.
    pub moving: Option<f64>,
    /// The script pushed the arms-forward category.
    pub arms_forward: bool,
    pub anim: Option<AnimRequest>,
}

impl Rig {
    /// Each bone's transform relative to its parent for `input`.
    fn local(&self, input: &PoseInput) -> Vec<Transform> {
        let mut pose = self.rest.clone();
        let branch = match (input.arms_forward, input.moving.is_some()) {
            (false, false) => STANDING,
            (false, true) => MOVING,
            (true, false) => ARMS_FORWARD_STANDING,
            (true, true) => ARMS_FORWARD_MOVING,
        };
        let locomotion = &self.branches[branch];
        let anim = input.anim.and_then(|request| {
            let branch = &self.anims.get(&request.alias)?[usize::from(input.arms_forward)];
            (!branch.is_empty()).then_some((branch, request))
        });
        let anim_time = |(branch, request): (&Branch, AnimRequest)| {
            let natural = branch
                .base
                .first()
                .map(|(index, _)| *index)
                .or_else(|| branch.layers.first().copied())
                .map_or(0.0, |index| self.clips[index].duration);
            match request.over {
                Some(over) if over > 0.0 => request.elapsed * natural / over,
                _ => request.elapsed,
            }
        };
        let base = match anim {
            Some((branch, request)) if !branch.base.is_empty() => {
                branch.base_at(&self.clips, anim_time((branch, request)), false)
            }
            _ => locomotion.base_at(&self.clips, input.moving.unwrap_or(input.age), true),
        };
        if let Some((clip, frame)) = base {
            clip.replace(frame, &mut pose);
        }
        for &layer in &locomotion.layers {
            let clip = &self.clips[layer];
            clip.add(clip.frame(input.age, true), &mut pose);
        }
        if let Some((branch, request)) = anim {
            let time = anim_time((branch, request));
            for &layer in &branch.layers {
                let clip = &self.clips[layer];
                clip.add(clip.frame(time, false), &mut pose);
            }
        }
        pose
    }

    /// Each bone's place in the body's frame for `input`: the local pose
    /// composed in place, parents first.
    fn places(&self, input: &PoseInput) -> Vec<Transform> {
        let mut pose = self.local(input);
        for index in 0..pose.len() {
            if let Some(parent) = self.parents[index] {
                pose[index] = pose[parent].then(&pose[index]);
            }
        }
        pose
    }
}

/// A hit shape in its bone's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BoneShape {
    bone: usize,
    shape: HitShape,
}

/// A bot body's rig and its shapes on the rig's bones.
#[derive(Debug)]
pub struct BodyRig {
    rig: Arc<Rig>,
    shapes: Vec<BoneShape>,
}

impl BodyRig {
    /// The body's shapes in its frame, posed for `input`: placed by their
    /// bone and scaled by its scale, the radius too (the client tests a
    /// shape in its bone's frame, the ray divided by the bone's scale,
    /// `0x7FF78AA3E2E0`). Under a scale that differs by axis (the first
    /// tenth of a second of the assembly) the client's shape is an
    /// ellipsoid; the radius takes the largest axis's. Then `fixed`, the
    /// model's physics body (`crate::hitboxes`), whose shapes don't move.
    pub fn shapes(&self, input: &PoseInput, fixed: &[HitShape]) -> Arc<[HitShape]> {
        let places = self.rig.places(input);
        self.shapes
            .iter()
            .map(|BoneShape { bone, shape }| {
                let place = &places[*bone];
                HitShape {
                    a: place.place(shape.a),
                    b: place.place(shape.b),
                    radius: shape.radius * place.scale.iter().copied().fold(0.0, f64::max),
                    ..*shape
                }
            })
            .chain(fixed.iter().copied())
            .collect()
    }
}

/// Every animated bot body's rig (`game_bot_animation_174.json`).
#[derive(Debug, Default)]
pub struct BotRigCatalog {
    by_body: HashMap<u64, Arc<BodyRig>>,
}

#[derive(Deserialize)]
struct RawFile {
    bodies: HashMap<String, RawBody>,
    rigs: HashMap<String, RawRig>,
}

#[derive(Deserialize)]
struct RawBody {
    rig: String,
    shapes: Vec<RawShape>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum RawShape {
    Sphere {
        bone: usize,
        centre: Vec3,
        radius: f64,
        critical: bool,
    },
    Capsule {
        bone: usize,
        a: Vec3,
        b: Vec3,
        radius: f64,
        critical: bool,
    },
}

#[derive(Deserialize)]
struct RawRig {
    bones: Vec<RawBone>,
    branches: HashMap<String, RawBranch>,
    anims: HashMap<String, RawAnim>,
    clips: HashMap<String, RawClip>,
}

#[derive(Deserialize)]
pub(crate) struct RawBone {
    parent: Option<usize>,
    rotation: [f64; 4],
    scale: Vec3,
    translation: Vec3,
}

impl RawBone {
    /// The bones' parents, each before its children.
    pub(crate) fn parents(bones: &[Self]) -> io::Result<Vec<Option<usize>>> {
        if bones
            .iter()
            .enumerate()
            .any(|(index, bone)| bone.parent.is_some_and(|parent| parent >= index))
        {
            return Err(invalid("a bone comes before its parent"));
        }
        Ok(bones.iter().map(|bone| bone.parent).collect())
    }

    /// Its transform relative to its parent.
    pub(crate) fn rest(&self) -> Transform {
        Transform {
            rotation: self.rotation,
            scale: self.scale,
            translation: self.translation,
        }
    }
}

#[derive(Deserialize)]
struct RawBranch {
    base: Vec<RawPlayed>,
    layers: Vec<String>,
}

#[derive(Deserialize)]
struct RawPlayed {
    clip: String,
    once: bool,
}

#[derive(Deserialize)]
struct RawAnim {
    plain: RawBranch,
    arms_forward: RawBranch,
}

#[derive(Deserialize)]
pub(crate) struct RawClip {
    duration: f64,
    fps: f64,
    frames: usize,
    /// Its tracks are deltas to add to the pose.
    #[serde(default)]
    pub additive: bool,
    tracks: Vec<RawTrack>,
}

impl RawClip {
    /// The clip on a rig of `count` bones.
    pub(crate) fn clip(self, count: usize) -> io::Result<Clip> {
        let tracks = self
            .tracks
            .into_iter()
            .map(|track| {
                if track.bone >= count {
                    return Err(invalid(format!("track bone {} of {count}", track.bone)));
                }
                Ok(Track {
                    bone: track.bone,
                    positions: keys(track.positions)?,
                    rotations: keys(track.rotations)?,
                    scales: keys(track.scales)?,
                })
            })
            .collect::<io::Result<_>>()?;
        Ok(Clip {
            duration: self.duration,
            fps: self.fps,
            last: self.frames.saturating_sub(1) as f64,
            tracks,
        })
    }
}

#[derive(Deserialize)]
struct RawTrack {
    bone: usize,
    #[serde(default)]
    positions: Vec<Vec<f64>>,
    #[serde(default)]
    rotations: Vec<Vec<f64>>,
    #[serde(default)]
    scales: Vec<Vec<f64>>,
}

pub(crate) fn invalid(error: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.into())
}

/// `[frame, values…]` keys, sorted by frame.
fn keys<const N: usize>(raw: Vec<Vec<f64>>) -> io::Result<Keys<N>> {
    let mut keys: Keys<N> = raw
        .into_iter()
        .map(|key| match key.as_slice() {
            [frame, values @ ..] if values.len() == N => {
                Ok((*frame, std::array::from_fn(|k| values[k])))
            }
            _ => Err(invalid(format!("a key of {} values", key.len()))),
        })
        .collect::<io::Result<_>>()?;
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(keys)
}

impl RawRig {
    fn rig(self) -> io::Result<Rig> {
        let count = self.bones.len();
        let parents = RawBone::parents(&self.bones)?;
        let mut names = Vec::new();
        let mut clips = Vec::new();
        for (name, raw) in self.clips {
            names.push(name);
            clips.push(raw.clip(count)?);
        }
        let clip = |name: &str| {
            names
                .iter()
                .position(|known| known == name)
                .ok_or_else(|| invalid(format!("unknown clip {name}")))
        };
        let branch = |raw: RawBranch| -> io::Result<Branch> {
            Ok(Branch {
                base: raw
                    .base
                    .into_iter()
                    .map(|played| Ok((clip(&played.clip)?, played.once)))
                    .collect::<io::Result<_>>()?,
                layers: raw
                    .layers
                    .iter()
                    .map(|name| clip(name))
                    .collect::<io::Result<_>>()?,
            })
        };
        let mut raw_branches = self.branches;
        let mut branches: [Branch; 4] = Default::default();
        for (slot, name) in BRANCHES.iter().enumerate() {
            let raw = raw_branches
                .remove(*name)
                .ok_or_else(|| invalid(format!("no {name} branch")))?;
            branches[slot] = branch(raw)?;
        }
        let anims = self
            .anims
            .into_iter()
            .map(|(alias, raw)| {
                let alias = parse_guid(&alias)
                    .ok_or_else(|| invalid(format!("invalid alias {alias:?}")))?;
                Ok((alias, [branch(raw.plain)?, branch(raw.arms_forward)?]))
            })
            .collect::<io::Result<_>>()?;
        Ok(Rig {
            parents,
            rest: self.bones.iter().map(RawBone::rest).collect(),
            branches,
            anims,
            clips,
        })
    }
}

impl BotRigCatalog {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_json(&fs::read_to_string(path)?)
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: RawFile =
            serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
        let rigs: HashMap<String, Arc<Rig>> = raw
            .rigs
            .into_iter()
            .map(|(name, rig)| Ok((name, Arc::new(rig.rig()?))))
            .collect::<io::Result<_>>()?;
        let mut by_body = HashMap::new();
        for (body, raw) in raw.bodies {
            let guid =
                parse_guid(&body).ok_or_else(|| invalid(format!("invalid body {body:?}")))?;
            let rig = rigs
                .get(&raw.rig)
                .ok_or_else(|| invalid(format!("{body} names no rig {}", raw.rig)))?;
            let bones = rig.rest.len();
            let shapes = raw
                .shapes
                .into_iter()
                .map(|shape| {
                    let (bone, shape) = match shape {
                        RawShape::Sphere {
                            bone,
                            centre,
                            radius,
                            critical,
                        } => (
                            bone,
                            HitShape {
                                a: centre,
                                b: centre,
                                radius,
                                critical,
                                fixed: false,
                            },
                        ),
                        RawShape::Capsule {
                            bone,
                            a,
                            b,
                            radius,
                            critical,
                        } => (
                            bone,
                            HitShape {
                                a,
                                b,
                                radius,
                                critical,
                                fixed: false,
                            },
                        ),
                    };
                    if bone >= bones {
                        return Err(invalid(format!("{body}: shape bone {bone} of {bones}")));
                    }
                    Ok(BoneShape { bone, shape })
                })
                .collect::<io::Result<_>>()?;
            by_body.insert(
                guid,
                Arc::new(BodyRig {
                    rig: Arc::clone(rig),
                    shapes,
                }),
            );
        }
        Ok(Self { by_body })
    }

    /// The body's rig; `None` for a body without one, which keeps its
    /// static shapes.
    pub fn for_body(&self, body: u64) -> Option<Arc<BodyRig>> {
        self.by_body.get(&body).cloned()
    }
}

/// What a statescript state asks of the animation (see the module notes).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScriptAnim {
    Play { alias: u64, over: Option<f64> },
    Category(u64),
}

/// The bots' graphs' states that play an alias or push an animation
/// category, by graph and state index.
#[derive(Debug, Default)]
pub struct ScriptAnims {
    states: HashMap<(u32, u16), ScriptAnim>,
}

/// A config's literal GUID field, `None` when it is another class or none.
fn literal(config: &Json, class: &str, field: &str) -> Option<u64> {
    (config.get("$")?.as_str()? == class)
        .then(|| parse_guid(config.get(field)?.as_str()?))
        .flatten()
}

impl ScriptAnims {
    pub fn from_graphs(graphs: &GraphStore) -> Self {
        let mut states = HashMap::new();
        for index in graphs.graph_indexes() {
            let Some(graph) = graphs.graph(index) else {
                continue;
            };
            for state in 0..graph.states.len() {
                let Some(node) = graph.state(state) else {
                    continue;
                };
                let fields = &node.fields;
                let anim = match node.class.as_str() {
                    "STUStatescriptStateAnim" => literal(
                        &fields["m_11E0A658"],
                        "STUConfigVarAnimAlias",
                        "m_animAlias",
                    )
                    .map(|alias| {
                        let over = &fields["m_86701C3E"];
                        let over = matches!(
                            over["$"].as_str(),
                            Some("STUConfigVarFloat" | "STUConfigVarInt")
                        )
                        .then(|| over["m_value"].as_f64())
                        .flatten()
                        .filter(|seconds| *seconds > 0.0);
                        ScriptAnim::Play { alias, over }
                    }),
                    "STUStatescriptStateStack"
                        if fields["m_out_Var"]["m_identifier"].as_u64()
                            == Some(CATEGORY_VARIABLE) =>
                    {
                        literal(&fields["m_value"], "STUConfigVarAnimCategory", "m_category")
                            .map(ScriptAnim::Category)
                    }
                    _ => None,
                };
                if let (Some(anim), Ok(state)) = (anim, u16::try_from(state)) {
                    states.insert((index, state), anim);
                }
            }
        }
        Self { states }
    }

    /// What the script's active states ask: the arms-forward category, the
    /// latest started `Anim` state.
    pub fn requests(&self, snapshot: &Snapshot) -> (bool, Option<AnimRequest>) {
        let mut arms_forward = false;
        let mut latest: Option<(u64, AnimRequest)> = None;
        for instance in snapshot.instances.values() {
            for (state, entry) in instance.states.iter().filter(|(_, entry)| entry.active) {
                match self.states.get(&(instance.graph, *state)) {
                    Some(ScriptAnim::Category(category)) => {
                        arms_forward |= *category == ARMS_FORWARD;
                    }
                    Some(&ScriptAnim::Play { alias, over })
                        if latest.is_none_or(|(start, _)| entry.start_ms >= start) =>
                    {
                        let elapsed = snapshot.now_ms.saturating_sub(entry.start_ms);
                        latest = Some((
                            entry.start_ms,
                            AnimRequest {
                                alias,
                                elapsed: elapsed as f64 / 1000.0,
                                over,
                            },
                        ));
                    }
                    _ => {}
                }
            }
        }
        (arms_forward, latest.map(|(_, request)| request))
    }
}

/// A bot's animation: its rig, clocks and this tick's shapes, its physics
/// body's fixed ones after the posed ones.
#[derive(Clone, Debug)]
pub struct BotAnimation {
    body: Arc<BodyRig>,
    /// The model's physics body (`crate::hitboxes`): empty for a body the
    /// hitboxes don't list.
    fixed: Arc<[HitShape]>,
    /// The first tick the bot was animated: its clock's start.
    born: Option<u32>,
    /// The tick it set off, while it walks.
    set_off: Option<u32>,
    shapes: Arc<[HitShape]>,
}

impl BotAnimation {
    /// A new body, posed standing at the start of its clock.
    pub fn new(body: Arc<BodyRig>, fixed: Arc<[HitShape]>) -> Self {
        let shapes = body.shapes(&PoseInput::default(), &fixed);
        Self {
            body,
            fixed,
            born: None,
            set_off: None,
            shapes,
        }
    }

    /// The same body made again (a respawn): its clocks start over.
    pub fn fresh(&self) -> Self {
        Self::new(Arc::clone(&self.body), Arc::clone(&self.fixed))
    }

    /// Pose the body at `tick`: `walking` its route, with the script's
    /// requests (see the module notes).
    pub fn advance(
        &mut self,
        tick: u32,
        walking: bool,
        arms_forward: bool,
        anim: Option<AnimRequest>,
    ) {
        let born = *self.born.get_or_insert(tick);
        self.set_off = if walking {
            Some(self.set_off.unwrap_or(tick))
        } else {
            None
        };
        let seconds = |from: u32| f64::from(tick.wrapping_sub(from)) * TICK_PERIOD.as_secs_f64();
        self.shapes = self.body.shapes(
            &PoseInput {
                age: seconds(born),
                moving: self.set_off.map(seconds),
                arms_forward,
                anim,
            },
            &self.fixed,
        );
    }

    /// The body's shapes as posed last, then its fixed ones.
    pub fn shapes(&self) -> Arc<[HitShape]> {
        Arc::clone(&self.shapes)
    }
}

#[cfg(test)]
mod tests;
