//! What the dashboard can read and change, independent of HTTP.
//!
//! Ported from the Python dashboard service. Profile changes are saved at
//! once and sent to the account's game when it is connected.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::Path,
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::Duration,
};

use serde_json::{Map, Value as Json, json};

use super::events::EVENT_INFO;
use crate::lobby::services::Services;
use orpheus_content::catalog::{GAME_REGIONS, REGIONS, Unlock};
use orpheus_core::guid::parse_guid as parse_guid_text;
use orpheus_lobby::{
    accounts::{
        Account, Accounts, BOX_TYPES, LootBox, Profile, account_id_for, load_or_create_profile,
        save_profile,
    },
    celebrations::{EVENT_PRESETS, challenges},
    collection::Collection,
    lootbox::SystemDice,
    menu::PVE_NPCS,
    passes::{MAX_PASSES, Pool},
    ranked::{CURRENT_SEASON, MAX_MATCHES, MAX_RATING, Queue, matches_of, rating_of, wins_of},
    saved_settings::{BOXES_OPENED_KEY, set_saved_value},
    shop::{INVALID_INPUT, Product},
};

use crate::platform::Platform;

const PROFILE_FIELDS: [&str; 18] = [
    "player_name",
    "region",
    "game_region",
    "level",
    "credits",
    "comp_points",
    "league_tokens",
    "endorsement_level",
    "season",
    "sms_protect",
    "lobby_hero",
    "events",
    "server_date",
    "challenge",
    "challenge_wins",
    "unlock_all",
    "bot_chat",
    "stats",
];
/// Whole-number fields that only need a range check; the level starts at 1.
const NUMBER_FIELDS: [&str; 5] = [
    "level",
    "credits",
    "comp_points",
    "league_tokens",
    "challenge_wins",
];
const COLLECTION_KINDS: [(&str, &str); 8] = [
    ("skins", "Skin"),
    ("weapons", "WeaponSkin"),
    ("icons", "Icon"),
    ("sprays", "Spray"),
    ("emotes", "Emote"),
    ("poses", "VictoryPose"),
    ("voicelines", "VoiceLine"),
    ("intros", "HighlightIntro"),
];
const FRAME_TIERS: [&str; 5] = ["Bronze", "Silver", "Gold", "Platinum", "Diamond"];
const PAGE_SIZE: usize = 24;
const DATE_HINT: &str = "Use a date like 2022-10-03.";
const MAX_INT: i128 = i32::MAX as i128;

#[derive(Debug, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }
}

impl From<io::Error> for ApiError {
    fn from(error: io::Error) -> Self {
        Self::new(500, format!("Could not save changes: {error}"))
    }
}

pub type ApiResult = Result<Json, ApiError>;

pub struct Api<'a> {
    pub accounts: &'a Accounts,
    /// The account a freshly started game logs in as.
    pub selected: &'a Mutex<Arc<Account>>,
    /// Logged-in accounts by account ID.
    pub online: &'a Mutex<HashMap<u32, Arc<AtomicBool>>>,
    pub collection: &'a Collection,
    /// The state folder: profiles and default_account.txt.
    pub data: &'a Path,
    /// Optional dashboard pictures: `assets/previews/<GUID>.webp`.
    pub assets: &'a Path,
    pub lobby_port: u16,
    /// Hosting and the games this PC started.
    pub platform: &'a Platform,
    /// The shop, the loot boxes and the live updates of a connected game.
    pub services: &'a Services,
    pub uptime: Duration,
}

