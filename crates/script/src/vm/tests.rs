use super::*;

use crate::{player::PlayerWorld, weapons::DamageTags};
use orpheus_content::{heroes::SOLDIER, maps::PRACTICE_RANGE_MODE};
use serde_json::json;

/// Soldier: 76's body (`006E.075` `m_gameplayEntity`).
const SOLDIER_BODY: u64 = 0x0400_0000_0000_03cf;

mod flow;

fn node(position: u16, class: &str, state: Option<u16>, mut fields: Json) -> Json {
    fields["m_2BBEEAB8"] = json!(state.is_some());
    fields["m_ADEB6E05"] =
        json!(class.contains("Action") || matches!(class, "STU_12F5D52D" | "STU_22926252"));
    json!({
        "pos":position, "class":class, "uid":position, "rid":position, "remote":-1,
        "state":state, "client_only":false, "server_only":false, "fields":fields,
    })
}

fn graph(index: u32, nodes: Vec<Json>, states: Json, entries: Json) -> Json {
    json!({
        "index":index, "items":nodes.len(), "nodes":nodes, "states":states,
        "entries":entries, "remote":[], "sync_vars":[], "containers":{},
        "unknown_nodes":[], "fields":{},
        "header":{
            "m_nodesBitCount":4, "m_statesBitCount":4, "m_remoteSyncNodesBitCount":4,
            "m_syncVarsBitCount":4
        }
    })
}

fn store(graphs: Vec<Json>) -> GraphStore {
    GraphStore::from_json(
        &serde_json::to_vec(&json!({
            "version":1, "graphs":graphs, "bodies":{}, "missing":[]
        }))
        .unwrap(),
    )
    .unwrap()
}

fn plug(links: &[(u16, &str)], states: &[u16]) -> Json {
    json!({"$":"Plug","links":links,"m_FF3DAF1E":states})
}

fn var(id: u32, entity: bool) -> Json {
    json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":entity})
}

fn parent_var(id: u32) -> Json {
    json!({"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[10,1,0,0], "m_dynamicVars":[var(id, false)]
    }})
}

#[test]
fn bridge_forwards_input_only_between_begin_and_abort() {
    let message = |position, input| {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":format!("0x024000000000{position:04X}")},
                "m_outPlug":plug(&[(0,input)], &[]),
            }),
        )
    };
    let fixture = graph(
        42,
        vec![
            node(
                0,
                "STU_7B9E8909",
                Some(0),
                json!({"m_outPlug":plug(&[(1,"m_inPlug")], &[])}),
            ),
            node(
                1,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(100,true), "m_value":true}),
            ),
            message(2, "m_inPlug"),
            message(3, "m_beginPlug"),
            message(4, ABORT_PLUG),
        ],
        json!([0]),
        json!([2, 3, 4]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![fixture])),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    for (message, active, activation, forwarded) in [
        (2, false, 0, false),
        (3, true, 1, false),
        (2, true, 1, true),
        (2, true, 1, true),
        (4, false, 1, false),
        (2, false, 1, false),
    ] {
        component
            .set_entity_variable(100, Value::Bool(false), &mut EmptyWorld)
            .unwrap();
        component
            .queue_message(1, 0x0240_0000_0000_0000 | message, Variables::new())
            .unwrap();
        component.run_now(&mut EmptyWorld).unwrap();
        let snapshot = component.snapshot().unwrap();
        let state = &snapshot.instances[&1].states[&0];
        assert_eq!((state.active, state.activation), (active, activation));
        assert_eq!(snapshot.variables[&100], Value::Bool(forwarded));
        assert!(snapshot.initialization_complete());
    }
}

#[test]
fn client_subgraph_controller_keeps_its_branch_off_the_server() {
    for client_only in [true, false] {
        let mut target = node(2, "STUStatescriptState", Some(1), json!({}));
        target["client_only"] = json!(client_only);
        let graphs = store(vec![graph(
            42,
            vec![
                node(
                    0,
                    "STUStatescriptEntry",
                    None,
                    json!({"m_outPlug":plug(&[(1, "m_beginPlug")], &[])}),
                ),
                node(
                    1,
                    "STU_BD02E168",
                    Some(0),
                    json!({"m_5B10D01F":{
                        "$":"STU_904BFCEC","links":[[2, "m_beginPlug"]],"m_FF3DAF1E":[1]
                    }}),
                ),
                target,
            ],
            json!([1, 2]),
            json!([0]),
        )]);
        let snapshot = initialize(
            &graphs,
            0xa000_0100,
            Variables::new(),
            vec![Root::new(1, 42)],
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        let states = &snapshot.instances[&1].states;
        assert!(!states.contains_key(&1));
        if client_only {
            assert!(snapshot.initialization_complete());
            assert!(states[&0].active);
            assert_eq!(states[&0].kind, StateKind::Plain);
        } else {
            assert!(!states.contains_key(&0));
            assert!(matches!(
                snapshot.diagnostics.as_slice(),
                [Diagnostic { issue: Issue::UnsupportedState(class), .. }]
                    if class == "STU_BD02E168"
            ));
        }
    }
}

#[test]
fn networked_for_code_state_keeps_its_subgraph_lifetime() {
    for (server_only, flags) in [(false, 0), (true, 0), (false, 1)] {
        let mut for_code = node(
            1,
            "STU_CD46AF93",
            Some(0),
            json!({
                "m_DC897C87":1, "m_598E32D5":flags, "m_0582B0E4":0,
                "m_values":[{
                    "$":"STUStatescriptForCodeValue",
                    "m_identifier":"0x0D80000000000042",
                    "m_value":var(100,true),
                }],
                "m_subgraphPlug":plug(&[(2,"m_beginPlug")], &[1]),
            }),
        );
        for_code["remote"] = json!(0);
        for_code["server_only"] = json!(server_only);
        let mut fixture = graph(
            42,
            vec![
                node(
                    0,
                    "STUStatescriptEntry",
                    None,
                    json!({"m_outPlug":plug(&[(1,"m_beginPlug")], &[])}),
                ),
                for_code,
                node(2, "STUStatescriptState", Some(1), json!({})),
                node(
                    3,
                    "STUStatescriptEntryGameMessage",
                    None,
                    json!({
                        "m_gameMessage":{"m_gameMessage":"0x0240000000000003"},
                        "m_outPlug":plug(&[(1,ABORT_PLUG)], &[]),
                    }),
                ),
            ],
            json!([1, 2]),
            json!([0, 3]),
        );
        fixture["remote"] = json!([1]);
        let mut component = Component::start(
            Arc::new(store(vec![fixture])),
            42,
            Variables::from([(100, Value::Integer(5))]),
            vec![Root::new(1, 42)],
            0,
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        let snapshot = component.snapshot().unwrap();
        let states = &snapshot.instances[&1].states;
        if server_only || flags != 0 {
            assert!(states.is_empty());
            assert!(matches!(
                snapshot.diagnostics.as_slice(),
                [Diagnostic { issue: Issue::UnsupportedState(class), .. }]
                    if class == "STU_CD46AF93"
            ));
            continue;
        }
        assert!(snapshot.initialization_complete());
        assert!(states[&0].active && states[&1].active);
        assert_eq!(states[&0].kind, StateKind::Plain);
        assert!(!snapshot.variables.contains_key(&0x42));
        component
            .queue_message(1, 0x0240_0000_0000_0003, Variables::new())
            .unwrap();
        component.run_now(&mut EmptyWorld).unwrap();
        let snapshot = component.snapshot().unwrap();
        let states = &snapshot.instances[&1].states;
        assert!(!states[&0].active && !states[&1].active);
    }
}

#[test]
fn movement_sample_updates_only_changed_authored_outputs() {
    struct SampleWorld(Option<u16>);
    impl World for SampleWorld {
        fn movement_sample(&self, _: Site) -> Option<u16> {
            self.0
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
                    "m_outPlug":plug(&[(1,"m_beginPlug")], &[])
                }),
            ),
            node(
                1,
                "STU_96F1DBCF",
                Some(0),
                json!({
                    "m_49F4B33C":var(100,true),
                    "m_103A8581":var(101,false),
                    "m_subgraphPlug":plug(&[(2,"m_beginPlug")], &[1])
                }),
            ),
            node(
                2,
                "STUStatescriptStateBooleanSwitch",
                Some(1),
                json!({
                    "m_condition":var(100,true)
                }),
            ),
        ],
        json!([1, 2]),
        json!([0]),
    );
    let graphs = Arc::new(store(vec![fixture]));
    let start = |world: &mut SampleWorld| {
        Component::start(
            Arc::clone(&graphs),
            42,
            Variables::new(),
            vec![Root::new(1, 42)],
            0,
            world,
            MAX_INIT_WORK,
        )
        .unwrap()
    };
    let mut world = SampleWorld(Some(2));
    let mut component = start(&mut world);
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.initialization_complete());
    let owner = &snapshot.instances[&1];
    assert_eq!(owner.states[&0].kind, StateKind::MovementSample(2));
    assert_eq!(owner.states[&1].kind, StateKind::BooleanSwitch(true));
    assert_eq!(snapshot.variables[&100], Value::Bool(true));
    assert!(!owner.variables.contains_key(&101));

    world.0 = Some(1 | (1 << 6));
    component.advance_frame(1, &mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    let owner = &snapshot.instances[&1];
    assert_eq!(owner.states[&0].kind, StateKind::MovementSample(65));
    assert_eq!(owner.states[&1].kind, StateKind::BooleanSwitch(false));
    assert_eq!(snapshot.variables[&100], Value::Bool(false));
    assert_eq!(owner.variables[&101], Value::Bool(true));

    world.0 = Some(0);
    component.advance_frame(2, &mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert_eq!(
        snapshot.instances[&1].states[&0].kind,
        StateKind::MovementSample(0)
    );
    assert_eq!(snapshot.instances[&1].variables[&101], Value::Bool(false));

    let unsupported = start(&mut SampleWorld(None));
    assert!(matches!(
        unsupported.snapshot().unwrap().diagnostics.as_slice(),
        [Diagnostic {
            issue: Issue::UnsupportedField("owner movement sample"),
            ..
        }]
    ));
}

#[test]
fn instance_exit_drains_in_order_and_finishes_the_linked_parent() {
    let entry = |links: &[(u16, &str)]| {
        node(
            0,
            "STUStatescriptEntry",
            None,
            json!({"m_outPlug":plug(links, &[])}),
        )
    };
    let mark = |position, identifier| {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({
                "m_out_Var":var(identifier,true), "m_value":true
            }),
        )
    };
    let subscript = |position, raw, child: u16, keep_parent| {
        node(
            position,
            "STU_37D754C8",
            Some(raw),
            json!({
                "m_36F7B64C":{"m_36F7B64C":{"m_C974E369":{
                    "m_graph":format!("0x058000000000{child:04X}")
                }}},
                "m_24AB04BB":keep_parent
            }),
        )
    };
    let child = graph(
        43,
        vec![
            entry(&[
                (1, "m_beginPlug"),
                (2, "m_beginPlug"),
                (3, "m_inPlug"),
                (4, "m_beginPlug"),
            ]),
            node(
                1,
                "STUStatescriptStateWait",
                Some(0),
                json!({
                    "m_timeout":{"$":"STUConfigVarFloat","m_value":1.0},
                    "m_onBeginPlug":plug(&[(5,"m_inPlug")], &[]),
                    "m_onAbortPlug":plug(&[(6,"m_inPlug")], &[]),
                    "m_onEndPlug":plug(&[(6,"m_inPlug")], &[])
                }),
            ),
            subscript(2, 1, 44, false),
            node(
                3,
                "STU_22926252",
                None,
                json!({"m_outPlug":plug(&[(7,"m_inPlug")], &[])}),
            ),
            node(
                4,
                "STUStatescriptState",
                Some(2),
                json!({"m_onBeginPlug":plug(&[(8,"m_inPlug")], &[])}),
            ),
            mark(5, 100),
            mark(6, 101),
            mark(7, 102),
            mark(8, 103),
        ],
        json!([1, 2, 4]),
        json!([0]),
    );
    let grandchild = graph(
        44,
        vec![entry(&[(1, "m_inPlug")]), mark(1, 104)],
        json!([]),
        json!([0]),
    );
    for keep_parent in [false, true] {
        let mut owner = subscript(1, 0, 43, keep_parent);
        owner["fields"]["m_onFinishedPlug"] = plug(&[(2, "m_inPlug")], &[]);
        owner["fields"]["m_onAbortPlug"] = plug(&[(3, "m_inPlug")], &[]);
        let parent = graph(
            42,
            vec![
                entry(&[(1, "m_beginPlug")]),
                owner,
                mark(2, 105),
                mark(3, 106),
            ],
            json!([1]),
            json!([0]),
        );
        let snapshot = initialize(
            &store(vec![parent, child.clone(), grandchild.clone()]),
            42,
            Variables::new(),
            vec![Root::new(1, 42)],
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        assert!(
            snapshot.initialization_complete(),
            "{:?}",
            snapshot.diagnostics
        );
        assert_eq!(snapshot.instances.keys().copied().collect::<Vec<_>>(), [1]);
        assert!(snapshot.pending_timers.is_empty());
        let state = &snapshot.instances[&1].states[&0];
        assert_eq!(state.active, keep_parent);
        assert_eq!(state.kind, StateKind::SubScript { child: None });
        let mut expected = Variables::from([(100, Value::Bool(true)), (102, Value::Bool(true))]);
        if !keep_parent {
            expected.insert(105, Value::Bool(true));
        }
        assert_eq!(snapshot.variables, expected);
    }
}

#[test]
fn server_no_ops_run_what_follows_and_assumed_ones_are_recorded() {
    let action = |position, class: &str, next: u16| {
        node(
            position,
            class,
            None,
            json!({"m_outPlug":plug(&[(next, "m_inPlug")], &[])}),
        )
    };
    let mark = |position, identifier| {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({"m_out_Var":var(identifier,true), "m_value":true}),
        )
    };
    let graph = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_inPlug"), (3, "m_inPlug"), (5, "m_inPlug")], &[])}),
            ),
            action(1, "STUStatescriptActionEffect", 2),
            mark(2, 100),
            action(3, "STU_E9694EFD", 4),
            mark(4, 101),
            action(5, "STU_6E5C5F66", 6),
            mark(6, 102),
        ],
        json!([]),
        json!([0]),
    );
    let snapshot = initialize(
        &store(vec![graph]),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(
        snapshot.variables,
        Variables::from([
            (100, Value::Bool(true)),
            (101, Value::Bool(true)),
            (102, Value::Bool(true))
        ])
    );
    // Only the unnamed action is an assumption; the effect and STU_6E5C5F66
    // (trace data only, upstream's research) are known no-ops.
    let assumed: Vec<_> = snapshot
        .assumptions
        .iter()
        .map(|item| (item.site.node, item.class.as_str(), item.answer))
        .collect();
    assert_eq!(assumed, [(Some(3), "STU_E9694EFD", "no-op")]);
}

