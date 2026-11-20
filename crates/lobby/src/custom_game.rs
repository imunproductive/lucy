//! Custom games: Play > Custom Games > Create, the lobby and Start.
//!
//! What the client does (`refs/reports/CustomGames.md`, image base
//! 0x7FF788F10000):
//! - Create sends 24000 kind 1 (`0x7FF7895DEAB0`, which marks a create as
//!   pending) and at once its default settings (`0x7FF7895DD340`).
//! - Settings travel as a chunked transfer: a header with the 576-byte
//!   settings struct and a "more follows" bool, then one message per entry
//!   of the large arrays, each with its own "more" bool (`0x7FF7895E42D0`).
//!   The client sends 24008-24013; the server answers with the same shapes:
//!   23304-23309 enter the lobby (with the lobby descriptor), 23312-23317
//!   update the settings. The settings' per-map array (`+0x0`) must come in
//!   its own messages: a header with entries there fails the transfer
//!   (`0x7FF7895E01E0`).
//! - The lobby descriptor's `+0x78` is what tells the client it is in a
//!   custom game lobby (system 38 +832, `0x7FF789D2D110`); its 72-byte list
//!   is the team colour sets, which put the client in team-colour mode, so
//!   none are sent.
//! - The seats are the match lobby's (53000 type 1 CUSTOM_GAME, 53001,
//!   53002); 24018, the lobby command, needs 53001's settings id.

use std::{collections::BTreeMap, io};

use crate::{
    handoff::GAME_REQUEST,
    lineup::{self, MATCH_LOBBY, Player},
    templates::CUSTOM_GAMES,
    transport::LobbyMessage,
};
use orpheus_wire::jam::codec::{
    Fields, Schemas, Value,
    Value::{Array, Bool, Struct, Unsigned},
    id16,
};

/// 24000 `+0x78`: create a custom game (2 is the Practice Range, 3 the
/// Tutorial).
pub const CREATE_CUSTOM_GAME: u64 = 1;
/// 24000's name (`+0x80`).
const CREATE_NAME: u32 = 0x80;
/// 24018 `{u8}`: a lobby command. The client checks its settings before it
/// sends 3, so 3 is taken to be Start (`0x7FF7895E2080`; the values are an
/// enum of the UI graphs, `113A7BAD`).
pub const LOBBY_COMMAND: u32 = 24018;
pub const START: u64 = 3;

/// The client's settings upload: header and chunks.
pub const UPLOAD: [u32; 6] = [24008, 24009, 24010, 24011, 24012, 24013];
/// The server's answers of the same shapes.
const ENTER_LOBBY: u32 = 23304;
const SETTINGS_UPDATE: u32 = 23312;

/// The header's settings, extra block and "more" bool: 24008 / 23312
/// offsets, and 23304's (after the descriptor and colour sets).
const SETTINGS: u32 = 0x78;
const EXTRA: u32 = 0x2b8;
const MORE: u32 = 0x3d0;
const ENTER_DESCRIPTOR: u32 = 0x78;
const ENTER_COLOURS: u32 = 0x128;
const ENTER_SETTINGS: u32 = 0x140;
const ENTER_EXTRA: u32 = 0x380;
const ENTER_MORE: u32 = 0x498;

/// The settings struct's per-map entries (`{312 block, +0x138 map}`) and
/// per-mode blocks; the Workshop block.
const MAPS: u32 = 0x0;
const MODES: u32 = 0x18;
const WORKSHOP: u32 = 0x30;
const ENTRY_MAP: u32 = 0x138;
/// A 312-byte block: its 168-byte part, the mode (as the game struct has
/// it, `orpheus_sim::instance`), the teams.
const BLOCK: u32 = 0x0;
const BLOCK_MODE: u32 = 0xb0;
/// `+0xA8`: the other u64 the Workshop report names the game mode
/// (`refs/reports/Workshop.md` §3.1); the game struct's mode is `+0xB0`.
const BLOCK_MODE_A8: u32 = 0xa8;
/// A block's `{u32 value, u16 key, u8 kind}` lists: `masterVars`,
/// `globalAdminVars` (`CGGameRuleset`).
const MASTER_VARS: u32 = 0xd8;
const ADMIN_VARS: u32 = 0xf0;
const BLOCK_TEAMS: u32 = 0xc0;
const TEAM_NUMBER: u32 = 0x60;
const TEAM_SIZE: u32 = 0x61;

