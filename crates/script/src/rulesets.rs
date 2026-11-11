//! `GetGameRulesetValue` defaults: the game ruleset schemas (`0C6`) and which
//! schemas each hero (`075`) and game mode (`0C5`) lists, read from
//! `data/game_rulesets_174.json` (the `orpheus-assets` derivation
//! `game_modes`). The Python reference froze the same table from a DataTool
//! dump (`ow174/game/script/rulesets.py`). Keys are the identifiers (`01C`
//! indexes) the graphs read.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::Path,
};

use serde::Deserialize;
use serde_json::Value as Json;

use crate::{graph::asset_guid, wire::Value};

/// The derived file, in the runtime `data` folder.
pub const FILE: &str = "game_rulesets_174.json";

#[derive(Clone, Copy, Debug)]
enum Rule {
    Bool(bool),
    Int(i64),
    Float(f64),
    Asset(u64),
}

impl From<Rule> for Value {
    fn from(value: Rule) -> Self {
        match value {
            Rule::Bool(value) => Value::Bool(value),
            Rule::Int(value) => Value::Integer(value),
            Rule::Float(value) => Value::Float(value),
            Rule::Asset(guid) => Value::Asset(guid),
        }
    }
}

/// Every schema's defaults by key, and the schemas each hero and mode lists
/// in order.
#[derive(Debug, Default)]
pub struct Rulesets {
    schemas: HashMap<u64, HashMap<u32, Rule>>,
    heroes: HashMap<u64, Vec<u64>>,
    modes: HashMap<u64, Vec<u64>>,
}

#[derive(Deserialize)]
struct RawRulesets {
    schemas: BTreeMap<String, BTreeMap<u32, Json>>,
    heroes: BTreeMap<String, Vec<String>>,
    modes: BTreeMap<String, Vec<String>>,
}

impl Rulesets {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_json(&data)
    }

    pub fn from_json(data: &[u8]) -> io::Result<Self> {
        let raw: RawRulesets = serde_json::from_slice(data).map_err(invalid)?;
        let schemas = raw
            .schemas
            .into_iter()
            .map(|(schema, entries)| {
                let entries = entries
                    .into_iter()
                    .map(|(key, value)| Ok((key, rule(&value)?)))
                    .collect::<io::Result<_>>()?;
                Ok((guid(&schema)?, entries))
            })
            .collect::<io::Result<_>>()?;
        let lists = |table: BTreeMap<String, Vec<String>>| {
            table
                .into_iter()
                .map(|(owner, schemas)| {
                    let schemas = schemas
                        .iter()
                        .map(|schema| guid(schema))
                        .collect::<io::Result<_>>()?;
                    Ok((guid(&owner)?, schemas))
                })
                .collect::<io::Result<HashMap<_, _>>>()
        };
        Ok(Self {
            schemas,
            heroes: lists(raw.heroes)?,
            modes: lists(raw.modes)?,
        })
    }

    /// The first default for `key` in the mode's schemas, then the hero's;
    /// None when no schema has it (`default` in `rulesets.py`).
    pub fn default(&self, key: u32, hero: u64, mode: u64) -> Option<Value> {
        let modes = self.modes.get(&mode).map_or(&[][..], Vec::as_slice);
        let heroes = self.heroes.get(&hero).map_or(&[][..], Vec::as_slice);
        modes
            .iter()
            .chain(heroes)
            .find_map(|schema| self.schemas.get(schema)?.get(&key))
            .map(|&rule| rule.into())
    }
}

/// A default as the derivation writes it: a bool, an integer, a float (always
/// with a fraction) or an enum choice's identifier GUID.
fn rule(value: &Json) -> io::Result<Rule> {
    if let Some(value) = value.as_bool() {
        return Ok(Rule::Bool(value));
    }
    if let Some(value) = value.as_i64() {
        return Ok(Rule::Int(value));
    }
    if let Some(value) = value.as_f64() {
        return Ok(Rule::Float(value));
    }
    asset_guid(value)
        .map(Rule::Asset)
        .ok_or_else(|| invalid(format!("unsupported ruleset default {value}")))
}

