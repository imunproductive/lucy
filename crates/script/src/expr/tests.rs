
use super::*;
use std::collections::BTreeMap;

use serde_json::json;

use crate::graph::GraphStore;

#[derive(Default)]
struct Vars {
    instance: BTreeMap<u32, Value>,
    entity: BTreeMap<u32, Value>,
    reads: Vec<Variable>,
}

impl Context for Vars {
    fn variable(&mut self, variable: Variable) -> Value {
        self.reads.push(variable);
        let values = match variable.scope {
            Scope::Instance => &self.instance,
            Scope::Entity => &self.entity,
        };
        values.get(&variable.id).cloned().unwrap_or(Value::Null)
    }

    fn parent_variable(&mut self, levels: u8, variable: Variable) -> Value {
        match (levels, variable.scope, variable.id) {
            (1, Scope::Instance, 14152) => Value::Asset(0x02e0_0000_0000_0002),
            _ => Value::Null,
        }
    }

    fn entity_member(&mut self, entity: u32, id: u32) -> Value {
        match (entity, id) {
            (0xa000_0100, 7423) => Value::Integer(23),
            _ => Value::Null,
        }
    }

    fn ruleset(&mut self, id: u32) -> Value {
        match id {
            10265 => Value::Float(1.0),
            _ => Value::Null,
        }
    }

    fn native(&mut self, config: &Json) -> Option<Value> {
        match class(config)? {
            "STU_37F20D24" => Some(Value::Asset(0x0230_0000_0000_0018)),
            _ => None,
        }
    }
}

fn variable(id: u32, scope: Scope) -> Json {
    json!({
        "$": "STUConfigVarDynamic",
        "m_identifier": id,
        "m_60DB8F99": scope == Scope::Entity,
    })
}

fn expression(opcodes: Json, dynamics: &[Json], floats: &[f64], configs: &[Json]) -> Json {
    json!({
        "$": "STU_919FD47C",
        "m_configVars": configs,
        "m_expression": {
            "m_opcodes": opcodes,
            "m_dynamicVars": dynamics,
            "m_D99EF254": floats,
        },
    })
}

#[test]
fn authored_message_map_looks_up_the_first_matching_key_without_wire() {
    use crate::wire::{ValueEncoding, WireError, write_value};
    use orpheus_core::bits::BitWriter;

    let row = |key: &str, message: &str| {
        json!({
            "$":"STU_98DD1872", "m_EE729DCB":0,
            "m_key":{"$":"STU_B70476F3","m_548E5344":key,"m_resourceKey":null},
            "m_value":{"$":"STUConfigVarGameMessage","m_gameMessage":message,"m_resourceKey":null},
        })
    };
    let mut literal = json!({
        "$":"STU_7757D403", "m_EE729DCB":0,
        "m_configVars":[
            row("0x0A90000000000042", "0x0240000000000003"),
            row("0x0A90000000000042", "0x0240000000000004"),
        ],
    });
    let mut context = Vars::default();
    let map = evaluate(&literal, &mut context).unwrap();
    assert_eq!(
        evaluate(&literal["m_configVars"][0]["m_key"], &mut context),
        Ok(Value::Asset(0x0a90_0000_0000_0042))
    );
    assert!(truthy(&map));
    context.instance.insert(1, map.clone());
    let lookup = expression(
        json!([8, 0, 8, 1, 17, 0]),
        &[variable(1, Scope::Instance), variable(2, Scope::Instance)],
        &[],
        &[],
    );
    for (key, result) in [
        (0x0a90_0000_0000_0042, Value::Asset(0x0240_0000_0000_0003)),
        (0x0a90_0000_0000_0043, Value::Null),
    ] {
        context.instance.insert(2, Value::Asset(key));
        assert_eq!(evaluate(&lookup, &mut context), Ok(result));
    }
    literal["m_configVars"][0]["m_key"]["m_548E5344"] = json!("0x0240000000000042");
    assert_eq!(
        evaluate(&literal["m_configVars"][0]["m_key"], &mut context),
        Err(EvalError::UnsupportedConfig("STU_B70476F3".into()))
    );
    assert_eq!(
        evaluate(&literal, &mut context),
        Err(EvalError::UnsupportedConfig("STU_7757D403".into()))
    );
    literal["m_configVars"] = Json::Null;
    let empty = evaluate(&literal, &mut context).unwrap();
    assert_eq!(empty, Value::Map(Vec::new()));
    assert!(!truthy(&empty));
    for encoding in [ValueEncoding::Explicit, ValueEncoding::Compact] {
        let mut bits = BitWriter::new();
        assert_eq!(
            write_value(&mut bits, &map, encoding),
            Err(WireError::InvalidValue)
        );
        assert_eq!(bits.bit_len(), 0);
    }
}

