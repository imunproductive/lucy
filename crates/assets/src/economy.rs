//! `data/economy_174.json`: the loot box economy the lobby reads, from the
//! player progression settings (`054/0018`,
//! `STUGenericSettings_PlayerProgression`).
//!
//! - `duplicate_credits`: the credits a duplicate item gives, by rarity
//!   (`ow174/services/lootbox.py` `DUPLICATE_CREDITS`). The settings price
//!   every rarity of each box type's pool in each currency (`m_0C6A363E`, an
//!   `STU_F021DC6B` per box type and currency, an `STU_3B4E8CA6` per rarity:
//!   `m_CE91A110` the price, `m_AB5A6142` what a duplicate is worth); the
//!   base box's credit table gives them.
//! - `credit_drops`: the currency unlock a base box holds in place of an
//!   item of each rarity (`m_lootBoxesCurrencyUnlocks`) and the credits it
//!   gives (`0A5`, `STUUnlock_Currency.m_currency`; `lootbox.py`
//!   `CREDIT_DROPS`).
//! - `celebration_boxes`: the event box each celebration type (`01C`) gives
//!   (`m_lootBoxCelebrationOverrides`) and the box type of that box
//!   (`STUUnlock_LootBox.m_lootboxType`; `ow174/catalog/events.py`).
//!
//! - `item_prices`: what the shop charges for an item, by currency and
//!   rarity: `credits` from the base box's credit table, `competitive_points`
//!   (the golden weapons' one price) and `league_tokens` (the Overwatch
//!   League items). `event_credits`: each event box type's own credit
//!   table (three times the base prices), which retail charged for the
//!   items of `event_priced`: the event box items (`data/extracted_items.json`,
//!   `data/extracted_general_unlocks.json` `LootBoxesUnlocks`) whose unlock
//!   leaves `m_1A546C64` (`STUUnlock`) unset. In the 1.68 retail capture
//!   exactly the Summer Games, Halloween, Winter and Lunar New Year items it
//!   sold at the event price have it unset; the Archives and Anniversary
//!   items it sold so are set in 1.74, whose unset ones are Anniversary
//!   2022's skins (`D:/Work/Coding/refs/reports/RetailTemplates.md`, "Event
//!   prices"). [INFERENCE] the flag marks an item retail sells at the base
//!   price.

use std::{collections::BTreeSet, path::Path};

use serde_json::{Map, Value as Json, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{Node, key, typed},
    heroes::read_json,
};

pub(crate) const PATH: &str = "data/economy_174.json";

/// `054/0018`, the player progression settings.
const PROGRESSION: u64 = 0x18;
const GENERIC_SETTINGS: u16 = 0x054;

/// `Enum_BABC4175::Base`, the standard loot box.
const BASE_BOX: i64 = 0;
/// `Enum_14895597` `x709296A6` (0): a price table in credits; 1, the
/// golden weapons' competitive points (one price); 2, the Overwatch League
/// tokens.
const CREDITS: i64 = 0;
const COMPETITIVE_POINTS: i64 = 1;
const LEAGUE_TOKENS: i64 = 2;

/// `STUUnlockRarity`, the rarities a box holds.
const RARITIES: [(i64, &str); 4] = [(0, "Common"), (1, "Rare"), (2, "Epic"), (3, "Legendary")];

pub(crate) fn derive(install: &Install, out: &Path) -> Result<Vec<Output>> {
    let asset = install.stu(typed(PROGRESSION, GENERIC_SETTINGS))?;
    let progression = Node::root(&asset)?;
    let mut prices = item_prices(progression)?;
    prices["event_credits"] = event_credits(progression)?;
    let event_boxes: BTreeSet<String> = LOOT_BOX_NAMES
        .iter()
        .filter(|(box_type, _)| prices["event_credits"].get(box_type.to_string()).is_some())
        .map(|(_, name)| (*name).to_owned())
        .collect();
    prices["event_priced"] = event_priced(install, out, &event_boxes)?;
    let table = json!({
        "duplicate_credits": duplicate_credits(progression)?,
        "credit_drops": credit_drops(install, progression)?,
        "celebration_boxes": celebration_boxes(install, progression)?,
        "item_prices": prices,
    });
    Ok(vec![Output::json(PATH, &table)])
}

/// `Enum_BABC4175`'s box types as the item catalog names them
/// (`lists::unlocks` `LOOT_BOXES`).
const LOOT_BOX_NAMES: [(i64, &str); 8] = [
    (0, "Base"),
    (1, "SummerGames"),
    (2, "Halloween"),
    (3, "Winter"),
    (4, "LunarNewYear"),
    (5, "Archives"),
    (6, "Anniversary"),
    (8, "Internal"),
];

/// Each event box type's credit prices by rarity, keyed by the box type.
fn event_credits(progression: Node) -> Result<Json> {
    let mut tables = Map::new();
    for table in progression.nodes("m_0C6A363E") {
        let box_type = table.int("m_A9D270A9");
        if box_type == BASE_BOX || table.int("m_227307C8") != CREDITS {
            continue;
        }
        tables.insert(box_type.to_string(), by_rarity(table)?);
    }
    Ok(Json::Object(tables))
}

