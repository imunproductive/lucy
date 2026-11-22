//! `data/game_movers_174.json`: each hero body's character mover values
//! (`STUCharacterMoverComponent`, class `0x6EFDBD73`), which the server's
//! mover runs the hero with.
//!
//! The bodies are those of `data/game_heroes_174.json` and the training bots'
//! of `data/ai_spawners_174.json` (when the build folder has it), both read
//! from the build folder.
//!
//! The asset names the fields by hash. Their meaning comes from where the
//! client's mover reads them: it keeps the component's values at mover
//! `+0x418` + the field's offset (TankLib's STU layout), for example the jump
//! speed at `+0x49C` (jump `0x7FF789B48880`), the gravity scale at `+0x4A0`
//! (`0x7FF789AD03A0`), the speeds at `+0x460..+0x474` (wish
//! `0x7FF789B44340`), the capsule at `+0x430..+0x444` (stance
//! `0x7FF789B4C3D0`).

use std::{collections::BTreeMap, path::Path};

use lucy_stu::Value;
use serde_json::{Map, json};

use super::{Assets, field, parse_guid};
use crate::{Install, Output, Result, asset_error};

const ENTITY: u16 = 0x003;
const MOVER: u32 = 0x6EFDBD73;

/// Field name and the pack's name for it, in the component's order (STU
/// offset `0x18` = mover `+0x430` … `0xB1` = `+0x4C9`).
const FIELDS: [(&str, &str); 34] = [
    ("m_1972C687", "stand_height"),
    ("m_7A70E3E3", "stand_radius"),
    ("m_4A576BC2", "stand_pogo"),
    ("m_03E04168", "crouch_height"),
    ("m_915FF368", "crouch_radius"),
    ("m_D97B3735", "crouch_pogo"),
    ("m_D4AE9D05", "ground_circle_radius"),
    ("m_5C6B4602", "ground_circle_rays"),
    ("m_6B7FDB90", "pogo_frequency"),
    ("m_E54438F7", "pogo_damping"),
    ("m_AEBAD1B0", "max_slope"),
    ("m_08FFAA0A", "step_down"),
    ("m_CFD3E635", "run_forward"),
    ("m_61D6B4E9", "run_backward"),
    ("m_19452F15", "run_strafe"),
    ("m_B470B8DA", "crouch_forward"),
    ("m_AA6F4CE1", "crouch_backward"),
    ("m_0DDD18C3", "crouch_strafe"),
    ("m_DAA0618B", "accel_ground"),
    ("m_E631BA84", "accel_air"),
    ("m_D24BF4E6", "accel_air_touching"),
    ("m_E0A0FE25", "decel_ground_base"),
    ("m_6F25AA23", "decel_ground_base_speed"),
    ("m_4E03E892", "decel_ground_per_speed"),
    ("m_142DA8D4", "decel_air_base"),
    ("m_031FE7EE", "decel_air_base_speed"),
    ("m_8895A9C8", "decel_air_per_speed"),
    ("m_EC289987", "jump_speed"),
    ("m_C2C40969", "gravity_scale"),
    ("m_B74245E8", "terminal_velocity"),
    ("m_975F21A5", "terminal_velocity_up"),
    ("m_B99EC983", "ground_gravity_accel"),
    ("m_CEF4898C", "ground_gravity_decel"),
    ("m_D4A3C694", "character_collision"),
];

/// The fields with TankLib initializers, which the Python reference wrote
/// first; the rest follow in [`FIELDS`] order.
const DEFAULTED: [&str; 4] = [
    "ground_circle_radius",
    "ground_circle_rays",
    "accel_air_touching",
    "character_collision",
];

/// Each body's mover by body GUID: `{"name", <field>: value, …}`, sorted by
/// GUID. A body listed twice keeps the last list's name; a body without an
/// asset or a mover component is skipped. Fields the asset doesn't store take
/// TankLib's initializers (`ground_circle_radius` 0.25, `ground_circle_rays`
/// 4, `accel_air_touching` 0.5, `character_collision` 1). The ray count and
/// the collision flag are integers, the rest floats as the client prints
/// them.
pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let bodies = super::bodies(out)?;
    let assets = Assets::of(install, ENTITY);
    let mut table = BTreeMap::new();
    for hero in &bodies {
        let key = hero
            .get("body")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| asset_error(format!("a hero without a body: {hero}")))?;
        let body = parse_guid(key)?;
        let Some(asset) = assets.read(install, body)? else {
            continue;
        };
        let Some(mover) = asset
            .instances
            .iter()
            .flatten()
            .find(|object| object.class == MOVER)
        else {
            continue;
        };
        let mut values = Map::new();
        values.insert("name".into(), hero.get("name").cloned().unwrap_or_default());
        for name in DEFAULTED {
            values.insert(name.into(), serde_json::Value::Null);
        }
        for (field_name, name) in FIELDS {
            let value = match field(mover, field_name) {
                Some(Value::Float(value)) => float(*value),
                Some(Value::Int(value)) => json!(value),
                Some(Value::UInt(value)) => json!(value),
                _ => return Err(asset_error(format!("{key}: mover without {name}"))),
            };
            values.insert(name.into(), value);
        }
        table.insert(key.to_owned(), serde_json::Value::Object(values));
    }
    let table: Map<String, serde_json::Value> = table.into_iter().collect();
    Ok(vec![Output::json(
        "data/game_movers_174.json",
        &table.into(),
    )])
}

/// An `f32` as DataTool prints it (the shortest text that reads back the
/// same `f32`), read back as a double the way the Python reference did.
pub(crate) fn float(value: f64) -> serde_json::Value {
    let shortest: f64 = (value as f32).to_string().parse().expect("a float");
    json!(shortest)
}
