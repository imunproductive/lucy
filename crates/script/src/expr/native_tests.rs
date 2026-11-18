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
    team_sizes: BTreeMap<i64, i64>,
    flags: BTreeMap<u32, u64>,
    speed_scales: BTreeMap<u32, f64>,
    map_modes: Option<Vec<u64>>,
    /// The map's floor, the plane `y = a + b·x + c·z` (`[a, b, c]`); None:
    /// no map.
    floor: Option<[f64; 3]>,
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

    fn team_size(&mut self, team: i64) -> Option<i64> {
        self.team_sizes.get(&team).copied()
    }

    fn entity_filter_flags(&mut self, entity: u32) -> Option<u64> {
        self.flags.get(&entity).copied()
    }

    fn speed_scale(&mut self, entity: u32) -> Option<f64> {
        self.speed_scales.get(&entity).copied()
    }

    fn map_ray(
        &mut self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
    ) -> Option<Option<[f64; 3]>> {
        let [a, b, c] = self.floor?;
        assert_eq!(direction, [0.0, -1.0, 0.0]);
        let y = a + b * origin[0] + c * origin[2];
        Some((origin[1] >= y && origin[1] - y <= reach).then_some([origin[0], y, origin[2]]))
    }

    fn map_supports_mode(&mut self, mode: u64) -> Option<bool> {
        Some(self.map_modes.as_ref()?.contains(&mode))
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

/// `Eval_STU_309B0570`: an entity's world position (component 28); no
/// entity fails (null); with `m_5B94EDB8` it is component 33's point, which
/// only a world's own answer gives.
#[test]
fn entity_world_position_reads_the_entity_or_its_anchor() {
    let mut world = World::default();
    world.positions.insert(OTHER, [1.0, 2.0, 3.0]);
    // `Eval_STU_8CBDD81C` reads the same position, with no anchor.
    for class in ["STU_8CBDD81C", "STU_309B0570"] {
        let position = query(class, "m_entity", var(1));
        for (value, expected) in [
            (Value::Entity(OTHER), Ok(Value::Vector([1.0, 2.0, 3.0]))),
            (Value::Entity(0), Ok(Value::Null)),
            (Value::Null, Ok(Value::Null)),
            (
                Value::Entity(UNKNOWN),
                Err(EvalError::UnsupportedConfig(class.into())),
            ),
        ] {
            world.variables.insert(1, value.clone());
            assert_eq!(evaluate(&position, &mut world), expected, "{value:?}");
        }
    }
    let mut anchored = query("STU_309B0570", "m_entity", var(1));
    anchored["m_5B94EDB8"] = json!({"$":"STUConfigVarBool","m_value":true});
    world.variables.insert(1, Value::Entity(OTHER));
    assert_eq!(
        evaluate(&anchored, &mut world),
        Err(EvalError::UnsupportedConfig("STU_309B0570".into()))
    );
}

/// `driver.py` (2200A66D, 021D01CD, C8C4C20D): the aim, the direction
/// along the ground and the heading in degrees, atan2(x, z); 66F3B89E the
/// side axis of the body's world matrix (0x7FF789C2B5E0).
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
    // 66F3B89E: the side axis, (cos yaw, 0, -sin yaw), pitch ignored.
    let Value::Vector(side) = read("STU_66F3B89E", &mut world) else {
        panic!("the side axis is a vector");
    };
    assert!(side[0].abs() < 1e-6 && side[1] == 0.0 && (side[2] + 1.0).abs() < 1e-6);
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

/// `STU_0762B33A` (0x7FF789BFB820): the first direction's yaw and pitch
/// moved to within the limits (degrees) of the second's; differences wrap
/// across ±180 degrees; a zero vector gives zero.
#[test]
fn a_direction_is_kept_within_the_yaw_and_pitch_limits_of_another() {
    let mut world = World::default();
    let cone = |yaw: i64, pitch: i64| {
        json!({"$":"STU_0762B33A","m_32B00625":var(1),"m_3661BC45":var(2),
            "m_5023B9B0":int(yaw),"m_E26E5E09":int(pitch),"m_EE729DCB":0})
    };
    let mut direction = |from: [f64; 3], around: [f64; 3], yaw: i64, pitch: i64| {
        world.variables.insert(1, Value::Vector(from));
        world.variables.insert(2, Value::Vector(around));
        match evaluate(&cone(yaw, pitch), &mut world).unwrap() {
            Value::Vector(parts) => parts,
            other => panic!("a direction, not {other:?}"),
        }
    };
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5);
    let (sin30, cos30) = 30f64.to_radians().sin_cos();
    let (sin10, cos10) = 10f64.to_radians().sin_cos();
    // +x is 90 degrees right of +z: held at 30.
    let held = direction([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], 30, 90);
    assert!(close(held, [sin30, 0.0, cos30]), "{held:?}");
    // Within the cone: only normalized.
    let kept = direction([0.0, 0.0, 2.0], [0.0, 0.0, 1.0], 30, 90);
    assert!(close(kept, [0.0, 0.0, 1.0]), "{kept:?}");
    // 45 degrees down (pitch asin(−y)), held at 10.
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let down = direction([0.0, -half, half], [0.0, 0.0, 1.0], 90, 10);
    assert!(close(down, [0.0, -sin10, cos10]), "{down:?}");
    // 170 and −170 degrees are 20 apart.
    let (sin170, cos170) = 170f64.to_radians().sin_cos();
    let across = direction([sin170, 0.0, cos170], [-sin170, 0.0, cos170], 30, 0);
    assert!(close(across, [sin170, 0.0, cos170]), "{across:?}");
    assert_eq!(
        direction([0.0; 3], [0.0, 0.0, 1.0], 30, 30),
        [0.0, 0.0, 0.0]
    );
}

