//! The Workshop catalog of the install (`data/workshop_catalog_174.json`,
//! derived by `orpheus_assets` from `054/0154`, `0155` and `0162`): the
//! events, the condition, the actions with their graphs, the values, the
//! enums, the predefined strings, and the hero, map and game mode names the
//! text form uses.
//!
//! Actions, events and conditions are named by their position (the logic's
//! `index` fields and event type); values and enum values by their `01C`
//! identifier index, as the logic stores them.

use std::{
    collections::HashMap,
    fs,
    io::{self, Read},
    path::Path,
};

use serde::Deserialize;

pub const FILE: &str = "workshop_catalog_174.json";

/// The `01C` identifier index of a value or an enum value.
pub type Id = u32;

/// What a parameter holds, by its class (the client's kind codes,
/// `sub_7FF789CA1D70`, in comments).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// 1 `STU_F5D532BC`: All Teams, Team 1 or Team 2.
    Team,
    /// 3 `STU_38B39A55`: a hero (two payload words in a value tree).
    Hero,
    /// 4 `STU_1554B38A`.
    Bool,
    /// 5 `STU_4DD36FA9`.
    Int,
    /// 6 `STU_8302E7AC`.
    Number,
    /// 8 `STU_DE6A15D2`: a variable index.
    Variable,
    /// 11 `STU_218BCF68`.
    CustomString,
    /// 12 `STU_C5BE2B08` (and `STU_8E0051DD`, the per-element slot).
    Value,
    /// 13 `STU_3FA24DEA`.
    Enum,
    /// 14 `STU_F654E6FB`: an index into the predefined strings.
    PredefinedString,
    /// 15 `STU_8504E8FE`.
    Compare,
    /// 16 `STU_CFF9EFAB`.
    Array,
    /// 17 `STU_28E537BD`: a map header (`09F`) index.
    Map,
    /// 18 `STU_93382EAB`: a game mode GUID.
    GameMode,
    /// 19 `STU_BE408E5C`: a subroutine index.
    Subroutine,
    /// 20 `STU_EE41F5B8`: a Button enum value.
    Button,
    /// 21 `STU_59873384`: flags (no shipped value or action uses it).
    Flags,
    /// `STU_7BF2036D`: Wait's behaviour (Ignore Condition, Abort When
    /// False, Restart When True), the parameter's `enumVal`.
    WaitBehavior,
    /// The event filters: `STU_E08C5126` (team), `STU_16886813` (player),
    /// `STU_27CC2BBE` (subroutine).
    EventFilter,
}

