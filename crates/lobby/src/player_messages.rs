//! The player's initial lobby messages and saved settings.
//!
//! Client research (1.74):
//!
//! - Account features (55500, right after the endorsements): the client looks
//!   a feature up by id and treats it as enabled when it is present with a
//!   non-zero value. 0x948D allows sending chat and whispers. The client drops
//!   the connection right after login if the list is incomplete or the value
//!   is 1. The ids and the value are copied from a retail 1.68 capture.
//! - The endorsement categories are STUIdentifier 01C, from 054/0x168 in the
//!   client data: sportsmanship (green), good teammate (purple), shot caller
//!   (orange). The ring around the level shows each category's share in its
//!   colour, and stays grey when the list is empty.
//! - 20802 +0x130 holds values `[{+0x0 value, +0x8 key}]`, a key being
//!   `(type table << 16) | identifier` (0x7FF7893512B0).
//! - A friend's card (20809/20810) goes into the client's card cache
//!   (0x7FF789687D80), so the friend shows an icon, level and endorsement;
//!   from presence alone it has only the BattleTag. The id, time and two flags
//!   after the endorsement are not known; left empty, the card shows and no
//!   other tab lists the friend (tested in game).
//! - 20812 interface states are kept by the client as index -> value
//!   (0x7FF789535EB0); a missing one reads as 0.

use std::{collections::BTreeMap, io};

use crate::{
    accounts::{Profile, battle_tag},
    login::Identity,
    templates::{LOBBY, restore_fields, restore_value},
    transport::LobbyMessage,
};
use lucy_content::catalog::{game_region_number, region_of};
use lucy_wire::jam::codec::{
    Fields, Schemas,
    Value::{self, Array, Bool, Float32, Signed, Struct, Unsigned},
    id16,
};

pub const ENDORSEMENTS: u32 = 0x79bf_ddbe;
pub const PERMISSIONS: u32 = 0xbcd5_7a46;

const ACCOUNT_FEATURES: [u64; 8] = [
    0x0d80_0000_0000_55ed,
    0x0d80_0000_0000_55ee,
    0x0d80_0000_0000_55eb,
    0x0d80_0000_0000_55ec,
    0x0d80_0000_0000_55f9,
    0x0d80_0000_0000_56f2,
    0x0d80_0000_0000_56f3,
    0x0d80_0000_0000_948d,
];
const ACCOUNT_FEATURE_VALUE: u64 = 0xff00_0000_0000_0006;

const SAVED_SETTINGS: [&str; 5] = ["+0x78", "+0x108", "+0x10D", "+0x120", "+0x130"];

const ENDORSEMENT_CATEGORIES: [(u64, u64); 3] = [
    (0x0d80_0000_0000_3946, 40),
    (0x0d80_0000_0000_3945, 35),
    (0x0d80_0000_0000_3944, 25),
];

pub fn hello(profile: &Profile, identity: Identity) -> Fields {
    let account = identity.account();
    let tag = battle_tag(&profile.player_name, identity.account_lo);
    let session = identity.session.into_iter().map(Unsigned).collect();
    let game_region = u64::from(game_region_number(&profile.game_region));
    // The six bools of 20500 nobody knows the meaning of (`+0x148`,
    // `+0x149`, `+0x14A`, `+0x154`, `+0x155`, `+0x156`) stay false, as the
    // Python reference left them. A lead for the client's hidden admin and
    // developer features (`refs/reports/AdminMode.md`).
    Fields::from([
        (0x78, Struct(account.clone())),
        (0x88, Struct(account)),
        (0x98, Struct(Fields::from([(0, Array(session))]))),
        (0xa8, Value::String(tag.clone())),
        (0xd0, Value::String(tag)),
        (
            0xf8,
            Value::String(region_of(&profile.region).country.into()),
        ),
        (0x120, Value::String(String::new())),
        (0x14c, Unsigned(game_region)),
        (0x150, Unsigned(game_region)),
    ])
}

pub fn record(profile: &Profile, identity: Identity, portrait_frame: u64, now: u64) -> Fields {
    let account = identity.account();
    Fields::from([
        (0x00, Struct(account.clone())),
        (0x10, Struct(account)),
        (0x20, Unsigned(profile.icon_guid)),
        (0x28, Unsigned(portrait_frame)),
        (0x30, Unsigned(now)),
        (0x38, Signed(profile.level)),
        (0x3c, Unsigned(1)),
        (
            0x40,
            Value::String(battle_tag(&profile.player_name, identity.account_lo)),
        ),
    ])
}

