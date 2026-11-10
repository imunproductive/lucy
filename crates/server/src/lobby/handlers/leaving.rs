//! Leaving a match: "Leave" and "Leave as group" of the in-match leave dialog
//! (05A/0715, graph 0C1E). Ports `ow174/lobby/handlers/leaving.py`; the match
//! lobby a game gets when it connects (53000) and loses when it leaves (53003)
//! comes from the matchmaker's tick (`lobby/matchmaker.rs`).
//!
//! What the client does (PC 1.74):
//! - With a match lobby id (system 0x6B +0x88) both buttons send 52902
//!   {+0x78 lobby id, +0x88 as group, +0x89 0} (0x7FF789695010) and wait for
//!   the server: the client does not leave the game by itself. "Leave as
//!   group" is offered only to the leader of a party with others, outside
//!   competitive (CanLeaveAsGroup, 0C1E #24749).
//! - Without one, "Leave" sends 21801 {} and leaves 3 s later by itself
//!   (0x7FF7896831A0), and "Leave as group" only cancels a queue entry (44102,
//!   0x7FF789A9DEB0): nothing reaches the server.
//! - No client path leaves the party: a player who leaves alone is taken out
//!   of it by the server.

use std::io;

use orpheus_lobby::{
    lineup::{LEAVE_MATCH_LOBBY, LEAVING_GAME, MATCH_LOBBY_OUT},
    login::OUT_CONNECT,
};
use orpheus_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        Route {
            group: MATCH_LOBBY_OUT,
            message: LEAVE_MATCH_LOBBY,
            handle: leave_match_lobby,
        },
        Route {
            group: OUT_CONNECT,
            message: LEAVING_GAME,
            handle: leaving_game,
        },
    ],
    hooks: Hooks::NONE,
};

fn truthy(value: Option<&Value>) -> bool {
    match value {
        Some(&Value::Bool(value)) => value,
        Some(&Value::Unsigned(value)) => value != 0,
        Some(&Value::Signed(value)) => value != 0,
        _ => false,
    }
}

/// The lobby id of a 52902 (+0x78 +0x0), or 0 0.
fn lobby_id(fields: &Fields) -> [u64; 2] {
    let Some(Value::Struct(lobby)) = fields.get(&0x78) else {
        return [0; 2];
    };
    let Some(Value::Array(id)) = lobby.get(&0) else {
        return [0; 2];
    };
    let half = |index: usize| match id.get(index) {
        Some(&Value::Unsigned(half)) => half,
        Some(&Value::Signed(half)) => half as u64,
        _ => 0,
    };
    [half(0), half(1)]
}

/// 52902: "Leave" or "Leave as group" in the leave dialog, with the match
/// lobby the client has.
fn leave_match_lobby(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby.matchmaker.leave_match_lobby(
        lobby,
        &peer.login.account,
        truthy(fields.get(&0x88)),
        lobby_id(&fields),
        truthy(fields.get(&0x89)),
    )
}

/// 21801 {}: the client leaves its game by itself 3 s later.
fn leaving_game(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby.matchmaker.leaving_game(lobby, &peer.login.account)
}
