//! The behaviours ported from leafall903/ORPHEUS (MIT), commit c239a84:
//! early ability presses (`docs/MCCREE_ROLL.txt`), state-input joins
//! (`docs/STATE_INPUT_JOIN.txt`), StateElapsed and the indexed action Switch
//! (`src/ow174/game/script/nodes.py`).

use super::*;

/// A plain state (raw `raw`) whose first transition plug leads to `target`.
fn plain(position: u16, raw: u16, target: u16) -> Json {
    node(
        position,
        "STUStatescriptState",
        Some(raw),
        json!({"m_transitionPlug":[plug(&[(target, "m_inPlug")], &[])]}),
    )
}

/// A join waiting for `count` distinct inputs, then checking `states`.
fn join(position: u16, count: i64, states: &[u16], out: &[(u16, &str)]) -> Json {
    node(
        position,
        "STU_1113EBD5",
        None,
        json!({
            "m_B24D1794":count, "m_122DF0CA":states,
            "m_inPlug":{"$":"STUStatescriptInputPlug","links":[]},
            "m_outPlug":plug(out, &[]),
        }),
    )
}

fn active(component: &Component, raw: u16) -> bool {
    component.snapshot().unwrap().instances[&1].states[&raw].active
}

fn marked(component: &Component, id: u32) -> Option<Value> {
    component.snapshot().unwrap().variables.get(&id).cloned()
}

/// Deadeye's shape (graph `022F`): the six-second state's transition is one
/// input of a two-input join. Its first input is not taken, so the state
/// keeps running; the second distinct input (here a message) follows the out
/// plug and aborts it. Following the out plug at once ended the state as it
/// began.
#[test]
fn a_join_waits_for_its_second_distinct_input_then_aborts_its_states() {
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            plain(1, 0, 2),
            join(2, 2, &[0], &[(3, "m_inPlug")]),
            set_entity(3, 100, json!(true)),
            on_message(4, 1, 2, "m_inPlug"),
        ],
        json!([1]),
        json!([0, 4]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    let snapshot = component.snapshot().unwrap();
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
    assert!(active(&component, 0));
    assert_eq!(marked(&component, 100), None);
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    assert!(active(&component, 0));
    message(&mut component, 1, &mut EmptyWorld);
    assert_eq!(marked(&component, 100), Some(Value::Bool(true)));
    assert!(!active(&component, 0));
}

/// The same input twice counts once; a join of three needs three distinct
/// producers or plugs. A count of 1 never passes: the first input only asks
/// for checks (upstream's native probe).
#[test]
fn a_join_counts_distinct_inputs_and_a_count_of_one_never_passes() {
    for (count, expected) in [(3, Some(Value::Bool(true))), (1, None)] {
        let graphs = Arc::new(store(vec![graph(
            42,
            vec![
                entry(&[(1, "m_beginPlug")]),
                plain(1, 0, 2),
                join(2, count, &[0], &[(3, "m_inPlug")]),
                set_entity(3, 100, json!(true)),
                on_message(4, 1, 2, "m_inPlug"),
                on_message(5, 2, 2, "m_inPlug"),
            ],
            json!([1]),
            json!([0, 4, 5]),
        )]));
        let mut component = start(graphs, 100, &mut EmptyWorld);
        message(&mut component, 1, &mut EmptyWorld);
        message(&mut component, 1, &mut EmptyWorld);
        assert_eq!(marked(&component, 100), None, "count {count}");
        assert!(active(&component, 0));
        message(&mut component, 2, &mut EmptyWorld);
        assert_eq!(marked(&component, 100), expected, "count {count}");
        assert_eq!(active(&component, 0), expected.is_none(), "count {count}");
    }
}

