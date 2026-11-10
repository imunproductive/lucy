//! The body and mode values derived from the install equal the values the
//! Python reference hard-coded from the same assets.

use orpheus_assets::{DERIVATIONS, Install};
use serde_json::{Value, json};

fn derived(path: &str) -> Value {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::runtime_dir();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "game_body_data")
        .unwrap();
    let output = (derivation.derive)(&install, &pack)
        .unwrap()
        .into_iter()
        .find(|output| output.path == path)
        .unwrap();
    serde_json::from_slice(&output.bytes).unwrap()
}

/// combat.py `EYE` and `EYE_CROUCHED`: Soldier: 76's `03CF.003` eyes.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn eyes_match_the_python_reference() {
    let eyes = derived("data/game_eyes_174.json");
    assert_eq!(
        eyes["0x04000000000003CF"],
        json!({"name": "Soldier: 76", "eye": 1.55, "eye_crouched": 1.085})
    );
    // A body without a first-person component: the type's defaults, combat.py
    // `EYE` and `EYE_CROUCHED_DEFAULT`.
    assert_eq!(eyes["0x040000000000143E"]["eye"], json!(1.55));
    assert_eq!(eyes["0x040000000000143E"]["eye_crouched"], json!(1.15));
}

/// combat.py `RESPAWN` (`0F4A` and `0EC2` send 06FD.025 {10}) and
/// `PRACTICE_RESPAWN` (`13C0` sends {3}).
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn respawns_match_the_python_reference() {
    let respawns = derived("data/game_respawns_174.json");
    for (mode, seconds) in [
        // 13C0
        ("0x0230000000000018", 3),
        ("0x023000000000005A", 3),
        // 0F4A's #20680
        ("0x0230000000000014", 10),
        ("0x0230000000000015", 10),
        ("0x0230000000000016", 10),
        // 0EC2
        ("0x0230000000000017", 10),
    ] {
        assert_eq!(respawns[mode], json!(seconds), "{mode}");
    }
}

/// projectiles.py `FIELD_ENTITY` and `FIELD_GRAPH`: the Biotic Field
/// `03DC.003`, which 0257's canister leaves, starts graph `0258`.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_biotic_field_starts_its_graph() {
    let spawns = derived("data/game_spawns_174.json");
    assert_eq!(spawns["0x04000000000003DC"], json!({"graphs": [0x0258]}));
}