/// Debug builds refuse unknown graph content; release builds do what the
/// Python reference did and record it.
#[test]
fn unknown_content_is_refused_or_tolerated_like_the_python_reference() {
    struct Tolerant;
    impl World for Tolerant {
        fn unknowns(&self) -> Unknowns {
            Unknowns::Tolerate
        }
    }

    let mark = |position, identifier| {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({"m_out_Var":var(identifier,true), "m_value":true}),
        )
    };
    let graph = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(
                    &[(1, "m_inPlug"), (3, "m_beginPlug"), (4, "m_inPlug")],
                    &[],
                )}),
            ),
            node(
                1,
                "STU_00000001",
                None,
                json!({"m_outPlug":plug(&[(2, "m_inPlug")], &[])}),
            ),
            mark(2, 100),
            node(3, "STU_00000002", Some(0), json!({})),
            node(
                4,
                "STU_3387AB5D",
                None,
                json!({
                    "m_condition":{"$":"STU_00000003"},
                    "m_truePlug":plug(&[(5, "m_inPlug")], &[]),
                    "m_falsePlug":plug(&[(6, "m_inPlug")], &[]),
                }),
            ),
            mark(5, 101),
            mark(6, 102),
        ],
        json!([3]),
        json!([0]),
    );
    let graphs = store(vec![graph]);
    let refused = initialize(
        &graphs,
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let refused: Vec<_> = refused
        .diagnostics
        .iter()
        .map(|item| (item.site.node, format!("{:?}", item.issue)))
        .collect();
    assert_eq!(
        refused,
        [
            (Some(1), r#"UnsupportedAction("STU_00000001")"#.into()),
            (
                Some(4),
                r#"Expression(UnsupportedConfig("STU_00000003"))"#.into()
            ),
            (Some(3), r#"UnsupportedState("STU_00000002")"#.into()),
        ]
    );

    let tolerated = initialize(
        &graphs,
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut Tolerant,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(tolerated.initialization_complete());
    // The action's out plug ran, the state is on, and the unknown condition
    // read as null, so the false branch ran.
    assert_eq!(
        tolerated.variables,
        Variables::from([(100, Value::Bool(true)), (102, Value::Bool(true))])
    );
    assert_eq!(tolerated.instances[&1].states[&0].kind, StateKind::Plain);
    let assumed: Vec<_> = tolerated
        .assumptions
        .iter()
        .map(|item| (item.site.node, item.class.as_str(), item.answer))
        .collect();
    assert_eq!(
        assumed,
        [
            (Some(1), "STU_00000001", "follows its out plug"),
            (Some(4), "STU_00000003", "null"),
            (Some(3), "STU_00000002", "plain state"),
        ]
    );
}

#[test]
fn target_tag_releases_on_abort_and_subscript_teardown() {
    #[derive(Default)]
    struct TagWorld(Vec<(TargetTagEffect, bool)>);

    impl World for TagWorld {
        fn set_target_tag_effect(
            &mut self,
            _: Site,
            effect: TargetTagEffect,
            active: bool,
        ) -> bool {
            self.0.push((effect, active));
            true
        }
    }

    const ABORT: u64 = 0x0240_0000_0000_0002;
    const RESTART: u64 = 0x0240_0000_0000_0003;
    let entry = |position, message: Option<u64>, input| {
        let (class, mut fields) = match message {
            Some(message) => (
                "STUStatescriptEntryGameMessage",
                json!({"m_gameMessage":{"m_gameMessage":format!("0x{message:016X}")}}),
            ),
            None => ("STUStatescriptEntry", json!({})),
        };
        fields["m_outPlug"] = plug(&[(1, input)], &[]);
        node(position, class, None, fields)
    };
    let parent = graph(
        42,
        vec![
            entry(0, None, "m_beginPlug"),
            node(
                1,
                "STU_37D754C8",
                Some(0),
                json!({
                    "m_36F7B64C":{"m_36F7B64C":{"m_C974E369":{"m_graph":"0x058000000000002B"}}}
                }),
            ),
            entry(2, Some(ABORT), ABORT_PLUG),
        ],
        json!([1]),
        json!([0, 2]),
    );
    let child = graph(
        43,
        vec![
            entry(0, None, "m_beginPlug"),
            node(
                1,
                "STU_E56BA8D8",
                Some(0),
                json!({
                    "m_31714AFC":{"$":"STU_B5A0CAF0"},
                    "m_44D1BE94":{"$":"STU_357DABBB","m_7D7C86A1":"0x0D60000000000042"}
                }),
            ),
            entry(2, Some(ABORT), ABORT_PLUG),
            entry(3, Some(RESTART), "m_beginPlug"),
        ],
        json!([1]),
        json!([0, 2, 3]),
    );
    let effect = TargetTagEffect {
        entity: 42,
        tag: 0x0d60_0000_0000_0042,
        present: false,
    };
    let mut world = TagWorld::default();
    let mut component = Component::start(
        Arc::new(store(vec![parent, child])),
        effect.entity,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(component.snapshot().unwrap().initialization_complete());
    assert_eq!(world.0, [(effect, true)]);

    for (id, message) in [(2, ABORT), (2, RESTART), (1, ABORT)] {
        assert_eq!(
            component
                .queue_message(id, message, Variables::new())
                .unwrap(),
            1
        );
        component.run_now(&mut world).unwrap();
    }
    assert!(!component.snapshot().unwrap().instances.contains_key(&2));
    assert_eq!(
        world.0,
        [
            (effect, true),
            (effect, false),
            (effect, true),
            (effect, false)
        ]
    );
}

#[test]
fn base_health_pool_keeps_remaining_amount_before_on_end_and_restarts_fresh() {
    use crate::vitals::{Death, Vitals};

    struct PoolWorld(Vitals);
    impl World for PoolWorld {
        fn body_health(&self, _: Site, kind: Option<PoolKind>, maximum: bool) -> Option<f64> {
            Some(self.0.amount(kind, maximum))
        }

        fn add_health_pool(
            &mut self,
            _: Site,
            kind: PoolKind,
            maximum: f64,
            current: f64,
        ) -> Option<u32> {
            self.0.add_pool(kind, maximum, current)
        }

        fn health_pool_current(&self, _: Site, pool: u32) -> Option<f64> {
            self.0.pool_current(pool)
        }

        fn remove_health_pool(&mut self, _: Site, pool: u32) -> Option<f64> {
            self.0.remove_pool(pool)
        }
    }

    let entry = |position, class, message, input| {
        node(
            position,
            class,
            None,
            json!({
                "m_gameMessage": {"m_gameMessage":message},
                "m_outPlug": plug(&[(1, input)], &[])
            }),
        )
    };
    let flag = |value| json!({"$":"STUConfigVarBool","m_value":value});
    let owner = json!({"$":"STU_B5A0CAF0"});
    let fixture = graph(
        42,
        vec![
            entry(0, "STUStatescriptEntry", "", "m_beginPlug"),
            node(
                1,
                "STUStatescriptStateHealthPool",
                Some(0),
                json!({
                    "m_FA3ECC43":owner, "m_7E8B666C":0,
                    "m_amount":var(100,true), "m_BCFDD2E9":var(101,true),
                    "m_69A20070":{"$":"STU_FEDD683C","m_entity":owner},
                    "m_4895B8B6":flag(true), "m_64839023":flag(false), "m_0E8A5C8E":flag(false),
                    "m_CEAB19E8":var(102,true),
                    "m_onEndPlug":plug(&[(4,"m_inPlug")], &[])
                }),
            ),
            entry(
                2,
                "STUStatescriptEntryGameMessage",
                "0x0240000000000002",
                ABORT_PLUG,
            ),
            entry(
                3,
                "STUStatescriptEntryGameMessage",
                "0x0240000000000003",
                "m_beginPlug",
            ),
            node(
                4,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(101,true), "m_value":var(102,true)
                }),
            ),
        ],
        json!([1]),
        json!([0, 2, 3]),
    );
    let mut world = PoolWorld(Vitals::empty());
    let mut component = Component::start(
        Arc::new(store(vec![fixture])),
        42,
        Variables::from([(100, Value::Float(200.0)), (101, Value::Float(175.0))]),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(component.snapshot().unwrap().initialization_complete());
    assert_eq!((world.0.total(), world.0.maximum()), (175.0, 200.0));
    world.0.damage(
        25.0,
        Death {
            time: 0.0,
            frame: 0,
            killer: 43,
        },
    );
    component
        .queue_message(1, 0x0240_0000_0000_0002, Variables::new())
        .unwrap();
    component.run_now(&mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.variables[&101], Value::Float(150.0));
    assert_eq!(snapshot.variables[&OWNER_HEALTH], Value::Float(0.0));
    assert!(!snapshot.instances[&1].states[&0].active);
    assert_eq!(world.0.total(), 0.0);
    component
        .queue_message(1, 0x0240_0000_0000_0003, Variables::new())
        .unwrap();
    component.run_now(&mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert_eq!(
        snapshot.instances[&1].states[&0].kind,
        StateKind::HealthPool { pool: Some(2) }
    );
    assert_eq!(snapshot.variables[&OWNER_HEALTH], Value::Float(150.0));
    assert_eq!(world.0.pool_current(1), None);
    assert_eq!(world.0.pool_current(2), Some(150.0));
    component
        .set_entity_variable(100, Value::Float(225.0), &mut world)
        .unwrap();
    component.run_now(&mut world).unwrap();
    assert_eq!(world.0.maximum(), 200.0);
    assert!(
        component
            .snapshot()
            .unwrap()
            .diagnostics
            .iter()
            .any(|diagnostic| {
                diagnostic.issue == Issue::UnsupportedField("changing health pool capacity")
            })
    );
}

fn delayed_switch_fixture(mut fields: Json) -> Arc<GraphStore> {
    fields["m_onTruePlug"] = plug(&[(6, "m_inPlug")], &[]);
    fields["m_onFalsePlug"] = plug(&[(7, "m_inPlug")], &[]);
    fields["m_trueSubgraphPlug"] = plug(&[(2, "m_beginPlug")], &[1]);
    fields["m_falseSubgraphPlug"] = plug(&[(3, "m_beginPlug")], &[2]);
    let entry = |position, input| {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":format!("0x024000000000{position:04X}")},
                "m_outPlug":plug(&[(1,input)], &[]),
            }),
        )
    };
    let record_branch = |position, branch| {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({"m_out_Var":var(103,true), "m_value":branch}),
        )
    };
    Arc::new(store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_beginPlug")], &[])}),
            ),
            node(1, "STU_B591EBFE", Some(0), fields),
            node(2, "STUStatescriptState", Some(1), json!({})),
            node(3, "STUStatescriptState", Some(2), json!({})),
            entry(4, ABORT_PLUG),
            entry(5, "m_beginPlug"),
            record_branch(6, true),
            record_branch(7, false),
        ],
        json!([1, 2, 3]),
        json!([0, 4, 5]),
    )]))
}

fn assert_delayed_switch(component: &Component, branch: Option<bool>, timer: Option<(u64, u16)>) {
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    let states = &snapshot.instances[&1].states;
    assert!(states[&0].active);
    assert_eq!(
        states[&0].kind,
        StateKind::DelayedSwitch {
            current: branch == Some(true),
            undecided: branch.is_none(),
        },
    );
    assert_eq!(
        states.get(&1).is_some_and(|state| state.active),
        branch == Some(true)
    );
    assert_eq!(
        states.get(&2).is_some_and(|state| state.active),
        branch == Some(false)
    );
    if let Some(branch) = branch {
        assert_eq!(snapshot.variables[&103], Value::Bool(branch));
    }
    let expected: Vec<_> = timer
        .map(|(due_ms, parameter)| PendingTimer {
            due_ms,
            raw_state: 0,
            finish: false,
            parameter,
        })
        .into_iter()
        .collect();
    assert_eq!(
        snapshot
            .pending_timers
            .get(&1)
            .map_or(&[][..], Vec::as_slice),
        expected,
    );
}

#[test]
fn delayed_switch_uses_initial_override_and_asymmetric_interruptible_timers() {
    let mut component = Component::start(
        delayed_switch_fixture(json!({
            "m_418D9D9D": true,
            "m_BD83C8E6": var(100,true),
            "m_E8FC060D": var(101,true),
            "m_7130456B": 0.0259,
            "m_ED60694A": 0.04075,
        })),
        42,
        Variables::from([(100, Value::Bool(false)), (101, Value::Bool(false))]),
        vec![Root::new(1, 42)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let write = |component: &mut Component, id, value| {
        component
            .set_entity_variable(id, Value::Bool(value), &mut EmptyWorld)
            .unwrap();
        component.run_now(&mut EmptyWorld).unwrap();
    };

    assert_delayed_switch(&component, Some(true), None);
    write(&mut component, 101, true);
    assert_delayed_switch(&component, Some(true), Some((40, SWITCH_TO_FALSE)));
    component.advance_frame(1, &mut EmptyWorld).unwrap();
    assert_delayed_switch(&component, Some(true), Some((40, SWITCH_TO_FALSE)));
    write(&mut component, 101, false);
    component.advance_frame(2, &mut EmptyWorld).unwrap();
    assert_delayed_switch(&component, Some(true), None);
    write(&mut component, 101, true);
    assert_delayed_switch(&component, Some(true), Some((87, SWITCH_TO_FALSE)));
    for frame in 3..=5 {
        component.advance_frame(frame, &mut EmptyWorld).unwrap();
    }
    assert_delayed_switch(&component, Some(false), None);
    write(&mut component, 101, false);
    write(&mut component, 100, true);
    assert_delayed_switch(&component, Some(false), Some((120, SWITCH_TO_TRUE)));
    for frame in 6..=7 {
        component.advance_frame(frame, &mut EmptyWorld).unwrap();
    }
    assert_delayed_switch(&component, Some(true), None);
}

#[test]
fn delayed_switch_polls_undecided_inputs_and_clears_timers_on_abort() {
    #[derive(Default)]
    struct Conditions {
        positive: bool,
        negative: bool,
    }

    impl World for Conditions {
        fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
            match config["$"].as_str()? {
                "test-delayed-positive" => Some(Value::Bool(self.positive)),
                "test-delayed-negative" => Some(Value::Bool(self.negative)),
                _ => None,
            }
        }
    }

    let mut world = Conditions::default();
    let mut component = Component::start(
        delayed_switch_fixture(json!({
            "m_308B8A80": true,
            "m_418D9D9D": true,
            "m_BD83C8E6": {"$":"test-delayed-positive"},
            "m_E8FC060D": {"$":"test-delayed-negative"},
            "m_7130456B": 0.075,
            "m_ED60694A": 0.001,
        })),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert_delayed_switch(&component, None, None);
    assert!(!component.snapshot().unwrap().variables.contains_key(&103));
    world.positive = true;
    component.advance_frame(0, &mut world).unwrap();
    assert_delayed_switch(&component, None, Some((90, SWITCH_TO_TRUE)));
    world.positive = false;
    component.advance_frame(1, &mut world).unwrap();
    assert_delayed_switch(&component, None, Some((90, SWITCH_TO_TRUE)));
    world.negative = true;
    component.advance_frame(2, &mut world).unwrap();
    assert_delayed_switch(&component, None, Some((48, SWITCH_TO_FALSE)));
    component.advance_frame(3, &mut world).unwrap();
    assert_delayed_switch(&component, Some(false), None);

    world.positive = true;
    component.advance_frame(4, &mut world).unwrap();
    assert_delayed_switch(&component, Some(false), Some((154, SWITCH_TO_TRUE)));
    component
        .queue_message(1, 0x0240_0000_0000_0004, Variables::new())
        .unwrap();
    component.run_now(&mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.pending_timers.is_empty());
    assert!(!snapshot.instances[&1].states[&0].active);
    assert!(!snapshot.instances[&1].states[&2].active);

    world = Conditions::default();
    component
        .queue_message(1, 0x0240_0000_0000_0005, Variables::new())
        .unwrap();
    component.run_now(&mut world).unwrap();
    component.advance_frame(10, &mut world).unwrap();
    assert_delayed_switch(&component, None, None);
    assert_eq!(
        component.snapshot().unwrap().instances[&1].states[&0].activation,
        2
    );
}

#[test]
fn startup_queues_states_rechecks_scoped_writes_and_starts_children_after_roots() {
    let mut client_only = node(6, "UnportedClientAction", None, json!({}));
    client_only["client_only"] = json!(true);
    let first = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":plug(&[(1,"m_beginPlug"),(2,"m_beginPlug")], &[])
                }),
            ),
            node(
                1,
                "STUStatescriptStateBooleanSwitch",
                Some(0),
                json!({
                    "m_condition":var(100, true),
                    "m_falseSubgraphPlug":plug(&[(3,"m_beginPlug")], &[2]),
                    "m_trueSubgraphPlug":plug(&[(4,"m_beginPlug")], &[3]),
                }),
            ),
            node(
                2,
                "STUStatescriptState",
                Some(1),
                json!({
                    "m_onBeginPlug":plug(&[(5,"m_inPlug"),(6,"m_inPlug")], &[])
                }),
            ),
            node(3, "STUStatescriptState", Some(2), json!({})),
            node(
                4,
                "STU_37D754C8",
                Some(3),
                json!({
                    "m_36F7B64C":{"m_36F7B64C":{"m_C974E369":{"m_graph":"0x058000000000002B"}}},
                    "m_89402D74":[{"m_out_Var":var(4, false), "m_value":var(9, false)}]
                }),
            ),
            node(
                5,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(100, true), "m_value":{"$":"STUConfigVarBool","m_value":true}
                }),
            ),
            client_only,
        ],
        json!([1, 2, 3, 4]),
        json!([0]),
    );
    let child = graph(
        43,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_inPlug")], &[])}),
            ),
            node(
                1,
                "STU_12F5D52D",
                None,
                json!({
                    "m_143D4C5B":[
                        {"m_out_Var":var(12, true),"m_value":parent_var(9)},
                        {"m_out_Var":var(14, true),"m_value":var(4, false)},
                        {"m_out_Var":var(15, true),"m_value":var(13, true)}
                    ]
                }),
            ),
        ],
        json!([]),
        json!([0]),
    );
    let second = graph(
        44,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_inPlug")], &[])}),
            ),
            node(
                1,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(13, true), "m_value":{"$":"STUConfigVarInt","m_value":99}
                }),
            ),
        ],
        json!([]),
        json!([0]),
    );
    let graphs = store(vec![first, child, second]);
    let mut root = Root::new(7, 42);
    root.variables.insert(9, Value::Integer(3));
    let snapshot = initialize(
        &graphs,
        0xa000_0100,
        Variables::from([(100, Value::Bool(false))]),
        vec![root, Root::new(1, 44)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(
        snapshot.instances.keys().copied().collect::<Vec<_>>(),
        [7, 1, 8]
    );
    assert_eq!(snapshot.instances[&8].parent, Some((7, 3)));
    assert_eq!(snapshot.instances[&8].variables[&4], Value::Integer(3));
    let states = &snapshot.instances[&7].states;
    assert_eq!(states[&0].kind, StateKind::BooleanSwitch(true));
    assert!(!states[&2].active);
    assert_eq!(states[&2].activation, 1);
    assert_eq!(states[&3].kind, StateKind::SubScript { child: Some(8) });
    assert_eq!(snapshot.variables[&12], Value::Integer(3));
    assert_eq!(snapshot.variables[&14], Value::Integer(3));
    assert_eq!(snapshot.variables[&15], Value::Integer(99));
}

#[test]
fn unsupported_branches_are_reported_and_cyclic_actions_are_bounded() {
    let diagnostics = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":plug(&[(1,"m_inPlug"),(2,"m_beginPlug")], &[])
                }),
            ),
            node(
                1,
                "UnknownAction",
                None,
                json!({"m_outPlug":plug(&[(3,"m_inPlug")], &[])}),
            ),
            node(2, "STUStatescriptState", Some(0), json!({})),
            node(
                3,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(42, true), "m_value":true
                }),
            ),
        ],
        json!([2]),
        json!([0]),
    );
    let cycle = graph(
        43,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_inPlug")], &[])}),
            ),
            node(
                1,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(1, false), "m_value":true, "m_outPlug":plug(&[(1,"m_inPlug")], &[])
                }),
            ),
        ],
        json!([]),
        json!([0]),
    );
    let graphs = store(vec![diagnostics, cycle]);
    let run = |graph, budget| {
        initialize(
            &graphs,
            42,
            Variables::new(),
            vec![Root::new(1, graph)],
            &mut EmptyWorld,
            budget,
        )
    };
    let snapshot = run(42, MAX_INIT_WORK).unwrap();
    assert!(!snapshot.initialization_complete());
    assert!(snapshot.instances[&1].states[&0].active);
    assert!(!snapshot.variables.contains_key(&42));
    assert_eq!(snapshot.diagnostics[0].site.node, Some(1));
    assert_eq!(
        snapshot.diagnostics[0].issue,
        Issue::UnsupportedAction("UnknownAction".into())
    );
    assert!(matches!(
        run(43, 9),
        Err(InitFault::WorkBudget { limit: 9, .. })
    ));
    assert!(matches!(run(43, MAX_INIT_WORK), Err(InitFault::PlugDepth)));
}

#[test]
fn queued_messages_abort_and_rearm_waits_without_crossing_instance_scope() {
    let message_node = |position, message, input| {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage": {"m_gameMessage":message},
                "m_params": [{"m_B5051BCE":"0x0A40000000000004","m_out_Var":var(4,false)}],
                "m_outPlug": plug(&[(1,input)],&[])
            }),
        )
    };
    let timed = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":plug(&[(1,"m_beginPlug"),(5,"m_beginPlug")],&[])
                }),
            ),
            node(
                1,
                "STUStatescriptStateWait",
                Some(0),
                json!({
                    "m_timeout":{"$":"STUConfigVarFloat","m_value":0.02},
                    "m_0DBAFD7F":1,
                    "m_onFinishedPlug":plug(&[(2,"m_inPlug")],&[]),
                    "m_onAbortPlug":plug(&[(3,"m_inPlug")],&[]),
                }),
            ),
            node(
                2,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(10,false), "m_value": true
                }),
            ),
            node(
                3,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(10,false), "m_value": false
                }),
            ),
            message_node(4, "0x0240000000000001", "m_beginPlug"),
            node(
                5,
                "STU_D74D4F47",
                Some(1),
                json!({
                    "m_3016B9A1":{"$":"STUConfigVarInt","m_value":1},
                    "m_onFinishedPlug":plug(&[(6,"m_inPlug")],&[]),
                }),
            ),
            node(
                6,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(11,false), "m_value": true
                }),
            ),
            message_node(7, "0x0240000000000002", ABORT_PLUG),
        ],
        json!([1, 5]),
        json!([0, 4, 7]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![timed])),
        42,
        Variables::new(),
        vec![Root::new(7, 42), Root::new(1, 42)],
        1,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let timer = |due_ms| PendingTimer {
        due_ms,
        raw_state: 0,
        finish: false,
        parameter: 1,
    };
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.initialization_complete());
    assert_eq!(snapshot.pending_timers[&7], [timer(36)]);
    assert_eq!(snapshot.pending_timers[&1], [timer(36)]);
    assert_eq!(
        snapshot.instances[&7].states[&1].kind,
        StateKind::WaitFrames { target: 2 }
    );

    component.advance_frame(1, &mut EmptyWorld).unwrap();
    assert_eq!(component.now_ms(), 31);
    assert_eq!(
        component
            .queue_message(
                7,
                0x0240_0000_0000_0001,
                Variables::from([(4, Value::Integer(15))])
            )
            .unwrap(),
        1
    );
    component.run_now(&mut EmptyWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.pending_timers[&7], [timer(51)]);
    assert_eq!(snapshot.pending_timers[&1], [timer(36)]);
    assert_eq!(snapshot.instances[&7].variables[&4], Value::Integer(15));
    assert!(!snapshot.instances[&1].variables.contains_key(&4));
    assert_eq!(snapshot.instances[&7].states[&0].activation, 1);
    assert_eq!(snapshot.instances[&7].states[&0].start_ms, 31);
    assert!(matches!(
        component.advance_frame(1, &mut EmptyWorld),
        Err(InitFault::FrameOrder { .. })
    ));
    component.advance_frame(2, &mut EmptyWorld).unwrap();
    assert_eq!(
        component
            .queue_message(7, 0x0240_0000_0000_0002, Variables::new())
            .unwrap(),
        1
    );
    component.run_now(&mut EmptyWorld).unwrap();
    component.advance_frame(3, &mut EmptyWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.pending_timers.is_empty());
    assert_eq!(snapshot.instances[&7].variables[&10], Value::Bool(false));
    assert_eq!(snapshot.instances[&1].variables[&10], Value::Bool(true));
    assert_eq!(snapshot.instances[&7].variables[&11], Value::Bool(true));
    assert_eq!(snapshot.instances[&1].variables[&11], Value::Bool(true));

    for _ in 0..=MAX_EVENTS_PER_UPDATE {
        component
            .queue_message(7, 0x0240_0000_0000_0001, Variables::new())
            .unwrap();
    }
    assert_eq!(
        component.run_now(&mut EmptyWorld),
        Err(InitFault::EventBudget { instance: 7 })
    );
    assert!(component.snapshot().is_none());
    assert_eq!(component.run_now(&mut EmptyWorld), Err(InitFault::Halted));
}

