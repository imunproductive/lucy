//! Typed config and expression evaluation for server-side graph initialization.
//!
//! Client research (1.74; interpreter 0x7FF78AA91210): value type numbers
//! are null 255, bool 1, int 2 (32-bit signed), float 3 (every result rounded
//! to float32), id 4 (64-bit asset), vec3 6, handle 7, entity 10, array 12,
//! map 13. An unset variable reads as null: false, 0, 0.0, and null == 0.
//! Truthiness 0x7FF789E74940 (an array is its element 0; an entity is valid
//! when not 0 and its low 29 bits are not all set); ToFloat 0x7FF789E74C80
//! and ToInt 0x7FF789E75260 (null 0, bool 0/1, a float rounds half away from
//! zero, an array is its element 0); equality 0x7FF789E738C0 (numbers within
//! 1.1920929e-5, a bool against the other side's truthiness, null against a
//! number equal when it is 0); compares use `a - eps <= b`, `a < b - eps`;
//! ADD is a vector only when both sides are vectors or entities, else float.
//! The opcode table was read in the PS4 build and checked against the PC
//! interpreter.

use std::{error::Error, fmt};

use serde_json::Value as Json;

pub use crate::wire::Value;
use crate::{
    graph::{asset_guid, target_tag_guid},
    vitals::PoolKind,
    wheel::{self, Cosmetic},
};
use orpheus_core::guid::parse_guid;

