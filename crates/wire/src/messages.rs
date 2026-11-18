//! The JAM message channel at the start of a 1.74 world-frame payload.
//!
//! Client research (1.74; reader 0x7FF789B7A480, writer 0x7FF789B7ADE0): 1
//! bit "any messages"; if set, 4 bits reliable count and 4 bits unreliable
//! count; when either is not 0, pad to a byte, then reliable messages `u32
//! seq, u8 protocol index, u8 message index, JAM fields` (in seq order) and
//! unreliable ones `u8 protocol index, u8 message index, JAM fields`; the
//! stream goes on at the byte after the last message. With both counts 0
//! there is no padding (the input record then starts at bit 9). The client
//! delivers reliable messages strictly in seq order: it parks one that comes
//! early and drops one it already had, so a lost one must be resent or
//! everything after it waits. There is no protocol announcement on this link:
//! the index points into a fixed table the client builds in its message
//! system (0x7FF789B797C0); the message id is the protocol's first id plus the
//! message index.

use std::{error::Error, fmt};

use crate::jam::codec::{CodecError, Fields, Schemas};
use orpheus_core::bits::{BitError, BitReader, BitWriter};

pub const MAX_PER_KIND: usize = 15;

// Wire index order. Each pair is the protocol CRC and first message ID.
pub const PROTOCOLS: [(u32, u32); 11] = [
    (0xc77f_6403, 20300), // ClientInInstance
    (0xa7fb_bd0c, 21600), // ClientOutInstance
    (0x8f89_e0df, 25000), // ClientInCombat
    (0x82a2_f991, 27500), // ClientInVoice
    (0x9f0a_9f88, 25800), // ClientInStats
    (0xa938_5a52, 30700), // ClientOutCheats
    (0x398e_145e, 42900),
    (0x741e_c7b2, 42800),
    (0x716a_d5d9, 22300), // ClientOutPvP
    (0x145c_e2da, 25700), // ClientOutStats
    (0xcf04_3764, 42700), // ClientOutSpectate
];

#[derive(Debug)]
pub enum ChannelError {
    Bit(BitError),
    Codec(CodecError),
    TooManyMessages,
    UnknownMessage(u32),
    UnknownProtocol(u8),
    TruncatedSequence,
    TruncatedMessageHeader,
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bit(error) => error.fmt(f),
            Self::Codec(error) => error.fmt(f),
            Self::TooManyMessages => write!(f, "at most {MAX_PER_KIND} messages of each kind"),
            Self::UnknownMessage(id) => write!(f, "message {id} is not on the game link"),
            Self::UnknownProtocol(index) => write!(f, "unknown game protocol index {index}"),
            Self::TruncatedSequence => f.write_str("reliable sequence past end of game channel"),
            Self::TruncatedMessageHeader => f.write_str("message header past end of game channel"),
        }
    }
}

