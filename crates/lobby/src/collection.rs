//! Lobby ownership, equipment, progression and the shop's store entries,
//! from the install's item catalog (`extracted_items.json`,
//! `extracted_general_unlocks.json`) and its prices (`economy_174.json`).
//!
//! What the catalog gives each item (24300 `+0xF8` and 24900 `+0x98` store
//! entries, `{unlock, 0, level, price, event, default}`):
//!
//! - An item of a level group (`LevelUnlocks`, `AdditionalUnlocks`) unlocks
//!   at that level; level 0 is a default item, owned from the start (`+0x17`
//!   set). Every other item has no level (65535).
//! - A loot box item belongs to its box's pool: `+0x16` is the box type
//!   (`Enum_BABC4175`, 0 the base box, 1-6 the events); any other item has
//!   -1. It costs its rarity's base credit price, an icon nothing (icons
//!   are not sold). Retail charged three times as much for some event items
//!   (each event's newest), which the install doesn't mark: every item here
//!   costs the base price.
//! - An Overwatch League item (category `OWL`) costs league tokens by
//!   rarity, a golden weapon the competitive point price; other items are
//!   not sold (price 0).
//!
//! These rules give the 7,040 entries of the 1.68 retail capture, but for
//! those event prices (160 items) and 64 Overwatch League skins retail did
//! not sell.
//!
//! Client research (1.74):
//!
//! - Portrait borders are 300 consecutive GUIDs: 5 tiers of 600 levels, each
//!   with 6 star counts (100 levels each) of 10 border steps (10 levels each).
//!   A made-up GUID shows no border at all.
//! - An item tagged with a hero (icons and sprays included) must be listed
//!   only under that hero: the client unlocks a hero's icon only from the
//!   hero's list, and when the account list has it too, it does not send the
//!   equip request at all (tested in game). The hero gallery shows only items
//!   from the hero's store list, and an item counts as owned only when it
//!   belongs to a hero; a default-item store entry (+0x10 0, +0x17 true)
//!   would leave it out of the hero's count.
//! - 24302 `{boxes, reason, flag}` merges boxes into the client's list,
//!   skipping ids it has (0x7FF789D32D00); retail's login sent reason 12 with
//!   the flag on. Reason 4 with the flag would store the first box as a shop
//!   purchase (0x7FF789726DE0).
//! - 24901 `{hero, unlock, new}` (a hero's item) and 24301 `{unlock, seen}`
//!   (an account item) both open the challenge reward window when the unlock
//!   is a challenge reward (0x7FF7896F8E30).
//! - 24306 `{unlock, hero}`: the unlock must go first; with the hero first
//!   the client charged the player and left the item locked.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
    sync::Arc,
};

use indexmap::IndexMap;
use serde_json::{Map, Value as Json};

use crate::{
    accounts::{LootBox, Profile},
    economy::{Economy, ItemPrices},
    transport::LobbyMessage,
};
use lucy_content::catalog::{ItemDb, Unlock};
use lucy_core::guid::parse_guid;
use lucy_wire::{
    entity::SkinTheme,
    jam::codec::{
        Fields, MAX_ARRAY_COUNT,
        Value::{self, Array, Bool, Signed, Struct, Unsigned},
    },
};

pub const HERO_CATALOG: u32 = 0x70e5_a247;
pub const PROGRESSION: u32 = 0x779a_8581;
const LOADOUT_SLOTS: [(u32, &str); 7] = [
    (0x38, "Skin"),
    (0x40, "HighlightIntro"),
    (0x48, "VictoryPose"),
    (0x50, "WeaponSkin"),
    (0x58, "Spray"),
    (0x70, "VoiceLine"),
    (0x88, "Emote"),
];
/// The loadout slots that hold a list (sprays, voice lines, emotes).
const LIST_SLOTS: [u32; 3] = [0x58, 0x70, 0x88];
const BORDER_BASE: u64 = 0x0250_0000_0000_0918;
const NO_UNLOCK_LEVEL: i64 = 65_535;
const BOX_SYNC_REASON: i64 = 12;
/// `Enum_BABC4175`, the loot box an item comes from, by the name the item
/// catalog gives it (`lucy_assets` `lists/unlocks.rs`).
const LOOT_BOXES: [(&str, i64); 8] = [
    ("Base", 0),
    ("SummerGames", 1),
    ("Halloween", 2),
    ("Winter", 3),
    ("LunarNewYear", 4),
    ("Archives", 5),
    ("Anniversary", 6),
    ("Internal", 8),
];
/// The progression message's `+0x110`: the golden weapon price in
/// competitive points, and `+0x114`, the most competitive points a player
/// keeps (`ranked::MAX_COMP_POINTS`) [INFERENCE: retail sent 3000 and 6000].
const PROGRESSION_POINT_LIMIT: u32 = 0x114;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreEntry {
    pub unlock_level: i64,
    pub price: i64,
    pub event: i64,
    /// A default item, owned from the start.
    pub default: bool,
}