pub(crate) const EPSILON: f64 = 1.192_092_895_507_812_5e-5;
const MAX_CONFIG_DEPTH: usize = 64;
pub const MAX_EXPRESSION_STEPS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scope {
    Instance,
    Entity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Variable {
    pub scope: Scope,
    pub id: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityRelation {
    OwningController,
    CurrentPossessor,
    PossessedBody,
    AiControlEntity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackPriority {
    pub level: i8,
    pub above_equals: bool,
}

/// Priority assets documented by the 104319 reference evaluator.
pub fn stack_priority(config: &Json) -> Result<StackPriority, EvalError> {
    let asset = child(config, "m_priority");
    if asset.is_null() {
        return Ok(StackPriority {
            level: 0,
            above_equals: true,
        });
    }
    let guid = resource(asset).ok_or(EvalError::MalformedConfig("stack priority asset"))?;
    let (level, above_equals) = match guid as u32 {
        0x0000 | 0x003d => (0, true),
        0x0001 | 0x008e => (0, false),
        0x0002 => (-10, false),
        0x0003 | 0x0066 => (10, false),
        0x0015 => (20, false),
        0x003e => (-5, true),
        0x0051 => (30, false),
        0x0079 => (-5, false),
        0x008d => (2, false),
        0x008f => (1, false),
        0x0090 => (-1, false),
        0x0091 => (3, false),
        0x00a1 => (-20, false),
        0x00b5 | 0x00f9 => (5, false),
        0x00c9 => (-2, false),
        0x00dd => (-10, true),
        0x00f1 => (25, false),
        0x00f2 => (16, false),
        0x00f3 => (99, false),
        0x00f4 => (4, false),
        0x00f6 => (14, true),
        0x00f7 => (9, false),
        0x00fa => (15, false),
        0x00fe => (19, false),
        _ => {
            return Err(EvalError::UnsupportedConfig(format!(
                "stack priority {guid:016X}"
            )));
        }
    };
    Ok(StackPriority {
        level,
        above_equals,
    })
}

/// External reads may register runtime watchers. An unset variable is null.
pub trait Context {
    fn variable(&mut self, _variable: Variable) -> Value {
        Value::Null
    }

    fn parent_variable(&mut self, _levels: u8, _variable: Variable) -> Value {
        Value::Null
    }

    fn parent_entity(&mut self, _levels: u8) -> Value {
        Value::Null
    }

    fn parameter(&mut self, _id: u32) -> Value {
        Value::Null
    }

    fn owner(&mut self) -> Value {
        Value::Null
    }

    fn entity_member(&mut self, _entity: u32, _id: u32) -> Value {
        Value::Null
    }

    fn position(&mut self, _entity: u32) -> [f64; 3] {
        [0.0; 3]
    }

    fn random_u64(&mut self) -> Option<u64> {
        None
    }

    fn body_scale(&mut self, _entity: u32) -> Option<f64> {
        None
    }

    fn body_health(
        &mut self,
        _entity: u32,
        _kind: Option<PoolKind>,
        _maximum: bool,
    ) -> Option<f64> {
        None
    }

    fn related_entity(&mut self, _relation: EntityRelation, _entity: u32) -> Option<u32> {
        None
    }

    /// None leaves a tag fact unknown for this entity.
    fn has_target_tag(&mut self, _entity: u32, _tag: u64) -> Option<bool> {
        None
    }

    /// None leaves candidate flags unknown. A known absence can report zero.
    fn entity_filter_flags(&mut self, _entity: u32) -> Option<u64> {
        None
    }

    fn body_on_fire(&mut self, _entity: u32) -> Option<bool> {
        None
    }

    /// Whether an entity exists; None: the world can't tell.
    fn entity_exists(&mut self, _entity: u32) -> Option<bool> {
        None
    }

    /// Whether `controller` may play `hero` (`STU_9AE5FC4C`); None: the
    /// world can't tell.
    fn hero_available(&mut self, _controller: u32, _hero: u64) -> Option<bool> {
        None
    }

    /// Where a bound entity stands; None: the world doesn't know it.
    fn entity_position(&mut self, _entity: u32) -> Option<[f64; 3]> {
        None
    }

    /// A body's yaw and pitch in turns of 65536 (the command's units; the
    /// pitch is positive looking down); None: unknown.
    fn entity_facing(&mut self, _entity: u32) -> Option<[i32; 2]> {
        None
    }

    /// An entity's team index, 0 to 4, or -1 for none; None: unknown.
    fn entity_team(&mut self, _entity: u32) -> Option<i64> {
        None
    }

    fn ruleset(&mut self, _id: u32) -> Value {
        Value::Null
    }

    fn would_stack_to_top(
        &mut self,
        _variable: Variable,
        _priority: StackPriority,
    ) -> Option<bool> {
        None
    }

    /// None means the native is unavailable. Some(Null) is a computed result.
    fn native(&mut self, _config: &Json) -> Option<Value> {
        None
    }

    /// The asset a body has equipped in a wheel slot (zero for an empty
    /// slot); None: the world doesn't know that body's loadout.
    fn equipped_cosmetic(&mut self, _entity: u32, _kind: Cosmetic, _slot: i64) -> Option<u64> {
        None
    }

    /// Whether an unknown config class or opcode reads as null, as in the
    /// Python reference, instead of failing the expression. A context that
    /// answers true records the case.
    fn tolerate_unknown(&mut self, _what: &str) -> bool {
        false
    }
}

#[derive(Default)]
pub struct EmptyContext;

impl Context for EmptyContext {}

#[derive(Debug, Eq, PartialEq)]
pub enum EvalError {
    UnsupportedConfig(String),
    UnsupportedOpcode { opcode: u8, offset: usize },
    MalformedConfig(&'static str),
    MalformedBytecode { offset: usize, reason: &'static str },
    BudgetExceeded { offset: usize, limit: usize },
    NestingExceeded,
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedConfig(class) => write!(f, "unsupported statescript config {class}"),
            Self::UnsupportedOpcode { opcode, offset } => {
                write!(
                    f,
                    "unsupported statescript opcode {opcode} at byte {offset}"
                )
            }
            Self::MalformedConfig(reason) => write!(f, "malformed statescript config: {reason}"),
            Self::MalformedBytecode { offset, reason } => {
                write!(
                    f,
                    "malformed statescript bytecode at byte {offset}: {reason}"
                )
            }
            Self::BudgetExceeded { offset, limit } => {
                write!(
                    f,
                    "statescript instruction budget {limit} exhausted at byte {offset}"
                )
            }
            Self::NestingExceeded => f.write_str("statescript config nesting limit exceeded"),
        }
    }
}

impl Error for EvalError {}

pub fn evaluate(config: &Json, context: &mut impl Context) -> Result<Value, EvalError> {
    evaluate_with_budget(config, context, MAX_EXPRESSION_STEPS)
}

/// The instruction budget is shared by nested expressions.
pub fn evaluate_with_budget(
    config: &Json,
    context: &mut impl Context,
    max_steps: usize,
) -> Result<Value, EvalError> {
    if max_steps == 0 {
        return Err(EvalError::MalformedConfig(
            "instruction budget must be positive",
        ));
    }
    Evaluator {
        limit: max_steps,
        steps: 0,
    }
    .config(config, context, 0)
}

/// The config list, `STU_06610EBC`: an ordered group of configs.
pub(crate) const CONFIG_LIST: &str = "STU_06610EBC";

/// A config's value as an assignment reads it (the native +28 interface,
/// which SetVar and SetVars use): a config list collects the values of all
/// its configs into one array. Every other config reads as [`evaluate`]
/// does. From leafall903/ORPHEUS (MIT), commit c239a84
/// (`docs/VARIABLE_ASSIGNMENTS.txt`, `play.evaluate_value`).
pub fn evaluate_value(config: &Json, context: &mut impl Context) -> Result<Value, EvalError> {
    Evaluator {
        limit: MAX_EXPRESSION_STEPS,
        steps: 0,
    }
    .value(config, context, 0)
}

/// The configs of a config list in order, nested lists opened, missing
/// configs left out (`array_actions._items` in leafall903/ORPHEUS, commit
/// c239a84). Any other config is its own only member.
pub(crate) fn config_list_members<'a>(
    config: &'a Json,
    depth: usize,
    out: &mut Vec<&'a Json>,
) -> Result<(), EvalError> {
    if depth >= MAX_CONFIG_DEPTH {
        return Err(EvalError::NestingExceeded);
    }
    if config.is_null() {
        return Ok(());
    }
    if class(config) != Some(CONFIG_LIST) {
        out.push(config);
        return Ok(());
    }
    for member in array_field(config, "m_configVars")? {
        config_list_members(member, depth + 1, out)?;
    }
    Ok(())
}

/// The rows one config value gives a collecting reader: null gives none, an
/// array its members with nested arrays opened (a null member stays a row),
/// any other value itself (`play._iter_pairs` in leafall903/ORPHEUS, commit
/// c239a84).
pub(crate) fn value_rows(value: Value, out: &mut Vec<Value>) -> Result<(), EvalError> {
    fn rows(value: Value, depth: usize, out: &mut Vec<Value>) -> Result<(), EvalError> {
        if depth >= MAX_CONFIG_DEPTH {
            return Err(EvalError::NestingExceeded);
        }
        match value {
            Value::Array(items) => items
                .into_iter()
                .try_for_each(|item| rows(item, depth + 1, out)),
            item => {
                out.push(item);
                Ok(())
            }
        }
    }
    if value == Value::Null {
        return Ok(());
    }
    rows(value, 0, out)
}

pub fn dynamic(config: &Json) -> Result<Option<Variable>, EvalError> {
    if class(config) != Some("STUConfigVarDynamic") {
        return Ok(None);
    }
    let scope = if json_truthy(child(config, "m_60DB8F99")) {
        Scope::Entity
    } else {
        Scope::Instance
    };
    Ok(Some(Variable {
        scope,
        id: identifier(config)?,
    }))
}

/// Return a direct dynamic variable or the bytecode form of a single variable push.
pub fn lvalue(config: &Json) -> Result<Option<Variable>, EvalError> {
    if let Some(variable) = dynamic(config)? {
        return Ok(Some(variable));
    }
    if !is_expression(class(config)) {
        return Ok(None);
    }
    let parts = Expression::read(config)?;
    if parts.code.len() < 2
        || !matches!(parts.code[0].as_u64(), Some(8 | 9))
        || (parts.code.len() > 2 && parts.code[2].as_u64() != Some(0))
    {
        return Ok(None);
    }
    let index = byte(&parts.code[1], 1)?;
    let entry = table(parts.dynamics, index, 0)?;
    dynamic(entry)
}

pub fn valid_entity(id: u32) -> bool {
    id != 0 && (id & 0x1fff_ffff) != 0x1fff_ffff
}

fn first_scalar(mut value: &Value) -> Option<&Value> {
    while let Value::Array(items) = value {
        value = items.first()?;
    }
    Some(value)
}

pub fn truthy(value: &Value) -> bool {
    match first_scalar(value) {
        Some(Value::Bool(value)) => *value,
        Some(Value::Integer(value)) => *value != 0,
        Some(Value::Float(value)) => *value != 0.0 && !value.is_nan(),
        Some(Value::Asset(guid)) => *guid != 0,
        Some(Value::Entity(id)) => valid_entity(*id),
        Some(Value::Vector(parts)) => parts.iter().any(|&part| part != 0.0),
        Some(Value::Handle { instance, .. }) => *instance != 0,
        Some(Value::Map(entries) | Value::Pairs(entries)) => !entries.is_empty(),
        _ => false,
    }
}

pub fn to_float(value: &Value) -> f64 {
    match first_scalar(value) {
        Some(Value::Bool(value)) => f64::from(u8::from(*value)),
        Some(Value::Integer(value)) => rounded(*value as f64),
        Some(Value::Float(value)) => *value,
        _ => 0.0,
    }
}

pub fn to_int(value: &Value, default: i64) -> i64 {
    match first_scalar(value) {
        Some(Value::Null) => 0,
        Some(Value::Bool(value)) => i64::from(*value),
        Some(Value::Integer(value)) => *value,
        Some(Value::Float(value)) => i64::from(round_half_away(*value)),
        _ => default,
    }
}

fn rounded(value: f64) -> f64 {
    f64::from(value as f32)
}

/// A uniform sample in `[0, 1)` from the top 53 bits of a random word.
fn unit_interval(sample: u64) -> f64 {
    (sample >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
}

pub fn round_half_away(value: f64) -> i32 {
    let number = rounded(if value >= 0.0 {
        value + 0.5
    } else {
        value - 0.5
    })
    .trunc();
    if number.is_finite() {
        number.rem_euclid(4_294_967_296.0) as u32 as i32
    } else {
        0
    }
}

fn close(a: f64, b: f64) -> bool {
    rounded(a - b).abs() <= EPSILON
}

pub fn equal(a: &Value, b: &Value) -> bool {
    let (a, b) = if matches!(b, Value::Array(_)) && !matches!(a, Value::Array(_)) {
        (b, a)
    } else {
        (a, b)
    };
    match (a, b) {
        (a, Value::Bool(b)) => truthy(a) == *b,
        (Value::Bool(a), b) => *a == truthy(b),
        (Value::Null, b) => !truthy(b),
        (Value::Integer(_) | Value::Float(_), Value::Null) => !truthy(a),
        (Value::Integer(a), Value::Float(b)) | (Value::Float(b), Value::Integer(a)) => {
            close(*a as f64, *b)
        }
        (Value::Float(a), Value::Float(b)) => close(*a, *b),
        (Value::Vector(a), Value::Vector(b)) => a.iter().zip(b).all(|(&a, &b)| close(a, b)),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
        }
        (Value::Array(a), b) => matches!(a.as_slice(), [item] if equal(item, b)),
        (Value::Map(a), Value::Map(b)) | (Value::Pairs(a), Value::Pairs(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ka, va), (kb, vb))| equal(ka, kb) && equal(va, vb))
        }
        _ => a == b,
    }
}

/// Whether a container has an element equal to the value: an array's
/// items, or any other value itself (`contains_value` in
/// `ow174/game/script/expr.py`).
pub(crate) fn contains(container: &Value, value: &Value) -> bool {
    match container {
        Value::Null => false,
        Value::Array(items) => items.iter().any(|item| equal(item, value)),
        item => equal(item, value),
    }
}

/// The entity in a value: an entity, or the first member of an array
/// (F64C20, `to_entity` in `ow174/game/script/expr.py`); none otherwise.
fn to_entity(value: &Value) -> Option<u32> {
    match first_scalar(value) {
        Some(Value::Entity(id)) => Some(*id),
        _ => None,
    }
}

/// The team in an entity's filter bits (5D9570): bits 23 to 27 are teams 0
/// to 4, the lowest set bit wins; -1 when none is set. The spectator bit and
/// the other bits don't count. From leafall903/ORPHEUS (MIT), commit c239a84
/// (`entity_team.index_from_filters`, `docs/ENTITY_TEAM_QUERY.txt`).
pub fn team_from_filter(bits: u64) -> i64 {
    let teams = (bits >> 23) & 0x1f;
    if teams == 0 {
        -1
    } else {
        i64::from(teams.trailing_zeros())
    }
}

