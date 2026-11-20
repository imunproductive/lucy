//! `data/game_rulesets_174.json` and `data/game_modes_174.json`: what the
//! game modes (`0C5`) and heroes (`075`) set up for a match.
//!
//! The rulesets file holds the defaults `GetGameRulesetValue` answers
//! (`ow174/game/script/rulesets.py`, which froze them from a DataTool dump):
//! every game ruleset schema (`0C6` `STUGameRulesetSchema`) the heroes and
//! modes list, each entry's identifier (`m_3E783677`, an `01C`) and default
//! (`m_value`: a bool, an int, a float or an enum choice, the identifier of
//! `m_default`), and each hero's and mode's `m_gameRulesetSchemas` in order.
//! Its `settings` are the same schemas as the Workshop settings text names
//! them: each schema's `m_displayText` and, per entry, the identifier, the
//! entry's `m_displayText` (the name a settings line starts with; the
//! client's importer matches it, `sub_7FF789A8A0C0`), the type, a bool's
//! two words, a number's range and an enum's choices with their names; an
//! ability's entry also its logical button and name format. Its
//! `abilities` are each hero's abilities (`m_heroLoadout`) by logical
//! button, the names a hero's block writes those entries with.
//!
//! The modes file holds, per mode, its teams (`m_teams`: the team, its
//! controller graph `m_controllerScript`, the graph every body of the team
//! runs `m_bodyScript`, and its players `m_170AA4B8`) and whether its
//! loading screen waits for the mode's own script (`m_0FC17230`;
//! `SCRIPTED_LOADING_MODES` in `ow174/game/content.py`).

use std::path::Path;

use orpheus_stu::Value;
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{INDEX, Node, key, load, typed},
};

pub(crate) const RULESETS: &str = "data/game_rulesets_174.json";
pub(crate) const MODES: &str = "data/game_modes_174.json";

const HERO: u16 = 0x075;
const MODE: u16 = 0x0C5;
const SCHEMA: u16 = 0x0C6;

const RULESETS_SOURCE: &str = "Build 1.74.0.0.104319: the game ruleset schemas (0C6 STUGameRulesetSchema \
m_entries: identifier m_3E783677 -> m_value.m_default; bools, ints, floats, enum choices as their 01C identifier) \
the heroes (075) and game modes (0C5) list in m_gameRulesetSchemas, in order. Entry keys are 01C indexes. \
settings: each 0C6's m_displayText and entries (key m_3E783677, name m_displayText, type by m_value's class, \
bool words m_9EC1DF9A (true) / m_03613078 (false), number m_min/m_max, enum m_3FE1EA9E choices: name \
m_displayText, identifier m_identifier; an ability's entry: button m_65184E78.m_logicalButton, format \
m_7DF418A5), texts in the install's locale. abilities: each hero's m_heroLoadout loadouts with a \
m_logicalButton: name m_name, button.";

const MODES_SOURCE: &str = "Build 1.74.0.0.104319: each game mode (0C5): its teams (m_teams: m_team as \
TeamIndex, m_controllerScript graph, m_bodyScript graph, m_170AA4B8 players) and m_0FC17230 (the loading \
screen waits for the mode's own script).";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let heroes = load(install, HERO)?;
    let modes = load(install, MODE)?;
    let mut hero_schemas = Map::new();
    let mut mode_schemas = Map::new();
    let mut listed = Vec::new();
    for (table, assets, kind) in [
        (&mut hero_schemas, &heroes, HERO),
        (&mut mode_schemas, &modes, MODE),
    ] {
        for (&index, asset) in assets {
            let schemas: Vec<u64> = Node::root(asset)?.guids("m_gameRulesetSchemas").collect();
            if schemas.is_empty() {
                continue;
            }
            listed.extend(schemas.iter().map(|schema| schema & INDEX));
            table.insert(
                key(typed(index, kind)),
                schemas.iter().map(|&schema| json!(key(schema))).collect(),
            );
        }
    }
    listed.sort_unstable();
    listed.dedup();
    let stored = load(install, SCHEMA)?;
    let mut schemas = Map::new();
    for index in listed {
        let asset = stored
            .get(&index)
            .ok_or_else(|| asset_error(format!("no ruleset schema {index:012X}.0C6")))?;
        let mut entries = Map::new();
        for entry in Node::root(asset)?.nodes("m_entries") {
            let identifier = entry.guid("m_3E783677") & INDEX;
            let value = entry_default(entry).ok_or_else(|| {
                asset_error(format!(
                    "{index:012X}.0C6: entry {identifier:X} has no default the server reads"
                ))
            })?;
            entries.insert(identifier.to_string(), value);
        }
        schemas.insert(key(typed(index, SCHEMA)), Json::Object(entries));
    }
    // Every schema's settings as the text names them, listed or not.
    let mut settings = Map::new();
    for (&index, asset) in &stored {
        let root = Node::root(asset)?;
        let entries: Vec<Json> = root
            .nodes("m_entries")
            .filter_map(|entry| entry_setting(install, entry))
            .collect();
        settings.insert(
            key(typed(index, SCHEMA)),
            json!({
                "name": install.string(root.guid("m_displayText")),
                "entries": entries,
            }),
        );
    }
    // Each hero's abilities on a logical button: a settings entry with a
    // button (`m_65184E78`) is named, in a hero's block, after that hero's
    // ability on it.
    let mut abilities = Map::new();
    for (&index, asset) in &heroes {
        let list: Vec<Json> = Node::root(asset)?
            .guids("m_heroLoadout")
            .filter_map(|loadout| {
                let asset = install.stu(loadout).ok()?;
                let node = Node::root(&asset).ok()?;
                let button = node.guid("m_logicalButton");
                (button != 0).then(
                    || json!({"name": install.string(node.guid("m_name")), "button": key(button)}),
                )
            })
            .collect();
        if !list.is_empty() {
            abilities.insert(key(typed(index, HERO)), Json::Array(list));
        }
    }
    let rulesets = json!({
        "source": RULESETS_SOURCE,
        "schemas": schemas,
        "heroes": hero_schemas,
        "modes": mode_schemas,
        "settings": settings,
        "abilities": abilities,
    });

    let mut entries = Map::new();
    for (&index, asset) in &modes {
        let root = Node::root(asset)?;
        let teams: Vec<Json> = root
            .nodes("m_teams")
            .map(|team| {
                json!({
                    "team": team.int("m_team"),
                    "controller": optional_key(team.guid("m_controllerScript")),
                    "body": optional_key(team.guid("m_bodyScript")),
                    "players": team.int("m_170AA4B8"),
                })
            })
            .collect();
        entries.insert(
            key(typed(index, MODE)),
            json!({
                "teams": teams,
                "scripted_loading": root.int("m_0FC17230") == 1,
            }),
        );
    }
    let modes = json!({ "source": MODES_SOURCE, "modes": entries });
    Ok(vec![
        Output::json(RULESETS, &rulesets),
        Output::json(MODES, &modes),
    ])
}