pub fn friend_card(profile: &Profile, identity: Identity, portrait_frame: u64, now: u64) -> Fields {
    Fields::from([
        (0x00, Struct(record(profile, identity, portrait_frame, now))),
        (0x68, Struct(endorsement(profile.endorsement_level))),
        (0x88, Struct(id16(0, 0))),
        (0x98, Unsigned(0)),
        (0xa0, Bool(false)),
        (0xa1, Bool(false)),
    ])
}

/// 39002: the card other players see (name, icon, frame, level, endorsement).
/// `summary`, `ow174/content/player.py`.
pub fn summary(profile: &Profile, identity: Identity, portrait_frame: u64, now: u64) -> Fields {
    Fields::from([
        (0x78, Struct(identity.account())),
        (
            0x88,
            Struct(summary_card(profile, identity, portrait_frame, now)),
        ),
    ])
}

/// The card of a 39002 summary, also a leaderboard row.
pub fn summary_card(
    profile: &Profile,
    identity: Identity,
    portrait_frame: u64,
    now: u64,
) -> Fields {
    Fields::from([
        (0x00, Struct(record(profile, identity, portrait_frame, now))),
        (0x68, Struct(Fields::new())),
        (0x80, Struct(endorsement(profile.endorsement_level))),
        (0xa0, Array(Vec::new())),
        (0xb8, Value::Float64(0.0)),
        (0xc0, Array(Vec::new())),
        (0xd8, Array(Vec::new())),
    ])
}

pub fn endorsement(level: i64) -> Fields {
    let counts = ENDORSEMENT_CATEGORIES
        .into_iter()
        .map(|(category, count)| {
            Struct(Fields::from([
                (0, Unsigned(category)),
                (8, Unsigned(count)),
            ]))
        })
        .collect();
    Fields::from([(0, Array(counts)), (0x18, Signed(level))])
}

pub fn endorsements(profile: &Profile, identity: Identity, now: u64) -> [LobbyMessage; 3] {
    let level = endorsement(profile.endorsement_level);
    [
        (
            ENDORSEMENTS,
            52002,
            Fields::from([
                (0x78, Struct(identity.account())),
                (0x88, Struct(level.clone())),
                (0xa8, Signed(1)),
                (0xac, Float32(15.0)),
                (0xb0, Unsigned(1)),
                (0xb4, Unsigned(0)),
            ]),
        ),
        (
            ENDORSEMENTS,
            52005,
            Fields::from([
                (0x78, Struct(level)),
                (0x98, Signed(1)),
                (0x9c, Float32(15.0)),
                (0xa0, Unsigned(1)),
                (0xa4, Unsigned(0)),
                (0xa8, Unsigned(1)),
                (0xb0, Unsigned(now)),
                (0xb8, Unsigned(0)),
                (0xbc, Unsigned(0)),
            ]),
        ),
        (
            ENDORSEMENTS,
            52006,
            Fields::from([(0x78, Array(Vec::new()))]),
        ),
    ]
}

pub fn ux_states(profile: &Profile) -> io::Result<Fields> {
    let mut states = BTreeMap::from([(0u8, Unsigned(2)), (4, Unsigned(0))]);
    for (index, value) in &profile.ux_states {
        let index = index.trim().parse::<u8>().map_err(invalid)?;
        states.insert(index, restore_value(value.clone())?);
    }
    let states = states
        .into_iter()
        .map(|(index, value)| Struct(Fields::from([(0, value), (4, Unsigned(u64::from(index)))])))
        .collect();
    Ok(Fields::from([(0x78, Array(states))]))
}

