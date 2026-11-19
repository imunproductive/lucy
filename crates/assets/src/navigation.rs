//! `data/navigation/<map>.json.gz`: each map's AI tactical volumes, the
//! navigation graph the server's AI heroes path over. The server's reader is
//! `orpheus_physics::nav`.
//!
//! Where it comes from: each map has one chunk of kind 0x12 (see
//! [`crate::map_chunk`]), present for all 132 maps. Its header is `{u64
//! document 0 offset, u64 document 1 offset, u64 record count, u64 record
//! offset}`; the records are 64 bytes each (an identity-like matrix; not
//! read). 54 maps (menus, lobbies, unused maps) have both offsets 0 and no
//! navigation; the other 78 have two STUv1 documents, and 77 of them have
//! volumes (map `06C7` has none).
//!
//! Document 1 is the tactical volume set, root `STU_5AE64839` at instance
//! offset `+0x20`. Its fields sit at fixed offsets from the instance, each
//! array `{i32 offset, ...}` 16 bytes, in the type table's field order:
//! `+0x08 m_dynamicTreeDef` (inline: `m_aNodes` byte array of 64-byte AABB
//! tree nodes, `+0x18 m_nodeCount`, `+0x1C m_root`, `+0x20 m_proxyCount`),
//! `+0x28 m_volumes`, `+0x38 m_chokes` (empty on every map), `+0x48
//! m_B1E00012` (one int), `+0x58 m_metaData` (AI level-placed markers),
//! `+0x68 m_276CC7EC`. A volume (`STUAITacticalVolume`, 0x68 bytes inline):
//! - `+0x00 m_42286A80` u32 words: the walkable voxel mask, bit `z + Sz·(x +
//!   Sx·y)` for the cell `origin + (x, y, z)`;
//! - `+0x10 m_63EE0EDB` u32 words: the walkable cells with a same-layer
//!   8-neighbour outside the mask (the mask's border; computed again by the
//!   reader, not written);
//! - `+0x20 m_CF7CD15A` the neighbour volumes (`m_index`, s32 each);
//! - `+0x30 m_7D1FC63E` the floor point (f32 x, y, z);
//! - `+0x3C` five floats and `+0x50 m_FEF6FAAD` an int, not understood;
//! - `+0x54 m_id` the volume's own index;
//! - `+0x58` six s16: the origin cell (x, y, z) and the size (Sx, Sy, Sz)
//!   in voxels of 1 × 1.4 × 1 m (y in 1.4 m layers). On Temple of Anubis each
//!   tree leaf (64 bytes: `f32 min[4], max[4], u64 volume, i32 parent,
//!   child 1, child 2, height`, children −1) bounds its volume from origin to
//!   origin + size in these units; the reader doesn't need the tree.
//!
//! Checked on the 77 maps: every volume's id is its index, the tree has one
//! proxy per volume, each mask has `ceil(Sx·Sy·Sz / 32)` words, the floor
//! point lies in a walkable cell of its volume (its layer is `round(y /
//! 0.35) / 4`: floors sit on the 0.35 m grid, a float error above or below),
//! and every neighbour link is symmetric.
//!
//! Document 0 (`STU_3BC32A6E`, the AI cell data, the client's own path
//! grids) is derived by [`crate::ai_cells`].

use std::{io::Write, path::Path};

use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};

use crate::{
    Install, Output, Result, asset_error, guid_hex, guid_name,
    map_chunk::{MAP_CHUNK, StuV1, chunk_kind, guid_of, short},
};

/// The chunk kind of the navigation data.
const NAVIGATION: u16 = 0x12;
const MAP: u16 = 0x002;
/// The root class of document 1.
const TACTICAL_VOLUMES: u32 = 0x5AE6_4839;
/// Root field offsets.
const PROXY_COUNT: usize = 0x20;
const VOLUMES: usize = 0x28;
/// A volume's size and field offsets.
const VOLUME: usize = 0x68;
const WALKABLE: usize = 0x00;
const NEIGHBOURS: usize = 0x20;
const FLOOR: usize = 0x30;
const ID: usize = 0x54;
const ORIGIN: usize = 0x58;
const SIZE: usize = 0x5E;

