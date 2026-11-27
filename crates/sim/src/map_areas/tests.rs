use super::*;
use lucy_content::maps::GameMap;

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
                    "idle": {"start": 15, "rate": 2.0},
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
                {"kind": "sphere", "category": 4096, "mask": 24, "center": [0, 1, 0], "radius": 0.5,
                 "bone": 1}]
        },
        "idle_animations": {"0x0400000000001307": {"frames": 30, "bones": [
            {"id": 1, "parent": null, "rotation": [0, 0, 0, 1], "scale": [1, 1, 1], "translation": [0, 0, 0]},
            {"id": 2, "parent": 0, "rotation": [0, 0, 0, 1], "scale": [1, 1, 1], "translation": [0, 1, 0]}],
            "clip": {"additive": true, "duration": 1.0, "fps": 30, "frames": 31, "tracks": [
                {"bone": 1, "positions": [[0, 0, 0, 0], [30, 0, 3, 0]]}]}}
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
    // (category 0x1000) is a moving shape on bone 1 of its idle rig.
    let shapes = listed_bodies[0].shapes().unwrap();
    assert_eq!(shapes.len(), 2);
    assert_eq!(shapes[0].geometry().mask(), 24);
    assert!(!shapes[0].is_moving() && shapes[1].is_moving());
    let bounds = shapes[0].fat_bounds().unwrap();
    assert!((bounds.low[0] - 1.0).abs() < 0.2 && (bounds.high[0] - 2.0).abs() < 0.2);
    assert!((bounds.low[1] - 1.5).abs() < 0.2 && (bounds.high[1] - 4.5).abs() < 0.2);
    // It starts 15 frames in at twice the rate: at 0.125 s, (0.125 + 0.5)
    // s · 30 · 2 = 37.5 frames, 7.5 into its 30-frame loop, a quarter of
    // the 1 s clip, whose delta lifts bone 1 by a quarter of 3 m: the
    // sphere rises 0.75 m, 1.5 m at the placeable's scale 2.
    let idle = listed_bodies[0].moving().unwrap();
    let [(1, frame)] = idle.frames(0.125, 0.125)[..] else {
        panic!("one part, the sphere");
    };
    let expected = [1.5, 3.5, -3.0];
    for (axis, value) in frame.translation.iter().enumerate() {
        assert!((value - expected[axis]).abs() < 1e-5, "{frame:?}");
    }
    assert_eq!(frame.rotation, [0.0, 0.0, 0.0, 1.0]);
    // A whole loop later (0.5 s at twice the rate) it is there again.
    let [(1, again)] = idle.frames(0.625, 0.625)[..] else {
        panic!("one part, the sphere");
    };
    for (axis, value) in again.translation.iter().enumerate() {
        assert!((value - expected[axis]).abs() < 1e-5, "{again:?}");
    }
    // The door posed by its animation component keeps both shapes at the
    // world origin, unscaled, its sphere too (no idle animation).
    assert!(listed_bodies[1].moving().is_none());
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

/// The Practice Range's four platforms (`0516`, component 98) walk their
/// path where the client walks them: in the client's physics world
/// (session `D:/Temp/mm/p1`, dump 0, 2026-11-23) all four polytopes stood,
/// to 0.2 mm across the floor, where the path puts each at one time of
/// the path clock, 82.759 s (each 1/4 lap apart: offsets 0.75, 0.99, 0.25,
/// 0.5 of a 143 m loop at 4 m/s). The height and turn differ by
/// millimetres: they bob on the idle clock, which the dump didn't time.
#[test]
#[ignore = "requires the derived 104319 data"]
fn the_range_platforms_walk_their_path_where_the_clients_physics_world_had_them() {
    let data = MapAreaData::load(&lucy_testkit::runtime_dir().join("data")).unwrap();
    let bodies = data.placeable_bodies(&GameMap::practice_range(), &BTreeSet::new());
    let client = [
        (0x81, [50.657_726, -7.009_680_3]),
        (0xC2, [79.549_58, 9.312_379]),
        (0x10F, [86.566_52, -20.054_947]),
        (0x193, [55.161_19, -34.559_75]),
    ];
    for (placeable, [x, z]) in client {
        let body = bodies
            .iter()
            .find(|body| body.placeable.placeable == placeable)
            .unwrap();
        let [(_, frame)] = body.moving().unwrap().frames(82.759, 82.759)[..] else {
            panic!("one polytope");
        };
        let [px, _, pz] = frame.translation;
        assert!(
            (px - x).abs() < 3e-4 && (pz - z).abs() < 3e-4,
            "{placeable:X}: {frame:?}"
        );
    }
}

/// A placeable whose transform comes from a movement state (Volskaya's lift
/// `3CD8`, component 28 by component 34) stands where its create puts it:
/// on the 1/1024 m grid, turned by its yaw word, unscaled; a turn not about
/// y alone is refused.
#[test]
fn a_state_placed_placeable_stands_where_its_create_puts_it() {
    let lift = StatePlace::new(
        [-64.685_005, -1.961_539_3, 53.190_575],
        [0.0, 0.269_654_36, 0.0, -0.962_957_14],
    )
    .unwrap();
    assert_eq!(
        lift,
        StatePlace {
            position: [-64.684_570_312_5, -1.961_914_062_5, 53.190_429_687_5],
            yaw: -5696,
        }
    );
    assert!(StatePlace::new([0.0; 3], [0.1, 0.0, 0.0, 0.995]).is_none());
    let data = MapAreaData::read(
        br#"{"maps": {
            "0x0800000000000001": {"server_placeables": [{"placeable": 341,
                "definition": "0x0400000000001307", "position": [5.0001, 0, 5],
                "rotation": [0, 0.7071068, 0, 0.7071068], "scale": [3, 3, 3], "state_placed": true}]}
        },
        "models": {
            "0x0400000000001307": [{"kind": "capsule", "category": 1, "mask": 24,
                "a": [0, 0, 0], "b": [1, 0, 0], "radius": 0.25}]
        }}"#,
    )
    .unwrap();
    let mut map = GameMap::practice_range();
    map.map_guid = 0x0800_0000_0000_0001;
    let none = BTreeSet::new();
    let placed = &data.placed_scripts(&map, &none)[0];
    assert_eq!(
        placed.state_place,
        Some(StatePlace {
            position: [5.0, 0.0, 5.0],
            yaw: 16384,
        })
    );
    // The capsule's 1 m along x, unscaled, turned a quarter about y: it
    // runs from (5, 0, 5) along -z.
    let shapes = data.placeable_bodies(&map, &none)[0].shapes().unwrap();
    let bounds = shapes[0].fat_bounds().unwrap();
    assert!((bounds.low[0] - 4.75).abs() < 0.2 && (bounds.high[0] - 5.25).abs() < 0.2);
    assert!((bounds.low[2] - 3.75).abs() < 0.2 && (bounds.high[2] - 5.25).abs() < 0.2);
}

/// A rider sees the same grid positions the movement records carry at
/// its delayed view time; the previous frame also supplies exit velocity.
#[test]
fn a_track_platform_interpolates_its_sent_positions_at_view_time() {
    let mut motion = TrackMotion::new([1.0, 2.0, 3.0]);
    motion.record(1.0, [1.0, 4.0, 3.0]);
    motion.record(2.0, [1.0, 8.0, 3.0]);
    assert_eq!(motion.at(-1.0), [1.0, 2.0, 3.0]);
    assert_eq!(motion.at(0.5), [1.0, 3.0, 3.0]);
    assert_eq!(motion.at(1.5), [1.0, 6.0, 3.0]);
    assert_eq!(motion.at(3.0), [1.0, 8.0, 3.0]);
    for frame in 3..=300 {
        motion.record(f64::from(frame), [1.0, frame as f32, 3.0]);
    }
    assert_eq!(motion.positions.len(), TrackMotion::CAPACITY);
    assert_eq!(motion.at(299.5), [1.0, 299.5, 3.0]);
}
