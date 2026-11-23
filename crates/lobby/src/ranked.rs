//! Competitive seasons and rating projections, from the install's season
//! cards (`data/competitive_seasons_174.json`, `lucy_assets` `seasons.rs`).
//!
//! Research behind the ranked state (36300):
//!
//! - The 1.68 retail capture holds season 32 (January to March 2022) on the
//!   role queue card 1B3 (tank, damage and support ratings), the open queue
//!   card 1B4, Lunar New Year's Competitive CTF (1C3) and 73 past seasons with
//!   their reward tiers. 1.74 added three fields to each rating record; they
//!   are sent as 0. An event's competitive Arcade card (Competitive CTF, the
//!   Summer Games' Lucio Cup 1AD) runs a season of its own while the Arcade
//!   shows it.
//! - Every season is its own 0C7 card. The season list (+0x98) gives its
//!   number (+0x118), and from season 23 an open queue card names it as its
//!   parent (+0xF0): season 1 is 0xDD, season 18 (the first with roles) 0xC2,
//!   season 32 0x1B3 with open queue 0x1B4.
//! - The client looks a rating up by card, role, platform and matchmaking
//!   pool. It asks for the PC platform and, when a PC player is in the group
//!   (player card +0x3C = 1), the PC pool; a console-only group asks for pool
//!   16020070. The capture's ratings have pool 0, which 1.74 never asks for,
//!   so every competitive card was locked with "group members' skill ratings
//!   differ too much".
//! - A rating record (36300 +0x80, 36302), as the rank icon and rating
//!   getters read it (0x7FF78934CFB0, 0x7FF789AB61F0, 0x7FF789930400): +0x18
//!   skill rating (0: none this season; below 500 shows as "<500"), +0x1A
//!   season high, +0x1C Top 500 place (the icon becomes the Top 500 one),
//!   +0x1E season high place, +0x20 the rating shown while placements run,
//!   +0x22 tier of +0x18, +0x23 tier of +0x1A, +0x28 placement results (1, 2
//!   or 3 each, drawn as different marks; their count is the matches played),
//!   +0x40 placements run (the first 5 matches).
//! - A party member carries the same in 20700 +0x68: +0x10 rating, +0x12
//!   place, +0x14 placement rating, +0x16 tier, +0x17 placements run, +0x18
//!   placement tier.
//! - The group check drops every role choice where a member has no rating for
//!   the card, then compares the highest and lowest rating with 1000 (500 at
//!   tier 5, 350 at tier 6); during placements it uses the placement rating
//!   and tier. One member without a rating on an event card greys it out for
//!   the whole group (game type list 0x7FF789AA3D80, check 0x7FF78934F3A0).
//! - The career profile lists a season only when it has placement results
//!   (0x7FF78934CA50). Retail sent one 36302 per card right after login,
//!   before the full state.

use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{self, BufReader, Read},
    path::Path,
};

use serde::Deserialize;
use serde_json::{Map, Value as Json};

use crate::{
    accounts::{Profile, saved_integer},
    clock::{DAY, from_stu_datetime, stu_datetime},
    progression::Outcome,
    transport::LobbyMessage,
};
use lucy_core::guid::parse_guid;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Float64, Signed, Struct, Unsigned},
};

pub const RANKED: u32 = 0x2372_a2e8;
pub const CARD_BASE: u64 = 0x0630_0000_0000_0000;
pub const ROLE_QUEUE: u64 = CARD_BASE | 0x1b3;
pub const OPEN_QUEUE: u64 = CARD_BASE | 0x1b4;
pub const COMPETITIVE_CTF: u64 = CARD_BASE | 0x1c3;
pub const LUCIO_CUP: u64 = CARD_BASE | 0x1ad;
pub const TANK: u64 = lucy_content::heroes::TANK_ROLE;
pub const DAMAGE: u64 = lucy_content::heroes::DAMAGE_ROLE;
pub const SUPPORT: u64 = lucy_content::heroes::SUPPORT_ROLE;
pub const PC_PLATFORM: u64 = 15_959_616;
pub const PC_POOL: u64 = 16_025_156;

const STATE: u32 = 36300;
const CARD_RATINGS: u32 = 36302;
pub const CURRENT_SEASON: u64 = 32;
pub const MAX_RATING: i64 = 5000;
pub const MAX_MATCHES: i64 = 9999;
const FIRST_ROLE_SEASON: u64 = 18;
const SEASON_DATES: [u32; 4] = [0x20, 0x28, 0x30, 0x38];
const DAYS_INTO_SEASON: u64 = 7;
/// Lowest rating of each tier from silver up (`TIER_FLOORS`,
/// `ow174/content/ranked.py`). The type table names their home
/// (`STUGenericSettings_Ranked.m_rankedTiers`, `STURankedTierDefinition`),
/// but the client install doesn't ship it: no `054` setting has that class
/// and a scan of the install's data assets found neither class, so the
/// floors stay Python's.
const TIER_FLOORS: [u16; 6] = [1500, 2000, 2500, 3000, 3500, 4000];
const PLACEMENT_MATCHES: u8 = 5;
const EVENT_CARDS: [u64; 2] = [COMPETITIVE_CTF, LUCIO_CUP];
/// A competitive win or loss moves the rating by this much. Rule 77: the
/// formula is not public; a fixed step is the server's [assumption].
const RATING_STEP: i64 = 25;
/// Rule 73: new players get larger swings over their first 10-20 matches:
/// twice the step over the queue's first 10 [assumption: the factor and the
/// lower end].
const NEW_PLAYER_MATCHES: u16 = 10;
/// Rule 87: a competitive leaver loses 50.
const LEAVER_RATING_LOSS: i64 = 50;
/// Rule 76: Silver to Diamond demote after 5 games under the tier's
/// minimum, Master and above at once.
const DEMOTION_GAMES: i64 = 5;
const PROTECTED_TIERS: std::ops::RangeInclusive<u8> = 1..=4;
/// Rule 81: competitive points, 15 a win, 5 a draw, none for a loss; they
/// stop at 6000 unspent.
const WIN_POINTS: i64 = 15;
const DRAW_POINTS: i64 = 5;
pub const MAX_COMP_POINTS: i64 = 6000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Queue {
    Tank,
    Damage,
    Support,
    Open,
    Ctf,
    Lucio,
}