/// `STU_D289FA32`: the world's size of the team `m_team` names (unset:
/// -1); a team it doesn't know is refused.
#[test]
fn a_team_size_reads_the_team_its_config_names() {
    let mut world = World {
        team_sizes: BTreeMap::from([(0, 3), (1, 2), (-1, 6)]),
        ..World::default()
    };
    let team = |index: i64| json!({"$":"STU_BD6CC023","m_6CE7A900":index,"m_EE729DCB":0});
    let size = |team: Json| query("STU_D289FA32", "m_team", team);
    assert_eq!(evaluate(&size(team(0)), &mut world), Ok(Value::Integer(3)));
    assert_eq!(evaluate(&size(team(1)), &mut world), Ok(Value::Integer(2)));
    let unset = json!({"$":"STU_D289FA32","m_EE729DCB":0});
    assert_eq!(evaluate(&unset, &mut world), Ok(Value::Integer(6)));
    assert_eq!(
        evaluate(&size(team(4)), &mut world),
        Err(EvalError::UnsupportedConfig("STU_D289FA32".into()))
    );
}

/// `Eval_STU_ED807E07`: the mover's speed scale (`+0x4D8`); no entity or
/// an invalid one 0.0; an entity the world doesn't know is unsupported.
#[test]
fn the_speed_scale_reads_the_entitys_mover() {
    let mut world = World::default();
    world.speed_scales.insert(OTHER, 0.65);
    let scale = query("STU_ED807E07", "m_entity", var(1));
    for (value, expected) in [
        (Value::Entity(OTHER), Ok(Value::Float(0.65_f32.into()))),
        (Value::Entity(0), Ok(Value::Float(0.0))),
        (Value::Null, Ok(Value::Float(0.0))),
        (
            Value::Entity(UNKNOWN),
            Err(EvalError::UnsupportedConfig("STU_ED807E07".into())),
        ),
    ] {
        world.variables.insert(1, value.clone());
        assert_eq!(evaluate(&scale, &mut world), expected, "{value:?}");
    }
}

