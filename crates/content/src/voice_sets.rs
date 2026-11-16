//! Which voice lines a body can say: each hero and training bot body's voice
//! set, read from `game_voice_sets_174.json` (derived from the install's
//! `STUVoiceSetComponent`, `05F` voice sets and `078` voice stimuli; see
//! `orpheus-assets` `voice_sets.rs`).
//!
//! A body's graphs raise a criteria identifier on it
//! (`STUStatescriptActionCriteria`); the stimulus of its set whose criteria
//! that identifier satisfies answers with one of its lines, and the server
//! tells the clients to play it (ClientInVoice 27500).

use std::{collections::HashMap, fs, io, path::Path};

use indexmap::IndexMap;
use orpheus_core::guid::parse_guid;
use serde::Deserialize;

/// What raises a stimulus, or lets a line play (`STUCriteriaContainer`).
#[derive(Clone, Debug, PartialEq)]
pub enum Criteria {
    /// `STUCriteria_Statescript`: the body has (or, with `not`, hasn't) the
    /// identifier raised.
    Statescript { identifier: u64, not: bool },
    /// `STU_7C69EA0F`: at least `amount` of the criteria hold.
    AtLeast { amount: u64, of: Vec<Criteria> },
    /// A class the server doesn't evaluate (map, team, hero, celebration):
    /// it never holds.
    Other(String),
}

impl Criteria {
    /// Whether it holds when `raised` is the only identifier the body has.
    pub fn holds(&self, raised: Option<u64>) -> bool {
        match self {
            Self::Statescript { identifier, not } => (raised == Some(*identifier)) != *not,
            Self::AtLeast { amount, of } => {
                of.iter().filter(|criteria| criteria.holds(raised)).count() as u64 >= *amount
            }
            Self::Other(_) => false,
        }
    }
}

/// One line of a stimulus.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Its key in the set's `m_voiceLineGuids` (`076`), which 27500 names.
    pub key: u64,
    pub criteria: Option<Criteria>,
}

impl Line {
    /// Whether it may play: its criteria hold with nothing raised (the
    /// bots' "not `191E`" does; the server raises no lasting identifiers).
    pub fn eligible(&self) -> bool {
        self.criteria
            .as_ref()
            .is_none_or(|criteria| criteria.holds(None))
    }
}

/// One voice stimulus (`078`) of a set.
#[derive(Clone, Debug, PartialEq)]
pub struct Stimulus {
    pub guid: u64,
    pub criteria: Option<Criteria>,
    /// `m_A73F2EBE` and `m_DD27AC95`, in seconds.
    pub cooldown: f64,
    pub cooldown_max: f64,
    /// `m_radius`, in metres.
    pub radius: f64,
    /// Its active lines, in the set's order.
    pub lines: Vec<Line>,
}

impl Stimulus {
    /// Whether raising `identifier` raises this stimulus: its criteria hold
    /// with it and don't without it.
    pub fn answers(&self, identifier: u64) -> bool {
        self.criteria
            .as_ref()
            .is_some_and(|criteria| criteria.holds(Some(identifier)) && !criteria.holds(None))
    }
}

/// A voice set (`05F`): its stimuli.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceSet {
    pub stimuli: Vec<Stimulus>,
}

impl VoiceSet {
    /// The first stimulus that `identifier` raises, with its index.
    pub fn answer(&self, identifier: u64) -> Option<(usize, &Stimulus)> {
        self.stimuli
            .iter()
            .enumerate()
            .find(|(_, stimulus)| stimulus.answers(identifier))
    }

    /// The stimulus `guid` (`078`) of the set, with its index: what a
    /// voice stimulus state names directly, whatever its criteria.
    pub fn stimulus(&self, guid: u64) -> Option<(usize, &Stimulus)> {
        self.stimuli
            .iter()
            .enumerate()
            .find(|(_, stimulus)| stimulus.guid == guid)
    }
}

/// Every body's voice set.
#[derive(Debug, Default)]
pub struct VoiceCatalog {
    bodies: HashMap<u64, usize>,
    sets: Vec<VoiceSet>,
}

