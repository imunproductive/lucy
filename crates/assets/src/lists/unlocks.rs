//! `list-unlocks` (`ListHeroUnlocks`): each hero's unlocks by hero name, and
//! `list-general-unlocks` (`ListGeneralUnlocks`): the player progression's
//! unlocks (`DataModels/ProgressionUnlocks.cs`, `PlayerProgression.cs`,
//! `Unlock.cs`, `TeamDefinition.cs`).

use lucy_stu::{Object, Value};
use serde_json::{Map, Value as Json, json};

use super::{Data, array, enum_json, guid, guid_json, guid_label, guids, inline, instance, int};

const HERO: u16 = 0x075;
const GENERIC_SETTINGS: u16 = 0x054;

/// `STUUnlockRarity`.
const RARITIES: &[(i64, &str)] = &[
    (-1, "None"),
    (0, "Common"),
    (1, "Rare"),
    (2, "Epic"),
    (3, "Legendary"),
    (4, "xBD3BA67B"),
    (5, "xEF68A54A"),
];
/// `Enum_BABC4175`, the loot box an unlock comes from.
const LOOT_BOXES: &[(i64, &str)] = &[
    (-1, "xC1854306"),
    (0, "Base"),
    (1, "SummerGames"),
    (2, "Halloween"),
    (3, "Winter"),
    (4, "LunarNewYear"),
    (5, "Archives"),
    (6, "Anniversary"),
    (7, "Golden"),
    (8, "Internal"),
    (9, "LegendaryAnniversary"),
    (10, "Hammond"),
    (11, "xBE88B178"),
    (12, "Legendary"),
    (13, "x8A0F302F"),
];

/// Each hero with a name, by that name (untrimmed); a later hero with the
/// same name replaces the earlier one's unlocks in its place.
pub(super) fn heroes(data: &Data) -> Json {
    let mut heroes = Map::new();
    for key in data.all(HERO) {
        let Some(asset) = data.asset(key) else {
            continue;
        };
        let Some(hero) = instance(&asset, "STUHero") else {
            continue;
        };
        let Some(name) = data.string(guid(hero, "m_0EDCE350")) else {
            continue;
        };
        heroes.insert(name, progression(data, guid(hero, "m_heroProgression")));
    }
    Json::Object(heroes)
}

/// `ProgressionUnlocks`: every group null when the hero has no
/// `STUProgressionUnlocks`.
fn progression(data: &Data, key: u64) -> Json {
    let asset = data.asset(key);
    let Some(progression) = asset
        .as_deref()
        .and_then(|asset| instance(asset, "STUProgressionUnlocks"))
    else {
        return json!({
            "OtherUnlocks": null,
            "UnknownUnlocks": null,
            "LevelUnlocks": null,
            "LootBoxesUnlocks": null,
        });
    };
    let levels = array(progression, "m_7846C401").map(|levels| {
        objects(levels)
            .map(|level| {
                json!({
                    "Unlocks": unlock_set(data, inline(level, "m_unlocks")),
                    "Level": int(level, "m_level"),
                })
            })
            .collect::<Vec<_>>()
    });
    json!({
        "OtherUnlocks": unlock_set(data, inline(progression, "m_otherUnlocks")),
        "UnknownUnlocks": unlock_set(data, inline(progression, "m_9135A4B2")),
        "LevelUnlocks": levels,
        "LootBoxesUnlocks": loot_boxes(data, progression),
    })
}

/// `PlayerProgression`, from the first generic settings asset (`054`) with a
/// `STUGenericSettings_PlayerProgression`.
pub(super) fn general(data: &Data) -> Json {
    for key in data.all(GENERIC_SETTINGS) {
        let Some(asset) = data.asset(key) else {
            continue;
        };
        let Some(progression) = instance(&asset, "STUGenericSettings_PlayerProgression") else {
            continue;
        };
        let additional = array(progression, "m_additionalUnlocks").map(|groups| {
            objects(groups)
                .map(|group| {
                    json!({
                        "Unlocks": unlocks(data, guids(group, "m_unlocks")),
                        "Level": int(group, "m_level"),
                    })
                })
                .collect::<Vec<_>>()
        });
        return json!({
            "LootBoxesUnlocks": loot_boxes(data, progression),
            "AdditionalUnlocks": additional,
            "OtherUnlocks": unlock_set(data, inline(progression, "m_otherUnlocks")),
        });
    }
    Json::Null
}

/// `LootBoxUnlocks` for each entry of `m_lootBoxesUnlocks`.
fn loot_boxes(data: &Data, progression: &Object) -> Json {
    array(progression, "m_lootBoxesUnlocks")
        .map(|boxes| {
            objects(boxes)
                .map(|loot_box| {
                    json!({
                        "Unlocks": unlock_set(data, inline(loot_box, "m_unlocks")),
                        "LootBoxType": enum_json(LOOT_BOXES, int(loot_box, "m_lootboxType")),
                    })
                })
                .collect::<Vec<_>>()
        })
        .into()
}

fn objects(values: &[Value]) -> impl Iterator<Item = &Object> {
    values.iter().filter_map(|value| match value {
        Value::Object(object) => Some(object),
        _ => None,
    })
}

