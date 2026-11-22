//! ClientOutInstance game messages and the known hero-select parameter layouts.
//!
//! The client sends the pick as ClientOutInstance 21616 `{entity, parameters,
//! game message 1361.025, instance}`, with the hero among the parameters as an
//! 8-byte little-endian GUID with the hero type bits (as in OW2's traffic; the
//! exact layout is not certain). 1360.025 asks to open or close the screen: its
//! parameters end with two u32 entities (0x7FF78AAB7D30), and the boolean
//! before them is its type byte 1 and a value byte.

use crate::heroes::{Hero, HeroCatalog};
use lucy_wire::{jam::codec::Value, messages::Message};

pub const CLIENT_GAME_MESSAGE: u32 = 21616;
pub const PICK_HERO: u64 = 0x0240_0000_0000_1361;
pub const OPEN_OR_CLOSE: u64 = 0x0240_0000_0000_1360;
const HERO_ASSET_TYPE: u16 = 0x02e0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientGameMessage<'a> {
    pub entity: u32,
    pub guid: u64,
    pub instance: i16,
    pub parameters: Parameters<'a>,
}

impl<'a> ClientGameMessage<'a> {
    /// Borrow the typed 21616 envelope from an already decoded game message.
    pub fn read(message: &'a Message) -> Option<Self> {
        if message.id != CLIENT_GAME_MESSAGE {
            return None;
        }
        let fields = &message.fields;
        let Value::Struct(entity) = fields.get(&0x78)? else {
            return None;
        };
        let Value::Unsigned(entity) = *entity.get(&0)? else {
            return None;
        };
        let Value::Blob(parameters) = fields.get(&0x80)? else {
            return None;
        };
        let Value::Unsigned(guid) = *fields.get(&0xa8)? else {
            return None;
        };
        let Value::Signed(instance) = *fields.get(&0xb0)? else {
            return None;
        };
        Some(Self {
            entity: u32::try_from(entity).ok()?,
            guid,
            instance: i16::try_from(instance).ok()?,
            parameters: Parameters::new(parameters),
        })
    }

    pub fn hero_select<'catalog>(
        self,
        heroes: &'catalog HeroCatalog,
    ) -> Option<HeroSelectRequest<'catalog>> {
        match self.guid {
            PICK_HERO => Some(HeroSelectRequest::Pick(
                self.parameters
                    .find_asset(HERO_ASSET_TYPE, |guid| heroes.get(guid)),
            )),
            OPEN_OR_CLOSE => Some(HeroSelectRequest::OpenOrClose(
                self.parameters.bool_before_entities(),
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeroSelectRequest<'a> {
    /// A recognized pick without a known hero has no gameplay effect.
    Pick(Option<&'a Hero>),
    /// Without a readable bool, the client request toggles the current screen.
    OpenOrClose(Option<bool>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Parameters<'a>(&'a [u8]);

impl<'a> Parameters<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    /// Split the two little-endian entity words found at the end of the
    /// observed parameter blobs. Their meaning depends on the message.
    pub fn split_entity_tail(self) -> Option<(&'a [u8], [u32; 2])> {
        let (body, tail) = self.0.split_last_chunk::<8>()?;
        let &[a, b, c, d, e, f, g, h] = tail;
        Some((
            body,
            [
                u32::from_le_bytes([a, b, c, d]),
                u32::from_le_bytes([e, f, g, h]),
            ],
        ))
    }

    pub fn bool_before_entities(self) -> Option<bool> {
        let (body, _) = self.split_entity_tail()?;
        match body {
            [.., 1, 0] => Some(false),
            [.., 1, 1] => Some(true),
            _ => None,
        }
    }

    /// Find the first known unaligned asset of this type. This matches the
    /// reference pick reader without assuming the rest of the blob grammar.
    pub fn find_asset<T>(
        self,
        asset_type: u16,
        mut known: impl FnMut(u64) -> Option<T>,
    ) -> Option<T> {
        self.0.windows(8).find_map(|bytes| {
            let guid = u64::from_le_bytes(bytes.try_into().expect("eight-byte window"));
            if guid >> 48 == u64::from(asset_type) {
                known(guid)
            } else {
                None
            }
        })
    }

    /// The blob's rows (key, value), as the client's message builder
    /// writes them (`sub_7FF78AAB7D30`): three compact u64 (a mask byte,
    /// bit i set for each non-zero byte i, then those bytes), a bool byte,
    /// the row count (unsigned varint, `sub_7FF7893015A0`), per row its
    /// key (a compact u64: the `0A4` parameter GUID) and its value (a type
    /// byte and a body, `sub_7FF789E76470`), then two u32 entities. None:
    /// a blob this grammar doesn't read (a value type it doesn't know).
    pub fn rows(self) -> Option<Vec<(u64, ParameterValue)>> {
        let mut blob = Blob(self.0);
        for _ in 0..3 {
            blob.compact()?;
        }
        blob.take(1)?;
        let count = blob.unsigned()?;
        let mut rows = Vec::new();
        for _ in 0..count {
            let key = blob.compact()?;
            let kind = blob.take(1)?[0];
            rows.push((key, blob.value(kind)?));
        }
        blob.take(8)?;
        blob.0.is_empty().then_some(rows)
    }
}

/// A value of a 21616 parameter blob, by its type byte (`sub_7FF789E76500`).
#[derive(Clone, Debug, PartialEq)]
pub enum ParameterValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f32),
    Asset(u64),
    Vector([f32; 3]),
    Entity(u32),
    /// Types 12 and 13: each item a key (`0xFF`: none) and a value
    /// (`sub_7FF789E76410`). `STU_79A131B4` sends every parameter as one
    /// (its values' items: `sub_7FF78AAB7C60`).
    Array(Vec<ParameterValue>),
}

struct Blob<'a>(&'a [u8]);

