//! Unkeyed membership (`STU_45EDF12D`, `STU_821116A6`) in authored
//! conditions and config expressions: the ports of
//! `tests/test_script_membership.py`.

use super::*;
use serde_json::json;

/// The instance variables an evaluation reads, and every config it hands to
/// the native query (which answers a computed null, as the Python test's
/// patched `Context.native` does).
#[derive(Default)]
struct Reads {
    instance: BTreeMap<u32, Value>,
    native: Vec<Json>,
}

impl Context for Reads {
    fn variable(&mut self, variable: Variable) -> Value {
        match variable.scope {
            Scope::Instance => self.instance.get(&variable.id).cloned(),
            Scope::Entity => None,
        }
        .unwrap_or(Value::Null)
    }

    fn native(&mut self, config: &Json) -> Option<Value> {
        self.native.push(config.clone());
        Some(Value::Null)
    }
}

fn dynamic(id: u32, entity: bool) -> Json {
    json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":entity})
}

/// `tests/test_script_membership.py::MembershipTests::test_config_matches_members_and_singletons_with_vm_equality`:
/// an unkeyed `STU_45EDF12D` is true when the container (an array, or a
/// single value) holds a member equal to the value under the VM's equality,
/// so an integer member matches a float value and nothing is in a null.
#[test]
fn config_matches_members_and_singletons_with_vm_equality() {
    let config = json!({
        "$":"STU_45EDF12D", "m_814F8EC3":dynamic(1, false), "m_value":dynamic(2, false),
    });
    let assets = |guids: &[u64]| Value::Array(guids.iter().copied().map(Value::Asset).collect());
    for (container, value, expected) in [
        (assets(&[1, 2]), Value::Asset(2), true),
        (assets(&[1, 2]), Value::Asset(3), false),
        (Value::Asset(2), Value::Asset(2), true),
        (
            Value::Array(vec![Value::Integer(1), Value::Integer(2)]),
            Value::Float(2.0),
            true,
        ),
        (Value::Array(Vec::new()), Value::Null, false),
        (Value::Null, Value::Bool(false), false),
    ] {
        let mut reads = Reads::default();
        reads.instance.insert(1, container.clone());
        reads.instance.insert(2, value.clone());
        assert_eq!(
            expr::evaluate(&config, &mut reads),
            Ok(Value::Bool(expected)),
            "{container:?} holds {value:?}"
        );
    }
}

/// `tests/test_script_membership.py::MembershipTests::test_keyed_config_leaves_its_operands_to_the_existing_native_query`:
/// a keyed `STU_45EDF12D` evaluates none of its operands; the native query
/// gets the whole config, once, and its answer is the value.
#[test]
fn keyed_config_leaves_its_operands_to_the_existing_native_query() {
    let config = json!({
        "$":"STU_45EDF12D", "m_key":1, "m_814F8EC3":{"$":"UnresolvedContainer"}, "m_value":2,
    });
    let mut reads = Reads::default();
    assert_eq!(expr::evaluate(&config, &mut reads), Ok(Value::Null));
    assert_eq!(reads.native, [config]);
}

fn authored_graphs() -> GraphStore {
    GraphStore::load(orpheus_testkit::runtime_dir().join("data/game_graphs_174.json.gz")).unwrap()
}

/// The probe graph's index, its run message, and the entity variables that
/// carry its container in and the tested branch out.
const PROBE: u32 = 0x4242;
const RUN: u64 = 0x0240_0000_0000_0001;
const CONTAINER: u32 = 900;
const BRANCH: u32 = 901;

/// An authored node at `pos` of the probe graph, its fields as authored.
fn copied(node: &Node, pos: u16) -> Json {
    json!({
        "pos":pos, "class":node.class, "uid":pos, "rid":pos, "remote":-1, "state":null,
        "client_only":node.client_only, "server_only":node.server_only,
        "fields":node.fields.clone(),
    })
}

