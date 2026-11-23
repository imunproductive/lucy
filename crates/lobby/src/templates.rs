//! The login messages whose content is the same for every account: the
//! values our server sets (each a named constant below, with what the client
//! does with it), and the two built from the install's derived data, the
//! preload lists (20505) and the custom game maps (23301).
//!
//! Retail's lobby sent these at login; the reference server replayed a
//! capture of them (1.68). Nothing here comes from that capture any more:
//! live-service data (replay hosts, the real-money store, the profanity
//! filter, anti-cheat payloads, Battle.net hosts and keys) is not sent, and
//! the rest is written by our code. `D:/Work/Coding/refs/reports/RetailTemplates.md`
//! has what each captured message was.

use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{self, BufReader},
    path::Path,
};

use serde::Deserialize;
use serde_json::{Map, Value as Json};

use crate::{
    arcade::ARCADE, lookups, lookups::OWL_LIVE, mode_rules::StatCatalogs, passes::PASSES,
    ranked::CARD_BASE, transport::LobbyMessage,
};
use lucy_content::catalog::UNLOCK_BASE;
use lucy_core::guid::parse_guid;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Float32, Signed, Struct, Unsigned},
};

pub const CLIENT_BUILD: u32 = 104319;
/// The 36600 config entry listing supported builds, a text list like "[1,2,3]".
pub const SUPPORTED_BUILDS_KEY: u64 = 0x0422_7e56;
/// The 36600 config key that can put any loaded hero record in the main menu.
/// The client re-reads 36600 while connected, so a change shows at once. PvE
/// characters have no lobby idle animation and stand in a bind pose.
pub const MENU_HERO_KEY: u64 = 0x8705_3c32;
/// The 36600 console variable that turns on the client's developer features
/// (cvar #79, DevFeatures). With [`ENGINE_CONSOLE_KEY`] set too, Alt+backtick
/// opens the client's own engine console (`refs/reports/AdminMode.md`,
/// "Client tools pass" and "Game checks", 2026-11-16).
pub const DEV_FEATURES_KEY: u64 = 0x0fe1_e03e;
/// The 36600 console variable that allows the client's engine console
/// (cvar #82; `refs/reports/AdminMode.md`, 2026-11-16).
pub const ENGINE_CONSOLE_KEY: u64 = 0x146e_5727;

pub const CONFIG: u32 = 0x1732_ebf2;
pub const IN_CONNECT: u32 = 0xf5ff_548a;
pub const LOBBY: u32 = 0x1a98_79a4;
pub(crate) const CUSTOM_GAMES: u32 = 0x2128_6b49;
const ACCOUNT_INFO: u32 = 0xaa91_be18;
const ACHIEVEMENTS: u32 = 0x267f_de9e;
const SECURITY: u32 = 0x0dfe_efef;
const REPLAYS: u32 = 0x5894_d085;
const CLOCK_SYNC: u32 = 0x34bb_385d;

/// The Practice Range's map (`002`), the custom game list's training map
/// (23301 `+0x90`).
const PRACTICE_RANGE: u64 = 0x0800_0000_0000_0688;
/// The unlock the Arcade gives for wins (the Arcade loot box; every Arcade
/// card names it as its reward, `0C7` `m_B1449DF7`), and the wins that earn
/// one (39801).
const ARCADE_REWARD: u64 = UNLOCK_BASE | 0x3e1;
const ARCADE_REWARD_WINS: [u64; 3] = [3, 6, 9];
/// 39803: the cards the client offers while the player waits in a queue,
/// Skirmish (`0C7` 108) and Deathmatch (105) [INFERENCE: from the names;
/// the handler stores the list, 0x7FF789AB8690].
const WAITING_CARDS: [u64; 2] = [CARD_BASE | 0x108, CARD_BASE | 0x105];

