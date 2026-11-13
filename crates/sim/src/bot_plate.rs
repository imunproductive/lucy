//! The live nameplate tool: changes to what the clients learn about the
//! training bots' health bar and nameplate, made while a match runs (the
//! dashboard's Bot plate page and the `.plate` chat command,
//! `docs/user/playing.md`). It is for finding which input makes our
//! Training Bot's bar look unlike retail's (black segment outlines, a mark
//! before the name; `docs/notes/behavior.md`, "Nameplate and bar look").
//!
//! The inputs the server sends:
//! - Component 51, the health pools: three arrays (health, armour,
//!   shields) of [`POOLS`] parts, then five more fields (`u32`, `u32`, an
//!   entity, `u8`, `u8`; we never send them). A part is 20 bytes
//!   ([`orpheus_wire::entity::pool`]): `u32` id (0 = none), `f32` maximum,
//!   `f32` current, `u32` kind, then four flag bytes. Bytes 17 and 18
//!   choose the colour: the client draws a part as health, armour or
//!   shields only with byte 17 set and byte 18 clear, else as over-health
//!   in another colour (leafall903/ORPHEUS (MIT) `world.py`, client
//!   function 0x7FF7894CCEE0; checked in the game, 2026-11-12: tails
//!   `0.0.0.0` and `1.1.1.1` drew the segments turquoise). What bytes 16
//!   and 19 do is not known. The 0.8 beta's part (saturn-xvi/prometheus,
//!   facts only) has the same 20 bytes: an enabled word, maximum, current,
//!   a word, then flag bytes at 0x10 and 0x11, both 1 on a player's part.
//! - `0D24`'s shared variables and the body's variables `0D24` and its bar
//!   read ([`INPUTS`]), and the one game message `0D24` takes from the
//!   server ([`MESSAGES`]).
//!
//! A change reaches every client at once as an update: the pools as a
//! component 51 update of each live bot, the variables in the bots' next
//! script frame.
//! Nothing respawns; a bot that respawns gets the changes from its create
//! on.

use std::{collections::BTreeMap, fmt::Write as _};

use orpheus_script::{vitals::Vitals, wire::Value};
use orpheus_wire::entity::{Component, ComponentValue, POOLS, pool};

/// The three pool arrays of component 51, by their kind.
pub const ARRAYS: [&str; 3] = ["health", "armour", "shields"];
/// The fields of component 51 after its pool arrays.
const TRAILER: usize = 5;
const FIRST_TRAILER: usize = ARRAYS.len() * POOLS;
/// The id a part the bot doesn't have gets when the tool adds it and names
/// no id: this plus its field, so that each has its own.
const ADDED_PART_ID: u32 = 0x100;

/// A named input of the nameplate graph `0D24` the server can change.
#[derive(Clone, Copy, Debug)]
pub struct PlateInput {
    /// Its command name.
    pub name: &'static str,
    pub variable: PlateVariable,
    /// What `0D24` does with it.
    pub what: &'static str,
}

