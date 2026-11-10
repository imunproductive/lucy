use super::*;

use serde_json::json;

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

struct ReferenceWorld;

impl World for ReferenceWorld {
    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        // Values observed in the Python fixtures, for this test only.
        match (site.graph, config.get("$")?.as_str()?) {
            (0x0c18, "STU_AC829876") => Some(Value::Bool(true)),
            (0x0c18, "STU_4A101787" | "STU_A77AC2AC") => Some(Value::Null),
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

#[derive(Default)]
struct SprintWorld {
    forward: i8,
}

impl World for SprintWorld {
    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        if site.graph != 0x0255 {
            return None;
        }
        match config.get("$")?.as_str()? {
            "STU_D3EB8D87" | "STU_A77AC2AC" => Some(Value::Bool(false)),
            "STU_310D6529" => {
                let unit = (1.0_f64 / 127.0) as f32;
                let forward = f64::from(f32::from(self.forward) * unit);
                Some(Value::Vector([0.0, forward, 0.0]))
            }
            _ => None,
        }
    }
}

#[test]
#[ignore = "requires a local 1.74 graph pack"]
fn authored_sprint_stack_gates_track_input_and_reveal_the_base_on_release() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = Arc::new(GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap());
    let mut world = SprintWorld::default();
    let mut component = Component::start(
        graphs,
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
    for (frame, forward, buttons, throttle, held) in [
        (1030, 63, 0, Value::Null, Value::Null),
        (1031, 64, 0, Value::Bool(true), Value::Null),
        (1032, 127, 0x881, Value::Bool(true), Value::Bool(true)),
        (1033, 0, 0, Value::Null, Value::Null),
    ] {
        world.forward = forward;
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
#[ignore = "requires a local 1.74 graph pack"]
fn isolated_soldier_sprint_ability_matches_the_authored_toggle_and_modifier_states() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = Arc::new(GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap());
    let mut world = SprintWorld::default();
    let mut component = Component::start(
        graphs,
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
        world.forward = forward;
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
    assert!(!component.snapshot().unwrap().initialization_complete());
    assert!(component.movement_modifiers(&mut world).unwrap().is_none());
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

struct RifleWorld(Arc<GraphStore>);

impl World for RifleWorld {
    fn ruleset(&mut self, id: u32) -> Option<Value> {
        match id {
            0x2538 | 0x2819 | 0x32e8 => Some(Value::Float(1.0)),
            0x260f => Some(Value::Bool(true)),
            0x281a | 0x32e0 | 0x32e9 => Some(Value::Bool(false)),
            _ => None,
        }
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != 0xd000_0010 {
            return Value::Null;
        }
        self.0
            .mode_variables(0x0230_0000_0000_0018)
            .and_then(|values| values.get(&id).copied())
            .map_or(Value::Null, Value::from)
    }

    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_1FF67CA6" => Some(Value::Entity(site.entity)),
            "STU_310D6529" => Some(Value::Vector([0.0; 3])),
            "STU_37F20D24" => Some(Value::Asset(0x0230_0000_0000_0018)),
            "STU_A3DEF368" if self.0.mode_variables(0x0230_0000_0000_0018).is_some() => {
                Some(Value::Entity(0xd000_0010))
            }
            "STU_D3EB8D87" | "STU_A77AC2AC" => Some(Value::Null),
            "STU_4A101787" => Some(Value::Bool(false)),
            _ => None,
        }
    }
}

#[test]
#[ignore = "requires a local 1.74 graph pack"]
fn authored_helix_uses_its_six_second_cooldown_before_the_next_volley() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = Arc::new(GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap());
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let mut world = RifleWorld(Arc::clone(&graphs));
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
}

#[test]
#[ignore = "requires a local 1.74 graph pack"]
fn authored_soldier_rifle_holds_releases_and_reloads_on_the_reference_clock() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = Arc::new(GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap());
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let mut world = RifleWorld(Arc::clone(&graphs));
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
    assert!(component.take_shots().is_none());
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
#[ignore = "requires a local 1.74 graph pack"]
fn soldier_sprint_graph_reads_primary_crouch_and_melee_edges_as_in_python() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = Arc::new(GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap());
    let mut component = Component::start(
        graphs,
        0xa000_0101,
        Variables::new(),
        vec![Root::new(5, 0x0255)],
        1029,
        &mut ReferenceWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let sprint = |snapshot: &Snapshot| {
        [4, 7, 16].map(|raw| {
            (
                snapshot.instances[&5].states[&raw].kind,
                snapshot.instances[&5].states[&raw].activation,
            )
        })
    };
    let up = StateKind::LogicalButton { down: false };
    let down = StateKind::LogicalButton { down: true };
    assert_eq!(sprint(component.snapshot().unwrap()), [(up, 1); 3]);
    component
        .advance_command(
            Command {
                frame: 1030,
                buttons: 0x881,
                ..Command::default()
            },
            &mut ReferenceWorld,
        )
        .unwrap();
    let snapshot = component.snapshot().unwrap();
    assert_eq!(sprint(snapshot), [(down, 1); 3]);
    assert_eq!(snapshot.instances[&5].variables[&1919], Value::Bool(true));
    assert!(!snapshot.initialization_complete());
    component
        .advance_command(
            Command {
                frame: 1031,
                ..Command::default()
            },
            &mut ReferenceWorld,
        )
        .unwrap();
    assert_eq!(sprint(component.snapshot().unwrap()), [(up, 1); 3]);
}

#[test]
#[ignore = "requires a local 1.74 Practice Range NPC graph pack with graph 0B92"]
fn firing_placement_reset_and_timer_boundaries_match_python() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = Arc::new(GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap());
    assert!(
        graphs.graph(0x0b92).is_some(),
        "the runtime pack lacks Practice Range graph 0B92"
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
#[ignore = "requires an authorized 104319 graph pack in ORPHEUS_RUNTIME_DIR"]
fn controller_and_training_bot_supported_startup_edges_match_python() {
    let runtime = orpheus_testkit::runtime_dir();
    let graphs = GraphStore::load(runtime.join("data/game_graphs_174.json.gz")).unwrap();
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

    // Only the enriched pack includes the NPC body bindings.
    if let Some(body) = graphs.entity_body(0x0400_0000_0000_1403) {
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
