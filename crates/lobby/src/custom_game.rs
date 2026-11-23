//! Custom games: Play > Custom Games (Create, the Game Browser), the lobby,
//! its settings, codes and saved settings.
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
//!   update the settings, 23320-23325 a lobby's details. The settings'
//!   per-map array (`+0x0`) must come in its own messages: a header with
//!   entries there fails the transfer (`0x7FF7895E01E0`).
//! - The lobby itself is in [`lobby`]: its descriptor, its seats (the
//!   match lobby's 53000 type 1, 53001, 53002) and the requests of its
//!   players and owner. Finding a game is [`browser`]; settings codes and
//!   saved settings are [`links`].

pub mod browser;
pub mod links;
pub mod lobby;

use std::{collections::BTreeMap, io};

use crate::{handoff::GAME_REQUEST, templates::CUSTOM_GAMES, transport::LobbyMessage};
use lucy_wire::jam::codec::{
    Fields, Value,
    Value::{Array, Bool, Struct, Unsigned},
};

/// 24000 `+0x78`: create a custom game (2 is the Practice Range, 3 the
/// Tutorial).
pub const CREATE_CUSTOM_GAME: u64 = 1;
/// 24000's name (`+0x80`).
const CREATE_NAME: u32 = 0x80;

/// One settings transfer of the custom games protocols: its header
/// message, where the header keeps the settings, the extra block and the
/// "more" bool; its chunks are the five messages after the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub protocol: u32,
    pub header: u32,
    settings: u32,
    extra: u32,
    more: u32,
}

impl Group {
    /// The group's message ids, header first.
    pub fn ids(self) -> [u32; 6] {
        std::array::from_fn(|index| self.header + index as u32)
    }
}

/// The client's settings upload to its lobby: 24008-24013.
pub const UPLOAD: Group = Group {
    protocol: GAME_REQUEST,
    header: 24008,
    settings: 0x78,
    extra: 0x2b8,
    more: 0x3d0,
};
/// The server's answers of the same shapes: 23304-23309 enter the lobby
/// (after the descriptor and colour sets), 23312-23317 update its
/// settings, 23320-23325 a lobby's details (`refs/reports/CustomGames.md`).
const ENTER_LOBBY: Group = Group {
    protocol: CUSTOM_GAMES,
    header: 23304,
    settings: 0x140,
    extra: 0x380,
    more: 0x498,
};
const SETTINGS_UPDATE: Group = Group {
    protocol: CUSTOM_GAMES,
    header: 23312,
    settings: 0x78,
    extra: 0x2b8,
    more: 0x3d0,
};
const DETAILS: Group = Group {
    protocol: CUSTOM_GAMES,
    header: 23320,
    settings: 0x198,
    extra: 0x3d8,
    more: 0x700,
};
const ENTER_DESCRIPTOR: u32 = 0x78;
const ENTER_COLOURS: u32 = 0x128;

