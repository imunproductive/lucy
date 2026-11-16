//! The death layer `01CB`'s camera states and kill cam actions
//! (`vm/nodes.rs`), in its shape: the camera states run as plain states in
//! a build that refuses unknown classes, the kill cam actions and the
//! entity list action follow their out plugs.

use super::*;

/// Entry → `STU_A1DF3B2D` (#20) → `STU_3218BAEB` (#21) → `STU_5EAA04BB` →
/// `STU_9F3FE778` → set #22, and st0 `STU_D75CDECC` with st1 CameraLookAt
/// and st2 `STU_A103C85E` in its subgraph.
fn death_layer() -> Component {
    let graphs = store(vec![graph(
        42,
        vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug":plug(&[(4, "m_inPlug"), (1, "m_beginPlug")], &[])}),
            ),
            node(
                1,
                "STU_D75CDECC",
                Some(0),
                json!({
                    "m_15C19570":{"$":"STUConfigVarFloat","m_value":-1.0},
                    "m_D11CBE74":1,
                    "m_subgraphPlug":plug(&[(2, "m_beginPlug"), (3, "m_beginPlug")], &[1, 2]),
                }),
            ),
            node(2, "STU_96333BBE", Some(1), json!({})),
            node(3, "STU_A103C85E", Some(2), json!({})),
            node(
                4,
                "STU_A1DF3B2D",
                None,
                json!({"m_9E580984":var(20, false), "m_outPlug":plug(&[(5, "m_inPlug")], &[])}),
            ),
            node(
                5,
                "STU_3218BAEB",
                None,
                json!({"m_D1F6024E":var(21, false), "m_outPlug":plug(&[(6, "m_inPlug")], &[])}),
            ),
            node(
                6,
                "STU_5EAA04BB",
                None,
                json!({"m_15C19570":var(21, false), "m_outPlug":plug(&[(7, "m_inPlug")], &[])}),
            ),
            node(
                7,
                "STU_9F3FE778",
                None,
                json!({"m_outPlug":plug(&[(8, "m_inPlug")], &[])}),
            ),
            node(
                8,
                "STUStatescriptActionSetVar",
                None,
                json!({"m_out_Var":var(22, false), "m_value":true}),
            ),
        ],
        json!([1, 2, 3]),
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

#[test]
fn the_death_layers_cameras_are_plain_states_and_its_kill_cam_actions_run_through() {
    let component = death_layer();
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    let instance = &snapshot.instances[&1];
    for raw in 0..3 {
        assert!(instance.states[&raw].active, "state {raw}");
    }
    assert_eq!(instance.variables.get(&20), Some(&Value::Array(Vec::new())));
    // No reel: the recorder leaves its output alone.
    assert_eq!(instance.variables.get(&21), None);
    assert_eq!(instance.variables.get(&22), Some(&Value::Bool(true)));
}