/// What the login sends, in retail's order. A built message (`built`)
/// replaces the constant of its group and id; the ones without a constant
/// must be built.
const AT_LOGIN: [(u32, u32); 27] = [
    (LOBBY, 20814),
    (LOBBY, 20806),
    (LOBBY, 20812),
    (LOBBY, 20813),
    (LOBBY, 20821),
    (PASSES, 58501),
    (ACCOUNT_INFO, 40900),
    (ACCOUNT_INFO, 40914),
    (OWL_LIVE, 43200),
    (ARCADE, 39800),
    (ARCADE, 39801),
    (ARCADE, 39802),
    (ARCADE, 39803),
    (ARCADE, 39810),
    (ARCADE, 39806),
    (ARCADE, 39807),
    (ARCADE, 39804),
    (ACHIEVEMENTS, 28001),
    (SECURITY, 38300),
    (CONFIG, 36600),
    (CONFIG, 36603),
    (CONFIG, 36602),
    (REPLAYS, 51804),
    (CLOCK_SYNC, 43301),
    (lookups::STORE, 26400),
    (LOBBY, 20805),
    (CUSTOM_GAMES, 23301),
];

/// The replay lists the client asks the replay service for (51807, 51810,
/// 51817, 51812, handlers 0x7FF789690770 and on): sent empty, after the
/// replay hosts (51804), also empty: the server keeps no replays.
const EMPTY_REPLAY_LISTS: [u32; 4] = [51807, 51810, 51817, 51812];

/// An empty list at `+0x78`.
fn empty_list() -> Fields {
    Fields::from([(0x78, Array(Vec::new()))])
}

/// The constant messages, by group and id, each with what the client does
/// with it.
fn constant(crc: u32, id: u32, now: u64, config: &Fields) -> Option<Vec<Fields>> {
    let messages = match (crc, id) {
        // Four account flags (Msg20814_Handler 0x7FF789688440): +0x78 shows a
        // login notice (text 45E4.07C, or 45E3.07C with +0x79), +0x7A and
        // +0x7B go to the account component's setters (0x7FF789535A10,
        // 0x7FF789535380). No notice, both flags off.
        (LOBBY, 20814) => vec![Fields::from([
            (0x78, Bool(false)),
            (0x79, Bool(true)),
            (0x7a, Bool(false)),
            (0x7b, Bool(false)),
        ])],
        // A flag the account component keeps at +0xD8 (Msg20806_Handler); off.
        (LOBBY, 20806) => vec![Fields::from([(0x78, Bool(false))])],
        // A list the account component stores (Msg20821_Handler); none.
        (LOBBY, 20821) => vec![empty_list()],
        // Two lists of component 38 (Msg40900_Handler) and lines the client
        // logs (Msg40914_Handler); none.
        (ACCOUNT_INFO, 40900 | 40914) => vec![empty_list()],
        // No Overwatch League match is live: the answer to the client's
        // poll (`lookups::no_owl_match`), then the same with the flag off,
        // as retail sent both at login.
        (OWL_LIVE, 43200) => vec![
            lookups::no_owl_match().2,
            Fields::from([(0x78, Bool(false)), (0x80, Array(Vec::new()))]),
        ],
        // The Arcade's state value (Msg39800_Handler reads +0x78 only).
        (ARCADE, 39800) => vec![Fields::from([(0x78, Unsigned(0)), (0x7c, Float32(0.0))])],
        // The Arcade's win rewards: its loot box at 3, 6 and 9 wins.
        (ARCADE, 39801) => vec![Fields::from([(
            0x78,
            Array(
                ARCADE_REWARD_WINS
                    .iter()
                    .map(|&wins| {
                        Struct(Fields::from([
                            (0, Unsigned(ARCADE_REWARD)),
                            (8, Unsigned(wins)),
                        ]))
                    })
                    .collect(),
            ),
        )])],
        (ARCADE, 39803) => vec![Fields::from([(
            0x78,
            Array(WAITING_CARDS.iter().copied().map(Unsigned).collect()),
        )])],
        // Two more Arcade lists (Msg39806_Handler, Msg39807_Handler); none.
        (ARCADE, 39806 | 39807) => vec![empty_list()],
        // The account's achievements (Msg28001_Handler: +0x78 and +0x90 go
        // to the account component); none earned.
        (ACHIEVEMENTS, 28001) => vec![Fields::from([
            (0x78, Array(Vec::new())),
            (0x90, Array(Vec::new())),
        ])],
        // The anti-cheat's command list (Msg38300_Handler); none.
        (SECURITY, 38300) => vec![empty_list()],
        (CONFIG, 36600) => vec![config.clone()],
        // Two lists of keyed values (Msg36603_Handler); none.
        (CONFIG, 36603) => vec![Fields::from([
            (0x78, Array(Vec::new())),
            (0x90, Array(Vec::new())),
        ])],
        // The server's clock.
        (CONFIG, 36602) => vec![Fields::from([(0x78, Unsigned(now))])],
        // Where the client finds replays: nowhere, and no replay lists.
        (REPLAYS, 51804) => {
            let mut messages = vec![empty_list()];
            messages.extend(EMPTY_REPLAY_LISTS.map(|_| empty_list()));
            return Some(messages);
        }
        // A setting of component 99 the client takes once per revision
        // (+0x88, 0x7FF789598A40): revision 1, both values 0.
        (CLOCK_SYNC, 43301) => vec![Fields::from([
            (0x78, Unsigned(0)),
            (0x80, Unsigned(0)),
            (0x88, Unsigned(1)),
        ])],
        // The account's play time for the play time reminders
        // (Msg20805_Handler warns past each hour up to five): none.
        (LOBBY, 20805) => vec![Fields::from([(
            0x78,
            Struct(Fields::from([
                (0, Unsigned(0)),
                (8, Unsigned(0)),
                (0x10, Bool(false)),
            ])),
        )])],
        _ => return None,
    };
    Some(messages)
}