/// The settings struct's per-map entries (`{312 block, +0x138 map}`) and
/// per-mode blocks; the Workshop block.
const MAPS: u32 = 0x0;
const MODES: u32 = 0x18;
const WORKSHOP: u32 = 0x30;
const ENTRY_MAP: u32 = 0x138;
/// The extra block's compressed Workshop text.
const EXTRA_TEXT: u32 = 0xf0;
/// A 312-byte block (`CGGameRuleset`): its 168-byte part, the mode (as
/// the game struct has it, `lucy_sim::instance`; `+0xA8` is the mode's
/// `0FD` preset), the teams, the on/off flag of a mode or a map.
const BLOCK: u32 = 0x0;
const BLOCK_MODE: u32 = 0xb0;
const BLOCK_ENABLED: u32 = 0x108;
/// A block's `{u32 value, u16 key, u8 kind}` lists: `masterVars`,
/// `globalAdminVars`.
const MASTER_VARS: u32 = 0xd8;
const ADMIN_VARS: u32 = 0xf0;
const BLOCK_TEAMS: u32 = 0xc0;
/// A team (`CGGameRulesetTeam`, 104 bytes): `bodyVars` (`{+0x0 pairs,
/// +0x18 hero}`), `allHeroBodyVars`, `controllerVars`, the hero indexes,
/// its number, size and whether the heroes are a whitelist.
const TEAM_HERO_VARS: u32 = 0x0;
const TEAM_ALL_HEROES: u32 = 0x18;
const TEAM_CONTROLLER: u32 = 0x30;
const TEAM_HEROES: u32 = 0x48;
const TEAM_NUMBER: u32 = 0x60;
const TEAM_SIZE: u32 = 0x61;
const TEAM_WHITELIST: u32 = 0x62;
const HERO_VARS_PAIRS: u32 = 0x0;
const HERO_VARS_HERO: u32 = 0x18;
/// Team numbers of the settings: 4 free for all (upstream's live-tested
/// `FREE_FOR_ALL_TEAM`), 5 every team (`0x7FF789CB1250`).
const FREE_FOR_ALL_TEAM: u64 = 4;
const EVERY_TEAM: u64 = 5;
/// The settings' link block (`CGLinkIdentity`, 152 bytes; the browser's
/// "lobby text"): the author's name for them and their code; the lobby's
/// description.
const LINK: u32 = 0x128;
const LINK_NAME: u32 = 0x48;
const LINK_CODE: u32 = 0x70;
const DESCRIPTION: u32 = 0x218;
/// The lobby options (`{i32 ×3, bool, bool}`): `+0x8` is the most
/// spectators (upstream's live-tested `+0x1D0`).
const LOBBY_OPTIONS: u32 = 0x1d0;
/// A lists chunk (24013) is the 240-byte lists (piece kind 0) or a piece
/// of the extra block's Workshop text (1).
const LISTS_PIECE: u32 = 0x168;
const LISTS_PIECE_KIND: u32 = 0x190;
const TEXT_PIECE: u64 = 1;

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
    /// The settings a new lobby starts with: Hybrid on King's Row with its
    /// preset (`0FD:2C`), teams of 6, 2 spectators (upstream's live-tested
    /// default room, `refs/tmp_upinv/custom_games.py` `RoomRules`). The
    /// client keeps its own defaults and uploads them only when they
    /// differ from these.
    pub fn default_game() -> Self {
        const KINGS_ROW: u64 = 0x0800_0000_0000_00d4;
        const HYBRID: u64 = 0x0230_0000_0000_0016;
        const HYBRID_PRESET: u64 = 0x0fd0_0000_0000_002c;
        let teams = Array(
            (0..2)
                .map(|team| {
                    Struct(Fields::from([
                        (TEAM_NUMBER, Unsigned(team)),
                        (TEAM_SIZE, Unsigned(6)),
                    ]))
                })
                .collect(),
        );
        let block = Fields::from([
            (0xa8, Unsigned(HYBRID_PRESET)),
            (BLOCK_MODE, Unsigned(HYBRID)),
            (BLOCK_TEAMS, teams),
            (BLOCK_ENABLED, Bool(true)),
        ]);
        let numbers = |numbers: [u64; 3]| {
            Struct(Fields::from([
                (0x0, Unsigned(numbers[0])),
                (0x4, Unsigned(numbers[1])),
                (0x8, Unsigned(numbers[2])),
            ]))
        };
        Self {
            settings: Fields::from([
                (LOBBY_OPTIONS, numbers([3, 1, 2])),
                (0x1e0, numbers([2, 2, 3])),
            ]),
            extra: Fields::new(),
            chunks: vec![
                Chunk {
                    kind: ChunkKind::Map,
                    fields: Fields::from([(
                        0x78,
                        Struct(Fields::from([
                            (BLOCK, Struct(block.clone())),
                            (ENTRY_MAP, Unsigned(KINGS_ROW)),
                        ])),
                    )]),
                },
                Chunk {
                    kind: ChunkKind::Mode,
                    fields: Fields::from([(0x78, Struct(block))]),
                },
            ],
        }
    }

    /// The maps of the games the settings play ([`CustomSettings::games`]).
    pub fn maps(&self) -> Vec<u64> {
        self.games().into_iter().map(|(map, _)| map).collect()
    }

    fn link_text(&self, offset: u32) -> &str {
        match self.settings.get(&LINK) {
            Some(Struct(link)) => match link.get(&offset) {
                Some(Value::String(text)) => text.trim_end_matches('\0'),
                _ => "",
            },
            _ => "",
        }
    }

    /// The settings code the settings came from or were shared under
    /// (the link block's `+0x70`); empty: none.
    pub fn code(&self) -> &str {
        self.link_text(LINK_CODE)
    }

    pub fn set_code(&mut self, code: &str) {
        let link = self
            .settings
            .entry(LINK)
            .or_insert_with(|| Struct(Fields::new()));
        if !matches!(link, Struct(_)) {
            *link = Struct(Fields::new());
        }
        if let Struct(link) = link {
            link.insert(LINK_CODE, Value::String(code.into()));
        }
    }

    /// The name the settings' author gave them (the link block's
    /// `+0x48`), which the browser lists; empty: none.
    pub fn name(&self) -> &str {
        self.link_text(LINK_NAME)
    }

    /// The lobby's description (settings `+0x218`, the browser's details
    /// pane, `0x7FF7898F0630`).
    pub fn description(&self) -> &str {
        match self.settings.get(&DESCRIPTION) {
            Some(Value::String(text)) => text.trim_end_matches('\0'),
            _ => "",
        }
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

    /// The transfer of these settings in `group`: the header (`header`
    /// with the settings, the extra block and "more"), then the chunks.
    /// The last message says no more.
    pub fn transfer(&self, group: Group, mut header: Fields) -> Vec<LobbyMessage> {
        let mut sent = self.settings.clone();
        // The per-map entries must not ride in the header
        // (0x7FF7895E01E0); the client's own header has both arrays empty.
        sent.insert(MAPS, Array(Vec::new()));
        sent.insert(MODES, Array(Vec::new()));
        header.insert(group.settings, Struct(sent));
        header.insert(group.extra, Struct(self.extra.clone()));
        header.insert(group.more, Bool(!self.chunks.is_empty()));
        let mut messages = vec![(group.protocol, group.header, header)];
        for (index, chunk) in self.chunks.iter().enumerate() {
            let mut fields = chunk.fields.clone();
            fields.insert(chunk.kind.more(), Bool(index + 1 < self.chunks.len()));
            messages.push((group.protocol, group.header + chunk.kind.index(), fields));
        }
        messages
    }

    /// 23312-23317: the server's settings of the lobby (the client adopts
    /// them as its own, system 38 slot 6 `0x7FF7895091E0`).
    pub fn update(&self) -> Vec<LobbyMessage> {
        self.transfer(SETTINGS_UPDATE, Fields::new())
    }

    /// The settings' 312-byte blocks in upload order: each map entry's
    /// (with its map) and each mode block, with the team hero entries
    /// (24011) that belong to it. The client's sender moves each team's
    /// `bodyVars` out of the block into a chunk after the block's own,
    /// with the block's index in its list (`+0x98`) and the team's index
    /// in the block (`+0x9C`) (`0x7FF7895E42D0`, loops at 0x7FF7895E48A6
    /// and 0x7FF7895E4CD6).
    fn blocks(&self) -> Vec<Block<'_>> {
        let mut blocks: Vec<Block<'_>> = Vec::new();
        let mut lists: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
        let mut last: Option<usize> = None;
        for chunk in &self.chunks {
            let Some(Struct(value)) = chunk.fields.get(&0x78) else {
                continue;
            };
            let (list, map, block) = match chunk.kind {
                ChunkKind::Map => match value.get(&BLOCK) {
                    Some(Struct(block)) => (0, value.get(&ENTRY_MAP).and_then(unsigned), block),
                    _ => continue,
                },
                ChunkKind::Mode => (1, None, value),
                ChunkKind::Team => {
                    let index = |offset| {
                        chunk
                            .fields
                            .get(&offset)
                            .and_then(unsigned)
                            .and_then(|index| usize::try_from(index).ok())
                    };
                    if let (Some(list), Some(entry), Some(team)) = (last, index(0x98), index(0x9c))
                        && let Some(&block) = lists[list].get(entry)
                    {
                        let block: &mut Block<'_> = &mut blocks[block];
                        block.hero_vars.push((team, value));
                    }
                    continue;
                }
                ChunkKind::Workshop | ChunkKind::Lists => continue,
            };
            last = Some(list);
            lists[list].push(blocks.len());
            blocks.push(Block {
                map,
                block,
                hero_vars: Vec::new(),
            });
        }
        blocks
    }

    /// The games the settings play, in the order the client lists them:
    /// each enabled map entry (`+0x108`) whose mode's block is enabled too
    /// (a mode without a block counts as on), with that mode (`+0xB0`).
    /// (Upstream's live-tested reading, `refs/tmp_upinv/custom_games.py`
    /// `enabled_maps`.)
    pub fn games(&self) -> Vec<(u64, u64)> {
        let blocks = self.blocks();
        let enabled = |block: &Fields| block.get(&BLOCK_ENABLED).is_none_or(truthy);
        let mode_on = |mode: u64| {
            blocks
                .iter()
                .filter(|block| block.map.is_none())
                .find(|block| block.block.get(&BLOCK_MODE).and_then(unsigned) == Some(mode))
                .is_none_or(|block| enabled(block.block))
        };
        blocks
            .iter()
            .filter_map(|block| {
                let map = block.map?;
                let mode = block.block.get(&BLOCK_MODE).and_then(unsigned)?;
                (enabled(block.block) && mode_on(mode)).then_some((map, mode))
            })
            .collect()
    }

    /// The enabled modes the mode blocks name (`+0xB0`), in upload order.
    pub fn modes(&self) -> Vec<u64> {
        self.blocks()
            .iter()
            .filter(|block| block.map.is_none())
            .filter(|block| block.block.get(&BLOCK_ENABLED).is_none_or(truthy))
            .filter_map(|block| block.block.get(&BLOCK_MODE).and_then(unsigned))
            .filter(|&mode| mode != 0)
            .collect()
    }

    /// The 312-byte block that describes a game of `mode` on `map`: the
    /// map entry's, else the mode block's (53001 `+0x0`; the lobby screen
    /// reads its teams' sizes, `0x7FF789A94BB0`).
    pub fn block(&self, map: u64, mode: u64) -> Option<Fields> {
        let blocks = self.blocks();
        let found = blocks
            .iter()
            .find(|block| block.map == Some(map))
            .or_else(|| {
                blocks.iter().find(|block| {
                    block.map.is_none()
                        && block.block.get(&BLOCK_MODE).and_then(unsigned) == Some(mode)
                })
            })?;
        Some(found.block.clone())
    }

    /// The most spectators (settings `+0x1D0` `+0x8`).
    pub fn max_spectators(&self) -> u8 {
        match self.settings.get(&LOBBY_OPTIONS) {
            Some(Struct(options)) => options
                .get(&0x8)
                .and_then(unsigned)
                .map_or(0, |count| u8::try_from(count).unwrap_or(u8::MAX)),
            _ => 0,
        }
    }

    /// The lobby options the match lobby's 53001 `+0x260` repeats
    /// (settings `+0x1D0`: its `+0x8` is the spectators).
    pub fn lobby_options(&self) -> Fields {
        match self.settings.get(&LOBBY_OPTIONS) {
            Some(Struct(options)) => options.clone(),
            _ => Fields::new(),
        }
    }

    /// What a match of `mode` on `map` plays with: the blocks of `mode`
    /// (mode blocks, then the map entries of `map` over them; a later
    /// value, team size or hero list replaces an earlier one).
    ///
    /// - Its ruleset values: the `{u32 value, u16 key, u8 kind}` pairs of
    ///   `masterVars` `+0xD8`, then `globalAdminVars` `+0xF0`.
    /// - Each team (`CGGameRulesetTeam`, the block's `+0xC0`): its size
    ///   (`+0x61`), its hero list (`+0x48` hero indexes, `+0x62` a
    ///   whitelist, else every hero but those), its values for every body
    ///   of the team (`allHeroBodyVars` `+0x18`, `controllerVars` `+0x30`)
    ///   and for one hero's bodies (`bodyVars` `+0x0`, or the 24011
    ///   chunks). Team number 5 is every team, as the client's hero check
    ///   reads it (`0x7FF789CB1250`: the team's own entry, else team 5's).
    ///
    /// The key is the `01C` ruleset key and the kind byte the value's type,
    /// as the client's settings check reads them (`0x7FF789353FB0`: 0 a
    /// bool, 1 an int, 2 float bits, 3 a `01C` choice of an enum entry;
    /// game check 2026-11-23). Other kinds are left out.
    pub fn rules(&self, map: u64, mode: u64) -> MatchRules {
        let blocks = self.blocks();
        let modes = blocks.iter().filter(|block| {
            block.map.is_none() && block.block.get(&BLOCK_MODE).and_then(unsigned) == Some(mode)
        });
        let maps = blocks.iter().filter(|block| block.map == Some(map));
        let mut rules = MatchRules::default();
        let mut sizes = BTreeMap::new();
        // By scope: every team (None), a team; then each hero's.
        let mut teams: BTreeMap<Option<u8>, BTreeMap<u32, RulesetValue>> = BTreeMap::new();
        let mut heroes: BTreeMap<(Option<u8>, u64), BTreeMap<u32, RulesetValue>> = BTreeMap::new();
        for block in modes.chain(maps) {
            for list in [MASTER_VARS, ADMIN_VARS] {
                rules.values.extend(pairs(block.block.get(&list)));
            }
            let Some(Array(entries)) = block.block.get(&BLOCK_TEAMS) else {
                continue;
            };
            for (index, team) in entries.iter().enumerate() {
                let Struct(team) = team else { continue };
                let Some(number) = team.get(&TEAM_NUMBER).and_then(unsigned) else {
                    continue;
                };
                let scope = match number {
                    EVERY_TEAM => None,
                    0 | 1 => Some(number as u8),
                    // Free for all: the room's one team.
                    FREE_FOR_ALL_TEAM => {
                        rules.free_for_all = true;
                        Some(0)
                    }
                    _ => continue,
                };
                if let (Some(team_index), Some(size)) =
                    (scope, team.get(&TEAM_SIZE).and_then(unsigned))
                {
                    sizes.insert(team_index, u8::try_from(size).unwrap_or(u8::MAX));
                }
                if let Some(Array(list)) = team.get(&TEAM_HEROES) {
                    rules.hero_lists.insert(
                        scope,
                        HeroList {
                            heroes: list.iter().filter_map(unsigned).map(hero_guid).collect(),
                            whitelist: team.get(&TEAM_WHITELIST).is_some_and(truthy),
                        },
                    );
                }
                let values = teams.entry(scope).or_default();
                values.extend(pairs(team.get(&TEAM_ALL_HEROES)));
                values.extend(pairs(team.get(&TEAM_CONTROLLER)));
                let inline = match team.get(&TEAM_HERO_VARS) {
                    Some(Array(entries)) => entries.iter().collect(),
                    _ => Vec::new(),
                };
                let moved = block
                    .hero_vars
                    .iter()
                    .filter(|&&(of, _)| of == index)
                    .map(|&(_, entry)| entry);
                for entry in inline
                    .into_iter()
                    .filter_map(|entry| match entry {
                        Struct(entry) => Some(entry),
                        _ => None,
                    })
                    .chain(moved)
                {
                    if let Some(hero) = entry.get(&HERO_VARS_HERO).and_then(unsigned) {
                        heroes
                            .entry((scope, hero_guid(hero)))
                            .or_default()
                            .extend(pairs(entry.get(&HERO_VARS_PAIRS)));
                    }
                }
            }
        }
        if let Some(&last) = sizes.keys().next_back() {
            let mut team_sizes = vec![0; usize::from(last) + 1];
            for (team, size) in sizes {
                team_sizes[usize::from(team)] = size;
            }
            rules.team_sizes = Some(team_sizes);
        }
        // Every team's before a team's, every hero's before a hero's: the
        // narrower scope wins (`GraphStore::body_variables` applies them
        // in order).
        let teams = teams
            .into_iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(team, values)| BodyRules {
                team,
                hero: None,
                values,
            });
        let heroes = heroes
            .into_iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|((team, hero), values)| BodyRules {
                team,
                hero: Some(hero),
                values,
            });
        rules.bodies = teams.chain(heroes).collect();
        rules
            .bodies
            .sort_by_key(|body| (body.hero.is_some(), body.team.is_some()));
        rules
    }

    /// The Workshop text the client wrote into the extra block (`+0xF0`):
    /// the whole settings and rules as Overwatch copies them, compressed
    /// with Zstandard (zstd 1.1.0, level 3, `0x7FF78936DD80`) and base64
    /// (standard alphabet, `=` padding) (`0x7FF789A94550`; CVar
    /// `0x1D407F01` on). An upload sends it in pieces of at most 64 KiB,
    /// each a lists chunk (24013) with piece kind `+0x190` 1 and the piece
    /// at `+0x168` (`0x7FF7895E42D0` 0x7FF7895E62F4); a header may carry
    /// it whole. None: the settings have no text.
    pub fn workshop_text(&self) -> io::Result<Option<String>> {
        use base64::Engine as _;
        use std::io::Read as _;

        let mut text = match self.extra.get(&EXTRA_TEXT) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        for chunk in &self.chunks {
            if chunk.kind == ChunkKind::Lists
                && chunk.fields.get(&LISTS_PIECE_KIND).and_then(unsigned) == Some(TEXT_PIECE)
                && let Some(Value::String(piece)) = chunk.fields.get(&LISTS_PIECE)
            {
                text.push_str(piece);
            }
        }
        let text = text.trim_end_matches('\0');
        if text.is_empty() {
            return Ok(None);
        }
        let compressed = base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|error| invalid(format!("the Workshop text's base64: {error}")))?;
        let mut source = compressed.as_slice();
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&mut source)
            .map_err(|error| invalid(format!("the Workshop text's zstd frame: {error}")))?;
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded)?;
        String::from_utf8(decoded)
            .map(Some)
            .map_err(|error| invalid(format!("the Workshop text: {error}")))
    }
}

