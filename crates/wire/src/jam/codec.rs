//! JAM lobby and game message fields, encoded in the order of the target build's schema.
//!
//! Ported from `ow174/jam/codec.py` and `ow174/jam/values.py`. The inherited
//! MIT notice and protocol research credits remain in `LICENSE` and `CREDITS.md`.
//!
//! The schemas are extracted from a running 1.74 client. Field types: 0 bool
//! (one bit, lowest first), 1-9 little-endian integers of 1, 2, 4 or 8 bytes,
//! 10/11 f32/f64, 12/13 zero-terminated UTF-8 string, 14 struct (inline), 15
//! blob (u32 length, bytes); arrays are a u32 count and the elements
//! (fixed-size arrays have no count). Bools fill bytes across the whole
//! message: a run of bools continues over struct, array-element and
//! fixed-array edges, and only a byte-sized write starts a new byte (tested in
//! game: in 20802 the 5 flags of +0x108 and the first 4 of +0x10D form one run
//! of 9 bits). Some protocols carry a u32 between the protocol byte and the
//! message id: the client registers them with header flag 8 (registration +8 =
//! 0xA, others 0x2, read live) and reads the u32 before the id
//! (0x7FF789471C50); its protocol table marks them with flag 2 (entry +0xC).
//! With the u32 after the id instead, only messages at id offset 0 (52300,
//! 55500) worked.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::Path;

const BOOL: u8 = 0;
const STRUCT: u8 = 14;
pub const MAX_ARRAY_COUNT: usize = 0x40000;

/// A JAM message or inline struct, keyed by the field's in-memory offset.
pub type Fields = BTreeMap<u32, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Unsigned(u64),
    Signed(i64),
    Float32(f32),
    Float64(f64),
    String(String),
    Blob(Vec<u8>),
    Struct(Fields),
    Array(Vec<Value>),
}

impl Value {
    fn integer(&self) -> Option<i128> {
        match self {
            Self::Unsigned(value) => Some(i128::from(*value)),
            Self::Signed(value) => Some(i128::from(*value)),
            Self::Bool(value) => Some(i128::from(*value)),
            _ => None,
        }
    }

    fn float(&self) -> Option<f64> {
        match self {
            Self::Float32(value) => Some(f64::from(*value)),
            Self::Float64(value) => Some(*value),
            _ => self.integer().map(|value| value as f64),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Unsigned(value) => *value != 0,
            Self::Signed(value) => *value != 0,
            Self::Float32(value) => *value != 0.0,
            Self::Float64(value) => *value != 0.0,
            Self::String(value) => !value.is_empty(),
            Self::Blob(value) => !value.is_empty(),
            Self::Struct(value) => !value.is_empty(),
            Self::Array(value) => !value.is_empty(),
        }
    }

    /// The diagnostic JSON representation used by the lobby's message logs.
    pub fn to_jsonable(&self) -> serde_json::Value {
        use serde_json::Value as Json;

        match self {
            Self::Bool(value) => Json::Bool(*value),
            Self::Unsigned(value) if *value > 0xffffff => Json::String(format!("0x{value:016X}")),
            Self::Signed(value) if *value > 0xffffff => Json::String(format!("0x{value:016X}")),
            Self::Unsigned(value) => Json::from(*value),
            Self::Signed(value) => Json::from(*value),
            Self::Float32(value) => Json::from(f64::from(*value)),
            Self::Float64(value) => Json::from(*value),
            Self::String(value) => Json::String(value.clone()),
            Self::Blob(value) => {
                let mut hex = String::with_capacity(value.len().saturating_mul(2));
                use std::fmt::Write;
                for byte in value {
                    write!(hex, "{byte:02x}").expect("writing to String is infallible");
                }
                serde_json::json!({ "blob": hex })
            }
            Self::Struct(fields) => Json::Object(
                fields
                    .iter()
                    .map(|(offset, value)| (format!("+0x{offset:X}"), value.to_jsonable()))
                    .collect(),
            ),
            Self::Array(values) => Json::Array(values.iter().map(Self::to_jsonable).collect()),
        }
    }
}

