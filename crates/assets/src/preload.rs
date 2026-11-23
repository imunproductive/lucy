//! `data/lobby_preload_174.json`: the four lists of the lobby's preload
//! message (20505), which the client loads before the main menu.
//!
//! - `heroes` (`+0x78`): every hero record (`075`). The client makes a hero
//!   playable only when its record is resident (with all of them the hero
//!   select shows the full roster).
//! - `skin_themes` (`+0x90`): each hero's skin themes (`075`
//!   `m_skinThemes[].m_5E9665E3`, `0A6`), resources the client loads at
//!   priority 2. The 2,457 themes of the 1.68 retail capture are exactly
//!   these lists as 1.68 had them; 1.74 adds 26 (the Anniversary menu themes
//!   among them).
//! - `map_headers` (`+0xA8`): every map header (`09F`). A map (`002`) in
//!   these lists crashes the client.
//! - `resident` (`+0xC0`): every `10D` asset; the install has one, the
//!   retail capture listed it.

use std::path::Path;

use serde_json::json;

use crate::{
    Install, Output, Result,
    game_queues::{Node, key, load, typed},
};

pub(crate) const PATH: &str = "data/lobby_preload_174.json";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let heroes = load(install, 0x075)?;
    let mut themes = Vec::new();
    for asset in heroes.values() {
        for theme in Node::root(asset)?.nodes("m_skinThemes") {
            let theme = key(theme.guid("m_5E9665E3"));
            if !themes.contains(&theme) {
                themes.push(theme);
            }
        }
    }
    let keys = |kind: u16| -> Result<Vec<String>> {
        Ok(load(install, kind)?
            .keys()
            .map(|&index| key(typed(index, kind)))
            .collect())
    };
    let preload = json!({
        "heroes": heroes.keys().map(|&index| key(typed(index, 0x075))).collect::<Vec<_>>(),
        "skin_themes": themes,
        "map_headers": keys(0x09F)?,
        "resident": keys(0x10D)?,
    });
    Ok(vec![Output::json(PATH, &preload)])
}
