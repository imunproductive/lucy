//! What the client saves on the server and gets back at login: its settings
//! (22200-22204, sent back in 20802), interface states such as dialogs
//! already seen (22207, sent back in 20812) and the competitive season intros
//! it showed (36200).
//!
//! Ports `ow174/lobby/handlers/settings.py`. A saved value keeps the decoded
//! message's shape, in the JSON form the profile and the retail templates use
//! (`+0x..` keys, `{"$bytes": hex}` blobs).

use serde_json::{Map, Value as Json};

use crate::accounts::Profile;
use orpheus_wire::jam::codec::Value;

pub const SOCIAL_OUT: u32 = 0x75d3_2ae2;
pub const RANKED_OUT: u32 = 0x17cc_bfb2;
pub const SAVE_VALUES: u32 = 22204;
pub const SAVE_UX_STATE: u32 = 22207;
pub const SEASON_INTRO_SEEN: u32 = 36200;

/// Messages 22200-22203 each save one block of 20802: the 20802 field it fills.
pub const BLOCKS: [(u32, &str); 4] = [
    (22200, "+0x78"),
    (22201, "+0x108"),
    (22202, SOCIAL_SETTINGS),
    (22203, "+0x120"),
];
/// The block with "members can invite" and who may whisper.
pub const SOCIAL_SETTINGS: &str = "+0x10D";
/// 22204 saves only the changed entries of the key/value list in this field.
const VALUES_FIELD: &str = "+0x130";
/// +0x130 holds values [{+0x0 value, +0x8 key}], a key being (type table <<
/// 16) | identifier. While key B03A of table E0 is above 0, the login popups
/// graph (01B/0C75) shows "N of your containers were opened" (05A/07D9, the
/// move to Overwatch 2) on the main menu, and Continue sets it to -1 (22204).
/// `ow174/content/player.py`.
pub const BOXES_OPENED_KEY: u32 = (0xe0 << 16) | 0xb03a;

/// The 20802 field a block message fills.
pub fn block_field(message: u32) -> Option<&'static str> {
    BLOCKS
        .iter()
        .find(|(id, _)| *id == message)
        .map(|(_, field)| *field)
}

/// Save one block of 20802 (`save_block`).
pub fn save_block(profile: &mut Profile, field: &str, value: Option<&Value>) {
    profile
        .settings
        .insert(field.into(), value.map_or(Json::Null, stored));
}

/// Merge the changed entries of 22204 into the saved list, matching them by
/// setting id (+0x8). A known id keeps its place; a new one goes last. Both
/// sides are first put in the wire's form ([`values`]), so a key saved
/// signed matches the same key sent unsigned.
pub fn save_values(profile: &mut Profile, changed: &[Value]) {
    let saved = values(profile.settings.remove(VALUES_FIELD));
    let mut values: Vec<Json> = Vec::with_capacity(saved.len() + changed.len());
    for entry in saved.into_iter().chain(
        changed
            .iter()
            .filter_map(|entry| value_entry(&stored(entry))),
    ) {
        let id = entry.get("+0x8").cloned();
        match values
            .iter_mut()
            .find(|kept| kept.get("+0x8").cloned() == id)
        {
            Some(kept) => *kept = entry,
            None => values.push(entry),
        }
    }
    profile
        .settings
        .insert(VALUES_FIELD.into(), Json::Array(values));
}

/// Set one of the values in 20802 +0x130 (`set_saved_value`,
/// `ow174/content/player.py`): an older entry with the key goes, the new one
/// goes last.
pub fn set_saved_value(profile: &mut Profile, key: u32, value: i64) {
    let mut values = values(profile.settings.remove(VALUES_FIELD));
    values.retain(|entry| entry.get("+0x8") != Some(&Json::from(key)));
    values.push(value_json(value as u64, key));
    profile
        .settings
        .insert(VALUES_FIELD.into(), Json::Array(values));
}

/// Put a loaded profile's +0x130 list in the wire's form, dropping entries
/// that have none (upstream `_sanitize_saved_settings`, main `8a464026`).
pub fn normalize_values(profile: &mut Profile) {
    if let Some(saved) = profile.settings.remove(VALUES_FIELD) {
        let values = values(Some(saved));
        profile
            .settings
            .insert(VALUES_FIELD.into(), Json::Array(values));
    }
}

