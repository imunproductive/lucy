//! The loot box economy of the install (`data/economy_174.json`, derived
//! from the player progression settings `054/0018` by `lucy_assets`): the
//! values `ow174/services/lootbox.py` (`DUPLICATE_CREDITS`, `CREDIT_DROPS`)
//! and `ow174/catalog/events.py` (each event's box) hold, read from the game
//! data instead.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
};

use serde::Deserialize;

use lucy_core::guid::parse_guid;

pub const FILE: &str = "economy_174.json";

/// The currency unlock a slot holds in place of an item, and its credits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreditDrop {
    pub unlock: u64,
    pub amount: i64,
}

/// The loot box a celebration gives at login, and its box type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventBox {
    pub unlock: u64,
    pub box_type: i32,
}

/// What the shop charges, by currency and rarity (`item_prices`): the base
/// box's credit prices, the golden weapons' price in competitive points,
/// the Overwatch League items' price in league tokens, and each event box
/// type's credit prices, which the items of `event_priced` cost
/// (`lucy_assets` `economy.rs`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ItemPrices {
    pub credits: HashMap<String, i64>,
    pub competitive_points: i64,
    pub league_tokens: HashMap<String, i64>,
    /// Box type (`Enum_BABC4175`) -> rarity -> credits.
    pub event_credits: HashMap<i64, HashMap<String, i64>>,
    /// The event box items sold at their box type's price.
    #[serde(deserialize_with = "guid_set")]
    pub event_priced: HashSet<u64>,
}

fn guid_set<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<HashSet<u64>, D::Error> {
    Vec::<String>::deserialize(deserializer)?
        .iter()
        .map(|text| guid(text).map_err(serde::de::Error::custom))
        .collect()
}

#[derive(Debug, Default)]
pub struct Economy {
    /// Rarity -> the credits a duplicate gives.
    duplicate_credits: HashMap<String, i64>,
    /// Rarity -> what a base box slot holds when it holds credits.
    credit_drops: HashMap<String, CreditDrop>,
    /// Celebration type (`01C` GUID) -> its box.
    event_boxes: HashMap<u64, EventBox>,
    item_prices: ItemPrices,
}

#[derive(Deserialize)]
struct Raw {
    duplicate_credits: HashMap<String, i64>,
    credit_drops: HashMap<String, RawDrop>,
    celebration_boxes: HashMap<String, RawBox>,
    item_prices: ItemPrices,
}

#[derive(Deserialize)]
struct RawDrop {
    unlock: String,
    amount: i64,
}

#[derive(Deserialize)]
struct RawBox {
    unlock: String,
    box_type: i32,
}

impl Economy {
    pub fn load(data_dir: &Path) -> io::Result<Self> {
        let path = data_dir.join(FILE);
        let file = File::open(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_reader(BufReader::new(file))
    }

    pub fn from_reader(reader: impl Read) -> io::Result<Self> {
        let raw: Raw = serde_json::from_reader(reader)
            .map_err(|error| invalid(format!("invalid {FILE}: {error}")))?;
        let credit_drops = raw
            .credit_drops
            .into_iter()
            .map(|(rarity, drop)| {
                let unlock = guid(&drop.unlock)?;
                Ok((
                    rarity,
                    CreditDrop {
                        unlock,
                        amount: drop.amount,
                    },
                ))
            })
            .collect::<io::Result<_>>()?;
        let event_boxes = raw
            .celebration_boxes
            .into_iter()
            .map(|(kind, event_box)| {
                Ok((
                    guid(&kind)?,
                    EventBox {
                        unlock: guid(&event_box.unlock)?,
                        box_type: event_box.box_type,
                    },
                ))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            duplicate_credits: raw.duplicate_credits,
            credit_drops,
            event_boxes,
            item_prices: raw.item_prices,
        })
    }

    pub fn item_prices(&self) -> &ItemPrices {
        &self.item_prices
    }

    /// The credits a duplicate item of `rarity` gives.
    pub fn duplicate_credits(&self, rarity: &str) -> i64 {
        self.duplicate_credits.get(rarity).copied().unwrap_or(0)
    }

    /// What a base box slot of `rarity` holds when it holds credits.
    pub fn credit_drop(&self, rarity: &str) -> Option<CreditDrop> {
        self.credit_drops.get(rarity).copied()
    }

    /// The box a celebration type (`01C` GUID) gives at login.
    pub fn event_box(&self, kind: u64) -> Option<EventBox> {
        self.event_boxes.get(&kind).copied()
    }
}

fn guid(text: &str) -> io::Result<u64> {
    parse_guid(text).ok_or_else(|| invalid(format!("{FILE}: invalid GUID {text:?}")))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
