//! BLTE, the encoded form of every stored file: `BLTE`, a big-endian header
//! size, then either one block (header size 0) or a frame table of
//! `{u32 encoded size, u32 decoded size, MD5}` entries after `0x0F` and a
//! 24-bit block count. Each block starts with its mode: `N` stored, `Z` zlib,
//! or `E` encrypted (Salsa20 with a keyring key) around another block.

use std::io::Read;

use flate2::read::ZlibDecoder;

use crate::{Error, Result, config::Keyring, error::format, salsa20::Salsa20};

pub(crate) fn decode(data: &[u8], keys: &Keyring) -> Result<Vec<u8>> {
    if data.get(..4) != Some(b"BLTE") {
        return Err(format("not a BLTE file"));
    }
    let header = be_u32(data, 4)? as usize;
    if header == 0 {
        let mut out = Vec::new();
        decode_block(&data[8..], 0, keys, &mut out)?;
        return Ok(out);
    }
    if data.get(8) != Some(&0x0F) {
        return Err(format("unknown BLTE frame table"));
    }
    let count = (be_u32(data, 8)? & 0x00FF_FFFF) as usize;
    if count == 0 || header < 12 + 24 * count {
        return Err(format("BLTE frame table is truncated"));
    }
    let blocks: Vec<(usize, usize)> = (0..count)
        .map(|block| {
            let entry = 12 + 24 * block;
            Ok((
                be_u32(data, entry)? as usize,
                be_u32(data, entry + 4)? as usize,
            ))
        })
        .collect::<Result<_>>()?;
    let mut out = Vec::with_capacity(blocks.iter().map(|(_, decoded)| decoded).sum());
    let mut position = header;
    for (number, (encoded, decoded)) in blocks.into_iter().enumerate() {
        let block = data
            .get(position..position + encoded)
            .ok_or_else(|| format("BLTE block runs past the file"))?;
        let start = out.len();
        decode_block(block, number, keys, &mut out)?;
        if out.len() - start != decoded {
            return Err(format(format!(
                "BLTE block {number} decoded to {} bytes, not {decoded}",
                out.len() - start
            )));
        }
        position += encoded;
    }
    Ok(out)
}

fn decode_block(block: &[u8], number: usize, keys: &Keyring, out: &mut Vec<u8>) -> Result<()> {
    let (&mode, body) = block
        .split_first()
        .ok_or_else(|| format("empty BLTE block"))?;
    match mode {
        b'N' => out.extend_from_slice(body),
        b'Z' => {
            ZlibDecoder::new(body)
                .read_to_end(out)
                .map_err(|error| format(format!("BLTE zlib block: {error}")))?;
        }
        b'E' => {
            let plain = decrypt(body, number, keys)?;
            decode_block(&plain, number, keys, out)?;
        }
        other => return Err(format(format!("BLTE block mode {:?}", other as char))),
    }
    Ok(())
}

/// `{u8 8, key name, u8 4, IV, b'S'}` then the Salsa20 ciphertext. The IV's low
/// 32 bits are xored with the block number.
fn decrypt(body: &[u8], number: usize, keys: &Keyring) -> Result<Vec<u8>> {
    let malformed = || format("malformed encrypted BLTE block");
    if body.first() != Some(&8) {
        return Err(malformed());
    }
    let name = u64::from_le_bytes(
        body.get(1..9)
            .ok_or_else(malformed)?
            .try_into()
            .map_err(|_| malformed())?,
    );
    if body.get(9) != Some(&4) {
        return Err(malformed());
    }
    let mut iv = [0; 8];
    iv[..4].copy_from_slice(body.get(10..14).ok_or_else(malformed)?);
    let low = u32::from_le_bytes(iv[..4].try_into().expect("four bytes")) ^ number as u32;
    iv[..4].copy_from_slice(&low.to_le_bytes());
    match body.get(14) {
        Some(b'S') => {}
        Some(other) => return Err(format(format!("BLTE encryption {:?}", *other as char))),
        None => return Err(malformed()),
    }
    let key = keys.get(name).ok_or(Error::MissingKey(name))?;
    let mut plain = body[15..].to_vec();
    Salsa20::new(key, &iv).apply(&mut plain);
    Ok(plain)
}

fn be_u32(data: &[u8], at: usize) -> Result<u32> {
    data.get(at..at + 4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().expect("four bytes")))
        .ok_or_else(|| format("BLTE header is truncated"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    fn framed(blocks: &[Vec<u8>], decoded: &[usize]) -> Vec<u8> {
        let header = 12 + 24 * blocks.len();
        let mut out = b"BLTE".to_vec();
        out.extend((header as u32).to_be_bytes());
        out.extend((0x0F00_0000 | blocks.len() as u32).to_be_bytes());
        for (block, size) in blocks.iter().zip(decoded) {
            out.extend((block.len() as u32).to_be_bytes());
            out.extend((*size as u32).to_be_bytes());
            out.extend([0; 16]);
        }
        blocks.iter().for_each(|block| out.extend(block));
        out
    }

    #[test]
    fn decodes_stored_zlib_and_single_block_files() {
        let mut zlib = ZlibEncoder::new(vec![b'Z'], Compression::default());
        zlib.write_all(b" world").unwrap();
        let file = framed(&[b"Nhello".to_vec(), zlib.finish().unwrap()], &[5, 6]);
        assert_eq!(decode(&file, &Keyring::default()).unwrap(), b"hello world");
        assert_eq!(
            decode(b"BLTE\0\0\0\0Nplain", &Keyring::default()).unwrap(),
            b"plain"
        );
    }

    #[test]
    fn rejects_blocks_whose_size_disagrees_with_the_table() {
        let file = framed(&[b"Nhello".to_vec()], &[4]);
        assert!(decode(&file, &Keyring::default()).is_err());
    }

    #[test]
    fn reports_the_missing_key_of_an_encrypted_block() {
        let mut block = vec![b'E', 8];
        block.extend(0x1122_3344_5566_7788_u64.to_le_bytes());
        block.extend([4, 0, 0, 0, 0, b'S', 0]);
        let file = framed(&[block], &[0]);
        assert!(matches!(
            decode(&file, &Keyring::default()),
            Err(Error::MissingKey(0x1122_3344_5566_7788))
        ));
    }
}
