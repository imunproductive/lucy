//! What the server reads from the client program in the install,
//! `_retail_/Overwatch.exe`: the lobby and game link message layouts, the
//! Battle.net protobuf descriptors and the key the lobby signs its state blob
//! with. The program's code is encrypted on disk, but its data sections
//! (`.rdata`, `.data`) are stored plain, and these four live there.
//!
//! - **Message layouts.** The client keeps one 32-byte `JamMessageInfo` per
//!   message, `{name*, protocol*, items*, u32 protocol CRC, u32 message id}`;
//!   its items are 0x30-byte records `{name*, u32 type, u32 offset, u32 size,
//!   i32 count, inner*, resize*, bits}`. Type 17 ends a list, type 14 nests a
//!   struct through `inner*`, and a non-null `resize*` marks an array. The
//!   pointers are absolute addresses at the program's preferred base. The
//!   table also holds protocols the server doesn't speak, so only the
//!   protocols below are read. This is the reader `tools/dump_schemas.py`
//!   (deleted in phase 8, tag `python-reference`) ran over a running client.
//! - **Battle.net descriptors.** Each `.proto` file of the client's
//!   Battle.net services is stored as its serialized `FileDescriptorProto`.
//!   The server loads every `bgs/low/pb/client/` file and the files they
//!   import, as a `FileDescriptorSet` in name order.
//! - **The state blob key.** 64 bytes at a fixed place in `.rdata`, checked
//!   against their SHA-256.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt::Write as _,
    fs,
    path::Path,
};

use sha2::{Digest, Sha256};

use crate::{Install, Output, Result, asset_error};

pub const LOBBY_SCHEMAS: &str = "data/lobby_schemas.json";
pub const GAME_SCHEMAS: &str = "data/game_schemas.json";
pub const BNET_DESCRIPTORS: &str = "data/bnet_descriptors.pb";
pub const LOBBY_STATE_BLOB_KEY: &str = "data/lobby_state_blob.key";

/// The protocols of the lobby link the client announces (by CRC), plus
/// `11D82194`, the link's own messages.
const LOBBY_PROTOCOLS: [u32; 87] = [
    0x00A8_654E,
    0x074D_AD18,
    0x0BFB_00A8,
    0x0DFE_EFEF,
    0x11D8_2194,
    0x158B_BD52,
    0x1732_EBF2,
    0x17CC_BFB2,
    0x1A26_4832,
    0x1A98_79A4,
    0x1C6E_C712,
    0x1CA9_B786,
    0x1CFB_43CD,
    0x1D80_BD86,
    0x1EC2_5650,
    0x1F8A_43DD,
    0x2128_6B49,
    0x2372_A2E8,
    0x2411_DE56,
    0x2558_BD8D,
    0x262E_F305,
    0x267F_DE9E,
    0x273F_F626,
    0x28A2_A1CD,
    0x34BB_385D,
    0x3C7E_3468,
    0x4116_E22C,
    0x46DC_9706,
    0x4815_1833,
    0x4996_3FA6,
    0x4BAD_7A7E,
    0x5217_E4CD,
    0x5635_CB1C,
    0x5894_D085,
    0x5F8C_3ADD,
    0x5F91_3F6B,
    0x6099_BC5F,
    0x692C_511B,
    0x7006_5EF5,
    0x70E5_A247,
    0x713D_7589,
    0x75D3_2AE2,
    0x779A_8581,
    0x783E_E77A,
    0x79BF_DDBE,
    0x7F4F_46CB,
    0x8133_E1F5,
    0x872E_4FB9,
    0x8887_16D3,
    0x8B3D_C153,
    0x8B77_C823,
    0x928C_FB44,
    0x9529_F0ED,
    0x981D_6A52,
    0xA149_8A6A,
    0xA287_DF29,
    0xA59B_4EEF,
    0xA6E5_3896,
    0xA743_DF58,
    0xA830_FDA2,
    0xAA91_BE18,
    0xAD1C_34BA,
    0xAFBE_5037,
    0xB2FF_5A5E,
    0xB4F8_BC62,
    0xB688_70B8,
    0xB689_E024,
    0xB9DC_A497,
    0xBAA0_7D0C,
    0xBCD5_7A46,
    0xBDDB_F58A,
    0xC64B_397E,
    0xC812_00AF,
    0xCAE2_5D1B,
    0xCBF5_9220,
    0xCEA9_8618,
    0xD555_0262,
    0xDDA5_83EF,
    0xDE3C_BAE7,
    0xE54A_AACD,
    0xEB45_AD29,
    0xEB59_E676,
    0xF470_E2AB,
    0xF5FF_548A,
    0xFB2E_5CAC,
    0xFDEC_4D9E,
    0xFFEB_57B9,
];

