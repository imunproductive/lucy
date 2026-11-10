//! The local indices `data/XX*.idx`, which say where each encoded file sits.
//!
//! There are 16 buckets; a key's bucket is its first 9 bytes xored together,
//! folded to 4 bits. Each bucket can have several versions of its index file
//! and the newest (the highest hex number after the bucket) wins, as in
//! TACTLib. An index keeps only the first 9 bytes of each encoding key, in
//! 18-byte entries: the key, 5 bytes big endian `archive << 30 | offset`, and
//! the encoded size as a little-endian `u32`.

use std::{fs, path::Path};

use crate::{Error, Result, error::format};

pub(crate) const KEY: usize = 9;
const ENTRY: usize = 18;
const BUCKETS: u8 = 16;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Location {
    pub archive: u16,
    pub offset: u32,
    pub size: u32,
}

pub(crate) struct Index {
    entries: Vec<([u8; KEY], Location)>,
}

impl Index {
    /// Every bucket's newest index file under `data`; entries pointing past
    /// the end of their archive, or into a missing one, are left out.
    pub fn load(data: &Path, archive_size: impl Fn(u16) -> Option<u64>) -> Result<Index> {
        let mut newest: [Option<(u32, std::path::PathBuf)>; BUCKETS as usize] = Default::default();
        for entry in fs::read_dir(data).map_err(Error::io(data))? {
            let path = entry.map_err(Error::io(data))?.path();
            let Some(stem) = path
                .extension()
                .filter(|extension| extension.eq_ignore_ascii_case("idx"))
                .and(path.file_stem())
                .and_then(|stem| stem.to_str())
            else {
                continue;
            };
            let (Some(bucket), Some(version)) = (
                stem.get(..2)
                    .and_then(|text| u8::from_str_radix(text, 16).ok()),
                stem.get(2..)
                    .and_then(|text| u32::from_str_radix(text, 16).ok()),
            ) else {
                continue;
            };
            if let Some(slot) = newest.get_mut(usize::from(bucket))
                && slot.as_ref().is_none_or(|(known, _)| version > *known)
            {
                *slot = Some((version, path));
            }
        }
        let mut entries = Vec::new();
        for (bucket, file) in newest.iter().enumerate() {
            if let Some((_, path)) = file {
                let raw = fs::read(path).map_err(Error::io(path))?;
                read_bucket(&raw, bucket as u8, &archive_size, &mut entries)
                    .map_err(|error| format(format!("{}: {error}", path.display())))?;
            }
        }
        // A stable sort keeps the first entry for a key, as TACTLib does.
        entries.sort_by_key(|(key, _)| *key);
        entries.dedup_by_key(|(key, _)| *key);
        Ok(Index { entries })
    }

    pub fn find(&self, ekey: &[u8; 16]) -> Option<Location> {
        let key: [u8; KEY] = ekey[..KEY].try_into().ok()?;
        self.entries
            .binary_search_by_key(&key, |(key, _)| *key)
            .ok()
            .map(|position| self.entries[position].1)
    }
}

fn read_bucket(
    raw: &[u8],
    bucket: u8,
    archive_size: &impl Fn(u16) -> Option<u64>,
    entries: &mut Vec<([u8; KEY], Location)>,
) -> std::result::Result<(), String> {
    // {u32 block size, u32 hash} then version, bucket, extra bytes, size
    // bytes, offset bytes, key bytes; the entry block starts at 32 with its
    // own {size, hash}.
    let layout = raw.get(8..14).ok_or("index is truncated")?;
    let version = u16::from_le_bytes([layout[0], layout[1]]);
    if version != 7 || layout[2] != bucket || layout[3] != 0 || layout[4] != 4 || layout[5] != 5 {
        return Err(format!("unknown index layout {layout:02X?}"));
    }
    if raw.get(14) != Some(&(KEY as u8)) {
        return Err("index keys are not 9 bytes".into());
    }
    let size = raw.get(32..36).ok_or("index is truncated")?;
    let size = u32::from_le_bytes(size.try_into().map_err(|_| "index is truncated")?) as usize;
    let block = raw
        .get(40..40 + size - size % ENTRY)
        .ok_or("index entries are truncated")?;
    for entry in block.chunks_exact(ENTRY) {
        let packed = u32::from_be_bytes([entry[10], entry[11], entry[12], entry[13]]);
        let archive = u16::from(entry[9]) << 2 | (packed >> 30) as u16;
        let offset = packed & 0x3FFF_FFFF;
        if archive_size(archive).is_none_or(|size| u64::from(offset) >= size) {
            continue;
        }
        let size = u32::from_le_bytes([entry[14], entry[15], entry[16], entry[17]]);
        let key = entry[..KEY]
            .try_into()
            .expect("an entry starts with a 9-byte key");
        entries.push((
            key,
            Location {
                archive,
                offset,
                size,
            },
        ));
    }
    Ok(())
}
