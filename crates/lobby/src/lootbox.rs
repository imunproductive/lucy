//! Opening loot boxes against the 1.74 catalog.
//!
//! Ports `ow174/services/lootbox.py`. Every store entry of the hero and
//! account catalogs has a price and an event: 0 for the base pool, 1-6 for a
//! seasonal event, -1 for items that never drop from boxes (challenge rewards
//! among them). Icons have no price, as the game never sold them, but they
//! drop. Rarity and item type come from the extracted unlock list; the credits
//! a duplicate gives and the credit drops come from the install
//! ([`Economy`]).

use std::{collections::HashMap, io, sync::Arc};

use serde_json::Value as Json;

use crate::{
    accounts::{Profile, box_type},
    collection::{Collection, PROGRESSION},
    economy::{CreditDrop, Economy},
    transport::LobbyMessage,
};
use lucy_wire::jam::codec::{
    Fields,
    Value::{Array, Bool, Signed, Struct, Unsigned},
};

/// The client's progression requests: 24201 open a box, 24203 buy an item.
pub const PROGRESSION_OUT: u32 = 0x7f4f_46cb;
pub const OPEN_BOX: u32 = 24201;
pub const BOX_OPENED: u32 = 24305;
pub const CREDITS: u32 = 24307;

/// Four slots, as retail's boxes. Not found in the install: the loot box
/// rules (`054/011D`, `STULootBoxRules`) hold two unnamed numbers per box
/// type (`m_24391F9B` 21, `m_53B8C818` 50), neither of them 4.
const SLOTS_PER_BOX: usize = 4;
/// A profile with fewer boxes left is refilled with the starter set.
const LOW_STOCK: usize = 5;
const LOOTABLE_TYPES: [&str; 7] = [
    "Skin",
    "Emote",
    "VictoryPose",
    "VoiceLine",
    "Spray",
    "HighlightIntro",
    "Icon",
];
const LEGENDARY: &str = "Legendary";
const EPIC: &str = "Epic";
const RARE: &str = "Rare";
const COMMON: &str = "Common";
// Chance of each rarity for an ordinary slot, and for the one slot that is at
// least rare. They give the odds of the game's own "average chance from
// containers" screen for a whole box: some common item 99%, rare 94% (here
// 95%), epic 18.5%, legendary 7.5%. Python fitted them to that screen. The
// install's loot box rules (`054/011D`, `STULootBoxRarityRules`) give each
// box type two numbers per rarity (`m_A07D6F99`/`m_CBE06580`: base box rare
// 1/20, epic 1/40, legendary 1/100; currency rules 11/30, 13/42, 91/250),
// whose meaning isn't known: read as odds per slot or per box they give none
// of the screen's chances, so Python's odds are kept.
const SLOT_ODDS: [(&str, f64); 3] = [(LEGENDARY, 0.0193), (EPIC, 0.0498), (RARE, 0.10)];
const GUARANTEED_ODDS: [(&str, f64); 2] = [(LEGENDARY, 0.0193), (EPIC, 0.0498)];

/// Credits drop in place of an item of the same rarity (the install's
/// currency unlocks, [`Economy::credit_drop`]; 500 is the chest full of
/// coins), so a box keeps the rarity odds of the game's odds screen. Retail's
/// share of credits in a rarity is not in the install (unless it is the
/// undecoded currency rules above): Python's guess is that half of the
/// legendary slots hold the 500 (as often as a legendary item) and 1 in 20
/// rare or epic slots holds 50 or 150. The guess is kept.
fn credit_share(rarity: &str) -> Option<f64> {
    match rarity {
        RARE | EPIC => Some(0.05),
        LEGENDARY => Some(0.5),
        _ => None,
    }
}

/// The random rolls of a box: Python's `random.random()` and `random.choice()`.
pub trait Dice {
    /// A number in `0.0..1.0`.
    fn unit(&mut self) -> io::Result<f64>;
    /// An index in `0..len`; `len` is never 0.
    fn below(&mut self, len: usize) -> io::Result<usize>;
}

/// Rolls from the operating system's random source.
pub struct SystemDice;

impl Dice for SystemDice {
    fn unit(&mut self) -> io::Result<f64> {
        Ok((random_u64()? >> 11) as f64 / (1u64 << 53) as f64)
    }

