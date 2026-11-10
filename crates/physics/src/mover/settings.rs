//! Character mover settings read from the 104319 runtime data.
//!
//! Every body's values come from the derived `game_movers_174.json`; body
//! lookup follows ow174/game/mover.py.

use std::{collections::HashMap, fs, io, path::Path};

use indexmap::IndexMap;
use serde::Deserialize;

use orpheus_core::guid::parse_guid;

/// Values of a body's STUCharacterMoverComponent.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[cfg_attr(test, derive(serde::Serialize))]
pub struct MoverSettings {
    pub stand_height: f64,
    pub stand_radius: f64,
    pub stand_pogo: f64,
    pub crouch_height: f64,
    pub crouch_radius: f64,
    pub crouch_pogo: f64,
    pub ground_circle_radius: f64,
    pub ground_circle_rays: u32,
    pub pogo_frequency: f64,
    pub pogo_damping: f64,
    /// Degrees.
    pub max_slope: f64,
    pub step_down: f64,
    pub run_forward: f64,
    pub run_backward: f64,
    pub run_strafe: f64,
    pub crouch_forward: f64,
    pub crouch_backward: f64,
    pub crouch_strafe: f64,
    /// Multiply by run speed to get acceleration in m/s².
    pub accel_ground: f64,
    pub accel_air: f64,
    pub accel_air_touching: f64,
    /// Multiply by run speed to get the base braking rate in m/s².
    pub decel_ground_base: f64,
    pub decel_ground_base_speed: f64,
    pub decel_ground_per_speed: f64,
    pub decel_air_base: f64,
    pub decel_air_base_speed: f64,
    pub decel_air_per_speed: f64,
    pub jump_speed: f64,
    pub gravity_scale: f64,
    pub terminal_velocity: f64,
    pub terminal_velocity_up: f64,
    pub ground_gravity_accel: f64,
    pub ground_gravity_decel: f64,
    pub character_collision: u32,
}

/// Soldier: 76's mover (`03CF.003`, as derived): the settings tests run
/// bodies with. The server reads every body's from the derived data.
#[cfg(any(test, feature = "fixtures"))]
static FIXTURE_SETTINGS: MoverSettings = MoverSettings {
    stand_height: 2.0,
    stand_radius: 0.5,
    stand_pogo: 0.5,
    crouch_height: 1.25,
    crouch_radius: 0.5,
    crouch_pogo: 0.25,
    ground_circle_radius: 0.45,
    ground_circle_rays: 8,
    pogo_frequency: 10.0,
    pogo_damping: 1.0,
    max_slope: 45.0,
    step_down: 0.5,
    run_forward: 5.5,
    run_backward: 4.95,
    run_strafe: 5.5,
    crouch_forward: 3.0,
    crouch_backward: 3.0,
    crouch_strafe: 3.0,
    accel_ground: 35.0,
    accel_air: 3.5,
    accel_air_touching: 0.5,
    decel_ground_base: 15.0,
    decel_ground_base_speed: 6.2,
    decel_ground_per_speed: 2.0,
    decel_air_base: 0.0,
    decel_air_base_speed: 5.5,
    decel_air_per_speed: 0.35,
    jump_speed: 6.0,
    gravity_scale: 1.75,
    terminal_velocity: 30.0,
    terminal_velocity_up: 0.0,
    ground_gravity_accel: 0.0,
    ground_gravity_decel: 0.0,
    character_collision: 1,
};

#[cfg(any(test, feature = "fixtures"))]
impl Default for MoverSettings {
    fn default() -> Self {
        FIXTURE_SETTINGS
    }
}

#[derive(Debug)]
pub struct MoverCatalog {
    by_body: HashMap<u64, MoverSettings>,
    fallback: MoverSettings,
}

impl MoverCatalog {
    /// Read game_movers_174.json from the selected runtime. A body it
    /// doesn't list moves as the `fallback` body, which it must list
    /// (mover.py `SOLDIER_BODY`: the server's fallback hero's body).
    pub fn load(path: impl AsRef<Path>, fallback: u64) -> io::Result<Self> {
        Self::from_json(&fs::read_to_string(path)?, fallback)
    }

    pub fn from_json(text: &str, fallback: u64) -> io::Result<Self> {
        let raw: IndexMap<String, MoverSettings> = serde_json::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let by_body: HashMap<u64, MoverSettings> = raw
            .into_iter()
            .map(|(body, settings)| Ok((body_guid(&body)?, settings)))
            .collect::<io::Result<_>>()?;
        let fallback = *by_body.get(&fallback).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the mover catalog has no fallback body 0x{fallback:016X}"),
            )
        })?;
        Ok(Self { by_body, fallback })
    }

    /// A missing body falls back to the fallback body's settings.
    pub fn for_body(&self, body: Option<u64>) -> &MoverSettings {
        body.and_then(|body| self.by_body.get(&body))
            .unwrap_or(&self.fallback)
    }
}

