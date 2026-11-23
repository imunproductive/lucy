//! `data/resource_keys.json`: the install's resource keys (type `090`,
//! `STUResourceKey`), the content keys the server hands the client for
//! events (20504, and each event record's key). Each asset holds the key's
//! 16 bytes (`m_key`) and, for two of 189, its name (`m_keyID`, the eight
//! bytes an encrypted block carries, as hex).
//!
//! A key without a name in its asset takes the name the install's keyring
//! gives the same bytes (63 do), else its own GUID. No stored file of the
//! install is encrypted with those keys (a scan of every stored file's first
//! block found only `28B8EBB462B83EEA`, the name asset `1A3` gives its key),
//! so the name only ties the event record to the key the client was given.
//! The retail server's names for them are not in the install; the shipped
//! file had taken them from TACTLib's keyring.
//!
//! Asset `1A3` sits in one of the eight bundles its own key encrypts, so it
//! reads only with TACTLib's static copy of that key (`lucy_casc`).

use std::path::Path;

use lucy_casc::Game;
use lucy_stu::Value;
use serde_json::{Map, json};

use crate::{Install, Output, Result, asset_error, guid_name};

pub const RESOURCE_KEYS: &str = "data/resource_keys.json";

const RESOURCE_KEY: u16 = 0x090;

/// One resource key asset.
struct ResourceKey {
    guid: u64,
    /// The name its asset gives it (0: none).
    name: u64,
    key: [u8; 16],
}

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let game = install.game();
    let mut keys = Map::new();
    for key in read(game)? {
        let name = Some(key.name)
            .filter(|&name| name != 0)
            .or_else(|| game.storage().key_name(&key.key))
            .unwrap_or(key.guid);
        keys.insert(
            format!("{:X}", key.guid & 0xFFFF_FFFF_FFFF),
            json!({"name": format!("{name:016X}"), "key": hex(&key.key)}),
        );
    }
    Ok(vec![Output::json(RESOURCE_KEYS, &keys.into())])
}

/// Every resource key asset, by GUID.
fn read(game: &Game) -> Result<Vec<ResourceKey>> {
    game.read_all(&game.guids(RESOURCE_KEY))
        .into_iter()
        .map(|(guid, bytes)| {
            let malformed = |reason: &str| asset_error(format!("{}: {reason}", guid_name(guid)));
            let bytes = bytes.map_err(|error| malformed(&error.to_string()))?;
            let asset = lucy_stu::read(&bytes).map_err(|error| malformed(&error.to_string()))?;
            let root = asset.root().ok_or_else(|| malformed("no root"))?;
            let name = match root.get("m_keyID") {
                Some(Value::String(text)) => name(text).ok_or_else(|| malformed("bad m_keyID"))?,
                _ => 0,
            };
            let key: Vec<u8> = match root.get("m_key") {
                Some(Value::Bytes(bytes)) => bytes.clone(),
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|value| match value {
                        Value::UInt(byte) => u8::try_from(*byte).ok(),
                        _ => None,
                    })
                    .collect::<Option<_>>()
                    .ok_or_else(|| malformed("bad m_key"))?,
                _ => return Err(malformed("no m_key")),
            };
            let key = key
                .try_into()
                .map_err(|_| malformed("m_key is not 16 bytes"))?;
            Ok(ResourceKey { guid, name, key })
        })
        .collect()
}

/// `m_keyID`: the name's eight bytes in block order, as hex; the keyring
/// reads them as a little-endian `u64`.
fn name(text: &str) -> Option<u64> {
    if text.len() != 16 {
        return None;
    }
    let mut bytes = [0; 8];
    for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(u64::from_le_bytes(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_id_reads_as_the_name_an_encrypted_block_carries() {
        // Asset 1A3's name; the block holds ea 3e b8 62 b4 eb b8 28.
        assert_eq!(name("ea3eb862b4ebb828"), Some(0x28B8_EBB4_62B8_3EEA));
        assert_eq!(name("0000000000000000"), Some(0));
        assert_eq!(name("ea3e"), None);
    }
}