/// The unit vector a body looks along, each axis rounded to f32: forward is
/// `(sin yaw, 0, cos yaw)` and a positive pitch looks down (`combat.aim` and
/// `Match.script_body_facing` in leafall903/ORPHEUS, commit c239a84).
fn aim([yaw, pitch]: [i32; 2]) -> [f64; 3] {
    const TURN: f64 = std::f64::consts::TAU / 65536.0;
    let (yaw_sin, yaw_cos) = (f64::from(yaw) * TURN).sin_cos();
    let (pitch_sin, pitch_cos) = (f64::from(pitch) * TURN).sin_cos();
    [yaw_sin * pitch_cos, -pitch_sin, yaw_cos * pitch_cos].map(rounded)
}

/// MEMBER_BY_INDEX. Typed pairs take a number as a position and any other
/// index as a key of the same type (`expr._index` for `PairArray` in
/// leafall903/ORPHEUS (MIT)).
fn index(object: Value, index: &Value) -> Value {
    if let Value::Pairs(entries) = object {
        let numeric = matches!(
            index,
            Value::Null | Value::Bool(_) | Value::Integer(_) | Value::Float(_)
        );
        let found = if numeric {
            usize::try_from(to_int(index, 0))
                .ok()
                .and_then(|number| entries.into_iter().nth(number))
                .map(|(_, value)| value)
        } else {
            entries.into_iter().find_map(|(key, value)| {
                (std::mem::discriminant(&key) == std::mem::discriminant(index)
                    && equal(&key, index))
                .then_some(value)
            })
        };
        return found.unwrap_or(Value::Null);
    }
    if let Value::Map(entries) = object {
        return entries
            .into_iter()
            .find_map(|(key, value)| equal(&key, index).then_some(value))
            .unwrap_or(Value::Null);
    }
    if matches!(
        index,
        Value::Null | Value::Bool(_) | Value::Integer(_) | Value::Float(_)
    ) {
        let number = to_int(index, 0);
        return match object {
            Value::Array(items) => usize::try_from(number)
                .ok()
                .and_then(|number| items.into_iter().nth(number))
                .unwrap_or(Value::Null),
            object if number == 0 => object,
            _ => Value::Null,
        };
    }
    match object {
        Value::Array(items) => items
            .into_iter()
            .find(|item| equal(item, index))
            .unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn class(config: &Json) -> Option<&str> {
    config.get("$")?.as_str()
}

pub fn native_shape(config: &Json, expected: &str, operand: Option<&str>) -> bool {
    class(config) == Some(expected) && native_fields(config, operand.as_slice())
}

fn native_fields(config: &Json, operands: &[&str]) -> bool {
    config.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| match key.as_str() {
            "$" => true,
            "m_EE729DCB" => value.as_u64() == Some(0),
            key => operands.contains(&key),
        })
    })
}

fn is_expression(class: Option<&str>) -> bool {
    matches!(class, Some("STU_919FD47C" | "STUConfigVarExpression"))
}

fn child<'a>(config: &'a Json, field: &str) -> &'a Json {
    config.get(field).unwrap_or(&Json::Null)
}

/// The operand of a config that only wraps it, such as the entity of a
/// target selector: `config.field` when the config is `class`.
fn wrapped<'a>(config: &'a Json, class_name: &str, field: &str) -> Option<&'a Json> {
    (class(config) == Some(class_name)).then(|| child(config, field))
}

fn literal_asset(config: &Json, expected: &str, field: &str, kind: u64) -> Option<Value> {
    if class(config) != Some(expected)
        || !native_fields(config, &[field, "m_resourceKey"])
        || !child(config, "m_resourceKey").is_null()
    {
        return None;
    }
    let guid = asset_guid(child(config, field))?;
    (guid >> 48 == kind).then_some(Value::Asset(guid))
}

/// Authored typed pairs (`STU_7757D403`, 104319 1B78280/1B77090): each
/// row's value, then its key, which must be a resource, in authored order
/// (`play._authored_pairs` in leafall903/ORPHEUS (MIT)).
fn authored_pairs(
    evaluator: &mut Evaluator,
    config: &Json,
    context: &mut impl Context,
    depth: usize,
) -> Result<Value, EvalError> {
    let unsupported = || EvalError::UnsupportedConfig("STU_7757D403".into());
    if !native_fields(config, &["m_configVars"]) {
        return Err(unsupported());
    }
    let mut pairs = Vec::new();
    for entry in array_field(config, "m_configVars")? {
        if class(entry) != Some("STU_98DD1872") || !native_fields(entry, &["m_key", "m_value"]) {
            return Err(unsupported());
        }
        let value = evaluator.config(child(entry, "m_value"), context, depth)?;
        let key = match evaluator.config(child(entry, "m_key"), context, depth) {
            Ok(key @ Value::Asset(_)) => key,
            Ok(_) | Err(EvalError::UnsupportedConfig(_)) => return Err(unsupported()),
            Err(error) => return Err(error),
        };
        pairs.push((key, value));
    }
    Ok(Value::Pairs(pairs))
}

fn json_truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(value) => *value,
        Json::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Json::String(value) => !value.is_empty(),
        Json::Array(value) => !value.is_empty(),
        Json::Object(value) => !value.is_empty(),
    }
}

fn number(value: &Json) -> Result<f64, EvalError> {
    match value {
        Json::Null => Some(0.0),
        Json::Bool(value) => Some(f64::from(u8::from(*value))),
        Json::Number(value) => value.as_f64(),
        Json::String(value) => value.trim().parse().ok(),
        _ => None,
    }
    .ok_or(EvalError::MalformedConfig("expected a number"))
}

fn integer(value: &Json) -> Result<i64, EvalError> {
    match value {
        Json::Null => Some(0),
        Json::Bool(value) => Some(i64::from(*value)),
        Json::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().map(|value| value as i64))
            .or_else(|| value.as_f64().map(|value| value as i64)),
        Json::String(value) => value.trim().parse().ok(),
        _ => None,
    }
    .ok_or(EvalError::MalformedConfig("expected an integer"))
}

fn identifier(config: &Json) -> Result<u32, EvalError> {
    let value = child(config, "m_identifier");
    if !json_truthy(value) {
        return Ok(0);
    }
    u32::try_from(integer(value)?)
        .map_err(|_| EvalError::MalformedConfig("variable identifier out of range"))
}

fn resource(value: &Json) -> Option<u64> {
    asset_guid(value).or_else(|| {
        let text = value.as_str()?;
        text.starts_with("0x").then(|| parse_guid(text)).flatten()
    })
}

pub(crate) fn target_tag_config(config: &Json) -> Option<u64> {
    if class(config) != Some("STU_357DABBB")
        || !native_fields(config, &["m_7D7C86A1", "m_resourceKey"])
        || !child(config, "m_resourceKey").is_null()
    {
        return None;
    }
    let tag = child(config, "m_7D7C86A1");
    if tag.is_null() {
        Some(0)
    } else {
        target_tag_guid(tag)
    }
}

