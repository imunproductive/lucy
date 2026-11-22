//! `data/game_hitboxes_174.json`: the shapes a shot hits on each hero and
//! bot body, in the body's frame: the bots' in their idle pose, the heroes'
//! at rest.
//!
//! The bodies are those of `data/game_heroes_174.json` and the bots' of
//! `data/ai_spawners_174.json`, read from the build folder. A body's model is
//! its first STUModelComponent model that isn't a client-only breakable, as
//! for the props ([`crate::props`]).
//!
//! **Which shapes.** A model's STUModel (MSTU chunk) lists its physics parts
//! in `m_CB4D298D` {`m_B7C8314A` shape, `m_9B0424EB` holder, `m_89AABA94`
//! motion type, `m_30F925F4` node}. The props module notes that the client
//! puts the parts of motion type 1 in a list of their own, outside the
//! physics world. On every body model these parts hang on the skeleton,
//! are held by `STU_00066F4C` (`STU_4EFF450F` {name, `m_888BCD8B` bone
//! label, `m_EDF0511C` id} plus `m_EE483487`, a damage zone), and are
//! spheres and capsules, 7 to 33 of them, on the head, chest, arms and
//! legs; motion type 4 holds the ragdoll bodies. We take the motion type 1
//! parts to be the hit shapes (assumption: nothing in the data names the
//! list). A few of them are polytopes or meshes, which this file leaves
//! out (a gap; no bot has any): 4 polytopes of Bastion, 1 of Junkrat, 1 of
//! Wrecking Ball, and 6 meshes of Echo (her wings).
//!
//! A model may name a second STUModel in `m_37ED05D0`. The models of the
//! seven heroes made last (Brigitte, Moira, Wrecking Ball, Ashe, Echo,
//! Baptiste, Sigma) list no motion type 1 parts of their own; only the
//! model they name does. Each of the 25 older heroes' models lists them
//! both itself and in the model it names: the same geometry on 20 of
//! them, and on 5 a few retuned shapes (Tracer's head a capsule instead of
//! a sphere, Reinhardt's head a sphere instead of a capsule, Mercy's wings,
//! and two more parts in her own list, Torbjörn's head 0.233 instead of
//! 0.25 m, Bastion within 5 mm). We read the named model's list when a
//! model names one, and the model's own otherwise (assumption: the named
//! model holds the live set, its own list being an older copy; the data
//! does not say).
//!
//! **Where they are.** Each part sits in the frame of the node it names.
//! The node number is a skeleton bone id: the low half of the bone label
//! GUID (`088`) the holder names, and an id of the skeleton chunk (`mskl`).
//! The skeleton is ported from OWLib (TankLib `teModelChunk_Skeleton`,
//! `ExportFormats/OverwatchModel.cs`, MIT): a header of eleven i64 offsets
//! (hierarchy, 4×4 matrices and their inverses, 4×3 matrices and their
//! inverses, …, bone ids at the seventh, …) then `u16` counts from byte 92
//! (bones first). Each 4×3 matrix is three rows of four floats: the
//! rotation (x, y, z, w), the scale, the translation. The 4×3 matrices
//! place each bone in the model (OWLib's `GetWorldSpace`); the inverse set
//! holds each bone relative to its parent (its `GetRefPoseTransform`).
//! Checked on every body's model: each bone of the STUModel's own bone
//! table (`m_FD232466` entries `STU_CB79EE74`, ids `088` GUIDs) has the
//! skeleton's parent-relative transform for that id (all of them, but two
//! of Bastion's 178), and on the Training Bot each bone's model place is
//! its parent's composed with it. The STU table alone cannot place a part:
//! its bones name no parents, so read as roots (as the props' `node_frame`
//! does) the Training Bot's head bone `0x11` would be 0.12 m up instead of
//! 1.33 m. A point `p` of a part is at `t + q·(s ⊙ p)` with the bone's
//! model matrix, and a radius is scaled by `s`. The idle poses scale hit
//! bones only uniformly (since scales unpack as `v/1024`, e.g. `0x1403`'s
//! bone `931` at 0.947); a non-uniform scale would bend a sphere, so it is
//! refused.
//!
//! **Axes.** The model frame is the body's: Y up, the origin at the feet
//! (the foot bones within 0.15 m of it, the Training Bot's lowest sphere's
//! centre 0.30 m up), metres. Forward is +Z: the Training Bot holds both
//! arms out along +Z (capsules from z 0 to 0.87 m) and its head bones
//! `0x69`, `0x6A` sit at z +0.17, +0.32; the foot bones of the heroes reach
//! further along +Z than back. The world's forward at yaw `y` (1/65536
//! turns) is `[sin y, 0, cos y]` (the bots' own aim, combat.py `aim`), so a
//! body point `p` is in the world at `feet + R(y)·p` with `R(y)` the turn
//! about +Y that takes +Z to that forward. OWLib's model exporter writes
//! positions as stored and the props module places model shapes by the
//! placeable's rotation alone (checked against the client's own world), so
//! nothing is mirrored. Which side is the body's right cannot be checked
//! from the rest pose: the heroes stand in a symmetric pose, the weapon
//! hand is not apparent (the Training Bot's two arms differ by 1 cm).
//!
//! **Critical.** A body's STUHealthComponent (`0xA658CA1C`) lists damage
//! zones in `m_73AEA60D` (`STU_9EA55F3D` {`m_D3F2EBDE` config, `m_EE483487`
//! zone}); a config (`STU_99720FE9`, `05C`) has a multiplier
//! `m_13B2D647`. A part is critical when its holder's zone is one its body
//! lists with a multiplier above 1. The data: every hero body and the
//! Training Bots list zone `0DA:000D` with config `05C:0015`, multiplier 2;
//! Bastion also lists `0DA:0015` with the same config. On every model
//! exactly one part carries `0DA:000D` (on bone `0x11` on the 26 bodies
//! checked: the Training Bot and the 25 heroes before Brigitte): the top part of
//! the model on 28 of the 32 heroes and on the Training Bots (not on
//! Reinhardt, 6 mm under another part, Bastion, Torbjörn, and Wrecking
//! Ball, whose hamster's head tops at 0.63 m in the ball); at rest the
//! Training Bot's tops at 1.92 m, posed (below) at 2.42 m.
//! Bastion's second zone is on one part on bone `0x933`. The Friendly
//! Bots' bodies (`143E`, `146E`) list no zone, so their model's head part
//! is not critical there. The 2 is not used: a shot's critical multiplier
//! is its weapon's (`HitDefinition::critical`).
//!
//! **Pose.** In the game the Training Bot hovers with its arms hanging
//! down; at rest its body stands on the ground with the arms forward, the
//! head sphere 0.5 m below the head players see. A bot body (those of
//! `data/ai_spawners_174.json`) is placed by its idle clip: of the clips
//! its blend tree set plays with no animation category set
//! ([`crate::blend_trees`]), the one that keys every bone of the skeleton.
//! Each of its tracks replaces that bone's parent-relative transform
//! ([`crate::animation`]) and the bones are placed again from the root.
//! All six bot bodies share the Training Bot's model and set (`5A8.021`);
//! their trees reach four clips, one keying all 43 bones: `006:3920`,
//! bound to alias `805A8009`, the standing branch of the locomotion tree
//! (the moving one, category `0005`, plays `3990` and `3952`). It holds one
//! key a bone: it lifts the body bone `0x2` by 0.5 m (0.824 to 1.324 m)
//! and turns the upper arms (`0xD`, `0x36`) a quarter turn down, so the
//! head sphere's centre is at 1.98 m (the mover's eye is at 2.0), the
//! lowest sphere 0.30 m above the feet and the arm capsules vertical, from
//! 1.46 to 0.59 m. The first frame is every frame. The trees also play a
//! 4 s layer, `391F` (14 bones, added to the pose: its body bone moves
//! −0.04 to +0.03 m), which bobs the bot by ±4 cm; this file leaves it out.
//! The server animates the bots instead, bob, walk and all, from
//! `data/game_bot_animation_174.json` ([`crate::bot_animation`], written
//! by this derivation too: each bot body's shapes on their bones and its
//! rig); it uses the static shapes here only for a body without a rig. A
//! hero's tree plays its base from an animation slot the game fills: on 29
//! of the 32 heroes the walk reaches no clip keying every bone (Reinhardt
//! one, Winston two, untested); the heroes stay at rest (players are hit by
//! the capsule, and the file keeps their shapes for later).
//!
//! **Hardpoints.** A model's STUModel lists its hardpoints in `m_C45F5F6F`:
//! `STU_BE9DDFDF` {`m_EDF0511C` id, a `0DC` GUID; `m_FF592924` the bone it
//! hangs on, an `088` GUID whose low half is the skeleton's bone id;
//! `m_7DC1550F` its translation and `m_AF9D3A0C` its rotation in that
//! bone's frame} (the props module reads the same records as nodes). The
//! graphs ask for a body's hardpoint with `STU_FC99C77E` (Soldier: 76's
//! Tactical Visor aims its rifle at `0DC.0001` of its target, `0254` st10's
//! `m_E2FD6F04`). The Training Bot's model lists 23, `0001` at the origin
//! of bone `0x3` (1.32 m up, posed); every body has a `0001` at its chest
//! (Tracer 1.30 m, Soldier: 76 1.52 m, Reinhardt 1.75 m). A model without
//! a list of its own takes the one of the model it names. Each is placed
//! by its bone in the pose the shapes have (the bots' idle pose, the
//! heroes' rest).
//!
//! The file: `{body: {"name", "model" (the model whose list was read),
//! "pose" (the idle clip, bots only), "shapes": [{"kind": "sphere",
//! "centre", "radius", "critical"} | {"kind": "capsule", "a", "b",
//! "radius", "critical"}], "hardpoints": {GUID: point}}}`, sorted by body,
//! the shapes in list order, the numbers as `f32`. A body without a model,
//! skeleton or parts has no shapes; one whose bones hold no hardpoints has
//! none.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use orpheus_stu::{Asset, Object, Value};
use serde_json::{Map, json};

