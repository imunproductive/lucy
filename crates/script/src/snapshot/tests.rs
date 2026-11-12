use super::*;
use crate::vm::{Diagnostic, EmptyWorld, Issue, MAX_INIT_WORK, Root, Site, initialize};
use serde_json::{Value as Json, json};

const ENTITY: u32 = 0xa000_0101;
const CODECS: &str = r#"{"classes":{
        "12340001":{"name":"STUStatescriptState","codec":"none","stec":false,"status":"VERIFIED","layout":[]},
        "12340002":{"name":"STUStatescriptStateBooleanSwitch","codec":"switch","stec":false,"status":"VERIFIED",
            "layout":[{"op":"bits","f":"current","n":1}]},
        "B591EBFE":{"name":"STU_B591EBFE","codec":"two_bits","stec":false,"status":"VERIFIED",
            "layout":[{"op":"bits","f":"a","n":1},{"op":"if","c":"!a",
                       "then":[{"op":"bits","f":"b","n":1}],"else":[]}]},
        "12340003":{"name":"STU_37D754C8","codec":"subscript","stec":false,"status":"VERIFIED",
            "layout":[{"op":"if","c":"H","then":[{"op":"w_u16","f":"child"},{"op":"bits","f":"flag","n":1}],"else":[]}]},
        "12340004":{"name":"STUStatescriptStateGameMessageEntry","codec":"message","stec":false,"status":"VERIFIED",
            "layout":[{"op":"if","c":"H","then":[{"op":"bits","f":"has_sender","n":1}],"else":[]},
                      {"op":"bits","f":"stacked","n":1}]},
        "12340005":{"name":"STUStatescriptStateWait","codec":"none","stec":false,"status":"VERIFIED","layout":[]},
        "594C2D80":{"name":"STUStatescriptStateStack","codec":"stack","stec":false,"status":"VERIFIED",
            "layout":[{"op":"if","c":"H","then":[{"op":"bits","f":"top","n":1},
                      {"op":"bits","f":"under","n":1}],"else":[]}]},
        "B480A974":{"name":"STUStatescriptStateLogicalButton","codec":"button","stec":true,"status":"VERIFIED",
            "layout":[{"op":"if","c":"H","then":[{"op":"bits","f":"counter","n":4}],"else":[]}]},
        "C38B92B4":{"name":"STUStatescriptStateAbility","codec":"ability","stec":false,"status":"VERIFIED",
            "layout":[{"op":"bits","f":"flags","n":8},{"op":"link","f":"link"},
                      {"op":"bits","f":"has_cur","n":1},{"op":"if","c":"has_cur",
                       "then":[{"op":"f32","f":"cur"},{"op":"bits","f":"has_rate","n":1},
                               {"op":"if","c":"has_rate","then":[{"op":"f32","f":"rate"}],"else":[]},
                               {"op":"var_b","f":"t"}],"else":[]}]},
        "2F4E2E3F":{"name":"STU_2F4E2E3F","codec":"none","stec":false,"status":"VERIFIED","layout":[]},
        "6AE5D1A1":{"name":"STUStatescriptStateChaseVar","codec":"chase","stec":false,"status":"VERIFIED",
            "layout":[{"op":"bits","f":"vector","n":1},{"op":"bits","f":"has_remaining","n":1},
                      {"op":"bits","f":"reached","n":1},{"op":"f32","f":"x"},
                      {"op":"if","c":"vector","then":[{"op":"f32","f":"y"},{"op":"f32","f":"z"}],"else":[]},
                      {"op":"if","c":"has_remaining","then":[{"op":"bits","f":"big","n":1},
                       {"op":"if","c":"big","then":[{"op":"bits","f":"remaining","n":32}],
                        "else":[{"op":"bits","f":"remaining","n":16}]}],"else":[]},
                      {"op":"bits","f":"has_last","n":1},{"op":"if","c":"has_last",
                       "then":[{"op":"var_b","f":"t"}],"else":[]}]},
        "EED6D952":{"name":"STUStatescriptStateWeaponVolley","codec":"volley","stec":true,"status":"VERIFIED",
            "layout":[{"op":"if","c":"H","then":[{"op":"signed_var","f":"t1"},
                      {"op":"bits","f":"has_offset","n":1},{"op":"if","c":"has_offset",
                       "then":[{"op":"signed_var","f":"offset"},{"op":"w_u16","f":"subindex"}],"else":[]}],
                       "else":[]},
                      {"op":"bits","f":"has_volleys","n":1},{"op":"if","c":"has_volleys",
                       "then":[{"op":"w_u16","f":"volleys"}],"else":[]},{"op":"bits","f":"counter","n":6}]},
        "D74D4F47":{"name":"STU_D74D4F47","codec":"frames","stec":false,"status":"VERIFIED",
            "layout":[{"op":"bits","f":"target","n":32}]},
        "96F1DBCF":{"name":"STU_96F1DBCF","codec":"flags13","stec":false,"status":"VERIFIED",
            "layout":[{"op":"bits","f":"low","n":6},{"op":"bits","f":"more","n":1},
                      {"op":"if","c":"more","then":[{"op":"bits","f":"high","n":7}],"else":[]}]}
    }}"#;

fn node(position: u16, class: &str, state: Option<u16>, mut fields: Json) -> Json {
    fields["m_2BBEEAB8"] = json!(state.is_some());
    fields["m_ADEB6E05"] = json!(false);
    json!({
        "pos":position, "class":class, "uid":position, "rid":position,
        "remote":-1, "state":state, "client_only":false, "server_only":false,
        "fields":fields,
    })
}

fn graph(
    index: u32,
    nodes: Vec<Json>,
    states: Json,
    entries: Json,
    remote: Json,
    sync: Json,
) -> Json {
    json!({
        "index":index, "items":nodes.len(), "nodes":nodes, "states":states,
        "entries":entries, "remote":remote, "sync_vars":sync, "containers":{},
        "unknown_nodes":[], "fields":{"m_9CEC6985":["0x0D800000000001A4"]},
        "header":{
            "m_nodesBitCount":4, "m_statesBitCount":4,
            "m_remoteSyncNodesBitCount":4, "m_syncVarsBitCount":4
        }
    })
}