#[test]
fn fieldless_native_resource_evaluates_to_a_zero_asset() {
    let literal = json!({"$":"STU_ABB6C23E", "m_EE729DCB":0, "m_resourceKey":null});
    let mut context = Vars::default();
    assert_eq!(evaluate(&literal, &mut context), Ok(Value::Asset(0)));

    for (field, value) in [
        ("m_resourceKey", json!("0x0D80000000000015")),
        ("m_EE729DCB", json!(1)),
        ("m_unknown", json!("0x0D80000000000015")),
    ] {
        let mut config = literal.clone();
        config[field] = value;
        assert_eq!(
            evaluate(&config, &mut context),
            Err(EvalError::UnsupportedConfig("STU_ABB6C23E".into()))
        );
    }
}

const FILTER_OWNER: u32 = 0xa000_0101;
const REQUIRED_TAG: u64 = 0x0d60_0000_0000_0069;
const SECOND_TAG: u64 = 0x0d60_0000_0000_0042;
const EXCLUDED_TAG: u64 = 0x0d60_0000_0000_0041;

#[derive(Default)]
struct FilterWorld {
    entity: Option<u32>,
    flags: Option<u64>,
    tags: BTreeMap<u64, bool>,
}

impl Context for FilterWorld {
    fn owner(&mut self) -> Value {
        self.entity.map_or(Value::Null, Value::Entity)
    }

    fn entity_filter_flags(&mut self, entity: u32) -> Option<u64> {
        assert_eq!(entity, FILTER_OWNER);
        self.flags
    }

    fn has_target_tag(&mut self, entity: u32, tag: u64) -> Option<bool> {
        assert_eq!(entity, FILTER_OWNER);
        self.tags.get(&tag).copied()
    }
}

fn tag_config(tag: u64) -> Json {
    json!({"$":"STU_357DABBB", "m_7D7C86A1":format!("0x{tag:016X}"), "m_resourceKey":null})
}

fn tag_list(tags: &[u64]) -> Json {
    json!({
        "$":"STU_06610EBC",
        "m_configVars": tags.iter().copied().map(tag_config).collect::<Vec<_>>(),
    })
}

fn entity_filter(exclusion: u64) -> Json {
    json!({
        "$":"STU_AC829876",
        "m_entity":{"$":"STU_B5A0CAF0"},
        "m_filter":{
            "$":"STUConfigVarFilter",
            "m_84065FD1":0x3fff_ffff,
            "m_4EA686DB":exclusion,
        },
    })
}

#[test]
fn entity_filter_distinguishes_rejection_from_unavailable_facts() {
    let mut config = entity_filter(2);
    config["m_filter"]["m_9C7BD8B8"] =
        json!({"$":"STU_357DABBB", "m_7D7C86A1":null, "m_resourceKey":null});
    config["m_filter"]["m_8B2A4DF3"] = tag_config(REQUIRED_TAG);
    let mut world = FilterWorld {
        entity: Some(FILTER_OWNER),
        ..Default::default()
    };
    let unknown = Err(EvalError::UnsupportedConfig("STU_AC829876".into()));
    assert_eq!(evaluate(&config, &mut world), unknown);
    world.tags.insert(REQUIRED_TAG, false);
    assert_eq!(evaluate(&config, &mut world), Ok(Value::Bool(false)));
    world.tags.insert(REQUIRED_TAG, true);
    assert_eq!(evaluate(&config, &mut world), unknown);
    world.flags = Some(0x100);
    assert_eq!(evaluate(&config, &mut world), Ok(Value::Bool(true)));
    world.flags = Some(2 | 0x100);
    world.tags.clear();
    assert_eq!(evaluate(&config, &mut world), Ok(Value::Bool(false)));
    config["m_filter"]["m_66747E7D"] = json!(2);
    assert_eq!(evaluate(&config, &mut world), unknown);
}