use crate::{
    Install, Output, Result,
    animation::{self, Bone},
    asset_error, blend_trees, bot_animation, guid_hex,
    heroes::{Assets, bodies, movers::float, parse_guid, read_json},
    props::{chunk, i64_at, model_components, slice},
};

const ENTITY: u16 = 0x003;
/// STUHealthComponent and its zones: m_73AEA60D {m_D3F2EBDE config,
/// m_EE483487 zone}; the config's multiplier.
const HEALTH: u32 = 0xA658_CA1C;
const ZONES: u32 = 0x73AE_A60D;
const ZONE_CONFIG: u32 = 0xD3F2_EBDE;
const ZONE: u32 = 0xEE48_3487;
const MULTIPLIER: u32 = 0x13B2_D647;
/// STUModel: the shape list and the model it names.
const SHAPES: u32 = 0xCB4D_298D;
const LINKED: u32 = 0x37ED_05D0;
/// A shape list entry (STU_2DD96D6D) and the hit holder STU_00066F4C.
const SHAPE: u32 = 0xB7C8_314A;
const HOLDER: u32 = 0x9B04_24EB;
const MOTION: u32 = 0x89AA_BA94;
const NODE: u32 = 0x30F9_25F4;
const HIT_MOTION: f64 = 1.0;
const HIT_HOLDER: u32 = 0x0006_6F4C;
const SPHERE: u32 = 0x4614_D0E5;
const CAPSULE: u32 = 0x4C8D_9F5E;
const CENTRE: u32 = 0xA8D1_2F28;
const POINT_A: u32 = 0x10EF_DE90;
const POINT_B: u32 = 0xC320_C90B;
const RADIUS: u32 = 0xED61_D926;
/// STUModel's hardpoints and their fields (see the module notes).
const HARDPOINTS: u32 = 0xC45F_5F6F;
const HARDPOINT: u32 = 0xBE9D_DFDF;
const NODE_ID: u32 = 0xEDF0_511C;
const NODE_PARENT: u32 = 0xFF59_2924;
const NODE_TRANSLATION: u32 = 0x7DC1_550F;
/// Byte 92 of the skeleton header: the bone count.
const BONE_COUNT: usize = 92;
/// The header's offsets (i64 each): the hierarchy, the 4×3 matrices and
/// their inverses, the bone ids.
const HIERARCHY: usize = 0;
const MODEL_MATRICES: usize = 3;
const PARENT_MATRICES: usize = 4;
const IDS: usize = 6;

