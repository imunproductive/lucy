//! Login event records and their externally supplied content keys.
//!
//! Mirrors `ow174/content/celebrations.py` and `ow174/catalog/events.py`.
//! Resource bytes stay in the separate runtime catalog and are sent on the
//! lobby link. Errors do not include the contents of that catalog.
//!
//! An event is a record in 38900. The fields the client needs, as in the
//! recorded Lunar New Year 2022 record: +0x40 celebration id (picks the
//! catalog of main menu maps), +0x70 celebration type (picks the event map
//! variants and the event loot box), +0x30/+0x38 content key name and id (the
//! key bytes go in 20504), +0x0 map swaps, +0x58 login rewards, +0xB0 event
//! loot box type. The "What's new" screen lists the contents of the +0x58 loot
//! box as the event's "New trophies"; the boxes are the ones
//! STULootBoxCelebrationOverride (054/0x18) gives each celebration type.
//! Summer Games and Archives have no main menu scene in 1.74, but their type
//! still picks the event loot box. The one event splash 1.74 still wires is
//! the pre-Overwatch 2 credits notice: 01B/0C74 shows 05A/07D7 when that
//! celebration is greeted.
//!
//! Hero challenges are event records too, with reward tiers and the stat that
//! counts toward them (the recorded one, Ashe's Year of the Tiger 2022 week
//! 1). In 1.74 it counts played plus won matches (a win counts twice). The
//! only challenges 1.74's Play menu has a banner for (01B/0B7C) are
//! celebrations 0x119, 0x11A and 0x11B, weeks 1-3 of Anniversary Remix 3;
//! their rewards are participation icon, spray and skin, in that order.

use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
    sync::Arc,
};

use indexmap::IndexMap;
use serde::Deserialize;

use crate::{
    accounts::{LootBox, Profile},
    clock::{DAY, stu_datetime},
    collection::Collection,
    economy::{Economy, EventBox},
    templates::IN_CONNECT,
    transport::LobbyMessage,
};
use lucy_content::catalog::{ItemDb, UNLOCK_BASE};
use lucy_core::guid::parse_guid;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Float64, Signed, Struct, Unsigned},
};

pub const EVENTS: u32 = 0x8b77_c823;
pub const CONTENT_KEYS: u32 = 20504;
pub const CELEBRATIONS: u32 = 38900;
pub const NEW_CELEBRATION: u32 = 38901;
pub const CHALLENGE_PROGRESS: u32 = 38902;

/// What greeting the player for new events gave.
#[derive(Debug, Default)]
pub struct Greeting {
    /// One 38901 per event.
    pub messages: Vec<LobbyMessage>,
    /// The reward items given.
    pub given: Vec<u64>,
    /// The event boxes given.
    pub boxes: Vec<LootBox>,
}

const CELEBRATION_BASE: u64 = 0x0430_0000_0000_0000;
const CELEBRATION_TYPE_BASE: u64 = 0x0d80_0000_0000_0000;
const RESOURCE_KEY_BASE: u64 = 0x0f10_0000_0000_0000;
const MAP_BASE: u64 = 0x0800_0000_0000_0000;
const CHALLENGE_WINS_STAT: u64 = 0x0860_0000_0000_073f;
const CHALLENGE_TIER_WINS: u64 = 9;
const KEY_LENGTH: usize = 16;

/// A celebration the server can run. A seasonal one names its celebration
/// type (`kind`, `01C`); the install gives the loot box of that type
/// ([`Economy::event_box`]), which is the first login reward and the record's
/// box type (`ow174/catalog/events.py` `EventDef.box`, `rewards`).
#[derive(Clone, Copy, Eq, PartialEq)]
struct Event {
    celebration: u16,
    key: u16,
    kind: u16,
    /// Login rewards besides the event box.
    rewards: &'static [u16],
    // Splash presets and raw IDs with the same celebration are separate entries.
    splash: bool,
}

impl Event {
    const fn new(celebration: u16) -> Self {
        Self {
            celebration,
            key: 0,
            kind: 0,
            rewards: &[],
            splash: false,
        }
    }

    const fn keyed(celebration: u16, key: u16) -> Self {
        Self {
            key,
            ..Self::new(celebration)
        }
    }

    const fn seasonal(celebration: u16, key: u16, kind: u16, rewards: &'static [u16]) -> Self {
        Self {
            kind,
            rewards,
            ..Self::keyed(celebration, key)
        }
    }

    fn guid(self) -> u64 {
        CELEBRATION_BASE | u64::from(self.celebration)
    }
}

