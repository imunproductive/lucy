//! The Workshop catalog build 104319 derives (`workshop_catalog_174.json`),
//! against what the client's editor shows and the research catalog
//! (`refs/reports/workshop/catalog_174.json`) recorded.

use serde_json::Value;

fn catalog() -> Value {
    let path = orpheus_testkit::runtime_dir().join("data/workshop_catalog_174.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn workshop_catalog_is_the_installs() {
    let catalog = catalog();
    let count = |key: &str| catalog[key].as_array().unwrap().len();
    assert_eq!(
        [
            count("events"),
            count("conditions"),
            count("actions"),
            count("values"),
            count("enums"),
            count("strings")
        ],
        [14, 1, 222, 267, 40, 725]
    );
    assert_eq!(catalog["events"][11]["name"], "Subroutine");
    let wait = &catalog["actions"][4];
    assert_eq!(catalog["conditions"][0]["graph"], "0x05800000000014A4");
    assert_eq!(wait["name"], "Wait");
    assert_eq!(wait["params"][0]["min"], 0.016);
    assert_eq!(wait["params"][1]["class"], "STU_7BF2036D");
    assert_eq!(wait["graph"], "0x0580000000001370");
    let set_global = &catalog["actions"][0];
    assert_eq!(set_global["params"][0]["kind"], 8);
    assert_eq!(set_global["params"][0]["global"], true);
    assert_eq!(set_global["params"][1]["default_value"], 0x4876);
    assert_eq!(catalog["actions"][136]["name"], "If");
    assert_eq!(catalog["actions"][136]["graph"], Value::Null);
    let number = catalog["values"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == 0x4876)
        .unwrap();
    assert_eq!(number["name"], "Number");
    assert_eq!(number["params"][0]["kind"], 6);
    assert_eq!(catalog["strings"][1], "{0}:");
    assert_eq!(catalog["heroes"]["Doomfist"], "0x02E000000000012F");
    assert_eq!(catalog["maps"]["Hanamura"], 0x165);
}