impl<'a> Blob<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(count)?;
        self.0 = rest;
        Some(head)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// `sub_7FF789301330`.
    fn compact(&mut self) -> Option<u64> {
        let mask = self.take(1)?[0];
        let mut value = 0;
        for byte in 0..8 {
            if mask & (1 << byte) != 0 {
                value |= u64::from(self.take(1)?[0]) << (8 * byte);
            }
        }
        Some(value)
    }

    /// `sub_7FF7893015A0`: below 0x80 one byte; below 0x4000 two (the
    /// high byte with 0x80); below 2^29 four (the top byte with 0xC0, the
    /// next, then a u16); else 0xFF and a u64.
    fn unsigned(&mut self) -> Option<u64> {
        let first = self.take(1)?[0];
        Some(match first {
            0x00..=0x7f => u64::from(first),
            0x80..=0xbf => u64::from(first & 0x3f) << 8 | u64::from(self.take(1)?[0]),
            0xff => self.u64()?,
            _ => {
                let next = self.take(1)?[0];
                let low = u16::from_le_bytes(self.take(2)?.try_into().ok()?);
                u64::from(first & 0x3f) << 24 | u64::from(next) << 16 | u64::from(low)
            }
        })
    }

    /// `sub_7FF789301460`: a sign bit (0x80) on the first byte; below 0x40
    /// one byte; below 0x2000 two (0x40 on the first); below 2^28 four
    /// (0x60 on the first, then a byte and a u16); else 0xFF and a u64.
    fn signed(&mut self) -> Option<i64> {
        let first = self.take(1)?[0];
        if first == 0xff {
            return Some(self.u64()? as i64);
        }
        let magnitude = match first & 0x60 {
            0x00 | 0x20 => i64::from(first & 0x3f),
            0x40 => i64::from(first & 0x1f) << 8 | i64::from(self.take(1)?[0]),
            _ => {
                let next = self.take(1)?[0];
                let low = u16::from_le_bytes(self.take(2)?.try_into().ok()?);
                i64::from(first & 0x0f) << 24 | i64::from(next) << 16 | i64::from(low)
            }
        };
        Some(if first & 0x80 != 0 {
            -magnitude
        } else {
            magnitude
        })
    }

    fn value(&mut self, kind: u8) -> Option<ParameterValue> {
        Some(match kind {
            0 => ParameterValue::Null,
            1 => ParameterValue::Bool(self.take(1)?[0] != 0),
            2 => ParameterValue::Integer(self.signed()?),
            3 => ParameterValue::Float(self.f32()?),
            4 => ParameterValue::Asset(self.u64()?),
            6 => ParameterValue::Vector([self.f32()?, self.f32()?, self.f32()?]),
            10 => ParameterValue::Entity(self.u32()?),
            12 | 13 => {
                let count = usize::try_from(self.signed()?).ok()?;
                let mut items = Vec::with_capacity(count.min(64));
                for _ in 0..count {
                    let key = self.take(1)?[0];
                    if key != 0xff {
                        self.value(key)?;
                    }
                    let kind = self.take(1)?[0];
                    items.push(self.value(kind)?);
                }
                ParameterValue::Array(items)
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::heroes::SOLDIER;
    use lucy_core::bits::BitReader;
    use lucy_wire::{
        jam::codec::{Fields, Schemas},
        messages::read_channel,
    };

    #[test]
    fn envelope_uses_the_client_message_id_and_fixed_width_fields() {
        let mut message = Message {
            id: CLIENT_GAME_MESSAGE,
            fields: Fields::from([
                (
                    0x78,
                    Value::Struct(Fields::from([(0, Value::Unsigned(0xa000_0100))])),
                ),
                (0x80, Value::Blob(vec![1, 2])),
                (0xa8, Value::Unsigned(OPEN_OR_CLOSE)),
                (0xb0, Value::Signed(-1)),
            ]),
        };
        let decoded = ClientGameMessage::read(&message).unwrap();
        assert_eq!(decoded.entity, 0xa000_0100);
        assert_eq!(decoded.guid, OPEN_OR_CLOSE);
        assert_eq!(decoded.instance, -1);
        assert_eq!(decoded.parameters.as_bytes(), [1, 2]);
        message.id -= 1;
        assert!(ClientGameMessage::read(&message).is_none());
        message.id += 1;
        message.fields.insert(0xb0, Value::Signed(32_768));
        assert!(ClientGameMessage::read(&message).is_none());
    }

    #[test]
    fn pick_scans_unaligned_asset_words_and_skips_unknown_candidates() {
        let heroes = HeroCatalog::from_json(
            r#"{"0x02E000000000006E":{"name":"Local fixture","class":"Damage","body":"0x0800000000000001"}}"#,
        )
        .unwrap();
        let mut bytes = vec![0xfa, 0xb0, 0x01];
        bytes.extend_from_slice(&0x02e0_0000_ffff_ffffu64.to_le_bytes());
        let pick = ClientGameMessage {
            entity: 1,
            guid: PICK_HERO,
            instance: 3,
            parameters: Parameters::new(&[]),
        };
        let unknown = ClientGameMessage {
            parameters: Parameters::new(&bytes),
            ..pick
        };
        assert_eq!(
            unknown.hero_select(&heroes),
            Some(HeroSelectRequest::Pick(None))
        );
        bytes.extend_from_slice(&SOLDIER.to_le_bytes());
        let known = ClientGameMessage {
            parameters: Parameters::new(&bytes),
            ..pick
        };
        assert_eq!(
            known.hero_select(&heroes),
            Some(HeroSelectRequest::Pick(Some(heroes.get(SOLDIER).unwrap())))
        );
    }

    #[test]
    fn only_the_bool_tag_before_the_complete_entity_tail_sets_the_screen() {
        let mut bytes = vec![0x05, 0x01, 0x01];
        bytes.extend_from_slice(&0xa000_0100u32.to_le_bytes());
        bytes.extend_from_slice(&0xa000_0200u32.to_le_bytes());
        let parameters = Parameters::new(&bytes);
        assert_eq!(parameters.as_bytes(), bytes);
        assert_eq!(
            parameters.split_entity_tail(),
            Some((&bytes[..3], [0xa000_0100, 0xa000_0200]))
        );
        assert_eq!(parameters.bool_before_entities(), Some(true));
        bytes[2] = 0;
        assert_eq!(Parameters::new(&bytes).bool_before_entities(), Some(false));
        bytes[2] = 2;
        assert_eq!(Parameters::new(&bytes).bool_before_entities(), None);
        bytes[1] = 2;
        bytes[2] = 1;
        assert_eq!(Parameters::new(&bytes).bool_before_entities(), None);
        assert_eq!(Parameters::new(&bytes[..7]).split_entity_tail(), None);
        assert_eq!(Parameters::new(&bytes[3..]).bool_before_entities(), None);
    }

    /// The pick blob of the captured traffic (the Python test's `1361`):
    /// one row, `005D` with an array of Soldier: 76; and the Training
    /// Partner picker's `1213` as `STU_79A131B4` builds it, `08E6` an array
    /// of a hero and `08E7` an array of a bool.
    #[test]
    fn parameter_rows_read_as_the_client_writes_them() {
        let pick = [
            0x00, 0x00, 0x00, 0x00, 0x01, 0xc1, 0x5d, 0x40, 0x0a, 0x0c, 0x01, 0xff, 0x04, 0x6e,
            0x00, 0x00, 0x00, 0x00, 0x00, 0xe0, 0x02, 0x00, 0x01, 0x00, 0xa0, 0x00, 0x02, 0x00,
            0xa0,
        ];
        assert_eq!(
            Parameters::new(&pick).rows(),
            Some(vec![(
                0x0a40_0000_0000_005d,
                ParameterValue::Array(vec![ParameterValue::Asset(SOLDIER)])
            )])
        );
        let mut partner = vec![0x00, 0x00, 0x00, 0x00, 0x02];
        partner.extend([0xc3, 0xe6, 0x08, 0x40, 0x0a, 0x0c, 0x01, 0xff, 0x04]);
        partner.extend(0x02e0_0000_0000_0002_u64.to_le_bytes());
        partner.extend([0xc3, 0xe7, 0x08, 0x40, 0x0a, 0x0c, 0x01, 0xff, 0x01, 0x01]);
        partner.extend(0xa000_0100_u32.to_le_bytes());
        partner.extend(0xa000_0100_u32.to_le_bytes());
        assert_eq!(
            Parameters::new(&partner).rows(),
            Some(vec![
                (
                    0x0a40_0000_0000_08e6,
                    ParameterValue::Array(vec![ParameterValue::Asset(0x02e0_0000_0000_0002)])
                ),
                (
                    0x0a40_0000_0000_08e7,
                    ParameterValue::Array(vec![ParameterValue::Bool(true)])
                ),
            ])
        );
        // A blob cut short, or with bytes after its entities, reads none.
        assert_eq!(Parameters::new(&partner[..partner.len() - 1]).rows(), None);
        partner.push(0);
        assert_eq!(Parameters::new(&partner).rows(), None);
    }

    #[test]
    #[ignore = "requires derived 104319 game schemas and hero catalog"]
    fn reliable_21616_messages_match_the_python_reader_and_hero_select_requests() {
        let data = lucy_testkit::runtime_dir().join("data");
        let schemas = Schemas::game();
        let heroes = HeroCatalog::load(data.join("game_heroes_174.json")).unwrap();
        // Python game.messages.write_channel with synthetic entity IDs and the
        // known pick/open parameter bytes from tests/test_game_statescript.py.
        let bytes = concat!(
            "0b00070000000110000100a01d0000000000000001c15d400a0c01ff046e0000000000e002000100a0000200a061130000000040020300",
            "080000000110000100a00e00000001c3a2050101000100a0000200a060130000000040020300",
            "090000000110000100a00e00000001c3a2050100000100a0000200a060130000000040020300",
            "0a0000000110000100a00a0000000102000100a0000200a060130000000040020300",
            "0b0000000110000100a0000000006213000000004002ffff",
        )
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
        let mut reader = BitReader::new(&bytes);
        let channel = read_channel(&mut reader, &schemas).unwrap();
        assert!(channel.unreliable.is_empty());
        assert_eq!(reader.position(), 1512);
        let expected = [
            Some(HeroSelectRequest::Pick(Some(heroes.get(SOLDIER).unwrap()))),
            Some(HeroSelectRequest::OpenOrClose(Some(true))),
            Some(HeroSelectRequest::OpenOrClose(Some(false))),
            Some(HeroSelectRequest::OpenOrClose(None)),
            None,
        ];
        assert_eq!(channel.reliable.len(), expected.len());
        for (index, (item, wanted)) in channel.reliable.iter().zip(expected).enumerate() {
            assert_eq!(item.sequence, 7 + index as u32);
            let message = ClientGameMessage::read(&item.message).unwrap();
            assert_eq!(message.entity, 0xa000_0100);
            assert_eq!(message.instance, if wanted.is_some() { 3 } else { -1 });
            assert_eq!(message.hero_select(&heroes), wanted);
            if wanted.is_some() {
                assert_eq!(
                    message.parameters.split_entity_tail().unwrap().1,
                    [0xa000_0100, 0xa000_0200],
                );
            }
        }
    }
}