/// A two-word identifier such as a lobby account or party ID.
pub fn id16(lo: u64, hi: u64) -> Fields {
    Fields::from([(0, Value::Unsigned(lo)), (8, Value::Unsigned(hi))])
}

#[derive(Debug)]
pub enum CodecError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidSchema(String),
    UnknownProtocol(u32),
    UnknownMessage { crc: u32, id: u32 },
    InvalidMessageOffset { base: u32, id: u32 },
    UnknownFieldType(u8),
    Truncated(&'static str),
    UnterminatedString,
    TrailingBytes(usize),
    ArrayTooLarge(usize),
    WrongType { offset: u32, expected: &'static str },
    OutOfRange { offset: u32 },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "could not read JAM schemas: {error}"),
            Self::Json(error) => write!(f, "invalid JAM schema JSON: {error}"),
            Self::InvalidSchema(error) => f.write_str(error),
            Self::UnknownProtocol(crc) => write!(f, "no schema for protocol {crc:08X}"),
            Self::UnknownMessage { crc, id } => {
                write!(f, "no schema for protocol {crc:08X} message {id}")
            }
            Self::InvalidMessageOffset { base, id } => {
                write!(f, "message {id} is outside the wire range for base {base}")
            }
            Self::UnknownFieldType(kind) => write!(f, "unknown field type {kind}"),
            Self::Truncated(what) => write!(f, "{what} past end"),
            Self::UnterminatedString => f.write_str("unterminated string"),
            Self::TrailingBytes(bytes) => write!(f, "{bytes} bytes left over"),
            Self::ArrayTooLarge(count) => write!(f, "array count {count} too large"),
            Self::WrongType { offset, expected } => {
                write!(f, "field +0x{offset:X} expects {expected}")
            }
            Self::OutOfRange { offset } => {
                write!(f, "field +0x{offset:X} is outside its wire range")
            }
        }
    }
}

impl std::error::Error for CodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Field {
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(rename = "off")]
    pub offset: u32,
    pub count: i32,
    pub array: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
}

impl Field {
    /// An array with a u32 count before its elements.
    pub fn counted(&self) -> bool {
        self.array || self.count == -1
    }

    fn wrong_type(&self, expected: &'static str) -> CodecError {
        CodecError::WrongType {
            offset: self.offset,
            expected,
        }
    }

    fn out_of_range(&self) -> CodecError {
        CodecError::OutOfRange {
            offset: self.offset,
        }
    }
}

type RawSchemas = BTreeMap<String, BTreeMap<u32, Vec<Field>>>;

/// Where the game data folder holds the lobby and game link layouts.
pub const LOBBY_SCHEMAS: &str = "data/lobby_schemas.json";
pub const GAME_SCHEMAS: &str = "data/game_schemas.json";

/// Wire layouts keyed by protocol CRC and message ID. Field vectors preserve schema order.
#[derive(Debug)]
pub struct Schemas {
    groups: BTreeMap<u32, BTreeMap<u32, Vec<Field>>>,
}

impl Schemas {
    /// The number of message layouts and of protocol groups.
    pub fn counts(&self) -> (usize, usize) {
        let messages = self.groups.values().map(BTreeMap::len).sum();
        (messages, self.groups.len())
    }

    /// The client's lobby message layouts, as `lucy_assets` derives them
    /// from the client program into the game data folder `runtime`.
    pub fn lobby(runtime: &Path) -> Result<Self, CodecError> {
        Self::load(runtime.join(LOBBY_SCHEMAS))
    }

