//! Overwatch's content manifests. The root file lists the manifests of the
//! build: `.cmf` files map asset GUIDs to content keys, per region and locale.
//! Since 1.48 a CMF header is twelve 32-bit fields ending in the asset count,
//! the entry count and a magic; the rest of the file is encrypted with
//! AES-256-CBC when the magic is the reversed `cmf` tag. The key and IV come
//! from a per-build procedure; this is TACTLib's `ProCMF_104319`. The entries
//! are 20 bytes `{u32 index, u64, u64}`, the assets 29 bytes
//! `{u64 GUID, u32 size, u8, content key}`.

use aes::{
    Aes256,
    cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray},
};
use sha1::{Digest, Sha1};

use crate::{Result, error::format};

pub const BUILD: u32 = 104_319;
const ENCRYPTED: u32 = 0x63_6D66;
const HEADER: usize = 48;
const ENTRY: usize = 20;
const ASSET: usize = 29;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asset {
    pub guid: u64,
    pub size: u32,
    pub ckey: [u8; 16],
}

/// One manifest named by the root file.
pub(crate) struct RootEntry {
    pub ckey: [u8; 16],
    pub name: String,
}

/// The root file: `#FILEID|MD5|CHUNK_ID|PRIORITY|MPRIORITY|FILENAME|INSTALLPATH`.
pub(crate) fn root_entries(text: &str) -> Result<Vec<RootEntry>> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    if lines.next() != Some("#FILEID|MD5|CHUNK_ID|PRIORITY|MPRIORITY|FILENAME|INSTALLPATH") {
        return Err(format("unknown root file columns"));
    }
    lines
        .map(|line| {
            let columns: Vec<&str> = line.split('|').collect();
            let ckey = columns.get(1).and_then(|text| crate::config::hex_key(text));
            match (ckey, columns.get(5)) {
                (Some(ckey), Some(name)) => Ok(RootEntry {
                    ckey,
                    name: (*name).to_owned(),
                }),
                _ => Err(format(format!("invalid root file line {line:?}"))),
            }
        })
        .collect()
}

/// The locale tag of a manifest name, such as `enUS` in `..._LenUS_...`.
pub(crate) fn locale(stem: &str) -> Option<&str> {
    stem.split('_')
        .rev()
        .find(|tag| tag.len() >= 4 && tag.starts_with('L'))
        .map(|tag| &tag[1..])
}

/// The assets of a CMF, sorted by GUID. `name` is the manifest's file name,
/// which seeds the IV.
pub(crate) fn content_manifest(data: &[u8], name: &str) -> Result<Vec<Asset>> {
    let field = |index: usize| -> Result<u32> {
        data.get(4 * index..4 * index + 4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("four bytes")))
            .ok_or_else(|| format(format!("{name}: header is truncated")))
    };
    let build = field(0)?;
    let assets = field(8)? as usize;
    let entries = field(10)? as usize;
    let magic = field(11)?;
    let body_size = entries * ENTRY + assets * ASSET;
    let body = data
        .get(HEADER..)
        .filter(|body| body.len() >= body_size)
        .ok_or_else(|| format(format!("{name}: {assets} assets do not fit the file")))?;
    let body = if magic >> 8 == ENCRYPTED {
        if build != BUILD {
            return Err(format(format!("{name} is from build {build}, not {BUILD}")));
        }
        let mut plain = body[..body_size.next_multiple_of(16).min(body.len() / 16 * 16)].to_vec();
        if plain.len() < body_size {
            return Err(format(format!("{name}: encrypted body is truncated")));
        }
        decrypt(&mut plain, &key(build), &iv(name));
        plain
    } else {
        body.to_vec()
    };
    // The first entry's index is 1 when the key and IV were right.
    if entries > 0 && body[..4] != 1u32.to_le_bytes() {
        return Err(format(format!("{name}: decryption failed")));
    }
    let mut list: Vec<Asset> = body[entries * ENTRY..body_size]
        .chunks_exact(ASSET)
        .map(|record| Asset {
            guid: u64::from_le_bytes(record[..8].try_into().expect("eight bytes")),
            size: u32::from_le_bytes(record[8..12].try_into().expect("four bytes")),
            ckey: record[13..29].try_into().expect("16 bytes"),
        })
        .collect();
    list.sort_unstable_by_key(|asset| asset.guid);
    Ok(list)
}