/// What a chunk message carries, and the offset of its "more" bool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkKind {
    /// 24009: one per-map entry (`{312 block, +0x138 map GUID}`).
    Map,
    /// 24010: one per-mode block.
    Mode,
    /// 24011: a team block's per-hero entry with its two indices.
    Team,
    /// 24012: one item of a Workshop array.
    Workshop,
    /// 24013: the 240-byte lists.
    Lists,
}

impl ChunkKind {
    const ALL: [Self; 5] = [
        Self::Map,
        Self::Mode,
        Self::Team,
        Self::Workshop,
        Self::Lists,
    ];

    /// Its place after the header in each group of six messages.
    fn index(self) -> u32 {
        match self {
            Self::Map => 1,
            Self::Mode => 2,
            Self::Team => 3,
            Self::Workshop => 4,
            Self::Lists => 5,
        }
    }

    fn of(index: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.index() == index)
    }

    fn more(self) -> u32 {
        match self {
            Self::Map => 0x1b8,
            Self::Mode => 0x1b0,
            Self::Team => 0xa0,
            Self::Workshop => 0x1d1,
            Self::Lists => 0x191,
        }
    }
}

/// One chunk of a transfer, without its "more" bool.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub kind: ChunkKind,
    pub fields: Fields,
}

/// A custom game's settings as the client uploaded them, decoded: the
/// header's 576-byte struct (its per-map and per-mode arrays empty, as
/// sent), its 280-byte extra block, and the chunks in their order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CustomSettings {
    pub settings: Fields,
    pub extra: Fields,
    pub chunks: Vec<Chunk>,
}

impl CustomSettings {
    fn chunk_values(&self, kind: ChunkKind) -> impl Iterator<Item = &Fields> {
        self.chunks
            .iter()
            .filter(move |chunk| chunk.kind == kind)
            .filter_map(|chunk| match chunk.fields.get(&0x78) {
                Some(Struct(value)) => Some(value),
                _ => None,
            })
    }

    /// The per-map entries' maps, in upload order: the enabled maps
    /// (`CustomGames.md` §2, an inference).
    pub fn maps(&self) -> Vec<u64> {
        self.chunk_values(ChunkKind::Map)
            .filter_map(|entry| unsigned(entry.get(&ENTRY_MAP)?))
            .collect()
    }

    /// The modes the per-mode blocks name (`+0xB0`), in upload order.
    pub fn modes(&self) -> Vec<u64> {
        self.chunk_values(ChunkKind::Mode)
            .filter_map(|block| unsigned(block.get(&BLOCK_MODE)?))
            .filter(|&mode| mode != 0)
            .collect()
    }

    /// Team sizes by team number from the first block, of a map entry of
    /// `map` or of a mode block of `mode`, that has teams: each team's
    /// `+0x60` number and `+0x61` size, as the game struct reads them.
    pub fn team_sizes(&self, map: u64, mode: u64) -> Option<Vec<u8>> {
        let maps = self
            .chunk_values(ChunkKind::Map)
            .filter(|entry| entry.get(&ENTRY_MAP).and_then(unsigned) == Some(map))
            .filter_map(|entry| match entry.get(&BLOCK) {
                Some(Struct(block)) => Some(block),
                _ => None,
            });
        let modes = self
            .chunk_values(ChunkKind::Mode)
            .filter(|block| block.get(&BLOCK_MODE).and_then(unsigned) == Some(mode));
        maps.chain(modes).find_map(team_sizes)
    }

