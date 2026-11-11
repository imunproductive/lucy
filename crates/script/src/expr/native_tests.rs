//! The config lists and entity natives ported from leafall903/ORPHEUS
//! (MIT), commit c239a84: `docs/VARIABLE_ASSIGNMENTS.txt`,
//! `docs/CONFIG_ARRAY_SELECTION.txt`, `docs/ENTITY_QUERIES.txt`,
//! `docs/ENTITY_TEAM_QUERY.txt`, `docs/CONFIG_EVALUATION.txt` and the
//! native answers in `scene.py` and `driver.py`.

use super::*;
use std::collections::BTreeMap;

use serde_json::json;

const BODY: u32 = 0xa000_0101;
const OTHER: u32 = 0xa000_0102;
const UNKNOWN: u32 = 0xa000_0103;

#[derive(Default)]
struct World {
    variables: BTreeMap<u32, Value>,
    exists: Vec<u32>,
    positions: BTreeMap<u32, [f64; 3]>,
    facing: BTreeMap<u32, [i32; 2]>,
    teams: BTreeMap<u32, i64>,
    health: BTreeMap<u32, (f64, f64)>,
}

impl Context for World {
    fn variable(&mut self, variable: Variable) -> Value {
        self.variables
            .get(&variable.id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn owner(&mut self) -> Value {
        Value::Entity(BODY)
    }

    fn body_health(&mut self, entity: u32, kind: Option<PoolKind>, maximum: bool) -> Option<f64> {
        assert_eq!(kind, None);
        let (current, most) = *self.health.get(&entity)?;
        Some(if maximum { most } else { current })
    }

    fn entity_exists(&mut self, entity: u32) -> Option<bool> {
        self.exists.contains(&entity).then_some(true)
    }

    fn entity_position(&mut self, entity: u32) -> Option<[f64; 3]> {
        self.positions.get(&entity).copied()
    }

    fn entity_facing(&mut self, entity: u32) -> Option<[i32; 2]> {
        self.facing.get(&entity).copied()
    }

    fn entity_team(&mut self, entity: u32) -> Option<i64> {
        self.teams.get(&entity).copied()
    }
}

fn var(id: u32) -> Json {
    json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":0,"m_EE729DCB":0})
}

fn int(value: i64) -> Json {
    json!({"$":"STUConfigVarInt","m_value":value})
}

fn float(value: f64) -> Json {
    json!({"$":"STUConfigVarFloat","m_value":value})
}

fn list(configs: &[Json]) -> Json {
    json!({"$":"STU_06610EBC","m_configVars":configs,"m_EE729DCB":0})
}

fn query(class: &str, operand: &str, value: Json) -> Json {
    json!({"$":class, operand:value, "m_EE729DCB":0})
}

/// `VARIABLE_ASSIGNMENTS.txt`: an assignment reads a config list as one
/// array, configs and nested lists in order; a failed config or a scalar
/// null gives no row, null members of an array stay, an empty list is an
/// empty array, and a direct variable's array stays one value.
#[test]
fn an_assignment_collects_a_config_list_into_one_array() {
    let mut world = World::default();
    world.variables.insert(
        1,
        Value::Array(vec![
            Value::Integer(3),
            Value::Null,
            Value::Array(vec![Value::Integer(4)]),
        ]),
    );
    let config = list(&[
        int(1),
        Json::Null,
        list(&[float(2.5), var(2)]),
        var(1),
        json!({"$":"STUConfigVarBool","m_value":1}),
    ]);
    assert_eq!(
        evaluate_value(&config, &mut world),
        Ok(Value::Array(vec![
            Value::Integer(1),
            Value::Float(2.5),
            Value::Integer(3),
            Value::Null,
            Value::Integer(4),
            Value::Bool(true),
        ]))
    );
    assert_eq!(
        evaluate_value(&list(&[]), &mut world),
        Ok(Value::Array(Vec::new()))
    );
    assert_eq!(
        evaluate_value(&json!({"$":"STU_06610EBC","m_configVars":null}), &mut world),
        Ok(Value::Array(Vec::new()))
    );
    assert_eq!(
        evaluate_value(&var(1), &mut world),
        Ok(world.variables[&1].clone())
    );
}

/// `VARIABLE_ASSIGNMENTS.txt`: the scalar read takes only the first config
/// of a list; the empty list fails, which reads as null here.
#[test]
fn a_scalar_read_of_a_config_list_takes_its_first_config() {
    let mut world = World::default();
    world.variables.insert(1, Value::Integer(7));
    assert_eq!(
        evaluate(&list(&[var(1), int(2)]), &mut world),
        Ok(Value::Integer(7))
    );
    assert_eq!(
        evaluate(&list(&[var(9), int(2)]), &mut world),
        Ok(Value::Null)
    );
    assert_eq!(evaluate(&list(&[]), &mut world), Ok(Value::Null));
    // A condition reads the first config's truth.
    let first = json!({"$":"STU_45EDF12D","m_814F8EC3":list(&[var(1)]),"m_value":int(7)});
    assert_eq!(evaluate(&first, &mut world), Ok(Value::Bool(true)));
}

/// `CONFIG_ARRAY_SELECTION.txt`: the index is a scalar (a missing one
/// selects 0, a float rounds half away from zero), the container is read
/// as an assignment, an element keeps its type, and a scalar container or
/// an index outside the array reads null.
#[test]
fn an_array_item_selects_by_index_from_an_array_only() {
    let mut world = World::default();
    world.variables.insert(
        1,
        Value::Array(vec![
            Value::Integer(10),
            Value::Null,
            Value::Array(vec![Value::Integer(5)]),
        ]),
    );
    world.variables.insert(2, Value::Integer(10));
    let item = |index: Json, container: Json| json!({"$":"STU_4BD39CFD","m_index":index,"m_814F8EC3":container,"m_EE729DCB":0});
    for (index, container, expected) in [
        (Json::Null, var(1), Value::Integer(10)),
        (float(1.5), var(1), Value::Array(vec![Value::Integer(5)])),
        (int(1), var(1), Value::Null),
        (int(3), var(1), Value::Null),
        (int(-1), var(1), Value::Null),
        (int(0), var(2), Value::Null),
        (int(1), list(&[int(4), int(6)]), Value::Integer(6)),
        (var(9), list(&[int(4), int(6)]), Value::Integer(4)),
    ] {
        assert_eq!(
            evaluate(&item(index.clone(), container.clone()), &mut world),
            Ok(expected),
            "{index} {container}"
        );
    }
}

/// `ENTITY_QUERIES.txt`: no entity, an invalid one or another value is
/// false; an existing one true; one the world doesn't know is unsupported.
#[test]
fn entity_exists_reads_the_world_for_valid_entities_only() {
    let mut world = World {
        exists: vec![OTHER],
        ..World::default()
    };
    let exists = |entity: Value, world: &mut World| {
        world.variables.insert(1, entity);
        evaluate(&query("STU_E72F6348", "m_entity", var(1)), world)
    };
    assert_eq!(
        exists(Value::Entity(OTHER), &mut world),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        exists(Value::Array(vec![Value::Entity(OTHER)]), &mut world),
        Ok(Value::Bool(true))
    );
    for value in [
        Value::Entity(0),
        Value::Entity(0x1fff_ffff),
        Value::Null,
        Value::Integer(OTHER.into()),
    ] {
        assert_eq!(exists(value, &mut world), Ok(Value::Bool(false)));
    }
    assert_eq!(
        exists(Value::Entity(UNKNOWN), &mut world),
        Err(EvalError::UnsupportedConfig("STU_E72F6348".into()))
    );
}

/// `scene.py` (9B7648C3, 60146072, 32E6E4D8, 7D7D2470): the health total
/// as f32, the maximums; an invalid entity reads 0 for the pool sums and
/// fails (null) for the total.
#[test]
fn health_natives_read_the_body_totals() {
    let mut world = World::default();
    world.health.insert(BODY, (125.1, 200.0));
    let owner = json!({"$":"STU_B5A0CAF0"});
    let total = query("STU_9B7648C3", "m_entity", owner.clone());
    let current = json!({"$":"STU_32E6E4D8","m_entity":owner.clone()});
    let normal = json!({"$":"STU_7D7D2470","m_entity":owner.clone()});
    let most = query("STU_60146072", "m_entity", owner);
    assert_eq!(
        evaluate(&total, &mut world),
        Ok(Value::Float(f64::from(125.1_f32)))
    );
    assert_eq!(
        evaluate(&current, &mut world),
        Ok(Value::Float(f64::from(125.1_f32)))
    );
    assert_eq!(evaluate(&normal, &mut world), Ok(Value::Float(200.0)));
    assert_eq!(evaluate(&most, &mut world), Ok(Value::Float(200.0)));
    world.variables.insert(1, Value::Entity(0));
    for class in ["STU_32E6E4D8", "STU_7D7D2470", "STU_60146072"] {
        assert_eq!(
            evaluate(&query(class, "m_entity", var(1)), &mut world),
            Ok(Value::Float(0.0)),
            "{class}"
        );
    }
    assert_eq!(
        evaluate(&query("STU_9B7648C3", "m_entity", var(1)), &mut world),
        Ok(Value::Null)
    );
    world.variables.insert(1, Value::Entity(OTHER));
    assert_eq!(
        evaluate(&query("STU_9B7648C3", "m_entity", var(1)), &mut world),
        Err(EvalError::UnsupportedConfig("STU_9B7648C3".into()))
    );
}

/// `scene.py` (71A87E92): a bound entity's position; a vector that isn't
/// zero passes through; a zero vector or no entity fails (null).
#[test]
fn entity_position_reads_bound_entities_and_passes_vectors() {
    let mut world = World::default();
    world.positions.insert(OTHER, [1.0, 2.0, 3.0]);
    let position = query("STU_71A87E92", "m_entity", var(1));
    for (value, expected) in [
        (Value::Entity(OTHER), Ok(Value::Vector([1.0, 2.0, 3.0]))),
        (
            Value::Vector([0.0, 0.5, 0.0]),
            Ok(Value::Vector([0.0, 0.5, 0.0])),
        ),
        (Value::Vector([0.0; 3]), Ok(Value::Null)),
        (Value::Entity(0), Ok(Value::Null)),
        (Value::Null, Ok(Value::Null)),
        (
            Value::Entity(UNKNOWN),
            Err(EvalError::UnsupportedConfig("STU_71A87E92".into())),
        ),
    ] {
        world.variables.insert(1, value.clone());
        assert_eq!(evaluate(&position, &mut world), expected, "{value:?}");
    }
}

/// `driver.py` (2200A66D, 021D01CD, C8C4C20D): the aim, the direction
/// along the ground and the heading in degrees, atan2(x, z).
#[test]
fn facing_natives_read_the_body_yaw_and_pitch() {
    let mut world = World::default();
    // A quarter turn to +x, looking 45 degrees down.
    world.facing.insert(BODY, [16384, 8192]);
    let owner = json!({"$":"STU_B5A0CAF0"});
    let read = |class: &str, world: &mut World| {
        evaluate(&query(class, "m_entity", owner.clone()), world).unwrap()
    };
    let half = f64::from(std::f32::consts::FRAC_1_SQRT_2);
    let Value::Vector(aim) = read("STU_2200A66D", &mut world) else {
        panic!("the aim is a vector");
    };
    assert!((aim[0] - half).abs() < 1e-6 && (aim[1] + half).abs() < 1e-6 && aim[2].abs() < 1e-6);
    let Value::Vector(flat) = read("STU_021D01CD", &mut world) else {
        panic!("the ground direction is a vector");
    };
    assert!((flat[0] - 1.0).abs() < 1e-6 && flat[1] == 0.0 && flat[2].abs() < 1e-6);
    assert_eq!(read("STU_C8C4C20D", &mut world), Value::Float(90.0));
    world.facing.insert(BODY, [-32768, 0]);
    assert_eq!(read("STU_C8C4C20D", &mut world), Value::Float(-180.0));
    world.facing.insert(BODY, [0, 0]);
    assert_eq!(read("STU_C8C4C20D", &mut world), Value::Float(0.0));
    world.variables.insert(1, Value::Entity(OTHER));
    assert_eq!(
        evaluate(&query("STU_C8C4C20D", "m_entity", var(1)), &mut world),
        Err(EvalError::UnsupportedConfig("STU_C8C4C20D".into()))
    );
}

/// `ENTITY_TEAM_QUERY.txt`: bits 23 to 27 are teams 0 to 4 and the lowest
/// set one wins; the spectator bit, the player bits and the upper word
/// don't count; no team bit is -1.
#[test]
fn team_index_reads_the_lowest_team_bit_of_all_32_masks() {
    const NOISE: u64 = (1 << 28) | 0x0004_0001 | (0xffff_ffff << 32);
    for mask in 0..32_u64 {
        let expected = if mask == 0 {
            -1
        } else {
            i64::from(mask.trailing_zeros())
        };
        assert_eq!(team_from_filter(mask << 23), expected);
        assert_eq!(team_from_filter((mask << 23) | NOISE), expected);
    }
}

/// `ENTITY_TEAM_QUERY.txt`: no entity or an invalid one is -1; a valid one
/// asks the world.
#[test]
fn entity_team_asks_the_world_for_valid_entities_only() {
    let mut world = World::default();
    world.teams.insert(OTHER, 1);
    let team = query("STU_65ABFCCC", "m_target", var(1));
    for (value, expected) in [
        (Value::Entity(OTHER), Ok(Value::Integer(1))),
        (Value::Entity(0), Ok(Value::Integer(-1))),
        (Value::Null, Ok(Value::Integer(-1))),
        (
            Value::Entity(UNKNOWN),
            Err(EvalError::UnsupportedConfig("STU_65ABFCCC".into())),
        ),
    ] {
        world.variables.insert(1, value.clone());
        assert_eq!(evaluate(&team, &mut world), expected, "{value:?}");
    }
}

/// `CONFIG_EVALUATION.txt`: TeamIndex keeps its signed authored value, and
/// D3685C04 writes false with no local input record.
#[test]
fn team_literal_and_local_input_test_need_no_world() {
    let mut world = World::default();
    for (authored, expected) in [(0, 0), (1, 1), (4, 4), (-1, -1), (0xffff_ffff_i64, -1)] {
        assert_eq!(
            evaluate(
                &query("STU_BD6CC023", "m_6CE7A900", json!(authored)),
                &mut world
            ),
            Ok(Value::Integer(expected))
        );
    }
    assert_eq!(
        evaluate(&json!({"$":"STU_D3685C04","m_EE729DCB":0}), &mut world),
        Ok(Value::Bool(false))
    );
}
