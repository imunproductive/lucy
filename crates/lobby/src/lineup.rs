//! The VERSUS screen a competitive match shows while it loads, and the match
//! lobby of a game. Ports `ow174/lobby/lineup.py` and the match lobby builders
//! of `ow174/lobby/handlers/leaving.py`.
//!
//! What the client reads (image base 0x7FF788F10000):
//! - The loading screen (graph 0147, which the client runs itself) shows
//!   VERSUS 0501.05A (065E.05A in free for all) for 7 s instead of the plain
//!   loading screen when the post-game instance type (subsystem 0x30 +0x20) is
//!   9 Ranked or 13 Ranked Offseason, or 11 Ruleset with a flag (STU_F667872C,
//!   0x7FF7898427B0).
//! - Only 53001 sets that type, from its +0x2A4, and only while the match lobby
//!   has type 4 (53000) and its settings id (+0x278) changed (0x7FF7896949C0).
//!   53003 leaves it alone; leaving a game clears it (20304, 0x7FF7896E6830).
//! - The teams are the match lobby's roster (53002, kept at lobby +0xC0 by
//!   0x7FF789694BA0). The screen finds the local player by record +0x0 (his
//!   account id), takes his team (+0xA8) as the friendly one and needs exactly
//!   one other team (0x7FF789AAB880). Each record gives the card (+0x0, as
//!   20308), the rating (+0x68, as 20700 +0x68), the role (+0xB1, as component
//!   74's) and the group (0x7FF78999A9D0, 0x7FF789999BD0).
//! - 53003 empties the match lobby (0x7FF789D2E5B0: state, roster, settings),
//!   so the roster goes after the 53003 of "Game Found!". A 53000 only replaces
//!   the lobby state (0x7FF789D2E430), so the roster stays when the game's own
//!   match lobby comes at connect. Its lobby id lets the screen show the groups
//!   (+0x90, the party id; 0x7FF789AAB7C0 needs a lobby id). The social menu's
//!   team lists need the settings' team sizes (0x7FF789A94BB0), which stay
//!   empty.
//! - A game that never connects gets no match lobby and no 53003, and one
//!   reader in the menu finds the local player in an old roster
//!   (0x7FF78973E420 region: not a spectator, team not 2), so the lobby also
//!   sends an empty roster when a player leaves a ranked match.
//!
//! The match lobby of a game: with a match lobby id (system 0x6B +0x88) both
//! buttons of the in-match leave dialog send 52902 and wait for the server, so
//! every game that connects gets a match lobby (53000, lobby id = match id)
//! and loses it again (53003) when it leaves the match.

use crate::transport::LobbyMessage;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Signed, Struct, Unsigned},
    id16,
};

/// 53000 state, 53001 settings, 53002 roster, 53003 left.
pub const MATCH_LOBBY: u32 = 0x1cfb_43cd;
/// 52902 leave the match lobby, 52903 the answer to a 53000 of a popped queue.
pub const MATCH_LOBBY_OUT: u32 = 0x8887_16d3;
pub const LEAVE_MATCH_LOBBY: u32 = 52902;
pub const LOBBY_ANSWER: u32 = 52903;
/// The client leaves its game by itself 3 s later (0x7FF7896831A0); in group
/// OUT_CONNECT.
pub const LEAVING_GAME: u32 = 21801;
const LOBBY_STATE: u32 = 53000;
const ROSTER: u32 = 53002;
const LOBBY_LEFT: u32 = 53003;
/// The instance type; the game's 20300 +0x78 carries the same.
pub const RANKED: u64 = 9;
/// 53000 type of a game's match lobby: 0 (UNKNOWN). The dialog sends 52902
/// with any type. 4 (GAME_QUEUE) would also turn the client's play state to 5
/// for the whole match (0x7FF789A9D590), which the menu's search widget shows
/// as "Game Found!"; with 0 the play state stays 0.
const LOBBY_TYPE: u64 = 0;

fn lobby_id(id: [u64; 2]) -> Value {
    Struct(Fields::from([(
        0,
        Array(id.into_iter().map(Unsigned).collect()),
    )]))
}

/// 53000: the match lobby of the player's match. No queue key, so the client
/// always resets the lobby itself after its own 52902 (0x7FF789A9DBC0 keeps it
/// only for a queue card asset).
pub fn lobby_state(match_id: [u64; 2]) -> LobbyMessage {
    state(match_id, LOBBY_TYPE)
}

/// 53000 of a custom game lobby: type 1 CUSTOM_GAME (the names of
/// 0x7FF7896954B0: 0 UNKNOWN, 1 CUSTOM_GAME, 2 PRACTICE, 3 TUTORIAL,
/// 4 GAME_QUEUE). The custom game screens compare it with their node's
/// type (`Eval_STU_A0806584`; `refs/reports/CustomGames.md` §4).
pub fn custom_game_lobby(lobby_id: [u64; 2]) -> LobbyMessage {
    state(lobby_id, CUSTOM_GAME)
}

