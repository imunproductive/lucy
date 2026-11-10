//! The install's assets for derivations: raw bytes, decoded structured data
//! and display strings, by GUID.

use std::path::Path;

use orpheus_casc::Game;
pub use orpheus_casc::{guid_name, guid_type};

use crate::{Result, asset_error};

/// The asset type of display strings.
const STRING: u16 = 0x07C;

pub struct Install {
    game: Game,
    locale: String,
}

impl Install {
    /// Open the install with the text manifests of `locale` (for example
    /// `enUS`), which decide the language of display strings.
    pub fn open(install: &Path, locale: &str) -> Result<Self> {
        let game = Game::open(install, locale)
            .map_err(|error| asset_error(format!("{}: {error}", install.display())))?;
        Ok(Self {
            game,
            locale: locale.to_owned(),
        })
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Every GUID of one asset type, sorted.
    pub fn guids(&self, kind: u16) -> Vec<u64> {
        self.game.guids(kind)
    }

    /// An asset's bytes, checked against its content key.
    pub fn raw(&self, guid: u64) -> Result<Vec<u8>> {
        self.game
            .read(guid)
            .map_err(|error| asset_error(format!("{}: {error}", guid_name(guid))))
    }

    /// A structured data asset, decoded.
    pub fn stu(&self, guid: u64) -> Result<orpheus_stu::Asset> {
        decode(guid, &self.raw(guid)?)
    }

    /// Several structured data assets, sorted by GUID, each decoded or with
    /// the reason it can't be. Bundles are read once.
    pub fn stu_all(&self, guids: &[u64]) -> Vec<(u64, Result<orpheus_stu::Asset>)> {
        self.game
            .read_all(guids)
            .into_iter()
            .map(|(guid, bytes)| {
                let asset = bytes
                    .map_err(|error| asset_error(format!("{}: {error}", guid_name(guid))))
                    .and_then(|bytes| decode(guid, &bytes));
                (guid, asset)
            })
            .collect()
    }

    /// A display string (type `07C`) in the install's locale, as DataTool
    /// reads it: a two-byte header, then UTF-8 up to the trailing zeros.
    /// `None` when the install has no such string or can't read it.
    pub fn string(&self, guid: u64) -> Option<String> {
        if guid == 0 || guid_type(guid) != STRING {
            return None;
        }
        let bytes = self.game.read(guid).ok()?;
        let text = String::from_utf8_lossy(bytes.get(2..)?);
        Some(text.trim_end_matches('\0').to_owned())
    }
}

fn decode(guid: u64, bytes: &[u8]) -> Result<orpheus_stu::Asset> {
    orpheus_stu::read(bytes).map_err(|error| asset_error(format!("{}: {error}", guid_name(guid))))
}

/// A GUID as the pack files write it: `0x` and sixteen upper-case digits.
pub fn guid_hex(guid: u64) -> String {
    format!("0x{guid:016X}")
}
