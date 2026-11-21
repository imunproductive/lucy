//! The volumes and server placeables build 104319 derives per map
//! (`game_map_areas_174.json`).

use std::collections::BTreeMap;

/// `0FA8`'s team variable v1777 (`0x06F1`) in a placeable's overrides.
const WALL_TEAM: &str = "0x0D800000000006F1";

/// The team walls (server placeables running `0FA8`) are definitions `1C90`
/// and `1C91` only, 263 and 264 of them on 24 map records (13 each on
/// Watchpoint: Gibraltar), each with its model's one mesh, a box of 12
/// triangles: `1C90`'s stops the team 0 mover alone (mask 0x88, team proxy
/// 0x08) and `1C91`'s team 1's (0x90, 0x10), and a wall's `0FA8` override
/// of v1777, where it has one, names that same team. Upstream's
/// `ROOM_BARRIER_TEAMS` (leafall903/ORPHEUS `tools/extract_game_maps.py`)
/// gives the definitions the same teams.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn team_walls_stop_the_team_their_definition_and_script_name() {
    let table = orpheus_testkit::runtime_json("data/game_map_areas_174.json");
    let mut walls: BTreeMap<&str, usize> = BTreeMap::new();
    let mut maps = 0;
    for (key, map) in table["maps"].as_object().unwrap() {
        let mut on_map = BTreeMap::new();
        for placeable in map["server_placeables"].as_array().into_iter().flatten() {
            let definition = placeable["definition"].as_str().unwrap();
            let runs_wall_graph = ["overrides", "extra"].iter().any(|list| {
                placeable["script"][list]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|binding| binding["graph"] == 0x0FA8)
            });
            let Some(collision) = placeable["collision"].as_array() else {
                assert!(!runs_wall_graph, "{key}: {definition} without collision");
                continue;
            };
            assert!(runs_wall_graph, "{key}: {definition}");
            let (team, mask) = match definition {
                "0x0400000000001C90" => (0, 0x88),
                "0x0400000000001C91" => (1, 0x90),
                _ => panic!("{key}: team wall {definition}"),
            };
            *on_map.entry(definition).or_insert(0) += 1;
            let [mesh] = &collision[..] else {
                panic!("{key}: {definition}: {} shapes", collision.len());
            };
            assert_eq!(mesh["mask"], mask);
            let triangles = mesh["triangles"].as_array().unwrap();
            assert_eq!(triangles.len(), 12);
            assert!(triangles.iter().all(|triangle| triangle[6] == mask));
            assert!(placeable["rotation"].is_array() && placeable["scale"].is_array());
            for binding in placeable["script"]["overrides"].as_array().unwrap() {
                for entry in binding["m_1EB5A024"].as_array().into_iter().flatten() {
                    if entry["m_0D09D2D9"] == WALL_TEAM {
                        assert_eq!(entry["m_value"]["m_6CE7A900"], team, "{key}: {definition}");
                    }
                }
            }
        }
        if key == "0x0800000000000184" {
            assert_eq!(on_map.values().collect::<Vec<_>>(), [&13, &13]);
        }
        maps += usize::from(!on_map.is_empty());
        for (definition, count) in on_map {
            *walls.entry(definition).or_insert(0) += count;
        }
    }
    assert_eq!(maps, 24);
    assert_eq!(
        walls,
        BTreeMap::from([("0x0400000000001C90", 263), ("0x0400000000001C91", 264)])
    );
}