fn filter_exclusion(filter: &Json) -> Option<u64> {
    if class(filter) != Some("STUConfigVarFilter")
        || child(filter, "m_84065FD1").as_u64() != Some(0x3fff_ffff)
    {
        return None;
    }
    // The supported form has no selectors beyond candidate exclusions and the three tag lists.
    let supported = filter
        .as_object()?
        .iter()
        .all(|(key, value)| match key.as_str() {
            "$" | "m_84065FD1" | "m_4EA686DB" | "m_B486603E" | "m_8B2A4DF3" | "m_C1903771" => true,
            "m_EE729DCB" | "m_1E14CCA8" | "m_27CF71D5" | "m_66747E7D" | "m_89831602" => {
                value.as_u64() == Some(0)
            }
            "m_93FA1642" => value.is_null() || value.as_array().is_some_and(Vec::is_empty),
            "m_9C7BD8B8" => value.is_null() || target_tag_config(value) == Some(0),
            "m_condition" | "m_F61726FE" | "m_B40362FC" => value.is_null(),
            _ => false,
        });
    if !supported {
        return None;
    }
    filter.get("m_4EA686DB").map_or(Some(0), Json::as_u64)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagCondition {
    Any,
    All,
    Exclude,
}

enum TagCheck {
    Unused,
    Accept,
    Reject,
    Unknown,
}

fn check_tag_set(
    config: &Json,
    condition: TagCondition,
    entity: u32,
    context: &mut impl Context,
) -> Result<TagCheck, EvalError> {
    let tags = if config.is_null() {
        &[]
    } else if native_shape(config, "STU_06610EBC", Some("m_configVars")) {
        array_field(config, "m_configVars")?
    } else {
        std::slice::from_ref(config)
    };
    let mut has_tag = false;
    let mut any_match = false;
    let mut unknown = false;
    for tag in tags {
        let tag = target_tag_config(tag)
            .ok_or_else(|| EvalError::UnsupportedConfig("STU_AC829876".into()))?;
        if tag == 0 {
            continue;
        }
        has_tag = true;
        if any_match {
            // Even a matching Any list must contain only supported operands.
            continue;
        }
        match (condition, context.has_target_tag(entity, tag)) {
            (TagCondition::Any, Some(true)) => any_match = true,
            (TagCondition::All, Some(false)) | (TagCondition::Exclude, Some(true)) => {
                return Ok(TagCheck::Reject);
            }
            (_, None) => unknown = true,
            _ => (),
        }
    }
    Ok(if any_match {
        TagCheck::Accept
    } else if unknown {
        TagCheck::Unknown
    } else if !has_tag {
        TagCheck::Unused
    } else if condition == TagCondition::Any {
        TagCheck::Reject
    } else {
        TagCheck::Accept
    })
}

fn resource_field(class: &str) -> Option<&'static str> {
    match class {
        "STUConfigVarIdentifier" => Some("m_identifier"),
        "STUConfigVarGameMessage" => Some("m_gameMessage"),
        "STUConfigVarEffect" => Some("m_effect"),
        "STUConfigVarLoadout" => Some("m_loadout"),
        "STUConfigVarHardPoint" => Some("m_hardPoint"),
        "STUConfigVarGameMode" => Some("m_gamemode"),
        "STU_BCD1C634" => Some("m_id"),
        "STU_8A491388" => Some("m_priority"),
        // An unset texture is the zero asset (0686 node 28 clears 3793).
        "STUConfigVarTexture" => Some("m_texture"),
        // `0215` rid31 to rid35 store the announcer's stimulus (`078`) in
        // #10122.
        "STUConfigVarVoiceStimulus" => Some("m_voiceStimulus"),
        _ => None,
    }
}

struct Evaluator {
    limit: usize,
    steps: usize,
}

impl Evaluator {
    fn config(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        match self.known_config(config, context, depth) {
            Err(EvalError::UnsupportedConfig(class)) if context.tolerate_unknown(&class) => {
                Ok(Value::Null)
            }
            result => result,
        }
    }

    /// The +28 read of [`evaluate_value`]: a config list collects the rows
    /// of each of its configs in order.
    fn value(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        if class(config) != Some(CONFIG_LIST) {
            return self.config(config, context, depth);
        }
        let mut members = Vec::new();
        config_list_members(config, depth, &mut members)?;
        let mut rows = Vec::new();
        for member in members {
            value_rows(self.config(member, context, depth + 1)?, &mut rows)?;
        }
        Ok(Value::Array(rows))
    }

