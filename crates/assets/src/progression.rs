//! `data/game_progression_174.json`: the experience a match gives and the
//! level curve, from the XP gains settings (`054/0008`,
//! `STUGenericSettings_XPGains`) and the player progression settings
//! (`054/0018`, `STUGenericSettings_PlayerProgression`).
//!
//! - `xp_gains`: each `m_XPGains` entry (`STUXPGain`) by its display string
//!   (`m_text`, a `07C` string, in snake case: "Gold Medal" is
//!   `gold_medal`), with its amount (`m_amount`), its type (`m_type`,
//!   `STUXPGainType`, whose names are not known) and the string's GUID.
//! - `xp_settings`: every other field of `054/0008` as it is, by its type
//!   table name. Their meaning is not known (see
//!   `docs/dev/hardcoded-values.md`).
//! - `level_experience`: `054/0018` `m_88922C14`, the experience by level
//!   (entry `n` is the experience from level `n - 1` to level `n`; see
//!   `docs/notes/behavior.md`).
//! - `level_up_unlocks` (`m_C84D463F`) and `ruleset_win_rewards`
//!   (`m_rulesetWinRewards`: wins and unlock) as they are, for later.

use std::path::Path;

use lucy_stu::{Types, Value};
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{Node, key, typed},
};

pub(crate) const PATH: &str = "data/game_progression_174.json";

const GENERIC_SETTINGS: u16 = 0x054;
/// `054/0008`, the XP gains settings.
const XP_GAINS: u64 = 0x08;
/// `054/0018`, the player progression settings.
const PROGRESSION: u64 = 0x18;

const SOURCE: &str = "Build 1.74.0.0.104319: 054/0008 STUGenericSettings_XPGains (m_XPGains by \
their 07C text; other fields as they are) and 054/0018 STUGenericSettings_PlayerProgression \
(m_88922C14, m_C84D463F, m_rulesetWinRewards).";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let gains = install.stu(typed(XP_GAINS, GENERIC_SETTINGS))?;
    let progression = install.stu(typed(PROGRESSION, GENERIC_SETTINGS))?;
    let (xp_gains, xp_settings) = xp_gains(install, &gains)?;
    let progression = Node::root(&progression)?;
    if progression.class_name() != Some("STUGenericSettings_PlayerProgression") {
        return Err(asset_error(
            "054/0018 is not the player progression settings",
        ));
    }
    let table = json!({
        "source": SOURCE,
        "xp_gains": xp_gains,
        "xp_settings": xp_settings,
        "level_experience": level_experience(progression)?,
        "level_up_unlocks": progression.guids("m_C84D463F").map(key).collect::<Vec<_>>(),
        "ruleset_win_rewards": progression
            .nodes("m_rulesetWinRewards")
            .map(|reward| json!({"wins": reward.int("m_120378C3"), "unlock": key(reward.guid("m_unlock"))}))
            .collect::<Vec<_>>(),
    });
    Ok(vec![Output::json(PATH, &table)])
}

fn xp_gains(install: &Install, asset: &lucy_stu::Asset) -> Result<(Json, Json)> {
    let root = Node::root(asset)?;
    if root.class_name() != Some("STUGenericSettings_XPGains") {
        return Err(asset_error("054/0008 is not the XP gains settings"));
    }
    let mut gains = Map::new();
    for gain in root.nodes("m_XPGains") {
        let text = gain.guid("m_text");
        let name = install.string(text).ok_or_else(|| {
            asset_error(format!(
                "054/0008: an XP gain without a string ({})",
                key(text)
            ))
        })?;
        let Some(&Value::Float(amount)) = gain.value("m_amount") else {
            return Err(asset_error(format!("054/0008: {name} has no amount")));
        };
        let entry = json!({"amount": amount, "type": gain.int("m_type"), "text": key(text)});
        if gains.insert(snake_case(&name), entry).is_some() {
            return Err(asset_error(format!("054/0008: two XP gains named {name}")));
        }
    }
    let object = asset
        .root()
        .ok_or_else(|| asset_error("054/0008 has no root"))?;
    let mut settings = Map::new();
    for field in Types::get().fields(object.class) {
        if field.name == "m_XPGains" {
            continue;
        }
        let value = match root.value(field.name) {
            Some(&Value::Float(value)) => json!(value),
            Some(&Value::UInt(value)) => json!(value),
            Some(&Value::Int(value)) => json!(value),
            _ => {
                return Err(asset_error(format!(
                    "054/0008: {} is not a number",
                    field.name
                )));
            }
        };
        settings.insert(field.name.into(), value);
    }
    Ok((Json::Object(gains), Json::Object(settings)))
}

fn level_experience(progression: Node) -> Result<Vec<u64>> {
    progression
        .list("m_88922C14")
        .iter()
        .map(|value| match value {
            Value::UInt(xp) => Ok(*xp),
            _ => Err(asset_error("054/0018: m_88922C14 holds a non-integer")),
        })
        .collect::<Result<Vec<_>>>()
        .and_then(|curve| {
            if curve.is_empty() {
                Err(asset_error("054/0018 has no level curve (m_88922C14)"))
            } else {
                Ok(curve)
            }
        })
}

/// "First win of the day" -> `first_win_of_the_day`.
fn snake_case(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(test)]
mod tests {
    use super::snake_case;

    #[test]
    fn display_strings_become_snake_case_keys() {
        assert_eq!(snake_case("Gold Medal"), "gold_medal");
        assert_eq!(snake_case("BACKFILL"), "backfill");
        assert_eq!(snake_case("CONSECUTIVE MATCH"), "consecutive_match");
        assert_eq!(snake_case("First win of the day"), "first_win_of_the_day");
    }
}
