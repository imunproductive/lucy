//! The array actions: ClearArray (`STU_1C2CC6D5`), the unkeyed array
//! insert (`STU_4C3509FB`, `STU_CBE8BDAA`) and the unkeyed remove
//! (`STU_DA54F8F7`). Ported from leafall903/ORPHEUS (MIT), commit c239a84:
//! `src/ow174/game/script/array_actions.py` and `docs/ARRAY_ACTIONS.txt`;
//! the remove from the client's run (`0x7FF78AA7BEB0`).
//!
//! Each writes the dynamic variable in `m_26E3B366`. A missing output, a
//! variable with identifier 0 or a literal config (whose output cast fails
//! natively) changes nothing and follows the out plug; any other output is
//! unsupported.
//!
//! The insert (1B6B720) reads its index as a scalar: missing, -1. A negative
//! index appends; an index past the end first pads the array with int 0; an
//! index inside the array inserts there, each row after the one before. A
//! destination that isn't an array starts empty. `STU_4C3509FB` iterates its
//! value (a config list's configs in order, arrays opened into rows, null
//! left out); `STU_CBE8BDAA` (1B6BCB0) inserts its one value as an
//! assignment reads it. With the unique flag (`m_8AD67B20`) a row equal to
//! one already there (the padding included) is left out. Later configs see
//! the rows earlier ones inserted; reaching the destination itself, when it
//! has rows, ends the whole loop. The watchers hear of the change once, at
//! the end. The native limit of 200 (index or rows) raises a script
//! diagnostic that isn't modelled, so it is unsupported here, as are keyed
//! rows and a destination a state links to.
//!
//! The remove reads its index the same way (missing, -1). An index inside
//! the array removes that row; a negative one removes every row equal to
//! a row of its value (iterated as the insert iterates it, until it
//! reaches the destination itself: `0x7FF78AA7EA20`); an index past the end
//! removes nothing. Without a value and an index it leaves the destination
//! alone; otherwise a destination that isn't an array becomes an empty one.

use super::*;

/// The largest index and row count the native helper accepts silently.
const NATIVE_LIMIT: usize = 200;

/// Literal configs: an array action's output cast fails on them
/// (`expr.CONSTANTS` in leafall903/ORPHEUS, commit c239a84).
const LITERALS: &[&str] = &[
    "STUConfigVarBool",
    "STUConfigVarInt",
    "STUConfigVarFloat",
    "STUConfigVarLogicalButton",
    "STU_BD6CC023",
    "STU_297E1A30",
];