    fn known_config(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        if depth >= MAX_CONFIG_DEPTH {
            return Err(EvalError::NestingExceeded);
        }
        let fields = match config {
            Json::Null => return Ok(Value::Null),
            Json::Bool(value) => return Ok(Value::Bool(*value)),
            Json::Number(value) if value.is_i64() || value.is_u64() => {
                return Ok(Value::Integer(integer(config)?));
            }
            Json::Number(_) => return Ok(Value::Float(number(config)?)),
            Json::String(value) => return Ok(Value::String(value.clone())),
            Json::Array(_) => return Err(EvalError::UnsupportedConfig("untyped array".into())),
            Json::Object(fields) => fields,
        };
        let class = class(config).unwrap_or("?");
        let value = child(config, "m_value");
        let nested = depth + 1;
        let result = match class {
            "STUConfigVarDynamic" => context.variable(dynamic(config)?.expect("dynamic class")),
            "STUConfigVarBool" => Value::Bool(json_truthy(value)),
            "STUConfigVarInt" => Value::Integer(i64::from(integer(value)? as i32)),
            "STUConfigVarFloat" => Value::Float(rounded(number(value)?)),
            "STUConfigVarLogicalButton" => {
                Value::Integer(integer(child(config, "m_logicalButton"))?)
            }
            // The body bindings and expression constants use this explicit null operand.
            "STU_92CA44CC"
                if fields.get("m_value") == Some(&Json::Null)
                    && !json_truthy(child(config, "m_EE729DCB")) =>
            {
                Value::Null
            }
            "STU_58AA0A3B" => {
                // The authored weapon-manager defaults take the false branch on an unset value.
                // Populated input still lacks a verified predicate.
                if self.config(value, context, nested)? != Value::Null {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                }
                Value::Bool(false)
            }
            "STUConfigVarAnimCategory"
                if fields.get("m_category") == Some(&Json::Null)
                    && child(config, "m_resourceKey").is_null() =>
            {
                Value::Null
            }
            "STUConfigVarVec3" => {
                let mut parts = [0.0; 3];
                for (part, (upper, lower)) in
                    parts.iter_mut().zip([("X", "x"), ("Y", "y"), ("Z", "z")])
                {
                    let source = value
                        .get(upper)
                        .or_else(|| value.get(lower))
                        .unwrap_or(&Json::Null);
                    *part = rounded(number(source)?);
                }
                Value::Vector(parts)
            }
            "STU_919FD47C" | "STUConfigVarExpression" => {
                return self.expression(config, context, nested);
            }
            "STU_B70476F3" => literal_asset(config, class, "m_548E5344", 0x0a90)
                .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
            "STU_ABB6C23E" => {
                if !native_shape(config, class, Some("m_resourceKey"))
                    || fields.get("m_resourceKey") != Some(&Json::Null)
                {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                }
                // The native evaluator writes zero with the identifier's asset type.
                Value::Asset(0)
            }
            "STU_7757D403" => authored_pairs(self, config, context, nested)?,
            // The communication wheel (`crate::wheel`; leafall903/ORPHEUS
            // (MIT) `BodyWorld.native` in `driver.py`). An action category
            // test (STU_4460419A) and the three slot queries (44A7D0,
            // 44A980, 44A950) read only the action.
            "STU_4460419A" if native_fields(config, &["m_D929182B", "m_548E5344"]) => {
                let category = child(child(config, "m_D929182B"), "m_value");
                let action = self.config(child(config, "m_548E5344"), context, nested)?;
                Value::Bool(match (action, asset_guid(child(category, "m_id"))) {
                    (Value::Asset(action), Some(id)) => wheel::has_category(
                        action,
                        id,
                        asset_guid(child(category, "m_type")).unwrap_or(0),
                    ),
                    _ => false,
                })
            }
            "STU_3E7F59D8" | "STU_955C40AE" | "STU_2E708EC1"
                if native_fields(config, &["m_548E5344"]) =>
            {
                let table = match class {
                    "STU_3E7F59D8" => &wheel::EMOTE_ACTIONS,
                    "STU_955C40AE" => &wheel::VOICE_ACTIONS,
                    _ => &wheel::SPRAY_ACTIONS,
                };
                // 069A gets its action in a one-item PlayScript array.
                let action = match self.config(child(config, "m_548E5344"), context, nested)? {
                    Value::Array(items) if items.len() == 1 => items.into_iter().next(),
                    action => Some(action),
                };
                Value::Integer(match action {
                    Some(Value::Asset(action)) => wheel::slot(action, table),
                    _ => -1,
                })
            }
            "STU_1ECB66A0" if native_fields(config, &["m_548E5344", "m_resourceKey"]) => {
                match self.config(child(config, "m_548E5344"), context, nested)? {
                    Value::Asset(action) => Value::Asset(wheel::callout_texture(action)),
                    _ => Value::Asset(0),
                }
            }
            // The equipped cosmetic in a slot of the named body: CD7490
            // (emote alias; its target is STU_33980609), CE18E0 (emote blend
            // tree), 93D1F0 (voice effect; the target is the body's
            // component 43 owner, STU_F7262B36) and 92E7E0 (spray unlock).
            "STU_250DEABD" | "STU_DF7C7C63" | "STU_2977339D" | "STU_17B517B9" => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let (kind, entity) = match class {
                    "STU_250DEABD" => (
                        Cosmetic::EmoteAlias,
                        wrapped(child(config, "m_target"), "STU_33980609", "m_entity"),
                    ),
                    "STU_DF7C7C63" => (Cosmetic::EmoteBlendTree, Some(child(config, "m_entity"))),
                    "STU_2977339D" => (
                        Cosmetic::VoiceEffect,
                        wrapped(child(config, "m_target"), "STU_F7262B36", "m_801B298F"),
                    ),
                    _ => (Cosmetic::Spray, Some(child(config, "m_entity"))),
                };
                if !child(config, "m_resourceKey").is_null() {
                    return Err(unsupported());
                }
                let Value::Entity(entity) =
                    self.config(entity.ok_or_else(unsupported)?, context, nested)?
                else {
                    return Err(unsupported());
                };
                let slot = to_int(&self.config(child(config, "m_slot"), context, nested)?, -1);
                Value::Asset(
                    context
                        .equipped_cosmetic(entity, kind, slot)
                        .ok_or_else(unsupported)?,
                )
            }
            "STU_B5A0CAF0" => context.owner(),
            "STU_25ECB55F" => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                if !native_fields(config, &["m_entity", "m_88B0050A"]) {
                    return Err(unsupported());
                }
                let tag = target_tag_guid(child(config, "m_88B0050A")).ok_or_else(unsupported)?;
                match self.config(child(config, "m_entity"), context, nested)? {
                    Value::Entity(entity) if valid_entity(entity) => Value::Bool(
                        context
                            .has_target_tag(entity, tag)
                            .ok_or_else(unsupported)?,
                    ),
                    Value::Entity(_) | Value::Null => Value::Bool(false),
                    _ => return Err(unsupported()),
                }
            }
            "STU_AC829876" => {
                return match self.entity_filter(config, context, nested) {
                    Err(error @ EvalError::UnsupportedConfig(_)) => match context.native(config) {
                        Some(value @ Value::Bool(_)) => Ok(value),
                        _ => Err(error),
                    },
                    result => result,
                };
            }
            "STU_1ED4F039" if native_shape(config, class, Some("m_entity")) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let Value::Entity(entity) =
                    self.config(child(config, "m_entity"), context, nested)?
                else {
                    return Err(unsupported());
                };
                Value::Bool(context.body_on_fire(entity).ok_or_else(unsupported)?)
            }
            "STU_4A101787" | "STU_E95A9C77" if native_shape(config, class, None) => {
                // The retail classes share a native evaluator that writes a typed false.
                Value::Bool(false)
            }
            // CE5450 tests bit 15 of the local input record and writes false
            // when there is none, as on a server (`CONFIG_EVALUATION.txt` in
            // leafall903/ORPHEUS, commit c239a84).
            "STU_D3685C04" if native_shape(config, class, None) => Value::Bool(false),
            // TeamIndex (1B860F0): the authored signed 32-bit team, -1 kept; a
            // literal, not the team of an entity (leafall903/ORPHEUS, commit
            // c239a84, `CONFIG_EVALUATION.txt`).
            "STU_BD6CC023" if native_shape(config, class, Some("m_6CE7A900")) => {
                Value::Integer(i64::from(integer(child(config, "m_6CE7A900"))? as i32))
            }
            // The scalar read (+20, 1B77980) of a config list takes only its
            // first config, failure included (`play._scalar` in
            // leafall903/ORPHEUS, commit c239a84). An empty list fails
            // without writing; this evaluator has no failure value, so it
            // reads as the caller's unset destination, null.
            CONFIG_LIST => match array_field(config, "m_configVars")?.first() {
                Some(first) => return self.config(first, context, nested),
                None => Value::Null,
            },
            // The item of an array at an index (1B79A60): the index reads as a
            // scalar (failure 0, a float rounded half away from zero), the
            // container as an assignment does; an index outside the array or
            // a container that isn't an array reads null
            // (`play.array_item` in leafall903/ORPHEUS, commit c239a84).
            "STU_4BD39CFD" if native_fields(config, &["m_index", "m_814F8EC3"]) => {
                let index = to_int(&self.config(child(config, "m_index"), context, nested)?, 0);
                match self.value(child(config, "m_814F8EC3"), context, nested)? {
                    Value::Array(items) => usize::try_from(index)
                        .ok()
                        .and_then(|index| items.into_iter().nth(index))
                        .unwrap_or(Value::Null),
                    _ => Value::Null,
                }
            }
            // Does an entity exist (CE4A60): false for no entity or an
            // invalid one (`entity_queries.evaluate` in leafall903/ORPHEUS,
            // commit c239a84, `ENTITY_QUERIES.txt`).
            "STU_E72F6348" if native_shape(config, class, Some("m_entity")) => {
                match to_entity(&self.config(child(config, "m_entity"), context, nested)?) {
                    Some(entity) if valid_entity(entity) => Value::Bool(
                        context
                            .entity_exists(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    _ => Value::Bool(false),
                }
            }
            // May a controller play a hero (`0C90` rid3/rid43, `288B` rid28):
            // the world's answer for a bound controller and a hero asset; no
            // hero is never available. Inferred from its uses, not read in
            // the client.
            "STU_9AE5FC4C" if native_fields(config, &["m_controller", "m_hero"]) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let controller =
                    to_entity(&self.config(child(config, "m_controller"), context, nested)?)
                        .filter(|&entity| valid_entity(entity))
                        .ok_or_else(unsupported)?;
                match self.config(child(config, "m_hero"), context, nested)? {
                    Value::Asset(hero) if hero != 0 => Value::Bool(
                        context
                            .hero_available(controller, hero)
                            .ok_or_else(unsupported)?,
                    ),
                    _ => Value::Bool(false),
                }
            }
            // An entity's health total as f32 (CEB6C0; `SceneWorld.native`
            // in leafall903/ORPHEUS, commit c239a84). No entity fails: null
            // here, as for the empty config list.
            "STU_9B7648C3" if native_shape(config, class, Some("m_entity")) => {
                match to_entity(&self.config(child(config, "m_entity"), context, nested)?) {
                    Some(entity) if valid_entity(entity) => Value::Float(rounded(
                        context
                            .body_health(entity, None, false)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    )),
                    _ => Value::Null,
                }
            }
            // Where an entity stands (CCC750): a bound entity's position; a
            // vector that isn't zero passes through; anything else fails,
            // null here (leafall903/ORPHEUS, commit c239a84). A world's own
            // answer comes first.
            "STU_71A87E92" if native_shape(config, class, Some("m_entity")) => {
                if let Some(value) = context.native(config) {
                    return Ok(value);
                }
                let value = self.config(child(config, "m_entity"), context, nested)?;
                match (to_entity(&value), value) {
                    (Some(entity), _) if valid_entity(entity) => Value::Vector(
                        context
                            .entity_position(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    (None, Value::Vector(position)) if position != [0.0; 3] => {
                        Value::Vector(position)
                    }
                    _ => Value::Null,
                }
            }
            // Where a body looks: 2200A66D along its aim, 021D01CD along the
            // ground, C8C4C20D its heading in degrees, atan2(x, z) of the
            // ground direction (`BodyWorld.native` in leafall903/ORPHEUS,
            // commit c239a84). A world's own answer comes first.
            "STU_2200A66D" | "STU_021D01CD" | "STU_C8C4C20D"
                if native_shape(config, class, Some("m_entity")) =>
            {
                if let Some(value) = context.native(config) {
                    return Ok(value);
                }
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let entity = to_entity(&self.config(child(config, "m_entity"), context, nested)?)
                    .filter(|&entity| valid_entity(entity))
                    .ok_or_else(unsupported)?;
                let [yaw, pitch] = context.entity_facing(entity).ok_or_else(unsupported)?;
                let pitch = if class == "STU_2200A66D" { pitch } else { 0 };
                let facing = aim([yaw, pitch]);
                if class == "STU_C8C4C20D" {
                    const DEGREES: f64 = 57.295_780_181_884_766;
                    let [x, _, z] = facing;
                    Value::Float(rounded(rounded(x.atan2(z)) * DEGREES))
                } else {
                    Value::Vector(facing)
                }
            }
            // An entity's team (CEEDC0): -1 for no entity or an invalid one
            // (`entity_team.evaluate` in leafall903/ORPHEUS, commit c239a84,
            // `ENTITY_TEAM_QUERY.txt`).
            "STU_65ABFCCC" if native_shape(config, class, Some("m_target")) => {
                match to_entity(&self.config(child(config, "m_target"), context, nested)?) {
                    Some(entity) if valid_entity(entity) => Value::Integer(
                        context
                            .entity_team(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    _ => Value::Integer(-1),
                }
            }
            "STU_F76C486C" | "STU_F7262B36" | "STU_0AC15F78" | "STU_DB742151" => {
                let (relation, operand) = match class {
                    "STU_F76C486C" => (EntityRelation::OwningController, "m_D63CBEC2"),
                    "STU_F7262B36" => (EntityRelation::CurrentPossessor, "m_801B298F"),
                    "STU_DB742151" => (EntityRelation::AiControlEntity, "m_entity"),
                    _ => (EntityRelation::PossessedBody, "m_entity"),
                };
                if !native_shape(config, class, Some(operand)) {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                }
                let Value::Entity(entity) = self.config(child(config, operand), context, nested)?
                else {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                };
                let related = context
                    .related_entity(relation, entity)
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                Value::Entity(related)
            }
            "STU_D3EB8D87" if native_shape(config, class, Some("m_00903303")) => {
                if let Some(value) = context.native(config) {
                    value
                } else {
                    match self.config(child(config, "m_00903303"), context, nested)? {
                        Value::Entity(entity) if !valid_entity(entity) => Value::Bool(false),
                        _ => return Err(EvalError::UnsupportedConfig(class.into())),
                    }
                }
            }
            // The normal pools' current (32E6E4D8) and maximum (7D7D2470)
            // health, and the sum of the maximums of all active pools
            // (60146072, D17AE0). No entity or an invalid one reads 0
            // (`SceneWorld.native` in leafall903/ORPHEUS, commit c239a84).
            "STU_32E6E4D8" | "STU_7D7D2470" | "STU_60146072" => {
                let target = child(config, "m_entity");
                match to_entity(&self.config(target, context, nested)?) {
                    Some(entity) if valid_entity(entity) => {
                        let health = context
                            .body_health(entity, None, class != "STU_32E6E4D8")
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                        Value::Float(rounded(health))
                    }
                    _ => Value::Float(0.0),
                }
            }
            "STU_783B4CF0" | "STU_68883EAC" => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                if !native_fields(config, &["m_entity", "m_7E8B666C", "m_7BE02F89"])
                    || !child(config, "m_7BE02F89").is_null()
                {
                    return Err(unsupported());
                }
                let selector = child(config, "m_7E8B666C");
                let kind = if selector.is_null() {
                    PoolKind::Health
                } else {
                    if !native_shape(selector, "STU_297E1A30", Some("m_7E8B666C")) {
                        return Err(unsupported());
                    }
                    match child(selector, "m_7E8B666C").as_i64() {
                        Some(0) => PoolKind::Health,
                        Some(1) => PoolKind::Armour,
                        Some(2) => PoolKind::Shields,
                        _ => return Err(unsupported()),
                    }
                };
                let Value::Entity(entity) =
                    self.config(child(config, "m_entity"), context, nested)?
                else {
                    return Err(unsupported());
                };
                let health = context
                    .body_health(entity, Some(kind), class == "STU_68883EAC")
                    .ok_or_else(unsupported)?;
                Value::Float(rounded(health))
            }
            "STU_FDBB0B67" => {
                let target = child(config, "m_entity");
                if !matches!(
                    self::class(target),
                    Some("STU_B5A0CAF0" | "STUConfigVarDynamic")
                ) {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                }
                let Value::Entity(entity) = self.config(target, context, nested)? else {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                };
                if entity == 0 || entity & 0x1fff_ffff == 0x1fff_ffff {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                }
                let scale = context
                    .body_scale(entity)
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                Value::Float(scale)
            }
            "STU_3832D36C" => {
                let key = match dynamic(value)? {
                    Some(variable) => Some(variable.id),
                    None => u32::try_from(to_int(&self.config(value, context, nested)?, 0)).ok(),
                };
                key.map_or(Value::Null, |key| context.ruleset(key))
            }
            "STU_91EFD5B1" => {
                let value = self.config(value, context, nested)?;
                if value == Value::Null {
                    self.config(child(config, "m_default"), context, nested)?
                } else {
                    value
                }
            }
            "STU_3ACE35FB" => {
                let Some(variable) = dynamic(child(config, "m_E50CF556"))? else {
                    return Ok(Value::Bool(false));
                };
                let priority = stack_priority(child(config, "m_012B7AF7"))?;
                let top = context
                    .would_stack_to_top(variable, priority)
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                Value::Bool(top)
            }
            "STU_45EDF12D" if child(config, "m_key").is_null() => {
                let container = self.config(child(config, "m_814F8EC3"), context, nested)?;
                let value = self.config(value, context, nested)?;
                Value::Bool(contains(&container, &value))
            }
            "STU_BE0BD267" => {
                // A random float between the asset's bounds (0B92's waits, 0C17's
                // chance gate), from the component's random stream.
                let bound = |name| match child(config, name) {
                    Json::Bool(value) => f64::from(u8::from(*value)),
                    Json::Number(value) => rounded(value.as_f64().unwrap_or_default()),
                    _ => 0.0,
                };
                let (low, high) = (bound("m_min"), bound("m_max"));
                let sample = context
                    .random_u64()
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                Value::Float(rounded(low + (high - low) * unit_interval(sample)))
            }
            "STU_7197C080" => {
                let base = to_float(&self.config(child(config, "m_01AFADB6"), context, nested)?);
                let least = to_float(&self.config(child(config, "m_EA70ACC2"), context, nested)?);
                let scalar = child(config, "m_69A20070");
                let scalar = if scalar.is_null() {
                    1.0
                } else {
                    to_float(&self.config(scalar, context, nested)?)
                };
                let product = rounded(base * scalar);
                Value::Float(if product > least { product } else { least })
            }
            _ => {
                if let Some(field) = resource_field(class) {
                    return Ok(Value::Asset(resource(child(config, field)).unwrap_or(0)));
                }
                if fields.contains_key("m_resourceKey") {
                    if let Some(guid) = fields
                        .iter()
                        .filter(|(key, _)| key.as_str() != "m_resourceKey")
                        .filter_map(|(_, value)| resource(value))
                        .find(|&guid| guid != 0)
                    {
                        return Ok(Value::Asset(guid));
                    }
                }
                if let Some(value) = context.native(config) {
                    return Ok(value);
                }
                match class {
                    "STU_CC623180" => Value::Bool(true),  // IsHUDEnabled
                    "STU_98E9109B" => Value::Bool(false), // CustomGameLogicEnabled
                    "STU_5E75FF05" => Value::Integer(0),  // ReticleType
                    "STU_C76C63AF" => Value::Float(0.0),  // ReticleNumericSetting
                    _ => return Err(EvalError::UnsupportedConfig(class.into())),
                }
            }
        };
        Ok(result)
    }

    fn entity_filter(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        let unsupported = || EvalError::UnsupportedConfig("STU_AC829876".into());
        if !native_fields(config, &["m_entity", "m_filter"]) {
            return Err(unsupported());
        }
        let filter = child(config, "m_filter");
        let exclude = filter_exclusion(filter).ok_or_else(unsupported)?;
        let Value::Entity(entity) = self.config(child(config, "m_entity"), context, depth)? else {
            return Err(unsupported());
        };
        if !valid_entity(entity) {
            return Ok(Value::Bool(false));
        }
        let flags = if exclude == 0 {
            None
        } else {
            context.entity_filter_flags(entity)
        };
        if flags.is_some_and(|flags| flags & exclude != 0) {
            return Ok(Value::Bool(false));
        }
        let mut unknown = exclude != 0 && flags.is_none();
        let mut has_fact = flags.is_some();
        for (field, condition) in [
            ("m_B486603E", TagCondition::Any),
            ("m_8B2A4DF3", TagCondition::All),
            ("m_C1903771", TagCondition::Exclude),
        ] {
            match check_tag_set(child(filter, field), condition, entity, context)? {
                TagCheck::Unused => (),
                TagCheck::Accept => has_fact = true,
                TagCheck::Reject => return Ok(Value::Bool(false)),
                TagCheck::Unknown => unknown = true,
            }
        }
        if unknown || !has_fact && context.entity_filter_flags(entity).is_none() {
            return Err(unsupported());
        }
        Ok(Value::Bool(true))
    }

    fn expression(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        match self.known_expression(config, context, depth) {
            Err(EvalError::UnsupportedOpcode { opcode, .. })
                if context.tolerate_unknown(&format!("expression opcode {opcode}")) =>
            {
                Ok(Value::Null)
            }
            result => result,
        }
    }

    fn known_expression(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        let Expression {
            code,
            dynamics,
            floats,
            configs,
        } = Expression::read(config)?;
        let mut pc = 0;
        let mut stack = Vec::new();
        while pc < code.len() {
            if self.steps >= self.limit {
                return Err(EvalError::BudgetExceeded {
                    offset: pc,
                    limit: self.limit,
                });
            }
            self.steps += 1;
            let opcode = byte(&code[pc], pc)?;
            let width = operand_count(opcode);
            let next = pc + 1 + width;
            let operands = code
                .get(pc + 1..next)
                .ok_or_else(|| malformed(pc, "truncated opcode"))?;
            let mut args = [0; 2];
            for (index, (arg, value)) in args.iter_mut().zip(operands).enumerate() {
                *arg = byte(value, pc + 1 + index)?;
            }
            let arg = args[0];
            let value = match opcode {
                0 => break,
                1 => {
                    pc = jump(pc, arg, code.len())?;
                    continue;
                }
                2 | 3 => {
                    let top = stack
                        .last()
                        .ok_or_else(|| malformed(pc, "expression stack underflow"))?;
                    if truthy(top) != (opcode == 2) {
                        pc = jump(pc, arg, code.len())?;
                    } else {
                        stack.pop();
                        pc = next;
                    }
                    continue;
                }
                4 | 5 => {
                    let flag = truthy(&pop(&mut stack, pc)?);
                    pc = if flag == (opcode == 5) {
                        jump(pc, arg, code.len())?
                    } else {
                        next
                    };
                    continue;
                }
                6 => {
                    let value = table(floats, arg, pc)?;
                    if value.is_null() {
                        return Err(malformed(pc, "expected a float constant"));
                    }
                    Value::Float(rounded(number(value)?))
                }
                7 => Value::Integer(i64::from(identifier(table(dynamics, arg, pc)?)?)),
                8 | 9 => context.variable(stack_var(dynamics, arg, pc)?),
                10 => context.parent_variable(arg, stack_var(dynamics, args[1], pc)?),
                11 | 12 => {
                    let scope = if opcode == 11 {
                        Scope::Instance
                    } else {
                        Scope::Entity
                    };
                    lookup(&pop(&mut stack, pc)?, scope).map_or(Value::Null, |variable| {
                        context.parent_variable(arg, variable)
                    })
                }
                13 => context.parameter(identifier(table(dynamics, arg, pc)?)?),
                14 => {
                    let value = configs.get(usize::from(arg)).unwrap_or(&Json::Null);
                    self.config(value, context, depth)?
                }
                15 | 16 => {
                    let (object, key) = pair(&mut stack, pc)?;
                    entity_member(&object, &key, context)
                }
                17 => {
                    let (object, key) = pair(&mut stack, pc)?;
                    index(object, &key)
                }
                18 => {
                    let length = match pop(&mut stack, pc)? {
                        Value::Array(items) => items.len() as i64,
                        _ => 0,
                    };
                    Value::Integer(length)
                }
                19 => {
                    // A map from key and value pairs; keys are integers.
                    let start = stack
                        .len()
                        .checked_sub(2 * usize::from(arg))
                        .ok_or_else(|| malformed(pc, "expression stack underflow"))?;
                    let mut items = stack.split_off(start).into_iter();
                    let mut entries = Vec::with_capacity(usize::from(arg));
                    while let (Some(key), Some(value)) = (items.next(), items.next()) {
                        entries.push((Value::Integer(to_int(&key, 0)), value));
                    }
                    Value::Map(entries)
                }
                20 => {
                    let start = stack
                        .len()
                        .checked_sub(usize::from(arg))
                        .ok_or_else(|| malformed(pc, "expression stack underflow"))?;
                    Value::Array(stack.split_off(start))
                }
                21 => context.parent_entity(arg),
                22 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    let mut items = into_array(a);
                    items.extend(into_array(b));
                    Value::Array(items)
                }
                23..=28 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    arithmetic(opcode, &a, &b, context)
                }
                29 => match pop(&mut stack, pc)? {
                    Value::Vector(parts) => Value::Vector(parts.map(|part| -part)),
                    value => Value::Float(rounded(-to_float(&value))),
                },
                30 => Value::Bool(!truthy(&pop(&mut stack, pc)?)),
                31 | 32 => {
                    let scope = if opcode == 31 {
                        Scope::Instance
                    } else {
                        Scope::Entity
                    };
                    lookup(&pop(&mut stack, pc)?, scope)
                        .map_or(Value::Null, |variable| context.variable(variable))
                }
                33..=36 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    let (a, b) = (to_float(&a), to_float(&b));
                    Value::Bool(match opcode {
                        33 => a - EPSILON <= b,
                        34 => a < b - EPSILON,
                        35 => a + EPSILON >= b,
                        36 => a > b + EPSILON,
                        _ => unreachable!(),
                    })
                }
                37 | 38 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    Value::Bool(equal(&a, &b) == (opcode == 37))
                }
                39 => {
                    let value = to_float(&pop(&mut stack, pc)?);
                    Value::Float(if value > 0.0 {
                        rounded(value.sqrt())
                    } else {
                        0.0
                    })
                }
                40..=44 | 46..=48 => {
                    let value = to_float(&pop(&mut stack, pc)?);
                    Value::Float(rounded(match opcode {
                        40 => value.to_radians(),
                        41 => value.to_degrees(),
                        42 => value.sin(),
                        43 => value.cos(),
                        44 => value.tan(),
                        46 => value.to_radians().sin(),
                        47 => value.to_radians().cos(),
                        _ => value.to_radians().tan(),
                    }))
                }
                45 | 49 => {
                    let (y, x) = pair(&mut stack, pc)?;
                    let angle = to_float(&y).atan2(to_float(&x));
                    Value::Float(rounded(if opcode == 49 {
                        angle.to_degrees()
                    } else {
                        angle
                    }))
                }
                50 | 51 => {
                    let value = pop(&mut stack, pc)?;
                    Value::Float(
                        vector_position(&value, context)
                            .map_or_else(|| rounded(to_float(&value).abs()), magnitude),
                    )
                }
                52 => {
                    let [x, _, z] = vector(&pop(&mut stack, pc)?);
                    Value::Float(rounded(x.hypot(z)))
                }
                53 | 54 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    let a = vector_position(&a, context).unwrap_or([0.0; 3]);
                    let b = vector_position(&b, context).unwrap_or([0.0; 3]);
                    let delta = vector_op(a, b, |a, b| a - b);
                    Value::Float(if opcode == 53 {
                        magnitude(delta)
                    } else {
                        rounded(delta[0].hypot(delta[2]))
                    })
                }
                55 | 56 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    let [ax, ay, az] = vector(&a);
                    let [bx, by, bz] = vector(&b);
                    if opcode == 55 {
                        Value::Float(rounded(ax * bx + ay * by + az * bz))
                    } else {
                        Value::Vector([
                            rounded(ay * bz - az * by),
                            rounded(az * bx - ax * bz),
                            rounded(ax * by - ay * bx),
                        ])
                    }
                }
                57 => {
                    let parts = vector(&pop(&mut stack, pc)?);
                    let length = magnitude(parts);
                    Value::Vector(if length == 0.0 {
                        [0.0; 3]
                    } else {
                        parts.map(|part| rounded(part / length))
                    })
                }
                58 => {
                    let z = to_float(&pop(&mut stack, pc)?);
                    let (x, y) = pair(&mut stack, pc)?;
                    Value::Vector([to_float(&x), to_float(&y), z])
                }
                60..=62 => Value::Float(vector(&pop(&mut stack, pc)?)[usize::from(opcode - 60)]),
                63 | 64 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    let (a, b) = (to_float(&a), to_float(&b));
                    Value::Float(if opcode == 63 {
                        minimum(a, b)
                    } else {
                        maximum(a, b)
                    })
                }
                65 => {
                    let hi = to_float(&pop(&mut stack, pc)?);
                    let (value, lo) = pair(&mut stack, pc)?;
                    let (value, lo) = (to_float(&value), to_float(&lo));
                    Value::Float(maximum(minimum(value, maximum(lo, hi)), minimum(lo, hi)))
                }
                66 => Value::Float(rounded(to_float(&pop(&mut stack, pc)?).floor())),
                67 => Value::Float(rounded(to_float(&pop(&mut stack, pc)?).ceil())),
                68 => Value::Integer(i64::from(round_half_away(to_float(&pop(&mut stack, pc)?)))),
                69 | 70 => {
                    let (a, b) = pair(&mut stack, pc)?;
                    let mut sample = context
                        .random_u64()
                        .ok_or(EvalError::UnsupportedOpcode { opcode, offset: pc })?;
                    if opcode == 70
                        || matches!(
                            (&a, &b),
                            (Value::Integer(_), Value::Integer(_) | Value::Bool(_))
                        )
                    {
                        let (a, b) = (to_int(&a, 0), to_int(&b, 0));
                        let low = i128::from(a.min(b));
                        let count = (i128::from(a.max(b)) - low + 1) as u128;
                        // Discard the incomplete residue range to keep inclusive integer rolls uniform.
                        let reject_below = (1_u128 << 64) % count;
                        while u128::from(sample) < reject_below {
                            if self.steps >= self.limit {
                                return Err(EvalError::BudgetExceeded {
                                    offset: pc,
                                    limit: self.limit,
                                });
                            }
                            self.steps += 1;
                            sample = context
                                .random_u64()
                                .ok_or(EvalError::UnsupportedOpcode { opcode, offset: pc })?;
                        }
                        Value::Integer((low + (u128::from(sample) % count) as i128) as i64)
                    } else {
                        let (a, b) = (to_float(&a), to_float(&b));
                        Value::Float(rounded(a + (b - a) * unit_interval(sample)))
                    }
                }
                71 => {
                    let t = to_float(&pop(&mut stack, pc)?);
                    let (a, b) = pair(&mut stack, pc)?;
                    let (a, b) = (to_float(&a), to_float(&b));
                    Value::Float(rounded(a + (b - a) * t))
                }
                _ => return Err(EvalError::UnsupportedOpcode { opcode, offset: pc }),
            };
            stack.push(value);
            pc = next;
        }
        Ok(stack.pop().unwrap_or(Value::Null))
    }
}