/// The event names `Profile::events` accepts, besides raw `0x` celebration IDs.
pub const EVENT_PRESETS: [&str; 15] = [
    "goodbye",
    "lunar",
    "halloween",
    "winter",
    "anniversary",
    "anniversary_remix_1",
    "anniversary_remix_2",
    "summer",
    "archives",
    "reaper",
    "cassidy",
    "malevento",
    "owl",
    "watchpoint",
    "ow2_credits",
];

/// Hero challenges by title: celebration and reward unlock indexes.
const CHALLENGES: [(&str, u16, [u64; 3]); 3] = [
    ("Tracer's Comic Challenge", 0x119, [0x4aea, 0x4aeb, 0x4aec]),
    (
        "Symmetra's Restoration Challenge",
        0x11a,
        [0x4b10, 0x4b11, 0x4b08],
    ),
    ("Kanezaka Challenge", 0x11b, [0x4bb2, 0x4bb1, 0x4aa4]),
];

/// Challenge titles and their reward unlock GUIDs.
pub fn challenges() -> impl Iterator<Item = (&'static str, [u64; 3])> {
    CHALLENGES
        .iter()
        .map(|&(title, _, indexes)| (title, indexes.map(|index| UNLOCK_BASE | index)))
}

fn preset(name: &str) -> Option<Event> {
    Some(match name {
        "goodbye" => Event::keyed(0x128, 0x1a3),
        "lunar" => Event::seasonal(0x105, 0x188, 0x237c, &[0x4f50]),
        "halloween" => Event::seasonal(0xfb, 0x181, 0x212e, &[]),
        "winter" => Event::seasonal(0xff, 0x185, 0x2168, &[]),
        "anniversary" => Event::seasonal(0x118, 0x198, 0x2428, &[]),
        "anniversary_remix_1" => Event::seasonal(0x10e, 0x196, 0x2428, &[]),
        "anniversary_remix_2" => Event::seasonal(0x112, 0x197, 0x2428, &[]),
        "summer" => Event::seasonal(0xeb, 0x178, 0x212f, &[]),
        "archives" => Event::seasonal(0xa8, 0x169, 0x2427, &[]),
        "reaper" => Event {
            rewards: &[0x4dc7, 0x4ef3, 0x4fcf],
            ..Event::keyed(0x104, 0x189)
        },
        "cassidy" => Event::keyed(0xf7, 0x17f),
        "malevento" => Event::new(0xfa),
        "owl" => Event::new(0x125),
        "watchpoint" => Event::new(0x126),
        "ow2_credits" => Event {
            splash: true,
            ..Event::new(0x123)
        },
        _ => return None,
    })
}

fn active_events(names: &[String]) -> io::Result<Vec<Event>> {
    let mut events = Vec::new();
    for name in names {
        let name = name.trim().to_ascii_lowercase();
        let event = if name.starts_with("0x") {
            let raw = parse_guid(&name).ok_or_else(|| invalid("invalid raw celebration ID"))?;
            Some(Event::new(raw as u16))
        } else {
            preset(&name)
        };
        if let Some(event) = event.filter(|event| !events.contains(event)) {
            events.push(event);
        }
    }
    Ok(events)
}

struct Challenge {
    event: Event,
    rewards: [u64; 3],
}

fn challenge(name: &str, items: &ItemDb) -> Option<Challenge> {
    let &(_, celebration, indexes) = CHALLENGES.iter().find(|(title, _, _)| *title == name)?;
    let rewards = indexes.map(|index| UNLOCK_BASE | index);
    rewards
        .iter()
        .all(|&guid| items.get(guid).is_some())
        .then_some(Challenge {
            event: Event::keyed(celebration, 0x198),
            rewards,
        })
}

struct ResourceKey {
    name: u64,
    bytes: [u8; KEY_LENGTH],
}

#[derive(Deserialize)]
struct RawKey {
    name: Option<String>,
    key: String,
}

#[derive(Deserialize)]
struct RawMap {
    #[serde(rename = "CelebrationVariants")]
    variants: Option<Vec<RawMapVariant>>,
}

#[derive(Deserialize)]
struct RawMapVariant {
    #[serde(rename = "Virtual01C")]
    kind: String,
    #[serde(rename = "MapInfo")]
    map: VariantMap,
}

#[derive(Deserialize)]
struct VariantMap {
    #[serde(rename = "GUID")]
    guid: String,
}

type MapSwaps = BTreeMap<u64, Vec<(u64, u64)>>;

