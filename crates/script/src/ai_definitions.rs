//! The AI that drives a hero's body when the game plays the hero (Practice
//! vs AI), from the derived file `game_ai_definitions_174.json` (the
//! `lucy-assets` derivation `game_ai_definitions`, which documents how
//! the data is read; the research is in
//! `D:/Work/Coding/refs/reports/AiDefinitions.md`).
//!
//! Each hero body names an AI definition (`STUEntityDefinition.m_04A8896C`,
//! an `003`). The entity made from it possesses the body (component 44: it
//! is the body's primary possessor, `STU_DB742151`), carries component 92
//! (`STU_76AF64F0`, which `STU_D3EB8D87` tests) and plays at one of the
//! difficulty profiles its `STUAIBehaviorComponent` lists
//! (`m_difficultyProfiles`: a difficulty identifier and an `0AE` table of
//! identifier → value). The heroes' weapon graphs read the AI's damage from
//! that profile: their amounts are `STU_D3EB8D87(STU_DB742151(owner)) ?
//! STU_4049CF77(parameter, fallback) : the player's damage` (Soldier: 76's
//! rifle `0254` st10: parameter `1961`, 4, 10 and 20 at Easy, Medium and
//! Hard, else the player's 20, `#476`).
//!
//! The difficulty identifiers are the client's: `195E` the lowest, `1960`
//! the highest, anything else (`195F`) the middle (the tunable triplet
//! picker 0x7FF789CAEA90 and the difficulty ranges of `STU_4B9EAB88`;
//! AiDefinitions.md, "Client tools pass (2026-11-16)"). Which one a room
//! plays is the server's: the Practice vs AI card's level.

use std::{collections::HashMap, fs, io, path::Path};

use serde::Deserialize;

pub const AI_DEFINITIONS_FILE: &str = "game_ai_definitions_174.json";

/// The difficulty identifiers (`01C`) of the hero AI profiles.
pub const EASY: u64 = 0x0d80_0000_0000_195e;
pub const MEDIUM: u64 = 0x0d80_0000_0000_195f;
pub const HARD: u64 = 0x0d80_0000_0000_1960;

/// The AI entity that possesses a body: made from the AI definition
/// `definition` (the body's `m_04A8896C`), playing at `difficulty`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AiControl {
    pub entity: u32,
    pub definition: u64,
    pub difficulty: u64,
}

impl AiControl {
    /// Whether the AI entity has component 92: its definition declares it.
    pub fn has_component_92(self, definitions: &AiDefinitions) -> bool {
        definitions
            .definitions
            .get(&self.definition)
            .is_some_and(|definition| definition.component_92)
    }

    /// `identifier`'s value in the AI's difficulty profile; none when the
    /// definition has no profile for the difficulty or the profile no such
    /// value (the graphs then take their authored fallback).
    pub fn parameter(self, definitions: &AiDefinitions, identifier: u64) -> Option<f64> {
        let profile = definitions
            .definitions
            .get(&self.definition)?
            .profiles
            .get(&self.difficulty)?;
        definitions.profiles.get(profile)?.get(&identifier).copied()
    }
}

#[derive(Debug, Default)]
pub struct AiDefinitions {
    /// Each hero's AI definition, by hero GUID.
    heroes: HashMap<u64, u64>,
    definitions: HashMap<u64, Definition>,
    /// Each profile's values (resolved through its parents), by `0AE` GUID.
    profiles: HashMap<u64, HashMap<u64, f64>>,
}

#[derive(Debug)]
struct Definition {
    component_92: bool,
    /// The profile of each difficulty identifier.
    profiles: HashMap<u64, u64>,
}

#[derive(Deserialize)]
struct Raw {
    heroes: HashMap<String, RawHero>,
    definitions: HashMap<String, RawDefinition>,
    profiles: HashMap<String, RawProfile>,
}

#[derive(Deserialize)]
struct RawHero {
    ai_definition: Option<String>,
}

