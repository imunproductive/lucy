//! Matches the game service saw end by their rules, into the players'
//! profiles. Python reports no match results (`ow174/game` keeps none), so
//! this is the Rust server's own: each seated player gets the match in his
//! queue's counts (`matches`, `wins`, `draws`: `ranked.rs` `matches_of`,
//! `wins_of`, `draws_of`), experience (with his medals'), levels and boxes, his competitive
//! rating and points, and the match in his leave record, by the public
//! rules (`orpheus_lobby::progression`). A player who left before the end
//! gets the match as a loss and a leave (rules 87, 88), unless the server
//! took him out (the dashboard, a queue match found). Each profile saved
//! goes to its connected game through the dashboard's live push
//! (`Services::push_profile`).

use std::{io, sync::Arc, time::Instant};

use orpheus_lobby::{
    accounts::{Account, Profile},
    clock::server_time,
    progression::{self, Earned, MatchPlay},
    queue,
    ranked::{DAMAGE, Queue, SUPPORT, Season, TANK},
};

use super::Room;
use crate::game_service::{MatchResult, Outcome};

/// What the result gives the room's seated players, in slot order: those
/// who finished it, and those who left it while it ran. A result's slot is
/// the room's seat order; a slot without a seat, or whose seat holds
/// another account, gets nothing, and so does the leave of an account that
/// also finished the match from another seat. A seat that never connected,
/// or that the server took out, is not a leave. `versus_ai` is the room
/// card's queue rules' Practice vs AI (`QueueRules::versus_ai`), which rule
/// 125 gives 90% experience; its AI heroes hold no seats and get nothing.
/// `season_of` is the competitive season the profile is in; a seat's time
/// in the match runs from its game's connection to `now`.
pub(super) fn recorded(
    room: &Room,
    result: &MatchResult,
    versus_ai: bool,
    season_of: impl Fn(&Profile) -> Season,
    now: Instant,
) -> Vec<(Arc<Account>, MatchPlay)> {
    let seated = |slot: usize, account: [u64; 2]| {
        room.seats
            .get(slot)
            .filter(|seat| account[0] == u64::from(seat.account.account_lo))
    };
    let finished = |account: [u64; 2]| {
        result.players.iter().any(|player| {
            player.finished && player.account == account && seated(player.slot, account).is_some()
        })
    };
    result
        .players
        .iter()
        .filter_map(|player| {
            let seat = seated(player.slot, player.account)?;
            let left = !player.finished;
            if left && (seat.joined_at.is_none() || seat.excused || finished(player.account)) {
                return None;
            }
            let season = season_of(&seat.account.profile());
            let group = seat.party != [0; 2]
                && room.seats.iter().any(|other| {
                    other.party == seat.party && other.account.account_lo != seat.account.account_lo
                });
            let play = MatchPlay {
                queue: queue_of(season, room.card, seat.role),
                outcome: match player.outcome {
                    Outcome::Win => progression::Outcome::Win,
                    Outcome::Loss => progression::Outcome::Loss,
                    Outcome::Draw => progression::Outcome::Draw,
                },
                finished: player.finished,
                competitive: room.ranked,
                versus_ai,
                group,
                seconds: seat
                    .joined_at
                    .map_or(0.0, |joined| now.duration_since(joined).as_secs_f64()),
                medals: player.medals,
            };
            Some((Arc::clone(&seat.account), play))
        })
        .collect()
}

/// The match into the account's profile, saved as other profile changes
/// are. `wall_time` is seconds since 1970; the day it falls on is the
/// lobby's (`clock::server_time`).
pub(super) fn save(account: &Account, play: &MatchPlay, wall_time: u64) -> io::Result<Earned> {
    account
        .edit_profile(|profile| {
            let now = server_time(&profile.server_date, wall_time)?;
            progression::apply(profile, play, now).map(Some)
        })
        .map(Option::unwrap_or_default)
}

/// The queue a seat counts in: the competitive queue of the card and role
/// where the season has one (`Season::queue`), else the role played, or the
/// open queue for a match without roles.
fn queue_of(season: Season, card: u64, role: u8) -> Queue {
    let guid = queue::role_guid(role);
    season.queue(card, guid.unwrap_or(0)).unwrap_or(match guid {
        Some(TANK) => Queue::Tank,
        Some(DAMAGE) => Queue::Damage,
        Some(SUPPORT) => Queue::Support,
        _ => Queue::Open,
    })
}
