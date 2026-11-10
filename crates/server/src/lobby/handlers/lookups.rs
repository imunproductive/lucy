//! Lookups: career profiles (22206), the leaderboard (39100), player names
//! (58202) and the OWL poll (42600). Ports `ow174/lobby/handlers/lookups.py`.

use std::{collections::HashSet, io};

use orpheus_lobby::{
    clock::server_time,
    leaderboard::{self, LEADERBOARD_OUT, PAGE_REQUEST, Player},
    login::Identity,
    lookups::{self, NAME_QUERY, NAMES, OWL_MATCHES, OWL_POLL},
    mode_rules::{CareerContent, PROFILE_REQUEST, career_profile},
    saved_settings::SOCIAL_OUT,
};
use orpheus_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
    services::{leaderboard_players, top500_places, wall_now},
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        Route {
            group: NAME_QUERY,
            message: NAMES,
            handle: player_names,
        },
        Route {
            group: OWL_POLL,
            message: OWL_MATCHES,
            handle: owl_live_matches,
        },
        Route {
            group: SOCIAL_OUT,
            message: PROFILE_REQUEST,
            handle: career,
        },
        Route {
            group: LEADERBOARD_OUT,
            message: PAGE_REQUEST,
            handle: leaderboard,
        },
    ],
    hooks: Hooks::NONE,
};

fn player_names(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let names = lookups::player_names(&fields, &peer.lobby.accounts)?;
    peer.send(&names)?;
    Ok(())
}

fn owl_live_matches(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    peer.send(&lookups::no_owl_match())?;
    Ok(())
}

/// The career profile of the player or of another saved account (+0x88);
/// an unknown player only gets a profile status (39001).
fn career(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    let target = match fields.get(&0x88) {
        Some(Value::Struct(target)) => target.clone(),
        _ => Fields::new(),
    };
    let account_lo = match target.get(&0) {
        Some(Value::Unsigned(lo)) => u32::try_from(*lo).ok(),
        _ => Some(0),
    };
    let known = match account_lo {
        Some(lo) => lobby.accounts.by_id(lo)?,
        None => None,
    };
    let (account, identity, request_id) = match &known {
        Some(account) => (
            account,
            Identity::for_account(account.account_lo),
            match fields.get(&0x78) {
                Some(Value::Struct(id)) => Some(id),
                _ => None,
            },
        ),
        None => (&peer.login.account, peer.login.identity, None),
    };
    let wall_time = wall_now()?;
    // Only a known account gets a full profile, with its competitive cards.
    let places = match &known {
        Some(_) => top500_places(lobby, account)?,
        None => Default::default(),
    };
    let profile = account.profile().clone();
    let now = server_time(&profile.server_date, wall_time)?;
    let arcade = lobby
        .arcade
        .state(&profile, lobby.ranked.season(&profile), now)?;
    let messages = career_profile(
        &profile,
        identity,
        &target,
        request_id,
        CareerContent {
            collection: &lobby.collection,
            ranked: &lobby.ranked,
            event_cards: &arcade.competitive_cards,
            places: &places,
            now: wall_time,
        },
    )?;
    peer.send_all(&messages)?;
    match &known {
        Some(account) => tracing::info!("[>>>] Career profile for {}", account.name),
        None => tracing::info!("[>>>] Career profile for {account_lo:#x?}"),
    }
    Ok(())
}

/// A page of the Top 500 (+0x78 {key, page}) among all saved accounts.
fn leaderboard(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let lobby = peer.lobby;
    let request = match fields.get(&0x78) {
        Some(Value::Struct(request)) => request.clone(),
        _ => Fields::new(),
    };
    let players = leaderboard_players(&lobby.accounts)?;
    let account = &peer.login.account;
    let profile = account.profile().clone();
    let friends: HashSet<String> = profile
        .friends
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    let me = Player {
        name: account.name.clone(),
        profile,
        identity: peer.login.identity,
    };
    let page = leaderboard::page(
        &lobby.ranked,
        &lobby.collection,
        &request,
        &players,
        &me,
        &friends,
        wall_now()?,
    )?;
    peer.send(&page)?;
    let rows = match page.2.get(&0x78) {
        Some(Value::Struct(body)) => match body.get(&0) {
            Some(Value::Array(rows)) => rows.len(),
            _ => 0,
        },
        _ => 0,
    };
    let number = |offset| match request.get(&offset) {
        Some(Value::Unsigned(value)) => *value,
        _ => 0,
    };
    tracing::info!(
        "[>>>] Leaderboard 0x{:016X} page {}: {rows} rows",
        number(0),
        number(8)
    );
    Ok(())
}