fn fixture() -> (GraphStore, Snapshot) {
    let root = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"],[2,"m_beginPlug"],
                        [3,"m_beginPlug"],[4,"m_beginPlug"]],"m_FF3DAF1E":[]}
                }),
            ),
            node(
                1,
                "STUStatescriptStateBooleanSwitch",
                Some(0),
                json!({
                    "m_condition":{"$":"STUConfigVarBool","m_value":true}
                }),
            ),
            node(
                2,
                "STU_37D754C8",
                Some(1),
                json!({
                    "m_36F7B64C":{"m_36F7B64C":{"m_C974E369":{"m_graph":"0x058000000000002B"}}}
                }),
            ),
            node(
                3,
                "STUStatescriptState",
                Some(2),
                json!({"m_45216F79":null}),
            ),
            node(4, "STUStatescriptStateGameMessageEntry", Some(3), json!({})),
        ],
        json!([1, 2, 3, 4]),
        json!([0]),
        json!([null, 3, 1, 2, 4, 1]),
        json!([[100, 0, 0], [100, 0, 0], [200, 1, 0], [null, 0, 0]]),
    );
    let child = graph(43, vec![], json!([]), json!([]), json!([]), json!([]));
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1, "graphs":[root,child], "bodies":{}, "missing":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let entity = Variables::from([
        (7, Value::Asset(0x0580_0000_0000_0042)),
        (55, Value::Integer(-12)),
        (253, Value::Float(200.0)),
        (420, Value::Integer(666)),
    ]);
    let mut root = Root::new(1, 42);
    root.variables.extend([
        (100, Value::Integer(17)),
        (200, Value::Bool(true)),
        (300, Value::Float(3.5)),
        (400, Value::Null),
        (420, Value::Integer(2)),
    ]);
    let snapshot = initialize(
        &graphs,
        ENTITY,
        entity,
        vec![root],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    (graphs, snapshot)
}

fn check(bits: &BitWriter, length: usize, hex: &str) {
    let actual: String = bits
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!((bits.bit_len(), actual.as_str()), (length, hex));
}