/// `0D24`'s shared variables (its sync list), by what its states do with
/// them, and the body's statuses its bar reads (graph `0D24` of 104319).
pub const INPUTS: [PlateInput; 20] = [
    PlateInput {
        name: "name",
        variable: PlateVariable::Nameplate(8812),
        what: "the name's text override: #17476 = #8812 || the controller's name (st31); we send the bot's name",
    },
    PlateInput {
        name: "enabled",
        variable: PlateVariable::Nameplate(8807),
        what: "the plate enabled (#17850, the root layout's 27A3); 0C3C sets it",
    },
    PlateInput {
        name: "nameonly",
        variable: PlateVariable::Nameplate(14680),
        what: "true: the name alone (st53, layout 0128) instead of the full plate (st42, layout 012B)",
    },
    PlateInput {
        name: "offset",
        variable: PlateVariable::Nameplate(8813),
        what: "the root layout's 68E9, with the dead flag (st35)",
    },
    PlateInput {
        name: "meter",
        variable: PlateVariable::Entity(6036),
        what: "the name's meter 3523 shows 1 - this (st17); not 0 takes st4's first case (st12: status icon 615B, or by #8838e/#9824e)",
    },
    PlateInput {
        name: "status",
        variable: PlateVariable::Entity(2143),
        what: "st4's default case (st5): status icon 23D1 25E9 (true) or 654A",
    },
    PlateInput {
        name: "antiheal",
        variable: PlateVariable::Entity(8202),
        what: "the bar's status icon 62B3 (st51): true 615C, the anti-heal icon over the plate (not the outline; seen in the game); Biotic Grenade's enemy aura (0C72 st6) holds it true",
    },
    PlateInput {
        name: "healboost",
        variable: PlateVariable::Entity(8203),
        what: "the bar's status icon 62B3 (st51): true (and #8202e not) 6084, the heal boost icon, neither 32F4 (none); Biotic Grenade's ally aura (0B99 st5) holds it true",
    },
    PlateInput {
        name: "entity.19452",
        variable: PlateVariable::Entity(19452),
        what: "the bar's 6872 (st51), a number; only 1763 writes it; 100 changed nothing seen in the game",
    },
    PlateInput {
        name: "entity.7209",
        variable: PlateVariable::Entity(7209),
        what: "st4's second case (st3): status icon 32F4",
    },
    PlateInput {
        name: "dead",
        variable: PlateVariable::Entity(478),
        what: "the dead flag: true hides the bar (#17549 = !#478e && ...)",
    },
    PlateInput {
        name: "entity.8836",
        variable: PlateVariable::Entity(8836),
        what: "a meter's part: #154 = #8836e / #8837e (st18)",
    },
    PlateInput {
        name: "entity.8837",
        variable: PlateVariable::Entity(8837),
        what: "a meter's whole: #154 = #8836e / #8837e (st18)",
    },
    PlateInput {
        name: "entity.8838",
        variable: PlateVariable::Entity(8838),
        what: "the name layout's 634A (#9932, st44) and the switch st12",
    },
    PlateInput {
        name: "entity.9824",
        variable: PlateVariable::Entity(9824),
        what: "st15: status icon 519A (true) or 62F0",
    },
    PlateInput {
        name: "entity.9841",
        variable: PlateVariable::Entity(9841),
        what: "a meter's part: #154 = #9841e / #9842e (st19)",
    },
    PlateInput {
        name: "entity.9842",
        variable: PlateVariable::Entity(9842),
        what: "a meter's whole: #154 = #9841e / #9842e (st19)",
    },
    PlateInput {
        name: "entity.11264",
        variable: PlateVariable::Entity(11264),
        what: "read by the bar's condition #17549 (st38)",
    },
    PlateInput {
        name: "entity.20017",
        variable: PlateVariable::Entity(20017),
        what: "read by #17549 (st67)",
    },
    PlateInput {
        name: "entity.29415",
        variable: PlateVariable::Entity(29415),
        what: "the switch st72: the plate's entity #8154 (st71)",
    },
];

/// The game message `0D24` takes from the server, by its `025` index: the
/// graph's one networked listener (st56).
pub const MESSAGES: [(u16, &str); 1] = [(0x0C3C, "#8807 true: the plate enabled (st57)")];

/// `0D24`'s other listeners, each a client-only state: only the clients'
/// own copy of `0D24` runs them, and no record carries a game message (the
/// server can only start a networked state that sends one,
/// `docs/notes/ow1-preservation.md`, "The game-message bus is local"). So
/// the server can't send these to a nameplate; the tool refuses them.
const CLIENT_ONLY_MESSAGES: [(u16, &str); 10] = [
    (
        0x0744,
        "st1 (#9162: hides the name); the networked states that send it to their own body are 0D6E st14 (Mei's Snowball Offensive body script), 0CBC st49 and 1930 st4, which a training bot doesn't run",
    ),
    (0x0768, "st8 (#9374)"),
    (0x0773, "st9 (#9489)"),
    (0x0841, "st20 (#11003)"),
    (0x08A0, "st22"),
    (0x094E, "st24 (status icon 615B)"),
    (0x09CC, "st25 (#13399)"),
    (0x0C23, "st36 (#17386)"),
    (0x06FD, "st48 (#6036)"),
    (0x13EA, "st74 (#31382)"),
];

/// A variable of a bot the tool sets: the body's own (an entity variable)
/// or one of each nameplate (`0D24`) instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PlateVariable {
    Entity(u32),
    Nameplate(u32),
}

impl PlateVariable {
    /// Its command name: an [`INPUTS`] name, else `entity.<id>` or
    /// `plate.<id>`.
    pub fn name(self) -> String {
        match INPUTS.iter().find(|input| input.variable == self) {
            Some(input) => input.name.to_owned(),
            None => match self {
                Self::Entity(id) => format!("entity.{id}"),
                Self::Nameplate(id) => format!("plate.{id}"),
            },
        }
    }
}

/// One attribute of a pool part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PartAttribute {
    Id,
    Max,
    Now,
    Kind,
    /// One of the flag bytes, 16 to 19.
    Byte(usize),
}

const ATTRIBUTES: [PartAttribute; 8] = [
    PartAttribute::Id,
    PartAttribute::Max,
    PartAttribute::Now,
    PartAttribute::Kind,
    PartAttribute::Byte(16),
    PartAttribute::Byte(17),
    PartAttribute::Byte(18),
    PartAttribute::Byte(19),
];

