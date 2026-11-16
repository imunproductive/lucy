//! The marker messages, on with `ORPHEUS_DEBUG_SEND=1` (off by default: the
//! routes do not exist then). Research sends a game link or lobby message
//! with chosen field values to a connected game, to find which of the
//! client's code reads a field.
//!
//! - `GET /api/debug/messages?id=<id>`: the message's layout in schema
//!   order, `{"id", "link": "game"|"lobby", "name", "fields": [{"index",
//!   "name", "off", "type"}]}`; a struct field lists its members the same
//!   way in `"fields"`.
//! - `POST /api/debug/send` `{"id", "account"?, "fields"?: {"<name or
//!   #index>": value}, "count"?}`: encodes the message with the schema
//!   encoder (fields not given take the schema's zero or empty value) and
//!   sends it `count` times: a game link message to the account's game in
//!   its match, queued as the server's own reliable messages; a lobby
//!   message through the account's lobby session, at its next wake. The
//!   answer lists every field's value as sent.
//!
//! A field is named `+0x<off>` (the schemas have no other names) or
//! `#<index>`, its place in the schema's list; a dotted path names a struct
//! member (`#1.#0`, `+0x80.+0x0`). Values: integers (JSON numbers, or text
//! in decimal or `0x` hex), floats, booleans, text for strings, hex text for
//! blobs, a JSON list for arrays, an object of members for a struct, and
//! `"self"` (the account's possessed body entity) and `"controller"` (its
//! player entity) for integers. A struct with one integer member (an entity
//! reference) also takes that member's value directly.

use std::{collections::BTreeMap, sync::LazyLock};

use serde_json::{Map, Value as Json, json};

use super::api::{Api, ApiError, ApiResult};
use crate::{game_service::DebugSeat, lobby::Lobby};
use orpheus_lobby::accounts::account_id_for;
use orpheus_wire::{
    jam::codec::{Field, Fields, Schemas, Value},
    messages,
};

pub(super) const MESSAGES: &str = "/api/debug/messages";
pub(super) const SEND: &str = "/api/debug/send";
/// The most repeats of one send.
const MAX_COUNT: i128 = 64;
const SEND_KEYS: [&str; 4] = ["id", "account", "fields", "count"];
const BOOL: u8 = 0;
const F32: u8 = 10;
const F64: u8 = 11;
const STRUCT: u8 = 14;
const BLOB: u8 = 15;

static GAME_SCHEMAS: LazyLock<Schemas> = LazyLock::new(Schemas::game);

/// Whether `ORPHEUS_DEBUG_SEND=1` turns the routes on; it logs a warning
/// when it does.
pub(in crate::server) fn enabled() -> bool {
    let on = std::env::var("ORPHEUS_DEBUG_SEND").is_ok_and(|value| value == "1");
    if on {
        tracing::warn!("ORPHEUS_DEBUG_SEND: the dashboard's marker message routes are on");
    }
    on
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Game,
    Lobby,
}

impl Link {
    fn name(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Lobby => "lobby",
        }
    }
}

/// A message's link, protocol and layout.
struct Layout<'a> {
    id: u32,
    link: Link,
    crc: u32,
    schemas: &'a Schemas,
    fields: &'a [Field],
}

impl Layout<'_> {
    fn name(&self) -> String {
        format!("{:08X}/{}", self.crc, self.id)
    }
}

/// The message's layout: a game link message, else a lobby one (the two
/// links' ids do not overlap). `lobby` is none when the lobby is not
/// running.
fn layout(lobby: Option<&Schemas>, id: u32) -> Result<Layout<'_>, ApiError> {
    if let Ok((_, crc, _)) = messages::locate(id) {
        if let Ok(fields) = GAME_SCHEMAS.fields(crc, id) {
            return Ok(Layout {
                id,
                link: Link::Game,
                crc,
                schemas: &GAME_SCHEMAS,
                fields,
            });
        }
    }
    let schemas = lobby.ok_or_else(|| ApiError::new(503, "The lobby is not running."))?;
    let crc = schemas.protocol_of(id).ok_or_else(|| {
        ApiError::new(
            404,
            format!("No message {id} on the game link or the lobby"),
        )
    })?;
    let fields = schemas.fields(crc, id).expect("the protocol holds the id");
    Ok(Layout {
        id,
        link: Link::Lobby,
        crc,
        schemas,
        fields,
    })
}