/// The saved +0x130 entries in the wire's form: the value (+0x0) a u64 and
/// the key (+0x8) a u32, as 22204 sends them and 20802 encodes them (the
/// lobby schema's types 8 and 6). A negative number is its two's complement
/// (-1 is `u64::MAX`), so the signed form an older profile or upstream's
/// Python saved matches the client's. An entry that fits neither is dropped
/// with a warning: 20802 could not encode it.
fn values(saved: Option<Json>) -> Vec<Json> {
    let Some(Json::Array(saved)) = saved else {
        return Vec::new();
    };
    saved
        .iter()
        .filter_map(|entry| {
            let value = value_entry(entry);
            if value.is_none() {
                tracing::warn!("skipping an invalid saved +0x130 entry {entry}");
            }
            value
        })
        .collect()
}

fn value_entry(entry: &Json) -> Option<Json> {
    let value = integer(entry.get("+0x0")?)?;
    let key = integer(entry.get("+0x8")?)?;
    let value = if value < 0 {
        i64::try_from(value).ok()? as u64
    } else {
        u64::try_from(value).ok()?
    };
    let key = if key < 0 {
        i32::try_from(key).ok()? as u32
    } else {
        u32::try_from(key).ok()?
    };
    Some(value_json(value, key))
}

/// A JSON integer, or one saved as decimal or `0x` text.
fn integer(value: &Json) -> Option<i128> {
    match value {
        Json::Number(number) => number
            .as_u64()
            .map(i128::from)
            .or_else(|| number.as_i64().map(i128::from)),
        Json::String(text) => {
            let text = text.trim();
            let (negative, digits) = match text.strip_prefix('-') {
                Some(digits) => (true, digits),
                None => (false, text.strip_prefix('+').unwrap_or(text)),
            };
            let magnitude = match digits
                .strip_prefix("0x")
                .or_else(|| digits.strip_prefix("0X"))
            {
                Some(hex) => i128::from_str_radix(hex, 16),
                None => digits.parse(),
            }
            .ok()?;
            Some(if negative { -magnitude } else { magnitude })
        }
        _ => None,
    }
}

fn value_json(value: u64, key: u32) -> Json {
    let mut entry = Map::new();
    entry.insert("+0x0".into(), Json::from(value));
    entry.insert("+0x8".into(), Json::from(key));
    Json::Object(entry)
}

/// One interface state (a dialog seen, "don't show again", ...). False when
/// it was saved already.
pub fn save_ux_state(profile: &mut Profile, index: &Value, state: &Value) -> bool {
    let key = match stored(index) {
        Json::Number(number) => number.to_string(),
        other => other.to_string(),
    };
    let state = stored(state);
    if profile.ux_states.get(&key) == Some(&state) {
        return false;
    }
    profile.ux_states.insert(key, state);
    true
}

/// A competitive season's intro was closed; the ranked state says so from now
/// on (card +0x2C). False when it was seen already.
pub fn season_intro_seen(profile: &mut Profile, card: u64) -> bool {
    let card = card as i64;
    if profile.seasons_seen.contains(&card) {
        return false;
    }
    profile.seasons_seen.push(card);
    true
}

/// A decoded value in the JSON form `templates::restore_value` reads back.
pub fn stored(value: &Value) -> Json {
    match value {
        Value::Bool(value) => Json::Bool(*value),
        Value::Unsigned(value) => Json::from(*value),
        Value::Signed(value) => Json::from(*value),
        Value::Float32(value) => Json::from(f64::from(*value)),
        Value::Float64(value) => Json::from(*value),
        Value::String(value) => Json::String(value.clone()),
        Value::Blob(bytes) => {
            let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let mut blob = Map::new();
            blob.insert("$bytes".into(), Json::String(hex));
            Json::Object(blob)
        }
        Value::Struct(fields) => Json::Object(
            fields
                .iter()
                .map(|(offset, value)| (format!("+0x{offset:X}"), stored(value)))
                .collect(),
        ),
        Value::Array(values) => Json::Array(values.iter().map(stored).collect()),
    }
}

#[cfg(test)]
mod tests;
