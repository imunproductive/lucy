//! The messages the lobby builds match the snapshot of the Python reference
//! (`tests/test_content_golden.py`, its set copied to
//! `crates/lobby/tests/golden/messages.json`): one hash per message for a few
//! fixed profiles. A hash covers a message's value as the Python builders hold
//! it before encoding, so each Rust message is first written as the same
//! canonical JSON (`canonical` and `digest` there).
//!
//! The Python server built some messages from the captured retail traffic;
//! ours builds them from the install or its own constants (`templates.rs`),
//! and their own tests check them: for those ([`REBUILT`]) the snapshot
//! checks only that they go out where Python sent them. The captured
//! messages the server no longer sends ([`DROPPED`]) are taken out of the
//! snapshot, and those it sends fewer copies of ([`COPIES`]) cut.
//!
//! It sits under `menu` so the random menu hero can roll the first catalog
//! hero, as the Python test's patched `random.choice` does.

use std::fmt::Write as _;

use serde_json::{Map, Value as Json, json};
use sha2::{Digest, Sha256};

use super::*;
use crate::{
    accounts::Accounts,
    arcade::Arcade,
    celebrations::Celebrations,
    clock::server_time,
    collection::Collection,
    login::Identity,
    mode_rules::{CareerContent, career_profile},
    party::{self, Membership, PartyMember},
    player_messages as player,
    presence::Presence,
    ranked::{LeaderboardPlaces, Ranked},
    session::{LoginContent, PreparedLogin},
    templates::{InstallContent, LOBBY},
    transport::LobbyMessage,
};
use lucy_wire::jam::codec::{Fields, Schemas, Value};

const GOLDEN: &str = include_str!("../../tests/golden/messages.json");
/// The Python test's patched `time.time`.
const NOW: u64 = 1_800_000_000;
const IDENTITY: Identity = Identity {
    account_lo: 0x1C2A_69FB,
    session: [7, 1],
    party_id: [11, 12],
    party_entity: [13, 14],
};
/// Parts of the snapshot that belonged to Python code removed as dead.
const REMOVED_PARTS: [&str; 2] = ["name_reply", "notifications"];
/// Parts built from the install's catalog, which the Python snapshot took
/// from the capture: the owned items and the menu config.
const REBUILT_PARTS: [&str; 3] = ["owned_account", "owned_heroes", "menu_config"];
/// Messages built from the install or our constants instead of the
/// capture, by group and id: the content keys (20504), preload (20505),
/// progression and hero catalog (24300, 24900), mode rules (27202), the
/// Arcade's state value (39800: `+0x7C`, which the client doesn't read, is
/// 0) and busyness (39804), achievements (28001), config (36600), replay
/// hosts (51804), store (26400), ranked state (36300), the custom game
/// maps (23301: the capture's maps, in header order), the career profile
/// (20807, which holds the 24300 and 24900 records), and the event records
/// (38900, 38901), whose content key names (`+0x30`) are the install's
/// (`resource_keys.json`) where Python had TACTLib's keyring names
/// (`celebrations` tests check them against 20504).
const REBUILT: [(u32, u32); 16] = [
    (0xf5ff_548a, 20504),
    (0xf5ff_548a, 20505),
    (0x779a_8581, 24300),
    (0x70e5_a247, 24900),
    (0xb9dc_a497, 27202),
    (0x00a8_654e, 39800),
    (0x00a8_654e, 39804),
    (0x267f_de9e, 28001),
    (0x1732_ebf2, 36600),
    (0x5894_d085, 51804),
    (0x4bad_7a7e, 26400),
    (0x2372_a2e8, 36300),
    (0x2128_6b49, 23301),
    (0x1a98_79a4, 20807),
    (0x8b77_c823, 38900),
    (0x8b77_c823, 38901),
];
/// Captured messages the server doesn't send: anti-cheat payloads (20820)
/// and Battle.net's profanity filter (27104).
const DROPPED: [(u32, u32); 2] = [(0x1a98_79a4, 20820), (0xf470_e2ab, 27104)];
/// Captured messages the capture held more copies of than the server sends:
/// the OWL state (43200) once of each kind, the store (26400) once.
const COPIES: [((u32, u32), usize); 2] = [((0x713d_7589, 43200), 2), ((0x4bad_7a7e, 26400), 1)];

