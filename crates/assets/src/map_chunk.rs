//! Map chunks (asset type `0BC`), shared by the derivations that read maps:
//! the placeables of a chunk, the entity placeables of the ENTITY chunk with
//! their component instance data, and that instance data's structured data
//! (STUv1).
//!
//! A map's chunks are GUIDs in the manifests: `0x0DD0 << 48 | kind << 32 |
//! the map's index` (TankLib's `GetChunkKey`), kind `0x0B` the entity
//! placeables, `0x11` the collision shapes.
//!
//! A chunk of placeables, as TankLib's `teMapPlaceableData` reads it: `{u32
//! placeable count, u32 instance data offset, u32 placeable offset, u32}`; per
//! placeable a 24-byte header `{uuid, u16, u8, u8 type, u32 size}` (the type
//! is byte 19, the size counts the header) and its body. An entity
//! placeable's body: `{u64 definition (003), u64 identifier 1 (01C), u64
//! identifier 2, f32 translation[3], scale[3], rotation[4] (x, y, z, w), u32
//! instance count, ...}`, its client id (u16) at +104 and from +112 one `{u32
//! type, u32 offset from that entry}` per component instance, each an STUv1
//! blob.
//!
//! An area placeable (chunk kind [`AREA_CHUNK`], placeable type [`AREA`]) is
//! a volume other placeables name by its UUID: a body of `{u32 ×3, u32 box
//! count (low 16 bits), u32, u32 flags, u32 offsets [6..13] from the body
//! start}`, the boxes from offset [6] to [7], each 40 bytes: rotation `{x, y,
//! z, w}`, world centre, half extents (refs report `CaptureAreas.md` §3).

use std::collections::HashMap;

use serde_json::{Value as Json, json};

use crate::{Install, Result, asset_error};

/// The asset type of map chunks.
pub(crate) const MAP_CHUNK: u16 = 0x0BC;
/// The chunk kind of the entity placeables.
pub(crate) const ENTITY: u16 = 0x0B;
/// The chunk kind of the static collision shapes.
pub(crate) const COLLISION: u16 = 0x11;
/// The chunk kind of the area placeables.
pub(crate) const AREA_CHUNK: u16 = 0x0A;
const BOX_SIZE: usize = 40;

/// The placeable types, byte 19 of each placeable.
pub(crate) const LIGHT: u8 = 7;
pub(crate) const AREA: u8 = 8;
pub(crate) const ENTITY_PLACEABLE: u8 = 9;

/// The GUID of a map's chunk of one kind.
pub(crate) fn chunk_guid(map: u64, kind: u16) -> u64 {
    0x0DD0_0000_0000_0000 | u64::from(kind) << 32 | (map & 0xFFFF_FFFF)
}

/// The kind of a chunk GUID (the four upper digits of its DataTool name).
pub(crate) fn chunk_kind(guid: u64) -> u16 {
    (guid >> 32) as u16
}

/// A map chunk's bytes, `None` when the install has no such chunk.
pub(crate) fn read_chunk(install: &Install, map: u64, kind: u16) -> Result<Option<Vec<u8>>> {
    let guid = chunk_guid(map, kind);
    if install.game().asset(guid).is_none() {
        return Ok(None);
    }
    install.raw(guid).map(Some)
}

/// One placeable: where its header starts and its type.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placeable {
    pub offset: usize,
    pub kind: u8,
}

/// The placeables of a chunk, in chunk order; empty for chunks in other
/// formats (a non-zero fourth header word, no placeable offset) and for
/// chunks whose placeables run past the end.
pub(crate) fn placeables(data: &[u8]) -> Vec<Placeable> {
    if data.len() < 16 {
        return Vec::new();
    }
    let count = u32_at(data, 0);
    let mut offset = u32_at(data, 8) as usize;
    if u32_at(data, 12) != 0 || offset == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if offset + 24 > data.len() {
            return Vec::new();
        }
        let kind = data[offset + 19];
        let length = u32_at(data, offset + 20) as usize;
        if length < 24 || offset + length > data.len() {
            return Vec::new();
        }
        out.push(Placeable { offset, kind });
        offset += length;
    }
    out
}

/// An entity placeable of the ENTITY chunk.
#[derive(Clone, Debug)]
pub(crate) struct Entity<'a> {
    data: &'a [u8],
    body: usize,
    /// Its number among the chunk's placeables.
    pub index: usize,
    pub definition: u64,
    pub identifier: u64,
    pub translation: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 4],
    /// Non-zero when the client makes the placeable itself.
    pub client_id: u16,
}

/// The size of an entity body up to its instance table.
const ENTITY_BODY: usize = 112;

