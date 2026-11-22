use super::*;
use orpheus_content::maps::GameMap;

fn level(y: f64, size: f64, mask: u16) -> SceneTriangle {
    SceneTriangle {
        vertices: [[0.0, y, 0.0], [size, y, 0.0], [0.0, y, size]],
        mask,
        surface: 0,
    }
}

fn room(team: Option<u8>, centre: [f64; 3]) -> SpawnRoom {
    SpawnRoom {
        placeable: 1,
        team,
        boxes: vec![CaptureBox {
            centre,
            half: [5.0, 5.0, 5.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }],
    }
}

/// The highest large, level player-clip floor below the spawns is the
/// catch floor; small ledges, other masks, sloped faces and floors above
/// the spawns don't count.
#[test]
fn the_catch_floor_is_the_highest_large_level_clip_floor_below_the_spawns() {
    let mut sloped = level(-3.0, 40.0, WORLD_CLIP);
    sloped.vertices[1][1] = -2.0;
    let triangles = [
        level(-19.8, 200.0, WORLD_CLIP),
        level(-11.0, 30.0, WORLD_CLIP),
        level(-4.0, 5.0, WORLD_CLIP),
        level(-2.0, 40.0, 0x01),
        level(3.0, 40.0, WORLD_CLIP),
        sloped,
    ];
    assert_eq!(kill_height(&triangles, 0.0), Some(-11.0));
    assert_eq!(kill_height(&triangles[2..], 0.0), None);
    // The centre of a standing 2.0 m hero with a 0.5 m pogo: 1.25 m up.
    assert!(caught(-11.0, -9.70, 1.25));
    assert!(caught(-11.0, -10.75, 1.25));
    assert!(!caught(-11.0, -9.60, 1.25));
}

/// A room the map gives the team counts for it alone; a room without a
/// team belongs to the team whose current spawn point stands in it; a team
/// with no room has none to gate.
#[test]
fn spawn_rooms_belong_to_their_team_or_hold_its_spawn_point() {
    let map = GameMap::practice_range();
    let (spawn, _) = map.spawn_point_in(0, 0, SpawnChoice::First).unwrap();
    let away = [spawn[0] + 50.0, spawn[1], spawn[2]];
    let given = [room(Some(0), away)];
    assert_eq!(
        in_spawn_room(&given, &map, 0, SpawnChoice::First, away),
        Some(true)
    );
    assert_eq!(
        in_spawn_room(&given, &map, 0, SpawnChoice::First, spawn),
        Some(false)
    );
    assert_eq!(
        in_spawn_room(&given, &map, 1, SpawnChoice::First, away),
        None
    );
    let open = [room(None, spawn)];
    assert_eq!(
        in_spawn_room(&open, &map, 0, SpawnChoice::First, spawn),
        Some(true)
    );
    assert_eq!(
        in_spawn_room(&open, &map, 0, SpawnChoice::First, away),
        Some(false)
    );
}

/// The derived file: a map's rooms, damage areas, door barriers, the
/// placeables' models, team walls and kill volumes, and a map whose rooms'
/// derivation failed has no rooms. Damage areas of 100 and more kill, an
/// area of 0 doesn't; an area or server placeable with an identifier exists
/// only in the modes that list it; a placeable's model stands where the
/// map turns, scales and puts it, a team wall (graph `0FA8`) among them;
/// kill volumes exist in every mode.
#[test]
fn the_area_file_reads_rooms_damage_areas_placeables_kill_volumes_and_failures() {
    let data = MapAreaData::read(
        br#"{"maps": {
            "0x0800000000000001": {"spawn_rooms": [{"placeable": 126, "team": 0,
                "boxes": [{"centre": [0, 0, 0], "half": [1, 1, 1], "rotation": [0, 0, 0, 1]}]}],
                "damage_areas": [
                    {"placeable": 700, "damage": 100.0,
                     "boxes": [{"centre": [0, -20, 0], "half": [10, 2, 10], "rotation": [0, 0, 0, 1]}]},
                    {"placeable": 701, "damage": 0.0,
                     "boxes": [{"centre": [0, 0, 0], "half": [1, 1, 1], "rotation": [0, 0, 0, 1]}]},
                    {"placeable": 702, "identifier": "0x0D80000000002962", "damage": 9999.0,
                     "boxes": [{"centre": [0, -40, 0], "half": [1, 1, 1], "rotation": [0, 0, 0, 1]}]}],
                "server_placeables": [{"placeable": 340, "definition": "0x0400000000001307",
                    "position": [1.5, 2, -3], "rotation": [0, 0, 0, 1], "scale": [2, 2, 2], "moving": true,
                    "script": {"overrides": [{"graph": 246,
                    "m_1EB5A024": [], "m_75408640": 0}], "extra": []}},
                    {"placeable": 341, "definition": "0x0400000000001307", "identifier": "0x0D80000000002962",
                     "position": [5, 0, 5], "rotation": [0, 0, 0, 1], "scale": [3, 3, 3], "posed": true},
                    {"placeable": 342, "definition": "0x0400000000001C90", "identifier": "0x0D80000000002962",
                     "position": [10, 0, 0], "rotation": [0, 0.7071068, 0, 0.7071068], "scale": [2, 1, 1],
                     "script": {"overrides": [{"graph": 4008, "m_1EB5A024": [], "m_75408640": 0}]}}],
                "kill_volumes": [{"centre": [10, -30, 0], "half": [20, 2, 5],
                    "rotation": [0, 0.7071068, 0, 0.7071068]}]},
            "0x0800000000000002": {"error": "a room's area is not a box"}
        },
        "models": {
            "0x0400000000001C90": [{"kind": "mesh", "category": 1, "mask": 136,
                "vertices": [[0, 0, 0], [1, 0, 0], [0, 2, 0]],
                "triangles": [[0, 1, 2, -1, -1, -1, 136, 0]]}],
            "0x0400000000001307": [{"kind": "capsule", "category": 128, "mask": 24,
                "a": [0, 0, 0], "b": [0, 1, 0], "radius": 0.25},
                {"kind": "sphere", "category": 4096, "mask": 24, "center": [0, 1, 0], "radius": 0.5}]
        }}"#,
    )
    .unwrap();
    let mut map = GameMap::practice_range();
    map.map_guid = 0x0800_0000_0000_0001;
    let rooms = data.spawn_rooms(&map);
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].team, Some(0));
    assert!(rooms[0].contains([0.5, 0.5, 0.5]));
    let none = BTreeSet::new();
    let listed = BTreeSet::from([0x0D80_0000_0000_2962]);
    let lethal = data.lethal_areas(&map, &none);
    assert_eq!(lethal.len(), 1);
    assert_eq!(lethal[0].placeable, 700);
    assert!(lethal[0].contains([5.0, -21.0, -5.0]));
    assert!(!lethal[0].contains([5.0, -17.0, -5.0]));
    let placeables: Vec<u32> = data
        .lethal_areas(&map, &listed)
        .iter()
        .map(|area| area.placeable)
        .collect();
    assert_eq!(placeables, [700, 702]);
    let placed = |identifiers| -> Vec<u32> {
        data.placed_scripts(&map, identifiers)
            .iter()
            .map(|placed| placed.placeable)
            .collect()
    };
    assert_eq!(placed(&none), [340]);
    assert_eq!(placed(&listed), [340, 341, 342]);
    let bodies = |identifiers| -> Vec<(u32, bool)> {
        data.placeable_bodies(&map, identifiers)
            .iter()
            .map(|body| (body.entity(), body.team_wall))
            .collect()
    };
    assert_eq!(bodies(&none), [(0x8000_0154, false)]);
    assert_eq!(
        bodies(&listed),
        [
            (0x8000_0154, false),
            (0x8000_0155, false),
            (0x8000_0156, true)
        ]
    );
    let listed_bodies = data.placeable_bodies(&map, &listed);
    // The idle-animated door's capsule, 0.5 m round and 2 m long once
    // scaled, stands upright on its position; its animated sphere
    // (category 0x1000) follows the animation, which the server doesn't
    // play, so it is left out.
    let shapes = listed_bodies[0].shapes().unwrap();
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].geometry().mask(), 24);
    let bounds = shapes[0].fat_bounds().unwrap();
    assert!((bounds.low[0] - 1.0).abs() < 0.2 && (bounds.high[0] - 2.0).abs() < 0.2);
    assert!((bounds.low[1] - 1.5).abs() < 0.2 && (bounds.high[1] - 4.5).abs() < 0.2);
    // The door posed by its animation component keeps both shapes at the
    // world origin, unscaled.
    let shapes = listed_bodies[1].shapes().unwrap();
    assert_eq!(shapes.len(), 2);
    let bounds = shapes[0].fat_bounds().unwrap();
    assert!((bounds.low[0] - -0.25).abs() < 0.2 && (bounds.high[0] - 0.25).abs() < 0.2);
    assert!((bounds.low[1] - -0.25).abs() < 0.2 && (bounds.high[1] - 1.25).abs() < 0.2);
    let bounds = shapes[1].fat_bounds().unwrap();
    assert!((bounds.low[0] - -0.5).abs() < 0.2 && (bounds.high[0] - 0.5).abs() < 0.2);
    assert!((bounds.low[1] - 0.5).abs() < 0.2 && (bounds.high[1] - 1.5).abs() < 0.2);
    let shapes = listed_bodies[2].shapes().unwrap();
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].geometry().mask(), 0x88);
    // Its 1 m edge along x, scaled to 2 m and turned a quarter about y,
    // runs from x 10 along -z.
    let bounds = shapes[0].fat_bounds().unwrap();
    assert!((bounds.low[2] - -2.0).abs() < 0.2 && (bounds.high[2] - 0.0).abs() < 0.2);
    assert!((bounds.low[0] - 10.0).abs() < 0.2 && (bounds.high[0] - 10.0).abs() < 0.2);
    assert!((bounds.high[1] - 2.0).abs() < 0.2);
    let door = &data.placed_scripts(&map, &none)[0];
    assert_eq!(door.entity(), 0x8000_0154);
    assert_eq!(door.definition, 0x0400_0000_0000_1307);
    assert_eq!(door.position, [1.5, 2.0, -3.0]);
    assert_eq!(door.overrides[0].graph, Some(246));
    // Turned a quarter about y: its 20 m half length runs along z.
    let volumes = data.kill_volumes(&map);
    assert_eq!(volumes.len(), 1);
    assert!(volumes[0].contains([14.0, -31.0, 15.0]));
    assert!(!volumes[0].contains([25.0, -30.0, 0.0]));
    assert!(!volumes[0].contains([10.0, -27.0, 0.0]));
    map.map_guid = 0x0800_0000_0000_0002;
    assert!(data.spawn_rooms(&map).is_empty());
    assert!(data.lethal_areas(&map, &listed).is_empty());
    assert!(data.kill_volumes(&map).is_empty());
}

