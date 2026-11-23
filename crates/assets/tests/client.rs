//! Install-tier checks of the files derived from the client program and the
//! resource key assets: each is byte for byte the file the repository used to
//! ship in `crates/fixed/data` (its SHA-256 in `docs/dev/data-sources.md`).

use lucy_assets::{client, resource_keys};
use sha2::{Digest, Sha256};

fn sha256(path: &str) -> String {
    let bytes = std::fs::read(lucy_testkit::runtime_dir().join(path)).unwrap();
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn the_client_programs_files_are_the_ones_the_repository_shipped() {
    for (path, digest) in [
        (
            client::LOBBY_SCHEMAS,
            "bd0d752bc365df93ab35b3749add3e34c690d06b33ba6ea9293d4589cc2cff53",
        ),
        (
            client::GAME_SCHEMAS,
            "904f77bd3d5fc18a5348aa32d5274c2015454aff274c13c7864a24306dccc12d",
        ),
        (
            client::BNET_DESCRIPTORS,
            "c20cac09537c013ef359d3c47aa39b178c51c3efe2258308ca0b1e8ee8f0fb2d",
        ),
        (
            client::LOBBY_STATE_BLOB_KEY,
            "ab024361c5acd29e3a45548cc675002b762895753fe6dc7613335b1f0ff033b8",
        ),
    ] {
        assert_eq!(sha256(path), digest, "{path}");
    }
}

/// Every key the shipped `resource_keys.json` held (189, by asset index) has
/// the same bytes. 65 take the name the install gives them (two their asset,
/// 63 the keyring): the shipped names where it had one, and a name for 16
/// keys it left unnamed. Its other names came from TACTLib's keyring; those
/// keys take their asset's GUID as their name.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn resource_keys_have_the_shipped_bytes_and_install_names() {
    let keys = lucy_testkit::runtime_json(resource_keys::RESOURCE_KEYS);
    let keys = keys.as_object().unwrap();
    assert_eq!(keys.len(), 189);
    let bytes: String = keys
        .iter()
        .map(|(index, key)| format!("{index}={}\n", key["key"].as_str().unwrap()))
        .collect();
    assert_eq!(
        Sha256::digest(bytes.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        SHIPPED_KEY_BYTES
    );
    // Named in their asset: 16 and the key of the eight bundles.
    assert_eq!(keys["16"]["name"], "402CD9D8D6BFED98");
    assert_eq!(keys["1A3"]["name"], "28B8EBB462B83EEA");
    // Named by the keyring: Sombra's key, and key 2, unnamed in the shipped
    // file.
    assert_eq!(keys["C6"]["name"], "11131FFDA0D18D30");
    assert_eq!(keys["2"]["name"], "FB680CB6A8BF81F3");
    // Neither: Lunar New Year 2022's key takes its GUID.
    assert_eq!(keys["188"]["name"], "0F10000000000188");
    let named = keys
        .values()
        .filter(|key| !key["name"].as_str().unwrap().starts_with("0F1"))
        .count();
    assert_eq!(named, 65);
}

/// SHA-256 of `<index>=<key hex>\n` over the shipped file's keys, in the
/// order of their asset GUIDs.
const SHIPPED_KEY_BYTES: &str = "741c56d9066ee1e3f8f916e97c8b16abb658eb3889bcca0411e448a57500e775";
