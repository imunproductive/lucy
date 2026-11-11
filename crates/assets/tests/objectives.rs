//! The objectives' component data derived from the install
//! (`data/game_objectives_174.json`).

use orpheus_assets::{DERIVATIONS, Install};
use serde_json::{Value, json};

fn derived() -> Value {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::runtime_dir();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "game_objectives")
        .unwrap();
    let output = (derivation.derive)(&install, &pack)
        .unwrap()
        .into_iter()
        .find(|output| output.path == "data/game_objectives_174.json")
        .unwrap();
    serde_json::from_slice(&output.bytes).unwrap()
}

/// The capture area (`0x37.003`) marks progress at 1/3 and 2/3, as `f32`s.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_capture_area_marks_thirds() {
    let file = derived();
    assert_eq!(
        file["capture_area"],
        json!({"entity": "0x0400000000000037", "segments": [0.33333334, 0.6666667]})
    );
}
