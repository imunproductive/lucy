//! `data/collision_props_174.json`, reduced to what the server reads: each
//! game mode's placeable identifiers (`modes`), which decide the props of a
//! mode (`lucy_sim`'s scene assets keep a collision cache's placements of
//! the identifiers the mode lists).
//!
//! The Python reference's catalogue (made by a scratch tool that isn't in the
//! repository) also held the storage keys of each map's ENTITY chunk
//! (`maps`), the definitions that are props with their models (`defs`) and
//! the models' keys (`models`). The manifests resolve the chunks and models
//! by GUID, and [`crate::props`] applies the catalogue's rule to the
//! definitions, so nothing reads those parts any more.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::{Install, Output, Result, game_maps::mode_identifiers, guid_hex};

pub(crate) const PATH: &str = "data/collision_props_174.json";

const SOURCE: &str = "Build 1.74.0.0.104319: each game mode's (0C5) m_A43573F4 identifiers, the modes that list \
any (a placeable with an identifier exists only in the modes that list it).";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut modes = Map::new();
    for (mode, identifiers) in mode_identifiers(install) {
        if !identifiers.is_empty() {
            let identifiers: Vec<String> = identifiers.into_iter().map(guid_hex).collect();
            modes.insert(guid_hex(mode), json!(identifiers));
        }
    }
    let mut table = Map::new();
    table.insert("_source".into(), json!(SOURCE));
    table.insert("modes".into(), Value::Object(modes));
    Ok(vec![Output::json(PATH, &Value::Object(table))])
}