/// The answers the Python reference gave for the training bot (`0B92`,
/// `0C18`) and hero select (`288A`) graphs. The server doesn't run those graphs
/// yet, so there is no server world to test them against; when it does, these
/// tests move to that world and this one goes. `STU_4A101787` takes the
/// client's answer (always false, its evaluator at `0x7FF789BFF730`); the
/// Python fixtures had null, which branches the same way. The `STU_AC829876`
/// target-tag filter in `0C18` is true as in those fixtures, although the
/// Python server answered every such filter false; what a bot's filter answers
/// is still open.
struct ReferenceWorld;

impl World for ReferenceWorld {
    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        match (site.graph, config.get("$")?.as_str()?) {
            (0x0c18, "STU_AC829876") => Some(Value::Bool(true)),
            (0x0c18, "STU_4A101787") => Some(Value::Bool(false)),
            (0x0c18, "STU_A77AC2AC") => Some(Value::Null),
            (0x288a, "STU_37F20D24") => Some(Value::Null),
            (0x0b92, "STU_71A87E92" | "STU_2200A66D") if matches!(site.node, Some(7 | 11)) => {
                Some(Value::Null)
            }
            _ => None,
        }
    }

    fn selects_ai_behavior(&mut self, site: Site) -> Option<bool> {
        (site.graph == 0x0b92 && site.node == Some(1)).then_some(false)
    }
}

#[test]
fn ordered_stack_entries_use_the_value_below_and_restore_the_hidden_base() {
    let message = |position, message, target| {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":message},
                "m_outPlug":plug(&[(target,ABORT_PLUG)],&[])
            }),
        )
    };
    let expression = json!({"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[8,0,6,0,23,0],
        "m_dynamicVars":[var(100,true)],"m_D99EF254":[1]
    }});
    let fixture = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug"),(2,"m_beginPlug")],&[])}),
            ),
            node(
                1,
                "STUStatescriptStateStack",
                Some(0),
                json!({
                    "m_out_Var":var(100,true), "m_value":10,
                    "m_priority":{"m_priority":"0x0C40000000000001"}
                }),
            ),
            node(
                2,
                "STUStatescriptStateStack",
                Some(1),
                json!({
                    "m_out_Var":var(100,true), "m_value":expression
                }),
            ),
            message(3, "0x0240000000000001", 1),
            message(4, "0x0240000000000002", 2),
        ],
        json!([1, 2]),
        json!([0, 3, 4]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![fixture])),
        42,
        Variables::from([(100, Value::Integer(5))]),
        vec![Root::new(1, 42)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let variable = Variable {
        scope: Scope::Entity,
        id: 100,
    };
    assert_eq!(
        component.resolved_variable(1, variable),
        Some(Value::Float(11.0))
    );
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.variables[&100], Value::Integer(5));
    assert_eq!(
        snapshot.bindings[&(0, 100)],
        [
            StackBinding {
                instance: 1,
                raw_state: 0,
                slot: 0,
                priority: StackPriority {
                    level: 0,
                    above_equals: false
                }
            },
            StackBinding {
                instance: 1,
                raw_state: 1,
                slot: 0,
                priority: StackPriority {
                    level: 0,
                    above_equals: true
                }
            }
        ]
    );
    assert_eq!(snapshot.resolved_bindings[&(0, 100)], Value::Float(11.0));
    assert_eq!(
        snapshot.instances[&1].states[&0].kind,
        StateKind::Stack {
            top: false,
            under: true
        }
    );
    assert_eq!(
        snapshot.instances[&1].states[&1].kind,
        StateKind::Stack {
            top: true,
            under: false
        }
    );
    component
        .set_entity_variable(100, Value::Integer(7), &mut EmptyWorld)
        .unwrap();
    component.run_now(&mut EmptyWorld).unwrap();
    assert_eq!(
        component.resolved_variable(1, variable),
        Some(Value::Float(11.0))
    );
    for (message, expected) in [(1, Value::Float(8.0)), (2, Value::Integer(7))] {
        component
            .queue_message(1, 0x0240_0000_0000_0000 | message, Variables::new())
            .unwrap();
        component.run_now(&mut EmptyWorld).unwrap();
        assert_eq!(component.resolved_variable(1, variable), Some(expected));
    }
    assert!(component.snapshot().unwrap().bindings.is_empty());
}

#[test]
fn ability_cooldown_restarts_and_stack_displacement_stops_the_running_ability() {
    let message = |position, message, target, input| {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":message},
                "m_outPlug":plug(&[(target, input)], &[]),
            }),
        )
    };
    let mut fixture = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug")],&[])}),
            ),
            node(
                1,
                "STUStatescriptStateAbility",
                Some(0),
                json!({
                    "m_out_CooldownVar":var(60,true),
                    "m_out_StackVar":var(61,true),
                    "m_5E561E77":{"m_priority":"0x0C40000000000001"},
                    "m_A5F1A77B":35,
                    "m_EF3C5C73":[{"m_out_Var":var(60,true), "m_value":0.1},
                                  {"m_out_Var":var(62,true), "m_value":true}],
                    "m_19899EAC":[{"m_out_Var":var(62,true), "m_value":false}]
                }),
            ),
            node(
                2,
                "STUStatescriptStateStack",
                Some(1),
                json!({
                    "m_out_Var":var(61,true),
                    "m_value":7,
                    "m_priority":{"m_priority":"0x0C40000000000003"}
                }),
            ),
            message(3, "0x0240000000000001", 2, "m_beginPlug"),
            message(4, "0x0240000000000002", 1, ABORT_PLUG),
        ],
        json!([1, 2]),
        json!([0, 3, 4]),
    );
    fixture["fields"]["m_680A2CB2"] = json!([0]);
    let mut component = Component::start(
        Arc::new(store(vec![fixture])),
        42,
        Variables::from([(3652, Value::Float(1.0)), (60, Value::Float(0.032))]),
        vec![Root::new(1, 42)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let timer = |component: &Component| {
        component
            .snapshot()
            .unwrap()
            .pending_timers
            .get(&1)
            .and_then(|timers| timers.first().copied())
    };
    assert_eq!(
        timer(&component),
        Some(PendingTimer {
            due_ms: 16,
            raw_state: 0,
            finish: false,
            parameter: 2
        })
    );
    for frame in 0..=1 {
        component
            .advance_command(
                Command {
                    frame,
                    buttons: 8,
                    ..Command::default()
                },
                &mut EmptyWorld,
            )
            .unwrap();
    }
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.variables[&62], Value::Bool(true));
    assert_eq!(
        snapshot.instances[&1].states[&0].kind,
        StateKind::Ability { flags: 7 }
    );
    assert_eq!(snapshot.resolved_bindings[&(0, 61)], Value::Bool(true));
    assert_eq!(snapshot.bindings[&(0, 61)][0].slot, 1);
    assert_eq!(timer(&component).map(|timer| timer.due_ms), Some(66));
    let remaining = snapshot.resolved_bindings[&(0, 60)].clone();
    for message in [1, 2] {
        component
            .queue_message(1, 0x0240_0000_0000_0000 | message, Variables::new())
            .unwrap();
        component.run_now(&mut EmptyWorld).unwrap();
    }
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.variables[&62], Value::Bool(false));
    assert_eq!(snapshot.variables[&60], remaining);
    assert_eq!(snapshot.resolved_bindings[&(0, 61)], Value::Integer(7));
    assert_eq!(snapshot.bindings[&(0, 61)][0].slot, 0);
    assert!(!snapshot.instances[&1].states[&0].active);
    assert!(timer(&component).is_none());
}

#[test]
fn switch_exits_old_branch_and_active_query_does_not_begin_its_target() {
    let set = |position, variable, value| {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({
                "m_out_Var":var(variable,true), "m_value":value,
            }),
        )
    };
    let fixture = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({
                    "m_outPlug":plug(&[(1,"m_beginPlug"),(2,"m_inPlug"),(3,"m_beginPlug")],&[])
                }),
            ),
            node(
                1,
                "STU_DEBD057B",
                Some(0),
                json!({
                    "m_262AE982":var(100,true),
                    "m_05706D1C":[
                        {"$":"Plug", "links":[[4,"m_beginPlug"]], "m_FF3DAF1E":[3], "m_value":2},
                        {"$":"Plug", "links":[[5,"m_beginPlug"]], "m_FF3DAF1E":[4], "m_value":2}
                    ],
                    "m_A5EF385D":plug(&[(6,"m_beginPlug")], &[5]),
                }),
            ),
            node(
                2,
                "STU_01178155",
                Some(1),
                json!({
                    "m_7273C81B":plug(&[(7,"m_inPlug")],&[]),
                    "m_C6F3744E":plug(&[(8,"m_inPlug")],&[]),
                }),
            ),
            node(
                3,
                "STUStatescriptStateGamePadVibration",
                Some(2),
                json!({
                    "m_onBeginPlug":plug(&[(9,"m_inPlug")],&[]),
                    "m_onFinishedPlug":plug(&[(10,"m_inPlug")],&[]),
                }),
            ),
            node(4, "STUStatescriptState", Some(3), json!({})),
            node(5, "STUStatescriptState", Some(4), json!({})),
            node(6, "STUStatescriptState", Some(5), json!({})),
            set(7, 101, 1),
            set(8, 101, 0),
            set(9, 102, 1),
            set(10, 102, 2),
            node(
                11,
                "STUStatescriptEntryGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000001"},
                    "m_outPlug":plug(&[(2,"m_beginPlug")],&[]),
                }),
            ),
            node(
                12,
                "STUStatescriptEntryGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000002"},
                    "m_outPlug":plug(&[(2,"m_inPlug")],&[]),
                }),
            ),
        ],
        json!([1, 2, 3, 4, 5, 6]),
        json!([0, 11, 12]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![fixture])),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let initial = component.snapshot().unwrap();
    let states = &initial.instances[&1].states;
    assert_eq!(states[&0].kind, StateKind::Switch { case: None });
    assert!(!states[&1].active);
    assert_eq!(states[&1].activation, 0);
    assert!(!states[&2].active);
    assert!(states[&5].active);
    assert_eq!(initial.variables[&101], Value::Integer(0));
    assert_eq!(initial.variables[&102], Value::Integer(2));
    component
        .set_entity_variable(100, Value::Integer(2), &mut EmptyWorld)
        .unwrap();
    for message in [1, 2] {
        component
            .queue_message(1, 0x0240_0000_0000_0000 | message, Variables::new())
            .unwrap();
        component.run_now(&mut EmptyWorld).unwrap();
    }
    let updated = component.snapshot().unwrap();
    let states = &updated.instances[&1].states;
    assert_eq!(states[&0].kind, StateKind::Switch { case: Some(0) });
    assert!(states[&1].active);
    assert_eq!(states[&1].activation, 1);
    assert!(states[&3].active);
    assert!(!states.contains_key(&4));
    assert!(!states[&5].active);
    assert_eq!(updated.variables[&101], Value::Integer(1));
    assert!(updated.initialization_complete());
}

/// The server's world for a human Soldier: 76 body in the Practice Range,
/// as `PlayerBodyFactory::start` builds it.
fn soldier_world(graphs: &Arc<GraphStore>) -> PlayerWorld {
    let mut world = PlayerWorld::new(
        Arc::clone(graphs),
        0xa000_0100,
        0xa000_0101,
        TEAM_ONE,
        PRACTICE_RANGE_MODE,
        SOLDIER,
    );
    world.pool_types_known = true;
    world.target_tags = graphs.bodies()[&SOLDIER_BODY].target_tags.clone();
    world
}

/// The body filter bit of the first team.
const TEAM_ONE: u64 = 0x0080_0000;

#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_sprint_stack_gates_track_input_and_reveal_the_base_on_release() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let mut world = soldier_world(&graphs);
    let mut component = Component::start(
        Arc::clone(&graphs),
        0xa000_0101,
        Variables::new(),
        vec![Root::new(5, 0x0255)],
        1029,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let var = |component: &Component, id| {
        component.resolved_variable(
            5,
            Variable {
                scope: Scope::Instance,
                id,
            },
        )
    };
    assert_eq!(var(&component, 9570), Some(Value::Bool(true)));
    // The primary, crouch and melee edges (states 4, 7 and 16) follow 0x881.
    let edges = |snapshot: &Snapshot| {
        [4, 7, 16].map(|raw| {
            (
                snapshot.instances[&5].states[&raw].kind,
                snapshot.instances[&5].states[&raw].activation,
            )
        })
    };
    let up = StateKind::LogicalButton { down: false };
    let down = StateKind::LogicalButton { down: true };
    assert_eq!(edges(component.snapshot().unwrap()), [(up, 1); 3]);
    for (frame, forward, buttons, throttle, held) in [
        (1030, 63, 0, Value::Null, Value::Null),
        (1031, 64, 0, Value::Bool(true), Value::Null),
        (1032, 127, 0x881, Value::Bool(true), Value::Bool(true)),
        (1033, 0, 0, Value::Null, Value::Null),
    ] {
        world.throttles = [0, forward];
        component
            .advance_command(
                Command {
                    frame,
                    forward,
                    buttons,
                    ..Command::default()
                },
                &mut world,
            )
            .unwrap();
        assert_eq!(var(&component, 2333), Some(throttle));
        assert_eq!(var(&component, 7681), Some(held));
        let snapshot = component.snapshot().unwrap();
        let edge = if buttons == 0 { up } else { down };
        assert_eq!(edges(snapshot), [(edge, 1); 3], "frame {frame}");
        if buttons != 0 {
            assert_eq!(snapshot.instances[&5].variables[&1919], Value::Bool(true));
            assert!(snapshot.initialization_complete());
        }
    }
    component
        .set_entity_variable(9535, Value::Bool(false), &mut world)
        .unwrap();
    component.run_now(&mut world).unwrap();
    assert_eq!(var(&component, 9570), Some(Value::Null));
    assert!(
        !component
            .snapshot()
            .unwrap()
            .bindings
            .contains_key(&(5, 9570))
    );
}

#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn isolated_soldier_sprint_ability_matches_the_authored_toggle_and_modifier_states() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let mut world = soldier_world(&graphs);
    let mut component = Component::start(
        Arc::clone(&graphs),
        0xa000_0101,
        Variables::new(),
        vec![Root::new(5, 0x0255)],
        1029,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let rows = [
        (1030, 0, 8, 0, None),
        (1031, 63, 8, 0, None),
        (1032, 64, 8, 0, None),
        (1033, 64, 8, 5, Some(1)),
        (1034, 127, 0, 1, Some(1)),
        (1035, 127, 8, 4, Some(0)),
        (1036, 127, 8, 4, Some(0)),
        (1037, 127, 0, 0, Some(0)),
        (1038, 127, 8, 5, Some(1)),
        (1039, 63, 0, 0, Some(0)),
        (1040, 127, 0, 0, Some(0)),
        (1041, 127, 9, 0, Some(0)),
    ];
    for (frame, forward, buttons, flags, hud) in rows {
        world.throttles = [0, forward];
        component
            .advance_command(
                Command {
                    frame,
                    forward,
                    buttons,
                    ..Command::default()
                },
                &mut world,
            )
            .unwrap();
        let snapshot = component.snapshot().unwrap();
        let instance = &snapshot.instances[&5];
        assert_eq!(
            instance.states[&1].kind,
            StateKind::Ability { flags },
            "frame {frame}"
        );
        assert_eq!(
            instance.variables.get(&699).cloned(),
            hud.map(Value::Integer),
            "frame {frame}"
        );
        for raw in [0, 20] {
            assert_eq!(
                instance.states.get(&raw).is_some_and(|state| state.active),
                flags & 1 != 0,
                "frame {frame}, state {raw}"
            );
        }
        let projected = Vm::new(&component.graphs, &mut world, &mut component.machine)
            .collect_movement_modifiers()
            .unwrap();
        let modifiers: Vec<_> = projected
            .iter()
            .map(|(key, modifier)| {
                (
                    key.state,
                    modifier.index,
                    modifier.operation,
                    modifier.value,
                    modifier.priority,
                )
            })
            .collect();
        if flags & 1 == 0 {
            assert!(modifiers.is_empty());
        } else {
            assert_eq!(
                modifiers,
                [
                    (0, 0, Operation::Add, 0.5, 0.0),
                    (20, 21, Operation::Set, 0.0, 0.0)
                ]
            );
        }
    }
    assert!(component.snapshot().unwrap().initialization_complete());
    // Complete now, so the server-facing call answers too: no modifier at the end.
    assert!(
        component
            .movement_modifiers(&mut world)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

fn active(snapshot: &Snapshot, id: u16) -> Vec<u16> {
    snapshot.instances[&id]
        .states
        .iter()
        .filter_map(|(&raw, state)| state.active.then_some(raw))
        .collect()
}

#[test]
fn volley_reports_exact_times_and_bakes_linked_ammo_on_finish() {
    let weapon = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug")],&[])}),
            ),
            node(
                1,
                "STUStatescriptStateWeaponVolley",
                Some(0),
                json!({
                    "m_A17BE89B":3,
                    "m_numShotsPerSecond":9,
                    "m_24E351A4":var(53,false),
                    "m_3FFC9EB9":var(231,false),
                    "m_FEC435C6":180
                }),
            ),
        ],
        json!([1]),
        json!([0]),
    );
    let mut root = Root::new(1, 42);
    root.variables.insert(53, Value::Integer(30));
    root.variables.insert(231, Value::Float(0.25));
    let mut component = Component::start(
        Arc::new(store(vec![weapon])),
        42,
        Variables::new(),
        vec![root],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let mut shots = Vec::new();
    for frame in 0..=14 {
        component.advance_frame(frame, &mut EmptyWorld).unwrap();
        shots.extend(component.take_shots().unwrap());
        if frame == 6 {
            let snapshot = component.snapshot().unwrap();
            assert!(snapshot.bindings.is_empty());
            assert_eq!(snapshot.instances[&1].variables[&53], Value::Integer(30));
            assert_eq!(snapshot.resolved_bindings[&(1, 53)], Value::Integer(29));
            assert_eq!(snapshot.resolved_bindings[&(1, 231)], Value::Float(0.25));
        }
    }
    assert_eq!(
        shots
            .iter()
            .map(|shot| (shot.frame, shot.time_ms, shot.ordinal))
            .collect::<Vec<_>>(),
        [(0, 0, 0), (6, 111, 1), (13, 222, 2)]
    );
    assert!(
        shots
            .iter()
            .all(|shot| shot.activation == 1 && shot.activation_ms == 0)
    );
    let node = component.shot_node(&shots[2]).unwrap();
    assert_eq!(
        component.evaluate_shot_config(
            &shots[2],
            node.field("m_FEC435C6").unwrap(),
            &mut EmptyWorld
        ),
        Ok(Value::Integer(180))
    );
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.pending_timers.is_empty());
    assert!(snapshot.resolved_bindings.is_empty());
    assert!(!snapshot.instances[&1].states[&0].active);
    assert_eq!(snapshot.instances[&1].variables[&53], Value::Integer(27));
}

