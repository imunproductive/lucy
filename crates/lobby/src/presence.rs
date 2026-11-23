//! Account and game-account presence for the lobby, built from a table of
//! the Battle.net presence fields the client reads.
//!
//! Presence records go in 27100 +0xA8 and 27113: an entity id (`+0x0`), its
//! owner (`+0x10`) and fields (`+0x20`), each a key and a value, both
//! Battle.net protobuf messages. A key is `{1: program, 2: group, 3: field,
//! 4: unique id}` (program 16974, "BN"); a value is a Variant (`2` bool, `3`
//! int, `5` string, `7` message, `8` four-character code, `9` uint, `10`
//! entity id). Group 1 is the account, group 2 a game account (field 1 its
//! online bool). Times are Variant ints in microseconds since 1970.
//!
//! A player has three records: the account, the Battle.net App's game
//! account and the Overwatch ("Pro") game account, whose ids our server
//! makes from the account id ([`APP_ACCOUNT_BITS`], [`PRO_ACCOUNT_BITS`]).
//! The fields and their order are those retail's lobby sent (the 1.68
//! capture); the values are the player's.
//!
//! The status bools of the account record (group 1): the client reads its own
//! status from them (0x7FF7896145E0 via 0x7FF789613F90: 7 away, 11 busy, 12
//! appear offline, none set is online), which the status dropdown (27011
//! +0x78) and the name card show. A friend's colour (0x7FF789616670) is away
//! on 7, busy on 11, else online while one of the friend's game accounts is.
//! All three always go out, so going back to online clears them.

use std::io;

use prost::encoding::{WireType, encode_key, encode_varint};

use crate::{
    accounts::{Profile, battle_tag},
    login::Identity,
    transport::LobbyMessage,
};
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Blob, Struct, Unsigned},
};

pub const FRIENDS: u32 = 0xf470_e2ab;
pub const STATUS_ONLINE: u8 = 1;
pub const STATUS_AWAY: u8 = 2;
pub const STATUS_BUSY: u8 = 3;
pub const STATUS_OFFLINE: u8 = 4;
const UPDATE: u32 = 27113;

/// The id bits that make a player's App and Overwatch game account ids from
/// the account id.
const APP_ACCOUNT_BITS: u64 = 0x0a0a_0000;
pub const PRO_ACCOUNT_BITS: u64 = 0x0b0b_0000;
/// The high halves of the entity ids: an account, the App's game account
/// (program "App") and Overwatch's ("Pro").
const ACCOUNT_HIGH: u64 = 0x0100_0000_0000_0000;
const APP_HIGH: u64 = 0x0200_0002_0041_7070;
const PRO_HIGH: u64 = 0x0200_0002_0050_726f;

const PRESENCE_PROGRAM: u64 = 16_974;
/// The program of the Overwatch player id field ("Pro").
const PRO_PROGRAM: u64 = 0x0050_726f;
const ACCOUNT_GROUP: u64 = 1;
const GAME_ACCOUNT_GROUP: u64 = 2;
/// The unique id in the key of each of the account's game account fields
/// (group 1, field 3), App's then Overwatch's: values retail's lobby sent;
/// the client only matches keys.
const APP_ENTRY: u64 = 9_965_679_503_634_278_804;
const PRO_ENTRY: u64 = 2_908_323_939_972_234_345;
/// Game account fields 15 and 17: values retail sent for both game accounts
/// (field 15 5728622, 17 and 13 2, 14 and 16 1).
const GAME_ACCOUNT_15: u64 = 5_728_622;
/// The App's field 8: `{1: "App", 2: fixed32 0x72707273, 3: 0}`, as retail
/// sent it.
const APP_FIELD_8: &[u8] = b"\x0d\x70\x70\x41\x00\x15\x73\x72\x70\x72\x18\x00";
const MICROSECONDS: u64 = 1_000_000;

