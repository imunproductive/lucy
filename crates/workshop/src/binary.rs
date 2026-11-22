//! The binary form of the rules: `PBCGLogic` as the custom games protocol
//! carries it (Workshop.md §3.1, `CustomGames.md`). The 24008 header's
//! settings hold the 168-byte logic block at `+0x30`; the client empties
//! its arrays into 24012 items, one per array element, in array order,
//! each with a selector at `+0x1D0` (`sub_7FF7895E42D0`: 0 rules `+0x78`,
//! 1 placed entities `+0xF0`, 2 global variable names and 3 player
//! variable names `+0x128`, 4 subroutine names `+0x150`, 5 user settings
//! `+0x178`, 6 extensions `+0x1C8`).
//!
//! The layouts (`lobby_schemas.json` 24012, the writers `sub_7FF789CA4DE0`
//! and `sub_7FF789CA5520`):
//! - logic: `+0x00` rules, `+0x18` entities, `+0x30` user settings,
//!   `+0x48` u32 extensions, `+0x60`/`+0x78`/`+0x90` `{string}` names of the
//!   global variables, player variables and subroutines, by index.
//! - rule (120): `+0x00` conditions, `+0x18` actions, `+0x30` target
//!   (`+0` hero, `+8` event type, `+0xC` team, `+0x10` slot or subroutine),
//!   `+0x48` disabled, `+0x50` comment.
//! - condition (72): `+0x00` parameters, `+0x18` index, `+0x1C` invert,
//!   `+0x1D` disabled, `+0x20` comment; action: the same with `+0x1C`
//!   disabled.
//! - parameter (72): `+0x00` `val`, `+0x08` `valueParams`, `+0x20`
//!   `enumVal`, `+0x24` `valueIdentifier`, `+0x28`/`+0x29` the variable
//!   flags, `+0x30` `valueStrs`.
//!
//! A top-level parameter keeps a literal in `val` (enum values, numbers as
//! `f32` bits, heroes, maps, buttons) or `enumVal` (teams, Wait behaviour,
//! comparisons, variable, subroutine and predefined string indexes), a
//! custom string in `valueStrs[0]`, a value tree as `valueIdentifier` plus
//! its parameters in `valueParams`. Each parameter of a value in
//! `valueParams` is its kind word then its payload: one word, two for a
//! hero (the GUID second), a nested value's identifier (as a `01C` GUID)
//! and its own parameters, an array's count and its items; a custom
//! string's word indexes the top-level parameter's `valueStrs`.

use std::{error, fmt};

use lucy_wire::jam::codec::{Fields, Value as Jam};

use crate::{
    catalog::{Catalog, Id, Param, ParamKind},
    logic::{self, Action, Arg, Compare, Condition, Event, Logic, Rule, Value, WorkshopSetting},
};

#[derive(Clone, Debug, PartialEq)]
pub struct DecodeError(pub String);

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl error::Error for DecodeError {}

type Result<T, E = DecodeError> = std::result::Result<T, E>;

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(DecodeError(message.into()))
}

/// The 24012 item selectors.
const RULES: u64 = 0;
const GLOBAL_NAMES: u64 = 2;
const PLAYER_NAMES: u64 = 3;
const SUBROUTINE_NAMES: u64 = 4;
const USER_SETTINGS: u64 = 5;
const EXTENSIONS: u64 = 6;