/// leafall903/ORPHEUS (MIT) e5463e3 `WeaponVolley.tick`: a launch
/// `STU_239B571E` volley's spread grows by 1/ramp a shot from the value the
/// volley began with, each tick publishes it on the linked variable until
/// it is full, each shot carries it, and the end bakes it 1 ms later.
#[test]
fn volley_spread_grows_each_shot_and_ticks_until_full() {
    let weapon = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug")],&[])}),
            ),
            node(
                1,
                "STUStatescriptStateWeaponVolley",
                Some(0),
                json!({
                    "m_A17BE89B":6,
                    "m_numShotsPerSecond":10,
                    "m_3FFC9EB9":var(231,false),
                    "m_5EC1EB38":{"$":"STU_239B571E","m_AC20B1C1":4,"m_23C72C36":null}
                }),
            ),
        ],
        json!([1]),
        json!([0]),
    );
    let mut root = Root::new(1, 42);
    root.variables.insert(231, Value::Float(0.125));
    let mut component = Component::start(
        Arc::new(store(vec![weapon])),
        42,
        Variables::new(),
        vec![root],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let mut shots = Vec::new();
    let mut published = Vec::new();
    for frame in 0..=40 {
        component.advance_frame(frame, &mut EmptyWorld).unwrap();
        shots.extend(component.take_shots().unwrap());
        let snapshot = component.snapshot().unwrap();
        let value = snapshot
            .resolved_bindings
            .get(&(1, 231))
            .unwrap_or(&snapshot.instances[&1].variables[&231]);
        if published.last() != Some(value) {
            published.push(value.clone());
        }
    }
    // Shots at 0, 100, ..., 500 ms: 1/8 plus a quarter for each shot so
    // far, the first included, at most 1.
    assert_eq!(
        shots.iter().map(|shot| shot.spread).collect::<Vec<_>>(),
        [0.375, 0.625, 0.875, 1.0, 1.0, 1.0].map(Some)
    );
    assert_eq!(
        published,
        [0.375, 0.625, 0.875, 1.0].map(Value::Float),
        "the linked variable follows each shot until full"
    );
    let snapshot = component.snapshot().unwrap();
    assert!(!snapshot.instances[&1].states[&0].active);
    assert_eq!(snapshot.instances[&1].variables[&231], Value::Float(1.0));
}

#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_helix_uses_its_six_second_cooldown_before_the_next_volley() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let mut world = soldier_world(&graphs);
    let mut component = Component::start(
        Arc::clone(&graphs),
        0xa000_0101,
        player_entity_variables(body, 200.0, 200.0),
        player_roots(body, &graphs).unwrap(),
        1000,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    component.advance_frame(1000, &mut world).unwrap();
    let mut shots = Vec::new();
    let cooldown = Variable {
        scope: Scope::Instance,
        id: 61,
    };
    for frame in 1001..=1411 {
        let buttons = if matches!(frame, 1030 | 1031 | 1040 | 1041 | 1410 | 1411) {
            2
        } else {
            0
        };
        component
            .advance_command(
                Command {
                    frame,
                    buttons,
                    ..Command::default()
                },
                &mut world,
            )
            .unwrap();
        shots.extend(std::mem::take(&mut component.machine.shots));
        let (flags, last_ms, remaining, due) = match frame {
            1029 => (0, 16000, 0.0, None),
            1030 => (5, 16480, 5.985000133514404, Some(22480)),
            1040 => (1, 16480, 5.824999809265137, Some(22480)),
            1404 => (1, 16480, 0.0009999275207519531, Some(22480)),
            1405 => (1, 16480, 0.0, None),
            1410 => (1, 22560, 5.985000133514404, Some(28560)),
            _ => continue,
        };
        let snapshot = component.snapshot().unwrap();
        let ability = &snapshot.instances[&11].states[&31];
        assert_eq!(ability.kind, StateKind::Ability { flags }, "frame {frame}");
        assert_eq!(
            ability.ability_cooldown,
            Some(AbilityCooldown {
                current: if frame == 1029 { 0.0 } else { 6.0 },
                rate: 1.0,
                last_ms,
            }),
            "frame {frame}"
        );
        assert_eq!(
            component.resolved_variable(11, cooldown),
            Some(Value::Float(remaining)),
            "frame {frame}"
        );
        assert_eq!(
            snapshot
                .pending_timers
                .get(&11)
                .into_iter()
                .flatten()
                .find(|timer| timer.raw_state == 31 && timer.parameter == 2)
                .map(|timer| timer.due_ms),
            due,
            "frame {frame}"
        );
    }
    assert_eq!(
        shots
            .iter()
            .map(|shot| (
                shot.instance,
                shot.raw_state,
                shot.time_ms,
                shot.activation,
                shot.ordinal
            ))
            .collect::<Vec<_>>(),
        [(11, 16, 16480, 1, 0), (11, 16, 22560, 2, 0)]
    );
    let shot = &shots[0];
    let node = component.shot_node(shot).unwrap();
    let rocket = crate::weapons::ProjectileBallistics::from_authored(node, |config| {
        component.evaluate_shot_config(shot, config, &mut world)
    })
    .unwrap()
    .unwrap();
    assert_eq!(rocket.definition, Some(0x0400_0000_0000_072c));
    assert_eq!((rocket.speed, rocket.lifetime), (50.0, 2.0));
    assert_eq!(rocket.radius, f64::from(0.2_f32));
    let direct = rocket.direct.unwrap();
    assert_eq!(
        (direct.amount, direct.loadout),
        (40.0, Some(0x0b90_0000_0000_008e))
    );
    let blast = rocket.blast.unwrap();
    assert_eq!(
        (blast.hit.amount, blast.hit.loadout),
        (80.0, Some(0x0b90_0000_0000_008e))
    );
    assert_eq!(blast.rings, [(1.0, 1.0), (3.0, 0.5)]);
    // The explosion's self-damage scale `m_8A3E3883` and its ModifyHealth's
    // push: 4 m/s away and 6 m/s up (each 4 and 6 × Soldier: 76's ruleset
    // value 0x2982, 1.0), at full within 0.5 m, then ×0.75 and ×0.25 at 3 m.
    assert_eq!(blast.self_damage, 0.5);
    let knockback = blast.knockback.unwrap();
    assert_eq!((knockback.away, knockback.upward), (4.0, 6.0));
    assert_eq!(knockback.away_rings, [(0.5, 1.0), (3.0, 0.75)]);
    assert_eq!(knockback.upward_rings, [(0.5, 1.0), (3.0, 0.25)]);
    assert_eq!(knockback.at(0.0), Some((4.0, 6.0)));
    assert_eq!(knockback.at(1.75), Some((3.5, 3.75)));
    assert_eq!(knockback.at(3.5), None);
}

#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_soldier_rifle_holds_releases_and_reloads_on_the_reference_clock() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let mut world = soldier_world(&graphs);
    let mut component = Component::start(
        Arc::clone(&graphs),
        0xa000_0101,
        player_entity_variables(body, 200.0, 200.0),
        player_roots(body, &graphs).unwrap(),
        999,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    component.advance_frame(999, &mut world).unwrap();
    let mut shots = Vec::new();
    let ammo = Variable {
        scope: Scope::Instance,
        id: 53,
    };
    let reload = Variable {
        scope: Scope::Instance,
        id: 51,
    };
    for frame in 1000..=1201 {
        let buttons = match frame {
            1030..=1073 | 1200 => 1,
            1095 => 0x400,
            _ => 0,
        };
        component
            .advance_command(
                Command {
                    frame,
                    buttons,
                    ..Command::default()
                },
                &mut world,
            )
            .unwrap();
        shots.extend(std::mem::take(&mut component.machine.shots));
        let expected = match frame {
            1029 => Some(30),
            1030 | 1036 => Some(29),
            1074 | 1095 | 1141 => Some(23),
            1142 | 1188 => Some(30),
            1201 => Some(29),
            _ => None,
        };
        if let Some(expected) = expected {
            assert_eq!(
                component.resolved_variable(11, ammo),
                Some(Value::Integer(expected)),
                "frame {frame}, rifle states {:?}, diagnostics {:?}",
                active(component.snapshot().unwrap(), 11),
                component
                    .snapshot()
                    .unwrap()
                    .diagnostics
                    .iter()
                    .filter(|d| d.site.graph == 0x0254)
                    .collect::<Vec<_>>()
            );
        }
        match frame {
            1095 | 1142 => assert_eq!(
                component.resolved_variable(11, reload),
                Some(Value::Bool(true))
            ),
            1188 => assert_eq!(
                component.resolved_variable(11, reload),
                Some(Value::Bool(false))
            ),
            _ => (),
        }
    }
    assert_eq!(
        shots
            .iter()
            .map(|shot| (shot.time_ms, shot.activation, shot.ordinal))
            .collect::<Vec<_>>(),
        [
            (16480, 1, 0),
            (16591, 1, 1),
            (16702, 1, 2),
            (16813, 1, 3),
            (16924, 1, 4),
            (17036, 1, 5),
            (17147, 1, 6),
            (19200, 2, 0),
        ]
    );
    assert!(
        shots
            .iter()
            .all(|shot| shot.instance == 11 && shot.raw_state == 10)
    );
    let shot = &shots[0];
    let node = component.shot_node(shot).unwrap();
    let mut evaluate = |config: &Json| component.evaluate_shot_config(shot, config, &mut world);
    let hit = crate::weapons::HitDefinition::from_authored(
        node.field("m_6396149F").unwrap(),
        &mut evaluate,
    )
    .unwrap()
    .unwrap();
    assert_eq!(hit.amount, 20.0);
    assert_eq!(hit.loadout, Some(0x0b90_0000_0000_008d));
    assert_eq!(hit.critical, 2.0);
    assert_eq!(
        hit.falloff,
        Some(crate::weapons::DamageFalloff {
            start: 30.0,
            end: 50.0,
            floor: f64::from(0.3_f32),
        })
    );
    assert_eq!(
        evaluate(node.field("m_FEC435C6").unwrap()),
        Ok(Value::Integer(180))
    );
    // The body is complete now, so the server-facing call answers; the loop
    // above already took every shot.
    assert_eq!(component.take_shots().map(|shots| shots.len()), Some(0));
}

#[test]
fn animation_callback_routes_a_typed_self_message_to_the_live_receiver() {
    let message = json!({"$":"STUConfigVarGameMessage","m_gameMessage":"0x0240000000000001"});
    let source = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug")],&[])}),
            ),
            node(
                1,
                "STUStatescriptStateAnim",
                Some(0),
                json!({
                    "m_86701C3E":{"$":"STUConfigVarFloat","m_value":0.025},
                    "m_A3B86BB0":plug(&[(2,"m_inPlug")],&[]),
                    "m_onFinishedPlug":plug(&[(3,"m_inPlug")],&[])
                }),
            ),
            node(
                2,
                "STUStatescriptActionSendGameMessage",
                None,
                json!({
                    "m_gameMessage":message,
                    "m_params":[{"m_B5051BCE":"0x0A40000000000123","m_value":true}]
                }),
            ),
            node(
                3,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(101,false),"m_value":true}),
            ),
        ],
        json!([1]),
        json!([0]),
    );
    let receiver = graph(
        43,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug")],&[])}),
            ),
            node(
                1,
                "STUStatescriptStateGameMessageEntry",
                Some(0),
                json!({
                    "m_gameMessage":message,
                    "m_33DA5163":plug(&[(2,"m_beginPlug")],&[1]),
                    "m_452862E6":plug(&[(3,"m_beginPlug")],&[2]),
                    "m_params":[{"m_B5051BCE":"0x0A40000000000123","m_out_Var":var(7,false)}]
                }),
            ),
            node(2, "STUStatescriptState", Some(1), json!({})),
            node(3, "STU_691BFA55", Some(2), json!({})),
        ],
        json!([1, 2, 3]),
        json!([0]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![source, receiver])),
        42,
        Variables::new(),
        vec![Root::new(1, 42), Root::new(2, 43)],
        100,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let initial = component.snapshot().unwrap();
    assert_eq!(initial.instances[&1].states[&0].kind, StateKind::Anim);
    assert_eq!(active(initial, 2), [0, 1]);
    assert_eq!(
        initial.pending_timers[&1],
        [
            PendingTimer {
                due_ms: 1625,
                raw_state: 0,
                finish: false,
                parameter: 1
            },
            PendingTimer {
                due_ms: 1625,
                raw_state: 0,
                finish: true,
                parameter: 0
            },
        ]
    );
    component.advance_frame(100, &mut EmptyWorld).unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 2), [0, 1]);
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(snapshot.instances[&1].variables[&101], Value::Bool(true));
    assert!(!snapshot.instances[&1].states[&0].active);
    assert_eq!(active(snapshot, 2), [0, 2]);
    assert_eq!(snapshot.instances[&2].variables[&7], Value::Bool(true));
    assert_eq!(snapshot.instances[&2].states[&2].kind, StateKind::Effect);
    assert_eq!(snapshot.instances[&2].states[&2].start_ms, 1625);
}

#[test]
fn buffered_action_press_uses_the_shared_clock_and_releases_after_one_millisecond() {
    let mut fixture = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntryGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000001"},
                    "m_outPlug":plug(&[(1,"m_beginPlug")],&[]),
                }),
            ),
            node(
                1,
                "STUStatescriptStateLogicalButton",
                Some(0),
                json!({
                    "m_logicalButton":{"$":"STUConfigVarLogicalButton","m_logicalButton":15},
                    "m_246438AD":{"$":"STUConfigVarFloat","m_value":0.03},
                    "m_onGoingDownPlug":plug(&[(2,"m_inPlug")],&[]),
                    "m_onComingUpPlug":plug(&[(3,"m_inPlug")],&[]),
                }),
            ),
            node(
                2,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(10,false), "m_value": true,
                }),
            ),
            node(
                3,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(10,false), "m_value": false,
                }),
            ),
        ],
        json!([1]),
        json!([0]),
    );
    fixture["fields"]["m_680A2CB2"] = json!([0]);
    let graphs = Arc::new(store(vec![fixture]));
    for (dead, delay) in [(false, 0), (false, 2), (true, 0)] {
        let mut component = Component::start(
            Arc::clone(&graphs),
            42,
            Variables::new(),
            vec![Root::new(1, 42)],
            100,
            &mut EmptyWorld,
            MAX_INIT_WORK,
        )
        .unwrap();
        component.set_owner_dead(dead);
        component
            .advance_command(
                Command {
                    frame: 100,
                    action: 1,
                    ..Command::default()
                },
                &mut EmptyWorld,
            )
            .unwrap();
        if delay != 0 {
            component
                .advance_frame(100 + delay, &mut EmptyWorld)
                .unwrap();
        }
        let buffered = !dead && delay == 0;
        assert_eq!(
            component
                .queue_message(1, 0x0240_0000_0000_0001, Variables::new())
                .unwrap(),
            1
        );
        component.run_now(&mut EmptyWorld).unwrap();
        let snapshot = component.snapshot().unwrap();
        let instance = &snapshot.instances[&1];
        assert_eq!(instance.states[&0].activation, 1);
        assert_eq!(
            instance.states[&0].kind,
            StateKind::LogicalButton { down: buffered }
        );
        if buffered {
            assert_eq!(instance.variables[&10], Value::Bool(true));
            assert_eq!(
                snapshot.pending_timers[&1],
                [PendingTimer {
                    due_ms: 1616,
                    raw_state: 0,
                    finish: false,
                    parameter: 1
                }]
            );
            component.advance_frame(101, &mut EmptyWorld).unwrap();
            let snapshot = component.snapshot().unwrap();
            assert_eq!(
                snapshot.instances[&1].states[&0].kind,
                StateKind::LogicalButton { down: false }
            );
            assert_eq!(snapshot.instances[&1].variables[&10], Value::Bool(false));
            assert!(snapshot.pending_timers.is_empty());
        } else {
            assert!(instance.variables.get(&10).is_none());
            assert!(snapshot.pending_timers.is_empty());
        }
    }
}

