//! Derives the server's game data from the player's 1.74 install.
//!
//! Each derivation reads assets through [`Install`] and produces one file of
//! the runtime pack, in the pack's own format, so the server's loaders read
//! it unchanged. [`build`] runs them in dependency order and writes the
//! results to a cache folder; [`diff`] compares them with an existing pack,
//! the oracle each derivation is checked against.

#![forbid(unsafe_code)]

mod ai_spawners;
mod animation;
mod arcade_cards;
mod blend_trees;
mod body_data;
pub mod cache;
mod collision;
mod economy;
mod game_maps;
mod game_modes;
mod game_queues;
mod graphs;
mod heroes;
mod hitboxes;
mod install;
mod lists;
mod map_chunk;
mod mode_props;
mod navigation;
mod objectives;
mod progression;
mod props;
mod tunables;

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

pub use collision::collision_cache;
pub use install::{Install, guid_hex, guid_name, guid_type};

/// One derived file: its path inside the pack (`data/…` or `collision/…`)
/// and its bytes.
pub struct Output {
    pub path: String,
    pub bytes: Vec<u8>,
}

impl Output {
    /// A JSON file, written as the Python extractors did: two-space indent,
    /// keys in insertion order.
    pub fn json(path: impl Into<String>, value: &serde_json::Value) -> Self {
        let mut bytes = serde_json::to_vec_pretty(value).expect("JSON values serialize");
        bytes.push(b'\n');
        Self {
            path: path.into(),
            bytes,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// The install lacks or can't decode an asset a derivation needs.
    Asset(String),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Asset(reason) => f.write_str(reason),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub(crate) fn asset_error(reason: impl Into<String>) -> Error {
    Error::Asset(reason.into())
}

/// A derivation: its name, the pack files it writes, and the function that
/// derives them. Derivations read earlier ones' outputs from the build
/// folder, so [`DERIVATIONS`] lists them in dependency order.
pub struct Derivation {
    pub name: &'static str,
    pub derive: fn(&Install, &Path) -> Result<Vec<Output>>,
}

/// Every derivation, inputs first.
pub const DERIVATIONS: &[Derivation] = &[
    Derivation {
        name: "lists",
        derive: lists::derive,
    },
    Derivation {
        name: "game_heroes",
        derive: heroes::derive,
    },
    Derivation {
        name: "game_maps",
        derive: game_maps::derive,
    },
    Derivation {
        name: "ai_spawners",
        derive: ai_spawners::derive,
    },
    Derivation {
        name: "game_movers",
        derive: heroes::movers::derive,
    },
    Derivation {
        name: "hitboxes",
        derive: hitboxes::derive,
    },
    Derivation {
        name: "game_stats",
        derive: heroes::stats::derive,
    },
    Derivation {
        name: "graphs",
        derive: graphs::derive,
    },
    Derivation {
        name: "game_body_data",
        derive: body_data::derive,
    },
    Derivation {
        name: "game_queues",
        derive: game_queues::derive,
    },
    Derivation {
        name: "arcade_cards",
        derive: arcade_cards::derive,
    },
    Derivation {
        name: "game_modes",
        derive: game_modes::derive,
    },
    Derivation {
        name: "game_tunables",
        derive: tunables::derive,
    },
    Derivation {
        name: "collision",
        derive: collision::derive,
    },
    Derivation {
        name: "collision_props",
        derive: mode_props::derive,
    },
    Derivation {
        name: "economy",
        derive: economy::derive,
    },
    Derivation {
        name: "navigation",
        derive: navigation::derive,
    },
    Derivation {
        name: "game_objectives",
        derive: objectives::derive,
    },
    Derivation {
        name: "game_progression",
        derive: progression::derive,
    },
];

/// Run every derivation and write its files under `out`.
pub fn build(install: &Install, out: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for derivation in DERIVATIONS {
        for output in (derivation.derive)(install, out)? {
            let path = out.join(&output.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, &output.bytes)?;
            written.push(path);
        }
    }
    Ok(written)
}

/// How a derived file compares with the oracle pack's.
#[derive(Debug, PartialEq)]
pub enum Difference {
    Same,
    /// The oracle has no such file.
    Missing,
    /// The first differing JSON path, or `bytes` for a binary file.
    Differs(String),
}

/// Compare a derived file with the pack's: JSON by value (`.json` and
/// `.json.gz`), everything else by bytes.
pub fn diff(output: &Output, pack: &Path) -> Result<Difference> {
    let oracle = match fs::read(pack.join(&output.path)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Difference::Missing),
        Err(error) => return Err(error.into()),
    };
    let decode = |bytes: &[u8]| -> Result<Option<serde_json::Value>> {
        let text = if output.path.ends_with(".json.gz") {
            let mut text = Vec::new();
            io::Read::read_to_end(&mut flate2::read::GzDecoder::new(bytes), &mut text)?;
            text
        } else if output.path.ends_with(".json") {
            bytes.to_vec()
        } else {
            return Ok(None);
        };
        serde_json::from_slice(&text)
            .map(Some)
            .map_err(|error| asset_error(format!("{}: {error}", output.path)))
    };
    Ok(match (decode(&output.bytes)?, decode(&oracle)?) {
        (Some(derived), Some(oracle)) => match first_difference(&derived, &oracle, "") {
            None => Difference::Same,
            Some(path) => Difference::Differs(path),
        },
        _ if output.bytes == oracle => Difference::Same,
        _ => Difference::Differs("bytes".into()),
    })
}

/// The JSON path of the first place two values differ.
pub fn first_difference(a: &serde_json::Value, b: &serde_json::Value, at: &str) -> Option<String> {
    use serde_json::Value::{Array, Object};
    match (a, b) {
        (Object(a), Object(b)) => {
            for (key, value) in a {
                let path = format!("{at}.{key}");
                match b.get(key) {
                    None => return Some(format!("{path} (only derived)")),
                    Some(other) => {
                        if let Some(found) = first_difference(value, other, &path) {
                            return Some(found);
                        }
                    }
                }
            }
            b.keys()
                .find(|key| !a.contains_key(*key))
                .map(|key| format!("{at}.{key} (only in the pack)"))
        }
        (Array(a), Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{at} (length {} vs {})", a.len(), b.len()));
            }
            a.iter()
                .zip(b)
                .enumerate()
                .find_map(|(index, (a, b))| first_difference(a, b, &format!("{at}[{index}]")))
        }
        _ if numbers_equal(a, b) || a == b => None,
        _ => Some(format!("{at} ({a} vs {b})")),
    }
}

/// Integers and floats compare by value, as Python's `json` round trip
/// writes `1.0` where Rust may write `1`.
fn numbers_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}
