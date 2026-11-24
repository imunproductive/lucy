//! The animation lengths derived from the install
//! (`data/game_anim_lengths_174.json`).

use serde_json::{Value, json};

fn derived() -> Value {
    assert_eq!(
        lucy_testkit::derived_outputs("game_anim_lengths"),
        ["data/game_anim_lengths_174.json"]
    );
    lucy_testkit::runtime_json("data/game_anim_lengths_174.json")
}

/// Every hero and bot body names one blend tree set, whose every alias is
/// listed (empty when its length can't be read). The common graph `02C5`
/// (node 51) plays alias `0C8:043C` with no authored length: on Reaper
/// (`0022.003`, set `0007.021`) a leaf of 4.77 s for every category, while
/// Soldier: 76's set (`011C.021`) doesn't list the alias.
/// The Training Bot's alias `0451` (graph `0BA8`'s reset) is 2.67 s
/// through a selector that inherits, so it is listed for no category and
/// for each category of the install, without `any`. Every scalar the
/// bodies' sets give is 0. The 292 emotes' sets are listed too (none is a
/// body's): Soldier: 76's default emote (`0250.0904`) plays `045C` from
/// `06AD.021` for 5 s.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_bodies_aliases_have_their_lengths() {
    let file = derived();
    let bodies = file["bodies"].as_object().unwrap();
    assert_eq!(bodies.len(), 38);
    let sets = file["sets"].as_object().unwrap();
    assert_eq!(sets.len(), 33 + 292);
    let set = |body: &str| &sets[bodies[body].as_str().unwrap()];
    assert_eq!(bodies["0x04000000000003CF"], json!("0x004000000000011C"));
    let soldier = set("0x04000000000003CF").as_object().unwrap();
    assert_eq!(soldier.len(), 67);
    assert!(soldier.get("0x0C8000000000043C").is_none());
    assert!(soldier.get("0x0C8000000000045C").is_none());
    assert_eq!(
        sets["0x00400000000006AD"]["0x0C8000000000045C"],
        json!({"lengths": {"any": 5.0000005}, "scalars": {"any": 0.0}})
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
    let scalars: Vec<_> = bodies
        .values()
        .flat_map(|set| sets[set.as_str().unwrap()].as_object().unwrap().values())
        .filter_map(|alias| alias.get("scalars"))
        .flat_map(|scalars| scalars.as_object().unwrap().values())
        .collect();
    assert!(!scalars.is_empty());
    assert!(scalars.iter().all(|scalar| **scalar == json!(0.0)));
}