/// Decode the rules of a custom game's settings: the header's logic block
/// (settings `+0x30`) and the 24012 items, in upload order, as the
/// custom games protocol decodes them. The result is normalised.
pub fn decode(block: &Fields, items: &[Fields], catalog: &Catalog) -> Result<Logic> {
    let mut logic = Logic::default();
    for rule in array(block, 0x00) {
        logic.rules.push(rule_of(struct_of(rule)?, catalog)?);
    }
    logic.global_names = names(block, 0x60);
    logic.player_names = names(block, 0x78);
    logic.subroutine_names = names(block, 0x90);
    for setting in array(block, 0x30) {
        logic
            .workshop_settings
            .push(user_setting(struct_of(setting)?));
    }
    logic.extensions.extend(
        array(block, 0x48)
            .iter()
            .filter_map(unsigned)
            .map(|word| word as u32),
    );
    for item in items {
        let selector = item.get(&0x1D0).and_then(unsigned).unwrap_or(u64::MAX);
        let name = |offset| field_struct(item, offset).map(name_of).unwrap_or_default();
        match selector {
            RULES => logic
                .rules
                .push(rule_of(field_struct(item, 0x78)?, catalog)?),
            GLOBAL_NAMES => logic.global_names.push(name(0x128)),
            PLAYER_NAMES => logic.player_names.push(name(0x128)),
            SUBROUTINE_NAMES => logic.subroutine_names.push(name(0x150)),
            USER_SETTINGS => logic
                .workshop_settings
                .push(user_setting(field_struct(item, 0x178)?)),
            EXTENSIONS => logic
                .extensions
                .push(item.get(&0x1C8).and_then(unsigned).unwrap_or(0) as u32),
            // 1: a placed entity (Workshop maps); the engine has no use for
            // them yet.
            _ => {}
        }
    }
    logic.normalise(catalog);
    Ok(logic)
}

fn rule_of(rule: &Fields, catalog: &Catalog) -> Result<Rule> {
    let target = field_struct(rule, 0x30)?;
    let event = Event {
        kind: integer(target, 0x08) as u8,
        team: integer(target, 0x0C) as u8,
        slot: integer(target, 0x10) as i32,
        hero: target.get(&0x00).and_then(unsigned).unwrap_or(0),
    };
    let mut conditions = Vec::new();
    for condition in array(rule, 0x00) {
        let condition = struct_of(condition)?;
        let params = array(condition, 0x00);
        let Some(entry) = catalog.conditions.get(integer(condition, 0x18) as usize) else {
            return fail("a condition outside the catalog");
        };
        let [left, op, right] = params else {
            return fail(format!("a condition with {} parameters", params.len()));
        };
        let left = param(struct_of(left)?, &entry.params[0], catalog)?;
        let op = match param(struct_of(op)?, &entry.params[1], catalog)? {
            Arg::Compare(op) => op,
            _ => return fail("a condition without its comparison"),
        };
        let right = param(struct_of(right)?, &entry.params[2], catalog)?;
        conditions.push(Condition {
            left,
            op,
            right,
            invert: boolean(condition, 0x1C),
            disabled: boolean(condition, 0x1D),
            comment: string(condition, 0x20),
        });
    }
    let mut actions = Vec::new();
    for action in array(rule, 0x18) {
        let action = struct_of(action)?;
        let index = integer(action, 0x18) as u16;
        let Some(entry) = catalog.action(index) else {
            return fail(format!("action {index} is outside the catalog"));
        };
        let args = array(action, 0x00)
            .iter()
            .zip(&entry.params)
            .map(|(value, spec)| param(struct_of(value)?, spec, catalog))
            .collect::<Result<_>>()?;
        actions.push(Action {
            index,
            args,
            disabled: boolean(action, 0x1C),
            comment: string(action, 0x20),
        });
    }
    Ok(Rule {
        name: string(rule, 0x50),
        disabled: boolean(rule, 0x48),
        event,
        conditions,
        actions,
    })
}

