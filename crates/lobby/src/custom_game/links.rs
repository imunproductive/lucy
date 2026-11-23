//! Settings codes (Share Code / Import Code) and saved settings (presets):
//! the "custom game link" service of the 1.74 client, protocols
//! `0x1CA9B786` (40800-40814, client → server) and `0xAA91BE18`
//! (40900-40915, server → client). Research: `refs/reports/CustomGames.md`
//! §11 (agent CustomGames, 2026-11-23).
//!
//! - Share Code sends 40802 kind 2 with the whole settings (40803-40806
//!   chunks, no Workshop text) and an empty code, or the code it shares
//!   again; the answer is 40913 {code, again, status}. On status 0 the
//!   client writes the code into its settings' link block (`+0x198`) and
//!   uploads them to its lobby (24008).
//! - Import Code sends 40809 {code as typed}. The answer is the settings
//!   transfer 40901 kind 2 (40902-40906), whose `+0x198` must equal the
//!   code as the client normalises it ([`normalise`]); the client then
//!   adopts them and uploads them to its lobby itself. 40912 {code}:
//!   "Error importing Game Settings Code.".
//! - Presets: 40802 kind 0 saves a slot, kind 1 the last used settings;
//!   40808 names a slot, 40801 deletes one; 40800 {slot, 0} or {-1, 1}
//!   asks for a slot's or the last used settings, answered by 40901 kind
//!   0 or 1. 40900 lists the slots' names (the client never asks for it).

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use lucy_wire::jam::codec::{
    Fields, Schemas,
    Value::{self, Array, Bool, Signed, Struct, Unsigned},
};

use super::{CustomSettings, Group, UPLOAD, Upload, invalid, unsigned};
use crate::transport::LobbyMessage;

/// Client → server.
pub const LINKS_IN: u32 = 0x1ca9_b786;
/// Server → client.
pub const LINKS: u32 = 0xaa91_be18;

pub const GET_SAVED: u32 = 40800;
pub const DELETE_PRESET: u32 = 40801;
pub const NAME_PRESET: u32 = 40808;
pub const LOAD_LINK: u32 = 40809;
pub const PRESET_NAMES: u32 = 40900;
const LINK_FAILED: u32 = 40912;
const SHARED: u32 = 40913;

/// The client's settings upload to the service: 40802-40807; the header
/// also has the kind (`+0x78`), the slot (`+0x7C`), a time (`+0x80`) and
/// the code shared again (`+0x88`).
pub const LINK_UPLOAD: Group = Group {
    protocol: LINKS_IN,
    header: 40802,
    settings: 0xb0,
    extra: 0x2f0,
    more: 0x408,
};
/// The server's settings transfer: 40901-40906; the header also has the
/// kind (`+0x78`), the code (`+0x80`), the slot (`+0xA8`) and a time
/// (`+0xB0`).
const LINK_TRANSFER: Group = Group {
    protocol: LINKS,
    header: 40901,
    settings: 0xb8,
    extra: 0x2f8,
    more: 0x410,
};
/// What a 40802 upload is for (its `+0x78` kind).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Saved {
    /// 0: a preset slot.
    Preset(u32),
    /// 1: the last used settings, with the client's local time.
    LastUsed(u64),
    /// 2: a new code, or the code shared again.
    Share(Option<String>),
}

impl Saved {
    /// The kind of a complete 40802 upload's header.
    pub fn of(header: &Fields) -> io::Result<Self> {
        let number = |offset| header.get(&offset).and_then(unsigned);
        match number(0x78) {
            Some(0) => Ok(Self::Preset(number(0x7c).unwrap_or(0) as u32)),
            Some(1) => Ok(Self::LastUsed(number(0x80).unwrap_or(0))),
            Some(2) => Ok(Self::Share(match header.get(&0x88) {
                Some(Value::String(code)) if !code.is_empty() => Some(code.clone()),
                _ => None,
            })),
            other => Err(invalid(format!("a settings upload of kind {other:?}"))),
        }
    }
}