pub fn settings(schemas: &Schemas, profile: &Profile) -> io::Result<Fields> {
    let saved = &profile.settings;
    let blocks = SAVED_SETTINGS
        .into_iter()
        .filter_map(|key| {
            saved
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| (key.into(), value.clone()))
        })
        .collect();
    let mut fields = schemas.empty(LOBBY, 20802).map_err(invalid)?;
    fields.extend(restore_fields(blocks)?);
    if !saved.contains_key("+0x10D") {
        let social = settings_block(&mut fields, 0x10d)?;
        social.insert(0, Bool(true)); // Party members may invite.
        social.insert(5, Unsigned(1)); // Receive whispers from anyone.
    }
    if !saved.contains_key("+0x108") {
        settings_block(&mut fields, 0x108)?.insert(4, Bool(true));
    }
    // Options > Gameplay "Kill Feed Display" (`+0x4`; `+0x5` Always Skip
    // Kill Cam, `+0x7` Tips Display). The client copies it into account
    // settings system 9 `+700`, and the kill feed panel of graph `0DEA`
    // shows only while it is on (#1962 = `STU_016D65A8` = system 9 `+700`).
    // The client's own Options defaults (`0x7FF7899B8650`) have Kill Feed
    // and Tips Display on, but system 9 holds only what 20802 sends, so we
    // send those defaults (upstream kjs `1819125b`). Python sent none;
    // `research/lizrollin-runtime.md:446-461`, `research/lizrollin-lobby.md:84`.
    if !saved.contains_key("+0x120") {
        let gameplay = settings_block(&mut fields, 0x120)?;
        gameplay.insert(4, Bool(true));
        gameplay.insert(7, Bool(true));
    }
    Ok(fields)
}

fn settings_block(fields: &mut Fields, offset: u32) -> io::Result<&mut Fields> {
    if let Some(Struct(block)) = fields.get_mut(&offset) {
        Ok(block)
    } else {
        Err(invalid(format!(
            "missing lobby settings block +0x{offset:X}"
        )))
    }
}

