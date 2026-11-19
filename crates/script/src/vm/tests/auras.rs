//! ApplyAura (`STU_431F3FCC`, `vm/auras.rs`): the applier's state puts its
//! aura on each body its targets list while listed, and the target's
//! component runs and removes the aura's instance by who put it there.

use super::*;

const AURA: u64 = 0x04a0_0000_0000_0001;
const PARAMETER: u64 = 0x0a40_0000_0000_002a;

/// A world that keeps the aura changes its states make.
#[derive(Default)]
struct Applier {
    changes: Vec<AuraChange>,
}

impl World for Applier {
    fn unknowns(&self) -> Unknowns {
        Unknowns::Refuse
    }

    fn change_aura(&mut self, _site: Site, change: AuraChange) -> bool {
        self.changes.push(change);
        true
    }
}

/// Entry → st0 Wait 1 s, whose subgraph holds st1 ApplyAura of [`AURA`]
/// on #375e with `0A4:002A` = 7, dealt by the owner.
fn applier(targets: &[u32], world: &mut Applier) -> Component {
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
                "STUStatescriptStateWait",
                Some(0),
                json!({"m_timeout":{"$":"STUConfigVarInt","m_value":1},
                    "m_subgraphPlug":plug(&[(2, "m_beginPlug")], &[1])}),
            ),
            node(
                2,
                "STU_431F3FCC",
                Some(1),
                json!({
                    "m_A76A8CF6":{"$":"STU_0E766B33","m_A76A8CF6":"0x04A0000000000001",
                        "m_resourceKey":null,"m_EE729DCB":0},
                    "m_targets":var(375, true),
                    "m_params":[{"$":"STU_814D595E","m_B5051BCE":"0x0A4000000000002A",
                        "m_value":{"$":"STUConfigVarInt","m_value":7}}],
                    "m_3273B2AB":{"$":"STU_B5A0CAF0"},
                }),
            ),
        ],
        json!([1, 2]),
        json!([0]),
    )]);
    let mut variables = Variables::new();
    variables.insert(
        375,
        Value::Array(
            targets
                .iter()
                .map(|&entity| Value::Entity(entity))
                .collect(),
        ),
    );
    Component::start(
        Arc::new(graphs),
        SEARCHER,
        variables,
        vec![Root::new(1, 42)],
        0,
        world,
        MAX_INIT_WORK,
    )
    .unwrap()
}

#[test]
fn an_apply_aura_puts_its_aura_on_each_listed_body_while_listed() {
    let mut world = Applier::default();
    let mut component = applier(&[ALLY, ENEMY], &mut world);
    let source = AuraSource {
        entity: SEARCHER,
        instance: 1,
        state: 1,
        activation: 1,
    };
    let apply = |target| AuraChange::Apply {
        source: Some(source),
        target,
        aura: AURA,
        instigator: SEARCHER,
        creator: SEARCHER,
        parameters: vec![(PARAMETER, Value::Integer(7))],
    };
    let remove = |target| AuraChange::Remove { source, target };
    assert_eq!(world.changes, [apply(ALLY), apply(ENEMY)]);
    world.changes.clear();
    // The ally leaves the list and a second enemy comes in; the enemy
    // still listed gets nothing new.
    component
        .set_entity_variable(
            375,
            Value::Array(vec![Value::Entity(ENEMY), Value::Entity(SECOND_ENEMY)]),
            &mut world,
        )
        .unwrap();
    component.advance_frame(1, &mut world).unwrap();
    assert_eq!(world.changes, [remove(ALLY), apply(SECOND_ENEMY)]);
    world.changes.clear();
    // The wait ends after 1 s (63 frames of 16 ms) and takes the state
    // with it: its auras come off.
    for frame in 2..=63 {
        component.advance_frame(frame, &mut world).unwrap();
    }
    assert_eq!(world.changes, [remove(ENEMY), remove(SECOND_ENEMY)]);
    assert!(
        component
            .snapshot()
            .unwrap()
            .assumptions
            .iter()
            .any(|assumption| { assumption.class == "STU_431F3FCC lifetime" })
    );
}

#[test]
fn a_body_runs_an_applied_aura_until_its_applier_takes_it_off() {
    // The aura names its graph in `m_14AA219A` only, as Nano Boost's
    // `0269` does: graph 50 takes `0A4:002A` into #499 and holds st0.
    let applied = json!({"$":"STUConfigVarGameMessage","m_gameMessage":"0x024000000000000F"});
    let aura_graph = graph(
        50,
        vec![
            node(
                0,
                "STUStatescriptEntryGameMessage",
                None,
                json!({"m_gameMessage":applied,
                    "m_params":[{"$":"STU_EF930FD7","m_B5051BCE":"0x0A4000000000002A",
                        "m_out_Var":var(499, false)}],
                    "m_outPlug":plug(&[(1, "m_beginPlug")], &[])}),
            ),
            node(1, "STUStatescriptState", Some(0), json!({})),
        ],
        json!([1]),
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
                "auras":{"0x04A0000000000001":{"m_14AA219A":"0x0580000000000032","m_7DAE8F10":99999.0}}
            }))
            .unwrap(),
        )
        .unwrap(),
    );
    let mut world = Applier::default();
    let mut component = Component::start(
        graphs,
        ENEMY,
        Variables::new(),
        vec![Root::new(1, 42)],
        0,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    let source = |activation| AuraSource {
        entity: SEARCHER,
        instance: 1,
        state: 1,
        activation,
    };
    let apply = |activation| AuraChange::Apply {
        source: Some(source(activation)),
        target: ENEMY,
        aura: AURA,
        instigator: SEARCHER,
        creator: SEARCHER,
        parameters: vec![(PARAMETER, Value::Integer(7))],
    };
    assert!(component.change_aura(&apply(1), &mut world).unwrap());
    assert!(component.change_aura(&apply(2), &mut world).unwrap());
    let auras = |component: &Component| -> Vec<(u16, Option<Value>)> {
        component
            .snapshot()
            .unwrap()
            .instances
            .iter()
            .filter(|(_, instance)| instance.graph == 50)
            .map(|(&id, instance)| (id, instance.variables.get(&499).cloned()))
            .collect()
    };
    let first = auras(&component);
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .all(|(_, value)| *value == Some(Value::Integer(7)))
    );
    let remove = AuraChange::Remove {
        source: source(1),
        target: ENEMY,
    };
    assert!(component.change_aura(&remove, &mut world).unwrap());
    assert_eq!(auras(&component), first[1..]);
    // Taken off once: a second removal changes nothing.
    assert!(!component.change_aura(&remove, &mut world).unwrap());
    assert!(world.changes.is_empty());
}