/// The Python test's profiles (`SCENARIOS`), each over the default profile.
fn scenarios() -> Vec<(&'static str, Json)> {
    vec![
        ("default", json!({"lobby_hero": "Genji"})),
        (
            "unlock_all_eu",
            json!({"lobby_hero": "Reaper", "unlock_all": true, "region": "EU", "level": 2500}),
        ),
        (
            "npc_gb",
            json!({"lobby_hero": "Talon Sniper", "region": "GB", "endorsement_level": 4}),
        ),
        (
            "events_challenge",
            json!({
                "lobby_hero": "Tracer",
                "events": ["anniversary"],
                "challenge": "Tracer's Comic Challenge",
                "challenge_wins": 20,
                "server_date": "2022-10-03",
            }),
        ),
        (
            "granted",
            json!({
                "lobby_hero": "none",
                "unlocked_items": ["0x0250000000001149", "0x0250000000000921"],
                "icon_guid": 0x0250_0000_0000_1149_u64,
                "loadouts": {"0x02E0000000000002": {"+0x38": "0x0250000000000921"}},
            }),
        ),
    ]
}

/// Python `json.dumps(canonical(value), sort_keys=True, ensure_ascii=False,
/// separators=(",", ":"))`: struct keys as `+0x..` offsets in string order,
/// blobs as `{"bytes": hex}`, floats as Python's `repr`.
fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Unsigned(number) => write!(out, "{number}").unwrap(),
        Value::Signed(number) => write!(out, "{number}").unwrap(),
        // Python holds the decimal it encodes into a 32-bit field (0.1),
        // which is the shortest decimal that reads back as the f32.
        Value::Float32(number) => write_float(out, number.to_string().parse().unwrap()),
        Value::Float64(number) => write_float(out, *number),
        Value::String(text) => write_string(out, text),
        Value::Blob(bytes) => {
            out.push_str(r#"{"bytes":""#);
            for byte in bytes {
                write!(out, "{byte:02x}").unwrap();
            }
            out.push_str(r#""}"#);
        }
        Value::Struct(fields) => write_fields(out, fields),
        Value::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, value);
            }
            out.push(']');
        }
    }
}

fn write_fields(out: &mut String, fields: &Fields) {
    let mut keyed: Vec<_> = fields
        .iter()
        .map(|(offset, value)| (format!("+0x{offset:X}"), value))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    out.push('{');
    for (index, (key, value)) in keyed.into_iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_string(out, &key);
        out.push(':');
        write_value(out, value);
    }
    out.push('}');
}

/// Python's float `repr`: positional between 1e-4 and 1e16, else a
/// mantissa and a signed exponent of at least two digits.
fn write_float(out: &mut String, number: f64) {
    if number.is_nan() {
        out.push_str("NaN");
    } else if number.is_infinite() {
        out.push_str(if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        });
    } else if number == 0.0 || (1e-4..1e16).contains(&number.abs()) {
        let text = number.to_string();
        out.push_str(&text);
        if !text.contains('.') {
            out.push_str(".0");
        }
    } else {
        let text = format!("{number:e}");
        let (mantissa, exponent) = text.split_once('e').unwrap();
        let (sign, digits) = exponent
            .strip_prefix('-')
            .map_or(('+', exponent), |digits| ('-', digits));
        write!(out, "{mantissa}e{sign}{digits:0>2}").unwrap();
    }
}

/// A JSON string as Python writes it with `ensure_ascii=False`.
fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if u32::from(control) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(control)).unwrap();
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The first 20 hex digits of the SHA-256 of the canonical JSON (`digest`).
fn digest(value: &Value) -> String {
    let mut text = String::new();
    write_value(&mut text, value);
    Sha256::digest(text.as_bytes())[..10]
        .iter()
        .fold(String::new(), |mut hex, byte| {
            write!(hex, "{byte:02x}").unwrap();
            hex
        })
}

