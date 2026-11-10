//! Who hears about parties, friends and presence: the social half of the
//! Python lobby server (`ow174/lobby/server.py` notify_friends,
//! notify_presence, notify_party, notify_watchers, game_left;
//! `ow174/lobby/session.py` party_messages and the social part of _cleanup;
//! the social messages of `ow174/lobby/handlers/login.py`).
//!
//! A request locks the social state once ([`Turn`]), queues what each player
//! must hear, and delivers it after the lock is released: to its own client
//! directly, to other sessions through their mailboxes.

use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, MutexGuard},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use orpheus_lobby::{
    accounts::Account,
    handoff::general_channel,
    ranked::LeaderboardPlaces,
    social::{ChannelChange, PartyId, SocialState, content::SocialContent, friends_of, messages},
    transport::LobbyMessage,
};

#[cfg(test)]
mod tests;

use super::{Lobby, peer::Peer};

/// No account has a Top 500 place yet (the leaderboard is another port).
static NO_PLACES: LeaderboardPlaces = BTreeMap::new();

/// Messages for players, delivered in the order they were queued.
#[derive(Default)]
pub(crate) struct Outbox(Vec<(u32, LobbyMessage)>);

impl Outbox {
    pub(crate) fn to(&mut self, account_lo: u32, message: LobbyMessage) {
        self.0.push((account_lo, message));
    }

    /// Every message through the mailboxes; offline players miss theirs.
    pub(crate) fn deliver(self, lobby: &Lobby) {
        for (account_lo, message) in self.0 {
            lobby.hub.post(account_lo, message);
        }
    }

    /// The peer's own messages directly, the others through the mailboxes.
    pub(crate) fn deliver_to(self, peer: &mut Peer<'_, '_>) -> io::Result<()> {
        let me = peer.login.account.account_lo;
        for (account_lo, message) in self.0 {
            if account_lo == me {
                peer.send(&message)?;
            } else {
                peer.lobby.hub.post(account_lo, message);
            }
        }
        Ok(())
    }
}

/// One request's hold on the social state.
pub(crate) struct Turn<'a> {
    pub lobby: &'a Lobby,
    pub state: MutexGuard<'a, SocialState>,
    pub content: SocialContent<'a>,
    pub out: Outbox,
}

impl<'a> Turn<'a> {
    pub(crate) fn new(lobby: &'a Lobby) -> io::Result<Self> {
        let wall_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        Ok(Self {
            lobby,
            state: lobby.social.lock(),
            content: SocialContent {
                social: &lobby.social,
                collection: &lobby.collection,
                ranked: &lobby.ranked,
                presence: &lobby.presence,
                places: &NO_PLACES,
                wall_time,
                now: Instant::now(),
            },
            out: Outbox::default(),
        })
    }

    /// Release the state and deliver, the peer's own messages directly.
    pub(crate) fn finish(self, peer: &mut Peer<'_, '_>) -> io::Result<()> {
        let Self { state, out, .. } = self;
        drop(state);
        out.deliver_to(peer)
    }

    /// Release the state and deliver everything through the mailboxes.
    pub(crate) fn finish_posted(self) {
        let Self {
            lobby, state, out, ..
        } = self;
        drop(state);
        out.deliver(lobby);
    }

    pub(crate) fn to(&mut self, account_lo: u32, message: LobbyMessage) {
        self.out.to(account_lo, message);
    }

    /// Whether a client of the account is logged in (not the bot).
    pub(crate) fn has_session(&self, account_lo: u32) -> bool {
        self.state.session(account_lo).is_some()
    }

