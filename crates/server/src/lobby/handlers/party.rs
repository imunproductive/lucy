//! Parties: invites and their answers, joining and merging from the group
//! finder, leader transfer, kicks and leaving (22102-22111), and invites that
//! nobody answered in time. Ports `ow174/lobby/handlers/party.py`. The client
//! sends 22102 whether or not the leader lets members invite (0x7FF7897652D0
//! checks only a full party).

use std::{io, sync::Arc};

use orpheus_lobby::{
    accounts::Account,
    social::{
        MAX_PARTY_SIZE, PartyId, account_lo_of, id_of,
        messages::{
            self, ACCEPT_INVITE, CANNOT_SUGGEST, DECLINE_INVITE, INVITE_GROUP, INVITE_PLAYER,
            JOIN_GROUP, KICK, LEAVE, MAKE_LEADER, MERGE_ACCEPT, MERGE_DECLINE, PARTY_OUT,
        },
    },
};
use orpheus_wire::jam::codec::Fields;

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
    social::Turn,
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        route(INVITE_PLAYER, invite),
        route(JOIN_GROUP, join_group),
        route(INVITE_GROUP, invite_group),
        route(KICK, kick),
        route(MAKE_LEADER, make_leader),
        route(LEAVE, leave),
        route(ACCEPT_INVITE, accept_invite),
        route(DECLINE_INVITE, decline_invite),
        route(MERGE_ACCEPT, accept_merge),
        route(MERGE_DECLINE, decline_merge),
    ],
    hooks: Hooks {
        login: None,
        tick: Some(expire_invites),
        logout: None,
    },
};

const fn route(message: u32, handle: crate::lobby::router::Handler) -> Route {
    Route {
        group: PARTY_OUT,
        message,
        handle,
    }
}

const BOT_GREETING: &str = "Hi! I'm in the group.";

/// The account whose id a party message carries at +0x78.
fn account_at(peer: &Peer<'_, '_>, fields: &Fields) -> io::Result<Option<Arc<Account>>> {
    match account_lo_of(fields.get(&0x78)) {
        Some(account_lo) => peer.lobby.accounts.by_id(account_lo),
        None => Ok(None),
    }
}

fn invite(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let Some(target) =
        account_at(peer, &fields)?.filter(|target| target.account_lo != me.account_lo)
    else {
        return Ok(());
    };
    let mut turn = Turn::new(peer.lobby)?;
    let mine = turn.state.party_of(&me)?;
    let party = turn.state.party(mine).expect("the inviter's party");
    if !party.may_invite(me.account_lo) {
        // The client sends 22102 whether or not the leader lets members
        // invite. A member who may not invite suggests the player to the
        // leader instead, who gets "X suggests Y for the group".
        let leader = party.leader.account_lo;
        if turn.has_session(leader) {
            let suggestion = messages::suggestion(turn.record(&me), turn.record(&target));
            turn.to(leader, suggestion);
        } else {
            turn.to(me.account_lo, messages::party_error(CANNOT_SUGGEST));
        }
        return turn.finish(peer);
    }
    let now = turn.content.now;
    let id = turn.state.invite(&me, &target, now)?;
    tracing::info!("[<<<] Party invite -> {}", target.name);
    if target.is_virtual() {
        // The bot accepts at once. A member without a group slot shows the
        // group finder icon on their party tile, so it takes a free one.
        turn.state.join(&target, id);
        if let Some(party) = turn.state.party_mut(id) {
            if party.listing.is_some() {
                party.take_free_slot(target.account_lo);
            }
        }
        turn.notify_party(id)?;
        if me.profile().bot_chat {
            let channel = turn
                .state
                .party(id)
                .expect("the bot's party")
                .chat_channel();
            let greeting = messages::chat_message(
                orpheus_wire::jam::codec::Value::Struct(channel),
                turn.record(&target),
                BOT_GREETING,
                0,
            );
            turn.to(me.account_lo, greeting);
        }
        return turn.finish(peer);
    }
    if turn.has_session(target.account_lo) {
        let inviter = turn.record(&me);
        turn.to(target.account_lo, messages::invite(inviter));
    }
    // The party panel shows the invitee as a pending tile until the answer
    // or the timeout.
    turn.notify_party(id)?;
    turn.finish(peer)
}

/// An invite nobody answered for 25 seconds goes, with its pending tile.
/// Whichever session ticks first drops it.
fn expire_invites(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let mut turn = Turn::new(peer.lobby)?;
    let now = turn.content.now;
    for id in turn.state.expire_invites(now) {
        turn.notify_party(id)?;
    }
    turn.finish(peer)
}

/// The player at +0x78 who invited or asked to merge, and that player's
/// party.
fn sender_party(turn: &Turn<'_>, inviter: Option<&Arc<Account>>) -> Option<PartyId> {
    inviter.and_then(|inviter| turn.state.party_id_of(inviter.account_lo))
}

/// Accept on the invite popup: join the inviter's party.
fn accept_invite(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let inviter = account_at(peer, &fields)?;
    let mut turn = Turn::new(peer.lobby)?;
    let Some(id) = sender_party(&turn, inviter.as_ref()) else {
        return Ok(());
    };
    let invited = turn.state.party(id).is_some_and(|party| {
        party
            .invites
            .iter()
            .any(|invite| invite.invitee.account_lo == me.account_lo)
    });
    if !invited {
        return Ok(());
    }
    turn.state.join(&me, id);
    tracing::info!(
        "[<<<] Joined {}'s party",
        inviter.map_or_else(String::new, |inviter| inviter.name.clone())
    );
    turn.notify_party(id)?;
    turn.finish(peer)
}