impl Queue {
    pub const ALL: [Self; 6] = [
        Self::Tank,
        Self::Damage,
        Self::Support,
        Self::Open,
        Self::Ctf,
        Self::Lucio,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Tank => "tank",
            Self::Damage => "damage",
            Self::Support => "support",
            Self::Open => "open",
            Self::Ctf => "ctf",
            Self::Lucio => "lucio",
        }
    }
}

pub type LeaderboardPlaces = BTreeMap<Queue, u16>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Season {
    pub number: u64,
    pub card: u64,
    pub open_card: Option<u64>,
}

impl Season {
    pub fn has_roles(self) -> bool {
        self.number >= FIRST_ROLE_SEASON
    }

    pub fn queue(self, card: u64, role: u64) -> Option<Queue> {
        match (card, role) {
            (COMPETITIVE_CTF, 0) => Some(Queue::Ctf),
            (LUCIO_CUP, 0) => Some(Queue::Lucio),
            (_, 0) if self.open_card == Some(card) => Some(Queue::Open),
            _ if self.card != card => None,
            (_, 0) if !self.has_roles() => Some(Queue::Open),
            _ if self.has_roles() => match role {
                TANK => Some(Queue::Tank),
                DAMAGE => Some(Queue::Damage),
                SUPPORT => Some(Queue::Support),
                _ => None,
            },
            _ => None,
        }
    }
}

/// Values used by the match team list. An unplaced player has all zeroes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchRank {
    pub skill_rating: u16,
    pub top_500_place: u16,
    pub tier: u8,
}

struct Rating {
    skill: u16,
    tier: u8,
    place: u16,
    placements_played: u8,
}

impl Rating {
    /// A card the player has no rating on.
    fn unrated() -> Self {
        Self {
            skill: 0,
            tier: 0,
            place: 0,
            placements_played: 0,
        }
    }

    fn read(profile: &Profile, queue: Queue, places: &LeaderboardPlaces) -> io::Result<Self> {
        let skill = rating_of(profile, queue)?;
        let matches = matches_of(profile, queue)?;
        Ok(Self {
            skill,
            tier: tier_of(profile, queue)?,
            place: places.get(&queue).copied().unwrap_or(0),
            placements_played: matches.min(u16::from(PLACEMENT_MATCHES)) as u8,
        })
    }

    fn match_rank(&self) -> MatchRank {
        if self.placements_played < PLACEMENT_MATCHES {
            return MatchRank::default();
        }
        MatchRank {
            skill_rating: self.skill,
            top_500_place: self.place,
            tier: self.tier,
        }
    }

    fn fields(&self) -> Fields {
        let rank = self.match_rank();
        let result = Struct(Fields::from([(0, Unsigned(1))]));
        Fields::from([
            (0x18, Unsigned(u64::from(rank.skill_rating))),
            (0x1a, Unsigned(u64::from(rank.skill_rating))),
            (0x1c, Unsigned(u64::from(rank.top_500_place))),
            (0x1e, Unsigned(u64::from(rank.top_500_place))),
            (0x20, Unsigned(u64::from(self.skill))),
            (0x22, Unsigned(u64::from(rank.tier))),
            (0x23, Unsigned(u64::from(rank.tier))),
            (
                0x28,
                Array(vec![result; usize::from(self.placements_played)]),
            ),
            (0x40, Bool(self.placements_played < PLACEMENT_MATCHES)),
        ])
    }
}

pub fn tier(rating: u16) -> u8 {
    TIER_FLOORS.partition_point(|&floor| rating >= floor) as u8
}

/// The tier the rank icon shows: the rating's, or the tier above it while
/// a Silver to Diamond player is held there for his games under its minimum
/// (rule 76). A hold the rating has left behind (the dashboard set it
/// elsewhere) is not shown.
pub fn tier_of(profile: &Profile, queue: Queue) -> io::Result<u8> {
    let rated = tier(rating_of(profile, queue)?);
    Ok(match held_tier(profile, queue)? {
        Some((held, _)) if held == rated + 1 => held,
        _ => rated,
    })
}

