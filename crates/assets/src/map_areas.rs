//! `data/game_map_areas_174.json`: the volumes of each map that the server
//! tests a player's body against (the spawn rooms and the damage areas) and
//! the spawn door barriers the server must create.
//!
//! A spawn room is a map entity that runs graph `0D62` (definition `1806` on
//! every map read: one per room). Its `STUTargetTagInstanceData` names the
//! room's area placeable (`m_2746D7E4`, `crate::map_chunk`) and the room's
//! target tags (`0x310`, `0x341`, ... as the spawn points of the room carry
//! them, `crate::game_maps`), and its `0D62` override of variable 1777 is
//! the team that owns it (`STU_BD6CC023`, 0 blue, 1 red, -1 none). The mode
//! graphs send the rooms game message `0724` (Practice Range `13C0`: its
//! three flags `03D4`, `03D5`, `03D3` all true); `0D62` passes them on in
//! `0723` to its team's heroes inside, and the mode body graph `0C8E` stores
//! them in v9014, v420 and v8419 (`m_allowHeroSelect`).
//!
//! Strong guess, not read in the client: that the area the room names is
//! where a player counts as in the room. On the Practice Range its three
//! boxes hold the room around the team 0 spawn point, and the client's
//! "is in spawn room" (`STU_E4C6AAB4`) reads only the server's component 74
//! flag (`docs/notes/related-projects.md`).
//!
//! A damage area is a map entity with `STUECDamageAreaInstanceData`
//! (class `6E7ED1C2`, definition `0109` on every map read; 65 entities): its
//! `m_2746D7E4` (+8) names a box area as a spawn room's does, `m_B81F7395`
//! (+24, a float) is its damage, and the bytes `m_1BB427AE` (+28) and
//! `m_13F2925E` (+29) are flags not read here. The damages are 100 (most
//! maps: the pits and edges of Ilios, Nepal, Lijiang Tower, Oasis,
//! Eichenwalde, Volskaya, ...), 9000 (Busan), 9999 (Petra) and 0 (the two
//! car-sized boxes on Oasis City Center's road and a box under Lijiang
//! Night Market and Nepal Village (Winter), which their graphs may turn on).
//! An area whose box area is not in the map's chunks is left out (Nepal
//! Village `0736` and map `0794` name such areas).
//!
//! A spawn door barrier is a server-owned entity placeable (client id 0)
//! running graph `00F6`: on the Practice Range six of them stand in the
//! three doorways of the spawn room (definitions `1307` and `130C`, a model
//! with the contact set `0092.049`, and `1225`, a model with the child
//! `12F4`'s effect `178C.00D`), each with `00F6` overrides `0877` = 0 and
//! `0878` = 1. The client builds a server-owned placeable only when the
//! server creates it (`crate::game_maps`), so without the create the doorway
//! is bare. Strong guess: these are the doorways' glowing barrier frames.

use std::path::Path;

use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_maps::{
        Placed, TARGET_AREA, TARGET_TAG_DATA, chunk_index, graph_vars, instance, map_chunks,
        read_map, target_tags,
    },
    guid_hex,
    map_chunk::{area_boxes, guid_of, name_index, short},
};

pub(crate) const PATH: &str = "data/game_map_areas_174.json";

/// The spawn room graph.
const SPAWN_ROOM_GRAPH: u64 = 0x0D62;
/// `0D62`'s team: its TrackTargets find this team's heroes.
const ROOM_TEAM: u64 = 1777;
/// The spawn door barrier graph.
const DOOR_BARRIER_GRAPH: u64 = 0x00F6;
/// `STUECDamageAreaInstanceData`: the area (a teUUID) at +8, the damage (a
/// float) at +24.
const DAMAGE_AREA_DATA: u32 = 0x6E7E_D1C2;
const DAMAGE_AREA: usize = 8;
const DAMAGE: usize = 24;
const TARGET_TAG: u16 = 0x06C;
const MAP: u16 = 0x002;