impl VoiceCatalog {
    /// Read `game_voice_sets_174.json`.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_json(&fs::read_to_string(path)?)
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: RawCatalog = serde_json::from_str(text).map_err(invalid)?;
        let mut index = HashMap::new();
        let mut sets = Vec::new();
        for (guid, stimuli) in raw.sets {
            let stimuli = stimuli
                .into_iter()
                .map(|(stimulus, raw)| {
                    Ok(Stimulus {
                        guid: guid_of(&stimulus)?,
                        criteria: raw.criteria.map(RawCriteria::read).transpose()?,
                        cooldown: raw.cooldown,
                        cooldown_max: raw.cooldown_max,
                        radius: raw.radius,
                        lines: raw
                            .lines
                            .into_iter()
                            .map(|line| {
                                Ok(Line {
                                    key: guid_of(&line.key)?,
                                    criteria: line.criteria.map(RawCriteria::read).transpose()?,
                                })
                            })
                            .collect::<io::Result<_>>()?,
                    })
                })
                .collect::<io::Result<_>>()?;
            index.insert(guid_of(&guid)?, sets.len());
            sets.push(VoiceSet { stimuli });
        }
        let bodies = raw
            .bodies
            .into_iter()
            .map(|(body, set)| {
                let set = guid_of(&set)?;
                let index = *index.get(&set).ok_or_else(|| {
                    invalid(format!("body {body} names unknown voice set {set:016X}"))
                })?;
                Ok((guid_of(&body)?, index))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self { bodies, sets })
    }

    /// The voice set of a body (`003`), if it has one.
    pub fn for_body(&self, body: u64) -> Option<&VoiceSet> {
        self.sets.get(*self.bodies.get(&body)?)
    }
}

#[derive(Deserialize)]
struct RawCatalog {
    bodies: IndexMap<String, String>,
    sets: IndexMap<String, IndexMap<String, RawStimulus>>,
}

#[derive(Deserialize)]
struct RawStimulus {
    criteria: Option<RawCriteria>,
    cooldown: f64,
    cooldown_max: f64,
    radius: f64,
    lines: Vec<RawLine>,
}

#[derive(Deserialize)]
struct RawLine {
    key: String,
    criteria: Option<RawCriteria>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawCriteria {
    Statescript { statescript: String, not: bool },
    AtLeast { at_least: u64, of: Vec<RawCriteria> },
    Other { other: String },
}

impl RawCriteria {
    fn read(self) -> io::Result<Criteria> {
        Ok(match self {
            Self::Statescript { statescript, not } => Criteria::Statescript {
                identifier: guid_of(&statescript)?,
                not,
            },
            Self::AtLeast { at_least, of } => Criteria::AtLeast {
                amount: at_least,
                of: of.into_iter().map(Self::read).collect::<io::Result<_>>()?,
            },
            Self::Other { other } => Criteria::Other(other),
        })
    }
}

fn guid_of(text: &str) -> io::Result<u64> {
    parse_guid(text).ok_or_else(|| invalid(format!("invalid voice set GUID {text:?}")))
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: &str = r#"{
        "bodies": {"0x0400000000001403": "0x07A000000000011D"},
        "sets": {"0x07A000000000011D": {
            "0x0EE0000000000004": {
                "criteria": {"statescript": "0x0D80000000000609", "not": false},
                "cooldown": 0.5, "cooldown_max": 1.0, "radius": 35.0,
                "lines": [
                    {"key": "0x076000000000000B",
                     "criteria": {"statescript": "0x0D8000000000191E", "not": true}},
                    {"key": "0x076000000000000C", "criteria": {"other": "STUCriteria_OnMap"}},
                    {"key": "0x076000000000000D"}
                ]
            },
            "0x0EE0000000000005": {
                "criteria": {"statescript": "0x0D80000000000609", "not": true},
                "cooldown": 0.0, "cooldown_max": 0.0, "radius": 35.0, "lines": []
            }
        }}
    }"#;

    /// A raised identifier picks the stimulus whose criteria need it (not
    /// one that holds without it), and only lines whose criteria hold with
    /// nothing raised may play.
    #[test]
    fn a_raised_identifier_picks_its_stimulus_and_its_eligible_lines() {
        let catalog = VoiceCatalog::from_json(SET).unwrap();
        let set = catalog.for_body(0x0400_0000_0000_1403).unwrap();
        let (index, stimulus) = set.answer(0x0d80_0000_0000_0609).unwrap();
        assert_eq!((index, stimulus.guid), (0, 0x0ee0_0000_0000_0004));
        let eligible: Vec<u64> = stimulus
            .lines
            .iter()
            .filter(|line| line.eligible())
            .map(|line| line.key)
            .collect();
        assert_eq!(eligible, [0x0760_0000_0000_000b, 0x0760_0000_0000_000d]);
        assert!(set.answer(0x0d80_0000_0000_0608).is_none());
        assert!(catalog.for_body(0x0400_0000_0000_0022).is_none());
    }
}
