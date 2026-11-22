//! The statescript graph bundles derived from the install match the pack's.

use std::io::Read;

use lucy_assets::{DERIVATIONS, Install, Output, first_difference};
use serde_json::Value;

/// Graphs the derivation adds to the pack's bundles (hero and bot): the initial graphs
/// of the entities hero weapon volleys spawn, which the Python extractor
/// didn't follow (it had only `0258`, the Biotic Field's, as a root), and the
/// graphs they reference. `0C1B` is Tracer's Pulse Bomb (`0045.003`).
const SPAWNED_GRAPHS: &[u64] = &[
    0x00C3, 0x0132, 0x0252, 0x065E, 0x096B, 0x0BA6, 0x0C1A, 0x0C1B, 0x0CCC, 0x0CCE, 0x0D00, 0x0E84,
    0x10FF, 0x1100, 0x1502, 0x150F, 0x151A, 0x157F, 0x1590, 0x1688, 0x1689, 0x18EB, 0x1C26, 0x1EB9,
    0x22B8,
];

fn derived(path: &str) -> Output {
    let install = Install::open(&lucy_testkit::game_dir(), "enUS").unwrap();
    let graphs = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "graphs")
        .unwrap();
    // The derivation reads other derivations' outputs (the placed entities
    // of `game_map_areas_174.json`, since 5866ba3, which no pack has), so it
    // runs over the derived cache, not the pack.
    (graphs.derive)(&install, &lucy_testkit::runtime_dir())
        .unwrap()
        .into_iter()
        .find(|output| output.path == path)
        .unwrap()
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
    let derived = keeps_the_pack(
        "data/game_graphs_174.json.gz",
        &["version", "bodies", "mode_variables"],
    );
    for index in SPAWNED_GRAPHS {
        assert!(derived.contains_key(index), "graph {index:04X}");
    }
    // The Workshop's graphs (`054/0155` actions, the condition `14A4`,
    // and helpers they play: the chase `1CCF`, Set Status's `224E`).
    for index in [0x1367, 0x1D8A, 0x1D45, 0x2E19, 0x14A4, 0x1CCF, 0x224E] {
        assert!(derived.contains_key(&index), "Workshop graph {index:04X}");
    }
}

/// The bot bundle likewise keeps every pack graph, entity definition and
/// AI weapon button; the derivation adds the same graphs as to the hero
/// bundle (volley-spawned and match flow graphs), each bot entity's target
/// tags, which the pack doesn't have, and the definitions of the map's
/// placed entities whose scripts the server runs (since 5866ba3, from
/// `game_map_areas_174.json`), so the pack's entities are a subset.
#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn bot_bundle_matches_the_pack() {
    let derived = keeps_the_pack(
        "data/game_graphs_bots_174.json.gz",
        &["version", "bodies", "mode_variables", "ai_weapon_buttons"],
    );
    for index in SPAWNED_GRAPHS {
        assert!(derived.contains_key(index), "graph {index:04X}");
    }
}

/// Check that the derived bundle at `path` keeps every graph of the pack's
/// unchanged, the listed top-level keys equal, and every root and missing
/// graph the pack lists; return the derived graphs by index.
fn keeps_the_pack(path: &str, equal: &[&str]) -> std::collections::BTreeMap<u64, Value> {
    let mut derived = json(&derived(path).bytes);
    if let Some(entities) = derived["entities"].as_object_mut() {
        for entity in entities.values_mut() {
            entity.as_object_mut().unwrap().remove("target_tags");
        }
    }
    let pack = json(&std::fs::read(lucy_testkit::oracle_pack().join(path)).unwrap());
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
    for &key in equal {
        assert_eq!(
            first_difference(&derived[key], &pack[key], key),
            None,
            "{key}"
        );
    }
    // Every entity definition the pack has comes out unchanged; the
    // derivation may add more.
    if let Some(entities) = pack["entities"].as_object() {
        for (key, entity) in entities {
            assert_eq!(
                first_difference(&derived["entities"][key], entity, key),
                None,
                "entity {key}"
            );
        }
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
    derived_graphs
}
