//! `data/game_voice_sets_174.json`: the voice lines a body can say, read
//! from the install.
//!
//! - `bodies`: each hero and training bot body (the bodies of
//!   `data/game_heroes_174.json` and `data/ai_spawners_174.json`) and its
//!   voice set, `STUVoiceSetComponent.m_voiceDefinition` (`05F`).
//! - `sets`: each of those voice sets: its active voice stimuli (`078`) in
//!   the order its lines first name them, each with
//!   - `criteria`: what raises it (`m_criteria`, below);
//!   - `cooldown` and `cooldown_max`: `m_A73F2EBE` and `m_DD27AC95`, in
//!     seconds (the bots' pain stimulus `0003` has 1.0 and 2.5, the
//!     communication wheel's 0 and 0);
//!   - `radius`: `m_radius`, in metres (35 by default);
//!   - `lines`: its active lines (`m_voiceLineInstances[].m_voiceLineRuntime`,
//!     `m_active` set), in the set's order: `key`, the line's entry in the
//!     set's `m_voiceLineGuids` (`076`, the same index), which is what
//!     ClientInVoice 27500 names, and `criteria`, the line's own, when it has
//!     some.
//!
//! A criteria tree (`STUCriteriaContainer`) becomes:
//! - `{"statescript": identifier, "not": bool}` for `STUCriteria_Statescript`
//!   (`m_identifier`, `m_57D96E27`; the bots' lines all have "not `191E`");
//! - `{"at_least": n, "of": [...]}` for `STU_7C69EA0F` (`m_amount`,
//!   `m_criteria`);
//! - `{"other": class}` for any other class (map, team, hero, celebration),
//!   which the server doesn't evaluate.
//!
//! The single-criteria container `STU_32A19631` is transparent. Read from
//! the 104319 install's `003`, `05F` and `078` assets (types from OWLib's
//! `TankLib/STU/Types`).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use orpheus_stu::{Asset, Object, Value};
use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error, guid_hex, guid_type,
    heroes::{Assets, field, guid, movers::float, parse_guid},
};

const ENTITY: u16 = 0x003;
const INDEX: u64 = 0xFFFF_FFFF_FFFF;
/// `STUVoiceSetComponent`.
const VOICE_SET_COMPONENT: &str = "STUVoiceSetComponent";
/// The container of one criteria.
const SINGLE: &str = "STU_32A19631";
/// A container of several, `m_amount` of which must hold.
const SEVERAL: &str = "STU_7C69EA0F";
const STATESCRIPT: &str = "STUCriteria_Statescript";

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    Ok(vec![Output::json(
        "data/game_voice_sets_174.json",
        &voice_sets(install, out)?,
    )])
}

/// `{"bodies": {body: set}, "sets": {set: {stimulus: {…}}}}`, sorted by
/// GUID; a body without an asset or a voice set is left out.
fn voice_sets(install: &Install, out: &Path) -> Result<Json> {
    let entities = Assets::of(install, ENTITY);
    let mut bodies = BTreeMap::new();
    for hero in crate::heroes::bodies(out)? {
        let key = hero
            .get("body")
            .and_then(Json::as_str)
            .ok_or_else(|| asset_error(format!("a hero without a body: {hero}")))?;
        let Some(asset) = entities.read(install, parse_guid(key)?)? else {
            continue;
        };
        let set = asset
            .instances
            .iter()
            .flatten()
            .find(|object| object.class_name() == Some(VOICE_SET_COMPONENT))
            .and_then(|component| match field(component, "m_voiceDefinition") {
                Some(Value::Guid(set)) if *set != 0 => Some(canonical(*set)),
                _ => None,
            });
        if let Some(set) = set {
            bodies.insert(key.to_owned(), set);
        }
    }
    let wanted: Vec<u64> = bodies
        .values()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut sets = BTreeMap::new();
    let mut stimuli = BTreeSet::new();
    for (set, asset) in install.stu_all(&wanted) {
        let lines = lines(&asset?, set)?;
        stimuli.extend(lines.iter().map(|line| line.stimulus));
        sets.insert(canonical(set), lines);
    }
    let stimuli: Vec<u64> = stimuli.into_iter().collect();
    let mut found = BTreeMap::new();
    for (stimulus, asset) in install.stu_all(&stimuli) {
        found.insert(canonical(stimulus), stimulus_record(&asset?, stimulus)?);
    }
    let sets = sets
        .into_iter()
        .map(|(set, lines)| {
            let mut table = Map::new();
            for line in lines {
                let Some(Some(record)) = found.get(&line.stimulus) else {
                    continue;
                };
                let entry = table
                    .entry(guid_hex(line.stimulus))
                    .or_insert_with(|| record.clone());
                let mut item = json!({"key": guid_hex(line.key)});
                if let Some(criteria) = line.criteria {
                    item["criteria"] = criteria;
                }
                entry["lines"]
                    .as_array_mut()
                    .expect("a stimulus record has lines")
                    .push(item);
            }
            (guid_hex(set), Json::Object(table))
        })
        .collect::<Map<_, _>>();
    let bodies = bodies
        .into_iter()
        .map(|(body, set)| (body, Json::String(guid_hex(set))))
        .collect::<Map<_, _>>();
    Ok(json!({"bodies": bodies, "sets": sets}))
}

