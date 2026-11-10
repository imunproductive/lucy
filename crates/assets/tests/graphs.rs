//! The statescript graph bundles derived from the install match the pack's.

use std::io::Read;

use orpheus_assets::{DERIVATIONS, Difference, Install, Output, diff, first_difference};
use serde_json::Value;

/// Graphs the derivation adds to the pack's hero bundle: the initial graphs
/// of the entities hero weapon volleys spawn, which the Python extractor
/// didn't follow (it had only `0258`, the Biotic Field's, as a root), and the
/// graphs they reference. `0C1B` is Tracer's Pulse Bomb (`0045.003`).
const SPAWNED_GRAPHS: &[u64] = &[
    0x00C3, 0x0132, 0x0252, 0x065E, 0x096B, 0x0BA6, 0x0C1A, 0x0C1B, 0x0CCC, 0x0CCE, 0x0D00, 0x0E84,
    0x10FF, 0x1100, 0x1502, 0x150F, 0x151A, 0x157F, 0x1590, 0x1688, 0x1689, 0x18EB, 0x1C26, 0x1EB9,
    0x22B8,
];

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

fn json(bytes: &[u8]) -> Value {
    let mut text = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut text)
        .unwrap();
    serde_json::from_slice(&text).unwrap()
}

/// Every pack graph comes out unchanged, and so do the bodies, the mode
/// variables and the version. The derivation may add graphs (among them
/// [`SPAWNED_GRAPHS`], which must be there), roots and missing graphs, but
/// keeps every one the pack has.
#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn hero_bundle_matches_the_pack() {
    let path = "data/game_graphs_174.json.gz";
    let derived = json(&derived(path).bytes);
    let pack = json(&std::fs::read(orpheus_testkit::runtime_dir().join(path)).unwrap());
    let by_index = |bundle: &Value| -> std::collections::BTreeMap<u64, Value> {
        bundle["graphs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|graph| (graph["index"].as_u64().unwrap(), graph.clone()))
            .collect()
    };
    let (derived_graphs, pack_graphs) = (by_index(&derived), by_index(&pack));
    for (index, graph) in &pack_graphs {
        let found = derived_graphs
            .get(index)
            .unwrap_or_else(|| panic!("graph {index:04X} is missing"));
        assert_eq!(
            first_difference(found, graph, ""),
            None,
            "graph {index:04X}"
        );
    }
    for index in SPAWNED_GRAPHS {
        assert!(derived_graphs.contains_key(index), "graph {index:04X}");
    }
    for key in ["version", "bodies", "mode_variables"] {
        assert_eq!(
            first_difference(&derived[key], &pack[key], key),
            None,
            "{key}"
        );
    }
    let contains = |outer: &Value, inner: &Value| {
        inner
            .as_array()
            .unwrap()
            .iter()
            .all(|item| outer.as_array().unwrap().contains(item))
    };
    assert!(contains(&derived["missing"], &pack["missing"]));
    assert_eq!(derived["roots"]["bodies"], pack["roots"]["bodies"]);
    assert!(contains(
        &derived["roots"]["graphs"],
        &pack["roots"]["graphs"]
    ));
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn bot_bundle_matches_the_pack() {
    assert_matches("data/game_graphs_bots_174.json.gz");
}
