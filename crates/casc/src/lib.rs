//! Reads build 104319's game assets out of the player's own install.
//!
//! The path from a GUID to its bytes:
//! 1. `.build.info` names the active build config and keyring under
//!    `data/casc/config`;
//! 2. the build config names the encoding table and the root file;
//! 3. the root file names the content manifests (`.cmf`), which map GUIDs to
//!    content keys;
//! 4. the encoding table maps a content key to an encoding key;
//! 5. the local indices map an encoding key to an archive `data.NNN`, an
//!    offset and a size;
//! 6. the archive holds a 30-byte header and the file in BLTE form.
//!
//! Assets inside a bundle are a slice of the bundle at the offset its table
//! gives. Every read is checked against its content key.
//!
//! Ported from TACTLib (MIT) at revision `a00459fe`; see `CREDITS.md`.

#![forbid(unsafe_code)]

mod blte;
mod bundle;
mod config;
mod encoding;
mod error;
mod index;
mod manifest;
mod salsa20;

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    path::{Path, PathBuf},
};

use md5::{Digest, Md5};

pub use config::BuildInfo;
pub use error::Error;
pub use manifest::{Asset, BUILD};

use config::{Config, Keyring, hex_key};
use encoding::Encoding;
use error::format;
use index::Index;

pub type Result<T> = std::result::Result<T, Error>;

/// Each bundled asset's bundle and offset.
type BundleTable = HashMap<u64, (u64, u32)>;

/// One asset of [`Game::read_all`]: its GUID and its bytes.
pub type Read = (u64, Result<Vec<u8>>);

/// The asset type of a GUID, such as `0x01B` for statescript graphs. It sits
/// bit-reversed, minus one, at bits 48..59.
pub fn guid_type(guid: u64) -> u16 {
    let raw = ((guid >> 48) & 0xFFF) as u16;
    (raw.reverse_bits() >> 4) + 1
}

/// The GUID DataTool names `<index>.<type>`, such as `000000001D8E.01C`.
pub fn guid_name(guid: u64) -> String {
    format!("{:012X}.{:03X}", guid & 0xFFFF_FFFF_FFFF, guid_type(guid))
}

/// The manifests' region tag TACTLib and DataTool read by default.
const REGION: &str = "RDEV";
const BUNDLE_TYPE: u16 = 0x09C;

/// The local CASC storage under `<install>/data/casc`.
pub struct Storage {
    info: BuildInfo,
    build_config: Config,
    archives: Vec<Option<File>>,
    index: Index,
    encoding: Encoding,
    keys: Keyring,
}

impl Storage {
    pub fn open(install: &Path) -> Result<Storage> {
        let info = BuildInfo::read(install)?;
        let casc = install.join("data").join("casc");
        let build_key = info
            .get("BuildKey")
            .ok_or_else(|| format(".build.info has no build key"))?;
        let build_config = Config::read(&casc, build_key)?;
        let keys = match info.get("Keyring") {
            Some(hash) => Keyring::from_config(&Config::read(&casc, hash)?),
            None => Keyring::default(),
        };
        let data = casc.join("data");
        let archives = archives(&data)?;
        let index = Index::load(&data, |archive| {
            archives
                .get(usize::from(archive))?
                .as_ref()
                .map(|(_, size)| *size)
        })?;
        let archives = archives
            .into_iter()
            .map(|archive| archive.map(|(file, _)| file))
            .collect();
        let mut storage = Storage {
            info,
            build_config,
            archives,
            index,
            encoding: Encoding::default(),
            keys,
        };
        let ekey = storage
            .build_config
            .values("encoding")
            .and_then(|values| values.get(1))
            .and_then(|text| hex_key(text))
            .ok_or_else(|| format("the build config has no encoding key"))?;
        storage.encoding = Encoding::parse(&storage.read_ekey(&ekey)?)?;
        Ok(storage)
    }

    pub fn info(&self) -> &BuildInfo {
        &self.info
    }

