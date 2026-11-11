//! The map derivations (`game_maps`, `ai_spawners`, the collision caches)
//! against the runtime pack.

use orpheus_assets::{DERIVATIONS, Difference, Install, diff};

fn check(name: &str) {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::oracle_pack();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == name)
        .unwrap_or_else(|| panic!("no derivation {name}"));
    let outputs = (derivation.derive)(&install, &pack).unwrap();
    assert!(!outputs.is_empty());
    for output in outputs {
        let found = diff(&output, &pack).unwrap();
        assert_eq!(found, Difference::Same, "{}", output.path);
    }
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_maps_matches_the_pack() {
    check("game_maps");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn ai_spawners_matches_the_pack() {
    check("ai_spawners");
}

/// The Practice Range's collision cache against the pack's `0688.col`.
///
/// Known difference: the pack's cache came from a prop catalogue whose tool
/// couldn't read models stored inside bundles, so it lacks the Practice
/// Range's definitions 049A to 049F (models 1391 to 1396). Their models have
/// no static shape that stops the mover, so the derived cache only lists six
/// more empty models: the same geometry, the same placements (their model
/// numbers shifted past the extra ones), a header with 39 models instead of
/// 33, and so other bytes.
#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn collision_matches_the_pack() {
    use orpheus_physics::collision::cache::read_cache;

    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::oracle_pack();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "collision")
        .unwrap();
    let outputs = (derivation.derive)(&install, &pack).unwrap();
    assert_eq!(outputs.len(), 1);
    let output = &outputs[0];
    assert_eq!(output.path, "collision/0688.col");
    assert_eq!(
        diff(output, &pack).unwrap(),
        Difference::Differs("bytes".into())
    );

    let derived = read_cache(output.bytes.as_slice()).unwrap();
    let oracle = read_cache(std::fs::read(pack.join(&output.path)).unwrap().as_slice()).unwrap();
    let mut header = derived.header.clone();
    assert_eq!((header.models, oracle.header.models), (39, 33));
    header.models = oracle.header.models;
    assert_eq!(header, oracle.header);
    assert_eq!(derived.meshes, oracle.meshes);
    assert_eq!(derived.hulls, oracle.hulls);
    // The oracle's models are the derived ones without six empty ones.
    let mut number = Vec::new();
    let mut extra = 0;
    let mut next = 0;
    for model in &derived.models {
        if oracle.models.get(next) == Some(model) {
            number.push(Some(next as u32));
            next += 1;
        } else {
            assert!(
                model.is_empty(),
                "a derived model the pack lacks has shapes"
            );
            number.push(None);
            extra += 1;
        }
    }
    assert_eq!((next, extra), (oracle.models.len(), 6));
    let mut placements = derived.placements.clone();
    for placement in &mut placements {
        placement.model = number[placement.model as usize].expect("placed models have shapes");
    }
    assert_eq!(placements, oracle.placements);
}

/// The game modes' identifiers, the part of `collision_props_174.json` the
/// server reads. Known difference: the derived file has only `_source` and
/// `modes`; the pack's also has the storage keys of the maps' ENTITY chunks
/// and of the prop models and the prop definitions, which the manifests and
/// the props rule replace.
#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn collision_props_matches_the_pack() {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::oracle_pack();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "collision_props")
        .unwrap();
    let outputs = (derivation.derive)(&install, &pack).unwrap();
    assert_eq!(outputs.len(), 1);
    let derived: serde_json::Value = serde_json::from_slice(&outputs[0].bytes).unwrap();
    let oracle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pack.join(&outputs[0].path)).unwrap()).unwrap();
    let keys = |value: &serde_json::Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(keys(&derived), ["_source", "modes"]);
    assert_eq!(
        keys(&oracle),
        ["_source", "maps", "defs", "models", "modes"]
    );
    assert_eq!(derived["modes"], oracle["modes"]);
}