fn body_guid(text: &str) -> io::Result<u64> {
    let trimmed = text.trim();
    let digits = trimmed.strip_prefix('+').unwrap_or(trimmed);
    let parsed = match digits.as_bytes() {
        [b'0', b'x' | b'X', ..] => parse_guid(digits),
        [b'_', ..] => None,
        _ => parse_guid(&format!("0x{digits}")),
    };
    parsed.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid mover body GUID {text:?}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOLDIER_BODY: u64 = 0x0400_0000_0000_03cf;
    const TRAINING_BODY: u64 = 0x0400_0000_0000_1403;
    const FAR_RUNNER_BODY: u64 = 0x0400_0000_0000_14ed;
    const FRIENDLY_BODY: u64 = 0x0400_0000_0000_143e;

    /// A catalog record: the fixture settings with these fields changed.
    fn record(name: &str, changes: serde_json::Value) -> serde_json::Value {
        let mut record = serde_json::to_value(MoverSettings::default()).unwrap();
        record["name"] = name.into();
        for (key, value) in changes.as_object().unwrap() {
            record[key] = value.clone();
        }
        record
    }

    #[test]
    fn bodies_read_their_own_values_and_others_the_fallback_body() {
        let text = serde_json::json!({
            "0x04000000000003CF": record("Fixture Soldier", serde_json::json!({"run_forward":5.75, "accel_ground":4})),
            "0400000000001403": record("Fixture training", serde_json::json!({"run_forward":4.5, "character_collision":0})),
            "0X04000000000014ED": record("Fixture runner", serde_json::json!({"run_forward":6, "ground_circle_rays":4})),
        })
        .to_string();
        let catalog = MoverCatalog::from_json(&text, SOLDIER_BODY).unwrap();
        let soldier = catalog.for_body(Some(SOLDIER_BODY));
        assert_eq!(soldier.run_forward, 5.75);
        assert_eq!(soldier.accel_ground, 4.0);
        assert_eq!(catalog.for_body(None), soldier);
        assert_eq!(catalog.for_body(Some(9)), soldier);
        let training = catalog.for_body(Some(TRAINING_BODY));
        assert_eq!(training.run_forward, 4.5);
        assert_eq!(training.character_collision, 0);
        let far_runner = catalog.for_body(Some(FAR_RUNNER_BODY));
        assert_eq!(far_runner.run_forward, 6.0);
        assert_eq!(far_runner.ground_circle_rays, 4);
    }

    #[test]
    fn missing_and_malformed_tables_are_errors() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("game_movers_174.json");
        assert_eq!(
            MoverCatalog::load(&path, SOLDIER_BODY).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let soldier = record("Soldier", serde_json::json!({}));
        let mut partial = soldier.clone();
        partial.as_object_mut().unwrap().remove("stand_height");
        for malformed in [
            serde_json::json!({"bad guid": soldier}).to_string(),
            serde_json::json!({"0x04000000000003CF": record("Soldier", serde_json::json!({"ground_circle_rays":4.5}))}).to_string(),
            serde_json::json!({"0x04000000000003CF": partial}).to_string(),
            serde_json::json!({"0x5": soldier}).to_string(),
            "{".to_owned(),
        ] {
            fs::write(&path, malformed).unwrap();
            assert_eq!(
                MoverCatalog::load(&path, SOLDIER_BODY).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    #[ignore = "requires the derived 104319 mover catalog"]
    fn actual_mover_components_match_the_python_reference() {
        use sha2::{Digest, Sha256};
        use std::collections::BTreeMap;

        let root = orpheus_testkit::runtime_dir();
        let catalog =
            MoverCatalog::load(root.join("data/game_movers_174.json"), SOLDIER_BODY).unwrap();
        assert_eq!(catalog.by_body.len(), 38);
        let settings = |body| {
            let values = catalog.for_body(Some(body));
            (
                values.run_forward,
                values.accel_ground,
                values.decel_ground_base,
            )
        };
        assert_eq!(settings(TRAINING_BODY), (4.5, 1.0, 15.0));
        assert_eq!(settings(FAR_RUNNER_BODY), (6.0, 11.0, 6.5));
        assert_eq!(settings(FRIENDLY_BODY), (5.5, 1.0, 15.0));
        assert_eq!(catalog.for_body(None), catalog.for_body(Some(SOLDIER_BODY)));
        // mover.py's built-in defaults (`RUN_SPEED`, `JUMP_SPEED`, …) are
        // Soldier: 76's values, and `GRAVITY_SCALE` is every body's.
        assert_eq!(
            catalog.for_body(Some(SOLDIER_BODY)),
            &MoverSettings::default()
        );
        assert!(
            catalog
                .by_body
                .values()
                .all(|settings| settings.gravity_scale == 1.75)
        );

        let mut digest = Sha256::new();
        for (body, settings) in catalog.by_body.iter().collect::<BTreeMap<_, _>>() {
            digest.update(body.to_le_bytes());
            let object = serde_json::to_value(settings).unwrap();
            let fields = object
                .as_object()
                .unwrap()
                .iter()
                .collect::<BTreeMap<_, _>>();
            assert_eq!(fields.len(), 34);
            for (key, value) in fields {
                digest.update(key.as_bytes());
                digest.update([0]);
                if matches!(key.as_str(), "ground_circle_rays" | "character_collision") {
                    digest.update(
                        u32::try_from(value.as_u64().unwrap())
                            .unwrap()
                            .to_le_bytes(),
                    );
                } else {
                    digest.update(value.as_f64().unwrap().to_le_bytes());
                }
            }
        }
        // Python mover._movers(), encoded as little-endian f64 and u32 values.
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "cc067b2e1ab557fb6cd92f5ed485ccaf565c99f22394e01133a63d6464507991"
        );
    }
}
