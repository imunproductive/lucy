use serde_json::{Map, json};

use super::*;
use crate::{ranked::CURRENT_SEASON, test_catalog};

fn player(name: &str, region: &str, ratings: &[(&str, i64, i64)]) -> Player {
    let map = |index: usize| -> Map<String, serde_json::Value> {
        ratings
            .iter()
            .map(|entry| {
                let value = [entry.1, entry.2, entry.2][index];
                (entry.0.to_owned(), json!(value))
            })
            .collect()
    };
    let account_lo = crate::accounts::account_id_for(name);
    Player {
        name: name.into(),
        profile: Profile {
            player_name: name.into(),
            game_region: region.into(),
            ratings: map(0),
            wins: map(1),
            matches: map(2),
            ..Profile::default()
        },
        identity: Identity::for_account(account_lo),
    }
}

fn players() -> Vec<Player> {
    let mut quiet = player("Quiet", "americas", &[("tank", 4800, 40)]);
    quiet.profile.sms_protect = false;
    vec![
        player(
            "Alpha",
            "americas",
            &[("tank", 4000, 30), ("damage", 3100, 25)],
        ),
        player(
            "Beta",
            "americas",
            &[("tank", 3500, 25), ("open", 3900, 25)],
        ),
        player("Gamma", "europe", &[("tank", 4500, 30)]),
        player("Delta", "americas", &[("tank", 4200, 10)]),
        quiet,
    ]
}

#[test]
fn places_count_only_qualified_players_of_the_same_region() {
    let players = players();
    let place = |index: usize| {
        let player = &players[index];
        places(player.identity.account_lo, &player.profile, &players).unwrap()
    };
    assert_eq!(
        place(0),
        LeaderboardPlaces::from([(Queue::Tank, 1), (Queue::Damage, 1)])
    );
    assert_eq!(
        place(1),
        LeaderboardPlaces::from([(Queue::Tank, 2), (Queue::Open, 1)])
    );
    assert_eq!(place(2), LeaderboardPlaces::from([(Queue::Tank, 1)]));
    assert!(place(3).is_empty(), "too few wins");
    assert!(place(4).is_empty(), "SMS Protect off");
}

fn rows(page: &LobbyMessage) -> Vec<(String, Fields)> {
    let Struct(body) = &page.2[&0x78] else {
        panic!("a page")
    };
    let Array(entries) = &body[&0] else {
        panic!("entries")
    };
    entries
        .iter()
        .map(|entry| {
            let Struct(entry) = entry else {
                panic!("an entry")
            };
            let Struct(record) = &entry[&0] else {
                panic!("a card")
            };
            let Value::String(tag) = &record[&0x40] else {
                panic!("a tag")
            };
            let Struct(rank) = &entry[&0x68] else {
                panic!("a rank")
            };
            (tag.split('#').next().unwrap().to_owned(), rank.clone())
        })
        .collect()
}

#[test]
fn a_page_lists_the_filters_players_best_first() {
    let ranked = test_catalog::ranked();
    let collection = test_catalog::collection();
    let players = players();
    let me = &players[1];
    let season = ranked.season(&me.profile);
    assert_eq!(season.number, CURRENT_SEASON);
    let friends = HashSet::from(["alpha".to_owned()]);
    let request = |key: u64| Fields::from([(0, Unsigned(key)), (8, Unsigned(3))]);
    let page_of = |key| {
        page(
            &ranked,
            &collection,
            &request(key),
            &players,
            me,
            &friends,
            1_800_000_000,
        )
        .unwrap()
    };
    let americas = 1 << 48;

    // Tank (role 2) in the Americas.
    let tank = page_of(CURRENT_SEASON | (2 << 24) | americas);
    assert_eq!(tank.0, PROFILES);
    assert_eq!(tank.1, PAGE);
    let listed = rows(&tank);
    assert_eq!(
        listed
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Beta"]
    );
    assert_eq!(
        listed[0].1,
        Fields::from([
            (0x00, Unsigned(season.card)),
            (0x08, Unsigned(TANK)),
            (0x10, Unsigned(4000)),
            (0x12, Unsigned(1)),
            (0x14, Unsigned(u64::from(tier(4000)))),
            (0x15, Unsigned(0)),
            (0x16, Bool(true)),
        ])
    );
    let Struct(body) = &tank.2[&0x78] else {
        panic!()
    };
    assert_eq!(
        body[&0x18],
        Struct(Fields::from([
            (0, Unsigned(CURRENT_SEASON | (2 << 24) | americas)),
            (8, Unsigned(3))
        ]))
    );

    // All roles list a player once, by the best role; any region.
    let all = rows(&page_of(CURRENT_SEASON | (4 << 24)));
    assert_eq!(
        all.iter()
            .map(|(name, rank)| (name.as_str(), rank[&0x10].clone()))
            .collect::<Vec<_>>(),
        [
            ("Gamma", Unsigned(4500)),
            ("Alpha", Unsigned(4000)),
            ("Beta", Unsigned(3500))
        ]
    );

    // Friends only: the player and its friends, any region.
    let friends_only = rows(&page_of(CURRENT_SEASON | (2 << 24) | (1 << 32) | (5 << 48)));
    assert_eq!(friends_only.len(), 2);

    // Another season, or a card the season does not run, lists nobody.
    assert!(rows(&page_of(31 | (2 << 24))).is_empty());
    let open = season.open_card.expect("season 32 has an open queue");
    let card_board = |card: u64| (card & 0xffff_ffff) | (CARD_BOARD << 40) | americas;
    let open_rows = rows(&page_of(card_board(open)));
    assert_eq!(open_rows.len(), 1);
    assert_eq!(open_rows[0].1[&0x00], Unsigned(open));
    assert!(rows(&page_of(card_board(CARD_BASE | 0x123))).is_empty());
}