#[derive(Deserialize)]
struct RawDefinition {
    component_92: bool,
    difficulty_profiles: HashMap<String, String>,
}

#[derive(Deserialize)]
struct RawProfile {
    values: HashMap<String, f64>,
}

fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn guid(text: &str) -> io::Result<u64> {
    lucy_core::guid::parse_guid(text).ok_or_else(|| invalid(format!("invalid GUID {text:?}")))
}

impl AiDefinitions {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let data = fs::read(path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_json(&data)
    }

    pub fn from_json(data: &[u8]) -> io::Result<Self> {
        let raw: Raw = serde_json::from_slice(data).map_err(invalid)?;
        let heroes = raw
            .heroes
            .into_iter()
            .filter_map(|(hero, record)| Some((hero, record.ai_definition?)))
            .map(|(hero, definition)| Ok((guid(&hero)?, guid(&definition)?)))
            .collect::<io::Result<_>>()?;
        let definitions = raw
            .definitions
            .into_iter()
            .map(|(key, record)| {
                let profiles = record
                    .difficulty_profiles
                    .iter()
                    .map(|(difficulty, profile)| Ok((guid(difficulty)?, guid(profile)?)))
                    .collect::<io::Result<_>>()?;
                let definition = Definition {
                    component_92: record.component_92,
                    profiles,
                };
                Ok((guid(&key)?, definition))
            })
            .collect::<io::Result<_>>()?;
        let profiles = raw
            .profiles
            .into_iter()
            .map(|(key, profile)| {
                let values = profile
                    .values
                    .iter()
                    .map(|(identifier, &value)| Ok((guid(identifier)?, value)))
                    .collect::<io::Result<_>>()?;
                Ok((guid(&key)?, values))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            heroes,
            definitions,
            profiles,
        })
    }

    /// The AI definition `hero`'s body names.
    pub fn definition(&self, hero: u64) -> Option<u64> {
        self.heroes.get(&hero).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parameter_is_the_value_of_the_profile_the_difficulty_names() {
        let definitions = AiDefinitions::from_json(
            br#"{
                "source": "",
                "heroes": {
                    "0x02E000000000006E": {"ai_definition": "0x04000000000011F4"},
                    "0x02E0000000000003": {"ai_definition": null}
                },
                "definitions": {"0x04000000000011F4": {
                    "component_92": true,
                    "difficulty_profiles": {
                        "0x0D8000000000195E": "0x0B50000000000013",
                        "0x0D80000000001960": "0x0B50000000000014"
                    }
                }},
                "profiles": {
                    "0x0B50000000000013": {"parent": null, "values": {"0x0D80000000001961": 4.0}},
                    "0x0B50000000000014": {"parent": null, "values": {"0x0D80000000001961": 20.0}}
                }
            }"#,
        )
        .unwrap();
        const DAMAGE: u64 = 0x0d80_0000_0000_1961;
        let definition = definitions.definition(0x02e0_0000_0000_006e).unwrap();
        assert_eq!(definition, 0x0400_0000_0000_11f4);
        assert_eq!(definitions.definition(0x02e0_0000_0000_0003), None);
        let ai = |difficulty| AiControl {
            entity: 0x8000_7000,
            definition,
            difficulty,
        };
        assert!(ai(EASY).has_component_92(&definitions));
        assert_eq!(ai(EASY).parameter(&definitions, DAMAGE), Some(4.0));
        assert_eq!(ai(HARD).parameter(&definitions, DAMAGE), Some(20.0));
        // No profile for the difficulty, no such value, no such definition.
        assert_eq!(ai(MEDIUM).parameter(&definitions, DAMAGE), None);
        assert_eq!(ai(EASY).parameter(&definitions, DAMAGE + 1), None);
        let unknown = AiControl {
            definition: 1,
            ..ai(EASY)
        };
        assert!(!unknown.has_component_92(&definitions));
        assert_eq!(unknown.parameter(&definitions, DAMAGE), None);
    }
}
