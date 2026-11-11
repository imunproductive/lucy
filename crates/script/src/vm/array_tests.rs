//! The array actions and the assignment reads of SetVar and SetVars, as
//! leafall903/ORPHEUS (MIT), commit c239a84, has them
//! (`docs/ARRAY_ACTIONS.txt`, `docs/VARIABLE_ASSIGNMENTS.txt`).

use super::*;
use serde_json::json;

const MARK: u32 = 999;

fn var(id: u32) -> Json {
    json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":1})
}

fn int(value: i64) -> Json {
    json!({"$":"STUConfigVarInt","m_value":value})
}

fn list(configs: &[Json]) -> Json {
    json!({"$":"STU_06610EBC","m_configVars":configs})
}

fn insert(output: Json, value: Json) -> (&'static str, Json) {
    (
        "STU_4C3509FB",
        json!({"m_26E3B366":output, "m_value":value}),
    )
}

fn set(output: u32, value: Json) -> (&'static str, Json) {
    (
        "STUStatescriptActionSetVar",
        json!({"m_out_Var":var(output), "m_value":value}),
    )
}

/// Runs the actions in order from the entry, then marks #999 true; returns
/// the snapshot.
fn run(actions: Vec<(&str, Json)>, variables: Variables) -> Snapshot {
    let count = actions.len() as u16;
    let mut nodes = vec![json!({
        "pos":0, "class":"STUStatescriptEntry", "uid":0, "rid":0, "remote":-1,
        "state":null, "client_only":false, "server_only":false,
        "fields":{"m_outPlug":{"$":"Plug","links":[[1,"m_inPlug"]],"m_FF3DAF1E":[]}},
    })];
    let mark = set(MARK, json!(true));
    for (position, (class, mut fields)) in (1..).zip(actions.into_iter().chain([mark])) {
        fields["m_ADEB6E05"] = json!(true);
        fields["m_2BBEEAB8"] = json!(false);
        if position <= count {
            fields["m_outPlug"] =
                json!({"$":"Plug","links":[[position + 1,"m_inPlug"]],"m_FF3DAF1E":[]});
        }
        nodes.push(json!({
            "pos":position, "class":class, "uid":position, "rid":position, "remote":-1,
            "state":null, "client_only":false, "server_only":false, "fields":fields,
        }));
    }
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1, "bodies":{}, "missing":[],
            "graphs":[{
                "index":42, "items":nodes.len(), "nodes":nodes, "states":[], "entries":[0],
                "remote":[], "sync_vars":[], "containers":{}, "unknown_nodes":[], "fields":{},
                "header":{
                    "m_nodesBitCount":5, "m_statesBitCount":4,
                    "m_remoteSyncNodesBitCount":4, "m_syncVarsBitCount":4
                }
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    initialize(
        &graphs,
        0xa000_0101,
        variables,
        vec![Root::new(1, 42)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap()
}

fn ints(values: &[i64]) -> Value {
    Value::Array(values.iter().copied().map(Value::Integer).collect())
}

/// ClearArray assigns a fresh empty array, whatever the variable held; a
/// literal output or identifier 0 changes nothing; each follows its plug.
#[test]
fn clear_array_assigns_a_fresh_empty_array() {
    let clear = |output: Json| ("STU_1C2CC6D5", json!({"m_26E3B366":output}));
    let snapshot = run(
        vec![
            clear(var(100)),
            clear(var(101)),
            clear(var(102)),
            clear(int(3)),
            clear(var(0)),
            clear(Json::Null),
        ],
        Variables::from([(100, ints(&[1, 2])), (101, Value::Integer(5))]),
    );
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert!(
        snapshot.assumptions.is_empty(),
        "{:?}",
        snapshot.assumptions
    );
    for id in [100, 101, 102] {
        assert_eq!(snapshot.variables[&id], Value::Array(Vec::new()), "#{id}");
    }
    assert_eq!(snapshot.variables[&MARK], Value::Bool(true));
}

/// The insert appends without an index, pads past the end with int 0,
/// inserts each row in turn inside the array, leaves out equal rows with
/// the unique flag (the padding too), and starts a scalar destination
/// empty. 4C3509FB opens arrays into rows and skips null configs and
/// values; CBE8BDAA inserts its one value as an assignment reads it.
#[test]
fn array_insert_appends_pads_and_inserts_rows() {
    let indexed = |output: u32, index: i64, value: Json| {
        (
            "STU_4C3509FB",
            json!({"m_26E3B366":var(output), "m_index":int(index), "m_value":value}),
        )
    };
    let unique = |(class, mut fields): (&'static str, Json)| {
        fields["m_8AD67B20"] = json!(1);
        (class, fields)
    };
    let one = |output: u32, value: Json| {
        (
            "STU_CBE8BDAA",
            json!({"m_26E3B366":var(output), "m_value":value}),
        )
    };
    let nested = Value::Array(vec![Value::Integer(3), Value::Null, ints(&[4])]);
    let snapshot = run(
        vec![
            insert(
                var(100),
                list(&[
                    int(1),
                    Json::Null,
                    json!({"$":"STUConfigVarFloat","m_value":2.5}),
                    var(103),
                    var(104),
                ]),
            ),
            indexed(105, 3, int(9)),
            indexed(106, 1, list(&[int(7), int(8)])),
            unique(insert(var(107), list(&[int(2), int(3), int(3)]))),
            unique(indexed(108, 2, int(0))),
            insert(var(109), int(1)),
            indexed(112, -4, int(5)),
            one(110, var(103)),
            one(111, list(&[int(4), int(5)])),
        ],
        Variables::from([
            (103, nested.clone()),
            (105, ints(&[1])),
            (106, ints(&[1, 2, 3])),
            (107, ints(&[1, 2])),
            (109, Value::Integer(5)),
            (110, ints(&[1])),
            (112, ints(&[1])),
        ]),
    );
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    let variables = &snapshot.variables;
    assert_eq!(
        variables[&100],
        Value::Array(vec![
            Value::Integer(1),
            Value::Float(2.5),
            Value::Integer(3),
            Value::Null,
            Value::Integer(4),
        ])
    );
    assert_eq!(variables[&105], ints(&[1, 0, 0, 9]));
    assert_eq!(variables[&106], ints(&[1, 7, 8, 2, 3]));
    assert_eq!(variables[&107], ints(&[1, 2, 3]));
    assert_eq!(variables[&108], ints(&[0, 0]));
    assert_eq!(variables[&109], ints(&[1]));
    assert_eq!(variables[&112], ints(&[1, 5]));
    assert_eq!(
        variables[&110],
        Value::Array(vec![Value::Integer(1), nested])
    );
    assert_eq!(variables[&111], Value::Array(vec![ints(&[4, 5])]));
    assert_eq!(variables[&MARK], Value::Bool(true));
}

/// A later config reads the rows the earlier ones inserted; reaching the
/// destination itself while it has rows ends the loop (with no rows yet it
/// gives none).
#[test]
fn array_insert_sees_its_own_rows_and_stops_at_the_destination() {
    let holds_six = json!({"$":"STU_45EDF12D","m_814F8EC3":var(100),"m_value":int(6)});
    let snapshot = run(
        vec![insert(
            var(100),
            list(&[var(100), int(6), holds_six, var(100), int(7)]),
        )],
        Variables::new(),
    );
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(
        snapshot.variables[&100],
        Value::Array(vec![Value::Integer(6), Value::Bool(true)])
    );
}

/// What upstream leaves to the native code is refused, not guessed: an
/// index past 200, a keyed row and an output that isn't a variable. A key
/// the native code rejects, a literal output and a missing value change
/// nothing and follow the plug.
#[test]
fn array_insert_skips_or_refuses_what_it_cannot_store() {
    let keyed = |key: Json| {
        (
            "STU_4C3509FB",
            json!({"m_26E3B366":var(100), "m_key":key, "m_value":int(1)}),
        )
    };
    let snapshot = run(
        vec![
            keyed(int(4)),
            insert(int(2), int(1)),
            insert(var(100), Json::Null),
        ],
        Variables::new(),
    );
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert!(!snapshot.variables.contains_key(&100));
    assert_eq!(snapshot.variables[&MARK], Value::Bool(true));
    for (action, issue) in [
        (
            (
                "STU_4C3509FB",
                json!({"m_26E3B366":var(100), "m_index":int(201), "m_value":int(1)}),
            ),
            "array insert index past the native limit",
        ),
        (
            keyed(json!({"$":"STUConfigVarIdentifier","m_identifier":"0x0D80000000000001"})),
            "keyed array insert",
        ),
        (
            insert(json!({"$":"STU_B5A0CAF0"}), int(1)),
            "array insert output",
        ),
    ] {
        let snapshot = run(vec![action], Variables::new());
        assert_eq!(
            snapshot
                .diagnostics
                .iter()
                .map(|diagnostic| &diagnostic.issue)
                .collect::<Vec<_>>(),
            [&Issue::UnsupportedField(issue)]
        );
        assert!(!snapshot.variables.contains_key(&MARK));
    }
}

/// SetVar and SetVars read a config list as one array. SetVar with no
/// value stores nothing; a SetVars pair with no value stores int 0.
#[test]
fn assignments_collect_config_lists_and_handle_missing_values() {
    let snapshot = run(
        vec![
            set(100, list(&[int(1), list(&[int(2), int(3)])])),
            set(101, Json::Null),
            (
                "STU_12F5D52D",
                json!({"m_143D4C5B":[
                    {"m_out_Var":var(102), "m_value":null},
                    {"m_out_Var":var(103), "m_value":list(&[])},
                    {"m_out_Var":null, "m_value":int(1)},
                ]}),
            ),
        ],
        Variables::new(),
    );
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(snapshot.variables[&100], ints(&[1, 2, 3]));
    assert!(!snapshot.variables.contains_key(&101));
    assert_eq!(snapshot.variables[&102], Value::Integer(0));
    assert_eq!(snapshot.variables[&103], Value::Array(Vec::new()));
    assert_eq!(snapshot.variables[&MARK], Value::Bool(true));
}
