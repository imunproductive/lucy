//! The Workshop rules as the client stores them (`PBCGLogic`, Workshop.md
//! §3.1), typed: rules with their event, conditions and actions, each
//! parameter a literal or a value tree. The text form (`crate::text`) and
//! the binary form (`crate::binary`) both produce this, normalised as the
//! client's normaliser (`sub_7FF789C9F450`) leaves it.

use crate::catalog::{Catalog, Id, Param, ParamKind};

/// Value identifiers the forms and the engine treat specially.
pub mod ids {
    use crate::catalog::Id;

    pub const NUMBER: Id = 0x4876;
    pub const FALSE: Id = 0x4877;
    pub const TRUE: Id = 0x4878;
    pub const NULL: Id = 0x4E63;
    pub const TEAM: Id = 0x49C7;
    pub const GLOBAL_VARIABLE: Id = 0x49CB;
    pub const PLAYER_VARIABLE: Id = 0x49CF;
    pub const VALUE_IN_ARRAY: Id = 0x4DCC;
    pub const AND: Id = 0x4B74;
    pub const OR: Id = 0x4B75;
    pub const NOT: Id = 0x4B76;
    pub const COMPARE: Id = 0x4B77;
    pub const ADD: Id = 0x55B1;
    pub const SUBTRACT: Id = 0x55B2;
    pub const MULTIPLY: Id = 0x55B4;
    pub const DIVIDE: Id = 0x55B6;
    pub const MODULO: Id = 0x55B7;
    pub const RAISE_TO_POWER: Id = 0x55BA;
    pub const IF_THEN_ELSE: Id = 0x7555;
    pub const BUTTON: Id = 0x74D2;
    pub const COLOR: Id = 0x7F8D;
    pub const CUSTOM_STRING: Id = 0x5C14;
    pub const STRING: Id = 0x519B;
    pub const ARRAY: Id = 0x4FDA;
}

/// Action indexes the text form writes as assignments and the engine runs
/// itself (`sub_7FF789CA7070`, the catalog order of `054/0155`).
pub mod actions {
    pub const SET_GLOBAL_VARIABLE: u16 = 0;
    pub const SET_PLAYER_VARIABLE: u16 = 1;
    pub const MODIFY_GLOBAL_VARIABLE: u16 = 2;
    pub const MODIFY_PLAYER_VARIABLE: u16 = 3;
    pub const WAIT: u16 = 4;
    pub const LOOP: u16 = 6;
    pub const LOOP_IF_CONDITION_IS_TRUE: u16 = 7;
    pub const LOOP_IF: u16 = 88;
    pub const LOOP_IF_CONDITION_IS_FALSE: u16 = 89;
    pub const ABORT: u16 = 90;
    pub const ABORT_IF: u16 = 91;
    pub const ABORT_IF_CONDITION_IS_TRUE: u16 = 92;
    pub const ABORT_IF_CONDITION_IS_FALSE: u16 = 93;
    pub const SKIP: u16 = 94;
    pub const SKIP_IF: u16 = 95;
    pub const SET_GLOBAL_VARIABLE_AT_INDEX: u16 = 108;
    pub const SET_PLAYER_VARIABLE_AT_INDEX: u16 = 109;
    pub const MODIFY_PLAYER_VARIABLE_AT_INDEX: u16 = 124;
    pub const MODIFY_GLOBAL_VARIABLE_AT_INDEX: u16 = 125;
    pub const IF: u16 = 136;
    pub const ELSE_IF: u16 = 137;
    pub const ELSE: u16 = 138;
    pub const WHILE: u16 = 139;
    pub const END: u16 = 140;
    pub const FOR_GLOBAL_VARIABLE: u16 = 144;
    pub const FOR_PLAYER_VARIABLE: u16 = 145;
    pub const CALL_SUBROUTINE: u16 = 146;
    pub const START_RULE: u16 = 147;
    pub const BREAK: u16 = 151;
    pub const CONTINUE: u16 = 152;
    pub const WAIT_UNTIL: u16 = 200;
}

