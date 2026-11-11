//! `data/game_bot_animation_174.json`: what the server needs to pose a bot
//! body's hit shapes at run time, derived with the hit shapes
//! ([`crate::hitboxes`]): each bot body's shapes in the frame of their
//! bone, and its rig: the skeleton, the clips its blend trees play in each
//! branch the server chooses from, and those clips' keys.
//!
//! **Branches.** The rig's set ([`crate::blend_trees::TreeSet`]) is walked
//! from its root alias four times: with no category set (`standing`), with
//! the locomotion tree's moving category `0005` (`moving`), with the
//! arms-forward category `007F` the Training Bot's graph `0BA8` pushes
//! (`arms_forward_standing`), and with both (`arms_forward_moving`). Every
//! alias of the set is also walked on its own, without and with `007F`
//! (`anims`): what a statescript `Anim` state that names the alias plays.
//! A branch lists its base clips (full poses, in the order the walk meets
//! them; `once` for a sequence's leading clips) and its layers (clips of
//! deltas, [`crate::animation`]'s additive bit), each once.
//!
//! On the Training Bot's set (`5A8.021`): `standing` plays `3920` (the idle
//! pose) with the layers `27AD` (a 2-frame blink, under 4 mm) and `391F`
//! (the 4 s hover bob); `moving` plays `3990` once (1.33 s, setting off)
//! and then `3952` (1.17 s; the four-way directional blend binds it in all
//! four directions, so the walk's direction does not change the pose) with
//! `27AD`; `arms_forward_standing` plays `394F` with `3950`, `39FA`, `39FB`
//! and `27AD`; `arms_forward_moving` plays `3A08` once, then `3A07`, with
//! `39FA`, `39FB` and `27AD`. Alias `0451` (graph `0BA8`'s 2.5 s state on a
//! reset) plays `39D7` (`39D8` with `007F`), the bot flying together from
//! its parts.
//!
//! The file: `{"bodies": {body: {"name", "rig", "shapes": [{"bone" (index
//! into the rig's bones), "kind": "sphere", "centre", "radius", "critical"}
//! | {"bone", "kind": "capsule", "a", "b", "radius", "critical"}]}},
//! "rigs": {"<model>/<set>": {"bones": [{"id", "parent" (index, null for
//! the root), "rotation", "scale", "translation"}] (relative to the parent,
//! parents first), "branches": {name: branch}, "anims": {alias: {"plain":
//! branch, "arms_forward": branch}}, "clips": {clip: {"additive",
//! "duration", "fps", "frames", "tracks": [{"bone", "positions": [[frame,
//! x, y, z]], "rotations": [[frame, x, y, z, w]], "scales": [[frame, x, y,
//! z]]}]}}}}}`, a branch `{"base": [{"clip", "once"}], "layers": [clip]}`.
//! Only bones of the skeleton are kept; empty key lists are left out. The
//! file is written without indentation (its clips hold many keys).

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value, json};

use crate::{
    Install,
    animation::{self, Animation},
    blend_trees::{Played, TreeSet},
    guid_hex,
    heroes::movers::float,
    hitboxes::{Geometry, Part, Skeleton},
};

/// The locomotion tree's moving category (`072.020`'s switch on data flow
/// `048:0018`, branch `0005`).
pub(crate) const MOVING: u64 = 0x0280_0000_0000_0005;
/// The arms-forward category of the Training Bot's trees (`06E.020`'s
/// switch on data flow `048:00E4`), which graph `0BA8` pushes into its
/// variable 1226.
pub(crate) const ARMS_FORWARD: u64 = 0x0280_0000_0000_007F;

/// The root alias's branches the server chooses from, by name.
const BRANCHES: [(&str, &[u64]); 4] = [
    ("standing", &[]),
    ("moving", &[MOVING]),
    ("arms_forward_standing", &[ARMS_FORWARD]),
    ("arms_forward_moving", &[ARMS_FORWARD, MOVING]),
];

fn floats(values: &[f32]) -> Value {
    Value::Array(values.iter().map(|v| float(f64::from(*v))).collect())
}

/// A body's shapes in the frame of their bone (see the module notes).
pub(crate) fn shapes(
    parts: &[Part],
    skeleton: &Skeleton,
    zones: &HashMap<u64, f64>,
) -> Result<Vec<Value>, String> {
    parts
        .iter()
        .map(|part| {
            let bone = skeleton
                .bone(part.node)
                .ok_or_else(|| format!("bone {:X} is not in the skeleton", part.node))?;
            let critical = part.critical(zones);
            Ok(match part.geometry {
                Geometry::Sphere(centre, radius) => json!({
                    "bone": bone,
                    "kind": "sphere",
                    "centre": floats(&centre),
                    "radius": float(f64::from(radius)),
                    "critical": critical,
                }),
                Geometry::Capsule(a, b, radius) => json!({
                    "bone": bone,
                    "kind": "capsule",
                    "a": floats(&a),
                    "b": floats(&b),
                    "radius": float(f64::from(radius)),
                    "critical": critical,
                }),
            })
        })
        .collect()
}