/// A 312-byte block of the settings and what came for it in other chunks.
struct Block<'a> {
    /// A map entry's map; None: a mode block.
    map: Option<u64>,
    block: &'a Fields,
    /// The 24011 entries: the team's index in the block, the entry
    /// (`{+0x0 pairs, +0x18 hero}`).
    hero_vars: Vec<(usize, &'a Fields)>,
}

/// What the settings give a match of one mode on one map
/// ([`CustomSettings::rules`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MatchRules {
    pub values: BTreeMap<u32, RulesetValue>,
    /// Values for some bodies only, every team's and every hero's first.
    pub bodies: Vec<BodyRules>,
    /// By team number, when a block names any.
    pub team_sizes: Option<Vec<u8>>,
    /// The hero lists by team (None: every team).
    pub hero_lists: BTreeMap<Option<u8>, HeroList>,
    /// A team of number 4: everyone plays for himself (the lobby's FFA
    /// column, `0x7FF789A95EE0`).
    pub free_for_all: bool,
}

/// Values of a team's bodies, or of one hero's bodies (in a team).
#[derive(Clone, Debug, PartialEq)]
pub struct BodyRules {
    /// The room's team (0 is Team 1); None: every team.
    pub team: Option<u8>,
    /// A hero GUID; None: every hero.
    pub hero: Option<u64>,
    pub values: BTreeMap<u32, RulesetValue>,
}

