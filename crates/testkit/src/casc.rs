//! A miniature install in the layout of build 104319's local storage, for
//! tests that read game assets without the game.
//!
//! Every file is stored as one uncompressed BLTE block in `data.000`, so a
//! test can find an asset's bytes in the archive and damage them. The content
//! manifest is unencrypted; the encrypted form is tested in `orpheus-casc`.

use std::{fs, path::Path};

use md5::{Digest, Md5};

/// The DataTool type tag of a GUID built by [`guid`].
pub const GRAPH: u16 = 0x01B;
pub const DEFINITION: u16 = 0x003;
const BUNDLE: u16 = 0x09C;
const BUILD: u32 = 104_319;
const MANIFEST: &str = "TactManifest/Win_SPWin_RDEV_EExt.cmf";

/// A GUID of the given asset type: the type minus one, bit-reversed, at bits 48..59.
pub fn guid(kind: u16, index: u64) -> u64 {
    let raw = ((kind - 1) << 4).reverse_bits();
    u64::from(raw) << 48 | index
}

#[derive(Default)]
pub struct MiniInstall {
    direct: Vec<(u64, Vec<u8>)>,
    bundled: Vec<(u64, Vec<u8>)>,
}

impl MiniInstall {
    /// An asset stored as its own file.
    pub fn asset(mut self, guid: u64, bytes: &[u8]) -> Self {
        self.direct.push((guid, bytes.to_vec()));
        self
    }

    /// An asset stored inside the install's one bundle.
    pub fn bundled(mut self, guid: u64, bytes: &[u8]) -> Self {
        self.bundled.push((guid, bytes.to_vec()));
        self
    }

    /// Write `.build.info` and `data/casc` under `root`.
    pub fn write(&self, root: &Path) {
        let mut archive = Archive::default();
        let mut manifest = Vec::new();
        for (guid, bytes) in &self.direct {
            manifest.push((*guid, bytes.len(), archive.store(bytes)));
        }
        if !self.bundled.is_empty() {
            let mut bundle = (self.bundled.len() as i32).to_le_bytes().to_vec();
            bundle.push(4);
            let mut offset = 5 + 12 * self.bundled.len();
            for (guid, bytes) in &self.bundled {
                bundle.extend(guid.to_le_bytes());
                bundle.extend((offset as u32).to_le_bytes());
                offset += bytes.len();
                manifest.push((*guid, bytes.len(), md5(bytes)));
            }
            self.bundled
                .iter()
                .for_each(|(_, bytes)| bundle.extend(bytes));
            manifest.push((guid(BUNDLE, 1), bundle.len(), archive.store(&bundle)));
        }
        let cmf = archive.store(&content_manifest(&manifest));
        let root_file = format!(
            "#FILEID|MD5|CHUNK_ID|PRIORITY|MPRIORITY|FILENAME|INSTALLPATH\n\
             0|{}|0|0|0|{MANIFEST}|\n\
             1|{}|0|0|0|TactManifest/Win_SPWin_RDEV_LdeDE_EExt.cmf|\n",
            hex(&cmf),
            "00".repeat(16),
        );
        let root_ckey = archive.store(root_file.as_bytes());
        let (encoding_ckey, encoding_ekey) = archive.store_encoding();

        let casc = root.join("data").join("casc");
        let build_config = format!(
            "root = {}\nencoding = {} {}\n",
            hex(&root_ckey),
            hex(&encoding_ckey),
            hex(&encoding_ekey)
        );
        let build_key = hex(&md5(build_config.as_bytes()));
        let config = casc
            .join("config")
            .join(&build_key[..2])
            .join(&build_key[2..4]);
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join(&build_key), build_config).unwrap();
        fs::write(
            root.join(".build.info"),
            format!(
                "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|Keyring!HEX:16|Version!STRING:0\n\
                 eu|1|{build_key}||1.74.0.0.{BUILD}\n"
            ),
        )
        .unwrap();
        archive.write(&casc.join("data"));
    }
}

#[derive(Default)]
struct Archive {
    data: Vec<u8>,
    files: Vec<Stored>,
}

