//! A small catalog for unit tests: one hero with a default skin, a skin of
//! each rarity, a Halloween skin, an Overwatch League home and away pair, a
//! golden weapon and a challenge reward, plus an account icon and spray; and
//! a few competitive seasons.

use std::{fs, sync::Arc};

use serde_json::json;

use crate::{
    clock::{DAY, stu_datetime},
    collection::Collection,
    economy::Economy,
    ranked::{CARD_BASE, Ranked},
    xp_table::XpTable,
};
use lucy_content::catalog::{HERO_BASE, ItemDb, TEAM_SKIN_TEXT, UNLOCK_BASE};

pub const HERO: u64 = HERO_BASE | 0x40;
/// The hero's default skin (level 0).
pub const DEFAULT: u64 = UNLOCK_BASE | 0x100;
pub const COMMON: u64 = UNLOCK_BASE | 0x101;
pub const RARE: u64 = UNLOCK_BASE | 0x102;
pub const EPIC: u64 = UNLOCK_BASE | 0x103;
pub const LEGENDARY: u64 = UNLOCK_BASE | 0x104;
/// A legendary skin of the Halloween pool (event 2).
pub const HALLOWEEN: u64 = UNLOCK_BASE | 0x105;
pub const HOME: u64 = UNLOCK_BASE | 0x106;
pub const AWAY: u64 = UNLOCK_BASE | 0x107;
pub const GOLDEN: u64 = UNLOCK_BASE | 0x108;
/// A challenge reward: no price, never in boxes.
pub const REWARD: u64 = UNLOCK_BASE | 0x109;
/// An account icon: never sold, but it drops.
pub const ICON: u64 = UNLOCK_BASE | 0x201;
pub const SPRAY: u64 = UNLOCK_BASE | 0x202;
// The general unlocks also hold Tracer's Comic Challenge rewards
// (4AEA-4AEC.0A5), so the challenge counts as known.
/// Lunar New Year's login icon.
pub const LUNAR_ICON: u64 = UNLOCK_BASE | 0x4f50;

pub fn collection() -> Collection {
    let data = tempfile::tempdir().unwrap();
    let skin = |guid: u64, name: &str, rarity: &str| json!({"GUID": format!("{:X}.0A5", guid & 0xffff), "Name": name, "Type": "Skin", "Rarity": rarity});
    let team = |guid: u64, name: &str| {
        json!({
            "GUID": format!("{:X}.0A5", guid & 0xffff), "Name": name, "Type": "Skin",
            "Rarity": "Epic", "Categories": ["OWL"], "AvailableIn": TEAM_SKIN_TEXT,
            "EsportsTeam": "Shock"
        })
    };
    fs::write(
        data.path().join("extracted_heroes.json"),
        json!({"40.075": {"Name": "Wrecking Ball", "Class": "Tank"}}).to_string(),
    )
    .unwrap();
    fs::write(
        data.path().join("extracted_items.json"),
        json!({"Wrecking Ball": {
            "LevelUnlocks": [{"Level": 0, "Unlocks": [skin(DEFAULT, "Classic", "Common")]}],
            "LootBoxesUnlocks": [
                {"LootBoxType": "Base", "Unlocks": [
                    skin(COMMON, "Plain", "Common"),
                    skin(RARE, "Blue", "Rare"),
                    skin(EPIC, "Shiny", "Epic"),
                    skin(LEGENDARY, "Grand", "Legendary"),
                ]},
                {"LootBoxType": "Halloween", "Unlocks": [skin(HALLOWEEN, "Pumpkin", "Legendary")]},
            ],
            "OtherUnlocks": [
                team(HOME, "Home 2019"),
                team(AWAY, "Away 2019"),
                {"GUID": "108.0A5", "Name": "GOLDEN", "Type": "WeaponSkin", "Rarity": "Legendary"},
                skin(REWARD, "Comic", "Epic"),
            ],
        }})
        .to_string(),
    )
    .unwrap();
    fs::write(
        data.path().join("extracted_general_unlocks.json"),
        json!({
            "LootBoxesUnlocks": [{"LootBoxType": "Base", "Unlocks": [
                {"GUID": "201.0A5", "Name": "Ring", "Type": "Icon", "Rarity": "Common"},
                {"GUID": "202.0A5", "Name": "Tag", "Type": "Spray", "Rarity": "Common"},
            ]}],
            "OtherUnlocks": [
                {"GUID": "4AEA.0A5", "Name": "Comic 1", "Type": "Spray", "Rarity": "Common"},
                {"GUID": "4AEB.0A5", "Name": "Comic 2", "Type": "Icon", "Rarity": "Common"},
                {"GUID": "4AEC.0A5", "Name": "Comic 3", "Type": "Spray", "Rarity": "Common"},
                {"GUID": "4F50.0A5", "Name": "Tiger", "Type": "Icon", "Rarity": "Common"},
            ],
        })
        .to_string(),
    )
    .unwrap();
    let items = ItemDb::load(data.path()).unwrap();
    Collection::new(Arc::new(items), economy().item_prices().clone())
}

