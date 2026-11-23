use super::*;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    thread,
};

use serde_json::json;

use crate::{
    handoff::{GAME_REQUEST, is_request},
    party::{self, Membership, PartyMember},
    presence::FRIENDS,
    ranked::{COMPETITIVE_CTF, Queue, RANKED},
};
use lucy_wire::{
    jam::framing::{self, FrameReader},
    jam::handshake::client_handshake,
};

#[test]
fn answers_battle_tag_queries_without_creating_unknown_accounts() {
    let temporary = tempfile::tempdir().unwrap();
    let accounts = Accounts::new(
        temporary.path().join("profiles"),
        temporary.path().join("seed"),
    )
    .unwrap();
    let local = accounts.get("account").unwrap();
    local.profile().player_name = "Nickname".into();
    let expected = local.battle_tag();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let schemas = Schemas::from_json("{}").unwrap();
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            assert!(
                LobbySession::accept(stream, 1, &[0; 64], &accounts, &schemas, Clock::wall())
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(accounts.all_saved().unwrap(), ["account"]);
    });
    assert_eq!(battle_tag_query::ask(address, "account").unwrap(), expected);
    assert_eq!(
        battle_tag_query::ask(address, "other").unwrap(),
        battle_tag("other", account_id_for("other"))
    );
    worker.join().unwrap();
}

#[test]
fn negotiates_pings_and_accepts_practice_messages_after_login() {
    let temporary = tempfile::tempdir().unwrap();
    let accounts = Accounts::new(
        temporary.path().join("profiles"),
        temporary.path().join("seed"),
    )
    .unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let schemas = Schemas::from_json(
            r#"{
                    "3C7E3468":{
                        "21799":[],
                        "21800":[{"type":12,"off":120,"count":0,"array":false}]
                    },
                    "F5FF548A":{
                        "20500":[{"type":12,"off":168,"count":0,"array":false}],
                        "20503":[{"type":8,"off":120,"count":0,"array":false},
                                 {"type":0,"off":128,"count":0,"array":false}]
                    },
                    "A6E53896":{"24000":[
                        {"type":2,"off":120,"count":0,"array":false},
                        {"type":12,"off":128,"count":0,"array":false},
                        {"type":7,"off":168,"count":0,"array":false}
                    ]
                    }
                }"#,
        )
        .unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut session =
            LobbySession::accept(stream, 2, &[42; 64], &accounts, &schemas, Clock::wall())
                .unwrap()
                .unwrap();
        assert_eq!(
            session.receive_message().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let LoginRequest::Prepared(prepared) = session
            .prepare_login(&accounts, || panic!("named login used the local default"))
            .unwrap()
        else {
            panic!("a valid login was not prepared")
        };
        assert!(prepared.tournament);
        assert_eq!(prepared.account.name, "player");
        assert_eq!(prepared.identity.account_lo, prepared.account.account_lo);
        assert_eq!(
            prepared.identity.session[0],
            u64::from(session.transport.sequence)
        );
        let profile = prepared.account.profile();
        assert_eq!(profile.last_online, 0);
        assert!(
            session
                .send(&(
                    IN_CONNECT,
                    20500,
                    player::hello(&profile, prepared.identity),
                ))
                .unwrap()
        );
        assert!(
            !session
                .send(&(player::PERMISSIONS, 55500, player::features()))
                .unwrap()
        );
        let practice = session.receive_message().unwrap().unwrap();
        assert!(is_request(&practice));
        assert_eq!(practice.2[&0x80], Value::String(String::new()));
        assert!(session.send_disconnect(INCOMPATIBLE_CLIENT).unwrap());
    });

    let mut client = TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut tx, rx) = client_handshake(&mut client);
    let mut announcement = vec![0, 0, 4, 0, 0, 0];
    for group in [
        IN_CONNECT,
        OUT_CONNECT,
        player::PERMISSIONS + 1,
        GAME_REQUEST,
    ] {
        announcement.extend_from_slice(&group.to_le_bytes());
    }
    let mut outgoing = Vec::new();
    for frame in [
        b"\x02\x01premature\0".as_slice(),
        &announcement,
        &framing::PONG,
        b"\x04\0\x02early\0\x04\0\0\0",
        b"\x03\x01wrong group\0",
        b"\x02\x00wrong message\0",
        b"\x02\x01unterminated",
        b"\x02\x01trailing byte\0X",
        b"\x02\x01 player \0",
        b"\x04\0\x02\0\x04\0\0\0X",
        b"\x04\0\x02\0\x04\0\0\0",
    ] {
        framing::send_frame(&mut outgoing, &mut tx, frame).unwrap();
    }
    client.write_all(&outgoing).unwrap();

    let mut data = Vec::new();
    client.read_to_end(&mut data).unwrap();
    let mut expected_hello = vec![1, 0];
    expected_hello.extend_from_slice(battle_tag("player", account_id_for("player")).as_bytes());
    expected_hello.push(0);
    let mut disconnect = vec![1, 3];
    disconnect.extend_from_slice(&INCOMPATIBLE_CLIENT.to_le_bytes());
    disconnect.push(0);
    assert_eq!(
        FrameReader::new(rx).feed(&data),
        [
            framing::CONNECTED.to_vec(),
            framing::PONG.to_vec(),
            expected_hello,
            disconnect,
        ]
    );
    worker.join().unwrap();
}

