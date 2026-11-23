//! The loot box economy (`data/economy_174.json`) against the install.

use std::collections::{BTreeSet, HashMap};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The event box items sold at the event price are, for the Summer Games,
/// Halloween, Winter and Lunar New Year boxes, exactly those the 1.68 retail
/// capture priced so (its 24300/24900 store entries; icons are not sold).
/// The digest is of their GUIDs, sorted, one per line. Each event box's
/// prices are three times the base box's.
#[test]
#[ignore = "requires an Overwatch 1.74 install"]
fn the_event_prices_are_retails() {
    let economy = lucy_testkit::runtime_json("data/economy_174.json");
    let prices = &economy["item_prices"];
    for box_type in 1..=6 {
        for (rarity, price) in prices["credits"].as_object().unwrap() {
            let event = &prices["event_credits"][box_type.to_string()][rarity];
            assert_eq!(event.as_i64(), Some(3 * price.as_i64().unwrap()));
        }
    }

    let mut catalog = HashMap::new();
    for path in [
        "data/extracted_items.json",
        "data/extracted_general_unlocks.json",
    ] {
        box_items(&lucy_testkit::runtime_json(path), &mut catalog);
    }
    let older_events = ["SummerGames", "Halloween", "Winter", "LunarNewYear"];
    let sold: BTreeSet<&str> = prices["event_priced"]
        .as_array()
        .unwrap()
        .iter()
        .map(|guid| guid.as_str().unwrap())
        .filter(|guid| {
            let index = u64::from_str_radix(&guid[2..], 16).unwrap() & 0xFFFF_FFFF_FFFF;
            let (box_type, item_type) = &catalog[&index];
            older_events.contains(&box_type.as_str()) && item_type != "Icon"
        })
        .collect();
    let lines = sold.into_iter().collect::<Vec<_>>().join("\n");
    let digest: String = Sha256::digest(lines)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        digest,
        "d1651838425b036a0fa13a0a001ee80d8570a0c03eec0f9a8a1ab7b8448d565b"
    );
}

/// Each loot box item's index -> (box type, item type).
fn box_items(value: &Value, out: &mut HashMap<u64, (String, String)>) {
    match value {
        Value::Object(map) => {
            for (name, value) in map {
                if name != "LootBoxesUnlocks" {
                    box_items(value, out);
                    continue;
                }
                for group in value.as_array().into_iter().flatten() {
                    let box_type = group["LootBoxType"].as_str().unwrap_or_default();
                    for unlock in group["Unlocks"].as_array().into_iter().flatten() {
                        let guid = unlock["GUID"].as_str().unwrap();
                        let index = u64::from_str_radix(guid.split('.').next().unwrap(), 16);
                        out.insert(
                            index.unwrap(),
                            (box_type.into(), unlock["Type"].as_str().unwrap().into()),
                        );
                    }
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| box_items(item, out)),
        _ => {}
    }
}
