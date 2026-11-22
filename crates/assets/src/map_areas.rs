//! `data/game_map_areas_174.json`: the volumes of each map that the server
//! tests a player's body against (the spawn rooms and the damage areas) and
//! the server-owned placeables the server must create after the loading
//! list.
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
//! and no other. Every one is a box of 12 triangles: 168 on 73 of the 95 map
//! records with a header. Most are 4 m thick slabs under a map's drops,
//! holding the drop's bottom (Watchpoint: Gibraltar three, tops -11.72,
//! -14.26 and -17.92 m; Dorado one, top -7.97 m; Route 66 two, tops -6.80
//! and -17.49 m; one under each part of Lijiang Tower, Nepal and Ilios).
//! King's Row also has walls round its pit east of the attackers' first
//! spawn, and Junkenstein's Revenge four 4 by 9 by 4 m boxes standing on
//! walkable floor. Temple of Anubis, the Practice Range, Hollywood and
//! Ecopoint: Antarctica have none. They are written as boxes like the
//! areas': world centre, half extents along the box's own axes, rotation
//! `{x, y, z, w}` from those axes to the world's.
//!
//! A server-owned placeable is an entity placeable with client id 0 that is
//! not on the loading list (its definition has neither component bit 58 nor
//! 94, `crate::game_maps`). The client builds it only when the server
//! creates it, so without the create it is missing: the spawn door barriers
//! (graph `00F6`), the doorway frames (`0D73`), the team walls (`0FA8`),
//! the spawn doors (`0D32`), the objective, capture point and payload
//! models, and the other scenery the map leaves to the server. Each is
//! written with its flat index, its definition and its first identifier (a
//! placeable with one exists only in the modes that list it).
//!
//! The spawn door barriers are an example: on the Practice Range six of
//! them run graph `00F6` in the three doorways of the spawn room
//! (definitions `1307` and `130C`, a model with the contact set `0092.049`,
//! and `1225`, a model with the child `12F4`'s effect `178C.00D`), each with
//! `00F6` overrides `0877` = 0 and `0878` = 1. Strong guess: these are the
//! doorways' glowing barrier frames.
//!
//! A server placeable whose definition's model has static physics shapes
//! that stop the mover (`crate::props`: the same rule as a client-made
//! prop's) is written with its rotation and scale, and the definition's
//! shapes go once into the file's `models`, in model space, each with the
//! category the client files it under (`props::Structured::static_category`)
//! and its geometry as `crate::collision` keeps a prop's: a mesh's holder
//! mask, vertices and triangles `[corners, wings, mask, surface]`, a
//! polytope's mask, centroid, vertices, planes, faces and half edges, a
//! capsule's or sphere's mask and geometry. The client builds these shapes
//! for every placeable the server creates (game check 2026-11-22, refs
//! `MapEntities.md` "MapCollision"): the Practice Range's Training Partner
//! console `3963` (one mesh, category 0x80, at its placement), its pad
//! `3992`, the moving platforms `0516` and the spawn doors `1793`/`230A`
//! (category 0x1000, animated). The server's mover takes them
//! (`orpheus_sim` `map_areas::PlaceableBody`).
//!
//! The team walls are among them: the server placeables that run graph
//! `0FA8` (definitions `1C90` and `1C91`: 263 and 264 placeables on 24 map
//! records; 237 of the `1C90` stand exactly where a `1C91` stands). Their
//! models' shapes are one mesh, a 5.28 by 5.28 by 0.35 m box of 12
//! triangles: model `09A0` (`1C90`) with mask 0x88, which stops the team 0
//! mover (category 0x08), and `099D` (`1C91`) with 0x90, team 1 (0x10);
//! upstream leafall903/ORPHEUS (MIT) `tools/extract_game_maps.py`
//! `ROOM_BARRIER_TEAMS` maps the two definitions to teams 0 and 1 the same
//! way. Each wall's `0FA8` override of v1777 (where it has one) is that
//! team too: 0 on `1C90`, 1 on `1C91`.
//!
//! The tagged entities are what the scene tag queries find
//! (`orpheus_script::map_tags`): every entity placeable the server's world
//! holds (on the loading list, or server-owned) with target tags of its
//! own (`STUTargetTagInstanceData.m_targetTags`) or of its definition
//! (`STUTargetTagComponent.m_targetTags`), both together. Spawn points,
//! spawn rooms and doors are among them.

use std::{collections::HashMap, path::Path};