    /// The client's game link message layouts (as [`Self::lobby`]). The
    /// first load also gives the channel writer
    /// ([`crate::messages::write_channel`]) the layouts it finds a frame's
    /// last field end with.
    pub fn game(runtime: &Path) -> Result<Self, CodecError> {
        let path = runtime.join(GAME_SCHEMAS);
        let schemas = Self::load(&path)?;
        crate::messages::keep_game_schemas(|| Self::load(&path));
        Ok(schemas)
    }

    pub fn from_json(json: &str) -> Result<Self, CodecError> {
        Self::from_reader(json.as_bytes())
    }

    pub fn from_reader(reader: impl Read) -> Result<Self, CodecError> {
        let raw: RawSchemas = serde_json::from_reader(reader).map_err(CodecError::Json)?;
        let mut groups = BTreeMap::new();
        for (key, messages) in raw {
            let crc = u32::from_str_radix(&key, 16)
                .map_err(|_| CodecError::InvalidSchema(format!("invalid protocol CRC {key:?}")))?;
            if messages.is_empty() {
                return Err(CodecError::InvalidSchema(format!(
                    "protocol {crc:08X} has no messages"
                )));
            }
            if groups.insert(crc, messages).is_some() {
                return Err(CodecError::InvalidSchema(format!(
                    "duplicate protocol {crc:08X}"
                )));
            }
        }
        Ok(Self { groups })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, CodecError> {
        let file = std::fs::File::open(path).map_err(CodecError::Io)?;
        Self::from_reader(io::BufReader::new(file))
    }

    pub fn fields(&self, crc: u32, id: u32) -> Result<&[Field], CodecError> {
        self.groups
            .get(&crc)
            .and_then(|group| group.get(&id))
            .map(Vec::as_slice)
            .ok_or(CodecError::UnknownMessage { crc, id })
    }

    /// The protocol whose layouts hold message `id`, if any.
    pub fn protocol_of(&self, id: u32) -> Option<u32> {
        self.groups
            .iter()
            .find(|(_, group)| group.contains_key(&id))
            .map(|(crc, _)| *crc)
    }

    pub fn base(&self, crc: u32) -> Result<u32, CodecError> {
        self.groups
            .get(&crc)
            .and_then(BTreeMap::first_key_value)
            .map(|(id, _)| *id)
            .ok_or(CodecError::UnknownProtocol(crc))
    }

    /// The protocol wire byte, optional four-byte prefix, then one byte of message offset.
    pub fn header(&self, crc: u32, wire: u8, id: u32) -> Result<Vec<u8>, CodecError> {
        let base = self.base(crc)?;
        let offset = id
            .checked_sub(base)
            .and_then(|delta| u8::try_from(delta).ok())
            .ok_or(CodecError::InvalidMessageOffset { base, id })?;
        let start = message_offset_position(crc);
        let mut header = vec![0; start + 1];
        header[0] = wire;
        header[start] = offset;
        Ok(header)
    }

    /// Extracts the message ID and body from a header for the given protocol.
    pub fn split<'a>(&self, crc: u32, frame: &'a [u8]) -> Result<(u32, &'a [u8]), CodecError> {
        let start = message_offset_position(crc);
        let offset = *frame
            .get(start)
            .ok_or(CodecError::Truncated("message header"))?;
        let base = self.base(crc)?;
        let id = base
            .checked_add(u32::from(offset))
            .ok_or(CodecError::InvalidSchema(format!(
                "message offset {offset} overflows base {base}"
            )))?;
        Ok((id, &frame[start + 1..]))
    }

    /// Decodes a message body. Strict mode rejects bytes after the last field.
    pub fn decode(
        &self,
        crc: u32,
        id: u32,
        body: &[u8],
        strict: bool,
    ) -> Result<Fields, CodecError> {
        let (fields, end) = self.read(crc, id, body, 0)?;
        if strict && end != body.len() {
            return Err(CodecError::TrailingBytes(body.len() - end));
        }
        Ok(fields)
    }