/// The entity placeables of a chunk, in chunk order.
pub(crate) fn entities(data: &[u8]) -> Result<Vec<Entity<'_>>> {
    placeables(data)
        .into_iter()
        .enumerate()
        .filter(|(_, placeable)| placeable.kind == ENTITY_PLACEABLE)
        .map(|(index, placeable)| Entity::read(data, index, placeable.offset + 24))
        .collect()
}

impl<'a> Entity<'a> {
    fn read(data: &'a [u8], index: usize, body: usize) -> Result<Self> {
        if body + ENTITY_BODY > data.len() {
            return Err(asset_error("entity placeable runs past its chunk"));
        }
        let floats = |at: usize, out: &mut [f32]| {
            for (k, value) in out.iter_mut().enumerate() {
                *value = f32_at(data, body + at + 4 * k);
            }
        };
        let mut translation = [0.0; 3];
        let mut scale = [0.0; 3];
        let mut rotation = [0.0; 4];
        floats(24, &mut translation);
        floats(36, &mut scale);
        floats(48, &mut rotation);
        Ok(Self {
            data,
            body,
            index,
            definition: u64_at(data, body),
            identifier: u64_at(data, body + 8),
            translation,
            scale,
            rotation,
            client_id: u16::from_le_bytes([data[body + 104], data[body + 105]]),
        })
    }

    /// The placeable's UUID: the first 16 bytes of its header, which other
    /// placeables link to.
    pub fn uuid(&self) -> [u8; 16] {
        self.data[self.body - 24..self.body - 8]
            .try_into()
            .expect("16 bytes")
    }

    /// The component instance data blobs in STUv1, in the placeable's order;
    /// blobs in another format are skipped, as DataTool skips what it can't
    /// read.
    pub fn instances(&self) -> impl Iterator<Item = StuV1<'a>> + '_ {
        let count = u32_at(self.data, self.body + 64) as usize;
        (0..count).filter_map(move |k| {
            let entry = self.body + ENTITY_BODY + 8 * k;
            let blob = entry + self.data.get(entry + 4..entry + 8).map(read_u32)? as usize;
            StuV1::new(self.data, blob)
        })
    }
}

/// An STUv1 blob (bytes "DUTS", u32 instance count, then `{i32 offset, u32}` per
/// instance, offsets from the blob's start). An instance is `{u32 class hash,
/// u32}` and then its fields at the offsets of the class layout. In TankLib's
/// reader: an asset reference is 16 bytes, the GUID in its second half; an
/// embedded instance an `{i32 offset, i32}`; an array an `{i32 offset, i32}`
/// of `{i64 count, i64 offset}` of the elements (embedded instances 8 bytes
/// each, inline instances their class size); a string an `{i64 offset, i64}`
/// of `{i32 size, u32, i64 offset}` of the UTF-8 bytes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StuV1<'a> {
    data: &'a [u8],
    start: usize,
}

const MAGIC: &[u8; 4] = b"DUTS";

impl<'a> StuV1<'a> {
    pub fn new(data: &'a [u8], start: usize) -> Option<Self> {
        (data.get(start..start + 4)? == MAGIC).then_some(Self { data, start })
    }

    /// The first instance, the one TankLib's map reader returns.
    pub fn root(&self) -> Option<usize> {
        if self.u32(4)? == 0 {
            return None;
        }
        let offset = self.i32(8)?;
        (offset > 0).then_some(offset as usize)
    }

    /// The class hash of the instance at an offset.
    pub fn class(&self, instance: usize) -> Option<u32> {
        self.u32(instance)
    }

    pub fn u8(&self, at: usize) -> Option<u8> {
        self.data.get(self.start + at).copied()
    }

    pub fn u32(&self, at: usize) -> Option<u32> {
        let at = self.start + at;
        self.data.get(at..at + 4).map(read_u32)
    }

    pub fn i32(&self, at: usize) -> Option<i32> {
        self.u32(at).map(|value| value as i32)
    }