/// A model's skeleton (`mskl`): each bone's id, parent, place in the model
/// and transform relative to its parent.
pub(crate) struct Skeleton {
    pub ids: Vec<u32>,
    pub parents: Vec<Option<usize>>,
    model: Vec<Bone>,
    pub local: Vec<Bone>,
}

impl Skeleton {
    pub(crate) fn read(data: &[u8]) -> Result<Option<Self>, String> {
        let Some(payload) = chunk(data, b"mskl")? else {
            return Ok(None);
        };
        let count = usize::from(u16::from_le_bytes(bytes(payload, BONE_COUNT)?));
        let ids = offset(payload, IDS)?;
        let hierarchy = offset(payload, HIERARCHY)?;
        Ok(Some(Self {
            ids: (0..count)
                .map(|k| bytes(payload, ids + 4 * k).map(u32::from_le_bytes))
                .collect::<Result<_, _>>()?,
            // Six bytes a bone, the parent index in the last two (OWLib
            // skips four), -1 for a root.
            parents: (0..count)
                .map(|k| {
                    bytes(payload, hierarchy + 6 * k + 4)
                        .map(|parent| usize::try_from(i16::from_le_bytes(parent)).ok())
                })
                .collect::<Result<_, _>>()?,
            model: bones(payload, offset(payload, MODEL_MATRICES)?, count)?,
            local: bones(payload, offset(payload, PARENT_MATRICES)?, count)?,
        }))
    }