/// `Eval_STU_47703870` → 0x7FF789B2CB00: is the team hostile to the
/// script's entity. Without filter bit 28 another team, team −1 or 4 on
/// either side is; with it, another team; with it and no team, team 1.
#[test]
fn a_team_relation_tells_whether_the_team_is_hostile_to_the_owner() {
    let team = |index: i64| json!({"$":"STU_BD6CC023","m_6CE7A900":index,"m_EE729DCB":0});
    let hostile = |own: i64, flags: u64, team: Option<Json>| {
        let mut world = World::default();
        world.teams.insert(BODY, own);
        world.flags.insert(BODY, flags);
        let config = match team {
            Some(team) => query("STU_47703870", "m_team", team),
            None => json!({"$":"STU_47703870","m_EE729DCB":0}),
        };
        evaluate(&config, &mut world)
    };
    let team_0 = 1 << 23;
    let bit_28 = 1 << 28;
    for (own, flags, other, expected) in [
        (0, team_0, Some(0), false),
        (0, team_0, Some(1), true),
        (0, team_0, Some(4), true),
        (0, team_0, None, true),
        (-1, 0, Some(1), true),
        (0, team_0 | bit_28, Some(0), false),
        (0, team_0 | bit_28, Some(-1), true),
        (-1, bit_28, Some(1), true),
        (-1, bit_28, Some(0), false),
        (-1, bit_28, Some(-1), false),
    ] {
        assert_eq!(
            hostile(own, flags, other.map(team)),
            Ok(Value::Bool(expected)),
            "own {own}, flags {flags:X}, team {other:?}"
        );
    }
    let mut world = World::default();
    assert_eq!(
        evaluate(&query("STU_47703870", "m_team", team(0)), &mut world),
        Err(EvalError::UnsupportedConfig("STU_47703870".into()))
    );
}