enum Output {
    /// Nothing to write; the action follows its out plug.
    Skip,
    Variable(Variable),
    /// Reported as unsupported.
    Refused,
}

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// ClearArray (1B6C130): a fresh empty array, written as SetVar writes.
    /// It neither writes null nor empties the old array in place.
    pub(super) fn clear_array(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        match self.array_output(id, node, "array clear output") {
            Output::Refused => return Ok(false),
            Output::Variable(variable) if variable.id != 0 => {
                self.write(source_key(id, variable), Value::Array(Vec::new()))?;
            }
            Output::Variable(_) | Output::Skip => {}
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    pub(super) fn insert_array(
        &mut self,
        id: u16,
        node: &Node,
        caller: Option<StateKey>,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let site = self.site(id, node);
        let source = field(&node.fields, "m_value");
        let output = if source.is_null() {
            Output::Skip
        } else {
            self.array_output(id, node, "array insert output")
        };
        let variable = match output {
            Output::Refused => return Ok(false),
            Output::Variable(variable) => variable,
            Output::Skip => {
                self.follow(id, node, "m_outPlug", None, depth)?;
                return Ok(true);
            }
        };
        let index = field(&node.fields, "m_index");
        let index = if index.is_null() {
            -1
        } else {
            let Some(value) = self.evaluate(id, node, index, caller) else {
                return Ok(false);
            };
            native_index(&value)
        };
        if index > NATIVE_LIMIT as i64 {
            self.report(
                site,
                Issue::UnsupportedField("array insert index past the native limit"),
            );
            return Ok(false);
        }
        let key = field(&node.fields, "m_key");
        if !key.is_null() {
            let Some(key) = self.evaluate(id, node, key, caller) else {
                return Ok(false);
            };
            // F663A0 refuses these keys before touching the destination.
            if !keyed_row(&key) || variable.id == 0 {
                self.follow(id, node, "m_outPlug", None, depth)?;
                return Ok(true);
            }
            self.report(site, Issue::UnsupportedField("keyed array insert"));
            return Ok(false);
        }
        if variable.id == 0 {
            self.follow(id, node, "m_outPlug", None, depth)?;
            return Ok(true);
        }
        let key = source_key(id, variable);
        if self
            .machine
            .links
            .get(&key)
            .is_some_and(|entries| !entries.is_empty())
        {
            self.report(
                site,
                Issue::UnsupportedField("array insert into a linked variable"),
            );
            return Ok(false);
        }
        let before = self.machine.value(key, None);
        let mut rows = match &before {
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        };
        let original = rows.len();
        let mut at = usize::try_from(index).ok();
        if let Some(index) = at.filter(|&index| index > original) {
            rows.resize(index, Value::Integer(0));
        }
        at = at.filter(|&index| index < original);
        let unique = configured(node, "m_8AD67B20");
        let inserted = if node.class == "STU_CBE8BDAA" {
            match self.evaluate_value(id, node, source, caller) {
                Some(value) => self.insert_row(&mut rows, &mut at, unique, value),
                None => Ok(false),
            }
        } else {
            self.insert_rows(id, node, caller, variable, key, &mut rows, &mut at, unique)
        };
        let count = rows.len();
        self.set_base(key, Value::Array(rows));
        self.notify_value(key, &before, None)?;
        if !inserted? {
            return Ok(false);
        }
        if count > NATIVE_LIMIT {
            self.report(
                site,
                Issue::UnsupportedField("array insert past the native limit"),
            );
            return Ok(false);
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// The rows of `m_value`, config by config; false when a config could
    /// not be read.
    #[allow(clippy::too_many_arguments)]
    fn insert_rows(
        &mut self,
        id: u16,
        node: &Node,
        caller: Option<StateKey>,
        variable: Variable,
        key: VarKey,
        rows: &mut Vec<Value>,
        at: &mut Option<usize>,
        unique: bool,
    ) -> Result<bool, InitFault> {
        let mut members = Vec::new();
        if let Err(error) =
            expr::config_list_members(field(&node.fields, "m_value"), 0, &mut members)
        {
            self.report(self.site(id, node), Issue::Expression(error));
            return Ok(false);
        }
        for member in members {
            if expr::dynamic(member).ok().flatten() == Some(variable) && !rows.is_empty() {
                // The destination's own rows: the native loop ends here.
                break;
            }
            // The config reads the destination as it is so far.
            self.set_base(key, Value::Array(rows.clone()));
            let Some(value) = self.evaluate(id, node, member, caller) else {
                return Ok(false);
            };
            let mut values = Vec::new();
            if let Err(error) = expr::value_rows(value, &mut values) {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
            for value in values {
                if !self.insert_row(rows, at, unique, value)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn insert_row(
        &mut self,
        rows: &mut Vec<Value>,
        at: &mut Option<usize>,
        unique: bool,
        value: Value,
    ) -> Result<bool, InitFault> {
        self.spend("array insert row")?;
        if unique && rows.iter().any(|row| equal(row, &value)) {
            return Ok(true);
        }
        match at {
            Some(index) => {
                rows.insert(*index, value);
                *index += 1;
            }
            None => rows.push(value),
        }
        Ok(true)
    }

    /// RemoveFromArray (`STU_DA54F8F7`, run `0x7FF78AA7BEB0`), unkeyed.
    pub(super) fn remove_array(
        &mut self,
        id: u16,
        node: &Node,
        caller: Option<StateKey>,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let site = self.site(id, node);
        let variable = match self.array_output(id, node, "array remove output") {
            Output::Refused => return Ok(false),
            Output::Variable(variable) if variable.id != 0 => variable,
            Output::Variable(_) | Output::Skip => {
                self.follow(id, node, "m_outPlug", None, depth)?;
                return Ok(true);
            }
        };
        if !field(&node.fields, "m_key").is_null() {
            self.report(site, Issue::UnsupportedField("keyed array remove"));
            return Ok(false);
        }
        let source = field(&node.fields, "m_value");
        let index = field(&node.fields, "m_index");
        if source.is_null() && index.is_null() {
            self.follow(id, node, "m_outPlug", None, depth)?;
            return Ok(true);
        }
        let key = source_key(id, variable);
        if self
            .machine
            .links
            .get(&key)
            .is_some_and(|entries| !entries.is_empty())
        {
            self.report(
                site,
                Issue::UnsupportedField("array remove from a linked variable"),
            );
            return Ok(false);
        }
        let index = if index.is_null() {
            -1
        } else {
            let Some(value) = self.evaluate(id, node, index, caller) else {
                return Ok(false);
            };
            native_index(&value)
        };
        let before = self.machine.value(key, None);
        let mut rows = match &before {
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        };
        if let Ok(index) = usize::try_from(index) {
            if index < rows.len() {
                rows.remove(index);
            }
        } else if !source.is_null() {
            let mut members = Vec::new();
            if let Err(error) = expr::config_list_members(source, 0, &mut members) {
                self.report(site, Issue::Expression(error));
                return Ok(false);
            }
            for member in members {
                if expr::dynamic(member).ok().flatten() == Some(variable) {
                    break;
                }
                let Some(value) = self.evaluate(id, node, member, caller) else {
                    return Ok(false);
                };
                let mut values = Vec::new();
                if let Err(error) = expr::value_rows(value, &mut values) {
                    self.report(site, Issue::Expression(error));
                    return Ok(false);
                }
                for value in values {
                    self.spend("array remove row")?;
                    rows.retain(|row| !equal(row, &value));
                }
            }
        }
        self.set_base(key, Value::Array(rows));
        self.notify_value(key, &before, None)?;
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    fn array_output(&mut self, id: u16, node: &Node, what: &'static str) -> Output {
        let config = field(&node.fields, "m_26E3B366");
        if config.is_null() {
            return Output::Skip;
        }
        match expr::dynamic(config) {
            Ok(Some(variable)) => Output::Variable(variable),
            Ok(None)
                if field(config, "$")
                    .as_str()
                    .is_some_and(|class| LITERALS.contains(&class)) =>
            {
                Output::Skip
            }
            Ok(None) => {
                self.report(self.site(id, node), Issue::UnsupportedField(what));
                Output::Refused
            }
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                Output::Refused
            }
        }
    }
}

/// The insert index (F65260 with the default -1): an array is its first
/// member (none: -1), null 0, a bool 0 or 1, an int as signed 32 bits, a
/// float rounded to f32 and then half away from zero (out of range: the
/// 32-bit minimum); anything else -1.
fn native_index(mut value: &Value) -> i64 {
    while let Value::Array(items) = value {
        let Some(first) = items.first() else {
            return -1;
        };
        value = first;
    }
    match value {
        Value::Null => 0,
        Value::Bool(value) => i64::from(*value),
        Value::Integer(value) => i64::from(*value as i32),
        Value::Float(value) => {
            let value = f64::from(*value as f32);
            let value = f64::from(
                (if value < 0.0 {
                    value - 0.5
                } else {
                    value + 0.5
                }) as f32,
            );
            if value.is_finite() && (-2_147_483_648.0..2_147_483_648.0).contains(&value) {
                value.trunc() as i64
            } else {
                i64::from(i32::MIN)
            }
        }
        _ => -1,
    }
}

/// F663A0: bools, numbers, arrays, maps and null are no row keys.
fn keyed_row(key: &Value) -> bool {
    !matches!(
        key,
        Value::Null
            | Value::Bool(_)
            | Value::Integer(_)
            | Value::Float(_)
            | Value::Array(_)
            | Value::Map(_)
            | Value::Pairs(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_index_converts_like_the_native_helper() {
        for (value, expected) in [
            (Value::Null, 0),
            (Value::Bool(true), 1),
            (Value::Integer(3), 3),
            (Value::Integer(0x1_0000_0002), 2),
            (Value::Integer(-5), -5),
            (Value::Float(1.5), 2),
            (Value::Float(-1.5), -2),
            (Value::Float(2.49), 2),
            (Value::Float(f64::NAN), i64::from(i32::MIN)),
            (Value::Float(3e9), i64::from(i32::MIN)),
            (Value::Array(vec![Value::Integer(4), Value::Integer(9)]), 4),
            (Value::Array(Vec::new()), -1),
            (Value::Asset(7), -1),
        ] {
            assert_eq!(native_index(&value), expected, "{value:?}");
        }
    }
}