#[test]
fn a_login_with_a_path_for_a_name_is_refused_without_a_profile() {
    let temporary = tempfile::tempdir().unwrap();
    let profiles = temporary.path().join("profiles");
    let accounts = Accounts::new(&profiles, temporary.path().join("seed")).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let schemas = Schemas::from_json(
            r#"{"3C7E3468":{
                "21799":[],
                "21800":[{"type":12,"off":120,"count":0,"array":false}]
            }}"#,
        )
        .unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut session =
            LobbySession::accept(stream, 2, &[42; 64], &accounts, &schemas, Clock::wall())
                .unwrap()
                .unwrap();
        let LoginRequest::Rejected { reason } = session
            .prepare_login(&accounts, || panic!("named login used the local default"))
            .unwrap()
        else {
            panic!("a name with a path was accepted")
        };
        assert_eq!(reason, login::NAME_INVALID_CHARACTERS);
        assert!(accounts.all_saved().unwrap().is_empty());
    });

    let mut client = TcpStream::connect(address).unwrap();
    let (mut tx, _) = client_handshake(&mut client);
    let mut announcement = vec![0, 0, 2, 0, 0, 0];
    for group in [IN_CONNECT, OUT_CONNECT] {
        announcement.extend_from_slice(&group.to_le_bytes());
    }
    let mut outgoing = Vec::new();
    for frame in [announcement.as_slice(), b"\x02\x01..\\..\\evil\0"] {
        framing::send_frame(&mut outgoing, &mut tx, frame).unwrap();
    }
    client.write_all(&outgoing).unwrap();
    worker.join().unwrap();
    let written: Vec<_> = std::fs::read_dir(temporary.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(written, ["profiles"]);
    assert_eq!(std::fs::read_dir(profiles).unwrap().count(), 0);
}