    fn below(&mut self, len: usize) -> io::Result<usize> {
        let len = len as u64;
        let limit = u64::MAX - u64::MAX % len;
        loop {
            let number = random_u64()?;
            if number < limit {
                return Ok((number % len) as usize);
            }
        }
    }
}

fn random_u64() -> io::Result<u64> {
    let mut entropy = [0; 8];
    getrandom::fill(&mut entropy).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(u64::from_le_bytes(entropy))
}

/// One slot of an opened box.
#[derive(Clone, Debug, PartialEq)]
pub struct Drop {
    pub hero: u64,
    pub unlock: u64,
    /// Credits given for a duplicate.
    pub credits: i64,
    pub order: u32,
    /// The box type, for a slot from the box's event pool.
    pub highlight: i64,
    pub duplicate: bool,
    pub new: bool,
    pub rarity: String,
    /// The credits of a slot that gave credits instead of an item.
    pub amount: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub struct BoxOpening {
    pub drops: Vec<Drop>,
    pub box_name: String,
}

/// The loot pools of the catalog and the install's credit values.
pub struct LootBoxes {
    /// (event, rarity) -> item GUIDs, in catalog order.
    pools: HashMap<(i64, String), Vec<u64>>,
    /// (hero name, rarity) -> the hero's base-pool items.
    hero_pools: HashMap<(String, String), Vec<u64>>,
    economy: Arc<Economy>,
}

impl LootBoxes {
    pub fn new(collection: &Collection, economy: Arc<Economy>) -> Self {
        let mut pools = HashMap::<_, Vec<u64>>::new();
        let mut hero_pools = HashMap::<_, Vec<u64>>::new();
        for (&guid, entry) in collection.store() {
            let Some(unlock) = collection.items().get(guid) else {
                continue;
            };
            if entry.event < 0 || !LOOTABLE_TYPES.contains(&unlock.item_type.as_str()) {
                continue;
            }
            if entry.price <= 0 && unlock.item_type != "Icon" {
                continue;
            }
            pools
                .entry((entry.event, unlock.rarity.clone()))
                .or_default()
                .push(guid);
            if let Some(hero) = unlock.hero.as_deref().filter(|hero| !hero.is_empty()) {
                if entry.event == 0 {
                    hero_pools
                        .entry((hero.to_owned(), unlock.rarity.clone()))
                        .or_default()
                        .push(guid);
                }
            }
        }
        Self {
            pools,
            hero_pools,
            economy,
        }
    }

    /// Open a box: remove it from the profile, add the drops and credits,
    /// update the stats. Without `refill` a low stock stays low. The caller
    /// saves the profile. An unknown box id opens a standard box.
    pub fn open(
        &self,
        box_id: u64,
        profile: &mut Profile,
        collection: &Collection,
        refill: bool,
        dice: &mut impl Dice,
    ) -> io::Result<BoxOpening> {
        let (kind_id, box_name) = match profile.loot_boxes.iter().position(|item| item.id == box_id)
        {
            Some(index) => {
                let taken = profile.loot_boxes.remove(index);
                (taken.box_type, taken.name)
            }
            None => (0, "Standard".to_owned()),
        };
        let kind = box_type(kind_id);
        let event = kind.map_or(0, |kind| kind.event);
        let hero = kind.map_or("", |kind| kind.hero);
        let legendary = kind.is_some_and(|kind| kind.legendary);
        let rarities = roll_slot_rarities(legendary, dice)?;
        let events = roll_slot_events(event, dice)?;
        let promised = legendary
            .then(|| rarities.iter().position(|&rarity| rarity == LEGENDARY))
            .flatten();

        // Worked out once per box: asking the collection for each candidate is slow.
        let mut owned = collection.owned_set(profile);
        let unlock_all = profile.unlock_all;
        let mut drops = Vec::with_capacity(SLOTS_PER_BOX);
        let mut credits = 0;
        for (order, (&rarity, &slot_event)) in rarities.iter().zip(&events).enumerate() {
            if let Some(drop) = roll_credits(
                rarity,
                slot_event,
                Some(order) == promised,
                &self.economy,
                dice,
            )? {
                profile.credits += drop.amount;
                drops.push(Drop {
                    hero: 0,
                    unlock: drop.unlock,
                    credits: 0,
                    order: order as u32,
                    highlight: 0,
                    duplicate: false,
                    new: false,
                    rarity: rarity.into(),
                    amount: Some(drop.amount),
                });
                continue;
            }
            let is_owned = |guid: &u64| unlock_all || owned.contains(guid);
            let guid = if hero.is_empty() {
                self.pick(slot_event, rarity, is_owned, dice)?
            } else {
                self.pick_hero_item(hero, rarity, is_owned, dice)?
            };
            let duplicate = unlock_all || owned.contains(&guid);
            let duplicate_credits = if duplicate {
                self.economy.duplicate_credits(rarity)
            } else {
                0
            };
            credits += duplicate_credits;
            if !duplicate {
                owned.insert(guid);
                profile
                    .unlocked_items
                    .push(Json::String(format!("0x{guid:016X}")));
            }
            drops.push(Drop {
                hero: collection.hero_for(guid).unwrap_or(0),
                unlock: guid,
                credits: duplicate_credits,
                order: order as u32,
                highlight: if slot_event != 0 {
                    i64::from(kind_id)
                } else {
                    0
                },
                duplicate,
                new: !duplicate,
                rarity: rarity.into(),
                amount: None,
            });
        }
        profile.credits += credits;
        count_stats(profile, &rarities, credits);
        if refill && profile.loot_boxes.len() < LOW_STOCK {
            profile.add_starter_boxes()?;
        }
        Ok(BoxOpening { drops, box_name })
    }