impl ParamKind {
    fn of(class: &str, kind: u8) -> io::Result<Self> {
        Ok(match (kind, class) {
            (1, _) => Self::Team,
            (3, _) => Self::Hero,
            (4, _) => Self::Bool,
            (5, _) => Self::Int,
            (6, _) => Self::Number,
            (8, _) => Self::Variable,
            (11, _) => Self::CustomString,
            (12, _) => Self::Value,
            (13, _) => Self::Enum,
            (14, _) => Self::PredefinedString,
            (15, _) => Self::Compare,
            (16, _) => Self::Array,
            (17, _) => Self::Map,
            (18, _) => Self::GameMode,
            (19, _) => Self::Subroutine,
            (20, _) => Self::Button,
            (21, _) => Self::Flags,
            (0, "STU_7BF2036D") => Self::WaitBehavior,
            (0, "STU_E08C5126" | "STU_16886813" | "STU_27CC2BBE") => Self::EventFilter,
            _ => return Err(invalid(format!("parameter class {class} kind {kind}"))),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: Option<String>,
    pub class: String,
    pub kind: ParamKind,
    /// A value slot's default value (`m_464FB148`).
    pub default_value: Option<Id>,
    /// A value slot's expected type (`m_16CCEFC8`, `Enum_542A081B`).
    pub value_type: i64,
    pub min: f64,
    pub max: f64,
    /// The default number of a number, integer or value slot; a bool's
    /// default as 0 or 1.
    pub default: f64,
    /// An enum parameter's enum and its own default value.
    pub enum_id: Option<Id>,
    pub enum_default: Option<Id>,
    /// A variable parameter names a global variable (else a player's).
    pub global: bool,
    /// The per-element slot of Filtered Array and the like: evaluated per
    /// element with Current Array Element bound.
    pub lambda: bool,
    /// `m_89C93A57`: hidden; the text form leaves it out.
    pub hidden: bool,
}

/// An event, the condition or an action.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub graph: Option<u64>,
    pub params: Vec<Param>,
    /// `m_89C93A57`, which the validator refuses (error 19).
    pub hidden: bool,
}

#[derive(Clone, Debug)]
pub struct ValueDef {
    pub id: Id,
    pub name: String,
    pub returns: i64,
    pub params: Vec<Param>,
}

#[derive(Clone, Debug)]
pub struct EnumDef {
    pub id: Id,
    pub default: Option<Id>,
    pub values: Vec<(Id, String)>,
}

/// The catalog with its name lookups. Names match case-insensitively, as
/// the client's settings parser does (`sub_7FF789A8A0C0`).
#[derive(Debug, Default)]
pub struct Catalog {
    pub events: Vec<Entry>,
    pub conditions: Vec<Entry>,
    pub actions: Vec<Entry>,
    pub values: Vec<ValueDef>,
    pub enums: Vec<EnumDef>,
    /// The `String` value's choices, by index.
    pub strings: Vec<String>,
    value_by_id: HashMap<Id, usize>,
    value_by_name: HashMap<String, Id>,
    action_by_name: HashMap<String, u16>,
    event_by_name: HashMap<String, u8>,
    enum_by_id: HashMap<Id, usize>,
    string_by_text: HashMap<String, u16>,
    heroes: HashMap<String, u64>,
    hero_names: HashMap<u64, String>,
    maps: HashMap<String, u32>,
    map_names: HashMap<u32, String>,
    game_modes: HashMap<String, u64>,
    game_mode_names: HashMap<u64, String>,
    /// Loadout GUID -> its logical button number (Event Ability).
    loadout_buttons: HashMap<u64, i64>,
}

#[derive(Deserialize)]
struct RawParam {
    name: Option<String>,
    class: String,
    kind: u8,
    #[serde(default, rename = "type")]
    value_type: i64,
    default_value: Option<Id>,
    #[serde(default)]
    min: f64,
    #[serde(default)]
    max: f64,
    #[serde(default)]
    default: serde_json::Value,
    #[serde(rename = "enum")]
    enum_id: Option<Id>,
    enum_default: Option<Id>,
    #[serde(default)]
    global: bool,
    #[serde(default)]
    lambda: bool,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
struct RawEntry {
    name: Option<String>,
    graph: Option<String>,
    params: Vec<RawParam>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
struct RawValue {
    id: Id,
    name: Option<String>,
    returns: i64,
    params: Vec<RawParam>,
}

#[derive(Deserialize)]
struct RawEnumValue {
    id: Id,
    name: Option<String>,
}

#[derive(Deserialize)]
struct RawEnum {
    id: Id,
    default: Option<Id>,
    values: Vec<RawEnumValue>,
}

#[derive(Deserialize)]
struct Raw {
    events: Vec<RawEntry>,
    conditions: Vec<RawEntry>,
    actions: Vec<RawEntry>,
    values: Vec<RawValue>,
    enums: Vec<RawEnum>,
    strings: Vec<String>,
    #[serde(default)]
    heroes: HashMap<String, String>,
    #[serde(default)]
    maps: HashMap<String, u32>,
    #[serde(default)]
    game_modes: HashMap<String, String>,
    #[serde(default)]
    loadouts: HashMap<String, i64>,
}

impl Catalog {
    /// The catalog in the game data folder `data_dir` (the pack's `data/`).
    pub fn load(data_dir: &Path) -> io::Result<Self> {
        let path = data_dir.join(FILE);
        let bytes = fs::read(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_reader(bytes.as_slice())
    }

    pub fn from_reader(reader: impl Read) -> io::Result<Self> {
        let raw: Raw = serde_json::from_reader(reader)
            .map_err(|error| invalid(format!("invalid {FILE}: {error}")))?;
        let entries = |raw: Vec<RawEntry>| -> io::Result<Vec<Entry>> {
            raw.into_iter()
                .map(|entry| {
                    Ok(Entry {
                        name: entry.name.unwrap_or_default(),
                        graph: entry.graph.as_deref().map(parse_guid).transpose()?,
                        params: params(entry.params)?,
                        hidden: entry.hidden,
                    })
                })
                .collect()
        };
        let mut catalog = Catalog {
            events: entries(raw.events)?,
            conditions: entries(raw.conditions)?,
            actions: entries(raw.actions)?,
            values: raw
                .values
                .into_iter()
                .map(|value| {
                    Ok(ValueDef {
                        id: value.id,
                        name: value.name.unwrap_or_default(),
                        returns: value.returns,
                        params: params(value.params)?,
                    })
                })
                .collect::<io::Result<_>>()?,
            enums: raw
                .enums
                .into_iter()
                .map(|def| EnumDef {
                    id: def.id,
                    default: def.default,
                    values: def
                        .values
                        .into_iter()
                        .map(|value| (value.id, value.name.unwrap_or_default()))
                        .collect(),
                })
                .collect(),
            strings: raw.strings,
            ..Catalog::default()
        };
        for (index, value) in catalog.values.iter().enumerate() {
            catalog.value_by_id.insert(value.id, index);
            catalog
                .value_by_name
                .entry(fold(&value.name))
                .or_insert(value.id);
        }
        for (index, action) in catalog.actions.iter().enumerate() {
            catalog
                .action_by_name
                .entry(fold(&action.name))
                .or_insert(index as u16);
        }
        for (index, event) in catalog.events.iter().enumerate() {
            catalog
                .event_by_name
                .entry(fold(&event.name))
                .or_insert(index as u8);
        }
        for (index, def) in catalog.enums.iter().enumerate() {
            catalog.enum_by_id.insert(def.id, index);
        }
        for (index, text) in catalog.strings.iter().enumerate() {
            catalog
                .string_by_text
                .entry(fold(text))
                .or_insert(index as u16);
        }
        for (name, guid) in raw.heroes {
            let guid = parse_guid(&guid)?;
            catalog.hero_names.insert(guid, name.clone());
            catalog.heroes.insert(fold(&name), guid);
        }
        for (name, index) in raw.maps {
            catalog.map_names.insert(index, name.clone());
            catalog.maps.insert(fold(&name), index);
        }
        for (name, guid) in raw.game_modes {
            let guid = parse_guid(&guid)?;
            catalog.game_mode_names.insert(guid, name.clone());
            catalog.game_modes.insert(fold(&name), guid);
        }
        for (guid, button) in raw.loadouts {
            catalog.loadout_buttons.insert(parse_guid(&guid)?, button);
        }
        Ok(catalog)
    }

    /// The logical button number (the hero graphs' and the Workshop's
    /// button table's: 20 Primary Fire, 29 Secondary Fire, 31 Melee…) of
    /// the ability `loadout` (a `09E` GUID) is, if it has one.
    pub fn loadout_button(&self, loadout: u64) -> Option<i64> {
        self.loadout_buttons.get(&loadout).copied()
    }

    pub fn value(&self, id: Id) -> Option<&ValueDef> {
        self.value_by_id.get(&id).map(|&index| &self.values[index])
    }

    pub fn value_named(&self, name: &str) -> Option<&ValueDef> {
        self.value(*self.value_by_name.get(&fold(name))?)
    }

    pub fn action(&self, index: u16) -> Option<&Entry> {
        self.actions.get(usize::from(index))
    }

    pub fn action_named(&self, name: &str) -> Option<u16> {
        self.action_by_name.get(&fold(name)).copied()
    }

    pub fn event_named(&self, name: &str) -> Option<u8> {
        self.event_by_name.get(&fold(name)).copied()
    }

    pub fn enum_def(&self, id: Id) -> Option<&EnumDef> {
        self.enum_by_id.get(&id).map(|&index| &self.enums[index])
    }

    /// The value of enum `id` named `name`.
    pub fn enum_value(&self, id: Id, name: &str) -> Option<Id> {
        let name = fold(name);
        self.enum_def(id)?
            .values
            .iter()
            .find(|(_, value)| fold(value) == name)
            .map(|&(value, _)| value)
    }

    /// The name of an enum value, from whichever enum has it (enum values
    /// are shared identifiers: `506A` None is in eight enums).
    pub fn enum_value_name(&self, value: Id) -> Option<&str> {
        self.enums
            .iter()
            .flat_map(|def| def.values.iter())
            .find(|(id, _)| *id == value)
            .map(|(_, name)| name.as_str())
    }

    /// Every hero by name.
    pub fn heroes(&self) -> impl Iterator<Item = (&str, u64)> {
        self.hero_names
            .iter()
            .map(|(&guid, name)| (name.as_str(), guid))
    }

    /// Every map by name.
    pub fn maps(&self) -> impl Iterator<Item = (&str, u32)> {
        self.map_names
            .iter()
            .map(|(&index, name)| (name.as_str(), index))
    }

    /// Every game mode by name.
    pub fn game_modes(&self) -> impl Iterator<Item = (&str, u64)> {
        self.game_mode_names
            .iter()
            .map(|(&guid, name)| (name.as_str(), guid))
    }

    pub fn predefined_string(&self, text: &str) -> Option<u16> {
        self.string_by_text.get(&fold(text)).copied()
    }

    pub fn hero(&self, name: &str) -> Option<u64> {
        self.heroes.get(&fold(name)).copied()
    }

    pub fn hero_name(&self, guid: u64) -> Option<&str> {
        self.hero_names.get(&guid).map(String::as_str)
    }

    pub fn map(&self, name: &str) -> Option<u32> {
        self.maps.get(&fold(name)).copied()
    }

    pub fn map_name(&self, index: u32) -> Option<&str> {
        self.map_names.get(&index).map(String::as_str)
    }

    pub fn game_mode(&self, name: &str) -> Option<u64> {
        self.game_modes.get(&fold(name)).copied()
    }

    pub fn game_mode_name(&self, guid: u64) -> Option<&str> {
        self.game_mode_names.get(&guid).map(String::as_str)
    }
}

fn params(raw: Vec<RawParam>) -> io::Result<Vec<Param>> {
    raw.into_iter()
        .map(|param| {
            let default = match param.default {
                serde_json::Value::Bool(value) => f64::from(u8::from(value)),
                serde_json::Value::Number(number) => number.as_f64().unwrap_or(0.0),
                _ => 0.0,
            };
            Ok(Param {
                kind: ParamKind::of(&param.class, param.kind)?,
                name: param.name,
                class: param.class,
                default_value: param.default_value,
                value_type: param.value_type,
                min: param.min,
                max: param.max,
                default,
                enum_id: param.enum_id,
                enum_default: param.enum_default,
                global: param.global,
                lambda: param.lambda,
                hidden: param.hidden,
            })
        })
        .collect()
}

/// Names compare case-insensitively.
pub(crate) fn fold(name: &str) -> String {
    name.to_lowercase()
}

fn parse_guid(text: &str) -> io::Result<u64> {
    text.strip_prefix("0x")
        .and_then(|digits| u64::from_str_radix(digits, 16).ok())
        .ok_or_else(|| invalid(format!("bad GUID {text}")))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