/// A first input from outside the states asks each listed active state to
/// check its transitions; their inputs then complete the join. The calling
/// state is not asked: its own transition is the input.
#[test]
fn a_join_s_first_input_asks_its_other_states_to_check_their_transitions() {
    let gate = |position, target| {
        node(
            position,
            "STU_3387AB5D",
            None,
            json!({
                "m_condition":var(50, true),
                "m_truePlug":plug(&[(target, "m_inPlug")], &[]),
                "m_falsePlug":plug(&[], &[]),
            }),
        )
    };
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug"), (2, "m_beginPlug")]),
            plain(1, 0, 3),
            plain(2, 1, 4),
            gate(3, 5),
            gate(4, 5),
            join(5, 2, &[0, 1], &[(6, "m_inPlug")]),
            set_entity(6, 100, json!(true)),
            on_message(7, 1, 8, "m_inPlug"),
            node(
                8,
                "STUStatescriptActionSetVar",
                None,
                json!({
                    "m_out_Var":var(50, true), "m_value":true,
                    "m_outPlug":plug(&[(5, "m_inPlug")], &[]),
                }),
            ),
        ],
        json!([1, 2]),
        json!([0, 7]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    assert!(active(&component, 0) && active(&component, 1));
    message(&mut component, 1, &mut EmptyWorld);
    assert_eq!(marked(&component, 100), Some(Value::Bool(true)));
    assert!(!active(&component, 0) && !active(&component, 1));
}

/// An out plug that takes no link still clears the inputs but aborts
/// nothing; the next input starts a new count.
#[test]
fn a_join_whose_out_plug_takes_nothing_aborts_nothing() {
    let graphs = Arc::new(store(vec![graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            plain(1, 0, 2),
            join(2, 2, &[0], &[]),
            on_message(3, 1, 2, "m_inPlug"),
        ],
        json!([1]),
        json!([0, 3]),
    )]));
    let mut component = start(graphs, 100, &mut EmptyWorld);
    message(&mut component, 1, &mut EmptyWorld);
    component.advance_frame(101, &mut EmptyWorld).unwrap();
    assert!(active(&component, 0));
}

/// StateElapsed in a transition: false while the state is younger than
/// m_AA6E611E, with the state's timer 0 set for when it is old enough; then
/// true, so the transition ends the state. Upstream: "1B75730 compares state
/// age and queues a timer when it is too early".
#[test]
fn state_elapsed_waits_for_the_calling_state_s_age() {
    let elapsed = |seconds: Json| {
        node(
            2,
            "STU_4E2BCAAB",
            None,
            json!({
                "m_AA6E611E":seconds,
                "m_truePlug":plug(&[(3, "m_inPlug")], &[]),
                "m_falsePlug":plug(&[], &[]),
            }),
        )
    };
    for (seconds, due_ms) in [
        (json!({"$":"STUConfigVarFloat","m_value":0.1}), 1700),
        (json!({"$":"STUConfigVarInt","m_value":1}), 2600),
    ] {
        let graphs = Arc::new(store(vec![graph(
            42,
            vec![
                entry(&[(1, "m_beginPlug")]),
                plain(1, 0, 2),
                elapsed(seconds),
                set_entity(3, 100, json!(true)),
            ],
            json!([1]),
            json!([0]),
        )]));
        let mut component = start(graphs, 100, &mut EmptyWorld);
        let snapshot = component.snapshot().unwrap();
        assert!(
            snapshot.assumptions.is_empty(),
            "{:?}",
            snapshot.assumptions
        );
        assert_eq!(
            snapshot.pending_timers[&1],
            [PendingTimer {
                due_ms,
                raw_state: 0,
                finish: false,
                parameter: 0
            }]
        );
        let due_frame = u32::try_from(due_ms / FRAME_MS).unwrap();
        component
            .advance_frame(due_frame - 1, &mut EmptyWorld)
            .unwrap();
        assert!(active(&component, 0));
        assert_eq!(marked(&component, 100), None);
        component.advance_frame(due_frame, &mut EmptyWorld).unwrap();
        assert_eq!(marked(&component, 100), Some(Value::Bool(true)));
        assert!(!active(&component, 0));
    }
}

/// A negative, out-of-range or non-number age is false and sets no timer.
#[test]
fn state_elapsed_with_a_bad_age_is_false_without_a_timer() {
    for seconds in [
        json!({"$":"STUConfigVarFloat","m_value":-0.5}),
        json!({"$":"STUConfigVarFloat","m_value":3_000_000.0}),
        json!({"$":"STUConfigVarBool","m_value":true}),
        Json::Null,
    ] {
        let graphs = Arc::new(store(vec![graph(
            42,
            vec![
                entry(&[(1, "m_beginPlug")]),
                plain(1, 0, 2),
                node(
                    2,
                    "STU_4E2BCAAB",
                    None,
                    json!({
                        "m_AA6E611E":seconds,
                        "m_truePlug":plug(&[(3, "m_inPlug")], &[]),
                        "m_falsePlug":plug(&[(4, "m_inPlug")], &[]),
                    }),
                ),
                set_entity(3, 100, json!(true)),
                set_entity(4, 100, json!(false)),
            ],
            json!([1]),
            json!([0]),
        )]));
        let component = start(graphs, 100, &mut EmptyWorld);
        let snapshot = component.snapshot().unwrap();
        assert_eq!(snapshot.variables[&100], Value::Bool(false), "{seconds}");
        assert!(snapshot.pending_timers.is_empty(), "{seconds}");
        assert!(!active(&component, 0), "{seconds}");
    }
}