#[test]
fn baseline_matches_python_for_both_views_and_enters_the_acknowledged_stream() {
    let (graphs, snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(&snapshot, View::Owner).unwrap();
    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    assert_eq!(owner.entity(), ENTITY);
    assert_eq!(owner.instance_count(), 2);
    assert_eq!(owner.instances[&2].parent, Some((1, 1)));
    assert_eq!(
        owner.variables.values.keys().copied().collect::<Vec<_>>(),
        [7, 55]
    );
    assert_eq!(
        owner.instances[&1]
            .variables
            .values
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [100, 200, 300]
    );
    assert!(owner.instances[&1].states.contains_key(&2));
    assert!(!observer.instances[&1].states.contains_key(&2));

    check(
        &encoder.full(&owner).unwrap(),
        250,
        "081500102b00142000e820031610e246e8012811840cc5128000006040000000",
    );
    check(
        &encoder.full(&observer).unwrap(),
        257,
        "081500102b00142000e820031610e246e8012811840cc512800000604000f83b00",
    );
    let mut stream = BodyStream::new(ENTITY);
    let chunk = encoder.queue_full(&mut stream, &owner).unwrap();
    assert_eq!(chunk.update.entity, ENTITY);
    assert!(chunk.resend);
    assert!(!stream.ready_to_send(chunk.last));
    assert!(stream.base().is_none());
    check(
        chunk.update.chunk.as_ref().unwrap(),
        286,
        "000004a08f500100b102400102800e326001216e841e801241c8502c0108000006040000",
    );
    stream.create_arrived();
    assert!(stream.ready_to_send(chunk.last));
    assert!(stream.arrived(chunk.last).advanced);
    let (_, base) = stream.base().unwrap();
    assert_eq!(base.view(), View::Owner);
    assert!(base.instances.values().all(|item| item.states.is_empty()));
    assert!(!owner.instances[&1].states.is_empty());
}

/// The authored typed pairs of graph `05AD` (variable 29859) go out in the
/// type-12 pair form (104319 D57899, leafall903/ORPHEUS (MIT)
/// `_write_pair_array`), like any other variable.
#[test]
fn typed_pairs_go_out_in_the_pair_array_form() {
    use crate::wire::{ValueEncoding, write_value};

    let (graphs, mut snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let pairs = Value::Pairs(vec![(
        Value::Asset(0x0a90_0000_0000_0171),
        Value::Asset(0x0240_0000_0000_05ad),
    )]);
    snapshot.instances[&1].variables.insert(500, pairs.clone());
    let with = encoder.project(&snapshot, View::Owner).unwrap();
    assert_eq!(with.instances[&1].variables.values.get(&500), Some(&pairs));
    let frame = encoder.full(&with).unwrap();
    let mut encoded = BitWriter::new();
    write_value(&mut encoded, &pairs, ValueEncoding::Compact).unwrap();
    // The value's bits appear in the frame (byte search at every shift).
    let needle: Vec<bool> = (0..encoded.bit_len())
        .map(|bit| encoded.as_bytes()[bit / 8] >> (bit % 8) & 1 == 1)
        .collect();
    let hay: Vec<bool> = (0..frame.bit_len())
        .map(|bit| frame.as_bytes()[bit / 8] >> (bit % 8) & 1 == 1)
        .collect();
    assert!(hay.windows(needle.len()).any(|window| window == needle));
}

#[test]
fn unsupported_active_states_and_initialization_diagnostics_are_explicit() {
    let (graphs, mut snapshot) = fixture();
    let mut raw: Json = serde_json::from_str(CODECS).unwrap();
    raw["classes"].as_object_mut().unwrap().remove("12340001");
    let codecs = CodecTable::from_json(&raw.to_string()).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    assert!(matches!(
        encoder.project(&snapshot, View::Owner),
        Err(SnapshotError::UnsupportedState {
            site: StateSite {
                instance: 1,
                graph: 42,
                state: 2
            },
            ..
        })
    ));
    assert!(encoder.project(&snapshot, View::Observer).is_ok());
    // Release builds leave the unwritable state out, as the Python reference
    // did, and send the rest.
    let tolerant =
        SnapshotEncoder::new(&graphs, &codecs).with_unknowns(crate::vm::Unknowns::Tolerate);
    let projected = tolerant.project(&snapshot, View::Owner).unwrap();
    assert!(!projected.instances[&1].states.contains_key(&2));
    assert!(!projected.instances[&1].states.is_empty());
    snapshot.diagnostics.push(Diagnostic {
        site: Site {
            entity: ENTITY,
            instance: 1,
            graph: 42,
            node: None,
        },
        issue: Issue::UnsupportedAction("fixture".into()),
    });
    assert!(matches!(
        encoder.project(&snapshot, View::Observer),
        Err(SnapshotError::Incomplete { entity: ENTITY, diagnostics: 1, first })
            if first.contains("fixture")
    ));
}

#[test]
fn stack_projection_rejects_missing_values_and_invalid_owner_binding_sources() {
    let (graphs, mut snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let baseline = encoder.project(&snapshot, View::Owner).unwrap();
    snapshot.bindings.insert(
        (0, 55),
        vec![crate::vm::StackBinding {
            instance: 1,
            raw_state: 2,
            slot: 0,
            priority: crate::expr::StackPriority {
                level: 0,
                above_equals: true,
            },
        }],
    );
    for view in [View::Owner, View::Observer] {
        assert!(matches!(
            encoder.project(&snapshot, view),
            Err(SnapshotError::MissingBoundValue {
                owner: 0,
                variable: 55,
            })
        ));
    }
    snapshot
        .resolved_bindings
        .insert((0, 55), Value::Bool(true));
    assert!(matches!(
        encoder.project_next(&snapshot, &baseline),
        Err(SnapshotError::InvalidBinding {
            owner: 0,
            variable: 55,
            ..
        })
    ));
}

#[test]
fn initial_owner_delta_matches_python_stack_bindings_and_payloads() {
    let entry = |targets: &[u16]| {
        let links: Vec<_> = targets.iter().map(|&id| (id, "m_beginPlug")).collect();
        node(
            0,
            "STUStatescriptEntry",
            None,
            json!({"m_outPlug":{"$":"Plug","links":links}}),
        )
    };
    let stack = |position, raw, variable, entity, value, priority: Option<&str>| {
        node(
            position,
            "STUStatescriptStateStack",
            Some(raw),
            json!({
                "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":variable,"m_60DB8F99":entity},
                "m_value":value,
                "m_priority":{"m_priority":priority},
            }),
        )
    };
    let mut root = Root::new(1, 42);
    root.variables.insert(100, Value::Integer(9));
    let main = graph(
        42,
        vec![
            entry(&[1, 2, 3, 4]),
            stack(1, 0, 100, false, 17, Some("0x0C40000000000001")),
            stack(2, 1, 7, true, 31, None),
            node(
                3,
                "STUStatescriptStateLogicalButton",
                Some(2),
                json!({"m_logicalButton":{"$":"STUConfigVarLogicalButton","m_logicalButton":20}}),
            ),
            node(
                4,
                "STUStatescriptStateAbility",
                Some(3),
                json!({"m_A5F1A77B":{"$":"STUConfigVarLogicalButton","m_logicalButton":35}}),
            ),
        ],
        json!([1, 2, 3, 4]),
        json!([0]),
        json!([1, 3, 4]),
        json!([[100, 0, 1]]),
    );
    let mut other = graph(
        43,
        vec![
            entry(&[1]),
            stack(1, 0, 7, true, 24, Some("0x0C40000000000002")),
        ],
        json!([1]),
        json!([0]),
        json!([1]),
        json!([]),
    );
    other["header"]["m_statesBitCount"] = json!(6);
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1, "graphs":[main,other], "bodies":{}, "missing":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let snapshot = initialize(
        &graphs,
        ENTITY,
        Variables::from([(7, Value::Integer(4))]),
        vec![root, Root::new(2, 43)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(&snapshot, View::Owner).unwrap();
    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    // The owner gets each bound variable's own value with its bindings (the
    // client works the stacks out itself); the observer, no bindings, gets
    // what the stacks make of it.
    assert_eq!(snapshot.variables[&7], Value::Integer(4));
    assert_eq!(owner.variables.values[&7], Value::Integer(4));
    assert_eq!(
        owner.instances[&1].variables.values[&100],
        Value::Integer(9)
    );
    assert_eq!(owner.variables.bindings[&7].len(), 2);
    assert_eq!(observer.variables.values[&7], Value::Integer(31));
    assert!(observer.variables.bindings.is_empty());
    assert!(observer.instances[&1].variables.bindings.is_empty());
    // Python sent the stacked values (31, 17) with the bindings; the frames
    // are Python's bytes for those (body_full_frame, remote_full_frame, then
    // owner_delta at CmFD 1001).
    let mut python = owner.clone();
    python.variables.values[&7] = Value::Integer(31);
    python.instances.get_mut(&1).unwrap().variables.values[&100] = Value::Integer(17);
    check(
        &encoder.full(&python).unwrap(),
        111,
        "081500102b00003a900f004a0400",
    );
    check(
        &encoder.full(&observer).unwrap(),
        126,
        "081500102b00003a900f004a047f000c",
    );
    let mut stream = BodyStream::new(ENTITY);
    let full = encoder.queue_full(&mut stream, &python).unwrap();
    stream.create_arrived();
    assert!(stream.arrived(full.last).advanced);
    let (_, baseline) = stream.base().unwrap();
    assert!(baseline.variables.bindings.is_empty());
    assert!(baseline.instances[&1].variables.bindings.is_empty());
    let kind = DeltaKind::InitialOwner(1001);
    let delta = encoder
        .differences(baseline, &python, &BTreeSet::new(), &BTreeSet::new())
        .unwrap();
    assert_eq!(delta.variables.values[&7], Value::Integer(31));
    assert_eq!(
        delta.instances[&1].variables.values[&100],
        Value::Integer(17)
    );
    check(
        &encoder
            .delta_frame(&python, &delta, kind, 16, None)
            .unwrap(),
        222,
        "2a7d40af40003a908f89004e0189008012116000f0db11009b981800",
    );
    // The bindings going away resend the values.
    let mut unbound = owner.clone();
    unbound.variables.bindings.clear();
    unbound
        .instances
        .get_mut(&1)
        .unwrap()
        .variables
        .bindings
        .clear();
    let delta = encoder
        .differences(&owner, &unbound, &BTreeSet::new(), &BTreeSet::new())
        .unwrap();
    assert_eq!(delta.variables.values[&7], Value::Integer(4));
    assert_eq!(
        delta.instances[&1].variables.values[&100],
        Value::Integer(9)
    );
    assert!(delta.variables.bindings.is_empty());
    assert!(delta.instances[&1].variables.bindings.is_empty());
}

/// A Stack that adds to its own variable (Ana's `0B94` st77: `#23184e +
/// 0.65`, read by `01C7` st49's MovementMod) with the owner's client
/// modelled by this VM: it predicts the same stack and takes the value the
/// owner frame carries as the variable's own, and the stack works its entry
/// out again from the value below it (client 0x7FF78AAA5FF0). With the
/// variable's own value it keeps the server's 0.65; with the stacked value
/// (what Python sent) it would count the stack twice: 1.3.
#[test]
fn a_stack_adding_to_its_own_variable_counts_once_on_the_owners_client() {
    use crate::{
        expr::{Scope, Variable},
        vm::Component,
    };
    use std::sync::Arc;
    let adds = json!({"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[8,0,6,0,23,0],
        "m_dynamicVars":[{"$":"STUConfigVarDynamic","m_identifier":100,"m_60DB8F99":true}],
        "m_D99EF254":[0.65]
    }});
    let graphs = Arc::new(
        GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version":1, "bodies":{}, "missing":[], "graphs":[graph(
                    42,
                    vec![
                        node(
                            0,
                            "STUStatescriptEntry",
                            None,
                            json!({"m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"]]}}),
                        ),
                        node(
                            1,
                            "STUStatescriptStateStack",
                            Some(0),
                            json!({
                                "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":100,"m_60DB8F99":true},
                                "m_value":adds,
                                "m_priority":{"m_priority":null},
                            }),
                        ),
                    ],
                    json!([1]),
                    json!([0]),
                    json!([1]),
                    json!([]),
                )]
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let start = || {
        Component::start(
            Arc::clone(&graphs),
            ENTITY,
            Variables::new(),
            vec![Root::new(1, 42)],
            0,
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap()
    };
    let variable = Variable {
        scope: Scope::Entity,
        id: 100,
    };
    let server = start();
    let stacked = server.resolved_variable(1, variable).unwrap();
    assert_eq!(stacked, Value::Float(f64::from(0.65f32)));
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let owner = SnapshotEncoder::new(&graphs, &codecs)
        .project(server.snapshot().unwrap(), View::Owner)
        .unwrap();
    assert_eq!(owner.variables.bindings[&100].len(), 1);
    let sent = owner.variables.values[&100].clone();
    assert_eq!(sent, Value::Null);
    let client_after = |received: Value| {
        let mut client = start();
        client
            .set_entity_variable(100, received, &mut EmptyWorld)
            .unwrap();
        client.run_now(&mut EmptyWorld).unwrap();
        client.resolved_variable(1, variable).unwrap()
    };
    assert_eq!(client_after(sent), stacked);
    let Value::Float(twice) = client_after(stacked.clone()) else {
        panic!("a float")
    };
    assert!((twice - 1.3).abs() < 1e-6, "{twice}");
}