const SOURCE: &str = "Build 1.74.0.0.104319: per map (keys as game_maps_174.json), its spawn rooms: \
every entity placeable running graph 0D62 (placeable = its flat index), its team (the placeable's 0D62 \
override of v1777, STU_BD6CC023 TeamIndex: 0 blue, 1 red, -1 none), its target tags \
(STUTargetTagInstanceData.m_targetTags) and the boxes of the area placeable its \
STUTargetTagInstanceData.m_2746D7E4 names (chunk 0x0A: centre, half extents, rotation x y z w); \
its damage areas: every entity placeable with STUECDamageAreaInstanceData whose m_2746D7E4 names a box \
area of the map, its first identifier (a placeable with one exists only in the modes that list it), \
its damage (m_B81F7395) and its boxes; its door barriers: every entity placeable with client id 0 \
running graph 00F6, with its first identifier.";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let chunks = chunk_index(install);
    let mut maps: Vec<u64> = chunks.keys().copied().collect();
    maps.sort_unstable();
    let mut derived = Map::new();
    for map in maps {
        let data = map_chunks(install, &chunks, map);
        // A map whose rooms don't fit keeps the reason instead: the server
        // then counts nobody in its rooms.
        let mut entry = match spawn_rooms(&data) {
            Ok(rooms) if rooms.is_empty() => Map::new(),
            Ok(rooms) => Map::from_iter([("spawn_rooms".into(), json!(rooms))]),
            Err(error) => Map::from_iter([("error".into(), json!(error.to_string()))]),
        };
        let (damage_areas, door_barriers) = match read_map(&data) {
            Ok((base, entities)) => (
                damage_areas(&data, base, &entities),
                door_barriers(base, &entities),
            ),
            Err(_) => (Vec::new(), Vec::new()),
        };
        if !damage_areas.is_empty() {
            entry.insert("damage_areas".into(), json!(damage_areas));
        }
        if !door_barriers.is_empty() {
            entry.insert("door_barriers".into(), json!(door_barriers));
        }
        if !entry.is_empty() {
            derived.insert(guid_hex(guid_of(map, MAP)), Json::Object(entry));
        }
    }
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "maps": derived}),
    )])
}

/// The damage areas of a map with a box area, in placeable order.
fn damage_areas(chunks: &[(u64, Vec<u8>)], base: usize, entities: &[Placed<'_>]) -> Vec<Json> {
    let areas = area_boxes(chunks);
    entities
        .iter()
        .enumerate()
        .filter_map(|(number, placed)| {
            let (stu, root) = instance(placed, DAMAGE_AREA_DATA)?;
            let boxes = areas.get(&stu.uuid(root + DAMAGE_AREA)?)?;
            let damage = stu.f32(root + DAMAGE)?;
            let mut area = placeable_entry(base + number, placed);
            area.insert("damage".into(), json!(short(damage)));
            area.insert("boxes".into(), json!(boxes));
            Some(Json::Object(area))
        })
        .collect()
}

/// The server-owned spawn door barriers of a map, in placeable order.
fn door_barriers(base: usize, entities: &[Placed<'_>]) -> Vec<Json> {
    entities
        .iter()
        .enumerate()
        .filter(|(_, placed)| {
            placed.entity.client_id == 0
                && graph_vars(&placed.instances).contains_key(&DOOR_BARRIER_GRAPH)
        })
        .map(|(number, placed)| Json::Object(placeable_entry(base + number, placed)))
        .collect()
}

/// A placeable's flat index and, when it has one, its first identifier.
fn placeable_entry(flat: usize, placed: &Placed<'_>) -> Map<String, Json> {
    let mut entry = Map::from_iter([("placeable".into(), json!(flat))]);
    if placed.entity.identifier != 0 {
        entry.insert(
            "identifier".into(),
            json!(guid_hex(placed.entity.identifier)),
        );
    }
    entry
}

/// The spawn rooms of a map, in placeable order.
fn spawn_rooms(chunks: &[(u64, Vec<u8>)]) -> Result<Vec<Json>> {
    let (base, entities) = read_map(chunks)?;
    let areas = area_boxes(chunks);
    let mut rooms = Vec::new();
    for (number, placed) in entities.iter().enumerate() {
        let graphs = graph_vars(&placed.instances);
        let Some(vars) = graphs.get(&SPAWN_ROOM_GRAPH) else {
            continue;
        };
        let flat = base + number;
        // A room the placeable gives no team gets one from the mode
        // (`0724` parameter `0473`, the PvP mode graphs).
        let team = vars.team(ROOM_TEAM)?.filter(|team| *team >= 0);
        let area = instance(placed, TARGET_TAG_DATA)
            .and_then(|(stu, root)| stu.uuid(root + TARGET_AREA))
            .filter(|uuid| *uuid != [0; 16])
            .ok_or_else(|| asset_error(format!("spawn room {flat} names no area")))?;
        let boxes = areas
            .get(&area)
            .ok_or_else(|| asset_error(format!("spawn room {flat}: its area is not a box area")))?;
        let tags: Vec<String> = target_tags(&placed.instances)
            .into_iter()
            .map(|tag| guid_hex(guid_of(name_index(tag), TARGET_TAG)))
            .collect();
        rooms.push(json!({
            "placeable": flat,
            "team": team,
            "tags": tags,
            "boxes": boxes,
        }));
    }
    Ok(rooms)
}
