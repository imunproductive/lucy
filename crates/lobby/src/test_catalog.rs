//! A small catalog for unit tests: one hero with a skin of each rarity, a
//! Halloween skin, an Overwatch League home and away pair, a golden weapon and
//! a challenge reward, plus an account icon and spray.

use std::{fs, sync::Arc};

use serde_json::json;

use crate::{collection::Collection, templates::RetailTemplates};
use orpheus_content::catalog::{HERO_BASE, ItemDb, TEAM_SKIN_TEXT, UNLOCK_BASE};

pub const HERO: u64 = HERO_BASE | 0x40;
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
            "Rarity": "Legendary", "Categories": ["OWL"], "AvailableIn": TEAM_SKIN_TEXT,
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
        json!({"Wrecking Ball": {"Skins": [
            skin(COMMON, "Plain", "Common"),
            skin(RARE, "Blue", "Rare"),
            skin(EPIC, "Shiny", "Epic"),
            skin(LEGENDARY, "Grand", "Legendary"),
            skin(HALLOWEEN, "Pumpkin", "Legendary"),
            team(HOME, "Home 2019"),
            team(AWAY, "Away 2019"),
            {"GUID": "108.0A5", "Name": "GOLDEN", "Type": "WeaponSkin", "Rarity": "Legendary"},
            skin(REWARD, "Comic", "Epic"),
        ]}})
        .to_string(),
    )
    .unwrap();
    fs::write(
        data.path().join("extracted_general_unlocks.json"),
        json!({"Icons": [
            {"GUID": "201.0A5", "Name": "Ring", "Type": "Icon", "Rarity": "Common"},
            {"GUID": "202.0A5", "Name": "Tag", "Type": "Spray", "Rarity": "Common"},
            {"GUID": "4AEA.0A5", "Name": "Comic 1", "Type": "Spray", "Rarity": "Common"},
            {"GUID": "4AEB.0A5", "Name": "Comic 2", "Type": "Icon", "Rarity": "Common"},
            {"GUID": "4AEC.0A5", "Name": "Comic 3", "Type": "Spray", "Rarity": "Common"},
            {"GUID": "4F50.0A5", "Name": "Tiger", "Type": "Icon", "Rarity": "Common"},
        ]})
        .to_string(),
    )
    .unwrap();
    let items = ItemDb::load(data.path()).unwrap();
    let entry = |guid: u64, price: i64, event: i64| json!({"+0x0": guid, "+0x10": 0, "+0x14": price, "+0x16": event});
    let templates = json!({"format": 1, "server": [
        ["70E5A247", 24900, {
            "+0x80": [{"+0x0": [], "+0x30": HERO, "+0x38": 0, "+0x40": 0, "+0x48": 0,
                "+0x50": 0, "+0x58": [], "+0x70": [], "+0x88": []}],
            "+0x98": [{"+0x18": HERO, "+0x0": [
                entry(COMMON, 25, 0), entry(RARE, 75, 0), entry(EPIC, 250, 0),
                entry(LEGENDARY, 1000, 0), entry(HALLOWEEN, 3000, 2),
                entry(HOME, 100, -1), entry(AWAY, 100, -1), entry(GOLDEN, 3000, -1),
                entry(REWARD, 0, -1),
            ]}]
        }],
        ["779A8581", 24300, {
            "+0x78": {"+0x30": []},
            "+0xF8": {"+0x0": [entry(ICON, 0, 0), entry(SPRAY, 25, 0)]}
        }]
    ]});
    let templates = RetailTemplates::from_reader(templates.to_string().as_bytes()).unwrap();
    Collection::new(&templates, Arc::new(items)).unwrap()
}