/// The items of the event boxes (`boxes`, by name) whose unlock leaves
/// `m_1A546C64` unset, in GUID order.
fn event_priced(install: &Install, out: &Path, boxes: &BTreeSet<String>) -> Result<Json> {
    let mut indexes = BTreeSet::new();
    for path in [
        "data/extracted_items.json",
        "data/extracted_general_unlocks.json",
    ] {
        box_items(&read_json(out, path)?, boxes, &mut indexes);
    }
    let guids: Vec<u64> = install
        .guids(UNLOCK)
        .into_iter()
        .filter(|guid| indexes.contains(&(guid & INDEX)))
        .collect();
    let mut priced = Vec::new();
    for (guid, asset) in install.stu_all(&guids) {
        if Node::root(&asset?)?.int("m_1A546C64") == 0 {
            priced.push(key(guid));
        }
    }
    Ok(priced.into())
}

const UNLOCK: u16 = 0x0A5;
const INDEX: u64 = 0xFFFF_FFFF_FFFF;

/// The indexes of the unlocks the `LootBoxesUnlocks` groups of `boxes` hold
/// anywhere in a catalog file.
fn box_items(value: &Json, boxes: &BTreeSet<String>, indexes: &mut BTreeSet<u64>) {
    match value {
        Json::Object(map) => {
            for (name, value) in map {
                if name == "LootBoxesUnlocks" {
                    for group in value.as_array().into_iter().flatten() {
                        let listed = group["LootBoxType"]
                            .as_str()
                            .is_some_and(|name| boxes.contains(name));
                        let unlocks = group["Unlocks"].as_array().into_iter().flatten();
                        for unlock in unlocks.filter(|_| listed) {
                            let index = unlock["GUID"]
                                .as_str()
                                .and_then(|guid| guid.split('.').next())
                                .and_then(|index| u64::from_str_radix(index, 16).ok());
                            indexes.extend(index);
                        }
                    }
                } else {
                    box_items(value, boxes, indexes);
                }
            }
        }
        Json::Array(items) => items
            .iter()
            .for_each(|item| box_items(item, boxes, indexes)),
        _ => {}
    }
}

fn rarity_name(rarity: i64) -> Result<&'static str> {
    RARITIES
        .iter()
        .find(|&&(value, _)| value == rarity)
        .map(|&(_, name)| name)
        .ok_or_else(|| asset_error(format!("054/0018: unexpected rarity {rarity}")))
}

fn duplicate_credits(progression: Node) -> Result<Json> {
    let table = progression
        .nodes("m_0C6A363E")
        .find(|table| table.int("m_A9D270A9") == BASE_BOX && table.int("m_227307C8") == CREDITS)
        .ok_or_else(|| asset_error("054/0018 has no credit prices for the base box"))?;
    let mut credits = Map::new();
    for &(rarity, name) in &RARITIES {
        let price = table
            .nodes("m_4C6FDF24")
            .find(|price| price.int("m_rarity") == rarity)
            .ok_or_else(|| asset_error(format!("054/0018 has no base {name} price")))?;
        credits.insert(name.into(), price.int("m_AB5A6142").into());
    }
    Ok(Json::Object(credits))
}

fn credit_drops(install: &Install, progression: Node) -> Result<Json> {
    let base = progression
        .nodes("m_lootBoxesCurrencyUnlocks")
        .find(|unlocks| unlocks.int("m_lootboxType") == BASE_BOX)
        .ok_or_else(|| asset_error("054/0018 has no base box currency unlocks"))?;
    let mut drops = Map::new();
    for drop in base.nodes("m_unlocks") {
        let unlock = drop.guid("m_unlock");
        let asset = install.stu(unlock)?;
        let amount = Node::root(&asset)?.int("m_currency");
        drops.insert(
            rarity_name(drop.int("m_rarity"))?.into(),
            json!({"unlock": key(unlock), "amount": amount}),
        );
    }
    Ok(Json::Object(drops))
}

fn celebration_boxes(install: &Install, progression: Node) -> Result<Json> {
    let mut boxes = Map::new();
    for event in progression.nodes("m_lootBoxCelebrationOverrides") {
        let unlock = event.guid("m_lootbox");
        let asset = install.stu(unlock)?;
        let box_type = Node::root(&asset)?.int("m_lootboxType");
        boxes.insert(
            key(event.guid("m_celebrationType")),
            json!({"unlock": key(unlock), "box_type": box_type}),
        );
    }
    Ok(Json::Object(boxes))
}

fn item_prices(progression: Node) -> Result<Json> {
    let table = |currency: i64, base_box: bool| {
        progression
            .nodes("m_0C6A363E")
            .find(|table| {
                table.int("m_227307C8") == currency
                    && (!base_box || table.int("m_A9D270A9") == BASE_BOX)
            })
            .ok_or_else(|| {
                asset_error(format!(
                    "054/0018 has no price table in currency {currency}"
                ))
            })
    };
    let points = table(COMPETITIVE_POINTS, false)?
        .nodes("m_4C6FDF24")
        .next()
        .ok_or_else(|| asset_error("054/0018 has no competitive point price"))?
        .int("m_CE91A110");
    Ok(json!({
        "credits": by_rarity(table(CREDITS, true)?)?,
        "competitive_points": points,
        "league_tokens": by_rarity(table(LEAGUE_TOKENS, false)?)?,
    }))
}

/// A price table's prices by rarity name.
fn by_rarity(table: Node) -> Result<Json> {
    let mut prices = Map::new();
    for &(rarity, name) in &RARITIES {
        let price = table
            .nodes("m_4C6FDF24")
            .find(|price| price.int("m_rarity") == rarity)
            .ok_or_else(|| asset_error(format!("054/0018 has no {name} price")))?;
        prices.insert(name.into(), price.int("m_CE91A110").into());
    }
    Ok(Json::Object(prices))
}