    /// The decoded file with this encoding key.
    pub fn read_ekey(&self, ekey: &[u8; 16]) -> Result<Vec<u8>> {
        let location = self
            .index
            .find(ekey)
            .ok_or_else(|| Error::NotFound(format!("encoding key {} is not stored", hex(ekey))))?;
        let file = self
            .archives
            .get(usize::from(location.archive))
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                Error::NotFound(format!("archive data.{:03} is missing", location.archive))
            })?;
        let mut record = vec![0; location.size as usize];
        read_at(file, &mut record, location.offset.into())
            .map_err(Error::io(format!("data.{:03}", location.archive)))?;
        // {encoding key reversed, u32 size, 10 bytes} then the BLTE file.
        if record.len() < 30 || record[16..20] != location.size.to_le_bytes() {
            return Err(format(format!("archive entry of {} is damaged", hex(ekey))));
        }
        blte::decode(&record[30..], &self.keys)
    }

    /// The file with this content key, checked against it.
    pub fn read_ckey(&self, ckey: &[u8; 16]) -> Result<Vec<u8>> {
        let ekey = self.encoding.find(ckey).ok_or_else(|| {
            Error::NotFound(format!(
                "content key {} is not in the encoding table",
                hex(ckey)
            ))
        })?;
        let content = self.read_ekey(&ekey)?;
        check(&content, ckey)?;
        Ok(content)
    }
}

/// The game's assets by GUID, through the 104319 content manifests.
pub struct Game {
    storage: Storage,
    /// The text, speech and root manifests, searched in that order as TACTLib does.
    manifests: Vec<Vec<Asset>>,
    bundled: BundleTable,
    unreadable: Vec<(u64, Error)>,
}

impl Game {
    /// Open the install with the text and speech manifests of `locale`.
    pub fn open(install: &Path, locale: &str) -> Result<Game> {
        let storage = Storage::open(install)?;
        let root = storage
            .build_config
            .values("root")
            .and_then(|values| values.first())
            .and_then(|text| hex_key(text))
            .ok_or_else(|| format("the build config has no root key"))?;
        let root = storage.read_ckey(&root)?;
        let root = std::str::from_utf8(&root).map_err(|_| format("the root file is not text"))?;
        let (mut text, mut speech, mut base) = (None, None, None);
        for entry in manifest::root_entries(root)? {
            let file = Path::new(&entry.name)
                .file_name()
                .and_then(|name| name.to_str());
            let Some(file) = file.filter(|file| file.ends_with(".cmf") && file.contains(REGION))
            else {
                continue;
            };
            let stem = file.trim_end_matches(".cmf");
            if manifest::locale(stem).is_some_and(|tag| tag != locale) {
                continue;
            }
            let slot = if stem.contains("speech") {
                &mut speech
            } else if stem.contains("text") {
                &mut text
            } else {
                &mut base
            };
            let assets = manifest::content_manifest(&storage.read_ckey(&entry.ckey)?, file)?;
            *slot = Some(assets);
        }
        let base =
            base.ok_or_else(|| format(format!("the install has no {REGION} root manifest")))?;
        let manifests: Vec<Vec<Asset>> = [text, speech, Some(base)].into_iter().flatten().collect();
        let mut game = Game {
            storage,
            manifests,
            bundled: HashMap::new(),
            unreadable: Vec::new(),
        };
        (game.bundled, game.unreadable) = game.bundle_table()?;
        Ok(game)
    }

    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// The manifest record of a GUID.
    pub fn asset(&self, guid: u64) -> Option<Asset> {
        self.manifests.iter().find_map(|assets| {
            assets
                .binary_search_by_key(&guid, |asset| asset.guid)
                .ok()
                .map(|at| assets[at])
        })
    }

    /// Every GUID of one asset type, sorted.
    pub fn guids(&self, kind: u16) -> Vec<u64> {
        let mut guids: Vec<u64> = self
            .manifests
            .iter()
            .flatten()
            .map(|asset| asset.guid)
            .filter(|guid| guid_type(*guid) == kind)
            .collect();
        guids.sort_unstable();
        guids.dedup();
        guids
    }

    /// Whether the asset's bytes are stored, directly or in a readable bundle.
    pub fn is_stored(&self, guid: u64) -> bool {
        self.bundled.contains_key(&guid)
            || self
                .asset(guid)
                .is_some_and(|asset| self.storage.encoding.find(&asset.ckey).is_some())
    }

    /// The asset's bytes, checked against its content key.
    pub fn read(&self, guid: u64) -> Result<Vec<u8>> {
        let asset = self.record(guid)?;
        match self.bundled.get(&guid) {
            Some(&(bundle, offset)) => slice(&self.read(bundle)?, asset, offset),
            None => self.storage.read_ckey(&asset.ckey),
        }
    }