pub struct Celebrations {
    resource_keys: BTreeMap<u64, ResourceKey>,
    map_swaps: MapSwaps,
    economy: Arc<Economy>,
}

impl Celebrations {
    pub fn load(data_dir: &Path, economy: Arc<Economy>) -> io::Result<Self> {
        Self::from_readers(
            lucy_fixed::RESOURCE_KEYS.as_bytes(),
            BufReader::new(File::open(data_dir.join("extracted_maps.json"))?),
            economy,
        )
    }

    fn from_readers(keys: impl Read, maps: impl Read, economy: Arc<Economy>) -> io::Result<Self> {
        Ok(Self {
            resource_keys: read_keys(keys)?,
            map_swaps: read_map_swaps(maps)?,
            economy,
        })
    }

    /// The loot box a seasonal event gives, from the install.
    fn event_box(&self, event: Event) -> Option<EventBox> {
        if event.kind == 0 {
            return None;
        }
        self.economy
            .event_box(CELEBRATION_TYPE_BASE | u64::from(event.kind))
    }

    /// An event's login rewards: its box, then the rest.
    fn rewards(&self, event: Event) -> impl Iterator<Item = u64> + use<> {
        self.event_box(event)
            .map(|event_box| event_box.unlock)
            .into_iter()
            .chain(
                event
                    .rewards
                    .iter()
                    .map(|&reward| UNLOCK_BASE | u64::from(reward)),
            )
    }

    /// Build 20504, 38900, and 38902 in their relative login order. The
    /// caller places preload between the content keys and the event records.
    pub fn login_messages(
        &self,
        profile: &Profile,
        items: &ItemDb,
        now: u64,
    ) -> io::Result<[LobbyMessage; 3]> {
        let events = active_events(&profile.events)?;
        let challenge = challenge(&profile.challenge, items);
        let keys = self.content_keys(&events, challenge.as_ref());
        let mut records = events
            .iter()
            .map(|&event| self.record_fields(event, now).map(Struct))
            .collect::<io::Result<Vec<_>>>()?;
        let mut progress = Vec::new();
        if let Some(challenge) = challenge {
            let mut record = self.record_fields(challenge.event, now)?;
            let tiers = challenge
                .rewards
                .into_iter()
                .zip(1..)
                .map(|(guid, tier)| {
                    Struct(Fields::from([
                        (0, Array(vec![Unsigned(guid)])),
                        (0x18, Unsigned(CHALLENGE_TIER_WINS * tier)),
                    ]))
                })
                .collect();
            record.insert(0x18, Array(tiers));
            record.insert(0x78, Unsigned(CHALLENGE_WINS_STAT));
            records.push(Struct(record));
            progress.push(Struct(Fields::from([
                (0, Unsigned(challenge.event.guid())),
                (8, Float64(profile.challenge_wins as f64)),
            ])));
        }
        Ok([
            (IN_CONNECT, CONTENT_KEYS, keys),
            (EVENTS, CELEBRATIONS, Fields::from([(0x78, Array(records))])),
            (
                EVENTS,
                CHALLENGE_PROGRESS,
                Fields::from([(0x78, Array(progress))]),
            ),
        ])
    }

    /// Give the login rewards of the events the player was not greeted for:
    /// one event box each, plus any item among the rewards (Lunar New Year's
    /// icon), and a 38901 for each.
    ///
    /// 38901 puts the record (the same as in 38900) on the client's list of
    /// new celebrations (0x7FF7895D0D20). On the main menu the login popups
    /// graph (01B/0C75) takes it (0x7FF789818320) and plays 01B/0C74, which
    /// shows the event's login rewards (+0x58) one by one. The client keeps no
    /// record of it, so the server greets once per event. Retail did this at
    /// the first login of Lunar New Year 2022: the box (24302), the icon
    /// (24301), then 38901. `greet`, `ow174/content/celebrations.py`.
    pub fn greet(
        &self,
        profile: &mut Profile,
        collection: &Collection,
        now: u64,
    ) -> io::Result<Greeting> {
        let ungreeted: Vec<Event> = active_events(&profile.events)?
            .into_iter()
            .filter(|&event| self.event_box(event).is_some() || event.splash)
            .filter(|event| {
                !profile
                    .greeted_events
                    .contains(&u64::from(event.celebration))
            })
            .collect();
        let mut greeting = Greeting::default();
        for &event in &ungreeted {
            let record = self.record_fields(event, now)?;
            greeting.messages.push((
                EVENTS,
                NEW_CELEBRATION,
                Fields::from([(0x78, Struct(record))]),
            ));
        }
        for event in ungreeted {
            if let Some(event_box) = self.event_box(event) {
                greeting
                    .boxes
                    .extend(profile.add_boxes(event_box.box_type, 1)?);
            }
            for guid in self.rewards(event) {
                // The box itself is an unlock too (STUUnlock_LootBox), but not an item.
                if collection.items().get(guid).is_none()
                    || collection.owns(profile, guid)
                    || greeting.given.contains(&guid)
                {
                    continue;
                }
                greeting.given.push(guid);
            }
            profile.greeted_events.push(u64::from(event.celebration));
        }
        profile.unlocked_items.extend(
            greeting
                .given
                .iter()
                .map(|guid| serde_json::Value::String(format!("0x{guid:016X}"))),
        );
        Ok(greeting)
    }