fn guid(text: &str) -> io::Result<u64> {
    asset_guid(&Json::String(text.into())).ok_or_else(|| invalid(format!("invalid GUID {text:?}")))
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TRACER: u64 = 0x02e0_0000_0000_0003;
    const SOLDIER: u64 = 0x02e0_0000_0000_006e;
    const PRACTICE_RANGE: u64 = 0x0230_0000_0000_0018;

    #[test]
    fn the_mode_schemas_answer_before_the_heroes() {
        let rulesets = Rulesets::from_json(
            &serde_json::to_vec(&json!({
                "schemas": {
                    "0x0A30000000000001": {"10": 3, "11": "0x0D800000000025BD"},
                    "0x0A30000000000002": {"10": 2, "12": false},
                    "0x0A30000000000003": {"10": 1.0, "13": 0.3, "12": true},
                },
                "heroes": {"0x02E0000000000003": ["0x0A30000000000003"]},
                "modes": {"0x0230000000000018": ["0x0A30000000000001", "0x0A30000000000002"]},
            }))
            .unwrap(),
        )
        .unwrap();
        // The mode's first listed schema wins, then its next, then the hero's.
        assert_eq!(
            rulesets.default(10, TRACER, PRACTICE_RANGE),
            Some(Value::Integer(3))
        );
        assert_eq!(
            rulesets.default(11, TRACER, PRACTICE_RANGE),
            Some(Value::Asset(0x0d80_0000_0000_25bd))
        );
        assert_eq!(
            rulesets.default(12, TRACER, PRACTICE_RANGE),
            Some(Value::Bool(false))
        );
        assert_eq!(
            rulesets.default(13, TRACER, PRACTICE_RANGE),
            Some(Value::Float(0.3))
        );
        assert_eq!(rulesets.default(10, TRACER, 0), Some(Value::Float(1.0)));
        assert_eq!(rulesets.default(13, SOLDIER, PRACTICE_RANGE), None);
        assert_eq!(rulesets.default(14, TRACER, PRACTICE_RANGE), None);
        assert!(Rulesets::from_json(br#"{"schemas":{"6":{}},"heroes":{},"modes":{}}"#).is_err());
        assert!(
            Rulesets::from_json(
                br#"{"schemas":{"0x0A30000000000001":{"1":null}},"heroes":{},"modes":{}}"#
            )
            .is_err()
        );
    }

    /// The ultimate's rates scale with the hero's ruleset settings, and a
    /// hero without them charges at 100%.
    #[test]
    fn ultimate_rates_follow_the_generation_settings() {
        use crate::ultimate::{ChargeRates, PASSIVE_POINTS_PER_SECOND};

        let rulesets = Rulesets::from_json(
            &serde_json::to_vec(&json!({
                "schemas": {"0x0A30000000000007": {"9532": 2.0, "13022": 0.5, "12918": 0.25}},
                "heroes": {"0x02E0000000000003": ["0x0A30000000000007"]},
                "modes": {},
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            ChargeRates::of(&rulesets, TRACER, PRACTICE_RANGE),
            ChargeRates {
                passive: PASSIVE_POINTS_PER_SECOND * 0.5,
                combat: 1.0,
            }
        );
        assert_eq!(
            ChargeRates::of(&rulesets, SOLDIER, PRACTICE_RANGE),
            ChargeRates {
                passive: PASSIVE_POINTS_PER_SECOND,
                combat: 1.0,
            }
        );
    }

    /// The install's table: 32 heroes and 24 modes list schemas, and the
    /// defaults answer as `rulesets.py` did (`test_script_expr.py`). Before
    /// the frozen tables were deleted, a test compared the derived table with
    /// them for every hero, mode and key (`the_derived_rulesets_equal_the_python_tables`).
    #[test]
    #[ignore = "requires the 1.74 game data"]
    fn the_install_s_rulesets_answer_as_python_s() {
        let data = orpheus_testkit::runtime_dir().join("data");
        let rulesets = Rulesets::load(data.join(FILE)).unwrap();
        assert_eq!(
            (
                rulesets.heroes.len(),
                rulesets.modes.len(),
                rulesets.schemas.len()
            ),
            (32, 24, 63)
        );
        // Schema 0060 of the Practice Range, then 0006 and 0008.
        assert_eq!(
            rulesets.default(0x6c03, TRACER, PRACTICE_RANGE),
            Some(Value::Float(1.0))
        );
        assert_eq!(
            rulesets.default(0x2546, TRACER, PRACTICE_RANGE),
            Some(Value::Asset(0x0d80_0000_0000_25bd))
        );
        // The clip size scalar and "no ammo use" come from the heroes'
        // schema 001B.
        assert_eq!(
            rulesets.default(0x2819, SOLDIER, PRACTICE_RANGE),
            Some(Value::Float(1.0))
        );
        assert_eq!(
            rulesets.default(0x281a, SOLDIER, PRACTICE_RANGE),
            Some(Value::Bool(false))
        );
        assert_eq!(rulesets.default(1, SOLDIER, PRACTICE_RANGE), None);
        // Schema 0013 is Soldier: 76's own.
        assert_eq!(
            rulesets.default(0x2982, SOLDIER, PRACTICE_RANGE),
            Some(Value::Float(1.0))
        );
        assert_eq!(rulesets.default(0x2982, TRACER, PRACTICE_RANGE), None);
        // 0x2295 is in several mode schemas: the mode's first listed wins.
        assert_eq!(
            rulesets.default(0x2295, TRACER, 0x0230_0000_0000_0007),
            Some(Value::Integer(3))
        );
        assert_eq!(rulesets.default(0x2819, 0x02e0_0000_0000_0001, 0), None);
    }
}
