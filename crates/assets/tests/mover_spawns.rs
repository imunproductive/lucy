//! A map's spawn rooms on the collision cache built from the install, as the
//! game service builds it for a map the pack has no cache of.

use std::{collections::BTreeSet, fs, sync::Arc};

use orpheus_assets::{Install, collision_cache};
use orpheus_physics::{
    collision::{cache::read_movement_cache, scene::CollisionScene},
    contact::world::ContactWorld,
    mover::{Mover, settings::MoverSettings},
};
use orpheus_wire::commands::Command;

const KINGS_ROW: u64 = 0x0800_0000_0000_00D4;
/// The team proxy categories (`MOVER` in `ow174/game/collision.py`).
const MOVER: u16 = 0x18;
/// In the air (`IN_AIR` in `ow174/game/mover.py`).
const IN_AIR: u32 = 0x80_000C;
const STEP: f64 = 1.0 / 1024.0;

/// `tests/test_game_mover.py::RealMapTests::test_the_spawn_rooms_of_kings_row_stand_on_the_floor`:
/// Soldier: 76 at each of the first 12 spawns stands on the floor after 20
/// still ticks.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_spawn_rooms_of_kings_row_stand_on_the_floor() {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let output = collision_cache(&install, KINGS_ROW, "King's Row").unwrap();
    let cache = read_movement_cache(output.bytes.as_slice()).unwrap();
    let identifiers = BTreeSet::new();
    let scene = CollisionScene::movement_probe(&cache, &identifiers).unwrap();
    let world = Arc::new(ContactWorld::from_movement_cache(cache, &identifiers).unwrap());
    let maps: serde_json::Value = serde_json::from_slice(
        &fs::read(orpheus_testkit::runtime_dir().join("data/game_maps_174.json")).unwrap(),
    )
    .unwrap();
    let spawns = maps["0x08000000000000D4"]["spawns"].as_array().unwrap();
    for spawn in &spawns[..12] {
        let position: [f64; 3] =
            std::array::from_fn(|axis| spawn["position"][axis].as_f64().unwrap());
        let mut mover = Mover::new(
            MoverSettings::default(),
            Arc::clone(&world),
            position,
            0,
            MOVER,
        )
        .unwrap();
        for frame in 0..20 {
            let command = Command {
                frame,
                ..Command::default()
            };
            mover
                .step(scene.triangles(), command, 0.016, |_, _| {})
                .unwrap();
        }
        let pose = mover.pose();
        let [x, y, z] = pose.position;
        let ground = scene
            .triangles()
            .ground(x, z, y + 1.0, y - 1.0, MOVER)
            .unwrap();
        assert_eq!(pose.flags & IN_AIR, 0, "{position:?}");
        assert!(
            (y - ground.y).abs() <= STEP,
            "{position:?}: {y} over {}",
            ground.y
        );
    }
}