impl StoreEntry {
    /// The entry the catalog's rules give an item (module documentation).
    fn of(unlock: &Unlock, prices: &ItemPrices) -> Self {
        let entry = |unlock_level, price, event| Self {
            unlock_level,
            price,
            event,
            default: false,
        };
        let rarity = |table: &HashMap<String, i64>| table.get(&unlock.rarity).copied().unwrap_or(0);
        match unlock.level {
            Some(0) => Self {
                default: true,
                ..entry(0, 0, -1)
            },
            Some(level) => entry(level, 0, -1),
            None => match unlock.box_type.as_deref() {
                Some(name) => {
                    let event = LOOT_BOXES
                        .iter()
                        .find(|(box_name, _)| *box_name == name)
                        .map_or(-1, |&(_, event)| event);
                    let price = if unlock.item_type == "Icon" {
                        0
                    } else {
                        rarity(&prices.credits)
                    };
                    entry(NO_UNLOCK_LEVEL, price, event)
                }
                None if unlock.categories.iter().any(|category| category == "OWL") => {
                    entry(NO_UNLOCK_LEVEL, rarity(&prices.league_tokens), -1)
                }
                None if unlock.item_type == "WeaponSkin" => {
                    entry(NO_UNLOCK_LEVEL, prices.competitive_points, -1)
                }
                None => entry(NO_UNLOCK_LEVEL, 0, -1),
            },
        }
    }

    fn wire(self, guid: u64) -> Value {
        Struct(Fields::from([
            (0x00, Unsigned(guid)),
            (0x08, Unsigned(0)),
            (0x10, Signed(self.unlock_level)),
            (0x14, Signed(self.price)),
            (0x16, Signed(self.event)),
            (0x17, Bool(self.default)),
        ]))
    }
}

struct HeroCollection {
    defaults: Vec<u64>,
    loadout: Fields,
}

pub struct Collection {
    items: Arc<ItemDb>,
    prices: ItemPrices,
    hero_order: Vec<u64>,
    heroes: HashMap<u64, HeroCollection>,
    hero_of: IndexMap<u64, u64>,
    hero_items: HashMap<u64, Vec<u64>>,
    default_account: Vec<u64>,
    all_account: Vec<u64>,
    store: IndexMap<u64, StoreEntry>,
    borders: Vec<(i64, u64)>,
}

impl Collection {
    /// The catalog and prices of a derived data folder.
    pub fn load(data_dir: &Path) -> io::Result<Self> {
        let economy = Economy::load(data_dir)?;
        Ok(Self::new(
            Arc::new(ItemDb::load(data_dir)?),
            economy.item_prices().clone(),
        ))
    }

    /// The heroes are those of the catalog with default items (the playable
    /// ones), in its order; every item of a hero, and every account item
    /// (no hero), has a store entry.
    pub fn new(items: Arc<ItemDb>, prices: ItemPrices) -> Self {
        let mut hero_of = IndexMap::new();
        let mut store = IndexMap::new();
        let mut default_account = Vec::new();
        let mut all_account = Vec::new();
        let mut borders = Vec::new();
        let mut hero_items = HashMap::<u64, Vec<u64>>::new();
        let mut defaults = HashMap::<u64, Vec<u64>>::new();
        for unlock in items.unlocks.values() {
            let entry = StoreEntry::of(unlock, &prices);
            match unlock.hero.as_deref() {
                Some(name) => {
                    let Some(hero) = items.hero_by_name(name) else {
                        continue;
                    };
                    hero_of.insert(unlock.guid, hero);
                    hero_items.entry(hero).or_default().push(unlock.guid);
                    if entry.default {
                        defaults.entry(hero).or_default().push(unlock.guid);
                    }
                }
                None => {
                    all_account.push(unlock.guid);
                    if entry.default {
                        default_account.push(unlock.guid);
                    }
                    if (0..NO_UNLOCK_LEVEL).contains(&entry.unlock_level)
                        && unlock.item_type == "PortraitFrame"
                    {
                        borders.push((entry.unlock_level, unlock.guid));
                    }
                }
            }
            store.insert(unlock.guid, entry);
        }
        borders.sort_unstable();
        let default_spray = default_account
            .iter()
            .find(|&&guid| {
                items
                    .get(guid)
                    .is_some_and(|item| item.item_type == "Spray")
            })
            .copied();
        let mut hero_order = Vec::new();
        let mut heroes = HashMap::new();
        for &hero in items.hero_names.keys() {
            let Some(mut owned) = defaults.remove(&hero) else {
                continue;
            };
            let first = |kind: &str| {
                owned
                    .iter()
                    .copied()
                    .find(|&guid| items.get(guid).is_some_and(|item| item.item_type == kind))
            };
            let loadout = LOADOUT_SLOTS
                .iter()
                .map(|&(slot, kind)| {
                    let item = if kind == "Spray" {
                        default_spray
                    } else {
                        first(kind)
                    };
                    let value = if LIST_SLOTS.contains(&slot) {
                        Array(item.into_iter().map(Unsigned).collect())
                    } else {
                        Unsigned(item.unwrap_or(0))
                    };
                    (slot, value)
                })
                .collect();
            owned.sort_unstable();
            hero_order.push(hero);
            heroes.insert(
                hero,
                HeroCollection {
                    defaults: owned,
                    loadout,
                },
            );
        }
        // Only the playable heroes' items are listed; another character's
        // (a bot's, a PvE enemy's) are not sold nor owned.
        hero_items.retain(|hero, _| heroes.contains_key(hero));
        hero_of.retain(|_, hero| heroes.contains_key(hero));
        store.retain(|guid, _| {
            items
                .get(*guid)
                .is_some_and(|item| item.hero.is_none() || hero_of.contains_key(guid))
        });
        all_account.retain(|&guid| {
            items.get(guid).is_some_and(|item| {
                matches!(item.item_type.as_str(), "Icon" | "Spray" | "PortraitFrame")
            })
        });
        Self {
            items,
            prices,
            hero_order,
            heroes,
            hero_of,
            hero_items,
            default_account,
            all_account,
            store,
            borders,
        }
    }

