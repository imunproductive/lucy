//! The objectives' component data derived from the install
//! (`data/game_objectives_174.json`).

use std::sync::LazyLock;

use serde_json::{Value, json};

/// The derivation's output, read once per test.
static DERIVED: LazyLock<Value> =
    LazyLock::new(|| lucy_testkit::runtime_json("data/game_objectives_174.json"));

fn derived() -> &'static Value {
    &DERIVED
}

const ANUBIS: &str = "0x080000000000005B";
const KINGS_ROW: &str = "0x08000000000000D4";
const HANAMURA: &str = "0x0800000000000165";
const DORADO: &str = "0x08000000000002C3";
const NEPAL: &str = "0x08000000000004B7";

fn map(key: &str) -> &'static Value {
    &derived()["maps"][key]
}

/// (placeable, capture s, decay s, boxes) of each point of a map.
fn points(key: &str) -> Vec<(u64, f64, f64, usize)> {
    map(key)["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| {
            (
                point["placeable"].as_u64().unwrap(),
                point["capture"].as_f64().unwrap(),
                point["decay"].as_f64().unwrap(),
                point["boxes"].as_array().unwrap().len(),
            )
        })
        .collect()
}

/// (order, competitive, rounds 3+, without Competitive rules) by order.
fn times(key: &str) -> Vec<(u64, f64, f64, f64)> {
    let mut times: Vec<_> = map(key)["objectives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|objective| {
            (
                objective["objective"].as_u64().unwrap(),
                objective["competitive"].as_f64().unwrap(),
                objective["competitive_extra"].as_f64().unwrap(),
                objective["quick_play"].as_f64().unwrap(),
            )
        })
        .collect();
    times.sort_by_key(|time| time.0);
    times
}

/// A waypoint with an event or the barrier flag: its stored distance, its
/// pause, whether it sends the payload on alone, and the barrier flag.
type Stop = (f64, Option<f64>, bool, bool);

/// The track's stops and its stored length.
fn track_stops(payload: &Value) -> (Vec<Stop>, f64) {
    let mut distance = 0.0;
    let mut stops = Vec::new();
    for waypoint in payload["track"].as_array().unwrap() {
        let event = &waypoint["event"];
        let barrier = waypoint["barrier"].as_bool().unwrap();
        if !event.is_null() || barrier {
            stops.push((
                (distance * 10.0_f64).round() / 10.0,
                event["pause"].as_f64(),
                event["auto_next"].as_bool().unwrap_or(false),
                barrier,
            ));
        }
        distance += waypoint["length"].as_f64().unwrap_or(0.0);
    }
    (stops, distance)
}

/// Assault points: their 0E8E base times and box counts, and the times
/// they add (refs `CaptureAreas.md` §2, `Payloads.md` §1): Anubis and
/// Hanamura A 30 s, B 18 s with a 22 s decay; 4:00, then +3:00 (+0:30 in
/// extra rounds) with or without Competitive rules.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn assault_points_have_their_times_and_boxes() {
    assert_eq!(points(ANUBIS), [(818, 30.0, 30.0, 3), (843, 18.0, 22.0, 3)]);
    assert_eq!(
        points(HANAMURA),
        [(487, 30.0, 30.0, 3), (1034, 18.0, 22.0, 3)]
    );
    for key in [ANUBIS, HANAMURA] {
        assert_eq!(
            times(key),
            [(0, 240.0, 0.0, 240.0), (1, 180.0, 30.0, 180.0)]
        );
    }
    assert!(map(ANUBIS).get("payloads").is_none());
}

/// King's Row: its point A (32 s), the times (Competitive 4:00, +2:30,
/// +1:30; without 5:00, +3:00, +2:00) and its payload: no speed factor
/// override (1), 4 + 5 s of waits, the unlock run to the barrier at 8.1 m,
/// the 6 s pause at checkpoint 1 (114.7 m).
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn kings_row_has_its_point_times_and_payload_track() {
    assert_eq!(points(KINGS_ROW), [(1182, 32.0, 32.0, 1)]);
    assert_eq!(
        times(KINGS_ROW),
        [
            (0, 240.0, 0.0, 300.0),
            (1, 150.0, 0.0, 180.0),
            (2, 90.0, 0.0, 120.0)
        ]
    );
    let payloads = map(KINGS_ROW)["payloads"].as_array().unwrap();
    assert_eq!(payloads.len(), 1);
    let payload = &payloads[0];
    assert_eq!(payload["placeable"], 655);
    assert_eq!(payload["speed"], 1.0);
    assert_eq!(payload["unlock"], json!([4.0, 5.0]));
    assert_eq!(payload["auto_start"], true);
    assert_eq!(payload["track"].as_array().unwrap().len(), 20);
    let (stops, _) = track_stops(payload);
    assert_eq!(
        stops,
        [
            (8.1, Some(0.0), false, true),
            (114.7, Some(6.0), false, true),
            (185.0, Some(0.0), false, false)
        ]
    );
}

/// Dorado: speed factor 1, no waits; checkpoint 0 pauses 15 s, and the
/// waypoint at 199.2 m pauses 5 s and sends the payload on alone to the
/// next event at 211.5 m.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn dorado_has_its_pauses_and_its_run_of_its_own() {
    assert!(map(DORADO).get("points").is_none());
    assert_eq!(
        times(DORADO),
        [
            (0, 240.0, 0.0, 300.0),
            (1, 150.0, 0.0, 180.0),
            (2, 90.0, 0.0, 120.0)
        ]
    );
    let payload = &map(DORADO)["payloads"][0];
    assert_eq!(payload["speed"], 1.0);
    assert_eq!(payload["unlock"], json!([0.0, 0.0]));
    assert_eq!(payload["auto_start"], false);
    let (stops, length) = track_stops(payload);
    assert_eq!(
        stops,
        [
            (86.3, Some(15.0), false, true),
            (182.6, Some(0.0), false, true),
            (199.2, Some(5.0), true, true),
            (211.5, Some(0.0), false, true),
            (268.4, Some(0.0), false, false)
        ]
    );
    assert!((length - 268.38).abs() < 0.01, "{length}");
}

/// Control: Nepal's points keep 0E8E's default times (1 s, scaled by the
/// mode's rates) and have 2, 7 and 1 boxes.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn control_points_keep_the_default_times() {
    assert_eq!(
        points(NEPAL),
        [
            (1207, 1.0, 1.0, 2),
            (2556, 1.0, 1.0, 7),
            (2887, 1.0, 1.0, 1)
        ]
    );
    assert!(map(NEPAL).get("objectives").is_none());
}

/// Every playable map's objectives have their data; only a menu map's
/// payload, whose track isn't on the map, has none.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn only_a_menu_map_lacks_its_objective_data() {
    let failed: Vec<&String> = derived()["maps"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, map)| map.get("error").is_some())
        .map(|(key, _)| key)
        .collect();
    assert_eq!(failed, ["0x0800000000000CAC"]);
}