    /// The Workshop: the header's 168-byte block and the Workshop chunks,
    /// undecoded beyond their shapes (a rule engine reads them later).
    pub fn workshop(&self) -> Workshop {
        Workshop {
            block: match self.settings.get(&WORKSHOP) {
                Some(Struct(block)) => block.clone(),
                _ => Fields::new(),
            },
            items: self
                .chunks
                .iter()
                .filter(|chunk| chunk.kind == ChunkKind::Workshop)
                .map(|chunk| chunk.fields.clone())
                .collect(),
        }
    }

    /// The transfer of these settings in a group of the custom games
    /// protocol: the header (`header` with the settings, the extra block
    /// and "more"), then the chunks. The last message says no more.
    fn transfer(&self, group: u32, mut header: Fields, offsets: [u32; 3]) -> Vec<LobbyMessage> {
        let [settings, extra, more] = offsets;
        let mut sent = self.settings.clone();
        // The per-map entries must not ride in the header
        // (0x7FF7895E01E0); the client's own header has both arrays empty.
        sent.insert(MAPS, Array(Vec::new()));
        sent.insert(MODES, Array(Vec::new()));
        header.insert(settings, Struct(sent));
        header.insert(extra, Struct(self.extra.clone()));
        header.insert(more, Bool(!self.chunks.is_empty()));
        let mut messages = vec![(CUSTOM_GAMES, group, header)];
        for (index, chunk) in self.chunks.iter().enumerate() {
            let mut fields = chunk.fields.clone();
            fields.insert(chunk.kind.more(), Bool(index + 1 < self.chunks.len()));
            messages.push((CUSTOM_GAMES, group + chunk.kind.index(), fields));
        }
        messages
    }

    /// 23312-23317: the server's settings of the lobby (the client adopts
    /// them as its own, system 38 slot 6 `0x7FF7895091E0`).
    pub fn update(&self) -> Vec<LobbyMessage> {
        self.transfer(SETTINGS_UPDATE, Fields::new(), [SETTINGS, EXTRA, MORE])
    }

    /// The ruleset values a match of `mode` on `map` plays with: the
    /// `{u32 value, u16 key, u8 kind}` pairs (`masterVars` `+0xD8`, then
    /// `globalAdminVars` `+0xF0`) of the mode blocks of `mode` (`+0xB0` or
    /// `+0xA8`), then of the map entries of `map` over them. The key is the
    /// `01C` ruleset key and the value is read by its type as the client's
    /// own lookup reads it (`0x7FF789356650`: bool `value != 0`, int, float
    /// bits, `01C` index); the kind byte is taken to be that type, 1 to 4
    /// (an inference, `CustomGames.md` §2). Other kinds are left out.
    pub fn ruleset_values(&self, map: u64, mode: u64) -> BTreeMap<u32, RulesetValue> {
        let modes = self.chunk_values(ChunkKind::Mode).filter(|block| {
            [BLOCK_MODE, BLOCK_MODE_A8]
                .iter()
                .any(|offset| block.get(offset).and_then(unsigned) == Some(mode))
        });
        let maps = self
            .chunk_values(ChunkKind::Map)
            .filter(|entry| entry.get(&ENTRY_MAP).and_then(unsigned) == Some(map))
            .filter_map(|entry| match entry.get(&BLOCK) {
                Some(Struct(block)) => Some(block),
                _ => None,
            });
        let mut values = BTreeMap::new();
        for block in modes.chain(maps) {
            for list in [MASTER_VARS, ADMIN_VARS] {
                let Some(Array(pairs)) = block.get(&list) else {
                    continue;
                };
                for pair in pairs {
                    if let Struct(pair) = pair
                        && let Some((key, value)) = ruleset_pair(pair)
                    {
                        values.insert(key, value);
                    }
                }
            }
        }
        values
    }