    pub fn heroes(&self) -> &[u64] {
        &self.hero_order
    }

    pub fn items(&self) -> &ItemDb {
        &self.items
    }

    pub fn hero_for(&self, guid: u64) -> Option<u64> {
        self.hero_of.get(&guid).copied()
    }

    pub fn store(&self) -> &IndexMap<u64, StoreEntry> {
        &self.store
    }

    pub fn border_levels(&self) -> &[(i64, u64)] {
        &self.borders
    }

    pub fn portrait_frame(&self, profile: &Profile) -> u64 {
        if profile.frame_guid != 0 {
            return profile.frame_guid;
        }
        let earned = self
            .borders
            .partition_point(|&(level, _)| level <= profile.level);
        if earned == 0 {
            let levels_done = profile.level.max(1) - 1;
            let tier = (levels_done / 600).min(4) as u64;
            let stars = (levels_done % 600 / 100) as u64;
            let step = (levels_done % 100 / 10) as u64;
            return BORDER_BASE + tier * 60 + stars * 10 + step;
        }
        self.borders[earned - 1].1
    }

    pub fn owned_for_hero(&self, profile: &Profile, hero: u64) -> Vec<u64> {
        self.owned_hero(hero, profile.unlock_all, &sorted_unlocks(profile))
    }

    fn owned_hero(&self, hero: u64, unlock_all: bool, unlocked: &[u64]) -> Vec<u64> {
        let defaults = self
            .heroes
            .get(&hero)
            .map_or(&[][..], |collection| collection.defaults.as_slice());
        if unlock_all {
            let offered = self.hero_items.get(&hero).into_iter().flatten().copied();
            return append_new(defaults, offered);
        }
        let extra = unlocked
            .iter()
            .copied()
            .filter(|&guid| self.hero_for(guid) == Some(hero));
        append_new(defaults, extra)
    }

    pub fn owned_account(&self, profile: &Profile) -> Vec<u64> {
        if profile.unlock_all {
            return append_new(&self.default_account, self.all_account.iter().copied());
        }
        let extra = sorted_unlocks(profile)
            .into_iter()
            .filter(|&guid| self.hero_for(guid).is_none());
        append_new(&self.default_account, extra)
    }

    pub fn owns(&self, profile: &Profile, guid: u64) -> bool {
        self.hero_for(guid)
            .is_some_and(|hero| self.owned_for_hero(profile, hero).contains(&guid))
            || self.owned_account(profile).contains(&guid)
    }

    pub fn owned_set(&self, profile: &Profile) -> HashSet<u64> {
        let mut owned: HashSet<_> = self.owned_account(profile).into_iter().collect();
        let unlocked = sorted_unlocks(profile);
        for &hero in self.hero_items.keys() {
            owned.extend(self.owned_hero(hero, profile.unlock_all, &unlocked));
        }
        owned
    }