/// `GET /api/debug/messages?id=<id>`.
pub(super) fn messages(lobby: Option<&Schemas>, query: &BTreeMap<String, String>) -> ApiResult {
    let id = query
        .get("id")
        .ok_or_else(|| bad("Give the message id: ?id=25000"))?;
    let layout = layout(lobby, message_id(&Json::String(id.clone()))?)?;
    Ok(json!({
        "id": layout.id,
        "link": layout.link.name(),
        "name": layout.name(),
        "fields": describe(layout.fields),
    }))
}

fn describe(fields: &[Field]) -> Vec<Json> {
    fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let mut row = json!({
                "index": index,
                "name": field_name(field),
                "off": field.offset,
                "type": type_name(field),
            });
            if field.kind == STRUCT {
                row["fields"] = Json::Array(describe(&field.fields));
            }
            row
        })
        .collect()
}

fn field_name(field: &Field) -> String {
    format!("+0x{:X}", field.offset)
}

fn type_name(field: &Field) -> String {
    let scalar = match field.kind {
        BOOL => "bool",
        1 | 2 => "u8",
        3 => "i8",
        4 => "u16",
        5 => "i16",
        6 => "u32",
        7 => "i32",
        8 => "u64",
        9 => "i64",
        F32 => "f32",
        F64 => "f64",
        12 | 13 => "string",
        STRUCT => "struct",
        BLOB => "blob",
        _ => "unknown",
    };
    if field.counted() {
        format!("{scalar}[]")
    } else if field.count > 1 {
        format!("{scalar}[{}]", field.count)
    } else {
        scalar.to_owned()
    }
}

/// `POST /api/debug/send`.
pub(super) fn send(api: &Api, lobby: Option<&Lobby>, data: &Map<String, Json>) -> ApiResult {
    if let Some(key) = data.keys().find(|key| !SEND_KEYS.contains(&key.as_str())) {
        return Err(bad(format!(
            "Unknown key \"{key}\": use {}",
            SEND_KEYS.join(", ")
        )));
    }
    let lobby = lobby.ok_or_else(|| ApiError::new(503, "The lobby is not running."))?;
    let id = message_id(
        data.get("id")
            .ok_or_else(|| bad("Give the message \"id\""))?,
    )?;
    let layout = layout(Some(&lobby.schemas), id)?;
    let count = match data.get("count") {
        None => 1,
        Some(count) => integer_text(count)
            .filter(|count| (1..=MAX_COUNT).contains(count))
            .ok_or_else(|| bad(format!("\"count\" is a whole number from 1 to {MAX_COUNT}")))?
            as usize,
    };
    let empty = Map::new();
    let given = match data.get("fields") {
        None => &empty,
        Some(Json::Object(given)) => given,
        Some(_) => {
            return Err(bad(
                "\"fields\" is an object of field names or #indexes and values",
            ));
        }
    };
    // Fields without "self" or "controller" are checked before the account,
    // so a wrong field shows with no game connected.
    let entities = given.values().any(names_entities);
    let checked = if entities {
        None
    } else {
        Some(build(layout.fields, given, None, "")?)
    };
    let account = match data.get("account") {
        None => None,
        Some(Json::String(name)) => Some(name.as_str()),
        Some(_) => return Err(bad("\"account\" is an account name")),
    };
    let name = connected_account(account, &connected(api)?)?;
    let account_lo = account_id_for(&name);

    let in_match = || -> Result<_, ApiError> {
        let game = lobby
            .game
            .as_deref()
            .ok_or_else(|| ApiError::new(409, "The game server is off (--game-port 0)."))?;
        let handoff = lobby
            .matchmaker
            .seat_of(account_lo)
            .ok_or_else(|| ApiError::new(409, format!("{name} is not in a match.")))?;
        let seat = game
            .debug_seat(&handoff)
            .map_err(game_error)?
            .ok_or_else(|| {
                ApiError::new(
                    409,
                    format!("{name}'s game has not connected to its match yet."),
                )
            })?;
        Ok((game, handoff, seat))
    };
    let seated = if layout.link == Link::Game || entities {
        Some(in_match()?)
    } else {
        None
    };
    let fields = match checked {
        Some(fields) => fields,
        None => build(
            layout.fields,
            given,
            seated.as_ref().map(|seated| seated.2),
            "",
        )?,
    };
    let sent = sent_fields(&layout, &fields)?;

    match layout.link {
        Link::Game => {
            let (game, handoff, _) = seated.expect("a game message needs the match");
            let message = messages::encode(&GAME_SCHEMAS, id, &fields)
                .map_err(|error| bad(format!("Message {id}: {error}")))?;
            if !game
                .debug_send(&handoff, vec![message; count])
                .map_err(game_error)?
            {
                return Err(ApiError::new(
                    409,
                    format!("{name}'s game is no longer in its match."),
                ));
            }
        }
        Link::Lobby => {
            for _ in 0..count {
                if !lobby.hub.post(account_lo, (layout.crc, id, fields.clone())) {
                    return Err(ApiError::new(
                        409,
                        format!("{name} is not connected to the lobby."),
                    ));
                }
            }
        }
    }
    Ok(json!({
        "sent": count,
        "link": layout.link.name(),
        "id": id,
        "account": name,
        "fields": sent,
    }))
}

