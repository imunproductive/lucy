//! Custom games: the lobby's requests (24001-24020), JOIN (52900), the
//! Game Browser (40700-40704) and the settings codes and saved settings
//! (40800-40809). The create request (24000 kind 1) comes through
//! `matchmaking.rs`. The work is the matchmaker's (`matchmaker/custom.rs`,
//! `matchmaker/codes.rs`); `refs/reports/CustomGames.md`.

use std::io;

use lucy_lobby::{
    custom_game::{
        UPLOAD,
        browser::{self, BROWSER_IN, FAVOURITE, LIST_REQUESTS, SEARCH},
        links::{self, DELETE_PRESET, GET_SAVED, LINK_UPLOAD, LINKS_IN, LOAD_LINK, NAME_PRESET},
        lobby::{
            ACCEPT_INVITE, ADD_BOT, DECLINE_INVITE, DETAILS_REQUEST, EDIT_BOT, INVITE,
            LOBBY_COMMAND, MOVE, PLAYER_RIGHTS, REMOVE, RENAME, SWAP,
        },
    },
    handoff::GAME_REQUEST,
    lineup::MATCH_LOBBY_OUT,
    player_messages,
};
use lucy_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    matchmaker::custom::{Asker, asker},
    peer::Peer,
    router::{Handler, Hooks, Module, Route},
};

/// 52900: join a match lobby (JOIN in the browser, a friend's Join game).
const JOIN_MATCH_LOBBY: u32 = 52900;

const fn route(group: u32, message: u32, handle: Handler) -> Route {
    Route {
        group,
        message,
        handle,
    }
}

const fn request(message: u32, handle: Handler) -> Route {
    route(GAME_REQUEST, message, handle)
}

pub(crate) const MODULE: Module = Module {
    routes: &[
        request(UPLOAD.header, |peer, fields| {
            upload(peer, UPLOAD.header, fields)
        }),
        request(UPLOAD.header + 1, |peer, fields| {
            upload(peer, UPLOAD.header + 1, fields)
        }),
        request(UPLOAD.header + 2, |peer, fields| {
            upload(peer, UPLOAD.header + 2, fields)
        }),
        request(UPLOAD.header + 3, |peer, fields| {
            upload(peer, UPLOAD.header + 3, fields)
        }),
        request(UPLOAD.header + 4, |peer, fields| {
            upload(peer, UPLOAD.header + 4, fields)
        }),
        request(UPLOAD.header + 5, |peer, fields| {
            upload(peer, UPLOAD.header + 5, fields)
        }),
        request(LOBBY_COMMAND, command),
        request(INVITE, |peer, fields| lobby_request(peer, INVITE, fields)),
        request(REMOVE, |peer, fields| lobby_request(peer, REMOVE, fields)),
        request(ADD_BOT, |peer, fields| lobby_request(peer, ADD_BOT, fields)),
        request(EDIT_BOT, |peer, fields| {
            lobby_request(peer, EDIT_BOT, fields)
        }),
        request(SWAP, |peer, fields| lobby_request(peer, SWAP, fields)),
        request(ACCEPT_INVITE, |peer, fields| {
            lobby_request(peer, ACCEPT_INVITE, fields)
        }),
        request(DECLINE_INVITE, |peer, fields| {
            lobby_request(peer, DECLINE_INVITE, fields)
        }),
        request(PLAYER_RIGHTS, |peer, fields| {
            lobby_request(peer, PLAYER_RIGHTS, fields)
        }),
        request(MOVE, |peer, fields| lobby_request(peer, MOVE, fields)),
        request(RENAME, |peer, fields| lobby_request(peer, RENAME, fields)),
        request(DETAILS_REQUEST, |peer, fields| {
            lobby_request(peer, DETAILS_REQUEST, fields)
        }),
        route(MATCH_LOBBY_OUT, JOIN_MATCH_LOBBY, join),
        route(BROWSER_IN, LIST_REQUESTS[0], |peer, _| {
            list(peer, LIST_REQUESTS[0])
        }),
        route(BROWSER_IN, LIST_REQUESTS[1], |peer, _| {
            list(peer, LIST_REQUESTS[1])
        }),
        route(BROWSER_IN, LIST_REQUESTS[2], |peer, _| {
            list(peer, LIST_REQUESTS[2])
        }),
        route(BROWSER_IN, SEARCH, search),
        route(BROWSER_IN, FAVOURITE, |peer, _| {
            peer.send(&browser::favourite_done()).map(drop)
        }),
        route(LINKS_IN, GET_SAVED, get_saved),
        route(LINKS_IN, DELETE_PRESET, delete_preset),
        route(LINKS_IN, NAME_PRESET, name_preset),
        route(LINKS_IN, LOAD_LINK, load_link),
        route(LINKS_IN, LINK_UPLOAD.header, |peer, fields| {
            link_upload(peer, LINK_UPLOAD.header, fields)
        }),
        route(LINKS_IN, LINK_UPLOAD.header + 1, |peer, fields| {
            link_upload(peer, LINK_UPLOAD.header + 1, fields)
        }),
        route(LINKS_IN, LINK_UPLOAD.header + 2, |peer, fields| {
            link_upload(peer, LINK_UPLOAD.header + 2, fields)
        }),
        route(LINKS_IN, LINK_UPLOAD.header + 3, |peer, fields| {
            link_upload(peer, LINK_UPLOAD.header + 3, fields)
        }),
        route(LINKS_IN, LINK_UPLOAD.header + 4, |peer, fields| {
            link_upload(peer, LINK_UPLOAD.header + 4, fields)
        }),
        route(LINKS_IN, LINK_UPLOAD.header + 5, |peer, fields| {
            link_upload(peer, LINK_UPLOAD.header + 5, fields)
        }),
    ],
    hooks: Hooks {
        login: Some(login),
        tick: None,
        logout: None,
    },
};