struct Expression<'a> {
    code: &'a [Json],
    dynamics: &'a [Json],
    floats: &'a [Json],
    configs: &'a [Json],
}

impl<'a> Expression<'a> {
    fn read(config: &'a Json) -> Result<Self, EvalError> {
        let data = child(config, "m_expression");
        Ok(Self {
            code: array_field(data, "m_opcodes")?,
            dynamics: array_field(data, "m_dynamicVars")?,
            floats: array_field(data, "m_D99EF254")?,
            configs: array_field(config, "m_configVars")?,
        })
    }
}

fn array_field<'a>(config: &'a Json, key: &str) -> Result<&'a [Json], EvalError> {
    match child(config, key) {
        Json::Null => Ok(&[]),
        Json::Array(values) => Ok(values),
        _ => Err(EvalError::MalformedConfig("expected an expression table")),
    }
}

fn malformed(offset: usize, reason: &'static str) -> EvalError {
    EvalError::MalformedBytecode { offset, reason }
}

fn byte(value: &Json, offset: usize) -> Result<u8, EvalError> {
    value
        .as_u64()
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| malformed(offset, "expected a byte"))
}

fn operand_count(opcode: u8) -> usize {
    match opcode {
        10 => 2,
        1..=9 | 11..=14 | 19..=21 => 1,
        _ => 0,
    }
}