    pub(crate) fn bot(&self) -> &'a Arc<Account> {
        &self.lobby.accounts.bot
    }

    pub(crate) fn record(&self, account: &Account) -> orpheus_wire::jam::codec::Fields {
        self.content.record(account)
    }

    /// The party state, plus joining or leaving the party chat channel when
    /// that changed (`session.py` party_messages).
    pub(crate) fn party_messages(
        &mut self,
        account: &Arc<Account>,
    ) -> io::Result<Vec<LobbyMessage>> {
        let id = self.state.party_of(account)?;
        let party = self.state.party(id).expect("the account's party");
        let mut messages = vec![self.content.party_state(party)?];
        match self.state.party_channel_change(account.account_lo, id) {
            ChannelChange::Join(channel) => messages.push(messages::joined(channel)),
            ChannelChange::Leave(channel) => messages.push(messages::left(channel)),
            ChannelChange::None => {}
        }
        Ok(messages)
    }

    /// Send the party state to every member that is online, and the group to
    /// players who watch it in the group finder. A party left empty only
    /// closes for its watchers.
    pub(crate) fn notify_party(&mut self, id: PartyId) -> io::Result<()> {
        let members = self
            .state
            .party(id)
            .map_or_else(Vec::new, |party| party.members.clone());
        for member in members {
            if self.has_session(member.account_lo) {
                for message in self.party_messages(&member)? {
                    self.to(member.account_lo, message);
                }
            }
        }
        self.notify_watchers(id)
    }

    /// 52301 refreshes a watched group while it looks for players; 52302
    /// drops a group that stopped from the client's watched list.
    pub(crate) fn notify_watchers(&mut self, id: PartyId) -> io::Result<()> {
        let open = self.state.party(id).filter(|party| party.is_open_group());
        let group = open.map(|party| self.content.group(party)).transpose()?;
        let closed = group.is_none();
        for watcher in self.state.watchers(id, closed) {
            let message = match &group {
                Some(group) => messages::group_changed(group.clone()),
                None => messages::group_closed(id),
            };
            self.to(watcher, message);
        }
        Ok(())
    }

    /// Show an account's online friends its presence (27113) and card
    /// (20809) again, after it logs in or out or changes its card. Not a
    /// whole friends list (27100): the client takes that as every online
    /// friend coming online again (0x7FF789613930).
    pub(crate) fn notify_friends(&mut self, account: &Account) -> io::Result<()> {
        let records = self.content.presence(&self.state, account)?;
        for friend in friends_of(&self.lobby.accounts, account)? {
            if !self.has_session(friend.account_lo) {
                continue;
            }
            let cards = self
                .content
                .friend_cards(&self.state, &self.lobby.accounts, &friend)?;
            self.to(friend.account_lo, messages::presence(records.clone()));
            self.to(friend.account_lo, cards);
        }
        Ok(())
    }

    /// Send an account's presence again (27113) after a status change: its
    /// online friends see the new colour, and its own client the new status
    /// (the dropdown and the name card read it from there).
    pub(crate) fn notify_presence(&mut self, account: &Account) -> io::Result<()> {
        let seen = self.content.presence(&self.state, account)?;
        for friend in friends_of(&self.lobby.accounts, account)? {
            if self.has_session(friend.account_lo) {
                self.to(friend.account_lo, messages::presence(seen.clone()));
            }
        }
        if self.has_session(account.account_lo) {
            let own = self.content.own_presence(account)?;
            self.to(account.account_lo, messages::presence(own));
        }
        Ok(())
    }

    /// Back in the menu after a match: out of the match chat (else its
    /// "Match" channel stays in the menu), into General again, and the party
    /// state again (`server.py` game_left). Only the first call for a match
    /// changes the channels.
    pub(crate) fn game_left(&mut self, account_lo: u32, match_id: PartyId) -> io::Result<()> {
        if !self.has_session(account_lo) {
            return Ok(());
        }
        if let Some(channel) = self.state.leave_match_chat(account_lo, Some(match_id)) {
            self.to(account_lo, messages::left(channel));
            self.to(account_lo, messages::joined(general_channel()));
        }
        // The VERSUS roster's removal (53002) and the match lobby belong to
        // the matchmaker's port.
        let Ok(Some(account)) = self.lobby.accounts.by_id(account_lo) else {
            return Ok(());
        };
        let id = self.state.party_of(&account)?;
        self.notify_party(id)
    }
}

// The calls other lobby ports make: the matchmaker (party_view,
// notify_party, leave_party, game_left) and the settings and live push
// (social_settings_saved, party_messages).

/// The account's party, as the matchmaker sees it.
#[expect(dead_code, reason = "the matchmaker's port reads parties")]
pub(crate) struct PartyView {
    pub id: PartyId,
    pub leader: u32,
    pub members: Vec<Arc<Account>>,
}

/// The account's party, a party of one on first use.
#[expect(dead_code, reason = "the matchmaker's port reads parties")]
pub(crate) fn party_view(lobby: &Lobby, account: &Arc<Account>) -> io::Result<PartyView> {
    let mut state = lobby.social.lock();
    let id = state.party_of(account)?;
    let party = state.party(id).expect("the account's party");
    Ok(PartyView {
        id,
        leader: party.leader.account_lo,
        members: party.members.clone(),
    })
}

