//! DataTool's list files (`list-heroes`, `list-maps`, `list-unlocks`,
//! `list-general-unlocks`), derived from the install and compared with the
//! pack.

use std::sync::LazyLock;

use lucy_assets::{DERIVATIONS, Difference, Install, Output, diff};
use serde_json::Value;

/// The derivation's files, derived once for every test.
static OUTPUTS: LazyLock<Vec<Output>> = LazyLock::new(|| {
    let install = Install::open(&lucy_testkit::game_dir(), "enUS").unwrap();
    let lists = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "lists")
        .expect("the lists derivation is registered");
    (lists.derive)(&install, &lucy_testkit::oracle_pack()).unwrap()
});

fn output(path: &str) -> &'static Output {
    OUTPUTS
        .iter()
        .find(|output| output.path == path)
        .unwrap_or_else(|| panic!("{path} is not derived"))
}

fn assert_same(path: &str) {
    let difference = diff(output(path), &lucy_testkit::oracle_pack()).unwrap();
    assert_eq!(difference, Difference::Same, "{path}");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn extracted_heroes_matches_the_pack() {
    assert_same("data/extracted_heroes.json");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn extracted_items_matches_the_pack() {
    assert_same("data/extracted_items.json");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn extracted_general_unlocks_matches_the_pack() {
    assert_same("data/extracted_general_unlocks.json");
}

/// The pack's `extracted_maps.json` was not made from this install's display
/// strings. Of the 278 strings (`07C`) it shows, 207 have other text there,
/// consistently per string: `""` for 193 (map descriptions, sublines,
/// attack/defend labels, every game mode name, and ten map names such as
/// `Tutorial`), and 14 seasonal map names without their season (`Hollywood`
/// for `Hollywood (Halloween)`). The install's strings decode plainly, and
/// DataTool (rev `7afe5f52`) run as `list-maps --json` on this install writes
/// what the derivation writes. Everything else (GUIDs, enums, game modes,
/// celebrations) matches. The test pins every differing place and its kind,
/// `path<TAB>empty` (the pack has `""`) or `path<TAB>season` (the pack has
/// the derived name without its last ` (…)`), without the install's text.
#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn extracted_maps_matches_the_pack() {
    let path = "data/extracted_maps.json";
    let derived: Value = serde_json::from_slice(&output(path).bytes).unwrap();
    let pack = std::fs::read(lucy_testkit::oracle_pack().join(path)).unwrap();
    let pack: Value = serde_json::from_slice(&pack).unwrap();
    let mut found = Vec::new();
    differences(&derived, &pack, "", &mut found);
    let found: Vec<String> = found.iter().map(|line| difference_kind(line)).collect();
    let known: Vec<&str> = include_str!("lists/extracted_maps_differences.tsv")
        .lines()
        .collect();
    assert_eq!(
        found, known,
        "{path} differs from the pack beyond the pinned strings"
    );
}

/// `path<TAB>derived<TAB>pack` as `path<TAB>empty`, `path<TAB>season`, or
/// unchanged when it is neither.
fn difference_kind(line: &str) -> String {
    let mut parts = line.splitn(3, '\t');
    let (Some(path), Some(derived), Some(pack)) = (parts.next(), parts.next(), parts.next()) else {
        return line.to_owned();
    };
    let text = |json: &str| {
        serde_json::from_str::<Value>(json)
            .ok()?
            .as_str()
            .map(str::to_owned)
    };
    let kind = match (text(derived), text(pack)) {
        (Some(_), Some(pack)) if pack.is_empty() => "empty",
        (Some(derived), Some(pack))
            if derived
                .strip_prefix(&pack)
                .is_some_and(|season| season.starts_with(" (") && season.ends_with(')')) =>
        {
            "season"
        }
        _ => return line.to_owned(),
    };
    format!("{path}\t{kind}")
}

/// Every place two JSON values differ, as `path<TAB>derived<TAB>pack`.
/// Numbers compare by value.
fn differences(derived: &Value, pack: &Value, at: &str, found: &mut Vec<String>) {
    match (derived, pack) {
        (Value::Object(derived), Value::Object(pack)) => {
            for (key, value) in derived {
                let path = format!("{at}.{key}");
                match pack.get(key) {
                    Some(other) => differences(value, other, &path, found),
                    None => found.push(format!("{path}\t{value}\t(absent)")),
                }
            }
            for (key, value) in pack {
                if !derived.contains_key(key) {
                    found.push(format!("{at}.{key}\t(absent)\t{value}"));
                }
            }
        }
        (Value::Array(derived), Value::Array(pack)) if derived.len() == pack.len() => {
            for (index, (derived, pack)) in derived.iter().zip(pack).enumerate() {
                differences(derived, pack, &format!("{at}[{index}]"), found);
            }
        }
        (Value::Number(a), Value::Number(b)) if a.as_f64() == b.as_f64() => {}
        _ if derived == pack => {}
        _ => found.push(format!("{at}\t{derived}\t{pack}")),
    }
}