    pub fn loadout(&self, profile: &Profile, hero: u64) -> io::Result<Fields> {
        let mut slots = self
            .heroes
            .get(&hero)
            .map_or_else(Fields::new, |collection| collection.loadout.clone());
        if let Some(saved) = profile
            .loadouts
            .get(&guid_text(hero))
            .and_then(Json::as_object)
        {
            for (slot, value) in &mut slots {
                if let Some(change) = saved.get(&slot_key(*slot)) {
                    *value = saved_loadout(change)?;
                }
            }
        }
        Ok(slots)
    }

    pub fn equipped_skin(&self, profile: &Profile, hero: u64) -> io::Result<SkinTheme> {
        let loadout = self.loadout(profile, hero)?;
        let item = |slot| {
            let guid = unsigned(loadout.get(&slot)?).ok()?;
            self.items.get(guid)
        };
        Ok(SkinTheme {
            guid: item(0x38).map_or(0, |skin| skin.skin_theme),
            golden: item(0x50).is_some_and(|weapon| weapon.name == "GOLDEN"),
        })
    }

    pub fn equip(
        &self,
        profile: &mut Profile,
        hero: u64,
        guid: u64,
        position: i32,
    ) -> io::Result<bool> {
        let Some(item) = self.items.get(guid) else {
            return Ok(false);
        };
        if item.item_type == "Icon" {
            profile.icon_guid = guid;
            return Ok(true);
        }
        if hero == 0 {
            return Ok(false);
        }
        let Some(&(slot, _)) = LOADOUT_SLOTS
            .iter()
            .find(|(_, kind)| *kind == item.item_type)
        else {
            return Ok(false);
        };
        let value = if LIST_SLOTS.contains(&slot) {
            let position = position.max(0) as usize;
            if position >= MAX_ARRAY_COUNT {
                return Ok(false);
            }
            let loadout = self.loadout(profile, hero)?;
            let mut values = array(&loadout, slot)?
                .iter()
                .map(|value| Ok(Json::String(guid_text(unsigned(value)?))))
                .collect::<io::Result<Vec<_>>>()?;
            let item = Json::String(guid_text(guid));
            if position >= values.len() {
                let filler = values.last().cloned().unwrap_or_else(|| item.clone());
                values.resize(position + 1, filler);
            }
            values[position] = item;
            Json::Array(values)
        } else {
            Json::String(guid_text(guid))
        };
        let saved = profile
            .loadouts
            .entry(guid_text(hero))
            .or_insert_with(|| Json::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| invalid("saved hero loadout is not an object"))?;
        saved.insert(slot_key(slot), value);
        Ok(true)
    }

    /// 24300: the account record (boxes, owned account items, border, icon,
    /// level and balances), the account items' store entries, the golden
    /// weapon price and the competitive point limit.
    pub fn progression(&self, profile: &Profile) -> io::Result<Fields> {
        let record = Fields::from([
            (0x00, box_list(&profile.loot_boxes)),
            (0x18, Array(Vec::new())),
            (0x30, ownership(self.owned_account(profile))),
            (0x48, Unsigned(0)),
            (0x50, Unsigned(2000)),
            (0x58, Unsigned(self.portrait_frame(profile))),
            (0x60, Unsigned(profile.icon_guid)),
            (0x68, Signed(profile.level)),
            (0x6c, Signed(profile.credits)),
            (0x70, Signed(profile.comp_points)),
            (0x74, Signed(profile.league_tokens)),
            (0x78, Bool(true)),
            (0x79, Bool(true)),
        ]);
        let entries = self
            .store
            .iter()
            .filter(|(guid, _)| self.hero_for(**guid).is_none())
            .map(|(&guid, entry)| entry.wire(guid))
            .collect();
        Ok(Fields::from([
            (0x78, Struct(record)),
            (0xf8, Struct(Fields::from([(0, Array(entries))]))),
            (0x110, Signed(self.prices.competitive_points)),
            (
                PROGRESSION_POINT_LIMIT,
                Signed(crate::ranked::MAX_COMP_POINTS),
            ),
            (0x118, Array(Vec::new())),
        ]))
    }

    /// 24900: each hero's owned items and loadout (`+0x80`) and store
    /// entries (`+0x98`).
    pub fn hero_catalog(&self, profile: &Profile) -> io::Result<Fields> {
        let unlocked = sorted_unlocks(profile);
        let mut records = Vec::with_capacity(self.hero_order.len());
        let mut stores = Vec::with_capacity(self.hero_order.len());
        for &hero in &self.hero_order {
            let mut record = Fields::from([
                (
                    0,
                    ownership(self.owned_hero(hero, profile.unlock_all, &unlocked)),
                ),
                (0x18, Array(Vec::new())),
                (0x30, Unsigned(hero)),
            ]);
            record.extend(self.loadout(profile, hero)?);
            records.push(Struct(record));
            let entries = self
                .hero_items
                .get(&hero)
                .into_iter()
                .flatten()
                .map(|&guid| self.store[&guid].wire(guid))
                .collect();
            stores.push(Struct(Fields::from([
                (0, Array(entries)),
                (0x18, Unsigned(hero)),
            ])));
        }
        Ok(Fields::from([
            (0x78, Unsigned(1)),
            (0x80, Array(records)),
            (0x98, Array(stores)),
            (0xb0, Array(Vec::new())),
        ]))
    }

