use std::{collections::BTreeMap, sync::Arc};

use orpheus_script::rulesets::Rulesets;
use serde_json::json;

use super::*;

/// The nodes of each fixture graph, as the derived bundle writes them.
type Nodes = BTreeMap<u32, Vec<Json>>;

fn node(pos: u16, uid: u32, class: &str, fields: Json) -> Json {
    json!({
        "pos": pos, "class": class, "uid": uid, "rid": pos, "remote": -1, "state": null,
        "client_only": 0, "server_only": 0, "fields": fields
    })
}

fn var(id: u32, entity: bool) -> Json {
    json!({"$": "STUConfigVarDynamic", "m_identifier": id, "m_60DB8F99": u8::from(entity)})
}

fn float(value: f64) -> Json {
    json!({"$": "STUConfigVarFloat", "m_value": value})
}

fn int(value: i64) -> Json {
    json!({"$": "STUConfigVarInt", "m_value": value})
}

fn expression(opcodes: &[u8], variables: Vec<Json>, constants: Json, configs: Json) -> Json {
    json!({
        "$": "STU_919FD47C",
        "m_expression": {
            "$": "STUConfigVarExpressionData", "m_fragments": null, "m_opcodes": opcodes,
            "m_dynamicVars": variables, "m_D99EF254": constants
        },
        "m_configVars": configs
    })
}

fn plug(next: Option<u16>) -> Json {
    let links: Vec<Json> = next
        .map(|pos| json!([pos, "m_inPlug"]))
        .into_iter()
        .collect();
    json!({"$": OUTPUT_PLUG, "links": links})
}

fn set_var(pos: u16, uid: u32, variable: Json, value: Json, next: Option<u16>) -> Json {
    node(
        pos,
        uid,
        SET_VAR,
        json!({"m_out_Var": variable, "m_value": value, "m_outPlug": plug(next)}),
    )
}

fn wait(pos: u16, uid: u32, timeout: Json, next: u16) -> Json {
    node(
        pos,
        uid,
        WAIT,
        json!({
            "m_timeout": timeout,
            "m_onFinishedPlug": plug(Some(next)),
            "m_onAbortPlug": plug(None),
            "m_subgraphPlug": {"$": "STU_904BFCEC", "links": []}
        }),
    )
}

/// A capture rate: the rate times v12304, as 0EC2 uid 657 has it.
fn rate(value: f64) -> Json {
    expression(
        &[6, 0, 8, 0, 25, 0],
        vec![var(12304, false)],
        json!([value]),
        Json::Null,
    )
}

fn parameter(key: u64, value: Json) -> Json {
    json!({"$": "STU_814D595E", "m_B5051BCE": format!("0x0A40{key:012X}"), "m_value": value})
}

fn rates(key: u64, values: &[f64]) -> Json {
    let rates: Vec<Json> = values.iter().copied().map(rate).collect();
    parameter(key, json!({"$": CONFIG_LIST, "m_configVars": rates}))
}

/// A mode graph's message to its points: rates by count and the delay.
fn sender(pos: u16, uid: u32, capture: Json, decay: Json, delay: f64) -> Json {
    node(
        pos,
        uid,
        SEND_MESSAGE,
        json!({
            "m_gameMessage": {"$": "STUConfigVarGameMessage", "m_gameMessage": "0x024000000000079E"},
            "m_params": [capture, decay, parameter(DECAY_DELAY, float(delay))]
        }),
    )
}