/// The derivation: one file per map that has navigation.
pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut outputs = Vec::new();
    for chunk in install
        .guids(MAP_CHUNK)
        .into_iter()
        .filter(|guid| chunk_kind(*guid) == NAVIGATION)
    {
        let index = chunk & 0xFFFF_FFFF;
        let data = install.raw(chunk)?;
        let volumes = volumes(&data)
            .map_err(|reason| asset_error(format!("{}: {reason}", guid_name(chunk))))?;
        let Some(volumes) = volumes else {
            continue;
        };
        let map = guid_of(index, MAP);
        let file = json!({
            "map": guid_hex(map),
            "chunk": guid_hex(chunk),
            "volumes": volumes,
        });
        outputs.push(gzip(format!("data/navigation/{index:04X}.json.gz"), &file)?);
    }
    Ok(outputs)
}

/// Compact JSON, gzipped at level 9 with no timestamp.
fn gzip(path: String, value: &Value) -> Result<Output> {
    let text = serde_json::to_vec(value).expect("JSON values serialize");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(9));
    encoder.write_all(&text)?;
    Ok(Output {
        path,
        bytes: encoder.finish()?,
    })
}

/// The tactical volumes of a navigation chunk, `None` when the map has none
/// (no documents, or, on map `06C7`, no volumes).
fn volumes(data: &[u8]) -> Result<Option<Vec<Value>>, String> {
    let offset = data
        .get(8..16)
        .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
        .ok_or("chunk header cut short")?;
    if offset == 0 {
        return Ok(None);
    }
    let stu = usize::try_from(offset)
        .ok()
        .and_then(|offset| StuV1::new(data, offset))
        .ok_or("document 1 is not STUv1")?;
    let root = stu.root().ok_or("document 1 has no root")?;
    if stu.class(root) != Some(TACTICAL_VOLUMES) {
        return Err(format!(
            "document 1 root is {:08X}",
            stu.class(root).unwrap_or(0)
        ));
    }
    let Some((count, first)) = stu.array(root + VOLUMES) else {
        return Ok(None);
    };
    if stu.i32(root + PROXY_COUNT) != i32::try_from(count).ok() {
        return Err("the AABB tree's proxy count isn't the volume count".into());
    }
    (0..count)
        .map(|index| volume(&stu, first + VOLUME * index, index, count))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn volume(stu: &StuV1<'_>, at: usize, index: usize, count: usize) -> Result<Value, String> {
    let cut = || format!("volume {index} cut short");
    if stu.i32(at + ID) != i32::try_from(index).ok() {
        return Err(format!("volume {index} has another id"));
    }
    let floor = (0..3)
        .map(|axis| stu.f32(at + FLOOR + 4 * axis).map(short))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(cut)?;
    let shorts = |base: usize| {
        (0..3)
            .map(|axis| stu.i16(at + base + 2 * axis))
            .collect::<Option<Vec<_>>>()
    };
    let origin = shorts(ORIGIN).ok_or_else(cut)?;
    let size = shorts(SIZE).ok_or_else(cut)?;
    if size.iter().any(|&side| side <= 0) {
        return Err(format!("volume {index} has an empty size"));
    }
    let cells: usize = size.iter().map(|&side| side as usize).product();
    let walkable = words(stu, at + WALKABLE).ok_or_else(cut)?;
    if walkable.len() != cells.div_ceil(32) {
        return Err(format!("volume {index}'s mask doesn't fit its size"));
    }
    let neighbours = words(stu, at + NEIGHBOURS).ok_or_else(cut)?;
    if neighbours.iter().any(|&other| other as usize >= count) {
        return Err(format!("volume {index} links a volume past the end"));
    }
    Ok(json!({
        "floor": floor,
        "origin": origin,
        "size": size,
        "neighbours": neighbours,
        "walkable": walkable,
    }))
}

/// A u32 array field's elements (empty when it has none).
fn words(stu: &StuV1<'_>, at: usize) -> Option<Vec<u32>> {
    let Some((count, first)) = stu.array(at) else {
        return Some(Vec::new());
    };
    (0..count).map(|k| stu.u32(first + 4 * k)).collect()
}