impl PartAttribute {
    fn name(self) -> String {
        match self {
            Self::Id => "id".to_owned(),
            Self::Max => "max".to_owned(),
            Self::Now => "now".to_owned(),
            Self::Kind => "kind".to_owned(),
            Self::Byte(byte) => format!("b{byte}"),
        }
    }
}

/// The changes to one pool part; none leaves an attribute as the bot has it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct PartTuning {
    id: Option<u32>,
    max: Option<f32>,
    now: Option<f32>,
    kind: Option<u32>,
    bytes: [Option<u8>; 4],
}

impl PartTuning {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn get(&self, attribute: PartAttribute) -> Option<f64> {
        match attribute {
            PartAttribute::Id => self.id.map(f64::from),
            PartAttribute::Max => self.max.map(f64::from),
            PartAttribute::Now => self.now.map(f64::from),
            PartAttribute::Kind => self.kind.map(f64::from),
            PartAttribute::Byte(byte) => self.bytes[byte - 16].map(f64::from),
        }
    }

    fn set(&mut self, attribute: PartAttribute, value: Option<f64>) -> Result<(), String> {
        let whole = |value: f64, most: f64| {
            (value.fract() == 0.0 && (0.0..=most).contains(&value))
                .then_some(value)
                .ok_or_else(|| {
                    format!(
                        "{} must be a whole number from 0 to {most}",
                        attribute.name()
                    )
                })
        };
        match attribute {
            PartAttribute::Id | PartAttribute::Kind => {
                let value = value
                    .map(|value| whole(value, f64::from(u32::MAX)))
                    .transpose()?
                    .map(|value| value as u32);
                if attribute == PartAttribute::Id {
                    self.id = value;
                } else {
                    self.kind = value;
                }
            }
            PartAttribute::Max | PartAttribute::Now => {
                let value = value
                    .map(|value| {
                        (value.is_finite() && value.abs() <= f64::from(f32::MAX))
                            .then_some(value as f32)
                            .ok_or_else(|| format!("{} must be a number", attribute.name()))
                    })
                    .transpose()?;
                if attribute == PartAttribute::Max {
                    self.max = value;
                } else {
                    self.now = value;
                }
            }
            PartAttribute::Byte(byte) => {
                self.bytes[byte - 16] = value
                    .map(|value| whole(value, 255.0))
                    .transpose()?
                    .map(|value| value as u8);
            }
        }
        Ok(())
    }

    fn apply(&self, part: &mut [u8; 20]) {
        if let Some(id) = self.id {
            part[..4].copy_from_slice(&id.to_le_bytes());
        }
        if let Some(max) = self.max {
            part[4..8].copy_from_slice(&max.to_le_bytes());
        }
        if let Some(now) = self.now {
            part[8..12].copy_from_slice(&now.to_le_bytes());
        }
        if let Some(kind) = self.kind {
            part[12..16].copy_from_slice(&kind.to_le_bytes());
        }
        for (byte, value) in part[16..].iter_mut().zip(self.bytes) {
            if let Some(value) = value {
                *byte = value;
            }
        }
    }
}

/// One change to the plate.
#[derive(Clone, Debug, PartialEq)]
pub enum PlateChange {
    /// Back to what the bots have of themselves.
    Reset,
    /// One attribute of the part in a field of component 51 (`array *
    /// POOLS + slot`); none: back to the bot's own.
    Part {
        field: usize,
        attribute: PartAttribute,
        value: Option<f64>,
    },
    /// One of component 51's fields after the pools (48 to 52); none: no
    /// longer sent (a field once sent goes as 0).
    Trailer {
        field: usize,
        value: Option<u32>,
    },
    /// A variable; none: back to what the bot had.
    Variable {
        variable: PlateVariable,
        value: Option<Value>,
    },
    /// A game message to every bot's nameplate instances, once.
    Message(u64),
    Many(Vec<PlateChange>),
}