/// The ids each `AT_LOGIN` entry sends: the replay hosts are followed by
/// the four replay lists.
fn ids(crc: u32, id: u32) -> Vec<u32> {
    if (crc, id) == (REPLAYS, 51804) {
        std::iter::once(id).chain(EMPTY_REPLAY_LISTS).collect()
    } else {
        vec![id]
    }
}

/// The login's content from the install: the preload lists (20505), the
/// custom game maps (23301) and the career stat catalogs (27202).
#[derive(Debug)]
pub struct InstallContent {
    preload: Fields,
    custom_game_maps: Fields,
    stat_catalogs: StatCatalogs,
}

#[derive(Deserialize)]
struct PreloadFile {
    heroes: Vec<String>,
    skin_themes: Vec<String>,
    map_headers: Vec<String>,
    resident: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct MapHeader {
    name: Option<String>,
    variant_name: Option<String>,
    state: Option<String>,
    map_type: Option<String>,
    #[serde(rename = "MapGUID")]
    map_guid: Option<String>,
    game_modes: Option<Vec<Json>>,
}

pub const PRELOAD_FILE: &str = "lobby_preload_174.json";
const MAPS_FILE: &str = "extracted_maps.json";

impl InstallContent {
    /// Read the derived preload lists, map headers and stat catalogs from a
    /// data folder.
    pub fn load(data_dir: &Path) -> io::Result<Self> {
        let preload: PreloadFile = read_json(&data_dir.join(PRELOAD_FILE))?;
        let headers: Map<String, Json> = read_json(&data_dir.join(MAPS_FILE))?;
        let headers = headers
            .into_iter()
            .map(|(key, value)| {
                let header: MapHeader = serde_json::from_value(value).map_err(invalid)?;
                Ok((key, header))
            })
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self {
            preload: preload_fields(&preload)?,
            custom_game_maps: custom_game_maps(headers)?,
            stat_catalogs: StatCatalogs::load(data_dir)?,
        })
    }

    /// 27202's stat catalogs.
    pub fn stat_catalogs(&self) -> &StatCatalogs {
        &self.stat_catalogs
    }