    /// The Workshop as the client sent it, JAM-encoded again from the
    /// decoded upload (the encoding is the same bytes): the settings header
    /// (24008, whose settings `+0x30` is the logic, `PBCGLogic`) and each
    /// Workshop item (24012), all with "more" false.
    pub fn workshop_block(&self, schemas: &Schemas) -> io::Result<WorkshopBlock> {
        let encode = |id: u32, fields: &Fields| {
            schemas
                .encode(GAME_REQUEST, id, fields)
                .map_err(|error| invalid(format!("{id}: {error}")))
        };
        let header = Fields::from([
            (SETTINGS, Struct(self.settings.clone())),
            (EXTRA, Struct(self.extra.clone())),
            (MORE, Bool(false)),
        ]);
        let rules = self
            .chunks
            .iter()
            .filter(|chunk| chunk.kind == ChunkKind::Workshop)
            .map(|chunk| {
                let mut fields = chunk.fields.clone();
                fields.insert(ChunkKind::Workshop.more(), Bool(false));
                encode(UPLOAD[ChunkKind::Workshop.index() as usize], &fields)
            })
            .collect::<io::Result<_>>()?;
        Ok(WorkshopBlock {
            header: encode(UPLOAD[0], &header)?,
            rules,
        })
    }
}

/// A ruleset value of the custom settings, typed as the client reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RulesetValue {
    Bool(bool),
    Integer(i64),
    Float(f32),
    /// A `01C` identifier (an enum choice), as a packed GUID.
    Asset(u64),
}

/// The packed GUID type bits of a `01C` identifier (`254D.01C` is
/// `0x0D8000000000254D`).
const IDENTIFIER_GUID: u64 = 0x0d80_0000_0000_0000;

fn ruleset_pair(pair: &Fields) -> Option<(u32, RulesetValue)> {
    let value = u32::try_from(pair.get(&0).and_then(unsigned)?).ok()?;
    let key = u32::try_from(pair.get(&4).and_then(unsigned)?).ok()?;
    let value = match pair.get(&6).and_then(unsigned)? {
        1 => RulesetValue::Bool(value != 0),
        2 => RulesetValue::Integer(i64::from(value as i32)),
        3 => RulesetValue::Float(f32::from_bits(value)),
        4 => RulesetValue::Asset(IDENTIFIER_GUID | u64::from(value)),
        _ => return None,
    };
    Some((key, value))
}

/// A custom game's Workshop, raw, for the Workshop engine to decode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkshopBlock {
    /// The settings header's JAM body (24008).
    pub header: Vec<u8>,
    /// Each Workshop item's JAM body (24012), in upload order.
    pub rules: Vec<Vec<u8>>,
}

/// What a custom game's match carries: the settings as uploaded and its
/// Workshop, raw.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomMatch {
    pub settings: CustomSettings,
    pub workshop: WorkshopBlock,
}

/// The Workshop part of the settings, as uploaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Workshop {
    /// Settings `+0x30`: rules, the two other arrays, the u32 list and the
    /// three name lists.
    pub block: Fields,
    /// The 24012 items, each with its array selector and index.
    pub items: Vec<Fields>,
}

impl Workshop {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
            && self
                .block
                .values()
                .all(|value| matches!(value, Array(items) if items.is_empty()))
    }
}