/// The whole login in wire order: the account's messages, then the static
/// ones (`templates.rs`, built messages in their place), the ranked state,
/// the ping, the party and the settings. Every message encodes.
#[test]
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn the_login_goes_in_wire_order() {
    let runtime = lucy_testkit::runtime_dir().join("data");
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let install = InstallContent::load(&runtime).unwrap();
    let collection = Collection::load(&runtime).unwrap();
    let reaper = collection.items().hero_by_name("Reaper").unwrap();
    let ranked = Ranked::load(&runtime).unwrap();
    let arcade = Arcade::load(runtime.join("arcade_cards_174.json")).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let accounts = Accounts::new(
        temporary.path().join("profiles"),
        temporary.path().join("seed"),
    )
    .unwrap();
    let account = accounts.get("player").unwrap();
    let prepared = PreparedLogin {
        identity: Identity::for_account(account.account_lo),
        account,
        tournament: false,
    };
    let arcade_group = crate::arcade::ARCADE;
    let static_messages = [
        (LOBBY, 20814),
        (LOBBY, 20806),
        (LOBBY, 20812),
        (LOBBY, 20813),
        (LOBBY, 20821),
        (passes::PASSES, 58501),
        (0xaa91_be18, 40900),
        (0xaa91_be18, 40914),
        (crate::lookups::OWL_LIVE, 43200),
        (crate::lookups::OWL_LIVE, 43200),
        (arcade_group, 39800),
        (arcade_group, 39801),
        (arcade_group, 39802),
        (arcade_group, 39803),
        (arcade_group, 39810),
        (arcade_group, 39806),
        (arcade_group, 39807),
        (arcade_group, 39804),
        (0x267f_de9e, 28001),
        (0x0dfe_efef, 38300),
        (CONFIG, 36600),
        (CONFIG, 36603),
        (CONFIG, 36602),
        (0x5894_d085, 51804),
        (0x5894_d085, 51807),
        (0x5894_d085, 51810),
        (0x5894_d085, 51817),
        (0x5894_d085, 51812),
        (0x34bb_385d, 43301),
        (crate::lookups::STORE, 26404),
        (crate::lookups::STORE, 26400),
        (LOBBY, 20805),
        (templates::CUSTOM_GAMES, 23301),
    ];
    for (level, frame, date, menu_hero, event_cards) in [
        (
            100,
            0x0250_0000_0000_0921,
            "2024-02-29",
            None,
            [].as_slice(),
        ),
        (
            3000,
            0x0250_0000_0000_0f97,
            "2026-09-29",
            Some(reaper),
            [COMPETITIVE_CTF].as_slice(),
        ),
    ] {
        let unlock_all = level == 3000;
        {
            let mut profile = prepared.account.profile();
            profile.level = level;
            profile.unlock_all = unlock_all;
            profile.server_date = date.into();
            profile.lobby_hero = if unlock_all { "Reaper" } else { "none" }.into();
            profile.events = vec![if unlock_all { "lunar" } else { "goodbye" }.into()];
            if unlock_all {
                profile
                    .settings
                    .insert("+0x10D".into(), json!({"+0x0": false, "+0x5": 1}));
            }
        }
        let places = LeaderboardPlaces::from([(Queue::Tank, 7)]);
        let solo = {
            let profile = prepared.account.profile();
            let ratings = ranked.party_ratings(&profile, &places).unwrap();
            let member = PartyMember {
                profile: &profile,
                identity: prepared.identity,
                menu_hero: menu_hero.unwrap_or_default(),
                ratings: &ratings,
                membership: Membership::Leader,
            };
            [party::state(
                &collection,
                [0x1234, 0x2345],
                [0x0100_1d00_0000_0033, 0x4444],
                &[member],
                !unlock_all,
                1_700_000_000,
            )
            .unwrap()]
        };
        let content = LoginContent {
            menu_hero,
            dev_console: false,
            extra_preload: &[],
            wall_time: 1_700_000_000,
            arcade: &arcade,
            celebrations: None,
            leaderboard_places: &places,
            party: &solo,
        };
        let messages = prepared
            .available_messages(&schemas, &install, &collection, &Presence, &ranked, content)
            .unwrap();
        let keys: Vec<_> = messages.iter().map(|&(crc, id, _)| (crc, id)).collect();
        let mut expected = vec![
            (IN_CONNECT, 20500),
            (IN_CONNECT, 20502),
            (IN_CONNECT, 20505),
            (player::ENDORSEMENTS, 52002),
            (player::ENDORSEMENTS, 52005),
            (player::ENDORSEMENTS, 52006),
            (FRIENDS, 27113),
            (PROGRESSION, 24300),
            (HERO_CATALOG, 24900),
            (MODE_RULES, RULES),
        ];
        expected.extend(static_messages);
        // A rating per card (the season's, its open queue's, the events'),
        // then the state.
        expected.extend(std::iter::repeat_n((RANKED, 36302), 2 + event_cards.len()));
        expected.extend([
            (RANKED, 36300),
            (PING, 35500),
            (party::PARTY, 20700),
            (LOBBY, 20802),
        ]);
        assert_eq!(keys, expected);
        let Value::Struct(card) = &messages[1].2[&0x78] else {
            panic!("missing player card");
        };
        let Value::Struct(progression) = &messages[7].2[&0x78] else {
            panic!("missing progression");
        };
        assert_eq!(card[&0x28], Value::Unsigned(frame));
        assert_eq!(progression[&0x58], card[&0x28]);
        // The config sets this build, and the menu hero when there is one.
        let config = &messages[keys.iter().position(|&key| key == (CONFIG, 36600)).unwrap()].2;
        assert_eq!(*config, templates::menu_config(menu_hero, false));
        for (crc, id, fields) in &messages {
            schemas.encode(*crc, *id, fields).unwrap();
        }
    }
}

