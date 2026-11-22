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
//! a volume other placeables name by its UUID, made of boxes, spheres and
//! capsules (TankLib's `teMapPlaceableArea`). Its body: a 72-byte header with
//! `u16` counts at +12 (boxes), +14 (spheres), +16 (capsules), +18 and +20
//! (two unknown shapes, none in 1.74), then `u32` offsets from the body's
//! start at +24 (boxes), +28 (spheres), +32 (capsules), +36 and +40 (the
//! unknown shapes; in 1.74 both are the body's end). Each kind ends where
//! the next starts. A box is 40 bytes: rotation `{x, y, z, w}`, world
//! centre, half extents (refs report `CaptureAreas.md` §3); a sphere 16:
//! world centre, radius; a capsule 40: rotation, world centre, `f32` height,
//! radius and a zero. 1.74 data for the capsule: one turned 90° about z with
//! `{30.76, 5.0}` sits among boxes of half extents about `{19.5, 5.0}` along
//! the same axes, so the second is the radius; whether the height counts the
//! caps is an assumption left open.

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
/// An area body's header and its shapes' sizes (see the module notes).
const AREA_HEADER: usize = 72;
const BOX_SIZE: usize = 40;
const SPHERE_SIZE: usize = 16;
const CAPSULE_SIZE: usize = 40;

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
    /// Identifier 2: what `STU_A3DC6A25` compares (the client's
    /// `sub_7FF78990C1B0` reads the placeable's body `+16`).
    pub second_identifier: u64,
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
            second_identifier: u64_at(data, body + 16),
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

/// An area placeable's shapes, each as JSON: boxes `{"centre", "half",
/// "rotation"}`, spheres `{"centre", "radius"}`, capsules `{"centre",
/// "rotation", "height", "radius"}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Area {
    pub boxes: Vec<Json>,
    pub spheres: Vec<Json>,
    pub capsules: Vec<Json>,
}

impl Area {
    /// The boxes of an area that is only boxes, the error naming `what`
    /// otherwise: the server tests bodies against boxes only.
    pub fn only_boxes(&self, what: &str) -> Result<&[Json]> {
        if self.boxes.is_empty() || !self.spheres.is_empty() || !self.capsules.is_empty() {
            return Err(asset_error(format!(
                "{what}: its area is not a box area ({} boxes, {} spheres, {} capsules)",
                self.boxes.len(),
                self.spheres.len(),
                self.capsules.len()
            )));
        }
        Ok(&self.boxes)
    }
}

/// The areas of a map by UUID.
pub(crate) fn areas(chunks: &[(u64, Vec<u8>)]) -> Result<HashMap<[u8; 16], Area>> {
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
            let area = area(body).map_err(|reason| {
                asset_error(format!(
                    "chunk {guid:016X} area at {:#X}: {reason}",
                    placeable.offset
                ))
            })?;
            out.insert(uuid, area);
        }
    }
    Ok(out)
}