/// Competitive seasons for unit tests: seasons 1 (card DD), 10 (E6), 18
/// (C2), 25 (121) and 32 (1B3, with its open queue card 1B4), the Lúcio
/// Cup (1AD) and Competitive CTF (1C3). Each lasts 60 days from 2026-01-01.
pub fn ranked() -> Ranked {
    const START: u64 = 1_767_225_600;
    let date = |seconds| stu_datetime(seconds).unwrap();
    let dates = [START, START + DAY, START + 60 * DAY, START + 60 * DAY].map(date);
    let card = |index: u64, parent: Option<u64>, children: &[u64]| {
        let key = |index: u64| format!("0x{:016X}", CARD_BASE | index);
        (
            key(index),
            json!({
                "tiers": [{"lowest": 1, "highest": 5000, "unlocks": ["0x0250000000002A61"]}],
                "dates": dates, "name": 0, "title": 0, "unlocks": [], "heroic_unlocks": [],
                "heroes": [], "maps_featured": [], "featured": [], "texture": 0, "image": 0,
                "maps": ["0x080000000000005B"], "parent": parent.map(key),
                "children": children.iter().map(|&child| key(child)).collect::<Vec<_>>(),
                "count_a": 25, "count_b": 5, "flag": true,
            }),
        )
    };
    let seasons: serde_json::Map<_, _> = [
        card(0xdd, None, &[]),
        card(0xe6, None, &[]),
        card(0xc2, None, &[]),
        card(0x121, None, &[]),
        card(0x1b3, None, &[0x1b4]),
        card(0x1b4, Some(0x1b3), &[]),
        card(0x1ad, None, &[]),
        card(0x1c3, None, &[]),
    ]
    .into_iter()
    .collect();
    Ranked::from_reader(serde_json::Value::Object(seasons).to_string().as_bytes()).unwrap()
}

/// The install's economy (`economy_174.json`) as build 104319 derives it,
/// cut to the events the tests run: Halloween and Lunar New Year.
pub fn economy() -> Arc<Economy> {
    let economy = json!({
        "duplicate_credits": {"Common": 5, "Rare": 15, "Epic": 50, "Legendary": 200},
        "credit_drops": {
            "Rare": {"unlock": "0x025000000000088B", "amount": 50},
            "Epic": {"unlock": "0x025000000000088C", "amount": 150},
            "Legendary": {"unlock": "0x025000000000088D", "amount": 500},
        },
        "celebration_boxes": {
            "0x0D8000000000212E": {"unlock": "0x0250000000000C23", "box_type": 2},
            "0x0D8000000000237C": {"unlock": "0x0250000000000D5A", "box_type": 4},
        },
        "item_prices": {
            "credits": {"Common": 25, "Rare": 75, "Epic": 250, "Legendary": 1000},
            "competitive_points": 3000,
            "league_tokens": {"Common": 10, "Rare": 25, "Epic": 100, "Legendary": 200},
            "event_credits": {},
            "event_priced": [],
        },
    });
    Arc::new(Economy::from_reader(economy.to_string().as_bytes()).unwrap())
}

/// The install's XP gains and level curve (`game_progression_174.json`) as
/// build 104319 derives them, cut to the fields the lobby reads.
pub fn xp_table() -> XpTable {
    let gain = |amount: f64| json!({ "amount": amount });
    let table = json!({
        "xp_gains": {
            "time": gain(1.75), "match_finish": gain(75.0), "win": gain(250.0),
            "gold_medal": gain(75.0), "silver_medal": gain(50.0), "bronze_medal": gain(25.0),
            "first_win_of_the_day": gain(1500.0), "backfill": gain(200.0),
            "consecutive_match": gain(150.0),
        },
        "level_experience": [
            0, 0, 2000, 3500, 5000, 6500, 7500, 8500, 9500, 10500, 11500, 12500, 13500, 14500,
            15000, 15500, 16000, 16500, 17000, 17500, 18000, 18500, 19000, 19500, 20000,
        ],
    });
    XpTable::from_reader(table.to_string().as_bytes()).unwrap()
}
