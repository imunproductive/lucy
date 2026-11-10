//! The statescript graph bundles derived from the install match the pack's.

use orpheus_assets::{DERIVATIONS, Difference, Install, Output, diff};

fn derived(path: &str) -> Output {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::runtime_dir();
    let graphs = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "graphs")
        .unwrap();
    (graphs.derive)(&install, &pack)
        .unwrap()
        .into_iter()
        .find(|output| output.path == path)
        .unwrap()
}

fn assert_matches(path: &str) {
    let output = derived(path);
    let difference = diff(&output, &orpheus_testkit::runtime_dir()).unwrap();
    assert_eq!(difference, Difference::Same, "{path}");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn hero_bundle_matches_the_pack() {
    assert_matches("data/game_graphs_174.json.gz");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn bot_bundle_matches_the_pack() {
    assert_matches("data/game_graphs_bots_174.json.gz");
}
