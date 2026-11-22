//! The group finder (52200-52205): searching, listing and closing a group,
//! the members' slot types and the groups a player watches. Ports the group
//! finder part of `ow174/lobby/handlers/matchmaking.py`. The client only
//! searches once it has latency data, which the empty data center list sent
//! at login (35500) gives it: with no data centers the client counts its
//! latency as known (0x7FF789D4B1A0).

use std::{io, sync::Arc};

use lucy_lobby::social::{
    ANY_SLOT, DAMAGE_SLOT, array, id_of, matches_filters,
    messages::{
        self, CLOSE_GROUP, FIND_GROUPS, GROUP_FINDER, GROUP_ROLES, LIST_GROUP, UNWATCH_GROUPS,
        WATCH_GROUPS,
    },
    structure, unsigned,
};
use lucy_wire::jam::codec::Fields;

use crate::lobby::{
    peer::Peer,
    router::{Handler, Hooks, Module, Route},
    social::Turn,
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        route(FIND_GROUPS, find_groups),
        route(LIST_GROUP, list_group),
        route(CLOSE_GROUP, close_group),
        route(GROUP_ROLES, group_roles),
        route(WATCH_GROUPS, watch_groups),
        route(UNWATCH_GROUPS, unwatch_groups),
    ],
    hooks: Hooks::NONE,
};

const fn route(message: u32, handle: Handler) -> Route {
    Route {
        group: GROUP_FINDER,
        message,
        handle,
    }
}

/// The open groups that pass the search filters, the player's own included:
/// the client turns Join off for it (0x7FF78934B1D0 wants another party id).
fn find_groups(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let empty = Fields::new();
    let wanted = structure(fields.get(&0x78)).unwrap_or(&empty);
    let mut turn = Turn::new(peer.lobby)?;
    let mine = turn.state.party_of(&me)?;
    let mut groups = Vec::new();
    for id in turn.state.listed_groups() {
        let party = turn.state.party(id).expect("a listed group");
        if matches_filters(party, wanted) && (id == mine || party.can_join(&me)?) {
            groups.push(turn.content.group(party)?);
        }
    }
    drop(turn);
    peer.send(&messages::groups_found(groups))?;
    Ok(())
}

/// Groups the player picked in the group finder. The client keeps them in LFG
/// +0x240 (0x7FF78953C070), renews them every 2 minutes and drops them on a
/// new search (52205).
fn watch_groups(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let mut state = peer.lobby.social.lock();
    if let Some(session) = state.session_mut(peer.login.account.account_lo) {
        for id in array(fields.get(&0x78)) {
            if let Some(id) = id_of(Some(id)) {
                session.watched_groups.insert(id);
            }
        }
    }
    Ok(())
}

fn unwatch_groups(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let mut state = peer.lobby.social.lock();
    if let Some(session) = state.session_mut(peer.login.account.account_lo) {
        for id in array(fields.get(&0x78)) {
            if let Some(id) = id_of(Some(id)) {
                session.watched_groups.remove(&id);
            }
        }
    }
    Ok(())
}

/// The leader lists the group (or updates the listing). 52301 is not an
/// answer to this: the client only uses it to refresh a group it already
/// found (0x7FF78953D0A0), so the leader gets the party state with the
/// listing. Creating a group sends 52201 and, one tick later, the leader's
/// slot types (52203; the client sets both flags together, 0x7FF7899AD830);
/// the first party state waits for them, because a listing without the
/// leader's slot opens "choose role(s)", which does not close by itself when
/// the slot arrives.
fn list_group(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let id = turn.state.party_of(&me)?;
    let party = turn.state.party_mut(id).expect("the player's party");
    let created = party.listing.is_none();
    party.listing = Some(structure(fields.get(&0x78)).cloned().unwrap_or_default());
    party.searching = true;
    tracing::info!("[group] listed (created: {created})");
    if !created {
        turn.notify_party(id)?;
    }
    turn.finish(peer)
}

/// The leader stops looking for players ("Done" on the group panel). A group
/// with players in it keeps its listing, so the panel offers "Find more
/// players"; a leader who is alone leaves the group finder. Players who
/// picked the group hear it closed.
fn close_group(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let id = turn.state.party_of(&me)?;
    let party = turn.state.party_mut(id).expect("the player's party");
    if party.listing.is_none() {
        return Ok(());
    }
    party.searching = false;
    tracing::info!("[group] search stopped");
    if party.members.len() == 1 {
        party.listing = None;
        party.slot_types.clear();
    }
    turn.notify_party(id)?;
    turn.finish(peer)
}

/// The slot types a member of a listed group takes, from the slots picked on
/// the "choose role(s)" screen. The client sends an empty list right after
/// creating a group; the screen stays until the party state gives the member
/// a slot.
fn group_roles(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let mut slot_types = Vec::new();
    for slot in array(fields.get(&0x78)) {
        let slot = unsigned(Some(slot));
        if (ANY_SLOT..=DAMAGE_SLOT).contains(&slot) && !slot_types.contains(&slot) {
            slot_types.push(slot);
        }
    }
    let mut turn = Turn::new(peer.lobby)?;
    let id = turn.state.party_of(&me)?;
    let party = turn.state.party_mut(id).expect("the player's party");
    party.slot_types.insert(me.account_lo, slot_types);
    if party.listing.is_some() {
        party.update_search();
        turn.notify_party(id)?;
    }
    turn.finish(peer)
}
