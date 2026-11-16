//! The velocity action (`vm/state_kinds.rs` `velocity_action`) in the shape
//! of Pharah's `0027` node 1.

use super::*;

fn float(value: f64) -> Json {
    json!({"$":"STUConfigVarFloat","m_value":value})
}

fn vector([x, y, z]: [f64; 3]) -> Json {
    json!({"$":"STUConfigVarVec3","m_value":{"X":x,"Y":y,"Z":z}})
}

/// Entry → the velocity action with `elements` → a set of #10.
fn velocity(elements: Json) -> Component {
    let graphs = store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(1, "m_inPlug")], &[])}),
            ),
            node(
                1,
                "STU_FF3471E9",
                None,
                json!({"m_BC5E91CF":elements, "m_outPlug":plug(&[(2, "m_inPlug")], &[])}),
            ),
            node(
                2,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(10, false), "m_value":true}),
            ),
        ],
        json!([]),
        json!([0]),
    )]);
    Component::start(
        Arc::new(graphs),
        42,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap()
}

fn followed(component: &Component) -> bool {
    component.snapshot().unwrap().instances[&1]
        .variables
        .get(&10)
        .is_some_and(|value| *value == Value::Bool(true))
}

#[test]
fn a_velocity_action_hands_its_elements_to_the_next_pose_in_order() {
    let mut component = velocity(json!([
        {"$":"STU_9AF0FA80", "m_9B7A63EA":float(0.0), "m_A9561CA0":2},
        {"$":"STU_4635C5E2", "m_9B7A63EA":vector([0.0, 20.0, 10.0]), "m_CFEC5F76":1},
        {"$":"STU_20E18047", "m_8213EB22":float(4.0), "m_A9561CA0":0, "m_priority":float(2.0)},
        // Multiply with no value: by 1, 1, 1. Channel 7 of a scalar does nothing.
        {"$":"STU_D93B3F5E", "m_CFEC5F76":4},
        {"$":"STU_748FC65A", "m_9B7A63EA":float(1.0), "m_A9561CA0":7},
    ]));
    assert!(followed(&component));
    let set = |index, operation, value, priority| {
        Motion::Velocity(VelocityElement::Modifier(Modifier {
            index,
            operation,
            value,
            priority,
        }))
    };
    assert_eq!(
        component.take_motions(),
        [
            set(34, Operation::Set, 0.0, 0.0),
            Motion::Velocity(VelocityElement::Vector {
                channel: 1,
                operation: VectorOperation::Add,
                value: [0.0, 20.0, 10.0],
                priority: 0.0,
            }),
            set(
                32,
                Operation::Clamp { upper: 4.0 },
                f64::from(f32::MIN_POSITIVE),
                2.0
            ),
            Motion::Velocity(VelocityElement::Vector {
                channel: 4,
                operation: VectorOperation::Multiply,
                value: [1.0; 3],
                priority: 0.0,
            }),
        ]
    );
}

#[test]
fn a_velocity_action_on_channel_0_is_refused() {
    let component = velocity(json!([
        {"$":"STU_4635C5E2", "m_9B7A63EA":vector([1.0, 0.0, 0.0]), "m_CFEC5F76":0},
    ]));
    assert!(!followed(&component));
    assert!(
        component
            .snapshot()
            .unwrap()
            .diagnostics
            .iter()
            .any(|diagnostic| format!("{diagnostic:?}").contains("velocity channel 0"))
    );
}