/// Decline on the invite popup: drop the invite and its pending tile.
fn decline_invite(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let inviter = account_at(peer, &fields)?;
    let mut turn = Turn::new(peer.lobby)?;
    let Some(id) = sender_party(&turn, inviter.as_ref()) else {
        return Ok(());
    };
    if turn.state.cancel_invite(id, me.account_lo).is_some() {
        turn.notify_party(id)?;
    }
    turn.finish(peer)
}

/// Join in the group finder: the group's party id in +0x88 and no inviter
/// (0x7FF78975FAB0). Refused outside the group's endorsement or rating
/// limits.
fn join_group(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let Some(group) = id_of(fields.get(&0x88)).and_then(|id| turn.state.party(id)) else {
        return Ok(());
    };
    if !group.is_open_group() || !group.can_join(&me)? {
        return Ok(());
    }
    let id = group.id;
    turn.state.join(&me, id);
    tracing::info!("[group] Joined a group from the group finder");
    turn.notify_party(id)?;
    turn.finish(peer)
}

/// Invite Group in the group finder: while the player's group is looking for
/// players, another listed group can be asked to join it whole
/// (0x7FF78975FB10, which checks nothing itself). The bot's group says yes at
/// once; a player's leader gets "X wants to merge groups" (20703).
fn invite_group(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let mine = turn.state.party_of(&me)?;
    let Some(target) = id_of(fields.get(&0x88)).filter(|&id| id != mine) else {
        return Ok(());
    };
    let Some(group) = turn.state.party(target) else {
        return Ok(());
    };
    let party = turn.state.party(mine).expect("the player's party");
    if party.members.len() + group.members.len() > MAX_PARTY_SIZE
        || !party.may_invite(me.account_lo)
    {
        return Ok(());
    }
    if group.members.iter().all(|member| member.is_virtual()) {
        turn.state.merge_into(target, mine);
        turn.notify_party(mine)?;
        // Left empty, so closed.
        turn.notify_watchers(target)?;
        return turn.finish(peer);
    }
    let leader = group.leader.account_lo;
    if !turn.has_session(leader) {
        return Ok(());
    }
    turn.state
        .party_mut(mine)
        .expect("the player's party")
        .merge_invites
        .insert(leader);
    let party = turn.state.party(mine).expect("the player's party");
    let request = messages::merge_request(turn.record(&me), turn.content.group(party)?);
    turn.to(leader, request);
    turn.finish(peer)
}

/// Yes on "X wants to merge groups": the player's party moves into X's group,
/// whose party id comes in +0x78.
fn accept_merge(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let Some(group) = id_of(fields.get(&0x78)) else {
        return Ok(());
    };
    let Some(asked) = turn
        .state
        .party_mut(group)
        .map(|party| party.merge_invites.remove(&me.account_lo))
    else {
        return Ok(());
    };
    if !asked {
        return Ok(());
    }
    let mine = turn.state.party_of(&me)?;
    let size = |id| turn.state.party(id).map_or(0, |party| party.members.len());
    if mine == group || size(mine) + size(group) > MAX_PARTY_SIZE {
        return Ok(());
    }
    turn.state.merge_into(mine, group);
    tracing::info!("[group] Merged into another group");
    turn.notify_party(group)?;
    // Left empty, so closed.
    turn.notify_watchers(mine)?;
    turn.finish(peer)
}

fn decline_merge(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = peer.login.account.account_lo;
    let mut state = peer.lobby.social.lock();
    if let Some(party) = id_of(fields.get(&0x78)).and_then(|id| state.party_mut(id)) {
        party.merge_invites.remove(&me);
    }
    Ok(())
}

/// The leader hands the party to another member (+0x78), who moves to the
/// front for the 20700 leader flag.
fn make_leader(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let target = account_at(peer, &fields)?;
    let mut turn = Turn::new(peer.lobby)?;
    let id = turn.state.party_of(&me)?;
    let Some(target) = target else {
        return Ok(());
    };
    if !turn.state.make_leader(id, me.account_lo, target.account_lo) {
        return Ok(());
    }
    turn.notify_party(id)?;
    turn.finish(peer)
}

/// Remove from group. The cross on a pending tile sends it too, with the
/// invitee's id (the client only checks that the sender leads,
/// 0x7FF789762FA0), and cancels that invite.
fn kick(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let target = account_at(peer, &fields)?;
    let mut turn = Turn::new(peer.lobby)?;
    let id = turn.state.party_of(&me)?;
    let Some(target) = target else {
        return Ok(());
    };
    if !turn
        .state
        .party(id)
        .is_some_and(|party| party.is_leader(me.account_lo))
    {
        return Ok(());
    }
    if let Some(pending) = turn.state.cancel_invite(id, target.account_lo) {
        turn.notify_party(id)?;
        if turn.has_session(target.account_lo) {
            let inviter = pending.inviter.account_id_fields();
            turn.to(target.account_lo, messages::invite_cancelled(inviter));
        }
        return turn.finish(peer);
    }
    if !turn
        .state
        .party(id)
        .is_some_and(|party| party.has_member(target.account_lo))
    {
        return Ok(());
    }
    turn.state.leave(target.account_lo);
    tracing::info!("[<<<] Kicked {}", target.name);
    turn.notify_party(id)?;
    if turn.has_session(target.account_lo) {
        for message in turn.party_messages(&target)? {
            turn.to(target.account_lo, message);
        }
    }
    turn.finish(peer)
}

fn leave(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    if let Some(id) = turn.state.leave(me.account_lo) {
        turn.notify_party(id)?;
    }
    tracing::info!("[<<<] Left the party");
    for message in turn.party_messages(&me)? {
        turn.to(me.account_lo, message);
    }
    turn.finish(peer)
}