/// A presence value (a Battle.net Variant).
enum Variant {
    Bool(bool),
    /// A time, the record's.
    Time,
    Uint(u64),
    String(Vec<u8>),
    Message(Vec<u8>),
    FourCc(&'static str),
    Entity(u64, u64),
}

impl Variant {
    fn encode(&self, time: u64) -> Vec<u8> {
        let mut value = Vec::new();
        let bytes = |value: &mut Vec<u8>, number: u32, bytes: &[u8]| {
            encode_key(number, WireType::LengthDelimited, value);
            encode_varint(bytes.len() as u64, value);
            value.extend_from_slice(bytes);
        };
        match self {
            Self::Bool(flag) => return bool_variant(*flag),
            Self::Time => {
                encode_key(3, WireType::Varint, &mut value);
                encode_varint(time, &mut value);
            }
            Self::Uint(number) => {
                encode_key(9, WireType::Varint, &mut value);
                encode_varint(*number, &mut value);
            }
            Self::String(text) => bytes(&mut value, 5, text),
            Self::Message(message) => bytes(&mut value, 7, message),
            Self::FourCc(code) => bytes(&mut value, 8, code.as_bytes()),
            Self::Entity(high, low) => {
                let mut entity = Vec::new();
                encode_key(1, WireType::SixtyFourBit, &mut entity);
                entity.extend_from_slice(&high.to_le_bytes());
                encode_key(2, WireType::SixtyFourBit, &mut entity);
                entity.extend_from_slice(&low.to_le_bytes());
                bytes(&mut value, 10, &entity);
            }
        }
        value
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Own,
    Friend,
}

pub struct Presence;

impl Presence {
    /// The account, app and game records visible to the caller. Seconds are
    /// passed in explicitly so a batch uses a single wall clock value.
    pub fn records(
        &self,
        profile: &Profile,
        account_lo: u32,
        status: u8,
        created: i64,
        now: u64,
        view: View,
    ) -> io::Result<Vec<Value>> {
        let time = [profile.last_online, created]
            .into_iter()
            .find(|&time| time != 0)
            .map(u64::try_from)
            .transpose()
            .map_err(|_| invalid("presence timestamp precedes the Unix epoch"))?
            .unwrap_or(now)
            .checked_mul(MICROSECONDS)
            .ok_or_else(|| invalid("presence timestamp exceeds the wire range"))?;
        let account = u64::from(account_lo);
        let app = account ^ APP_ACCOUNT_BITS;
        let pro = account ^ PRO_ACCOUNT_BITS;
        let tag = battle_tag(&profile.player_name, account_lo);
        // The tag field keeps at most 127 bytes (one-byte length).
        let tag = tag.as_bytes()[..tag.len().min(0x7f)].to_vec();
        let online = view == View::Own || status != STATUS_OFFLINE;
        let game_account = format!("{account}#1").into_bytes();
        let mut player_id = Vec::new();
        for (number, value) in [(1, 1), (2, ACCOUNT_HIGH), (3, account)] {
            encode_key(number, WireType::Varint, &mut player_id);
            encode_varint(value, &mut player_id);
        }
        use Variant::*;
        let account_fields = vec![
            (PRESENCE_PROGRAM, ACCOUNT_GROUP, 12, 0, Bool(false)),
            (PRESENCE_PROGRAM, ACCOUNT_GROUP, 4, 0, String(tag.clone())),
            (
                PRESENCE_PROGRAM,
                ACCOUNT_GROUP,
                3,
                APP_ENTRY,
                Entity(APP_HIGH, app),
            ),
            (
                PRESENCE_PROGRAM,
                ACCOUNT_GROUP,
                1,
                0,
                String(profile.player_name.clone().into_bytes()),
            ),
            (
                PRESENCE_PROGRAM,
                ACCOUNT_GROUP,
                3,
                PRO_ENTRY,
                Entity(PRO_HIGH, pro),
            ),
            (PRESENCE_PROGRAM, ACCOUNT_GROUP, 6, 0, Time),
        ];
        let game = |field: u64, value| (PRESENCE_PROGRAM, GAME_ACCOUNT_GROUP, field, 0, value);
        let app_fields = vec![
            game(17, Uint(2)),
            game(13, Uint(2)),
            game(10, Bool(false)),
            game(11, Time),
            game(1, Bool(online)),
            game(12, Bool(false)),
            game(5, String(tag.clone())),
            game(3, FourCc("App")),
            game(7, Entity(ACCOUNT_HIGH, account)),
            game(8, Message(APP_FIELD_8.to_vec())),
            game(16, Uint(1)),
            game(15, Uint(GAME_ACCOUNT_15)),
            game(4, Time),
            game(6, String(game_account.clone())),
            game(14, Uint(1)),
            game(9, Time),
        ];
        let pro_fields = vec![
            game(3, FourCc("Pro")),
            game(7, Entity(ACCOUNT_HIGH, account)),
            game(16, Uint(1)),
            game(17, Uint(2)),
            game(13, Uint(2)),
            game(15, Uint(GAME_ACCOUNT_15)),
            (PRO_PROGRAM, GAME_ACCOUNT_GROUP, 3, 0, Message(player_id)),
            game(10, Bool(false)),
            game(11, Time),
            game(1, Bool(online)),
            game(4, Time),
            game(6, String(game_account)),
            game(14, Uint(1)),
            game(12, Bool(false)),
            game(5, String(tag)),
        ];
        let entity =
            |high: u64, low: u64| Struct(Fields::from([(0, Unsigned(low)), (8, Unsigned(high))]));
        let flags = [
            (12, status == STATUS_OFFLINE && view == View::Own),
            (7, status == STATUS_AWAY),
            (11, status == STATUS_BUSY),
        ];
        let record = |id: Value, owner: Value, table: Vec<(u64, u64, u64, u64, Variant)>| {
            let mut fields: Vec<Value> = table
                .into_iter()
                .map(|(program, group, field, unique, value)| {
                    let value = match value {
                        // The account's status flags (module documentation).
                        Bool(_) if group == ACCOUNT_GROUP && program == PRESENCE_PROGRAM => flags
                            .iter()
                            .find(|&&(flag, _)| flag == field)
                            .map_or_else(|| value.encode(time), |&(_, on)| bool_variant(on)),
                        value => value.encode(time),
                    };
                    presence_field(field_key(program, group, field, unique), value)
                })
                .collect();
            if fields.len() == 6 {
                // The account record: the two status flags it lacks, added
                // with only their key and value.
                for &(flag, on) in &flags[1..] {
                    fields.push(Struct(Fields::from([
                        (8, Blob(field_key(PRESENCE_PROGRAM, ACCOUNT_GROUP, flag, 0))),
                        (0x30, Blob(bool_variant(on))),
                    ])));
                }
            }
            Struct(Fields::from([
                (0, id),
                (0x10, owner),
                (0x20, Array(fields)),
            ]))
        };
        Ok(vec![
            record(entity(ACCOUNT_HIGH, account), entity(0, 0), account_fields),
            record(
                entity(APP_HIGH, app),
                entity(ACCOUNT_HIGH, account),
                app_fields,
            ),
            record(
                entity(PRO_HIGH, pro),
                entity(ACCOUNT_HIGH, account),
                pro_fields,
            ),
        ])
    }

