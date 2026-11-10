use super::*;
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    thread,
};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{
    handoff::{GAME_REQUEST, is_request},
    presence::FRIENDS,
    ranked::{COMPETITIVE_CTF, Queue, RANKED},
};
use orpheus_content::catalog::ItemDb;
use orpheus_wire::{
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
                LobbySession::accept(stream, 1, &[0; 64], &accounts, &schemas)
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
        let mut session = LobbySession::accept(stream, 2, &[42; 64], &accounts, &schemas)
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
#[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
fn prepared_subset_matches_the_python_login_in_wire_order() {
    let runtime = orpheus_testkit::runtime_dir().join("data");
    let schemas = Schemas::lobby();
    let templates = RetailTemplates::captured();
    let items = Arc::new(ItemDb::load(&runtime).unwrap());
    let reaper = items.hero_by_name("Reaper").unwrap();
    let collection = Collection::new(&templates, items).unwrap();
    let presence = Presence::new(&templates).unwrap();
    let ranked = Ranked::new(&templates).unwrap();
    let arcade = Arcade::load(runtime.join("arcade_cards_174.json"), &templates).unwrap();
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
    // Python login ordering replaces the early party from Content with the
    // session's actual party after PING. Compare both subsets.
    for (level, frame, date, menu_hero, event_cards, places, base, with_extras) in [
        (
            100,
            0x0250_0000_0000_0921,
            "2024-02-29",
            None,
            [].as_slice(),
            LeaderboardPlaces::new(),
            "c19b931eacf9c54cd61056f82694453fa93a1e716d0c7f01f1cb273480ac76ef",
            "092d0b645d812659197dc2f3e4ab5790ae1c7264060c33f2b6d4491d1f6a170b",
        ),
        (
            3000,
            0x0250_0000_0000_0f97,
            "2026-09-29",
            Some(reaper),
            [COMPETITIVE_CTF].as_slice(),
            LeaderboardPlaces::from([(Queue::Tank, 7)]),
            "7d7660da99e8973ce1ca8c0b2d73122f0822bead8a4d2fa7785876025b0251c7",
            "551492de79257b2112e0cdbcb3e2fc1ceffbc9493f4d55d18bce2863c558ace7",
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
        let inputs = LoginContent {
            menu_hero,
            extra_preload: &[],
            wall_time: 1_700_000_000,
            arcade: &arcade,
            celebrations: None,
            leaderboard_places: &places,
            solo_party: None,
        };
        let exact_inputs = LoginContent {
            solo_party: Some(SoloLoginParty {
                id: [0x1234, 0x2345],
                entity: [0x0100_1d00_0000_0033, 0x4444],
                menu_hero: menu_hero.unwrap_or_default(),
                members_may_invite: !unlock_all,
            }),
            ..inputs
        };
        for (content, count, expected) in [
            (inputs, 46, base),
            (exact_inputs, 50 + event_cards.len(), with_extras),
        ] {
            let mut messages = prepared
                .available_messages(
                    &schemas,
                    &templates,
                    &collection,
                    &presence,
                    &ranked,
                    content,
                )
                .unwrap();
            let arcade_and_passes = [
                (passes::PASSES, 58501),
                (crate::arcade::ARCADE, 39802),
                (crate::arcade::ARCADE, 39810),
                (crate::arcade::ARCADE, 39804),
            ];
            let actual: Vec<_> = messages
                .iter()
                .map(|&(crc, id, _)| (crc, id))
                .filter(|key| arcade_and_passes.contains(key))
                .collect();
            assert_eq!(actual, arcade_and_passes);
            let ranks = messages
                .iter()
                .filter(|message| message.0 == RANKED)
                .count();
            assert_eq!(ranks, 3 + event_cards.len());
            assert_eq!(messages[9].0, MODE_RULES);
            assert_eq!(messages[9].1, RULES);
            assert_eq!(
                messages.len(),
                54 + event_cards.len() + usize::from(content.solo_party.is_some())
            );
            // Keep the earlier reference corpus for the original subset.
            // The new dynamic rows have their own complete wire comparison.
            messages.retain(|&(crc, id, _)| {
                (crc, id) != (MODE_RULES, RULES)
                    && !arcade_and_passes.contains(&(crc, id))
                    && (content.solo_party.is_some() || crc != RANKED)
            });
            let keys: Vec<_> = messages.iter().map(|&(crc, id, _)| (crc, id)).collect();
            assert_eq!(
                keys[..9],
                [
                    (IN_CONNECT, 20500),
                    (IN_CONNECT, 20502),
                    (IN_CONNECT, 20505),
                    (player::ENDORSEMENTS, 52002),
                    (player::ENDORSEMENTS, 52005),
                    (player::ENDORSEMENTS, 52006),
                    (FRIENDS, 27113),
                    (PROGRESSION, 24300),
                    (HERO_CATALOG, 24900),
                ]
            );
            if content.solo_party.is_some() {
                assert_eq!(
                    keys[keys.len() - 4..],
                    [
                        (RANKED, 36300),
                        (PING, 35500),
                        (party::PARTY, 20700),
                        (LOBBY, 20802)
                    ]
                );
            } else {
                assert_eq!(keys[keys.len() - 2..], [(PING, 35500), (LOBBY, 20802)]);
            }
            assert!(keys[9..].contains(&(LOBBY, 20812)));
            let Value::Struct(card) = &messages[1].2[&0x78] else {
                panic!("missing player card");
            };
            let Value::Struct(progression) = &messages[7].2[&0x78] else {
                panic!("missing progression");
            };
            assert_eq!(card[&0x28], Value::Unsigned(frame));
            assert_eq!(progression[&0x58], card[&0x28]);
            assert_eq!(messages.len(), count);
            let mut digest = Sha256::new();
            for (crc, id, fields) in messages {
                let mut wire = schemas.header(crc, 1, id).unwrap();
                wire.extend(schemas.encode(crc, id, &fields).unwrap());
                digest.update(crc.to_le_bytes());
                digest.update(id.to_le_bytes());
                digest.update(u32::try_from(wire.len()).unwrap().to_le_bytes());
                digest.update(wire);
            }
            assert_eq!(format!("{:x}", digest.finalize()), expected);
        }
    }
}