/// The nodes this module reads, with the values and the links of 104319.
fn fixture() -> Nodes {
    let fall = expression(
        &[8, 0, 6, 0, 8, 1, 24, 25, 8, 2, 26, 0],
        vec![var(6976, false), var(7713, true), var(7714, true)],
        json!([1.0]),
        Json::Null,
    );
    let floor = expression(
        &[8, 0, 8, 1, 25, 0],
        vec![var(7713, true), var(6976, false)],
        Json::Null,
        Json::Null,
    );
    // v10354 times each config: 0F4A uid 856 has two, 0EC2 uid 658 one.
    let speed = |configs: Json| {
        let opcodes: &[u8] = if configs.as_array().map_or(0, Vec::len) == 2 {
            &[8, 0, 14, 0, 25, 14, 1, 25, 0]
        } else {
            &[8, 0, 14, 0, 25, 0]
        };
        expression(opcodes, vec![var(10354, false)], Json::Null, configs)
    };
    let ruleset = |key: u32| json!({"$": "STU_3832D36C", "m_value": var(key, true)});
    let doubled = [1.0, 1.5, 2.0];
    let sided = vec![
        wait(1, 359, float(11.6), 2),
        set_var(2, 529, var(7901, false), int(1), Some(3)),
        set_var(3, 535, var(PHASE, false), int(1), None),
        set_var(4, 1093, var(6976, false), int(5), None),
        set_var(5, 1090, var(7712, true), float(15.0), None),
        set_var(6, 1091, var(7714, true), float(15.0), None),
        set_var(7, 1092, var(7713, true), float(0.3), None),
        set_var(8, 1115, var(3393, false), fall, None),
        node(
            9,
            1117,
            CHASE_VAR,
            json!({"m_out_Var": var(7717, false), "m_destination": floor}),
        ),
        set_var(10, 525, var(10354, false), int(1), None),
        node(
            11,
            856,
            STACK,
            json!({
                "m_out_Var": var(12304, false),
                "m_value": speed(json!([ruleset(9518), {"$": CAPTURE_SPEED_NATIVE}]))
            }),
        ),
        sender(
            12,
            614,
            rates(CAPTURE_RATES, &doubled),
            rates(DECAY_RATES, &doubled),
            5.0,
        ),
    ];
    let quarter = [0.125, 0.1875, 0.25];
    let control = vec![
        wait(1, 359, float(10.9), 2),
        node(2, 570, "STU_57692020", json!({"m_outPlug": plug(Some(3))})),
        set_var(3, 535, var(PHASE, false), int(2), None),
        set_var(4, 525, var(10354, false), int(1), None),
        node(
            5,
            658,
            STACK,
            json!({"m_out_Var": var(12304, false), "m_value": speed(json!([ruleset(9518)]))}),
        ),
        sender(
            6,
            657,
            rates(CAPTURE_RATES, &quarter),
            rates(DECAY_RATES, &quarter),
            5.0,
        ),
    ];
    // One over the ruleset's capture time.
    let over_time = || {
        expression(
            &[6, 0, 14, 0, 26, 0],
            Vec::new(),
            json!([1.0]),
            json!([ruleset(10595)]),
        )
    };
    let elimination = vec![
        set_var(1, 621, var(8882, false), float(2.3), None),
        wait(2, 454, var(8882, false), 3),
        node(
            3,
            634,
            "STU_3387AB5D",
            json!({"m_truePlug": plug(None), "m_falsePlug": plug(Some(4))}),
        ),
        set_var(4, 508, var(PHASE, false), int(1), None),
        sender(
            5,
            839,
            parameter(CAPTURE_RATES, over_time()),
            parameter(DECAY_RATES, over_time()),
            2.0,
        ),
    ];
    Nodes::from([
        (SIDED_GRAPH, sided),
        (CONTROL_GRAPH, control),
        (ELIMINATION_GRAPH, elimination),
    ])
}

/// The fixture's graphs, with Control's ruleset capture speed `speed`.
fn store(nodes: Nodes, speed: f64) -> GraphStore {
    let graphs: Vec<Json> = nodes
        .into_iter()
        .map(|(index, nodes)| {
            let items = nodes
                .iter()
                .filter_map(|node| node["pos"].as_u64())
                .max()
                .unwrap_or(0)
                + 1;
            json!({
                "index": index,
                "header": {
                    "m_nodesBitCount": 1, "m_statesBitCount": 1,
                    "m_remoteSyncNodesBitCount": 1, "m_syncVarsBitCount": 1
                },
                "fields": {}, "items": items, "containers": {}, "unknown_nodes": [],
                "nodes": nodes, "states": [], "entries": [], "remote": [], "sync_vars": []
            })
        })
        .collect();
    let bundle = json!({"version": 1, "graphs": graphs, "bodies": {}, "missing": []});
    let rulesets = json!({
        "schemas": {
            "0x0A3000000000000C": {"9518": speed},
            "0x0A30000000000009": {"10595": 3}
        },
        "heroes": {},
        "modes": {
            "0x0230000000000014": ["0x0A3000000000000C"],
            "0x0230000000000017": ["0x0A3000000000000C"],
            "0x0230000000000009": ["0x0A30000000000009"]
        }
    });
    GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap())
        .unwrap()
        .with_rulesets(Arc::new(
            Rulesets::from_json(&serde_json::to_vec(&rulesets).unwrap()).unwrap(),
        ))
}

fn edit(nodes: &mut Nodes, graph: u32, uid: u32, change: impl FnOnce(&mut Json)) {
    let node = nodes
        .get_mut(&graph)
        .and_then(|nodes| nodes.iter_mut().find(|node| node["uid"] == json!(uid)))
        .expect("a fixture node");
    change(node);
}

/// The graphs give single-precision numbers.
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "{actual}, expected {expected}"
    );
}

fn assert_rates(actual: CaptureRates, capture: [f64; 3], delay: f64) {
    for (actual, expected) in actual.capture.into_iter().zip(capture) {
        assert_close(actual, expected);
    }
    assert_eq!(
        actual.decay, actual.capture,
        "the decay rates are the capture rates"
    );
    assert_close(actual.decay_delay, delay);
}