/// A code as the client compares it (`0x7FF7893460D0`): upper case, `I`
/// and `L` read as `1`, `O` as `0`. The server keys its codes the same
/// way, so a code typed either way finds them.
pub fn normalise(code: &str) -> String {
    code.trim()
        .chars()
        .map(|letter| match letter.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        })
        .collect()
}

/// 40913: the result of a Share Code. Status 0 is done; 2 "wait",
/// with the seconds; 4 not the code's owner; 5 a malformed code; 6 no
/// such code (`0x7FF7895DAEC0`).
pub fn shared(code: &str, again: bool, status: ShareStatus) -> LobbyMessage {
    let (status, wait) = match status {
        ShareStatus::Done => (0, 0),
        ShareStatus::NotOwner => (4, 0),
        ShareStatus::NoSuchCode => (6, 0),
        ShareStatus::Failed => (1, 0),
    };
    (
        LINKS,
        SHARED,
        Fields::from([
            (0x78, Value::String(code.into())),
            (0xa0, Bool(again)),
            (0xa1, Unsigned(status)),
            (0xa8, Unsigned(wait)),
        ]),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareStatus {
    Done,
    NotOwner,
    NoSuchCode,
    Failed,
}

/// 40912: the code could not be loaded.
pub fn link_failed(code: &str) -> LobbyMessage {
    (
        LINKS,
        LINK_FAILED,
        Fields::from([(0x78, Value::String(code.into()))]),
    )
}

/// 40901 kind 2: a code's settings, its link block naming the code as the
/// client normalised it, so the client adopts them (`0x7FF7895DC330`).
pub fn link(code: &str, settings: &CustomSettings) -> Vec<LobbyMessage> {
    let mut settings = settings.clone();
    settings.set_code(&normalise(code));
    settings.transfer(
        LINK_TRANSFER,
        Fields::from([
            (0x78, Signed(2)),
            (0x80, Value::String(code.into())),
            (0xa8, Signed(-1)),
            (0xb0, Unsigned(0)),
        ]),
    )
}

/// 40901 kind 0: a preset slot's settings.
pub fn preset(slot: u32, settings: &CustomSettings) -> Vec<LobbyMessage> {
    settings.transfer(
        LINK_TRANSFER,
        Fields::from([
            (0x78, Signed(0)),
            (0x80, Value::String(String::new())),
            (0xa8, Unsigned(u64::from(slot))),
            (0xb0, Unsigned(0)),
        ]),
    )
}

/// 40901 kind 1: the last used settings and when.
pub fn last_used(time: u64, settings: &CustomSettings) -> Vec<LobbyMessage> {
    settings.transfer(
        LINK_TRANSFER,
        Fields::from([
            (0x78, Signed(1)),
            (0x80, Value::String(String::new())),
            (0xa8, Signed(-1)),
            (0xb0, Unsigned(time)),
        ]),
    )
}

/// 40900: the preset slots' names.
pub fn preset_names(names: &BTreeMap<u32, String>) -> LobbyMessage {
    (
        LINKS,
        PRESET_NAMES,
        Fields::from([(
            0x78,
            Array(
                names
                    .iter()
                    .map(|(&slot, name)| {
                        Struct(Fields::from([
                            (0x0, Unsigned(u64::from(slot))),
                            (0x8, Value::String(name.clone())),
                        ]))
                    })
                    .collect(),
            ),
        )]),
    )
}

/// 40800 `{u32 slot, i8 which}`: which saved settings the client asks for.
pub fn saved_request(fields: &Fields) -> Option<SavedRequest> {
    match fields.get(&0x7c).and_then(unsigned) {
        Some(1) => Some(SavedRequest::LastUsed),
        _ => Some(SavedRequest::Preset(
            u32::try_from(fields.get(&0x78).and_then(unsigned)?).ok()?,
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedRequest {
    Preset(u32),
    LastUsed,
}

/// A request's string at `offset`.
pub fn text(fields: &Fields, offset: u32) -> String {
    match fields.get(&offset) {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The settings stored on disk as the client's own upload (24008-24013),
/// each message encoded with the lobby schemas.
fn encode(schemas: &Schemas, settings: &CustomSettings) -> io::Result<Vec<(u32, String)>> {
    settings
        .transfer(UPLOAD, Fields::new())
        .into_iter()
        .map(|(crc, id, fields)| {
            let body = schemas
                .encode(crc, id, &fields)
                .map_err(|error| invalid(format!("settings message {id}: {error}")))?;
            Ok((id, hex(&body)))
        })
        .collect()
}

fn decode(schemas: &Schemas, messages: &[(u32, String)]) -> io::Result<CustomSettings> {
    let mut upload = Upload::new(UPLOAD);
    for (id, body) in messages {
        let body = unhex(body)?;
        let fields = schemas
            .decode(UPLOAD.protocol, *id, &body, true)
            .map_err(|error| invalid(format!("settings message {id}: {error}")))?;
        if let Some(done) = upload.push(*id, fields)? {
            return Ok(done.settings);
        }
    }
    Err(invalid(
        "stored settings end before their last message".into(),
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> io::Result<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|at| {
            text.get(at..at + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| invalid("stored settings are not hex".into()))
        })
        .collect()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredSettings {
    /// The account that shared them (`account_lo`); presets: none.
    #[serde(default)]
    owner: Option<u32>,
    #[serde(default)]
    time: u64,
    messages: Vec<(u32, String)>,
}

/// One account's saved settings: preset names and settings by slot, and
/// the last used settings.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Presets {
    #[serde(default)]
    names: BTreeMap<u32, String>,
    #[serde(default)]
    slots: BTreeMap<u32, StoredSettings>,
    #[serde(default)]
    last_used: Option<StoredSettings>,
}

/// The server's settings codes and every account's presets, kept in the
/// state folder (`custom_games/`): a code's settings as the client shared
/// them, by the code as [`normalise`] writes it.
pub struct LinkStore {
    dir: PathBuf,
    schemas: Schemas,
}

/// The letters of a new code: no `I`, `L` or `O`, which the client reads
/// as digits ([`normalise`]), and no `0`/`1` either, to read it aloud.
const CODE_LETTERS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 5;

impl LinkStore {
    pub fn new(state: &Path) -> Self {
        Self {
            dir: state.join("custom_games"),
            schemas: Schemas::lobby(),
        }
    }

    fn code_path(&self, code: &str) -> Option<PathBuf> {
        let code = normalise(code);
        (!code.is_empty() && code.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then(|| self.dir.join("codes").join(format!("{code}.json")))
    }

    /// The settings under `code`, if the server has them.
    pub fn code(&self, code: &str) -> io::Result<Option<CustomSettings>> {
        let Some(path) = self.code_path(code) else {
            return Ok(None);
        };
        match read_json::<StoredSettings>(&path)? {
            Some(stored) => decode(&self.schemas, &stored.messages).map(Some),
            None => Ok(None),
        }
    }

    /// Share `settings` for `owner`: under a new code, or again under
    /// `again` when he owns it.
    pub fn share(
        &self,
        owner: u32,
        again: Option<&str>,
        settings: &CustomSettings,
    ) -> io::Result<Result<String, ShareStatus>> {
        let code = match again {
            Some(code) => {
                let Some(path) = self.code_path(code) else {
                    return Ok(Err(ShareStatus::NoSuchCode));
                };
                match read_json::<StoredSettings>(&path)? {
                    None => return Ok(Err(ShareStatus::NoSuchCode)),
                    Some(stored) if stored.owner != Some(owner) => {
                        return Ok(Err(ShareStatus::NotOwner));
                    }
                    Some(_) => normalise(code),
                }
            }
            None => self.new_code()?,
        };
        let stored = StoredSettings {
            owner: Some(owner),
            time: 0,
            messages: encode(&self.schemas, settings)?,
        };
        let path = self.code_path(&code).expect("a valid code");
        write_json(&path, &stored)?;
        Ok(Ok(code))
    }

    fn new_code(&self) -> io::Result<String> {
        loop {
            let mut bytes = [0u8; CODE_LENGTH];
            getrandom::fill(&mut bytes).map_err(io::Error::other)?;
            let code: String = bytes
                .iter()
                .map(|&byte| char::from(CODE_LETTERS[usize::from(byte) % CODE_LETTERS.len()]))
                .collect();
            let path = self.code_path(&code).expect("a valid code");
            if !path.exists() {
                return Ok(code);
            }
        }
    }

    fn presets_path(&self, account: u32) -> PathBuf {
        self.dir.join("presets").join(format!("{account}.json"))
    }

    fn presets(&self, account: u32) -> io::Result<Presets> {
        Ok(read_json(&self.presets_path(account))?.unwrap_or_default())
    }

    fn change(&self, account: u32, change: impl FnOnce(&mut Presets)) -> io::Result<()> {
        let mut presets = self.presets(account)?;
        change(&mut presets);
        write_json(&self.presets_path(account), &presets)
    }

    /// The account's preset names by slot.
    pub fn preset_names(&self, account: u32) -> io::Result<BTreeMap<u32, String>> {
        Ok(self.presets(account)?.names)
    }

    pub fn save(&self, account: u32, saved: &Saved, settings: &CustomSettings) -> io::Result<()> {
        let stored = StoredSettings {
            owner: None,
            time: match saved {
                Saved::LastUsed(time) => *time,
                _ => 0,
            },
            messages: encode(&self.schemas, settings)?,
        };
        self.change(account, |presets| match saved {
            Saved::Preset(slot) => {
                presets.slots.insert(*slot, stored);
            }
            Saved::LastUsed(_) => presets.last_used = Some(stored),
            Saved::Share(_) => {}
        })
    }

    pub fn name(&self, account: u32, slot: u32, name: String) -> io::Result<()> {
        self.change(account, |presets| {
            presets.names.insert(slot, name);
        })
    }

    pub fn delete(&self, account: u32, slot: u32) -> io::Result<()> {
        self.change(account, |presets| {
            presets.names.remove(&slot);
            presets.slots.remove(&slot);
        })
    }

    /// 40901 for the saved settings asked for, if the account has them.
    pub fn saved(&self, account: u32, request: SavedRequest) -> io::Result<Vec<LobbyMessage>> {
        let presets = self.presets(account)?;
        let stored = match request {
            SavedRequest::Preset(slot) => presets.slots.get(&slot),
            SavedRequest::LastUsed => presets.last_used.as_ref(),
        };
        let Some(stored) = stored else {
            return Ok(Vec::new());
        };
        let settings = decode(&self.schemas, &stored.messages)?;
        Ok(match request {
            SavedRequest::Preset(slot) => preset(slot, &settings),
            SavedRequest::LastUsed => last_used(stored.time, &settings),
        })
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| invalid(format!("{}: {error}", path.display()))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Written whole to a temporary file, then moved over the old one.
fn write_json(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let dir = path.parent().expect("a file in a folder");
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_game::{Chunk, ChunkKind};

    fn settings(map: u64) -> CustomSettings {
        CustomSettings {
            settings: Fields::from([(0x218, Value::String("Parkour".into()))]),
            extra: Fields::new(),
            chunks: vec![Chunk {
                kind: ChunkKind::Map,
                fields: Fields::from([(
                    0x78,
                    Struct(Fields::from([
                        (0x0, Struct(Fields::new())),
                        (0x138, Unsigned(map)),
                    ])),
                )]),
            }],
        }
    }

    #[test]
    fn a_code_reads_as_the_client_compares_it() {
        assert_eq!(normalise(" c32s2 "), "C32S2");
        assert_eq!(normalise("olive"), "011VE");
    }

    #[test]
    fn a_shared_code_loads_back_and_only_its_owner_shares_it_again() {
        let state = tempfile::tempdir().unwrap();
        let store = LinkStore::new(state.path());
        let code = store.share(7, None, &settings(1)).unwrap().unwrap();
        assert_eq!(code.len(), CODE_LENGTH);
        assert_eq!(normalise(&code), code);
        let loaded = store.code(&code.to_lowercase()).unwrap().unwrap();
        assert_eq!(loaded.maps(), Vec::<u64>::new(), "no mode: no game");
        assert_eq!(loaded.settings[&0x218], Value::String("Parkour".into()));
        assert_eq!(loaded.chunks.len(), 1);
        assert_eq!(
            store.share(8, Some(&code), &settings(2)).unwrap(),
            Err(ShareStatus::NotOwner)
        );
        assert_eq!(
            store.share(7, Some("ZZZZZ"), &settings(2)).unwrap(),
            Err(ShareStatus::NoSuchCode)
        );
        assert_eq!(
            store.share(7, Some(&code), &settings(2)).unwrap(),
            Ok(code.clone())
        );
        let Value::Struct(entry) = &store.code(&code).unwrap().unwrap().chunks[0].fields[&0x78]
        else {
            panic!()
        };
        assert_eq!(entry[&0x138], Unsigned(2));
        assert!(store.code("NOPE1").unwrap().is_none());
        assert!(store.code("../x").unwrap().is_none());
    }

    #[test]
    fn presets_keep_their_names_and_settings_per_account() {
        let state = tempfile::tempdir().unwrap();
        let store = LinkStore::new(state.path());
        store.save(7, &Saved::Preset(2), &settings(1)).unwrap();
        store.name(7, 2, "Parkour".into()).unwrap();
        store.save(7, &Saved::LastUsed(99), &settings(3)).unwrap();
        assert_eq!(
            store.preset_names(7).unwrap(),
            BTreeMap::from([(2, "Parkour".to_owned())])
        );
        assert!(store.preset_names(8).unwrap().is_empty());
        let slot = store.saved(7, SavedRequest::Preset(2)).unwrap();
        assert_eq!(slot[0].1, 40901);
        assert_eq!(slot[0].2[&0x78], Signed(0));
        assert_eq!(slot[0].2[&0xa8], Unsigned(2));
        let last = store.saved(7, SavedRequest::LastUsed).unwrap();
        assert_eq!(last[0].2[&0x78], Signed(1));
        assert_eq!(last[0].2[&0xb0], Unsigned(99));
        store.delete(7, 2).unwrap();
        assert!(store.saved(7, SavedRequest::Preset(2)).unwrap().is_empty());
        assert!(store.preset_names(7).unwrap().is_empty());
    }

    #[test]
    fn an_import_answer_names_the_code_the_client_waits_for() {
        let schemas = Schemas::lobby();
        let messages = link("c32s2", &settings(1));
        let ids: Vec<u32> = messages.iter().map(|(_, id, _)| *id).collect();
        assert_eq!(ids, [40901, 40902]);
        let Value::Struct(sent) = &messages[0].2[&0xb8] else {
            panic!()
        };
        assert_eq!(
            sent[&0x128],
            Struct(Fields::from([(0x70, Value::String("C32S2".into()))]))
        );
        assert_eq!(messages[0].2[&0x410], Bool(true));
        assert_eq!(messages[1].2[&0x1b8], Bool(false));
        for (crc, id, fields) in messages
            .iter()
            .chain(&[shared("ABCDE", false, ShareStatus::Done), link_failed("X")])
        {
            schemas.encode(*crc, *id, fields).unwrap();
        }
        schemas
            .encode(
                LINKS,
                PRESET_NAMES,
                &preset_names(&BTreeMap::from([(1, "A".into())])).2,
            )
            .unwrap();
    }
}
