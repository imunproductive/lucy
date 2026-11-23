//! `data/career_stats_174.json`: the stats (`062`, `STUStat`) the career
//! profile keeps for each hero and each map, the two stat catalogs of the
//! lobby's mode rules (27202 `+0x90` and `+0xC0`). Without them the career
//! profile's Statistics show 0.
//!
//! Each stat says what it is kept for (`m_heroesType`, `m_mapsType`); the
//! lists follow from that. Checked against the lists retail sent in 2022
//! (1.68): equal for every hero, "all heroes", role and map
//! (`D:/Work/Coding/refs/reports/RetailTemplates.md`, "Career stat
//! catalogs"; test `the_career_stat_catalogs_are_retails`,
//! `tests/heroes.rs`).
//!
//! - `heroes`, by hero (`075`, the heroes with a role, `m_category`), then
//!   "all heroes" (index `0xFFFFFFFF`), then each role (`0D8`):
//!   - a hero: the stats of `m_heroesType` 1 (every hero), those of type 2
//!     whose `m_heroes` names the hero, and the derived stats
//!     (`m_derivedStat`) of type 1 whose inputs (`m_4C9917C1`,
//!     `m_155E2A47`) are all in the hero's list or are account stats (no
//!     `m_heroesType`);
//!   - all heroes: the stats without `m_heroesType`, except those of
//!     `m_type` 4 (kept for a match only);
//!   - a role: the stats of `m_heroesType` 3.
//! - `maps`, by map (`002`, one per map header `09F`, as the preload lists
//!   them), then "all maps" (index `0xFFFFFFFF`): the stats of
//!   `m_heroesType` 4: for a map those of `m_mapsType` 1 (every map) and
//!   those of type 2 whose `m_maps` names it, and the derived ones of type 1
//!   whose inputs are all in the map's list; for all maps those without
//!   `m_mapsType`.
//!
//! Every list is in GUID order, as retail's were.

use std::{collections::BTreeSet, path::Path};

use serde_json::json;

use crate::{
    Install, Output, Result,
    game_queues::{Node, key, load, typed},
};

pub(crate) const PATH: &str = "data/career_stats_174.json";

const STAT: u16 = 0x062;
const HERO: u16 = 0x075;
const MAP: u16 = 0x002;
const MAP_HEADER: u16 = 0x09F;
/// The index of the "all heroes" and "all maps" entries.
const ALL: u64 = 0xFFFF_FFFF;
const INDEX: u64 = 0xFFFF_FFFF_FFFF;

/// `m_heroesType`: no value is an account stat.
const EVERY_HERO: i64 = 1;
const NAMED_HEROES: i64 = 2;
const ROLE: i64 = 3;
const MAP_STAT: i64 = 4;
/// `m_mapsType`.
const EVERY_MAP: i64 = 1;
const NAMED_MAPS: i64 = 2;
/// `m_type` of a stat kept for a match only.
const MATCH_ONLY: i64 = 4;

