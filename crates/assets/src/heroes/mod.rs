//! `data/game_heroes_174.json`: each playable hero's GUID, name, class and
//! gameplay entity (the `003` body the game server creates for it), and the
//! slices built on it: the bodies' character movers ([`movers`]) and the Tab
//! board's stats ([`stats`]).
//!
//! The hero list, names and classes come from `data/extracted_heroes.json`
//! in the build folder; the body from each hero asset (`075`,
//! `STUHero.m_gameplayEntity`).

pub(crate) mod movers;
pub(crate) mod stats;

use std::{collections::BTreeMap, fs, path::Path};

use orpheus_stu::{Object, Types, Value};
use serde_json::{Map, json};

use crate::{Install, Output, Result, asset_error, guid_hex, guid_type};

const HERO: u16 = 0x075;
const ENTITY: u16 = 0x003;
/// The bits of a GUID DataTool names as its index (`%012X`).
const INDEX: u64 = 0xFFFF_FFFF_FFFF;

/// Each hero of `extracted_heroes.json` marked `IsHero` whose asset names a
/// gameplay entity, by hero GUID: `{"name", "class", "body"}`, sorted by
/// GUID. A hero without an asset or a gameplay entity is skipped. The class
/// is `""` when the list has none.
pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let heroes = read_json(out, "data/extracted_heroes.json")?;
    let heroes = heroes
        .as_object()
        .ok_or_else(|| asset_error("data/extracted_heroes.json is not an object"))?;
    let assets = Assets::of(install, HERO);
    let mut table = BTreeMap::new();
    for (key, hero) in heroes {
        if hero.get("IsHero").and_then(serde_json::Value::as_bool) != Some(true) {
            continue;
        }
        let index = key
            .split('.')
            .next()
            .and_then(|index| u64::from_str_radix(index, 16).ok())
            .ok_or_else(|| asset_error(format!("extracted_heroes.json: bad hero key {key}")))?;
        let Some(asset) = assets.read(install, index)? else {
            continue;
        };
        let Some(Value::Guid(body)) = asset.root().and_then(|hero| hero.get("m_gameplayEntity"))
        else {
            continue;
        };
        if *body == 0 || guid_type(*body) != ENTITY {
            continue;
        }
        table.insert(
            guid_hex(guid(index, HERO)),
            json!({
                "name": hero.get("Name").cloned().unwrap_or_default(),
                "class": hero.get("Class").cloned().unwrap_or_else(|| json!("")),
                "body": guid_hex(guid(body & INDEX, ENTITY)),
            }),
        );
    }
    let table: Map<String, serde_json::Value> = table.into_iter().collect();
    Ok(vec![Output::json(
        "data/game_heroes_174.json",
        &table.into(),
    )])
}

/// A GUID as the pack writes it, from DataTool's index and type: the type
/// sits bit-reversed at bits 48..59, nothing else is set.
pub(crate) fn guid(index: u64, kind: u16) -> u64 {
    let reversed = u64::from((kind - 1).reverse_bits() >> 4);
    reversed << 48 | index & INDEX
}

/// A GUID as the pack files write it (`0x` and hex digits).
pub(crate) fn parse_guid(text: &str) -> Result<u64> {
    text.strip_prefix("0x")
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
        .ok_or_else(|| asset_error(format!("bad GUID {text}")))
}

/// A pack file an earlier derivation wrote to the build folder.
pub(crate) fn read_json(out: &Path, path: &str) -> Result<serde_json::Value> {
    let bytes = fs::read(out.join(path)).map_err(|error| {
        asset_error(format!("{path} (an earlier derivation's output): {error}"))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| asset_error(format!("{path}: {error}")))
}

/// The hero bodies of `data/game_heroes_174.json`, then the training bots'
/// of `data/ai_spawners_174.json` when the build folder has it: each hero's
/// record (`{"name", "body", …}`).
pub(crate) fn bodies(out: &Path) -> Result<Vec<serde_json::Value>> {
    let heroes = read_json(out, "data/game_heroes_174.json")?;
    let mut bodies: Vec<serde_json::Value> = heroes
        .as_object()
        .ok_or_else(|| asset_error("data/game_heroes_174.json is not an object"))?
        .values()
        .cloned()
        .collect();
    if out.join("data/ai_spawners_174.json").is_file() {
        let spawners = read_json(out, "data/ai_spawners_174.json")?;
        let bots = spawners
            .get("heroes")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| asset_error("data/ai_spawners_174.json has no heroes"))?;
        bodies.extend(bots.values().cloned());
    }
    Ok(bodies)
}

/// The install's assets of one type by DataTool's index, the name its XML
/// export files carry.
pub(crate) struct Assets(BTreeMap<u64, u64>);

impl Assets {
    pub(crate) fn of(install: &Install, kind: u16) -> Self {
        Self(
            install
                .guids(kind)
                .into_iter()
                .map(|guid| (guid & INDEX, guid))
                .collect(),
        )
    }

    /// The asset with this index, decoded; `None` when the install has none.
    pub(crate) fn read(&self, install: &Install, index: u64) -> Result<Option<orpheus_stu::Asset>> {
        self.0
            .get(&(index & INDEX))
            .map(|guid| install.stu(*guid))
            .transpose()
    }
}

/// A field's value, or its type's default when the asset doesn't store it,
/// as DataTool's deserialized object holds it.
pub(crate) fn field<'a>(object: &'a Object, name: &str) -> Option<&'a Value> {
    object.get(name).or_else(|| {
        Types::get()
            .field_named(object.class, name)
            .map(|field| &field.default)
    })
}