fn unsigned(value: &Value) -> Option<u64> {
    match value {
        Unsigned(value) => Some(*value),
        Value::Signed(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

fn team_sizes(block: &Fields) -> Option<Vec<u8>> {
    let Some(Array(teams)) = block.get(&BLOCK_TEAMS) else {
        return None;
    };
    let mut sizes = Vec::new();
    for team in teams {
        let Struct(team) = team else { continue };
        let number = team.get(&TEAM_NUMBER).and_then(unsigned)?;
        let size = team.get(&TEAM_SIZE).and_then(unsigned)?;
        let number = usize::try_from(number).ok().filter(|&number| number < 4)?;
        if sizes.len() <= number {
            sizes.resize(number + 1, 0);
        }
        sizes[number] = u8::try_from(size).ok()?;
    }
    (!sizes.is_empty()).then_some(sizes)
}

/// Reassembles the client's settings uploads (24008-24013).
#[derive(Debug, Default)]
pub struct Upload {
    pending: Option<CustomSettings>,
}

impl Upload {
    /// Take one upload message; the settings when it was the last one.
    /// A header starts a new upload; a chunk without one is an error.
    pub fn push(&mut self, id: u32, mut fields: Fields) -> io::Result<Option<CustomSettings>> {
        let more = |fields: &mut Fields, offset| matches!(fields.remove(&offset), Some(value) if truthy(&value));
        let index = id
            .checked_sub(UPLOAD[0])
            .filter(|&index| index < 6)
            .ok_or_else(|| invalid(format!("{id} is not a settings upload message")))?;
        let follows = if index == 0 {
            let follows = more(&mut fields, MORE);
            let struct_at = |fields: &mut Fields, offset| match fields.remove(&offset) {
                Some(Struct(value)) => value,
                _ => Fields::new(),
            };
            self.pending = Some(CustomSettings {
                settings: struct_at(&mut fields, SETTINGS),
                extra: struct_at(&mut fields, EXTRA),
                chunks: Vec::new(),
            });
            follows
        } else {
            let kind = ChunkKind::of(index).expect("chunk index");
            let follows = more(&mut fields, kind.more());
            self.pending
                .as_mut()
                .ok_or_else(|| invalid(format!("{id} came without a 24008 before it")))?
                .chunks
                .push(Chunk { kind, fields });
            follows
        };
        Ok(if follows { None } else { self.pending.take() })
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Bool(value) => *value,
        Unsigned(value) => *value != 0,
        Value::Signed(value) => *value != 0,
        _ => false,
    }
}

fn invalid(error: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

/// The 24000 of a custom game: its name, when it is one.
pub fn create_request((crc, id, fields): &LobbyMessage) -> Option<String> {
    if (*crc, *id) != (GAME_REQUEST, crate::handoff::CREATE_GAME)
        || fields.get(&0x78).and_then(unsigned) != Some(CREATE_CUSTOM_GAME)
    {
        return None;
    }
    Some(match fields.get(&CREATE_NAME) {
        Some(Value::String(name)) => name.clone(),
        _ => String::new(),
    })
}

/// A custom game lobby and its host.
#[derive(Clone, Debug)]
pub struct Lobby {
    pub id: [u64; 2],
    pub name: String,
    /// The host's card (`player_messages::record`).
    pub host_card: Fields,
    /// The host's party, for the roster's group.
    pub host_group: [u64; 2],
}

impl Lobby {
    /// The lobby descriptor (176 bytes): the host's card, the lobby id, the
    /// non-zero `+0x78` that marks the client as in a lobby (the id's high
    /// half; what retail sends is open) and the name.
    fn descriptor(&self) -> Fields {
        Fields::from([
            (0x0, Struct(self.host_card.clone())),
            (0x68, Struct(id16(self.id[0], self.id[1]))),
            (0x78, Unsigned(self.id[1].max(1))),
            (0x88, Value::String(self.name.clone())),
        ])
    }

    /// Everything that puts the host in his new lobby, in order: 23304-23309
    /// (descriptor, no colour sets, the settings he uploaded), then the
    /// match lobby: 53000 type 1, 53001 with the lobby id as its settings id
    /// (24018 needs one), 53002 with him in team 0's first slot.
    pub fn enter(&self, settings: &CustomSettings) -> Vec<LobbyMessage> {
        let header = Fields::from([
            (ENTER_DESCRIPTOR, Struct(self.descriptor())),
            (ENTER_COLOURS, Array(Vec::new())),
        ]);
        let mut messages = settings.transfer(
            ENTER_LOBBY,
            header,
            [ENTER_SETTINGS, ENTER_EXTRA, ENTER_MORE],
        );
        messages.extend([
            lineup::custom_game_lobby(self.id),
            (
                MATCH_LOBBY,
                53001,
                lineup::settings(self.id, CUSTOM_INSTANCE),
            ),
            (
                MATCH_LOBBY,
                53002,
                lineup::roster(&[Player {
                    card: self.host_card.clone(),
                    team: 0,
                    role: 0,
                    rating: None,
                    group: self.host_group,
                }]),
            ),
        ]);
        messages
    }
}

/// 53001's instance type for a custom game: 1, as a match's 20300 has it
/// when nobody is ranked (`orpheus_sim::instance`).
const CUSTOM_INSTANCE: u64 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_wire::jam::codec::Schemas;

    fn block(mode: u64, teams: &[(u64, u64)]) -> Fields {
        Fields::from([
            (BLOCK_MODE, Unsigned(mode)),
            (
                BLOCK_TEAMS,
                Array(
                    teams
                        .iter()
                        .map(|&(number, size)| {
                            Struct(Fields::from([
                                (TEAM_NUMBER, Unsigned(number)),
                                (TEAM_SIZE, Unsigned(size)),
                            ]))
                        })
                        .collect(),
                ),
            ),
        ])
    }

    /// The client's upload of C32S2-like settings: Hanamura's entry with
    /// teams of 12 and 0, an Assault block, one Workshop item; each encoded
    /// and decoded with the lobby schemas as the session would.
    fn upload() -> Vec<(u32, Fields)> {
        let schemas = Schemas::lobby();
        let workshop = Fields::from([(
            0,
            Array(vec![Struct(Fields::from([(
                0x50,
                Value::String("rule".into()),
            )]))]),
        )]);
        let map_entry = {
            let mut entry = Fields::from([(ENTRY_MAP, Unsigned(0x0800_0000_0000_0010))]);
            entry.insert(BLOCK, Struct(block(0, &[(0, 12), (1, 0)])));
            entry
        };
        // The Assault block's masterVars: a float of 0 (key 9531), an
        // int of 12 (key 2) and a pair of an unknown kind.
        let pair = |value: u32, key: u64, kind: u64| {
            Struct(Fields::from([
                (0, Unsigned(u64::from(value))),
                (4, Unsigned(key)),
                (6, Unsigned(kind)),
            ]))
        };
        let mut assault = block(0x0230_0000_0000_0014, &[]);
        assault.insert(
            MASTER_VARS,
            Array(vec![
                pair(0.0f32.to_bits(), 9531, 3),
                pair(12, 2, 2),
                pair(1, 9539, 9),
            ]),
        );
        let messages = [
            (
                24008,
                Fields::from([
                    (
                        SETTINGS,
                        Struct(Fields::from([(WORKSHOP, Struct(workshop))])),
                    ),
                    (MORE, Bool(true)),
                ]),
            ),
            (
                24009,
                Fields::from([(0x78, Struct(map_entry)), (0x1b8, Bool(true))]),
            ),
            (
                24010,
                Fields::from([(0x78, Struct(assault)), (0x1b0, Bool(true))]),
            ),
            (
                24012,
                Fields::from([(0x1cc, Value::Signed(7)), (0x1d1, Bool(false))]),
            ),
        ];
        messages
            .into_iter()
            .map(|(id, fields)| {
                let body = schemas.encode(GAME_REQUEST, id, &fields).unwrap();
                (id, schemas.decode(GAME_REQUEST, id, &body, true).unwrap())
            })
            .collect()
    }

    #[test]
    fn an_upload_completes_with_its_last_chunk_and_reads_back() {
        let mut upload_state = Upload::default();
        let mut done = None;
        for (id, fields) in upload() {
            assert!(done.is_none(), "complete before its last chunk");
            done = upload_state.push(id, fields).unwrap();
        }
        let settings = done.expect("the last chunk completes the upload");
        assert_eq!(settings.maps(), [0x0800_0000_0000_0010]);
        assert_eq!(settings.modes(), [0x0230_0000_0000_0014]);
        assert_eq!(
            settings.team_sizes(0x0800_0000_0000_0010, 0x0230_0000_0000_0014),
            Some(vec![12, 0])
        );
        assert_eq!(settings.team_sizes(0x0800_0000_0000_0011, 0), None);
        let workshop = settings.workshop();
        assert!(!workshop.is_empty());
        assert_eq!(workshop.items.len(), 1);
        assert_eq!(
            settings.ruleset_values(0x0800_0000_0000_0010, 0x0230_0000_0000_0014),
            BTreeMap::from([
                (2, RulesetValue::Integer(12)),
                (9531, RulesetValue::Float(0.0)),
            ])
        );
        assert!(settings.ruleset_values(0, 0x0230_0000_0000_0010).is_empty());
        // The raw Workshop: the header and the one item, which decode
        // back to what came.
        let schemas = Schemas::lobby();
        let block = settings.workshop_block(&schemas).unwrap();
        assert_eq!(block.rules.len(), 1);
        let item = schemas
            .decode(GAME_REQUEST, 24012, &block.rules[0], true)
            .unwrap();
        assert_eq!(item[&0x1cc], Value::Signed(7));
        let header = schemas
            .decode(GAME_REQUEST, 24008, &block.header, true)
            .unwrap();
        assert_eq!(header[&SETTINGS], Struct(settings.settings.clone()));
        assert!(
            upload_state.push(24009, Fields::new()).is_err(),
            "a chunk after the upload ended has no header"
        );
    }

    #[test]
    fn the_answers_replay_the_upload_in_the_server_groups() {
        let mut upload_state = Upload::default();
        let settings = upload()
            .into_iter()
            .find_map(|(id, fields)| upload_state.push(id, fields).unwrap())
            .unwrap();
        let lobby = Lobby {
            id: [3, 4],
            name: "Doomfist Parkour".into(),
            host_card: Fields::new(),
            host_group: [0, 0],
        };
        let schemas = Schemas::lobby();
        let entered = lobby.enter(&settings);
        let ids: Vec<_> = entered.iter().map(|(_, id, _)| *id).collect();
        assert_eq!(ids, [23304, 23305, 23306, 23308, 53000, 53001, 53002]);
        let updated = settings.update();
        let ids: Vec<_> = updated.iter().map(|(_, id, _)| *id).collect();
        assert_eq!(ids, [23312, 23313, 23314, 23316]);
        for messages in [&entered, &updated] {
            // Every message but the last of the transfer says more follows.
            let transfer: Vec<_> = messages
                .iter()
                .filter(|(crc, _, _)| *crc == CUSTOM_GAMES)
                .collect();
            let mores: Vec<_> = transfer
                .iter()
                .map(|(_, id, fields)| {
                    let offset = match id {
                        23304 => ENTER_MORE,
                        23312 => MORE,
                        id => ChunkKind::of((id - 23304) % 8).unwrap().more(),
                    };
                    fields[&offset].clone()
                })
                .collect();
            assert_eq!(mores, [Bool(true), Bool(true), Bool(true), Bool(false)]);
            for (crc, id, fields) in messages.iter() {
                schemas.encode(*crc, *id, fields).unwrap();
            }
        }
        // The header carries no per-map entry: the client fails a transfer
        // whose header has one.
        let (_, _, header) = &entered[0];
        let Struct(sent) = &header[&ENTER_SETTINGS] else {
            panic!()
        };
        assert_eq!(sent[&MAPS], Array(Vec::new()));
        let Struct(descriptor) = &header[&ENTER_DESCRIPTOR] else {
            panic!()
        };
        assert_ne!(descriptor[&0x78], Unsigned(0));
    }

    #[test]
    fn only_kind_1_is_a_custom_game() {
        let request = |kind| {
            (
                GAME_REQUEST,
                24000,
                Fields::from([
                    (0x78, Unsigned(kind)),
                    (0x80, Value::String("Lobby".into())),
                    (0xa8, Value::Signed(4)),
                ]),
            )
        };
        assert_eq!(create_request(&request(1)).as_deref(), Some("Lobby"));
        assert_eq!(create_request(&request(2)), None);
    }
}
