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

/// The derived file: a map's rooms, damage areas, door barriers and kill
/// volumes, and a map whose rooms' derivation failed has no rooms. Damage
/// areas of 100 and more kill, an area of 0 doesn't; an area or server placeable with
/// an identifier exists only in the modes that list it; kill volumes exist
/// in every mode.
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
                "server_placeables": [{"placeable": 340, "definition": "0x0400000000001307"},
                    {"placeable": 341, "definition": "0x0400000000001307", "identifier": "0x0D80000000002962"}],
                "kill_volumes": [{"centre": [10, -30, 0], "half": [20, 2, 5],
                    "rotation": [0, 0.7071068, 0, 0.7071068]}]},
            "0x0800000000000002": {"error": "a room's area is not a box"}
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
    assert_eq!(data.server_placeables(&map, &none), [340]);
    assert_eq!(data.server_placeables(&map, &listed), [340, 341]);
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