    /// Grant the challenge unlocks the current win count earned that the
    /// profile does not own yet (`claim_rewards`). Ownership is checked
    /// against the profile as it was before any of them.
    pub fn claim_rewards(profile: &mut Profile, collection: &Collection) -> Vec<u64> {
        let Some(challenge) = challenge(&profile.challenge, collection.items()) else {
            return Vec::new();
        };
        let unlocked = profile.unlocked_guids();
        let earned: Vec<u64> = challenge
            .rewards
            .into_iter()
            .zip(1..)
            .filter(|&(guid, tier)| {
                profile.challenge_wins >= (CHALLENGE_TIER_WINS * tier) as i64
                    && !unlocked.contains(&guid)
                    && !collection.owns(profile, guid)
            })
            .map(|(guid, _)| guid)
            .collect();
        profile.unlocked_items.extend(
            earned
                .iter()
                .map(|guid| serde_json::Value::String(format!("0x{guid:016X}"))),
        );
        earned
    }

    fn content_keys(&self, events: &[Event], challenge: Option<&Challenge>) -> Fields {
        let mut names = Vec::new();
        let mut bytes = Vec::new();
        let mut seen = HashSet::new();
        let prioritized = events
            .iter()
            .map(|event| event.key)
            .chain(challenge.map(|challenge| challenge.event.key))
            .filter(|&key| key != 0)
            .filter_map(|key| self.resource_keys.get(&u64::from(key)));
        for key in prioritized.chain(self.resource_keys.values()) {
            if key.name != 0 && seen.insert(key.name) {
                names.push(key.name);
                bytes.extend(key.bytes);
            }
        }
        Fields::from([
            (0x78, Array(names.into_iter().map(Unsigned).collect())),
            (
                0x90,
                Array(
                    bytes
                        .into_iter()
                        .map(|byte| Unsigned(u64::from(byte)))
                        .collect(),
                ),
            ),
        ])
    }

    fn record_fields(&self, event: Event, now: u64) -> io::Result<Fields> {
        let key = u64::from(event.key);
        let (key_name, key_guid) = if key == 0 {
            (0, 0)
        } else {
            (
                self.resource_keys.get(&key).map_or(0, |key| key.name),
                RESOURCE_KEY_BASE | key,
            )
        };
        let kind = u64::from(event.kind);
        let kind_guid = if kind == 0 {
            0
        } else {
            CELEBRATION_TYPE_BASE | kind
        };
        let swaps = self
            .map_swaps
            .get(&kind)
            .into_iter()
            .flatten()
            .map(|&(base, variant)| {
                Struct(Fields::from([(0, Unsigned(base)), (8, Unsigned(variant))]))
            })
            .collect();
        let rewards = self.rewards(event).map(Unsigned).collect();
        let box_type = self
            .event_box(event)
            .map_or(-1, |event_box| i64::from(event_box.box_type));
        let start = now
            .checked_sub(2 * DAY)
            .ok_or_else(|| invalid("invalid celebration start date"))?;
        let stop = now
            .checked_add(365 * DAY)
            .ok_or_else(|| invalid("invalid celebration end date"))?;
        Ok(Fields::from([
            (0, Array(swaps)),
            (0x18, Array(Vec::new())),
            (0x30, Unsigned(key_name)),
            (0x38, Unsigned(key_guid)),
            (0x40, Unsigned(event.guid())),
            (0x48, date(start)?),
            (0x50, date(stop)?),
            (0x58, Array(rewards)),
            (0x70, Unsigned(kind_guid)),
            (0x78, Unsigned(0)),
            (0x80, Array(Vec::new())),
            (0x98, Array(Vec::new())),
            (0xb0, Signed(box_type)),
        ]))
    }
}