    /// 20505: the hero records (`+0x78`), skin themes (`+0x90`), map
    /// headers (`+0xA8`) and other records (`+0xC0`) the client loads before
    /// the main menu (`lucy_assets` `preload.rs`), and `extra` resources
    /// (the weapon cosmetics' definitions) after the themes, each once.
    pub fn preload(&self, extra: &[u64]) -> Fields {
        let mut fields = self.preload.clone();
        if let Some(Array(resources)) = fields.get_mut(&0x90) {
            let mut listed: HashSet<_> = resources
                .iter()
                .filter_map(|value| match value {
                    Unsigned(guid) => Some(*guid),
                    _ => None,
                })
                .collect();
            for &guid in extra {
                if listed.insert(guid) {
                    resources.push(Unsigned(guid));
                }
            }
        }
        fields
    }

    /// The static login messages in retail's order. A built message
    /// replaces the constant of its group and id; `region` is the account's
    /// store region.
    pub fn at_login(
        &self,
        menu_hero: Option<u64>,
        dev_console: bool,
        now: u64,
        region: &str,
        built: &BTreeMap<(u32, u32), Fields>,
    ) -> io::Result<Vec<LobbyMessage>> {
        let config = menu_config(menu_hero, dev_console);
        let mut messages = Vec::new();
        for (crc, id) in AT_LOGIN {
            if let Some(fields) = built.get(&(crc, id)) {
                messages.push((crc, id, fields.clone()));
                continue;
            }
            let found = match (crc, id) {
                (lookups::STORE, 26400) => {
                    messages.extend(lookups::store(region));
                    continue;
                }
                (CUSTOM_GAMES, 23301) => vec![self.custom_game_maps.clone()],
                _ => constant(crc, id, now, &config).ok_or_else(|| {
                    invalid(format!("the login lacks its built message {crc:08X}/{id}"))
                })?,
            };
            let ids = ids(crc, id);
            messages.extend(
                found
                    .into_iter()
                    .enumerate()
                    .map(|(index, fields)| (crc, ids.get(index).copied().unwrap_or(id), fields)),
            );
        }
        Ok(messages)
    }
}

/// 36600, the client's console variables the server sets: this build in the
/// supported list, and the menu hero and the developer console entries when
/// asked for. Retail also set about 130 others (hosts, keys, store links,
/// tuning); the client keeps its own defaults for them.
pub fn menu_config(menu_hero: Option<u64>, dev_console: bool) -> Fields {
    let entry = |key: u64, value: String| {
        Struct(Fields::from([
            (0x0, Unsigned(key)),
            (0x8, Value::String(value)),
        ]))
    };
    let mut entries = vec![entry(SUPPORTED_BUILDS_KEY, format!("[{CLIENT_BUILD}]"))];
    if let Some(menu_hero) = menu_hero {
        entries.push(entry(MENU_HERO_KEY, format!("0x{menu_hero:016x}")));
    }
    if dev_console {
        for key in [DEV_FEATURES_KEY, ENGINE_CONSOLE_KEY] {
            entries.push(entry(key, "true".into()));
        }
    }
    Fields::from([(0x78, Array(entries))])
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    let file = File::open(path)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))?;
    serde_json::from_reader(BufReader::new(file)).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {error}", path.display()),
        )
    })
}

fn preload_fields(preload: &PreloadFile) -> io::Result<Fields> {
    let list = |keys: &[String]| -> io::Result<Value> {
        keys.iter()
            .map(|key| {
                parse_guid(key)
                    .map(Unsigned)
                    .ok_or_else(|| invalid(format!("invalid preload GUID {key}")))
            })
            .collect::<io::Result<_>>()
            .map(Array)
    };
    Ok(Fields::from([
        (0x78, list(&preload.heroes)?),
        (0x90, list(&preload.skin_themes)?),
        (0xa8, list(&preload.map_headers)?),
        (0xc0, list(&preload.resident)?),
    ]))
}

