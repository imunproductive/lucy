//! Bundles (asset type `09C`) pack many small assets into one stored file.
//! Since 1.48 a bundle starts with `{i32 count, u8 offset size}` and then
//! `count` entries of `{u64 GUID, offset}`, the offset 1, 2 or 4 bytes. An
//! asset's size comes from the manifest, not the bundle.

use crate::{Result, error::format};

pub(crate) fn entries(data: &[u8]) -> Result<Vec<(u64, u32)>> {
    let truncated = || format("bundle header is truncated");
    let count = data.get(..4).ok_or_else(truncated)?;
    let count = i32::from_le_bytes(count.try_into().expect("four bytes"));
    let width = usize::from(*data.get(4).ok_or_else(truncated)?);
    if !matches!(width, 1 | 2 | 4) || count < 0 {
        return Err(format(format!(
            "unknown bundle layout: {count} entries of offset size {width}"
        )));
    }
    let table = data
        .get(5..5 + count as usize * (8 + width))
        .ok_or_else(truncated)?;
    Ok(table
        .chunks_exact(8 + width)
        .map(|entry| {
            let guid = u64::from_le_bytes(entry[..8].try_into().expect("eight bytes"));
            let mut offset = [0; 4];
            offset[..width].copy_from_slice(&entry[8..]);
            (guid, u32::from_le_bytes(offset))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_offset_width() {
        for width in [1u8, 2, 4] {
            let mut data = 2i32.to_le_bytes().to_vec();
            data.push(width);
            for (guid, offset) in [(7u64, 30u32), (9, 200)] {
                data.extend(guid.to_le_bytes());
                data.extend(&offset.to_le_bytes()[..usize::from(width)]);
            }
            assert_eq!(entries(&data).unwrap(), [(7, 30), (9, 200)]);
        }
        assert!(entries(&[1, 0, 0, 0, 3]).is_err());
    }
}
