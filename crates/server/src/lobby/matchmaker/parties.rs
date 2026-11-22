//! The parties the matchmaker works with.
//!
//! Python keeps parties in `ow174/services/social.py`; the matchmaker takes a
//! party through [`Parties`], which the lobby's social state answers.

use std::{io, sync::Arc};

use lucy_lobby::{accounts::Account, queue::QueueFields};

use crate::lobby::{Lobby, social};

/// A party as the matchmaker sees it.
#[derive(Clone)]
pub(crate) struct Party {
    pub id: [u64; 2],
    pub leader: u32,
    /// Leader first.
    pub members: Vec<Arc<Account>>,
}

/// What the matchmaker needs from the social state.
pub(crate) trait Parties {
    /// The account's party; a player alone is a party of one.
    fn party_of(&self, account: &Arc<Account>) -> io::Result<Party>;
    /// Send the party state (20700) to every member that is online, with the
    /// role queue the party is in (Python `LobbyServer.notify_party`).
    fn notify_party(&self, party: &Party, queue: Option<&QueueFields>) -> io::Result<()>;
    /// Take the account out of its party, as 22107 does; false for a party of
    /// one (`handlers/leaving.py` `leave_party`).
    fn leave_party(&self, account: &Arc<Account>) -> io::Result<bool>;
    /// The players of a match share its chat channel (Python
    /// `social.open_match_chat`).
    fn open_match_chat(&self, match_id: [u64; 2], accounts: &[u32]);
    /// The account's game left its match: out of the match chat, back into
    /// General, and the party state again (the social half of Python
    /// `LobbyServer.game_left`).
    fn game_left(&self, account_lo: u32, match_id: [u64; 2]) -> io::Result<()>;
}

impl Parties for Lobby {
    fn party_of(&self, account: &Arc<Account>) -> io::Result<Party> {
        let view = social::party_view(self, account)?;
        Ok(Party {
            id: view.id,
            leader: view.leader,
            members: view.members,
        })
    }

    fn notify_party(&self, party: &Party, queue: Option<&QueueFields>) -> io::Result<()> {
        social::notify_party(self, party.id, queue)
    }

    fn leave_party(&self, account: &Arc<Account>) -> io::Result<bool> {
        social::leave_party(self, account)
    }

    fn open_match_chat(&self, match_id: [u64; 2], accounts: &[u32]) {
        let mut state = self.social.lock();
        for &account_lo in accounts {
            state.open_match_chat(match_id, account_lo);
        }
    }

    fn game_left(&self, account_lo: u32, match_id: [u64; 2]) -> io::Result<()> {
        social::game_left(self, account_lo, match_id)
    }
}