    /// Decodes fields at a byte position and returns their end for back-to-back game messages.
    pub fn read(
        &self,
        crc: u32,
        id: u32,
        data: &[u8],
        pos: usize,
    ) -> Result<(Fields, usize), CodecError> {
        if pos > data.len() {
            return Err(CodecError::Truncated("message body"));
        }
        let mut decoder = Decoder {
            data,
            pos,
            bit: 8,
            byte: 0,
        };
        let fields = decoder.fields(self.fields(crc, id)?)?;
        Ok((fields, decoder.pos))
    }

    /// Like [`Self::read`], but the end is the bit after the last field: a
    /// message ending in packed bools stops inside its last byte, and the
    /// game link goes on right there (client JAM field reader, RVA 5670E0;
    /// leafall903/ORPHEUS `messages.py`, `docs/MCCREE_COMBAT.txt`).
    pub fn read_bit_end(
        &self,
        crc: u32,
        id: u32,
        data: &[u8],
        pos: usize,
    ) -> Result<(Fields, usize), CodecError> {
        if pos > data.len() {
            return Err(CodecError::Truncated("message body"));
        }
        let mut decoder = Decoder {
            data,
            pos,
            bit: 8,
            byte: 0,
        };
        let fields = decoder.fields(self.fields(crc, id)?)?;
        let end = if decoder.bit == 8 {
            decoder.pos * 8
        } else {
            (decoder.pos - 1) * 8 + usize::from(decoder.bit)
        };
        Ok((fields, end))
    }

    pub fn encode(&self, crc: u32, id: u32, value: &Fields) -> Result<Vec<u8>, CodecError> {
        let mut encoder = Encoder {
            data: Vec::new(),
            bit: 8,
        };
        encoder.fields(self.fields(crc, id)?, Some(value))?;
        Ok(encoder.data)
    }

    /// Builds a template containing the schema defaults for every field.
    pub fn empty(&self, crc: u32, id: u32) -> Result<Fields, CodecError> {
        let body = self.encode(crc, id, &Fields::new())?;
        self.decode(crc, id, &body, true)
    }
}

fn message_offset_position(crc: u32) -> usize {
    // These protocol registrations carry a zero u32 before the message
    // offset: the client's protocol table marks them with flag 2 (entry
    // `+0xC`; 0x262EF305, the Game Browser's answers, at 0x7FF78B50F19C),
    // and its reader takes the u32 first (0x7FF789471C50, header flag 8).
    match crc {
        0xBCD57A46 | 0xBDDBF58A | 0x262E_F305 => 5,
        _ => 1,
    }
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u8,
    byte: u8,
}