/// One active line of a voice set.
struct Line {
    key: u64,
    stimulus: u64,
    criteria: Option<Json>,
}

/// A voice set's active lines with a stimulus, in its order.
fn lines(asset: &Asset, set: u64) -> Result<Vec<Line>> {
    let root = asset
        .root()
        .ok_or_else(|| asset_error(format!("voice set {set:016X} is empty")))?;
    let (Some(Value::Array(keys)), Some(Value::Array(instances))) = (
        field(root, "m_voiceLineGuids"),
        field(root, "m_voiceLineInstances"),
    ) else {
        return Ok(Vec::new());
    };
    if keys.len() != instances.len() {
        return Err(asset_error(format!(
            "voice set {set:016X}: {} keys for {} lines",
            keys.len(),
            instances.len()
        )));
    }
    let mut lines = Vec::new();
    for (key, instance) in keys.iter().zip(instances) {
        let Value::UInt(key) = key else {
            return Err(asset_error(format!("voice set {set:016X}: key {key:?}")));
        };
        let Some(line) = asset
            .resolve(instance)
            .and_then(|instance| field(instance, "m_voiceLineRuntime"))
            .and_then(|line| asset.resolve(line))
        else {
            continue;
        };
        if !active(line) {
            continue;
        }
        let Some(Value::Guid(stimulus)) = field(line, "m_stimulus") else {
            continue;
        };
        if *stimulus == 0 {
            continue;
        }
        lines.push(Line {
            key: *key,
            stimulus: canonical(*stimulus),
            criteria: criteria(asset, field(line, "m_criteria")),
        });
    }
    Ok(lines)
}

/// A stimulus's record without its lines; none for an inactive one.
fn stimulus_record(asset: &Asset, stimulus: u64) -> Result<Option<Json>> {
    let root = asset
        .root()
        .ok_or_else(|| asset_error(format!("voice stimulus {stimulus:016X} is empty")))?;
    if !active(root) {
        return Ok(None);
    }
    let number = |name| match field(root, name) {
        Some(Value::Float(value)) => Ok(float(*value)),
        other => Err(asset_error(format!(
            "voice stimulus {stimulus:016X}: {name} is {other:?}"
        ))),
    };
    Ok(Some(json!({
        "criteria": criteria(asset, field(root, "m_criteria")).unwrap_or(Json::Null),
        "cooldown": number("m_A73F2EBE")?,
        "cooldown_max": number("m_DD27AC95")?,
        "radius": number("m_radius")?,
        "lines": [],
    })))
}

/// `m_active` (1 by default).
fn active(object: &Object) -> bool {
    !matches!(
        field(object, "m_active"),
        Some(Value::UInt(0) | Value::Int(0))
    )
}

/// A criteria tree as the module documentation lays it out; none for none.
fn criteria(asset: &Asset, value: Option<&Value>) -> Option<Json> {
    let object = asset.resolve(value?)?;
    let class = object.class_name().unwrap_or("?");
    Some(match class {
        SINGLE => return criteria(asset, field(object, "m_criteria")),
        STATESCRIPT => match field(object, "m_identifier") {
            Some(Value::Guid(identifier)) => json!({
                "statescript": guid_hex(canonical(*identifier)),
                "not": !matches!(field(object, "m_57D96E27"), Some(Value::UInt(0) | Value::Int(0))),
            }),
            _ => json!({"other": class}),
        },
        SEVERAL => {
            let amount = match field(object, "m_amount") {
                Some(Value::UInt(amount)) => *amount,
                _ => 1,
            };
            let of: Vec<Json> = match field(object, "m_criteria") {
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        criteria(asset, Some(item)).unwrap_or_else(|| json!({"other": "?"}))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            json!({"at_least": amount, "of": of})
        }
        _ => json!({"other": class}),
    })
}

/// A reference without the bits outside its index and type.
fn canonical(raw: u64) -> u64 {
    guid(raw & INDEX, guid_type(raw))
}