#[test]
fn entity_filter_applies_any_all_and_excluded_tag_lists() {
    let mut config = entity_filter(0);
    config["m_filter"]["m_B486603E"] = tag_list(&[SECOND_TAG, REQUIRED_TAG]);
    config["m_filter"]["m_8B2A4DF3"] = tag_list(&[REQUIRED_TAG]);
    config["m_filter"]["m_C1903771"] = tag_config(EXCLUDED_TAG);
    let mut world = FilterWorld {
        entity: Some(FILTER_OWNER),
        tags: BTreeMap::from([(REQUIRED_TAG, true), (EXCLUDED_TAG, false)]),
        ..Default::default()
    };
    assert_eq!(evaluate(&config, &mut world), Ok(Value::Bool(true)));
    world.tags.insert(REQUIRED_TAG, false);
    assert_eq!(evaluate(&config, &mut world), Ok(Value::Bool(false)));
    world.tags.insert(REQUIRED_TAG, true);
    world.tags.insert(EXCLUDED_TAG, true);
    assert_eq!(evaluate(&config, &mut world), Ok(Value::Bool(false)));
    world.tags.remove(&EXCLUDED_TAG);
    let unsupported = Err(EvalError::UnsupportedConfig("STU_AC829876".into()));
    assert_eq!(evaluate(&config, &mut world), unsupported);
    world.tags.insert(EXCLUDED_TAG, false);
    config["m_filter"]["m_B486603E"]["m_configVars"] =
        json!([tag_config(REQUIRED_TAG), {"$":"unresolved-tag-config"}]);
    assert_eq!(evaluate(&config, &mut world), unsupported,);
}

#[test]
fn direct_target_tag_query_reads_membership_on_the_typed_entity() {
    let query = json!({
        "$":"STU_25ECB55F", "m_88B0050A":"0x0D60000000000069", "m_entity":{"$":"STU_B5A0CAF0"}
    });
    let mut world = FilterWorld {
        entity: Some(FILTER_OWNER),
        ..Default::default()
    };
    assert_eq!(
        evaluate(&query, &mut world),
        Err(EvalError::UnsupportedConfig("STU_25ECB55F".into())),
    );
    for present in [true, false] {
        world.tags.insert(REQUIRED_TAG, present);
        assert_eq!(evaluate(&query, &mut world), Ok(Value::Bool(present)));
    }
    world.entity = None;
    assert_eq!(evaluate(&query, &mut world), Ok(Value::Bool(false)));
}

#[test]
fn distinguishes_identifiers_arrays_entities_and_float_comparisons() {
    let id = Value::Asset(5);
    assert!(!equal(&id, &Value::Integer(5)));
    assert!(equal(&id, &Value::Bool(true)));
    assert!(equal(&Value::Integer(1), &Value::Float(1.000001)));
    assert!(!equal(&Value::Integer(1), &Value::Float(1.001)));
    assert!(equal(&Value::Float(0.0), &Value::Null));
    assert!(equal(&Value::Array(vec![id.clone()]), &id));
    assert!(!truthy(&Value::Array(vec![
        Value::Bool(false),
        Value::Bool(true)
    ])));
    assert!(!truthy(&Value::Entity(0xbfff_ffff)));
    assert!(truthy(&Value::Entity(0xa000_0101)));
    assert_eq!(to_int(&Value::Array(vec![Value::Float(-2.5)]), 0), -3);
    assert_eq!(to_int(&id, 9), 9);
    assert_eq!(round_half_away(4_294_967_296.0), 0);
}