/// One top-level parameter.
fn param(fields: &Fields, spec: &Param, catalog: &Catalog) -> Result<Arg> {
    let val = fields.get(&0x00).and_then(unsigned).unwrap_or(0);
    let enum_val = integer(fields, 0x20);
    let words: Vec<u64> = array(fields, 0x08).iter().filter_map(unsigned).collect();
    let strings: Vec<String> = array(fields, 0x30)
        .iter()
        .filter_map(|value| match value {
            Jam::String(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    let mut cursor = Cursor {
        words: &words,
        at: 0,
        strings: &strings,
        catalog,
    };
    Ok(match spec.kind {
        ParamKind::Value => {
            let id = integer(fields, 0x24) as Id;
            Arg::Value(cursor.value_params(id)?)
        }
        ParamKind::Array => {
            let mut items = Vec::new();
            for _ in 0..val {
                cursor.word()?;
                items.push(cursor.value()?);
            }
            Arg::Array(items)
        }
        ParamKind::Team => Arg::Team(enum_val as u8),
        ParamKind::WaitBehavior | ParamKind::EventFilter => Arg::WaitBehavior(enum_val as u8),
        ParamKind::Compare => match Compare::from_code(enum_val) {
            Some(op) => Arg::Compare(op),
            None => return fail(format!("comparison {enum_val}")),
        },
        ParamKind::Variable => Arg::Variable(enum_val as u8),
        ParamKind::Subroutine => Arg::Subroutine(enum_val as u8),
        ParamKind::PredefinedString => Arg::PredefinedString(enum_val as u16),
        ParamKind::CustomString => Arg::CustomString(strings.first().cloned().unwrap_or_default()),
        kind => literal(kind, val, &[])?,
    })
}

/// A one-word literal: a top-level `val` or a value parameter's payload.
fn literal(kind: ParamKind, word: u64, strings: &[String]) -> Result<Arg> {
    Ok(match kind {
        ParamKind::Team => Arg::Team(word as u8),
        ParamKind::Bool => Arg::Bool(word != 0),
        ParamKind::Int => Arg::Int(i64::from(word as u32 as i32)),
        ParamKind::Number => Arg::Number(f32::from_bits(word as u32)),
        ParamKind::Variable => Arg::Variable(word as u8),
        ParamKind::CustomString => {
            Arg::CustomString(strings.get(word as usize).cloned().unwrap_or_default())
        }
        ParamKind::Enum => Arg::Enum(word as u32),
        ParamKind::PredefinedString => Arg::PredefinedString(word as u16),
        ParamKind::Compare => match Compare::from_code(word as i64) {
            Some(op) => Arg::Compare(op),
            None => return fail(format!("comparison {word}")),
        },
        ParamKind::Map => Arg::Map(word as u32),
        ParamKind::GameMode => Arg::GameMode(word),
        ParamKind::Subroutine => Arg::Subroutine(word as u8),
        ParamKind::Button => Arg::Button(word as u32),
        ParamKind::Flags => Arg::Flags(word),
        ParamKind::Hero => Arg::Hero(word),
        ParamKind::WaitBehavior | ParamKind::EventFilter => Arg::WaitBehavior(word as u8),
        ParamKind::Value | ParamKind::Array => return fail("a value where a literal belongs"),
    })
}

/// Reads a value tree from `valueParams`.
struct Cursor<'a> {
    words: &'a [u64],
    at: usize,
    strings: &'a [String],
    catalog: &'a Catalog,
}

impl Cursor<'_> {
    fn word(&mut self) -> Result<u64> {
        let word = self.words.get(self.at).copied();
        self.at += 1;
        word.map_or_else(|| fail("value parameters end early"), Ok)
    }

    /// A value's identifier word and its parameters.
    fn value(&mut self) -> Result<Value> {
        let id = self.word()? as u32;
        self.value_params(id)
    }

    fn value_params(&mut self, id: Id) -> Result<Value> {
        let Some(def) = self.catalog.value(id) else {
            return fail(format!("value {id:04X} is outside the catalog"));
        };
        let mut args = Vec::new();
        for spec in &def.params {
            let expected = kind_code(spec.kind);
            if self.words.get(self.at).copied() != Some(u64::from(expected)) && workshop_setting(id)
            {
                // The normaliser inserts what an older Workshop Setting
                // value lacks (`sub_7FF789C9FFF0`).
                args.push(logic::default_arg(self.catalog, spec));
                continue;
            }
            let kind = self.word()?;
            if kind != u64::from(expected) {
                return fail(format!(
                    "value {id:04X}: kind {kind} where the catalog has {expected}"
                ));
            }
            args.push(match spec.kind {
                ParamKind::Value => Arg::Value(self.value()?),
                ParamKind::Array => {
                    let count = self.word()?;
                    let mut items = Vec::new();
                    for _ in 0..count {
                        self.word()?;
                        items.push(self.value()?);
                    }
                    Arg::Array(items)
                }
                ParamKind::Hero => {
                    self.word()?;
                    Arg::Hero(self.word()?)
                }
                kind => literal(kind, self.word()?, self.strings)?,
            });
        }
        Ok(Value::new(id, args))
    }
}

/// The values the normaliser checks for missing parameters: `7B01`,
/// `7B03`, `7B05` (`sub_7FF789C9FCD0`).
fn workshop_setting(id: Id) -> bool {
    matches!(id, 0x7B01 | 0x7B03 | 0x7B05)
}

/// The client's kind code of a parameter kind.
fn kind_code(kind: ParamKind) -> u8 {
    match kind {
        ParamKind::Team => 1,
        ParamKind::Hero => 3,
        ParamKind::Bool => 4,
        ParamKind::Int => 5,
        ParamKind::Number => 6,
        ParamKind::Variable => 8,
        ParamKind::CustomString => 11,
        ParamKind::Value => 12,
        ParamKind::Enum => 13,
        ParamKind::PredefinedString => 14,
        ParamKind::Compare => 15,
        ParamKind::Array => 16,
        ParamKind::Map => 17,
        ParamKind::GameMode => 18,
        ParamKind::Subroutine => 19,
        ParamKind::Button => 20,
        ParamKind::Flags => 21,
        ParamKind::WaitBehavior | ParamKind::EventFilter => 0,
    }
}

/// A user setting instance (80 bytes: `+0x00` a vector, `+0x0C` a type,
/// `+0x10` heroes, `+0x28` the name). Which component holds which kind of
/// value is not traced; the value is the vector's X [assumption].
fn user_setting(fields: &Fields) -> WorkshopSetting {
    let x = field_struct(fields, 0x00)
        .ok()
        .and_then(|vector| vector.get(&0x00))
        .and_then(float)
        .unwrap_or(0.0);
    WorkshopSetting {
        name: string(fields, 0x28),
        value: x.to_string(),
    }
}

fn names(block: &Fields, offset: u32) -> Vec<String> {
    array(block, offset)
        .iter()
        .map(|entry| struct_of(entry).map(name_of).unwrap_or_default())
        .collect()
}

fn name_of(entry: &Fields) -> String {
    string(entry, 0x00)
}

fn array(fields: &Fields, offset: u32) -> &[Jam] {
    match fields.get(&offset) {
        Some(Jam::Array(items)) => items,
        _ => &[],
    }
}

fn struct_of(value: &Jam) -> Result<&Fields> {
    match value {
        Jam::Struct(fields) => Ok(fields),
        _ => fail("expected a structure"),
    }
}

fn field_struct(fields: &Fields, offset: u32) -> Result<&Fields> {
    match fields.get(&offset) {
        Some(Jam::Struct(inner)) => Ok(inner),
        _ => fail(format!("no structure at +0x{offset:X}")),
    }
}

fn unsigned(value: &Jam) -> Option<u64> {
    match value {
        Jam::Unsigned(value) => Some(*value),
        Jam::Signed(value) => Some(*value as u64),
        Jam::Bool(value) => Some(u64::from(*value)),
        _ => None,
    }
}

fn float(value: &Jam) -> Option<f64> {
    match value {
        Jam::Float32(value) => Some(f64::from(*value)),
        Jam::Float64(value) => Some(*value),
        _ => None,
    }
}

fn integer(fields: &Fields, offset: u32) -> i64 {
    match fields.get(&offset) {
        Some(Jam::Signed(value)) => *value,
        Some(Jam::Unsigned(value)) => *value as i64,
        Some(Jam::Bool(value)) => i64::from(*value),
        _ => 0,
    }
}

fn boolean(fields: &Fields, offset: u32) -> bool {
    integer(fields, offset) != 0
}

fn string(fields: &Fields, offset: u32) -> String {
    match fields.get(&offset) {
        Some(Jam::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The 01C GUID of an identifier, as value trees and enum literals store
/// it (`sub_7FF789C9F790` builds `id | 01C type bits`).
const IDENTIFIER: u64 = 0x0D80_0000_0000_0000;

/// Encode rules the way the client uploads them: the logic block with
/// empty arrays, and one 24012 item per rule, name, user setting and
/// extension in [`decode`]'s order (without the "more" flag at `+0x1D1`,
/// which the transfer sets). The inverse of [`decode`].
pub fn encode(logic: &Logic, catalog: &Catalog) -> (Fields, Vec<Fields>) {
    let block: Fields = [0x00, 0x18, 0x30, 0x48, 0x60, 0x78, 0x90]
        .into_iter()
        .map(|offset| (offset, Jam::Array(Vec::new())))
        .collect();
    let item = |selector: u64, offset: u32, value: Jam| -> Fields {
        let mut fields = Fields::new();
        fields.insert(offset, value);
        fields.insert(0x1CC, Jam::Signed(0));
        fields.insert(0x1D0, Jam::Unsigned(selector));
        fields
    };
    let name = |name: &String| Jam::Struct(Fields::from([(0x00, Jam::String(name.clone()))]));
    let mut items = Vec::new();
    for rule in &logic.rules {
        items.push(item(RULES, 0x78, Jam::Struct(encode_rule(rule, catalog))));
    }
    for global in &logic.global_names {
        items.push(item(GLOBAL_NAMES, 0x128, name(global)));
    }
    for player in &logic.player_names {
        items.push(item(PLAYER_NAMES, 0x128, name(player)));
    }
    for subroutine in &logic.subroutine_names {
        items.push(item(SUBROUTINE_NAMES, 0x150, name(subroutine)));
    }
    for setting in &logic.workshop_settings {
        let value = setting.value.parse::<f32>().unwrap_or(0.0);
        let vector = Fields::from([
            (0x00, Jam::Float32(value)),
            (0x04, Jam::Float32(0.0)),
            (0x08, Jam::Float32(0.0)),
        ]);
        let fields = Fields::from([
            (0x00, Jam::Struct(vector)),
            (0x0C, Jam::Signed(0)),
            (0x10, Jam::Array(Vec::new())),
            (0x28, Jam::String(setting.name.clone())),
        ]);
        items.push(item(USER_SETTINGS, 0x178, Jam::Struct(fields)));
    }
    for &extension in &logic.extensions {
        items.push(item(EXTENSIONS, 0x1C8, Jam::Unsigned(u64::from(extension))));
    }
    (block, items)
}

fn encode_rule(rule: &Rule, catalog: &Catalog) -> Fields {
    let condition_params = catalog
        .conditions
        .first()
        .map(|entry| entry.params.as_slice())
        .unwrap_or_default();
    let conditions = rule
        .conditions
        .iter()
        .map(|condition| {
            let params = [
                condition.left.clone(),
                Arg::Compare(condition.op),
                condition.right.clone(),
            ];
            let mut fields = Fields::from([
                (0x00, encode_params(&params, condition_params, catalog)),
                (0x18, Jam::Signed(0)),
                (0x1C, Jam::Bool(condition.invert)),
                (0x1D, Jam::Bool(condition.disabled)),
            ]);
            fields.insert(0x20, Jam::String(condition.comment.clone()));
            Jam::Struct(fields)
        })
        .collect();
    let actions = rule
        .actions
        .iter()
        .map(|action| {
            let params = catalog
                .action(action.index)
                .map(|entry| entry.params.as_slice())
                .unwrap_or_default();
            Jam::Struct(Fields::from([
                (0x00, encode_params(&action.args, params, catalog)),
                (0x18, Jam::Signed(i64::from(action.index))),
                (0x1C, Jam::Bool(action.disabled)),
                (0x20, Jam::String(action.comment.clone())),
            ]))
        })
        .collect();
    let target = Fields::from([
        (0x00, Jam::Unsigned(rule.event.hero)),
        (0x08, Jam::Signed(i64::from(rule.event.kind))),
        (0x0C, Jam::Signed(i64::from(rule.event.team))),
        (0x10, Jam::Signed(i64::from(rule.event.slot))),
    ]);
    Fields::from([
        (0x00, Jam::Array(conditions)),
        (0x18, Jam::Array(actions)),
        (0x30, Jam::Struct(target)),
        (0x48, Jam::Bool(rule.disabled)),
        (0x50, Jam::String(rule.name.clone())),
    ])
}

fn encode_params(args: &[Arg], params: &[Param], catalog: &Catalog) -> Jam {
    Jam::Array(
        args.iter()
            .zip(params)
            .map(|(arg, spec)| Jam::Struct(encode_param(arg, spec, catalog)))
            .collect(),
    )
}

/// One top-level parameter (see [`param`]).
fn encode_param(arg: &Arg, spec: &Param, catalog: &Catalog) -> Fields {
    let mut val = 0u64;
    let mut enum_val = 0i64;
    let mut identifier = 0i64;
    let mut words = Vec::new();
    let mut strings = Vec::new();
    match (spec.kind, arg) {
        (ParamKind::Value, Arg::Value(value)) => {
            identifier = i64::from(value.id);
            value_params(value, catalog, &mut words, &mut strings);
        }
        (ParamKind::Array, Arg::Array(items)) => {
            val = items.len() as u64;
            for item in items {
                words.push(12);
                words.push(IDENTIFIER | u64::from(item.id));
                value_params(item, catalog, &mut words, &mut strings);
            }
        }
        (_, Arg::Team(team)) => enum_val = i64::from(*team),
        (_, Arg::WaitBehavior(behavior)) => enum_val = i64::from(*behavior),
        (_, Arg::Compare(op)) => enum_val = compare_code(*op),
        (_, Arg::Variable(index)) | (_, Arg::Subroutine(index)) => enum_val = i64::from(*index),
        (_, Arg::PredefinedString(index)) => enum_val = i64::from(*index),
        (_, Arg::CustomString(text)) => strings.push(Jam::String(text.clone())),
        (_, literal) => val = literal_word(literal),
    }
    Fields::from([
        (0x00, Jam::Unsigned(val)),
        (
            0x08,
            Jam::Array(words.into_iter().map(Jam::Unsigned).collect()),
        ),
        (0x20, Jam::Signed(enum_val)),
        (0x24, Jam::Signed(identifier)),
        (0x28, Jam::Bool(spec.kind == ParamKind::Variable)),
        (
            0x29,
            Jam::Bool(spec.kind == ParamKind::Variable && spec.global),
        ),
        (0x30, Jam::Array(strings)),
    ])
}

/// A value's parameters into `valueParams`: each its kind word and payload.
fn value_params(value: &Value, catalog: &Catalog, words: &mut Vec<u64>, strings: &mut Vec<Jam>) {
    let Some(def) = catalog.value(value.id) else {
        return;
    };
    for (arg, spec) in value.args.iter().zip(&def.params) {
        words.push(u64::from(kind_code(spec.kind)));
        match arg {
            Arg::Value(inner) => {
                words.push(IDENTIFIER | u64::from(inner.id));
                value_params(inner, catalog, words, strings);
            }
            Arg::Array(items) => {
                words.push(items.len() as u64);
                for item in items {
                    words.push(12);
                    words.push(IDENTIFIER | u64::from(item.id));
                    value_params(item, catalog, words, strings);
                }
            }
            Arg::Hero(hero) => {
                // The first of a hero's two words is not read back
                // [assumption: zero].
                words.push(0);
                words.push(*hero);
            }
            Arg::CustomString(text) => {
                words.push(strings.len() as u64);
                strings.push(Jam::String(text.clone()));
            }
            Arg::Team(team) => words.push(u64::from(*team)),
            Arg::WaitBehavior(behavior) => words.push(u64::from(*behavior)),
            Arg::Compare(op) => words.push(compare_code(*op) as u64),
            Arg::Variable(index) | Arg::Subroutine(index) => words.push(u64::from(*index)),
            Arg::PredefinedString(index) => words.push(u64::from(*index)),
            literal => words.push(literal_word(literal)),
        }
    }
}

fn literal_word(arg: &Arg) -> u64 {
    match arg {
        Arg::Bool(value) => u64::from(*value),
        Arg::Int(value) => u64::from(*value as i32 as u32),
        Arg::Number(value) => u64::from(value.to_bits()),
        Arg::Enum(id) | Arg::Button(id) => IDENTIFIER | u64::from(*id),
        Arg::Map(map) => u64::from(*map),
        Arg::GameMode(guid) | Arg::Hero(guid) | Arg::Flags(guid) => *guid,
        _ => 0,
    }
}

fn compare_code(op: Compare) -> i64 {
    match op {
        Compare::Eq => 0,
        Compare::Ne => 1,
        Compare::Lt => 2,
        Compare::Le => 3,
        Compare::Gt => 4,
        Compare::Ge => 5,
    }
}
