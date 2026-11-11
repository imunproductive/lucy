//! How long the animation an alias plays runs on each body, and the scalar
//! it outputs: the derived file `game_anim_lengths_174.json` (the
//! `orpheus-assets` derivation `game_anim_lengths`, which documents how the
//! data is read). A statescript `Anim` state without an authored length
//! runs for this length.
//!
//! A lookup takes the body's blend tree set, the alias and the animation
//! category the state names (0 for none), and falls back to the alias's
//! length for every category (`any`) when that category has no entry.
//! Ported from leafall903/ORPHEUS (MIT), commit `c239a84`,
//! `src/ow174/game/script/driver.py` (`wheel_animation_durations`,
//! `wheel_animation_scalars`, `animation_duration`): lengths outside
//! (0, 3600] seconds are dropped, values are `f32`.

use std::{collections::HashMap, fs, io, path::Path};

use serde::Deserialize;

pub const ANIM_LENGTHS_FILE: &str = "game_anim_lengths_174.json";

/// The category key of the length for every category.
const ANY: u64 = u64::MAX;
/// The longest length kept, in seconds.
const MAX_SECONDS: f64 = 3600.0;

type Key = (u64, u64, u64);

#[derive(Debug, Default)]
pub struct AnimLengths {
    /// Each body's blend tree set.
    bodies: HashMap<u64, u64>,
    /// Seconds by set, alias and category.
    lengths: HashMap<Key, f64>,
    /// The scalar by set, alias and category; none where the data has no
    /// single value (which hides the `any` entry).
    scalars: HashMap<Key, Option<f64>>,
}

#[derive(Deserialize)]
struct Raw {
    bodies: HashMap<String, String>,
    sets: HashMap<String, HashMap<String, RawAlias>>,
}

#[derive(Deserialize)]
struct RawAlias {
    #[serde(default)]
    lengths: HashMap<String, f64>,
    #[serde(default)]
    scalars: HashMap<String, Option<f64>>,
}

fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn guid(text: &str) -> io::Result<u64> {
    text.strip_prefix("0x")
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .ok_or_else(|| invalid(format!("invalid GUID {text:?}")))
}

fn category(text: &str) -> io::Result<u64> {
    if text == "any" { Ok(ANY) } else { guid(text) }
}

fn f32_value(value: f64) -> f64 {
    f64::from(value as f32)
}

impl AnimLengths {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_json(&data)
    }

    pub fn from_json(data: &[u8]) -> io::Result<Self> {
        let raw: Raw = serde_json::from_slice(data).map_err(invalid)?;
        let mut table = Self {
            bodies: raw
                .bodies
                .iter()
                .map(|(body, set)| Ok((guid(body)?, guid(set)?)))
                .collect::<io::Result<_>>()?,
            ..Self::default()
        };
        for (set, aliases) in &raw.sets {
            let set = guid(set)?;
            for (alias, entry) in aliases {
                let alias = guid(alias)?;
                for (key, &seconds) in &entry.lengths {
                    if seconds.is_finite() && 0.0 < seconds && seconds <= MAX_SECONDS {
                        table
                            .lengths
                            .insert((set, alias, category(key)?), f32_value(seconds));
                    }
                }
                for (key, &scalar) in &entry.scalars {
                    let scalar = scalar.filter(|value| value.is_finite()).map(f32_value);
                    table.scalars.insert((set, alias, category(key)?), scalar);
                }
            }
        }
        Ok(table)
    }

    pub fn is_empty(&self) -> bool {
        self.lengths.is_empty() && self.scalars.is_empty()
    }

    /// The length in seconds of `alias` on `body` with `category` set (0:
    /// none); none when the data can't tell.
    pub fn length(&self, body: u64, alias: u64, category: u64) -> Option<f64> {
        let set = *self.bodies.get(&body)?;
        self.lengths
            .get(&(set, alias, category))
            .or_else(|| self.lengths.get(&(set, alias, ANY)))
            .copied()
    }

    /// The scalar `alias` outputs on `body` with `category` set; none when
    /// the data can't tell.
    pub fn scalar(&self, body: u64, alias: u64, category: u64) -> Option<f64> {
        let set = *self.bodies.get(&body)?;
        match self.scalars.get(&(set, alias, category)) {
            Some(scalar) => *scalar,
            None => self.scalars.get(&(set, alias, ANY)).copied().flatten(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: u64 = 0x0400_0000_0000_03CF;
    const ALIAS: u64 = 0x0C80_0000_0000_043C;
    const OTHER: u64 = 0x0C80_0000_0000_0001;
    const CATEGORY: u64 = 0x0280_0000_0000_011B;

    fn table() -> AnimLengths {
        AnimLengths::from_json(
            br#"{
                "bodies": {"0x04000000000003CF": "0x0040000000000123"},
                "sets": {"0x0040000000000123": {
                    "0x0C8000000000043C": {
                        "lengths": {"0x0000000000000000": 1.5, "0x028000000000011B": 0.7, "any": 1.5},
                        "scalars": {"any": 0.25, "0x028000000000011B": null}
                    },
                    "0x0C80000000000001": {"lengths": {"any": 7200.0, "0x028000000000011B": 0.1}}
                }}
            }"#,
        )
        .unwrap()
    }

    /// Upstream `animation_duration`: the exact category first, then the
    /// length for every category.
    #[test]
    fn a_length_falls_back_to_every_category() {
        let table = table();
        assert_eq!(
            table.length(BODY, ALIAS, CATEGORY),
            Some(f64::from(0.7_f32))
        );
        assert_eq!(table.length(BODY, ALIAS, 0), Some(1.5));
        assert_eq!(table.length(BODY, ALIAS, CATEGORY + 1), Some(1.5));
        assert_eq!(table.length(BODY + 1, ALIAS, 0), None);
        assert_eq!(table.length(BODY, ALIAS + 1, 0), None);
    }

    /// Upstream `wheel_animation_durations` keeps lengths up to an hour.
    #[test]
    fn a_length_over_an_hour_is_dropped() {
        let table = table();
        assert_eq!(table.length(BODY, OTHER, 0), None);
        assert_eq!(
            table.length(BODY, OTHER, CATEGORY),
            Some(f64::from(0.1_f32))
        );
    }

    /// Upstream `wheel_animation_scalars`: a category whose track has no
    /// single value hides the scalar for every category.
    #[test]
    fn an_unknown_scalar_hides_every_category() {
        let table = table();
        assert_eq!(table.scalar(BODY, ALIAS, 0), Some(0.25));
        assert_eq!(table.scalar(BODY, ALIAS, CATEGORY), None);
        assert_eq!(table.scalar(BODY, OTHER, 0), None);
        assert!(!table.is_empty() && AnimLengths::default().is_empty());
    }
}
