//! The animation lengths derived from the install
//! (`data/game_anim_lengths_174.json`).

use orpheus_assets::{DERIVATIONS, Install};
use serde_json::{Value, json};

fn derived() -> Value {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::runtime_dir();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "game_anim_lengths")
        .unwrap();
    let [output] = &(derivation.derive)(&install, &pack).unwrap()[..] else {
        panic!("one file");
    };
    assert_eq!(output.path, "data/game_anim_lengths_174.json");
    serde_json::from_slice(&output.bytes).unwrap()
}

/// Every hero and bot body names one blend tree set. The common graph
/// `02C5` (node 51) plays alias `0C8:043C` with no authored length: on
/// Reaper (`0022.003`, set `0007.021`) a leaf of 4.77 s for every
/// category, while Soldier: 76's set (`011C.021`) doesn't list the alias.
/// The Training Bot's alias `0451` (graph `0BA8`'s reset) is 2.67 s
/// through a selector that inherits, so it is listed for no category and
/// for each category of the install, without `any`. Every scalar these
/// sets give is 0.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_bodies_aliases_have_their_lengths() {
    let file = derived();
    let bodies = file["bodies"].as_object().unwrap();
    assert_eq!(bodies.len(), 38);
    let sets = file["sets"].as_object().unwrap();
    let set = |body: &str| &sets[bodies[body].as_str().unwrap()];
    assert_eq!(bodies["0x04000000000003CF"], json!("0x004000000000011C"));
    assert!(
        set("0x04000000000003CF")
            .get("0x0C8000000000043C")
            .is_none()
    );
    assert_eq!(bodies["0x0400000000000022"], json!("0x0040000000000007"));
    assert_eq!(
        set("0x0400000000000022")["0x0C8000000000043C"]["lengths"],
        json!({"any": 4.766667})
    );
    let reset = set("0x0400000000001403")["0x0C80000000000451"]["lengths"]
        .as_object()
        .unwrap();
    assert!(reset.len() > 100, "{}", reset.len());
    assert!(!reset.contains_key("any"));
    assert!(reset.values().all(|seconds| *seconds == json!(2.6666667)));
    assert_eq!(reset["0x0000000000000000"], json!(2.6666667));
    let scalars: Vec<_> = sets
        .values()
        .flat_map(|aliases| aliases.as_object().unwrap().values())
        .filter_map(|alias| alias.get("scalars"))
        .flat_map(|scalars| scalars.as_object().unwrap().values())
        .collect();
    assert!(!scalars.is_empty());
    assert!(scalars.iter().all(|scalar| **scalar == json!(0.0)));
}