/// An area placeable's body.
fn area(body: &[u8]) -> std::result::Result<Area, String> {
    if body.len() < AREA_HEADER {
        return Err(format!("{} bytes, shorter than its header", body.len()));
    }
    let count = |at: usize| usize::from(u16::from_le_bytes([body[at], body[at + 1]]));
    let offset = |k: usize| u32_at(body, 24 + 4 * k) as usize;
    let unknown = [count(18), count(20)];
    if unknown != [0, 0] {
        return Err(format!("unknown shape counts {unknown:?}"));
    }
    // Where each shape starts, by kind: boxes, spheres, capsules.
    let mut kinds: [Vec<usize>; 3] = Default::default();
    for (k, size) in [BOX_SIZE, SPHERE_SIZE, CAPSULE_SIZE]
        .into_iter()
        .enumerate()
    {
        let (start, count) = (offset(k), count(12 + 2 * k));
        if start + count * size != offset(k + 1) {
            return Err(format!(
                "shape kind {k}: {count} at {start:#X} do not end at the next kind"
            ));
        }
        kinds[k] = (0..count).map(|n| start + n * size).collect();
    }
    if offset(3) != body.len() || offset(4) != body.len() {
        return Err("the shapes do not end at the body's end".to_owned());
    }
    let floats = |at: usize, n: usize| -> Vec<f64> {
        (0..n).map(|i| short(f32_at(body, at + 4 * i))).collect()
    };
    let float = |at: usize| short(f32_at(body, at));
    let [boxes, spheres, capsules] = kinds;
    Ok(Area {
        boxes: boxes
            .into_iter()
            .map(|at| {
                json!({"centre": floats(at + 16, 3), "half": floats(at + 28, 3), "rotation": floats(at, 4)})
            })
            .collect(),
        spheres: spheres
            .into_iter()
            .map(|at| json!({"centre": floats(at, 3), "radius": float(at + 12)}))
            .collect(),
        capsules: capsules
            .into_iter()
            .map(|at| {
                json!({
                    "centre": floats(at + 16, 3),
                    "rotation": floats(at, 4),
                    "height": float(at + 28),
                    "radius": float(at + 32),
                })
            })
            .collect(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An area body with `counts` (boxes, spheres, capsules) of shapes
    /// whose floats count up from 1.
    fn area_body(counts: [u16; 3]) -> Vec<u8> {
        let mut body = vec![0; AREA_HEADER];
        let mut at = AREA_HEADER;
        let mut offsets = Vec::new();
        for (k, (count, size)) in counts
            .into_iter()
            .zip([BOX_SIZE, SPHERE_SIZE, CAPSULE_SIZE])
            .enumerate()
        {
            body[12 + 2 * k..14 + 2 * k].copy_from_slice(&count.to_le_bytes());
            offsets.push(at);
            at += usize::from(count) * size;
        }
        offsets.extend([at, at]);
        for (k, offset) in offsets.into_iter().enumerate() {
            body[24 + 4 * k..28 + 4 * k].copy_from_slice(&(offset as u32).to_le_bytes());
        }
        let floats = (at - AREA_HEADER) / 4;
        for n in 1..=floats {
            body.extend_from_slice(&(n as f32).to_le_bytes());
        }
        body
    }

    #[test]
    fn an_area_reads_its_boxes_spheres_and_capsules() {
        let area = area(&area_body([1, 1, 1])).unwrap();
        assert_eq!(
            area.boxes,
            [
                json!({"rotation": [1.0, 2.0, 3.0, 4.0], "centre": [5.0, 6.0, 7.0], "half": [8.0, 9.0, 10.0]})
            ]
        );
        assert_eq!(
            area.spheres,
            [json!({"centre": [11.0, 12.0, 13.0], "radius": 14.0})]
        );
        assert_eq!(
            area.capsules,
            [
                json!({"rotation": [15.0, 16.0, 17.0, 18.0], "centre": [19.0, 20.0, 21.0], "height": 22.0, "radius": 23.0})
            ]
        );
        assert!(area.only_boxes("x").is_err());
        let boxes = super::area(&area_body([2, 0, 0])).unwrap();
        assert_eq!(boxes.only_boxes("x").unwrap().len(), 2);
    }

    #[test]
    fn an_area_with_unknown_shapes_or_loose_offsets_is_an_error() {
        let mut body = area_body([1, 0, 0]);
        body[18] = 1;
        assert!(area(&body).unwrap_err().contains("unknown shape"));
        let mut body = area_body([1, 1, 0]);
        // The spheres start a word late.
        body[28..32].copy_from_slice(&(AREA_HEADER as u32 + 44).to_le_bytes());
        assert!(area(&body).is_err());
        assert!(area(&[0; 40]).is_err());
    }

    /// Every area of the 1.74 maps reads; boxes are the rule, 13 areas have
    /// spheres and 65 capsules (none of them named by a capture point, spawn
    /// room or damage area: their derivations refuse areas that are not all
    /// boxes, and the pack tests pass).
    #[test]
    #[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
    fn every_area_of_the_install_reads() {
        let install = Install::open(&lucy_testkit::game_dir(), "enUS").unwrap();
        let (mut areas, mut spheres, mut capsules) = (0, 0, 0);
        for chunk in install.guids(MAP_CHUNK) {
            if chunk_kind(chunk) != AREA_CHUNK {
                continue;
            }
            let data = install.raw(chunk).unwrap();
            for placeable in placeables(&data) {
                if placeable.kind != AREA {
                    continue;
                }
                let size = u32_at(&data, placeable.offset + 20) as usize;
                let read = area(&data[placeable.offset + 24..placeable.offset + size])
                    .unwrap_or_else(|error| panic!("{chunk:016X}: {error}"));
                areas += 1;
                spheres += usize::from(!read.spheres.is_empty());
                capsules += usize::from(!read.capsules.is_empty());
            }
        }
        assert_eq!((areas, spheres, capsules), (2340, 13, 65));
    }
}
