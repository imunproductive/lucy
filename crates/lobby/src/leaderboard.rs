//! The competitive leaderboard (Top 500): message 39000, the answer to 39100.
//!
//! Ports `ow174/content/leaderboard.py`. The client asks for a page by key and
//! page number, and keeps the answer for 15 s under that key and page, so the
//! answer repeats both. The Top 500 screen packs its filters into the key:
//!
//! bits 0-15 season, 24-31 role (0 before role queue, 1 damage, 2 tank, 3
//! support, 4 all roles), 32-47 friends only, 48-63 region (1 Americas, 2
//! Europe, 3 Asia; 5 with friends only).
//!
//! Open queue and event card (Competitive CTF, Lucio Cup) boards name a card
//! instead: card in bits 0-31, 2 in bits 40-47, region in bits 48-55
//! (0x00010200000001B4 is season 32's open queue in the Americas).
//!
//! An entry is a player summary, the same as 39002. The screen sorts the
//! entries by the f64 at +0xB8, highest first, and takes names and icons from
//! the client's player card cache.
//!
//! The game's own rules for the board (string 82C5.07C): the best 500 of each
//! region, SMS Protect on, 25 matches completed, any rating. Ours are
//! stricter, as Python's: SMS Protect on, a diamond rating (3000) and 25
//! matches won in that queue this season.

use std::{collections::HashSet, io};

use indexmap::IndexMap;

use crate::{
    accounts::Profile,
    collection::Collection,
    login::Identity,
    player_messages::summary_card,
    ranked::{
        CARD_BASE, COMPETITIVE_CTF, DAMAGE, LUCIO_CUP, LeaderboardPlaces, Queue, Ranked, SUPPORT,
        TANK, rating_of, tier, wins_of,
    },
    transport::LobbyMessage,
};
use lucy_content::catalog::game_region_number;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Float64, Signed, Struct, Unsigned},
};

pub const LEADERBOARD_OUT: u32 = 0x1f8a_43dd;
pub const PAGE_REQUEST: u32 = 39100;
/// 39000 leaderboard page, 39001 profile status, 39002 summary.
pub const PROFILES: u32 = 0xd555_0262;
pub const PAGE: u32 = 39000;

const TOP: usize = 500;
const RATING_NEEDED: u16 = 3000;
const WINS_NEEDED: u16 = 25;
const CARD_BOARD: u64 = 2;
/// The role numbers of the key, as the client's group check uses them
/// (`ROLE_NUMBERS`, `ow174/content/queue.py`).
const ROLE_NUMBERS: [(u64, u64); 3] = [(1, DAMAGE), (2, TANK), (3, SUPPORT)];

/// A saved account as the leaderboard ranks it.
pub struct Player {
    pub name: String,
    pub profile: Profile,
    pub identity: Identity,
}

pub fn on_board(profile: &Profile, queue: Queue) -> io::Result<bool> {
    Ok(profile.sms_protect
        && rating_of(profile, queue)? >= RATING_NEEDED
        && wins_of(profile, queue)? >= WINS_NEEDED)
}

/// The profile's place in each queue's board of its own region, for the
/// queues it is in the Top 500 of. `account_lo` names the player among
/// `players`.
pub fn places(
    account_lo: u32,
    profile: &Profile,
    players: &[Player],
) -> io::Result<LeaderboardPlaces> {
    let region = game_region_number(&profile.game_region);
    let rivals: Vec<&Player> = players
        .iter()
        .filter(|player| game_region_number(&player.profile.game_region) == region)
        .collect();
    let mut places = LeaderboardPlaces::new();
    for queue in Queue::ALL {
        let mut board = Vec::new();
        for &player in &rivals {
            if on_board(&player.profile, queue)? {
                board.push((rating_of(&player.profile, queue)?, player));
            }
        }
        board.sort_by(|(left, _), (right, _)| right.cmp(left));
        if let Some(place) = board
            .iter()
            .take(TOP)
            .position(|(_, player)| player.identity.account_lo == account_lo)
        {
            places.insert(queue, place as u16 + 1);
        }
    }
    Ok(places)
}

struct Filters {
    /// None: a season board.
    card: Option<u64>,
    season: u64,
    role: u64,
    /// 0: any.
    region: u64,
    friends_only: bool,
}