/// A team's heroes (`CGGameRulesetTeam` `+0x48`, `+0x62`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeroList {
    /// Hero GUIDs.
    pub heroes: Vec<u64>,
    /// Only these heroes; else every hero but these.
    pub whitelist: bool,
}

impl HeroList {
    pub fn allows(&self, hero: u64) -> bool {
        self.heroes.contains(&hero) == self.whitelist
    }
}

/// A team's hero index as its `075` hero GUID (Doomfist's `075:12F` is
/// `0x02E000000000012F`).
fn hero_guid(index: u64) -> u64 {
    HERO_GUID | (index & 0xffff)
}

const HERO_GUID: u64 = 0x02e0_0000_0000_0000;

fn pairs(list: Option<&Value>) -> impl Iterator<Item = (u32, RulesetValue)> + '_ {
    let pairs = match list {
        Some(Array(pairs)) => pairs.as_slice(),
        _ => &[],
    };
    pairs.iter().filter_map(|pair| match pair {
        Struct(pair) => ruleset_pair(pair),
        _ => None,
    })
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

/// A pair's kind byte (`0x7FF789353FB0`: the entry's value type it must
/// match, `STU_8A8AA0A4` bool, `STU_118786E9` int, `STU_A499C365` float,
/// `STU_776E5ADD` enum).
pub const BOOL_KIND: u64 = 0;
pub const INTEGER_KIND: u64 = 1;
pub const FLOAT_KIND: u64 = 2;
pub const CHOICE_KIND: u64 = 3;