fn table(values: &[Json], index: u8, offset: usize) -> Result<&Json, EvalError> {
    values
        .get(usize::from(index))
        .ok_or_else(|| malformed(offset, "expression table index out of range"))
}

fn stack_var(values: &[Json], index: u8, offset: usize) -> Result<Variable, EvalError> {
    dynamic(table(values, index, offset)?)?
        .ok_or_else(|| malformed(offset, "expected a dynamic variable"))
}

fn jump(pc: usize, offset: u8, size: usize) -> Result<usize, EvalError> {
    if offset == 0 {
        return Err(malformed(pc, "non-progressing jump"));
    }
    let target = pc + usize::from(offset);
    if target >= size {
        return Err(malformed(pc, "jump leaves the stream"));
    }
    Ok(target)
}

fn pop(stack: &mut Vec<Value>, pc: usize) -> Result<Value, EvalError> {
    stack
        .pop()
        .ok_or_else(|| malformed(pc, "expression stack underflow"))
}

fn pair(stack: &mut Vec<Value>, pc: usize) -> Result<(Value, Value), EvalError> {
    let b = pop(stack, pc)?;
    Ok((pop(stack, pc)?, b))
}

fn lookup(key: &Value, scope: Scope) -> Option<Variable> {
    Some(Variable {
        scope,
        id: u32::try_from(to_int(key, 0)).ok()?,
    })
}