    /// Open every box, as the move to Overwatch 2 did. Returns how many were
    /// opened and the items that were new.
    pub fn open_all(
        &self,
        profile: &mut Profile,
        collection: &Collection,
        dice: &mut impl Dice,
    ) -> io::Result<(usize, Vec<u64>)> {
        let box_ids: Vec<u64> = profile.loot_boxes.iter().map(|item| item.id).collect();
        let mut granted = Vec::new();
        for &box_id in &box_ids {
            let opening = self.open(box_id, profile, collection, false, dice)?;
            granted.extend(
                opening
                    .drops
                    .iter()
                    .filter(|drop| drop.new)
                    .map(|drop| drop.unlock),
            );
        }
        Ok((box_ids.len(), granted))
    }

    /// An item of the rarity from the event pool, then the base pool. Like the
    /// real game it prefers items the player does not own yet: a duplicate only
    /// drops when every candidate of that rarity is owned.
    fn pick(
        &self,
        event: i64,
        rarity: &str,
        is_owned: impl Fn(&u64) -> bool,
        dice: &mut impl Dice,
    ) -> io::Result<u64> {
        let mut pools: Vec<&[u64]> = [event, 0]
            .into_iter()
            .filter_map(|pool_event| self.pools.get(&(pool_event, rarity.to_owned())))
            .filter(|pool| !pool.is_empty())
            .map(Vec::as_slice)
            .collect();
        if pools.is_empty() {
            pools.push(
                self.pools
                    .get(&(0, COMMON.to_owned()))
                    .ok_or_else(|| missing_pool("the base common"))?,
            );
        }
        for pool in &pools {
            let unowned: Vec<u64> = pool
                .iter()
                .copied()
                .filter(|guid| !is_owned(guid))
                .collect();
            if !unowned.is_empty() {
                return choose(&unowned, dice);
            }
        }
        choose(pools[0], dice)
    }

    /// An item for a hero's box (the Wrecking Ball box), which only holds that
    /// hero's items from the base pool, preferring items not owned yet.
    fn pick_hero_item(
        &self,
        hero: &str,
        rarity: &str,
        is_owned: impl Fn(&u64) -> bool,
        dice: &mut impl Dice,
    ) -> io::Result<u64> {
        let pool = self
            .hero_pools
            .get(&(hero.to_owned(), rarity.to_owned()))
            .ok_or_else(|| missing_pool(&format!("{hero}'s {rarity}")))?;
        let unowned: Vec<u64> = pool
            .iter()
            .copied()
            .filter(|guid| !is_owned(guid))
            .collect();
        choose(if unowned.is_empty() { pool } else { &unowned }, dice)
    }
}

fn choose(pool: &[u64], dice: &mut impl Dice) -> io::Result<u64> {
    if pool.is_empty() {
        return Err(missing_pool("an empty"));
    }
    Ok(pool[dice.below(pool.len())?])
}

fn missing_pool(which: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the catalog has no {which} loot pool"),
    )
}

