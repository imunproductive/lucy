//! `data/workshop_catalog_174.json`: the Workshop catalog the rule engine
//! (`orpheus-workshop`) reads, from the three "generic settings" assets the
//! client's editor, validator and normaliser load (`sub_7FF789C9F450`,
//! `sub_7FF7893589F0`; `D:/Work/Coding/refs/reports/Workshop.md` §1):
//!
//! - `054/0154` (`STU_8C73C07E`): `m_targets`, the 14 events in the order
//!   a rule's event type indexes them; `m_16EC4AA9`, the one condition
//!   (Comparison) and its graph.
//! - `054/0155` (`STU_B85A66BB`): `m_35CA5DCD`, the 222 actions in the
//!   order a logic action's `index` names them (`STU_106956CF`: graph,
//!   display name, parameters).
//! - `054/0162` (`STU_A21A7043`): `m_C87139B0` the enums (`STU_94CC7CA8`),
//!   `m_7169E470` the predefined strings of the `String` value, `m_values`
//!   the values (`STU_BFC1FB55`, keyed by their `01C` identifier).
//!
//! Each parameter keeps its class and the kind code the client gives it
//! (`sub_7FF789CA1D70`) and what the normaliser's defaults need: a value
//! slot's default value (`m_464FB148`), clamp and default number; an
//! enum's identifier and default; a variable's global flag. Identifiers are
//! `01C` indexes; graphs and other assets are GUIDs as the pack writes them.
//! The literal names the Workshop text uses that live elsewhere come along:
//! heroes (`data/game_heroes_174.json`), maps (the maps of
//! `data/game_maps_174.json` by their headers' Workshop names), game modes
//! (`0C5` display names). `loadouts` gives each loadout's logical button
//! (the Workshop's Event Ability).

use std::{collections::BTreeMap, path::Path};

use orpheus_stu::Value;
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{INDEX, Node, key, load, typed},
    heroes::{parse_guid, read_json},
};

pub(crate) const PATH: &str = "data/workshop_catalog_174.json";

const GENERIC_SETTINGS: u16 = 0x054;
const EVENTS: u64 = 0x154;
const ACTIONS: u64 = 0x155;
const VALUES: u64 = 0x162;
const GAME_MODE: u16 = 0x0C5;
const MAP_HEADER: u16 = 0x09F;
const LOADOUT: u16 = 0x09E;

const SOURCE: &str = "Build 1.74.0.0.104319: the Workshop catalog. 054/0154 m_targets (events, in \
event-type order) and m_16EC4AA9 (conditions); 054/0155 m_35CA5DCD (actions, in action-index \
order); 054/0162 m_C87139B0 (enums), m_7169E470 (predefined strings), m_values (values by 01C \
identifier). Parameter kinds are the client's (sub_7FF789CA1D70). Heroes from \
game_heroes_174.json, maps of game_maps_174.json by their 09F header's m_overrideName or else \
m_displayName (an enabled map first, then the first index of a name), game modes from 0C5 \
m_displayName, loadouts: each 09E \
STULoadout's m_logicalButton input action (0A9 STU_C5243F93) by its m_logicalButton number.";