fn ruleset_pair(pair: &Fields) -> Option<(u32, RulesetValue)> {
    let value = u32::try_from(pair.get(&0).and_then(unsigned)?).ok()?;
    let key = u32::try_from(pair.get(&4).and_then(unsigned)?).ok()?;
    let value = match pair.get(&6).and_then(unsigned)? {
        BOOL_KIND => RulesetValue::Bool(value != 0),
        INTEGER_KIND => RulesetValue::Integer(i64::from(value as i32)),
        FLOAT_KIND => RulesetValue::Float(f32::from_bits(value)),
        CHOICE_KIND => RulesetValue::Asset(IDENTIFIER_GUID | u64::from(value)),
        _ => return None,
    };
    Some((key, value))
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

/// The most chunks one upload may have: more than the client's own limits
/// (400 map entries, 100 mode blocks, `0x7FF7895E01E0`; the Workshop's
/// items), so only a broken or hostile sender reaches it.
const MOST_CHUNKS: usize = 8192;

/// Reassembles a settings upload of one group (24008-24013, 40802-40807).
#[derive(Debug)]
pub struct Upload {
    group: Group,
    pending: Option<Uploaded>,
}

/// A complete upload: the header's other fields and the settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Uploaded {
    pub header: Fields,
    pub settings: CustomSettings,
}

