//! The match's body and mode values that no pack file of the Python
//! reference held, read from the install:
//!
//! - `data/game_eyes_174.json`: each body's eye heights over its feet,
//!   standing and crouched (`STUFirstPersonComponent`, class `0x9D28963F`,
//!   `m_E29993A5` and `m_816B830F`), for the bodies of
//!   `data/game_heroes_174.json` and the training bots' of
//!   `data/ai_spawners_174.json`. A body without the component takes the
//!   component's type defaults (1.55 and 1.15), which is what
//!   `ow174/game/combat.py` gave every body but Soldier: 76 (`EYE`,
//!   `EYE_CROUCHED_DEFAULT`).
//! - `data/game_respawns_174.json`: each game mode's respawn time in seconds,
//!   the value its mode graph (`STUGameMode.m_CF63B633`) sends the player in
//!   `06FD.025`'s parameter `00B7` (combat.py `RESPAWN`: `0F4A` and `0EC2`
//!   send 10; `PRACTICE_RESPAWN`: `13C0` sends 3). The value is a literal or
//!   a graph variable every `SetVar` of the graph sets to the same literal
//!   (`0F4A` sends its `#20680`, set to 10). A mode whose graph sends no such
//!   message, or several values, is left out.
//! - `data/game_spawns_174.json`: each entity a hero weapon volley's
//!   projectile is (`m_projectileEntity`) or leaves where it lands
//!   (`m_49CBA3BC`, as `ow174/game/projectiles.py` `_defs` reads them):
//!   the `STU_8556841E` definitions in either field of a
//!   `STUStatescriptStateWeaponVolley`, with the initial graphs of the
//!   entity's statescript component (`m_B634821A[].m_graph`). `03DC.003`,
//!   the Biotic Field, starts `0258`; Moira's Biotic Orbs `1E60.003` and
//!   `1E61.003` start `10FF` and `1100`.
//!
//! The hero bundle (`data/game_graphs_174.json.gz`) is read from the build
//! folder.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

use orpheus_stu::{Asset, Object, Types, Value};
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error, guid_hex, guid_type,
    heroes::{Assets, field, guid, movers::float, parse_guid},
};

const ENTITY: u16 = 0x003;
const GRAPH: u16 = 0x01B;
const MODE: u16 = 0x0C5;
const MESSAGE: u16 = 0x025;
const PARAMETER: u16 = 0x026;
const INDEX: u64 = 0xFFFF_FFFF_FFFF;
/// `STUFirstPersonComponent`.
const FIRST_PERSON: u32 = 0x9D28963F;
const EYE: &str = "m_E29993A5";
const EYE_CROUCHED: &str = "m_816B830F";
/// The game message that tells the player his respawn time, and its
/// seconds parameter.
const RESPAWN_MESSAGE: u64 = 0x06FD;
const RESPAWN_SECONDS: u64 = 0x00B7;

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    Ok(vec![
        Output::json("data/game_eyes_174.json", &eyes(install, out)?),
        Output::json("data/game_respawns_174.json", &respawns(install)?),
        Output::json("data/game_spawns_174.json", &spawns(install, out)?),
    ])
}

/// `{body: {"name", "eye", "eye_crouched"}}`, sorted by body GUID. A body
/// listed twice keeps the last list's name; a body without an asset is
/// skipped.
fn eyes(install: &Install, out: &Path) -> Result<Json> {
    let assets = Assets::of(install, ENTITY);
    let defaults = |name| {
        Types::get()
            .field_named(FIRST_PERSON, name)
            .map(|field| field.default.clone())
            .ok_or_else(|| asset_error(format!("the type table has no {name}")))
    };
    let (eye_default, crouched_default) = (defaults(EYE)?, defaults(EYE_CROUCHED)?);
    let mut table = BTreeMap::new();
    for hero in crate::heroes::bodies(out)? {
        let key = hero
            .get("body")
            .and_then(Json::as_str)
            .ok_or_else(|| asset_error(format!("a hero without a body: {hero}")))?;
        let Some(asset) = assets.read(install, parse_guid(key)?)? else {
            continue;
        };
        let component = asset
            .instances
            .iter()
            .flatten()
            .find(|object| object.class == FIRST_PERSON);
        let height = |name, default: &Value| match component
            .map_or(default, |object| field(object, name).unwrap_or(default))
        {
            Value::Float(value) => Ok(float(*value)),
            other => Err(asset_error(format!("{key}: {name} is {other:?}"))),
        };
        table.insert(
            key.to_owned(),
            json!({
                "name": hero.get("name").cloned().unwrap_or_default(),
                "eye": height(EYE, &eye_default)?,
                "eye_crouched": height(EYE_CROUCHED, &crouched_default)?,
            }),
        );
    }
    Ok(Json::Object(table.into_iter().collect()))
}

/// `{mode: seconds}`, sorted by mode GUID.
fn respawns(install: &Install) -> Result<Json> {
    let graphs = Assets::of(install, GRAPH);
    let mut table = BTreeMap::new();
    for (mode, asset) in install.stu_all(&install.guids(MODE)) {
        let asset = asset?;
        let Some(Value::Guid(graph)) = asset.root().and_then(|root| field(root, "m_CF63B633"))
        else {
            continue;
        };
        if *graph == 0 {
            continue;
        }
        let Some(graph) = graphs.read(install, *graph)? else {
            continue;
        };
        if let [Some(seconds)] = respawn_seconds(&graph).as_slice() {
            table.insert(guid_hex(guid(mode & INDEX, MODE)), json!(seconds));
        }
    }
    Ok(Json::Object(table.into_iter().collect::<Map<_, _>>()))
}

