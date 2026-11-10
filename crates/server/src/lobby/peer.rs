//! What a handler gets: the lobby, the logged-in session and its account.

use std::io;

use orpheus_lobby::{
    handoff::LobbySeat,
    session::{LobbySession, PreparedLogin},
    transport::LobbyMessage,
};

use super::Lobby;

pub(crate) struct Peer<'a, 's> {
    pub lobby: &'a Lobby,
    pub login: &'a PreparedLogin,
    pub session: &'a mut LobbySession<'s>,
    /// The game seat this session holds, if any.
    pub seat: &'a mut Option<LobbySeat>,
    /// Seconds since 1970 at login.
    pub wall_time: u64,
}

impl Peer<'_, '_> {
    /// Send to this session. False when the client did not announce the
    /// message's protocol group.
    pub(crate) fn send(&mut self, message: &LobbyMessage) -> io::Result<bool> {
        self.session.send(message)
    }

    /// Send several, in order.
    pub(crate) fn send_all(&mut self, messages: &[LobbyMessage]) -> io::Result<()> {
        for message in messages {
            self.session.send(message)?;
        }
        Ok(())
    }
}
