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
    graph::{asset_guid, member, target_tag_guid},
    vitals::PoolKind,
    wheel::{self, Cosmetic},
};
use orpheus_core::guid::parse_guid;

/// The tolerance of an expression's comparisons (a float's epsilon).
pub const EPSILON: f64 = 1.192_092_895_507_812_5e-5;
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

/// How one entity relates to another, as the client's natives resolve it
/// (ElimName.md and KillFeed.md, "Client tools pass (2026-11-16)").
/// An entity the relation doesn't reach answers entity 0 (none).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityRelation {
    /// `STU_F76C486C`: the owner of an entity the game created for another
    /// (component 72 `+0xD8`: a projectile's or a deployable's owner); none
    /// for an entity without component 72, a body or a controller
    /// (`0x7FF789C2BEE0`). The graphs read it as "the owner, else the entity
    /// itself" (`01CF` rid115).
    Owner,
    /// `STU_F7262B36`: the entity that currently possesses the body.
    CurrentPossessor,
    /// `STU_0AC15F78`: the body the entity possesses.
    PossessedBody,
    /// `STU_DB742151`: the entity itself if it has component 44 (it
    /// possesses a body: a controller), else its possessor from the
    /// client's reverse table of component 44 (`0x7FF789CB7F10`).
    Controller,
    /// The AI entity that drives the body (`STU_066B4605`'s target): none for
    /// a human player's body. Not a native of its own; the server's.
    AiControlEntity,
    /// `STU_2B2E2F6D` kinds 0, 1 and 4 of a projectile no one re-owned
    /// (`Eval_STU_2B2E2F6D` 0x7FF789BDFCC0 on component 50): the entity
    /// that shot it, `+0x18`, which the projectile's creation
    /// `0x7FF789C7E590` writes from the firing entity (the volley's body)
    /// and copies to `+0x20` (kind 4's raw field); the null entity for an
    /// entity that isn't a projectile.
    Shooter,
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

    /// The match's value of the mode entity's component 114 field 9
    /// (`m_CFF0C4E3`, in [0, 1)), which `STU_B182C5C9` reads; None: the
    /// world doesn't know it.
    fn mode_random(&mut self) -> Option<f64> {
        None
    }

    /// The match's random table index (`STU_CBF6FF14`, server-only, in
    /// `[0, 16384)`); None: the world doesn't know it.
    fn match_seed(&mut self) -> Option<i64> {
        None
    }

    /// Whether `entity` holds logical button `button` (`STU_A2A35433`:
    /// the button is in the held list of the entity's statescript
    /// component, 0x7FF78AA9D940); None: unknown.
    fn button_held(&mut self, _entity: u32, _button: i64) -> Option<bool> {
        None
    }

    /// A team's size as the game struct gives it (`STU_D289FA32`; the
    /// entry of `team`, else the "any team" entry 5); None: unknown.
    fn team_size(&mut self, _team: i64) -> Option<i64> {
        None
    }

    /// Whether `entity` is an AI controller (has component 92); None:
    /// unknown.
    fn controller_is_ai(&mut self, _entity: u32) -> Option<bool> {
        None
    }

    /// An AI parameter (`STU_4049CF77`): the value of `identifier` in the
    /// difficulty profile of the AI that possesses `entity` (a body, or
    /// the AI entity itself); Some(None) when `entity` has no AI, or its
    /// AI's profile no such value; None: the world doesn't know `entity`.
    fn ai_parameter(&mut self, _entity: u32, _identifier: u64) -> Option<Option<f64>> {
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

    /// The entities that carry target tag `tag`, each once, in the world's
    /// order (`STU_EB216A0B`); None: the world can't list them.
    fn entities_with_tag(&mut self, _tag: u64) -> Option<Vec<u32>> {
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

    /// The role identifier of `hero` (`STU_BE1161C0`); None: the world
    /// can't tell.
    fn hero_role(&mut self, _hero: u64) -> Option<u64> {
        None
    }

    /// The body (`m_gameplayEntity`) of `hero` (`STU_94AED9DC`); None: the
    /// world can't tell.
    fn hero_body(&mut self, _hero: u64) -> Option<u64> {
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

    /// A body's yaw alone, in the same units (its facing along the ground);
    /// by default the facing's.
    fn entity_yaw(&mut self, entity: u32) -> Option<i32> {
        self.entity_facing(entity).map(|[yaw, _]| yaw)
    }

    /// The move-speed scale of `entity`'s mover (`STU_ED807E07`); None:
    /// unknown.
    fn speed_scale(&mut self, _entity: u32) -> Option<f64> {
        None
    }

    /// The map point a ray from `origin` along unit `direction` meets
    /// within `reach` (a cast without a ray profile); Some(None) when it
    /// meets none, None when this world has no map.
    fn map_ray(
        &mut self,
        _origin: [f64; 3],
        _direction: [f64; 3],
        _reach: f64,
    ) -> Option<Option<[f64; 3]>> {
        None
    }

    /// Whether the match's map supports game mode `mode` (`STU_16753B11`);
    /// None: unknown.
    fn map_supports_mode(&mut self, _mode: u64) -> Option<bool> {
        None
    }

    /// An entity's team index, 0 to 4, or -1 for none; None: unknown.
    fn entity_team(&mut self, _entity: u32) -> Option<i64> {
        None
    }

    /// Whether `position` is inside `entity`'s volume (`STU_73E1C5F0`);
    /// None: unknown.
    fn in_entity_volume(&mut self, _entity: u32, _position: [f64; 3]) -> Option<bool> {
        None
    }

    /// The value `entity`'s statescript registers under ForCode identifier
    /// `identifier` (an active ForCode or ForCodeSynced row), evaluated
    /// now; Some(Null) when no row has it; None: the world can't read that
    /// entity's rows.
    fn for_code(&mut self, _entity: u32, _identifier: u64) -> Option<Value> {
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

    /// The config of Workshop value `value` (its `01C` GUID) in `1B4A`
    /// ([`GraphStore::workshop_value`](crate::graph::GraphStore::workshop_value));
    /// None: unknown.
    fn workshop_value(&mut self, _value: u64) -> Option<Json> {
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
    Evaluator::new(max_steps).config(config, context, 0)
}

/// The config list, `STU_06610EBC`: an ordered group of configs.
pub(crate) const CONFIG_LIST: &str = "STU_06610EBC";

/// A config's value as an assignment reads it (the native +28 interface,
/// which SetVar and SetVars use): a config list collects the values of all
/// its configs into one array. Every other config reads as [`evaluate`]
/// does. From leafall903/ORPHEUS (MIT), commit c239a84
/// (`docs/VARIABLE_ASSIGNMENTS.txt`, `play.evaluate_value`).
pub fn evaluate_value(config: &Json, context: &mut impl Context) -> Result<Value, EvalError> {
    Evaluator::new(MAX_EXPRESSION_STEPS).value(config, context, 0)
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

/// An assignment to an array element, `#v[index]` (`[8 v, <index>, 17]`:
/// Set Global Variable At Index `1DDD` writes `#514[clamp(#18739[1], 0,
/// 999)]` so): the variable and the index, evaluated now. None: not that
/// form.
pub fn element_lvalue(
    config: &Json,
    context: &mut impl Context,
) -> Result<Option<(Variable, Value)>, EvalError> {
    if !is_expression(class(config)) {
        return Ok(None);
    }
    let parts = Expression::read(config)?;
    let end = parts
        .code
        .iter()
        .rposition(|op| op.as_u64() != Some(0))
        .unwrap_or(0);
    if end < 3
        || !matches!(parts.code[0].as_u64(), Some(8 | 9))
        || parts.code[end].as_u64() != Some(17)
    {
        return Ok(None);
    }
    let Some(variable) = dynamic(table(parts.dynamics, byte(&parts.code[1], 1)?, 0)?)? else {
        return Ok(None);
    };
    let index = Evaluator::new(MAX_EXPRESSION_STEPS).run(
        Expression {
            code: &parts.code[2..end],
            ..parts
        },
        context,
        0,
    )?;
    Ok(Some((variable, index)))
}

/// Workshop variable `index` (clamped to 0..=127) of an entity: its entity
/// variable `index - 129` as a u16, `FF7F` to `FFFE` (the client's
/// opcode 72, case 0x48 of 0x7FF78AA91210, writes a reference's id so).
/// Workshop global variables are the mode entity's, player variables the
/// player's (`refs/reports/Workshop.md` §2).
pub fn workshop_variable(index: i64) -> u32 {
    u32::from((index.clamp(0, 127) as u16).wrapping_sub(129))
}

/// An `m_out_Var` that names an entity's Workshop variable with opcode 72
/// (`STU_0BDBC7B2` in Set Global Variable `1367`: `[14 0, 8 0, 6 0, 17,
/// 72]`, the mode entity's variable `#18739[0]`): the entity the
/// expression leaves and the variable. None: no opcode 72 named one.
pub fn workshop_reference(
    config: &Json,
    context: &mut impl Context,
) -> Result<Option<(Value, u32)>, EvalError> {
    let mut evaluator = Evaluator::new(MAX_EXPRESSION_STEPS);
    evaluator.reference = Some(None);
    let entity = evaluator.config(config, context, 0)?;
    Ok(evaluator.reference.flatten().map(|id| (entity, id)))
}

/// The keys of a Workshop value tree (a keyed array, `sub_7FF789C14870`):
/// the value's `01C` GUID, its parameters, and the mark of an item taken
/// as it is.
pub const WORKSHOP_VALUE: u16 = 0x229a;
pub const WORKSHOP_PARAMETERS: u16 = 0x4932;
const WORKSHOP_AS_IS: u16 = 0x4e90;

/// Workshop value `value` (its `01C` GUID) with `parameters`, as the
/// action graphs' `STU_77B0987E` reads it from `#18739`.
pub fn workshop_tree(value: u64, parameters: Vec<Value>) -> Value {
    Value::Map(vec![
        (
            Value::Integer(i64::from(WORKSHOP_VALUE)),
            Value::Asset(value),
        ),
        (
            Value::Integer(i64::from(WORKSHOP_PARAMETERS)),
            Value::Array(parameters),
        ),
    ])
}

fn workshop_key(tree: &[(Value, Value)], key: u16) -> Option<&Value> {
    tree.iter()
        .find(|(item, _)| *item == Value::Integer(i64::from(key)))
        .map(|(_, value)| value)
}

/// A value as Workshop text, for the client's string configs
/// (`STU_903EAF27`, `sub_7FF789A700F0`): a string as it is, an integer
/// with `%d`, a float with `%d` when it is whole and `%.2f` otherwise, an
/// array's items joined with ", ".
pub fn workshop_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(value) => if *value { "True" } else { "False" }.to_owned(),
        Value::Integer(value) => value.to_string(),
        Value::Float(value) => {
            let value = *value as f32;
            let whole = value.round();
            if (whole - value).abs() < 0.000_011_920_929 {
                format!("{}", whole as i32)
            } else {
                format!("{value:.2}")
            }
        }
        Value::Vector(parts) => {
            let parts: Vec<String> = parts
                .iter()
                .map(|part| workshop_text(&Value::Float(*part)))
                .collect();
            format!("({})", parts.join(", "))
        }
        Value::Array(items) => items
            .iter()
            .map(workshop_text)
            .collect::<Vec<_>>()
            .join(", "),
        other => format!("{other:?}"),
    }
}

/// The variable an entity member expression reads (`entity.#id`: a config
/// push, the identifier, the member read: `[14 c, 7 v, 15 0]`).
fn member_identifier(config: &Json) -> Result<Option<u32>, EvalError> {
    if !is_expression(class(config)) {
        return Ok(None);
    }
    let parts = Expression::read(config)?;
    let code: Vec<Option<u64>> = parts.code.iter().map(Json::as_u64).collect();
    if !matches!(
        code.as_slice(),
        [Some(14), _, Some(7), _, Some(15), Some(0)]
    ) {
        return Ok(None);
    }
    let index = byte(&parts.code[3], 3)?;
    identifier(table(parts.dynamics, index, 2)?).map(Some)
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

/// The client's f32 factors from radians to degrees (0x7FF78B53BD0C) and
/// back (0x7FF78B51FC98).
const DEGREES: f64 = 57.295_780_181_884_766;
const RADIANS: f64 = 0.017_453_292_384_743_69;

/// A unit direction's yaw atan2(x, z) (0 when x and z are both 0) and
/// pitch asin(−y), clamped, in radians (0x7FF789BFB820, 0x7FF789BE29B0).
fn yaw_pitch([x, y, z]: [f64; 3]) -> [f64; 2] {
    let yaw = if x != 0.0 || z != 0.0 {
        x.atan2(z)
    } else {
        0.0
    };
    [yaw, (-y).clamp(-1.0, 1.0).asin()]
}

/// The direction of a yaw and pitch in radians: the forward row of a
/// pitch, then yaw, rotation of the identity (0x7FF78A7596E0).
fn forward(yaw: f64, pitch: f64) -> [f64; 3] {
    let (yaw_sin, yaw_cos) = yaw.sin_cos();
    let (pitch_sin, pitch_cos) = pitch.sin_cos();
    [yaw_sin * pitch_cos, -pitch_sin, yaw_cos * pitch_cos]
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
pub(crate) fn to_entity(value: &Value) -> Option<u32> {
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

/// Filter bit 28 of component 25's word, which the team relation
/// 0x7FF789B2CB00 reads through 0x7FF789B2F000 and 0x7FF789B2ECD0.
const TEAM_RELATION_BIT: u64 = 1 << 28;

/// Whether team `team` is hostile to an entity of team `own` (−1 none)
/// with filter word `flags` (0x7FF789B2CB00 without an override): with
/// filter bit 28 and no team, only team 1 is; with bit 28, any other team;
/// else any other team, and teams −1 and 4 on either side are hostile to
/// all.
fn hostile_to_team(team: i64, own: i64, flags: u64) -> bool {
    if flags & TEAM_RELATION_BIT != 0 {
        return if team_from_filter(flags) == -1 {
            team == 1
        } else {
            team != own
        };
    }
    matches!(team, -1 | 4) || matches!(own, -1 | 4) || team != own
}

/// A quarter of a turn in the yaw units of [`aim`].
const QUARTER_TURN: i32 = 16_384;

/// The unit vector a body looks along, each axis rounded to f32: forward is
/// `(sin yaw, 0, cos yaw)` and a positive pitch looks down (`combat.aim` and
/// `Match.script_body_facing` in leafall903/ORPHEUS, commit c239a84).
pub(crate) fn aim([yaw, pitch]: [i32; 2]) -> [f64; 3] {
    const TURN: f64 = std::f64::consts::TAU / 65536.0;
    let (yaw_sin, yaw_cos) = (f64::from(yaw) * TURN).sin_cos();
    let (pitch_sin, pitch_cos) = (f64::from(pitch) * TURN).sin_cos();
    [yaw_sin * pitch_cos, -pitch_sin, yaw_cos * pitch_cos].map(rounded)
}

/// The client's arcsine (0x7FF7894D7970): the sine clamped to [−1, 1], then
/// an odd polynomial in f32 (π/2 at 1), as the client sums it.
pub(crate) fn client_asin(sine: f32) -> f32 {
    let x = sine.clamp(-1.0, 1.0);
    let a = x.abs();
    let a2 = a * a;
    let a3 = a2 * a;
    let a5 = a3 * a2;
    let a7 = a5 * a2;
    let a9 = a7 * a2;
    let a11 = a9 * a2;
    let a13 = a11 * a2;
    let a15 = a2 * a13;
    let value = a * 0.872_197_5
        + a3 * 2.821_105_7
        + a5 * -21.305_187
        + a7 * 88.440_08
        + a9 * -205.448_36
        + a11 * 271.334_17
        - a13 * 189.901_58
        + a15 * 54.758_385;
    if x < 0.0 { -value } else { value }
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
    member(config, "$")?.as_str()
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

/// A scene tag query's fields (`STU_EB216A0B`, `STU_B3E67A36`, the state
/// `STU_160F2578` and the action `STU_42578342` share them) in the forms
/// [`Evaluator::tag_query`] runs: the three tag lists and the participant
/// filter.
fn tag_query_shape(config: &Json) -> bool {
    config.as_object().is_some_and(|fields| {
        fields.iter().all(|(key, value)| match key.as_str() {
            "$" | "m_9C7BD8B8" | "m_B486603E" | "m_8B2A4DF3" | "m_filter" => true,
            "m_EE729DCB" => value.as_u64() == Some(0),
            _ => value.is_null(),
        })
    })
}

fn is_expression(class: Option<&str>) -> bool {
    matches!(class, Some("STU_919FD47C" | "STUConfigVarExpression"))
}

fn child<'a>(config: &'a Json, field: &str) -> &'a Json {
    member(config, field).unwrap_or(&Json::Null)
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

/// Insert into a native identifier map's buckets (104319 F66E90/F683E0/
/// F68480, nonnegative keys; leafall903/ORPHEUS (MIT) `_insert` in
/// `play.py`): eight buckets at first, linear probing from `key % len`; an
/// equal key is replaced; a full table doubles and takes the old entries
/// in their bucket order. Key 0 marks an empty bucket, so it clears one.
fn identifier_map_insert(buckets: &mut Vec<Option<(u32, Value)>>, key: u32, value: Value) {
    if buckets.is_empty() {
        buckets.resize(8, None);
    }
    let len = buckets.len();
    for offset in 0..len {
        let position = (key as usize + offset) % len;
        if buckets[position]
            .as_ref()
            .is_none_or(|(existing, _)| *existing == key)
        {
            buckets[position] = (key != 0).then_some((key, value));
            return;
        }
    }
    let mut grown = vec![None; len * 2];
    for (old_key, old_value) in std::mem::take(buckets).into_iter().flatten() {
        identifier_map_insert(&mut grown, old_key, old_value);
    }
    identifier_map_insert(&mut grown, key, value);
    *buckets = grown;
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

pub(crate) fn resource(value: &Json) -> Option<u64> {
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

/// A `STUConfigVarFilter`'s masks and relation words, as the client's
/// `Filter_TestTeamMasks` (0x7FF789ADAAE0) reads them (`vm/targets.rs`
/// module notes): any-of `m_84065FD1` (`0x3FFFFFFF` every kind), all-of
/// `m_1E14CCA8`, none-of `m_4EA686DB` against the entity's filter word;
/// include `m_27CF71D5`, require `m_66747E7D`, exclude `m_89831602` of the
/// relation bits 1 friendly, 2 hostile, 4 the searcher.
pub(crate) struct FilterMasks {
    any: u64,
    all: u64,
    none: u64,
    include: u64,
    require: u64,
    exclude: u64,
}

/// `m_84065FD1`: every kind.
const ALL_KINDS: u64 = 0x3fff_ffff;
const FRIENDLY: u64 = 1;
const HOSTILE: u64 = 2;
const SEARCHER: u64 = 4;

impl FilterMasks {
    /// The masks of a filter whose other fields are the three tag lists
    /// (`m_C1903771`, +0x70, is not read by the client's filter test
    /// 0x7FF789C1DA20) and a condition ([`passes_filter`]); None: a field
    /// the evaluator can't compute.
    fn read(filter: &Json) -> Option<Self> {
        if class(filter) != Some("STUConfigVarFilter") {
            return None;
        }
        let supported = filter
            .as_object()?
            .iter()
            .all(|(key, value)| match key.as_str() {
                "$" | "m_9C7BD8B8" | "m_B486603E" | "m_8B2A4DF3" | "m_C1903771" | "m_condition" => {
                    true
                }
                "m_84065FD1" | "m_1E14CCA8" | "m_4EA686DB" | "m_27CF71D5" | "m_66747E7D"
                | "m_89831602" => value.as_u64().is_some(),
                "m_EE729DCB" => value.as_u64() == Some(0),
                "m_93FA1642" => value.is_null() || value.as_array().is_some_and(Vec::is_empty),
                "m_F61726FE" | "m_B40362FC" => value.is_null(),
                _ => false,
            });
        supported.then(|| Self::words(filter))
    }

    /// The six words of a filter, each 0 when unset; its other fields are
    /// the caller's.
    pub(crate) fn words(filter: &Json) -> Self {
        let word = |name: &str| child(filter, name).as_u64().unwrap_or(0);
        Self {
            any: word("m_84065FD1"),
            all: word("m_1E14CCA8"),
            none: word("m_4EA686DB"),
            include: word("m_27CF71D5"),
            require: word("m_66747E7D"),
            exclude: word("m_89831602"),
        }
    }

    /// Whether the masks read the entity's filter word.
    fn read_word(&self) -> bool {
        self.any != ALL_KINDS || self.all != 0 || self.none != 0
    }

    pub(crate) fn relations(&self) -> bool {
        self.include | self.require | self.exclude != 0
    }

    /// Whether nothing can pass: no any-of mask and no include word.
    pub(crate) fn passes_none(&self) -> bool {
        self.any == 0 && self.include == 0
    }

    /// The client's test: `word` the entity's filter word (unread when
    /// [`Self::read_word`] is false), `friendly` and `searcher` its
    /// relation to the evaluating entity (unread without relation words).
    pub(crate) fn passes(&self, word: u64, friendly: bool, searcher: bool) -> bool {
        let relation = |bits: u64| {
            (bits & FRIENDLY != 0 && friendly)
                || (bits & HOSTILE != 0 && !friendly)
                || (bits & SEARCHER != 0 && searcher)
        };
        let any = self.any == ALL_KINDS || word & self.any != 0 || relation(self.include);
        let all = word & self.all == self.all
            && !(self.require & FRIENDLY != 0 && !friendly
                || self.require & HOSTILE != 0 && friendly
                || self.require & SEARCHER != 0 && !searcher);
        let none = word & self.none != 0 || relation(self.exclude);
        any && all && !none
    }
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
        // A graph resource (`180A`'s phase script lists; upstream keeps an
        // empty one as the zero asset).
        "STU_105E1BCC" => Some("m_graph"),
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
    /// [`workshop_reference`]: opcode 72 with only its entity below it
    /// names its variable here instead of reading it (none: not asked).
    reference: Option<Option<u32>>,
    /// The parameters of the Workshop values being evaluated, innermost
    /// last ([`Self::workshop_tree`]).
    workshop_parameters: Vec<Vec<Value>>,
}

impl Evaluator {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            steps: 0,
            reference: None,
            workshop_parameters: Vec::new(),
        }
    }

    /// A Workshop value tree as the client evaluates it
    /// (`sub_7FF789C14870`): key `229A` names the value (its `01C` GUID),
    /// whose config in `1B4A` ([`Context::workshop_value`]) runs with the
    /// items of key `4932` as its parameters, at most eight. An item that
    /// is itself a tree with parameters is evaluated first; one with key
    /// `4E90` too, or any other value, is taken as it is. Nothing for an
    /// unknown value. (The client's other item forms, keys `0C6B`, `06F1`,
    /// `03AC`, `6933`, `1EE2`, `01D1`, are a player slot, team, hero, map,
    /// game mode and an identifier read from the match; the server sends
    /// none of them.)
    fn workshop_tree(
        &mut self,
        tree: &[(Value, Value)],
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        let Some(Value::Asset(value)) = workshop_key(tree, WORKSHOP_VALUE) else {
            return Ok(Value::Null);
        };
        let Some(config) = context.workshop_value(*value) else {
            return Ok(Value::Null);
        };
        let mut parameters = Vec::new();
        if let Some(Value::Array(items)) = workshop_key(tree, WORKSHOP_PARAMETERS) {
            for item in items.iter().take(8) {
                parameters.push(match item {
                    Value::Map(nested)
                        if workshop_key(nested, WORKSHOP_PARAMETERS).is_some()
                            && workshop_key(nested, WORKSHOP_AS_IS).is_none() =>
                    {
                        self.workshop_tree(nested, context, depth + 1)?
                    }
                    item => item.clone(),
                });
            }
        }
        self.workshop_parameters.push(parameters);
        let result = self.config(&config, context, depth + 1);
        self.workshop_parameters.pop();
        result
    }
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
        if class(config) == Some("STU_45B0D2B2") {
            let mut items = Vec::new();
            for member in array_field(config, "m_configVars")? {
                items.push(if member.is_null() {
                    Value::Integer(0)
                } else {
                    self.value(member, context, depth + 1)?
                });
            }
            return Ok(Value::Array(items));
        }
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

    /// The entities of a scene tag query, as the client keeps them
    /// (`STU_160F2578` after its begin, 0x7FF789C4FE60): each field's tags
    /// (0x7FF789C5E9A0: every tag its value gives, each once), then over
    /// every entity (0x7FF789CC3850) those that carry a tag of
    /// `m_9C7BD8B8` (any entity when it gives none), every tag of
    /// `m_B486603E` and no tag of `m_8B2A4DF3` (0x7FF789CC5310), and pass
    /// the participant filter `m_filter` when it has one. A world lists
    /// only the entities with a tag ([`Context::entities_with_tag`]), so
    /// the entities come from the first two fields' tags, else from the
    /// filter's own any-of and all-of tag lists; a query that names no tag
    /// can't be run. Assumption: the filter is tested as `STU_AC829876`
    /// tests one (the script's owner the searcher); the call that tests it
    /// in 0x7FF789CC3850 was not read.
    fn tag_query(
        &mut self,
        config: &Json,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Vec<u32>, EvalError> {
        let unsupported = || EvalError::UnsupportedConfig(class(config).unwrap_or("?").into());
        if !tag_query_shape(config) {
            return Err(unsupported());
        }
        let mut lists: [Vec<u64>; 3] = Default::default();
        for (list, name) in lists
            .iter_mut()
            .zip(["m_9C7BD8B8", "m_B486603E", "m_8B2A4DF3"])
        {
            let mut rows = Vec::new();
            value_rows(self.value(child(config, name), context, depth)?, &mut rows)?;
            for row in rows {
                match row {
                    Value::Null | Value::Asset(0) => (),
                    Value::Asset(tag) if tag >> 48 == crate::graph::TARGET_TAG_TYPE => {
                        if !list.contains(&tag) {
                            list.push(tag);
                        }
                    }
                    _ => return Err(unsupported()),
                }
            }
        }
        let [any, all, none] = lists;
        let filter = child(config, "m_filter");
        let filter = if filter.is_null() {
            None
        } else {
            Some((filter, FilterMasks::read(filter).ok_or_else(unsupported)?))
        };
        let filter_tags = |field: &str| -> Result<Vec<u64>, EvalError> {
            let Some((filter, _)) = filter else {
                return Ok(Vec::new());
            };
            filter_tag_list(child(filter, field)).ok_or_else(unsupported)
        };
        let (filter_any, filter_all) = (filter_tags("m_9C7BD8B8")?, filter_tags("m_B486603E")?);
        let listed: &[u64] = [&any, &all, &filter_any, &filter_all]
            .into_iter()
            .zip([usize::MAX, 1, usize::MAX, 1])
            .find(|(tags, _)| !tags.is_empty())
            .map(|(tags, take)| &tags[..tags.len().min(take)])
            .ok_or_else(unsupported)?;
        let mut found = Vec::new();
        for &tag in listed {
            for entity in context.entities_with_tag(tag).ok_or_else(unsupported)? {
                if found.contains(&entity) {
                    continue;
                }
                let mut keep = true;
                for (tags, wanted) in [(&all, true), (&none, false)] {
                    for &tag in tags {
                        let carries = context
                            .has_target_tag(entity, tag)
                            .ok_or_else(unsupported)?;
                        keep &= carries == wanted;
                    }
                }
                if keep && let Some((filter, masks)) = &filter {
                    keep = match passes_filter(filter, masks, entity, context) {
                        Err(EvalError::UnsupportedConfig(_)) => return Err(unsupported()),
                        result => result?,
                    };
                }
                if keep {
                    found.push(entity);
                }
            }
        }
        Ok(found)
    }
}

/// The tags of a filter's tag list field (null, one `STU_357DABBB` or a
/// config list of them), as [`check_tag_set`] reads them; None: another
/// form.
pub(crate) fn filter_tag_list(field: &Json) -> Option<Vec<u64>> {
    let configs = if field.is_null() {
        &[]
    } else if native_shape(field, CONFIG_LIST, Some("m_configVars")) {
        field.get("m_configVars")?.as_array()?.as_slice()
    } else {
        std::slice::from_ref(field)
    };
    let mut tags = Vec::new();
    for config in configs {
        let tag = target_tag_config(config)?;
        if tag != 0 && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    Some(tags)
}

impl Evaluator {
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
            // A Workshop string as text (`Eval_STU_903EAF27`
            // 0x7FF78984F4A0 formats its `m_value` with `sub_7FF789A700F0`;
            // `STU_2566340D` gives its `m_string`'s with five flags,
            // `m_DBF18833` … `m_796613D4`, unread here). The server hands
            // the action graphs its strings already formatted (a string
            // value), so the text is the value's: a string as it is, other
            // values as [`workshop_text`] writes them. Assumption: the
            // client's formatter leaves a plain string as it is.
            "STU_903EAF27" => Value::String(workshop_text(&self.config(value, context, nested)?)),
            "STU_2566340D" => {
                return self.config(child(config, "m_string"), context, nested);
            }
            // A Workshop parameter as the action graphs read it from
            // `#18739` (391 uses in 177 graphs): `m_35547D6E` within
            // `m_FDCA1FD0` and `m_68CFF844` (slot 5 `sub_7FF789C19A10`,
            // then `sub_7FF789C15350`: an integer clamped to them, a float
            // only when they don't hold 0, any other value as it is). The
            // value is a Workshop value tree, a keyed array (type 13):
            // anything else reads as nothing ([`Self::workshop_tree`]).
            "STU_77B0987E" => {
                let value = match self.config(child(config, "m_35547D6E"), context, nested)? {
                    Value::Map(tree) => self.workshop_tree(&tree, context, nested)?,
                    _ => Value::Null,
                };
                let low = to_float(&self.config(child(config, "m_FDCA1FD0"), context, nested)?);
                let high = to_float(&self.config(child(config, "m_68CFF844"), context, nested)?);
                match value {
                    Value::Integer(value) => {
                        Value::Integer(value.clamp(low as i64, (high as i64).max(low as i64)))
                    }
                    Value::Float(value) if low > 0.0 || high < 0.0 => {
                        Value::Float(value.clamp(low, high.max(low)))
                    }
                    value => value,
                }
            }
            // A Workshop value's parameters as `1B4A`'s configs read them
            // (slot 5: the parameter array at `+0x370` plus 16 bytes per
            // index; evaluate `sub_7FF78AA88550`, shared).
            "STU_710FE2E1" | "STU_F619A2A1" | "STU_F8C22BD0" | "STU_E2D4B7FE" | "STU_B7520848" => {
                let index = match class {
                    "STU_710FE2E1" => 0,
                    "STU_F619A2A1" => 1,
                    "STU_F8C22BD0" => 2,
                    "STU_E2D4B7FE" => 3,
                    _ => 4,
                };
                self.workshop_parameters
                    .last()
                    .and_then(|parameters| parameters.get(index))
                    .cloned()
                    .unwrap_or(Value::Null)
            }
            // Local Player (`Eval_STU_8985E00A`: the client's own player
            // entity): the server has none.
            "STU_8985E00A" => Value::Null,
            // A copy of a reevaluated Workshop parameter (Create HUD Text's
            // scope `1D8A` rid6 and five more Create graphs). Server-only:
            // the client's evaluate is the base config's `xor al, al`; the
            // server gives its `m_value` (assumption: the server evaluates
            // it again on its reevaluation, Workshop.md §8).
            "STU_70757664" => return self.config(value, context, nested),
            "STU_919FD47C" | "STUConfigVarExpression" => {
                return self.expression(config, context, nested);
            }
            "STU_B70476F3" => literal_asset(config, class, "m_548E5344", 0x0a90)
                .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
            // An aura (`053`): ApplyAura's `m_A76A8CF6`, ModifyHealth's
            // `m_277A4ECA` (`vm/auras.rs`).
            "STU_0E766B33" => literal_asset(config, class, "m_A76A8CF6", 0x04a0)
                .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
            // A ray profile (`097`): what Sombra's, Moira's, Torbjörn's and
            // Baptiste's graphs store in `#8518` by the owner's team for
            // their volleys' `m_C1903771` (`weapons::ray_profile`).
            "STU_20DB8AB5" => literal_asset(config, class, "m_C1903771", 0x0690)
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
            // `Eval_STU_F4C77F4A` (0x7FF78AA8AB10) sets the evaluation
            // context's `+0x4C0` to `m_4BA4C058` while it evaluates
            // `m_value`, and gives that value. `01CA` rid153/rid158 ask it
            // of `GetGameMode()` at level 1 (and compare with Elimination
            // `0009`, `0020`, `0029`): answered with the mode itself.
            // Assumption: the level picks a base mode, and each mode the
            // server runs is its own base.
            "STU_F4C77F4A" => return self.config(child(config, "m_value"), context, nested),
            // A pick from the list `m_814F8EC3` (between `m_8054375D` and
            // `m_418D9D9D` of them, `m_C239F989` a count; `01CA` rid23 sends
            // `00BC.025` to the hero's allies near his victim). Only its
            // empty form is known: an empty or unset list picks none (as
            // when the server's `STU_ED45C5E8` query finds nobody).
            "STU_A6B80C63" => match self.config(child(config, "m_814F8EC3"), context, nested)? {
                Value::Null => Value::Array(Vec::new()),
                Value::Array(items) if items.is_empty() => Value::Array(Vec::new()),
                _ => return Err(EvalError::UnsupportedConfig(class.into())),
            },
            "STU_7757D403" => authored_pairs(self, config, context, nested)?,
            // An authored identifier map (`STU_7042F028`, 104319 1B78C00;
            // leafall903/ORPHEUS (MIT) `authored_map` in `play.py`): each
            // row's `m_value` (read as an assignment; unset: 0) under its
            // identifier's low 32 bits, in the native map's bucket order
            // ([`identifier_map_insert`]). `180A` keeps its phase scripts
            // so (#31759-#31766: identifier `416C` → a `STU_45B0D2B2`
            // graph list).
            "STU_7042F028" => {
                let mut buckets = Vec::new();
                for row in array_field(config, "m_92829780")? {
                    let value = match row.get("m_value") {
                        None | Some(Json::Null) => Value::Integer(0),
                        Some(member) => self.value(member, context, nested)?,
                    };
                    let key = resource(child(row, "m_identifier"))
                        .ok_or(EvalError::MalformedConfig("authored map identifier"))?
                        as u32;
                    if key & 0x8000_0000 != 0 {
                        return Err(EvalError::UnsupportedConfig(class.into()));
                    }
                    identifier_map_insert(&mut buckets, key, value);
                }
                Value::Map(
                    buckets
                        .into_iter()
                        .flatten()
                        .map(|(key, value)| (Value::Integer(i64::from(key)), value))
                        .collect(),
                )
            }
            // An enum as an integer: its evaluate is the integer config's
            // (0x7FF78AA960F0: `SetInt` of the dword at +0x10, here
            // `m_3CE40E49`, enum 1A77DFEC; unset: 0).
            "STU_038C32E8" => Value::Integer(child(config, "m_3CE40E49").as_i64().unwrap_or(0)),
            // An authored array (`STU_45B0D2B2`; upstream `AUTHORED_ARRAY`)
            // as a scalar: its first config (unset: 0; none: null). As a
            // value ([`Evaluator::value`]) every one in order.
            "STU_45B0D2B2" => match array_field(config, "m_configVars")?.first() {
                None => Value::Null,
                Some(Json::Null) => Value::Integer(0),
                Some(first) => self.value(first, context, nested)?,
            },
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
            // The local player's crouch toggle (`Eval_STU_555DD49F`
            // 0x7FF78983F910 → 0x7FF789491330: the local player's entity's
            // component 59 (command/aim) `+0x1588`, else false). The input
            // sampler 0x7FF78961E170 flips it on input action 13, clears it
            // on 11/12, and while it is on sets button bit 0x80 (logical
            // button 12) in the command. A server has no local player:
            // false. Wrecking Ball's `160A` rid72 gates a voice line on it.
            "STU_555DD49F" if native_shape(config, class, None) => Value::Bool(false),
            // One frame's length in seconds (`Eval_STU_4B02509E`
            // 0x7FF789C026E0: the script context's (host `+0x10`) int64
            // `+0x430` × 0.001f; the context keeps the milliseconds per
            // frame there: the ability state's timing block, 0x7FF789B94AC0,
            // makes a start time of `(frame + 1) × it`, the frame at
            // context `+0x1D0`). Brigitte's `1248` waits it beyond #10349,
            // Moira's `0F76` rid43 adds `3·x / 0.016` (3 a frame) and `15F5`
            // rid18 moves a speed for `4·x` seconds.
            "STU_4B02509E" if native_shape(config, class, None) => {
                Value::Float(f64::from(crate::timeline::FRAME_MS as f32 * 0.001_f32))
            }
            // The game ruleset's "Competitive Rules" (`Eval_STU_BEDA850F`
            // 0x7FF789BED1D0 → 0x7FF789CAD2C0(game, `0D8:2548`, bool)): the
            // row of key 0x2548 (9544) in the game's ruleset table (`+216`
            // items {u32 value, u16 key at +4}, `+224` count; 0x7FF789356650),
            // which 0x7FF789CADDB0 loads from the resource in the mode
            // entity's component 114 field 2 (`+0x140`) when it changes. The
            // fallback through the mode's `STUGameMode.m_3CE93B76`
            // (0x7FF789CAD640) writes a value but returns "not found", so
            // the evaluate fails either way without that row. The server
            // sends field 2 unset (`orpheus_sim::scene::mode_create`): the
            // client has no row, so no value here either.
            "STU_BEDA850F" if native_shape(config, class, None) => Value::Null,
            // A server-only bool of `m_entity` (no registration of its own,
            // one use: `01CB` rid64, `!mode.id(9541) || 68F68CD7(owner) ||
            // #11485`, which skips the kill cam). The client runs that
            // networked node with the base config's evaluate (record #155,
            // 0x7FF78A14CC10 `xor al, al`; slot 5 0x7FF78984CA10 then gives
            // no value), and the expression's config opcode (0x7FF78AA91210,
            // case 0xE) pushes the invalid value (type 255) for it: null,
            // read as false. The server's own meaning is unknown; this is
            // the client's answer (assumption: the same on the server).
            "STU_68F68CD7" => Value::Null,
            // The ForCode value `254B` of the script's entity as a float
            // (`Eval_STU_295F1CDE` 0x7FF789BD9A80 → 0x7FF789B2BBB0 →
            // 0x7FF789C6ACC0: the entity's statescript registry, first row
            // of that identifier; it also makes the config depend on #9527,
            // the variable the row reads). Every hero body registers it in
            // `0033` st13 (ForCodeSynced, `valueOr(#9527e, 1.0)`); Reinhardt's
            // `0095` scales his barrier by it. With no row (or an invalid
            // value) the client asks the entity's owner (component 72),
            // past a Rose-hidden jump: unanswered here.
            "STU_295F1CDE" if native_shape(config, class, None) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let Value::Entity(entity) = context.owner() else {
                    return Err(unsupported());
                };
                match context.for_code(entity, 0x0d80_0000_0000_254b) {
                    Some(Value::Null) | None => return Err(unsupported()),
                    Some(value) => Value::Float(rounded(to_float(&value))),
                }
            }
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
            // A hero's role (`0C90` node 44 on `070B`, the hero limits):
            // `Eval_STU_BE1161C0` 0x7FF789BF79E0 loads `m_hero`'s asset and,
            // for a hero (`STU_7C3457DC`), answers its `+0x58` (`m_category`,
            // the role identifier), else asset 0.
            "STU_BE1161C0"
                if native_fields(config, &["m_hero", "m_resourceKey"])
                    && child(config, "m_resourceKey").is_null() =>
            {
                match self.config(child(config, "m_hero"), context, nested)? {
                    Value::Asset(hero) if hero != 0 => Value::Asset(
                        context
                            .hero_role(hero)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    Value::Null | Value::Asset(0) => Value::Asset(0),
                    _ => return Err(EvalError::UnsupportedConfig(class.into())),
                }
            }
            // A hero's body (`265F`'s pick filters rid17/rid22, `25B4` rid3):
            // `Eval_STU_94AED9DC` 0x7FF789BF7B80 loads `m_hero`'s asset and,
            // for a hero (`STU_7C3457DC`), answers the GUID of its
            // `m_gameplayEntity` (`+0x20`, the GUID at `+0x28`); else no
            // value.
            "STU_94AED9DC"
                if native_fields(config, &["m_hero", "m_resourceKey"])
                    && child(config, "m_resourceKey").is_null() =>
            {
                match self.config(child(config, "m_hero"), context, nested)? {
                    Value::Asset(hero) if hero != 0 => Value::Asset(
                        context
                            .hero_body(hero)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    Value::Null | Value::Asset(0) => Value::Null,
                    _ => return Err(EvalError::UnsupportedConfig(class.into())),
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
            // A server-only wrapper of one config (`m_094F4D35`; its client
            // evaluate is the base `xor al, al`): the patrol `0B8E` (rid5)
            // wraps `#6947[#39]`, the next hint, in it. Assumption: it
            // passes its value through.
            "STU_A8FB8712" if native_shape(config, class, Some("m_094F4D35")) => {
                self.config(child(config, "m_094F4D35"), context, nested)?
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
            // An entity's position (`Eval_STU_309B0570` 0x7FF789C028B0 →
            // `sub_7FF789C2FD80`; ModeGraphGaps.md, "Client tools pass 2
            // (2026-11-16)"): with `m_5B94EDB8` true, the point of its
            // component 33 (`STU_AF1D443B`), which the server doesn't
            // model (a world's own answer or unsupported); else its world
            // position (component 28, the node's local vector placed in its
            // parent's frame). No entity or an invalid one fails, null here.
            // Soldier: 76's death path (`01D8` rid92) takes the attacker's
            // (#392) away from #681.
            "STU_309B0570" if native_fields(config, &["m_entity", "m_5B94EDB8"]) => {
                let anchored = child(config, "m_5B94EDB8");
                if !anchored.is_null() && truthy(&self.config(anchored, context, nested)?) {
                    return context
                        .native(config)
                        .ok_or_else(|| EvalError::UnsupportedConfig(class.into()));
                }
                match to_entity(&self.config(child(config, "m_entity"), context, nested)?) {
                    Some(entity) if valid_entity(entity) => Value::Vector(
                        context
                            .entity_position(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    _ => Value::Null,
                }
            }
            // An entity's position (`Eval_STU_8CBDD81C` 0x7FF789BF6430 →
            // 0x7FF789C26460): an entity with a mover (component 31) and a
            // pose, the pose's position (0x7FF789B471D0); else component
            // 1's translation placed in its parent's frame. No entity or an
            // invalid one fails, null here. Mostly the owner (32 of 37).
            "STU_8CBDD81C" if native_shape(config, class, Some("m_entity")) => {
                match to_entity(&self.config(child(config, "m_entity"), context, nested)?) {
                    Some(entity) if valid_entity(entity) => Value::Vector(
                        context
                            .entity_position(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    _ => Value::Null,
                }
            }
            // An entity's side axis (`Eval_STU_66F3B89E` 0x7FF789BF8460 →
            // 0x7FF789C2B5E0): the first row of its world matrix, which for
            // a posed body is up × forward of its model's pose (the yaw
            // alone, the model doesn't pitch): `(cos yaw, 0, −sin yaw)`, the
            // facing turned a quarter (upstream `scene.py`: `aim(yaw +
            // 16384, 0)`). No entity or an invalid one fails, null here.
            // Mostly the owner (51 of 66): knockback and launch sides.
            "STU_66F3B89E" if native_shape(config, class, Some("m_entity")) => {
                if let Some(value) = context.native(config) {
                    return Ok(value);
                }
                match to_entity(&self.config(child(config, "m_entity"), context, nested)?) {
                    Some(entity) if valid_entity(entity) => {
                        let yaw = context
                            .entity_yaw(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                        Value::Vector(aim([yaw.wrapping_add(QUARTER_TURN), 0]))
                    }
                    _ => Value::Null,
                }
            }
            // An entity's move-speed scale (`Eval_STU_ED807E07`
            // 0x7FF789BE3570): the float its mover (component 31) keeps at
            // `+0x4D8`, which the mover update 0x7FF789B4CBA0 writes each
            // move with the result of mod 0, the factor of every run and
            // crouch speed (OwnerPrediction.md 11.3); 1.0 without a mover;
            // 0.0 for no entity or one the host can't find. Reinhardt,
            // Doomfist and Brigitte charge speeds (`007D` st95, `0D06`
            // st83/84, `0EE3` st100), all on the owner.
            "STU_ED807E07" if native_shape(config, class, Some("m_entity")) => {
                let entity = child(config, "m_entity");
                let entity = if entity.is_null() {
                    None
                } else {
                    to_entity(&self.config(entity, context, nested)?)
                };
                match entity {
                    Some(entity) if valid_entity(entity) => Value::Float(rounded(
                        context
                            .speed_scale(entity)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    )),
                    _ => Value::Float(0.0),
                }
            }
            // The steepest drop under points in an entity's frame
            // (`Eval_STU_6EDA44A7` 0x7FF789BF2870): `m_height` (unset 2.0,
            // 0x7FF78B52275C); each point of `m_BD631144` goes through
            // `m_entity`'s world matrix (0x7FF789C33B40: side, up, forward
            // rows and position; no entity: the point as given and the
            // origin as position), and the scene query (flags 0x1081, type
            // 10, 0x7FF78AA65110 on the script entity's scene) runs from it
            // down `m_height`. A hit gives the angle below the entity's
            // position, `asin((position.y − hit.y) / |hit − position|)`
            // in degrees ([`client_asin`], × 57.29578). The result is the
            // largest angle when more than half the points hit (0.5 at
            // 0x7FF78B505A48), else 90.0 (0x7FF78B542000). Reinhardt's and
            // Brigitte's barriers (`0095` rid57, `0EE3` rid86 ChaseVar
            // #502), on the owner, three points 2 m up and 0.75 to 1.5 m
            // ahead, 3 m down. Assumption: the query meets the map as a
            // ray without a ray profile does ([`Context::map_ray`]); what
            // flags 0x1081 and type 10 select was not read.
            "STU_6EDA44A7" if native_fields(config, &["m_entity", "m_BD631144", "m_height"]) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let height = child(config, "m_height");
                let height = if height.is_null() {
                    2.0
                } else {
                    to_float(&self.config(height, context, nested)?)
                };
                let entity = child(config, "m_entity");
                let entity = if entity.is_null() {
                    None
                } else {
                    to_entity(&self.config(entity, context, nested)?).filter(|e| valid_entity(*e))
                };
                let (position, yaw) = match entity {
                    Some(entity) => (
                        context.entity_position(entity).ok_or_else(unsupported)?,
                        context.entity_yaw(entity).ok_or_else(unsupported)?,
                    ),
                    None => ([0.0; 3], 0),
                };
                let side = aim([yaw.wrapping_add(QUARTER_TURN), 0]);
                let forward = aim([yaw, 0]);
                let mut rows = Vec::new();
                value_rows(
                    self.value(child(config, "m_BD631144"), context, nested)?,
                    &mut rows,
                )?;
                let mut hits = 0usize;
                let mut steepest = -10_000.0f32;
                for row in &rows {
                    let Value::Vector(local) = row else {
                        return Err(unsupported());
                    };
                    let start = if entity.is_some() {
                        std::array::from_fn(|axis| {
                            position[axis]
                                + local[0] * side[axis]
                                + local[2] * forward[axis]
                                + if axis == 1 { local[1] } else { 0.0 }
                        })
                    } else {
                        *local
                    };
                    let Some(hit) = context
                        .map_ray(start, [0.0, -1.0, 0.0], height)
                        .ok_or_else(unsupported)?
                    else {
                        continue;
                    };
                    let offset: [f64; 3] = std::array::from_fn(|axis| hit[axis] - position[axis]);
                    let length = offset.iter().map(|part| part * part).sum::<f64>().sqrt();
                    let sine = ((position[1] - hit[1]) / length) as f32;
                    hits += 1;
                    steepest = steepest.max(client_asin(sine) * 57.295_78);
                }
                let share = if rows.is_empty() {
                    0.0
                } else {
                    hits as f32 / rows.len() as f32
                };
                Value::Float(f64::from(if share > 0.5 { steepest } else { 90.0 }))
            }
            // Is `m_team` hostile to the script's entity (`Eval_STU_47703870`
            // 0x7FF789845DA0 → 0x7FF789B2CB00): the team (unset −1), the
            // entity's team (0x7FF789B2B570: component 25's filter bits 23
            // to 27; −1 none) and its filter bit 28 ([`hostile_to_team`]).
            // 0x7FF789B2CB00 first asks a per-entity override list (entity
            // `+40` object `+0x140`, slot 30; its last entry's team `+24`
            // decides) whose writer was not traced: no world keeps one, so
            // the rule below is the answer for an entity without overrides.
            // `0EC2` rid420/421 order the two teams' Control progress.
            "STU_47703870" if native_shape(config, class, Some("m_team")) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let team = child(config, "m_team");
                let team = if team.is_null() {
                    -1
                } else {
                    i64::from(to_int(&self.config(team, context, nested)?, -1) as i32)
                };
                let owner = to_entity(&context.owner())
                    .filter(|&entity| valid_entity(entity))
                    .ok_or_else(unsupported)?;
                let own = context.entity_team(owner).ok_or_else(unsupported)?;
                let flags = context.entity_filter_flags(owner).ok_or_else(unsupported)?;
                Value::Bool(hostile_to_team(team, own, flags))
            }
            // Does the current map support `m_gamemode`
            // (`Eval_STU_16753B11` 0x7FF789C03860): the mode is in the map
            // header's (`STU_02514B6A`) `m_supportedGamemodes` (+56, 16-byte
            // entries, GUID at +8), or is `0230:0045`; no mode, false.
            // Skirmish `0D76` rid37/39/112.
            "STU_16753B11" if native_shape(config, class, Some("m_gamemode")) => {
                const ALWAYS: u64 = 0x0230_0000_0000_0045;
                let mode = child(config, "m_gamemode");
                let mode = if mode.is_null() {
                    Value::Null
                } else {
                    self.config(mode, context, nested)?
                };
                match mode {
                    Value::Asset(ALWAYS) => Value::Bool(true),
                    Value::Asset(mode) if mode != 0 => Value::Bool(
                        context
                            .map_supports_mode(mode)
                            .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                    ),
                    Value::Asset(_) | Value::Null => Value::Bool(false),
                    _ => return Err(EvalError::UnsupportedConfig(class.into())),
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
                let facing = if class == "STU_2200A66D" {
                    aim(context.entity_facing(entity).ok_or_else(unsupported)?)
                } else {
                    aim([context.entity_yaw(entity).ok_or_else(unsupported)?, 0])
                };
                if class == "STU_C8C4C20D" {
                    let [x, _, z] = facing;
                    Value::Float(rounded(rounded(x.atan2(z)) * DEGREES))
                } else {
                    Value::Vector(facing)
                }
            }
            // A direction kept within a cone of another (Eval 0x7FF789BFB820):
            // `m_32B00625`'s yaw atan2(x, z) and pitch asin(−y) (both
            // normalized; unset or not a vector: (0, 0, 1)), each moved to
            // within `m_5023B9B0` (yaw) and `m_E26E5E09` (pitch) degrees of
            // `m_3661BC45`'s (differences wrapped to ±π, 0x7FF78A75AEC0),
            // and made a direction again (0x7FF78A7596E0: the forward row of
            // a pitch, then yaw, rotation). A zero vector gives (0, 0, 0).
            // Knockback and launch directions (`1513`, `010A`, `0B27`).
            "STU_0762B33A" => {
                let mut direction = |name: &str| -> Result<Option<[f64; 3]>, EvalError> {
                    let value = match self.config(child(config, name), context, nested)? {
                        Value::Vector(parts) => parts,
                        _ => [0.0, 0.0, 1.0],
                    };
                    let length = value.iter().map(|part| part * part).sum::<f64>().sqrt();
                    Ok((length * length >= 1.175_494_4e-36)
                        .then(|| value.map(|part| part / length)))
                };
                let (Some(from), Some(around)) =
                    (direction("m_32B00625")?, direction("m_3661BC45")?)
                else {
                    return Ok(Value::Vector([0.0; 3]));
                };
                let mut degrees = |name: &str| -> Result<f64, EvalError> {
                    Ok(to_float(&self.config(child(config, name), context, nested)?).to_radians())
                };
                let yaw_limit = degrees("m_5023B9B0")?;
                let pitch_limit = degrees("m_E26E5E09")?;
                let [yaw, pitch] = yaw_pitch(from);
                let [around_yaw, around_pitch] = yaw_pitch(around);
                let within = |angle: f64, around: f64, limit: f64| {
                    use std::f64::consts::{PI, TAU};
                    let difference = (around - angle + PI).rem_euclid(TAU) - PI;
                    if difference.abs() <= limit {
                        angle
                    } else if difference > 0.0 {
                        around - limit
                    } else {
                        around + limit
                    }
                };
                let yaw = within(yaw, around_yaw, yaw_limit);
                let pitch = within(pitch, around_pitch, pitch_limit);
                Value::Vector(forward(yaw, pitch).map(rounded))
            }
            // A direction from a yaw `m_7C831A8D` and a pitch `m_C0EDD7EC`
            // in degrees (Eval 0x7FF789BDC480: each read as a float, unset
            // 0, × 0.017453292 (0x7FF78B51FC98), then 0x7FF78A7596E0's
            // forward row, as `STU_0762B33A` makes it). Ashe's coach gun
            // (`1513` rid18) pushes her against it; Brigitte's `1248`
            // searches and `0622`'s created entities (a random yaw) use it.
            "STU_B113D362" if native_fields(config, &["m_7C831A8D", "m_C0EDD7EC"]) => {
                let mut radians = |name: &str| -> Result<f64, EvalError> {
                    let angle = match child(config, name) {
                        Json::Null => 0.0,
                        angle => to_float(&self.config(angle, context, nested)?),
                    };
                    Ok(rounded(angle * RADIANS))
                };
                let yaw = radians("m_7C831A8D")?;
                let pitch = radians("m_C0EDD7EC")?;
                Value::Vector(forward(yaw, pitch).map(rounded))
            }
            // A direction's yaw and pitch in degrees as the first two parts
            // of a vector (Eval 0x7FF789BE29B0, written by 0x7FF789E774D0):
            // `m_3ABD589C` read as a vector (default (0, 0, 1),
            // 0x7FF78C1517A0), normalized, (0, 0, 1) when its squared length
            // is under 1.1754944e-36 (0x7FF78B4F1860); yaw atan2(x, z) (0
            // when both are 0), pitch asin(clamp(−y, −1, 1)), each ×
            // 57.29578 (0x7FF78B53BD0C). Ashe's coach gun takes the yaw of
            // her facing (`1513` rid18).
            "STU_D0E0DA18" if native_shape(config, class, Some("m_3ABD589C")) => {
                let vector = match child(config, "m_3ABD589C") {
                    Json::Null => [0.0, 0.0, 1.0],
                    vector => match self.config(vector, context, nested)? {
                        Value::Vector(parts) => parts,
                        _ => return Err(EvalError::UnsupportedConfig(class.into())),
                    },
                };
                let squared: f64 = vector.iter().map(|part| part * part).sum();
                let direction = if squared >= 1.175_494_4e-36 {
                    vector.map(|part| part / squared.sqrt())
                } else {
                    [0.0, 0.0, 1.0]
                };
                let [yaw, pitch] = yaw_pitch(direction);
                Value::Vector([
                    rounded(rounded(yaw) * DEGREES),
                    rounded(rounded(pitch) * DEGREES),
                    0.0,
                ])
            }
            // A team's size (Eval 0x7FF789BF48D0 → 0x7FF789CADB40): the
            // game struct's entry for `m_team` (unset: -1), else the "any
            // team" entry 5. Elimination picks as many heroes as the larger
            // team (`0D0B`), Deathmatch counts half the players (`0F73`
            // team 4), a body asks whether its team is alone (`006D`).
            "STU_D289FA32" if native_shape(config, class, Some("m_team")) => {
                let team = match child(config, "m_team") {
                    Json::Null => -1,
                    team => to_int(&self.config(team, context, nested)?, -1),
                };
                Value::Integer(
                    context
                        .team_size(team)
                        .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                )
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
            // Whether a point is inside an entity's volume
            // (`Eval_STU_73E1C5F0` 0x7FF789BF7D20): it evaluates the point
            // `m_A8D12F28`, then each entity of `m_entity` (a value or an
            // array) through the host (vtable +0x168) and tests the point
            // against that entity's volume (0x7FF789CC4E40); true at the
            // first that holds it, false for none or no point. The spawn
            // room `0D62` asks it of its own entity for each hero its search
            // finds (where its feet are, `STU_8CBDD81C`).
            "STU_73E1C5F0" if native_fields(config, &["m_A8D12F28", "m_entity"]) => {
                let Value::Vector(point) =
                    self.config(child(config, "m_A8D12F28"), context, nested)?
                else {
                    return Ok(Value::Bool(false));
                };
                let entities = match self.config(child(config, "m_entity"), context, nested)? {
                    Value::Array(items) => items,
                    value => vec![value],
                };
                for entity in entities.iter().filter_map(to_entity) {
                    if !valid_entity(entity) {
                        continue;
                    }
                    if context
                        .in_entity_volume(entity, point)
                        .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?
                    {
                        return Ok(Value::Bool(true));
                    }
                }
                Value::Bool(false)
            }
            // `STU_0165541D` (`Eval_STU_0165541D` 0x7FF789BF3A40, which
            // looks up the entity's component 44): the Workshop actions'
            // body of a target player (Teleport `1D45`, Set Ability
            // Cooldown `2735`). Assumption: the possessed body, as
            // `STU_0AC15F78`; its other paths are not traced.
            "STU_F76C486C" | "STU_F7262B36" | "STU_0AC15F78" | "STU_DB742151" | "STU_0165541D" => {
                let (relation, operand) = match class {
                    "STU_F76C486C" => (EntityRelation::Owner, "m_D63CBEC2"),
                    "STU_F7262B36" => (EntityRelation::CurrentPossessor, "m_801B298F"),
                    "STU_DB742151" => (EntityRelation::Controller, "m_entity"),
                    _ => (EntityRelation::PossessedBody, "m_entity"),
                };
                if !native_shape(config, class, Some(operand)) {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                }
                // An entity, or the first of an array (as `to_entity` reads
                // it): a PlayScript's parameters are arrays (`0C72`'s
                // instigator `#40`).
                let Some(entity) =
                    to_entity(&self.config(child(config, operand), context, nested)?)
                else {
                    return Err(EvalError::UnsupportedConfig(class.into()));
                };
                // No entity relates to none (the client's lookups fail):
                // `0033` rid40 asks which body the owner's owner possesses,
                // and a body has no owner.
                if !valid_entity(entity) {
                    return Ok(Value::Entity(0));
                }
                let related = context
                    .related_entity(relation, entity)
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?;
                Value::Entity(related)
            }
            // A projectile's related entity (`Eval_STU_2B2E2F6D`
            // 0x7FF789BDFCC0, component 50 of `m_81A364DE`; BioticOrb.md):
            // kind `m_7BDB02D1` 0 is the last re-owner (a deflect), else the
            // `+0x240` override, else the shooter `+0x18`; 1 the override,
            // else the shooter; 4 the raw `+0x20`, the shooter at the
            // creation. The server re-owns no projectile and sets no
            // override, so all three are its shooter
            // ([`EntityRelation::Shooter`]). Kinds 2 and 3 (`+0x1C` from the
            // volley's state, `+0x244`) are not modelled. No entity, or one
            // that isn't a projectile, reads the null entity.
            "STU_2B2E2F6D" if native_fields(config, &["m_81A364DE", "m_7BDB02D1"]) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let kind = match child(config, "m_7BDB02D1") {
                    Json::Null => 0,
                    kind => kind.as_i64().ok_or_else(unsupported)?,
                };
                if !matches!(kind, 0 | 1 | 4) {
                    return Err(unsupported());
                }
                let entity =
                    to_entity(&self.config(child(config, "m_81A364DE"), context, nested)?)
                        .ok_or_else(unsupported)?;
                if !valid_entity(entity) {
                    return Ok(Value::Entity(0));
                }
                let shooter = context
                    .related_entity(EntityRelation::Shooter, entity)
                    .ok_or_else(unsupported)?;
                Value::Entity(shooter)
            }
            // Whether something of `m_entity` keeps it from `m_82B6345B`
            // (`Eval_STU_7C005F8F` 0x7FF789BDBF90 → `0x7FF789B2DB30`): false
            // when either entity doesn't exist, and for an entity whose
            // filter word (component 25 `+0x58`) has neither `0x100` nor
            // `0x200`, which no body's has (`vm/targets.rs` `filter_word`).
            // With either bit the client tests the distance between the two
            // against the `STU_33A46033` states on the first and the
            // entities' hostility: not modelled. The damaging Biotic Orb's
            // search `1100` st0 leaves out the bodies it is true for.
            "STU_7C005F8F" if native_fields(config, &["m_entity", "m_82B6345B"]) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let mut entity = |name| -> Result<Option<u32>, EvalError> {
                    Ok(
                        to_entity(&self.config(child(config, name), context, nested)?)
                            .filter(|&entity| valid_entity(entity)),
                    )
                };
                let (Some(first), Some(second)) = (entity("m_entity")?, entity("m_82B6345B")?)
                else {
                    return Ok(Value::Bool(false));
                };
                if [first, second]
                    .into_iter()
                    .any(|entity| context.entity_exists(entity) == Some(false))
                {
                    return Ok(Value::Bool(false));
                }
                match context.entity_filter_flags(first) {
                    Some(word) if word & 0x300 == 0 => Value::Bool(false),
                    _ => return Err(unsupported()),
                }
            }
            // The player entity of `m_entity`, as the client's
            // `Eval_STU_0340A42A` (0x7FF789C00660): the entity itself when it
            // is a player entity (component 74, `sub_7FF789C1CAE0`), else
            // its owner (component 72 `+0xD8`) when that is one. The
            // client's last two tries (component 43, then a list from the
            // world's slot `+576`) are not modelled: an entity they would
            // answer is unsupported. The Biotic Field's `0258` sends `0EEB`
            // and tracks its stat to it with its owner, and its aura `0B3C`
            // st2 names it with the field. Assumption: a controller (it
            // possesses a body, component 44) is a player entity.
            "STU_0340A42A" if native_shape(config, class, Some("m_entity")) => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let Some(entity) =
                    to_entity(&self.config(child(config, "m_entity"), context, nested)?)
                else {
                    return Err(unsupported());
                };
                if !valid_entity(entity) {
                    return Ok(Value::Entity(0));
                }
                fn player(context: &mut impl Context, entity: u32) -> bool {
                    context.related_entity(EntityRelation::Controller, entity) == Some(entity)
                }
                if player(context, entity) {
                    Value::Entity(entity)
                } else {
                    match context.related_entity(EntityRelation::Owner, entity) {
                        Some(owner) if valid_entity(owner) && player(context, owner) => {
                            Value::Entity(owner)
                        }
                        _ => return Err(unsupported()),
                    }
                }
            }
            // The body a controller possesses: `Eval_STU_00707E1E`
            // (0x7FF789BF7660) takes the entity's component 44 (a
            // controller's) and answers its possessed entity
            // (`sub_7FF789AE4A90`); an entity without one answers none.
            // `STU_69DBE90D` (0x7FF789C02EE0, a list) answers the same
            // entity, then the controller's other one (`sub_7FF789AE56F0`)
            // when that differs, none without component 44. The console
            // `265F` sends `1214` from a player's scope 126 (on his
            // controller) to `STU_69DBE90D(owner())`, which its scope 12 (on
            // his body) takes, and reads `STU_00707E1E(owner()).#25456`,
            // which scope 12 writes on the body. Assumption: the second
            // entity is the body as well (the server's controllers possess
            // their body and view it).
            "STU_00707E1E" | "STU_69DBE90D"
                if native_shape(
                    config,
                    class,
                    Some(if class == "STU_00707E1E" {
                        "m_00903303"
                    } else {
                        "m_entity"
                    }),
                ) =>
            {
                let operand = if class == "STU_00707E1E" {
                    "m_00903303"
                } else {
                    "m_entity"
                };
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let entity = to_entity(&self.config(child(config, operand), context, nested)?)
                    .filter(|&entity| valid_entity(entity));
                let body = match entity {
                    Some(entity)
                        if context.related_entity(EntityRelation::Controller, entity)
                            == Some(entity) =>
                    {
                        Some(
                            context
                                .related_entity(EntityRelation::PossessedBody, entity)
                                .ok_or_else(unsupported)?,
                        )
                    }
                    _ => None,
                }
                .filter(|&body| valid_entity(body));
                if class == "STU_00707E1E" {
                    Value::Entity(body.unwrap_or(0))
                } else {
                    Value::Array(body.map(Value::Entity).into_iter().collect())
                }
            }
            // "`m_target` is not an enemy of `m_source`", as the client's
            // `Eval_STU_53F4CA65` (0x7FF789BFAD90 → 0x7FF789B2C0E0, the negated
            // enemy test 0x7FF789B2CF60; HealthAuras.md, "Client tools pass
            // (2026-11-16)"): with `m_AFBBE9C3` the source is ignored and the
            // answer is "the target's team is not 1"; otherwise a missing
            // entity is an ally only of itself, each side counts as its owner
            // when it has one (a projectile as its thrower), the same entity
            // is an ally, and teams -1 and 4 (free-for-all) make everyone else
            // an enemy. `0C71` plays Biotic Grenade's ally aura `0B99` when
            // true, its enemy aura `0C72` when false. Not modelled: the
            // client's spectator test (0x7FF789B2F000) on both sides. An
            // entity whose owner the world doesn't know counts as ownerless
            // (assumption).
            //
            // `STU_A58C1E63` ("`m_target` is an enemy of `m_source`") is the
            // same enemy test called directly: `Eval_STU_A58C1E63`
            // (0x7FF789BF5020) looks up both entities' records and returns
            // `sub_7FF789B2CF60(target, source, m_AFBBE9C3)`, so it is the
            // negation of the ally answer above. A value that is no entity
            // reads as the invalid entity there (dword_7FF78C151288).
            "STU_53F4CA65" | "STU_A58C1E63"
                if native_fields(config, &["m_target", "m_source", "m_AFBBE9C3"]) =>
            {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let enemy = class == "STU_A58C1E63";
                let flag = child(config, "m_AFBBE9C3");
                let target_only = !flag.is_null() && truthy(&self.config(flag, context, nested)?);
                let target = to_entity(&self.config(child(config, "m_target"), context, nested)?);
                let ally = 'ally: {
                    if target_only {
                        let team = match target.filter(|&entity| valid_entity(entity)) {
                            Some(target) => context.entity_team(target).ok_or_else(unsupported)?,
                            None => -1,
                        };
                        break 'ally team != 1;
                    }
                    let source =
                        to_entity(&self.config(child(config, "m_source"), context, nested)?);
                    let (target, source) = match (target, source) {
                        (Some(target), Some(source)) => (target, source),
                        // A value that is no entity at all is no ally.
                        _ if !enemy => break 'ally false,
                        (target, source) => (target.unwrap_or(0), source.unwrap_or(0)),
                    };
                    if !valid_entity(target) || !valid_entity(source) {
                        break 'ally target == source;
                    }
                    let mut owner = |entity: u32| {
                        context
                            .related_entity(EntityRelation::Owner, entity)
                            .filter(|&owner| valid_entity(owner))
                            .unwrap_or(entity)
                    };
                    let (target, source) = (owner(target), owner(source));
                    if target == source {
                        true
                    } else {
                        let target = context.entity_team(target).ok_or_else(unsupported)?;
                        let source = context.entity_team(source).ok_or_else(unsupported)?;
                        let free_for_all = |team| team == -1 || team == 4;
                        !free_for_all(target) && !free_for_all(source) && target == source
                    }
                };
                Value::Bool(ally != enemy)
            }
            // Whether the entity has component 92 (an AI controller). A
            // world's own answer comes first; no entity has none.
            "STU_D3EB8D87" if native_shape(config, class, Some("m_00903303")) => {
                if let Some(value) = context.native(config) {
                    value
                } else {
                    match self.config(child(config, "m_00903303"), context, nested)? {
                        Value::Entity(entity) if !valid_entity(entity) => Value::Bool(false),
                        Value::Entity(entity) => Value::Bool(
                            context
                                .controller_is_ai(entity)
                                .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                        ),
                        _ => return Err(EvalError::UnsupportedConfig(class.into())),
                    }
                }
            }
            // An AI parameter (`STU_4049CF77`; no client code, BotDamage.md
            // "Client tools pass"): the value of identifier `m_F00C2F3D`
            // in the difficulty profile of the AI that possesses
            // `m_C5D0FA56` (none: the owner), else the authored fallback
            // `m_A53100D5`. The weapons ask it behind `STU_D3EB8D87` (an AI
            // possessor): Soldier: 76's rifle `0254` st10 deals `1961` (4,
            // 10 or 20 by difficulty), fallback 10. Assumption: the
            // fallback is the answer where the AI's profile has no such
            // value (every graph's fallback is its hero's Medium value,
            // AiDefinitions.md).
            "STU_4049CF77"
                if native_fields(config, &["m_F00C2F3D", "m_A53100D5", "m_C5D0FA56"]) =>
            {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let Value::Asset(identifier) =
                    self.config(child(config, "m_F00C2F3D"), context, nested)?
                else {
                    return Err(unsupported());
                };
                let target = child(config, "m_C5D0FA56");
                let entity = if target.is_null() {
                    context.owner()
                } else {
                    self.config(target, context, nested)?
                };
                let entity = to_entity(&entity)
                    .filter(|&entity| valid_entity(entity))
                    .ok_or_else(unsupported)?;
                match context
                    .ai_parameter(entity, identifier)
                    .ok_or_else(unsupported)?
                {
                    Some(value) => Value::Float(value),
                    None => self.config(child(config, "m_A53100D5"), context, nested)?,
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
            // A pool kind's current (783B4CF0, F1C45C59) or maximum
            // (68883EAC, 005D99E6) health: kind 0 the health pools, 1 armour,
            // 2 shields. F1C45C59 and 005D99E6 are the client's sums over
            // component 51's pools of the kind (`Eval_STU_F1C45C59`
            // 0x7FF789C055D0 → 0x7FF7894CFBA0, `Eval_STU_005D99E6`
            // 0x7FF789BDBD10 → 0x7FF7894D01F0; no entity reads 0; BotVoice.md,
            // "Client tools pass (2026-11-16)"). `05AD` rid147 rates the
            // sender of Need Healing's `0307` by them. `m_7BE02F89` (a named
            // pool, read as an identifier: 0x7FF789E75540) is not modelled;
            // an identifier literal of none (Zenyatta's `0176`) reads 0, as
            // when it is unset (`Eval_STU_783B4CF0` 0x7FF789BEFCC0).
            "STU_783B4CF0" | "STU_68883EAC" | "STU_F1C45C59" | "STU_005D99E6" => {
                let unsupported = || EvalError::UnsupportedConfig(class.into());
                let pool = child(config, "m_7BE02F89");
                let no_pool = pool.is_null()
                    || (native_fields(pool, &["m_identifier", "m_resourceKey"])
                        && self::class(pool) == Some("STUConfigVarIdentifier")
                        && child(pool, "m_identifier").is_null()
                        && child(pool, "m_resourceKey").is_null());
                if !native_fields(config, &["m_entity", "m_7E8B666C", "m_7BE02F89"]) || !no_pool {
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
                let entity = to_entity(&self.config(child(config, "m_entity"), context, nested)?);
                let summed = matches!(class, "STU_F1C45C59" | "STU_005D99E6");
                let entity = match entity {
                    Some(entity) if summed && !valid_entity(entity) => {
                        return Ok(Value::Float(0.0));
                    }
                    Some(entity) => entity,
                    None => return Err(unsupported()),
                };
                let maximum = matches!(class, "STU_68883EAC" | "STU_005D99E6");
                let health = context
                    .body_health(entity, Some(kind), maximum)
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
            // GetGameRulesetValue (`Eval_STU_3832D36C` 0x7FF789BDFAC0,
            // GameSettings.md 15.2): `m_value` first, and a value wins (an
            // entity variable of the key's id); else the ruleset default of
            // the key, `m_value`'s variable: a dynamic one, or an entity's
            // member (`mode.#29450`, 58 uses: the Practice Range pad `26F8`
            // and console `265F` read the Training Partner setting so).
            "STU_3832D36C" => {
                let found = self.config(value, context, nested)?;
                let key = match (dynamic(value)?, member_identifier(value)?) {
                    (Some(variable), _) => Some(variable.id),
                    (None, id) => id,
                };
                match key {
                    Some(key) if found == Value::Null => context.ruleset(key),
                    _ => found,
                }
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
            // Not a fresh random number: the client returns the mode
            // entity's component 114 mirror field 9 `m_CFF0C4E3` (an f32 at
            // component `+0x170`, read from the first entry of the
            // component-114 pool `sub_7FF789C1A0A0`), the same every
            // evaluation, and 0.5 when there is no mode entity
            // (`Eval_STU_B182C5C9` 0x7FF789BDDF50; ModeGraphGaps.md and
            // Payloads.md, "Client tools pass 2 (2026-11-16)"). The server
            // draws one per match in [0, 1) and sends it in the mode
            // entity's create (`orpheus_sim::scene::mode_create`); every
            // world answers that value. Its uses: `0F4A` uid 1299 keeps it
            // as #29093, `1225` uid 363 scales the payload's speed by
            // `1 - v + 2v·x` (v = 0.1), `0C8E` uid 136 takes `(x - 0.5)·2`.
            "STU_B182C5C9" if native_shape(config, class, None) => Value::Float(
                context
                    .mode_random()
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
            ),
            // An entry of the client's uniform random table between two
            // bounds (`Eval_STU_4EC41E8A` 0x7FF789BE0DD0): `m_index`,
            // `m_1C002272` and `m_52CAFE20` as ints (0x7FF789E75260, unset
            // 0), then 0x7FF78A75B220(index, least, greatest + 1). Control
            // `0EC2` rid92/94/241 pick the match's stages with it.
            "STU_4EC41E8A" if native_fields(config, &["m_index", "m_1C002272", "m_52CAFE20"]) => {
                let mut int = |field: &str| -> Result<i32, EvalError> {
                    Ok(match child(config, field) {
                        Json::Null => 0,
                        value => to_int(&self.config(value, context, nested)?, 0) as i32,
                    })
                };
                let index = int("m_index")?;
                let (first, second) = (int("m_1C002272")?, int("m_52CAFE20")?);
                let (low, high) = (first.min(second), first.max(second));
                Value::Integer(i64::from(crate::random_tables::uniform_pick(
                    index,
                    low,
                    high.wrapping_add(1),
                )))
            }
            // Server-only (no registration of its own: the client's base
            // evaluate gives no value). Its two uses, Control `0EC2` rid92
            // (`STU_4EC41E8A(m_index = CBF6FF14())`, the first stage) and
            // rid239 (`CBF6FF14() + #7901`, the next round's), index the
            // random table with it: the match's random index (assumption),
            // drawn once per match by the server.
            "STU_CBF6FF14" if native_shape(config, class, None) => Value::Integer(
                context
                    .match_seed()
                    .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
            ),
            // Whether an entity holds a logical button (`Eval_STU_A2A35433`
            // 0x7FF789BDF4A0): the entity `m_entity` (no value: the invalid
            // entity) through the host's entity table (not found: no
            // value), its statescript component (47), then the button
            // `m_logicalButton` (ToInt, unset 0) in that component's held
            // list (0x7FF78AA9D940: `+0x440` items, `+0x448` count).
            // Baptiste's `1983` rid39 asks it of his own jump.
            "STU_A2A35433" if native_fields(config, &["m_entity", "m_logicalButton"]) => {
                let entity = match child(config, "m_entity") {
                    Json::Null => None,
                    entity => to_entity(&self.config(entity, context, nested)?),
                };
                let Some(entity) = entity.filter(|&entity| valid_entity(entity)) else {
                    return Ok(Value::Null);
                };
                let button = match child(config, "m_logicalButton") {
                    Json::Null => 0,
                    button => to_int(&self.config(button, context, nested)?, 0),
                };
                Value::Bool(
                    context
                        .button_held(entity, button)
                        .ok_or_else(|| EvalError::UnsupportedConfig(class.into()))?,
                )
            }
            // The least of the values of `m_814F8EC3` (arrays opened), each as
            // an f32, a value that is no number counting as the default
            // `m_A53100D5` (itself 0 when it is no number); no value gives the
            // default. Client evaluator 1B76C90 (leafall903/ORPHEUS (MIT),
            // `play.minimum_value`). `0CBB` st40/st41 read the lowest of
            // #12114/#12115 with the default 9999.
            "STU_17330930" if native_fields(config, &["m_814F8EC3", "m_A53100D5"]) => {
                let default = match self.config(child(config, "m_A53100D5"), context, nested)? {
                    value @ (Value::Integer(_) | Value::Float(_) | Value::Bool(_)) => {
                        rounded(to_float(&value))
                    }
                    _ => 0.0,
                };
                let mut rows = Vec::new();
                value_rows(
                    self.value(child(config, "m_814F8EC3"), context, nested)?,
                    &mut rows,
                )?;
                let least = rows
                    .iter()
                    .map(|row| match row {
                        Value::Integer(_) | Value::Float(_) | Value::Bool(_) => {
                            rounded(to_float(row))
                        }
                        _ => default,
                    })
                    .reduce(f64::min);
                Value::Float(least.unwrap_or(default))
            }
            // A scene tag query (`STU_EB216A0B`, a `STU_1361E674` scene
            // query with the fields of `STU_160F2578`): its entities
            // ([`Evaluator::tag_query`]). `0F4A` gathers the spawn rooms of
            // the objective's two tags so (node 162, #11023) and the spawn
            // doors by tag `04B3` (node 310, #12750). Its evaluator
            // (0x7FF78AA881A0) was not read; the fields and uses are those
            // of `STU_160F2578`, whose query the client's code shows. The
            // VM reads the fields of that state and of the action
            // `STU_42578342` as this config (`vm/entities.rs`).
            "STU_EB216A0B" | "STU_160F2578" | "STU_42578342" => Value::Array(
                self.tag_query(config, context, nested)?
                    .into_iter()
                    .map(Value::Entity)
                    .collect(),
            ),
            // The first entity of the same query (TrainingPartner.md: "first
            // filter match"; the Practice Range console `265F` finds its pad
            // by tag `09B7`), none when nothing matches.
            "STU_B3E67A36" => Value::Entity(
                self.tag_query(config, context, nested)?
                    .first()
                    .copied()
                    .unwrap_or(0),
            ),
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
                    "STU_CC623180" => Value::Bool(true), // IsHUDEnabled
                    // CustomGameLogicEnabled: the client's console variable
                    // 0x027006C2 (object 0x7FF78BD46728), 1 by default and in
                    // the in-game dump (GameSettings.md, "Client tools pass
                    // (2026-11-16)"). Its 28 uses, each a `Condition_Bool`
                    // right after a graph's entry, push #31254e by a networked
                    // StateStack that no graph reads.
                    "STU_98E9109B" => Value::Bool(true),
                    "STU_5E75FF05" => Value::Integer(0), // ReticleType
                    "STU_C76C63AF" => Value::Float(0.0), // ReticleNumericSetting
                    // An engine console variable by its hash `m_74A2248F`
                    // (getter table 0x7FF789C04A60), as the in-game dump has
                    // them (ModeGraphGaps.md, "Client tools pass
                    // (2026-11-16)"; `python -m tools.owsearch cvar`, value
                    // and default the same): 0xE1C1E402 an int, 128 (`0CBB`
                    // rid104), 0x0FE1E03E a bool, false (`0CBB` rid122);
                    // Reaper's server-only `0075` rid25/26: 0xD645EF78 a bool
                    // (#48, getter 0x7FF789BFA5C0), false, and 0xA3E1FE20 a
                    // double (#49, getter 0x7FF789C05E20 writes it as a
                    // float), 1.0. The others' values are not known.
                    "STU_A753DC79" => {
                        match child(config, "m_74A2248F").as_i64().map(|hash| hash as u32) {
                            Some(0xE1C1_E402) => Value::Integer(128),
                            Some(0x0FE1_E03E | 0xD645_EF78) => Value::Bool(false),
                            Some(0xA3E1_FE20) => Value::Float(1.0),
                            _ => return Err(EvalError::UnsupportedConfig(class.into())),
                        }
                    }
                    _ => return Err(EvalError::UnsupportedConfig(class.into())),
                }
            }
        };
        Ok(result)
    }

    /// `STU_AC829876`: whether `m_entity` passes `m_filter`, as the
    /// client's `ScriptHost_EntityPassesFilter` (0x7FF789C1DA20): the masks
    /// against the entity's filter word and its relation to the evaluating
    /// entity (`Filter_TestTeamMasks`, [`FilterMasks`]; the searcher is the
    /// script's owner, hostile as `vm/targets.rs` tells), then the tag
    /// lists. Unsupported when a fact it needs is unknown, or nothing about
    /// the entity is known at all.
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
        let masks = FilterMasks::read(filter).ok_or_else(unsupported)?;
        let Value::Entity(entity) = self.config(child(config, "m_entity"), context, depth)? else {
            return Err(unsupported());
        };
        if !valid_entity(entity) {
            return Ok(Value::Bool(false));
        }
        passes_filter(filter, &masks, entity, context).map(Value::Bool)
    }
}

/// Whether `entity` passes a filter whose masks are `masks`, as
/// [`Evaluator::entity_filter`] tells. The client also requires the
/// filter's `m_condition` (`Filter_TestEntityCondition` 0x7FF78AA9D970,
/// evaluated with the entity pushed as the candidate parameter); the
/// evaluator doesn't bind one, so a set condition leaves an entity the
/// masks and tag lists pass unanswered, while one they reject fails
/// ([`passes_filter_as_candidate`] binds it).
fn passes_filter(
    filter: &Json,
    masks: &FilterMasks,
    entity: u32,
    context: &mut impl Context,
) -> Result<bool, EvalError> {
    match filter_facts(filter, masks, entity, context)? {
        Some(false) => Ok(false),
        Some(true) if child(filter, "m_condition").is_null() => Ok(true),
        _ => Err(EvalError::UnsupportedConfig("STU_AC829876".into())),
    }
}

/// Whether `entity` passes `filter`, its condition included, as the
/// client's `ScriptHost_EntityPassesFilter` (0x7FF789C1DA20) tests it:
/// `context` must answer every parameter with `entity`, the candidate
/// `Filter_TestEntityCondition` pushes. A filter without a condition
/// passes only on a known fact ([`passes_filter`]).
pub(crate) fn passes_filter_as_candidate(
    filter: &Json,
    entity: u32,
    context: &mut impl Context,
) -> Result<bool, EvalError> {
    let unsupported = || EvalError::UnsupportedConfig("STU_AC829876".into());
    let masks = FilterMasks::read(filter).ok_or_else(unsupported)?;
    if !valid_entity(entity) {
        return Ok(false);
    }
    let known = match filter_facts(filter, &masks, entity, context)? {
        Some(false) => return Ok(false),
        known => known,
    };
    let condition = child(filter, "m_condition");
    if condition.is_null() {
        return known.ok_or_else(unsupported);
    }
    Ok(truthy(&evaluate(condition, context)?))
}

/// The masks and tag lists of `filter` on `entity`: Some(false) rejects
/// it, Some(true) passes it on a known fact, None passes it with nothing
/// known about it. Unsupported when a fact they need is unknown and none
/// rejects it.
fn filter_facts(
    filter: &Json,
    masks: &FilterMasks,
    entity: u32,
    context: &mut impl Context,
) -> Result<Option<bool>, EvalError> {
    let flags = context.entity_filter_flags(entity);
    let mut has_fact = flags.is_some();
    let mut unknown = masks.read_word() && flags.is_none();
    let (friendly, searcher) = if masks.relations() {
        let owner = to_entity(&context.owner());
        if owner == Some(entity) {
            has_fact = true;
            (true, true)
        } else {
            let teams = owner.and_then(|owner| context.entity_team(owner));
            match (teams, context.entity_team(entity)) {
                (Some(own), Some(team)) => {
                    has_fact = true;
                    // Teams -1 and 4 (a free-for-all body) are hostile to
                    // every other entity.
                    let hostile = own != team || matches!(own, -1 | 4);
                    (!hostile, false)
                }
                _ => {
                    unknown = true;
                    (true, false)
                }
            }
        }
    } else {
        (true, false)
    };
    if !unknown && !masks.passes(flags.unwrap_or(0), friendly, searcher) {
        return Ok(Some(false));
    }
    // The client's filter tag test (0x7FF789C1EC70 → 0x7FF789CC2FD0, the
    // rule of 0x7FF789CC5310): any of `m_9C7BD8B8`, all of `m_B486603E`,
    // none of `m_8B2A4DF3`.
    for (field, condition) in [
        ("m_9C7BD8B8", TagCondition::Any),
        ("m_B486603E", TagCondition::All),
        ("m_8B2A4DF3", TagCondition::Exclude),
    ] {
        match check_tag_set(child(filter, field), condition, entity, context)? {
            TagCheck::Unused => (),
            TagCheck::Accept => has_fact = true,
            TagCheck::Reject => return Ok(Some(false)),
            TagCheck::Unknown => unknown = true,
        }
    }
    if unknown {
        return Err(EvalError::UnsupportedConfig("STU_AC829876".into()));
    }
    Ok(has_fact.then_some(true))
}

impl Evaluator {
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
        self.run(Expression::read(config)?, context, depth)
    }

    fn run(
        &mut self,
        parts: Expression,
        context: &mut impl Context,
        depth: usize,
    ) -> Result<Value, EvalError> {
        let Expression {
            code,
            dynamics,
            floats,
            configs,
        } = parts;
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
                // An entity's Workshop variable: the index on top (an
                // integer clamped to 0..=127) under the entity
                // ([`workshop_variable`]). The client reads it through the
                // entity's script host (case 0x48 of 0x7FF78AA91210); in a
                // variable reference with only the entity below, it names
                // the variable and leaves the entity ([`workshop_reference`]).
                72 => {
                    let id = workshop_variable(to_int(&pop(&mut stack, pc)?, 0));
                    if let Some(reference) = self.reference.as_mut().filter(|_| stack.len() == 1) {
                        *reference = Some(id);
                        pc = next;
                        continue;
                    }
                    let entity = pop(&mut stack, pc)?;
                    entity_member(&entity, &Value::Integer(i64::from(id)), context)
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