/// The indexed Switch (m_F0F0720B = 1) follows the case whose number is the
/// value; a value out of range or not an integer takes the default plug.
#[test]
fn an_indexed_switch_selects_its_case_by_number() {
    for (value, expected) in [
        (Value::Integer(0), 0),
        (Value::Integer(1), 1),
        (Value::Integer(2), -1),
        (Value::Integer(-1), -1),
        (Value::Float(1.0), -1),
    ] {
        let case =
            |target: u16| json!({"$":"STU_F23DE9D7","m_value":null,"links":[[target,"m_inPlug"]]});
        let graphs = Arc::new(store(vec![graph(
            42,
            vec![
                entry(&[(1, "m_inPlug")]),
                node(
                    1,
                    "STUStatescriptActionSwitch",
                    None,
                    json!({
                        "m_F0F0720B":1,
                        "m_262AE982":var(50, true),
                        "m_FDD34C25":[case(2), case(3)],
                        "m_CD38D5DB":plug(&[(4, "m_inPlug")], &[]),
                        "m_outPlug":plug(&[(5, "m_inPlug")], &[]),
                    }),
                ),
                set_entity(2, 100, json!(0)),
                set_entity(3, 100, json!(1)),
                set_entity(4, 100, json!(-1)),
                set_entity(5, 101, json!(true)),
            ],
            json!([]),
            json!([0]),
        )]));
        let component = start_with(
            graphs,
            Variables::from([(50, value.clone())]),
            100,
            &mut EmptyWorld,
        );
        let snapshot = component.snapshot().unwrap();
        assert_eq!(
            snapshot.variables[&100],
            Value::Integer(expected),
            "{value:?}"
        );
        assert_eq!(snapshot.variables[&101], Value::Bool(true));
    }
}

/// An ability on button 35 (command bit 0x008) with a 0.1 s early-press
/// window; starting it sets a 0.5 s cooldown and #62, button 36 (0x010)
/// stops it and clears #62.
fn early_press_ability() -> Store {
    let mut fixture = graph(
        42,
        vec![
            entry(&[(1, "m_beginPlug")]),
            node(
                1,
                "STUStatescriptStateAbility",
                Some(0),
                json!({
                    "m_out_CooldownVar":var(60, true),
                    "m_A5F1A77B":35,
                    "m_8CBD15DA":36,
                    "m_246438AD":{"$":"STUConfigVarFloat","m_value":0.1},
                    "m_EF3C5C73":[{"m_out_Var":var(60, true), "m_value":0.5},
                                  {"m_out_Var":var(62, true), "m_value":true}],
                    "m_19899EAC":[{"m_out_Var":var(62, true), "m_value":false}],
                }),
            ),
        ],
        json!([1]),
        json!([0]),
    );
    fixture["fields"]["m_680A2CB2"] = json!([0]);
    Arc::new(store(vec![fixture]))
}

/// Runs frames `frames`, holding the command bits `buttons(frame)` gives.
fn drive(
    component: &mut Component,
    frames: std::ops::RangeInclusive<u32>,
    buttons: impl Fn(u32) -> u32,
) {
    for frame in frames {
        component
            .advance_command(
                Command {
                    frame,
                    buttons: buttons(frame),
                    ..Command::default()
                },
                &mut EmptyWorld,
            )
            .unwrap();
    }
}