    pub(crate) fn bone(&self, id: u32) -> Option<usize> {
        self.ids.iter().position(|bone| *bone == id)
    }

    /// Each bone's place in the model for parent-relative transforms
    /// `local` (one a bone, in the skeleton's order, where each parent
    /// comes before its children).
    fn pose(&self, local: &[Bone]) -> Result<Vec<Bone>, String> {
        let mut model: Vec<Bone> = Vec::with_capacity(local.len());
        for (index, bone) in local.iter().enumerate() {
            model.push(match self.parents[index] {
                None => *bone,
                Some(parent) => compose(
                    model
                        .get(parent)
                        .ok_or_else(|| format!("bone {index} comes before its parent"))?,
                    bone,
                ),
            });
        }
        Ok(model)
    }
}

fn bytes<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], String> {
    data.get(at..at + N)
        .map(|bytes| bytes.try_into().expect("N bytes"))
        .ok_or_else(|| "truncated skeleton".to_owned())
}

fn offset(header: &[u8], field: usize) -> Result<usize, String> {
    usize::try_from(i64_at(header, 8 * field)?).map_err(|_| "bad skeleton offset".to_owned())
}

/// `count` 4×3 matrices from `at`: rows of four floats.
fn bones(data: &[u8], at: usize, count: usize) -> Result<Vec<Bone>, String> {
    (0..count)
        .map(|k| {
            let value = |row: usize, column: usize| {
                bytes(data, at + 48 * k + 16 * row + 4 * column).map(f32::from_le_bytes)
            };
            Ok(Bone {
                rotation: [value(0, 0)?, value(0, 1)?, value(0, 2)?, value(0, 3)?],
                scale: [value(1, 0)?, value(1, 1)?, value(1, 2)?],
                translation: [value(2, 0)?, value(2, 1)?, value(2, 2)?],
            })
        })
        .collect()
}

/// v turned by the quaternion q (x, y, z, w).
fn rotate(q: [f32; 4], v: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = q.map(f64::from);
    let [vx, vy, vz] = v;
    let (cx, cy, cz) = (y * vz - z * vy, z * vx - x * vz, x * vy - y * vx);
    let (ux, uy, uz) = (w * vx + cx, w * vy + cy, w * vz + cz);
    let (tx, ty, tz) = (y * uz - z * uy, z * ux - x * uz, x * uy - y * ux);
    [vx + 2.0 * tx, vy + 2.0 * ty, vz + 2.0 * tz]
}

/// A point of a bone's frame in the model: scaled, turned, moved.
fn place(bone: &Bone, point: [f32; 3]) -> [f64; 3] {
    let scaled = std::array::from_fn(|k| f64::from(point[k]) * f64::from(bone.scale[k]));
    let turned = rotate(bone.rotation, scaled);
    std::array::from_fn(|k| turned[k] + f64::from(bone.translation[k]))
}

/// A child bone's transform relative to its parent, placed in the model by
/// the parent's (the scales multiply, as for the unit and uniform scales
/// the bodies have).
fn compose(parent: &Bone, child: &Bone) -> Bone {
    let [x1, y1, z1, w1] = parent.rotation.map(f64::from);
    let [x2, y2, z2, w2] = child.rotation.map(f64::from);
    Bone {
        rotation: [
            w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
            w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
            w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
            w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
        ]
        .map(|v| v as f32),
        scale: std::array::from_fn(|k| parent.scale[k] * child.scale[k]),
        translation: place(parent, child.translation).map(|v| v as f32),
    }
}