fn after_body_change(mut snapshot: ProjectedSnapshot) -> ProjectedSnapshot {
    snapshot.variables.values.swap_remove(&55);
    let root = snapshot.instances.get_mut(&1).unwrap();
    root.variables.values.insert(100, Value::Integer(99));
    root.variables.values.swap_remove(&200);
    root.states
        .get_mut(&0)
        .unwrap()
        .payload
        .insert("current".into(), Value::Bool(false));
    root.states.remove(&1);
    root.states.remove(&2);
    snapshot.instances.remove(&2);
    snapshot
}

#[test]
fn ability_cooldown_and_push_binding_match_python_bytes() {
    use crate::expr::StackPriority;
    use crate::vm::AbilityCooldown;

    let definition = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"]]}}),
            ),
            node(
                1,
                "STUStatescriptStateAbility",
                Some(0),
                json!({"m_A5F1A77B":{"$":"STUConfigVarLogicalButton","m_logicalButton":35}}),
            ),
        ],
        json!([1]),
        json!([0]),
        json!([1]),
        json!([]),
    );
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1,"graphs":[definition],"bodies":{},"missing":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let mut snapshot = initialize(
        &graphs,
        ENTITY,
        Variables::from([(7, Value::Bool(false))]),
        vec![Root::new(1, 42)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let state = &mut snapshot.instances[&1].states[&0];
    state.kind = StateKind::Ability { flags: 3 };
    state.ability_cooldown = Some(AbilityCooldown {
        current: 1.25,
        rate: 0.5,
        last_ms: 15_990,
    });
    snapshot.resolved_bindings.insert((0, 7), Value::Bool(true));
    snapshot.bindings.insert(
        (0, 7),
        vec![StackBinding {
            instance: 1,
            raw_state: 0,
            slot: 1,
            priority: StackPriority {
                level: 20,
                above_equals: false,
            },
        }],
    );
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(&snapshot, View::Owner).unwrap();
    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    let mut stream = BodyStream::new(ENTITY);
    stream.create_arrived();
    let full = encoder.queue_full(&mut stream, &owner).unwrap();
    stream.arrived(full.last);
    let initial = encoder
        .queue_delta(&mut stream, &owner, DeltaKind::InitialOwner(1001), 16)
        .unwrap()
        .unwrap();
    // Python owner_delta and remote_full_frame; the observer has neither
    // a pushed binding nor the owner's cooldown timestamp.
    check(
        initial.update.chunk.as_ref().unwrap(),
        245,
        "04000010add207c00a4007300928602800e0010100407f020000fc50490c00",
    );
    check(
        &encoder.full(&observer).unwrap(),
        146,
        "081500001d40003c200000e84f0000801f0000",
    );

    snapshot.bindings[&(0, 7)][0].slot = 0;
    assert!(matches!(
        encoder.project(&snapshot, View::Owner),
        Err(SnapshotError::InvalidBinding {
            owner: 0,
            variable: 7,
            ..
        })
    ));
}

#[test]
fn delayed_switch_projects_reference_bits_and_owner_timer() {
    use crate::vm::PendingTimer;

    let mut delayed = node(
        1,
        "STU_B591EBFE",
        Some(0),
        json!({"m_BD83C8E6": {"$": "STUConfigVarBool", "m_value": false}}),
    );
    delayed["remote"] = json!(0);
    let graph = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug": {"$": "Plug", "links": [[1, "m_beginPlug"]]}}),
            ),
            delayed,
        ],
        json!([1]),
        json!([0]),
        json!([1]),
        json!([]),
    );
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version": 1, "graphs": [graph], "bodies": {}, "missing": []
        }))
        .unwrap(),
    )
    .unwrap();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let codec = codecs.codec("STU_B591EBFE").unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let mut snapshot = initialize(
        &graphs,
        ENTITY,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();

    // The authorized 104319 layout gives identical state bits in owner and
    // observer records. Only the owner receives the pending branch timer.
    for (current, undecided, width, byte) in [
        (false, false, 2, 0),
        (false, true, 2, 2),
        (true, false, 1, 1),
    ] {
        snapshot.instances[&1].states[&0].kind = StateKind::DelayedSwitch { current, undecided };
        snapshot.pending_timers.clear();
        if undecided {
            snapshot.pending_timers.insert(
                1,
                vec![PendingTimer {
                    due_ms: 16_064,
                    raw_state: 0,
                    finish: false,
                    parameter: 1,
                }],
            );
        }
        let owner = encoder.project(&snapshot, View::Owner).unwrap();
        let observer = encoder.project(&snapshot, View::Observer).unwrap();
        let owner = &owner.instances[&1];
        let observer = &observer.instances[&1];
        assert_eq!(owner.events.len(), usize::from(undecided));
        if undecided {
            assert_eq!(owner.events[0].parameter, 1);
            assert_eq!(owner.events[0].due_ms, 16_064);
        }
        assert!(observer.events.is_empty());
        for (state, mode) in [
            (
                &owner.states[&0],
                PayloadMode::Owner {
                    full: false,
                    frame_time: 16_000,
                    frame_ms: 16,
                },
            ),
            (&observer.states[&0], PayloadMode::Remote { full: false }),
            (&observer.states[&0], PayloadMode::Remote { full: true }),
        ] {
            let bits = codec.payload(&state.payload, mode).unwrap();
            assert_eq!(
                (bits.bit_len(), bits.as_bytes()),
                (width, [byte].as_slice())
            );
        }
    }
}