/// The tier a player is held in, and his games under its minimum
/// (`held_tiers`).
fn held_tier(profile: &Profile, queue: Queue) -> io::Result<Option<(u8, i64)>> {
    let Some(held) = profile.held_tiers.get(queue.name()) else {
        return Ok(None);
    };
    let read = |key: &str| {
        held.get(key)
            .and_then(saved_integer)
            .ok_or_else(|| invalid(format!("invalid held tier for {}", queue.name())))
    };
    Ok(Some((
        u8::try_from(read("tier")?).unwrap_or(0),
        read("games")?,
    )))
}

/// A competitive match into the queue's rating (section 9), before the
/// match is counted. A win or a loss moves it by the step, twice that over
/// the queue's first matches (rule 73), a draw not at all (rule 73), and a
/// leaver loses 50 (rule 87); it stays within 1-5000 (rule 73). The tier
/// follows (rules 75, 76). Returns the change.
pub fn rate(profile: &mut Profile, queue: Queue, outcome: Outcome, left: bool) -> io::Result<i64> {
    let before = i64::from(rating_of(profile, queue)?);
    let shown = tier_of(profile, queue)?;
    let held = held_tier(profile, queue)?;
    let step = if matches_of(profile, queue)? < NEW_PLAYER_MATCHES {
        2 * RATING_STEP
    } else {
        RATING_STEP
    };
    let change = match outcome {
        _ if left => -LEAVER_RATING_LOSS,
        Outcome::Win => step,
        Outcome::Loss => -step,
        Outcome::Draw => 0,
    };
    let after = (before + change).clamp(1, MAX_RATING);
    profile
        .ratings
        .insert(queue.name().into(), Json::from(after));
    let rated = tier(after as u16);
    let games = match held {
        Some((tier, games)) if tier == shown => games + 1,
        _ => 1,
    };
    if rated < shown && PROTECTED_TIERS.contains(&shown) && games < DEMOTION_GAMES {
        profile.held_tiers.insert(
            queue.name().into(),
            serde_json::json!({"tier": shown, "games": games}),
        );
    } else {
        profile.held_tiers.remove(queue.name());
    }
    Ok(after - before)
}

/// Rule 81: the match's competitive points. Returns those given.
pub fn award_points(profile: &mut Profile, outcome: Outcome) -> i64 {
    let points = match outcome {
        Outcome::Win => WIN_POINTS,
        Outcome::Draw => DRAW_POINTS,
        Outcome::Loss => 0,
    };
    let before = profile.comp_points;
    if before < MAX_COMP_POINTS {
        profile.comp_points = (before + points).min(MAX_COMP_POINTS);
    }
    profile.comp_points - before
}

pub fn rating_of(profile: &Profile, queue: Queue) -> io::Result<u16> {
    Ok(profile_integer(&profile.ratings, queue, 2333)?.clamp(1, MAX_RATING) as u16)
}

pub fn matches_of(profile: &Profile, queue: Queue) -> io::Result<u16> {
    Ok(profile_integer(&profile.matches, queue, 25)?.clamp(0, MAX_MATCHES) as u16)
}

pub fn wins_of(profile: &Profile, queue: Queue) -> io::Result<u16> {
    Ok(
        profile_integer(&profile.wins, queue, 0)?.clamp(0, i64::from(matches_of(profile, queue)?))
            as u16,
    )
}

/// The queue's matches that ended in a draw; none for a profile without
/// them (Python keeps no draws).
pub fn draws_of(profile: &Profile, queue: Queue) -> io::Result<u16> {
    Ok(
        profile_integer(&profile.draws, queue, 0)?.clamp(0, i64::from(matches_of(profile, queue)?))
            as u16,
    )
}

fn profile_integer(values: &Map<String, Json>, queue: Queue, default: i64) -> io::Result<i64> {
    let Some(value) = values.get(queue.name()) else {
        return Ok(default);
    };
    saved_integer(value).ok_or_else(|| {
        invalid(format!(
            "invalid saved rating or match count for {}",
            queue.name()
        ))
    })
}

pub const SEASONS_FILE: &str = "competitive_seasons_174.json";

/// Each numbered season's card (`0C7`), by number: the card whose name
/// strings read "Season N" (`m_1E4E5957`) and that no other card names as
/// its own (an open queue card names its role queue season, `m_2593569C`).
const SEASON_CARDS: [u64; 36] = [
    0xdd, 0xde, 0xdf, 0xe0, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xeb, 0xec,
    0xc1, 0xc2, 0xc3, 0xc4, 0x11d, 0x11e, 0x11f, 0x120, 0x121, 0x19d, 0x19e, 0x19f, 0x1a0, 0x1a1,
    0x1a2, 0x1b3, 0x1b5, 0x1b7, 0x1b9, 0x1bb,
];
/// A role queue card's roles, in the order of its role slots (`0C7`
/// `m_66C923AF`: tank, support, damage), which is retail's order of the
/// card's ratings.
const ROLE_QUEUE_ROLES: [u64; 3] = [TANK, SUPPORT, DAMAGE];

/// A season card's record in the derived file.
#[derive(Deserialize)]
struct SeasonCard {
    tiers: Vec<Tier>,
    dates: [u64; 4],
    name: Json,
    title: Json,
    unlocks: Vec<String>,
    heroic_unlocks: Vec<String>,
    heroes: Vec<String>,
    maps_featured: Vec<String>,
    featured: Vec<String>,
    texture: Json,
    image: Json,
    maps: Vec<String>,
    parent: Option<String>,
    children: Vec<String>,
    count_a: u64,
    count_b: u64,
    flag: bool,
}

