//! The encoding table maps content keys (the MD5 of a file) to the encoding
//! key the indices know it by. Header: `EN`, version 1, key sizes 16 and 16,
//! big-endian page sizes in KiB and page counts, one byte, the size of the
//! encoding-spec strings; then those strings, a `{first key, MD5}` header per
//! content-key page, and the pages. A page entry is `{u8 key count, u40 size,
//! content key, encoding keys...}`; a count of 0 ends the page.

use crate::{Result, error::format};

#[derive(Default)]
pub(crate) struct Encoding {
    entries: Vec<([u8; 16], [u8; 16])>,
}

impl Encoding {
    pub fn parse(data: &[u8]) -> Result<Encoding> {
        let truncated = || format("encoding table is truncated");
        let header = data.get(..22).ok_or_else(truncated)?;
        if &header[..2] != b"EN" || header[2] != 1 || header[3] != 16 || header[4] != 16 {
            return Err(format("unknown encoding table header"));
        }
        let page_size = usize::from(u16::from_be_bytes([header[5], header[6]])) * 1024;
        let pages = u32::from_be_bytes(header[9..13].try_into().expect("four bytes")) as usize;
        let strings = u32::from_be_bytes(header[18..22].try_into().expect("four bytes")) as usize;
        let mut position = 22 + strings + 32 * pages;
        let mut entries = Vec::new();
        for _ in 0..pages {
            let page = data
                .get(position..position + page_size)
                .ok_or_else(truncated)?;
            let mut at = 0;
            while at + 22 <= page.len() {
                let count = usize::from(page[at]);
                if count == 0 {
                    break;
                }
                let ckey = page[at + 6..at + 22].try_into().expect("16 bytes");
                let ekey = page
                    .get(at + 22..at + 38)
                    .ok_or_else(truncated)?
                    .try_into()
                    .expect("16 bytes");
                entries.push((ckey, ekey));
                at += 22 + 16 * count;
            }
            position += page_size;
        }
        entries.sort_unstable_by_key(|(ckey, _)| *ckey);
        Ok(Encoding { entries })
    }

    pub fn find(&self, ckey: &[u8; 16]) -> Option<[u8; 16]> {
        self.entries
            .binary_search_by_key(ckey, |(ckey, _)| *ckey)
            .ok()
            .map(|position| self.entries[position].1)
    }
}