#[test]
#[ignore = "requires the derived game data's bot graph bundle (graph 0B92)"]
fn firing_placement_reset_and_timer_boundaries_match_python() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
    assert!(
        graphs.graph(0x0b92).is_some(),
        "the bot graph bundle lacks Practice Range graph 0B92"
    );
    let mut component = Component::start(
        graphs,
        0xa100_0000,
        Variables::new(),
        vec![Root::new(1, 0x0b92)],
        100,
        &mut ReferenceWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    // Python BotWorld with no selected AI target, frame 100, scoped RESET.
    assert_eq!(
        component
            .queue_message(1, 0x0240_0000_0000_0013, Variables::new())
            .unwrap(),
        1
    );
    component.run_now(&mut ReferenceWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(active(snapshot, 1), [5, 12]);
    assert_eq!(snapshot.instances[&1].variables[&889], Value::Bool(false));
    assert_eq!(
        snapshot.pending_timers[&1],
        [
            PendingTimer {
                due_ms: 2100,
                raw_state: 5,
                finish: false,
                parameter: 1
            },
            PendingTimer {
                due_ms: 4350,
                raw_state: 12,
                finish: false,
                parameter: 1
            },
        ]
    );
    for frame in 101..=130 {
        component.advance_frame(frame, &mut ReferenceWorld).unwrap();
    }
    assert_eq!(component.now_ms(), 2095);
    assert_eq!(active(component.snapshot().unwrap(), 1), [5, 12]);
    component.advance_frame(131, &mut ReferenceWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(active(snapshot, 1), [12, 0]);
    assert_eq!(
        snapshot.instances[&1].states[&0].kind,
        StateKind::AiCandidate { selected: false }
    );
    assert_eq!(snapshot.instances[&1].states[&0].start_ms, 2100);
    assert_eq!(snapshot.instances[&1].variables[&889], Value::Bool(false));
    for frame in 132..=270 {
        component.advance_frame(frame, &mut ReferenceWorld).unwrap();
    }
    assert_eq!(component.now_ms(), 4335);
    assert_eq!(active(component.snapshot().unwrap(), 1), [12, 0]);
    component.advance_frame(271, &mut ReferenceWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(active(snapshot, 1), [0]);
    assert_eq!(snapshot.instances[&1].variables[&889], Value::Bool(true));
    assert!(snapshot.pending_timers.is_empty());
}

#[test]
#[ignore = "requires the derived game data's bot graph bundle"]
fn controller_and_training_bot_supported_startup_edges_match_python() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap();
    let run = |entity, roots| {
        initialize(
            &graphs,
            entity,
            Variables::new(),
            roots,
            &mut ReferenceWorld,
            MAX_INIT_WORK,
        )
    };
    let hud = run(0xa000_0100, vec![Root::new(1, 0x20e4)]).unwrap();
    assert!(hud.initialization_complete(), "{:?}", hud.diagnostics);
    assert_eq!(active(&hud, 1), [25]);

    let mut controller = Root::new(1, 0x13c1);
    controller.variables.insert(8419, Value::Bool(true));
    let owner = run(0xa000_0100, vec![controller]).unwrap();
    let instances: Vec<_> = owner
        .instances
        .iter()
        .map(|(&id, instance)| (id, instance.graph, instance.parent))
        .collect();
    // Python Component with the practice controller and v8419 enabled.
    assert_eq!(
        instances,
        [
            (1, 0x13c1, None),
            (2, 0x20e4, Some((1, 42))),
            (3, 0x288a, Some((1, 43))),
            (4, 0x288b, Some((3, 0))),
            (5, 0x288d, Some((3, 1))),
        ]
    );
    assert_eq!(active(&owner, 2), [25]);
    assert!(!owner.initialization_complete());

    let body = graphs.entity_body(0x0400_0000_0000_1403).unwrap();
    {
        let roots = body_roots(body).unwrap();
        assert_eq!(
            roots
                .iter()
                .map(|root| (root.id, root.graph))
                .collect::<Vec<_>>(),
            [
                (1, 0x0033),
                (2, 0x0ba8),
                (3, 0x0be2),
                (4, 0x0ba3),
                (5, 0x174f),
                (6, 0x2206)
            ],
        );
        assert_eq!(roots[0].variables[&199], Value::Integer(200));
        let bot = run(0xa100_0000, vec![Root::new(1, 0x0c18)]).unwrap();
        assert_eq!(bot.instances[&2].graph, 0x0d24);
        assert_eq!(bot.instances[&2].parent, Some((1, 3)));
        assert_eq!(active(&bot, 2), [56, 76]);
        assert!(!bot.initialization_complete());
    }
}

#[test]
fn ai_control_state_finishes_at_begin_time_only_for_an_absent_owner_relation() {
    const OWNER: u32 = 0xa000_0101;
    const FRAME: u32 = 100;
    struct AiWorld(Option<u32>);
    impl World for AiWorld {
        fn related_entity(
            &mut self,
            site: Site,
            relation: EntityRelation,
            entity: u32,
        ) -> Option<u32> {
            assert_eq!(site.node, Some(1));
            assert_eq!(relation, EntityRelation::AiControlEntity);
            assert_eq!(entity, OWNER);
            self.0
        }
    }

    let owner = json!({"$":"STU_B5A0CAF0","m_EE729DCB":0});
    for (target, relation, finishes) in [
        (owner.clone(), Some(0), true),
        (owner.clone(), None, false),
        (owner, Some(OWNER + 1), false),
        (var(100, true), Some(0), false),
    ] {
        let graphs = Arc::new(store(vec![graph(
            42,
            vec![
                node(
                    0,
                    "STUStatescriptEntry",
                    None,
                    json!({"m_outPlug":plug(&[(1,"m_beginPlug")], &[])}),
                ),
                node(
                    1,
                    "STU_066B4605",
                    Some(0),
                    json!({
                        "m_00D49978":target,
                        "m_D66E7D68":{"$":"STUConfigVarInt","m_value":0},
                        "m_F2810377":{"$":"STUConfigVarIdentifier","m_identifier":"0x0D8000000000023A"},
                        "m_F8585ABF":{"$":"STUConfigVarBool","m_value":0},
                        "m_priority":{"$":"STU_8A491388","m_priority":"0x0C40000000000002"},
                        "m_onBeginPlug":plug(&[(2,"m_inPlug")], &[]),
                        "m_onFinishedPlug":plug(&[(3,"m_inPlug")], &[]),
                        "m_onAbortPlug":plug(&[(4,"m_inPlug")], &[]),
                    }),
                ),
                node(
                    2,
                    "STUStatescriptActionSetVar",
                    None,
                    json!({"m_out_Var":var(100,false), "m_value":true}),
                ),
                node(
                    3,
                    "STUStatescriptActionSetVar",
                    None,
                    json!({"m_out_Var":var(101,false), "m_value":var(100,false)}),
                ),
                node(
                    4,
                    "STUStatescriptActionSetVar",
                    None,
                    json!({"m_out_Var":var(101,false), "m_value":false}),
                ),
            ],
            json!([1]),
            json!([0]),
        )]));
        let component = Component::start(
            graphs,
            OWNER,
            Variables::new(),
            vec![Root::new(1, 42)],
            FRAME,
            &mut AiWorld(relation),
            MAX_INIT_WORK,
        )
        .unwrap();
        let snapshot = component.snapshot().unwrap();
        let instance = &snapshot.instances[&1];
        assert_eq!(component.now_ms(), frame_start(FRAME));
        assert!(snapshot.pending_timers.is_empty());
        if finishes {
            assert!(snapshot.initialization_complete());
            let state = &instance.states[&0];
            assert!(!state.active);
            assert_eq!(state.activation, 1);
            assert_eq!(state.start_ms, frame_start(FRAME));
            assert_eq!(instance.variables[&101], Value::Bool(true));
        } else {
            assert!(matches!(
                snapshot.diagnostics.as_slice(),
                [Diagnostic { issue: Issue::UnsupportedState(class), .. }]
                    if class == "STU_066B4605"
            ));
            assert!(instance.states.is_empty() && instance.variables.is_empty());
        }
    }
}

/// An entry for `message` (`0x0240…` low bits) that links to `target`.
fn on_message(position: u16, message: u64, target: u16, input: &str) -> Json {
    node(
        position,
        "STUStatescriptEntryGameMessage",
        None,
        json!({
            "m_gameMessage":{"m_gameMessage":format!("0x{:016X}", 0x0240_0000_0000_0000 | message)},
            "m_outPlug":plug(&[(target, input)], &[]),
        }),
    )
}

fn set_entity(position: u16, id: u32, value: Json) -> Json {
    node(
        position,
        "STUStatescriptActionSetVar",
        None,
        json!({"m_out_Var":var(id,true), "m_value":value}),
    )
}

fn entry(links: &[(u16, &str)]) -> Json {
    node(
        0,
        "STUStatescriptEntry",
        None,
        json!({"m_outPlug":plug(links, &[])}),
    )
}

fn start(graphs: Store, frame: u32, world: &mut impl World) -> Component {
    start_with(graphs, Variables::new(), frame, world)
}

type Store = Arc<GraphStore>;

fn start_with(
    graphs: Store,
    variables: Variables,
    frame: u32,
    world: &mut impl World,
) -> Component {
    Component::start(
        graphs,
        0xa000_0101,
        variables,
        vec![Root::new(1, 42)],
        frame,
        world,
        MAX_INIT_WORK,
    )
    .unwrap()
}

fn message(component: &mut Component, message: u64, world: &mut impl World) {
    component
        .queue_message(1, 0x0240_0000_0000_0000 | message, Variables::new())
        .unwrap();
    component.run_now(world).unwrap();
}

#[test]
fn defer_runs_its_plugs_highest_priority_first_and_once() {
    let defer = |position, state, priority: f64, out: u16, sub: u16| {
        node(
            position,
            "STU_9C9A422A",
            Some(state),
            json!({
                "m_priority":{"$":"STUConfigVarFloat","m_value":priority},
                "m_FD0C0646":plug(&[(out,"m_inPlug")], &[]),
                "m_8E484B2E":plug(&[(sub,"m_beginPlug")], &[sub - 3]),
            }),
        )
    };
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug"), (2, "m_beginPlug")]),
            defer(1, 0, 1.0, 3, 5),
            defer(2, 1, 2.0, 4, 6),
            set_entity(3, 100, json!(1)),
            set_entity(4, 100, json!(2)),
            node(5, "STUStatescriptState", Some(2), json!({})),
            node(6, "STUStatescriptState", Some(3), json!({})),
            on_message(7, 1, 1, ABORT_PLUG),
        ],
        json!([1, 2, 5, 6]),
        json!([0, 7]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    // The higher priority ran first, so the lower one's write is last.
    assert_eq!(snapshot.variables[&100], Value::Integer(1));
    // The higher priority's subgraph began first, too.
    assert_eq!(active(snapshot, 1), [0, 1, 3, 2]);
    component
        .set_entity_variable(100, Value::Null, &mut EmptyWorld)
        .unwrap();
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    assert_eq!(component.snapshot().unwrap().variables[&100], Value::Null);
    // Ending a Defer exits its subgraph.
    message(&mut component, 1, &mut EmptyWorld);
    assert_eq!(active(component.snapshot().unwrap(), 1), [1, 3]);
}

#[test]
fn pulser_counts_every_begin_and_marks_pulses_after_the_first_frame_fresh() {
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(1, "STU_B9898052", Some(0), json!({})),
            on_message(2, 1, 1, "m_beginPlug"),
            on_message(3, 2, 1, ABORT_PLUG),
        ],
        json!([1]),
        json!([0, 2, 3]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    let kind = |component: &Component| component.snapshot().unwrap().instances[&1].states[&0].kind;
    assert_eq!(
        kind(&component),
        StateKind::Pulser {
            pulses: 1,
            pulsed_ms: None
        }
    );
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    message(&mut component, 1, &mut EmptyWorld);
    let pulsed = frame_start(101) + FRAME_MS - 1;
    assert_eq!(
        kind(&component),
        StateKind::Pulser {
            pulses: 2,
            pulsed_ms: Some(pulsed)
        }
    );
    // The count carries over to the next activation.
    message(&mut component, 2, &mut EmptyWorld);
    message(&mut component, 1, &mut EmptyWorld);
    let state = &component.snapshot().unwrap().instances[&1].states[&0];
    assert!(state.active);
    assert_eq!(
        state.kind,
        StateKind::Pulser {
            pulses: 3,
            pulsed_ms: Some(pulsed)
        }
    );
}

/// A logical button (20, the primary fire) whose down subgraph is state 1,
/// and a `DisableLogicalButton` for it begun by message 1 and aborted by
/// message 2.
fn disabled_button_fixture(ignore: Option<bool>) -> Store {
    let mut button = json!({
        "m_logicalButton":{"$":"STUConfigVarLogicalButton","m_logicalButton":20},
        "m_80AF45FB":plug(&[(2,"m_beginPlug")], &[1]),
    });
    if let Some(ignore) = ignore {
        button["m_53C4B6A6"] = json!({"$":"STUConfigVarBool","m_value":ignore});
    }
    let mut fixture = graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(1, "STUStatescriptStateLogicalButton", Some(0), button),
            node(2, "STUStatescriptState", Some(1), json!({})),
            node(
                3,
                "STU_E4A30BCF",
                Some(2),
                json!({"m_logicalButton":{"$":"STUConfigVarLogicalButton","m_logicalButton":20}}),
            ),
            on_message(4, 1, 3, "m_beginPlug"),
            on_message(5, 2, 3, ABORT_PLUG),
        ],
        json!([1, 2, 3]),
        json!([0, 4, 5]),
    );
    fixture["fields"]["m_680A2CB2"] = json!([0]);
    Arc::new(store(vec![fixture]))
}

#[test]
fn disabled_button_brings_held_buttons_up_and_drops_presses_until_it_ends() {
    let press = |component: &mut Component, frame, buttons| {
        component
            .advance_command(
                Command {
                    frame,
                    buttons,
                    ..Command::default()
                },
                &mut EmptyWorld,
            )
            .unwrap();
    };
    let down = |component: &Component| {
        let state = &component.snapshot().unwrap().instances[&1].states[&0];
        (
            state.kind,
            component.snapshot().unwrap().instances[&1]
                .states
                .get(&1)
                .is_some_and(|state| state.active),
        )
    };
    let mut component = start(disabled_button_fixture(None), 100, &mut EmptyWorld);
    press(&mut component, 101, 1);
    assert_eq!(
        down(&component),
        (StateKind::LogicalButton { down: true }, true)
    );
    message(&mut component, 1, &mut EmptyWorld);
    assert_eq!(
        down(&component),
        (StateKind::LogicalButton { down: false }, false)
    );
    // A press while blocked does nothing.
    press(&mut component, 102, 0);
    press(&mut component, 103, 1);
    assert_eq!(
        down(&component),
        (StateKind::LogicalButton { down: false }, false)
    );
    message(&mut component, 2, &mut EmptyWorld);
    assert_eq!(
        down(&component),
        (StateKind::LogicalButton { down: true }, true)
    );
    assert!(component.snapshot().unwrap().initialization_complete());

    // A button that ignores the block stays down.
    let mut component = start(disabled_button_fixture(Some(true)), 100, &mut EmptyWorld);
    press(&mut component, 101, 1);
    message(&mut component, 1, &mut EmptyWorld);
    assert_eq!(
        down(&component),
        (StateKind::LogicalButton { down: true }, true)
    );
}

#[test]
fn ai_fire_holds_its_slot_button_until_it_ends() {
    struct Buttons(Option<u8>);
    impl World for Buttons {
        fn ai_weapon_button(&mut self, slot: i64) -> Option<u8> {
            assert_eq!(slot, 3);
            self.0
        }
    }
    let mut fixture = graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STUStatescriptStateLogicalButton",
                Some(0),
                json!({
                    "m_logicalButton":{"$":"STUConfigVarInt","m_value":42},
                    "m_80AF45FB":plug(&[(2,"m_beginPlug")], &[1]),
                }),
            ),
            node(2, "STUStatescriptState", Some(1), json!({})),
            node(
                3,
                "STU_D7F9E1C3",
                Some(2),
                json!({
                    "m_14EDDDA7":{"$":"STU_B5A0CAF0"},
                    "m_20BCEE62":{"$":"STUConfigVarBool","m_value":1},
                    "m_A60150E2":{"$":"STUConfigVarInt","m_value":3},
                }),
            ),
            on_message(4, 1, 3, "m_beginPlug"),
            on_message(5, 2, 3, ABORT_PLUG),
        ],
        json!([1, 2, 3]),
        json!([0, 4, 5]),
    );
    fixture["fields"]["m_680A2CB2"] = json!([0]);
    let graphs = Arc::new(store(vec![fixture]));
    for button in [Some(42), None] {
        let mut world = Buttons(button);
        let mut component = start(Arc::clone(&graphs), 100, &mut world);
        message(&mut component, 1, &mut world);
        let snapshot = component.snapshot().unwrap();
        assert!(
            snapshot.initialization_complete(),
            "{:?}",
            snapshot.diagnostics
        );
        assert_eq!(
            snapshot.instances[&1].states[&2].kind,
            StateKind::AiFire { button }
        );
        assert_eq!(
            active(snapshot, 1),
            if button.is_some() {
                vec![0, 2, 1]
            } else {
                vec![0, 2]
            }
        );
        message(&mut component, 2, &mut world);
        let snapshot = component.snapshot().unwrap();
        assert_eq!(active(snapshot, 1), [0]);
        assert_eq!(
            snapshot.instances[&1].states[&2].kind,
            StateKind::AiFire { button: None }
        );
    }
}

#[test]
fn server_frame_wait_finishes_after_its_frames_and_is_recorded() {
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STU_E3CEF6B9",
                Some(0),
                json!({"m_6B2EB0B2":{"$":"STUConfigVarInt","m_value":3}}),
            ),
        ],
        json!([1]),
        json!([0]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    let snapshot = component.snapshot().unwrap();
    assert_eq!(
        snapshot.pending_timers[&1],
        [PendingTimer {
            due_ms: frame_start(103),
            raw_state: 0,
            finish: true,
            parameter: 0
        }]
    );
    assert_eq!(snapshot.assumptions[0].class, "STU_E3CEF6B9");
    component.advance_frame(102, &mut EmptyWorld).unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
    component.advance_frame(103, &mut EmptyWorld).unwrap();
    assert!(active(component.snapshot().unwrap(), 1).is_empty());
}

#[test]
fn wait_with_a_live_timeout_rechecks_it_every_tick() {
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STUStatescriptStateWait",
                Some(0),
                json!({"m_timeout":var(100,true), "m_BF1A93B0":true}),
            ),
        ],
        json!([1]),
        json!([0]),
    )]));
    let mut component = start_with(
        graphs,
        Variables::from([(100, Value::Float(1.0))]),
        0,
        &mut EmptyWorld,
    );
    for frame in 1..=30 {
        component.advance_frame(frame, &mut EmptyWorld).unwrap();
    }
    let snapshot = component.snapshot().unwrap();
    assert_eq!(active(snapshot, 1), [0]);
    assert!(snapshot.pending_timers.is_empty());
    // 495 ms have passed: a shorter timeout ends it at the next tick.
    component
        .set_entity_variable(100, Value::Float(0.25), &mut EmptyWorld)
        .unwrap();
    component.advance_frame(31, &mut EmptyWorld).unwrap();
    assert!(active(component.snapshot().unwrap(), 1).is_empty());
}

#[test]
fn move_to_exposes_only_a_fixed_target_while_active() {
    let fixture = |mode: Json| {
        Arc::new(store(vec![graph(
            42,
            vec![
                entry(&[(1, "m_beginPlug")]),
                node(
                    1,
                    "STUStatescriptStateMoveTo",
                    Some(0),
                    json!({
                        "m_B060F688":mode,
                        "m_speed":{"$":"STUConfigVarFloat","m_value":5.5},
                        "m_865F3D01":{"$":"STUConfigVarVec3","m_value":{"X":1,"Y":2,"Z":-3}},
                    }),
                ),
                on_message(2, 1, 1, ABORT_PLUG),
            ],
            json!([1]),
            json!([0, 2]),
        )]))
    };
    let mut component = start(fixture(json!(0)), 100, &mut EmptyWorld);
    assert_eq!(
        component.move_requests(),
        [MoveRequest {
            instance: 1,
            raw_state: 0,
            target: [1.0, 2.0, -3.0],
            speed: 5.5
        }]
    );
    message(&mut component, 1, &mut EmptyWorld);
    assert!(component.move_requests().is_empty());
    for mode in [json!(1), Json::Null] {
        let component = start(fixture(mode), 100, &mut EmptyWorld);
        assert!(component.move_requests().is_empty());
        assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
    }
}

/// State 0 of `class` with its begin, finish, abort and end plugs setting
/// entity variables 100 to 1, 101 to 2, 101 to 3 and 102 to 4; nothing begins it.
fn external_state_fixture(class: &str, mut fields: Json) -> Store {
    fields["m_onBeginPlug"] = plug(&[(2, "m_inPlug")], &[]);
    fields["m_onFinishedPlug"] = plug(&[(3, "m_inPlug")], &[]);
    fields["m_onAbortPlug"] = plug(&[(4, "m_inPlug")], &[]);
    fields["m_onEndPlug"] = plug(&[(5, "m_inPlug")], &[]);
    Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[]),
            node(1, class, Some(0), fields),
            set_entity(2, 100, json!(1)),
            set_entity(3, 101, json!(2)),
            set_entity(4, 101, json!(3)),
            set_entity(5, 102, json!(4)),
        ],
        json!([1]),
        json!([0]),
    )]))
}