/// The saved accounts that are logged in.
fn connected(api: &Api) -> Result<Vec<String>, ApiError> {
    let online = api.online.lock().expect("lobby accounts");
    Ok(api
        .accounts
        .all_saved()?
        .into_iter()
        .filter(|name| online.contains_key(&account_id_for(name)))
        .collect())
}

/// The account named (any letter case), which must be connected, or the
/// only connected one.
fn connected_account(given: Option<&str>, connected: &[String]) -> Result<String, ApiError> {
    let list = || {
        if connected.is_empty() {
            "nobody is connected".to_owned()
        } else {
            format!("connected: {}", connected.join(", "))
        }
    };
    match given.map(str::trim) {
        Some(given) if !given.is_empty() => connected
            .iter()
            .find(|name| name.eq_ignore_ascii_case(given))
            .cloned()
            .ok_or_else(|| ApiError::new(409, format!("{given} is not connected ({}).", list()))),
        _ => match connected {
            [only] => Ok(only.clone()),
            [] => Err(ApiError::new(409, "No account is connected.")),
            _ => Err(bad(format!(
                "Several accounts are connected ({}): give \"account\".",
                connected.join(", ")
            ))),
        },
    }
}

/// Whether a value anywhere uses `"self"` or `"controller"`.
fn names_entities(value: &Json) -> bool {
    match value {
        Json::String(text) => matches!(text.trim(), "self" | "controller"),
        Json::Array(items) => items.iter().any(names_entities),
        Json::Object(members) => members.values().any(names_entities),
        _ => false,
    }
}

/// Every top-level field's value as encoded, defaults included.
fn sent_fields(layout: &Layout, fields: &Fields) -> Result<Vec<Json>, ApiError> {
    let invalid = |error: orpheus_wire::jam::codec::CodecError| {
        bad(format!("Message {}: {error}", layout.id))
    };
    let body = layout
        .schemas
        .encode(layout.crc, layout.id, fields)
        .map_err(invalid)?;
    let sent = layout
        .schemas
        .decode(layout.crc, layout.id, &body, true)
        .map_err(invalid)?;
    Ok(layout
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            json!({
                "index": index,
                "name": field_name(field),
                "off": field.offset,
                "value": sent[&field.offset].to_jsonable(),
            })
        })
        .collect())
}

