//! `data/ai_spawners_174.json`: every map's AI spawners (the Practice
//! Range's training bots) and hint points, and the bot heroes' bodies
//! (`tools/extract_ai_spawners.py`).
//!
//! Inputs: each map's ENTITY chunk (its GUID in the manifests; the Python
//! extractor took its storage keys from `data/collision_props_174.json`), the
//! bot heroes (`075`) and their bodies (`003`), and the English hero names of
//! `data/extracted_heroes.json` (read from the build folder).
//!
//! The ENTITY chunk is described in [`crate::map_chunk`]. Each component
//! instance is an STUv1 blob ("DUTS", u32 version, u32 offset of the
//! instance). An AI spawner is the instance STUECAISpawnerInstanceData
//! (0x99257985): +8 the hero (075; an asset ref, its GUID in the second half),
//! +24 an identifier (01C; 0 = in every mode), +40 the offset of `{u64 count,
//! u64 offset}` of its STUStatescriptGraphWithOverrides (40 bytes each: the
//! graph's GUID at +8, at +16 the offset of `{count, offset}` of the
//! overrides' offsets), +56 the team (TeamIndex: 0 blue, 1 red), +60 and +61
//! two flags (both 1 by default). An override (0x5866C45E) names a variable
//! (+16) and the offset of its value (+24): a list (0x06610EBC: `{count,
//! offset}` at +16), an identifier (0xFF80C481: GUID at +40) or an int
//! (0x12E434A1: at +16). An AI hint point is STUECHintPointInstanceData
//! (0xF199249B): its identifier at +16, its place the placeable's
//! translation.
//!
//! Per bot hero: the body (m_gameplayEntity), its health, armour and shields
//! (the body's override of the common body graph 0033's variables 199, 221
//! and 1736) and its run speed (the mover's run_forward, m_CFD3E635).

use std::{collections::BTreeSet, fs, path::Path};

use orpheus_stu::{Asset, Object, Value as Stu};
use serde_json::{Map, Value, json};

use crate::{
    Install, Output, Result, asset_error, guid_hex, guid_name, guid_type,
    map_chunk::{ENTITY, MAP_CHUNK, StuV1, chunk_kind, entities, guid_of, round, short},
};

pub(crate) const PATH: &str = "data/ai_spawners_174.json";
/// The English hero names, another derivation's output.
const NAMES: &str = "data/extracted_heroes.json";

const SPAWNER: u32 = 0x9925_7985;
const HINT: u32 = 0xF199_249B;
const OVERRIDE: u32 = 0x5866_C45E;
const LIST: u32 = 0x0661_0EBC;
const IDENTIFIER: u32 = 0xFF80_C481;
const INT: u32 = 0x12E4_34A1;
const GRAPH_STRIDE: u64 = 40;
const HERO_TYPE: u64 = 0x02E0;
const ENTITY_TYPE: u64 = 0x0400;
const MAP: u16 = 0x002;
/// The common body graph 0033's health, armour and shields variables (199,
/// 221, 1736).
const HEALTH_VARS: [(&str, u64); 3] = [("health", 0xC7), ("armor", 0xDD), ("shields", 0x6C8)];

const SOURCE: &str = "Build 1.74.0.0.104319: each map's ENTITY chunk (0x0DD0000B << 32 | map index), its \
STUECAISpawnerInstanceData and STUECHintPointInstanceData instances (tools/extract_ai_spawners.py); hero bodies \
from the heroes' m_gameplayEntity, their health, armor and shields from the body's overrides of 0033's vars 199, \
221, 1736, speed = the mover's run_forward. team: TeamIndex (0 blue, 1 red). identifier: the mode identifier the \
spawner needs (null: every mode). vars: the graphs' variable overrides.";

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let names: Map<String, Value> = serde_json::from_slice(&fs::read(out.join(NAMES))?)
        .map_err(|error| asset_error(format!("{NAMES}: {error}")))?;
    let mut maps = Map::new();
    let mut heroes = BTreeSet::new();
    for guid in install.guids(MAP_CHUNK) {
        if chunk_kind(guid) != ENTITY {
            continue;
        }
        // A chunk the install can't read has no storage keys the extractor
        // knew either.
        let Ok(data) = install.raw(guid) else {
            continue;
        };
        let (found, hints) = spawners(&data)?;
        if found.is_empty() {
            continue;
        }
        for spawner in &found {
            let hero = spawner["hero"].as_str().expect("a GUID");
            heroes.insert(u64::from_str_radix(&hero[2..], 16).expect("hex"));
        }
        let mut entry = Map::new();
        entry.insert("hints".into(), Value::Object(hints));
        entry.insert("spawners".into(), json!(found));
        maps.insert(
            guid_hex(guid_of(guid & 0xFFFF_FFFF, MAP)),
            Value::Object(entry),
        );
    }
    let bad: Vec<String> = heroes
        .iter()
        .filter(|hero| *hero >> 48 != HERO_TYPE)
        .map(|hero| guid_hex(*hero))
        .collect();
    if !bad.is_empty() {
        return Err(asset_error(format!("not heroes: {bad:?}")));
    }
    let mut table = Map::new();
    table.insert("_source".into(), json!(SOURCE));
    table.insert(
        "heroes".into(),
        Value::Object(hero_bodies(install, &heroes, &names)?),
    );
    table.insert("maps".into(), Value::Object(maps));
    Ok(vec![Output::json(PATH, &Value::Object(table))])
}

