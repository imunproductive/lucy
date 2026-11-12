//! The voice sets derived from the install: the bots' pain lines and Soldier:
//! 76's communication wheel lines, as read from their `05F` and `078` assets.

use orpheus_assets::{DERIVATIONS, Install};
use serde_json::{Value, json};

fn derived() -> Value {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::runtime_dir();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "game_voice_sets")
        .unwrap();
    let output = (derivation.derive)(&install, &pack)
        .unwrap()
        .into_iter()
        .find(|output| output.path == "data/game_voice_sets_174.json")
        .unwrap();
    serde_json::from_slice(&output.bytes).unwrap()
}

/// The Practice Range bots share voice set `011D`, whose stimulus `0004`
/// ("PnSvr") answers criteria `0609` with ten lines, each "not `191E`";
/// Soldier: 76's `0056` answers the wheel's Hello (`0AEB`) with three lines,
/// heard within 35 m and with no cooldown.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn bodies_name_their_voice_sets_and_the_sets_their_lines() {
    let sets = derived();
    let bots = "0x07A000000000011D";
    for body in [
        "0x0400000000001403",
        "0x040000000000143E",
        "0x04000000000014ED",
    ] {
        assert_eq!(sets["bodies"][body], json!(bots), "{body}");
    }
    assert_eq!(
        sets["bodies"]["0x04000000000003CF"],
        json!("0x07A0000000000056")
    );
    let severe = &sets["sets"][bots]["0x0EE0000000000004"];
    assert_eq!(
        severe["criteria"],
        json!({"statescript": "0x0D80000000000609", "not": false})
    );
    assert_eq!(
        (
            &severe["cooldown"],
            &severe["cooldown_max"],
            &severe["radius"]
        ),
        (&json!(0.5), &json!(1.0), &json!(35.0))
    );
    let keys: Vec<&str> = severe["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| {
            assert_eq!(
                line["criteria"],
                json!({"statescript": "0x0D8000000000191E", "not": true})
            );
            line["key"].as_str().unwrap()
        })
        .collect();
    assert_eq!(
        keys,
        [
            "0x076000000000000B",
            "0x076000000000000C",
            "0x076000000000000D",
            "0x076000000000000E",
            "0x076000000000000F",
            "0x0760000000000059",
            "0x076000000000005A",
            "0x076000000000005B",
            "0x076000000000005C",
            "0x076000000000005D",
        ]
    );
    let hello = &sets["sets"]["0x07A0000000000056"]["0x0EE00000000001CF"];
    assert_eq!(
        hello,
        &json!({
            "criteria": {"statescript": "0x0D80000000000AEB", "not": false},
            "cooldown": 0.0,
            "cooldown_max": 0.0,
            "radius": 35.0,
            "lines": [
                {"key": "0x0760000000006F78"},
                {"key": "0x0760000000007F02"},
                {"key": "0x0760000000007F03"},
            ],
        })
    );
}