fn date(seconds: u64) -> io::Result<Value> {
    Ok(Struct(Fields::from([(
        0,
        Unsigned(stu_datetime(seconds)?),
    )])))
}

fn read_keys(reader: impl Read) -> io::Result<BTreeMap<u64, ResourceKey>> {
    let malformed = || invalid("invalid external resource-key catalog");
    let entries: IndexMap<String, RawKey> =
        serde_json::from_reader(reader).map_err(|_| malformed())?;
    entries
        .into_iter()
        .map(|(index, entry)| {
            let index = hex_id(&index).ok_or_else(malformed)?;
            let name = match entry.name.as_deref() {
                Some(name) if !name.is_empty() => hex_id(name).ok_or_else(malformed)?,
                _ => 0,
            };
            let bytes = key_bytes(&entry.key).ok_or_else(malformed)?;
            Ok((index, ResourceKey { name, bytes }))
        })
        .collect()
}

fn key_bytes(hex: &str) -> Option<[u8; KEY_LENGTH]> {
    let mut bytes = Vec::with_capacity(KEY_LENGTH);
    for part in hex.split_ascii_whitespace() {
        let pairs = part.as_bytes().chunks_exact(2);
        if !pairs.remainder().is_empty() {
            return None;
        }
        for pair in pairs {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            bytes.push((high * 16 + low) as u8);
        }
    }
    bytes.try_into().ok()
}

fn read_map_swaps(reader: impl Read) -> io::Result<MapSwaps> {
    let maps: IndexMap<String, RawMap> =
        serde_json::from_reader(reader).map_err(|_| invalid("invalid celebration map catalog"))?;
    let mut swaps = MapSwaps::new();
    for (guid, map) in maps {
        let base = MAP_BASE | datatool_index(&guid)?;
        for variant in map.variants.into_iter().flatten() {
            let kind = datatool_index(&variant.kind)?;
            let variant = MAP_BASE | datatool_index(&variant.map.guid)?;
            swaps.entry(kind).or_default().push((base, variant));
        }
    }
    Ok(swaps)
}

fn datatool_index(guid: &str) -> io::Result<u64> {
    let index = guid.split_once('.').map_or(guid, |(index, _)| index);
    hex_id(index).ok_or_else(|| invalid("invalid DataTool celebration map GUID"))
}

