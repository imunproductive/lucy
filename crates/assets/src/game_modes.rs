//! `data/game_rulesets_174.json` and `data/game_modes_174.json`: what the
//! game modes (`0C5`) and heroes (`075`) set up for a match.
//!
//! The rulesets file holds the defaults `GetGameRulesetValue` answers
//! (`ow174/game/script/rulesets.py`, which froze them from a DataTool dump):
//! every game ruleset schema (`0C6` `STUGameRulesetSchema`) the heroes and
//! modes list, each entry's identifier (`m_3E783677`, an `01C`) and default
//! (`m_value`: a bool, an int, a float or an enum choice, the identifier of
//! `m_default`), and each hero's and mode's `m_gameRulesetSchemas` in order.
//!
//! The modes file holds, per mode, its teams (`m_teams`: the team, its
//! controller graph `m_controllerScript` and its players `m_170AA4B8`) and
//! whether its loading screen waits for the mode's own script
//! (`m_0FC17230`; `SCRIPTED_LOADING_MODES` in `ow174/game/content.py`).

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
the heroes (075) and game modes (0C5) list in m_gameRulesetSchemas, in order. Entry keys are 01C indexes.";

const MODES_SOURCE: &str = "Build 1.74.0.0.104319: each game mode (0C5): its teams (m_teams: m_team as \
TeamIndex, m_controllerScript graph, m_170AA4B8 players) and m_0FC17230 (the loading screen waits for the \
mode's own script).";

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
    let rulesets = json!({
        "source": RULESETS_SOURCE,
        "schemas": schemas,
        "heroes": hero_schemas,
        "modes": mode_schemas,
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
        "STU_A499C365" => match value.value("m_default")? {
            Value::Float(number) => float(*number),
            _ => return None,
        },
        "STU_776E5ADD" => json!(key(value.guid("m_default"))),
        _ => return None,
    })
}

/// An `f32` as DataTool prints it (the shortest text that reads back the
/// same `f32`), read back as a double the way the Python reference did.
fn float(value: f64) -> Json {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}