/// A null reference as JSON null, anything else as its key.
fn optional_key(guid: u64) -> Json {
    if guid == 0 {
        Json::Null
    } else {
        json!(key(guid))
    }
}

/// A schema entry's default by the class of its `m_value`: `STU_8A8AA0A4`
/// a bool, `STU_118786E9` an int, `STU_A499C365` a float, `STU_776E5ADD`
/// an enum choice (the identifier of `m_default`).
fn entry_default(entry: Node) -> Option<Json> {
    let value = entry.node("m_value")?;
    Some(match value.class_name()? {
        "STU_8A8AA0A4" => json!(value.int("m_default") != 0),
        "STU_118786E9" => json!(value.int("m_default")),
        "STU_A499C365" => float(value.value("m_default")?)?,
        "STU_776E5ADD" => json!(key(value.guid("m_default"))),
        _ => return None,
    })
}

/// A schema entry as the settings text names it (the importer matches a
/// line's name with `m_displayText`, `sub_7FF789A8A0C0`): its identifier,
/// name, and the value's type with what the text writes: a bool's two
/// words (`m_9EC1DF9A` for true, `m_03613078` for false: "Enabled",
/// "Disabled"; an inference from the defaults), a number's range
/// (`m_min`, `m_max`), an enum's choices by name. An entry of a hero's
/// ability has its logical button (`m_65184E78.m_logicalButton`) and the
/// name format a hero's block writes it with (`m_7DF418A5`: "%1$s
/// Cooldown Time" makes "Rocket Punch Cooldown Time"; none: the ability's
/// name alone, as "Seismic Slam: Off").
fn entry_setting(install: &Install, entry: Node) -> Option<Json> {
    let value = entry.node("m_value")?;
    let text = |node: Node, field: &str| install.string(node.guid(field));
    let mut setting = Map::new();
    setting.insert("key".into(), json!(entry.guid("m_3E783677") & INDEX));
    setting.insert("name".into(), json!(text(entry, "m_displayText")));
    let button = entry
        .node("m_65184E78")
        .map_or(0, |button| button.guid("m_logicalButton"));
    if button != 0 {
        setting.insert("button".into(), json!(key(button)));
        setting.insert("format".into(), json!(text(entry, "m_7DF418A5")));
    }
    match value.class_name()? {
        "STU_8A8AA0A4" => {
            setting.insert("type".into(), json!("bool"));
            setting.insert("true".into(), json!(text(value, "m_9EC1DF9A")));
            setting.insert("false".into(), json!(text(value, "m_03613078")));
        }
        "STU_118786E9" => {
            setting.insert("type".into(), json!("int"));
            setting.insert("min".into(), json!(value.int("m_min")));
            setting.insert("max".into(), json!(value.int("m_max")));
        }
        "STU_A499C365" => {
            setting.insert("type".into(), json!("float"));
            setting.insert("min".into(), float(value.value("m_min")?)?);
            setting.insert("max".into(), float(value.value("m_max")?)?);
        }
        "STU_776E5ADD" => {
            setting.insert("type".into(), json!("enum"));
            let choices: Vec<Json> = value
                .nodes("m_3FE1EA9E")
                .map(|choice| {
                    json!({
                        "name": text(choice, "m_displayText"),
                        "identifier": choice.guid("m_identifier") & INDEX,
                    })
                })
                .collect();
            setting.insert("choices".into(), json!(choices));
        }
        _ => return None,
    }
    Some(Json::Object(setting))
}

/// An `f32` field as DataTool prints it.
fn float(value: &Value) -> Option<Json> {
    match value {
        Value::Float(number) => Some(shortest(*number)),
        _ => None,
    }
}

/// An `f32` as DataTool prints it (the shortest text that reads back the
/// same `f32`), read back as a double the way the Python reference did.
fn shortest(value: f64) -> Json {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}