fn set_var(pos: u16, target: Json, value: Json, next: Option<u16>) -> Json {
    let links = next.map_or_else(Vec::new, |next| vec![json!([next, "m_inPlug"])]);
    json!({
        "pos":pos, "class":"STUStatescriptActionSetVar", "uid":pos, "rid":pos, "remote":-1,
        "state":null, "client_only":false, "server_only":false,
        "fields":{
            "m_out_Var":target, "m_value":value, "m_2BBEEAB8":false, "m_ADEB6E05":true,
            "m_outPlug":{"$":"STUStatescriptOutputPlug","links":links},
        },
    })
}

/// A graph whose `RUN` entry writes the entity's `CONTAINER` into the
/// instance variable `operand`, then runs the node at `tested`.
fn probe(operand: u32, tested: u16, mut nodes: Vec<Json>) -> Component {
    nodes.push(json!({
        "pos":0, "class":"STUStatescriptEntryGameMessage", "uid":0, "rid":0, "remote":-1,
        "state":null, "client_only":false, "server_only":false,
        "fields":{
            "m_gameMessage":{"m_gameMessage":format!("0x{RUN:016X}")},
            "m_outPlug":{"$":"STUStatescriptOutputPlug","links":[[1, "m_inPlug"]]},
        },
    }));
    nodes.push(set_var(
        1,
        dynamic(operand, false),
        dynamic(CONTAINER, true),
        Some(tested),
    ));
    let items = nodes
        .iter()
        .filter_map(|node| node["pos"].as_u64())
        .max()
        .unwrap()
        + 1;
    let graph = json!({
        "index":PROBE, "items":items, "nodes":nodes, "states":[], "entries":[0], "remote":[],
        "sync_vars":[], "containers":{}, "unknown_nodes":[], "fields":{},
        "header":{
            "m_nodesBitCount":8, "m_statesBitCount":4, "m_remoteSyncNodesBitCount":4,
            "m_syncVarsBitCount":4
        },
    });
    let bundle = json!({"version":1, "graphs":[graph], "bodies":{}, "missing":[]});
    let graphs = GraphStore::from_json(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    Component::start(
        Arc::new(graphs),
        0x8000_0101,
        Variables::new(),
        vec![Root::new(1, PROBE)],
        0,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap()
}

/// Write the container and run the probe's entry.
fn run_with(component: &mut Component, container: Value) {
    component
        .set_entity_variable(CONTAINER, container, &mut EmptyWorld)
        .unwrap();
    component
        .set_entity_variable(BRANCH, Value::Null, &mut EmptyWorld)
        .unwrap();
    assert_eq!(
        component.queue_message(1, RUN, Variables::new()).unwrap(),
        1
    );
    component.run_now(&mut EmptyWorld).unwrap();
}

/// `tests/test_script_membership.py::AuthoredMembershipTests::test_authored_017b_condition_and_config_read_the_same_members`:
/// 017B's condition expression (rid 18, an `STU_45EDF12D` config) and its
/// membership condition (rid 44, `STU_821116A6`) read the same operands,
/// and both take the true branch exactly when the container in #659 holds
/// identifier 01FD, as an array member or as a single value.
#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_017b_condition_and_config_read_the_same_members() {
    let graphs = authored_graphs();
    let authored = graphs.graph(0x017B).unwrap();
    let expression = authored.by_rid(18).unwrap();
    let condition = authored.by_rid(44).unwrap();
    let config = expression.field("m_condition").unwrap();
    for field in ["m_814F8EC3", "m_key", "m_value"] {
        assert_eq!(config.get(field), condition.fields.get(field), "{field}");
    }
    let target = Value::Asset(0x0D80_0000_0000_01FD);
    for node in [expression, condition] {
        let mut tested = copied(node, 2);
        tested["fields"]["m_truePlug"]["links"] = json!([[3, "m_inPlug"]]);
        tested["fields"]["m_falsePlug"]["links"] = json!([[4, "m_inPlug"]]);
        let branch = |pos, taken: bool| {
            set_var(
                pos,
                dynamic(BRANCH, true),
                json!({"$":"STUConfigVarBool","m_value":taken}),
                None,
            )
        };
        let mut component = probe(659, 2, vec![tested, branch(3, true), branch(4, false)]);
        for (container, expected) in [
            (Value::Array(vec![Value::Asset(1), target.clone()]), true),
            (Value::Array(vec![Value::Asset(1)]), false),
            (target.clone(), true),
        ] {
            run_with(&mut component, container.clone());
            let snapshot = component.snapshot().unwrap();
            assert_eq!(
                snapshot.variables.get(&BRANCH),
                Some(&Value::Bool(expected)),
                "{} with {container:?}",
                node.class
            );
            assert!(
                snapshot.diagnostics.is_empty(),
                "{:?}",
                snapshot.diagnostics
            );
        }
    }
}

/// `tests/test_script_membership.py::AuthoredMembershipTests::test_0254_authored_rifle_and_helix_tags_select_distinct_stat_conditions`:
/// with the damage tags of 0254's rifle volley (rid 23) in #510, only the
/// rifle's stat condition (rid 118) holds; with the Helix Rocket volley's
/// (rid 35), only the Helix condition (rid 122) does.
#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_0254_rifle_and_helix_tags_select_distinct_stat_conditions() {
    let graphs = authored_graphs();
    let authored = graphs.graph(0x0254).unwrap();
    let conditions =
        [118, 122].map(|rid| authored.by_rid(rid).unwrap().field("m_condition").unwrap());
    for (volley, expected) in [(23, [true, false]), (35, [false, true])] {
        let hit = authored
            .by_rid(volley)
            .unwrap()
            .field("m_6396149F")
            .unwrap();
        let tags = hit["m_modifyHealth"]["m_148F3152"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| expr::evaluate(&tag["m_EE3055BD"], &mut Reads::default()).unwrap())
            .collect();
        let mut reads = Reads::default();
        reads.instance.insert(510, Value::Array(tags));
        let found = conditions.map(|config| expr::evaluate(config, &mut reads).unwrap());
        assert_eq!(found, expected.map(Value::Bool), "volley rid {volley}");
    }
}

