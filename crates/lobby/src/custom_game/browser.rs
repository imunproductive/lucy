//! Play > Game Browser: the custom games a player can find, their details
//! and the lists of saved settings. Protocols `0x783EE77A` (40700-40710,
//! client → server) and `0x262EF305` (40600-40605, server → client);
//! research in `refs/reports/CustomGames.md` §11.
//!
//! - BROWSE sends 40703 with a filter (`0x7FF7895E7240`) and spins until a
//!   40603 with `+0x85` other than 3 comes (`0x7FF7895E6BD4`); the client
//!   shows the server's results as they are, so the server filters. A
//!   search by code sends 40703 with request kind 3 and the code as a
//!   sub-filter's key; its answer has `+0x85` = 3.
//! - Selecting a game sends 24020 {lobby id, second id}; the answer 23320
//!   (`CustomSettings` transfer 23320-23325) fills the details pane, but
//!   only when it names the lobby asked for (`0x7FF7895E1DA0`).
//! - JOIN sends 52900 (the match lobby protocol), answered as a create is.
//! - 40700-40702 refresh three lists of saved settings (40600-40602);
//!   40704 marks one as a favourite (40604 {0} is done).

use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Signed, Struct, Unsigned},
    id16,
};

use super::unsigned;
use crate::transport::LobbyMessage;

/// Client → server.
pub const BROWSER_IN: u32 = 0x783e_e77a;
/// Server → client.
pub const BROWSER: u32 = 0x262e_f305;

pub const SEARCH: u32 = 40703;
pub const FAVOURITE: u32 = 40704;
/// The list refreshes and their answers: 40702 list 0 → 40602, 40701
/// list 1 → 40601, 40700 list 2 → 40600 (`0x7FF7895E7300`).
pub const LIST_REQUESTS: [u32; 3] = [40700, 40701, 40702];
const RESULTS: u32 = 40603;
const FAVOURITE_DONE: u32 = 40604;
/// 40603 `+0x85`: the answer to a search by code.
const CODE_LOOKUP: i64 = 3;

/// The map header of a map: `0x0790…` with the map's index, as the
/// browser's map filter and its rows name maps (upstream's live-tested
/// lobby preview, `refs/tmp_upinv/custom_games.py` `MAP_HEADER_TYPE`).
pub fn map_header(map: u64) -> u64 {
    if map == 0 {
        0
    } else {
        MAP_HEADER | (map & 0xffff_ffff)
    }
}

const MAP_HEADER: u64 = 0x0790_0000_0000_0000;

/// What the browser shows of one custom game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub lobby: [u64; 2],
    /// The settings' link block (`CGLinkIdentity`): their name and code.
    pub link: Fields,
    /// The game its settings play first: a map and mode GUID.
    pub map: u64,
    pub mode: u64,
    /// The heroes anyone may pick; None: every hero.
    pub heroes: Option<Vec<u64>>,
    pub players: u8,
    pub spectators: u8,
    pub max_spectators: u8,
    /// Team 1's and Team 2's places, or a free for all's.
    pub team_max: [u8; 2],
    pub free_for_all_max: u8,
    pub name: String,
    /// The host's card (`player_messages::record`).
    pub host: Fields,
    /// The settings' code ([`super::CustomSettings::code`]).
    pub code: String,
}

impl Listing {
    fn max_players(&self) -> u32 {
        u32::from(self.team_max[0]) + u32::from(self.team_max[1]) + u32::from(self.free_for_all_max)
    }

    /// A 40603 entry (504 bytes): the link block, the lobby id (JOIN and
    /// the details request send it back), the map header, the counts, the
    /// name and the host's card twice (`0x7FF7898E8F40`).
    pub fn entry(&self) -> Value {
        let count = |value: u8| Unsigned(u64::from(value));
        let game = Fields::from([
            (0x0, Struct(self.link.clone())),
            (
                0x98,
                Struct(Fields::from([(
                    0,
                    Array(vec![Unsigned(self.lobby[0]), Unsigned(self.lobby[1])]),
                )])),
            ),
            (0xa8, Struct(id16(self.lobby[0], self.lobby[1]))),
            (0xb8, Unsigned(map_header(self.map))),
            (0xc0, Unsigned(0)),
            (0xc8, count(self.players)),
            (0xc9, count(self.spectators)),
            (0xca, count(self.max_spectators)),
            (0xcb, count(self.team_max[0])),
            (0xcc, count(self.team_max[1])),
            (0xcd, count(self.free_for_all_max)),
            (0xce, Signed(0)),
            // Not restricted (hidden unless the player's settings allow
            // it), joinable.
            (0xcf, Bool(false)),
            (0xd0, Bool(true)),
            (0xd8, Value::String(String::new())),
            (0x100, Value::String(self.name.clone())),
        ]);
        Struct(Fields::from([
            (0x0, Struct(game)),
            (0x128, Struct(self.host.clone())),
            (0x190, Struct(self.host.clone())),
        ]))
    }
}

