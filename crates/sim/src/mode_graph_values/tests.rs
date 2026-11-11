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

fn rates(key: u64, values: [f64; 3]) -> Json {
    json!({
        "$": "STU_814D595E",
        "m_B5051BCE": format!("0x0A40{key:012X}"),
        "m_value": {"$": "STU_06610EBC", "m_configVars": values.map(rate)}
    })
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
    ];
    let speed = expression(
        &[8, 0, 14, 0, 25, 0],
        vec![var(10354, false)],
        Json::Null,
        json!([{"$": "STU_3832D36C", "m_value": var(9518, true)}]),
    );
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
            json!({"m_out_Var": var(12304, false), "m_value": speed}),
        ),
        node(
            6,
            CAPTURE_SENDER,
            SEND_MESSAGE,
            json!({
                "m_gameMessage": {"$": "STUConfigVarGameMessage", "m_gameMessage": "0x024000000000079E"},
                "m_params": [rates(0x0420, quarter), rates(0x0421, quarter)]
            }),
        ),
    ];
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
        "schemas": {"0x0A3000000000000C": {"9518": speed}},
        "heroes": {},
        "modes": {"0x0230000000000017": ["0x0A3000000000000C"]}
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

fn assert_times(actual: CaptureTimes, expected: CaptureTimes) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert_close(actual, expected);
    }
}

/// The values 104319 gives: the wait after a round per mode, the fuse,
/// and Control's capture seconds by player count.
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
    assert_times(values.control_capture, [8.0, 16.0 / 3.0, 4.0]);
}

#[test]
fn reads_the_round_ends_the_fuse_and_the_control_capture_times() {
    let values = ModeGraphValues::read(&store(fixture(), 1.0)).unwrap();
    assert_104319(&values);
}

#[test]
fn the_rulesets_capture_speed_scales_the_control_capture_times() {
    let values = ModeGraphValues::read(&store(fixture(), 2.0)).unwrap();
    assert_times(values.control_capture, [4.0, 8.0 / 3.0, 2.0]);
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
    let cases: [Change; 7] = [
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
        (
            "the two rate tables differ",
            CONTROL_GRAPH,
            CAPTURE_SENDER,
            |node| {
                node["fields"]["m_params"][1] = rates(0x0421, [0.125, 0.25, 0.25]);
            },
        ),
        ("a rate is zero", CONTROL_GRAPH, CAPTURE_SENDER, |node| {
            node["fields"]["m_params"][0] = rates(0x0420, [0.0, 0.1875, 0.25]);
            node["fields"]["m_params"][1] = rates(0x0421, [0.0, 0.1875, 0.25]);
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