impl PlateChange {
    /// Read changes as the chat command and the dashboard write them, one
    /// or more separated by `;`:
    /// - `reset`;
    /// - `<array>[.<slot>].<attribute> <value>`: array `health`, `armour`
    ///   or `shields`, slot 0 to 15 (0 when left out), attribute `id`,
    ///   `max`, `now`, `kind` or `b16` to `b19` (the flag bytes);
    ///   `<array>[.<slot>].tail <b16>.<b17>.<b18>.<b19>` sets the four
    ///   bytes; `<array>[.<slot>] default` drops every change of the part;
    /// - `c51.<48 to 52> <value>`: component 51's fields after the pools;
    /// - `<input> <value>` with an [`INPUTS`] name, or `entity.<id>` /
    ///   `plate.<id>` for any variable of the body or of its nameplate
    ///   instances: `null`, `true`, `false`, a whole number, a number with
    ///   a point, `0x` and 16 hex digits (an asset), or text (in quotes
    ///   when it would read as one of those);
    /// - `message <id>`: a game message, its `025` index (`0C3C`) or whole
    ///   GUID; the ones `0D24` waits for only on the client are refused;
    /// - `hero ...` is refused ([`HERO_REFUSED`]).
    ///
    /// `default` as a value takes the change back.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut changes = text
            .split(';')
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(Self::parse_one)
            .collect::<Result<Vec<_>, _>>()?;
        match changes.len() {
            0 => Err("No change given.".to_owned()),
            1 => Ok(changes.remove(0)),
            _ => Ok(Self::Many(changes)),
        }
    }

    /// The game messages it sends, in order.
    pub fn messages(&self) -> Vec<u64> {
        match self {
            Self::Message(message) => vec![*message],
            Self::Many(changes) => changes.iter().flat_map(Self::messages).collect(),
            _ => Vec::new(),
        }
    }

    fn parse_one(command: &str) -> Result<Self, String> {
        let (name, value) = command
            .split_once(char::is_whitespace)
            .map_or((command, ""), |(name, value)| (name, value.trim()));
        let name = name.to_ascii_lowercase();
        if name == "reset" {
            return Ok(Self::Reset);
        }
        if name == "message" {
            return parse_message(value).map(Self::Message);
        }
        if name == "hero" {
            return Err(HERO_REFUSED.to_owned());
        }
        if let Some(input) = INPUTS.iter().find(|input| input.name == name) {
            return Ok(Self::Variable {
                variable: input.variable,
                value: parse_value(value)?,
            });
        }
        let mut parts = name.split('.');
        let head = parts.next().unwrap_or_default();
        let rest: Vec<&str> = parts.collect();
        let number = |text: &str| {
            text.parse::<u32>()
                .map_err(|_| format!("'{text}' is not a number."))
        };
        match (head, rest.as_slice()) {
            ("entity", [id]) => Ok(Self::Variable {
                variable: PlateVariable::Entity(number(id)?),
                value: parse_value(value)?,
            }),
            ("plate", [id]) => Ok(Self::Variable {
                variable: PlateVariable::Nameplate(number(id)?),
                value: parse_value(value)?,
            }),
            ("c51", [field]) => {
                let field = number(field)? as usize;
                if !(FIRST_TRAILER..FIRST_TRAILER + TRAILER).contains(&field) {
                    return Err(format!("Component 51 has fields 48 to 52, not {field}."));
                }
                let most = if field >= FIRST_TRAILER + 3 {
                    u32::from(u8::MAX)
                } else {
                    u32::MAX
                };
                let value = match value {
                    "default" => None,
                    text => Some(
                        parse_number(text)?
                            .filter(|&value| value <= most)
                            .ok_or_else(|| format!("c51.{field} takes 0 to {most}."))?,
                    ),
                };
                Ok(Self::Trailer { field, value })
            }
            (array, rest) => {
                let kind = ARRAYS
                    .iter()
                    .position(|name| *name == array || (array == "armor" && *name == "armour"))
                    .ok_or_else(|| format!("Unknown setting '{name}'. Try .plate help."))?;
                let (slot, attribute) = match rest {
                    [] => (0, None),
                    [slot] if slot.parse::<usize>().is_ok() => (number(slot)? as usize, None),
                    [attribute] => (0, Some(*attribute)),
                    [slot, attribute] => (number(slot)? as usize, Some(*attribute)),
                    _ => return Err(format!("Unknown setting '{name}'.")),
                };
                if slot >= POOLS {
                    return Err(format!("A pool array has slots 0 to {}.", POOLS - 1));
                }
                let field = kind * POOLS + slot;
                let part = |attribute, value| Self::Part {
                    field,
                    attribute,
                    value,
                };
                match attribute {
                    None if value == "default" => Ok(Self::Many(
                        ATTRIBUTES
                            .iter()
                            .map(|&attribute| part(attribute, None))
                            .collect(),
                    )),
                    None => Err(format!(
                        "Say which attribute of {name}: id, max, now, kind, b16 to b19 or tail."
                    )),
                    Some("tail") => {
                        let bytes: Vec<Option<f64>> = if value == "default" {
                            vec![None; 4]
                        } else {
                            value
                                .split('.')
                                .map(|byte| byte.parse::<f64>().map(Some))
                                .collect::<Result<_, _>>()
                                .map_err(|_| "A tail is four numbers like 0.1.0.1.".to_owned())?
                        };
                        if bytes.len() != 4 {
                            return Err("A tail is four numbers like 0.1.0.1.".to_owned());
                        }
                        Ok(Self::Many(
                            bytes
                                .into_iter()
                                .enumerate()
                                .map(|(index, value)| part(PartAttribute::Byte(16 + index), value))
                                .collect(),
                        ))
                    }
                    Some(attribute) => {
                        let attribute = ATTRIBUTES
                            .into_iter()
                            .find(|known| known.name() == attribute)
                            .ok_or_else(|| format!("Unknown part attribute '{attribute}'."))?;
                        let value = match value {
                            "default" => None,
                            text => Some(
                                text.parse::<f64>()
                                    .map_err(|_| format!("'{text}' is not a number."))?,
                            ),
                        };
                        Ok(part(attribute, value))
                    }
                }
            }
        }
    }
}