    pub fn own(&self, profile: &Profile, identity: Identity, now: u64) -> io::Result<LobbyMessage> {
        let records = self.records(
            profile,
            identity.account_lo,
            STATUS_ONLINE,
            0,
            now,
            View::Own,
        )?;
        Ok((FRIENDS, UPDATE, Fields::from([(0x78, Array(records))])))
    }
}

fn presence_field(key: Vec<u8>, value: Vec<u8>) -> Value {
    Struct(Fields::from([
        (0, Unsigned(0)),
        (8, Blob(key)),
        (0x30, Blob(value)),
    ]))
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// A Variant bool (field 2).
fn bool_variant(value: bool) -> Vec<u8> {
    vec![0x10, u8::from(value)]
}

fn field_key(program: u64, group: u64, field: u64, unique: u64) -> Vec<u8> {
    let mut key = Vec::new();
    for (number, value) in [(1, program), (2, group), (3, field), (4, unique)] {
        encode_key(number, WireType::Varint, &mut key);
        encode_varint(value, &mut key);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    use sha2::{Digest, Sha256};

    use lucy_wire::jam::codec::Schemas;

    /// Each record's fields, as `(group, field, value)`.
    fn fields(record: &Value) -> Vec<(u64, u64, Vec<u8>)> {
        let Struct(record) = record else {
            panic!("a record")
        };
        let Array(fields) = &record[&0x20] else {
            panic!("fields")
        };
        fields
            .iter()
            .map(|field| {
                let Struct(field) = field else {
                    panic!("a field")
                };
                let (Blob(key), Blob(value)) = (&field[&8], &field[&0x30]) else {
                    panic!("blobs")
                };
                (key[5].into(), key[7].into(), value.clone())
            })
            .collect()
    }

    #[test]
    fn own_account_flags_and_friend_game_account_status_have_distinct_fields() {
        let profile = Profile {
            player_name: "Local".into(),
            ..Profile::default()
        };
        let records = |status, view| {
            Presence
                .records(&profile, 0x1234, status, 0, 1_700_000_000, view)
                .unwrap()
        };
        let flag = |records: &[Value], group: u64, field: u64| {
            let fields = fields(&records[usize::from(group != 1)]);
            fields
                .into_iter()
                .filter(|&(g, f, _)| (g, f) == (group, field))
                .map(|(_, _, value)| value)
                .collect::<Vec<_>>()
        };
        // Appear offline: the own account's flag 12 is set, 7 and 11 sent clear.
        let own = records(STATUS_OFFLINE, View::Own);
        assert_eq!(flag(&own, 1, 12), [bool_variant(true)]);
        assert_eq!(flag(&own, 1, 7), [bool_variant(false)]);
        assert_eq!(flag(&own, 1, 11), [bool_variant(false)]);
        assert_eq!(flag(&own, 2, 1), [bool_variant(true)]);
        // A friend sees an offline player's game account offline, and no
        // appear-offline flag.
        let friend = records(STATUS_OFFLINE, View::Friend);
        assert_eq!(flag(&friend, 1, 12), [bool_variant(false)]);
        assert_eq!(flag(&friend, 2, 1), [bool_variant(false)]);
        let busy = records(STATUS_BUSY, View::Friend);
        assert_eq!(flag(&busy, 1, 11), [bool_variant(true)]);
        assert_eq!(flag(&busy, 2, 1), [bool_variant(true)]);
    }

    #[test]
    fn own_and_friend_presence_match_python_wire_values() {
        // The table reproduces, value for value, the records the Python
        // server made by personalizing the retail capture.
        let schemas = Schemas::lobby();
        let presence = Presence;
        let profile = Profile {
            player_name: "Local Tester".into(),
            last_online: 1_700_000_000,
            ..Profile::default()
        };
        let never = Profile {
            last_online: 0,
            ..profile.clone()
        };
        let account = 0x1234_5678;
        let now = 1_700_020_000;
        let created = 1_690_000_000;
        let expected_online = "34a2b0939b236458b4a6fde8e2009719cb5e54494d0545639a7023a327938d9c";
        let (crc, id, own) = presence
            .own(&profile, Identity::for_account(account), now)
            .unwrap();
        assert_eq!((crc, id), (FRIENDS, UPDATE));
        let wire = schemas.encode(crc, id, &own).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(wire)), expected_online);

        // Independent Python Presence.records vectors for both views and all time fallbacks.
        for (profile, status, created, view, expected) in [
            (&profile, STATUS_ONLINE, created, View::Own, expected_online),
            (
                &profile,
                STATUS_AWAY,
                created,
                View::Own,
                "e9ed876b46b90b5277a42648f0fb476756475711fbb4b83b2183e23afc7ae099",
            ),
            (
                &profile,
                STATUS_BUSY,
                created,
                View::Friend,
                "7837f25c7de62b5bfbf110d08f83f5707bc8d53902a0ed96ccdf1a99519e7085",
            ),
            (
                &profile,
                STATUS_OFFLINE,
                created,
                View::Own,
                "c171c5d849ea586de7a5423d4f1805823f22f05298e2a59812183b428d0fb581",
            ),
            (
                &profile,
                STATUS_OFFLINE,
                created,
                View::Friend,
                "e0351ad021be99d04757abb038603671ad8d67e54b2672523716bdd3a3ac3b79",
            ),
            (
                &never,
                STATUS_OFFLINE,
                created,
                View::Friend,
                "eda7c96b352f678e8ca6813903084fb9e72017c4c2e0c803f08fd991fb2e4fc6",
            ),
            (
                &never,
                STATUS_ONLINE,
                0,
                View::Own,
                "ffbc1c0e8e9c8093c104ed1664674df00830fc69f39b2011e1122b62f860335e",
            ),
        ] {
            let records = presence
                .records(profile, account, status, created, now, view)
                .unwrap();
            assert_eq!(records.len(), 3);
            let wire = schemas
                .encode(FRIENDS, UPDATE, &Fields::from([(0x78, Array(records))]))
                .unwrap();
            assert_eq!(format!("{:x}", Sha256::digest(wire)), expected);
        }
    }
}