/// A part's geometry in its bone's frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Geometry {
    Sphere([f32; 3], f32),
    Capsule([f32; 3], [f32; 3], f32),
}

/// A motion type 1 part: its node, geometry and damage zone (0 none).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Part {
    pub node: u32,
    pub geometry: Geometry,
    zone: u64,
}

impl Part {
    /// Its body's damage zones list its zone with a multiplier above 1
    /// (see the module notes).
    pub(crate) fn critical(&self, zones: &HashMap<u64, f64>) -> bool {
        self.zone
            .ne(&0)
            .then(|| zones.get(&self.zone))
            .flatten()
            .is_some_and(|multiplier| *multiplier > 1.0)
    }
}

/// The STUModel of a model file (its MSTU chunk), decoded.
fn model_stu(data: &[u8]) -> Result<Option<Asset>, String> {
    let Some(payload) = chunk(data, b"MSTU")? else {
        return Ok(None);
    };
    let start = usize::try_from(i64_at(payload, 0)?).map_err(|_| "bad MSTU offset")?;
    let size = usize::try_from(i64_at(payload, 8)?).map_err(|_| "bad MSTU size")?;
    orpheus_stu::read(slice(payload, start, start + size))
        .map(Some)
        .map_err(|error| error.to_string())
}

fn field(object: &Object, hash: u32) -> Option<&Value> {
    object
        .fields
        .iter()
        .find(|(field, _)| *field == hash)
        .map(|(_, value)| value)
}

fn floats<const N: usize>(object: &Object, hash: u32) -> Option<[f32; N]> {
    match field(object, hash) {
        Some(Value::Floats(values)) => values.get(..N)?.try_into().ok(),
        _ => None,
    }
}

fn number(object: &Object, hash: u32) -> Option<f64> {
    match field(object, hash)? {
        Value::Float(value) => Some(*value),
        Value::Int(value) => Some(*value as f64),
        Value::UInt(value) => Some(*value as f64),
        _ => None,
    }
}

fn reference(object: &Object, hash: u32) -> u64 {
    match field(object, hash) {
        Some(Value::Guid(guid)) => *guid,
        _ => 0,
    }
}

/// The model's motion type 1 spheres and capsules held by STU_00066F4C, in
/// list order.
fn parts(stu: &Asset) -> Result<Vec<Part>, String> {
    let Some(Value::Array(list)) = stu.root().and_then(|root| field(root, SHAPES)) else {
        return Ok(Vec::new());
    };
    let mut parts = Vec::new();
    for entry in list.iter().filter_map(|item| stu.resolve(item)) {
        if number(entry, MOTION) != Some(HIT_MOTION) {
            continue;
        }
        let Some(holder) = field(entry, HOLDER)
            .and_then(|holder| stu.resolve(holder))
            .filter(|holder| holder.class == HIT_HOLDER)
        else {
            continue;
        };
        let Some(shape) = field(entry, SHAPE).and_then(|shape| stu.resolve(shape)) else {
            continue;
        };
        let point = |hash| floats::<3>(shape, hash).ok_or("a part without its points");
        let radius = || {
            number(shape, RADIUS)
                .ok_or("a part without a radius")
                .map(|r| r as f32)
        };
        let geometry = match shape.class {
            SPHERE => Geometry::Sphere(point(CENTRE)?, radius()?),
            CAPSULE => Geometry::Capsule(point(POINT_A)?, point(POINT_B)?, radius()?),
            // Polytopes and meshes (see the module notes) are left out.
            _ => continue,
        };
        parts.push(Part {
            node: number(entry, NODE).ok_or("a part without a node")? as u32,
            geometry,
            zone: reference(holder, ZONE),
        });
    }
    Ok(parts)
}

/// A hardpoint: its id (a `0DC` GUID), the skeleton bone id it hangs on
/// and its point in that bone's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hardpoint {
    id: u64,
    bone: u32,
    translation: [f32; 3],
}

