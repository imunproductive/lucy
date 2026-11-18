//! The hero derivations against the runtime pack: the heroes, their bodies'
//! movers and the Tab board's stats.

use orpheus_assets::{DERIVATIONS, Difference, Install, diff};

/// Derive `name` with the pack as the build folder and compare every output
/// with the pack's.
fn matches_the_pack(name: &str) {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::oracle_pack();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == name)
        .unwrap_or_else(|| panic!("no derivation {name}"));
    let outputs = (derivation.derive)(&install, &pack).unwrap();
    assert!(!outputs.is_empty());
    for output in &outputs {
        let difference = diff(output, &pack).unwrap();
        assert_eq!(difference, Difference::Same, "{}", output.path);
    }
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_heroes_matches_the_pack() {
    matches_the_pack("game_heroes");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_movers_matches_the_pack() {
    matches_the_pack("game_movers");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_stats_matches_the_pack() {
    matches_the_pack("game_stats");
}

/// The stat rules read from the install's stats: the "Best" stats keep
/// their largest amount (`0346` Multikill - Best, `023B`), Fastest Gun
/// (`0525`) its smallest; Scoped Accuracy (`0218`) is Scoped Hits over
/// Scoped Shots, and Weapon Accuracy's divisor (`05F3`) is Shots Hit plus
/// Shots Missed. Eliminations (`0025`, a copy of `001F`) has no rule.
#[test]
#[ignore = "requires an Overwatch 1.74 install"]
fn the_stat_rules_come_from_the_stats_own_fields() {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "game_stat_rules")
        .unwrap();
    let outputs = (derivation.derive)(&install, std::path::Path::new(".")).unwrap();
    let table: serde_json::Value = serde_json::from_slice(&outputs[0].bytes).unwrap();
    let stat = |index: u32| format!("0x0860{:012X}", index);
    assert_eq!(table["best"][stat(0x346)], "max");
    assert_eq!(table["best"][stat(0x23b)], "max");
    assert_eq!(table["best"][stat(0x525)], "min");
    assert_eq!(
        table["derived"][stat(0x218)]["ratio"],
        serde_json::json!([stat(0x216), stat(0x217)])
    );
    assert_eq!(
        table["derived"][stat(0x5f3)]["sum"],
        serde_json::json!([stat(0x31), stat(0x5f2)])
    );
    assert!(table["derived"].get(stat(0x25)).is_none());
}