    /// Several assets, sorted by GUID, decoding each bundle once.
    pub fn read_all(&self, guids: &[u64]) -> Vec<Read> {
        let mut out = Vec::with_capacity(guids.len());
        let mut by_bundle: BTreeMap<u64, Vec<(u64, u32)>> = BTreeMap::new();
        for &guid in guids {
            match self.bundled.get(&guid) {
                Some(&(bundle, offset)) => {
                    by_bundle.entry(bundle).or_default().push((guid, offset))
                }
                None => out.push((guid, self.read(guid))),
            }
        }
        for (bundle, members) in by_bundle {
            let container = self.read(bundle).map_err(|error| error.to_string());
            for (guid, offset) in members {
                let result = match &container {
                    Ok(container) => self
                        .record(guid)
                        .and_then(|asset| slice(container, asset, offset)),
                    Err(error) => Err(format(format!("bundle {}: {error}", guid_name(bundle)))),
                };
                out.push((guid, result));
            }
        }
        out.sort_unstable_by_key(|(guid, _)| *guid);
        out
    }

    fn record(&self, guid: u64) -> Result<Asset> {
        self.asset(guid)
            .ok_or_else(|| Error::NotFound(format!("{} is not in the manifests", guid_name(guid))))
    }

    /// Bundles the storage could not read, with the reason; their assets
    /// are unavailable.
    pub fn unreadable_bundles(&self) -> &[(u64, Error)] {
        &self.unreadable
    }

    /// Where each bundled asset sits. Bundles whose content can't be read
    /// are skipped, as in TACTLib.
    fn bundle_table(&self) -> Result<(BundleTable, Vec<(u64, Error)>)> {
        let mut table = HashMap::new();
        let mut unreadable = Vec::new();
        for bundle in self.guids(BUNDLE_TYPE) {
            let data = match self.read(bundle) {
                Ok(data) => data,
                Err(error) => {
                    unreadable.push((bundle, error));
                    continue;
                }
            };
            for (guid, offset) in bundle::entries(&data)? {
                table.insert(guid, (bundle, offset));
            }
        }
        Ok((table, unreadable))
    }
}

/// The archives `data.NNN` by number, with their sizes.
fn archives(data: &Path) -> Result<Vec<Option<(File, u64)>>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(data).map_err(Error::io(data))? {
        let path: PathBuf = entry.map_err(Error::io(data))?.path();
        let number = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("data."))
            .filter(|digits| digits.len() == 3)
            .and_then(|digits| digits.parse::<usize>().ok());
        let Some(number) = number else {
            continue;
        };
        let file = File::open(&path).map_err(Error::io(&path))?;
        let size = file.metadata().map_err(Error::io(&path))?.len();
        if files.len() <= number {
            files.resize_with(number + 1, || None);
        }
        files[number] = Some((file, size));
    }
    Ok(files)
}

/// A bundled asset: its manifest size from its offset, checked against its content key.
fn slice(container: &[u8], asset: Asset, offset: u32) -> Result<Vec<u8>> {
    let start = offset as usize;
    let bytes = container
        .get(start..start + asset.size as usize)
        .ok_or_else(|| format(format!("{} runs past its bundle", guid_name(asset.guid))))?;
    check(bytes, &asset.ckey)?;
    Ok(bytes.to_vec())
}

fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buffer, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buffer.len() {
            match file.seek_read(&mut buffer[done..], offset + done as u64)? {
                0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                read => done += read,
            }
        }
        Ok(())
    }
}

fn check(content: &[u8], ckey: &[u8; 16]) -> Result<()> {
    if Md5::digest(content).as_slice() == ckey {
        Ok(())
    } else {
        Err(format(format!(
            "content does not match its key {}",
            hex(ckey)
        )))
    }
}

fn hex(key: &[u8]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_types_match_datatool_names() {
        assert_eq!(guid_type(0x0D80_0000_0000_1D8E), 0x01C);
        assert_eq!(guid_name(0x0D80_0000_0000_1D8E), "000000001D8E.01C");
        assert_eq!(guid_type(0x0D90_0000_0000_0001), BUNDLE_TYPE);
    }
}
