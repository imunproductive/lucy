//! The hit shapes derived from the install (`data/game_hitboxes_174.json`).

use orpheus_assets::{DERIVATIONS, Install};
use serde_json::Value;

fn derived() -> Value {
    let install = Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let pack = orpheus_testkit::runtime_dir();
    let derivation = DERIVATIONS
        .iter()
        .find(|derivation| derivation.name == "hitboxes")
        .unwrap();
    let output = (derivation.derive)(&install, &pack)
        .unwrap()
        .into_iter()
        .find(|output| output.path == "data/game_hitboxes_174.json")
        .unwrap();
    serde_json::from_slice(&output.bytes).unwrap()
}

/// The highest point of a shape.
fn top(shape: &Value) -> f64 {
    let radius = shape["radius"].as_f64().unwrap();
    let y = |point: &Value| point[1].as_f64().unwrap();
    match shape["kind"].as_str().unwrap() {
        "sphere" => y(&shape["centre"]) + radius,
        "capsule" => y(&shape["a"]).max(y(&shape["b"])) + radius,
        kind => panic!("a {kind}"),
    }
}

/// The lowest point of a shape.
fn bottom(shape: &Value) -> f64 {
    let radius = shape["radius"].as_f64().unwrap();
    let y = |point: &Value| point[1].as_f64().unwrap();
    match shape["kind"].as_str().unwrap() {
        "sphere" => y(&shape["centre"]) - radius,
        "capsule" => y(&shape["a"]).min(y(&shape["b"])) - radius,
        kind => panic!("a {kind}"),
    }
}

fn shapes<'a>(file: &'a Value, body: &str) -> &'a Vec<Value> {
    file[body]["shapes"].as_array().unwrap()
}

fn critical(shapes: &[Value]) -> Vec<&Value> {
    shapes
        .iter()
        .filter(|shape| shape["critical"] == Value::Bool(true))
        .collect()
}

/// The Training Bot (`1403.003`, model `312E`): five spheres and two arm
/// capsules, placed by its idle pose, clip `006:3920` (the one clip keying
/// every bone that its blend trees play when no animation category is
/// set), as the bot is seen in the game: it hovers 0.5 m above its rest
/// pose with its arms hanging down. Its head sphere, the top of the body,
/// is the only critical one, centred at its mover's 2 m eye.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_training_bot_hovers_with_its_arms_down_and_a_critical_head() {
    let file = derived();
    let bot = &file["0x0400000000001403"];
    assert_eq!(bot["name"], "Training Bot");
    assert_eq!(bot["model"], "0x0D0000000000312E");
    assert_eq!(bot["pose"], "0x0A00000000003920");
    let shapes = shapes(&file, "0x0400000000001403");
    assert_eq!(shapes.len(), 7);
    let kinds = |kind: &str| shapes.iter().filter(|shape| shape["kind"] == kind).count();
    assert_eq!((kinds("sphere"), kinds("capsule")), (5, 2));
    let critical = critical(shapes);
    assert_eq!(critical.len(), 1);
    let head = critical[0];
    assert_eq!(head["kind"], "sphere");
    assert!((head["radius"].as_f64().unwrap() - 0.44).abs() < 1e-6);
    let highest = shapes.iter().map(top).fold(f64::MIN, f64::max);
    assert_eq!(top(head), highest);
    // At rest the head's centre is 1.48 m up; posed, at the eye.
    let centre = head["centre"][1].as_f64().unwrap();
    assert!((centre - 2.0).abs() < 0.05, "head centre {centre}");
    // It floats: nothing comes within 0.3 m of the feet (at rest the lowest
    // sphere reached 0.2 m below them).
    let lowest = shapes.iter().map(bottom).fold(f64::MAX, f64::min);
    assert!((lowest - 0.3).abs() < 0.01, "bottom {lowest}");
    // The arms hang straight down from the shoulders (at rest they reached
    // 0.87 m forward along +Z).
    for arm in shapes.iter().filter(|shape| shape["kind"] == "capsule") {
        let point = |end: &str| -> [f64; 3] {
            std::array::from_fn(|axis| arm[end][axis].as_f64().unwrap())
        };
        let (a, b) = (point("a"), point("b"));
        assert!(
            (a[0] - b[0]).abs() < 0.01 && (a[2] - b[2]).abs() < 0.01,
            "{arm}"
        );
        assert!(a[1] - b[1] > 0.8, "{arm}");
    }
}

/// Every bot body is the Training Bot's model with its blend trees, so in
/// the same pose; the heroes stay at rest (their trees reach no single clip
/// keying every bone, and players are hit by the capsule).
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_bots_are_posed_and_the_heroes_rest() {
    let file = derived();
    let geometry = |body: &str| -> Vec<Value> {
        shapes(&file, body)
            .iter()
            .map(|shape| {
                let mut shape = shape.clone();
                shape.as_object_mut().unwrap().remove("critical");
                shape
            })
            .collect()
    };
    for bot in [
        "0x040000000000143E",
        "0x040000000000146E",
        "0x04000000000014A9",
        "0x04000000000014D2",
        "0x04000000000014ED",
    ] {
        assert_eq!(file[bot]["pose"], "0x0A00000000003920", "{bot}");
        assert_eq!(geometry(bot), geometry("0x0400000000001403"), "{bot}");
    }
    let heroes: Value = serde_json::from_slice(
        &std::fs::read(orpheus_testkit::runtime_dir().join("data/game_heroes_174.json")).unwrap(),
    )
    .unwrap();
    for hero in heroes.as_object().unwrap().values() {
        let body = hero["body"].as_str().unwrap();
        assert!(file[body].get("pose").is_none(), "{}", hero["name"]);
    }
}

/// Every hero body has shapes and exactly one critical one (Bastion two:
/// its health component lists a second zone); the Friendly Bots' bodies
/// list no zone, so nothing on them is critical.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn every_body_has_its_critical_shapes() {
    let file = derived();
    let heroes: Value = serde_json::from_slice(
        &std::fs::read(orpheus_testkit::runtime_dir().join("data/game_heroes_174.json")).unwrap(),
    )
    .unwrap();
    for hero in heroes.as_object().unwrap().values() {
        let body = hero["body"].as_str().unwrap();
        let shapes = shapes(&file, body);
        let expected = if hero["name"] == "Bastion" { 2 } else { 1 };
        assert!(shapes.len() >= 7, "{}", hero["name"]);
        assert_eq!(critical(shapes).len(), expected, "{}", hero["name"]);
    }
    for friendly in ["0x040000000000143E", "0x040000000000146E"] {
        let shapes = shapes(&file, friendly);
        assert_eq!(shapes.len(), 7);
        assert!(critical(shapes).is_empty());
    }
}
