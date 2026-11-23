//! Career statistics and the mode-rules message sent at login.
//!
//! Ports `ow174/content/career.py`. 27202 carries the career stat lists
//! twice (`+0x78`, `+0xA8`) and two stat catalogs, each hero's (`+0x90`) and
//! each map's (`+0xC0`) `062` stats, which the server sends empty (see
//! [`mode_rules`]).
//!
//! Client research: a hero's lifetime "Time Played" is stat 062/021 (seconds)
//! and "Matches won" 062/039. The career profile's role tables sum a hero stat
//! per role themselves: "Matches won" of a role is 062/713, the sum of 062/039
//! (read by 0x7FF789936F33), so each role queue's wins go on one hero of the
//! role. The "All modes" stats category has key 0; a competitive season's has
//! key `(season << 16) | 3`, with 0x20 added when the season item has the
//! flag 0xD800000000084E7 (0x7FF7893528A0, 0x7FF78992D210); both keys are sent.

use std::io;

use indexmap::IndexMap;

use crate::{
    accounts::Profile,
    collection::Collection,
    leaderboard::PROFILES,
    login::Identity,
    menu::explicit_hero,
    player_messages::{endorsement, summary},
    ranked::{LeaderboardPlaces, Queue, Ranked, Season, wins_of},
    templates::LOBBY,
    transport::LobbyMessage,
};
use lucy_content::catalog::ItemDb;
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Blob, Bool, Float64, Signed, Struct, Unsigned},
};

pub const MODE_RULES: u32 = 0xb9dc_a497;
pub const RULES: u32 = 27202;
/// 22206 (group SOCIAL_OUT): a career profile request.
pub const PROFILE_REQUEST: u32 = 22206;
const FULL_PROFILE: u32 = 20807;
const PROFILE_STATUS: u32 = 39001;
const SUMMARY: u32 = 39002;

const TIME_PLAYED: u64 = 0x0860_0000_0000_0021;
const MATCHES_WON: u64 = 0x0860_0000_0000_0039;
const SEASON_KIND: u64 = 3;
const SEASON_FLAG: u64 = 0x20;
const ROLE_QUEUES: [Queue; 3] = [Queue::Tank, Queue::Damage, Queue::Support];

/// 27202: the career stats and two empty stat catalogs. Retail listed each
/// hero's and each map's tracked stats (`062`); the install has no list of
/// them found yet (the `062` assets name their heroes for only some stats).
/// Without them the career profile's statistics show 0 (seen in the game;
/// `docs/dev/loose-ends.md`, `D:/Work/Coding/refs/reports/RetailTemplates.md`).
pub fn mode_rules(
    profile: &Profile,
    heroes: &[u64],
    items: &ItemDb,
    season: Season,
) -> io::Result<Fields> {
    let stats = Array(career_stats(profile, heroes, items, season)?);
    Ok(Fields::from([
        (0x78, stats.clone()),
        (0x90, Array(Vec::new())),
        (0xa8, stats),
        (0xc0, Array(Vec::new())),
    ]))
}

/// What a career profile shows besides the profile itself.
pub struct CareerContent<'a> {
    pub collection: &'a Collection,
    pub ranked: &'a Ranked,
    /// The Arcade's competitive event cards.
    pub event_cards: &'a [u64],
    pub places: &'a LeaderboardPlaces,
    /// Seconds since 1970, for the player card.
    pub now: u64,
}