    pub fn unlock_granted(&self, guid: u64) -> LobbyMessage {
        match self.hero_for(guid) {
            Some(hero) if hero != 0 => (
                HERO_CATALOG,
                24901,
                Fields::from([
                    (0x78, Unsigned(hero)),
                    (0x80, Unsigned(guid)),
                    (0x88, Bool(true)),
                ]),
            ),
            _ => (
                PROGRESSION,
                24301,
                Fields::from([(0x78, Unsigned(guid)), (0x80, Bool(false))]),
            ),
        }
    }

    pub fn boxes_update(boxes: &[LootBox]) -> LobbyMessage {
        (
            PROGRESSION,
            24302,
            Fields::from([
                (0x78, box_list(boxes)),
                (0x90, Signed(BOX_SYNC_REASON)),
                (0x94, Bool(true)),
            ]),
        )
    }

    pub fn unlock_bought(&self, guid: u64) -> LobbyMessage {
        (
            PROGRESSION,
            24306,
            Fields::from([
                (0x78, Unsigned(guid)),
                (0x80, Unsigned(self.hero_for(guid).unwrap_or_default())),
            ]),
        )
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn missing(offset: u32) -> io::Error {
    invalid(format!("missing collection field +0x{offset:X}"))
}

fn field(fields: &Fields, offset: u32) -> io::Result<&Value> {
    fields.get(&offset).ok_or_else(|| missing(offset))
}

fn array(fields: &Fields, offset: u32) -> io::Result<&[Value]> {
    match field(fields, offset)? {
        Array(values) => Ok(values),
        _ => Err(invalid(format!(
            "expected collection array at +0x{offset:X}"
        ))),
    }
}

fn unsigned(value: &Value) -> io::Result<u64> {
    match value {
        Unsigned(value) => Ok(*value),
        Signed(value) => u64::try_from(*value).map_err(|_| invalid("negative collection GUID")),
        _ => Err(invalid("expected collection GUID")),
    }
}

fn guid_text(guid: u64) -> String {
    format!("0x{guid:016X}")
}

fn slot_key(slot: u32) -> String {
    format!("+0x{slot:X}")
}

fn saved_loadout(value: &Json) -> io::Result<Value> {
    fn saved_guid(value: &Json) -> io::Result<Value> {
        value
            .as_str()
            .and_then(parse_guid)
            .map(Unsigned)
            .ok_or_else(|| invalid("expected saved loadout GUID text"))
    }
    match value {
        Json::Array(values) => values
            .iter()
            .map(saved_guid)
            .collect::<io::Result<_>>()
            .map(Array),
        value => saved_guid(value),
    }
}

fn sorted_unlocks(profile: &Profile) -> Vec<u64> {
    if profile.unlock_all {
        return Vec::new();
    }
    let mut unlocked: Vec<_> = profile.unlocked_guids().into_iter().collect();
    unlocked.sort_unstable();
    unlocked
}

fn append_new(defaults: &[u64], extra: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut seen: HashSet<_> = defaults.iter().copied().collect();
    let mut owned = defaults.to_vec();
    owned.extend(extra.into_iter().filter(|&guid| seen.insert(guid)));
    owned
}

fn ownership(guids: Vec<u64>) -> Value {
    Array(
        guids
            .into_iter()
            .map(|guid| Struct(Fields::from([(0, Unsigned(guid)), (8, Bool(true))])))
            .collect(),
    )
}

fn box_list(boxes: &[LootBox]) -> Value {
    Array(
        boxes
            .iter()
            .map(|loot_box| {
                Struct(Fields::from([
                    (
                        0,
                        Struct(Fields::from([(
                            0,
                            Array(vec![Unsigned(loot_box.id), Unsigned(0)]),
                        )])),
                    ),
                    (0x10, Signed(i64::from(loot_box.box_type))),
                    (0x14, Unsigned(1)),
                ]))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use lucy_content::catalog::UNLOCK_BASE;
    use lucy_wire::jam::codec::Schemas;

    const HERO: u64 = 0x10;
    const ACCOUNT_SPRAY: u64 = 0x11;
    const DEFAULT_SKIN: u64 = 0x20;
    const NEW_SKIN: u64 = 0x21;
    const HERO_SPRAY: u64 = 0x22;
    const ACCOUNT_ICON: u64 = 0x30;
    const LOW_BORDER: u64 = 0x31;
    const HIGH_BORDER: u64 = 0x32;

    fn unlock(guid: u64, item_type: &str, hero: bool, level: Option<i64>) -> Unlock {
        Unlock {
            guid,
            name: guid_text(guid),
            item_type: item_type.into(),
            rarity: "Common".into(),
            hero: hero.then(|| "Local Hero".into()),
            source: String::new(),
            box_type: None,
            categories: Vec::new(),
            available_in: String::new(),
            skin_theme: 0,
            esports_team: String::new(),
            level,
        }
    }

    /// One hero with a default skin, an unsold skin and spray; account
    /// defaults (an icon and a spray) and two borders.
    fn synthetic_collection() -> Collection {
        let mut items = ItemDb::default();
        items.hero_names.insert(HERO, "Local Hero".into());
        items.hero_names.insert(0x40, "A Bot".into());
        for unlock in [
            unlock(DEFAULT_SKIN, "Skin", true, Some(0)),
            unlock(NEW_SKIN, "Skin", true, None),
            unlock(HERO_SPRAY, "Spray", true, None),
            unlock(ACCOUNT_ICON, "Icon", false, Some(0)),
            unlock(ACCOUNT_SPRAY, "Spray", false, Some(0)),
            unlock(HIGH_BORDER, "PortraitFrame", false, Some(200)),
            unlock(LOW_BORDER, "PortraitFrame", false, Some(50)),
            Unlock {
                hero: Some("A Bot".into()),
                ..unlock(0x41, "Skin", false, None)
            },
        ] {
            items.unlocks.insert(unlock.guid, unlock);
        }
        Collection::new(Arc::new(items), ItemPrices::default())
    }

    #[test]
    fn store_entries_follow_the_catalogs_rules() {
        let prices = ItemPrices {
            credits: [("Rare", 75), ("Legendary", 1000)]
                .map(|(rarity, price)| (rarity.into(), price))
                .into(),
            competitive_points: 3000,
            league_tokens: [("Epic".into(), 100)].into(),
        };
        let item = |item_type: &str, rarity: &str, box_type: Option<&str>| Unlock {
            rarity: rarity.into(),
            box_type: box_type.map(Into::into),
            ..unlock(1, item_type, true, None)
        };
        let entry = |unlock: &Unlock| {
            let entry = StoreEntry::of(unlock, &prices);
            (entry.unlock_level, entry.price, entry.event, entry.default)
        };
        assert_eq!(entry(&unlock(1, "Skin", true, Some(0))), (0, 0, -1, true));
        assert_eq!(
            entry(&unlock(1, "PortraitFrame", false, Some(50))),
            (50, 0, -1, false)
        );
        // Every box sells at the base credit price, an event box too; an
        // icon is not sold.
        assert_eq!(
            entry(&item("Skin", "Legendary", Some("Base"))),
            (65_535, 1000, 0, false)
        );
        assert_eq!(
            entry(&item("Skin", "Legendary", Some("Halloween"))),
            (65_535, 1000, 2, false)
        );
        assert_eq!(
            entry(&item("VictoryPose", "Rare", Some("Anniversary"))),
            (65_535, 75, 6, false)
        );
        assert_eq!(
            entry(&item("Icon", "Rare", Some("Winter"))),
            (65_535, 0, 3, false)
        );
        let league = Unlock {
            categories: vec!["OWL".into()],
            ..item("Skin", "Epic", None)
        };
        assert_eq!(entry(&league), (65_535, 100, -1, false));
        assert_eq!(
            entry(&item("WeaponSkin", "Legendary", None)),
            (65_535, 3000, -1, false)
        );
        assert_eq!(
            entry(&item("Spray", "Common", None)),
            (65_535, 0, -1, false)
        );
    }

    #[test]
    fn equipped_skin_and_golden_weapon_come_from_the_same_loadout() {
        let mut collection = synthetic_collection();
        let items = Arc::get_mut(&mut collection.items).unwrap();
        let skin = items.unlocks.get_mut(&NEW_SKIN).unwrap();
        skin.skin_theme = 0x0a40_0000_0000_0210;
        let mut golden = skin.clone();
        golden.guid = 0x23;
        golden.name = "GOLDEN".into();
        golden.item_type = "WeaponSkin".into();
        golden.skin_theme = 0;
        items.unlocks.insert(golden.guid, golden);
        let mut profile = Profile::default();
        assert_eq!(
            collection.equipped_skin(&profile, HERO).unwrap(),
            SkinTheme::default()
        );
        assert!(collection.equip(&mut profile, HERO, NEW_SKIN, 0).unwrap());
        assert!(collection.equip(&mut profile, HERO, 0x23, 0).unwrap());
        assert_eq!(
            collection.equipped_skin(&profile, HERO).unwrap(),
            SkinTheme {
                guid: 0x0a40_0000_0000_0210,
                golden: true
            },
        );
        assert_eq!(
            collection.equipped_skin(&profile, 0).unwrap(),
            SkinTheme::default()
        );
    }

    #[test]
    fn new_hero_items_keep_catalog_order_and_stay_out_of_account_ownership() {
        let collection = synthetic_collection();
        let mut profile = Profile {
            unlocked_items: vec![json!("0x22"), json!("0x21"), json!("0x33")],
            ..Profile::default()
        };
        assert_eq!(collection.heroes(), [HERO]);
        assert_eq!(
            collection.owned_for_hero(&profile, HERO),
            [DEFAULT_SKIN, NEW_SKIN, HERO_SPRAY]
        );
        assert_eq!(
            collection.owned_account(&profile),
            [ACCOUNT_ICON, ACCOUNT_SPRAY, 0x33]
        );
        assert_eq!(collection.store()[&NEW_SKIN].event, -1);
        // A character that isn't playable has no collection.
        assert!(collection.hero_for(0x41).is_none());
        assert!(!collection.store().contains_key(&0x41));
        assert!(collection.owns(&profile, HERO_SPRAY));
        assert!(collection.owned_set(&profile).contains(&HERO_SPRAY));

        let (group, id, granted) = collection.unlock_granted(HERO_SPRAY);
        assert_eq!((group, id), (HERO_CATALOG, 24901));
        assert_eq!(granted[&0x78], Unsigned(HERO));
        let (group, id, bought) = collection.unlock_bought(NEW_SKIN);
        assert_eq!((group, id), (PROGRESSION, 24306));
        assert_eq!(bought[&0x78], Unsigned(NEW_SKIN));
        assert_eq!(bought[&0x80], Unsigned(HERO));

        profile.unlock_all = true;
        assert_eq!(
            collection.owned_account(&profile),
            [ACCOUNT_ICON, ACCOUNT_SPRAY, HIGH_BORDER, LOW_BORDER]
        );
        assert_eq!(
            collection.owned_for_hero(&profile, HERO),
            [DEFAULT_SKIN, NEW_SKIN, HERO_SPRAY]
        );
    }

    #[test]
    fn borders_follow_the_level_catalog_or_a_player_override() {
        let collection = synthetic_collection();
        let mut profile = Profile {
            level: -1,
            ..Profile::default()
        };
        assert_eq!(collection.portrait_frame(&profile), BORDER_BASE);
        assert_eq!(
            collection.border_levels(),
            [(50, LOW_BORDER), (200, HIGH_BORDER)]
        );
        profile.level = 50;
        assert_eq!(collection.portrait_frame(&profile), LOW_BORDER);
        profile.level = i64::MAX;
        assert_eq!(collection.portrait_frame(&profile), HIGH_BORDER);
        profile.frame_guid = LOW_BORDER;
        assert_eq!(collection.portrait_frame(&profile), LOW_BORDER);
    }

    #[test]
    fn equips_account_icons_and_retains_profile_loadout_text() {
        let collection = synthetic_collection();
        let mut profile = Profile::default();
        assert!(
            collection
                .equip(&mut profile, HERO, ACCOUNT_ICON, 0)
                .unwrap()
        );
        assert_eq!(profile.icon_guid, ACCOUNT_ICON);
        assert!(!collection.equip(&mut profile, 0, NEW_SKIN, 0).unwrap());
        assert!(collection.equip(&mut profile, HERO, NEW_SKIN, 0).unwrap());
        assert!(collection.equip(&mut profile, HERO, HERO_SPRAY, 3).unwrap());
        assert!(
            !collection
                .equip(&mut profile, HERO, HERO_SPRAY, i32::MAX)
                .unwrap()
        );
        assert_eq!(
            collection.loadout(&profile, HERO).unwrap()[&0x38],
            Unsigned(NEW_SKIN)
        );
        // The default spray is the account's; the gap is filled with it.
        assert_eq!(
            collection.loadout(&profile, HERO).unwrap()[&0x58],
            Array(vec![
                Unsigned(ACCOUNT_SPRAY),
                Unsigned(ACCOUNT_SPRAY),
                Unsigned(ACCOUNT_SPRAY),
                Unsigned(HERO_SPRAY)
            ])
        );
        assert_eq!(
            profile.loadouts[&guid_text(HERO)]["+0x58"],
            json!([
                guid_text(ACCOUNT_SPRAY),
                guid_text(ACCOUNT_SPRAY),
                guid_text(ACCOUNT_SPRAY),
                guid_text(HERO_SPRAY)
            ])
        );
    }

    /// The install's catalog (build 104319): 32 playable heroes, 1,119
    /// account items and 6,003 hero items, each with an entry; 300 borders
    /// (level 0, then every 10 levels). The expected values follow from the
    /// rules in the module documentation over `extracted_items.json`.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_installs_catalog_gives_defaults_loadouts_and_prices() {
        let data = lucy_testkit::runtime_dir().join("data");
        let schemas = Schemas::lobby();
        let collection = Collection::load(&data).unwrap();
        let reaper = collection.items().hero_by_name("Reaper").unwrap();
        assert_eq!(collection.heroes().len(), 32);
        assert_eq!(collection.store().len(), 7122);
        assert_eq!(collection.border_levels().len(), 300);
        assert_eq!(collection.border_levels()[0], (0, BORDER_BASE));

        let unlock = |index: u64| UNLOCK_BASE | index;
        // Reaper's level 0 items, by GUID, and the first of each kind as
        // his loadout; the spray is the account's first default spray.
        assert_eq!(
            collection.owned_for_hero(&Profile::default(), reaper),
            [0x21b, 0x22e, 0x3b4, 0x3f6, 0x884, 0x8d4, 0x901, 0x289e].map(unlock)
        );
        let loadout = collection.loadout(&Profile::default(), reaper).unwrap();
        assert_eq!(
            loadout,
            Fields::from([
                (0x38, Unsigned(unlock(0x3f6))),
                (0x40, Unsigned(unlock(0x21b))),
                (0x48, Unsigned(unlock(0x884))),
                (0x50, Unsigned(unlock(0x3b4))),
                (0x58, Array(vec![Unsigned(unlock(0x8d))])),
                (0x70, Array(vec![Unsigned(unlock(0x8d4))])),
                (0x88, Array(vec![Unsigned(unlock(0x901))])),
            ])
        );
        let price = |index| {
            let entry = collection.store()[&unlock(index)];
            (entry.price, entry.event)
        };
        assert_eq!(price(0x3fb), (250, 0), "Reaper's Desert, an epic skin");
        assert_eq!(price(0xa86), (1000, 2), "Pumpkin, a Halloween legendary");
        assert_eq!(price(0x3c9), (3000, -1), "Reaper's golden weapon");
        assert_eq!(price(0x13c3), (100, -1), "an Overwatch League skin");
        assert_eq!(price(0x9d9), (0, 0), "a base box icon");

        let aurisa = unlock(0x1149); // Orisa's icon
        let pachimari = unlock(0x08e9); // an account icon
        let luchador = unlock(0x4f53); // a Reaper skin
        let mut equipped = Profile {
            unlocked_items: [aurisa, pachimari, luchador]
                .map(|guid| Json::String(guid_text(guid)))
                .to_vec(),
            level: 247,
            credits: 347,
            ..Profile::default()
        };
        equipped.add_boxes(2, 2).unwrap();
        assert!(
            collection
                .equip(&mut equipped, reaper, luchador, 0)
                .unwrap()
        );
        // Owned: the 59 account defaults and each hero's (317 in all), plus
        // what the profile unlocked; everything with unlock_all. The border
        // is the last one earned (level 100: 0x921, 247: 0x93B).
        for (profile, expected_frame, expected_owned) in [
            (Profile::default(), unlock(0x921), (59, 8, 317)),
            (equipped, unlock(0x93b), (60, 9, 320)),
            (
                Profile {
                    unlock_all: true,
                    level: 3000, // The highest level the dashboard accepts.
                    ..Profile::default()
                },
                unlock(0xf97),
                (1119, 196, 7122),
            ),
        ] {
            assert_eq!(collection.portrait_frame(&profile), expected_frame);
            assert_eq!(
                (
                    collection.owned_account(&profile).len(),
                    collection.owned_for_hero(&profile, reaper).len(),
                    collection.owned_set(&profile).len(),
                ),
                expected_owned
            );
            for (crc, id, fields) in [
                (
                    PROGRESSION,
                    24300,
                    collection.progression(&profile).unwrap(),
                ),
                (
                    HERO_CATALOG,
                    24900,
                    collection.hero_catalog(&profile).unwrap(),
                ),
                collection.unlock_granted(aurisa),
                collection.unlock_granted(pachimari),
                collection.unlock_bought(luchador),
                Collection::boxes_update(&profile.loot_boxes),
            ] {
                schemas.encode(crc, id, &fields).unwrap();
            }
        }
    }
}