/// The game link's protocols. The link has no announcement: they sit in a
/// fixed table in the client's message system (`0x7FF789B797C0`), in this
/// order, the wire index.
const GAME_PROTOCOLS: [u32; 11] = [
    0xC77F_6403,
    0xA7FB_BD0C,
    0x8F89_E0DF,
    0x82A2_F991,
    0x9F0A_9F88,
    0xA938_5A52,
    0x398E_145E,
    0x741E_C7B2,
    0x716A_D5D9,
    0x145C_E2DA,
    0xCF04_3764,
];

/// The state blob key's place (relative to the program's base) and its
/// SHA-256.
const BLOB_KEY_RVA: usize = 0x278_CF30;
const BLOB_KEY_SHA256: &str = "ab024361c5acd29e3a45548cc675002b762895753fe6dc7613335b1f0ff033b8";

/// The Battle.net files the server loads, with the files they import.
const BNET_PREFIX: &str = "bgs/low/pb/client/";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let program = install.client_program();
    let image = Image::read(&program)?;
    let failed = |what: String| asset_error(format!("{}: {what}", program.display()));
    let lobby = image.schemas(&LOBBY_PROTOCOLS);
    let game = image.schemas(&GAME_PROTOCOLS);
    for (link, schemas, protocols) in [
        ("lobby", &lobby, &LOBBY_PROTOCOLS[..]),
        ("game", &game, &GAME_PROTOCOLS[..]),
    ] {
        if let Some(missing) = protocols.iter().find(|crc| !schemas.contains_key(crc)) {
            return Err(failed(format!(
                "no {link} message of protocol {missing:08X}"
            )));
        }
    }
    let key = image.blob_key().ok_or_else(|| {
        failed("the lobby state blob key is not where build 104319 keeps it".into())
    })?;
    Ok(vec![
        Output {
            path: LOBBY_SCHEMAS.into(),
            bytes: schemas_json(&lobby).into_bytes(),
        },
        Output {
            path: GAME_SCHEMAS.into(),
            bytes: schemas_json(&game).into_bytes(),
        },
        Output {
            path: BNET_DESCRIPTORS.into(),
            bytes: image.descriptors().map_err(failed)?,
        },
        Output {
            path: LOBBY_STATE_BLOB_KEY.into(),
            bytes: key.to_vec(),
        },
    ])
}

/// The Battle.net descriptor set of the client program at `program`, as
/// [`BNET_DESCRIPTORS`] holds it: for a Battle.net emulator that runs
/// without a derived pack (joining another PC's server).
pub fn bnet_descriptors(program: &Path) -> Result<Vec<u8>> {
    Image::read(program)?
        .descriptors()
        .map_err(|error| asset_error(format!("{}: {error}", program.display())))
}

/// The program as the loader maps it, at its preferred base: each section's
/// stored bytes at its address. Space with no stored bytes reads as zeros.
struct Image {
    base: u64,
    bytes: Vec<u8>,
}

impl Image {
    fn read(program: &Path) -> Result<Self> {
        let file = fs::read(program)
            .map_err(|error| asset_error(format!("{}: {error}", program.display())))?;
        Self::map(&file)
            .ok_or_else(|| asset_error(format!("{} is not a 64-bit program", program.display())))
    }
}

/// One field of a message layout.
struct Field {
    kind: u32,
    offset: u32,
    size: u32,
    count: i32,
    array: bool,
    fields: Option<Vec<Field>>,
}

type Schemas = BTreeMap<u32, BTreeMap<u32, Vec<Field>>>;

const ITEM: usize = 0x30;
const END: u32 = 17;
const STRUCT: u32 = 14;
/// How many items a list may have and how deep structs may nest before the
/// reader takes a record for something else.
const MAX_ITEMS: usize = 300;
const MAX_DEPTH: usize = 16;

impl Image {
    fn map(file: &[u8]) -> Option<Self> {
        let pe = u32_at(file, 0x3C)? as usize;
        if file.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        let sections = usize::from(u16_at(file, pe + 6)?);
        let optional_size = usize::from(u16_at(file, pe + 20)?);
        let optional = pe + 24;
        // PE32+.
        if u16_at(file, optional)? != 0x20B {
            return None;
        }
        let base = u64_at(file, optional + 24)?;
        let size = u32_at(file, optional + 56)? as usize;
        let mut bytes = vec![0; size];
        for section in 0..sections {
            let header = optional + optional_size + 40 * section;
            let virtual_size = u32_at(file, header + 8)? as usize;
            let address = u32_at(file, header + 12)? as usize;
            let stored = u32_at(file, header + 16)? as usize;
            let start = u32_at(file, header + 20)? as usize;
            let length = stored.min(virtual_size);
            bytes
                .get_mut(address..address + length)?
                .copy_from_slice(file.get(start..start + length)?);
        }
        Some(Self { base, bytes })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        u32_at(&self.bytes, at)
    }