/// Event types (`054/0154 m_targets` order).
pub mod events {
    pub const ONGOING_GLOBAL: u8 = 0;
    pub const ONGOING_EACH_PLAYER: u8 = 1;
    pub const PLAYER_EARNED_ELIMINATION: u8 = 2;
    pub const PLAYER_DEALT_FINAL_BLOW: u8 = 3;
    pub const PLAYER_DEALT_DAMAGE: u8 = 4;
    pub const PLAYER_TOOK_DAMAGE: u8 = 5;
    pub const PLAYER_DIED: u8 = 6;
    pub const PLAYER_DEALT_HEALING: u8 = 7;
    pub const PLAYER_RECEIVED_HEALING: u8 = 8;
    pub const PLAYER_JOINED_MATCH: u8 = 9;
    pub const PLAYER_LEFT_MATCH: u8 = 10;
    pub const SUBROUTINE: u8 = 11;
    pub const PLAYER_DEALT_KNOCKBACK: u8 = 12;
    pub const PLAYER_RECEIVED_KNOCKBACK: u8 = 13;
    /// The event count; the validator refuses a type at or above it.
    pub const COUNT: u8 = 14;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Compare {
    /// The operator's code (`sub_7FF789CA4D10`).
    pub fn from_code(code: i64) -> Option<Self> {
        Some(match code {
            0 => Self::Eq,
            1 => Self::Ne,
            2 => Self::Lt,
            3 => Self::Le,
            4 => Self::Gt,
            5 => Self::Ge,
            _ => return None,
        })
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// One parameter of an action, a condition or a value.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Value(Value),
    /// 0 All Teams, 1 Team 1, 2 Team 2.
    Team(u8),
    Hero(u64),
    Bool(bool),
    Int(i64),
    Number(f32),
    Variable(u8),
    CustomString(String),
    Enum(Id),
    PredefinedString(u16),
    Compare(Compare),
    Array(Vec<Value>),
    Map(u32),
    GameMode(u64),
    Subroutine(u8),
    Button(Id),
    Flags(u64),
    /// 0 Ignore Condition, 1 Abort When False, 2 Restart When True.
    WaitBehavior(u8),
}

/// A value tree: a catalog value and its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub id: Id,
    pub args: Vec<Arg>,
}

impl Value {
    pub fn new(id: Id, args: Vec<Arg>) -> Self {
        Self { id, args }
    }

    pub fn number(number: f32) -> Self {
        Self::new(ids::NUMBER, vec![Arg::Number(number)])
    }
}

/// A rule's event: its type and, for player events, the team and player
/// filters; for a subroutine rule, the subroutine (`slot`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: u8,
    /// 0 All, 1 Team 1, 2 Team 2.
    pub team: u8,
    /// The slot filter, or the subroutine of a subroutine rule; -1 none.
    pub slot: i32,
    /// The hero filter, 0 none.
    pub hero: u64,
}