/// A chunk's AI spawners and hint points.
type Spawners = (Vec<Map<String, Value>>, Map<String, Value>);

/// The AI spawners of one ENTITY chunk, in placeable order, and its hint
/// points {identifier: place}.
fn spawners(data: &[u8]) -> Result<Spawners> {
    let mut out = Vec::new();
    let mut hints = Map::new();
    let missing = || asset_error("AI spawner runs past its chunk");
    for entity in entities(data)? {
        let position: Vec<f64> = entity
            .translation
            .iter()
            .map(|v| round(f64::from(*v), 4))
            .collect();
        for stu in entity.instances() {
            let Some(inst) = stu.root() else { continue };
            let kind = stu.u32(inst).ok_or_else(missing)?;
            if kind == HINT {
                hints.insert(
                    guid_hex(stu.u64(inst + 16).ok_or_else(missing)?),
                    json!(position),
                );
            }
            if kind != SPAWNER {
                continue;
            }
            let mut graphs = Vec::new();
            let mut variables = Map::new();
            let array = stu.u64(inst + 40).ok_or_else(missing)?;
            if array != 0 {
                let total = stu.u64(array as usize).ok_or_else(missing)?;
                let first = stu.u64(array as usize + 8).ok_or_else(missing)?;
                // The graphs sit in the array itself.
                for n in 0..total {
                    let at = (first + GRAPH_STRIDE * n) as usize;
                    graphs.push(guid_hex(stu.u64(at + 8).ok_or_else(missing)?));
                    let overrides = stu.u64(at + 16).ok_or_else(missing)?;
                    if overrides == 0 {
                        continue;
                    }
                    for item in offsets(&stu, overrides).ok_or_else(missing)? {
                        if stu.u32(item) == Some(OVERRIDE) {
                            let name = guid_hex(stu.u64(item + 16).ok_or_else(missing)?);
                            let place = stu.u64(item + 24).ok_or_else(missing)?;
                            variables.insert(
                                name,
                                override_value(&stu, place as usize).ok_or_else(missing)?,
                            );
                        }
                    }
                }
            }
            let identifier = stu.u64(inst + 32).ok_or_else(missing)?;
            let rotation: Vec<f64> = entity
                .rotation
                .iter()
                .map(|v| round(f64::from(*v), 6))
                .collect();
            let mut spawner = Map::new();
            spawner.insert("placeable".into(), json!(entity.index));
            spawner.insert(
                "hero".into(),
                json!(guid_hex(stu.u64(inst + 16).ok_or_else(missing)?)),
            );
            spawner.insert(
                "team".into(),
                json!(stu.i32(inst + 56).ok_or_else(missing)?),
            );
            spawner.insert(
                "identifier".into(),
                json!((identifier != 0).then(|| guid_hex(identifier))),
            );
            spawner.insert("position".into(), json!(position));
            spawner.insert("rotation".into(), json!(rotation));
            spawner.insert("graphs".into(), json!(graphs));
            spawner.insert("vars".into(), Value::Object(variables));
            let flags = [
                stu.u8(inst + 60).ok_or_else(missing)?,
                stu.u8(inst + 61).ok_or_else(missing)?,
            ];
            spawner.insert("flags".into(), json!(flags));
            out.push(spawner);
        }
    }
    Ok((out, hints))
}

/// The blob offsets an inline array's `{u64 count, u64 offset}` at `at`
/// lists (u64 each).
fn offsets(stu: &StuV1<'_>, at: u64) -> Option<Vec<usize>> {
    let at = at as usize;
    let total = stu.u64(at)?;
    let first = stu.u64(at + 8)? as usize;
    (0..total as usize)
        .map(|n| stu.u64(first + 8 * n).map(|offset| offset as usize))
        .collect()
}

/// An override's value: a list, an identifier GUID (hex), an int, or the
/// type's hex when unknown.
fn override_value(stu: &StuV1<'_>, at: usize) -> Option<Value> {
    let kind = stu.u32(at)?;
    Some(match kind {
        LIST => {
            let items = offsets(stu, stu.u64(at + 16)?)?;
            Value::Array(
                items
                    .into_iter()
                    .map(|item| override_value(stu, item))
                    .collect::<Option<_>>()?,
            )
        }
        IDENTIFIER => json!(guid_hex(stu.u64(at + 40)?)),
        INT => json!(stu.i32(at + 16)?),
        other => json!(format!("type 0x{other:08X}")),
    })
}

