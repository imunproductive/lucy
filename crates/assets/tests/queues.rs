//! The queue cards and Arcade cards derived from the install match the pack's.

use orpheus_assets::{DERIVATIONS, Difference, Install, diff};

fn assert_matches(name: &str) {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::oracle_pack();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == name)
        .unwrap();
    let outputs = (derivation.derive)(&install, &pack).unwrap();
    assert!(!outputs.is_empty());
    for output in &outputs {
        assert_eq!(
            diff(output, &pack).unwrap(),
            Difference::Same,
            "{}",
            output.path
        );
    }
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_queues_matches_the_pack() {
    assert_matches("game_queues");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn arcade_cards_matches_the_pack() {
    assert_matches("arcade_cards");
}