struct Stat {
    guid: u64,
    kind: i64,
    heroes_type: i64,
    maps_type: i64,
    derived: bool,
    /// Index sets of `m_heroes` and `m_maps`.
    heroes: BTreeSet<u64>,
    maps: BTreeSet<u64>,
    inputs: Vec<u64>,
}

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let stats = stats(install)?;
    let account: BTreeSet<u64> = stats
        .iter()
        .filter(|stat| stat.heroes_type == 0)
        .map(|stat| stat.guid)
        .collect();

    let mut roles = BTreeSet::new();
    let mut heroes = Vec::new();
    for (index, asset) in load(install, HERO)? {
        let role = Node::root(&asset)?.guid("m_category");
        if role != 0 {
            roles.insert(role);
            let base = stats.iter().filter(|stat| {
                !stat.derived
                    && (stat.heroes_type == EVERY_HERO
                        || stat.heroes_type == NAMED_HEROES && stat.heroes.contains(&index))
            });
            let list = with_derived(&stats, base, EVERY_HERO, |stat| stat.heroes_type, &account);
            heroes.push(entry(typed(index, HERO), list));
        }
    }
    let all_heroes = stats
        .iter()
        .filter(|stat| stat.heroes_type == 0 && stat.kind != MATCH_ONLY);
    heroes.push(entry(
        typed(ALL, HERO),
        all_heroes.map(|stat| stat.guid).collect(),
    ));
    for role in roles {
        let list = stats.iter().filter(|stat| stat.heroes_type == ROLE);
        heroes.push(entry(role, list.map(|stat| stat.guid).collect()));
    }

    let map_stats: Vec<&Stat> = stats
        .iter()
        .filter(|stat| stat.heroes_type == MAP_STAT)
        .collect();
    let mut maps = Vec::new();
    for index in load(install, MAP_HEADER)?.into_keys() {
        let base = map_stats.iter().copied().filter(|stat| {
            !stat.derived
                && (stat.maps_type == EVERY_MAP
                    || stat.maps_type == NAMED_MAPS && stat.maps.contains(&index))
        });
        let list = with_derived(
            &stats,
            base,
            EVERY_MAP,
            |stat| {
                if stat.heroes_type == MAP_STAT {
                    stat.maps_type
                } else {
                    0
                }
            },
            &BTreeSet::new(),
        );
        maps.push(entry(typed(index, MAP), list));
    }
    let all_maps = map_stats.iter().filter(|stat| stat.maps_type == 0);
    maps.push(entry(
        typed(ALL, MAP),
        all_maps.map(|stat| stat.guid).collect(),
    ));

    let table = json!({"heroes": heroes, "maps": maps});
    Ok(vec![Output::json(PATH, &table)])
}

/// `base`, then every derived stat of `kind(stat) == of` whose inputs are
/// all in the list or in `also`, until none is added (a derived stat can
/// take another's value).
fn with_derived<'a>(
    stats: &[Stat],
    base: impl Iterator<Item = &'a Stat>,
    of: i64,
    kind: impl Fn(&Stat) -> i64,
    also: &BTreeSet<u64>,
) -> BTreeSet<u64> {
    let mut list: BTreeSet<u64> = base.map(|stat| stat.guid).collect();
    loop {
        let added: Vec<u64> = stats
            .iter()
            .filter(|stat| {
                stat.derived
                    && kind(stat) == of
                    && !list.contains(&stat.guid)
                    && !stat.inputs.is_empty()
                    && stat
                        .inputs
                        .iter()
                        .all(|input| list.contains(input) || also.contains(input))
            })
            .map(|stat| stat.guid)
            .collect();
        if added.is_empty() {
            return list;
        }
        list.extend(added);
    }
}

fn entry(key_guid: u64, stats: BTreeSet<u64>) -> serde_json::Value {
    json!({
        "key": key(key_guid),
        "stats": stats.into_iter().map(key).collect::<Vec<_>>(),
    })
}

/// Every stat of the install, in GUID order.
fn stats(install: &Install) -> Result<Vec<Stat>> {
    let mut stats = Vec::new();
    for (index, asset) in load(install, STAT)? {
        let node = Node::root(&asset)?;
        let indexes = |name| node.guids(name).map(|guid| guid & INDEX).collect();
        let inputs = ["m_4C9917C1", "m_155E2A47"]
            .into_iter()
            .map(|name| node.guid(name))
            .filter(|&guid| guid != 0)
            .map(|guid| typed(guid & INDEX, STAT))
            .collect();
        stats.push(Stat {
            guid: typed(index, STAT),
            kind: node.int("m_type"),
            heroes_type: node.int("m_heroesType"),
            maps_type: node.int("m_mapsType"),
            derived: node.int("m_derivedStat") != 0,
            heroes: indexes("m_heroes"),
            maps: indexes("m_maps"),
            inputs,
        });
    }
    Ok(stats)
}