/// The clips read so far, by GUID.
struct Clips<'a> {
    install: &'a Install,
    read: BTreeMap<u64, Animation>,
}

impl Clips<'_> {
    fn get(&mut self, clip: u64) -> Result<&Animation, String> {
        if !self.read.contains_key(&clip) {
            let data = self
                .install
                .raw(clip)
                .map_err(|error| format!("clip {}: {error}", guid_hex(clip)))?;
            let animation = animation::read(&data)?;
            self.read.insert(clip, animation);
        }
        Ok(&self.read[&clip])
    }

    /// A walk's clips as a branch: the full poses in order, the layers.
    fn branch(&mut self, played: &[Played]) -> Result<Value, String> {
        let mut base: Vec<Played> = Vec::new();
        let mut layers: Vec<u64> = Vec::new();
        for played in played {
            if self.get(played.clip)?.additive {
                if !layers.contains(&played.clip) {
                    layers.push(played.clip);
                }
            } else if base.iter().all(|seen| seen.clip != played.clip) {
                base.push(*played);
            }
        }
        Ok(json!({
            "base": base
                .iter()
                .map(|played| json!({"clip": guid_hex(played.clip), "once": played.once}))
                .collect::<Vec<_>>(),
            "layers": layers.iter().map(|clip| guid_hex(*clip)).collect::<Vec<_>>(),
        }))
    }

    /// A clip's keys on the skeleton's bones.
    fn keys(animation: &Animation, skeleton: &Skeleton) -> Value {
        let tracks: Vec<Value> = animation
            .tracks
            .iter()
            .filter_map(|track| {
                let bone = skeleton.bone(track.bone)?;
                let mut entry = Map::new();
                entry.insert("bone".into(), json!(bone));
                let keyed = |keys: Vec<Value>| (!keys.is_empty()).then_some(Value::Array(keys));
                let keys = |frame: usize, values: &[f32]| {
                    let mut key = vec![json!(frame)];
                    key.extend(values.iter().map(|v| float(f64::from(*v))));
                    Value::Array(key)
                };
                for (name, list) in [
                    (
                        "positions",
                        keyed(track.positions.iter().map(|(f, v)| keys(*f, v)).collect()),
                    ),
                    (
                        "rotations",
                        keyed(track.rotations.iter().map(|(f, v)| keys(*f, v)).collect()),
                    ),
                    (
                        "scales",
                        keyed(track.scales.iter().map(|(f, v)| keys(*f, v)).collect()),
                    ),
                ] {
                    if let Some(list) = list {
                        entry.insert(name.into(), list);
                    }
                }
                Some(Value::Object(entry))
            })
            .collect();
        json!({
            "additive": animation.additive,
            "duration": float(f64::from(animation.duration)),
            "fps": float(f64::from(animation.fps)),
            "frames": animation.frames,
            "tracks": tracks,
        })
    }
}

/// The rig of a skeleton animated by the blend tree set `set` (see the
/// module notes).
pub(crate) fn rig(install: &Install, set: u64, skeleton: &Skeleton) -> Result<Value, String> {
    let trees = TreeSet::read(install, set)?;
    let mut clips = Clips {
        install,
        read: BTreeMap::new(),
    };
    let mut branches = Map::new();
    for (name, categories) in BRANCHES {
        let played = trees.branch(trees.root(), categories);
        branches.insert(name.into(), clips.branch(&played)?);
    }
    let mut anims = Map::new();
    for &alias in trees.aliases() {
        let plain = trees.branch(alias, &[]);
        let forward = trees.branch(alias, &[ARMS_FORWARD]);
        if plain.is_empty() && forward.is_empty() {
            continue;
        }
        anims.insert(
            guid_hex(alias),
            json!({"plain": clips.branch(&plain)?, "arms_forward": clips.branch(&forward)?}),
        );
    }
    let bones: Vec<Value> = skeleton
        .local
        .iter()
        .enumerate()
        .map(|(index, bone)| {
            json!({
                "id": skeleton.ids[index],
                "parent": skeleton.parents[index],
                "rotation": floats(&bone.rotation),
                "scale": floats(&bone.scale),
                "translation": floats(&bone.translation),
            })
        })
        .collect();
    let keys: Map<String, Value> = clips
        .read
        .iter()
        .map(|(clip, animation)| (guid_hex(*clip), Clips::keys(animation, skeleton)))
        .collect();
    Ok(json!({
        "bones": bones,
        "branches": branches,
        "anims": anims,
        "clips": keys,
    }))
}
