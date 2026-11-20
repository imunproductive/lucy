//! The Workshop against build 104319's own catalog: the test subset is
//! the install's, and the Doomfist parkour scripts of the DPK archive
//! parse, round-trip through the binary form and run.

mod common;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use orpheus_workshop::{Catalog, Engine, binary::encode, decode, parse, text::Setting};
use serde_json::Value as Json;

fn install_catalog() -> Arc<Catalog> {
    Arc::new(Catalog::load(&orpheus_testkit::runtime_dir().join("data")).unwrap())
}

fn scripts(folder: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scripts(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "txt") {
            found.push(path);
        }
    }
}

#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn catalog_subset_is_the_installs() {
    let path = orpheus_testkit::runtime_dir().join("data/workshop_catalog_174.json");
    let install: Json = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let subset: Json = serde_json::from_str(common::CATALOG).unwrap();
    for key in ["events", "conditions", "strings"] {
        assert_eq!(subset[key], install[key], "{key}");
    }
    for (index, action) in subset["actions"].as_array().unwrap().iter().enumerate() {
        let full = &install["actions"][index];
        if action.get("stripped").is_some() {
            assert_eq!(
                (&action["name"], &action["graph"]),
                (&full["name"], &full["graph"])
            );
        } else {
            assert_eq!(action, full, "action {index}");
        }
    }
    for (key, id) in [("values", "id"), ("enums", "id")] {
        for item in subset[key].as_array().unwrap() {
            let full = install[key]
                .as_array()
                .unwrap()
                .iter()
                .find(|full| full[id] == item[id])
                .unwrap();
            assert_eq!(item, full, "{key} {}", item[id]);
        }
    }
    for key in ["heroes", "maps", "game_modes"] {
        for (name, value) in subset[key].as_object().unwrap() {
            assert_eq!(&install[key][name], value, "{key} {name}");
        }
    }
}

#[test]
#[ignore = "requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn dpk_archive_scripts_parse_round_trip_and_run() {
    let catalog = install_catalog();
    let mut found = Vec::new();
    scripts(&orpheus_testkit::dpk_archive(), &mut found);
    found.sort();
    assert!(found.len() >= 15, "{} scripts", found.len());
    let mut failures = Vec::new();
    for path in &found {
        let text = fs::read_to_string(path).unwrap();
        let script = match parse(&text, &catalog) {
            Ok(script) => script,
            Err(error) => {
                let name = path.file_name().unwrap().to_string_lossy();
                failures.push((name.into_owned(), error.to_string()));
                continue;
            }
        };
        // User settings travel as a vector whose layout is not known
        // (IDA-questions.md "WorkshopModel" 2); the rest must survive.
        let mut logic = script.logic.clone();
        logic.workshop_settings.clear();
        let (block, items) = encode(&logic, &catalog);
        let decoded = decode(&block, &items, &catalog).unwrap();
        assert_eq!(decoded, logic, "{}: binary round trip", path.display());
    }
    // Four scripts name what 1.74's catalog lacks: effect types added after
    // 1.74 (Lúcio Sound Barrier Cast Effect, Reaper Wraith Form Effect,
    // Baptiste Immortality Field Protected Effect) and an `ElseIf` action
    // (1.74 writes `Else If`). The 1.74 client's importer refuses them too.
    let refused: Vec<&str> = failures.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        refused,
        [
            "DoomPioneerParkour.txt",
            "DGTA7_beta_alternate_doom_parkour_framework.txt",
            "E7MMF_Doomfist_Parkour_Academy_Learn_Rollouts_Bounces_Diags_Map_Sp.txt",
            "ZBFZG_Trap_Maker.txt",
        ],
        "{failures:#?}"
    );

    let c32s2 = found
        .iter()
        .find(|path| path.to_string_lossy().contains("C32S2"))
        .unwrap();
    let script = parse(&fs::read_to_string(c32s2).unwrap(), &catalog).unwrap();
    assert_eq!(script.logic.rules.len(), 103);
    assert_eq!(script.logic.global_names[127], "TimeRemaining");
    assert_eq!(script.logic.player_names[127], "ServerPerformance");
    assert_eq!(script.logic.subroutine_names[67], "MissionTimeChange");
    // The Overwatch 2 map identifier after the map name.
    let mut map_lines = Vec::new();
    collect(&script.settings, &mut |setting| {
        if let Some(line) = setting.ow2_map_id() {
            map_lines.push((line.0.to_owned(), line.1));
        }
    });
    assert_eq!(map_lines, [("Hanamura".to_owned(), 972777519512068154)]);

    // C32S2 runs: one player for a few seconds of ticks, against the fake
    // match, without the engine stopping a rule.
    let mut engine = Engine::new(&script.logic, Arc::clone(&catalog), 0.016, 7);
    let mut game = common::Match::new(Arc::clone(&catalog));
    game.players.push(common::Player::new(1, 0));
    for _ in 0..300 {
        game.advance();
        engine.tick(&mut game);
    }
    assert_eq!(engine.faults(), [] as [String; 0]);
    assert!(!game.started.is_empty());
}

fn collect(settings: &[Setting], visit: &mut impl FnMut(&Setting)) {
    for setting in settings {
        visit(setting);
        if let Setting::Block(_, block) = setting {
            collect(block, visit);
        }
    }
}