/// The fields of `schema` from the request's `given` names and values.
/// `prefix` is the struct's path for errors (`+0x80.`).
fn build(
    schema: &[Field],
    given: &Map<String, Json>,
    seat: Option<DebugSeat>,
    prefix: &str,
) -> Result<Fields, ApiError> {
    let mut whole: BTreeMap<usize, &Json> = BTreeMap::new();
    let mut members: BTreeMap<usize, Map<String, Json>> = BTreeMap::new();
    let twice = |index: usize| {
        bad(format!(
            "{prefix}{} is given twice",
            field_name(&schema[index])
        ))
    };
    for (key, value) in given {
        let (first, rest) = match key.split_once('.') {
            Some((first, rest)) => (first, Some(rest)),
            None => (key.as_str(), None),
        };
        let index = resolve(schema, first, prefix)?;
        let field = &schema[index];
        let single_struct = field.kind == STRUCT && !field.counted() && field.count <= 1;
        let inner = match (rest, value) {
            (Some(rest), _) if single_struct => vec![(rest.to_owned(), value.clone())],
            (Some(_), _) => {
                return Err(bad(format!(
                    "{prefix}{} is not a struct: it has no members",
                    field_name(field)
                )));
            }
            (None, Json::Object(object)) if single_struct => object.clone().into_iter().collect(),
            (None, value) => {
                if whole.insert(index, value).is_some() {
                    return Err(twice(index));
                }
                continue;
            }
        };
        let map = members.entry(index).or_default();
        for (member, value) in inner {
            if map.insert(member, value).is_some() {
                return Err(twice(index));
            }
        }
    }
    let mut fields = Fields::new();
    for (index, value) in whole {
        if members.contains_key(&index) {
            return Err(twice(index));
        }
        let field = &schema[index];
        let name = format!("{prefix}{}", field_name(field));
        fields.insert(field.offset, value_of(field, value, seat, &name)?);
    }
    for (index, given) in members {
        let field = &schema[index];
        let name = format!("{prefix}{}.", field_name(field));
        fields.insert(
            field.offset,
            Value::Struct(build(&field.fields, &given, seat, &name)?),
        );
    }
    Ok(fields)
}

/// The field a name (`+0x78`, any letter case) or `#index` names.
fn resolve(schema: &[Field], name: &str, prefix: &str) -> Result<usize, ApiError> {
    let found = match name.strip_prefix('#') {
        Some(index) => index.parse().ok().filter(|&index| index < schema.len()),
        None => schema
            .iter()
            .position(|field| field_name(field).eq_ignore_ascii_case(name)),
    };
    found.ok_or_else(|| {
        let known: Vec<String> = schema
            .iter()
            .enumerate()
            .map(|(index, field)| format!("#{index} {}", field_name(field)))
            .collect();
        let known = if known.is_empty() {
            "none".to_owned()
        } else {
            known.join(", ")
        };
        bad(format!("No field {prefix}{name}: the fields are {known}"))
    })
}

fn value_of(
    field: &Field,
    value: &Json,
    seat: Option<DebugSeat>,
    name: &str,
) -> Result<Value, ApiError> {
    if !field.counted() && field.count <= 1 {
        return element(field, value, seat, name);
    }
    let Json::Array(items) = value else {
        return Err(wrong(name, field, "a list"));
    };
    if !field.counted() && items.len() > field.count as usize {
        return Err(bad(format!(
            "{name} holds {} elements, not {}",
            field.count,
            items.len()
        )));
    }
    items
        .iter()
        .enumerate()
        .map(|(index, item)| element(field, item, seat, &format!("{name}[{index}]")))
        .collect::<Result<_, _>>()
        .map(Value::Array)
}