/// The answer to a career profile request (22206) for `target` (+0x88).
///
/// Only the profile's own player has a full profile (20807) and summary
/// (39002); anyone else gets a profile status (39001). 20807 repeats the
/// request's own id (22206 +0x78): with another id the client leaves the
/// screen empty. `profile`, `ow174/content/career.py`.
pub fn career_profile(
    profile: &Profile,
    identity: Identity,
    target: &Fields,
    request_id: Option<&Fields>,
    content: CareerContent<'_>,
) -> io::Result<Vec<LobbyMessage>> {
    let target_lo = match target.get(&0) {
        Some(Unsigned(lo)) => Some(*lo),
        Some(Signed(lo)) => Some(*lo as u64),
        _ => None,
    };
    if target_lo != Some(u64::from(identity.account_lo)) {
        return Ok(vec![(
            PROFILES,
            PROFILE_STATUS,
            Fields::from([(0x78, Struct(target.clone())), (0x88, Unsigned(1))]),
        )]);
    }
    let CareerContent {
        collection,
        ranked,
        event_cards,
        places,
        now,
    } = content;
    let season = ranked.season(profile);
    let stats = Array(career_stats(
        profile,
        collection.heroes(),
        collection.items(),
        season,
    )?);
    let mut progression = collection.progression(profile)?;
    let mut catalog = collection.hero_catalog(profile)?;
    let cards = ranked
        .cards(profile, event_cards, places)?
        .into_iter()
        .map(Struct)
        .collect();
    let body = Fields::from([
        (
            0x00,
            progression.remove(&0x78).unwrap_or(Struct(Fields::new())),
        ),
        (0x80, catalog.remove(&0x80).unwrap_or(Array(Vec::new()))),
        (0x98, Struct(Fields::from([(0, Array(cards))]))),
        (0xb0, stats.clone()),
        (0xc8, stats),
        (0xe0, Struct(endorsement(profile.endorsement_level))),
        (0x100, Struct(identity.account())),
        (0x110, Array(Vec::new())),
        (0x128, Unsigned(0)),
        (0x12c, Unsigned(0)),
        (0x130, Bool(false)),
        (0x138, Value::String(profile.player_name.clone())),
        (0x160, Value::String(String::new())),
    ]);
    let id = request_id
        .filter(|id| !id.is_empty())
        .cloned()
        .unwrap_or_else(|| identity.account());
    Ok(vec![
        (
            LOBBY,
            FULL_PROFILE,
            Fields::from([
                (0x78, Struct(id)),
                // False: the profile is inline at +0x90; true would mean
                // compressed in the +0x218 blob.
                (0x88, Bool(false)),
                (0x90, Struct(body)),
                (0x218, Blob(Vec::new())),
            ]),
        ),
        (
            PROFILES,
            SUMMARY,
            summary(profile, identity, collection.portrait_frame(profile), now),
        ),
    ])
}

/// The lifetime list followed by the selected season's two career categories.
/// The roster is the collection's ordered catalog, shared with other login messages.
pub fn career_stats(
    profile: &Profile,
    heroes: &[u64],
    items: &ItemDb,
    season: Season,
) -> io::Result<Vec<Value>> {
    if heroes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty career hero catalog",
        ));
    }
    let main = explicit_hero(&profile.lobby_hero, heroes, items).unwrap_or_else(|| {
        heroes[crc32fast::hash(profile.player_name.as_bytes()) as usize % heroes.len()]
    });
    let played: IndexMap<_, _> = std::iter::once(main)
        .chain(heroes.iter().copied().filter(|&hero| hero != main))
        .zip([500., 50., 40., 30., 20.])
        .map(|(hero, hours)| (hero, vec![stat(TIME_PLAYED, hours * 3600.)]))
        .collect();

    let mut won = IndexMap::new();
    for queue in ROLE_QUEUES {
        let wins = wins_of(profile, queue)?;
        if wins == 0 {
            continue;
        }
        if let Some(hero) = played.keys().chain(heroes).copied().find(|hero| {
            items
                .hero_classes
                .get(hero)
                .and_then(Option::as_deref)
                .is_some_and(|class| class.eq_ignore_ascii_case(queue.name()))
        }) {
            *won.entry(hero).or_insert(0) += wins;
        }
    }

    let mut all_modes = played;
    let mut seasonal = IndexMap::new();
    for (hero, wins) in won {
        let wins = stat(MATCHES_WON, f64::from(wins));
        all_modes.entry(hero).or_default().push(wins.clone());
        seasonal.insert(hero, vec![wins]);
    }
    let key = (season.number << 16) | SEASON_KIND;
    Ok(vec![
        category(all_modes, 0),
        category(seasonal.clone(), key),
        category(seasonal, key | SEASON_FLAG),
    ])
}

fn stat(guid: u64, value: f64) -> Value {
    Struct(Fields::from([(0, Unsigned(guid)), (8, Float64(value))]))
}