/// MEMBER (`.id(n)`): an entity's variable, or the entry of a map value
/// whose key is the identifier (`0686` node 56 reads `#29862.id(1384)`, the
/// configurable wheel choice's voice criteria, from the map its node 57
/// builds with `id(2464)` and `id(1384)` keys).
fn entity_member(object: &Value, key: &Value, context: &mut impl Context) -> Value {
    let id = match object {
        Value::Entity(id) => *id,
        Value::Array(items) => match items.first() {
            Some(Value::Entity(id)) => *id,
            _ => return Value::Null,
        },
        Value::Map(_) => return index(object.clone(), key),
        _ => return Value::Null,
    };
    let key = match key {
        Value::Asset(guid) => Some((guid & 0xffff) as u32),
        key => u32::try_from(to_int(key, 0)).ok(),
    };
    if valid_entity(id) {
        key.map_or(Value::Null, |key| context.entity_member(id, key))
    } else {
        Value::Null
    }
}

fn into_array(value: Value) -> Vec<Value> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items,
        value => vec![value],
    }
}

fn vector(value: &Value) -> [f64; 3] {
    match value {
        Value::Vector(parts) => *parts,
        _ => [0.0; 3],
    }
}

fn vector_position(value: &Value, context: &mut impl Context) -> Option<[f64; 3]> {
    match value {
        Value::Vector(parts) => Some(*parts),
        Value::Entity(id) => Some(context.position(*id)),
        _ => None,
    }
}

fn vector_op(a: [f64; 3], b: [f64; 3], op: impl Fn(f64, f64) -> f64) -> [f64; 3] {
    std::array::from_fn(|index| rounded(op(a[index], b[index])))
}

fn magnitude([x, y, z]: [f64; 3]) -> f64 {
    rounded((x * x + y * y + z * z).sqrt())
}

fn minimum(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

fn maximum(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

fn arithmetic(opcode: u8, a: &Value, b: &Value, context: &mut impl Context) -> Value {
    if matches!(opcode, 23 | 24)
        && matches!(a, Value::Vector(_) | Value::Entity(_))
        && matches!(b, Value::Vector(_) | Value::Entity(_))
    {
        let a = vector_position(a, context).expect("vector or entity");
        let b = vector_position(b, context).expect("vector or entity");
        return Value::Vector(vector_op(
            a,
            b,
            |a, b| if opcode == 23 { a + b } else { a - b },
        ));
    }
    if opcode == 25 {
        match (a, b) {
            (Value::Vector(a), Value::Vector(b)) => {
                return Value::Vector(vector_op(*a, *b, |a, b| a * b));
            }
            (Value::Vector(parts), scalar) | (scalar, Value::Vector(parts)) => {
                let scalar = to_float(scalar);
                return Value::Vector(parts.map(|part| rounded(part * scalar)));
            }
            _ => (),
        }
    }
    if let (26, Value::Vector(parts)) = (opcode, a) {
        if let Value::Vector(divisor) = b {
            return Value::Vector(vector_op(*parts, *divisor, |a, b| {
                if b == 0.0 { 0.0 } else { a / b }
            }));
        }
        let scalar = to_float(b);
        if scalar.abs() < 1.18e-36 {
            return Value::Vector([0.0; 3]);
        }
        let inverse = rounded(1.0 / scalar);
        return Value::Vector(parts.map(|part| rounded(part * inverse)));
    }
    let (x, y) = (to_float(a), to_float(b));
    let value = match opcode {
        23 => x + y,
        24 => x - y,
        25 => x * y,
        26 if y != 0.0 && !y.is_nan() => x / y,
        27 if y != 0.0 => x % y,
        28 if x > 0.0 => x.powf(y),
        _ => 0.0,
    };
    Value::Float(rounded(value))
}

#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