impl Api<'_> {
    // Reads

    pub fn state(&self, name: Option<&str>) -> ApiResult {
        let account = self.account(name)?;
        let profile = account.profile().clone();
        Ok(json!({
            "profile": snapshot(&profile)?,
            "battle_tag": account.battle_tag(),
            "accounts": self.account_rows()?,
            "server": self.server_info(),
            "catalogs": self.catalogs(),
        }))
    }

    pub fn collection(&self, query: &BTreeMap<String, String>) -> ApiResult {
        let get = |key: &str| query.get(key).map_or("", |value| value.trim());
        let account = self.account(query.get("account").map(String::as_str))?;
        let profile = account.profile().clone();
        let page = get("page").parse::<usize>().unwrap_or(1).max(1);
        let kind = match get("kind") {
            "" => "all",
            kind => kind,
        };
        if kind == "frames" {
            return Ok(self.frames(&profile, page));
        }
        let types: Vec<&str> = if kind == "all" {
            COLLECTION_KINDS.iter().map(|(_, kind)| *kind).collect()
        } else {
            let found = COLLECTION_KINDS.iter().find(|(name, _)| *name == kind);
            vec![found.ok_or_else(|| ApiError::bad("Unknown item type"))?.1]
        };
        let hero = get("hero");
        let text = get("q").to_lowercase();
        let currency = get("currency");
        let owl_only = matches!(
            get("owl").to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
        let collection = &self.collection;
        let shop = &self.services.shop;
        let mut matches: Vec<(&Unlock, Option<&Product>)> = collection
            .items()
            .unlocks
            .values()
            .filter(|unlock| types.contains(&unlock.item_type.as_str()) && !unlock.name.is_empty())
            .filter(|unlock| hero.is_empty() || unlock.hero.as_deref().unwrap_or("") == hero)
            .filter(|unlock| !owl_only || is_owl(unlock))
            .filter(|unlock| {
                text.is_empty()
                    || format!("{} {}", unlock.name, unlock.hero.as_deref().unwrap_or(""))
                        .to_lowercase()
                        .contains(&text)
            })
            .map(|unlock| (unlock, shop.product(unlock.guid)))
            .filter(|(_, product)| {
                currency.is_empty()
                    || product.is_some_and(|product| product.currency.name() == currency)
            })
            .collect();
        matches.sort_by_cached_key(|(unlock, _)| {
            (
                unlock.hero.clone().unwrap_or_else(|| "~".into()),
                -rarity_rank(&unlock.rarity),
                unlock.name.to_lowercase(),
            )
        });
        let unlocked = profile.unlocked_guids();
        let items: Vec<Json> = matches
            .iter()
            .skip((page - 1) * PAGE_SIZE)
            .take(PAGE_SIZE)
            .map(|(unlock, product)| {
                let owned = profile.unlock_all || collection.owns(&profile, unlock.guid);
                json!({
                    "guid": guid_text(unlock.guid),
                    "name": unlock.name,
                    "hero": unlock.hero.as_deref().unwrap_or(""),
                    "rarity": unlock.rarity,
                    "type": unlock.item_type,
                    "owl": is_owl(unlock),
                    "owned": owned,
                    // Only items unlocked here can be taken back; the starter ones stay.
                    "removable": unlocked.contains(&unlock.guid) && !profile.unlock_all,
                    "price": product.map(|product| product.price),
                    "currency": product.map(|product| product.currency.name()),
                    "purchasable": product.is_some() && !owned,
                    "preview": self.preview(unlock.guid),
                })
            })
            .collect();
        Ok(page_result(items, page, matches.len()))
    }

    fn frames(&self, profile: &Profile, page: usize) -> Json {
        let collection = &self.collection;
        let in_use = collection.portrait_frame(profile);
        let borders = collection.border_levels();
        let items = borders
            .iter()
            .skip((page - 1) * PAGE_SIZE)
            .take(PAGE_SIZE)
            .map(|&(level, guid)| {
                json!({
                    "guid": guid_text(guid),
                    "name": frame_name(level),
                    "hero": "",
                    "rarity": "Common",
                    "type": "PortraitFrame",
                    "frame": true,
                    "in_use": guid == in_use,
                    "chosen": guid == profile.frame_guid,
                    "preview": self.preview(guid),
                })
            })
            .collect();
        page_result(items, page, borders.len())
    }

    fn account_rows(&self) -> Result<Vec<Json>, ApiError> {
        let selected = self.selected().name.to_lowercase();
        let default = self.default_account_name()?.to_lowercase();
        let online = self.online.lock().expect("lobby accounts");
        Ok(self
            .accounts
            .all_saved()?
            .into_iter()
            .map(|name| {
                json!({
                    "online": online.contains_key(&account_id_for(&name)),
                    "selected": name.to_lowercase() == selected,
                    "default": name.to_lowercase() == default,
                    "name": name,
                })
            })
            .collect())
    }

    fn server_info(&self) -> Json {
        let connected = self.online.lock().expect("lobby accounts").len();
        json!({
            "host": self.platform.host.to_string(),
            "port": self.lobby_port,
            "connected_clients": connected,
            "uptime_seconds": self.uptime.as_secs(),
            "game_instances": [],
            "game_runtime_available": true,
            "second_games": self.platform.games.is_some(),
            "matchmaking_supported": false,
            "test_players": 0,
            "modes": [],
            "maps": [],
            "forced_map": "",
        })
    }

    fn catalogs(&self) -> Json {
        let items = self.collection.items();
        let mut heroes: Vec<(&str, u64)> = self
            .collection
            .heroes()
            .iter()
            .filter_map(|&guid| Some((items.hero_names.get(&guid)?.as_str(), guid)))
            .collect();
        heroes.sort();
        let mut npcs: Vec<&str> = PVE_NPCS.iter().map(|(name, _)| *name).collect();
        npcs.sort_unstable();
        let challenges: Vec<Json> = challenges()
            .map(|(title, rewards)| {
                let rewards: Vec<Json> = rewards
                    .iter()
                    .filter_map(|&guid| items.get(guid))
                    .map(|reward| json!({"name": reward.name, "type": reward.item_type}))
                    .collect();
                json!({"id": title, "title": title, "rewards": rewards})
            })
            .collect();
        json!({
            "heroes": heroes
                .iter()
                .map(|(name, guid)| json!({"guid": guid_text(*guid), "name": name}))
                .collect::<Vec<_>>(),
            "npcs": npcs,
            "events": EVENT_INFO
                .iter()
                .map(|event| json!({
                    "id": event.id,
                    "label": event.label,
                    "description": event.description,
                    "category": event.category,
                    "scene_status": event.scene_status,
                    "scene_note": event.scene_note,
                }))
                .collect::<Vec<_>>(),
            "box_types": BOX_TYPES
                .iter()
                .map(|kind| json!({"id": kind.id, "name": kind.name, "label": kind.label}))
                .collect::<Vec<_>>(),
            "challenges": challenges,
            "frames": self.collection
                .border_levels()
                .iter()
                .map(|&(level, guid)| json!({"level": level, "guid": guid_text(guid)}))
                .collect::<Vec<_>>(),
        })
    }

    fn preview(&self, guid: u64) -> Option<String> {
        let name = format!("{guid:016X}.webp");
        self.assets
            .join("previews")
            .join(&name)
            .is_file()
            .then(|| format!("/assets/previews/{name}"))
    }

    // Writes

    pub fn update_profile(&self, data: &Map<String, Json>) -> ApiResult {
        let unknown: Vec<&str> = data
            .keys()
            .map(String::as_str)
            .filter(|key| !is_editable(key))
            .collect();
        if !unknown.is_empty() {
            return Err(ApiError::bad(format!(
                "Unknown field: {}",
                sorted(unknown).join(", ")
            )));
        }
        let account = self.account(data.get("account").and_then(Json::as_str))?;
        let profile = self.edit(&account, |profile| {
            self.apply(profile, data)?;
            Ok(Vec::new())
        })?;
        Ok(json!({"status": "ok", "profile": snapshot(&profile)?}))
    }

    /// Give the lobby event or the hero challenge to every account, and to new
    /// ones through the template profile. Challenge wins stay each player's own.
    pub fn apply_to_all(&self, data: &Map<String, Json>) -> ApiResult {
        let changes: Map<String, Json> = data
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "events" | "challenge"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        if changes.is_empty() || changes.len() != data.len() {
            return Err(ApiError::bad("Send an event or a challenge"));
        }
        let names = self.accounts.all_saved()?;
        for name in &names {
            let account = self.accounts.get(name)?;
            self.edit(&account, |profile| {
                self.apply(profile, &changes)?;
                Ok(Vec::new())
            })?;
        }
        let template = self.data.join("profile.json");
        let mut profile = load_or_create_profile(&template)?;
        self.apply(&mut profile, &changes)?;
        save_profile(&profile, &template)?;
        Ok(json!({"status": "ok", "accounts": names.len()}))
    }

    pub fn add_boxes(&self, data: &Map<String, Json>) -> ApiResult {
        let kind = parse_int(
            data.get("type").unwrap_or(&json!(0)),
            "Box type",
            0,
            MAX_INT,
        )?;
        let count = parse_int(data.get("count").unwrap_or(&json!(10)), "Box count", 1, 100)?;
        if !BOX_TYPES
            .iter()
            .any(|box_type| i128::from(box_type.id) == kind)
        {
            return Err(ApiError::bad("No such box type in the catalog"));
        }
        let account = self.account(data.get("account").and_then(Json::as_str))?;
        let profile = self.edit(&account, |profile| {
            profile.add_boxes(kind as i32, count as u64)?;
            Ok(Vec::new())
        })?;
        Ok(json!({"status": "ok", "added": count, "total_boxes": profile.loot_boxes.len()}))
    }

    /// The move to Overwatch 2: every box opens, and the game says how many
    /// on the main menu (20802 +0x130, sent to a connected game).
    pub fn open_all_boxes(&self, data: &Map<String, Json>) -> ApiResult {
        let account = self.account(data.get("account").and_then(Json::as_str))?;
        let mut opened = 0;
        let mut new_items = 0;
        let profile = self.edit(&account, |profile| {
            let (count, granted) =
                self.services
                    .loot
                    .open_all(profile, self.collection, &mut SystemDice)?;
            if count == 0 {
                return Err(ApiError::bad("No boxes to open"));
            }
            set_saved_value(profile, BOXES_OPENED_KEY, count as i64);
            opened = count;
            new_items = granted.len();
            Ok(granted)
        })?;
        self.services.push_settings(account.account_lo);
        Ok(json!({
            "status": "ok",
            "opened": opened,
            "new_items": new_items,
            "profile": snapshot(&profile)?,
        }))
    }

    /// Buy an item for the account with its own currency.
    pub fn purchase(&self, data: &Map<String, Json>) -> ApiResult {
        let guid = purchase_guid(data.get("guid").unwrap_or(&Json::Null))?;
        let account = self.account(data.get("account").and_then(Json::as_str))?;
        let mut receipt = None;
        let profile = self.edit(&account, |profile| {
            let bought = self
                .services
                .shop
                .purchase(profile, guid, self.collection)
                .map_err(|refused| ApiError::new(refused.status, refused.message))?;
            let granted = [bought.guid]
                .into_iter()
                .chain(bought.also.clone())
                .collect();
            receipt = Some(bought);
            Ok(granted)
        })?;
        let receipt = receipt.expect("a saved purchase has a receipt");
        Ok(json!({
            "status": "ok",
            "receipt": {
                "guid": guid_text(receipt.guid),
                "price": receipt.price,
                "currency": receipt.currency.name(),
                "also": receipt.also.iter().map(|&pair| guid_text(pair)).collect::<Vec<_>>(),
            },
            "profile": snapshot(&profile)?,
        }))
    }

    /// Put a portrait frame on the player card, or go back to the level's frame.
    pub fn set_frame(&self, data: &Map<String, Json>) -> ApiResult {
        let guid = match data.get("guid") {
            None | Some(Json::Null) => 0,
            Some(Json::String(text)) if text.is_empty() => 0,
            Some(value) => parse_guid(value)?,
        };
        let frames = self.collection.border_levels();
        if guid != 0 && !frames.iter().any(|&(_, frame)| frame == guid) {
            return Err(ApiError::bad("Invalid frame"));
        }
        let account = self.account(data.get("account").and_then(Json::as_str))?;
        self.edit(&account, |profile| {
            profile.frame_guid = guid;
            Ok(Vec::new())
        })?;
        Ok(json!({"status": "ok", "frame": (guid != 0).then(|| guid_text(guid))}))
    }

    /// Grant items by GUID, or take them back with `"revoke": true`.
    pub fn grant_skin(&self, data: &Map<String, Json>) -> ApiResult {
        let values = match (data.get("guids"), data.get("guid")) {
            (Some(Json::Array(values)), _) if !values.is_empty() => values.clone(),
            (None, Some(value)) => vec![value.clone()],
            _ => return Err(ApiError::bad("No skin selected")),
        };
        let items = self.collection.items();
        let mut guids = Vec::with_capacity(values.len());
        for value in &values {
            let guid = parse_guid(value)?;
            if items.get(guid).is_none() {
                return Err(ApiError::bad("Invalid item"));
            }
            guids.push(guid);
        }
        let revoke = data
            .get("revoke")
            .map(parse_bool)
            .transpose()?
            .unwrap_or(false);
        let account = self.account(data.get("account").and_then(Json::as_str))?;
        let profile = self.edit(&account, |profile| {
            let mut owned = profile.unlocked_guids();
            let granted = if revoke {
                Vec::new()
            } else {
                guids
                    .iter()
                    .copied()
                    .filter(|guid| !owned.contains(guid))
                    .collect()
            };
            for guid in &guids {
                if revoke {
                    owned.remove(guid);
                } else {
                    owned.insert(*guid);
                }
            }
            let mut owned: Vec<u64> = owned.into_iter().collect();
            owned.sort_unstable();
            profile.unlocked_items = owned
                .into_iter()
                .map(|guid| Json::String(guid_text(guid)))
                .collect();
            Ok(granted)
        })?;
        let changed: Vec<String> = guids.iter().map(|&guid| guid_text(guid)).collect();
        let (granted, revoked) = if revoke {
            (Vec::new(), changed)
        } else {
            (changed, Vec::new())
        };
        Ok(json!({
            "status": "ok",
            "granted": granted,
            "revoked": revoked,
            "unlocked_count": profile.unlocked_items.len(),
            "profile": snapshot(&profile)?,
        }))
    }

    /// Make this account the one a freshly started game logs in as.
    pub fn select_account(&self, data: &Map<String, Json>) -> ApiResult {
        let account = self.saved(data.get("name").and_then(Json::as_str))?;
        *self.selected.lock().expect("selected account") = account;
        Ok(json!({"status": "ok"}))
    }

    /// Make this account the one the game logs in as each time the server starts.
    pub fn default_account(&self, data: &Map<String, Json>) -> ApiResult {
        let account = self.saved(data.get("name").and_then(Json::as_str))?;
        fs::write(
            self.data.join("default_account.txt"),
            format!("{}\n", account.name),
        )?;
        Ok(json!({"message": format!("{} is the default account now.", account.name)}))
    }

    // Helpers

    fn selected(&self) -> Arc<Account> {
        Arc::clone(&self.selected.lock().expect("selected account"))
    }

    /// The saved account with this name (any letter case), or the selected one.
    fn account(&self, name: Option<&str>) -> Result<Arc<Account>, ApiError> {
        match name {
            None | Some("") => Ok(self.selected()),
            name => self.saved(name),
        }
    }

    fn saved(&self, name: Option<&str>) -> Result<Arc<Account>, ApiError> {
        let wanted = name
            .ok_or_else(|| ApiError::bad("Invalid profile name"))?
            .trim()
            .to_lowercase();
        let saved = self.accounts.all_saved()?;
        let name = saved
            .iter()
            .find(|saved| saved.to_lowercase() == wanted)
            .ok_or_else(|| ApiError::new(404, "Profile not found."))?;
        Ok(self.accounts.get(name)?)
    }

    fn default_account_name(&self) -> io::Result<String> {
        match fs::read_to_string(self.data.join("default_account.txt")) {
            Ok(name) => Ok(name.trim().to_owned()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    /// Change a copy of the profile; save and keep it only when every change
    /// is valid, then send it to the account's game when that is connected.
    /// `change` returns the items the edit gave (`_save`).
    fn edit(
        &self,
        account: &Account,
        change: impl FnOnce(&mut Profile) -> Result<Vec<u64>, ApiError>,
    ) -> Result<Profile, ApiError> {
        let mut rejected = None;
        let saved = account.edit_profile(|profile| match change(profile) {
            Ok(granted) => Ok(Some((profile.clone(), granted))),
            Err(error) => {
                rejected = Some(error);
                Ok(None)
            }
        })?;
        match (saved, rejected) {
            (Some((profile, granted)), _) => {
                self.services.push_profile(account.account_lo, granted);
                Ok(profile)
            }
            (None, Some(error)) => Err(error),
            (None, None) => unreachable!("an edit either saves or is rejected"),
        }
    }

    /// Check and copy each field present in `data` onto the profile.
    fn apply(&self, profile: &mut Profile, data: &Map<String, Json>) -> Result<(), ApiError> {
        if let Some(value) = data.get("player_name") {
            profile.player_name = parse_player_name(value)?;
        }
        if let Some(value) = data.get("region") {
            let region = value
                .as_str()
                .filter(|code| REGIONS.iter().any(|r| r.code == *code));
            profile.region = region
                .ok_or_else(|| ApiError::bad("Unknown region"))?
                .into();
        }
        if let Some(value) = data.get("game_region") {
            let region = value
                .as_str()
                .filter(|name| GAME_REGIONS.iter().any(|(known, _)| known == name));
            profile.game_region = region
                .ok_or_else(|| ApiError::bad("Unknown game region"))?
                .into();
        }
        let level_before = profile.level;
        for field in NUMBER_FIELDS {
            let Some(value) = data.get(field) else {
                continue;
            };
            let lowest = if field == "level" { 1 } else { 0 };
            let number = parse_int(value, field, lowest, MAX_INT)? as i64;
            match field {
                "level" => profile.level = number,
                "credits" => profile.credits = number,
                "comp_points" => profile.comp_points = number,
                "league_tokens" => profile.league_tokens = number,
                _ => profile.challenge_wins = number,
            }
        }
        if profile.level != level_before {
            profile.frame_guid = 0; // a new level brings back the level's frame
        }
        if let Some(value) = data.get("endorsement_level") {
            profile.endorsement_level = parse_int(value, "Endorsement level", 1, 5)? as i64;
        }
        if let Some(value) = data.get("season") {
            profile.season = parse_int(value, "Season", 1, i128::from(CURRENT_SEASON))? as i64;
        }
        for queue in Queue::ALL {
            let name = queue.name();
            let label = capitalize(name);
            if let Some(value) = data.get(&format!("rating_{name}")) {
                let rating = parse_int(value, &format!("{label} rating"), 1, MAX_RATING.into())?;
                profile.ratings.insert(name.into(), json!(rating as i64));
            }
            if let Some(value) = data.get(&format!("matches_{name}")) {
                let matches = parse_int(value, &format!("{label} matches"), 0, MAX_MATCHES.into())?;
                profile.matches.insert(name.into(), json!(matches as i64));
            }
            if let Some(value) = data.get(&format!("wins_{name}")) {
                let wins = parse_int(value, &format!("{label} wins"), 0, MAX_MATCHES.into())?;
                profile.wins.insert(name.into(), json!(wins as i64));
            }
        }
        for queue in Queue::ALL {
            let name = queue.name();
            let changed = data.contains_key(&format!("wins_{name}"))
                || data.contains_key(&format!("matches_{name}"));
            let wins = profile.wins.get(name).and_then(Json::as_i64).unwrap_or(0);
            if changed && wins > i64::from(matches_of(profile, queue)?) {
                return Err(ApiError::bad(format!(
                    "{} wins can't be more than {name} matches.",
                    capitalize(name)
                )));
            }
        }
        for pool in Pool::ALL {
            if let Some(value) = data.get(&format!("passes_{}", pool.profile_key())) {
                let count = parse_int(value, "Priority passes", 0, MAX_PASSES.into())?;
                profile
                    .priority_passes
                    .insert(pool.profile_key().into(), json!(count as i64));
            }
        }
        if let Some(value) = data.get("sms_protect") {
            profile.sms_protect = parse_bool(value)?;
        }
        if let Some(value) = data.get("unlock_all") {
            profile.unlock_all = parse_bool(value)?;
        }
        if let Some(value) = data.get("bot_chat") {
            profile.bot_chat = parse_bool(value)?;
        }
        if let Some(value) = data.get("events") {
            profile.events = parse_events(value)?;
        }
        if let Some(value) = data.get("lobby_hero") {
            profile.lobby_hero = self.parse_lobby_hero(value)?;
        }
        if let Some(value) = data.get("server_date") {
            profile.server_date = parse_server_date(value)?;
        }
        if let Some(value) = data.get("challenge") {
            profile.challenge = parse_challenge(value)?;
        }
        Ok(())
    }

    fn parse_lobby_hero(&self, value: &Json) -> Result<String, ApiError> {
        let names = &self.collection.items().hero_names;
        value
            .as_str()
            .filter(|name| {
                matches!(*name, "random" | "none")
                    || names.values().any(|hero| hero == name)
                    || PVE_NPCS.iter().any(|(npc, _)| npc == name)
            })
            .map(str::to_owned)
            .ok_or_else(|| ApiError::bad("Pick a hero from the list."))
    }
}

fn is_editable(key: &str) -> bool {
    if key == "account" || (key != "stats" && PROFILE_FIELDS.contains(&key)) {
        return true;
    }
    let queue = ["rating_", "matches_", "wins_"]
        .iter()
        .find_map(|prefix| key.strip_prefix(prefix));
    if queue.is_some_and(|name| Queue::ALL.iter().any(|queue| queue.name() == name)) {
        return true;
    }
    key.strip_prefix("passes_")
        .is_some_and(|name| Pool::ALL.iter().any(|pool| pool.profile_key() == name))
}

/// The profile as the page sees it, plus loot box and collection counts.
pub fn snapshot(profile: &Profile) -> Result<Json, ApiError> {
    let Json::Object(all) = serde_json::to_value(profile).map_err(io::Error::other)? else {
        unreachable!("a profile serializes to an object");
    };
    let mut result: Map<String, Json> = PROFILE_FIELDS
        .iter()
        .map(|&name| (name.into(), all.get(name).cloned().unwrap_or(Json::Null)))
        .collect();
    result.insert("loot_boxes_count".into(), json!(profile.loot_boxes.len()));
    result.insert("unlocked_count".into(), json!(profile.unlocked_items.len()));
    result.insert("box_counts".into(), json!(box_counts(&profile.loot_boxes)));
    result.insert(
        "frame".into(),
        json!((profile.frame_guid != 0).then(|| guid_text(profile.frame_guid))),
    );
    for queue in Queue::ALL {
        let name = queue.name();
        result.insert(format!("rating_{name}"), json!(rating_of(profile, queue)?));
        result.insert(
            format!("matches_{name}"),
            json!(matches_of(profile, queue)?),
        );
        result.insert(format!("wins_{name}"), json!(wins_of(profile, queue)?));
    }
    for pool in Pool::ALL {
        let key = pool.profile_key();
        result.insert(format!("passes_{key}"), json!(pool.count(profile)?));
    }
    Ok(Json::Object(result))
}

fn box_counts(boxes: &[LootBox]) -> Vec<Json> {
    let mut counts = BTreeMap::new();
    for loot_box in boxes {
        *counts.entry(loot_box.box_type).or_insert(0usize) += 1;
    }
    counts
        .into_iter()
        .map(|(kind, count)| {
            let known = BOX_TYPES.iter().find(|box_type| box_type.id == kind);
            json!({
                "type": kind,
                "name": known.map_or_else(|| kind.to_string(), |box_type| box_type.name.into()),
                "label": known.map_or_else(|| kind.to_string(), |box_type| box_type.label.into()),
                "count": count,
            })
        })
        .collect()
}

/// How a frame looks, from its unlock level: "Gold ★★ · level 1441+".
fn frame_name(level: i64) -> String {
    let done = level.max(1) - 1;
    let tier = FRAME_TIERS[(done / 600).min(4) as usize];
    let stars = (done % 600 / 100) as usize;
    let stars = if stars == 0 {
        String::new()
    } else {
        format!(" {}", "★".repeat(stars))
    };
    format!("{tier}{stars} · level {}+", level.max(1))
}

fn page_result(items: Vec<Json>, page: usize, total: usize) -> Json {
    let pages = total.div_ceil(PAGE_SIZE).max(1);
    json!({"items": items, "page": page, "total": total, "pages": pages})
}

fn guid_text(guid: u64) -> String {
    format!("0x{guid:016X}")
}

fn is_owl(unlock: &Unlock) -> bool {
    unlock.categories.iter().any(|category| category == "OWL")
}

fn rarity_rank(rarity: &str) -> i8 {
    match rarity {
        "Rare" => 1,
        "Epic" => 2,
        "Legendary" => 3,
        _ => 0,
    }
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn sorted(mut names: Vec<&str>) -> Vec<&str> {
    names.sort_unstable();
    names
}

/// A whole number in `low..=high`. Booleans and fractions are refused.
fn parse_int(value: &Json, label: &str, low: i128, high: i128) -> Result<i128, ApiError> {
    let whole = || ApiError::bad(format!("{label}: enter a whole number"));
    let number = match value {
        Json::Number(number) => number
            .as_i64()
            .map(i128::from)
            .or_else(|| number.as_u64().map(i128::from))
            .ok_or_else(whole)?,
        Json::String(text) => text.trim().parse::<i128>().map_err(|_| whole())?,
        _ => return Err(whole()),
    };
    if !(low..=high).contains(&number) {
        return Err(ApiError::bad(format!(
            "{label}: allowed range is {low} to {high}"
        )));
    }
    Ok(number)
}

fn parse_bool(value: &Json) -> Result<bool, ApiError> {
    match value {
        Json::Bool(value) => Ok(*value),
        Json::String(text) if text == "true" || text == "false" => Ok(text == "true"),
        _ => Err(ApiError::bad("Use true or false.")),
    }
}

/// An item GUID given as a number or as text such as "0x0250000000001149".
fn parse_guid(value: &Json) -> Result<u64, ApiError> {
    match value {
        Json::Number(number) => number.as_u64(),
        Json::String(text) => parse_guid_text(text),
        _ => None,
    }
    .ok_or_else(|| ApiError::bad("Invalid item"))
}

fn parse_player_name(value: &Json) -> Result<String, ApiError> {
    let name = value
        .as_str()
        .ok_or_else(|| ApiError::bad("Enter a valid nickname"))?
        .trim();
    let length = name.chars().count();
    if !(1..=32).contains(&length) || name.chars().any(|c| (c as u32) < 32) {
        return Err(ApiError::bad("Nickname must be 1 to 32 characters"));
    }
    Ok(name.into())
}

/// The lobby event list: empty, or one event id. A plain form sends a
/// comma-separated string.
fn parse_events(value: &Json) -> Result<Vec<String>, ApiError> {
    let pick = || ApiError::bad("Pick an event from the list.");
    let events: Vec<String> = match value {
        Json::String(text) => text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_lowercase)
            .collect(),
        Json::Array(values) => values
            .iter()
            .map(|value| value.as_str().map(str::to_owned).ok_or_else(pick))
            .collect::<Result<_, _>>()?,
        _ => return Err(pick()),
    };
    if events.len() > 1
        || events
            .iter()
            .any(|id| !EVENT_PRESETS.contains(&id.as_str()))
    {
        return Err(pick());
    }
    Ok(events)
}

/// "", "now", or an ISO date the client clock can hold.
fn parse_server_date(value: &Json) -> Result<String, ApiError> {
    let text = value
        .as_str()
        .ok_or_else(|| ApiError::bad(DATE_HINT))?
        .trim();
    if text.is_empty() || text == "now" {
        return Ok(text.into());
    }
    let date = parse_iso_date(text).ok_or_else(|| ApiError::bad(DATE_HINT))?;
    // The event start is two days earlier (an unsigned STU year from 2000), and
    // CONFIG 36602 holds a u32 Unix timestamp at UTC noon.
    if !((2000, 1, 3)..=(2106, 2, 6)).contains(&date) {
        return Err(ApiError::bad(
            "Server date must be between 2000-01-03 and 2106-02-06",
        ));
    }
    Ok(text.into())
}

/// A calendar date written exactly as `YYYY-MM-DD`.
fn parse_iso_date(text: &str) -> Option<(u32, u32, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| {
        let part = &text[range];
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (year >= 1 && (1..=days).contains(&day)).then_some((year, month, day))
}

/// A challenge title, or "" for none.
fn parse_challenge(value: &Json) -> Result<String, ApiError> {
    value
        .as_str()
        .filter(|title| title.is_empty() || challenges().any(|(known, _)| known == *title))
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad("Challenge not found"))
}

mod games;
/// The item GUID of a purchase, read as Python's `parse_guid` then the shop's
/// own check: a number or text such as "0x0250000000001149" (an unreadable
/// one is "Invalid item"), between 1 and 2^64 - 1 (else "Invalid item
/// identifier.").
fn purchase_guid(value: &Json) -> Result<u64, ApiError> {
    let unreadable = || ApiError::bad("Invalid item");
    let number = match value {
        Json::Bool(value) => i128::from(*value),
        Json::Number(number) => number
            .as_u64()
            .map(i128::from)
            .or_else(|| number.as_i64().map(i128::from))
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(|value| value.trunc() as i128)
            })
            .ok_or_else(unreadable)?,
        Json::String(text) => match text.trim().strip_prefix('-') {
            Some(magnitude) => -i128::from(parse_guid_text(magnitude).ok_or_else(unreadable)?),
            None => i128::from(parse_guid_text(text).ok_or_else(unreadable)?),
        },
        _ => return Err(unreadable()),
    };
    u64::try_from(number)
        .ok()
        .filter(|&guid| guid != 0)
        .ok_or_else(|| ApiError::new(INVALID_INPUT.status, INVALID_INPUT.message))
}
#[cfg(test)]
mod tests;