fn parse_number(text: &str) -> Result<Option<u32>, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed
        .map(Some)
        .map_err(|_| format!("'{text}' is not a whole number."))
}

fn parse_message(text: &str) -> Result<u64, String> {
    let digits = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .split('.')
        .next()
        .unwrap_or_default();
    let id =
        u64::from_str_radix(digits, 16).map_err(|_| format!("'{text}' is not a message id."))?;
    let id = if id <= 0xFFFF {
        0x0240_0000_0000_0000 | id
    } else {
        id
    };
    if let Some((index, wait)) = CLIENT_ONLY_MESSAGES
        .iter()
        .find(|(index, _)| 0x0240_0000_0000_0000 | u64::from(*index) == id)
    {
        return Err(format!(
            "0D24 waits for {index:04X} only on the client, in {wait}: the server can't send it to a nameplate."
        ));
    }
    Ok(id)
}

/// Why the tool has no `hero` command. It put component 75 (the hero, as
/// a player's: hero, no skin, none, hero) on each bot's stand-in
/// controller so `0D24` would draw the plate's hero icon (`4447`); in the
/// game (2026-11-13) `hero bot` and `hero 013B` crashed the client. That
/// stand-in was a player entity without component 74 that possesses a
/// training bot body: assumption, the client sets up the hero of a
/// non-participant for that body and fails. Not checked: no IDA. The
/// stand-in is a participant now and carries the bot's own hero from its
/// create (`scene.rs` `bot_controller_create`), so another hero has no use.
const HERO_REFUSED: &str = "The hero command is gone: component 75 on a bot's stand-in controller crashed the game; the stand-in now has the bot's own hero.";

/// A variable's value as the commands write it; none for `default`.
fn parse_value(text: &str) -> Result<Option<Value>, String> {
    Ok(Some(match text {
        "" => return Err("Give a value, or default.".to_owned()),
        "default" => return Ok(None),
        "null" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        text if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') => {
            Value::String(text[1..text.len() - 1].to_owned())
        }
        text if text.starts_with("0x") || text.starts_with("0X") => Value::Asset(
            u64::from_str_radix(&text[2..], 16).map_err(|_| format!("'{text}' is not a GUID."))?,
        ),
        text => match (text.parse::<i64>(), text.parse::<f64>()) {
            (Ok(integer), _) => Value::Integer(integer),
            (_, Ok(float)) if float.is_finite() => Value::Float(float),
            _ => Value::String(text.to_owned()),
        },
    }))
}

/// A value as the commands write it.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Float(value) if value.fract() == 0.0 => format!("{value:.1}"),
        Value::Float(value) => value.to_string(),
        Value::Asset(guid) => format!("0x{guid:016X}"),
        Value::String(text) => format!("\"{text}\""),
        other => format!("{other:?}"),
    }
}

/// The changes the tool makes to every training bot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BotPlate {
    /// By component 51 field.
    parts: BTreeMap<usize, PartTuning>,
    trailer: [Option<u32>; TRAILER],
    variables: BTreeMap<PlateVariable, Value>,
}

/// One pool part as a bot sends it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartView {
    /// Its component 51 field: `array * POOLS + slot`.
    pub field: usize,
    pub id: u32,
    pub max: f32,
    pub now: f32,
    pub kind: u32,
    /// Bytes 16 to 19.
    pub bytes: [u8; 4],
    /// The tool changes it.
    pub tuned: bool,
}

impl PartView {
    /// `health.0`, `armour.2`, …
    pub fn name(&self) -> String {
        format!("{}.{}", ARRAYS[self.field / POOLS], self.field % POOLS)
    }
}

