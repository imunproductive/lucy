//! Assault on its mode graph `0F4A` on every map a queue card plays it on.

use std::collections::BTreeSet;

use lucy_content::maps::MapCatalog;
use lucy_lobby::matchmaker::{load_rules, map_choices};
use lucy_sim::{match_flow::ASSAULT, mode_script::ModeRunner, scene_assets::SceneAssets};

/// The room's start of a match (`Room::begin_mode`): the placeables' first
/// tick, then the mode graph with their reports. `0F4A` starts whole and
/// drives the mode entity for Assault on every map and ruleset a queue card
/// offers it with (Quick Play, Competitive, Practice vs AI and the others
/// in `game_queues_174.json`), and still does once the match starts
/// (`0789`) and runs two frames.
#[test]
#[ignore = "requires the derived 104319 data"]
fn assault_runs_on_its_mode_graph_on_every_map_a_queue_offers() {
    const FIRST_TICK: u32 = 1;
    let runtime = lucy_testkit::runtime_dir();
    let data = runtime.join("data");
    let catalog = MapCatalog::load(&data).unwrap();
    let queues = load_rules(&data.join("game_queues_174.json")).unwrap();
    // The maps no other test plays have no collision in the pack yet: they
    // build it from the install, as the server does.
    let assets = SceneAssets::load(&runtime).unwrap().with_collision_build(
        crate::game_service::collision_build(lucy_testkit::game_dir()),
    );
    let mut seen = BTreeSet::new();
    let mut played = Vec::new();
    let mut gaps = Vec::new();
    for rules in queues.values() {
        for (map, _) in map_choices(&catalog, &rules.maps, &rules.rulesets) {
            if map.mode_guid != ASSAULT || !seen.insert((map.map_guid, map.ruleset)) {
                continue;
            }
            let mut scene = assets
                .prepare(map.clone())
                .unwrap_or_else(|error| panic!("{} {:X}: {error}", map.name, map.map_guid));
            scene.advance_placed(FIRST_TICK, &[]);
            let mut mode = ModeRunner::start(&scene, FIRST_TICK, Vec::new());
            if mode.holds_rooms() {
                mode.start_match();
                for tick in FIRST_TICK + 1..FIRST_TICK + 3 {
                    mode.advance(tick, &Default::default(), Vec::new());
                }
            }
            played.push((map.name.clone(), map.ruleset));
            if !mode.holds_rooms() {
                gaps.push((map.name, map.ruleset, mode.gaps().cloned()));
            }
        }
    }
    let names: BTreeSet<&str> = played.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "Hanamura",
            "Horizon Lunar Colony",
            "Paris",
            "Temple of Anubis",
            "Volskaya Industries"
        ]),
        "{played:?}"
    );
    // Quick Play `002A`, Competitive `002B`, Practice vs AI and Quick Play
    // Classic `00E8`, Competitive Open Queue `010B`, and the Arcade's.
    let rulesets: BTreeSet<u64> = played.iter().map(|&(_, ruleset)| ruleset).collect();
    assert_eq!(
        rulesets,
        BTreeSet::from([
            0x0fd0_0000_0000_002a,
            0x0fd0_0000_0000_002b,
            0x0fd0_0000_0000_003e,
            0x0fd0_0000_0000_0040,
            0x0fd0_0000_0000_006b,
            0x0fd0_0000_0000_00ca,
            0x0fd0_0000_0000_00e4,
            0x0fd0_0000_0000_00e8,
            0x0fd0_0000_0000_0107,
            0x0fd0_0000_0000_010b,
            0x0fd0_0000_0000_0124,
        ])
    );
    assert_eq!(played.len(), 38, "{played:?}");
    assert!(gaps.is_empty(), "{gaps:#?}");
}