#[test]
fn short_circuit_preserves_the_deciding_operand_and_scope_reads() {
    let variables = [variable(1, Scope::Instance), variable(2, Scope::Entity)];
    let mut context = Vars {
        instance: BTreeMap::from([(1, Value::Float(0.0))]),
        entity: BTreeMap::from([(2, Value::Integer(7))]),
        ..Vars::default()
    };
    let and = expression(json!([8, 0, 2, 4, 8, 1, 0]), &variables, &[], &[]);
    assert_eq!(evaluate(&and, &mut context).unwrap(), Value::Float(0.0));
    assert_eq!(
        context.reads,
        [Variable {
            scope: Scope::Instance,
            id: 1
        }]
    );
    context.reads.clear();
    let or = expression(json!([8, 0, 3, 4, 8, 1, 0]), &variables, &[], &[]);
    assert_eq!(evaluate(&or, &mut context).unwrap(), Value::Integer(7));
    assert_eq!(
        context.reads,
        [
            Variable {
                scope: Scope::Instance,
                id: 1
            },
            Variable {
                scope: Scope::Entity,
                id: 2
            }
        ],
    );
    let lvalue_expression = expression(json!([9, 1, 0]), &variables, &[], &[]);
    assert_eq!(
        lvalue(&lvalue_expression).unwrap(),
        Some(Variable {
            scope: Scope::Entity,
            id: 2
        })
    );
}

#[test]
fn executes_only_the_selected_path_and_bounds_nested_expressions() {
    let untaken = expression(json!([6, 0, 2, 255, 6, 1, 0]), &[], &[1.0, 7.0], &[]);
    assert_eq!(
        evaluate(&untaken, &mut EmptyContext).unwrap(),
        Value::Float(7.0)
    );
    for (opcodes, offset, reason) in [
        (json!([1, 0]), 0, "non-progressing jump"),
        (json!([1, 2]), 0, "jump leaves the stream"),
        (json!([10, 1]), 0, "truncated opcode"),
        (json!([23]), 0, "expression stack underflow"),
        (json!([1, -1]), 1, "expected a byte"),
    ] {
        let config = expression(opcodes, &[], &[], &[]);
        assert_eq!(
            evaluate(&config, &mut EmptyContext),
            Err(EvalError::MalformedBytecode { offset, reason }),
        );
    }
    let inner = expression(json!([6, 0, 0]), &[], &[3.0], &[]);
    let nested = expression(json!([14, 0, 0]), &[], &[], &[inner]);
    assert_eq!(
        evaluate_with_budget(&nested, &mut EmptyContext, 3),
        Err(EvalError::BudgetExceeded {
            offset: 2,
            limit: 3
        }),
    );
    assert_eq!(
        evaluate_with_budget(&nested, &mut EmptyContext, 4).unwrap(),
        Value::Float(3.0)
    );
}

#[test]
fn reports_unsupported_operations_instead_of_returning_a_guessed_null() {
    let map = expression(json!([19, 0, 0]), &[], &[], &[]);
    assert_eq!(
        evaluate(&map, &mut EmptyContext),
        Err(EvalError::UnsupportedOpcode {
            opcode: 19,
            offset: 0
        }),
    );
    assert_eq!(
        evaluate(&json!({"$": "STU_BE0BD267"}), &mut EmptyContext),
        Err(EvalError::UnsupportedConfig("STU_BE0BD267".into())),
    );
}

#[test]
fn retail_constant_boolean_queries_reject_unrecognized_forms() {
    for class in ["STU_4A101787", "STU_E95A9C77"] {
        assert_eq!(
            evaluate(&json!({"$":class, "m_EE729DCB":0}), &mut EmptyContext),
            Ok(Value::Bool(false))
        );
        for invalid in [
            json!({"$":class, "m_EE729DCB":1}),
            json!({"$":class, "m_entity":{"$":"STU_B5A0CAF0"}}),
        ] {
            assert_eq!(
                evaluate(&invalid, &mut EmptyContext),
                Err(EvalError::UnsupportedConfig(class.into()))
            );
        }
    }
}