impl Filters {
    fn from_key(key: u64) -> Self {
        if (key >> 40) & 0xff == CARD_BOARD {
            return Self {
                card: Some(CARD_BASE | (key & 0xffff_ffff)),
                season: 0,
                role: 0,
                region: (key >> 48) & 0xff,
                friends_only: false,
            };
        }
        let friends_only = (key >> 32) & 0xffff != 0;
        Self {
            card: None,
            season: key & 0xffff,
            role: (key >> 24) & 0xff,
            region: if friends_only {
                0
            } else {
                (key >> 48) & 0xffff
            },
            friends_only,
        }
    }
}

/// The (card, role) ratings a request lists; only the profile's running
/// season has any.
fn boards(ranked: &Ranked, filters: &Filters, profile: &Profile) -> Vec<(u64, u64)> {
    let season = ranked.season(profile);
    if let Some(card) = filters.card {
        return if season.open_card == Some(card) || [COMPETITIVE_CTF, LUCIO_CUP].contains(&card) {
            vec![(card, 0)]
        } else {
            Vec::new()
        };
    }
    if filters.season != season.number {
        return Vec::new();
    }
    if !season.has_roles() {
        return vec![(season.card, 0)];
    }
    match ROLE_NUMBERS
        .iter()
        .find(|(number, _)| *number == filters.role)
    {
        Some(&(_, role)) => vec![(season.card, role)],
        None => ROLE_NUMBERS
            .iter()
            .map(|&(_, role)| (season.card, role))
            .collect(),
    }
}

/// 39000 for a 39100 request {key, page}: the players the filters keep, best
/// rating first. "All roles" lists a player once, by best role. `friends` are
/// the requester's friends, lower case.
pub fn page(
    ranked: &Ranked,
    collection: &Collection,
    request: &Fields,
    players: &[Player],
    me: &Player,
    friends: &HashSet<String>,
    now: u64,
) -> io::Result<LobbyMessage> {
    let key = unsigned(request.get(&0));
    let number = unsigned(request.get(&8));
    let filters = Filters::from_key(key);
    let season = ranked.season(&me.profile);
    // name -> (rating, card, role, player)
    let mut best: IndexMap<&str, (u16, u64, u64, &Player)> = IndexMap::new();
    for (card, role) in boards(ranked, &filters, &me.profile) {
        let Some(queue) = season.queue(card, role) else {
            continue;
        };
        for player in players {
            if filters.friends_only
                && player.name != me.name
                && !friends.contains(&player.name.to_lowercase())
            {
                continue;
            }
            if filters.region != 0
                && u64::from(game_region_number(&player.profile.game_region)) != filters.region
            {
                continue;
            }
            if !on_board(&player.profile, queue)? {
                continue;
            }
            let rating = rating_of(&player.profile, queue)?;
            if best
                .get(player.name.as_str())
                .is_none_or(|&(kept, ..)| rating > kept)
            {
                best.insert(&player.name, (rating, card, role, player));
            }
        }
    }
    let mut rows: Vec<_> = best.into_values().collect();
    rows.sort_by(|(left, ..), (right, ..)| right.cmp(left));
    let entries = rows
        .into_iter()
        .take(TOP)
        .zip(1..)
        .map(|((rating, card, role, player), place)| {
            Struct(entry(collection, player, card, role, rating, place, now))
        })
        .collect();
    Ok((
        PROFILES,
        PAGE,
        Fields::from([(
            0x78,
            Struct(Fields::from([
                (0, Array(entries)),
                (
                    0x18,
                    Struct(Fields::from([(0, Unsigned(key)), (8, Unsigned(number))])),
                ),
            ])),
        )]),
    ))
}

fn entry(
    collection: &Collection,
    player: &Player,
    card: u64,
    role: u64,
    rating: u16,
    place: u16,
    now: u64,
) -> Fields {
    let mut entry = summary_card(
        &player.profile,
        player.identity,
        collection.portrait_frame(&player.profile),
        now,
    );
    entry.insert(
        0x68,
        Struct(Fields::from([
            (0x00, Unsigned(card)),
            (0x08, Unsigned(role)),
            (0x10, Unsigned(u64::from(rating))),
            (0x12, Unsigned(u64::from(place))),
            (0x14, Unsigned(u64::from(tier(rating)))),
            (0x15, Unsigned(0)),
            (0x16, Bool(true)),
        ])),
    );
    entry.insert(0xb8, Float64(f64::from(rating)));
    entry
}

fn unsigned(value: Option<&Value>) -> u64 {
    match value {
        Some(Unsigned(value)) => *value,
        Some(Signed(value)) => *value as u64,
        _ => 0,
    }
}

#[cfg(test)]
mod tests;