/// The tagged entities: an entity with an identifier is found only in the
/// modes that list it, each as `0x80000000 | flat` with its tags.
#[test]
fn the_tag_queries_find_the_maps_tagged_entities_of_the_mode() {
    let data = MapAreaData::read(
        br#"{"maps": {"0x0800000000000001": {"tagged": [
            {"placeable": 126, "position": [1, 2, 3], "rotation": [0, 0, 0, 1],
             "tags": ["0x0D6000000000030F", "0x0D60000000000310"]},
            {"placeable": 127, "identifier": "0x0D80000000002962", "position": [0, 0, 0],
             "rotation": [0, 0, 0, 1], "tags": ["0x0D60000000000310"]}
        ]}}}"#,
    )
    .unwrap();
    let graphs =
        GraphStore::from_json(br#"{"version": 1, "graphs": [], "bodies": {}, "missing": []}"#)
            .unwrap();
    let mut map = GameMap::practice_range();
    map.map_guid = 0x0800_0000_0000_0001;
    let none = BTreeSet::new();
    let listed = BTreeSet::from([0x0D80_0000_0000_2962]);
    let tags = data.map_tags(&map, &none, &graphs);
    assert_eq!(
        tags.with_tag(0x0D60_0000_0000_0310),
        Some(vec![0x8000_007E])
    );
    assert_eq!(
        tags.get(0x8000_007E).map(|entity| entity.position),
        Some([1.0, 2.0, 3.0])
    );
    let tags = data.map_tags(&map, &listed, &graphs);
    assert_eq!(
        tags.with_tag(0x0D60_0000_0000_0310),
        Some(vec![0x8000_007E, 0x8000_007F])
    );
    assert_eq!(tags.with_tag(0x0D60_0000_0000_0311), Some(Vec::new()));
    map.map_guid = 0x0800_0000_0000_0002;
    assert_eq!(
        data.map_tags(&map, &listed, &graphs)
            .with_tag(0x0D60_0000_0000_0310),
        Some(Vec::new())
    );
}