impl Error for ChannelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Bit(error) => Some(error),
            Self::Codec(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BitError> for ChannelError {
    fn from(error: BitError) -> Self {
        Self::Bit(error)
    }
}

impl From<CodecError> for ChannelError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

#[derive(Debug, PartialEq)]
pub struct Message {
    pub id: u32,
    pub fields: Fields,
}

#[derive(Debug, PartialEq)]
pub struct ReliableMessage {
    pub sequence: u32,
    pub message: Message,
}

#[derive(Debug, Default, PartialEq)]
pub struct Channel {
    pub reliable: Vec<ReliableMessage>,
    pub unreliable: Vec<Message>,
}

/// Resolve the wire index, CRC and offset. The highest eligible base wins
/// where protocol ID ranges overlap.
pub fn locate(id: u32) -> Result<(u8, u32, u8), ChannelError> {
    let (index, &(crc, first)) = PROTOCOLS
        .iter()
        .enumerate()
        .filter(|(_, protocol)| (protocol.1..protocol.1 + 256).contains(&id))
        .max_by_key(|(_, protocol)| protocol.1)
        .ok_or(ChannelError::UnknownMessage(id))?;
    Ok((index as u8, crc, (id - first) as u8))
}

/// Encode a game message without the channel envelope or reliable sequence.
pub fn encode(schemas: &Schemas, id: u32, fields: &Fields) -> Result<Vec<u8>, ChannelError> {
    let (index, crc, offset) = locate(id)?;
    let body = schemas.encode(crc, id, fields)?;
    let mut message = Vec::with_capacity(2 + body.len());
    message.extend([index, offset]);
    message.extend(body);
    Ok(message)
}

/// Write pre-encoded messages in caller order. Reliable sequences are assigned
/// by the caller, and loss recovery belongs to the session. An encoded message
/// fills its last byte with zeros; only the last message's unused bits are
/// free for what follows, so the frame goes on right after its last field
/// (a final packed bool, as in 25005, ends inside its byte).
pub fn write_channel(
    bits: &mut BitWriter,
    reliable: &[(u32, Vec<u8>)],
    unreliable: &[Vec<u8>],
) -> Result<(), ChannelError> {
    if reliable.is_empty() && unreliable.is_empty() {
        bits.bit(false);
        return Ok(());
    }
    if reliable.len() > MAX_PER_KIND || unreliable.len() > MAX_PER_KIND {
        return Err(ChannelError::TooManyMessages);
    }
    let final_message = unreliable
        .last()
        .or_else(|| reliable.last().map(|(_, message)| message))
        .expect("a message");
    // Only the last byte's unused bits are given back. Bytes whose fields
    // don't end in their last byte (test payloads, not game messages of the
    // client's layouts) are written whole.
    let whole = final_message.len() * 8;
    let final_end = read_message(&GAME_SCHEMAS, final_message, 0)
        .ok()
        .map(|(_, end)| end)
        .filter(|&end| end <= whole && whole - end < 8)
        .unwrap_or(whole);
    bits.bit(true);
    bits.bits(reliable.len() as u64, 4)?;
    bits.bits(unreliable.len() as u64, 4)?;
    bits.align();
    let last = reliable.len() + unreliable.len() - 1;
    let messages = reliable
        .iter()
        .map(|(sequence, message)| (Some(*sequence), message))
        .chain(unreliable.iter().map(|message| (None, message)));
    for (index, (sequence, message)) in messages.enumerate() {
        if let Some(sequence) = sequence {
            bits.raw(&sequence.to_le_bytes());
        }
        if index == last {
            bits.raw_bits(message, final_end)
                .expect("a message ends within its bytes");
        } else {
            bits.raw(message);
        }
    }
    Ok(())
}

/// The game link's message layouts, for finding where a message's last
/// field ends.
static GAME_SCHEMAS: std::sync::LazyLock<Schemas> = std::sync::LazyLock::new(Schemas::game);

/// Decode the channel and leave the reader at the next field in the frame.
/// A set presence bit with both counts zero ends immediately after the counts.
pub fn read_channel(
    reader: &mut BitReader<'_>,
    schemas: &Schemas,
) -> Result<Channel, ChannelError> {
    if !reader.bit()? {
        return Ok(Channel::default());
    }
    let reliable_count = reader.bits(4)? as usize;
    let unreliable_count = reader.bits(4)? as usize;
    if reliable_count == 0 && unreliable_count == 0 {
        return Ok(Channel::default());
    }

    reader.align();
    let data = reader
        .aligned_bytes()
        .ok_or(ChannelError::TruncatedMessageHeader)?;
    let mut channel = Channel {
        reliable: Vec::with_capacity(reliable_count),
        unreliable: Vec::with_capacity(unreliable_count),
    };
    let mut position = 0;
    let mut end = 0;
    for _ in 0..reliable_count {
        let sequence = data
            .get(position..position + 4)
            .ok_or(ChannelError::TruncatedSequence)?;
        let sequence = u32::from_le_bytes(sequence.try_into().unwrap());
        let (message, bit_end) = read_message(schemas, data, position + 4)?;
        channel.reliable.push(ReliableMessage { sequence, message });
        end = bit_end;
        position = bit_end.div_ceil(8);
    }
    for _ in 0..unreliable_count {
        let (message, bit_end) = read_message(schemas, data, position)?;
        channel.unreliable.push(message);
        end = bit_end;
        position = bit_end.div_ceil(8);
    }
    // The next field starts right after the last message's last field.
    reader.advance(end)?;
    Ok(channel)
}

/// A message at a byte position and the bit after its last field.
fn read_message(
    schemas: &Schemas,
    data: &[u8],
    position: usize,
) -> Result<(Message, usize), ChannelError> {
    let header = data
        .get(position..position + 2)
        .ok_or(ChannelError::TruncatedMessageHeader)?;
    let index = header[0];
    let &(crc, first) = PROTOCOLS
        .get(usize::from(index))
        .ok_or(ChannelError::UnknownProtocol(index))?;
    let id = first + u32::from(header[1]);
    let (fields, end) = schemas.read_bit_end(crc, id, data, position + 2)?;
    Ok((Message { id, fields }, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jam::codec::Value;

    fn fixture_schemas() -> Schemas {
        // Small field layouts from the 1.74 schema used by the golden channel.
        Schemas::from_json(
            r#"{
                "C77F6403": {
                    "20320": [
                        {"type":12,"off":120,"size":40,"count":0,"array":false},
                        {"type":0,"off":160,"size":1,"count":0,"array":false}
                    ],
                    "20322": [{"type":9,"off":120,"size":8,"count":0,"array":false}]
                },
                "A7FBBD0C": {"21602":[]}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn preserves_mixed_channel_and_next_field() {
        // Python ow174.game.messages encoded this from the same values.
        let golden = [
            0x25, 0x00, 0x98, 0xba, 0xdc, 0xfe, 0x00, 0x14, b'O', b'R', b'P', b'H', b'E', b'U',
            b'S', 0x00, 0x01, 0x99, 0xba, 0xdc, 0xfe, 0x01, 0x02, 0x00, 0x16, 0xf9, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0xff, 0x2b,
        ];
        let schemas = fixture_schemas();
        let title = Fields::from([
            (0x78, Value::String("ORPHEUS".into())),
            (0xa0, Value::Bool(true)),
        ]);
        let empty = Fields::new();
        let value = Fields::from([(0x78, Value::Signed(-7))]);
        let mut bits = BitWriter::new();
        write_channel(
            &mut bits,
            &[
                (0xfedc_ba98, encode(&schemas, 20320, &title).unwrap()),
                (0xfedc_ba99, encode(&schemas, 21602, &empty).unwrap()),
            ],
            &[encode(&schemas, 20322, &value).unwrap()],
        )
        .unwrap();
        bits.bits(0x2b, 6).unwrap();
        assert_eq!(bits.as_bytes(), golden);

        let mut reader = BitReader::with_range(&golden, 0, 270).unwrap();
        assert_eq!(
            read_channel(&mut reader, &schemas).unwrap(),
            Channel {
                reliable: vec![
                    ReliableMessage {
                        sequence: 0xfedc_ba98,
                        message: Message {
                            id: 20320,
                            fields: title
                        },
                    },
                    ReliableMessage {
                        sequence: 0xfedc_ba99,
                        message: Message {
                            id: 21602,
                            fields: empty
                        },
                    },
                ],
                unreliable: vec![Message {
                    id: 20322,
                    fields: value
                }],
            }
        );
        assert_eq!(reader.position(), 264);
        assert_eq!(reader.bits(6).unwrap(), 0x2b);
        assert_eq!(reader.left(), 0);
    }

    #[test]
    fn empty_forms_do_not_align_the_next_field() {
        let schemas = fixture_schemas();
        let mut bits = BitWriter::new();
        write_channel(&mut bits, &[], &[]).unwrap();
        bits.bits(0x16, 5).unwrap();
        assert_eq!(bits.as_bytes(), &[0x2c]);

        for (bytes, end) in [(&[0x2c][..], 1), (&[0x01, 0x2c][..], 9)] {
            let mut reader = BitReader::with_range(bytes, 0, end + 5).unwrap();
            assert_eq!(
                read_channel(&mut reader, &schemas).unwrap(),
                Channel::default()
            );
            assert_eq!(reader.position(), end);
            assert_eq!(reader.bits(5).unwrap(), 0x16);
        }
    }

    #[test]
    fn message_mapping_prefers_the_highest_eligible_base() {
        assert_eq!(locate(25800).unwrap(), (4, 0x9f0a_9f88, 0));
        assert_eq!(locate(42900).unwrap(), (6, 0x398e_145e, 0));
        assert_eq!(locate(42955).unwrap(), (6, 0x398e_145e, 55));
        assert_eq!(locate(20555).unwrap(), (0, 0xc77f_6403, 255));
        assert!(matches!(
            locate(20556),
            Err(ChannelError::UnknownMessage(20556))
        ));
    }

    #[test]
    fn rejects_excess_counts_and_malformed_messages() {
        let mut writer = BitWriter::new();
        let overflow = vec![vec![]; MAX_PER_KIND + 1];
        assert!(matches!(
            write_channel(&mut writer, &[], &overflow),
            Err(ChannelError::TooManyMessages)
        ));
        assert_eq!(writer.bit_len(), 0);

        let schemas = fixture_schemas();
        let mut sequence = BitReader::new(&[0x03, 0, 0, 0]);
        assert!(matches!(
            read_channel(&mut sequence, &schemas),
            Err(ChannelError::TruncatedSequence)
        ));
        let mut header = BitReader::new(&[0x21, 0, 0]);
        assert!(matches!(
            read_channel(&mut header, &schemas),
            Err(ChannelError::TruncatedMessageHeader)
        ));
        let mut index = BitReader::new(&[0x21, 0, 11, 0]);
        assert!(matches!(
            read_channel(&mut index, &schemas),
            Err(ChannelError::UnknownProtocol(11))
        ));
        let mut body = BitReader::new(&[0x21, 0, 0, 20, b'x']);
        assert!(matches!(
            read_channel(&mut body, &schemas),
            Err(ChannelError::Codec(_))
        ));
    }

    /// 25005 ends in a packed bool, so its last field ends inside its last
    /// byte; the world frame must go on right after that bit, alone or after
    /// other messages, or the client misreads the rest of the frame (the
    /// freeze after a kill, leafall903/ORPHEUS issue 14).
    #[test]
    fn the_frame_goes_on_after_a_final_packed_bool() {
        use crate::combat_messages::CombatNotice;

        let schemas = Schemas::game();
        let elimination = CombatNotice::Elimination {
            killer: 0xa000_0101,
            victim: 0xa002_0005,
            assist: false,
        }
        .encode(&schemas)
        .unwrap();
        let (_, end) = read_message(&schemas, &elimination, 0).unwrap();
        assert!(end % 8 != 0 && elimination.len() * 8 - end < 8);
        for reliable in [
            vec![(7, elimination.clone())],
            vec![(7, elimination.clone()), (8, elimination.clone())],
        ] {
            let mut bits = BitWriter::new();
            write_channel(&mut bits, &reliable, &[]).unwrap();
            // Presence and counts, aligned; then each message's sequence and
            // body, every one but the last whole.
            let before_last = 16 + (reliable.len() - 1) * (32 + elimination.len() * 8) + 32;
            assert_eq!(bits.bit_len(), before_last + end);
            bits.bits(0b101, 3).unwrap();
            let mut reader = BitReader::new(bits.as_bytes());
            let channel = read_channel(&mut reader, &schemas).unwrap();
            assert_eq!(channel.reliable.len(), reliable.len());
            assert!(
                channel
                    .reliable
                    .iter()
                    .all(|message| message.message.id == 25005)
            );
            assert_eq!(reader.bits(3).unwrap(), 0b101);
        }
    }
}
