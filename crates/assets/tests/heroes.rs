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