#[derive(Deserialize)]
struct Tier {
    lowest: u64,
    highest: u64,
    unlocks: Vec<String>,
}

pub struct Ranked {
    seasons: BTreeMap<u64, Season>,
    /// Every competitive season's record (36300 `+0x98`), by card, in card
    /// order.
    history: BTreeMap<u64, Fields>,
}

impl Ranked {
    pub fn load(data_dir: &Path) -> io::Result<Self> {
        let path = data_dir.join(SEASONS_FILE);
        let file = File::open(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        Self::from_reader(BufReader::new(file))
    }

    pub fn from_reader(reader: impl Read) -> io::Result<Self> {
        let cards: BTreeMap<String, SeasonCard> = serde_json::from_reader(reader)
            .map_err(|error| invalid(format!("invalid {SEASONS_FILE}: {error}")))?;
        let mut history = BTreeMap::new();
        let mut children = BTreeMap::new();
        for (key, card) in cards {
            let id = guid(&key)?;
            let number = SEASON_CARDS
                .iter()
                .position(|&index| CARD_BASE | index == id)
                .map_or(0, |position| position as u64 + 1);
            if let Some(child) = card.children.first() {
                children.insert(id, guid(child)?);
            }
            history.insert(id, season_record(id, number, &card)?);
        }
        let mut seasons = BTreeMap::new();
        for (number, &index) in (1..).zip(SEASON_CARDS.iter()) {
            let card = CARD_BASE | index;
            if history.contains_key(&card) {
                seasons.insert(
                    number,
                    Season {
                        number,
                        card,
                        open_card: children.get(&card).copied(),
                    },
                );
            }
        }
        if !seasons.contains_key(&CURRENT_SEASON) {
            return Err(invalid("the competitive seasons lack the current season"));
        }
        Ok(Self { seasons, history })
    }

    pub fn season(&self, profile: &Profile) -> Season {
        let number = u64::try_from(profile.season).unwrap_or(0);
        self.seasons
            .get(&number)
            .copied()
            .unwrap_or(self.seasons[&CURRENT_SEASON])
    }

    pub fn rank(
        &self,
        profile: &Profile,
        card: u64,
        role: u64,
        places: &LeaderboardPlaces,
    ) -> io::Result<Option<MatchRank>> {
        let season = self.season(profile);
        if card == 0
            || (card != season.card
                && season.open_card != Some(card)
                && !EVENT_CARDS.contains(&card))
        {
            return Ok(None);
        }
        let rank = match season.queue(card, role) {
            Some(queue) => Rating::read(profile, queue, places)?.match_rank(),
            None => MatchRank::default(),
        };
        Ok(Some(rank))
    }

    /// Ratings grouped by card, for the career profile and competitive
    /// state: the season's card (a rating per role from season 18), its
    /// open queue card and the event cards, a rating each.
    pub fn cards(
        &self,
        profile: &Profile,
        event_cards: &[u64],
        places: &LeaderboardPlaces,
    ) -> io::Result<Vec<Fields>> {
        let season = self.season(profile);
        let main: &[u64] = if season.has_roles() {
            &ROLE_QUEUE_ROLES
        } else {
            &[0]
        };
        let cards = [(season.card, main)]
            .into_iter()
            .chain(season.open_card.map(|card| (card, &[0][..])))
            .chain(event_cards.iter().map(|&card| (card, &[0][..])));
        let mut rated_cards = Vec::new();
        for (card_id, roles) in cards {
            let mut ratings = Vec::with_capacity(roles.len());
            for &role in roles {
                let mut rating = Fields::from([
                    (0x00, Unsigned(role)),
                    (0x08, Float64(0.0)),
                    (0x10, Unsigned(PC_PLATFORM)),
                    (0x14, Unsigned(PC_POOL)),
                ]);
                rating.extend(match season.queue(card_id, role) {
                    Some(queue) => Rating::read(profile, queue, places)?.fields(),
                    None => Rating::unrated().fields(),
                });
                ratings.push(Struct(rating));
            }
            let seen =
                i64::try_from(card_id).is_ok_and(|card| profile.seasons_seen.contains(&card));
            rated_cards.push(Fields::from([
                (0x00, Array(ratings)),
                (0x18, Unsigned(card_id)),
                (0x20, Float64(0.0)),
                (0x28, Unsigned(0)),
                (0x2c, Bool(seen)),
            ]));
        }
        Ok(rated_cards)
    }

    /// Emit 36302 for each rated card before the complete 36300 state.
    pub fn login_messages(
        &self,
        profile: &Profile,
        event_cards: &[u64],
        places: &LeaderboardPlaces,
        now: u64,
    ) -> io::Result<Vec<LobbyMessage>> {
        let season = self.season(profile);
        let cards = self.cards(profile, event_cards, places)?;
        let state = self.state(season, event_cards, now, &cards)?;
        let mut messages: Vec<_> = cards
            .into_iter()
            .map(|card| (RANKED, CARD_RATINGS, Fields::from([(0x78, Struct(card))])))
            .collect();
        messages.push((RANKED, STATE, state));
        Ok(messages)
    }

    /// Every competitive event appears in the party and match-roster ratings,
    /// even when it is absent from the player's current Arcade.
    pub fn party_ratings(
        &self,
        profile: &Profile,
        places: &LeaderboardPlaces,
    ) -> io::Result<Vec<Value>> {
        let cards = self.cards(profile, &EVENT_CARDS, places)?;
        cards.iter().map(party_card).collect()
    }

    /// 36300: the season's card, the ratings, every season up to the
    /// player's (later seasons and their open queue cards are left out), a
    /// timer for each season running now (the player's and its open queue,
    /// and the Arcade's competitive events) with its dates moved so that it
    /// began a week ago, and the season cards (`+0xC8`) but the running
    /// events [assumption: retail's list left out only the running event].
    fn state(
        &self,
        season: Season,
        event_cards: &[u64],
        now: u64,
        cards: &[Fields],
    ) -> io::Result<Fields> {
        let later: HashSet<_> = self
            .seasons
            .values()
            .filter(|entry| entry.number > season.number)
            .flat_map(|entry| [Some(entry.card), entry.open_card].into_iter().flatten())
            .collect();
        let running: HashSet<_> = [Some(season.card), season.open_card]
            .into_iter()
            .flatten()
            .chain(event_cards.iter().copied())
            .filter(|&card| card != 0)
            .collect();
        let mut history = Vec::new();
        let mut timers = Vec::new();
        let mut card_ids = Vec::new();
        for (&card, entry) in &self.history {
            if later.contains(&card) {
                continue;
            }
            let mut entry = entry.clone();
            if running.contains(&card) {
                timers.push(Struct(shift_season(&mut entry, card, now)?));
            }
            if !event_cards.contains(&card) {
                card_ids.push(Unsigned(card));
            }
            history.push(Struct(entry));
        }
        Ok(Fields::from([
            (0x78, Unsigned(season.card)),
            (
                0x80,
                Struct(Fields::from([(
                    0,
                    Array(cards.iter().cloned().map(Struct).collect()),
                )])),
            ),
            (0x98, Array(history)),
            (0xb0, Array(timers)),
            (0xc8, Array(card_ids)),
        ]))
    }
}

/// A running season's dates moved so that it began [`DAYS_INTO_SEASON`]
/// days before `now`, and its timer: the card, the seconds left and a flag
/// (set in every timer retail sent).
fn shift_season(entry: &mut Fields, card: u64, now: u64) -> io::Result<Fields> {
    let start = season_time(entry, 0x20)?;
    let shift = i128::from(now) - i128::from(DAYS_INTO_SEASON * DAY) - i128::from(start);
    for key in SEASON_DATES {
        let time = i128::from(season_time(entry, key)?) + shift;
        let time = u64::try_from(time).map_err(|_| invalid("ranked season date out of range"))?;
        entry.insert(
            key,
            Struct(Fields::from([(0, Unsigned(stu_datetime(time)?))])),
        );
    }
    let remaining = i128::from(season_time(entry, 0x30)?) - i128::from(now);
    Ok(Fields::from([
        (0x0, Unsigned(card)),
        (0x8, Float64(remaining as f64)),
        (0xc, Bool(true)),
    ]))
}

/// A season card's record (36300 `+0x98`), from the derived card.
fn season_record(card: u64, number: u64, source: &SeasonCard) -> io::Result<Fields> {
    let guids = |keys: &[String]| -> io::Result<Value> {
        keys.iter()
            .map(|key| guid(key).map(Unsigned))
            .collect::<io::Result<_>>()
            .map(Array)
    };
    let optional = |value: &Json| -> io::Result<Value> {
        Ok(Unsigned(match value {
            Json::String(key) => guid(key)?,
            _ => 0,
        }))
    };
    let tiers = source
        .tiers
        .iter()
        .map(|tier| {
            Ok(Struct(Fields::from([
                (0x00, guids(&tier.unlocks)?),
                (0x18, Unsigned(tier.lowest)),
                (0x1c, Unsigned(tier.highest)),
            ])))
        })
        .collect::<io::Result<_>>()?;
    let mut record = Fields::from([
        (0x00, Array(tiers)),
        (0x18, Unsigned(card)),
        (0x40, optional(&source.name)?),
        (0x48, optional(&source.title)?),
        (0x50, guids(&source.unlocks)?),
        (0x68, guids(&source.heroic_unlocks)?),
        (0x80, guids(&source.heroes)?),
        (0x98, guids(&source.maps_featured)?),
        (0xb0, optional(&source.texture)?),
        (0xb8, optional(&source.image)?),
        (0xc0, guids(&source.featured)?),
        (0xd8, guids(&source.maps)?),
        (
            0xf0,
            Unsigned(source.parent.as_deref().map(guid).transpose()?.unwrap_or(0)),
        ),
        (0xf8, guids(&source.children)?),
        (0x110, Unsigned(source.count_a)),
        (0x114, Unsigned(source.count_b)),
        (0x118, Unsigned(number)),
        (0x11c, Bool(source.flag)),
    ]);
    for (key, date) in SEASON_DATES.into_iter().zip(source.dates) {
        record.insert(key, Struct(Fields::from([(0, Unsigned(date))])));
    }
    Ok(record)
}

fn party_card(card: &Fields) -> io::Result<Value> {
    let mut ratings = Vec::new();
    for rating in array(card, 0)? {
        let rating = structure(rating)?;
        let mut role = [
            (0, 0),
            (8, 0x10),
            (0xc, 0x14),
            (0x10, 0x18),
            (0x12, 0x1c),
            (0x14, 0x20),
            (0x16, 0x22),
            (0x17, 0x40),
        ]
        .into_iter()
        .map(|(destination, source)| Ok((destination, field(rating, source)?.clone())))
        .collect::<io::Result<Fields>>()?;
        let placement = u16::try_from(integer(rating, 0x20)?)
            .map_err(|_| invalid("invalid ranked placement rating"))?;
        role.insert(0x18, Unsigned(u64::from(tier(placement))));
        ratings.push(Struct(role));
    }
    Ok(Struct(Fields::from([
        (0, Array(ratings)),
        (0x18, field(card, 0x18)?.clone()),
        (0x20, Unsigned(0)),
    ])))
}

fn season_time(entry: &Fields, offset: u32) -> io::Result<u64> {
    from_stu_datetime(integer(structure(field(entry, offset)?)?, 0)?)
}

fn guid(key: &str) -> io::Result<u64> {
    parse_guid(key).ok_or_else(|| invalid(format!("{SEASONS_FILE}: invalid GUID {key:?}")))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn missing(offset: u32) -> io::Error {
    invalid(format!("missing ranked field +0x{offset:X}"))
}

fn field(fields: &Fields, offset: u32) -> io::Result<&Value> {
    fields.get(&offset).ok_or_else(|| missing(offset))
}

fn structure(value: &Value) -> io::Result<&Fields> {
    match value {
        Struct(fields) => Ok(fields),
        _ => Err(invalid("expected a ranked record")),
    }
}

fn array(fields: &Fields, offset: u32) -> io::Result<&[Value]> {
    match field(fields, offset)? {
        Array(values) => Ok(values),
        _ => Err(invalid(format!("expected a ranked array at +0x{offset:X}"))),
    }
}

fn unsigned(value: &Value) -> io::Result<u64> {
    match value {
        Unsigned(value) => Ok(*value),
        Signed(value) => {
            u64::try_from(*value).map_err(|_| invalid("expected unsigned ranked field"))
        }
        _ => Err(invalid("expected a ranked integer")),
    }
}

fn integer(fields: &Fields, offset: u32) -> io::Result<u64> {
    unsigned(field(fields, offset)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::{party::PARTY, test_catalog};
    use lucy_wire::jam::codec::Schemas;

    #[test]
    fn queue_selection_depends_on_season_role_and_event_card() {
        let role_season = Season {
            number: CURRENT_SEASON,
            card: ROLE_QUEUE,
            open_card: Some(OPEN_QUEUE),
        };
        assert_eq!(role_season.queue(ROLE_QUEUE, DAMAGE), Some(Queue::Damage));
        assert_eq!(role_season.queue(ROLE_QUEUE, 0), None);
        assert_eq!(role_season.queue(OPEN_QUEUE, SUPPORT), None);
        assert_eq!(role_season.queue(OPEN_QUEUE, 0), Some(Queue::Open));
        assert_eq!(role_season.queue(LUCIO_CUP, 0), Some(Queue::Lucio));
        assert_eq!(role_season.queue(COMPETITIVE_CTF, TANK), None);

        let early_season = Season {
            number: 10,
            card: CARD_BASE | 0xe6,
            open_card: None,
        };
        assert_eq!(early_season.queue(early_season.card, 0), Some(Queue::Open));
        assert_eq!(early_season.queue(early_season.card, SUPPORT), None);
        assert_eq!(early_season.queue(OPEN_QUEUE, 0), None);
    }

    #[test]
    fn placements_use_saved_skill_and_keep_match_rank_empty_until_fifth_match() {
        let mut profile = Profile::default();
        profile.ratings.insert("tank".into(), json!("4100"));
        profile.matches.insert("tank".into(), json!(4));
        profile.wins.insert("tank".into(), json!(10));
        let places = LeaderboardPlaces::from([(Queue::Tank, 7)]);
        assert_eq!(wins_of(&profile, Queue::Tank).unwrap(), 4);
        let rating = Rating::read(&profile, Queue::Tank, &places).unwrap();
        assert_eq!(rating.match_rank(), MatchRank::default());
        let fields = rating.fields();
        assert_eq!(fields[&0x20], Unsigned(4100));
        assert_eq!(fields[&0x40], Bool(true));
        assert_eq!(array(&fields, 0x28).unwrap().len(), 4);

        profile.matches.insert("tank".into(), json!(5));
        let rating = Rating::read(&profile, Queue::Tank, &places).unwrap();
        assert_eq!(
            rating.match_rank(),
            MatchRank {
                skill_rating: 4100,
                top_500_place: 7,
                tier: 6
            }
        );
        assert_eq!(rating.fields()[&0x40], Bool(false));
        profile.ratings.insert("tank".into(), json!(9000));
        assert_eq!(rating_of(&profile, Queue::Tank).unwrap(), 5000);
        profile.matches.insert("tank".into(), json!(-5));
        assert_eq!(matches_of(&profile, Queue::Tank).unwrap(), 0);
    }

    /// Rule 73: a win and a loss move the rating by the step, twice that
    /// over the queue's first 10 matches, a draw not at all, within 1-5000;
    /// rule 87: a leaver loses 50.
    #[test]
    fn competitive_results_move_the_rating() {
        let mut profile = Profile::default();
        profile.ratings.insert("support".into(), json!(2000));
        profile.matches.insert("support".into(), json!(9));
        let support = Queue::Support;
        assert_eq!(
            rate(&mut profile, support, Outcome::Win, false).unwrap(),
            50
        );
        profile.matches.insert("support".into(), json!(10));
        assert_eq!(
            rate(&mut profile, support, Outcome::Win, false).unwrap(),
            25
        );
        assert_eq!(
            rate(&mut profile, support, Outcome::Draw, false).unwrap(),
            0
        );
        assert_eq!(
            rate(&mut profile, support, Outcome::Loss, false).unwrap(),
            -25
        );
        assert_eq!(
            rate(&mut profile, support, Outcome::Win, true).unwrap(),
            -50
        );
        assert_eq!(rating_of(&profile, support).unwrap(), 2000);

        profile.ratings.insert("support".into(), json!(4990));
        assert_eq!(
            rate(&mut profile, support, Outcome::Win, false).unwrap(),
            10
        );
        profile.ratings.insert("support".into(), json!(20));
        assert_eq!(
            rate(&mut profile, support, Outcome::Win, true).unwrap(),
            -19
        );
        assert_eq!(rating_of(&profile, support).unwrap(), 1);
    }

    /// Rule 76: Silver to Diamond stay in their tier for 4 games under its
    /// minimum and drop with the 5th; a game back over it ends the hold;
    /// Master and above drop at once. The rank record shows the held tier.
    #[test]
    fn tiers_hold_for_five_games_below_silver_to_diamond_only() {
        let mut profile = Profile::default();
        let tank = Queue::Tank;
        profile.ratings.insert("tank".into(), json!(3010));
        assert_eq!(tier_of(&profile, tank).unwrap(), 4);
        rate(&mut profile, tank, Outcome::Loss, false).unwrap();
        // 2985: Platinum by its rating, held in Diamond.
        assert_eq!(tier(rating_of(&profile, tank).unwrap()), 3);
        assert_eq!(tier_of(&profile, tank).unwrap(), 4);
        let places = LeaderboardPlaces::new();
        let fields = Rating::read(&profile, tank, &places).unwrap().fields();
        assert_eq!(
            (&fields[&0x18], &fields[&0x22]),
            (&Unsigned(2985), &Unsigned(4))
        );
        for _ in 0..3 {
            rate(&mut profile, tank, Outcome::Draw, false).unwrap();
            assert_eq!(tier_of(&profile, tank).unwrap(), 4);
        }
        rate(&mut profile, tank, Outcome::Draw, false).unwrap();
        assert_eq!(tier_of(&profile, tank).unwrap(), 3);
        assert!(profile.held_tiers.is_empty());

        // Back over the minimum ends the hold; under it again starts anew.
        profile.ratings.insert("tank".into(), json!(3010));
        rate(&mut profile, tank, Outcome::Loss, false).unwrap();
        rate(&mut profile, tank, Outcome::Win, false).unwrap();
        assert!(profile.held_tiers.is_empty());
        rate(&mut profile, tank, Outcome::Loss, false).unwrap();
        assert_eq!(profile.held_tiers["tank"], json!({"tier": 4, "games": 1}));
        // A rating the dashboard moved away from the hold shows its own tier.
        profile.ratings.insert("tank".into(), json!(1200));
        assert_eq!(tier_of(&profile, tank).unwrap(), 0);

        // Master drops at once.
        let mut master = Profile::default();
        master.ratings.insert("damage".into(), json!(3510));
        rate(&mut master, Queue::Damage, Outcome::Loss, false).unwrap();
        assert_eq!(tier_of(&master, Queue::Damage).unwrap(), 4);
        assert!(master.held_tiers.is_empty());
    }

    /// Rule 81: 15 points a win, 5 a draw, none for a loss, accrual stops at
    /// 6000.
    #[test]
    fn competitive_points_stop_at_6000() {
        let mut profile = Profile {
            comp_points: 5990,
            ..Profile::default()
        };
        assert_eq!(award_points(&mut profile, Outcome::Loss), 0);
        assert_eq!(award_points(&mut profile, Outcome::Draw), 5);
        assert_eq!(award_points(&mut profile, Outcome::Win), 5);
        assert_eq!(profile.comp_points, 6000);
        profile.comp_points = 7000;
        assert_eq!(award_points(&mut profile, Outcome::Win), 0);
        assert_eq!(profile.comp_points, 7000);
    }

    /// The state lists every season up to the player's, times the running
    /// ones from a week ago (the test seasons last 60 days: 53 days left)
    /// and leaves the running events out of `+0xC8`.
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_state_lists_seasons_up_to_the_players_and_times_the_running_ones() {
        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        let ranked = test_catalog::ranked();
        let card = |index| Unsigned(CARD_BASE | index);
        let cards = |indexes: &[u64]| indexes.iter().map(|&index| card(index)).collect::<Vec<_>>();
        let now = 1_790_000_000;
        let left = Float64((53 * DAY) as f64);
        let season_10 = Profile {
            season: 10,
            ..Profile::default()
        };
        for (profile, event_cards, (number, main), history, timers, listed) in [
            (
                &Profile::default(),
                [COMPETITIVE_CTF].as_slice(),
                (32, 0x1b3),
                [0xc2, 0xdd, 0xe6, 0x121, 0x1ad, 0x1b3, 0x1b4, 0x1c3].as_slice(),
                [0x1b3, 0x1b4, 0x1c3].as_slice(),
                [0xc2, 0xdd, 0xe6, 0x121, 0x1ad, 0x1b3, 0x1b4].as_slice(),
            ),
            (
                &season_10,
                [LUCIO_CUP].as_slice(),
                (10, 0xe6),
                [0xdd, 0xe6, 0x1ad, 0x1c3].as_slice(),
                [0xe6, 0x1ad].as_slice(),
                [0xdd, 0xe6, 0x1c3].as_slice(),
            ),
        ] {
            let places = LeaderboardPlaces::new();
            let messages = ranked
                .login_messages(profile, event_cards, &places, now)
                .unwrap();
            let (crc, id, state) = messages.last().unwrap();
            assert_eq!((*crc, *id), (RANKED, STATE));
            assert_eq!(state[&0x78], card(main));
            let ids = |offset: u32, key: u32| match &state[&offset] {
                Array(entries) => entries
                    .iter()
                    .map(|entry| match entry {
                        Struct(fields) => fields[&key].clone(),
                        other => other.clone(),
                    })
                    .collect::<Vec<_>>(),
                _ => panic!("a list"),
            };
            assert_eq!(ids(0x98, 0x18), cards(history));
            assert_eq!(ids(0xb0, 0), cards(timers));
            assert_eq!(ids(0xb0, 8), vec![left.clone(); timers.len()]);
            assert_eq!(ids(0xc8, 0), cards(listed));
            // The season's number goes with it; its dates moved to a week ago.
            let Array(entries) = &state[&0x98] else {
                panic!("seasons")
            };
            let Some(Struct(own)) = entries
                .iter()
                .find(|entry| matches!(entry, Struct(fields) if fields[&0x18] == card(main)))
            else {
                panic!("the player's season")
            };
            assert_eq!(own[&0x118], Unsigned(number));
            assert_eq!(season_time(own, 0x20).unwrap(), now - 7 * DAY);

            let ratings = Array(ranked.party_ratings(profile, &places).unwrap());
            let member = Struct(Fields::from([(0x68, ratings)]));
            let party = Struct(Fields::from([(0, Array(vec![member]))]));
            for (crc, id, fields) in
                messages
                    .iter()
                    .chain([&(PARTY, 20700, Fields::from([(0x78, party)]))])
            {
                schemas.encode(*crc, *id, fields).unwrap();
            }
        }

        let season_25 = Profile {
            season: 25,
            ratings: Map::from_iter([("tank".into(), json!(3100))]),
            matches: Map::from_iter([("damage".into(), json!(2))]),
            ..Profile::default()
        };
        let card = CARD_BASE | 0x121;
        let places = LeaderboardPlaces::from([(Queue::Tank, 7)]);
        assert_eq!(
            ranked.rank(&season_25, card, TANK, &places).unwrap(),
            Some(MatchRank {
                skill_rating: 3100,
                top_500_place: 7,
                tier: 4
            })
        );
        assert_eq!(
            ranked.rank(&season_25, card, DAMAGE, &places).unwrap(),
            Some(MatchRank::default())
        );
        assert_eq!(
            ranked.rank(&season_25, ROLE_QUEUE, TANK, &places).unwrap(),
            None
        );
    }

    /// The install's seasons: 1-36 with their cards, open queue cards from
    /// season 23 (`lucy_assets` `seasons.rs` has the derivation and its
    /// check against the retail capture).
    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_installs_seasons_are_numbered_with_their_open_queue_cards() {
        let ranked = Ranked::load(&lucy_testkit::runtime_dir().join("data")).unwrap();
        let season = |number: i64| {
            ranked.season(&Profile {
                season: number,
                ..Profile::default()
            })
        };
        assert_eq!(ranked.seasons.len(), 36);
        assert_eq!(ranked.history.len(), 83);
        for (number, card, open_card) in [
            (1, 0xdd, None),
            (18, 0xc2, None),
            (22, 0x11e, None),
            (23, 0x11f, Some(0x143)),
            (32, 0x1b3, Some(0x1b4)),
            (36, 0x1bb, Some(0x1bc)),
        ] {
            assert_eq!(
                season(number),
                Season {
                    number: number as u64,
                    card: CARD_BASE | card,
                    open_card: open_card.map(|card| CARD_BASE | card),
                }
            );
        }
        assert_eq!(season(0).number, CURRENT_SEASON);
        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        for (crc, id, fields) in ranked
            .login_messages(
                &Profile::default(),
                &[COMPETITIVE_CTF],
                &LeaderboardPlaces::new(),
                1_790_000_000,
            )
            .unwrap()
        {
            schemas.encode(crc, id, &fields).unwrap();
        }
    }
}