const CUSTOM_GAME: u64 = 1;

fn state(lobby: [u64; 2], kind: u64) -> LobbyMessage {
    let lobby = Fields::from([(0x30, lobby_id(lobby)), (0x60, Unsigned(kind))]);
    (
        MATCH_LOBBY,
        LOBBY_STATE,
        Fields::from([
            (0x78, Struct(lobby)),
            (0xe0, Bool(false)),
            (0xe8, Unsigned(0)),
        ]),
    )
}

/// 53003: out of the match lobby (0x7FF789693120 resets it). +0x88 0 and
/// reason 0: no status shown.
pub fn lobby_left(match_id: [u64; 2]) -> LobbyMessage {
    (
        MATCH_LOBBY,
        LOBBY_LEFT,
        Fields::from([
            (0x78, lobby_id(match_id)),
            (0x88, Unsigned(0)),
            (0x90, lobby_id([0, 0])),
            (0xa0, Signed(0)),
        ]),
    )
}

/// 53001: the match lobby's game settings. Only the settings id (the match id:
/// it must differ from the last one) and the instance type matter here; the
/// rest stays empty.
pub fn settings(match_id: [u64; 2], instance_type: u64) -> Fields {
    let settings = Fields::from([
        (0x278, Struct(id16(match_id[0], match_id[1]))),
        (0x2a4, Unsigned(instance_type)),
    ]);
    Fields::from([(0x78, Struct(settings))])
}

/// One player of the VERSUS roster.
pub struct Player {
    /// The player's card as 20308 has it.
    pub card: Fields,
    pub team: u8,
    /// The role queue role number; 0 for none.
    pub role: u8,
    /// The rating on the queue card, as 20700 +0x68 lists it.
    pub rating: Option<Value>,
    /// The party id.
    pub group: [u64; 2],
}

/// One player of the roster (a 184-byte 53002 record). +0xA0 stays 0: the
/// client then adds the card to its card cache.
fn record(player: &Player, slot: u64) -> Value {
    let no_rating = || {
        Struct(Fields::from([
            (0x0, Array(Vec::new())),
            (0x18, Unsigned(0)),
            (0x20, Unsigned(0)),
        ]))
    };
    Struct(Fields::from([
        (0x0, Struct(player.card.clone())),
        (0x68, player.rating.clone().unwrap_or_else(no_rating)),
        (0x90, Struct(id16(player.group[0], player.group[1]))),
        (0xa8, Unsigned(u64::from(player.team))),
        (0xaf, Unsigned(slot)),
        (0xb1, Unsigned(u64::from(player.role))),
    ]))
}

/// 53002: every player still in the match, each with his place in his team.
pub fn roster(players: &[Player]) -> Fields {
    let mut slots = std::collections::HashMap::<u8, u64>::new();
    let records = players
        .iter()
        .map(|player| {
            let slot = slots.entry(player.team).or_insert(0);
            let record = record(player, *slot);
            *slot += 1;
            record
        })
        .collect();
    Fields::from([(0x78, Array(records))])
}

/// 53002 without players: the roster of a ranked match the player left.
pub fn no_roster() -> LobbyMessage {
    (
        MATCH_LOBBY,
        ROSTER,
        Fields::from([(0x78, Array(Vec::new()))]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Schemas;

    fn player(team: u8) -> Player {
        Player {
            card: Fields::new(),
            team,
            role: 2,
            rating: None,
            group: [5, 6],
        }
    }

    #[test]
    fn roster_slots_count_within_each_team() {
        let roster = roster(&[player(0), player(1), player(0)]);
        let Array(records) = &roster[&0x78] else {
            panic!()
        };
        let places: Vec<_> = records
            .iter()
            .map(|record| {
                let Struct(record) = record else { panic!() };
                (record[&0xa8].clone(), record[&0xaf].clone())
            })
            .collect();
        assert_eq!(
            places,
            [
                (Unsigned(0), Unsigned(0)),
                (Unsigned(1), Unsigned(0)),
                (Unsigned(0), Unsigned(1)),
            ]
        );
    }

    #[test]
    fn the_messages_encode_with_the_lobby_schemas() {
        let schemas = Schemas::lobby();
        let messages = [
            lobby_state([1, 2]),
            lobby_left([1, 2]),
            (MATCH_LOBBY, 53001, settings([1, 2], RANKED)),
            (MATCH_LOBBY, ROSTER, roster(&[player(0), player(1)])),
            no_roster(),
        ];
        for (crc, id, fields) in messages {
            schemas.encode(crc, id, &fields).unwrap();
        }
    }
}