fn decrypt(data: &mut [u8], key: &[u8; 32], iv: &[u8; 16]) {
    let cipher = Aes256::new(GenericArray::from_slice(key));
    let mut previous = *iv;
    for block in data.chunks_exact_mut(16) {
        let ciphertext: [u8; 16] = (*block).try_into().expect("16 bytes");
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        block
            .iter_mut()
            .zip(previous)
            .for_each(|(byte, chain)| *byte ^= chain);
        previous = ciphertext;
    }
}

/// C#'s `SignedMod`: both sides truncated to `int`, the result made positive.
fn signed_mod(value: i64, modulus: i64) -> usize {
    (value as i32).rem_euclid(modulus as i32) as usize
}

fn key(build: u32) -> [u8; 32] {
    let mut key = [0; 32];
    let mut index = u32::from(KEY_TABLE[key.len() + 256]);
    for byte in &mut key {
        *byte = KEY_TABLE[signed_mod(index.into(), 512)];
        match signed_mod(index.into(), 3) {
            0 => index = index.wrapping_add(103),
            1 => index = signed_mod(index.wrapping_mul(4).into(), build.into()) as u32,
            _ => index = index.wrapping_sub(1),
        }
    }
    key
}

fn iv(name: &str) -> [u8; 16] {
    let digest = Sha1::digest(name.as_bytes());
    let mut iv = [0; 16];
    let start = 2 * u32::from(digest[5]);
    let mut index = start;
    for (position, byte) in iv.iter_mut().enumerate() {
        *byte = KEY_TABLE[signed_mod(index.into(), 512)];
        index += if digest[6] & 1 != 0 { 37 } else { start % 61 };
        *byte ^= digest[signed_mod(i64::from(index) - position as i64, 20)];
    }
    iv
}