use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    collision::mask_triangles,
    game_maps::{
        Components, Placed, REQUIRED, SPAWN, TARGET_AREA, TARGET_TAG_DATA, chunk_index, graph_vars,
        instance, map_chunks, placed_script, read_map, target_tags,
    },
    guid_hex,
    map_chunk::{COLLISION, areas, chunk_kind, guid_of, name_index, short},
    props::{self, Shape},
};

pub(crate) const PATH: &str = "data/game_map_areas_174.json";

/// The spawn room graph.
const SPAWN_ROOM_GRAPH: u64 = 0x0D62;
/// `0D62`'s team: its TrackTargets find this team's heroes.
const ROOM_TEAM: u64 = 1777;
/// `STUECDamageAreaInstanceData`: the area (a teUUID) at +8, the damage (a
/// float) at +24.
const DAMAGE_AREA_DATA: u32 = 0x6E7E_D1C2;
const DAMAGE_AREA: usize = 8;
const DAMAGE: usize = 24;
/// `STUMirroredIdleAnimComponent` (client component 27): the definition's
/// model plays an idle animation that moves it (the Practice Range's
/// platforms, definition `0516`).
const MIRRORED_IDLE_ANIM: u32 = 0x3AA5_CB04;
/// `STU_496257B5` (client component 39, mirror `STU_56B22CE6`): the
/// definition's model is posed by an animation state the server mirrors
/// (the spawn doors `0D32`); until it comes, the client keeps the model,
/// and its physics, at the world origin.
const POSED: u32 = 0x4962_57B5;
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
its damage (m_B81F7395) and its boxes; its server placeables: every entity placeable with client id 0 \
whose definition (003) has neither component 0x737C5C41 nor 0xCA1FB0B8 (those are on the loading \
list), with its definition, its first identifier and its second identifier (body +16), and when its \
definition's model has static physics shapes that stop the team proxies 0x18 (STUModel m_CB4D298D) \
its rotation x y z w and scale; its models: each such definition's shapes in model space with the \
category the client files each under (meshes: holder mask, vertices, triangles as corners, wings, \
mask, surface; polytopes: mask, centroid, vertices, planes, faces, half edges as twin, origin, face, \
next; capsules and spheres: mask and geometry); its tagged entities: every entity placeable on \
the loading list or with client id 0 that has target tags (STUTargetTagInstanceData.m_targetTags, then \
its definition's STUTargetTagComponent.m_targetTags, each once), with its first identifier, \
position and rotation x y z w; its kill volumes: the boxes the triangles of the \
collision chunk (0x11) with mask 0x0002 alone make (12 triangles each), as centre, half extents \
and rotation x y z w.";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let chunks = chunk_index(install);
    let mut maps: Vec<u64> = chunks.keys().copied().collect();
    maps.sort_unstable();
    let mut derived = Map::new();
    let mut components = Components::default();
    // Each server placeable definition's shapes (null without any).
    let mut models = Map::new();
    for map in maps {
        let data = map_chunks(install, &chunks, map);
        // A map whose rooms don't fit keeps the reason instead: the server
        // then counts nobody in its rooms.
        let mut entry = match spawn_rooms(&data) {
            Ok(rooms) if rooms.is_empty() => Map::new(),
            Ok(rooms) => Map::from_iter([("spawn_rooms".into(), json!(rooms))]),
            Err(error) => Map::from_iter([("error".into(), json!(error.to_string()))]),
        };
        let (damage_areas, server_placeables, tagged) = match read_map(&data) {
            Ok((base, entities)) => (
                damage_areas(&data, base, &entities)
                    .map_err(|error| asset_error(format!("map {map:X}: {error}")))?,
                server_placeables(install, &mut components, &mut models, base, &entities)
                    .map_err(|error| asset_error(format!("map {map:X}: {error}")))?,
                tagged(&components, base, &entities),
            ),
            Err(_) => (Vec::new(), Vec::new(), Vec::new()),
        };
        if !damage_areas.is_empty() {
            entry.insert("damage_areas".into(), json!(damage_areas));
        }
        if !server_placeables.is_empty() {
            entry.insert("server_placeables".into(), json!(server_placeables));
        }
        if !tagged.is_empty() {
            entry.insert("tagged".into(), json!(tagged));
        }
        let kill_volumes =
            kill_volumes(&data).map_err(|error| asset_error(format!("map {map:X}: {error}")))?;
        if !kill_volumes.is_empty() {
            entry.insert("kill_volumes".into(), json!(kill_volumes));
        }
        if !entry.is_empty() {
            derived.insert(guid_hex(guid_of(map, MAP)), Json::Object(entry));
        }
    }
    models.retain(|_, shapes| !shapes.is_null());
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "maps": derived, "models": models}),
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