/// The unlocks of an inline `STUUnlocks`; empty when it is null.
fn unlock_set(data: &Data, set: Option<&Object>) -> Json {
    unlocks(data, set.and_then(|set| guids(set, "m_unlocks")))
}

/// `Unlock.GetArray`: the GUIDs with a `STU_3021DDED`, in order; empty when
/// there are none.
fn unlocks(data: &Data, keys: Option<Vec<u64>>) -> Json {
    keys.unwrap_or_default()
        .into_iter()
        .filter_map(|key| {
            let asset = data.asset(key)?;
            let stu = instance(&asset, "STU_3021DDED")?;
            Some(unlock(data, key, stu))
        })
        .collect::<Vec<_>>()
        .into()
}

/// DataTool's unlock type, from the unlock's exact class.
fn kind(stu: &Object) -> &'static str {
    match stu.class_name() {
        Some("STUUnlock_SkinTheme") => "Skin",
        Some("STUUnlock_AvatarPortrait") => "Icon",
        Some("STUUnlock_Emote") => "Emote",
        Some("STUUnlock_Pose") => "VictoryPose",
        Some("STUUnlock_VoiceLine") => "VoiceLine",
        Some("STUUnlock_SprayPaint") => "Spray",
        Some("STUUnlock_Currency") => "Currency",
        Some("STUUnlock_PortraitFrame") => "PortraitFrame",
        Some("STUUnlock_Weapon") => "WeaponSkin",
        Some("STUUnlock_POTGAnimation") => "HighlightIntro",
        Some("STUUnlock_HeroMod") => "HeroMod",
        Some("STUUnlock_CompetitiveCurrency") => "CompetitiveCurrency",
        Some("STUUnlock_OWLToken") => "OWLToken",
        Some("STUUnlock_LootBox") => "Lootbox",
        _ => "Unknown",
    }
}

/// One `Unlock`. Members that only concern some types are written only for
/// them (its `ShouldSerialize…` methods): where to get it and the gallery
/// categories for cosmetics, the skin theme for skins, the esports team for
/// skins and team unlocks, the amount for currencies, the box for loot boxes.
fn unlock(data: &Data, key: u64, stu: &Object) -> Json {
    let kind = kind(stu);
    let cosmetic = matches!(
        kind,
        "Icon" | "Spray" | "Skin" | "HighlightIntro" | "VictoryPose" | "VoiceLine" | "Emote"
    );
    let team = team_name(data, guid(stu, "m_0B1BA7C1"));
    let mut rarity = int(stu, "m_rarity");
    if kind == "Lootbox" {
        rarity = int(stu, "m_2F922165");
    }
    let mut unlock = Map::new();
    unlock.insert("GUID".into(), guid_json(key));
    // Blizzard ends some names with a space.
    let name = data.string(guid(stu, "m_name"));
    unlock.insert(
        "Name".into(),
        name.map(|name| name.trim_end_matches(' ').to_owned())
            .into(),
    );
    unlock.insert("Type".into(), kind.into());
    unlock.insert("Rarity".into(), enum_json(RARITIES, rarity));
    unlock.insert(
        "Description".into(),
        data.description(guid(stu, "m_3446F580")).into(),
    );
    if cosmetic {
        unlock.insert("AvailableIn".into(), data.text(stu, "m_53145FAF"));
    }
    if kind == "Skin" {
        unlock.insert("SkinThemeGUID".into(), guid_json(guid(stu, "m_skinTheme")));
    }
    if team.is_some() || kind == "Skin" {
        unlock.insert("IsEsportsUnlock".into(), team.is_some().into());
    }
    if let Some(team) = team {
        unlock.insert("EsportsTeam".into(), team.into());
    }
    if cosmetic {
        let categories = guids(stu, "m_BEE9BCDA")
            .map(|categories| categories.into_iter().map(guid_label).collect::<Vec<_>>());
        unlock.insert("Categories".into(), categories.into());
    }
    let currency = match kind {
        "CompetitiveCurrency" => Some(int(stu, "m_760BF18E")),
        "Currency" => Some(int(stu, "m_currency")),
        "OWLToken" => Some(int(stu, "m_63A026AF")),
        _ => None,
    };
    if let Some(currency) = currency {
        unlock.insert("Currency".into(), currency.into());
    }
    if kind == "Lootbox" {
        let loot_box = enum_json(LOOT_BOXES, int(stu, "m_lootboxType"));
        unlock.insert("LootBoxType".into(), loot_box);
    }
    Json::Object(unlock)
}

/// An esports team's full name (`TeamDefinition.FullName`): its location,
/// then its name unless the two are equal. `None` without a team asset.
fn team_name(data: &Data, key: u64) -> Option<String> {
    let asset = data.asset(key)?;
    let team = instance(&asset, "STU_73AE9738")?;
    let name = data.string(guid(team, "m_137210AF"));
    let location = data.string(guid(team, "m_4BA3B3CE"));
    let name = if location == name { None } else { name };
    let full = format!(
        "{} {}",
        location.unwrap_or_default(),
        name.unwrap_or_default()
    );
    Some(full.trim().to_owned())
}