#[test]
fn movement_sample_uses_the_owner_flags_codec() {
    struct Sample;
    impl crate::vm::World for Sample {
        fn movement_sample(&self, _: Site) -> Option<u16> {
            Some(0x842)
        }
    }

    let fixture = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"]]}
                }),
            ),
            node(1, "STU_96F1DBCF", Some(0), json!({})),
        ],
        json!([1]),
        json!([0]),
        json!([]),
        json!([]),
    );
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({"version":1,"graphs":[fixture],"bodies":{},"missing":[]}))
            .unwrap(),
    )
    .unwrap();
    let snapshot = initialize(
        &graphs,
        ENTITY,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut Sample,
        MAX_INIT_WORK,
    )
    .unwrap();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(&snapshot, View::Owner).unwrap();
    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    assert!(observer.instances[&1].states.is_empty());
    let payload = codecs
        .codec("STU_96F1DBCF")
        .unwrap()
        .payload(
            &owner.instances[&1].states[&0].payload,
            PayloadMode::Owner {
                full: true,
                frame_time: 0,
                frame_ms: 16,
            },
        )
        .unwrap();
    assert_eq!(
        (payload.bit_len(), payload.as_bytes()),
        (14, [0xc2, 0x10].as_slice())
    );
}

#[test]
fn initial_owner_and_corrections_match_python_bytes_in_both_views() {
    let (graphs, snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(&snapshot, View::Owner).unwrap();
    let mut stream = BodyStream::new(ENTITY);
    let baseline = encoder.queue_full(&mut stream, &owner).unwrap();
    stream.create_arrived();
    stream.arrived(baseline.last);
    let initial = encoder
        .queue_delta(&mut stream, &owner, DeltaKind::InitialOwner(1000), 16)
        .unwrap()
        .unwrap();
    check(
        initial.update.chunk.as_ref().unwrap(),
        127,
        "040000b0a5d007e80800f02726313100",
    );
    stream.sent(initial.last);
    stream.arrived(initial.last);
    let owner = after_body_change(owner);
    let correction = encoder
        .queue_delta(&mut stream, &owner, DeltaKind::Correction(1016), 16)
        .unwrap()
        .unwrap();
    check(
        correction.update.chunk.as_ref().unwrap(),
        181,
        "06000010e9f007c00944d0cd0f80903121ff00b8480c00",
    );
    assert!(stream.touched().contains(&ChangeKey::Instance(2)));
    assert!(stream.touched().contains(&ChangeKey::Variable(1, 200)));

    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    let mut stream = BodyStream::new(ENTITY);
    let baseline = encoder.queue_full(&mut stream, &observer).unwrap();
    stream.create_arrived();
    stream.arrived(baseline.last);
    let kind = DeltaKind::Observer { resend: false };
    assert!(
        encoder
            .queue_delta(&mut stream, &observer, kind, 16)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        encoder.queue_delta(&mut stream, &owner, kind, 16),
        Err(SnapshotError::DifferentStream)
    ));
    let observer = after_body_change(observer);
    let delta = encoder
        .queue_delta(&mut stream, &observer, kind, 16)
        .unwrap()
        .unwrap();
    check(
        delta.update.chunk.as_ref().unwrap(),
        140,
        "04000080864004ddfc00081913f20f003601",
    );
}