impl<'a> Decoder<'a> {
    fn bytes(&mut self, count: usize, what: &'static str) -> Result<&'a [u8], CodecError> {
        let data = self.data[self.pos..]
            .get(..count)
            .ok_or(CodecError::Truncated(what))?;
        self.pos += count;
        Ok(data)
    }

    fn number<const N: usize, T>(
        &mut self,
        parse: fn([u8; N]) -> T,
        what: &'static str,
    ) -> Result<T, CodecError> {
        let bytes = self
            .bytes(N, what)?
            .try_into()
            .expect("a slice of exactly N bytes");
        Ok(parse(bytes))
    }

    fn fields(&mut self, schema: &[Field]) -> Result<Fields, CodecError> {
        let mut fields = Fields::new();
        for field in schema {
            let value = if field.counted() {
                self.bit = 8;
                let count = self.number(u32::from_le_bytes, "array count")? as usize;
                let remaining = self.data.len() - self.pos;
                if field.kind == BOOL && count > remaining.saturating_mul(8) {
                    return Err(CodecError::Truncated("bool"));
                }
                if field.kind != BOOL
                    && count > MAX_ARRAY_COUNT
                    && count > remaining.saturating_add(1)
                {
                    return Err(CodecError::ArrayTooLarge(count));
                }
                self.array(field, count)?
            } else if field.count > 1 {
                self.array(field, field.count as usize)?
            } else {
                self.value(field)?
            };
            fields.insert(field.offset, value);
        }
        Ok(fields)
    }

    fn array(&mut self, field: &Field, count: usize) -> Result<Value, CodecError> {
        // The peer controls variable counts. Allocate only as elements are read.
        let mut values = Vec::new();
        for _ in 0..count {
            values.push(self.value(field)?);
        }
        Ok(Value::Array(values))
    }

    fn value(&mut self, field: &Field) -> Result<Value, CodecError> {
        if field.kind == BOOL {
            if self.bit == 8 {
                self.byte = self.bytes(1, "bool")?[0];
                self.bit = 0;
            }
            let value = self.byte & (1 << self.bit) != 0;
            self.bit += 1;
            return Ok(Value::Bool(value));
        }
        if field.kind == STRUCT {
            return Ok(Value::Struct(self.fields(&field.fields)?));
        }

        self.bit = 8;
        const SCALAR: &str = "scalar";
        Ok(match field.kind {
            1 | 2 => Value::Unsigned(self.number(u8::from_le_bytes, SCALAR)?.into()),
            3 => Value::Signed(self.number(i8::from_le_bytes, SCALAR)?.into()),
            4 => Value::Unsigned(self.number(u16::from_le_bytes, SCALAR)?.into()),
            5 => Value::Signed(self.number(i16::from_le_bytes, SCALAR)?.into()),
            6 => Value::Unsigned(self.number(u32::from_le_bytes, SCALAR)?.into()),
            7 => Value::Signed(self.number(i32::from_le_bytes, SCALAR)?.into()),
            8 => Value::Unsigned(self.number(u64::from_le_bytes, SCALAR)?),
            9 => Value::Signed(self.number(i64::from_le_bytes, SCALAR)?),
            10 => Value::Float32(self.number(f32::from_le_bytes, SCALAR)?),
            11 => Value::Float64(self.number(f64::from_le_bytes, SCALAR)?),
            12 | 13 => {
                let tail = &self.data[self.pos..];
                let length = tail
                    .iter()
                    .position(|byte| *byte == 0)
                    .ok_or(CodecError::UnterminatedString)?;
                self.pos += length + 1;
                Value::String(String::from_utf8_lossy(&tail[..length]).into_owned())
            }
            15 => {
                let length = self.number(u32::from_le_bytes, "blob length")? as usize;
                Value::Blob(self.bytes(length, "blob")?.to_vec())
            }
            kind => return Err(CodecError::UnknownFieldType(kind)),
        })
    }
}

struct Encoder {
    data: Vec<u8>,
    bit: u8,
}

impl Encoder {
    fn fields(&mut self, schema: &[Field], fields: Option<&Fields>) -> Result<(), CodecError> {
        for field in schema {
            let value = fields.and_then(|fields| fields.get(&field.offset));
            if field.counted() || field.count > 1 {
                let elements = match value {
                    Some(Value::Array(elements)) => elements.as_slice(),
                    None => &[],
                    _ => return Err(field.wrong_type("an array")),
                };
                let count = if field.counted() {
                    self.bit = 8;
                    self.count(field, elements.len())?;
                    elements.len()
                } else {
                    field.count as usize
                };
                for i in 0..count {
                    self.value(field, elements.get(i))?;
                }
            } else {
                self.value(field, value)?;
            }
        }
        Ok(())
    }

    fn count(&mut self, field: &Field, count: usize) -> Result<(), CodecError> {
        let count = u32::try_from(count).map_err(|_| field.out_of_range())?;
        self.data.extend_from_slice(&count.to_le_bytes());
        Ok(())
    }

