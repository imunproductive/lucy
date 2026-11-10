//! The parties the matchmaker works with.
//!
//! Python keeps parties in `ow174/services/social.py`; the matchmaker takes a
//! party through [`Parties`]. Until the social state is wired in, every player
//! is alone: a party of one with the party id of its login (the one the login
//! party state carries), and nothing to leave.

use std::{io, sync::Arc};

use orpheus_lobby::{
    accounts::Account,
    handoff::{general_channel, join_chat_channel, match_channel},
    login::Identity,
    menu::MenuHero,
    party::{self, Membership, PartyMember},
    queue::{QueueFields, add_to_state},
    ranked::LeaderboardPlaces,
    transport::LobbyMessage,
};
use orpheus_wire::jam::codec::{Fields, Value};

use crate::lobby::Lobby;

const CHAT_IN: u32 = 0x5f91_3f6b;
const LEAVE_CHANNEL: u32 = 20404;

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
        let id = self
            .matchmaker
            .identity(account.account_lo)
            .map_or([0; 2], |identity| identity.party_id);
        Ok(Party {
            id,
            leader: account.account_lo,
            members: vec![Arc::clone(account)],
        })
    }

    fn notify_party(&self, party: &Party, queue: Option<&QueueFields>) -> io::Result<()> {
        for member in &party.members {
            let Some(identity) = self.matchmaker.identity(member.account_lo) else {
                continue;
            };
            let mut state = solo_state(self, member, identity)?;
            if let Some(queue) = queue {
                add_to_state(&mut state, queue, &[Some(member.account_lo)]);
            }
            self.hub.post(member.account_lo, state);
        }
        Ok(())
    }

    fn leave_party(&self, _account: &Arc<Account>) -> io::Result<bool> {
        Ok(false)
    }

    fn open_match_chat(&self, _match_id: [u64; 2], _accounts: &[u32]) {}

    fn game_left(&self, account_lo: u32, match_id: [u64; 2]) -> io::Result<()> {
        let leave = (
            CHAT_IN,
            LEAVE_CHANNEL,
            Fields::from([(0x78, Value::Struct(match_channel(match_id)))]),
        );
        self.hub.post(account_lo, leave);
        self.hub
            .post(account_lo, join_chat_channel(general_channel()));
        let Some(account) = self.accounts.by_id(account_lo)? else {
            return Ok(());
        };
        let party = self.party_of(&account)?;
        let queue = self.matchmaker.queue_fields(self, &party);
        self.notify_party(&party, queue.as_ref())
    }
}

/// 20700 for a player alone, as the login sends it.
fn solo_state(lobby: &Lobby, account: &Account, identity: Identity) -> io::Result<LobbyMessage> {
    let profile = account.profile();
    let menu = MenuHero::default().select(
        &profile,
        lobby.collection.heroes(),
        lobby.collection.items(),
    )?;
    let members_may_invite = profile
        .settings
        .get("+0x10D")
        .and_then(|settings| settings.get("+0x0"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let ratings = lobby
        .ranked
        .party_ratings(&profile, &LeaderboardPlaces::new())?;
    let member = PartyMember {
        profile: &profile,
        identity,
        menu_hero: menu.party_hero,
        ratings: &ratings,
        membership: Membership::Leader,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    party::state(
        &lobby.collection,
        identity.party_id,
        identity.party_entity,
        &[member],
        members_may_invite,
        now,
    )
}
