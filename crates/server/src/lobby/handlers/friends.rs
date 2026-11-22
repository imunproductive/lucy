//! Friends and presence: requests (27000-27002), the status dropdown (27011),
//! friends told of logins and logouts, and offline friends' presence sent
//! again after login. Ports `ow174/lobby/handlers/friends.py` and the
//! presence parts of `ow174/lobby/handlers/login.py`.

use std::{io, sync::Arc};

use lucy_lobby::{
    accounts::Account,
    presence::{STATUS_OFFLINE, STATUS_ONLINE},
    social::{
        FriendRequest, accept_friend, account_lo_of, decline_friend, is_friend,
        messages::{
            self, ALREADY_FRIENDS_TEXT, ANSWER_REQUEST, BAD_NAME_TEXT, FAILED, FRIENDS_OUT, OK,
            REMOVE_FRIEND, SEND_REQUEST, SENT_TEXT, SET_STATUS,
        },
        remove_friend, request_friend, text, unsigned,
    },
};
use lucy_wire::jam::codec::Fields;

use crate::lobby::{
    Lobby,
    peer::Peer,
    router::{Hooks, Module, Route},
    social::{self, Turn},
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        Route {
            group: FRIENDS_OUT,
            message: SEND_REQUEST,
            handle: send_request,
        },
        Route {
            group: FRIENDS_OUT,
            message: ANSWER_REQUEST,
            handle: answer_request,
        },
        Route {
            group: FRIENDS_OUT,
            message: REMOVE_FRIEND,
            handle: remove,
        },
        Route {
            group: FRIENDS_OUT,
            message: SET_STATUS,
            handle: set_status,
        },
    ],
    hooks: Hooks {
        login: Some(logged_in),
        tick: Some(resend_offline_presence),
        logout: Some(logged_out),
    },
};

/// The account's online friends see it come (`login.py`: notify_friends).
fn logged_in(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let account = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    turn.notify_friends(&account)?;
    turn.finish(peer)
}

/// Offline friends shown during login get an empty "offline ()" time until
/// their presence comes again, and the client only moves that time when it
/// arrives: it goes again at +3 s, +7 s, then every 60 s (`login.py`
/// _after_menu_ready).
fn resend_offline_presence(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let account = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let now = turn.content.now;
    if !turn.state.presence_due(account.account_lo, now) {
        return Ok(());
    }
    let offline = turn
        .content
        .offline_presence(&turn.state, &peer.lobby.accounts, &account)?;
    if !offline.is_empty() {
        turn.to(account.account_lo, messages::presence(offline));
    }
    turn.finish(peer)
}

fn logged_out(lobby: &Lobby, account: &Arc<Account>) {
    let _ = social::session_ended(lobby, account);
}

fn send_request(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let token = unsigned(fields.get(&0x78));
    let tag = text(fields.get(&0x88)).trim().to_owned();
    let mut turn = Turn::new(peer.lobby)?;
    let (target, outcome) = request_friend(&peer.lobby.accounts, &me, &tag)?;
    tracing::info!("[friends] Request {token} to {tag:?}: {outcome:?}");
    let shown = match outcome {
        FriendRequest::Sent | FriendRequest::Pending | FriendRequest::Added => SENT_TEXT,
        FriendRequest::Already => ALREADY_FRIENDS_TEXT,
        FriendRequest::Unknown | FriendRequest::ToSelf => BAD_NAME_TEXT,
    };
    turn.to(me.account_lo, messages::request_result(token, shown));
    match (outcome, target) {
        (FriendRequest::Sent, Some(target)) => {
            if turn.has_session(target.account_lo) {
                let request = turn.content.request_record(&target, &me);
                turn.to(target.account_lo, messages::request_received(request));
            }
        }
        (FriendRequest::Added, Some(target)) => {
            turn.to(
                me.account_lo,
                messages::request_removed(target.account_id_fields()),
            );
            befriend(&mut turn, &me, &target)?;
        }
        _ => {}
    }
    turn.finish(peer)
}

fn answer_request(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let token = unsigned(fields.get(&0x78));
    let accept = unsigned(fields.get(&0x90)) == 0;
    let mut turn = Turn::new(peer.lobby)?;
    let inviter = match account_lo_of(fields.get(&0x80)) {
        Some(account_lo) => peer.lobby.accounts.by_id(account_lo)?,
        None => None,
    };
    let Some(inviter) =
        inviter.filter(|inviter| me.profile().friend_requests.contains(&inviter.name))
    else {
        turn.to(me.account_lo, messages::answered(token, FAILED));
        return turn.finish(peer);
    };
    tracing::info!(
        "[friends] {} {}",
        if accept { "Accepted" } else { "Declined" },
        inviter.name
    );
    if accept {
        accept_friend(&me, &inviter)?;
    } else {
        decline_friend(&me, &inviter)?;
    }
    turn.to(me.account_lo, messages::answered(token, OK));
    turn.to(
        me.account_lo,
        messages::request_removed(inviter.account_id_fields()),
    );
    if accept {
        befriend(&mut turn, &me, &inviter)?;
    }
    turn.finish(peer)
}

fn remove(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let token = unsigned(fields.get(&0x78));
    let mut turn = Turn::new(peer.lobby)?;
    let friend = match account_lo_of(fields.get(&0x80)) {
        Some(account_lo) => peer.lobby.accounts.by_id(account_lo)?,
        None => None,
    };
    let friend = match friend {
        Some(friend) if !friend.is_virtual() && is_friend(&peer.lobby.accounts, &me, &friend)? => {
            friend
        }
        _ => {
            turn.to(me.account_lo, messages::removed(token, FAILED));
            return turn.finish(peer);
        }
    };
    remove_friend(&me, &friend)?;
    tracing::info!("[friends] Removed {}", friend.name);
    turn.to(me.account_lo, messages::removed(token, OK));
    turn.to(
        me.account_lo,
        messages::friend_removed(friend.account_id_fields()),
    );
    if turn.has_session(friend.account_lo) {
        turn.to(
            friend.account_lo,
            messages::friend_removed(me.account_id_fields()),
        );
    }
    turn.finish(peer)
}

/// The status dropdown: remember it and send the presence again, so the
/// player's own client and their friends show it. The party panel tiles read
/// the status only when a party state arrives (0x7FF7898FD14E), so the party
/// state goes too.
fn set_status(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let status = unsigned(fields.get(&0x78));
    let Some(status) = u8::try_from(status)
        .ok()
        .filter(|status| (STATUS_ONLINE..=STATUS_OFFLINE).contains(status))
    else {
        return Ok(());
    };
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    me.set_status(status);
    tracing::info!("[friends] Status -> {status}");
    turn.notify_presence(&me)?;
    let party = turn.state.party_of(&me)?;
    turn.notify_party(party)?;
    turn.finish(peer)
}

/// Show two new friends each other, the other one only when online: the list
/// entry, the presence and the friends' cards.
fn befriend(turn: &mut Turn<'_>, me: &Arc<Account>, other: &Arc<Account>) -> io::Result<()> {
    for (to, friend) in [(me, other), (other, me)] {
        if !turn.has_session(to.account_lo) {
            continue;
        }
        let entry = messages::friend_entry(friend.account_id_fields());
        let presence = turn.content.presence(&turn.state, friend)?;
        let cards = turn
            .content
            .friend_cards(&turn.state, &turn.lobby.accounts, to)?;
        turn.to(to.account_lo, messages::friend_added(entry));
        turn.to(to.account_lo, messages::presence(presence));
        turn.to(to.account_lo, cards);
    }
    Ok(())
}