#[test]
fn external_begin_and_end_run_the_state_and_its_plugs_now() {
    let mut component = start(
        external_state_fixture("STU_40E05F6B", json!({})),
        100,
        &mut EmptyWorld,
    );
    let value =
        |component: &Component, id| component.snapshot().unwrap().variables.get(&id).cloned();
    assert!(active(component.snapshot().unwrap(), 1).is_empty());
    for (instance, state) in [(1, 1), (2, 0)] {
        assert!(
            !component
                .begin_state(instance, state, &mut EmptyWorld)
                .unwrap()
        );
    }
    assert!(!component.end_state(1, 0, true, &mut EmptyWorld).unwrap());
    assert!(component.begin_state(1, 0, &mut EmptyWorld).unwrap());
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(active(snapshot, 1), [0]);
    assert_eq!(snapshot.instances[&1].states[&0].activation, 1);
    assert_eq!(value(&component, 100), Some(Value::Integer(1)));
    // Already on: nothing happens.
    assert!(!component.begin_state(1, 0, &mut EmptyWorld).unwrap());
    assert!(component.end_state(1, 0, true, &mut EmptyWorld).unwrap());
    assert!(active(component.snapshot().unwrap(), 1).is_empty());
    assert_eq!(value(&component, 101), Some(Value::Integer(2)));
    assert_eq!(value(&component, 102), Some(Value::Integer(4)));
    assert!(!component.end_state(1, 0, true, &mut EmptyWorld).unwrap());
    // An abort follows the abort plug.
    assert!(component.begin_state(1, 0, &mut EmptyWorld).unwrap());
    assert!(component.end_state(1, 0, false, &mut EmptyWorld).unwrap());
    assert_eq!(value(&component, 101), Some(Value::Integer(3)));
    assert_eq!(
        component.snapshot().unwrap().instances[&1].states[&0].activation,
        2
    );
}

#[test]
fn external_begin_leaves_events_it_queues_for_the_next_advance() {
    // A vibration queues its own finish at the current clock when it begins.
    let mut component = start(
        external_state_fixture("STUStatescriptStateGamePadVibration", json!({})),
        100,
        &mut EmptyWorld,
    );
    assert!(component.begin_state(1, 0, &mut EmptyWorld).unwrap());
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(active(snapshot, 1).is_empty());
    assert_eq!(snapshot.variables[&101], Value::Integer(2));
}

#[test]
fn finished_movement_ends_the_move_to_state_at_the_next_advance() {
    let move_to = || {
        external_state_fixture(
            "STUStatescriptStateMoveTo",
            json!({
                "m_B060F688":0,
                "m_speed":{"$":"STUConfigVarFloat","m_value":5.5},
                "m_865F3D01":{"$":"STUConfigVarVec3","m_value":{"X":1,"Y":2,"Z":-3}},
            }),
        )
    };
    for (arrived, plug_value) in [(true, 2), (false, 3)] {
        let mut component = start(move_to(), 100, &mut EmptyWorld);
        assert!(component.begin_state(1, 0, &mut EmptyWorld).unwrap());
        assert_eq!(component.move_requests().len(), 1);
        let delay = frame_start(101) - component.now_ms();
        component
            .finish_move(1, 0, arrived, delay, &mut EmptyWorld)
            .unwrap();
        let snapshot = component.snapshot().unwrap();
        assert_eq!(active(snapshot, 1), [0]);
        // A finish request is a pending timer of the owner frame; an abort is not.
        let expected = if arrived {
            vec![PendingTimer {
                due_ms: frame_start(101),
                raw_state: 0,
                finish: true,
                parameter: 0,
            }]
        } else {
            Vec::new()
        };
        assert_eq!(
            snapshot.pending_timers.get(&1).cloned().unwrap_or_default(),
            expected
        );
        component.advance_frame(101, &mut EmptyWorld).unwrap();
        let snapshot = component.snapshot().unwrap();
        assert!(active(snapshot, 1).is_empty());
        assert_eq!(snapshot.variables[&101], Value::Integer(plug_value));
        assert!(component.move_requests().is_empty());
    }
    // A missing instance takes no request.
    let mut component = start(move_to(), 100, &mut EmptyWorld);
    component
        .finish_move(7, 0, true, 0, &mut EmptyWorld)
        .unwrap();
    assert_eq!(component.next_due_ms(), None);
}

#[test]
fn cycle_index_keeps_the_first_candidate_its_condition_accepts() {
    // The candidate is every parameter the condition reads: candidate < 2.
    let below_two = json!({"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[13,0,6,0,34,0],
        "m_dynamicVars":[{"$":"STUConfigVarDynamic","m_identifier":1}],
        "m_D99EF254":[2.0],
    }});
    let never = json!({"$":"STUConfigVarBool","m_value":false});
    for (backwards, condition, expected) in [
        (false, &below_two, 0),
        (true, &below_two, 1),
        (false, &never, -1),
    ] {
        let mut root = Root::new(1, 42);
        root.variables.insert(100, Value::Integer(2));
        let graphs = store(vec![graph(
            42,
            vec![
                entry(&[(1, "m_inPlug")]),
                node(
                    1,
                    "STU_BC5E4622",
                    None,
                    json!({
                        "m_D10618D1":{"$":"STUConfigVarInt","m_value":4},
                        "m_339A63DB":var(100,false),
                        "m_C78954CB":{"$":"STUConfigVarBool","m_value":backwards},
                        "m_condition":condition,
                        "m_outPlug":plug(&[(2,"m_inPlug")], &[]),
                    }),
                ),
                set_entity(2, 101, json!(true)),
            ],
            json!([]),
            json!([0]),
        )]);
        let snapshot = initialize(
            &graphs,
            42,
            Variables::new(),
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
        assert_eq!(
            snapshot.instances[&1].variables[&100],
            Value::Integer(expected)
        );
        assert_eq!(snapshot.variables[&101], Value::Bool(true));
    }
}

#[test]
fn conditions_branch_on_containers_held_buttons_and_a_state_age_without_a_state() {
    let condition = |position, class: &str, mut fields: Json, yes: u16, no: u16| {
        fields["m_truePlug"] = plug(&[(yes, "m_inPlug")], &[]);
        fields["m_falsePlug"] = plug(&[(no, "m_inPlug")], &[]);
        node(position, class, None, fields)
    };
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_inPlug"), (2, "m_inPlug"), (3, "m_inPlug")]),
            condition(
                1,
                "STU_821116A6",
                json!({"m_814F8EC3":var(90,true), "m_value":{"$":"STUConfigVarInt","m_value":2}}),
                4,
                5,
            ),
            condition(
                2,
                "STU_7DEB522D",
                json!({"m_logicalButton":{"$":"STUConfigVarLogicalButton","m_logicalButton":20}}),
                6,
                7,
            ),
            condition(3, "STU_4E2BCAAB", json!({}), 8, 9),
            set_entity(4, 100, json!(true)),
            set_entity(5, 100, json!(false)),
            set_entity(6, 101, json!(true)),
            set_entity(7, 101, json!(false)),
            set_entity(8, 102, json!(true)),
            set_entity(9, 102, json!(false)),
            on_message(10, 1, 2, "m_inPlug"),
        ],
        json!([]),
        json!([0, 10]),
    )]));
    let variables =
        Variables::from([(90, Value::Array(vec![Value::Integer(1), Value::Float(2.0)]))]);
    let mut component = start_with(graphs, variables, 100, &mut EmptyWorld);
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(snapshot.variables[&100], Value::Bool(true));
    assert_eq!(snapshot.variables[&101], Value::Bool(false));
    assert_eq!(snapshot.variables[&102], Value::Bool(false));
    // StateElapsed with no age and no state is false (upstream
    // leafall903/ORPHEUS `state_elapsed`); it is no longer an assumption.
    assert!(
        snapshot.assumptions.is_empty(),
        "{:?}",
        snapshot.assumptions
    );
    component
        .advance_command(
            Command {
                frame: 101,
                buttons: 1,
                ..Command::default()
            },
            &mut EmptyWorld,
        )
        .unwrap();
    message(&mut component, 1, &mut EmptyWorld);
    assert_eq!(
        component.snapshot().unwrap().variables[&101],
        Value::Bool(true)
    );
}

#[test]
fn keyed_set_var_replaces_equal_keys_and_raycast_only_follows_its_plug() {
    let keyed = |position, key: i64, value: i64, next: u16| {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({
                "m_out_Var":var(100,true),
                "m_key":{"$":"STUConfigVarInt","m_value":key},
                "m_value":{"$":"STUConfigVarInt","m_value":value},
                "m_outPlug":plug(&[(next,"m_inPlug")], &[]),
            }),
        )
    };
    let graphs = store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_inPlug")]),
            keyed(1, 7, 1, 2),
            keyed(2, 8, 2, 3),
            keyed(3, 7, 3, 4),
            node(
                4,
                "STU_08D6BA6E",
                None,
                json!({
                    "m_out_HitSomething":var(101,true),
                    "m_outPlug":plug(&[(5,"m_inPlug")], &[]),
                }),
            ),
            set_entity(5, 102, json!(true)),
        ],
        json!([]),
        json!([0]),
    )]);
    let snapshot = initialize(
        &graphs,
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(
        snapshot.variables[&100],
        Value::Map(vec![
            (Value::Integer(8), Value::Integer(2)),
            (Value::Integer(7), Value::Integer(3)),
        ])
    );
    assert!(!snapshot.variables.contains_key(&101));
    assert_eq!(snapshot.variables[&102], Value::Bool(true));
    assert_eq!(
        snapshot
            .assumptions
            .iter()
            .map(|item| (item.class.as_str(), item.answer))
            .collect::<Vec<_>>(),
        [("STU_08D6BA6E", "no cast")]
    );
}

#[test]
fn stop_ability_stops_every_running_ability_of_the_instance() {
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STUStatescriptStateAbility",
                Some(0),
                json!({"m_6EEEC0AA":plug(&[(2,"m_inPlug")], &[])}),
            ),
            set_entity(2, 100, json!(true)),
            node(
                3,
                "STU_187D5125",
                None,
                json!({"m_outPlug":plug(&[(4,"m_inPlug")], &[])}),
            ),
            set_entity(4, 101, json!(true)),
            on_message(5, 1, 3, "m_inPlug"),
        ],
        json!([1]),
        json!([0, 5]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    let flags = |component: &Component| component.snapshot().unwrap().instances[&1].states[&0].kind;
    assert_eq!(
        flags(&component),
        StateKind::Ability {
            flags: ABILITY_RUNNING
        }
    );
    message(&mut component, 1, &mut EmptyWorld);
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.variables[&100], Value::Bool(true));
    assert_eq!(snapshot.variables[&101], Value::Bool(true));
    assert_eq!(flags(&component), StateKind::Ability { flags: 0 });
}

#[test]
fn messages_to_other_entities_go_through_the_world_and_reach_every_instance() {
    const OTHER: u32 = 0xa000_0201;
    #[derive(Default)]
    struct Router(Vec<(u32, u32, u64, Variables)>);
    impl World for Router {
        fn send_message(
            &mut self,
            source: Site,
            target: u32,
            message: u64,
            parameters: &Variables,
        ) -> bool {
            self.0
                .push((source.entity, target, message, parameters.clone()));
            true
        }
    }
    // An action, and a SendGameMessage state whose target kind
    // `STU_9B213168` names the entities of its `m_targets` (`0C8E` st41).
    let sender = graph(
        42,
        vec![
            entry(&[(1, "m_inPlug"), (2, "m_beginPlug")]),
            node(
                1,
                "STUStatescriptActionSendGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000005"},
                    "m_target":var(90,true),
                    "m_params":[{"m_B5051BCE":"0x0D80000000000007","m_value":{"$":"STUConfigVarInt","m_value":9}}],
                }),
            ),
            node(
                2,
                "STU_38EE1100",
                Some(0),
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000006"},
                    "m_target":{"$":"STU_9B213168","m_targets":var(90,true)},
                }),
            ),
        ],
        json!([2]),
        json!([0]),
    );
    let receiver = graph(
        43,
        vec![node(
            0,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":"0x0240000000000005"},
                "m_params":[{"m_B5051BCE":"0x0D80000000000007","m_out_Var":var(100,false)}],
            }),
        )],
        json!([]),
        json!([0]),
    );
    let graphs = Arc::new(store(vec![sender, receiver]));
    let mut router = Router::default();
    start_with(
        Arc::clone(&graphs),
        Variables::from([(90, Value::Entity(OTHER))]),
        100,
        &mut router,
    );
    let parameters = Variables::from([(7, Value::Integer(9))]);
    assert_eq!(
        router.0,
        [
            (
                0xa000_0101,
                OTHER,
                0x0240_0000_0000_0005,
                parameters.clone()
            ),
            (0xa000_0101, OTHER, 0x0240_0000_0000_0006, Variables::new())
        ]
    );
    let mut other = Component::start(
        graphs,
        OTHER,
        Variables::new(),
        vec![Root::new(1, 43), Root::new(2, 43)],
        100,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert_eq!(
        other
            .receive_message(0x0240_0000_0000_0005, parameters, &mut EmptyWorld)
            .unwrap(),
        2
    );
    let snapshot = other.snapshot().unwrap();
    for id in [1, 2] {
        assert_eq!(snapshot.instances[&id].variables[&100], Value::Integer(9));
    }

    // The player's own world queues them for the match.
    let mut world = PlayerWorld::new(
        Arc::new(store(Vec::new())),
        0xa000_0100,
        0xa000_0101,
        1,
        PRACTICE_RANGE_MODE,
        SOLDIER,
    );
    let site = Site {
        entity: 0xa000_0101,
        instance: 1,
        graph: 42,
        node: None,
    };
    assert!(world.send_message(site, OTHER, 5, &Variables::new()));
    assert!(!world.send_message(
        Site {
            entity: OTHER,
            ..site
        },
        1,
        5,
        &Variables::new()
    ));
    assert_eq!(
        world.take_messages(),
        [GameMessage {
            sender: 0xa000_0101,
            target: OTHER,
            message: 5,
            parameters: Variables::new()
        }]
    );
    assert!(world.take_messages().is_empty());
}

#[test]
fn a_message_to_every_entity_reaches_the_sender_and_goes_through_the_world() {
    #[derive(Default)]
    struct Broadcaster(Vec<(u32, u64, Variables)>);
    impl World for Broadcaster {
        fn broadcast_message(
            &mut self,
            source: Site,
            message: u64,
            parameters: &Variables,
        ) -> bool {
            self.0.push((source.entity, message, parameters.clone()));
            true
        }
    }
    let channels = |primary, mode| json!({"$":"STU_1D719351","m_6A8DB059":primary,"m_1199BFE0":mode,"m_111E4A0E":0});
    // `0F4A` st5 sends `0700` so; st2's channels are the clients' only.
    let fixture = graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug"), (2, "m_beginPlug")]),
            node(
                1,
                "STU_38EE1100",
                Some(0),
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000006"},
                    "m_target":channels(1, 1),
                    "m_params":[{"m_B5051BCE":"0x0D80000000000007","m_value":{"$":"STUConfigVarInt","m_value":9}}],
                }),
            ),
            node(
                2,
                "STU_38EE1100",
                Some(1),
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000008"},
                    "m_target":channels(0, 1),
                }),
            ),
            node(
                3,
                "STUStatescriptEntryGameMessage",
                None,
                json!({
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000006"},
                    "m_params":[{"m_B5051BCE":"0x0D80000000000007","m_out_Var":var(100,false)}],
                }),
            ),
        ],
        json!([1, 2]),
        json!([0, 3]),
    );
    let graphs = Arc::new(store(vec![fixture]));
    let mut world = Broadcaster::default();
    let mut component = start(Arc::clone(&graphs), 100, &mut world);
    component.advance_frame(101, &mut world).unwrap();
    let parameters = Variables::from([(7, Value::Integer(9))]);
    assert_eq!(
        world.0,
        [(0xa000_0101, 0x0240_0000_0000_0006, parameters.clone())]
    );
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.instances[&1].variables[&100], Value::Integer(9));
    assert!(snapshot.initialization_complete());
    // A world that can't reach every entity refuses it.
    let component = start(graphs, 100, &mut EmptyWorld);
    assert_eq!(
        component.snapshot().unwrap().diagnostics[0].issue,
        Issue::UnsupportedField("broadcast game message delivery")
    );
}

#[test]
fn filter_array_keeps_its_output_the_candidates_that_pass() {
    const TAG: u64 = 0x0d60_0000_0000_0310;
    const ROOM: u64 = 0x0d60_0000_0000_04b3;
    const ENTITIES: [u32; 3] = [0xa000_0201, 0xa000_0202, 0xa000_0203];
    struct Rooms;
    impl World for Rooms {
        fn entities_with_tag(&self, _site: Site, tag: u64) -> Option<Vec<u32>> {
            (tag == ROOM).then(|| ENTITIES.to_vec())
        }
        fn has_target_tag(&self, _site: Site, entity: u32, tag: u64) -> Option<bool> {
            Some(tag == TAG && entity == ENTITIES[1])
        }
    }
    let tag = |tag: u64| json!({"$":"STU_357DABBB","m_7D7C86A1":format!("0x{tag:016X}"),"m_resourceKey":null});
    let filter = |flags: u64, extra: Json| {
        let mut filter = json!({
            "$":"STUConfigVarFilter","m_84065FD1":flags,"m_1E14CCA8":0,"m_4EA686DB":0,
            "m_27CF71D5":0,"m_66747E7D":0,"m_89831602":0,
        });
        for (key, value) in extra.as_object().unwrap() {
            filter[key] = value.clone();
        }
        filter
    };
    // The candidate is every parameter: keep those above 1.
    let above_one = json!({"m_condition":{"$":"STU_919FD47C","m_expression":{
        "m_opcodes":[13,0,6,0,36,0], "m_dynamicVars":[var(8, false)], "m_D99EF254":[1.0]
    }}});
    let filter_array = |position, state, source: Json, filter: Json, output| {
        node(
            position,
            "STU_0A6BE10E",
            Some(state),
            json!({
                "m_814F8EC3":source, "m_filter":filter, "m_F2805C84":var(output, true),
                "m_C485751C":0, "m_A70DB860":{"$":"STUStatescriptOutputPlug","links":[]},
            }),
        )
    };
    let fixture = graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug"), (2, "m_beginPlug"), (3, "m_beginPlug")]),
            filter_array(1, 0, var(90, true), filter(0, above_one), 91),
            // `0F4A` st88: the rooms of a tag (`STU_EB216A0B`) that carry
            // another.
            filter_array(
                2,
                1,
                json!({"$":"STU_EB216A0B","m_9C7BD8B8":tag(ROOM)}),
                filter(0, json!({"m_9C7BD8B8":tag(TAG)})),
                92,
            ),
            // A filter on entity flags (melee `0043` st63) stays inert.
            filter_array(3, 2, var(90, true), filter(128, json!({})), 93),
        ],
        json!([1, 2, 3]),
        json!([0]),
    );
    let graphs = Arc::new(store(vec![fixture]));
    let numbers =
        |values: &[i64]| Value::Array(values.iter().copied().map(Value::Integer).collect());
    let mut component = start_with(
        graphs,
        Variables::from([(90, numbers(&[1, 2, 3]))]),
        100,
        &mut Rooms,
    );
    let snapshot = component.snapshot().unwrap();
    assert_eq!(snapshot.variables[&91], numbers(&[2, 3]));
    assert_eq!(
        snapshot.variables[&92],
        Value::Array(vec![Value::Entity(ENTITIES[1])])
    );
    assert!(!snapshot.variables.contains_key(&93));
    assert_eq!(
        snapshot
            .assumptions
            .iter()
            .map(|assumption| (assumption.site.node, assumption.class.as_str()))
            .collect::<Vec<_>>(),
        [(Some(3), "STU_0A6BE10E")]
    );
    // A change of the source filters again.
    component
        .set_entity_variable(90, numbers(&[5, 0]), &mut Rooms)
        .unwrap();
    component.advance_frame(101, &mut Rooms).unwrap();
    assert_eq!(component.snapshot().unwrap().variables[&91], numbers(&[5]));
}