/// 23301: the maps custom games offer (`+0x78`; the client drops a saved
/// setting's map that isn't listed, 0x7FF789D2D300) and the training map
/// (`+0x90`). A map is listed once, by the first of its headers (by GUID)
/// with a name and variant of its own; menu scenes, the tutorial, headers
/// without game modes and disabled PvE missions are not. This is exactly
/// the list retail sent in 1.68 (70 maps).
fn custom_game_maps(mut headers: Vec<(String, MapHeader)>) -> io::Result<Fields> {
    headers.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut seen = HashSet::new();
    let mut maps = Vec::new();
    for (key, header) in headers {
        let name = header.name.unwrap_or_default();
        if header.game_modes.is_none_or(|modes| modes.is_empty())
            || name.starts_with("Menu/")
            || name == "Tutorial"
            || (header.map_type.as_deref() == Some("PVE")
                && header.state.as_deref() != Some("Enabled"))
            || !seen.insert((name, header.variant_name))
        {
            continue;
        }
        let map = header
            .map_guid
            .as_deref()
            .and_then(map_guid)
            .ok_or_else(|| invalid(format!("map header {key} has no map")))?;
        maps.push(Unsigned(map));
    }
    Ok(Fields::from([
        (0x78, Array(maps)),
        (0x90, Array(vec![Unsigned(PRACTICE_RANGE)])),
    ]))
}

/// A DataTool map GUID such as `5B.002` as the key the game sends.
fn map_guid(text: &str) -> Option<u64> {
    let (index, kind) = text.split_once('.')?;
    kind.eq_ignore_ascii_case("002")
        .then(|| u64::from_str_radix(index, 16).ok())
        .flatten()
        .map(|index| 0x0800_0000_0000_0000 | index)
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// Read fields written as JSON: keys `+0x..` offsets, blobs `{"$bytes":
/// hex}` (the format of saved settings).
pub(crate) fn restore_fields(map: Map<String, Json>) -> io::Result<Fields> {
    map.into_iter()
        .map(|(key, value)| {
            let offset = key
                .strip_prefix("+0x")
                .and_then(|text| u32::from_str_radix(text, 16).ok())
                .ok_or_else(|| invalid(format!("invalid field offset {key:?}")))?;
            Ok((offset, restore_value(value)?))
        })
        .collect()
}

pub(crate) fn restore_value(value: Json) -> io::Result<Value> {
    match value {
        Json::Object(mut map) if map.len() == 1 && map.contains_key("$bytes") => {
            let Some(Json::String(hex)) = map.remove("$bytes") else {
                return Err(invalid("a blob must be hexadecimal text"));
            };
            Ok(Value::Blob(decode_blob(&hex)?))
        }
        Json::Object(map) => Ok(Struct(restore_fields(map)?)),
        Json::Array(values) => Ok(Array(
            values
                .into_iter()
                .map(restore_value)
                .collect::<io::Result<_>>()?,
        )),
        Json::Bool(value) => Ok(Bool(value)),
        Json::String(value) => Ok(Value::String(value)),
        Json::Number(value) => {
            if let Some(value) = value.as_u64() {
                Ok(Unsigned(value))
            } else if let Some(value) = value.as_i64() {
                Ok(Signed(value))
            } else {
                value
                    .as_f64()
                    .map(Value::Float64)
                    .ok_or_else(|| invalid("invalid number"))
            }
        }
        Json::Null => Err(invalid("unexpected null in a field")),
    }
}

fn decode_blob(hex: &str) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let malformed = || invalid("invalid hex in a blob");
    for part in hex.split_ascii_whitespace() {
        let pairs = part.as_bytes().chunks_exact(2);
        if !pairs.remainder().is_empty() {
            return Err(malformed());
        }
        for pair in pairs {
            let high = char::from(pair[0]).to_digit(16).ok_or_else(malformed)?;
            let low = char::from(pair[1]).to_digit(16).ok_or_else(malformed)?;
            bytes.push((high * 16 + low) as u8);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