/// `[crc, id, digest]` for each message (`summarize`).
fn summarize(messages: &[LobbyMessage]) -> Json {
    messages
        .iter()
        .map(|(crc, id, fields)| {
            json!([
                format!("{crc:08X}"),
                id,
                digest(&Value::Struct(fields.clone()))
            ])
        })
        .collect()
}

/// A part that holds one message the Python test summarizes with crc and id
/// 0.
fn single(fields: Fields) -> Json {
    summarize(&[(0, 0, fields)])
}

fn guids(guids: impl IntoIterator<Item = u64>) -> Value {
    Value::Array(guids.into_iter().map(Value::Unsigned).collect())
}

/// The golden part with the party state Python's `Content` puts after 38902
/// moved to where the session sends its own (`without_party_state` and
/// `party_messages`, `ow174/lobby/session.py`), and then `extra`.
fn session_order(part: &Json, party: &Json, extra: Option<&Json>) -> Json {
    let mut messages: Vec<Json> = part
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message[1] != 20700)
        .cloned()
        .collect();
    let crc = format!("{:08X}", party::PARTY);
    messages.push(json!([crc, 20700, party[0][2]]));
    if let Some((crc, id, digest)) = extra.map(|extra| (LOBBY, 20802, &extra[0][2])) {
        messages.push(json!([format!("{crc:08X}"), id, digest]));
    }
    Json::Array(messages)
}

/// The live part with the leave policy (20813) the Rust session sends after
/// the level (24313) since a match's result can change it
/// (`progression.rs`); a profile without leaves sends the captured one, as
/// the golden login has it.
fn with_leave_policy(live: Json, login: &Json) -> Json {
    let lobby = format!("{LOBBY:08X}");
    let policy = login
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message[0] == lobby.as_str() && message[1] == 20813)
        .unwrap()
        .clone();
    let mut messages = live.as_array().unwrap().clone();
    let level = messages
        .iter()
        .position(|message| message[1] == 24313)
        .unwrap();
    messages.insert(level + 1, policy);
    Json::Array(messages)
}

/// A summarized message's group and id.
fn key(message: &Json) -> Option<(u32, u32)> {
    let crc = u32::from_str_radix(message[0].as_str()?, 16).ok()?;
    Some((crc, u32::try_from(message[1].as_u64()?).ok()?))
}

/// A golden part without the captured messages the server doesn't send
/// ([`DROPPED`]), and with as many copies of the others as it sends
/// ([`COPIES`]).
fn without_captured(part: &Json) -> Json {
    let mut sent: Vec<(u32, u32)> = Vec::new();
    let messages = part
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            let Some(key) = key(message) else {
                return true;
            };
            if DROPPED.contains(&key) {
                return false;
            }
            sent.push(key);
            let count = sent.iter().filter(|&&seen| seen == key).count();
            COPIES
                .iter()
                .find(|(copied, _)| *copied == key)
                .is_none_or(|&(_, copies)| count <= copies)
        })
        .cloned()
        .collect();
    Json::Array(messages)
}

struct Content {
    schemas: Schemas,
    install: InstallContent,
    collection: Collection,
    presence: Presence,
    ranked: Ranked,
    arcade: Arcade,
    celebrations: Celebrations,
    extra_preload: Vec<u64>,
    accounts: Accounts,
}

impl Content {
    /// 20802 as Python sent it: without the Kill Feed Display default
    /// (`player::settings`), block +0x120 the schema's empty one. The golden
    /// hashes are Python's; the rest of the message must still match them.
    fn settings_as_python_sent(&self, mut fields: Fields) -> Fields {
        let mut empty = self.schemas.empty(LOBBY, 20802).unwrap();
        fields.insert(0x120, empty.remove(&0x120).unwrap());
        fields
    }