#[test]
fn ultimate_charges_by_itself_and_for_damage_and_waits_while_it_runs() {
    use crate::ultimate::{ChargeRates, ULT_CHARGE, ULT_COST, UltimateCharge, set_ult_charge};
    const RATES: ChargeRates = ChargeRates {
        passive: 5.0,
        combat: 0.5,
    };

    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STUStatescriptStateAbility",
                Some(0),
                json!({"m_150F0D92":var(ULT_CHARGE,true)}),
            ),
        ],
        json!([1]),
        json!([0]),
    )]));
    let mut component = start_with(
        Arc::clone(&graphs),
        Variables::from([
            (ULT_COST, Value::Float(100.0)),
            (ULT_CHARGE, Value::Float(0.0)),
        ]),
        100,
        &mut EmptyWorld,
    );
    let charge =
        |component: &Component| component.snapshot().unwrap().variables[&ULT_CHARGE].clone();
    let ultimate = UltimateCharge::new(&component, &graphs, RATES);
    ultimate.charge(&mut component, &mut EmptyWorld).unwrap();
    // 5 points a second for 16 ms, then half of 30 damage.
    let step = f64::from(0.08_f32);
    assert_eq!(charge(&component), Value::Float(step));
    ultimate
        .gain(30.0, &mut component, &mut EmptyWorld)
        .unwrap();
    let step = f64::from((step + 15.0) as f32);
    assert_eq!(charge(&component), Value::Float(step));
    // The charge starts the ultimate at its next tick; it doesn't charge
    // while running.
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    ultimate.charge(&mut component, &mut EmptyWorld).unwrap();
    ultimate
        .gain(30.0, &mut component, &mut EmptyWorld)
        .unwrap();
    assert_eq!(charge(&component), Value::Float(step));
    set_ult_charge(&mut component, 99.99, &mut EmptyWorld).unwrap();
    assert_eq!(charge(&component), Value::Float(f64::from(99.99_f32)));

    // An idle ultimate fills to its cost and stops there.
    let mut component = start_with(
        Arc::clone(&graphs),
        Variables::from([
            (ULT_COST, Value::Float(100.0)),
            (ULT_CHARGE, Value::Float(99.99)),
        ]),
        100,
        &mut EmptyWorld,
    );
    let ultimate = UltimateCharge::new(&component, &store(Vec::new()), RATES);
    ultimate.charge(&mut component, &mut EmptyWorld).unwrap();
    ultimate
        .gain(1000.0, &mut component, &mut EmptyWorld)
        .unwrap();
    assert_eq!(charge(&component), Value::Float(100.0));
}

#[test]
fn player_world_answers_the_ruleset_defaults_of_its_hero_and_mode() {
    let rulesets = crate::rulesets::Rulesets::from_json(
        br#"{"schemas": {
            "0x0A3000000000001B": {"10265": 1.0},
            "0x0A30000000000060": {"29450": true}
        },
        "heroes": {"0x02E0000000000003": ["0x0A3000000000001B"]},
        "modes": {"0x023000000000005A": ["0x0A30000000000060"]}}"#,
    )
    .unwrap();
    let mut world = PlayerWorld::new(
        Arc::new(store(Vec::new()).with_rulesets(Arc::new(rulesets))),
        0xa000_0100,
        0xa000_0101,
        1,
        PRACTICE_RANGE_MODE,
        0x02e0_0000_0000_0003,
    );
    // Tracer's clip size scalar and the Practice Range's own schema.
    assert_eq!(world.ruleset(0x2819), Some(Value::Float(1.0)));
    assert_eq!(world.ruleset(0x730a), Some(Value::Bool(true)));
    assert_eq!(world.ruleset(0x2982), None);
    world.hero = SOLDIER;
    assert_eq!(world.ruleset(0x2819), None);
}

/// The clip size scalar `0x2819` comes from the heroes' ruleset schema, so
/// Tracer's pistols load a 40-round magazine (`ow174/game/script/rulesets.py`).
#[test]
#[ignore = "requires a local 1.74 graph pack"]
fn tracer_pistols_load_forty_rounds_from_the_ruleset_defaults() {
    const TRACER: u64 = 0x02e0_0000_0000_0003;
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let body = &graphs.bodies()[&0x0400_0000_0000_0001];
    assert_eq!(body.hero.as_deref(), Some("0x02E0000000000003"));
    let mut world = PlayerWorld::new(
        Arc::clone(&graphs),
        0xa000_0100,
        0xa000_0101,
        1,
        PRACTICE_RANGE_MODE,
        TRACER,
    );
    let mut component = Component::start(
        Arc::clone(&graphs),
        0xa000_0101,
        player_entity_variables(body, 150.0, 150.0),
        player_roots(body, &graphs).unwrap(),
        999,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    component.advance_frame(999, &mut world).unwrap();
    let pistols = component
        .snapshot()
        .unwrap()
        .instances
        .iter()
        .find(|(_, instance)| instance.graph == 0x0016)
        .map(|(&id, _)| id)
        .unwrap();
    let variable = |component: &Component, id| {
        component
            .resolved_variable(
                pistols,
                Variable {
                    scope: Scope::Instance,
                    id,
                },
            )
            .unwrap()
    };
    assert_eq!(variable(&component, 198), Value::Integer(40));
    assert_eq!(variable(&component, 53), Value::Integer(40));
    for frame in 1000..1030 {
        component
            .advance_command(
                Command {
                    frame,
                    buttons: 1,
                    ..Command::default()
                },
                &mut world,
            )
            .unwrap();
    }
    assert!(!component.machine.shots.is_empty());
    assert!(expr::to_int(&variable(&component, 53), 40) < 40);
}

/// leafall903/ORPHEUS (MIT) e5463e3
/// `test_held_primary_updates_owner_spread_and_recovers_on_release`:
/// Wrecking Ball's Quad Cannons (graph `1514` state 9, ramp 20) grow the
/// linked spread variable 231 each shot while the trigger is held, and the
/// owner's reticle (entity variable 243, a graph's expression over it)
/// follows: 0.2 and 0.4° after 4 shots, 1 and 2° after 20.
#[test]
#[ignore = "requires a local 1.74 graph pack"]
fn wrecking_balls_reticle_follows_the_growing_spread() {
    const WRECKING_BALL: u64 = 0x02e0_0000_0000_01ca;
    const QUAD_CANNONS: u32 = 0x1514;
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let body = &graphs.bodies()[&0x0400_0000_0000_200b];
    let mut world = PlayerWorld::new(
        Arc::clone(&graphs),
        0xa000_0100,
        0xa000_0101,
        TEAM_ONE,
        PRACTICE_RANGE_MODE,
        WRECKING_BALL,
    );
    world.pool_types_known = true;
    world.unknowns = Unknowns::Tolerate;
    let mut component = Component::start(
        Arc::clone(&graphs),
        0xa000_0101,
        player_entity_variables(body, 600.0, 600.0),
        player_roots(body, &graphs).unwrap(),
        999,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let cannons = component
        .snapshot()
        .unwrap()
        .instances
        .iter()
        .find(|(_, instance)| instance.graph == QUAD_CANNONS)
        .map(|(&id, _)| id)
        .unwrap();
    let read = |component: &Component, scope, id| {
        expr::to_float(
            &component
                .resolved_variable(cannons, Variable { scope, id })
                .unwrap(),
        )
    };
    let mut frame = 999;
    let mut advance = |component: &mut Component, frames: u32, buttons: u32| {
        for _ in 0..frames {
            frame += 1;
            component
                .advance_command(
                    Command {
                        frame,
                        buttons,
                        ..Command::default()
                    },
                    &mut world,
                )
                .unwrap();
        }
    };
    advance(&mut component, 24, 0);
    assert_eq!(read(&component, Scope::Entity, 243), 0.0);
    advance(&mut component, 9, 1);
    assert_eq!(read(&component, Scope::Instance, 53), 76.0);
    assert!((read(&component, Scope::Instance, 231) - 0.2).abs() < 1e-6);
    assert!((read(&component, Scope::Entity, 243) - 0.4).abs() < 1e-6);
    advance(&mut component, 41, 1);
    assert_eq!(read(&component, Scope::Instance, 53), 60.0);
    assert_eq!(
        (
            read(&component, Scope::Instance, 231),
            read(&component, Scope::Entity, 243)
        ),
        (1.0, 2.0)
    );
}

/// The bodies a target search can find, as a match lists them.
struct Bodies(Vec<TargetCandidate>);

impl World for Bodies {
    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        Some(&self.0)
    }
}

const SEARCHER: u32 = 0xa000_0101;
const ALLY: u32 = 0xa000_0201;
const FAR_ALLY: u32 = 0xa000_0202;
const DEAD_ALLY: u32 = 0xa000_0203;
const ENEMY: u32 = 0xa000_0301;
const SECOND_ENEMY: u32 = 0xa000_0302;

fn body(entity: u32, team: u8, feet: [f64; 3]) -> TargetCandidate {
    TargetCandidate {
        entity,
        team: Some(team),
        dead: false,
        controller: None,
        feet,
        height: 1.8,
        radius: 0.4,
    }
}

/// Soldier: 76's search when his Biotic Field lands (`0257` st6): a 10 m
/// sphere a metre over his feet, allies (`m_66747E7D` 1) but not himself
/// (`m_89831602` 4), living (`m_4EA686DB` 16) heroes (`m_84065FD1` 1); the
/// targets to #375, their count to #1192. A switch reads the count.
fn search_fixture(filter: Json, extra: Json) -> GraphStore {
    let mut search = json!({
        "m_8DCA77B6":{"$":"STU_B5A0CAF0"},
        "m_05BE62DD":{"$":"STUConfigVarVec3","m_value":{"X":0.0,"Y":1.0,"Z":0.0}},
        "m_radius":{"$":"STUConfigVarInt","m_value":10},
        "m_A30B383E":{"$":"STUConfigVarInt","m_value":360},
        "m_filter":filter,
        "m_8A2905A0":var(375, false),
        "m_1B89EA79":var(1192, false),
        "m_8CBE558B":{"$":"STUConfigVarInt","m_value":0},
        "m_DD792FF0":3, "m_08285CD6":1, "m_E4A295CE":1,
    });
    for (key, value) in extra.as_object().unwrap() {
        search[key] = value.clone();
    }
    store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_beginPlug"), (2, "m_beginPlug")], &[])}),
            ),
            node(1, "STUStatescriptStateTrackTargets", Some(0), search),
            node(
                2,
                "STUStatescriptStateBooleanSwitch",
                Some(1),
                json!({"m_condition":{"$":"STU_919FD47C","m_expression":{
                    "m_opcodes":[8,0,6,0,36,0], "m_dynamicVars":[var(1192, false)],
                    "m_D99EF254":[0.0]
                }}}),
            ),
        ],
        json!([1, 2]),
        json!([0]),
    )])
}

fn allies_filter() -> Json {
    json!({"$":"STUConfigVarFilter","m_84065FD1":1,"m_1E14CCA8":0,"m_4EA686DB":16,
        "m_27CF71D5":0,"m_66747E7D":1,"m_89831602":4,"m_condition":null})
}

fn found(component: &Component) -> (Vec<u32>, Value, Value, StateKind) {
    let snapshot = component.snapshot().unwrap();
    let instance = &snapshot.instances[&1];
    (
        instance.states[&0]
            .tracked
            .iter()
            .map(|target| target.entity)
            .collect(),
        instance.variables.get(&375).cloned().unwrap_or(Value::Null),
        instance
            .variables
            .get(&1192)
            .cloned()
            .unwrap_or(Value::Null),
        instance.states[&1].kind,
    )
}

/// A body entering the sphere joins the targets and one leaving it, or
/// dying, goes; each change rewrites the outputs, and the switch reading
/// the count follows. The searcher, the enemy, the dead and the far ally
/// are never found; the owner frame carries each target and its age.
#[test]
fn a_target_search_follows_bodies_in_and_out_of_its_sphere() {
    use crate::{
        codecs::CodecTable,
        snapshot::{SnapshotEncoder, View},
    };

    let graphs = Arc::new(search_fixture(allies_filter(), json!({})));
    let mut dead = body(DEAD_ALLY, 0, [1.0, 0.0, 0.0]);
    dead.dead = true;
    let mut world = Bodies(vec![
        body(SEARCHER, 0, [0.0; 3]),
        body(ALLY, 0, [3.0, 0.0, 0.0]),
        body(ENEMY, 1, [2.0, 0.0, 0.0]),
        body(FAR_ALLY, 0, [30.0, 0.0, 0.0]),
        dead,
    ]);
    let mut component = Component::start(
        Arc::clone(&graphs),
        SEARCHER,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let entities = |entities: &[u32]| {
        Value::Array(
            entities
                .iter()
                .map(|&entity| Value::Entity(entity))
                .collect(),
        )
    };
    assert_eq!(
        found(&component),
        (
            vec![ALLY],
            entities(&[ALLY]),
            Value::Integer(1),
            StateKind::BooleanSwitch(true)
        )
    );

    // The far ally walks in, within 10 m of the sphere's centre.
    world.0[3].feet = [9.5, 0.0, 0.0];
    component.advance_frame(1, &mut world).unwrap();
    assert_eq!(
        found(&component),
        (
            vec![ALLY, FAR_ALLY],
            entities(&[ALLY, FAR_ALLY]),
            Value::Integer(2),
            StateKind::BooleanSwitch(true)
        )
    );
    let snapshot = component.snapshot().unwrap();
    let tracked = &snapshot.instances[&1].states[&0].tracked;
    assert!(tracked[0].since_ms < tracked[1].since_ms);
    let codecs = CodecTable::client();
    let encoder = SnapshotEncoder::new(&graphs, &codecs);
    let owner = encoder.project(snapshot, View::Owner).unwrap();
    let age = |since: u64| Value::Integer((snapshot.now_ms - since) as i64);
    assert_eq!(
        owner.state_payload(1, 0).unwrap()["targets"],
        Value::Array(vec![
            Value::Array(vec![Value::Entity(ALLY), age(tracked[0].since_ms)]),
            Value::Array(vec![Value::Entity(FAR_ALLY), age(tracked[1].since_ms)]),
        ])
    );
    encoder.full(&owner).unwrap();

    // The first ally walks off; the other stays.
    world.0[1].feet = [40.0, 0.0, 0.0];
    component.advance_frame(2, &mut world).unwrap();
    assert_eq!(
        found(&component),
        (
            vec![FAR_ALLY],
            entities(&[FAR_ALLY]),
            Value::Integer(1),
            StateKind::BooleanSwitch(true)
        )
    );

    // The other dies: nobody is left, and the switch turns.
    world.0[3].dead = true;
    component.advance_frame(3, &mut world).unwrap();
    assert_eq!(
        found(&component),
        (
            Vec::new(),
            entities(&[]),
            Value::Integer(0),
            StateKind::BooleanSwitch(false)
        )
    );
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.initialization_complete());
    assert!(
        snapshot
            .assumptions
            .iter()
            .any(|item| item.class == "STUStatescriptStateTrackTargets"
                && item.answer == "a sphere search (flag fields unread)")
    );
}

/// The team relation and the searcher bits as authored: enemies only
/// (`m_66747E7D` 2, or bit 1 of `m_89831602`), or anyone (0) with the
/// searcher himself when bit 4 of `m_89831602` is clear. Without teams (a
/// free-for-all) only the searcher is his own ally.
#[test]
fn a_target_search_filters_by_team_and_the_searcher_as_authored() {
    let run = |relation: u64, flags: u64, teams: bool| {
        let mut filter = allies_filter();
        filter["m_66747E7D"] = json!(relation);
        filter["m_89831602"] = json!(flags);
        let mut world = Bodies(
            [
                body(SEARCHER, 0, [0.0; 3]),
                body(ALLY, 0, [3.0, 0.0, 0.0]),
                body(ENEMY, 1, [2.0, 0.0, 0.0]),
            ]
            .into_iter()
            .map(|mut candidate| {
                if !teams {
                    candidate.team = None;
                }
                candidate
            })
            .collect(),
        );
        let component = Component::start(
            Arc::new(search_fixture(filter, json!({}))),
            SEARCHER,
            Variables::new(),
            vec![Root::new(1, 42)],
            0,
            &mut world,
            MAX_INIT_WORK,
        )
        .unwrap();
        found(&component).0
    };
    assert_eq!(run(2, 0, true), [ENEMY]);
    assert_eq!(run(1, 0, true), [SEARCHER, ALLY]);
    assert_eq!(run(0, 0, true), [SEARCHER, ALLY, ENEMY]);
    assert_eq!(run(0, 4, true), [ALLY, ENEMY]);
    assert_eq!(run(0, 1, true), [ENEMY]);
    assert_eq!(run(0, 5, true), [ENEMY]);
    assert_eq!(run(1, 0, false), [SEARCHER]);
    assert_eq!(run(2, 0, false), [ALLY, ENEMY]);
    assert_eq!(run(0, 1, false), [ALLY, ENEMY]);
}

/// A form with a part the server doesn't model (here the scoring
/// `m_84E5DE4E`, or an exclusion bit other than 16 and 4) finds nothing and
/// says which part, without stopping the body; so does a world that lists
/// no bodies. The outputs keep their values.
#[test]
fn a_target_search_the_server_cant_run_finds_nothing_and_names_the_gap() {
    let run = |filter: Json, extra: Json, lists: bool| {
        let graphs = Arc::new(search_fixture(filter, extra));
        let start = |world: &mut Bodies| {
            Component::start(
                Arc::clone(&graphs),
                SEARCHER,
                Variables::new(),
                vec![Root::new(1, 42)],
                0,
                world,
                MAX_INIT_WORK,
            )
        };
        let component = if lists {
            start(&mut Bodies(vec![body(ALLY, 0, [1.0, 0.0, 0.0])]))
        } else {
            Component::start(
                Arc::clone(&graphs),
                SEARCHER,
                Variables::new(),
                vec![Root::new(1, 42)],
                0,
                &mut EmptyWorld,
                MAX_INIT_WORK,
            )
        }
        .unwrap();
        let snapshot = component.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        assert_eq!(found(&component).0, Vec::<u32>::new());
        assert_eq!(found(&component).2, Value::Null);
        snapshot
            .assumptions
            .iter()
            .filter(|item| item.class.starts_with("STUStatescriptStateTrackTargets"))
            .map(|item| (item.class.clone(), item.answer))
            .collect::<Vec<_>>()
    };
    let owner = json!({"$":"STU_21A13145","m_entity":{"$":"STU_B5A0CAF0"}});
    assert_eq!(
        run(allies_filter(), json!({"m_84E5DE4E":owner}), true),
        [(
            "STUStatescriptStateTrackTargets m_84E5DE4E".to_owned(),
            "finds no targets"
        )]
    );
    let mut filter = allies_filter();
    filter["m_4EA686DB"] = json!(16 | 128);
    assert_eq!(
        run(filter, json!({}), true),
        [(
            "STUStatescriptStateTrackTargets m_filter.m_4EA686DB".to_owned(),
            "finds no targets"
        )]
    );
    assert_eq!(
        run(allies_filter(), json!({}), false),
        [(
            "STUStatescriptStateTrackTargets bodies".to_owned(),
            "finds no targets (the world lists none)"
        )]
    );
}