    fn integer<const N: usize, T: TryFrom<i128>>(
        &mut self,
        field: &Field,
        value: Option<&Value>,
        bytes: fn(T) -> [u8; N],
    ) -> Result<(), CodecError> {
        let value = match value {
            Some(value) => value
                .integer()
                .ok_or_else(|| field.wrong_type("an integer"))?,
            None => 0,
        };
        let value = T::try_from(value).map_err(|_| field.out_of_range())?;
        self.data.extend_from_slice(&bytes(value));
        Ok(())
    }

    fn value(&mut self, field: &Field, value: Option<&Value>) -> Result<(), CodecError> {
        if field.kind == BOOL {
            if self.bit == 8 {
                self.data.push(0);
                self.bit = 0;
            }
            if value.is_some_and(Value::truthy) {
                *self.data.last_mut().expect("an open bit byte") |= 1 << self.bit;
            }
            self.bit += 1;
            return Ok(());
        }
        if field.kind == STRUCT {
            let fields = match value {
                Some(Value::Struct(fields)) => Some(fields),
                None => None,
                _ => return Err(field.wrong_type("a struct")),
            };
            return self.fields(&field.fields, fields);
        }

        self.bit = 8;
        match field.kind {
            1 | 2 => self.integer(field, value, u8::to_le_bytes),
            3 => self.integer(field, value, i8::to_le_bytes),
            4 => self.integer(field, value, u16::to_le_bytes),
            5 => self.integer(field, value, i16::to_le_bytes),
            6 => self.integer(field, value, u32::to_le_bytes),
            7 => self.integer(field, value, i32::to_le_bytes),
            8 => self.integer(field, value, u64::to_le_bytes),
            9 => self.integer(field, value, i64::to_le_bytes),
            10 | 11 => {
                let number = match value {
                    Some(value) => value.float().ok_or_else(|| field.wrong_type("a number"))?,
                    None => 0.0,
                };
                if field.kind == 10 {
                    let narrow = match value {
                        Some(Value::Float32(value)) => *value,
                        _ => number as f32,
                    };
                    if number.is_finite() && narrow.is_infinite() {
                        return Err(field.out_of_range());
                    }
                    self.data.extend_from_slice(&narrow.to_le_bytes());
                } else {
                    self.data.extend_from_slice(&number.to_le_bytes());
                }
                Ok(())
            }
            12 | 13 => {
                match value {
                    Some(Value::String(text)) => {
                        self.data.extend(text.bytes().filter(|byte| *byte != 0))
                    }
                    None => (),
                    _ => return Err(field.wrong_type("a string")),
                }
                self.data.push(0);
                Ok(())
            }
            15 => {
                let blob = match value {
                    Some(Value::Blob(blob)) => blob.as_slice(),
                    None => &[],
                    _ => return Err(field.wrong_type("a blob")),
                };
                self.count(field, blob.len())?;
                self.data.extend_from_slice(blob);
                Ok(())
            }
            kind => Err(CodecError::UnknownFieldType(kind)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const CRC: u32 = 0xba5eba11;

    fn synthetic(fields: &str) -> Schemas {
        Schemas::from_json(&format!(r#"{{"{CRC:X}":{{"12":{fields}}}}}"#)).unwrap()
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn lobby_wire_vectors_match_python() {
        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        let login = 0x3c7e3468;
        let mut fields = schemas.empty(login, 21800).unwrap();
        fields.insert(0x78, Value::String("Player".into()));
        let body = schemas.encode(login, 21800, &fields).unwrap();
        assert_eq!(schemas.decode(login, 21800, &body, true).unwrap(), fields);
        assert_eq!(
            format!("{:x}", Sha256::digest(body)),
            "5ef774494c2150e5b721533367200cd1ff1d64c9b44ef776dc8f9a16200cdba1"
        );
        assert_eq!(
            schemas.encode(0x9529f0ed, 52204, &Fields::new()).unwrap(),
            [0; 4]
        );
        assert_eq!(
            schemas.encode(0xbddbf58a, 52300, &Fields::new()).unwrap(),
            [0; 8]
        );

        for (crc, id, expected) in [
            (0xbddbf58a, 52301, &[70, 0, 0, 0, 0, 1][..]),
            (0xbcd57a46, 55500, &[70, 0, 0, 0, 0, 0][..]),
            (0x262ef305, 40603, &[70, 0, 0, 0, 0, 3][..]),
            (0x2411de56, 20703, &[70, 3][..]),
        ] {
            let mut frame = schemas.header(crc, 70, id).unwrap();
            assert_eq!(frame, expected);
            frame.push(19);
            assert_eq!(schemas.split(crc, &frame).unwrap(), (id, &[19][..]));
            assert!(matches!(
                schemas.split(crc, &[70]),
                Err(CodecError::Truncated(_))
            ));
        }
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn all_shipped_lobby_and_game_schemas_round_trip_defaults() {
        for schemas in [
            Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap(),
            Schemas::game(&lucy_testkit::runtime_dir()).unwrap(),
        ] {
            for (crc, group) in &schemas.groups {
                for id in group.keys() {
                    let default_body = schemas.encode(*crc, *id, &Fields::new()).unwrap();
                    let empty = schemas.empty(*crc, *id).unwrap();
                    assert_eq!(
                        schemas.encode(*crc, *id, &empty).unwrap(),
                        default_body,
                        "{crc:08X}/{id}"
                    );
                }
            }
        }
    }

    #[test]
    fn bools_cross_struct_and_array_edges_in_schema_order() {
        let schemas = synthetic(
            r#"[
            {"type":0,"off":9,"count":0,"array":false},
            {"type":14,"off":1,"count":2,"array":false,"fields":[
                {"type":0,"off":2,"count":0,"array":false},
                {"type":0,"off":3,"count":2,"array":false}
            ]},
            {"type":0,"off":7,"count":0,"array":false},
            {"type":0,"off":8,"count":-1,"array":false},
            {"type":0,"off":0,"count":0,"array":false},
            {"type":4,"off":10,"count":0,"array":false}
        ]"#,
        );
        let bools =
            |values: &[bool]| Value::Array(values.iter().copied().map(Value::Bool).collect());
        let mut fields = Fields::from([
            (9, Value::Bool(true)),
            (
                1,
                Value::Array(vec![
                    Value::Struct(Fields::from([(2, Value::Bool(true)), (3, bools(&[false]))])),
                    Value::Struct(Fields::from([(3, bools(&[true, true, true]))])),
                ]),
            ),
            (7, Value::Bool(true)),
            (8, bools(&[true, false])),
            (0, Value::Bool(true)),
            (10, Value::Unsigned(0x3412)),
        ]);
        let body = schemas.encode(CRC, 12, &fields).unwrap();
        assert_eq!(body, [0xe3, 2, 0, 0, 0, 0x05, 0x12, 0x34]);
        fields.insert(
            1,
            Value::Array(vec![
                Value::Struct(Fields::from([
                    (2, Value::Bool(true)),
                    (3, bools(&[false, false])),
                ])),
                Value::Struct(Fields::from([
                    (2, Value::Bool(false)),
                    (3, bools(&[true, true])),
                ])),
            ]),
        );
        let bytes = [vec![1, 2], body, vec![4, 5]].concat();
        assert_eq!(
            schemas.read(CRC, 12, &bytes, 2).unwrap(),
            (fields.clone(), 10)
        );
        assert_eq!(schemas.decode(CRC, 12, &bytes[2..], false).unwrap(), fields);
        assert!(matches!(
            schemas.decode(CRC, 12, &bytes[2..], true),
            Err(CodecError::TrailingBytes(2))
        ));
    }

    #[test]
    fn blobs_strings_and_float_bits_round_trip() {
        let schemas = synthetic(
            r#"[
            {"type":12,"off":1,"count":0,"array":false},
            {"type":13,"off":2,"count":0,"array":false},
            {"type":15,"off":3,"count":0,"array":false},
            {"type":10,"off":4,"count":0,"array":false},
            {"type":11,"off":5,"count":0,"array":false}
        ]"#,
        );
        let mut fields = Fields::from([
            (1, Value::String("é\0水".into())),
            (2, Value::String("Player".into())),
            (3, Value::Blob(vec![1, 0, 0xff])),
            (4, Value::Float32(f32::from_bits(0x7fc01234))),
            (5, Value::Float64(-0.0)),
        ]);
        let mut body = schemas.encode(CRC, 12, &fields).unwrap();
        let expected = [
            "é水\0Player\0".as_bytes(),
            &[3, 0, 0, 0, 1, 0, 0xff],
            &0x7fc01234u32.to_le_bytes(),
            &(-0.0f64).to_le_bytes(),
        ]
        .concat();
        assert_eq!(body, expected);
        body[0..2].copy_from_slice(&[0xff, b'!']);
        let decoded = schemas.decode(CRC, 12, &body, true).unwrap();
        assert_eq!(decoded[&1], Value::String("�!水".into()));
        assert!(matches!(decoded[&4], Value::Float32(value) if value.to_bits() == 0x7fc01234));
        assert!(matches!(decoded[&5], Value::Float64(value) if value.to_bits() == 1 << 63));

        fields.insert(4, Value::Float64(f64::MAX));
        assert!(matches!(
            schemas.encode(CRC, 12, &fields),
            Err(CodecError::OutOfRange { offset: 4 })
        ));
    }

    #[test]
    fn rejects_malformed_messages_and_out_of_range_integers() {
        let schemas = synthetic(r#"[{"type":2,"off":1,"count":-1,"array":true}]"#);
        assert!(matches!(
            schemas.decode(CRC, 12, &[255; 4], true),
            Err(CodecError::ArrayTooLarge(_))
        ));
        assert!(matches!(
            schemas.decode(CRC, 12, &[1, 0, 0], true),
            Err(CodecError::Truncated("array count"))
        ));
        assert!(matches!(
            schemas.decode(CRC, 12, &[1, 0, 0, 0], true),
            Err(CodecError::Truncated("scalar"))
        ));
        let value = Fields::from([(1, Value::Array(vec![Value::Unsigned(256)]))]);
        assert!(matches!(
            schemas.encode(CRC, 12, &value),
            Err(CodecError::OutOfRange { offset: 1 })
        ));
        assert!(matches!(
            schemas.header(CRC, 1, 268),
            Err(CodecError::InvalidMessageOffset { .. })
        ));

        let schemas = synthetic(r#"[{"type":0,"off":1,"count":-1,"array":true}]"#);
        assert!(matches!(
            schemas.decode(CRC, 12, &[255; 4], true),
            Err(CodecError::Truncated("bool"))
        ));
        let schemas = synthetic(r#"[{"type":15,"off":1,"count":0,"array":false}]"#);
        assert!(matches!(
            schemas.decode(CRC, 12, &[2, 0, 0, 0, 1], true),
            Err(CodecError::Truncated("blob"))
        ));
        let schemas = synthetic(r#"[{"type":12,"off":1,"count":0,"array":false}]"#);
        assert!(matches!(
            schemas.decode(CRC, 12, b"Player", true),
            Err(CodecError::UnterminatedString)
        ));
    }

    #[test]
    fn jam_values_have_the_existing_log_representation() {
        let value = Value::Struct(id16(5, 1 << 56));
        assert_eq!(
            value.to_jsonable(),
            serde_json::json!({"+0x0": 5, "+0x8": "0x0100000000000000"})
        );
        assert_eq!(
            Value::Blob(vec![0x10, 0xf4]).to_jsonable(),
            serde_json::json!({"blob": "10f4"})
        );
    }
}