    /// The image offset a pointer stored in the image points at.
    fn target(&self, at: usize) -> Option<usize> {
        let pointer = u64_at(&self.bytes, at)?;
        let offset = pointer.checked_sub(self.base)?;
        (offset < self.bytes.len() as u64).then_some(offset as usize)
    }

    /// Every message of `protocols`: a `JamMessageInfo` is found by its CRC
    /// and id, and taken when its items read as a layout. The first one
    /// found for a message wins.
    fn schemas(&self, protocols: &[u32]) -> Schemas {
        let wanted: HashSet<u32> = protocols.iter().copied().collect();
        let mut found: HashSet<(u32, u32)> = HashSet::new();
        let mut schemas = Schemas::new();
        for at in (0x18..self.bytes.len().saturating_sub(8)).step_by(8) {
            let (Some(crc), Some(id)) = (self.u32(at), self.u32(at + 4)) else {
                break;
            };
            if !wanted.contains(&crc) || !(1..70_000).contains(&id) || found.contains(&(crc, id)) {
                continue;
            }
            let Some(items) = self.target(at - 8) else {
                continue;
            };
            if self.valid(items, 0) {
                found.insert((crc, id));
                schemas
                    .entry(crc)
                    .or_default()
                    .insert(id, self.fields(items, 0));
            }
        }
        schemas
    }

    fn valid(&self, mut item: usize, depth: usize) -> bool {
        for _ in 0..MAX_ITEMS {
            if item + ITEM > self.bytes.len() {
                return false;
            }
            let (Some(kind), Some(offset), Some(size), Some(count)) = (
                self.u32(item + 8),
                self.u32(item + 12),
                self.u32(item + 16),
                self.u32(item + 20),
            ) else {
                return false;
            };
            let count = count as i32;
            if kind == END {
                return true;
            }
            if kind > END || offset > 0x10000 || size > 0x10000 || !(-1..=0x1000).contains(&count) {
                return false;
            }
            if kind == STRUCT {
                let inner = self.target(item + 0x18);
                if inner.is_none_or(|inner| depth > MAX_DEPTH || !self.valid(inner, depth + 1)) {
                    return false;
                }
            }
            item += ITEM;
        }
        false
    }

    /// The fields of a list [`Self::valid`] accepted.
    fn fields(&self, mut item: usize, depth: usize) -> Vec<Field> {
        let mut fields = Vec::new();
        for _ in 0..MAX_ITEMS {
            let read = |at| self.u32(item + at).expect("a valid list is in the image");
            let kind = read(8);
            if kind == END {
                break;
            }
            let inner = self.target(item + 0x18);
            fields.push(Field {
                kind,
                offset: read(12),
                size: read(16),
                count: read(20) as i32,
                array: u64_at(&self.bytes, item + 0x20).is_some_and(|resize| resize != 0),
                fields: inner
                    .filter(|_| kind == STRUCT && depth < MAX_DEPTH)
                    .map(|inner| self.fields(inner, depth + 1)),
            });
            item += ITEM;
        }
        fields
    }

    /// The `FileDescriptorSet` of the Battle.net files the server loads.
    fn descriptors(&self) -> std::result::Result<Vec<u8>, String> {
        let files = self.proto_files();
        let mut wanted: BTreeSet<&str> = files
            .keys()
            .map(String::as_str)
            .filter(|name| name.starts_with(BNET_PREFIX))
            .collect();
        if wanted.is_empty() {
            return Err("no Battle.net protobuf descriptors".into());
        }
        let mut queue: Vec<&str> = wanted.iter().copied().collect();
        while let Some(name) = queue.pop() {
            for import in dependencies(files[name]) {
                let Some((known, _)) = files.get_key_value(import.as_str()) else {
                    return Err(format!("{name} imports {import}, which the program lacks"));
                };
                if wanted.insert(known) {
                    queue.push(known);
                }
            }
        }
        let mut set = Vec::new();
        for name in wanted {
            let file = files[name];
            set.push(0x0A);
            varint(&mut set, file.len() as u64);
            set.extend_from_slice(file);
        }
        Ok(set)
    }