/// The model's hardpoints (see the module notes), in list order.
fn hardpoints(stu: &Asset) -> Vec<Hardpoint> {
    let Some(Value::Array(list)) = stu.root().and_then(|root| field(root, HARDPOINTS)) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| stu.resolve(item))
        .filter(|node| node.class == HARDPOINT)
        .filter_map(|node| {
            let (Some(Value::UInt(id)), Some(Value::UInt(parent))) =
                (field(node, NODE_ID), field(node, NODE_PARENT))
            else {
                return None;
            };
            Some(Hardpoint {
                id: *id,
                bone: *parent as u32,
                translation: floats(node, NODE_TRANSLATION).unwrap_or_default(),
            })
        })
        .collect()
}

/// What a body's model gives its hit shapes and hardpoints.
struct HitModel {
    /// The model whose parts were read.
    source: u64,
    parts: Vec<Part>,
    hardpoints: Vec<Hardpoint>,
    skeleton: Skeleton,
}

/// The model's hit parts, hardpoints and skeleton: the parts of the model
/// it names, if it names one, else its own; its own hardpoints, else those
/// of the model it names (see the module notes); `None` without an
/// STUModel or a skeleton.
fn hit_model(install: &Install, model: u64) -> Result<Option<HitModel>, String> {
    let data = install.raw(model).map_err(|error| error.to_string())?;
    let (Some(stu), Some(skeleton)) = (model_stu(&data)?, Skeleton::read(&data)?) else {
        return Ok(None);
    };
    let own = hardpoints(&stu);
    let linked = stu.root().map_or(0, |root| reference(root, LINKED));
    if linked == 0 {
        return Ok(Some(HitModel {
            source: model,
            parts: parts(&stu)?,
            hardpoints: own,
            skeleton,
        }));
    }
    let named = install.raw(linked).map_err(|error| error.to_string())?;
    let named = model_stu(&named)?.ok_or("the named model has no STUModel")?;
    Ok(Some(HitModel {
        source: linked,
        parts: parts(&named)?,
        hardpoints: if own.is_empty() {
            hardpoints(&named)
        } else {
            own
        },
        skeleton,
    }))
}

/// The body's damage zones and each one's multiplier (see the module
/// notes).
fn zones(install: &Install, body: &Asset) -> Result<HashMap<u64, f64>> {
    let mut zones = HashMap::new();
    for health in body
        .instances
        .iter()
        .flatten()
        .filter(|o| o.class == HEALTH)
    {
        let Some(Value::Array(list)) = field(health, ZONES) else {
            continue;
        };
        for item in list.iter().filter_map(|item| body.resolve(item)) {
            let config = install.stu(reference(item, ZONE_CONFIG))?;
            let multiplier = config
                .root()
                .and_then(|root| number(root, MULTIPLIER))
                .unwrap_or(0.0);
            zones.insert(reference(item, ZONE), multiplier);
        }
    }
    Ok(zones)
}

fn point(value: [f64; 3]) -> serde_json::Value {
    json!(value.map(float))
}

/// A bot body's blend tree set: its model component's `m_animBlendTreeSet`
/// for `model`, 0 without one.
fn blend_tree_set(body: &Asset, model: u64) -> u64 {
    body.instances
        .iter()
        .flatten()
        .find(|object| object.get("m_model") == Some(&Value::Guid(model)))
        .and_then(|component| match component.get("m_animBlendTreeSet") {
            Some(Value::Guid(set)) => Some(*set),
            _ => None,
        })
        .unwrap_or(0)
}

/// The idle pose of a skeleton by its blend tree set (see the module
/// notes): the one clip of the set's idle clips that keys every bone, at
/// its first frame; `None` when there is no such clip or more than one.
fn idle_pose(
    install: &Install,
    set: u64,
    skeleton: &Skeleton,
) -> Result<Option<(u64, Vec<Bone>)>, String> {
    let mut full = Vec::new();
    for clip in blend_trees::idle_clips(install, set)? {
        let data = install.raw(clip).map_err(|error| error.to_string())?;
        let animation = animation::read(&data)?;
        let keyed = |id: &u32| animation.tracks.iter().any(|track| track.bone == *id);
        if skeleton.ids.iter().all(keyed) {
            full.push((clip, animation));
        }
    }
    let [(clip, animation)] = &full[..] else {
        return Ok(None);
    };
    let mut local = skeleton.local.clone();
    for track in &animation.tracks {
        if let Some(index) = skeleton.bone(track.bone) {
            local[index] = track.sample(0.0, local[index]);
        }
    }
    Ok(Some((*clip, skeleton.pose(&local)?)))
}