#[test]
#[ignore = "requires a derived 104319 runtime"]
fn live_messages_follow_the_python_refresh_order() {
    let runtime = lucy_testkit::runtime_dir().join("data");
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let collection = Collection::load(&runtime).unwrap();
    let ranked = Ranked::load(&runtime).unwrap();
    let arcade = Arcade::load(runtime.join("arcade_cards_174.json")).unwrap();
    let economy = Arc::new(crate::economy::Economy::load(&runtime).unwrap());
    let celebrations = Celebrations::load(&runtime, economy).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let accounts = Accounts::new(
        temporary.path().join("profiles"),
        temporary.path().join("seed"),
    )
    .unwrap();
    let account = accounts.get("player").unwrap();
    {
        let mut profile = account.profile();
        profile.credits = 1234;
        profile.comp_points = 56;
        profile.league_tokens = 78;
        profile.level = 901;
        profile.experience = 4321;
        profile.leave_mask = 0b10;
        profile.server_date = "2024-02-29".into();
    }
    let prepared = PreparedLogin {
        identity: Identity::for_account(account.account_lo),
        account,
        tournament: false,
    };
    let places = LeaderboardPlaces::new();
    let content = LoginContent {
        menu_hero: None,
        dev_console: false,
        extra_preload: &[],
        wall_time: 1_700_000_000,
        arcade: &arcade,
        celebrations: Some(&celebrations),
        leaderboard_places: &places,
        party: &[(party::PARTY, 20700, Fields::new())],
    };
    let messages = prepared
        .live_messages(&collection, &ranked, content)
        .unwrap();
    let keys: Vec<_> = messages.iter().map(|&(crc, id, _)| (crc, id)).collect();
    let ratings = keys.iter().filter(|&&key| key == (RANKED, 36302)).count();
    let events = crate::celebrations::EVENTS;
    let arcade_group = crate::arcade::ARCADE;
    let mut expected = vec![
        (CONFIG, 36602),
        (CONFIG, 36600),
        (IN_CONNECT, 20502),
        (IN_CONNECT, 20504),
        (events, 38900),
        (events, 38902),
        (arcade_group, 39802),
        (arcade_group, 39810),
        (arcade_group, 39804),
        (player::ENDORSEMENTS, 52002),
        (player::ENDORSEMENTS, 52005),
        (player::ENDORSEMENTS, 52006),
        (player::PERMISSIONS, 55500),
        (PROGRESSION, 24300),
        (PROGRESSION, 24302),
        (PROGRESSION, 24307),
        (PROGRESSION, 24308),
        (PROGRESSION, 24309),
        (PROGRESSION, 24313),
        // New: the leave policy after a match's result (progression.rs).
        (LOBBY, 20813),
        (HERO_CATALOG, 24900),
    ];
    expected.extend(std::iter::repeat_n((RANKED, 36302), ratings));
    expected.extend([
        (RANKED, 36300),
        (passes::PASSES, 58501),
        (party::PARTY, 20700),
    ]);
    assert!(ratings >= 2);
    assert_eq!(keys, expected);
    let value = |id| {
        messages
            .iter()
            .find(|message| message.1 == id)
            .map(|message| message.2[&0x78].clone())
            .unwrap()
    };
    assert_eq!(value(24307), Value::Signed(1234));
    assert_eq!(value(24308), Value::Signed(56));
    assert_eq!(value(24309), Value::Signed(78));
    assert_eq!(value(24313), Value::Signed(901));
    let level = &messages
        .iter()
        .find(|message| message.1 == 24313)
        .unwrap()
        .2;
    assert_eq!(level[&0x80], Value::Unsigned(4321));
    assert_eq!(value(20813), Value::Unsigned(0b10));
    let noon = crate::clock::server_time("2024-02-29", 0).unwrap();
    assert_eq!(value(36602), Value::Unsigned(noon));
    for (crc, id, fields) in messages {
        schemas.encode(crc, id, &fields).unwrap();
    }
}