#[test]
fn chased_values_and_watch_match_python_owner_and_observer_frames() {
    use crate::vm::Component;
    use std::sync::Arc;

    let var = |id, entity| json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":entity});
    let vector = json!({"$":"STUConfigVarVec3","m_value":{"X":1.0,"Y":2.0,"Z":3.0}});
    let definition = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":{"$":"Plug","links":[
                    [1,"m_beginPlug"],[2,"m_beginPlug"],[3,"m_beginPlug"]
                ]}}),
            ),
            node(
                1,
                "STUStatescriptStateChaseVar",
                Some(0),
                json!({"m_out_Var":var(100,false),"m_EB0FC261":0.5,"m_destination":3.0,
                           "m_5BEFF040":0.125}),
            ),
            node(
                2,
                "STUStatescriptStateChaseVar",
                Some(1),
                json!({"m_out_Var":var(7,true),"m_EB0FC261":vector,"m_destination":vector,
                           "m_5BEFF040":70.0}),
            ),
            node(
                3,
                "STU_2F4E2E3F",
                Some(2),
                json!({"m_B7A3CF78":var(7,true)}),
            ),
        ],
        json!([1, 2, 3]),
        json!([0]),
        json!([1, 2, 3]),
        json!([[100, 0, 1]]),
    );
    let graphs = Arc::new(
        GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version":1,"graphs":[definition],"bodies":{},"missing":[]
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let mut world = EmptyWorld;
    let mut component = Component::start(
        Arc::clone(&graphs),
        ENTITY,
        Variables::new(),
        vec![Root::new(1, 42)],
        1000,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let owner = encoder
        .project(component.snapshot().unwrap(), View::Owner)
        .unwrap();
    // The same graph run by the Python VM, then written by body_full_frame,
    // owner_delta and remote_full_frame. The two durations use distinct widths.
    check(
        &encoder.full(&owner).unwrap(),
        199,
        "081500001df00100007f000000800000808000880000003f00",
    );
    let mut stream = BodyStream::new(ENTITY);
    stream.create_arrived();
    let baseline = encoder.queue_full(&mut stream, &owner).unwrap();
    stream.arrived(baseline.last);
    component.advance_frame(1001, &mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    let owner = encoder.project(snapshot, View::Owner).unwrap();
    let delta = encoder
        .queue_delta(&mut stream, &owner, DeltaKind::InitialOwner(1001), 16)
        .unwrap()
        .unwrap();
    check(
        delta.update.chunk.as_ref().unwrap(),
        364,
        "040000e82740a50f54112000620ad7e30faf14aec71f5e00051c0000e00f00000010000010d0a888008002260600",
    );
    let observer = encoder.project(snapshot, View::Observer).unwrap();
    check(
        &encoder.full(&observer).unwrap(),
        388,
        "081500001df00100007f00000080000080800088295c8f3fac14aec71f5e001e0000e00f00000010000010d0a888000003",
    );
}

#[test]
fn a_new_volley_reseeds_ammo_after_an_unacknowledged_stop() {
    const RIFLE: u16 = 11;
    const VOLLEY: u16 = 10;
    const AMMO: u32 = 53;
    let definition = graph(
        0x0254,
        vec![node(
            0,
            "STUStatescriptStateWeaponVolley",
            Some(VOLLEY),
            json!({}),
        )],
        json!([
            null, null, null, null, null, null, null, null, null, null, 0
        ]),
        json!([]),
        json!([0]),
        json!([[AMMO, 0, 1]]),
    );
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1, "graphs":[definition], "bodies":{}, "missing":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let mut root = Root::new(RIFLE, 0x0254);
    root.variables.insert(AMMO, Value::Integer(30));
    let mut snapshot = initialize(
        &graphs,
        ENTITY,
        Variables::new(),
        vec![root],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let idle = encoder.project(&snapshot, View::Owner).unwrap();
    let observer_idle = encoder.project(&snapshot, View::Observer).unwrap();
    let (_, fixture) = fixture();
    let mut state = fixture.instances[&1].states[&0].clone();
    let mut volley = |active, activation, start_frame, ammo0, ammo| {
        state.active = active;
        state.activation = activation;
        state.start_ms = u64::from(start_frame) * 16;
        state.kind = StateKind::WeaponVolley {
            volleys: 1,
            offset_ms: 0,
            ammo0,
            ammo_var: Some(Variable {
                scope: Scope::Instance,
                id: AMMO,
            }),
        };
        snapshot.instances[&RIFLE]
            .states
            .insert(VOLLEY, state.clone());
        snapshot
            .resolved_bindings
            .insert((RIFLE, AMMO), Value::Integer(ammo));
        encoder.project(&snapshot, View::Owner).unwrap()
    };
    let first = volley(true, 1, 1031_u32, 30, 29);
    let stopped = volley(false, 1, 1031_u32, 30, 29);
    let ended = volley(false, 3, 1050_u32, 28, 27);
    let second = volley(true, 2, 1042_u32, 29, 28);
    let observer_second = encoder.project(&snapshot, View::Observer).unwrap();
    let mut stream = BodyStream::new(ENTITY);
    stream.create_arrived();
    let baseline = encoder.queue_full(&mut stream, &idle).unwrap();
    stream.arrived(baseline.last);
    let initial = encoder
        .queue_delta(&mut stream, &idle, DeltaKind::InitialOwner(1001), 16)
        .unwrap()
        .unwrap();
    stream.sent(initial.last);
    stream.arrived(initial.last);

    // Python owner_delta and chunk vectors from the same minimal 0254 layout.
    let intro = ChangeKey::VolleyIntroduction(RIFLE, VOLLEY);
    let key = ChangeKey::Variable(RIFLE, AMMO);
    for (frame, state, bits, bytes, ammo, introduced, dropped) in [
        (
            1031,
            &first,
            137,
            "06000050e60e08105980e2811c0082ec0000",
            30,
            true,
            false,
        ),
        (
            1032,
            &stopped,
            137,
            "08000050e61008105980d2810c0882ec0000",
            29,
            false,
            true,
        ),
        (
            1042,
            &second,
            137,
            "08000450e62408105980d2811c0084ec0000",
            29,
            true,
            true,
        ),
        (
            1043,
            &second,
            137,
            "08000850e62608105980d2811c0884ec0000",
            29,
            true,
            false,
        ),
        (
            1044,
            &second,
            114,
            "0e0000e0e42808b45880c201d90100",
            28,
            false,
            false,
        ),
        (
            1050,
            &ended,
            137,
            "10000050e63408105980b2810c0086ec0000",
            27,
            true,
            false,
        ),
    ] {
        let chunk = encoder
            .queue_delta(&mut stream, state, DeltaKind::Correction(frame), 16)
            .unwrap()
            .unwrap();
        check(chunk.update.chunk.as_ref().unwrap(), bits, bytes);
        let sent = stream.sent(chunk.last).unwrap();
        assert_eq!(sent.changes.contains(&intro), introduced);
        assert!(sent.changes.contains(&key));
        assert_eq!(
            sent.snapshot.instances[&RIFLE].variables.values[&AMMO],
            Value::Integer(ammo)
        );
        assert!(stream.sent(chunk.last).is_none());
        if dropped {
            let lost = stream.lost(chunk.last);
            assert!(!lost.retry);
            assert_eq!(lost.owner_report, Some(false));
        } else {
            assert!(stream.arrived(chunk.last).advanced);
        }
        assert!(!stream.touched().contains(&intro));
    }

    let mut stream = BodyStream::new(ENTITY);
    stream.create_arrived();
    let baseline = encoder.queue_full(&mut stream, &observer_idle).unwrap();
    stream.arrived(baseline.last);
    let delta = encoder
        .queue_delta(
            &mut stream,
            &observer_second,
            DeltaKind::Observer { resend: false },
            16,
        )
        .unwrap()
        .unwrap();
    check(
        delta.update.chunk.as_ref().unwrap(),
        78,
        "040000a08205281c9c00",
    );
}

#[test]
fn observer_delta_removes_an_instance_introduced_by_an_unacknowledged_chunk() {
    let (graphs, snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    let mut stream = BodyStream::new(ENTITY);
    let baseline = encoder.queue_full(&mut stream, &observer).unwrap();
    stream.create_arrived();
    stream.arrived(baseline.last);
    let mut temporary = observer.clone();
    temporary
        .instances
        .insert(3, temporary.instances[&2].clone());
    let kind = DeltaKind::Observer { resend: false };
    let created = encoder
        .queue_delta(&mut stream, &temporary, kind, 16)
        .unwrap()
        .unwrap();
    check(
        created.update.chunk.as_ref().unwrap(),
        92,
        "040000808371058002040000",
    );
    stream.sent(created.last);
    stream.lost(created.last);
    let removed = encoder
        .queue_delta(&mut stream, &observer, kind, 16)
        .unwrap()
        .unwrap();
    check(removed.update.chunk.as_ref().unwrap(), 52, "04000400810900");
    stream.arrived(removed.last);
    assert!(
        encoder
            .queue_delta(&mut stream, &observer, kind, 16)
            .unwrap()
            .is_none()
    );
}

#[test]
fn owner_timer_replacement_preserves_signed_time_steps_and_fifo_ties() {
    let (graphs, snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let mut owner = encoder.project(&snapshot, View::Owner).unwrap();
    owner.instances.get_mut(&1).unwrap().states.clear();
    let mut stream = BodyStream::new(ENTITY);
    let baseline = encoder.queue_full(&mut stream, &owner).unwrap();
    stream.create_arrived();
    stream.arrived(baseline.last);
    owner.instances.get_mut(&1).unwrap().events = vec![
        ProjectedEvent {
            due_ms: 16000,
            raw_state: 0,
            finish: false,
            parameter: 1,
        },
        ProjectedEvent {
            due_ms: 16004,
            raw_state: 1,
            finish: false,
            parameter: 0,
        },
        ProjectedEvent {
            due_ms: 16004,
            raw_state: 2,
            finish: true,
            parameter: 1,
        },
        ProjectedEvent {
            due_ms: 15992,
            raw_state: 3,
            finish: false,
            parameter: 7,
        },
    ];
    let delta = encoder
        .queue_delta(&mut stream, &owner, DeltaKind::InitialOwner(1000), 16)
        .unwrap()
        .unwrap();
    check(
        delta.update.chunk.as_ref().unwrap(),
        163,
        "040000f0a7d007780189b801022084860894130300",
    );
}

#[test]
fn larger_masks_preserve_group_presence_before_slot_bits() {
    let mut bits = BitWriter::new();
    write_mask(&mut bits, 22, &BTreeSet::from([0, 8, 17]));
    check(&bits, 25, "0f081000");
}

#[test]
fn later_snapshots_keep_initial_entity_scope_exclusions_after_graph_removal() {
    let (graphs, mut snapshot) = fixture();
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let initial = encoder.project(&snapshot, View::Observer).unwrap();
    snapshot.instances.clear();
    let next = encoder.project_next(&snapshot, &initial).unwrap();
    assert!(!next.variables.values.contains_key(&420));
    assert!(!next.variables.values.contains_key(&OWNER_HEALTH));
    assert!(
        encoder
            .project(&snapshot, View::Observer)
            .unwrap()
            .variables
            .values
            .contains_key(&420)
    );
}

#[test]
fn initialized_waits_supply_owner_timers_and_remote_frame_targets() {
    let timed = graph(
        44,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"],[2,"m_beginPlug"]],
                        "m_FF3DAF1E":[]}
                }),
            ),
            node(
                1,
                "STUStatescriptStateWait",
                Some(0),
                json!({
                    "m_timeout":{"$":"STUConfigVarFloat","m_value":1.0}
                }),
            ),
            node(
                2,
                "STU_D74D4F47",
                Some(1),
                json!({
                    "m_3016B9A1":{"$":"STUConfigVarInt","m_value":3}
                }),
            ),
        ],
        json!([1, 2]),
        json!([0]),
        json!([1, 2]),
        json!([]),
    );
    let graphs = GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1, "graphs":[timed], "bodies":{}, "missing":[]
        }))
        .unwrap(),
    )
    .unwrap();
    let snapshot = initialize(
        &graphs,
        ENTITY,
        Variables::new(),
        vec![Root::new(1, 44)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(snapshot.pending_timers[&1].len(), 1);
    assert_eq!(snapshot.pending_timers[&1][0].due_ms, 1000);
    let codecs = CodecTable::from_json(CODECS).unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(&snapshot, View::Owner).unwrap();
    let observer = encoder.project(&snapshot, View::Observer).unwrap();
    assert!(observer.instances[&1].events.is_empty());
    check(&encoder.full(&observer).unwrap(), 72, "08160000fc00000000");
    let mut stream = BodyStream::new(ENTITY);
    let baseline = encoder.queue_full(&mut stream, &owner).unwrap();
    stream.create_arrived();
    stream.arrived(baseline.last);
    assert!(stream.base().unwrap().1.instances[&1].events.is_empty());
    let update = encoder
        .queue_delta(&mut stream, &owner, DeltaKind::InitialOwner(1), 16)
        .unwrap()
        .unwrap();
    check(
        update.update.chunk.as_ref().unwrap(),
        151,
        "040000302701b604801f0000004800863d3000",
    );
}

#[test]
#[ignore = "requires the derived 104319 graph and codec tables"]
fn actual_shared_ui_graph_baselines_match_python() {
    use sha2::{Digest, Sha256};

    let root = orpheus_testkit::runtime_dir();
    let graphs = GraphStore::load(root.join("data/game_graphs_174.json.gz")).unwrap();
    let codecs = CodecTable::client();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    for (graph, expected_owner, expected_observer, delta_bits, expected_delta) in [
        (
            0x20e4,
            "8b5be8f21a99210445094b65c6ba62621a489da48170c97ef9062a99127ec0f6",
            "7e3658bf64c37a631561b21b33fff4f68119e038896d153ee84d7f4cb6e29649",
            73,
            "3fee631d2eede1aa07fd6b2505734ac71f1f6c0e015524a62d04419db413a3ee",
        ),
        (
            0x0cb6,
            "6de347e8a7097efa974f5fc61cce965c69998e849c11c11a3c96165dd8408f48",
            "e62bc520a0acf2f9064de7de9b053ba46c3861c4b4c481f606e357560eca6eea",
            76,
            "3a927d71037d9d56bb2e3d0ef1a532f252df632e590875823dca787120dd5153",
        ),
    ] {
        let snapshot = initialize(
            &graphs,
            ENTITY,
            Variables::new(),
            vec![Root::new(1, graph)],
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        assert!(
            snapshot.initialization_complete(),
            "{:?}",
            snapshot.diagnostics
        );
        for (view, expected_bits, expected) in [
            (View::Owner, 37, expected_owner),
            (View::Observer, 38, expected_observer),
        ] {
            let projection = encoder.project(&snapshot, view).unwrap();
            let frame = encoder.full(&projection).unwrap();
            assert_eq!(frame.bit_len(), expected_bits);
            assert_eq!(format!("{:x}", Sha256::digest(frame.as_bytes())), expected);
        }
        let owner = encoder.project(&snapshot, View::Owner).unwrap();
        let mut stream = BodyStream::new(ENTITY);
        let baseline = encoder.queue_full(&mut stream, &owner).unwrap();
        stream.create_arrived();
        stream.arrived(baseline.last);
        let (_, base) = stream.base().unwrap();
        let delta = encoder
            .differences(base, &owner, &BTreeSet::new(), &BTreeSet::new())
            .unwrap();
        let frame = encoder
            .delta_frame(&owner, &delta, DeltaKind::InitialOwner(1000), 16, None)
            .unwrap();
        assert_eq!(frame.bit_len(), delta_bits);
        assert_eq!(
            format!("{:x}", Sha256::digest(frame.as_bytes())),
            expected_delta
        );
    }
}

#[test]
fn kill_feed_pulse_waits_for_the_world_then_stays_fresh_until_acknowledged() {
    use crate::vm::Component;
    use std::sync::Arc;

    let mut states = vec![Json::Null; 108];
    states.push(json!(1));
    let mut feed = graph(
        0x01cf,
        vec![
            node(
                0,
                "STUStatescriptEntryGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x024000000000003E"},
                    "m_outPlug":{"$":"Plug","links":[[1,"m_beginPlug"]],"m_FF3DAF1E":[]},
                }),
            ),
            node(
                1,
                "STU_B9898052",
                Some(108),
                json!({"m_B6A27F9A":{"$":"Plug","links":[[2,"m_inPlug"]]}}),
            ),
            {
                // Its client-only plug posts the kill feed line.
                let mut post = node(
                    2,
                    "STU_100EF8C2",
                    None,
                    json!({"m_gameMessage":{"m_gameMessage":"0x0240000000000765"}}),
                );
                post["client_only"] = json!(true);
                post
            },
        ],
        json!(states),
        json!([0]),
        json!([]),
        json!([]),
    );
    feed["header"]["m_statesBitCount"] = json!(8);
    let graphs = Arc::new(
        GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version":1, "graphs":[feed], "bodies":{}, "missing":[]
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let codecs = CodecTable::client();
    let mut component = Component::start(
        Arc::clone(&graphs),
        ENTITY,
        Variables::new(),
        vec![Root::new(1, 0x01cf)],
        100,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let mut stream = BodyStream::new(ENTITY);
    let full = encoder
        .queue_full(
            &mut stream,
            &encoder
                .project(component.snapshot().unwrap(), View::Owner)
                .unwrap(),
        )
        .unwrap();
    stream.create_arrived();
    stream.arrived(full.last);

    component.advance_frame(101, &mut EmptyWorld).unwrap();
    component
        .queue_message(1, 0x0240_0000_0000_003e, Variables::new())
        .unwrap();
    component.run_now(&mut EmptyWorld).unwrap();
    let pulse = |snapshot: &ProjectedSnapshot| {
        snapshot.instances[&1]
            .states
            .get(&108)
            .map(|state| (state.active, state.payload.clone()))
    };
    let payload = |fresh| {
        StatePayload::from([
            ("count".into(), Value::Integer(1)),
            ("fresh".into(), Value::Bool(fresh)),
        ])
    };
    let live = encoder
        .project(component.snapshot().unwrap(), View::Owner)
        .unwrap();
    assert_eq!(pulse(&live), Some((true, payload(true))));
    assert_eq!(graphs.graph(0x01cf).unwrap().kill_feed_pulser(), Some(108));
    // Not allowed yet: the client keeps what it acknowledged.
    assert_eq!(
        pulse(&live.clone().gate_kill_feed(&graphs, &stream, false)),
        None
    );
    let allowed = live.gate_kill_feed(&graphs, &stream, true);
    assert_eq!(pulse(&allowed), Some((true, payload(true))));
    let sent = encoder
        .queue_delta(&mut stream, &allowed, DeltaKind::InitialOwner(101), 16)
        .unwrap()
        .unwrap();

    // Later the pulse is stale, but the unacknowledged one keeps its value.
    for frame in 102..=140 {
        component.advance_frame(frame, &mut EmptyWorld).unwrap();
    }
    let stale = encoder
        .project(component.snapshot().unwrap(), View::Owner)
        .unwrap();
    assert_eq!(pulse(&stale), Some((true, payload(false))));
    assert_eq!(
        pulse(&stale.clone().gate_kill_feed(&graphs, &stream, false)),
        Some((true, payload(true)))
    );
    stream.sent(sent.last);
    stream.arrived(sent.last);
    assert_eq!(
        pulse(&stale.gate_kill_feed(&graphs, &stream, false)),
        Some((true, payload(false)))
    );
}

/// driver.py `KILL_FEED_PULSERS`: `{0x01CF: 108, 0x0C18: 8}`, found in the
/// bot bundle (the hero bundle has no `0C18`).
#[test]
#[ignore = "requires the derived 104319 graph bundles"]
fn the_kill_feed_pulsers_are_the_python_reference_table() {
    let pulsers = |path: std::path::PathBuf| {
        let graphs = GraphStore::load(path).unwrap();
        graphs
            .graph_indexes()
            .filter_map(|index| Some((index, graphs.graph(index)?.kill_feed_pulser()?)))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pulsers(orpheus_testkit::bot_graphs()),
        [(0x01cf, 108), (0x0c18, 8)]
    );
    assert_eq!(
        pulsers(orpheus_testkit::runtime_dir().join("data/game_graphs_174.json.gz")),
        [(0x01cf, 108)]
    );
}
