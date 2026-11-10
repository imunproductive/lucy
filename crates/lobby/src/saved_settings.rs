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
pub const BOXES_OPENED_KEY: u64 = (0xe0 << 16) | 0xb03a;

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
/// setting id (+0x8). A known id keeps its place; a new one goes last.
pub fn save_values(profile: &mut Profile, changed: &[Value]) {
    let saved = match profile.settings.remove(VALUES_FIELD) {
        Some(Json::Array(values)) => values,
        _ => Vec::new(),
    };
    let mut values: Vec<Json> = Vec::with_capacity(saved.len() + changed.len());
    for entry in saved.into_iter().chain(changed.iter().map(stored)) {
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
pub fn set_saved_value(profile: &mut Profile, key: u64, value: i64) {
    let mut values: Vec<Json> = match profile.settings.remove(VALUES_FIELD) {
        Some(Json::Array(values)) => values,
        _ => Vec::new(),
    };
    values.retain(|entry| entry.get("+0x8") != Some(&Json::from(key)));
    let mut entry = Map::new();
    entry.insert("+0x0".into(), Json::from(value));
    entry.insert("+0x8".into(), Json::from(key));
    values.push(Json::Object(entry));
    profile
        .settings
        .insert(VALUES_FIELD.into(), Json::Array(values));
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
