//! The hero derivations against the runtime pack: the heroes, their bodies'
//! movers and the Tab board's stats.

use lucy_assets::{DERIVATIONS, Difference, Install, diff};

/// Derive `name` with the pack as the build folder and compare every output
/// with the pack's.
fn matches_the_pack(name: &str) {
    let install = Install::open(&lucy_testkit::game_dir(), "enUS").unwrap();
    let pack = lucy_testkit::oracle_pack();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == name)
        .unwrap_or_else(|| panic!("no derivation {name}"));
    let outputs = (derivation.derive)(&install, &pack).unwrap();
    assert!(!outputs.is_empty());
    for output in &outputs {
        let difference = diff(output, &pack).unwrap();
        assert_eq!(difference, Difference::Same, "{}", output.path);
    }
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_heroes_matches_the_pack() {
    matches_the_pack("game_heroes");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_movers_matches_the_pack() {
    matches_the_pack("game_movers");
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and a runtime pack (just test-assets <install> <pack>)"]
fn game_stats_matches_the_pack() {
    matches_the_pack("game_stats");
}

/// The stat rules read from the install's stats: the "Best" stats keep
/// their largest amount (`0346` Multikill - Best, `023B`), Fastest Gun
/// (`0525`) its smallest; Scoped Accuracy (`0218`) is Scoped Hits over
/// Scoped Shots, and Weapon Accuracy's divisor (`05F3`) is Shots Hit plus
/// Shots Missed. Eliminations (`0025`, a copy of `001F`) has no rule.
#[test]
#[ignore = "requires an Overwatch 1.74 install"]
fn the_stat_rules_come_from_the_stats_own_fields() {
    let table = lucy_testkit::runtime_json("data/game_stat_rules_174.json");
    let stat = |index: u32| format!("0x0860{:012X}", index);
    assert_eq!(table["best"][stat(0x346)], "max");
    assert_eq!(table["best"][stat(0x23b)], "max");
    assert_eq!(table["best"][stat(0x525)], "min");
    assert_eq!(
        table["derived"][stat(0x218)]["ratio"],
        serde_json::json!([stat(0x216), stat(0x217)])
    );
    assert_eq!(
        table["derived"][stat(0x5f3)]["sum"],
        serde_json::json!([stat(0x31), stat(0x5f2)])
    );
    assert!(table["derived"].get(stat(0x25)).is_none());
}

/// The career stat catalogs are the lists retail sent in 2022 (1.68, the
/// lobby's 27202 `+0x90`/`+0xC0`): every hero's, "all heroes", the roles',
/// and every map's but the three maps added after 1.68. The digests are of
/// retail's lists written as `key:stat,stat,…` lines.
#[test]
#[ignore = "requires an Overwatch 1.74 install"]
fn the_career_stat_catalogs_are_retails() {
    use sha2::{Digest, Sha256};
    let table = lucy_testkit::runtime_json("data/career_stats_174.json");
    let digest = |list: &str, leave_out: &[&str]| {
        let mut lines = String::new();
        for entry in table[list].as_array().unwrap() {
            let key = entry["key"].as_str().unwrap();
            if leave_out.contains(&key) {
                continue;
            }
            let stats: Vec<&str> = entry["stats"]
                .as_array()
                .unwrap()
                .iter()
                .map(|stat| stat.as_str().unwrap())
                .collect();
            lines += &format!("{key}:{}\n", stats.join(","));
        }
        Sha256::digest(lines)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        digest("heroes", &[]),
        "d8810c95aacf9620aa690f48e865f311ea72039e3ef5f85f34845b383013c3d4"
    );
    let after_1_68 = [
        "0x0800000000000E83",
        "0x0800000000000EEE",
        "0x0800000000000F14",
    ];
    assert_eq!(
        digest("maps", &after_1_68),
        "48130d31330ee3850feb920a6845bb212c4223a321e6488fd920923db7437cfb"
    );
    assert_eq!(table["maps"].as_array().unwrap().len(), 133);
}