/// The body's shapes as the file writes them, each part on its bone's
/// place `model` (the skeleton's order).
fn shapes(
    parts: &[Part],
    skeleton: &Skeleton,
    model: &[Bone],
    zones: &HashMap<u64, f64>,
) -> Result<Vec<serde_json::Value>, String> {
    parts
        .iter()
        .map(|part| {
            let bone = skeleton
                .bone(part.node)
                .map(|index| &model[index])
                .ok_or_else(|| format!("bone {:X} is not in the skeleton", part.node))?;
            let [sx, sy, sz] = bone.scale;
            if (sx - sy).abs() > 1e-3 || (sx - sz).abs() > 1e-3 {
                return Err(format!(
                    "bone {:X} is scaled unevenly {:?}",
                    part.node, bone.scale
                ));
            }
            // Within 1e-3 of 1 (float noise of the composed scales) the
            // radius stays as the file has it.
            let scale = if (sx - 1.0).abs() > 1e-3 {
                f64::from(sx)
            } else {
                1.0
            };
            let radius = |radius: f32| float(f64::from(radius) * scale);
            let critical = part.critical(zones);
            Ok(match part.geometry {
                Geometry::Sphere(centre, r) => json!({
                    "kind": "sphere",
                    "centre": point(place(bone, centre)),
                    "radius": radius(r),
                    "critical": critical,
                }),
                Geometry::Capsule(a, b, r) => json!({
                    "kind": "capsule",
                    "a": point(place(bone, a)),
                    "b": point(place(bone, b)),
                    "radius": radius(r),
                    "critical": critical,
                }),
            })
        })
        .collect()
}