/// A 40703 filter (`refs/reports/CustomGames.md` §11.1).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    modes: Vec<u64>,
    /// Map headers.
    maps: Vec<u64>,
    heroes: Vec<u64>,
    party: u8,
    player_places: bool,
    spectator_places: bool,
    /// The `+0xCF` flag: 0 any, 1 only games without it, 2 only with it.
    restricted: u8,
    /// Any of these, when there are some.
    any_of: Vec<SubFilter>,
    /// A search by code (request kind 3): the code.
    code: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct SubFilter {
    maps: Vec<u64>,
    modes: Vec<u64>,
    heroes: Vec<u64>,
    sizes: Vec<(u8, u8)>,
    text: String,
    key: String,
}

fn numbers(fields: &Fields, offset: u32) -> Vec<u64> {
    match fields.get(&offset) {
        Some(Array(values)) => values.iter().filter_map(unsigned).collect(),
        _ => Vec::new(),
    }
}

fn byte(fields: &Fields, offset: u32) -> u8 {
    fields
        .get(&offset)
        .and_then(unsigned)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(0)
}

fn text(fields: &Fields, offset: u32) -> String {
    match fields.get(&offset) {
        Some(Value::String(text)) => text.trim_end_matches('\0').to_owned(),
        _ => String::new(),
    }
}