    /// Every part the Python test builds (`build_all`); `login` and `live`
    /// in the order the session sends them.
    fn build_all(&self, profile: &Profile) -> Map<String, Json> {
        let collection = &self.collection;
        let places = LeaderboardPlaces::new();
        let selection = MenuHero::default()
            .select_with(profile, collection.heroes(), collection.items(), |_| Ok(0))
            .unwrap();
        let ratings = self.ranked.party_ratings(profile, &places).unwrap();
        let party = party::state(
            collection,
            IDENTITY.party_id,
            IDENTITY.party_entity,
            &[PartyMember {
                profile,
                identity: IDENTITY,
                menu_hero: selection.party_hero,
                ratings: &ratings,
                membership: Membership::Leader,
            }],
            true,
            NOW,
        )
        .unwrap();
        let account = self.accounts.get("player").unwrap();
        *account.profile() = profile.clone();
        let prepared = PreparedLogin {
            account,
            identity: IDENTITY,
            tournament: false,
        };
        let content = LoginContent {
            menu_hero: selection.menu_guid,
            dev_console: false,
            extra_preload: &self.extra_preload,
            wall_time: NOW,
            arcade: &self.arcade,
            celebrations: Some(&self.celebrations),
            leaderboard_places: &places,
            party: std::slice::from_ref(&party),
        };
        let mut login = prepared
            .available_messages(
                &self.schemas,
                &self.install,
                collection,
                &self.presence,
                &self.ranked,
                content,
            )
            .unwrap();
        for (_, id, fields) in &mut login {
            if *id == 20802 {
                *fields = self.settings_as_python_sent(fields.clone());
            }
        }
        let live = prepared
            .live_messages(collection, &self.ranked, content)
            .unwrap();
        let now = server_time(&profile.server_date, NOW).unwrap();
        let season = self.ranked.season(profile);
        let event_cards = self
            .arcade
            .state(profile, season, now)
            .unwrap()
            .competitive_cards;
        let career = |target: Fields| {
            let content = CareerContent {
                collection,
                ranked: &self.ranked,
                event_cards: &event_cards,
                places: &places,
                now: NOW,
            };
            summarize(&career_profile(profile, IDENTITY, &target, None, content).unwrap())
        };
        let own = Fields::from([
            (0, Value::Unsigned(IDENTITY.account_lo.into())),
            (8, Value::Unsigned(0x0100_0000_0000_0000)),
        ]);
        let other = Fields::from([(0, Value::Unsigned(5)), (8, Value::Unsigned(1))]);
        let greetings = self
            .celebrations
            .greet(&mut profile.clone(), collection, now)
            .unwrap()
            .messages;
        let owned_heroes = Value::Array(
            collection
                .heroes()
                .iter()
                .map(|&hero| guids(collection.owned_for_hero(profile, hero)))
                .collect(),
        );
        Map::from_iter([
            ("login".into(), summarize(&login)),
            ("live".into(), summarize(&live)),
            (
                "menu_config".into(),
                single(crate::templates::menu_config(selection.menu_guid, false)),
            ),
            ("career".into(), career(own)),
            ("career_other".into(), career(other)),
            ("greetings".into(), summarize(&greetings)),
            ("party".into(), single(party.2.clone())),
            (
                "settings".into(),
                single(
                    self.settings_as_python_sent(player::settings(&self.schemas, profile).unwrap()),
                ),
            ),
            (
                "summary".into(),
                single(player::summary(
                    profile,
                    IDENTITY,
                    collection.portrait_frame(profile),
                    NOW,
                )),
            ),
            (
                "owned_account".into(),
                json!([digest(&guids(collection.owned_account(profile)))]),
            ),
            ("owned_heroes".into(), json!([digest(&owned_heroes)])),
        ])
    }
}

