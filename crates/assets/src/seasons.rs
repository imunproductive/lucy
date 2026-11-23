//! `data/competitive_seasons_174.json`: every competitive season the install
//! knows, from the season block of its queue card (`0C7`).
//!
//! A competitive card (`m_157AF68C` = 1) carries its season in
//! `m_2226CBD8` (`STU_91B28B0A`). Each season record of the lobby's ranked
//! state (36300 `+0x98`) is that block in wire form, field for field:
//!
//! | wire | card field | |
//! |---|---|---|
//! | `+0x0` | `m_5BB8DFF3` | reward tiers: lowest (`m_4BF7CD58`) and highest (`m_D2F14FFA`) rating, the tier's unlocks |
//! | `+0x20`..`+0x38` | `m_F82A8727`, `m_275B6D5C`, `m_961B4853`, `m_9A31D01C` | four dates (`STU` date-times, 8 bytes each): the start, the end of the first stage, the end and the end of the rewards |
//! | `+0x40`, `+0x48` | `m_1E4E5957`, `m_B804C4DB` | the season's name strings ("Season 32") |
//! | `+0x50`, `+0x68` | `m_58066D8F`, `m_heroicUnlocks` | unlock lists |
//! | `+0x80`, `+0x98`, `+0xC0` | `m_EC70AD04`, `m_590CF462`, `m_56E7DD46` | lists of heroes and maps (seasons 8-10 only) |
//! | `+0xB0`, `+0xB8` | `m_E0B3D69F`, `m_21EB3E73` | textures |
//! | `+0xD8` | the card's maps | from `game_queues_174.json` |
//! | `+0xF0` | `m_2593569C` | the role queue season an open queue card belongs to |
//! | `+0xF8` | | the cards that name this one as theirs (the reverse of `+0xF0`) |
//! | `+0x110`, `+0x114` | `m_AE69132D`, `m_82AA08B4` | two counts, 50 and 10 unless the card sets them (25 and 5 on role queue seasons) |
//! | `+0x11C` | `m_B5DD91B7` | a flag (seasons 11 and later) |
//!
//! Checked against the 1.68 retail capture of 36300: every one of its 73
//! season records is a card of this list with these values.

use std::path::Path;

use lucy_stu::Value;
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{Node, key, load, typed},
};

pub(crate) const PATH: &str = "data/competitive_seasons_174.json";
const QUEUES: &str = "data/game_queues_174.json";

/// The four dates, in wire order.
const DATES: [&str; 4] = ["m_F82A8727", "m_275B6D5C", "m_961B4853", "m_9A31D01C"];

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let queues: Json = serde_json::from_slice(&std::fs::read(out.join(QUEUES))?)
        .map_err(|error| asset_error(format!("{QUEUES}: {error}")))?;
    let cards = load(install, 0x0C7)?;
    let mut seasons = Map::new();
    let mut parents = Vec::new();
    for (&index, asset) in &cards {
        let card = Node::root(asset)?;
        let Some(season) = card.node("m_2226CBD8") else {
            continue;
        };
        if card.int("m_157AF68C") != 1 {
            continue;
        }
        let name = key(typed(index, 0x0C7));
        let parent = card.guid("m_2593569C");
        if parent != 0 {
            parents.push((name.clone(), key(parent)));
        }
        let maps: Vec<Json> = queues["cards"][&name]["maps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|map| map["map"].clone())
            .collect();
        let mut dates = Vec::with_capacity(DATES.len());
        for field in DATES {
            dates.push(json!(date(season, field).ok_or_else(|| {
                asset_error(format!("{name}: season date {field} is not 8 bytes"))
            })?));
        }
        let tiers: Vec<Json> = season
            .nodes("m_5BB8DFF3")
            .map(|tier| {
                json!({
                    "lowest": tier.int("m_4BF7CD58"),
                    "highest": tier.int("m_D2F14FFA"),
                    "unlocks": unlocks(tier.node("m_unlocks")),
                })
            })
            .collect();
        seasons.insert(
            name,
            json!({
                "tiers": tiers,
                "dates": dates,
                "name": optional(season.guid("m_1E4E5957")),
                "title": optional(season.guid("m_B804C4DB")),
                "unlocks": unlocks(season.node("m_58066D8F")),
                "heroic_unlocks": unlocks(season.node("m_heroicUnlocks")),
                "heroes": guids(season, "m_EC70AD04"),
                "maps_featured": guids(season, "m_590CF462"),
                "featured": guids(season, "m_56E7DD46"),
                "texture": optional(season.guid("m_E0B3D69F")),
                "image": optional(season.guid("m_21EB3E73")),
                "maps": maps,
                "parent": if parent == 0 { Json::Null } else { json!(key(parent)) },
                "children": [],
                "count_a": season.int("m_AE69132D"),
                "count_b": season.int("m_82AA08B4"),
                "flag": season.int("m_B5DD91B7") == 1,
            }),
        );
    }
    for (child, parent) in parents {
        if let Some(Json::Array(children)) = seasons
            .get_mut(&parent)
            .and_then(|season| season.get_mut("children"))
        {
            children.push(json!(child));
        }
    }
    Ok(vec![Output::json(PATH, &Json::Object(seasons))])
}

fn optional(guid: u64) -> Json {
    if guid == 0 {
        json!(0)
    } else {
        json!(key(guid))
    }
}

fn date(season: Node, field: &str) -> Option<u64> {
    match season.value(field)? {
        Value::Bytes(bytes) => Some(u64::from_le_bytes(bytes.as_slice().try_into().ok()?)),
        _ => None,
    }
}

fn guids(node: Node, field: &str) -> Vec<String> {
    node.guids(field).map(key).collect()
}

fn unlocks(node: Option<Node>) -> Vec<String> {
    node.map_or_else(Vec::new, |node| guids(node, "m_unlocks"))
}