fn roll_rarity(
    guaranteed: bool,
    legendary: bool,
    dice: &mut impl Dice,
) -> io::Result<&'static str> {
    if legendary {
        return Ok(LEGENDARY);
    }
    let odds: &[(&str, f64)] = if guaranteed {
        &GUARANTEED_ODDS
    } else {
        &SLOT_ODDS
    };
    let mut roll = dice.unit()?;
    for &(rarity, chance) in odds {
        if roll < chance {
            return Ok(rarity);
        }
        roll -= chance;
    }
    Ok(if guaranteed { RARE } else { COMMON })
}

/// One guaranteed slot and three ordinary ones, in random order (Python's
/// `random.shuffle`).
fn roll_slot_rarities(
    always_legendary: bool,
    dice: &mut impl Dice,
) -> io::Result<Vec<&'static str>> {
    let mut rarities = vec![roll_rarity(true, always_legendary, dice)?];
    for _ in 1..SLOTS_PER_BOX {
        rarities.push(roll_rarity(false, false, dice)?);
    }
    for index in (1..rarities.len()).rev() {
        let other = dice.below(index + 1)?;
        rarities.swap(index, other);
    }
    Ok(rarities)
}

/// The pool event of each slot. In an event box the first slot is always from
/// the event, each other one half the time.
fn roll_slot_events(event: i64, dice: &mut impl Dice) -> io::Result<Vec<i64>> {
    (0..SLOTS_PER_BOX)
        .map(|slot| {
            Ok(if event != 0 && (slot == 0 || dice.unit()? < 0.5) {
                event
            } else {
                0
            })
        })
        .collect()
}

/// The currency unlock and credits a slot gives instead of an item of its
/// rarity, if any. Never in an event slot, nor in place of the legendary item
/// a legendary box promises.
fn roll_credits(
    rarity: &str,
    slot_event: i64,
    promised: bool,
    economy: &Economy,
    dice: &mut impl Dice,
) -> io::Result<Option<CreditDrop>> {
    if slot_event != 0 || promised {
        return Ok(None);
    }
    let Some(share) = credit_share(rarity) else {
        return Ok(None);
    };
    Ok(if dice.unit()? < share {
        economy.credit_drop(rarity)
    } else {
        None
    })
}

fn count_stats(profile: &mut Profile, rarities: &[&str], credits: i64) {
    let legendaries = rarities
        .iter()
        .filter(|&&rarity| rarity == LEGENDARY)
        .count() as i64;
    for (key, amount) in [
        ("boxes_opened", 1),
        ("legendaries_dropped", legendaries),
        ("credits_earned_from_duplicates", credits),
    ] {
        let total = profile.stats.get(key).and_then(Json::as_i64).unwrap_or(0) + amount;
        profile.stats.insert(key.into(), Json::from(total));
    }
}

/// The box id in a client "open box" request (24201 +0x78 +0x0 [0]).
pub fn box_id(fields: &Fields) -> Option<u64> {
    let Some(Struct(id)) = fields.get(&0x78) else {
        return None;
    };
    match id.get(&0)? {
        Array(words) => match words.first()? {
            Unsigned(id) => Some(*id),
            _ => None,
        },
        _ => None,
    }
}

/// 24305: what the client shows after a box opens.
pub fn opened_message(box_id: [u64; 2], drops: &[Drop]) -> LobbyMessage {
    let items = drops
        .iter()
        .map(|drop| {
            Struct(Fields::from([
                (0x00, Unsigned(drop.hero)),
                (0x08, Unsigned(drop.unlock)),
                (0x10, Signed(drop.credits)),
                (0x14, Unsigned(u64::from(drop.order))),
                (0x18, Signed(drop.highlight)),
                (0x1c, Bool(drop.duplicate)),
                (0x1d, Bool(drop.new)),
            ]))
        })
        .collect();
    (
        PROGRESSION,
        BOX_OPENED,
        Fields::from([(
            0x78,
            Struct(Fields::from([
                (0x00, Array(items)),
                (
                    0x18,
                    Struct(Fields::from([(0, Array(box_id.map(Unsigned).to_vec()))])),
                ),
                (0x28, Unsigned(0)),
            ])),
        )]),
    )
}

/// 24307: the credit balance.
pub fn credits_message(credits: i64) -> LobbyMessage {
    (
        PROGRESSION,
        CREDITS,
        Fields::from([(0x78, Signed(credits))]),
    )
}

#[cfg(test)]
mod tests;
