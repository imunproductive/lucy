//! `data/game_map_areas_174.json`: the volumes of each map that the server
//! tests a player's body against: the spawn rooms.
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

use std::path::Path;

use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_maps::{
        TARGET_AREA, TARGET_TAG_DATA, chunk_index, graph_vars, instance, map_chunks, read_map,
        target_tags,
    },
    guid_hex,
    map_chunk::{area_boxes, guid_of, name_index},
};

pub(crate) const PATH: &str = "data/game_map_areas_174.json";

/// The spawn room graph.
const SPAWN_ROOM_GRAPH: u64 = 0x0D62;
/// `0D62`'s team: its TrackTargets find this team's heroes.
const ROOM_TEAM: u64 = 1777;
const TARGET_TAG: u16 = 0x06C;
const MAP: u16 = 0x002;

const SOURCE: &str = "Build 1.74.0.0.104319: per map (keys as game_maps_174.json), its spawn rooms: \
every entity placeable running graph 0D62 (placeable = its flat index), its team (the placeable's 0D62 \
override of v1777, STU_BD6CC023 TeamIndex: 0 blue, 1 red, -1 none), its target tags \
(STUTargetTagInstanceData.m_targetTags) and the boxes of the area placeable its \
STUTargetTagInstanceData.m_2746D7E4 names (chunk 0x0A: centre, half extents, rotation x y z w).";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let chunks = chunk_index(install);
    let mut maps: Vec<u64> = chunks.keys().copied().collect();
    maps.sort_unstable();
    let mut derived = Map::new();
    for map in maps {
        let data = map_chunks(install, &chunks, map);
        // A map whose rooms don't fit keeps the reason instead: the server
        // then counts nobody in its rooms.
        let entry = match spawn_rooms(&data) {
            Ok(rooms) if rooms.is_empty() => continue,
            Ok(rooms) => json!({"spawn_rooms": rooms}),
            Err(error) => json!({"error": error.to_string()}),
        };
        derived.insert(guid_hex(guid_of(map, MAP)), entry);
    }
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "maps": derived}),
    )])
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