/// The damage areas of a map with an area, in placeable order.
fn damage_areas(
    chunks: &[(u64, Vec<u8>)],
    base: usize,
    entities: &[Placed<'_>],
) -> Result<Vec<Json>> {
    let areas = areas(chunks)?;
    let mut out = Vec::new();
    for (number, placed) in entities.iter().enumerate() {
        let Some((stu, root)) = instance(placed, DAMAGE_AREA_DATA) else {
            continue;
        };
        let (Some(found), Some(damage)) = (
            stu.uuid(root + DAMAGE_AREA)
                .and_then(|uuid| areas.get(&uuid)),
            stu.f32(root + DAMAGE),
        ) else {
            continue;
        };
        let boxes = found.only_boxes(&format!("damage area {}", base + number))?;
        let mut area = placeable_entry(base + number, placed);
        area.insert("damage".into(), json!(short(damage)));
        area.insert("boxes".into(), json!(boxes));
        out.push(Json::Object(area));
    }
    Ok(out)
}

/// The server-owned placeables of a map that are not on the loading list,
/// in placeable order, each with its definition and its second identifier
/// when it has one (`STU_A3DC6A25` removes the placeables of one: the
/// health packs' `2AB0.01C`), and with its rotation and scale when its
/// definition's model has physics shapes that stop the mover (see the
/// module notes; `models` keeps each definition's shapes).
fn server_placeables(
    install: &Install,
    components: &mut Components,
    models: &mut Map<String, Json>,
    base: usize,
    entities: &[Placed<'_>],
) -> Result<Vec<Json>> {
    components.load(
        install,
        entities.iter().map(|placed| placed.entity.definition),
    );
    let mut out = Vec::new();
    for (number, placed) in entities.iter().enumerate() {
        let entity = &placed.entity;
        if entity.client_id != 0
            || components
                .of(entity.definition)
                .is_some_and(|found| found.contains(&SPAWN) || found.contains(&REQUIRED))
        {
            continue;
        }
        let mut entry = placeable_entry(base + number, placed);
        entry.insert("definition".into(), json!(guid_hex(entity.definition)));
        if entity.second_identifier != 0 {
            entry.insert(
                "second_identifier".into(),
                json!(guid_hex(entity.second_identifier)),
            );
        }
        entry.insert("position".into(), json!(entity.translation.map(short)));
        if placeable_shapes(install, models, entity.definition)? {
            entry.insert("rotation".into(), json!(entity.rotation.map(short)));
            entry.insert("scale".into(), json!(entity.scale.map(short)));
        }
        if let Some(script) = placed_script(&placed.instances) {
            entry.insert("script".into(), script);
        }
        let found = components.of(entity.definition);
        if found.is_some_and(|found| found.contains(&MIRRORED_IDLE_ANIM)) {
            entry.insert("moving".into(), json!(true));
        }
        if found.is_some_and(|found| found.contains(&POSED)) {
            entry.insert("posed".into(), json!(true));
        }
        out.push(Json::Object(entry));
    }
    Ok(out)
}

/// Whether a server placeable's definition has static physics shapes that
/// stop the mover, its model's shapes kept in `models` by definition the
/// first time (null when it has none; see the module notes).
fn placeable_shapes(
    install: &Install,
    models: &mut Map<String, Json>,
    definition: u64,
) -> Result<bool> {
    let key = guid_hex(definition);
    if let Some(found) = models.get(&key) {
        return Ok(!found.is_null());
    }
    let shapes = props::definition_shapes(install, definition, props::MOVER)?;
    let has = !shapes.is_empty();
    let json = if has {
        Json::from(
            shapes
                .iter()
                .map(|(category, shape)| {
                    let mut json = shape_json(shape);
                    json["category"] = json!(category);
                    json
                })
                .collect::<Vec<_>>(),
        )
    } else {
        Json::Null
    };
    models.insert(key, json);
    Ok(has)
}

/// One model-space physics shape (`crate::props`), as the server reads it
/// (`orpheus_sim` `map_areas`): a mesh's holder mask, vertices and
/// triangles `[corners, wings, mask, surface]`; a polytope's mask,
/// centroid, vertices, planes, faces and half edges `[twin, origin, face,
/// next]`; a capsule's and a sphere's mask and geometry. The caller adds
/// the shape's category.
fn shape_json(shape: &Shape) -> Json {
    let points = |points: &[[f32; 3]]| -> Vec<[f64; 3]> {
        points.iter().map(|point| point.map(short)).collect()
    };
    match shape {
        Shape::Mesh(mask, vertices, records) => json!({
            "kind": "mesh",
            "mask": mask,
            "vertices": points(vertices),
            "triangles": records
                .iter()
                .map(|(corners, mask, surface)| {
                    let mut row = corners.map(i64::from).to_vec();
                    row.extend([i64::from(*mask), i64::from(*surface)]);
                    row
                })
                .collect::<Vec<_>>(),
        }),
        Shape::Hull(hull) => json!({
            "kind": "hull",
            "mask": hull.mask,
            "centroid": hull.centroid.map(short),
            "vertices": points(&hull.points),
            "planes": hull.planes.iter().map(|plane| plane.map(short)).collect::<Vec<_>>(),
            "faces": hull.faces,
            "half_edges": hull
                .half
                .iter()
                .map(|[twin, origin, face, next]| [i64::from(*twin as i8), i64::from(*origin), i64::from(*face), i64::from(*next)])
                .collect::<Vec<_>>(),
        }),
        Shape::Capsule(mask, a, b, radius) => json!({
            "kind": "capsule",
            "mask": mask,
            "a": a.map(short),
            "b": b.map(short),
            "radius": short(*radius),
        }),
        Shape::Sphere(mask, center, radius) => json!({
            "kind": "sphere",
            "mask": mask,
            "center": center.map(short),
            "radius": short(*radius),
        }),
    }
}

/// The map's entities with target tags that the server's world holds, in
/// placeable order: those on the loading list and the server-owned ones
/// (client id 0); the client-made rest exists only in the clients. Each
/// with its first identifier, its position and rotation and the tags of its instance
/// data and its definition together (each once, instance tags first).
/// [`server_placeables`] loads the definitions first.
fn tagged(components: &Components, base: usize, entities: &[Placed<'_>]) -> Vec<Json> {
    entities
        .iter()
        .enumerate()
        .filter_map(|(number, placed)| {
            let found = components.of(placed.entity.definition);
            let listed =
                found.is_some_and(|found| found.contains(&SPAWN) || found.contains(&REQUIRED));
            if !listed && placed.entity.client_id != 0 {
                return None;
            }
            let mut tags: Vec<u64> = target_tags(&placed.instances);
            for tag in components.tags(placed.entity.definition) {
                if !tags.contains(tag) {
                    tags.push(*tag);
                }
            }
            if tags.is_empty() {
                return None;
            }
            let mut entry = placeable_entry(base + number, placed);
            entry.insert(
                "position".into(),
                json!(placed.entity.translation.map(short)),
            );
            entry.insert("rotation".into(), json!(placed.entity.rotation.map(short)));
            entry.insert(
                "tags".into(),
                json!(
                    tags.into_iter()
                        .map(|tag| guid_hex(guid_of(tag, TARGET_TAG)))
                        .collect::<Vec<_>>()
                ),
            );
            Some(Json::Object(entry))
        })
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
    let areas = areas(chunks)?;
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
            .ok_or_else(|| asset_error(format!("spawn room {flat} names a missing area")))?
            .only_boxes(&format!("spawn room {flat}"))?;
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
        let slab = box_of(&turned_box(
            [100.0, -15.0, 50.0],
            [40.0, 2.0, 20.0],
            30f64.to_radians(),
        ))
        .unwrap();
        let close = |found: Vec<f64>, wanted: [f64; 3]| {
            found
                .iter()
                .zip(wanted)
                .all(|(found, wanted)| (found - wanted).abs() < 1e-4)
        };
        assert!(
            close(numbers(&slab["centre"]), [100.0, -15.0, 50.0]),
            "{slab}"
        );
        assert!(close(numbers(&slab["half"]), [40.0, 2.0, 20.0]), "{slab}");
        let rotation = numbers(&slab["rotation"]);
        let half_turn = 15f64.to_radians();
        assert!(
            close(rotation[..3].to_vec(), [0.0, half_turn.sin(), 0.0]),
            "{slab}"
        );
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