impl Event {
    pub fn global() -> Self {
        Self {
            kind: events::ONGOING_GLOBAL,
            team: 0,
            slot: -1,
            hero: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub left: Arg,
    pub op: Compare,
    pub right: Arg,
    pub invert: bool,
    pub disabled: bool,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub index: u16,
    pub args: Vec<Arg>,
    pub disabled: bool,
    pub comment: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub name: String,
    pub disabled: bool,
    pub event: Event,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
}

/// A Workshop setting's value, from the text's `workshop` block or the
/// binary form's user setting instances.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkshopSetting {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Logic {
    pub rules: Vec<Rule>,
    /// Variable and subroutine names by index; an empty name is unnamed.
    pub global_names: Vec<String>,
    pub player_names: Vec<String>,
    pub subroutine_names: Vec<String>,
    pub extensions: Vec<u32>,
    pub workshop_settings: Vec<WorkshopSetting>,
}

impl Logic {
    /// Give every action, condition and value the parameters the catalog
    /// lists, as the client's normaliser does (`sub_7FF789CA02B0`: missing
    /// trailing parameters get their defaults) and rewrite bare Button and
    /// Color constants into their wrapper values (`sub_7FF789C9F790`,
    /// `sub_7FF789C9FA10`).
    pub fn normalise(&mut self, catalog: &Catalog) {
        for rule in &mut self.rules {
            if rule.event.kind >= events::COUNT {
                continue;
            }
            for condition in &mut rule.conditions {
                for arg in [&mut condition.left, &mut condition.right] {
                    normalise_arg(catalog, arg, condition_param(catalog));
                }
            }
            for action in &mut rule.actions {
                if let Some(entry) = catalog.action(action.index) {
                    normalise_args(catalog, &mut action.args, &entry.params);
                }
            }
        }
    }
}

fn condition_param(catalog: &Catalog) -> Option<&Param> {
    catalog.conditions.first()?.params.first()
}

fn normalise_args(catalog: &Catalog, args: &mut Vec<Arg>, params: &[Param]) {
    for (arg, param) in args.iter_mut().zip(params) {
        normalise_arg(catalog, arg, Some(param));
    }
    for param in params.iter().skip(args.len()) {
        args.push(default_arg(catalog, param));
    }
}

fn normalise_arg(catalog: &Catalog, arg: &mut Arg, param: Option<&Param>) {
    match arg {
        Arg::Value(value) => {
            if let Some(param) = param {
                wrap_constant(value, param);
            }
            normalise_value(catalog, value);
        }
        Arg::Array(values) => values
            .iter_mut()
            .for_each(|value| normalise_value(catalog, value)),
        _ => {}
    }
}

fn normalise_value(catalog: &Catalog, value: &mut Value) {
    let Some(def) = catalog.value(value.id) else {
        return;
    };
    normalise_args(catalog, &mut value.args, &def.params);
}

/// A bare Button or Color constant in a slot whose default is the Button
/// or Color value becomes that value (`sub_7FF789C9F790`: identifiers
/// `4A3A`-`4A40`, `4BAA`, `6ED7`-`6ED8`; `sub_7FF789C9FA10`: `50A7`,
/// `50A8`, `50AB`-`50AD`, `4D4F`-`4D50`, `534F`, `5B37`-`5B3B`).
fn wrap_constant(value: &mut Value, param: &Param) {
    let id = value.id;
    match param.default_value {
        Some(ids::BUTTON)
            if (0x4A3A..=0x4A40).contains(&id)
                || id == 0x4BAA
                || (0x6ED7..=0x6ED8).contains(&id) =>
        {
            *value = Value::new(ids::BUTTON, vec![Arg::Button(id)]);
        }
        Some(ids::COLOR)
            if matches!(
                id,
                0x50A7 | 0x50A8 | 0x50AB..=0x50AD | 0x4D4F | 0x4D50 | 0x534F
            ) || (0x5B37..=0x5B3B).contains(&id) =>
        {
            *value = Value::new(ids::COLOR, vec![Arg::Enum(id)]);
        }
        _ => {}
    }
}

/// A parameter's default (`sub_7FF789CA02B0`).
pub fn default_arg(catalog: &Catalog, param: &Param) -> Arg {
    match param.kind {
        ParamKind::Bool => Arg::Bool(param.default != 0.0),
        ParamKind::Int => Arg::Int(param.default as i64),
        ParamKind::Number => Arg::Number(param.default as f32),
        ParamKind::Enum => Arg::Enum(
            param
                .enum_default
                .or_else(|| param.enum_id.and_then(|id| catalog.enum_def(id)?.default))
                .unwrap_or(0),
        ),
        ParamKind::Value => Arg::Value(default_value(catalog, param)),
        ParamKind::Array => Arg::Array(Vec::new()),
        ParamKind::CustomString => Arg::CustomString(String::new()),
        ParamKind::Team => Arg::Team(0),
        ParamKind::Hero => Arg::Hero(0),
        ParamKind::Variable => Arg::Variable(0),
        ParamKind::PredefinedString => Arg::PredefinedString(0),
        ParamKind::Compare => Arg::Compare(Compare::Eq),
        ParamKind::Map => Arg::Map(0),
        ParamKind::GameMode => Arg::GameMode(0),
        ParamKind::Subroutine => Arg::Subroutine(0),
        ParamKind::Button => Arg::Button(0x4A3A),
        ParamKind::Flags => Arg::Flags(0),
        ParamKind::WaitBehavior | ParamKind::EventFilter => Arg::WaitBehavior(0),
    }
}

/// A value slot's default value with its own defaults; a Number default
/// takes the slot's default number.
fn default_value(catalog: &Catalog, param: &Param) -> Value {
    let id = param.default_value.unwrap_or(ids::NULL);
    if id == ids::NUMBER {
        return Value::number(param.default as f32);
    }
    let mut value = Value::new(id, Vec::new());
    normalise_value(catalog, &mut value);
    value
}
