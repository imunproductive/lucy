//! `data/arcade_cards_174.json`: the game's Arcade cards (`0C7`). Port of
//! `tools/extract_arcade_cards.py`.
//!
//! A card in message 39802 is the wire form of its `0C7` asset: the
//! identifier, the display entry, the reward unlocks and seven of its eleven
//! flags come straight from the asset. The server adds the rest (how long a
//! card stays, whether it is permanent).

use std::path::Path;

use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result,
    game_queues::{INDEX, Node, key},
};

/// Card flags 13-19 of message 39802, in wire order, and the asset fields
/// they come from.
const ASSET_FLAGS: [&str; 7] = [
    "m_157AF68C",
    "m_5E1EE04A",
    "m_D66FABE0",
    "m_7F08E444",
    "m_313C5ED5",
    "m_F011ED7A",
    "m_8F862EC7",
];

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut cards = Map::new();
    for (guid, asset) in install.stu_all(&install.guids(0x0C7)) {
        let asset = asset?;
        cards.insert(format!("0x{:X}", guid & INDEX), card(Node::root(&asset)?));
    }
    Ok(vec![Output::json(
        "data/arcade_cards_174.json",
        &Json::Object(cards),
    )])
}

fn card(card: Node) -> Json {
    let reference = |guid: u64| if guid == 0 { "0x0".into() } else { key(guid) };
    json!({
        "identifier": reference(card.guid("m_0503A846")),
        "display": reference(card.guid("m_A848F2C7")),
        "rewards": card.guids("m_B1449DF7").map(key).collect::<Vec<_>>(),
        "flags": ASSET_FLAGS.map(|flag| card.int(flag) == 1),
        // Role queue cards list their role slots (the role in `m_1F753B44`,
        // 2 players each).
        "roles": card.nodes("m_66C923AF").any(|slot| slot.guid("m_1F753B44") != 0),
        // The priority pass pool of a role queue (the client reads it at
        // card +0x250).
        "passes": reference(card.guid("m_DD50F6A1")),
    })
}
