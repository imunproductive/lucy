//! Queueing for a game (44100-44107), the Practice Range request (24000) and
//! the answer to "Game Found!" (52903). Ports the queue part of
//! `ow174/lobby/handlers/matchmaking.py`; the matchmaker
//! (`lobby/matchmaker.rs`) puts the parties into matches.

use std::io;

use lucy_lobby::{
    handoff::{CREATE_GAME, GAME_REQUEST, is_request},
    lineup::{LOBBY_ANSWER, MATCH_LOBBY_OUT},
    queue::{
        ACCEPT, CANCEL_QUEUE, CHANGE_ROLES, DECLINE, ENTER_QUEUE, MATCHMAKE, PASS_ROLE, SET_ROLES,
    },
};
use lucy_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    Lobby,
    peer::Peer,
    router::{Hooks, Module, Route},
};

const fn route(group: u32, message: u32, handle: crate::lobby::router::Handler) -> Route {
    Route {
        group,
        message,
        handle,
    }
}

/// The client's migration query (60002, group `0x49963FA6`): it sends it
/// once its main menu has the initial data (20502, `ow1-preservation.md`
/// "Initial data"). Nothing answers it.
const MIGRATION: u32 = 0x4996_3fa6;
const MIGRATION_QUERY: u32 = 60002;

pub(crate) const MODULE: Module = Module {
    routes: &[
        route(MATCHMAKE, ENTER_QUEUE, enter_queue),
        route(MATCHMAKE, CANCEL_QUEUE, cancel_queue),
        route(MATCHMAKE, SET_ROLES, set_roles),
        route(MATCHMAKE, ACCEPT, accept),
        route(MATCHMAKE, DECLINE, decline),
        route(MATCHMAKE, PASS_ROLE, pass_role),
        route(MATCHMAKE, CHANGE_ROLES, change_roles),
        route(GAME_REQUEST, CREATE_GAME, create_game),
        route(MATCH_LOBBY_OUT, LOBBY_ANSWER, lobby_answer),
        route(MIGRATION, MIGRATION_QUERY, menu_ready),
    ],
    hooks: Hooks {
        login: Some(login),
        tick: Some(tick),
        logout: Some(logout),
    },
};

fn login(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    peer.lobby.matchmaker.login(peer.login);
    peer.lobby.matchmaker.workshop_login(peer.login);
    Ok(())
}

/// The menu is up: a `--workshop` player's match is due.
fn menu_ready(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    peer.lobby
        .matchmaker
        .workshop_menu_ready(peer.lobby, peer.login);
    Ok(())
}

fn tick(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    peer.lobby.matchmaker.tick(peer)
}

fn logout(lobby: &Lobby, account: &std::sync::Arc<lucy_lobby::accounts::Account>) {
    lobby.matchmaker.logout(lobby, account);
}

/// The queue key of a 44100 or 44102 (+0x78).
fn key(fields: &Fields) -> Fields {
    match fields.get(&0x78) {
        Some(Value::Struct(key)) => key.clone(),
        _ => Fields::new(),
    }
}

/// The role numbers of a 44103 or 44106 (+0x78).
fn numbers(fields: &Fields) -> Vec<u64> {
    let Some(Value::Array(values)) = fields.get(&0x78) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| match *value {
            Value::Unsigned(number) => Some(number),
            Value::Signed(number) => u64::try_from(number).ok(),
            _ => None,
        })
        .collect()
}

/// 44100: a queue was picked (a role queue opens its role screen at once).
fn enter_queue(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let shown: String = format!("{fields:?}").chars().take(300).collect();
    tracing::info!("[MM] Enter queue (44100) {shown}");
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .enter_queue(lobby, &peer.login.account, key(&fields))
}

/// 44102 {key}: the player left the queue.
fn cancel_queue(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .cancel_queue(lobby, &peer.login.account, key(&fields))
}

/// 44103 {role numbers}: Ready on the role screen, with the whole list.
fn set_roles(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .set_roles(lobby, &peer.login.account, &numbers(&fields))
}

/// 44104: Accept on the group banner "pick a role now?".
fn accept(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby.matchmaker.accept(lobby, &peer.login.account)
}

/// 44105: Decline on that banner.
fn decline(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby.matchmaker.decline(lobby, &peer.login.account)
}

/// 44106 {role numbers}: the role to spend a priority pass on, sent after
/// 44103.
fn pass_role(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .pass_role(lobby, &peer.login.account, &numbers(&fields))
}

/// 44107: Change Role on the role screen, once the member is ready.
fn change_roles(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby.matchmaker.change_roles(lobby, &peer.login.account)
}

/// 24000: a create-game request. Kind 2 with flags 4 is the Practice Range;
/// kind 1 a custom game (`custom_game.rs`). It carries a creation kind, not
/// a mode GUID.
fn create_game(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let request = (GAME_REQUEST, CREATE_GAME, fields);
    if let Some(name) = lucy_lobby::custom_game::create_request(&request) {
        return super::custom_game::create(peer, name);
    }
    if !is_request(&request) {
        tracing::info!("[MM] Unknown create-game request: {:?}", request.2);
        return Ok(());
    }
    // A game that still holds its seat stays where it is, as the solo
    // Practice Range did before the matchmaker answered 24000.
    if let (Some(old), Some(game)) = (peer.seat.as_ref(), peer.lobby.game.as_deref()) {
        if game.seat_exists(&old.handoff)? {
            return Ok(());
        }
    }
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .practice(lobby, &peer.login.account, peer.login.identity.session)
}

/// 52903 {bool}: sent by the client itself on a 53000 with match lobby type 4
/// (0x7FF789692FE0): true when the lobby's ticket is the one of the queue
/// entry 44202 took out. Nothing waits for it.
fn lobby_answer(_: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    tracing::info!(
        "[MM] \"Game Found!\" match lobby answer (52903): {:?}",
        fields.get(&0x78)
    );
    Ok(())
}