struct Stored {
    ckey: [u8; 16],
    ekey: [u8; 16],
    size: usize,
    offset: usize,
    record: usize,
}

impl Archive {
    /// Store a file; its content key.
    fn store(&mut self, content: &[u8]) -> [u8; 16] {
        let mut blte = b"BLTE\0\0\0\0N".to_vec();
        blte.extend(content);
        let ckey = md5(content);
        let ekey = md5(&blte);
        let offset = self.data.len();
        let size = 30 + blte.len();
        self.data.extend(ekey.iter().rev());
        self.data.extend((size as u32).to_le_bytes());
        self.data.extend([0; 10]);
        self.data.extend(blte);
        self.files.push(Stored {
            ckey,
            ekey,
            size: content.len(),
            offset,
            record: size,
        });
        ckey
    }

    /// Store the encoding table of everything stored so far.
    fn store_encoding(&mut self) -> ([u8; 16], [u8; 16]) {
        let mut entries: Vec<_> = self
            .files
            .iter()
            .map(|file| (file.ckey, file.ekey, file.size))
            .collect();
        entries.sort();
        let mut page = Vec::new();
        for (ckey, ekey, size) in entries {
            page.push(1);
            page.extend(&(size as u64).to_be_bytes()[3..]);
            page.extend(ckey);
            page.extend(ekey);
        }
        assert!(
            page.len() <= 4096,
            "the miniature encoding table has one page"
        );
        page.resize(4096, 0);
        let mut table = b"EN\x01\x10\x10".to_vec();
        table.extend(4u16.to_be_bytes());
        table.extend(4u16.to_be_bytes());
        table.extend(1u32.to_be_bytes());
        table.extend(0u32.to_be_bytes());
        table.push(0);
        table.extend(0u32.to_be_bytes());
        table.extend([0; 32]);
        table.extend(page);
        let ckey = self.store(&table);
        let ekey = self.files.last().unwrap().ekey;
        (ckey, ekey)
    }

    fn write(&self, data: &Path) {
        fs::create_dir_all(data).unwrap();
        fs::write(data.join("data.000"), &self.data).unwrap();
        for bucket in 0..16u8 {
            let mut entries = Vec::new();
            for Stored {
                ekey,
                offset,
                record,
                ..
            } in &self.files
            {
                let folded = ekey[..9].iter().fold(0, |value, byte| value ^ byte);
                if (folded & 0xF) ^ (folded >> 4) != bucket {
                    continue;
                }
                entries.extend(&ekey[..9]);
                entries.push(0);
                entries.extend((*offset as u32).to_be_bytes());
                entries.extend((*record as u32).to_le_bytes());
            }
            let mut index = 16u32.to_le_bytes().to_vec();
            index.extend([0; 4]);
            index.extend(7u16.to_le_bytes());
            index.extend([bucket, 0, 4, 5, 9, 30]);
            index.extend(0x40_0000_0000u64.to_le_bytes());
            index.extend([0; 8]);
            index.extend((entries.len() as u32).to_le_bytes());
            index.extend([0; 4]);
            index.extend(entries);
            fs::write(data.join(format!("{bucket:02x}00000001.idx")), index).unwrap();
        }
    }
}

/// An unencrypted 1.48+ content manifest with one entry.
fn content_manifest(assets: &[(u64, usize, [u8; 16])]) -> Vec<u8> {
    let magic = 0x0066_6D63 | 26 << 24;
    let header = [BUILD, 0, 0, 0, 0, 0, 0, 0, assets.len() as u32, 0, 1, magic];
    let mut cmf: Vec<u8> = header
        .iter()
        .flat_map(|field| field.to_le_bytes())
        .collect();
    cmf.extend(1u32.to_le_bytes());
    cmf.extend([0; 16]);
    for (guid, size, ckey) in assets {
        cmf.extend(guid.to_le_bytes());
        cmf.extend((*size as u32).to_le_bytes());
        cmf.push(0);
        cmf.extend(ckey);
    }
    cmf
}

fn md5(bytes: &[u8]) -> [u8; 16] {
    Md5::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