/// `ProCMF_104319`'s table (TACTLib, MIT).
#[rustfmt::skip]
const KEY_TABLE: [u8; 512] = [
    0xF5, 0xA1, 0x1C, 0xC6, 0xB9, 0xCF, 0x1F, 0xA2, 0x87, 0x51, 0x0A, 0x92, 0x57, 0x20, 0xA9, 0xBA,
    0xE5, 0xAE, 0xB7, 0xC0, 0x5B, 0xDB, 0x8B, 0x7A, 0x5C, 0x0D, 0xE3, 0x14, 0x05, 0x42, 0xED, 0xBD,
    0xA2, 0xA4, 0x8B, 0x6C, 0xBB, 0x30, 0xC2, 0x56, 0xBD, 0x35, 0xCC, 0xB3, 0xEC, 0x9A, 0x58, 0x95,
    0x05, 0xF6, 0xEB, 0x52, 0x14, 0xC2, 0x71, 0x9E, 0x5C, 0x7E, 0x29, 0x3E, 0xD0, 0x72, 0xE9, 0x49,
    0x50, 0xF3, 0xBF, 0xF6, 0x1B, 0xD2, 0x7E, 0xE7, 0x5F, 0x77, 0x4F, 0x1E, 0xB3, 0xC8, 0x4C, 0xAE,
    0xCE, 0xE3, 0xCE, 0x66, 0xE2, 0xC9, 0x13, 0x2F, 0x6C, 0x19, 0x16, 0xFA, 0x8D, 0xD8, 0x48, 0xD2,
    0xAD, 0xC0, 0x46, 0x8F, 0xF4, 0xC9, 0xBB, 0x55, 0x50, 0xCA, 0x41, 0xBF, 0x85, 0xFE, 0xC2, 0x58,
    0xC7, 0x12, 0xC7, 0xDD, 0x1E, 0xC9, 0xB3, 0xB8, 0xDD, 0x89, 0x9B, 0x8B, 0x8E, 0x6F, 0x64, 0xF1,
    0xE4, 0x09, 0x93, 0x93, 0x66, 0xEE, 0x61, 0x66, 0xCE, 0x66, 0x4A, 0x63, 0x2E, 0xC9, 0x49, 0x16,
    0x48, 0xB7, 0x72, 0x16, 0x52, 0x57, 0x8D, 0xF0, 0xCF, 0x99, 0x79, 0x66, 0x6C, 0xCB, 0x82, 0xD2,
    0x19, 0x9D, 0xBD, 0x38, 0x9E, 0x2B, 0x6A, 0x7B, 0x41, 0xD9, 0x72, 0x64, 0x88, 0x78, 0x69, 0x75,
    0x4E, 0xCE, 0xBE, 0x35, 0x14, 0x45, 0x06, 0x33, 0xB3, 0x62, 0x25, 0x58, 0xAA, 0x00, 0x6E, 0xC6,
    0x8A, 0x59, 0xEE, 0x80, 0xBE, 0x29, 0x37, 0xBE, 0x36, 0x5C, 0xF9, 0x12, 0x0A, 0x9A, 0x94, 0x92,
    0xC7, 0x31, 0x6D, 0xA4, 0x66, 0x0A, 0x7F, 0x91, 0x94, 0xBA, 0x47, 0x49, 0x79, 0xAF, 0x1D, 0xE5,
    0x5B, 0xCD, 0xEF, 0xE9, 0x66, 0x95, 0xE2, 0x5A, 0xB9, 0xED, 0xB8, 0xEA, 0xDA, 0xAF, 0x2C, 0x6D,
    0xDF, 0xD0, 0x5A, 0x6A, 0x50, 0x4D, 0x37, 0x39, 0xA1, 0xE6, 0x38, 0x7B, 0x7B, 0x89, 0xBD, 0x42,
    0x32, 0xE9, 0xE3, 0x93, 0xBC, 0x0C, 0x0B, 0xA0, 0xAD, 0x19, 0x27, 0x01, 0xA3, 0xE9, 0x38, 0x40,
    0x68, 0xB5, 0x42, 0x0C, 0xFD, 0x27, 0xA7, 0x49, 0x08, 0x2A, 0x70, 0xE7, 0x72, 0x3B, 0x46, 0x34,
    0x19, 0x19, 0x94, 0x19, 0x4D, 0x69, 0x77, 0x22, 0x3E, 0xDF, 0x77, 0xE4, 0x0C, 0x1B, 0x55, 0x43,
    0x16, 0xD8, 0x14, 0x19, 0xFE, 0xA5, 0xEF, 0x68, 0xCB, 0x86, 0x89, 0x5B, 0x2E, 0x52, 0xDE, 0x36,
    0x13, 0xFC, 0x60, 0x4C, 0x11, 0x2F, 0xFB, 0xF1, 0xA7, 0xDA, 0x2B, 0xDB, 0x4C, 0x4B, 0x2C, 0x11,
    0x95, 0x10, 0xAB, 0x12, 0x27, 0x6A, 0x9F, 0x59, 0x00, 0x17, 0xEF, 0xC8, 0x1D, 0xEF, 0xCD, 0xD7,
    0x82, 0x42, 0xED, 0x96, 0x59, 0xCE, 0x38, 0xF2, 0x77, 0x95, 0x4D, 0x61, 0x5D, 0x7B, 0xAA, 0xCA,
    0xD8, 0x60, 0x0B, 0xA6, 0x7F, 0x48, 0x42, 0x60, 0xA1, 0xE5, 0x37, 0xEB, 0x1B, 0x2E, 0x14, 0x3B,
    0xB5, 0xB5, 0xE5, 0x79, 0x1D, 0xF3, 0x94, 0x1F, 0xCE, 0xC9, 0x91, 0xF5, 0x86, 0xE0, 0x55, 0x58,
    0x73, 0x1B, 0xEF, 0x1D, 0xEA, 0x73, 0x2F, 0x0A, 0x69, 0xA8, 0xEE, 0x4F, 0x06, 0x98, 0x7A, 0x71,
    0x22, 0x23, 0x10, 0x43, 0xE5, 0x14, 0xE2, 0xB5, 0x3C, 0x05, 0xDF, 0x0E, 0x13, 0xFC, 0x60, 0x74,
    0x7E, 0x33, 0xAB, 0x11, 0xC7, 0x0E, 0x67, 0x64, 0x9D, 0xD3, 0x01, 0xDC, 0xEB, 0x99, 0x0D, 0x5F,
    0xA3, 0xCB, 0xB5, 0x46, 0x98, 0xA8, 0x8D, 0xE8, 0xC0, 0x14, 0x91, 0xD2, 0x83, 0xBA, 0x75, 0x27,
    0x42, 0xCE, 0x5F, 0x41, 0xA1, 0x2E, 0xFB, 0x96, 0xF7, 0x16, 0xCD, 0x22, 0x7A, 0x5A, 0x38, 0x20,
    0x36, 0x1C, 0xE5, 0x24, 0xB3, 0x0E, 0x06, 0x41, 0xC6, 0xBD, 0x14, 0xF6, 0x34, 0xF1, 0xF7, 0xF8,
    0x2F, 0xD2, 0x83, 0xE3, 0xB1, 0x26, 0x0A, 0x24, 0xE9, 0x5D, 0xE0, 0x93, 0xAB, 0xFC, 0x69, 0xAE,
];

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncrypt;

    #[test]
    fn locale_is_the_last_l_tag() {
        assert_eq!(locale("Win_SPWin_RDEV_LenUS_EExt"), Some("enUS"));
        assert_eq!(locale("Win_SPWin_RDEV_LenUS_speech_EExt"), Some("enUS"));
        assert_eq!(locale("Win_SPWin_RDEV_EExt"), None);
    }

    fn encrypt(data: &mut [u8], key: &[u8; 32], iv: &[u8; 16]) {
        let cipher = Aes256::new(GenericArray::from_slice(key));
        let mut previous = *iv;
        for block in data.chunks_exact_mut(16) {
            block
                .iter_mut()
                .zip(previous)
                .for_each(|(byte, chain)| *byte ^= chain);
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
            previous = block.try_into().unwrap();
        }
    }

    #[test]
    fn reads_an_encrypted_manifest_and_rejects_a_wrong_name() {
        let name = "Win_SPWin_RDEV_EExt.cmf";
        let mut body = Vec::new();
        body.extend(1u32.to_le_bytes());
        body.extend([0; 16]);
        for (guid, size) in [
            (0x0D00_0000_0000_0002_u64, 9u32),
            (0x0400_0000_0000_0001, 4),
        ] {
            body.extend(guid.to_le_bytes());
            body.extend(size.to_le_bytes());
            body.push(0);
            body.extend([guid as u8; 16]);
        }
        body.resize(body.len().next_multiple_of(16), 0);
        encrypt(&mut body, &key(BUILD), &iv(name));
        let mut file: Vec<u8> = [BUILD, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, ENCRYPTED << 8 | 26]
            .iter()
            .flat_map(|field| field.to_le_bytes())
            .collect();
        file.extend(body);
        let assets = content_manifest(&file, name).unwrap();
        assert_eq!(
            assets
                .iter()
                .map(|asset| (asset.guid, asset.size))
                .collect::<Vec<_>>(),
            [(0x0400_0000_0000_0001, 4), (0x0D00_0000_0000_0002, 9)]
        );
        assert_eq!(assets[0].ckey, [1; 16]);
        assert!(content_manifest(&file, "Win_SPWin_RDEV_LenUS_EExt.cmf").is_err());
    }
}