fn category(stats: IndexMap<u64, Vec<Value>>, key: u64) -> Value {
    let records = stats
        .into_iter()
        .map(|(hero, stats)| Struct(Fields::from([(0, Array(stats)), (0x18, Unsigned(hero))])))
        .collect();
    Struct(Fields::from([(0, Array(records)), (0x18, Unsigned(key))]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Schemas;

    const SEASON: Season = Season {
        number: 32,
        card: 1,
        open_card: None,
    };

    fn roster() -> ItemDb {
        let mut items = ItemDb::default();
        items.hero_names.insert(6, "Reaper".into());
        items.hero_names.insert(20, "Fictional NPC".into());
        for (hero, role) in [(2, "Damage"), (5, "SUPPORT"), (6, "Damage"), (7, "Tank")] {
            items.hero_classes.insert(hero, Some(role.into()));
        }
        items
    }

    #[test]
    fn preserves_played_hero_order_and_adds_role_wins_after_time_played() {
        let items = roster();
        let heroes = [1, 2, 3, 4, 5, 6, 7];
        let mut profile = Profile {
            lobby_hero: " rEaPeR ".into(),
            ..Profile::default()
        };
        profile.wins.extend([
            ("tank".into(), 7.into()),
            ("damage".into(), 3.into()),
            ("support".into(), 4.into()),
        ]);
        profile.matches.insert("tank".into(), 5.into());
        profile.wins.insert("open".into(), 11.into());
        let got = career_stats(&profile, &heroes, &items, SEASON).unwrap();
        let all = IndexMap::from([
            (
                6,
                vec![stat(TIME_PLAYED, 1_800_000.), stat(MATCHES_WON, 3.)],
            ),
            (1, vec![stat(TIME_PLAYED, 180_000.)]),
            (2, vec![stat(TIME_PLAYED, 144_000.)]),
            (3, vec![stat(TIME_PLAYED, 108_000.)]),
            (4, vec![stat(TIME_PLAYED, 72_000.)]),
            (7, vec![stat(MATCHES_WON, 5.)]),
            (5, vec![stat(MATCHES_WON, 4.)]),
        ]);
        let seasonal = IndexMap::from([
            (7, vec![stat(MATCHES_WON, 5.)]),
            (6, vec![stat(MATCHES_WON, 3.)]),
            (5, vec![stat(MATCHES_WON, 4.)]),
        ]);
        assert_eq!(
            got,
            [
                category(all, 0),
                category(seasonal.clone(), 0x20_0003),
                category(seasonal, 0x20_0023)
            ]
        );
        profile.lobby_hero = "0X0000000000000006".into();
        assert_eq!(
            career_stats(&profile, &heroes, &items, SEASON).unwrap(),
            got
        );
    }

    #[test]
    fn the_stat_lists_go_twice_and_non_catalog_heroes_count_as_random() {
        let items = roster();
        let heroes = [1, 2, 3, 4, 5, 6, 7];
        let mut profile = Profile {
            player_name: "Fävorite 群".into(),
            lobby_hero: "random".into(),
            ..Profile::default()
        };
        let random = mode_rules(&profile, &heroes, &items, SEASON).unwrap();
        profile.lobby_hero = "Fictional NPC".into();
        let npc = mode_rules(&profile, &heroes, &items, SEASON).unwrap();
        assert_eq!(npc, random);
        assert_eq!(npc[&0x78], npc[&0xa8]);
        assert_eq!(
            npc[&0x78],
            Array(career_stats(&profile, &heroes, &items, SEASON).unwrap())
        );
        assert_eq!(npc[&0x90], Array(Vec::new()));
        assert_eq!(npc[&0xc0], Array(Vec::new()));
        assert!(career_stats(&profile, &[], &items, SEASON).is_err());
        Schemas::lobby().encode(MODE_RULES, RULES, &npc).unwrap();
    }

    #[test]
    fn only_the_players_own_account_gets_a_full_career_profile() {
        let collection = crate::test_catalog::collection();
        let ranked = crate::test_catalog::ranked();
        let schemas = Schemas::lobby();
        let profile = Profile {
            player_name: "Career".into(),
            ..Profile::default()
        };
        let identity = Identity::for_account(0x1000_0042);
        let places = LeaderboardPlaces::new();
        let content = || CareerContent {
            collection: &collection,
            ranked: &ranked,
            event_cards: &[],
            places: &places,
            now: 1_800_000_000,
        };
        let stranger = crate::login::Identity::for_account(7).account();
        let messages = career_profile(&profile, identity, &stranger, None, content()).unwrap();
        assert_eq!(
            messages,
            [(
                PROFILES,
                PROFILE_STATUS,
                Fields::from([(0x78, Struct(stranger)), (0x88, Unsigned(1))])
            )]
        );

        let request_id = crate::login::Identity::for_account(99).account();
        let own = identity.account();
        let messages =
            career_profile(&profile, identity, &own, Some(&request_id), content()).unwrap();
        let keys: Vec<_> = messages.iter().map(|&(crc, id, _)| (crc, id)).collect();
        assert_eq!(keys, [(LOBBY, FULL_PROFILE), (PROFILES, SUMMARY)]);
        let full = &messages[0].2;
        assert_eq!(full[&0x78], Struct(request_id));
        let Struct(body) = &full[&0x90] else {
            panic!("an inline profile")
        };
        assert_eq!(body[&0x138], Value::String("Career".into()));
        assert_eq!(body[&0x100], Struct(identity.account()));
        assert_eq!(body[&0xb0], body[&0xc8]);
        for (crc, id, fields) in &messages {
            schemas.encode(*crc, *id, fields).unwrap();
        }
        // Without a request id the profile answers with the account id.
        let messages =
            career_profile(&profile, identity, &own, Some(&Fields::new()), content()).unwrap();
        assert_eq!(messages[0].2[&0x78], Struct(identity.account()));
    }
}