/// Bodies to search, and the health changes a world applies.
#[derive(Default)]
struct Healer {
    bodies: Vec<TargetCandidate>,
    changes: Vec<HealthChange>,
    facing: Option<[i32; 2]>,
}

impl World for Healer {
    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        Some(&self.bodies)
    }

    fn entity_facing(&self, _site: Site, _entity: u32) -> Option<[i32; 2]> {
        self.facing
    }

    fn modify_health(&mut self, change: HealthChange) -> bool {
        self.changes.push(change);
        true
    }
}

/// Quick melee's form (`0043` st5 starting st4): a 1 m search 1.5 m ahead
/// of the owner's eye (1.6 m over his feet), his enemies (`m_89831602` 1)
/// of kind 64 but the dead, the found to #375; a ModifyHealth of 30 to
/// #375 with a tag and a loadout, a blow (`m_7B2B3F5C` 0) of damage
/// (`m_93CDFD2F` 1). The search's line of sight and its open exclusion bit
/// aren't read.
fn melee_fixture(rate: bool) -> GraphStore {
    let search = json!({
        "m_8DCA77B6":{"$":"STU_B5A0CAF0"},
        "m_05BE62DD":{"$":"STUConfigVarVec3","m_value":{"X":0.0,"Y":1.6,"Z":1.5}},
        "m_radius":{"$":"STUConfigVarInt","m_value":1},
        "m_D59188D5":{"$":"STU_21A13145","m_entity":{"$":"STU_B5A0CAF0"}},
        "m_filter":{"$":"STUConfigVarFilter","m_84065FD1":64,"m_4EA686DB":20,
            "m_66747E7D":0,"m_89831602":1},
        "m_8A2905A0":var(375, false),
        "m_subgraphPlug":plug(&[(2, "m_beginPlug")], &[1]),
    });
    let modify = json!({
        "m_FA3ECC43":var(375, false),
        "m_7B2B3F5C":i32::from(rate),
        "m_95F1FB0B":if rate { json!({"$":"STUConfigVarFloat","m_value":0.5}) } else { Json::Null },
        "m_modifyHealth":{
            "$":"STUStatescriptModifyHealth",
            "m_amount":{"$":"STUConfigVarInt","m_value":30},
            "m_148F3152":[{"$":"STUStatescriptModifyHealthTag",
                "m_EE3055BD":{"$":"STUConfigVarIdentifier","m_identifier":"0x0D80000000000302"}}],
            "m_instigator":{"$":"STU_B5A0CAF0"},
            "m_loadout":{"$":"STUConfigVarLoadout","m_loadout":"0x0B900000000002F5"},
            "m_93CDFD2F":1,
        },
    });
    store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_beginPlug")], &[])}),
            ),
            node(1, "STUStatescriptStateTrackTargets", Some(0), search),
            node(2, "STUStatescriptStateModifyHealth", Some(1), modify),
        ],
        json!([1, 2]),
        json!([0]),
    )])
}

/// A blow strikes each body the search finds once per activation: the one
/// ahead at once, one that walks in at the tick it is found, none again
/// while it stays, as damage from the owner with the authored tag and
/// loadout. The sphere goes 1.5 m along where the owner looks.
#[test]
fn a_modify_health_blow_strikes_each_found_body_once() {
    let graphs = Arc::new(melee_fixture(false));
    let mut world = Healer {
        // Facing +x (yaw a quarter turn), level.
        facing: Some([16384, 0]),
        bodies: vec![
            body(SEARCHER, 0, [0.0; 3]),
            // 1.5 m ahead: within the sphere.
            body(ENEMY, 1, [1.5, 0.0, 0.0]),
            // 1.5 m behind: out of it.
            body(SECOND_ENEMY, 1, [-1.5, 0.0, 0.0]),
        ],
        changes: Vec::new(),
    };
    let mut component = Component::start(
        graphs,
        SEARCHER,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let struck = |world: &mut Healer| {
        std::mem::take(&mut world.changes)
            .into_iter()
            .map(|change| (change.target, change.amount, change.heal, change.frame))
            .collect::<Vec<_>>()
    };
    assert_eq!(struck(&mut world), [(ENEMY, 30.0, false, 0)]);
    component.advance_frame(1, &mut world).unwrap();
    assert_eq!(struck(&mut world), []);
    // The second enemy comes round in front: struck once, at frame 2.
    world.bodies[2].feet = [1.2, 0.0, 0.4];
    component.advance_frame(2, &mut world).unwrap();
    let changes = std::mem::take(&mut world.changes);
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
    assert_eq!(
        (
            change.target,
            change.instigator,
            change.frame,
            change.loadout
        ),
        (SECOND_ENEMY, SEARCHER, 2, Some(0x0b90_0000_0000_02f5))
    );
    assert_eq!(
        change.tags.iter().collect::<Vec<_>>(),
        [0x0d80_0000_0000_0302]
    );
    component.advance_frame(3, &mut world).unwrap();
    assert_eq!(struck(&mut world), []);
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.initialization_complete());
    let assumed: Vec<_> = snapshot
        .assumptions
        .iter()
        .map(|item| (item.class.as_str(), item.answer))
        .collect();
    assert!(assumed.contains(&(
        "STUStatescriptStateTrackTargets m_D59188D5",
        "no line-of-sight test"
    )));
}

/// Quick melee strikes neither the swinger nor his allies: looking down,
/// its sphere (1.5 m along the aim from his eye) reaches his own capsule
/// and an ally's beside him, yet a swing at nothing changes no health,
/// and a swing at an enemy who walks in strikes the enemy alone.
#[test]
fn quick_melee_strikes_neither_the_swinger_nor_his_allies() {
    let graphs = Arc::new(melee_fixture(false));
    let mut world = Healer {
        // Facing +x, an eighth of a turn down: the sphere's centre is
        // 1.06 m ahead of his feet and 0.54 m up, 0.66 m from his capsule.
        facing: Some([16384, 8192]),
        bodies: vec![
            body(SEARCHER, 0, [0.0; 3]),
            body(ALLY, 0, [1.2, 0.0, 0.6]),
            body(ENEMY, 1, [-3.0, 0.0, 0.0]),
        ],
        changes: Vec::new(),
    };
    let mut component = Component::start(
        graphs,
        SEARCHER,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    component.advance_frame(1, &mut world).unwrap();
    assert!(world.changes.is_empty());
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.initialization_complete());
    assert_eq!(
        snapshot.instances[&1].variables.get(&375),
        Some(&Value::Array(Vec::new()))
    );

    world.bodies[2].feet = [1.5, 0.0, 0.0];
    component.advance_frame(2, &mut world).unwrap();
    let struck: Vec<_> = world
        .changes
        .iter()
        .map(|change| (change.target, change.instigator, change.heal))
        .collect();
    assert_eq!(struck, [(ENEMY, SEARCHER, false)]);
}

/// A rate (`m_7B2B3F5C` 1) gives every body in its targets its amount each
/// `m_95F1FB0B` seconds, at each tick the share of the time since the last
/// (or its begin): 30 every 0.5 s is 0.96 a 16 ms frame.
#[test]
fn a_modify_health_rate_applies_a_share_each_tick() {
    let graphs = Arc::new(melee_fixture(true));
    let mut world = Healer {
        facing: Some([16384, 0]),
        bodies: vec![body(SEARCHER, 0, [0.0; 3]), body(ENEMY, 1, [1.5, 0.0, 0.0])],
        changes: Vec::new(),
    };
    let mut component = Component::start(
        graphs,
        SEARCHER,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert!(world.changes.is_empty());
    for frame in 1..=2 {
        component.advance_frame(frame, &mut world).unwrap();
    }
    let shares: Vec<_> = world
        .changes
        .iter()
        .map(|change| (change.target, change.amount))
        .collect();
    // The state began at 0 ms; frame 1's tick comes at its last
    // millisecond (2 × 16 − 1 = 31 ms), frame 2's a frame later.
    let share = |ms: f64| 30.0 * (ms / 1000.0 / 0.5);
    assert_eq!(
        shares,
        [
            (ENEMY, share((2 * FRAME_MS - 1) as f64)),
            (ENEMY, share(FRAME_MS as f64))
        ]
    );
}

/// A body's world that keeps its statuses and modifiers.
#[derive(Default)]
struct Aurad {
    changes: Vec<HealthChange>,
    effects: BodyEffects,
}

impl World for Aurad {
    fn modify_health(&mut self, change: HealthChange) -> bool {
        self.changes.push(change);
        true
    }

    fn set_status(&mut self, site: Site, status: Option<i64>) -> bool {
        self.effects.set_status(site, status)
    }

    fn combat_mod(&mut self, site: Site, modifier: Option<CombatMod>) -> bool {
        self.effects.combat_mod(site, modifier)
    }
}

/// An aura (`053:0001`, 0.1 s) whose graph 50 takes the instigator into
/// #40 at its "aura applied" entry, then holds a status of type 0 and a
/// healing modifier ×0 (Biotic Grenade's `0C72` st8) and deals 60 to its
/// body from #40, until a 4 s wait ends the instance.
#[test]
fn an_aura_runs_its_graph_on_the_body_until_its_instance_ends() {
    const AURA: u64 = 0x0530_0000_0000_0001;
    let applied = json!({"$":"STUConfigVarGameMessage","m_gameMessage":"0x024000000000000F"});
    let aura_graph = graph(
        50,
        vec![
            node(
                0,
                "STUStatescriptEntryGameMessage",
                None,
                json!({"m_gameMessage":applied, "m_B60ABDA4":var(40, false),
                    "m_outPlug":plug(&[(1, "m_beginPlug")], &[])}),
            ),
            node(
                1,
                "STUStatescriptStateWait",
                Some(0),
                json!({"m_timeout":{"$":"STUConfigVarInt","m_value":4},
                    "m_subgraphPlug":plug(&[(2, "m_beginPlug"), (3, "m_beginPlug"), (4, "m_beginPlug")], &[1, 2, 3]),
                    "m_onEndPlug":plug(&[(5, "m_inPlug")], &[])}),
            ),
            node(2, "STU_9028E035", Some(1), json!({"m_type":0})),
            node(
                3,
                "STUStatescriptStateCombatModFilter",
                Some(2),
                json!({"m_60703B1E":"0x0D8000000000031A",
                    "m_50530EAB":{"$":"STUConfigVarInt","m_value":0}, "m_41B10FF2":1}),
            ),
            node(
                4,
                "STUStatescriptStateModifyHealth",
                Some(3),
                json!({"m_FA3ECC43":{"$":"STU_B5A0CAF0"}, "m_7B2B3F5C":0,
                    "m_modifyHealth":{"$":"STUStatescriptModifyHealth",
                        "m_amount":{"$":"STUConfigVarInt","m_value":60},
                        "m_instigator":var(40, false), "m_93CDFD2F":1}}),
            ),
            node(5, "STU_22926252", None, json!({})),
        ],
        json!([1, 2, 3, 4]),
        json!([0]),
    );
    let idle = graph(
        42,
        vec![node(
            0,
            "STUStatescriptEntry",
            None,
            json!({"m_outPlug":plug(&[], &[])}),
        )],
        json!([]),
        json!([0]),
    );
    let graphs = Arc::new(
        GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version":1, "graphs":[idle, aura_graph], "bodies":{}, "missing":[],
                "auras":{"0x0530000000000001":{"m_5A4051BC":"0x0580000000000032","m_7DAE8F10":4.5}}
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let mut world = Aurad::default();
    let mut component = Component::start(
        graphs,
        SEARCHER,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let id = component
        .apply_aura(AURA, ENEMY, &[], &mut world)
        .unwrap()
        .unwrap();
    assert_eq!(
        world
            .changes
            .iter()
            .map(|change| (change.target, change.instigator, change.amount, change.heal))
            .collect::<Vec<_>>(),
        [(SEARCHER, ENEMY, 60.0, false)]
    );
    assert!(world.effects.invulnerable());
    assert_eq!(world.effects.healing_scale(&DamageTags::NONE), 0.0);
    // 4 s (250 frames of 16 ms) later the wait ends, the instance exits
    // and its states let go.
    for frame in 1..=251 {
        component.advance_frame(frame, &mut world).unwrap();
    }
    assert!(!component.snapshot().unwrap().instances.contains_key(&id));
    assert!(!world.effects.invulnerable());
    assert_eq!(world.effects.healing_scale(&DamageTags::NONE), 1.0);
    assert!(component.snapshot().unwrap().initialization_complete());
}

/// Alias `0C8:0001` of [`Animations`].
const ANIMATED: &str = "0x0C80000000000001";
const ANIMATED_ALIAS: u64 = 0x0C80_0000_0000_0001;
const ANIMATED_CATEGORY: u64 = 0x0280_0000_0000_011B;

/// A body whose animation data knows one alias ([`ANIMATED`]): 1.25 s, or
/// 0.5 s with category `015:011B`, and a scalar of 0.75.
struct Animations;

impl World for Animations {
    fn unknowns(&self) -> Unknowns {
        Unknowns::Refuse
    }

    fn animation_length(&self, _site: Site, alias: u64, category: u64) -> Option<f64> {
        (alias == ANIMATED_ALIAS).then_some(if category == ANIMATED_CATEGORY {
            0.5
        } else {
            1.25
        })
    }

    fn animation_scalar(&self, _site: Site, alias: u64, _category: u64) -> Option<f64> {
        (alias == ANIMATED_ALIAS).then_some(0.75)
    }
}

/// An `Anim` state (node 1) entered at 1600 ms (frame 100) with `fields`
/// and alias `alias`.
fn start_anim(alias: Json, fields: Json, world: &mut impl World) -> Component {
    let mut fields = fields;
    fields["m_11E0A658"] = alias;
    let fixture = graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1,"m_beginPlug")],&[])}),
            ),
            node(1, "STUStatescriptStateAnim", Some(0), fields),
        ],
        json!([1]),
        json!([0]),
    );
    Component::start(
        Arc::new(store(vec![fixture])),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        100,
        world,
        MAX_INIT_WORK,
    )
    .unwrap()
}

fn anim_alias(alias: &str) -> Json {
    json!({"$":"STUConfigVarAnimAlias","m_animAlias":alias,"m_resourceKey":null})
}

/// Each pending timer of the state as (due ms, finish).
fn anim_timers(component: &Component) -> Vec<(u64, bool)> {
    component
        .snapshot()
        .unwrap()
        .pending_timers
        .get(&1)
        .into_iter()
        .flatten()
        .map(|timer| (timer.due_ms, timer.finish))
        .collect()
}

/// leafall903/ORPHEUS `c239a84` `Anim.on_enter`: with no authored length
/// (or one under 1000 × `f32::EPSILON`), the state runs for its
/// animation's length on the body, times `m_speedScalar`, for the category
/// it names; timer 1 and the finish come then.
#[test]
fn an_anim_without_a_length_runs_for_the_body_animation_length() {
    let speed = json!({"m_speedScalar":{"$":"STUConfigVarFloat","m_value":2.0}});
    let component = start_anim(anim_alias(ANIMATED), speed, &mut Animations);
    assert_eq!(anim_timers(&component), [(4100, false), (4100, true)]);
    let category = json!({"m_22326315":{
        "$":"STUConfigVarAnimCategory","m_category":"0x028000000000011B","m_resourceKey":null
    }});
    let component = start_anim(anim_alias(ANIMATED), category, &mut Animations);
    assert_eq!(anim_timers(&component), [(2100, false), (2100, true)]);
    let zero = json!({"m_86701C3E":{"$":"STUConfigVarFloat","m_value":0.0001}});
    let component = start_anim(anim_alias(ANIMATED), zero, &mut Animations);
    assert_eq!(anim_timers(&component), [(2850, false), (2850, true)]);
    let authored = json!({"m_86701C3E":{"$":"STUConfigVarFloat","m_value":0.025}});
    let component = start_anim(anim_alias(ANIMATED), authored, &mut Animations);
    assert_eq!(anim_timers(&component), [(1625, false), (1625, true)]);
    assert!(component.snapshot().unwrap().assumptions.is_empty());
}

/// `m_37EE887D` arms no timer, whatever the length: the state stays on.
#[test]
fn an_anim_that_skips_its_timer_stays_on() {
    let fields = json!({
        "m_37EE887D":{"$":"STUConfigVarBool","m_value":1},
        "m_86701C3E":{"$":"STUConfigVarFloat","m_value":0.025},
    });
    let mut component = start_anim(anim_alias(ANIMATED), fields, &mut Animations);
    assert_eq!(anim_timers(&component), []);
    component.advance_frame(200, &mut Animations).unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
}

/// An alias the body's data doesn't know, with no authored length, never
/// ends by itself, and the answer is recorded; a zero alias skips the model
/// query (length 0) and ends at once.
#[test]
fn an_anim_the_data_cant_time_never_ends_and_a_zero_alias_ends_at_once() {
    let mut component = start_anim(anim_alias("0x0C80000000000002"), json!({}), &mut Animations);
    assert_eq!(anim_timers(&component), []);
    let assumed: Vec<_> = component
        .snapshot()
        .unwrap()
        .assumptions
        .iter()
        .map(|item| (item.site.node, item.class.as_str(), item.answer))
        .collect();
    assert_eq!(
        assumed,
        [(Some(1), "STUStatescriptStateAnim", "never ends by itself")]
    );
    component.advance_frame(200, &mut Animations).unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
    let mut component = start_anim(Json::Null, json!({}), &mut Animations);
    component.advance_frame(101, &mut Animations).unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(snapshot.assumptions.is_empty());
    assert_eq!(active(snapshot, 1), Vec::<u16>::new());
}

/// `m_EBEFB4A7` reads the animation's scalar (output slot 2) while the
/// state runs, and is let go when it ends.
#[test]
fn an_anim_outputs_its_scalar_while_it_runs() {
    let fields = json!({
        "m_EBEFB4A7":var(7, false),
        "m_86701C3E":{"$":"STUConfigVarFloat","m_value":0.025},
    });
    let mut component = start_anim(anim_alias(ANIMATED), fields, &mut Animations);
    let output = Variable {
        scope: Scope::Instance,
        id: 7,
    };
    assert_eq!(
        component.resolved_variable(1, output),
        Some(Value::Float(0.75))
    );
    component.advance_frame(102, &mut Animations).unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), Vec::<u16>::new());
    assert_eq!(component.resolved_variable(1, output), Some(Value::Null));
}