/// `tests/test_content_golden.py::GoldenMessageTests::test_messages_are_unchanged`.
///
/// The Rust session builds `login` and `live` with the party state already
/// where the Python session moves it (after 35500, and last), and `login`
/// ends with the settings (20802) the Python login handler adds; their golden
/// lists are put in that order, with the golden `party` and `settings`
/// hashes. `live` also has the leave policy (`with_leave_policy`).
#[test]
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn messages_are_unchanged() {
    let data = lucy_testkit::runtime_dir().join("data");
    let temporary = tempfile::tempdir().unwrap();
    let content = Content {
        schemas: Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap(),
        install: InstallContent::load(&data).unwrap(),
        collection: Collection::load(&data).unwrap(),
        presence: Presence,
        ranked: Ranked::load(&data).unwrap(),
        arcade: Arcade::load(data.join("arcade_cards_174.json")).unwrap(),
        celebrations: Celebrations::load(
            &data,
            std::sync::Arc::new(crate::economy::Economy::load(&data).unwrap()),
        )
        .unwrap(),
        extra_preload: Vec::new(),
        accounts: Accounts::new(
            temporary.path().join("profiles"),
            temporary.path().join("seed"),
        )
        .unwrap(),
    };
    let golden: Map<String, Json> = serde_json::from_str(GOLDEN).unwrap();
    let scenarios = scenarios();
    let mut names: Vec<_> = scenarios.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    let mut expected_names: Vec<_> = golden.keys().map(String::as_str).collect();
    expected_names.sort_unstable();
    assert_eq!(names, expected_names);
    let mut changed = Vec::new();
    for (name, fields) in scenarios {
        let profile: Profile = serde_json::from_value(fields).unwrap();
        let actual = content.build_all(&profile);
        let mut expected = golden[name].as_object().unwrap().clone();
        expected.retain(|part, _| !REMOVED_PARTS.contains(&part.as_str()));
        for part in ["login", "live"] {
            let sent = without_captured(&expected[part]);
            expected.insert(part.into(), sent);
        }
        let party = expected["party"].clone();
        let settings = expected["settings"].clone();
        let login = session_order(&expected["login"], &party, Some(&settings));
        let live = with_leave_policy(
            session_order(&expected["live"], &party, None),
            &expected["login"],
        );
        expected.insert("login".into(), login);
        expected.insert("live".into(), live);
        let mut parts: Vec<_> = actual.keys().collect();
        parts.sort_unstable();
        let mut expected_parts: Vec<_> = expected.keys().collect();
        expected_parts.sort_unstable();
        assert_eq!(parts, expected_parts, "{name}");
        for (part, digests) in &expected {
            if REBUILT_PARTS.contains(&part.as_str()) {
                continue;
            }
            let (want, got) = (
                digests.as_array().unwrap(),
                actual[part].as_array().unwrap(),
            );
            if want.len() != got.len() {
                changed.push(format!(
                    "{name}/{part}: {} entries instead of {}",
                    got.len(),
                    want.len()
                ));
            }
            for (index, (want, got)) in want.iter().zip(got).enumerate() {
                let rebuilt = key(want).is_some_and(|key| REBUILT.contains(&key));
                let same = if rebuilt {
                    want[0] == got[0] && want[1] == got[1]
                } else {
                    want == got
                };
                if !same {
                    changed.push(format!("{name}/{part}[{index}]: {got} instead of {want}"));
                }
            }
        }
    }
    assert!(changed.is_empty(), "changed:\n{}", changed.join("\n"));
}

/// The JSON the hashes cover, as `canonical` and `digest` in
/// `tests/test_content_golden.py` write it.
#[test]
fn canonical_json_is_written_like_python() {
    let mut text = String::new();
    write_value(
        &mut text,
        &Value::Struct(Fields::from([
            (0x100, Value::Float64(15.0)),
            (0x78, Value::String("a\"\n\u{1}é".into())),
            (0x8, Value::Blob(vec![0, 0xab])),
            (
                0x0,
                Value::Array(vec![
                    Value::Bool(true),
                    Value::Signed(-3),
                    Value::Float64(1e-5),
                    Value::Float64(1e16),
                    Value::Float32(0.1),
                ]),
            ),
        ])),
    );
    // json.dumps(canonical({"+0x100": 15.0, "+0x78": "a\"\n\x01é",
    // "+0x8": b"\x00\xab", "+0x0": [True, -3, 1e-05, 1e16, 0.1]}), ...)
    assert_eq!(
        text,
        r#"{"+0x0":[true,-3,1e-05,1e+16,0.1],"+0x100":15.0,"+0x78":"a\"\n\u0001é","+0x8":{"bytes":"00ab"}}"#
    );
}
