//! Receiving states and their held messages (`0F4A`, `0E8E`): computed
//! messages, a held message's changed parameters, collection mode.

use super::*;

/// A receiving state whose message is an expression (`0F4A` st53: `07A1`
/// or `07A2` by the attacking team) takes the message it names now, not
/// another, and its sender's end lets it go.
#[test]
fn a_receiving_state_takes_the_message_its_expression_names() {
    const NAMED: u64 = 0x0240_0000_0000_0007;
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
                    "m_gameMessage":var(5,true),
                    "m_33DA5163":plug(&[(2,"m_beginPlug")],&[1]),
                    "m_452862E6":plug(&[(3,"m_beginPlug")],&[2]),
                }),
            ),
            node(2, "STUStatescriptState", Some(1), json!({})),
            node(3, "STUStatescriptState", Some(2), json!({})),
        ],
        json!([1, 2, 3]),
        json!([0]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![receiver])),
        0xd000_0010,
        Variables::from([(5, Value::Asset(NAMED))]),
        vec![Root::new(1, 43)],
        100,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), [0, 1]);
    component
        .receive_message(
            0x8000_0332,
            0x0240_0000_0000_0006,
            Variables::new(),
            &mut EmptyWorld,
        )
        .unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), [0, 1]);
    component
        .receive_message(0x8000_0332, NAMED, Variables::new(), &mut EmptyWorld)
        .unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(active(snapshot, 1), [0, 2]);
    component
        .end_message(0x8000_0332, NAMED, &mut EmptyWorld)
        .unwrap();
    assert_eq!(active(component.snapshot().unwrap(), 1), [0, 1]);
}

/// A persistent sender's new parameters reach the receiving state that
/// holds its message (`Component::update_message`): its output takes the
/// new value and its receive subgraph runs on, not begun again (its
/// action, which copies the output, keeps the first value).
#[test]
fn a_held_messages_new_parameters_reach_its_receiver() {
    const MESSAGE: u64 = 0x0240_0000_0000_0724;
    const SENDER: u32 = 0xd000_0010;
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
                    "m_gameMessage":{"m_gameMessage":"0x0240000000000724"},
                    "m_params":[{"m_B5051BCE":"0x0A40000000000473","m_out_Var":var(7,false)}],
                    "m_33DA5163":plug(&[(2,"m_beginPlug")],&[1]),
                    "m_452862E6":plug(&[(3,"m_inPlug")],&[]),
                }),
            ),
            node(2, "STUStatescriptState", Some(1), json!({})),
            node(
                3,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(8,false),"m_value":var(7,false)}),
            ),
        ],
        json!([1, 2]),
        json!([0]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![receiver])),
        0x8000_0416,
        Variables::new(),
        vec![Root::new(1, 43)],
        100,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let variable = |component: &Component, id| {
        component.snapshot().unwrap().instances[&1]
            .variables
            .get(&id)
            .cloned()
    };
    component
        .receive_message(
            SENDER,
            MESSAGE,
            Variables::from([(0x0473, Value::Integer(0))]),
            &mut EmptyWorld,
        )
        .unwrap();
    assert_eq!(variable(&component, 7), Some(Value::Integer(0)));
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
    // The receive subgraph's action copied the output (#8 = #7).
    assert_eq!(variable(&component, 8), Some(Value::Integer(0)));
    component
        .update_message(
            SENDER,
            MESSAGE,
            Variables::from([(0x0473, Value::Integer(1))]),
            &mut EmptyWorld,
        )
        .unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    assert_eq!(variable(&component, 7), Some(Value::Integer(1)));
    assert_eq!(variable(&component, 8), Some(Value::Integer(0)));
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
}

/// `0E8E` st49: a receiving state in collection mode (`m_39B47960` 1)
/// outputs every holder's parameter, in the order they came in, and keeps
/// the others when one lets go; the last one's end takes it back to idle.
#[test]
fn a_collecting_receiver_lists_every_holders_parameter() {
    const MESSAGE: u64 = 0x0240_0000_0000_08ae;
    const FIRST: u32 = 0xa000_0201;
    const SECOND: u32 = 0xa000_0301;
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
                    "m_gameMessage":{"m_gameMessage":"0x02400000000008AE"},
                    "m_params":[{"m_B5051BCE":"0x0A400000000004B8","m_out_Var":var(7,false)}],
                    "m_39B47960":1,
                    "m_33DA5163":plug(&[(2,"m_beginPlug")],&[1]),
                }),
            ),
            node(2, "STUStatescriptState", Some(1), json!({})),
        ],
        json!([1, 2]),
        json!([0]),
    );
    let mut component = Component::start(
        Arc::new(store(vec![receiver])),
        0x8000_0332,
        Variables::new(),
        vec![Root::new(1, 43)],
        100,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    let listed = |component: &Component| {
        component.snapshot().unwrap().instances[&1]
            .variables
            .get(&7)
            .cloned()
    };
    for body in [FIRST, SECOND] {
        component
            .receive_message(
                body,
                MESSAGE,
                Variables::from([(0x04b8, Value::Entity(body))]),
                &mut EmptyWorld,
            )
            .unwrap();
    }
    assert_eq!(
        listed(&component),
        Some(Value::Array(vec![
            Value::Entity(FIRST),
            Value::Entity(SECOND)
        ]))
    );
    component
        .end_message(FIRST, MESSAGE, &mut EmptyWorld)
        .unwrap();
    assert_eq!(
        listed(&component),
        Some(Value::Array(vec![Value::Entity(SECOND)]))
    );
    assert_eq!(active(component.snapshot().unwrap(), 1), [0]);
    component
        .end_message(SECOND, MESSAGE, &mut EmptyWorld)
        .unwrap();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
    // Back in its idle subgraph, listing no one.
    assert_eq!(active(snapshot, 1), [0, 1]);
    assert_eq!(listed(&component), Some(Value::Array(Vec::new())));
}