/// The body keys of `data/ai_spawners_174.json`'s bots, when the build
/// folder has it.
fn bot_bodies(out: &Path) -> Result<HashSet<String>> {
    if !out.join("data/ai_spawners_174.json").is_file() {
        return Ok(HashSet::new());
    }
    let spawners = read_json(out, "data/ai_spawners_174.json")?;
    Ok(spawners
        .get("heroes")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|bots| bots.values())
        .filter_map(|bot| bot.get("body").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect())
}

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let assets = Assets::of(install, ENTITY);
    let bots = bot_bodies(out)?;
    let mut table = BTreeMap::new();
    let mut animated = BTreeMap::new();
    let mut rigs = BTreeMap::new();
    for record in bodies(out)? {
        let key = record
            .get("body")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| asset_error(format!("a hero without a body: {record}")))?;
        let body = parse_guid(key)?;
        let Some(asset) = assets.read(install, body)? else {
            continue;
        };
        let mut entry = Map::new();
        entry.insert(
            "name".into(),
            record.get("name").cloned().unwrap_or_default(),
        );
        let mut listed = Vec::new();
        let mut placed = Map::new();
        if let Some((model, _)) = model_components(&asset).into_iter().find(|(_, c)| !c) {
            let failed =
                |reason: String| asset_error(format!("{key} model {}: {reason}", guid_hex(model)));
            if let Some(HitModel {
                source,
                parts,
                hardpoints,
                skeleton,
            }) = hit_model(install, model).map_err(failed)?
            {
                entry.insert("model".into(), json!(guid_hex(source)));
                let set = blend_tree_set(&asset, model);
                let pose = if bots.contains(key) && set != 0 {
                    idle_pose(install, set, &skeleton).map_err(failed)?
                } else {
                    None
                };
                let places = match pose {
                    Some((clip, places)) => {
                        entry.insert("pose".into(), json!(guid_hex(clip)));
                        places
                    }
                    None => skeleton.model.clone(),
                };
                let zones = zones(install, &asset)?;
                listed = shapes(&parts, &skeleton, &places, &zones).map_err(failed)?;
                // A hardpoint on a bone the skeleton lacks has no place.
                for hardpoint in hardpoints {
                    if let Some(index) = skeleton.bone(hardpoint.bone) {
                        placed.insert(
                            guid_hex(hardpoint.id),
                            point(place(&places[index], hardpoint.translation)),
                        );
                    }
                }
                if bots.contains(key) && set != 0 {
                    let rig = format!("{}/{}", guid_hex(model), guid_hex(set));
                    if !rigs.contains_key(&rig) {
                        let derived =
                            bot_animation::rig(install, set, &skeleton).map_err(failed)?;
                        rigs.insert(rig.clone(), derived);
                    }
                    animated.insert(
                        key.to_owned(),
                        json!({
                            "name": record.get("name").cloned().unwrap_or_default(),
                            "rig": rig,
                            "shapes": bot_animation::shapes(&parts, &skeleton, &zones)
                                .map_err(failed)?,
                        }),
                    );
                }
            }
        }
        entry.insert("shapes".into(), json!(listed));
        entry.insert("hardpoints".into(), serde_json::Value::Object(placed));
        table.insert(key.to_owned(), serde_json::Value::Object(entry));
    }
    let table: Map<String, serde_json::Value> = table.into_iter().collect();
    let animation = json!({
        "bodies": animated.into_iter().collect::<Map<_, _>>(),
        "rigs": rigs.into_iter().collect::<Map<_, _>>(),
    });
    Ok(vec![
        Output::json("data/game_hitboxes_174.json", &table.into()),
        Output {
            path: "data/game_bot_animation_174.json".into(),
            bytes: serde_json::to_vec(&animation).expect("JSON values serialize"),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAINING_BOT_MODEL: u64 = 0x0D00_0000_0000_312E;
    /// STUModel m_FD232466: the model's bone table, STU_CB79EE74 {m_EDF0511C
    /// id, m_7DC1550F translation, m_AF9D3A0C rotation}.
    const BONES: u32 = 0xFD23_2466;
    const BONE: u32 = 0xCB79_EE74;
    const BONE_ID: u32 = 0xEDF0_511C;
    const TRANSLATION: u32 = 0x7DC1_550F;
    const ROTATION: u32 = 0xAF9D_3A0C;

    fn distance(a: [f32; 3], b: [f64; 3]) -> f64 {
        (0..3)
            .map(|k| (f64::from(a[k]) - b[k]).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    /// The skeleton's 4×3 matrices place each bone in the model (its
    /// parent's place composed with its parent-relative one, the inverse
    /// set), and the STUModel's bone table agrees with the parent-relative
    /// set for every bone.
    #[test]
    #[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
    fn the_skeleton_places_the_bones_the_model_table_lists() {
        let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
        let data = install.raw(TRAINING_BOT_MODEL).unwrap();
        let skeleton = Skeleton::read(&data).unwrap().unwrap();
        assert_eq!(skeleton.ids.len(), 43);
        assert_eq!(skeleton.parents[0], None);
        assert!(skeleton.parents[1..].iter().all(Option::is_some));
        let posed = skeleton.pose(&skeleton.local).unwrap();
        for (index, (model, posed)) in skeleton.model.iter().zip(&posed).enumerate() {
            let translation = posed.translation.map(f64::from);
            assert!(
                distance(model.translation, translation) < 1e-5,
                "bone {index}"
            );
            let dot: f32 = (0..4).map(|k| model.rotation[k] * posed.rotation[k]).sum();
            assert!(dot.abs() > 1.0 - 1e-5, "bone {index}");
        }
        let stu = model_stu(&data).unwrap().unwrap();
        let Some(Value::Array(table)) = stu.root().and_then(|root| field(root, BONES)) else {
            panic!("the model has no bone table");
        };
        let mut agree = 0;
        for bone in table
            .iter()
            .filter_map(|item| stu.resolve(item))
            .filter(|bone| bone.class == BONE)
        {
            let Some(Value::UInt(id)) = field(bone, BONE_ID) else {
                panic!("a bone without an id");
            };
            let id = *id as u32;
            let local = &skeleton.local[skeleton.bone(id).unwrap()];
            let translation = floats::<3>(bone, TRANSLATION).unwrap_or_default();
            let rotation = floats::<4>(bone, ROTATION).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            assert!(
                distance(translation, local.translation.map(f64::from)) < 1e-4,
                "bone {id:X}"
            );
            let dot: f32 = (0..4).map(|k| rotation[k] * local.rotation[k]).sum();
            assert!(dot.abs() > 0.9999, "bone {id:X}");
            agree += 1;
        }
        assert_eq!(agree, 43);
        // Every hit part hangs on a skeleton bone; the head bone 0x11 is
        // 1.33 m up, where the table's parent-relative entry alone says 0.12.
        let parts = parts(&stu).unwrap();
        assert_eq!(parts.len(), 7);
        assert!(parts.iter().all(|part| skeleton.bone(part.node).is_some()));
        let head = skeleton.model[skeleton.bone(0x11).unwrap()].translation;
        assert!((head[1] - 1.331).abs() < 1e-3, "{head:?}");
    }
}