impl BotPlate {
    /// Apply a change. A message is no change to the plate: the match sends
    /// it.
    pub fn change(&mut self, change: PlateChange) -> Result<(), String> {
        match change {
            PlateChange::Reset => *self = Self::default(),
            PlateChange::Part {
                field,
                attribute,
                value,
            } => {
                if field >= FIRST_TRAILER {
                    return Err(format!("Component 51 has no pool part {field}."));
                }
                let part = self.parts.entry(field).or_default();
                let result = part.set(attribute, value);
                if part.is_empty() {
                    self.parts.remove(&field);
                }
                result?;
            }
            PlateChange::Trailer { field, value } => {
                let slot = field
                    .checked_sub(FIRST_TRAILER)
                    .filter(|&slot| slot < TRAILER)
                    .ok_or_else(|| format!("Component 51 has fields 48 to 52, not {field}."))?;
                self.trailer[slot] = value;
            }
            PlateChange::Variable { variable, value } => match value {
                Some(value) => {
                    self.variables.insert(variable, value);
                }
                None => {
                    self.variables.remove(&variable);
                }
            },
            PlateChange::Message(_) => {}
            PlateChange::Many(changes) => {
                for change in changes {
                    self.change(change)?;
                }
            }
        }
        Ok(())
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The variables it sets, in order.
    pub fn variables(&self) -> impl Iterator<Item = (PlateVariable, &Value)> {
        self.variables
            .iter()
            .map(|(&variable, value)| (variable, value))
    }

    /// The changes in force, as commands.
    pub fn commands(&self) -> Vec<String> {
        let mut commands = Vec::new();
        for (&field, part) in &self.parts {
            let name = format!("{}.{}", ARRAYS[field / POOLS], field % POOLS);
            for attribute in ATTRIBUTES {
                if let Some(value) = part.get(attribute) {
                    commands.push(format!("{name}.{} {value}", attribute.name()));
                }
            }
        }
        for (slot, value) in self.trailer.iter().enumerate() {
            if let Some(value) = value {
                commands.push(format!("c51.{} {value}", FIRST_TRAILER + slot));
            }
        }
        for (variable, value) in self.variables() {
            commands.push(format!("{} {}", variable.name(), value_text(value)));
        }
        commands
    }

    /// The bot's component 51 with the changes.
    pub fn pools(&self, vitals: &Vitals) -> Component {
        let mut pools = vitals.component();
        if self.parts.is_empty() && self.trailer == [None; TRAILER] {
            return pools;
        }
        let real = pools.clone();
        for &field in self.parts.keys() {
            pools.fields[field] = self.part(field, &real).map(ComponentValue::Blob20);
        }
        self.fill_trailer(&mut pools, &[None; TRAILER]);
        pools
    }

    /// What a live bot's pools need now that the plate was `previous`: each
    /// part either changes, as it is now (a part the tool added and no
    /// longer has goes as cleared), and the fields after the pools either
    /// sets. None when the pools are as they were.
    pub fn pool_update(&self, previous: &Self, vitals: &Vitals) -> Option<Component> {
        if self.parts == previous.parts && self.trailer == previous.trailer {
            return None;
        }
        let real = vitals.component();
        let mut pools = Component {
            index: real.index,
            fields: vec![None; FIRST_TRAILER],
        };
        for &field in self.parts.keys().chain(previous.parts.keys()) {
            pools.fields[field] = Some(ComponentValue::Blob20(
                self.part(field, &real).unwrap_or([0; 20]),
            ));
        }
        self.fill_trailer(&mut pools, &previous.trailer);
        Some(pools)
    }

    /// Every part the bot sends, in field order.
    pub fn parts(&self, vitals: &Vitals) -> Vec<PartView> {
        let real = vitals.component();
        (0..FIRST_TRAILER)
            .filter_map(|field| {
                let part = self.part(field, &real)?;
                let word = |at: usize| [part[at], part[at + 1], part[at + 2], part[at + 3]];
                Some(PartView {
                    field,
                    id: u32::from_le_bytes(word(0)),
                    max: f32::from_le_bytes(word(4)),
                    now: f32::from_le_bytes(word(8)),
                    kind: u32::from_le_bytes(word(12)),
                    bytes: word(16),
                    tuned: self.parts.contains_key(&field),
                })
            })
            .collect()
    }

    /// The part a field holds with the changes; a part the bot doesn't have
    /// starts as an empty part of the array's kind with its own id.
    fn part(&self, field: usize, real: &Component) -> Option<[u8; 20]> {
        let own = match real.fields.get(field) {
            Some(Some(ComponentValue::Blob20(part))) if part[..4] != [0; 4] => Some(*part),
            _ => None,
        };
        let Some(tuning) = self.parts.get(&field) else {
            return own;
        };
        let mut part = own.unwrap_or_else(|| {
            pool(
                ADDED_PART_ID + field as u32,
                (field / POOLS) as u32,
                0.0,
                0.0,
            )
        });
        tuning.apply(&mut part);
        Some(part)
    }

    /// The fields after the pools that this plate or `previous` sets; one
    /// only `previous` set goes as 0.
    fn fill_trailer(&self, pools: &mut Component, previous: &[Option<u32>; TRAILER]) {
        for (slot, previous) in previous.iter().enumerate() {
            let Some(value) = self.trailer[slot].or(previous.map(|_| 0)) else {
                continue;
            };
            pools
                .fields
                .resize(pools.fields.len().max(FIRST_TRAILER + slot + 1), None);
            pools.fields[FIRST_TRAILER + slot] = Some(match slot {
                0 | 1 => ComponentValue::U32(value),
                2 => ComponentValue::Entity(value),
                _ => ComponentValue::U8(value as u8),
            });
        }
    }
}

/// What the tool shows: the changes in force and, while a match has a live
/// training bot, what the first one sends now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlateView {
    pub commands: Vec<String>,
    pub bot: Option<PlateSample>,
}

