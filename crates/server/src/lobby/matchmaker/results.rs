//! Matches the game service saw end by their rules, into the players'
//! profiles. Python reports no match results (`ow174/game` keeps none), so
//! this is the Rust server's own: each player who was still in the match at
//! its end gets one more match in his queue, and one more win or draw, in
//! the profile keys the career and competitive screens read (`matches`,
//! `wins`: `ranked.rs` `matches_of`, `wins_of`; `draws` is new). A player
//! who left before the end gets nothing. The lobby keeps no XP, only the
//! level, so a result gives none.

use std::{io, sync::Arc};

use orpheus_lobby::{
    accounts::{Account, Profile},
    queue,
    ranked::{DAMAGE, MAX_MATCHES, Queue, SUPPORT, Season, TANK, draws_of, matches_of, wins_of},
};
use serde_json::Value;

use super::Room;
use crate::game_service::{MatchResult, Outcome};

/// What the result gives the room's seated players who finished the match:
/// the queue each counts in and how it went for him, in slot order. A
/// result's slot is the room's seat order; a slot without a seat, or whose
/// seat holds another account, gets nothing. `season_of` is the competitive
/// season the profile is in.
pub(super) fn recorded(
    room: &Room,
    result: &MatchResult,
    season_of: impl Fn(&Profile) -> Season,
) -> Vec<(Arc<Account>, Queue, Outcome)> {
    result
        .players
        .iter()
        .filter(|player| player.finished)
        .filter_map(|player| {
            let seat = room.seats.get(player.slot)?;
            if player.account[0] != u64::from(seat.account.account_lo) {
                return None;
            }
            let season = season_of(&seat.account.profile());
            let queue = queue_of(season, room.card, seat.role);
            Some((Arc::clone(&seat.account), queue, player.outcome))
        })
        .collect()
}

/// The result into the account's profile, saved as other profile changes
/// are.
pub(super) fn save(account: &Account, queue: Queue, outcome: Outcome) -> io::Result<()> {
    account
        .edit_profile(|profile| count(profile, queue, outcome).map(Some))
        .map(drop)
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

/// One more match in the queue, and one more win or draw. Counts go on from
/// what the lobby reads (a queue without a saved count reads 25 matches),
/// and stop at the most the competitive screen shows.
fn count(profile: &mut Profile, queue: Queue, outcome: Outcome) -> io::Result<()> {
    let matches = (i64::from(matches_of(profile, queue)?) + 1).min(MAX_MATCHES);
    let more = match outcome {
        Outcome::Win => Some((wins_of(profile, queue)?, &mut profile.wins)),
        Outcome::Draw => Some((draws_of(profile, queue)?, &mut profile.draws)),
        Outcome::Loss => None,
    };
    if let Some((done, counts)) = more {
        let done = (i64::from(done) + 1).min(matches);
        counts.insert(queue.name().into(), Value::from(done));
    }
    profile
        .matches
        .insert(queue.name().into(), Value::from(matches));
    Ok(())
}