/// The player's card as a lobby shows it.
fn card(peer: &Peer<'_, '_>) -> Fields {
    let login = peer.login;
    let profile = login.account.profile();
    let portrait_frame = peer.lobby.collection.portrait_frame(&profile);
    player_messages::record(&profile, login.identity, portrait_frame, peer.wall_time)
}

fn asking<'a>(peer: &'a Peer<'_, '_>) -> io::Result<Asker<'a>> {
    asker(peer.lobby, &peer.login.account, card(peer))
}

/// At login: the player's preset names (40900; the client never asks).
fn login(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let names = peer.lobby.matchmaker.preset_names(&peer.login.account)?;
    peer.send(&names).map(drop)
}

/// 24000 kind 1: a new custom game lobby with the player as its owner.
pub(super) fn create(peer: &mut Peer<'_, '_>, name: String) -> io::Result<()> {
    let asker = asking(peer)?;
    peer.lobby
        .matchmaker
        .create_custom_game(peer.lobby, &asker, name)
}

fn upload(peer: &mut Peer<'_, '_>, id: u32, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .custom_game_settings(lobby, &peer.login.account, id, fields)
}

/// 24018 `{u8}`.
fn command(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    lobby
        .matchmaker
        .custom_game_command(lobby, &peer.login.account, &fields)
}

fn lobby_request(peer: &mut Peer<'_, '_>, id: u32, fields: Fields) -> io::Result<()> {
    let asker = asking(peer)?;
    peer.lobby
        .matchmaker
        .custom_game_request(peer.lobby, &asker, id, &fields)
}

/// 52900 `{+0x78 lobby, +0x88 id, +0x98 type, +0x99 as, …}`: a custom
/// game's (type 1) joins it; nothing else asks this server for one.
fn join(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let asker = asking(peer)?;
    if !peer
        .lobby
        .matchmaker
        .join_custom_game(peer.lobby, &asker, &fields)?
    {
        tracing::info!("[<<<] Join match lobby (52900) of no custom game: {fields:?}");
    }
    Ok(())
}

fn list(peer: &mut Peer<'_, '_>, request: u32) -> io::Result<()> {
    match browser::empty_list(request) {
        Some(answer) => peer.send(&answer).map(drop),
        None => Ok(()),
    }
}

/// 40703: the custom games that pass the filter.
fn search(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let results = peer.lobby.matchmaker.browse_custom_games(&fields);
    peer.send(&results).map(drop)
}

fn link_upload(peer: &mut Peer<'_, '_>, id: u32, fields: Fields) -> io::Result<()> {
    let account = &peer.login.account;
    match peer.lobby.matchmaker.link_upload(account, id, fields) {
        Ok(messages) => peer.send_all(&messages),
        Err(error) => {
            tracing::warn!("[CG] Settings upload {id} from {}: {error}", account.name);
            Ok(())
        }
    }
}

/// 40809 `{+0x78 code, +0xA0 bool}`: Import Code.
fn load_link(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let code = links::text(&fields, 0x78);
    let messages = peer.lobby.matchmaker.load_link(&code)?;
    peer.send_all(&messages)
}

/// 40800 `{slot, which}`.
fn get_saved(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let Some(request) = links::saved_request(&fields) else {
        return Ok(());
    };
    let messages = peer
        .lobby
        .matchmaker
        .saved_settings(&peer.login.account, request)?;
    peer.send_all(&messages)
}

fn slot(fields: &Fields) -> Option<u32> {
    match fields.get(&0x78) {
        Some(Value::Unsigned(slot)) => u32::try_from(*slot).ok(),
        Some(Value::Signed(slot)) => u32::try_from(*slot).ok(),
        _ => None,
    }
}

/// 40801 `{slot}`.
fn delete_preset(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    match slot(&fields) {
        Some(slot) => peer
            .lobby
            .matchmaker
            .delete_preset(&peer.login.account, slot),
        None => Ok(()),
    }
}

/// 40808 `{slot, name}`.
fn name_preset(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    match slot(&fields) {
        Some(slot) => {
            peer.lobby
                .matchmaker
                .name_preset(&peer.login.account, slot, links::text(&fields, 0x80))
        }
        None => Ok(()),
    }
}