/// One value (an array's element, or a field that is not an array).
fn element(
    field: &Field,
    value: &Json,
    seat: Option<DebugSeat>,
    name: &str,
) -> Result<Value, ApiError> {
    Ok(match field.kind {
        BOOL => match value {
            Json::Bool(value) => Value::Bool(*value),
            _ => return Err(wrong(name, field, "true or false")),
        },
        1..=9 => integer(field, value, seat, name)?,
        F32 | F64 => {
            let number = value
                .as_f64()
                .ok_or_else(|| wrong(name, field, "a number"))?;
            if field.kind == F64 {
                Value::Float64(number)
            } else if (number as f32).is_finite() {
                Value::Float32(number as f32)
            } else {
                return Err(bad(format!("{name}: {number} does not fit an f32")));
            }
        }
        12 | 13 => match value {
            Json::String(text) => Value::String(text.clone()),
            _ => return Err(wrong(name, field, "text")),
        },
        BLOB => value
            .as_str()
            .and_then(hex_bytes)
            .map(Value::Blob)
            .ok_or_else(|| wrong(name, field, "hex text such as \"0a1b\""))?,
        STRUCT => match (value, field.fields.as_slice()) {
            (Json::Object(members), _) => {
                Value::Struct(build(&field.fields, members, seat, &format!("{name}."))?)
            }
            (_, [member])
                if (1..=9).contains(&member.kind) && !member.counted() && member.count <= 1 =>
            {
                Value::Struct(Fields::from([(
                    member.offset,
                    integer(member, value, seat, name)?,
                )]))
            }
            _ => return Err(wrong(name, field, "an object of its members")),
        },
        kind => return Err(bad(format!("{name} has the unknown field type {kind}"))),
    })
}

fn integer(
    field: &Field,
    value: &Json,
    seat: Option<DebugSeat>,
    name: &str,
) -> Result<Value, ApiError> {
    let number = match value.as_str().map(str::trim) {
        Some(symbol @ ("self" | "controller")) => {
            let seat =
                seat.ok_or_else(|| bad(format!("\"{symbol}\" needs the account in a match")))?;
            let entity = if symbol == "self" {
                seat.body.ok_or_else(|| {
                    ApiError::new(409, "\"self\": the account has no hero body yet")
                })?
            } else {
                seat.controller
            };
            i128::from(entity)
        }
        _ => integer_text(value).ok_or_else(|| {
            wrong(
                name,
                field,
                "a whole number, 0x hex text, \"self\" or \"controller\"",
            )
        })?,
    };
    let (low, high, signed) = match field.kind {
        1 | 2 => (0, i128::from(u8::MAX), false),
        3 => (i128::from(i8::MIN), i128::from(i8::MAX), true),
        4 => (0, i128::from(u16::MAX), false),
        5 => (i128::from(i16::MIN), i128::from(i16::MAX), true),
        6 => (0, i128::from(u32::MAX), false),
        7 => (i128::from(i32::MIN), i128::from(i32::MAX), true),
        8 => (0, i128::from(u64::MAX), false),
        _ => (i128::from(i64::MIN), i128::from(i64::MAX), true),
    };
    if !(low..=high).contains(&number) {
        return Err(bad(format!(
            "{name}: {number} is outside {} ({low} to {high})",
            type_name(field)
        )));
    }
    Ok(if signed {
        Value::Signed(number as i64)
    } else {
        Value::Unsigned(number as u64)
    })
}

/// A whole JSON number, or text in decimal or `0x` hex, either with a sign.
fn integer_text(value: &Json) -> Option<i128> {
    match value {
        Json::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from)),
        Json::String(text) => {
            let text = text.trim();
            let (negative, digits) = match text.strip_prefix('-') {
                Some(digits) => (true, digits),
                None => (false, text),
            };
            let magnitude = match digits
                .strip_prefix("0x")
                .or_else(|| digits.strip_prefix("0X"))
            {
                Some(hex) if !hex.starts_with(['+', '-']) => i128::from_str_radix(hex, 16).ok()?,
                Some(_) => return None,
                None if !digits.starts_with(['+', '-']) => digits.parse().ok()?,
                None => return None,
            };
            Some(if negative { -magnitude } else { magnitude })
        }
        _ => None,
    }
}

