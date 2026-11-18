//! The hero bodies' AI definitions and difficulty profiles build 104319
//! derives (`game_ai_definitions_174.json`).

use orpheus_assets::{DERIVATIONS, Install};
use serde_json::{Value, json};

fn derived() -> Value {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "game_ai_definitions")
        .unwrap();
    // It reads game_heroes_174.json from the build folder.
    let outputs = (derivation.derive)(&install, &orpheus_testkit::runtime_dir()).unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].path, "data/game_ai_definitions_174.json");
    serde_json::from_slice(&outputs[0].bytes).unwrap()
}

/// The twelve heroes the AI plays each have an AI definition of their own;
/// the other heroes share `1853` (Mercy `195B` and Junkrat `195C` have their
/// own but no AI support), and Moira and Echo share Reaper's `118E`.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn each_ai_hero_has_its_own_definition() {
    let table = derived();
    let heroes = table["heroes"].as_object().unwrap();
    let mut ai: Vec<(&str, &str)> = heroes
        .values()
        .filter(|hero| hero["supports_ai"] == true)
        .map(|hero| {
            (
                hero["name"].as_str().unwrap(),
                hero["ai_definition"].as_str().unwrap(),
            )
        })
        .collect();
    ai.sort();
    assert_eq!(
        ai,
        [
            ("Ana", "0x040000000000160D"),
            ("Bastion", "0x0400000000000D57"),
            ("Cassidy", "0x04000000000015F0"),
            ("Lúcio", "0x0400000000001635"),
            ("Mei", "0x0400000000001868"),
            ("Reaper", "0x040000000000118E"),
            ("Roadhog", "0x040000000000147E"),
            ("Soldier: 76", "0x04000000000011F4"),
            ("Sombra", "0x040000000000178D"),
            ("Torbjörn", "0x0400000000001667"),
            ("Zarya", "0x04000000000014BA"),
            ("Zenyatta", "0x0400000000001468"),
        ]
    );
    let named = |name: &str| {
        heroes.values().find(|hero| hero["name"] == name).unwrap()["ai_definition"].clone()
    };
    assert_eq!(named("Tracer"), "0x0400000000001853");
    assert_eq!(named("Moira"), "0x040000000000118E");
    assert_eq!(named("Mercy"), "0x040000000000195B");
}

/// Bastion's first per-mode list, with `06FE`'s overrides, and his three
/// difficulty profiles. Every AI definition declares component 92
/// (`STU_76AF64F0`).
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn bastion_lists_his_graphs_and_profiles() {
    let table = derived();
    let bastion = &table["definitions"]["0x0400000000000D57"];
    assert_eq!(
        bastion["modes"]["m_9325CAD8"],
        json!({
            "graphs": [
                {"graph": "0x0580000000000778", "overrides": {}},
                {"graph": "0x058000000000083E", "overrides": {}},
                {"graph": "0x05800000000006FE", "overrides": {
                    "0x0D800000000009EA": 20, "0x0D800000000009EB": 4,
                }},
                {"graph": "0x0580000000000340", "overrides": {}},
            ],
            "schema": {},
        })
    );
    assert_eq!(
        bastion["modes"]["m_3B8A79C3"]["schema"],
        json!({"0x0D80000000001A55": 0.5})
    );
    assert_eq!(
        bastion["difficulty_profiles"],
        json!({
            "0x0D8000000000195F": "0x0B50000000000002",
            "0x0D8000000000195E": "0x0B50000000000003",
            "0x0D80000000001960": "0x0B5000000000000A",
        })
    );
    assert_eq!(bastion["m_72D00D60"], 0.1);
    let definitions = table["definitions"].as_object().unwrap();
    assert_eq!(definitions.len(), 15);
    assert!(
        definitions
            .values()
            .all(|definition| definition["component_92"] == true)
    );
}

/// A profile's values are its own over its parent's: Soldier: 76's weapon
/// damage `1961` is 4, 10 and 20 under `195E`, `195F` and `1960`, and his
/// aim values come from the root profile `0001`.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn profiles_resolve_through_their_parents() {
    let table = derived();
    let profiles = &table["profiles"];
    let soldier = &table["definitions"]["0x04000000000011F4"]["difficulty_profiles"];
    let value = |difficulty: &str, identifier: &str| {
        let profile = soldier[difficulty].as_str().unwrap();
        profiles[profile]["values"][identifier].clone()
    };
    assert_eq!(value("0x0D8000000000195E", "0x0D80000000001961"), 4.0);
    assert_eq!(value("0x0D8000000000195F", "0x0D80000000001961"), 10.0);
    assert_eq!(value("0x0D80000000001960", "0x0D80000000001961"), 20.0);
    assert_eq!(value("0x0D80000000001960", "0x0D80000000001CD8"), 0.8);
    assert_eq!(value("0x0D8000000000195F", "0x0D8000000000196E"), 6.0);
    assert_eq!(
        profiles["0x0B50000000000005"]["parent"],
        "0x0B50000000000004"
    );
    assert_eq!(profiles["0x0B50000000000001"]["parent"], Value::Null);
}