/// The account's party state and party chat change, for its own client
/// (the dashboard's live push).
pub(crate) fn party_messages(
    lobby: &Lobby,
    account: &Arc<Account>,
) -> io::Result<Vec<LobbyMessage>> {
    Turn::new(lobby)?.party_messages(account)
}

#[expect(dead_code, reason = "the matchmaker's port tells parties")]
pub(crate) fn notify_party(lobby: &Lobby, id: PartyId) -> io::Result<()> {
    let mut turn = Turn::new(lobby)?;
    turn.notify_party(id)?;
    turn.finish_posted();
    Ok(())
}

/// The account's player card changed: its party and its friends see the new
/// one (`ow174/lobby/handlers/gallery.py`).
pub(crate) fn card_changed(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let account = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let id = turn.state.party_of(&account)?;
    turn.notify_party(id)?;
    turn.notify_friends(&account)?;
    turn.finish(peer)
}

/// The Social options (22202 +0x10D) were saved: "Members can invite" lives
/// there, and the members' party state shows it (+0x94), so a leader's party
/// of several hears it (`ow174/lobby/handlers/settings.py`).
pub(crate) fn social_settings_saved(lobby: &Lobby, account: &Arc<Account>) -> io::Result<()> {
    let mut turn = Turn::new(lobby)?;
    let Some(id) = turn.state.party_id_of(account.account_lo) else {
        return Ok(());
    };
    let party = turn.state.party(id).expect("the account's party");
    if party.is_leader(account.account_lo) && party.members.len() > 1 {
        turn.notify_party(id)?;
    }
    turn.finish_posted();
    Ok(())
}

/// Out of the party, as 22107 does; nothing for a party of one
/// (`ow174/lobby/handlers/leaving.py` leave_party). True when it left.
#[expect(dead_code, reason = "the leaving port (52902, 21801) leaves parties")]
pub(crate) fn leave_party(lobby: &Lobby, account: &Arc<Account>) -> io::Result<bool> {
    let mut turn = Turn::new(lobby)?;
    let id = turn.state.party_of(account)?;
    if turn
        .state
        .party(id)
        .is_some_and(|party| party.members.len() < 2)
    {
        return Ok(false);
    }
    turn.state.leave(account.account_lo);
    turn.notify_party(id)?;
    for message in turn.party_messages(account)? {
        turn.to(account.account_lo, message);
    }
    turn.finish_posted();
    Ok(true)
}

/// The social half of a match ending for a player; see [`Turn::game_left`].
#[expect(dead_code, reason = "the matchmaker's port sees games leave")]
pub(crate) fn game_left(lobby: &Lobby, account_lo: u32, match_id: PartyId) -> io::Result<()> {
    let mut turn = Turn::new(lobby)?;
    turn.game_left(account_lo, match_id)?;
    turn.finish_posted();
    Ok(())
}

/// A session of the account logged in: it is online, its per-session state
/// starts afresh, and its login carries its party state (with the party chat
/// channel when in a group).
pub(crate) fn session_started(
    lobby: &Lobby,
    account: &Arc<Account>,
) -> io::Result<Vec<LobbyMessage>> {
    let mut turn = Turn::new(lobby)?;
    let now = turn.content.now;
    turn.state.connect(account, now);
    turn.party_messages(account)
}

/// The friends list (27100) and the online friends' cards (20809) of the
/// login.
pub(crate) fn login_friends(lobby: &Lobby, account: &Account) -> io::Result<[LobbyMessage; 2]> {
    let turn = Turn::new(lobby)?;
    Ok([
        turn.content
            .friends_list(&turn.state, &lobby.accounts, account)?,
        turn.content
            .friend_cards(&turn.state, &lobby.accounts, account)?,
    ])
}

/// The account's last session ended: offline, out of its party, and its
/// friends see it go (`session.py` _cleanup; the lobby saved `last_online`
/// before, which the friends' presence shows).
pub(crate) fn session_ended(lobby: &Lobby, account: &Arc<Account>) -> io::Result<()> {
    let mut turn = Turn::new(lobby)?;
    turn.state.disconnect(account.account_lo);
    if let Some(id) = turn.state.leave(account.account_lo) {
        turn.notify_party(id)?;
    }
    turn.notify_friends(account)?;
    turn.finish_posted();
    Ok(())
}