impl Filter {
    /// A 40703's filter.
    pub fn of(request: &Fields) -> Self {
        let Some(Struct(fields)) = request.get(&0x78) else {
            return Self::default();
        };
        let any_of: Vec<SubFilter> = match fields.get(&0x0) {
            Some(Array(entries)) => entries
                .iter()
                .filter_map(|entry| match entry {
                    Struct(entry) => Some(SubFilter {
                        maps: numbers(entry, 0x0),
                        modes: numbers(entry, 0x18),
                        heroes: numbers(entry, 0x30),
                        sizes: match entry.get(&0x48) {
                            Some(Array(pairs)) => pairs
                                .iter()
                                .filter_map(|pair| match pair {
                                    Struct(pair) => Some((byte(pair, 0), byte(pair, 1))),
                                    _ => None,
                                })
                                .collect(),
                            _ => Vec::new(),
                        },
                        text: text(entry, 0x78),
                        key: text(entry, 0xa0),
                    }),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let code = (fields.get(&0x6e).and_then(unsigned) == Some(CODE_LOOKUP as u64)).then(|| {
            any_of
                .first()
                .map(|entry| entry.key.clone())
                .unwrap_or_default()
        });
        let flags = match fields.get(&0x68) {
            Some(Struct(flags)) => flags.clone(),
            _ => Fields::new(),
        };
        Self {
            modes: numbers(fields, 0x18),
            maps: numbers(fields, 0x30),
            heroes: numbers(fields, 0x48),
            party: byte(&flags, 0),
            player_places: byte(&flags, 1) != 0,
            spectator_places: byte(&flags, 2) != 0,
            restricted: byte(fields, 0x6c),
            any_of,
            code,
        }
    }

    pub fn is_code_lookup(&self) -> bool {
        self.code.is_some()
    }

    /// Whether `game` passes, as the client's own matcher reads the same
    /// filter for friends' games (`0x7FF7893471D0`, `0x7FF789347540`).
    pub fn passes(&self, game: &Listing) -> bool {
        if let Some(code) = &self.code {
            return !code.is_empty() && super::links::normalise(code) == game.code;
        }
        let allows = |heroes: &[u64]| {
            heroes.is_empty()
                || heroes.iter().any(|hero| {
                    game.heroes
                        .as_ref()
                        .is_none_or(|allowed| allowed.contains(hero))
                })
        };
        let header = map_header(game.map);
        let party = u32::from(self.party.max(1));
        (self.modes.is_empty() || self.modes.contains(&game.mode))
            && (self.maps.is_empty() || self.maps.contains(&header))
            && allows(&self.heroes)
            && self.restricted != 2
            && (!self.player_places || party + u32::from(game.players) <= game.max_players())
            && (!self.spectator_places
                || party + u32::from(game.spectators) <= u32::from(game.max_spectators))
            && (self.any_of.is_empty()
                || self.any_of.iter().any(|entry| {
                    entry.maps.contains(&header)
                        || entry.modes.contains(&game.mode)
                        || (!entry.heroes.is_empty() && allows(&entry.heroes))
                        || entry.sizes.iter().any(|&(low, high)| {
                            game.team_max
                                .iter()
                                .any(|&size| (low..=high.max(low)).contains(&size))
                        })
                        || (!entry.text.is_empty()
                            && game
                                .name
                                .to_lowercase()
                                .contains(&entry.text.to_lowercase()))
                        || (!entry.key.is_empty()
                            && super::links::normalise(&entry.key) == game.code)
                }))
    }
}

/// 40603: the games that pass a search, or a search by code's.
pub fn results(filter: &Filter, games: &[Listing]) -> LobbyMessage {
    let entries = games
        .iter()
        .filter(|game| filter.passes(game))
        .map(Listing::entry)
        .collect();
    (
        BROWSER,
        RESULTS,
        Fields::from([
            (0x78, Unsigned(0)),
            (0x80, Unsigned(0)),
            (0x84, Bool(false)),
            (
                0x85,
                Signed(if filter.is_code_lookup() {
                    CODE_LOOKUP
                } else {
                    0
                }),
            ),
            (0x88, Array(entries)),
        ]),
    )
}

/// The answer to a list refresh (40700-40702): the list, empty. The
/// server keeps no public lists of saved settings.
pub fn empty_list(request: u32) -> Option<LobbyMessage> {
    let answer = match request {
        40700 => 40600,
        40701 => 40601,
        40702 => 40602,
        _ => return None,
    };
    Some((
        BROWSER,
        answer,
        Fields::from([
            (0x78, Unsigned(0)),
            (0x80, Unsigned(0)),
            (0x84, Bool(false)),
            (0x88, Array(Vec::new())),
        ]),
    ))
}

/// 40604 {0}: the favourite mark is done.
pub fn favourite_done() -> LobbyMessage {
    (BROWSER, FAVOURITE_DONE, Fields::from([(0x78, Unsigned(0))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Schemas;

    const HANAMURA: u64 = 0x0800_0000_0000_0165;
    const ASSAULT: u64 = 0x0230_0000_0000_0014;

    fn game() -> Listing {
        Listing {
            lobby: [5, 6],
            map: HANAMURA,
            mode: ASSAULT,
            heroes: Some(vec![0x02e0_0000_0000_012f]),
            players: 3,
            max_spectators: 2,
            team_max: [6, 0],
            name: "Doomfist Parkour".into(),
            code: "C32S2".into(),
            ..Listing::default()
        }
    }

    fn search(filter: Fields) -> Fields {
        Fields::from([(0x78, Struct(filter))])
    }

    #[test]
    fn the_server_filters_a_browse_as_the_client_reads_its_filter() {
        let everything = Filter::of(&search(Fields::new()));
        assert!(everything.passes(&game()));
        let elsewhere = Filter::of(&search(Fields::from([(
            0x30,
            Array(vec![Unsigned(map_header(0x0800_0000_0000_0010))]),
        )])));
        assert!(!elsewhere.passes(&game()));
        let here = Filter::of(&search(Fields::from([(
            0x30,
            Array(vec![Unsigned(map_header(HANAMURA))]),
        )])));
        assert!(here.passes(&game()));
        // A party of four needs four free places: 3 + 4 > 6.
        let places = |party| {
            Filter::of(&search(Fields::from([(
                0x68,
                Struct(Fields::from([(0, Unsigned(party)), (1, Unsigned(1))])),
            )])))
        };
        assert!(places(3).passes(&game()));
        assert!(!places(4).passes(&game()));
        let named = |text: &str| {
            Filter::of(&search(Fields::from([(
                0x0,
                Array(vec![Struct(Fields::from([(
                    0x78,
                    Value::String(text.into()),
                )]))]),
            )])))
        };
        assert!(named("parkour").passes(&game()));
        assert!(!named("elimination").passes(&game()));
        let mercy = Filter::of(&search(Fields::from([(
            0x48,
            Array(vec![Unsigned(0x02e0_0000_0000_0004)]),
        )])));
        assert!(!mercy.passes(&game()));
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn a_search_by_code_finds_the_game_with_those_settings() {
        let lookup = |code: &str| {
            Filter::of(&search(Fields::from([
                (
                    0x0,
                    Array(vec![Struct(Fields::from([(
                        0xa0,
                        Value::String(code.into()),
                    )]))]),
                ),
                (0x6e, Signed(3)),
            ])))
        };
        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        let found = results(&lookup("c32s2"), &[game()]);
        assert_eq!(found.2[&0x85], Signed(3));
        assert!(matches!(&found.2[&0x88], Array(entries) if entries.len() == 1));
        schemas.encode(found.0, found.1, &found.2).unwrap();
        let none = results(&lookup("AAAAA"), &[game()]);
        assert!(matches!(&none.2[&0x88], Array(entries) if entries.is_empty()));
        for request in LIST_REQUESTS {
            let (crc, id, fields) = empty_list(request).unwrap();
            schemas.encode(crc, id, &fields).unwrap();
        }
        let (crc, id, fields) = favourite_done();
        schemas.encode(crc, id, &fields).unwrap();
    }
}