impl Upload {
    pub fn new(group: Group) -> Self {
        Self {
            group,
            pending: None,
        }
    }

    /// Take one upload message; the upload when it was the last one. A
    /// header starts a new upload; a chunk without one is an error.
    pub fn push(&mut self, id: u32, mut fields: Fields) -> io::Result<Option<Uploaded>> {
        let more = |fields: &mut Fields, offset| matches!(fields.remove(&offset), Some(value) if truthy(&value));
        let index = id
            .checked_sub(self.group.header)
            .filter(|&index| index < 6)
            .ok_or_else(|| invalid(format!("{id} is not a settings upload message")))?;
        let follows = if index == 0 {
            let follows = more(&mut fields, self.group.more);
            let struct_at = |fields: &mut Fields, offset| match fields.remove(&offset) {
                Some(Struct(value)) => value,
                _ => Fields::new(),
            };
            let settings = CustomSettings {
                settings: struct_at(&mut fields, self.group.settings),
                extra: struct_at(&mut fields, self.group.extra),
                chunks: Vec::new(),
            };
            self.pending = Some(Uploaded {
                header: fields,
                settings,
            });
            follows
        } else {
            let kind = ChunkKind::of(index).expect("chunk index");
            let follows = more(&mut fields, kind.more());
            let header = self.group.header;
            let pending = self
                .pending
                .as_mut()
                .ok_or_else(|| invalid(format!("{id} came without a {header} before it")))?;
            if pending.settings.chunks.len() >= MOST_CHUNKS {
                self.pending = None;
                return Err(invalid(format!(
                    "an upload of more than {MOST_CHUNKS} chunks"
                )));
            }
            pending.settings.chunks.push(Chunk { kind, fields });
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

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Schemas;

    const HANAMURA: u64 = 0x0800_0000_0000_0165;
    const ASSAULT: u64 = 0x0230_0000_0000_0014;
    const DOOMFIST: u64 = 0x02e0_0000_0000_012f;
    const MERCY: u64 = 0x02e0_0000_0000_0004;

    fn pair(value: u32, key: u64, kind: u64) -> Value {
        Struct(Fields::from([
            (0, Unsigned(u64::from(value))),
            (4, Unsigned(key)),
            (6, Unsigned(kind)),
        ]))
    }

    fn team(number: u64, size: u64) -> Fields {
        Fields::from([(TEAM_NUMBER, Unsigned(number)), (TEAM_SIZE, Unsigned(size))])
    }

    /// The client's upload of C32S2-like settings, each message encoded
    /// and decoded with the lobby schemas as the session would:
    /// - Hanamura's entry (Assault, on): Team 1 of 12 with Doomfist alone
    ///   (a whitelist), Team 2 of 0 whose bodies get 50% damage, every
    ///   team without Mercy; Team 1's Doomfist gets his own value in a
    ///   24011 chunk, as the client's sender moves it;
    /// - an Assault block (on) with a float of 0 (key 9531), an int of 12
    ///   (key 2) and a pair of an unknown kind;
    /// - one Workshop item, then the compressed Workshop text in two
    ///   pieces (24013 piece kind 1).
    fn upload(text: &str) -> Vec<(u32, Fields)> {
        use base64::Engine as _;

        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        let workshop = Fields::from([(
            0,
            Array(vec![Struct(Fields::from([(
                0x50,
                Value::String("rule".into()),
            )]))]),
        )]);
        let mut first = team(0, 12);
        first.insert(TEAM_HEROES, Array(vec![Unsigned(DOOMFIST & 0xffff)]));
        first.insert(TEAM_WHITELIST, Bool(true));
        let mut second = team(1, 0);
        second.insert(
            TEAM_ALL_HEROES,
            Array(vec![pair(0.5f32.to_bits(), 9529, FLOAT_KIND)]),
        );
        let mut every = team(EVERY_TEAM, 0);
        every.insert(TEAM_HEROES, Array(vec![Unsigned(MERCY & 0xffff)]));
        let map_block = Fields::from([
            (BLOCK_MODE, Unsigned(ASSAULT)),
            (BLOCK_ENABLED, Bool(true)),
            (
                BLOCK_TEAMS,
                Array(vec![Struct(first), Struct(second), Struct(every)]),
            ),
        ]);
        let map_entry = Fields::from([(BLOCK, Struct(map_block)), (ENTRY_MAP, Unsigned(HANAMURA))]);
        let assault = Fields::from([
            (BLOCK_MODE, Unsigned(ASSAULT)),
            (BLOCK_ENABLED, Bool(true)),
            (
                MASTER_VARS,
                Array(vec![
                    pair(0.0f32.to_bits(), 9531, FLOAT_KIND),
                    pair(12, 2, INTEGER_KIND),
                    pair(1, 9539, 9),
                ]),
            ),
        ]);
        let compressed = ruzstd::encoding::compress_to_vec(
            text.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(compressed);
        let (head, tail) = encoded.split_at(encoded.len() / 2);
        let piece = |piece: &str, more| {
            Fields::from([
                (LISTS_PIECE, Value::String(piece.into())),
                (LISTS_PIECE_KIND, Unsigned(TEXT_PIECE)),
                (0x191, Bool(more)),
            ])
        };
        let messages = [
            (
                24008,
                Fields::from([
                    (
                        UPLOAD.settings,
                        Struct(Fields::from([(WORKSHOP, Struct(workshop))])),
                    ),
                    (UPLOAD.more, Bool(true)),
                ]),
            ),
            (
                24009,
                Fields::from([(0x78, Struct(map_entry)), (0x1b8, Bool(true))]),
            ),
            (
                24011,
                Fields::from([
                    (
                        0x78,
                        Struct(Fields::from([
                            (
                                HERO_VARS_PAIRS,
                                Array(vec![pair(0.25f32.to_bits(), 9744, FLOAT_KIND)]),
                            ),
                            (HERO_VARS_HERO, Unsigned(DOOMFIST & 0xffff)),
                        ])),
                    ),
                    (0x98, Unsigned(0)),
                    (0x9c, Unsigned(0)),
                    (0xa0, Bool(true)),
                ]),
            ),
            (
                24010,
                Fields::from([(0x78, Struct(assault)), (0x1b0, Bool(true))]),
            ),
            (
                24012,
                Fields::from([(0x1cc, Value::Signed(7)), (0x1d1, Bool(true))]),
            ),
            (24013, piece(head, true)),
            (24013, piece(tail, false)),
        ];
        messages
            .into_iter()
            .map(|(id, fields)| {
                let body = schemas.encode(GAME_REQUEST, id, &fields).unwrap();
                (id, schemas.decode(GAME_REQUEST, id, &body, true).unwrap())
            })
            .collect()
    }

    fn uploaded(text: &str) -> CustomSettings {
        let mut upload = Upload::new(UPLOAD);
        let mut done = None;
        for (id, fields) in self::upload(text) {
            assert!(done.is_none(), "complete before its last chunk");
            done = upload.push(id, fields).unwrap();
        }
        done.expect("the last chunk completes the upload").settings
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn an_upload_completes_with_its_last_chunk_and_reads_back() {
        const TEXT: &str = "settings\n{\n\tmain\n\t{\n\t\tDescription: \"Parkour\"\n\t}\n}\n";
        let settings = uploaded(TEXT);
        assert_eq!(settings.games(), [(HANAMURA, ASSAULT)]);
        assert_eq!(settings.modes(), [ASSAULT]);
        assert_eq!(settings.workshop().items.len(), 1);
        assert_eq!(settings.workshop_text().unwrap().as_deref(), Some(TEXT));
        let mut upload = Upload::new(UPLOAD);
        assert!(
            upload.push(24009, Fields::new()).is_err(),
            "a chunk without a header"
        );
    }

    /// Every block of the game's mode and map: the mode's values with the
    /// map entry's teams; Team 1's Doomfist list and his own value, Team
    /// 2's values, every team's list.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn a_game_plays_its_blocks_values_teams_and_heroes() {
        let settings = uploaded("");
        let rules = settings.rules(HANAMURA, ASSAULT);
        assert_eq!(
            rules.values,
            BTreeMap::from([
                (2, RulesetValue::Integer(12)),
                (9531, RulesetValue::Float(0.0)),
            ])
        );
        assert_eq!(rules.team_sizes, Some(vec![12, 0]));
        assert!(!rules.free_for_all);
        assert_eq!(
            rules.bodies,
            [
                BodyRules {
                    team: Some(1),
                    hero: None,
                    values: BTreeMap::from([(9529, RulesetValue::Float(0.5))]),
                },
                BodyRules {
                    team: Some(0),
                    hero: Some(DOOMFIST),
                    values: BTreeMap::from([(9744, RulesetValue::Float(0.25))]),
                },
            ]
        );
        let team_1 = &rules.hero_lists[&Some(0)];
        assert!(team_1.allows(DOOMFIST) && !team_1.allows(MERCY));
        let every = &rules.hero_lists[&None];
        assert!(every.allows(DOOMFIST) && !every.allows(MERCY));
        // Another map: the mode's values only.
        let elsewhere = settings.rules(0x0800_0000_0000_0010, ASSAULT);
        assert_eq!(elsewhere.values, rules.values);
        assert_eq!(elsewhere.team_sizes, None);
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_answers_replay_the_upload_in_the_server_groups() {
        let settings = uploaded("x");
        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        let updated = settings.update();
        let ids: Vec<_> = updated.iter().map(|(_, id, _)| *id).collect();
        assert_eq!(ids, [23312, 23313, 23315, 23314, 23316, 23317, 23317]);
        // Every message but the last of the transfer says more follows.
        let mores: Vec<_> = updated
            .iter()
            .map(|(_, id, fields)| {
                let offset = match id {
                    23312 => SETTINGS_UPDATE.more,
                    id => ChunkKind::of(id - 23312).unwrap().more(),
                };
                fields[&offset].clone()
            })
            .collect();
        assert_eq!(mores, [true, true, true, true, true, true, false].map(Bool));
        for (crc, id, fields) in &updated {
            schemas.encode(*crc, *id, fields).unwrap();
        }
        // The header carries no per-map entry: the client fails a transfer
        // whose header has one.
        let Struct(sent) = &updated[0].2[&SETTINGS_UPDATE.settings] else {
            panic!()
        };
        assert_eq!(sent[&MAPS], Array(Vec::new()));
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
