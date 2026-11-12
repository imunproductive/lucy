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
//! A kill volume is a box of the map's collision (chunk 0x11, see
//! `crate::collision`) whose triangles have mask `0x0002` and nothing else:
//! the query category `0x02`, which also meets every player clip (`0x1a`),
//! and no other. Every one is a box of 12 triangles, most of them 4 m thick
//! slabs under a map's drops (Watchpoint: Gibraltar three, tops -11.72,
//! -14.26 and -17.92 m; Dorado one, top -7.97 m; Route 66 two, tops -6.80
//! and -17.49 m; one under each part of Lijiang Tower, Nepal and Ilios); the
//! Practice Range, Hollywood and Ecopoint: Antarctica have none. They are
//! written as boxes like the areas': world centre, half extents along the
//! box's own axes, rotation `{x, y, z, w}` from those axes to the world's.
//!
//! A spawn door barrier is a server-owned entity placeable (client id 0)
//! running graph `00F6`: on the Practice Range six of them stand in the
//! three doorways of the spawn room (definitions `1307` and `130C`, a model
//! with the contact set `0092.049`, and `1225`, a model with the child
//! `12F4`'s effect `178C.00D`), each with `00F6` overrides `0877` = 0 and
//! `0878` = 1. The client builds a server-owned placeable only when the
//! server creates it (`crate::game_maps`), so without the create the doorway
//! is bare. Strong guess: these are the doorways' glowing barrier frames.

use std::{collections::HashMap, path::Path};

use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    collision::mask_triangles,
    game_maps::{
        Placed, TARGET_AREA, TARGET_TAG_DATA, chunk_index, graph_vars, instance, map_chunks,
        read_map, target_tags,
    },
    guid_hex,
    map_chunk::{COLLISION, area_boxes, chunk_kind, guid_of, name_index, short},
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
/// The collision mask of a kill volume: query category `0x02` alone.
const KILL_VOLUME_MASK: u16 = 0x0002;
/// A kill volume's corners and triangles: a box.
const BOX_CORNERS: usize = 8;
const BOX_TRIANGLES: usize = 12;

const SOURCE: &str = "Build 1.74.0.0.104319: per map (keys as game_maps_174.json), its spawn rooms: \
every entity placeable running graph 0D62 (placeable = its flat index), its team (the placeable's 0D62 \
override of v1777, STU_BD6CC023 TeamIndex: 0 blue, 1 red, -1 none), its target tags \
(STUTargetTagInstanceData.m_targetTags) and the boxes of the area placeable its \
STUTargetTagInstanceData.m_2746D7E4 names (chunk 0x0A: centre, half extents, rotation x y z w); \
its damage areas: every entity placeable with STUECDamageAreaInstanceData whose m_2746D7E4 names a box \
area of the map, its first identifier (a placeable with one exists only in the modes that list it), \
its damage (m_B81F7395) and its boxes; its door barriers: every entity placeable with client id 0 \
running graph 00F6, with its first identifier; its kill volumes: the boxes the triangles of the \
collision chunk (0x11) with mask 0x0002 alone make (12 triangles each), as centre, half extents \
and rotation x y z w.";

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
        let kill_volumes = kill_volumes(&data)
            .map_err(|error| asset_error(format!("map {map:X}: {error}")))?;
        if !kill_volumes.is_empty() {
            entry.insert("kill_volumes".into(), json!(kill_volumes));
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

/// The kill volumes of a map's collision chunk, in chunk order: each set of
/// triangles with mask [`KILL_VOLUME_MASK`] that share corners is a box.
fn kill_volumes(chunks: &[(u64, Vec<u8>)]) -> Result<Vec<Json>> {
    let Some((_, data)) = chunks
        .iter()
        .find(|(guid, _)| chunk_kind(*guid) == COLLISION)
    else {
        return Ok(Vec::new());
    };
    let triangles = mask_triangles(data, KILL_VOLUME_MASK).map_err(asset_error)?;
    // Each triangle joins the set of the first triangle that has one of its
    // corners; the sets it links merge into that one.
    let mut set_of_corner: HashMap<[u32; 3], usize> = HashMap::new();
    let mut sets: Vec<Vec<[[f32; 3]; 3]>> = Vec::new();
    for triangle in triangles {
        let keys = triangle.map(|corner| corner.map(f32::to_bits));
        let mut found: Vec<usize> = keys
            .iter()
            .filter_map(|key| set_of_corner.get(key).copied())
            .collect();
        found.sort_unstable();
        found.dedup();
        let set = match found.split_first() {
            Some((&first, others)) => {
                for &other in others {
                    let moved = std::mem::take(&mut sets[other]);
                    for corner in moved.iter().flatten() {
                        set_of_corner.insert(corner.map(f32::to_bits), first);
                    }
                    sets[first].extend(moved);
                }
                first
            }
            None => {
                sets.push(Vec::new());
                sets.len() - 1
            }
        };
        for key in keys {
            set_of_corner.insert(key, set);
        }
        sets[set].push(triangle);
    }
    sets.iter()
        .filter(|set| !set.is_empty())
        .map(|set| box_of(set))
        .collect()
}

/// The box 12 triangles make: its centre, half extents along its faces'
/// normals and the rotation from those axes to the world's.
fn box_of(triangles: &[[[f32; 3]; 3]]) -> Result<Json> {
    let mut corners: Vec<[f64; 3]> = Vec::new();
    for corner in triangles.iter().flatten() {
        let corner = corner.map(f64::from);
        if !corners.contains(&corner) {
            corners.push(corner);
        }
    }
    let not_a_box = || {
        asset_error(format!(
            "a kill volume of {} triangles and {} corners is not a box",
            triangles.len(),
            corners.len()
        ))
    };
    if triangles.len() != BOX_TRIANGLES || corners.len() != BOX_CORNERS {
        return Err(not_a_box());
    }
    let normals: Vec<[f64; 3]> = triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|corner| corner.map(f64::from));
            unit(cross(sub(b, a), sub(c, a)))
        })
        .collect();
    // The box's axes: for each world axis the face normal nearest to it,
    // pointing along it.
    let mut axes: [[f64; 3]; 3] = std::array::from_fn(|world| {
        let nearest = normals
            .iter()
            .copied()
            .max_by(|a, b| a[world].abs().total_cmp(&b[world].abs()))
            .expect("12 normals");
        nearest.map(|part| part * nearest[world].signum())
    });
    let parallel = |a: [f64; 3], b: [f64; 3]| dot(a, b).abs() > 1.0 - 1e-4;
    let square = (0..3).all(|i| (i + 1..3).all(|j| dot(axes[i], axes[j]).abs() < 1e-4));
    if !square
        || !normals
            .iter()
            .all(|normal| axes.iter().any(|axis| parallel(*normal, *axis)))
    {
        return Err(not_a_box());
    }
    // A rotation, not a mirror.
    axes[2] = cross(axes[0], axes[1]);
    let centre: [f64; 3] = std::array::from_fn(|axis| {
        corners.iter().map(|corner| corner[axis]).sum::<f64>() / BOX_CORNERS as f64
    });
    let half = axes.map(|axis| {
        corners
            .iter()
            .map(|corner| dot(sub(*corner, centre), axis).abs())
            .fold(0.0, f64::max)
    });
    let float = |value: f64| short(value as f32);
    Ok(json!({
        "centre": centre.map(float),
        "half": half.map(float),
        "rotation": rotation(axes).map(float),
    }))
}