/// A live bot's plate inputs as it sends them now.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateSample {
    pub entity: u32,
    pub name: String,
    pub parts: Vec<PartView>,
    /// [`INPUTS`] and every other variable the plate sets: what each reads
    /// now (none: never set) and whether the tool sets it.
    pub variables: Vec<(PlateVariable, Option<Value>, bool)>,
}

impl PlateView {
    /// A few lines for the chat.
    pub fn summary(&self) -> String {
        let mut text = String::new();
        if self.commands.is_empty() {
            text.push_str("No changes.");
        } else {
            let _ = write!(text, "Changes: {}.", self.commands.join("; "));
        }
        match &self.bot {
            None => text.push_str(" No live training bot."),
            Some(bot) => {
                for part in &bot.parts {
                    let [a, b, c, d] = part.bytes;
                    let _ = write!(
                        text,
                        " {} id {} {}/{} kind {} tail {a}.{b}.{c}.{d}.",
                        part.name(),
                        part.id,
                        part.now,
                        part.max,
                        part.kind
                    );
                }
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_wire::entity::{ARMOUR_POOL, HEALTH_POOL};

    fn blob(pools: &Component, field: usize) -> Option<[u8; 20]> {
        match pools.fields.get(field) {
            Some(Some(ComponentValue::Blob20(part))) => Some(*part),
            Some(None) | None => None,
            Some(Some(other)) => panic!("field {field} is {other:?}"),
        }
    }

    fn change(plate: &mut BotPlate, command: &str) {
        plate.change(PlateChange::parse(command).unwrap()).unwrap();
    }

    /// Our pool part: id, maximum, current, kind, then bytes 16 to 19
    /// 0, 1, 0, 1 (byte 17 set and 18 clear: drawn in the array's colour).
    #[test]
    fn a_pool_part_is_id_maximum_current_kind_and_four_flag_bytes() {
        let part = pool(3, ARMOUR_POOL as u32, 200.0, 150.0);
        assert_eq!(part[..4], 3u32.to_le_bytes());
        assert_eq!(part[4..8], 200f32.to_le_bytes());
        assert_eq!(part[8..12], 150f32.to_le_bytes());
        assert_eq!(part[12..16], 1u32.to_le_bytes());
        assert_eq!(part[16..], [0, 1, 0, 1]);
    }

    /// Each attribute of a part changes on its own; a part the bot doesn't
    /// have is added with its own id; `default` takes a change back, and an
    /// untouched plate sends the bot's pools as they are.
    #[test]
    fn the_tool_changes_each_attribute_of_a_part_on_its_own() {
        let mut vitals = Vitals::new(200.0, 0.0, 0.0);
        let death = orpheus_script::vitals::Death {
            time: 0.0,
            frame: 0,
            killer: 0,
        };
        vitals.damage(50.0, death);
        let mut plate = BotPlate::default();
        assert_eq!(plate.pools(&vitals), vitals.component());

        change(&mut plate, "health.b19 0");
        let mut expected = pool(1, HEALTH_POOL as u32, 200.0, 150.0);
        expected[19] = 0;
        assert_eq!(blob(&plate.pools(&vitals), 0), Some(expected));

        change(
            &mut plate,
            "health.0.id 7; health.max 250; health.now 25.5; health.kind 2",
        );
        change(&mut plate, "health.tail 1.1.0.0");
        let mut expected = pool(7, 2, 250.0, 25.5);
        expected[16..].copy_from_slice(&[1, 1, 0, 0]);
        assert_eq!(blob(&plate.pools(&vitals), 0), Some(expected));

        change(&mut plate, "armour.1.max 50; armour.1.now 50");
        let added = ARMOUR_POOL * POOLS + 1;
        assert_eq!(
            blob(&plate.pools(&vitals), added),
            Some(pool(0x100 + added as u32, ARMOUR_POOL as u32, 50.0, 50.0))
        );
        assert_eq!(
            plate.commands(),
            [
                "health.0.id 7",
                "health.0.max 250",
                "health.0.now 25.5",
                "health.0.kind 2",
                "health.0.b16 1",
                "health.0.b17 1",
                "health.0.b18 0",
                "health.0.b19 0",
                "armour.1.max 50",
                "armour.1.now 50",
            ]
        );

        change(&mut plate, "health default; armour.1 default");
        assert!(plate.is_default());
        assert_eq!(plate.pools(&vitals), vitals.component());
    }

    /// A change goes to the live bots as one component 51 update: the
    /// parts either plate changes (a part only the old plate added, as
    /// cleared) and the fields after the pools; none when the pools are
    /// the same.
    #[test]
    fn a_pool_change_updates_the_parts_it_touches_and_clears_a_dropped_one() {
        let vitals = Vitals::new(200.0, 0.0, 0.0);
        let mut before = BotPlate::default();
        change(&mut before, "shields.max 25; c51.51 1");
        let mut after = BotPlate::default();
        change(&mut after, "health.b19 0; meter 1");

        let update = after.pool_update(&before, &vitals).unwrap();
        let mut health = pool(1, HEALTH_POOL as u32, 200.0, 200.0);
        health[19] = 0;
        assert_eq!(blob(&update, 0), Some(health));
        assert_eq!(blob(&update, 2 * POOLS), Some([0; 20]));
        assert_eq!(update.fields[51], Some(ComponentValue::U8(0)));
        let sent: Vec<usize> = (0..update.fields.len())
            .filter(|&field| update.fields[field].is_some())
            .collect();
        assert_eq!(sent, [0, 2 * POOLS, 51]);

        let mut variables_only = after.clone();
        change(&mut variables_only, "status true");
        assert_eq!(variables_only.pool_update(&after, &vitals), None);
    }

    /// The variables and messages as the chat and the dashboard write
    /// them; a bad command changes nothing.
    #[test]
    fn the_commands_name_variables_messages_and_values() {
        let variable = |command: &str| match PlateChange::parse(command).unwrap() {
            PlateChange::Variable { variable, value } => (variable, value),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            variable("name Training Bot"),
            (
                PlateVariable::Nameplate(8812),
                Some(Value::String("Training Bot".to_owned()))
            )
        );
        assert_eq!(
            variable("name \"null\""),
            (
                PlateVariable::Nameplate(8812),
                Some(Value::String("null".to_owned()))
            )
        );
        assert_eq!(variable("name null").1, Some(Value::Null));
        assert_eq!(variable("meter 0.5").1, Some(Value::Float(0.5)));
        assert_eq!(variable("METER 1").1, Some(Value::Integer(1)));
        assert_eq!(
            variable("entity.9824 true"),
            (PlateVariable::Entity(9824), Some(Value::Bool(true)))
        );
        assert_eq!(
            variable("plate.8813 0x0D8000000000654A"),
            (
                PlateVariable::Nameplate(8813),
                Some(Value::Asset(0x0D80_0000_0000_654A))
            )
        );
        assert_eq!(variable("status default").1, None);
        // 0D24 waits for 0744 only on the client (st1): the server can't
        // send it, so the tool says so instead of sending it to no one.
        let refused = PlateChange::parse("message 0744").unwrap_err();
        assert!(refused.contains("only on the client"), "{refused}");
        assert_eq!(
            PlateChange::parse("message 0C3C"),
            Ok(PlateChange::Message(0x0240_0000_0000_0C3C))
        );
        assert_eq!(
            variable("antiheal true"),
            (PlateVariable::Entity(8202), Some(Value::Bool(true)))
        );
        assert_eq!(variable("healboost true").0, PlateVariable::Entity(8203));
        assert_eq!(
            PlateChange::parse("message 0x0240000000000C3C"),
            Ok(PlateChange::Message(0x0240_0000_0000_0C3C))
        );
        assert_eq!(PlateChange::parse("reset"), Ok(PlateChange::Reset));

        let mut plate = BotPlate::default();
        change(&mut plate, "status true; entity.9824 1; name \"Bot\"");
        assert_eq!(
            plate.commands(),
            ["status true", "entity.9824 1", "name \"Bot\""]
        );
        let refused = PlateChange::parse("hero bot").unwrap_err();
        assert!(refused.contains("crashed the game"), "{refused}");
        for bad in [
            "",
            "health.16.id 1",
            "health.b20 1",
            "health.b16 256",
            "health.id -1",
            "health.max x",
            "health.tail 1.1",
            "c51.47 1",
            "c51.52 300",
            "meter",
            "bogus 1",
            "message zz",
            "message 13EA",
            "hero bot",
            "hero 013B",
        ] {
            let mut tried = plate.clone();
            let result = PlateChange::parse(bad).and_then(|change| tried.change(change));
            assert!(result.is_err(), "{bad:?} was taken");
        }
        change(&mut plate, "reset");
        assert!(plate.is_default());
    }
}