/// The distinct seconds a graph's `SendGameMessage` states (`STU_38EE1100`)
/// send in `06FD.025`, or `None` for a value that is no literal.
fn respawn_seconds(graph: &Asset) -> Vec<Option<i64>> {
    let mut sent = BTreeSet::new();
    for node in objects(graph, "STU_38EE1100") {
        let message = field(node, "m_gameMessage")
            .and_then(|config| graph.resolve(config))
            .and_then(|config| field(config, "m_gameMessage"));
        if !matches!(message, Some(Value::Guid(guid)) if is(*guid, MESSAGE, RESPAWN_MESSAGE)) {
            continue;
        }
        let Some(Value::Array(parameters)) = field(node, "m_params") else {
            continue;
        };
        for parameter in parameters.iter().filter_map(|value| graph.resolve(value)) {
            let Some(Value::Guid(id)) = field(parameter, "m_B5051BCE") else {
                continue;
            };
            if is(*id, PARAMETER, RESPAWN_SECONDS) {
                let value = field(parameter, "m_value").and_then(|value| graph.resolve(value));
                sent.insert(value.and_then(|value| literal(graph, value)));
            }
        }
    }
    sent.into_iter().collect()
}

/// An integer config value: a literal, or a graph variable every `SetVar`
/// of the graph sets to one literal.
fn literal(graph: &Asset, config: &Object) -> Option<i64> {
    match config.class_name()? {
        "STUConfigVarInt" => match field(config, "m_value")? {
            Value::Int(value) => Some(*value),
            _ => None,
        },
        "STUConfigVarDynamic" => {
            let variable = field(config, "m_identifier")?;
            let mut set = BTreeSet::new();
            for action in objects(graph, "STUStatescriptActionSetVar") {
                let target = field(action, "m_out_Var").and_then(|value| graph.resolve(value));
                if target.and_then(|target| field(target, "m_identifier")) != Some(variable) {
                    continue;
                }
                let value = field(action, "m_value").and_then(|value| graph.resolve(value))?;
                if value.class_name() != Some("STUConfigVarInt") {
                    return None;
                }
                set.insert(literal(graph, value)?);
            }
            let mut values = set.into_iter();
            values.next().filter(|_| values.next().is_none())
        }
        _ => None,
    }
}

/// `{entity: {"graphs": [graph index]}}`, sorted by entity GUID.
fn spawns(install: &Install, out: &Path) -> Result<Json> {
    let path = "data/game_graphs_174.json.gz";
    let mut text = Vec::new();
    let bytes = fs::read(out.join(path)).map_err(|error| {
        asset_error(format!("{path} (an earlier derivation's output): {error}"))
    })?;
    flate2::read::GzDecoder::new(bytes.as_slice()).read_to_end(&mut text)?;
    let bundle: Json =
        serde_json::from_slice(&text).map_err(|error| asset_error(format!("{path}: {error}")))?;
    let mut spawned = BTreeSet::new();
    let graphs = bundle
        .get("graphs")
        .and_then(Json::as_array)
        .ok_or_else(|| asset_error(format!("{path} has no graph list")))?;
    for node in graphs
        .iter()
        .filter_map(|graph| graph.get("nodes").and_then(Json::as_array))
        .flatten()
        .filter(|node| {
            node.get("class").and_then(Json::as_str) == Some("STUStatescriptStateWeaponVolley")
        })
    {
        for name in ["m_49CBA3BC", "m_projectileEntity"] {
            if let Some(spawn) = node.get("fields").and_then(|fields| fields.get(name)) {
                definitions(spawn, &mut spawned)?;
            }
        }
    }
    let entities = Assets::of(install, ENTITY);
    let mut table = Map::new();
    for entity in spawned {
        let Some(asset) = entities.read(install, entity)? else {
            continue;
        };
        let Some(component) = asset
            .instances
            .iter()
            .flatten()
            .find(|object| object.class_name() == Some("STUStatescriptComponent"))
        else {
            continue;
        };
        let mut initial = Vec::new();
        if let Some(Value::Array(entries)) = field(component, "m_B634821A") {
            for entry in entries.iter().filter_map(|entry| asset.resolve(entry)) {
                if let Some(Value::Guid(graph)) = field(entry, "m_graph") {
                    if *graph != 0 && guid_type(*graph) == GRAPH {
                        initial.push(graph & INDEX);
                    }
                }
            }
        }
        table.insert(guid_hex(guid(entity, ENTITY)), json!({"graphs": initial}));
    }
    Ok(Json::Object(table))
}

/// Every `003` GUID a projectile entity field names (both sides of a
/// condition).
fn definitions(config: &Json, found: &mut BTreeSet<u64>) -> Result<()> {
    match config {
        Json::Object(fields) => {
            if fields.get("$").and_then(Json::as_str) == Some("STU_8556841E") {
                if let Some(text) = fields.get("m_entityDef").and_then(Json::as_str) {
                    let entity = parse_guid(text)?;
                    if guid_type(entity) == ENTITY {
                        found.insert(entity & INDEX);
                    }
                }
            }
            for value in fields.values() {
                definitions(value, found)?;
            }
        }
        Json::Array(values) => {
            for value in values {
                definitions(value, found)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The asset's objects of one class.
fn objects<'a>(asset: &'a Asset, class: &'a str) -> impl Iterator<Item = &'a Object> {
    asset
        .instances
        .iter()
        .flatten()
        .filter(move |object| object.class_name() == Some(class))
}

/// Whether a GUID is this index of this type.
fn is(guid: u64, kind: u16, index: u64) -> bool {
    guid_type(guid) == kind && guid & INDEX == index
}