/// Hero GUID -> {name, body, health, armor, shields, speed}.
fn hero_bodies(
    install: &Install,
    heroes: &BTreeSet<u64>,
    names: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for hero in heroes {
        let key = format!("{:012X}.075", hero & 0xFFFF_FFFF_FFFF);
        let asset = install.stu(*hero)?;
        let body = match asset.root().and_then(|root| root.get("m_gameplayEntity")) {
            Some(Stu::Guid(body)) if guid_type(*body) == 0x003 => *body,
            _ => return Err(asset_error(format!("no m_gameplayEntity in {key}"))),
        };
        let body_asset = install.stu(body)?;
        let name = names
            .get(&key)
            .and_then(|entry| entry.get("Name"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut entry = Map::new();
        entry.insert("name".into(), json!(name));
        entry.insert(
            "body".into(),
            json!(guid_hex(ENTITY_TYPE << 48 | (body & 0xFFFF_FFFF_FFFF))),
        );
        body_values(&body_asset, &mut entry)
            .map_err(|reason| asset_error(format!("{}: {reason}", guid_name(body))))?;
        out.insert(guid_hex(*hero), Value::Object(entry));
    }
    Ok(out)
}

/// A body's health, armour and shields (its statescript component's
/// overrides of 0033's 199, 221, 1736; 0 when it has none) and its run speed
/// (its mover component's m_CFD3E635; 0 without a mover).
fn body_values(asset: &Asset, entry: &mut Map<String, Value>) -> Result<(), String> {
    let statescript = component(asset, "STUStatescriptComponent");
    for (name, identifier) in HEALTH_VARS {
        let value = match statescript {
            Some(component) => override_of(asset, component, identifier)?,
            None => 0.0,
        };
        entry.insert(name.into(), json!(value));
    }
    let speed = match component(asset, "STUCharacterMoverComponent") {
        Some(mover) => match mover.get("m_CFD3E635") {
            Some(Stu::Float(speed)) => short(*speed as f32),
            _ => 0.0,
        },
        None => 0.0,
    };
    entry.insert("speed".into(), json!(speed));
    Ok(())
}

/// The first component of a class in the entity's component map.
fn component<'a>(asset: &'a Asset, class: &str) -> Option<&'a Object> {
    let Some(Stu::Map(entries)) = asset.root()?.get("m_componentMap") else {
        return None;
    };
    entries
        .iter()
        .filter_map(|(_, index)| asset.instances.get((*index)?)?.as_ref())
        .find(|object| object.class_name() == Some(class))
}

/// The value of the first override of an identifier (`01C` index) in the
/// component, in the order DataTool writes it; 0 when there is none.
fn override_of(asset: &Asset, component: &Object, identifier: u64) -> Result<f64, String> {
    let Some(entry) = find_entry(asset, component, identifier, 0) else {
        return Ok(0.0);
    };
    let value = entry.get("m_value").and_then(|value| asset.resolve(value));
    match value.and_then(|value| value.get("m_value")) {
        Some(Stu::Float(value)) => Ok(short(*value as f32)),
        Some(Stu::Int(value)) => Ok(*value as f64),
        Some(Stu::UInt(value)) => Ok(*value as f64),
        _ => Err(format!("override of {identifier:X} has no number")),
    }
}

/// The first STUStatescriptSchemaEntry for the identifier under `object`,
/// depth first in field order.
fn find_entry<'a>(
    asset: &'a Asset,
    object: &'a Object,
    identifier: u64,
    depth: usize,
) -> Option<&'a Object> {
    if depth > 32 {
        return None;
    }
    if object.class == OVERRIDE
        && matches!(object.get("m_0D09D2D9"), Some(Stu::Guid(id)) if guid_type(*id) == 0x01C && id & 0xFFFF_FFFF_FFFF == identifier)
    {
        return Some(object);
    }
    object
        .fields
        .iter()
        .find_map(|(_, value)| find_in_value(asset, value, identifier, depth + 1))
}

fn find_in_value<'a>(
    asset: &'a Asset,
    value: &'a Stu,
    identifier: u64,
    depth: usize,
) -> Option<&'a Object> {
    match value {
        Stu::Instance(_) | Stu::Object(_) => {
            find_entry(asset, asset.resolve(value)?, identifier, depth)
        }
        Stu::Array(items) => items
            .iter()
            .find_map(|item| find_in_value(asset, item, identifier, depth)),
        _ => None,
    }
}