/// The values 104319 gives: the wait after a round per mode, the fuse,
/// and the capture rates of Assault and Hybrid, Control and Elimination.
fn assert_104319(values: &ModeGraphValues) {
    assert_close(values.sided_round_end, 11.6);
    assert_close(values.control_round_end, 10.9);
    assert_close(values.elimination_round_end, 2.3);
    let Fuse {
        full,
        steady,
        shortening,
        floor,
    } = values.fuse;
    for (actual, expected) in [
        (full, 5.0),
        (steady, 15.0),
        (shortening, 15.0),
        (floor, 0.3),
    ] {
        assert_close(actual, expected);
    }
    assert_rates(values.sided_capture, [1.0, 1.5, 2.0], 5.0);
    assert_rates(values.control_capture, [0.125, 0.1875, 0.25], 5.0);
    assert_rates(values.elimination_capture, [1.0 / 3.0; 3], 2.0);
}

#[test]
fn reads_the_round_ends_the_fuse_and_the_capture_rates() {
    let values = ModeGraphValues::read(&store(fixture(), 1.0)).unwrap();
    assert_104319(&values);
    assert_eq!(values.capture(Mode::Hybrid), values.sided_capture);
    assert_eq!(values.capture(Mode::Control), values.control_capture);
    assert_eq!(
        values.capture(Mode::Elimination),
        values.elimination_capture
    );
}

#[test]
fn the_rulesets_capture_speed_scales_the_capture_rates() {
    let values = ModeGraphValues::read(&store(fixture(), 2.0)).unwrap();
    assert_rates(values.sided_capture, [2.0, 3.0, 4.0], 5.0);
    assert_rates(values.control_capture, [0.25, 0.375, 0.5], 5.0);
    assert_rates(values.elimination_capture, [1.0 / 3.0; 3], 2.0);
}

#[test]
fn each_mode_waits_its_own_round_end() {
    let values = ModeGraphValues::read(&store(fixture(), 1.0)).unwrap();
    for (mode, expected) in [
        (Mode::Assault, 11.6),
        (Mode::Escort, 11.6),
        (Mode::Hybrid, 11.6),
        (Mode::Control, 10.9),
        (Mode::Elimination, 2.3),
        // No wait after a round in their graphs: Assault's (assumption).
        (Mode::Deathmatch, 11.6),
        (Mode::TeamDeathmatch, 11.6),
        (Mode::Skirmish, 11.6),
    ] {
        assert_close(values.round_end(mode), expected);
    }
}

#[test]
fn the_fuse_is_full_then_falls_evenly_to_its_floor() {
    let fuse = Fuse {
        full: 5.0,
        steady: 15.0,
        shortening: 15.0,
        floor: 0.3,
    };
    for (elapsed, seconds) in [
        (0.0, 5.0),
        (15.0, 5.0),
        (20.0, 5.0 - 3.5 / 3.0),
        (22.5, 3.25),
        (30.0, 1.5),
        (60.0, 1.5),
    ] {
        assert_close(fuse.seconds(elapsed), seconds);
    }
}

/// A changed fixture: what changed, and the graph and node uid it changes.
type Change = (&'static str, u32, u32, fn(&mut Json));

#[test]
fn data_that_no_longer_matches_is_an_error() {
    let cases: [Change; 8] = [
        ("a wait became a state", CONTROL_GRAPH, 359, |node| {
            node["class"] = json!("STUStatescriptState");
        }),
        ("a wait leads elsewhere", SIDED_GRAPH, 359, |node| {
            node["fields"]["m_onFinishedPlug"] = plug(None);
        }),
        (
            "the phase node writes another variable",
            ELIMINATION_GRAPH,
            508,
            |node| {
                node["fields"]["m_out_Var"] = var(60, false);
            },
        ),
        (
            "a fuse number writes another variable",
            SIDED_GRAPH,
            1092,
            |node| {
                node["fields"]["m_out_Var"] = var(7999, true);
            },
        ),
        (
            "the fuse no longer falls evenly",
            SIDED_GRAPH,
            1115,
            |node| {
                node["fields"]["m_value"]["m_expression"]["m_D99EF254"] = json!([2.0]);
            },
        ),
        ("two rates instead of three", CONTROL_GRAPH, 657, |node| {
            node["fields"]["m_params"][1] = rates(DECAY_RATES, &[0.125, 0.25]);
        }),
        ("a rate is zero", SIDED_GRAPH, 614, |node| {
            node["fields"]["m_params"][0] = rates(CAPTURE_RATES, &[0.0, 1.5, 2.0]);
        }),
        ("no decay delay", ELIMINATION_GRAPH, 839, |node| {
            node["fields"]["m_params"]
                .as_array_mut()
                .unwrap()
                .truncate(2);
        }),
    ];
    for (case, graph, uid, change) in cases {
        let mut nodes = fixture();
        edit(&mut nodes, graph, uid, change);
        let error = ModeGraphValues::read(&store(nodes, 1.0)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{case}: {error}");
    }
    let mut nodes = fixture();
    nodes.remove(&ELIMINATION_GRAPH);
    assert!(
        ModeGraphValues::read(&store(nodes, 1.0)).is_err(),
        "a missing graph"
    );
}

#[test]
#[ignore = "requires the derived 104319 graph bundle and rulesets"]
fn the_install_gives_the_104319_values() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap();
    assert_104319(&ModeGraphValues::read(&graphs).unwrap());
}
