//! The solo Practice Range request (24000, kind 2).

use std::io;

use orpheus_lobby::handoff::{CREATE_GAME, GAME_REQUEST, is_request};
use orpheus_wire::jam::codec::Fields;

use crate::{
    lobby::{
        match_id,
        peer::Peer,
        router::{Hooks, Module, Route},
    },
    practice::{self, SoloPracticeHandoff},
};

pub(crate) const MODULE: Module = Module {
    routes: &[Route {
        group: GAME_REQUEST,
        message: CREATE_GAME,
        handle: create_game,
    }],
    hooks: Hooks::NONE,
};

fn create_game(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let request = (GAME_REQUEST, CREATE_GAME, fields);
    if !is_request(&request) {
        return Ok(());
    }
    if let Some(old) = peer.seat.as_ref() {
        if peer.lobby.game.seat_exists(&old.handoff)? {
            return Ok(());
        }
    }
    let lobby = peer.lobby;
    let host = peer.session.game_host(&lobby.platform.game_host);
    let input = SoloPracticeHandoff::new(match_id()?, &host);
    let session = &mut *peer.session;
    *peer.seat = practice::dispatch_scene(
        &request,
        peer.login,
        &lobby.game,
        input,
        (&lobby.assets, &lobby.collection),
        |map| {
            let first = lobby.first_scene.lock().expect("prepared scene").take();
            first.map_or_else(|| lobby.assets.prepare(map), Ok)
        },
        |message| session.send(message),
    )?;
    Ok(())
}