    pub fn i16(&self, at: usize) -> Option<i16> {
        let at = self.start + at;
        self.data
            .get(at..at + 2)
            .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn f32(&self, at: usize) -> Option<f32> {
        self.u32(at).map(f32::from_bits)
    }

    pub fn u64(&self, at: usize) -> Option<u64> {
        let at = self.start + at;
        self.data
            .get(at..at + 8)
            .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    /// A `teUUID` field: 16 bytes.
    pub fn uuid(&self, at: usize) -> Option<[u8; 16]> {
        let at = self.start + at;
        self.data.get(at..at + 16)?.try_into().ok()
    }

    /// The GUID of an asset reference field.
    pub fn asset_ref(&self, at: usize) -> Option<u64> {
        self.u64(at + 8)
    }

    /// The instance an embedded instance field names.
    pub fn embedded(&self, at: usize) -> Option<usize> {
        let offset = self.i32(at)?;
        (offset > 0).then_some(offset as usize)
    }

    /// The elements of an array field: (count, where the first one is).
    pub fn array(&self, at: usize) -> Option<(usize, usize)> {
        let offset = self.i32(at)?;
        if offset <= 0 {
            return None;
        }
        let offset = offset as usize;
        let count = self.u64(offset)? as i64;
        let first = self.u64(offset + 8)? as i64;
        (count >= 0 && first > 0).then_some((count as usize, first as usize))
    }

    /// The instances an array of embedded instances names, in order.
    pub fn embedded_array(&self, at: usize) -> Vec<usize> {
        let Some((count, first)) = self.array(at) else {
            return Vec::new();
        };
        (0..count)
            .filter_map(|k| self.embedded(first + 8 * k))
            .collect()
    }

    /// A string field, `None` when it has none.
    pub fn string(&self, at: usize) -> Option<String> {
        let info = self.u64(at)? as i64;
        if info <= 0 {
            return None;
        }
        let info = info as usize;
        let size = self.i32(info)?;
        if size == 0 {
            return Some(String::new());
        }
        let offset = self.start + self.u64(info + 8)? as usize;
        let bytes = self
            .data
            .get(offset..offset + usize::try_from(size).ok()?)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    read_u32(&data[at..at + 4])
}

/// The box areas of a map: area UUID -> its boxes as
/// `{"centre", "half", "rotation"}` (an area that lists no box is left out).
pub(crate) fn area_boxes(chunks: &[(u64, Vec<u8>)]) -> HashMap<[u8; 16], Vec<Json>> {
    let mut out = HashMap::new();
    for (guid, data) in chunks {
        if chunk_kind(*guid) != AREA_CHUNK {
            continue;
        }
        for placeable in placeables(data) {
            if placeable.kind != AREA {
                continue;
            }
            let uuid: [u8; 16] = data[placeable.offset..placeable.offset + 16]
                .try_into()
                .expect("16 bytes");
            let size = u32_at(data, placeable.offset + 20) as usize;
            let body = &data[placeable.offset + 24..placeable.offset + size];
            if let Some(boxes) = boxes(body) {
                out.insert(uuid, boxes);
            }
        }
    }
    out
}

/// An area placeable's boxes, `None` for a body that lists none or that
/// runs past its placeable.
fn boxes(body: &[u8]) -> Option<Vec<Json>> {
    let word = |k: usize| (k * 4 + 4 <= body.len()).then(|| u32_at(body, k * 4));
    let count = (word(3)? & 0xFFFF) as usize;
    let (start, end) = (word(6)? as usize, word(7)? as usize);
    if count == 0 || end != start + count * BOX_SIZE || end > body.len() {
        return None;
    }
    Some(
        (0..count)
            .map(|k| {
                let at = start + k * BOX_SIZE;
                let floats = |from: usize, n: usize| -> Vec<f64> {
                    (0..n)
                        .map(|i| short(f32_at(body, at + from + 4 * i)))
                        .collect()
                };
                json!({"centre": floats(16, 3), "half": floats(28, 3), "rotation": floats(0, 4)})
            })
            .collect(),
    )
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

fn f32_at(data: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(data, at))
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}

/// A GUID from its index and asset type, as DataTool's file names give
/// them: the type sits bit-reversed at bits 48..59, the other bits of a full
/// GUID are dropped.
pub(crate) fn guid_of(index: u64, kind: u16) -> u64 {
    let reversed = (u64::from(kind - 1) & 0xFFF).reverse_bits() >> 52;
    reversed << 48 | index
}

/// The index part of a GUID that DataTool's file names show (the lower 48
/// bits).
pub(crate) fn name_index(guid: u64) -> u64 {
    guid & 0xFFFF_FFFF_FFFF
}

/// The shortest decimal that is still the same 32-bit float, as a 64-bit
/// float: the value correctly rounded to 1, 2, ... 9 significant digits, the
/// first that reads back as the same float (Python's `f"{value:.{n}g}"`; a
/// shortest-digits formatter may pick another digit string of the same
/// length).
pub(crate) fn short(value: f32) -> f64 {
    for digits in 0..9 {
        let text = format!("{:.digits$e}", f64::from(value));
        let parsed: f64 = text.parse().expect("a float formats");
        if (parsed as f32).to_bits() == value.to_bits() {
            return parsed;
        }
    }
    f64::from(value)
}

/// `value` rounded to `digits` decimals like Python's `round`.
pub(crate) fn round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}")
        .parse()
        .expect("a float formats")
}
