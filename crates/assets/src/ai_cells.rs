//! `data/aicells/<map>.bin.gz`: each map's AI cell data, the voxel grids the
//! client's path query searches. The server's reader is
//! `lucy_physics::ai_cells`.
//!
//! Where it comes from: document 0 of the map's chunk 0x12 (see
//! [`crate::navigation`] for the chunk header), root `STU_3BC32A6E` at
//! instance offset `+0x20`: `+0x08 m_cellData` (24-byte entries `{u64 cell
//! key, u64 hash, u32 index into m_cellBytes, u32}`), `+0x18 m_cellBytes`
//! (`STUAICellBytes`, a byte array per entry: `{i32 offset, ...}` 16 bytes,
//! pointing at `{u64 length, u64 offset}`), `+0x28 m_collisionCrc`, `+0x30
//! m_D4E3DEAB`. Several keys can share one byte block (same hash).
//!
//! The client's loader (`0x7FF78A61CB80`, from `0x7FF78AA64190`, which builds
//! the grid with cells of 16 × 22.4 × 16 m) keeps each block as is, keyed by
//! the cell key; the block's own layout is read by
//! `lucy_physics::ai_cells`. The derivation checks that each block starts
//! with its 17 section offsets in order and inside the block, and stores the
//! blocks unchanged:
//!
//! - `b"ORPHAIC1"`;
//! - u32 block count, then per block a u32 length and the bytes;
//! - u32 cell count, then per cell `{u32 key, u32 block index}`, sorted by
//!   key.
//!
//! All little-endian, gzipped. Maps without document 0 (54 menus, lobbies and
//! unused maps) get no file.

use std::{collections::BTreeMap, io::Write, path::Path};

use flate2::{Compression, write::GzEncoder};

use crate::{
    Install, Output, Result, asset_error, guid_name,
    map_chunk::{MAP_CHUNK, StuV1, chunk_kind},
};

/// The chunk kind of the navigation data.
const NAVIGATION: u16 = 0x12;
/// The root class of document 0.
const CELL_DATA: u32 = 0x3BC3_2A6E;
/// Root field offsets.
const CELLS: usize = 0x08;
const BYTES: usize = 0x18;
/// A `m_cellData` entry's size and its block index.
const CELL: usize = 24;
const CELL_INDEX: usize = 16;
/// A block's header: 17 section offsets, two masks, two counts.
const SECTIONS: usize = 17;
const HEADER: usize = 160;
const MAGIC: &[u8; 8] = b"ORPHAIC1";

/// The derivation: one file per map that has AI cell data.
pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut outputs = Vec::new();
    for chunk in install
        .guids(MAP_CHUNK)
        .into_iter()
        .filter(|guid| chunk_kind(*guid) == NAVIGATION)
    {
        let data = install.raw(chunk)?;
        let cells = cells(&data)
            .map_err(|reason| asset_error(format!("{}: {reason}", guid_name(chunk))))?;
        let Some((blocks, keys)) = cells else {
            continue;
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&count(blocks.len()));
        for block in &blocks {
            bytes.extend_from_slice(&count(block.len()));
            bytes.extend_from_slice(block);
        }
        bytes.extend_from_slice(&count(keys.len()));
        for (key, block) in keys {
            bytes.extend_from_slice(&key.to_le_bytes());
            bytes.extend_from_slice(&block.to_le_bytes());
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(9));
        encoder.write_all(&bytes)?;
        let index = chunk & 0xFFFF_FFFF;
        outputs.push(Output {
            path: format!("data/aicells/{index:04X}.bin.gz"),
            bytes: encoder.finish()?,
        });
    }
    Ok(outputs)
}

fn count(value: usize) -> [u8; 4] {
    u32::try_from(value)
        .expect("counts fit 32 bits")
        .to_le_bytes()
}

/// The blocks (in `m_cellBytes` order, only those a key names) and the key
/// → block index map of a navigation chunk; `None` without document 0.
#[allow(clippy::type_complexity)]
fn cells(data: &[u8]) -> Result<Option<(Vec<&[u8]>, BTreeMap<u32, u32>)>, String> {
    let offset = data
        .get(..8)
        .map(|bytes| u64::from_le_bytes(bytes.try_into().unwrap()))
        .ok_or("chunk header cut short")?;
    if offset == 0 {
        return Ok(None);
    }
    let stu = usize::try_from(offset)
        .ok()
        .and_then(|offset| StuV1::new(data, offset))
        .ok_or("document 0 is not STUv1")?;
    let root = stu.root().ok_or("document 0 has no root")?;
    if stu.class(root) != Some(CELL_DATA) {
        return Err(format!(
            "document 0 root is {:08X}",
            stu.class(root).unwrap_or(0)
        ));
    }
    let (cell_count, first_cell) = stu.array(root + CELLS).ok_or("no m_cellData")?;
    let (byte_count, first_bytes) = stu.array(root + BYTES).ok_or("no m_cellBytes")?;
    let start = offset as usize;
    let mut blocks = Vec::new();
    let mut by_index = BTreeMap::new();
    let mut keys = BTreeMap::new();
    for cell in 0..cell_count {
        let at = first_cell + CELL * cell;
        let key = stu.u64(at).ok_or("m_cellData cut short")?;
        let key = u32::try_from(key).map_err(|_| format!("cell key {key:X} past 32 bits"))?;
        let index = stu.u32(at + CELL_INDEX).ok_or("m_cellData cut short")? as usize;
        if index >= byte_count {
            return Err(format!("cell {key:08X} names block {index} past the end"));
        }
        let block = match by_index.get(&index) {
            Some(&block) => block,
            None => {
                let (length, offset) = stu
                    .array(first_bytes + 16 * index)
                    .ok_or_else(|| format!("block {index} has no bytes"))?;
                let bytes = data
                    .get(start + offset..start + offset + length)
                    .ok_or_else(|| format!("block {index} runs past the chunk"))?;
                check(bytes).map_err(|reason| format!("block {index}: {reason}"))?;
                blocks.push(bytes);
                let block = u32::try_from(blocks.len() - 1).expect("blocks fit 32 bits");
                by_index.insert(index, block);
                block
            }
        };
        if keys.insert(key, block).is_some() {
            return Err(format!("cell {key:08X} listed twice"));
        }
    }
    Ok(Some((blocks, keys)))
}

/// A block starts with its section offsets, in order, from the header's end
/// to the block's.
fn check(block: &[u8]) -> Result<(), String> {
    if block.len() < HEADER {
        return Err("shorter than its header".into());
    }
    let mut last = HEADER as u64;
    for section in 0..SECTIONS {
        let offset = u64::from_le_bytes(block[8 * section..8 * section + 8].try_into().unwrap());
        if offset < last || offset > block.len() as u64 {
            return Err(format!("section {section} at {offset} out of order"));
        }
        last = offset;
    }
    Ok(())
}
