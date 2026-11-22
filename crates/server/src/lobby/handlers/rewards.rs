//! Event greetings and login gifts (38901), challenge rewards (24901), team-skin pairs, and the account features (55500) sent again after login.
//!
//! The rewards are claimed before the login messages are built
//! (`services::claim_at_login`, called by the session) so the login state
//! already holds them. This module keeps the session's timed work: the
//! account features at +3 s and +10 s, and the dashboard's live pushes.

use std::{io, sync::Arc};

use lucy_lobby::accounts::Account;

use crate::lobby::{
    Lobby,
    peer::Peer,
    router::{Hooks, Module},
    services,
};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks {
        login: Some(logged_in),
        tick: Some(services::tick),
        logout: Some(logged_out),
    },
};

fn logged_in(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    peer.lobby
        .services
        .logged_in(peer.login, peer.lobby.clock.now());
    Ok(())
}

fn logged_out(lobby: &Lobby, account: &Arc<Account>) {
    lobby.services.logged_out(account);
}