fn hex_id(text: &str) -> Option<u64> {
    let text = text.trim();
    let text = text.strip_prefix('+').unwrap_or(text);
    if text.starts_with("0x") || text.starts_with("0X") {
        parse_guid(text)
    } else {
        parse_guid(&format!("0x{text}"))
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::clock::server_time;
    use lucy_content::catalog::Unlock;
    use lucy_wire::jam::codec::Schemas;

    const TRACER: &str = "Tracer's Comic Challenge";
    const TRACER_REWARDS: [u64; 3] = [
        UNLOCK_BASE | 0x4aea,
        UNLOCK_BASE | 0x4aeb,
        UNLOCK_BASE | 0x4aec,
    ];

    #[test]
    fn every_listed_event_preset_is_known() {
        for name in EVENT_PRESETS {
            assert!(preset(name).is_some(), "{name}");
        }
        assert!(preset("unknown").is_none());
    }

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|&value| value.into()).collect()
    }

    fn fixture() -> Celebrations {
        let keys = &br#"{
            "198":{"name":"b2","key":"b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2"},
            "1":{"name":"a1","key":"11111111111111111111111111111111"},
            "0x188":{"name":"a1","key":"a1a1a1a1a1a1a1a1 a1a1a1a1a1a1a1a1"},
            "4":{"name":"c3","key":"c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3"},
            "181":{"name":null,"key":"81818181818181818181818181818181"},
            "3":{"name":"9","key":"33333333333333333333333333333333"}
        }"#[..];
        let maps = &br#"{
            "02A.0CE":{"CelebrationVariants":[{"Virtual01C":"237C.01C","MapInfo":{"GUID":"044.0CE"}}]},
            "01.0CE":{"CelebrationVariants":[{"Virtual01C":"237C.01C","MapInfo":{"GUID":"045.0CE"}}]},
            "03.0CE":{"CelebrationVariants":null},
            "04.0CE":{}
        }"#[..];
        Celebrations::from_readers(keys, maps, crate::test_catalog::economy()).unwrap()
    }

    fn tracer_rewards() -> ItemDb {
        let mut items = ItemDb::default();
        for guid in TRACER_REWARDS {
            items.unlocks.insert(
                guid,
                Unlock {
                    guid,
                    name: String::new(),
                    item_type: String::new(),
                    rarity: String::new(),
                    hero: None,
                    source: String::new(),
                    box_type: None,
                    categories: Vec::new(),
                    available_in: String::new(),
                    skin_theme: 0,
                    esports_team: String::new(),
                    level: None,
                },
            );
        }
        items
    }

    fn as_array(value: &Value) -> &[Value] {
        let Array(values) = value else {
            panic!("expected an array")
        };
        values
    }

    fn as_struct(value: &Value) -> &Fields {
        let Struct(fields) = value else {
            panic!("expected a struct")
        };
        fields
    }

    #[test]
    fn prioritizes_event_then_challenge_keys_before_numeric_catalog_order() {
        let celebrations = fixture();
        let now = server_time("2022-09-16", 0).unwrap();
        let profile = Profile {
            events: names(&[" LUNAR ", "lunar"]),
            challenge: TRACER.into(),
            ..Profile::default()
        };
        let items = tracer_rewards();
        let [(_, _, keys), ..] = celebrations.login_messages(&profile, &items, now).unwrap();
        // The event's and challenge's keys first, then the catalog's in id
        // order, each name once (key 3 is named 9).
        assert_eq!(
            keys[&0x78],
            Array([0xa1, 0xb2, 9, 0xc3].map(Unsigned).to_vec())
        );
        assert_eq!(
            keys[&0x90],
            Array(
                [0xa1, 0xb2, 0x33, 0xc3]
                    .into_iter()
                    .flat_map(|byte| std::iter::repeat_n(Unsigned(byte), KEY_LENGTH))
                    .collect()
            )
        );
        assert_eq!(keys.len(), 2);

        let empty = Profile {
            events: Vec::new(),
            challenge: String::new(),
            ..Profile::default()
        };
        let [(_, _, fallback), ..] = celebrations.login_messages(&empty, &items, 0).unwrap();
        assert_eq!(
            fallback[&0x78],
            Array([0xa1, 9, 0xc3, 0xb2].map(Unsigned).to_vec())
        );
        assert_eq!(as_array(&fallback[&0x90])[0], Unsigned(0x11));
        assert_eq!(as_array(&fallback[&0x90])[KEY_LENGTH], Unsigned(0x33));
    }

    #[test]
    fn preserves_event_order_map_swaps_and_challenge_tiers() {
        let celebrations = fixture();
        let now = server_time("2022-09-16", 0).unwrap();
        let profile = Profile {
            events: names(&[
                "lunar",
                "0x105",
                "LUNAR",
                "owl",
                "0x125",
                "ow2_credits",
                "0x123",
                "halloween",
                "unknown",
            ]),
            challenge: TRACER.into(),
            challenge_wins: 17,
            ..Profile::default()
        };
        let mut items = tracer_rewards();
        let [_, (_, _, records), (_, _, progress)] =
            celebrations.login_messages(&profile, &items, now).unwrap();
        let records = as_array(&records[&0x78]);
        assert_eq!(
            records
                .iter()
                .map(|record| as_struct(record)[&0x40].clone())
                .collect::<Vec<_>>(),
            [0x105, 0x105, 0x125, 0x123, 0x123, 0xfb, 0x119]
                .map(|id| Unsigned(CELEBRATION_BASE | id))
        );
        let lunar = as_struct(&records[0]);
        assert_eq!(lunar[&0x30], Unsigned(0xa1));
        assert_eq!(lunar[&0x38], Unsigned(RESOURCE_KEY_BASE | 0x188));
        assert_eq!(lunar[&0x70], Unsigned(CELEBRATION_TYPE_BASE | 0x237c));
        assert_eq!(lunar[&0x48], date(now - 2 * DAY).unwrap());
        assert_eq!(lunar[&0x50], date(now + 365 * DAY).unwrap());
        assert_eq!(lunar[&0xb0], Signed(4));
        assert_eq!(
            lunar[&0],
            Array(
                [(0x2a, 0x44), (1, 0x45)]
                    .map(|(base, variant)| Struct(Fields::from([
                        (0, Unsigned(MAP_BASE | base)),
                        (8, Unsigned(MAP_BASE | variant)),
                    ])))
                    .to_vec()
            )
        );
        assert_eq!(as_struct(&records[1])[&0x30], Unsigned(0));
        assert_eq!(
            as_struct(&records[5])[&0x38],
            Unsigned(RESOURCE_KEY_BASE | 0x181)
        );
        let challenge = as_struct(&records[6]);
        assert_eq!(challenge[&0x78], Unsigned(CHALLENGE_WINS_STAT));
        assert_eq!(
            challenge[&0x18],
            Array(
                TRACER_REWARDS
                    .into_iter()
                    .zip([9, 18, 27])
                    .map(|(guid, wins)| Struct(Fields::from([
                        (0, Array(vec![Unsigned(guid)])),
                        (0x18, Unsigned(wins)),
                    ])))
                    .collect()
            )
        );
        assert_eq!(
            progress[&0x78],
            Array(vec![Struct(Fields::from([
                (0, Unsigned(CELEBRATION_BASE | 0x119)),
                (8, Float64(17.0)),
            ]))])
        );

        items.unlocks.swap_remove(&TRACER_REWARDS[2]);
        let [_, (_, _, records), (_, _, progress)] =
            celebrations.login_messages(&profile, &items, now).unwrap();
        assert_eq!(as_array(&records[&0x78]).len(), 6);
        assert!(as_array(&progress[&0x78]).is_empty());
    }

    #[test]
    fn rejects_malformed_keys_without_echoing_input() {
        let invalid = &br#"{"1":{"name":"1","key":"fictional malformed private input"}}"#[..];
        let error = read_keys(invalid).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "invalid external resource-key catalog");
        let wrong_type = &br#"{"1":{"name":17,"key":"fictional malformed private input"}}"#[..];
        assert_eq!(
            read_keys(wrong_type).err().unwrap().to_string(),
            error.to_string()
        );
    }

    #[test]
    fn greeting_gives_each_new_event_its_box_and_items_once() {
        use crate::test_catalog::{self, LUNAR_ICON};
        let celebrations = fixture();
        let collection = test_catalog::collection();
        let now = server_time("2022-09-16", 0).unwrap();
        let mut profile = Profile {
            events: names(&["lunar", "owl", "halloween", "ow2_credits"]),
            next_box_id: 500,
            loot_boxes: Vec::new(),
            ..Profile::default()
        };
        let greeting = celebrations.greet(&mut profile, &collection, now).unwrap();
        let greeted: Vec<_> = greeting
            .messages
            .iter()
            .map(|(crc, id, fields)| {
                assert_eq!((*crc, *id), (EVENTS, NEW_CELEBRATION));
                as_struct(&fields[&0x78])[&0x40].clone()
            })
            .collect();
        // The OWL lobby has no box and no splash: no greeting.
        assert_eq!(
            greeted,
            [0x105, 0xfb, 0x123].map(|id| Unsigned(CELEBRATION_BASE | id))
        );
        assert_eq!(
            greeting.messages[0].2[&0x78],
            Struct(
                celebrations
                    .record_fields(preset("lunar").unwrap(), now)
                    .unwrap()
            )
        );
        assert_eq!(
            greeting
                .boxes
                .iter()
                .map(|item| (item.id, item.box_type))
                .collect::<Vec<_>>(),
            [(500, 4), (501, 2)]
        );
        // The Lunar box unlock (0D5A) is not an item; the icon is.
        assert_eq!(greeting.given, [LUNAR_ICON]);
        assert_eq!(profile.greeted_events, [0x105, 0xfb, 0x123]);
        assert!(profile.unlocked_guids().contains(&LUNAR_ICON));
        assert_eq!(profile.loot_boxes.len(), 2);

        let again = celebrations.greet(&mut profile, &collection, now).unwrap();
        assert!(again.messages.is_empty() && again.given.is_empty() && again.boxes.is_empty());

        // An owned reward is not given again.
        let mut owner = Profile {
            events: names(&["lunar"]),
            unlocked_items: vec![serde_json::json!(format!("0x{LUNAR_ICON:016X}"))],
            ..Profile::default()
        };
        let greeting = celebrations.greet(&mut owner, &collection, now).unwrap();
        assert_eq!(greeting.messages.len(), 1);
        assert!(greeting.given.is_empty());
    }

    #[test]
    fn challenge_rewards_come_once_each_tier_is_won() {
        let collection = crate::test_catalog::collection();
        let mut profile = Profile {
            challenge: TRACER.into(),
            challenge_wins: 18,
            unlocked_items: vec![serde_json::json!(format!("0x{:016X}", TRACER_REWARDS[0]))],
            ..Profile::default()
        };
        assert_eq!(
            Celebrations::claim_rewards(&mut profile, &collection),
            [TRACER_REWARDS[1]]
        );
        assert!(Celebrations::claim_rewards(&mut profile, &collection).is_empty());
        profile.challenge_wins = 27;
        assert_eq!(
            Celebrations::claim_rewards(&mut profile, &collection),
            [TRACER_REWARDS[2]]
        );
        profile.challenge = String::new();
        profile.unlocked_items.clear();
        assert!(Celebrations::claim_rewards(&mut profile, &collection).is_empty());
    }

    /// The event boxes the install gives are those of
    /// `ow174/catalog/events.py` (`EventDef.box` and the first reward).
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_installs_event_boxes_are_pythons() {
        let data = lucy_testkit::runtime_dir().join("data");
        let economy = Arc::new(Economy::load(&data).unwrap());
        let celebrations = Celebrations::load(&data, economy).unwrap();
        let python: [(&str, i32, &[u16]); 8] = [
            ("lunar", 4, &[0x0d5a, 0x4f50]),
            ("halloween", 2, &[0x0c23]),
            ("winter", 3, &[0x0d0b]),
            ("anniversary", 6, &[0x0ec3]),
            ("anniversary_remix_1", 6, &[0x0ec3]),
            ("anniversary_remix_2", 6, &[0x0ec3]),
            ("summer", 1, &[0x0b2f]),
            ("archives", 5, &[0x0ec2]),
        ];
        for (name, box_type, rewards) in python {
            let event = preset(name).unwrap();
            assert_eq!(
                celebrations
                    .event_box(event)
                    .map(|event_box| event_box.box_type),
                Some(box_type),
                "{name}"
            );
            let rewards: Vec<u64> = rewards
                .iter()
                .map(|&reward| UNLOCK_BASE | u64::from(reward))
                .collect();
            assert_eq!(
                celebrations.rewards(event).collect::<Vec<_>>(),
                rewards,
                "{name}"
            );
        }
        for name in [
            "goodbye",
            "reaper",
            "cassidy",
            "malevento",
            "owl",
            "watchpoint",
        ] {
            assert_eq!(
                celebrations.event_box(preset(name).unwrap()),
                None,
                "{name}"
            );
        }
    }

    /// The event records and challenge progress are Python's (their lengths
    /// and bytes did not depend on the retail capture: the Python digests
    /// covered the capture's 20504 key prefix too, which is gone, so each
    /// message is checked on its own). The content keys are every named
    /// resource key once, the events' and challenge's first.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn login_messages_for_events_challenges_and_content_key_order() {
        let data = lucy_testkit::runtime_dir().join("data");
        let economy = Arc::new(Economy::load(&data).unwrap());
        let celebrations = Celebrations::load(&data, economy).unwrap();
        let items = ItemDb::load(&data).unwrap();
        let schemas = Schemas::lobby();
        let empty = Profile {
            events: Vec::new(),
            server_date: "2022-09-16".into(),
            ..Profile::default()
        };
        let all = Profile {
            events: names(&[
                " LUNAR ",
                "anniversary",
                "LUNAR",
                "0x105",
                "halloween",
                "winter",
                "anniversary_remix_1",
                "anniversary_remix_2",
                "summer",
                "archives",
                "reaper",
                "cassidy",
                "malevento",
                "owl",
                "watchpoint",
                "ow2_credits",
                "0x123",
                "goodbye",
                "unknown",
            ]),
            challenge: TRACER.into(),
            challenge_wins: 17,
            ..empty.clone()
        };
        let kana = Profile {
            events: names(&["owl", "0x125", "0x10000", "winter"]),
            challenge: "Kanezaka Challenge".into(),
            challenge_wins: 27,
            server_date: "2020-02-29".into(),
            ..empty.clone()
        };
        let named: HashSet<_> = celebrations
            .resource_keys
            .values()
            .map(|key| key.name)
            .filter(|&name| name != 0)
            .collect();
        for (profile, lengths) in [(empty, [4, 4]), (all, [1742, 20]), (kana, [448, 20])] {
            let now = server_time(&profile.server_date, 0).unwrap();
            let [(_, _, keys), records, progress] =
                celebrations.login_messages(&profile, &items, now).unwrap();
            assert_eq!(as_array(&keys[&0x78]).len(), named.len());
            assert_eq!(as_array(&keys[&0x90]).len(), named.len() * KEY_LENGTH);
            for ((crc, id, fields), length) in [records, progress].into_iter().zip(lengths) {
                assert_eq!(schemas.encode(crc, id, &fields).unwrap().len(), length);
            }
            schemas.encode(IN_CONNECT, CONTENT_KEYS, &keys).unwrap();
        }
    }
}