/// The unit quaternion `{x, y, z, w}` of the rotation whose matrix has
/// `columns` (an orthonormal, right-handed basis).
fn rotation(columns: [[f64; 3]; 3]) -> [f64; 4] {
    let m = |row: usize, column: usize| columns[column][row];
    let trace = m(0, 0) + m(1, 1) + m(2, 2);
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (m(2, 1) - m(1, 2)) / s,
            (m(0, 2) - m(2, 0)) / s,
            (m(1, 0) - m(0, 1)) / s,
            s / 4.0,
        ]
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
        [
            s / 4.0,
            (m(0, 1) + m(1, 0)) / s,
            (m(0, 2) + m(2, 0)) / s,
            (m(2, 1) - m(1, 2)) / s,
        ]
    } else if m(1, 1) > m(2, 2) {
        let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
        [
            (m(0, 1) + m(1, 0)) / s,
            s / 4.0,
            (m(1, 2) + m(2, 1)) / s,
            (m(0, 2) - m(2, 0)) / s,
        ]
    } else {
        let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
        [
            (m(0, 2) + m(2, 0)) / s,
            (m(1, 2) + m(2, 1)) / s,
            s / 4.0,
            (m(1, 0) - m(0, 1)) / s,
        ]
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|axis| a[axis] * b[axis]).sum()
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    v.map(|part| part / length)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The 12 triangles of a box turned `turn` radians about y.
    fn turned_box(centre: [f64; 3], half: [f64; 3], turn: f64) -> Vec<[[f32; 3]; 3]> {
        let (sin, cos) = turn.sin_cos();
        let corner = |signs: [f64; 3]| -> [f32; 3] {
            let [x, y, z] = std::array::from_fn(|axis| signs[axis] * half[axis]);
            [
                (centre[0] + cos * x + sin * z) as f32,
                (centre[1] + y) as f32,
                (centre[2] - sin * x + cos * z) as f32,
            ]
        };
        let mut triangles = Vec::new();
        for axis in 0..3 {
            for side in [-1.0, 1.0] {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let at = |a: f64, b: f64| {
                    let mut signs = [0.0; 3];
                    signs[axis] = side;
                    signs[u] = a;
                    signs[v] = b;
                    corner(signs)
                };
                triangles.push([at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0)]);
                triangles.push([at(-1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)]);
            }
        }
        triangles
    }

    fn numbers(value: &Json) -> Vec<f64> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|number| number.as_f64().unwrap())
            .collect()
    }

    /// A slab turned 30° about y (as Gibraltar's are turned 45°) comes back
    /// as its centre, its half extents along its own axes and the quarter
    /// turn's quaternion.
    #[test]
    fn a_kill_volume_is_the_box_its_triangles_make() {
        let slab = box_of(&turned_box([100.0, -15.0, 50.0], [40.0, 2.0, 20.0], 30f64.to_radians()))
            .unwrap();
        let close = |found: Vec<f64>, wanted: [f64; 3]| {
            found
                .iter()
                .zip(wanted)
                .all(|(found, wanted)| (found - wanted).abs() < 1e-4)
        };
        assert!(close(numbers(&slab["centre"]), [100.0, -15.0, 50.0]), "{slab}");
        assert!(close(numbers(&slab["half"]), [40.0, 2.0, 20.0]), "{slab}");
        let rotation = numbers(&slab["rotation"]);
        let half_turn = 15f64.to_radians();
        assert!(close(rotation[..3].to_vec(), [0.0, half_turn.sin(), 0.0]), "{slab}");
        assert!((rotation[3] - half_turn.cos()).abs() < 1e-4, "{slab}");
    }

    /// Triangles that make no box are refused, not guessed at.
    #[test]
    fn a_kill_volume_that_is_not_a_box_is_refused() {
        let mut slab = turned_box([0.0; 3], [1.0, 1.0, 1.0], 0.0);
        slab.pop();
        assert!(box_of(&slab).is_err());
    }
}