fn message_id(value: &Json) -> Result<u32, ApiError> {
    integer_text(value)
        .and_then(|id| u32::try_from(id).ok())
        .ok_or_else(|| bad("The message id is a whole number such as 25000"))
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

fn wrong(name: &str, field: &Field, expected: &str) -> ApiError {
    bad(format!("{name} ({}) takes {expected}", type_name(field)))
}

fn game_error(error: std::io::Error) -> ApiError {
    ApiError::new(500, format!("The game server did not answer: {error}"))
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::new(400, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEAT: DebugSeat = DebugSeat {
        controller: 0xa000_0100,
        body: Some(0x0000_1234),
    };

    fn query(id: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("id".to_owned(), id.to_owned())])
    }

    fn given(fields: Json) -> Map<String, Json> {
        let Json::Object(fields) = fields else {
            panic!("an object")
        };
        fields
    }

    fn game(id: u32, fields: Json, seat: Option<DebugSeat>) -> Result<Fields, ApiError> {
        let layout = layout(None, id).unwrap();
        build(layout.fields, &given(fields), seat, "")
    }

    fn entity(id: u32) -> Value {
        Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(id)))]))
    }

    fn error(result: Result<Fields, ApiError>) -> String {
        result.unwrap_err().message
    }

    #[test]
    fn a_layout_lists_the_fields_in_schema_order_with_struct_members() {
        assert_eq!(
            messages(None, &query("25005")).unwrap(),
            json!({
                "id": 25005,
                "link": "game",
                "name": "8F89E0DF/25005",
                "fields": [
                    {"index": 0, "name": "+0x78", "off": 120, "type": "u64"},
                    {"index": 1, "name": "+0x80", "off": 128, "type": "struct",
                     "fields": [{"index": 0, "name": "+0x0", "off": 0, "type": "u32"}]},
                    {"index": 2, "name": "+0x84", "off": 132, "type": "struct",
                     "fields": [{"index": 0, "name": "+0x0", "off": 0, "type": "u32"}]},
                    {"index": 3, "name": "+0x88", "off": 136, "type": "u8"},
                    {"index": 4, "name": "+0x89", "off": 137, "type": "bool"},
                ],
            })
        );
        let lobby = Schemas::from_json(
            r#"{"BA5EBA11": {"60000": [
                {"type": 8, "off": 120, "count": 2, "array": false},
                {"type": 12, "off": 136, "count": 0, "array": true}
            ]}}"#,
        )
        .unwrap();
        assert_eq!(
            messages(Some(&lobby), &query("0xEA60")).unwrap(),
            json!({
                "id": 60000,
                "link": "lobby",
                "name": "BA5EBA11/60000",
                "fields": [
                    {"index": 0, "name": "+0x78", "off": 120, "type": "u64[2]"},
                    {"index": 1, "name": "+0x88", "off": 136, "type": "string[]"},
                ],
            })
        );
        assert_eq!(
            messages(Some(&lobby), &query("60001")).unwrap_err().status,
            404
        );
        // Without the lobby, only the game link's layouts are known.
        assert_eq!(messages(None, &query("60000")).unwrap_err().status, 503);
    }

    #[test]
    fn fields_are_named_by_offset_index_or_path_and_the_rest_take_defaults() {
        let fields = game(
            25005,
            json!({
                "#0": "0x1122334455667788",
                "+0x80": "self",
                "#2.#0": "controller",
                "+0X88": 7,
                "#4": true,
            }),
            Some(SEAT),
        )
        .unwrap();
        assert_eq!(
            fields,
            Fields::from([
                (0x78, Value::Unsigned(0x1122_3344_5566_7788)),
                (0x80, entity(0x1234)),
                (
                    0x84,
                    Value::Struct(Fields::from([(0, Value::Unsigned(0xa000_0100))]))
                ),
                (0x88, Value::Unsigned(7)),
                (0x89, Value::Bool(true)),
            ])
        );
        // An object gives a struct's members; members not given are zero.
        let fields = game(25008, json!({"#0": {"+0x4": 1.5, "#2": -2}}), None).unwrap();
        assert_eq!(
            fields,
            Fields::from([(
                0x78,
                Value::Struct(Fields::from([
                    (4, Value::Float32(1.5)),
                    (8, Value::Float32(-2.0)),
                ]))
            )])
        );
        let layout = layout(None, 25005).unwrap();
        let sent = sent_fields(&layout, &game(25005, json!({"#3": "0x10"}), None).unwrap());
        assert_eq!(
            sent.unwrap(),
            [
                json!({"index": 0, "name": "+0x78", "off": 120, "value": 0}),
                json!({"index": 1, "name": "+0x80", "off": 128, "value": {"+0x0": 0}}),
                json!({"index": 2, "name": "+0x84", "off": 132, "value": {"+0x0": 0}}),
                json!({"index": 3, "name": "+0x88", "off": 136, "value": 16}),
                json!({"index": 4, "name": "+0x89", "off": 137, "value": false}),
            ]
        );
    }

    #[test]
    fn values_are_checked_against_the_field_type() {
        assert_eq!(
            game(25000, json!({"#0": "-0x10"}), None).unwrap()[&0x78],
            Value::Signed(-16)
        );
        for (fields, message) in [
            (
                json!({"#9": 1}),
                "No field #9: the fields are #0 +0x78, #1 +0x80",
            ),
            (json!({"+0x7C": 1}), "No field +0x7C: the fields are"),
            (
                json!({"#1.#1": 1}),
                "No field +0x80.#1: the fields are #0 +0x0",
            ),
            (json!({"#0.#0": 1}), "+0x78 is not a struct"),
            (json!({"#0": 1, "+0x78": 2}), "+0x78 is given twice"),
            (json!({"#1": 1, "#1.#0": 2}), "+0x80 is given twice"),
            (json!({"#0": 1.5}), "+0x78 (u64) takes a whole number"),
            (json!({"#0": true}), "+0x78 (u64) takes a whole number"),
            (json!({"#0": "abc"}), "+0x78 (u64) takes a whole number"),
            (
                json!({"#0": -1}),
                "+0x78: -1 is outside u64 (0 to 18446744073709551615)",
            ),
            (json!({"#3": 256}), "+0x88: 256 is outside u8 (0 to 255)"),
            (json!({"#4": 1}), "+0x89 (bool) takes true or false"),
            (json!({"#1": [1]}), "+0x80 (u32) takes a whole number"),
            (
                json!({"#1": {"#0": 1, "+0x0": 2}}),
                "+0x80.+0x0 is given twice",
            ),
            (
                json!({"#1": "self"}),
                "\"self\" needs the account in a match",
            ),
        ] {
            let error = error(game(25005, fields.clone(), None));
            assert!(error.starts_with(message), "{fields}: {error}");
        }
        let unpicked = DebugSeat { body: None, ..SEAT };
        assert_eq!(
            error(game(25005, json!({"#1": "self"}), Some(unpicked))),
            "\"self\": the account has no hero body yet"
        );
        assert_eq!(
            error(game(25008, json!({"#0.#0": "1"}), None)),
            "+0x78.+0x0 (f32) takes a number"
        );
        assert_eq!(
            error(game(25008, json!({"#0": 5}), None)),
            "+0x78 (struct) takes an object of its members"
        );
    }

    #[test]
    fn the_account_is_the_named_or_the_only_connected_one() {
        let one = ["Researcher".to_owned()];
        let two = ["Alpha".to_owned(), "Beta".to_owned()];
        assert_eq!(connected_account(None, &one).unwrap(), "Researcher");
        assert_eq!(connected_account(Some("beta"), &two).unwrap(), "Beta");
        let error = |given, connected| connected_account(given, connected).unwrap_err();
        assert_eq!(error(None, &[]).message, "No account is connected.");
        assert_eq!(
            error(None, &two).message,
            "Several accounts are connected (Alpha, Beta): give \"account\"."
        );
        assert_eq!(
            error(Some("Gamma"), &two),
            ApiError::new(409, "Gamma is not connected (connected: Alpha, Beta).")
        );
    }
}