/// `tests/test_script_membership.py::AuthoredMembershipTests::test_01c7_gain_uses_the_first_matching_authored_damage_tag`:
/// 01C7's switch (rid 101) sets the gain #18702 by the first case whose
/// damage tag #510 holds: 4910 alone gives -6, an unlisted tag the default
/// 0, and 4910 with 490F the earlier 490F case's -3.
#[test]
#[ignore = "requires a derived 1.74 graph bundle"]
fn authored_01c7_gain_uses_the_first_matching_authored_damage_tag() {
    let graphs = authored_graphs();
    let authored = graphs.graph(0x01C7).unwrap();
    let selector = authored.by_rid(101).unwrap();
    let mut nodes = vec![copied(selector, selector.pos)];
    let cases = selector.fields["m_FDD34C25"].as_array().unwrap().len();
    let mut targets: Vec<u16> = selector
        .links("m_CD38D5DB")
        .filter_map(|(target, _)| target)
        .collect();
    for case in 0..cases {
        targets.extend(
            selector
                .links(&format!("m_FDD34C25[{case}]"))
                .filter_map(|(target, _)| target),
        );
    }
    for target in targets {
        // The gain's writes, without what follows them.
        let node = authored.node(target).unwrap();
        let mut write = copied(node, node.pos);
        write["fields"]["m_outPlug"]["links"] = json!([]);
        nodes.push(write);
    }
    let mut component = probe(510, selector.pos, nodes);
    let gain = Variable {
        scope: Scope::Instance,
        id: 18702,
    };
    for (tags, expected) in [(&[0x4910][..], -6), (&[0x2D2F], 0), (&[0x4910, 0x490F], -3)] {
        let tags = tags
            .iter()
            .map(|&tag| Value::Asset(0x0D80_0000_0000_0000 | tag))
            .collect();
        run_with(&mut component, Value::Array(tags));
        assert_eq!(
            component.resolved_variable(1, gain),
            Some(Value::Integer(expected))
        );
        let snapshot = component.snapshot().unwrap();
        assert!(
            snapshot.diagnostics.is_empty(),
            "{:?}",
            snapshot.diagnostics
        );
    }
}