/// The client's kind code of each parameter class (`sub_7FF789CA1D70`).
/// `STU_7BF2036D` (Wait's behaviour) has none; the event filters have none.
const KINDS: &[(&str, u8)] = &[
    ("STU_F5D532BC", 1),
    ("STU_38B39A55", 3),
    ("STU_1554B38A", 4),
    ("STU_4DD36FA9", 5),
    ("STU_8302E7AC", 6),
    ("STU_DE6A15D2", 8),
    ("STU_218BCF68", 11),
    ("STU_C5BE2B08", 12),
    ("STU_8E0051DD", 12),
    ("STU_3FA24DEA", 13),
    ("STU_F654E6FB", 14),
    ("STU_8504E8FE", 15),
    ("STU_CFF9EFAB", 16),
    ("STU_28E537BD", 17),
    ("STU_93382EAB", 18),
    ("STU_BE408E5C", 19),
    ("STU_EE41F5B8", 20),
    ("STU_59873384", 21),
    ("STU_7BF2036D", 0),
    ("STU_E08C5126", 0),
    ("STU_16886813", 0),
    ("STU_27CC2BBE", 0),
];

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let settings = |index: u64| install.stu(typed(index, GENERIC_SETTINGS));
    let events_asset = settings(EVENTS)?;
    let events_root = Node::root(&events_asset)?;
    let actions_asset = settings(ACTIONS)?;
    let actions_root = Node::root(&actions_asset)?;
    let values_asset = settings(VALUES)?;
    let values_root = Node::root(&values_asset)?;

    let text = |guid: u64| install.string(guid).map_or(Json::Null, Json::String);
    let entry = |node: Node| -> Result<Json> {
        Ok(json!({
            "name": text(node.guid("m_displayName")),
            "graph": graph(node.guid("m_graph")),
            "params": node.nodes("m_params").map(|param| parameter(install, param)).collect::<Result<Vec<_>>>()?,
        }))
    };
    let events = events_root
        .nodes("m_targets")
        .map(entry)
        .collect::<Result<Vec<_>>>()?;
    let conditions = events_root
        .nodes("m_16EC4AA9")
        .map(entry)
        .collect::<Result<Vec<_>>>()?;
    let actions = actions_root
        .nodes("m_35CA5DCD")
        .map(|node| {
            let mut action = entry(node)?;
            action["hidden"] = json!(node.int("m_89C93A57") != 0);
            Ok(action)
        })
        .collect::<Result<Vec<_>>>()?;
    let values = values_root
        .nodes("m_values")
        .map(|node| {
            Ok(json!({
                "id": identifier(node.guid("m_identifier")),
                "name": text(node.guid("m_displayName")),
                "returns": node.int("m_904BDD85"),
                "params": node.nodes("m_CF17DD30").map(|param| parameter(install, param)).collect::<Result<Vec<_>>>()?,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let enums: Vec<Json> = values_root
        .nodes("m_C87139B0")
        .map(|node| {
            json!({
                "id": identifier(node.guid("m_7533CD4C")),
                "default": optional_identifier(node.guid("m_0A0AA524")),
                "values": node.nodes("m_3FE1EA9E").map(|value| json!({
                    "id": identifier(value.guid("m_identifier")),
                    "name": text(value.guid("m_displayName")),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let strings: Vec<Json> = values_root
        .guids("m_7169E470")
        .map(|guid| Json::String(install.string(guid).unwrap_or_default()))
        .collect();

    let table = json!({
        "source": SOURCE,
        "events": events,
        "conditions": conditions,
        "actions": actions,
        "values": values,
        "enums": enums,
        "strings": strings,
        "heroes": heroes(out)?,
        "maps": maps(install, out)?,
        "game_modes": game_modes(install)?,
        "loadouts": loadouts(install)?,
    });
    Ok(vec![Output::json(PATH, &table)])
}

/// One parameter: its class and kind, and per class what the normaliser's
/// defaults and the text's names need.
fn parameter(install: &Install, param: Node) -> Result<Json> {
    let class = param
        .class_name()
        .ok_or_else(|| asset_error("a Workshop parameter of an unknown class"))?;
    let kind = KINDS
        .iter()
        .find(|(name, _)| *name == class)
        .map(|&(_, kind)| kind)
        .ok_or_else(|| asset_error(format!("a Workshop parameter of class {class}")))?;
    let mut fields = Map::new();
    fields.insert(
        "name".into(),
        install
            .string(param.guid("m_B9AD8659"))
            .map_or(Json::Null, Json::String),
    );
    fields.insert("class".into(), json!(class));
    fields.insert("kind".into(), json!(kind));
    // `m_89C93A57`: a hidden parameter; the text writer leaves it out
    // (`sub_7FF789CA5520` writes it to a scratch string).
    if param.int("m_89C93A57") != 0 {
        fields.insert("hidden".into(), json!(true));
    }
    match class {
        "STU_C5BE2B08" | "STU_8E0051DD" | "STU_CFF9EFAB" => {
            fields.insert("type".into(), json!(param.int("m_16CCEFC8")));
            fields.insert(
                "default_value".into(),
                optional_identifier(param.guid("m_464FB148")),
            );
            number_fields(param, &mut fields);
            if class == "STU_8E0051DD" {
                fields.insert("lambda".into(), json!(true));
            }
        }
        "STU_4DD36FA9" | "STU_8302E7AC" => number_fields(param, &mut fields),
        "STU_1554B38A" => {
            fields.insert("default".into(), json!(number(param, "m_D62358FA") != 0.0));
        }
        "STU_3FA24DEA" => {
            fields.insert("enum".into(), optional_identifier(param.guid("m_7533CD4C")));
            fields.insert(
                "enum_default".into(),
                optional_identifier(param.guid("m_1FCB161F")),
            );
        }
        "STU_DE6A15D2" => {
            fields.insert("global".into(), json!(param.int("m_2F489352") != 0));
        }
        _ => {}
    }
    Ok(Json::Object(fields))
}

fn number_fields(param: Node, fields: &mut Map<String, Json>) {
    for (field, name) in [
        ("m_min", "min"),
        ("m_max", "max"),
        ("m_D62358FA", "default"),
    ] {
        fields.insert(name.into(), float(number(param, field)));
    }
}

/// A number field, stored as an integer or a float.
fn number(node: Node, field: &str) -> f64 {
    match node.value(field) {
        Some(Value::Int(value)) => *value as f64,
        Some(Value::UInt(value)) => *value as f64,
        Some(Value::Float(value)) => *value,
        _ => 0.0,
    }
}

/// An `f32` as its shortest decimal, so the file reads like the data.
fn float(value: f64) -> Json {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}

fn identifier(guid: u64) -> u64 {
    guid & INDEX
}

fn optional_identifier(guid: u64) -> Json {
    if guid == 0 {
        Json::Null
    } else {
        json!(identifier(guid))
    }
}

fn graph(guid: u64) -> Json {
    if guid == 0 {
        Json::Null
    } else {
        json!(key(guid))
    }
}

/// Hero name -> hero GUID, from the hero table.
fn heroes(out: &Path) -> Result<Json> {
    let heroes = read_json(out, "data/game_heroes_174.json")?;
    let heroes = heroes
        .as_object()
        .ok_or_else(|| asset_error("data/game_heroes_174.json is not an object"))?;
    let mut names = Map::new();
    for (guid, hero) in heroes {
        if let Some(name) = hero.get("name").and_then(Json::as_str) {
            names.insert(name.into(), json!(guid));
        }
    }
    Ok(Json::Object(names))
}

/// Map name -> map header (`09F`) index for the maps of
/// `game_maps_174.json`, named as the Workshop names them: the header's
/// `m_overrideName`, else its `m_displayName` (`sub_7FF78934C490`, which
/// `sub_7FF789CA4B50` uses to write a Map constant: "Hanamura Winter",
/// not the pack's "Hanamura (Winter)"). Of the maps sharing a name, an
/// enabled one (the pack's `state`) wins over a disabled one, then the
/// lowest index: "Rialto" is the Escort map `871`, not the disabled
/// Rialto (Retribution) `79F`, whose header names it "Rialto" too.
fn maps(install: &Install, out: &Path) -> Result<Json> {
    let maps = read_json(out, "data/game_maps_174.json")?;
    let maps = maps
        .as_object()
        .ok_or_else(|| asset_error("data/game_maps_174.json is not an object"))?;
    let mut names: BTreeMap<String, (bool, u64)> = BTreeMap::new();
    for (key, map) in maps {
        // The pack keys maps by their `002` GUID; the header has the index.
        let index = parse_guid(key)? & INDEX;
        let header = install.stu(typed(index, MAP_HEADER))?;
        let header = Node::root(&header)?;
        let name = [header.guid("m_overrideName"), header.guid("m_displayName")]
            .into_iter()
            .find(|&text| text != 0)
            .and_then(|text| install.string(text));
        let Some(name) = name else { continue };
        let rank = (map["state"] != "Enabled", index);
        names
            .entry(name)
            .and_modify(|first| *first = (*first).min(rank))
            .or_insert(rank);
    }
    let names: BTreeMap<String, u64> = names
        .into_iter()
        .map(|(name, (_, index))| (name, index))
        .collect();
    Ok(json!(names))
}

/// Game mode name -> game mode GUID (`0C5` `m_displayName`).
fn game_modes(install: &Install) -> Result<Json> {
    let mut names = Map::new();
    for (&index, asset) in &load(install, GAME_MODE)? {
        let mode = Node::root(asset)?;
        if let Some(name) = install.string(mode.guid("m_displayName")) {
            names
                .entry(name)
                .or_insert_with(|| json!(key(typed(index, GAME_MODE))));
        }
    }
    Ok(Json::Object(names))
}

/// Loadout GUID -> the logical button number of its input action, for
/// the Workshop's Event Ability: each `09E` `STULoadout`'s
/// `m_logicalButton` names an input action (`STU_C5243F93`, the `0A9`
/// asset the settings' per-ability entries name too) whose own
/// `m_logicalButton` is the number the hero graphs' ability states and
/// the Workshop's button table use (Rocket Punch `0B9:107` -> `0A9:66` ->
/// 29 Secondary Fire; Quick Melee `0B9:2F5` -> `0A9:72` -> 31). A
/// loadout without an input action is left out.
fn loadouts(install: &Install) -> Result<Json> {
    let mut buttons = Map::new();
    for (&index, asset) in &load(install, LOADOUT)? {
        let action = Node::root(asset)?.guid("m_logicalButton");
        if action == 0 {
            continue;
        }
        let action = install.stu(action)?;
        let button = Node::root(&action)?.int("m_logicalButton");
        if button != 0 {
            buttons.insert(key(typed(index, LOADOUT)), json!(button));
        }
    }
    Ok(Json::Object(buttons))
}
