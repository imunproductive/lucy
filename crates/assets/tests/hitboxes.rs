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
/// capsules; its head sphere, the top of the body at 1.92 m of its 2 m
/// mover, is the only critical one.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_training_bot_has_seven_shapes_and_a_critical_head() {
    let file = derived();
    let bot = &file["0x0400000000001403"];
    assert_eq!(bot["name"], "Training Bot");
    assert_eq!(bot["model"], "0x0D0000000000312E");
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
    assert!((highest - 2.0).abs() < 0.1, "top {highest}");
    // The arms reach forward (+Z) from the shoulders.
    for arm in shapes.iter().filter(|shape| shape["kind"] == "capsule") {
        let z = |point: &str| arm[point][2].as_f64().unwrap();
        assert!(z("a").max(z("b")) > 0.8, "{arm}");
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
