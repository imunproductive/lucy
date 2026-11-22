//! `data/game_stats_174.json`: the stats the Tab board lists for a player,
//! from the game data.
//!
//! The board shows the local player's component 64: the player stats
//! (eliminations, objective kills and time, hero damage, healing, deaths) and
//! the hero stats (accuracy, hero abilities). Which stats those are is in the
//! data: the generic settings `0051.054` (`STU_13047F85`) `m_FC833C02` holds
//! the player stats and `m_A38A50B4` the game modes that list others; each
//! hero (`075`) `m_FC833C02` holds its hero stats and `m_A341183E` the game
//! modes that list others. An entry's `m_60BFB2D1` is 1 on Deaths (the
//! server's medals leave it out). Each stat (`062`) says how the client prints
//! its value (`m_displayType`, `STUStatDisplayType`: 0 a count, 2 a fraction
//! shown as a percentage, 8 seconds shown as a time; enum table
//! `0x7FF78C049280`).
//!
//! The file: `"player"` and `"heroes"` (by hero GUID) hold `{"default":
//! [[stat, no medal]], "modes": {mode: [...]}}`, `"stats"` each stat's display
//! type. The heroes are those of `data/game_heroes_174.json`, read from the
//! build folder.
//!
//! The Python reference's separate `--score-events` export (the `003F.054`
//! score event catalog, written outside the pack) is not part of the pack and
//! has no derivation.

use std::{collections::BTreeSet, path::Path};

use lucy_stu::{Asset, Object, Value};
use serde_json::{Map, json};

use super::{Assets, field, guid, parse_guid, read_json};
use crate::{Install, Output, Result, asset_error, guid_hex, guid_type};

const SETTINGS: u16 = 0x054;
const HERO: u16 = 0x075;
const STAT: u16 = 0x062;
const GAME_MODE: u16 = 0x0C5;
/// The generic settings that hold the player stats.
const PLAYER_STATS: u64 = 0x51;
/// `STUStatDisplayType`'s values in the 1.74 client's enum table; a value
/// outside them has no display type (`null`).
const DISPLAY_TYPES: std::ops::RangeInclusive<i64> = 0..=8;

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let settings = Assets::of(install, SETTINGS)
        .read(install, PLAYER_STATS)?
        .ok_or_else(|| asset_error("the install has no 000000000051.054"))?;
    let root = settings
        .root()
        .filter(|root| root.class_name() == Some("STU_13047F85"))
        .ok_or_else(|| {
            asset_error("000000000051.054 is not the player stats settings (STU_13047F85)")
        })?;
    let player = lists(&settings, root, "m_FC833C02", "m_A38A50B4");

    let heroes = read_json(out, "data/game_heroes_174.json")?;
    let heroes = heroes
        .as_object()
        .ok_or_else(|| asset_error("data/game_heroes_174.json is not an object"))?;
    let hero_assets = Assets::of(install, HERO);
    let mut hero_lists = Map::new();
    for key in heroes.keys() {
        let index = parse_guid(key)?;
        let asset = hero_assets
            .read(install, index)?
            .ok_or_else(|| asset_error(format!("the install has no hero {key}")))?;
        let root = asset
            .root()
            .ok_or_else(|| asset_error(format!("hero {key} has no root")))?;
        hero_lists.insert(key.clone(), lists(&asset, root, "m_FC833C02", "m_A341183E"));
    }

    let mut used = BTreeSet::new();
    for group in std::iter::once(&player).chain(hero_lists.values()) {
        let modes = group["modes"].as_object().into_iter().flat_map(Map::values);
        for stats in std::iter::once(&group["default"]).chain(modes) {
            for entry in stats.as_array().into_iter().flatten() {
                used.insert(entry[0].as_str().expect("a stat GUID").to_owned());
            }
        }
    }
    let stat_assets = Assets::of(install, STAT);
    let mut stats = Map::new();
    for stat in used {
        let index = parse_guid(&stat)?;
        let asset = stat_assets
            .read(install, index)?
            .ok_or_else(|| asset_error(format!("the install has no stat {stat}")))?;
        let display = match asset.root().and_then(|stat| field(stat, "m_displayType")) {
            Some(Value::Int(value)) if DISPLAY_TYPES.contains(value) => json!(value),
            _ => serde_json::Value::Null,
        };
        stats.insert(stat, display);
    }

    let table = json!({"player": player, "heroes": hero_lists, "stats": stats});
    Ok(vec![Output::json("data/game_stats_174.json", &table)])
}