    /// Every serialized `FileDescriptorProto` in the image by its file
    /// name: field 1 (the name, ending `.proto`) first, then fields up to
    /// the first byte that can't start one (the zero after it).
    fn proto_files(&self) -> BTreeMap<String, &[u8]> {
        let bytes = &self.bytes;
        let mut files = BTreeMap::new();
        for at in memchr_all(bytes, 0x0A) {
            let Some(&length) = bytes.get(at + 1) else {
                break;
            };
            let length = usize::from(length);
            if !(7..0x80).contains(&length) {
                continue;
            }
            let Some(name) = bytes.get(at + 2..at + 2 + length) else {
                continue;
            };
            if !name.ends_with(b".proto")
                || !name
                    .iter()
                    .all(|&byte| byte.is_ascii_alphanumeric() || b"_/.".contains(&byte))
            {
                continue;
            }
            let end = message_end(bytes, at);
            let name = String::from_utf8_lossy(name).into_owned();
            files.entry(name).or_insert(&bytes[at..end]);
        }
        files
    }

    fn blob_key(&self) -> Option<&[u8]> {
        let key = self.bytes.get(BLOB_KEY_RVA..BLOB_KEY_RVA + 64)?;
        (hex(&Sha256::digest(key)) == BLOB_KEY_SHA256).then_some(key)
    }
}

/// The offsets of every `byte` in `bytes`.
fn memchr_all(bytes: &[u8], byte: u8) -> impl Iterator<Item = usize> + '_ {
    bytes
        .iter()
        .enumerate()
        .filter(move |&(_, &found)| found == byte)
        .map(|(at, _)| at)
}

/// Where the protobuf message starting at `start` ends: at the first byte
/// that is no field's tag (field 0, or a field number above the
/// descriptor's), or one whose value runs past the image.
fn message_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start;
    loop {
        let Some((tag, value)) = read_varint(bytes, at) else {
            return at;
        };
        let field = tag >> 3;
        if !(1..=20).contains(&field) {
            return at;
        }
        let next = match tag & 7 {
            0 => read_varint(bytes, value).map(|(_, next)| next),
            1 => Some(value + 8),
            2 => read_varint(bytes, value).map(|(length, next)| next + length as usize),
            5 => Some(value + 4),
            _ => None,
        };
        match next {
            Some(next) if next <= bytes.len() => at = next,
            _ => return at,
        }
    }
}

/// The files a `FileDescriptorProto` imports (field 3).
fn dependencies(file: &[u8]) -> Vec<String> {
    let mut imports = Vec::new();
    let mut at = 0;
    while at < file.len() {
        let Some((tag, value)) = read_varint(file, at) else {
            break;
        };
        let Some(next) = (match tag & 7 {
            0 => read_varint(file, value).map(|(_, next)| next),
            1 => Some(value + 8),
            2 => read_varint(file, value).map(|(length, next)| {
                if tag >> 3 == 3 {
                    let name = file.get(next..next + length as usize).unwrap_or_default();
                    imports.push(String::from_utf8_lossy(name).into_owned());
                }
                next + length as usize
            }),
            5 => Some(value + 4),
            _ => None,
        }) else {
            break;
        };
        at = next;
    }
    imports
}

fn read_varint(bytes: &[u8], mut at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(at)?;
        at += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte < 0x80 {
            return Some((value, at));
        }
    }
    None
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// The layouts as `{"<CRC>": {"<id>": [field, …]}}`, protocols and messages
/// in order, written as the Python dumper wrote them (`json.dumps`).
fn schemas_json(schemas: &Schemas) -> String {
    let mut out = String::from("{");
    for (number, (crc, messages)) in schemas.iter().enumerate() {
        if number > 0 {
            out.push_str(", ");
        }
        let _ = write!(out, "\"{crc:08X}\": {{");
        for (number, (id, fields)) in messages.iter().enumerate() {
            if number > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "\"{id}\": ");
            fields_json(&mut out, fields);
        }
        out.push('}');
    }
    out.push('}');
    out
}

fn fields_json(out: &mut String, fields: &[Field]) {
    out.push('[');
    for (number, field) in fields.iter().enumerate() {
        if number > 0 {
            out.push_str(", ");
        }
        let _ = write!(
            out,
            "{{\"type\": {}, \"off\": {}, \"size\": {}, \"count\": {}, \"array\": {}",
            field.kind, field.offset, field.size, field.count, field.array
        );
        if let Some(inner) = &field.fields {
            out.push_str(", \"fields\": ");
            fields_json(out, inner);
        }
        out.push('}');
    }
    out.push(']');
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}
