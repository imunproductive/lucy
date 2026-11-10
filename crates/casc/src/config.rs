//! The install's text files: `.build.info` and the configs it names.

use std::{collections::HashMap, fs, path::Path};

use crate::{Error, Result, error::format};

/// The row of `.build.info` the launcher last activated. The file is a table:
/// a header of `Name!TYPE:size` columns, then one row per installed build,
/// all separated by `|`.
pub struct BuildInfo {
    columns: HashMap<String, String>,
}

impl BuildInfo {
    pub fn read(root: &Path) -> Result<BuildInfo> {
        let path = root.join(".build.info");
        let text = fs::read_to_string(&path).map_err(Error::io(&path))?;
        BuildInfo::parse(&text)
            .ok_or_else(|| format(format!("{} has no active build", path.display())))
    }

    pub fn parse(text: &str) -> Option<BuildInfo> {
        let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
        let names: Vec<String> = lines
            .next()?
            .split('|')
            .map(|column| column.split('!').next().unwrap_or(column).replace(' ', ""))
            .collect();
        lines
            .map(|line| {
                let columns: HashMap<String, String> = names
                    .iter()
                    .cloned()
                    .zip(line.split('|').map(str::to_owned))
                    .collect();
                BuildInfo { columns }
            })
            .find(|row| row.get("Active") == Some("1"))
    }

    /// A column by its name without spaces, such as `BuildKey` for `Build Key`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.columns
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

/// A `key = value value ...` config, stored under `config/ab/cd/<hash>`.
pub(crate) struct Config {
    values: HashMap<String, Vec<String>>,
}

impl Config {
    pub fn read(storage: &Path, hash: &str) -> Result<Config> {
        if hash.len() < 4 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format(format!("invalid config key {hash:?}")));
        }
        let path = storage
            .join("config")
            .join(&hash[..2])
            .join(&hash[2..4])
            .join(hash);
        let text = fs::read_to_string(&path).map_err(Error::io(&path))?;
        Ok(Config::parse(&text))
    }

    pub fn parse(text: &str) -> Config {
        let values = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| {
                (
                    key.trim().to_owned(),
                    value.split_whitespace().map(str::to_owned).collect(),
                )
            })
            .collect();
        Config { values }
    }

    pub fn values(&self, key: &str) -> Option<&[String]> {
        self.values.get(key).map(Vec::as_slice)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.values
            .iter()
            .map(|(key, values)| (key.as_str(), values.as_slice()))
    }
}

pub(crate) fn hex_key<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != 2 * N {
        return None;
    }
    let mut key = [0; N];
    for (byte, pair) in key.iter_mut().zip(text.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(key)
}

/// The keys for encrypted blocks, from the keyring config `.build.info` names.
/// Each line is `key-<name> = <key>`: the name is the eight bytes an encrypted
/// block carries, written in order, and the key is 16 bytes.
#[derive(Default)]
pub(crate) struct Keyring {
    keys: HashMap<u64, [u8; 16]>,
}

impl Keyring {
    pub fn from_config(config: &Config) -> Keyring {
        let keys = config
            .entries()
            .filter_map(|(name, values)| {
                let name = hex_key::<8>(name.strip_prefix("key-")?)?;
                Some((u64::from_le_bytes(name), hex_key::<16>(values.first()?)?))
            })
            .collect();
        Keyring { keys }
    }

    /// The install's key of that name, else the static one TACTLib ships.
    pub fn get(&self, name: u64) -> Option<&[u8; 16]> {
        self.keys.get(&name).or_else(|| {
            STATIC_KEYS
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, key)| key)
        })
    }
}

/// Keys this build's content needs that the install's keyring lacks, from
/// TACTLib's static `Overwatch.keyring` (MIT; see `CREDITS.md`): the entry
/// tagged `[1.74]`, which encrypts eight bundles of build 104319 (among them
/// the Genji skin unlock `000000005058.0A5`).
const STATIC_KEYS: &[(u64, [u8; 16])] = &[(
    0x28B8_EBB4_62B8_3EEA,
    [
        0x23, 0x8B, 0xA6, 0x99, 0x3D, 0x60, 0xA9, 0x7A, 0x46, 0x4C, 0xEB, 0xB4, 0xAA, 0xC4, 0x38,
        0xB5,
    ],
)];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installs_keys_win_over_the_static_ones() {
        let name = 0x28B8_EBB4_62B8_3EEA;
        assert_eq!(Keyring::default().get(name), Some(&STATIC_KEYS[0].1));
        let install = Keyring {
            keys: HashMap::from([(name, [7; 16])]),
        };
        assert_eq!(install.get(name), Some(&[7; 16]));
        assert_eq!(Keyring::default().get(1), None);
    }
}