fn invalid(message: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

pub fn features() -> Fields {
    let features = ACCOUNT_FEATURES
        .into_iter()
        .map(|feature| {
            Struct(Fields::from([
                (0x00, Unsigned(feature)),
                (0x08, Unsigned(0)),
                (0x10, Unsigned(ACCOUNT_FEATURE_VALUE)),
            ]))
        })
        .collect();
    Fields::from([(0x78, Struct(Fields::from([(0, Array(features))])))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::IN_CONNECT;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    #[test]
    fn hello_uses_account_session_nickname_and_region() {
        let profile = Profile {
            player_name: "Local".into(),
            region: "GB".into(),
            game_region: "europe".into(),
            ..Profile::default()
        };
        let identity = Identity::for_account(42);
        let fields = hello(&profile, identity);
        assert_eq!(fields[&0x78], Struct(identity.account()));
        assert_eq!(fields[&0x88], fields[&0x78]);
        assert_eq!(
            fields[&0x98],
            Struct(Fields::from([(0, Array(vec![Unsigned(42), Unsigned(1)]))]))
        );
        assert_eq!(fields[&0xa8], Value::String("Local#1042".into()));
        assert_eq!(fields[&0xf8], Value::String("GBR".into()));
        assert_eq!(fields[&0x14c], Unsigned(2));
        assert_eq!(fields[&0x150], Unsigned(2));
    }

    #[test]
    fn saved_settings_override_only_selected_blocks() {
        let schemas = Schemas::from_json(
            r#"{"1A9879A4":{"20802":[
                {"type":14,"off":264,"count":0,"array":false,"fields":[
                    {"type":0,"off":0,"count":0,"array":false},
                    {"type":0,"off":4,"count":0,"array":false}
                ]},
                {"type":14,"off":269,"count":0,"array":false,"fields":[
                    {"type":0,"off":0,"count":0,"array":false},
                    {"type":2,"off":5,"count":0,"array":false}
                ]},
                {"type":14,"off":288,"count":0,"array":false,"fields":[
                    {"type":6,"off":0,"count":0,"array":false},
                    {"type":0,"off":4,"count":0,"array":false},
                    {"type":0,"off":5,"count":0,"array":false},
                    {"type":0,"off":7,"count":0,"array":false}
                ]}
            ]}}"#,
        )
        .unwrap();
        let mut profile = Profile::default();
        let defaults = settings(&schemas, &profile).unwrap();
        let Value::Struct(social) = &defaults[&0x10d] else {
            panic!("missing social settings")
        };
        assert_eq!(social[&0], Bool(true));
        assert_eq!(social[&5], Unsigned(1));
        let Value::Struct(flags) = &defaults[&0x108] else {
            panic!("missing flags")
        };
        assert_eq!(flags[&4], Bool(true));
        let Value::Struct(gameplay) = &defaults[&0x120] else {
            panic!("missing gameplay options")
        };
        // The client's own defaults: Kill Feed and Tips Display on; Always
        // Skip Kill Cam stays the schema's off.
        assert_eq!(gameplay[&4], Bool(true));
        assert_eq!(gameplay[&5], Bool(false));
        assert_eq!(gameplay[&7], Bool(true));

        profile
            .settings
            .insert("+0x10D".into(), json!({"+0x0": false, "+0x5": 2}));
        profile
            .settings
            .insert("+0x120".into(), json!({"+0x4": false, "+0x5": true}));
        profile
            .settings
            .insert("ignore_me".into(), json!("not a protocol field"));
        let loaded = settings(&schemas, &profile).unwrap();
        assert_eq!(
            loaded[&0x10d],
            Struct(Fields::from([(0, Bool(false)), (5, Unsigned(2))]))
        );
        assert_eq!(loaded[&0x108], defaults[&0x108]);
        let Value::Struct(gameplay) = &loaded[&0x120] else {
            panic!("missing gameplay options")
        };
        assert_eq!(gameplay[&4], Bool(false));
        assert_eq!(gameplay[&5], Bool(true));
    }

    #[test]
    fn saved_ux_values_replace_defaults_and_use_wire_index_order() {
        let mut profile = Profile::default();
        profile.ux_states.insert("10".into(), json!(-3));
        profile.ux_states.insert("0".into(), json!(9));
        profile.ux_states.insert("2".into(), json!(1));
        let fields = ux_states(&profile).unwrap();
        let Value::Array(values) = &fields[&0x78] else {
            panic!("missing UX array")
        };
        assert_eq!(
            values,
            &[
                Struct(Fields::from([(0, Unsigned(9)), (4, Unsigned(0))])),
                Struct(Fields::from([(0, Unsigned(1)), (4, Unsigned(2))])),
                Struct(Fields::from([(0, Unsigned(0)), (4, Unsigned(4))])),
                Struct(Fields::from([(0, Signed(-3)), (4, Unsigned(10))])),
            ]
        );
    }

    #[test]
    fn prepared_player_message_wire_bytes_match_python() {
        let schemas = Schemas::lobby();
        let digest = |messages: &[LobbyMessage]| {
            let mut hash = Sha256::new();
            for (crc, id, fields) in messages {
                let body = schemas.encode(*crc, *id, fields).unwrap();
                hash.update(crc.to_le_bytes());
                hash.update(id.to_le_bytes());
                hash.update(u32::try_from(body.len()).unwrap().to_le_bytes());
                hash.update(body);
            }
            format!("{:x}", hash.finalize())
        };
        // Python sent 20802 without a Kill Feed Display default (block
        // +0x120 stayed the schema's empty one); we turn it on so the kill
        // feed draws (`settings`). Everything else matches Python's bytes.
        let python_settings = |profile: &Profile| {
            let mut fields = settings(&schemas, profile).unwrap();
            let empty = schemas.empty(LOBBY, 20802).unwrap().remove(&0x120).unwrap();
            fields.insert(0x120, empty);
            fields
        };
        let mut profile = Profile {
            player_name: "Parity".into(),
            region: "GB".into(),
            game_region: "europe".into(),
            level: 42,
            endorsement_level: 4,
            ..Profile::default()
        };
        let identity = Identity {
            session: [0x1122_3344, 1],
            ..Identity::for_account(0x1234_5678)
        };
        let now = 1_800_000_000;
        let frame = 0x0250_0000_0000_02aa;
        let mut messages = vec![
            (IN_CONNECT, 20500, hello(&profile, identity)),
            (
                IN_CONNECT,
                20502,
                Fields::from([(0x78, Struct(record(&profile, identity, frame, now)))]),
            ),
            (LOBBY, 20802, python_settings(&profile)),
            (LOBBY, 20812, ux_states(&profile).unwrap()),
            (PERMISSIONS, 55500, features()),
        ];
        messages.extend(endorsements(&profile, identity, now));
        assert_eq!(
            digest(&messages),
            "04a4bc2ade095867821fa2b2c35fa006c7a74424408aec7b73708817e76d3a55"
        );

        profile.settings = serde_json::from_value(json!({
            "+0x108": {"+0x0": true},
            "+0x10D": {"+0x0": false, "+0x5": 2},
            "+0x130": [{"+0x0": u64::MAX, "+0x8": 0xe0b03a}]
        }))
        .unwrap();
        profile.ux_states = serde_json::from_value(json!({"7": -4, "0": 9})).unwrap();
        assert_eq!(
            digest(&[
                (LOBBY, 20802, python_settings(&profile)),
                (LOBBY, 20812, ux_states(&profile).unwrap()),
            ]),
            "c22c0478c94df785719930c95019a75754b75e82418d8e9a65a2fd2d1c9657a7"
        );
    }
}