/// `Eval_STU_16753B11`: the map supports the mode, or the mode is
/// `0230:0045`; no mode is false.
#[test]
fn a_mode_test_reads_the_maps_supported_modes() {
    let mode =
        |guid: u64| json!({"$":"STUConfigVarGameMode","m_gamemode":format!("0x{guid:016X}")});
    let supports = |guid: Option<u64>, world: &mut World| {
        let config = match guid {
            Some(guid) => query("STU_16753B11", "m_gamemode", mode(guid)),
            None => json!({"$":"STU_16753B11","m_EE729DCB":0}),
        };
        evaluate(&config, world)
    };
    let mut world = World::default();
    assert_eq!(
        supports(Some(0x0230_0000_0000_0045), &mut world),
        Ok(Value::Bool(true))
    );
    assert_eq!(supports(None, &mut world), Ok(Value::Bool(false)));
    assert_eq!(
        supports(Some(0x0230_0000_0000_0022), &mut world),
        Err(EvalError::UnsupportedConfig("STU_16753B11".into()))
    );
    world.map_modes = Some(vec![0x0230_0000_0000_0022]);
    assert_eq!(
        supports(Some(0x0230_0000_0000_0022), &mut world),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        supports(Some(0x0230_0000_0000_0023), &mut world),
        Ok(Value::Bool(false))
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

/// `STU_68F68CD7` (`01CB` rid64) has only the base config's evaluate on
/// the client (0x7FF78A14CC10, `xor al, al`): no value, whatever its entity,
/// so the kill cam test falls through to its other operands.
#[test]
fn the_death_layers_server_only_bool_reads_as_the_clients_no_value() {
    let mut world = World::default();
    for entity in [json!({"$":"STU_B5A0CAF0"}), var(1)] {
        let config = json!({"$":"STU_68F68CD7","m_entity":entity,"m_EE729DCB":0});
        assert_eq!(evaluate(&config, &mut world), Ok(Value::Null));
    }
}

/// `Eval_STU_D0E0DA18` (0x7FF789BE29B0): a direction's yaw atan2(x, z) and
/// pitch asin(−y) in degrees, the vector normalized first, (0, 0, 1) for a
/// zero vector; `Eval_STU_B113D362` (0x7FF789BDC480) makes the direction
/// of a yaw and pitch in degrees again. Ashe's coach gun (`1513` rid18):
/// her facing's yaw, 55° down.
#[test]
fn yaw_and_pitch_in_degrees_make_a_direction_and_back() {
    let vector =
        |[x, y, z]: [f64; 3]| json!({"$":"STUConfigVarVec3","m_value":{"X":x,"Y":y,"Z":z}});
    let angles = |parts| json!({"$":"STU_D0E0DA18","m_3ABD589C":vector(parts),"m_EE729DCB":0});
    let direction = |yaw: Json, pitch: Json| json!({"$":"STU_B113D362","m_7C831A8D":yaw,"m_C0EDD7EC":pitch,"m_EE729DCB":0});
    let parts = |value: Result<Value, EvalError>| match value {
        Ok(Value::Vector(parts)) => parts,
        other => panic!("{other:?}"),
    };
    let close = |value: Result<Value, EvalError>, expected: [f64; 3]| {
        let parts = parts(value);
        assert!(
            parts
                .iter()
                .zip(expected)
                .all(|(part, expected)| (part - expected).abs() < 1e-5),
            "{parts:?} != {expected:?}"
        );
    };
    let mut world = World::default();
    close(
        evaluate(&angles([2.0, 0.0, 0.0]), &mut world),
        [90.0, 0.0, 0.0],
    );
    close(
        evaluate(&angles([3.0, 0.0, 3.0]), &mut world),
        [45.0, 0.0, 0.0],
    );
    close(
        evaluate(&angles([0.0, -1.0, 0.0]), &mut world),
        [0.0, 90.0, 0.0],
    );
    close(evaluate(&angles([0.0; 3]), &mut world), [0.0; 3]);

    let (sin, cos) = 55_f64.to_radians().sin_cos();
    close(
        evaluate(&direction(float(0.0), float(55.0)), &mut world),
        [0.0, -sin, cos],
    );
    close(
        evaluate(&direction(float(-90.0), Json::Null), &mut world),
        [-1.0, 0.0, 0.0],
    );
    // Back and forth: a direction 30° right of +z and 20° up.
    let start = forward(30_f64.to_radians(), -20_f64.to_radians());
    let [yaw, pitch, _] = parts(evaluate(&angles(start), &mut world));
    close(
        evaluate(&direction(float(yaw), float(pitch)), &mut world),
        start,
    );
}

/// `Eval_STU_6EDA44A7`: points in the entity's frame (side, up, forward),
/// each swept down `m_height`; the largest angle of a hit below the
/// entity's position when more than half the points hit, else 90. With
/// Reinhardt's barrier points (`0095` rid57): level ground is 0, a floor
/// falling 1 m a metre ahead is 45, out of reach 90, and the points turn
/// with the body.
#[test]
fn the_ground_drop_sweeps_down_under_points_ahead_of_the_entity() {
    let point = |z: f64| json!({"$":"STUConfigVarVec3","m_value":{"X":0.0,"Y":2.0,"Z":z}});
    let drop = json!({"$":"STU_6EDA44A7", "m_entity": var(1),
        "m_BD631144": list(&[point(0.75), point(1.0), point(1.5)]),
        "m_height": float(3.0), "m_EE729DCB":0});
    let mut world = World::default();
    world.variables.insert(1, Value::Entity(BODY));
    world.positions.insert(BODY, [10.0, 5.0, -4.0]);
    world.facing.insert(BODY, [0, 0]);
    let angle = |world: &mut World| match evaluate(&drop, world) {
        Ok(Value::Float(angle)) => angle,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        evaluate(&drop, &mut world),
        Err(EvalError::UnsupportedConfig("STU_6EDA44A7".into()))
    );
    world.floor = Some([5.0, 0.0, 0.0]);
    assert_eq!(angle(&mut world), 0.0);
    // Falling 1 m a metre along +z, through the feet.
    world.floor = Some([1.0, 0.0, -1.0]);
    assert!(
        (angle(&mut world) - 45.0).abs() < 0.01,
        "{}",
        angle(&mut world)
    );
    // Facing +x, the same floor runs sideways: level under the points.
    world.facing.insert(BODY, [16_384, 0]);
    world.floor = Some([5.0 + 10.0, -1.0, 0.0]);
    assert!(
        (angle(&mut world) - 45.0).abs() < 0.01,
        "{}",
        angle(&mut world)
    );
    world.floor = Some([1.0, 0.0, -1.0]);
    assert!(angle(&mut world).abs() < 0.01, "{}", angle(&mut world));
    world.floor = Some([-10.0, 0.0, 0.0]);
    assert_eq!(angle(&mut world), 90.0);
}

/// The client's arcsine polynomial: π/2 at ±1 (clamped beyond), odd, and
/// near but not equal to `asin` between (0.2460 at 0.25, where `asin`
/// gives 0.2527).
#[test]
fn the_clients_arcsine_is_its_polynomial() {
    for (sine, expected) in [
        (1.0f32, std::f32::consts::FRAC_PI_2),
        (2.0, std::f32::consts::FRAC_PI_2),
        (0.5, std::f32::consts::FRAC_PI_6),
        (-0.5, -std::f32::consts::FRAC_PI_6),
        (0.25, 0.2460),
        (0.0, 0.0),
    ] {
        assert!((client_asin(sine) - expected).abs() < 1e-4, "{sine}");
    }
}