/// Upstream `docs/MCCREE_ROLL.txt`: a buffered press stays with its own time
/// until the ability can start and is taken only then. A press released
/// long before the cooldown ends expires; one within the early-press window
/// starts the ability when the cooldown ends, once; a held button starts it
/// then too, and a fresh press after the cooldown does. Putting the press
/// back with the current time on every frame of the cooldown started the
/// ability by itself at the end of it.
#[test]
fn an_early_press_starts_the_ability_only_within_its_window() {
    // Frame 1 starts the ability (cooldown from 16 ms to 516 ms, frame 32),
    // frame 2 stops it.
    let opening = |frame| match frame {
        1 => 0x008,
        2 => 0x010,
        _ => 0,
    };
    let running = |component: &Component| marked(component, 62) == Some(Value::Bool(true));
    // Released 356 ms before the cooldown ends: expired.
    let mut component = start(early_press_ability(), 0, &mut EmptyWorld);
    drive(&mut component, 1..=40, |frame| match frame {
        10 => 0x008,
        frame => opening(frame),
    });
    assert!(!running(&component));
    // A fresh press after the cooldown starts it.
    drive(
        &mut component,
        41..=42,
        |frame| {
            if frame == 41 { 0x008 } else { 0 }
        },
    );
    assert!(running(&component));

    // Released 116 ms before: just outside the 100 ms window.
    let mut component = start(early_press_ability(), 0, &mut EmptyWorld);
    drive(&mut component, 1..=40, |frame| match frame {
        25 => 0x008,
        frame => opening(frame),
    });
    assert!(!running(&component));

    // Pressed 52 ms before: it starts when the cooldown ends, and only once.
    let mut component = start(early_press_ability(), 0, &mut EmptyWorld);
    drive(&mut component, 1..=31, |frame| match frame {
        29 => 0x008,
        frame => opening(frame),
    });
    assert!(!running(&component));
    drive(&mut component, 32..=33, |_| 0);
    assert!(running(&component));
    // Stopped; the next cooldown ends at frame 64 with no press left.
    drive(
        &mut component,
        34..=80,
        |frame| {
            if frame == 34 { 0x010 } else { 0 }
        },
    );
    assert!(!running(&component));

    // Held through the cooldown: it starts when the cooldown ends.
    let mut component = start(early_press_ability(), 0, &mut EmptyWorld);
    drive(&mut component, 1..=31, |frame| match frame {
        10.. => 0x008,
        frame => opening(frame),
    });
    assert!(!running(&component));
    drive(&mut component, 32..=33, |_| 0x008);
    assert!(running(&component));
}

/// Cassidy's Deadeye (graph `022F`): its six-second window (ChaseVar st18,
/// node 27) feeds two joins through its transitions. It keeps running when it
/// begins (it used to end at once) until secondary fire (button 29, command
/// bit 0x002) gives join 8 its second input, or until its six seconds run
/// out. The world tolerates the graph's other unknown content, as release
/// builds do.
#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_deadeye_window_runs_until_cancelled_or_six_seconds_pass() {
    struct Tolerant;
    impl World for Tolerant {
        fn unknowns(&self) -> Unknowns {
            Unknowns::Tolerate
        }
    }

    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
    let window = |component: &Component| {
        component.snapshot().unwrap().instances[&5]
            .states
            .get(&18)
            .is_some_and(|state| state.active)
    };
    for cancel in [true, false] {
        let mut world = Tolerant;
        let mut root = Root::new(5, 0x022f);
        // The window's length in seconds, which the ChaseVar counts down.
        root.variables.insert(567, Value::Float(6.0));
        let mut component = Component::start(
            Arc::clone(&graphs),
            0xa000_0101,
            Variables::new(),
            vec![root],
            1000,
            &mut world,
            MAX_INIT_WORK,
        )
        .unwrap();
        assert!(component.begin_state(5, 18, &mut world).unwrap());
        assert!(window(&component));
        for frame in 1001..=1062 {
            component
                .advance_command(
                    Command {
                        frame,
                        ..Command::default()
                    },
                    &mut world,
                )
                .unwrap();
            assert!(window(&component), "frame {frame}");
        }
        if cancel {
            component
                .advance_command(
                    Command {
                        frame: 1063,
                        buttons: 0x002,
                        ..Command::default()
                    },
                    &mut world,
                )
                .unwrap();
            assert!(!window(&component));
            continue;
        }
        // Begun at 16 000 ms: it still runs 5.9 s later (frame 1370) and has
        // ended at 6 s (22 000 ms), by frame 1375 (the ChaseVar's own tick
        // sets the exact frame).
        for frame in 1063..=1375 {
            component
                .advance_command(
                    Command {
                        frame,
                        ..Command::default()
                    },
                    &mut world,
                )
                .unwrap();
            if frame <= 1370 {
                assert!(window(&component), "frame {frame}");
            }
        }
        assert!(!window(&component));
    }
}