#[test]
fn authored_random_expression_preserves_operand_types_and_inclusive_bounds() {
    struct Samples([u64; 3], usize);

    impl Context for Samples {
        fn random_u64(&mut self) -> Option<u64> {
            let sample = self.0.get(self.1).copied();
            self.1 += 1;
            sample
        }
    }

    let delay = expression(json!([6, 0, 6, 1, 69, 0]), &[], &[6.0, 12.0], &[]);
    let mut samples = Samples([1 << 63, 0, 2], 0);
    assert_eq!(evaluate(&delay, &mut samples), Ok(Value::Float(9.0)));
    let high = json!({"$":"STUConfigVarInt","m_value":3});
    let low = json!({"$":"STUConfigVarInt","m_value":1});
    let inclusive = expression(json!([14, 0, 14, 1, 69, 0]), &[], &[], &[high, low]);
    assert_eq!(evaluate(&inclusive, &mut samples), Ok(Value::Integer(3)));
    assert_eq!(samples.1, 3);

    let integral = expression(json!([6, 0, 6, 1, 70, 0]), &[], &[1.0, 2.0], &[]);
    assert_eq!(
        evaluate(&integral, &mut Samples([0; 3], 0)),
        Ok(Value::Integer(1))
    );
    assert_eq!(
        evaluate_with_budget(&inclusive, &mut Samples([0; 3], 0), 3),
        Err(EvalError::BudgetExceeded {
            offset: 4,
            limit: 3
        }),
    );
}

#[test]
#[ignore = "requires an authorized 104319 graph pack in ORPHEUS_RUNTIME_DIR"]
fn authored_controller_hud_and_soldier_config_values_match_python() {
    let data = orpheus_testkit::runtime_dir().join("data");
    let graphs = GraphStore::load(data.join("game_graphs_174.json.gz")).unwrap();
    let mut context = Vars {
        instance: BTreeMap::from([
            (28, Value::Integer(1)),
            (
                375,
                Value::Array(vec![Value::Bool(false), Value::Bool(true)]),
            ),
            (581, Value::Float(1.5)),
            (6884, Value::Float(rounded(0.511))),
            (6885, Value::Float(rounded(0.1))),
            (
                8449,
                Value::Array(vec![Value::Integer(1), Value::Integer(2)]),
            ),
            (11938, Value::Integer(7)),
            (17937, Value::Entity(0xa000_0100)),
            (25485, Value::Bool(true)),
        ]),
        entity: BTreeMap::from([
            (31, Value::Integer(1)),
            (9537, Value::Float(0.5)),
            (17153, Value::Bool(true)),
            (17154, Value::Asset(0x02e0_0000_0000_006e)),
            (27534, Value::Bool(true)),
        ]),
        ..Vars::default()
    };
    // Existing Python expr.evaluate against the same target graph fields
    // and scoped fixture variables. The timeout vector includes f32 bits.
    for (graph, rid, field, expected) in [
        (0x13c0, 6, "m_value", Value::Bool(false)),
        (0x13c0, 20, "m_condition", Value::Bool(true)),
        (0x13c1, 3, "m_condition", Value::Integer(7)),
        (0x0c90, 0, "m_16432044", Value::Asset(0x02e0_0000_0000_006e)),
        (0x288a, 10, "m_condition", Value::Bool(true)),
        (0x288b, 14, "m_hero", Value::Asset(0x02e0_0000_0000_0002)),
        (0x20e4, 3, "m_87276FE6", Value::Integer(23)),
        (0x0254, 8, "m_value", Value::Integer(30)),
    ] {
        let node = graphs.graph(graph).unwrap().by_rid(rid).unwrap();
        assert_eq!(
            evaluate(node.field(field).unwrap(), &mut context).unwrap(),
            expected,
            "{graph:04x} rid {rid} {field}"
        );
    }
    for (graph, state, field, expected) in [
        (
            0x0254,
            51,
            "m_timeout",
            Value::Float(f64::from(f32::from_bits(0x3f44_3958))),
        ),
        (0x0257, 12, "m_value", Value::Float(7.5)),
        (0x0258, 0, "m_05BE62DD", Value::Vector([0.0, 1.0, 0.0])),
    ] {
        let node = graphs.graph(graph).unwrap().state(state).unwrap();
        assert_eq!(
            evaluate(node.field(field).unwrap(), &mut context).unwrap(),
            expected,
            "{graph:04x} st {state} {field}"
        );
    }
}
