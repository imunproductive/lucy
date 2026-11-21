//! Custom games: the settings upload (24008-24013) and the lobby command
//! (24018). The create request (24000 kind 1) comes through
//! `matchmaking.rs`, which answers 24000. `refs/reports/CustomGames.md`.

use std::io;

use orpheus_lobby::{
    custom_game::{LOBBY_COMMAND, UPLOAD},
    handoff::GAME_REQUEST,
    player_messages,
};
use orpheus_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
};

const fn route(message: u32, handle: crate::lobby::router::Handler) -> Route {
    Route {
        group: GAME_REQUEST,
        message,
        handle,
    }
}

pub(crate) const MODULE: Module = Module {
    routes: &[
        route(UPLOAD[0], |peer, fields| upload(peer, UPLOAD[0], fields)),
        route(UPLOAD[1], |peer, fields| upload(peer, UPLOAD[1], fields)),
        route(UPLOAD[2], |peer, fields| upload(peer, UPLOAD[2], fields)),
        route(UPLOAD[3], |peer, fields| upload(peer, UPLOAD[3], fields)),
        route(UPLOAD[4], |peer, fields| upload(peer, UPLOAD[4], fields)),
        route(UPLOAD[5], |peer, fields| upload(peer, UPLOAD[5], fields)),
        route(LOBBY_COMMAND, command),
    ],
    hooks: Hooks::NONE,
};

/// 24000 kind 1: a new custom game lobby with the player as its host; his
/// card as the lobby shows it (the descriptor's `+0x0`).
pub(super) fn create(peer: &mut Peer<'_, '_>, name: String) -> io::Result<()> {
    let login = peer.login;
    let profile = login.account.profile();
    let portrait_frame = peer.lobby.collection.portrait_frame(&profile);
    let card = player_messages::record(&profile, login.identity, portrait_frame, peer.wall_time);
    peer.lobby
        .matchmaker
        .create_custom_game(peer.lobby, &login.account, name, card)
}

fn upload(peer: &mut Peer<'_, '_>, id: u32, fields: Fields) -> io::Result<()> {
    let account = &peer.login.account;
    let messages = match peer
        .lobby
        .matchmaker
        .custom_game_settings(account, id, fields)
    {
        Ok(messages) => messages,
        Err(error) => {
            tracing::warn!("[CG] Settings {id} from {}: {error}", account.name);
            return Ok(());
        }
    };
    peer.send_all(&messages)
}

/// 24018 `{u8}`.
fn command(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let command = match fields.get(&0x78) {
        Some(Value::Unsigned(value)) => Some(*value),
        Some(Value::Signed(value)) => u64::try_from(*value).ok(),
        _ => None,
    };
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .custom_game_command(lobby, &peer.login.account, command)
}