/// `data/game_stat_rules_174.json`: how the stat table keeps the stats
/// whose value isn't a plain sum, from each stat's (`062`, `STUStat`) own
/// fields (every stat of the install, `refs/reports/StatCredit.md`):
///
/// - `"best"`: a "Best" stat (`m_timeFrame` 6: "Multikill - Best",
///   "Transcendence Healing - Best", the arcade "Best … Score" stats) keeps
///   one amount, the largest (`m_updateType` 1, `"max"`) or the smallest
///   (2, `"min"`: "Fastest Gun") rather than their sum.
/// - `"derived"`: a derived stat (`m_derivedStat` 1) of `m_updateType` 3
///   combines its base stat `m_4C9917C1` with `m_155E2A47` by
///   `m_16CCEFC8`: 1 the base divided by the other (the accuracies, shown
///   as a percentage, "Average Energy", the averages), 3 their sum ("Shots
///   Hits + Missed", the Weapon Accuracy's divisor): `{"ratio": [base,
///   other]}` or `{"sum": [base, other]}`. A derived stat of
///   `m_updateType` 4 is a copy of its base (Eliminations `0025` of
///   `001F`): left out, the server counts the board's copies themselves.
///   Another operator is left out.
pub(crate) fn derive_rules(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut best = Map::new();
    let mut derived = Map::new();
    let mut guids = install.guids(STAT);
    guids.sort_unstable();
    for (stat, asset) in install.stu_all(&guids) {
        let asset = asset?;
        let Some(root) = asset.root() else {
            continue;
        };
        let int = |name| match field(root, name) {
            Some(Value::Int(value)) => *value,
            Some(Value::UInt(value)) => *value as i64,
            _ => 0,
        };
        let stat_ref = |name| match field(root, name) {
            Some(Value::Guid(other)) if *other != 0 && guid_type(*other) == STAT => {
                Some(guid_hex(guid(*other, STAT)))
            }
            _ => None,
        };
        let key = guid_hex(guid(stat, STAT));
        match (int("m_timeFrame"), int("m_updateType")) {
            (BEST, 1) => {
                best.insert(key.clone(), json!("max"));
            }
            (BEST, 2) => {
                best.insert(key.clone(), json!("min"));
            }
            _ => {}
        }
        if int("m_derivedStat") == 0 || int("m_updateType") != COMBINED {
            continue;
        }
        let (Some(base), Some(other)) = (stat_ref("m_4C9917C1"), stat_ref("m_155E2A47")) else {
            continue;
        };
        let operator = match int("m_16CCEFC8") {
            1 => "ratio",
            3 => "sum",
            _ => continue,
        };
        let mut rule = Map::new();
        rule.insert(operator.to_owned(), json!([base, other]));
        derived.insert(key, rule.into());
    }
    let table = json!({"best": best, "derived": derived});
    Ok(vec![Output::json("data/game_stat_rules_174.json", &table)])
}

/// `STUStatTimeframe` of a "Best" stat.
const BEST: i64 = 6;
/// `STUStatUpdateType` of a derived stat that combines two stats.
const COMBINED: i64 = 3;

/// `{"default": entries of the default list, "modes": {mode: entries}}` of
/// one settings or hero asset.
///
/// The Python reference paired each game mode with the next stats list after
/// it, so a mode entry without a game mode is left out, and a mode entry
/// without a list (null `m_stats`) takes the next entry's list, that entry's
/// own mode being lost; a mode with no list after it is left out. In 1.74 the
/// only entry without a list is hero `0x79`'s last (mode `0x20`), which is
/// therefore left out. A mode named twice keeps its first place and its last
/// list.
fn lists(asset: &Asset, root: &Object, default: &str, modes: &str) -> serde_json::Value {
    let mut by_mode = Map::new();
    let mut pending = None;
    for mode in objects(asset, root.get(modes)) {
        if pending.is_none() {
            pending = match mode.get("m_gamemode") {
                Some(Value::Guid(game_mode))
                    if *game_mode != 0 && guid_type(*game_mode) == GAME_MODE =>
                {
                    Some(*game_mode)
                }
                _ => None,
            };
        }
        let stats @ Some(Value::Array(_)) = mode.get("m_stats") else {
            continue;
        };
        if let Some(game_mode) = pending.take() {
            by_mode.insert(guid_hex(guid(game_mode, GAME_MODE)), entries(asset, stats));
        }
    }
    json!({"default": entries(asset, root.get(default)), "modes": by_mode})
}

/// `[stat GUID, no medal]` of each `STU_C0D5117B` of a list that names a
/// stat. "No medal" is `m_60BFB2D1`; the Python reference matched it as one
/// digit, so an entry with a larger value is left out.
fn entries(asset: &Asset, list: Option<&Value>) -> serde_json::Value {
    let mut out = Vec::new();
    for entry in objects(asset, list) {
        let Some(Value::Guid(stat)) = entry.get("m_stat") else {
            continue;
        };
        if *stat == 0 || guid_type(*stat) != STAT {
            continue;
        }
        let flag = match field(entry, "m_60BFB2D1") {
            Some(Value::UInt(flag)) if *flag <= 9 => *flag,
            _ => continue,
        };
        out.push(json!([guid_hex(guid(*stat, STAT)), flag]));
    }
    out.into()
}

/// The objects of an array field.
fn objects<'a>(asset: &'a Asset, list: Option<&'a Value>) -> impl Iterator<Item = &'a Object> {
    let items = match list {
        Some(Value::Array(items)) => items.as_slice(),
        _ => &[],
    };
    items.iter().filter_map(|item| asset.resolve(item))
}
