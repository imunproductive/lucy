//! Custom games through the whole server on its virtual clocks, with the
//! lobby test client as each player's game (`refs/reports/CustomGames.md`
//! §11; behavior.md "Custom games").

use std::net::SocketAddr;

use lucy_lobby::{
    custom_game::{
        CustomSettings, UPLOAD,
        browser::{BROWSER, BROWSER_IN},
        links::{LINKS, LINKS_IN},
    },
    handoff::{CREATE_GAME, GAME_REQUEST, HANDOFF, HANDOFF_MESSAGE},
    lineup::{MATCH_LOBBY, MATCH_LOBBY_OUT},
    login::OUT_CONNECT,
    templates::IN_CONNECT,
};
use lucy_wire::jam::codec::{
    Fields, Schemas,
    Value::{self, Array, Bool, Signed, Struct, Unsigned},
    id16,
};

use crate::{
    lobby::{
        TICK,
        test_client::{Client, run},
    },
    server::{Server, ServerConfig, VirtualTime},
};

const CUSTOM_GAMES: u32 = 0x2128_6b49;
const CHAT_IN: u32 = 0x5f91_3f6b;
const GROUPS: [u32; 12] = [
    IN_CONNECT,
    OUT_CONNECT,
    GAME_REQUEST,
    CUSTOM_GAMES,
    MATCH_LOBBY,
    MATCH_LOBBY_OUT,
    HANDOFF,
    CHAT_IN,
    BROWSER_IN,
    BROWSER,
    LINKS_IN,
    LINKS,
];
const HANAMURA: u64 = 0x0800_0000_0000_0165;
const ASSAULT: u64 = 0x0230_0000_0000_0014;

fn server(state: &tempfile::TempDir) -> (Server, VirtualTime) {
    let mut config = ServerConfig::new(lucy_testkit::runtime_dir(), state.path().into());
    config.lobby_port = 0;
    config.game_port = Some(0);
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = None;
    Server::start_virtual(config).unwrap()
}

fn structure(value: &Value) -> &Fields {
    let Struct(fields) = value else {
        panic!("expected a struct, got {value:?}")
    };
    fields
}

fn array(value: &Value) -> &[Value] {
    let Array(values) = value else {
        panic!("expected an array, got {value:?}")
    };
    values
}

fn ours(crc: u32) -> bool {
    matches!(crc, CUSTOM_GAMES | MATCH_LOBBY | BROWSER | LINKS)
}

/// The ids of the custom games, match lobby, browser and code messages
/// received so far.
fn ids(client: &Client<'_>) -> Vec<u32> {
    client
        .received
        .iter()
        .filter(|&&(crc, _, _)| ours(crc))
        .map(|&(_, id, _)| id)
        .collect()
}

/// The ids of those messages received by the next sync, which are
/// consumed.
fn custom(client: &mut Client<'_>) -> Vec<u32> {
    client
        .take()
        .into_iter()
        .filter(|&(crc, _, _)| ours(crc))
        .map(|(_, id, _)| id)
        .collect()
}

fn last(client: &Client<'_>, wanted: u32) -> Fields {
    client
        .received
        .iter()
        .rev()
        .find(|&&(_, id, _)| id == wanted)
        .unwrap_or_else(|| panic!("no {wanted}"))
        .2
        .clone()
}

/// Hanamura Assault (on) with teams of 2 and 2, and Assault's block.
fn hanamura() -> CustomSettings {
    use lucy_lobby::custom_game::{Chunk, ChunkKind};
    let teams = Array(
        (0..2)
            .map(|team| Struct(Fields::from([(0x60, Unsigned(team)), (0x61, Unsigned(2))])))
            .collect(),
    );
    let block = Fields::from([
        (0xb0, Unsigned(ASSAULT)),
        (0xc0, teams),
        (0x108, Bool(true)),
    ]);
    CustomSettings {
        settings: Fields::new(),
        extra: Fields::new(),
        chunks: vec![
            Chunk {
                kind: ChunkKind::Map,
                fields: Fields::from([(
                    0x78,
                    Struct(Fields::from([
                        (0x0, Struct(block.clone())),
                        (0x138, Unsigned(HANAMURA)),
                    ])),
                )]),
            },
            Chunk {
                kind: ChunkKind::Mode,
                fields: Fields::from([(0x78, Struct(block))]),
            },
        ],
    }
}

/// The client's upload of `settings` (24008-24013), as its sender sends
/// them.
fn upload(client: &mut Client<'_>, settings: &CustomSettings) {
    for (crc, id, fields) in settings.transfer(UPLOAD, Fields::new()) {
        client.send(crc, id, fields);
    }
}

fn create(client: &mut Client<'_>, name: &str, settings: &CustomSettings) {
    client.send(
        GAME_REQUEST,
        CREATE_GAME,
        Fields::from([
            (0x78, Unsigned(1)),
            (0x80, Value::String(name.into())),
            (0xa8, Signed(4)),
        ]),
    );
    upload(client, settings);
}

fn lobby_id(id: [u64; 2]) -> Value {
    Struct(Fields::from([(
        0,
        Array(vec![Unsigned(id[0]), Unsigned(id[1])]),
    )]))
}

/// The places of the roster (53002): (team, kind, seat) by card account.
fn places(roster: &Fields) -> Vec<(u64, u64, i64, u64)> {
    array(&roster[&0x78])
        .iter()
        .map(|record| {
            let record = structure(record);
            let card = structure(&record[&0x0]);
            let account = match &structure(&card[&0x0])[&0x0] {
                Unsigned(account) => *account,
                other => panic!("{other:?}"),
            };
            let number = |offset| match &record[&offset] {
                Unsigned(value) => *value as i64,
                Signed(value) => *value,
                other => panic!("{other:?}"),
            };
            (
                account,
                number(0xa8) as u64,
                number(0xac),
                number(0xaf) as u64,
            )
        })
        .collect()
}

/// A host creates a lobby; a second player finds it in the browser, reads
/// its details and joins; the host moves him to Team 2 and renames the
/// lobby; Start sends both into one match of Hanamura on their teams;
/// Return to Lobby takes them out of it and back into the lobby; the
/// second player leaves.
#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn two_players_create_join_start_and_return_to_the_lobby() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, time) = server(&state);
    let schemas = Schemas::lobby();
    let lobby = SocketAddr::from(server.lobby_addr());
    let mut host = Client::login(&schemas, lobby, &GROUPS, "Host");
    let mut guest = Client::login(&schemas, lobby, &GROUPS, "Guest");
    custom(&mut host);
    custom(&mut guest);

    create(&mut host, "Parkour", &hanamura());
    assert_eq!(
        custom(&mut host),
        [23304, 23305, 23306, 53000, 53001, 53002]
    );

    // The browser lists it; its row's lobby id is what JOIN sends.
    guest.send(
        BROWSER_IN,
        40703,
        Fields::from([(0x78, Struct(Fields::new()))]),
    );
    let results = guest.expect(BROWSER, 40603);
    let [entry] = array(&results[&0x88]) else {
        panic!("{results:?}")
    };
    let game = structure(&structure(entry)[&0x0]);
    assert_eq!(game[&0x100], Value::String("Parkour".into()));
    let count = |offset| match &game[&offset] {
        Signed(count) => *count,
        other => panic!("{other:?}"),
    };
    assert_eq!(count(0xc8), 1, "one player");
    assert_eq!((count(0xcb), count(0xcc)), (2, 2));
    assert_eq!(
        game[&0xb8],
        Unsigned(0x0790_0000_0000_0165),
        "Hanamura's header"
    );
    let id = match &structure(&game[&0x98])[&0] {
        Array(halves) => [halves[0].clone(), halves[1].clone()].map(|half| match half {
            Unsigned(half) => half,
            other => panic!("{other:?}"),
        }),
        other => panic!("{other:?}"),
    };

    // Its details, then JOIN.
    guest.send(
        GAME_REQUEST,
        24020,
        Fields::from([(0x78, lobby_id(id)), (0x88, Struct(id16(id[0], id[1])))]),
    );
    let details = guest.expect(CUSTOM_GAMES, 23320);
    assert_eq!(details[&0x78], Signed(0));
    assert_eq!(structure(&details[&0x80])[&0x30], lobby_id(id));
    custom(&mut guest);
    guest.send(
        MATCH_LOBBY_OUT,
        52900,
        Fields::from([
            (0x78, lobby_id(id)),
            (0x98, Unsigned(1)),
            (0x99, Unsigned(3)),
            (0xa0, Signed(-1)),
        ]),
    );
    assert_eq!(
        custom(&mut guest),
        [23304, 23305, 23306, 53000, 53001, 53002]
    );
    host.sync();
    assert_eq!(ids(&host), [23310, 53001, 53002]);
    let roster = last(&host, 53002);
    let accounts: Vec<u64> = places(&roster).iter().map(|place| place.0).collect();
    let guest_id = accounts[1];
    // Seated in Team 2, the emptier team, at seat 0; the host in Team 1.
    assert_eq!(
        places(&roster),
        [(accounts[0], 0, 0, 0), (guest_id, 1, 0, 0)]
    );

    // The owner moves him to Team 1's second seat and renames the lobby.
    let id_of = |account: u64| Struct(id16(account, 1 << 56));
    host.send(
        GAME_REQUEST,
        24017,
        Fields::from([
            (0x78, id_of(guest_id)),
            (0x88, Unsigned(0)),
            (0x89, Unsigned(0)),
            (0x8a, Unsigned(255)),
        ]),
    );
    assert_eq!(
        places(&last(&host, 53002)),
        [(accounts[0], 0, 0, 0), (guest_id, 0, 0, 1)]
    );
    guest.sync();
    assert_eq!(places(&last(&guest, 53002))[1], (guest_id, 0, 0, 1));
    // Only the owner moves players.
    guest.received.clear();
    guest.send(
        GAME_REQUEST,
        24017,
        Fields::from([
            (0x78, id_of(accounts[0])),
            (0x88, Unsigned(2)),
            (0x89, Unsigned(3)),
            (0x8a, Unsigned(255)),
        ]),
    );
    assert_eq!(ids(&guest), Vec::<u32>::new());
    host.send(
        GAME_REQUEST,
        24019,
        Fields::from([
            (0x78, Unsigned(1)),
            (0x80, Value::String("Doom".into())),
            (0xa8, Unsigned(0)),
        ]),
    );
    guest.sync();
    let updated = last(&guest, 23310);
    assert_eq!(
        structure(&updated[&0x88])[&0x88],
        Value::String("Doom".into())
    );
    host.received.clear();
    guest.received.clear();

    // Start: both into Hanamura, on Team 1, at once.
    host.send(GAME_REQUEST, 24018, Fields::from([(0x78, Unsigned(1))]));
    // As for the Practice Range the 20600s are due at once, each from its
    // session's next tick.
    run(&time.lobby, &mut [&mut host, &mut guest], TICK);
    host.expect(HANDOFF, HANDOFF_MESSAGE);
    guest.expect(HANDOFF, HANDOFF_MESSAGE);
    let games = server.game_service().unwrap().snapshot().unwrap();
    let [game] = games.as_slice() else {
        panic!("{games:?}")
    };
    assert_eq!(game.map, "Hanamura");
    let mut teams: Vec<(u32, u8)> = game
        .players
        .iter()
        .map(|player| (player.account, player.team))
        .collect();
    teams.sort();
    let mut expected = [(accounts[0] as u32, 0), (guest_id as u32, 0)];
    expected.sort();
    assert_eq!(teams, expected);

    // Return to Lobby: the notice, then the games out of the match; once
    // the lobby's sweep (a lobby tick) sees them gone, each gets its lobby
    // again, with no 53003 (which would take it out of the custom game).
    host.received.clear();
    guest.received.clear();
    host.send(GAME_REQUEST, 24018, Fields::from([(0x78, Unsigned(4))]));
    guest.expect(CUSTOM_GAMES, 23319);
    run(&time.lobby, &mut [&mut host, &mut guest], TICK);
    for client in [&mut host, &mut guest] {
        let ids = custom(client);
        assert!(!ids.contains(&53003), "{ids:?}");
        assert!(ids.contains(&23304) && ids.contains(&53002), "{ids:?}");
    }
    assert!(
        server
            .game_service()
            .unwrap()
            .snapshot()
            .unwrap()
            .is_empty()
    );

    // The guest leaves: Leave Game sends 52902 with the lobby id; the
    // host's roster has him alone.
    guest.send(
        MATCH_LOBBY_OUT,
        52902,
        Fields::from([
            (0x78, lobby_id(id)),
            (0x88, Bool(false)),
            (0x89, Bool(true)),
        ]),
    );
    host.sync();
    assert_eq!(places(&last(&host, 53002)), [(accounts[0], 0, 0, 0)]);
    host.close();
    guest.close();
    server.stop().unwrap();
}

/// Share Code stores the settings under a new code; Import Code in
/// another player's game gives them back as 40901 with the code the
/// client compares (`0x7FF7895DC330`); an unknown code gets 40912. A
/// Workshop script in `workshop_codes/` answers its code too.
#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn a_shared_code_imports_in_another_game() {
    let state = tempfile::tempdir().unwrap();
    let codes = state.path().join("workshop_codes");
    std::fs::create_dir_all(&codes).unwrap();
    std::fs::write(
        codes.join("TEST1_hanamura.txt"),
        "settings\n{\n\tmodes\n\t{\n\t\tAssault\n\t\t{\n\t\t\tenabled maps\n\t\t\t{\n\t\t\t\tHanamura\n\t\t\t}\n\t\t}\n\t}\n}\n",
    )
    .unwrap();
    let (mut server, _time) = server(&state);
    let schemas = Schemas::lobby();
    let lobby = SocketAddr::from(server.lobby_addr());
    let mut host = Client::login(&schemas, lobby, &GROUPS, "Host");
    let mut guest = Client::login(&schemas, lobby, &GROUPS, "Guest");
    // At login: the (empty) preset names.
    assert_eq!(array(&host.expect(LINKS, 40900)[&0x78]).len(), 0);

    let settings = hanamura();
    let mut header = Fields::from([
        (0x78, Unsigned(2)),
        (0x7c, Signed(-1)),
        (0x80, Unsigned(0)),
        (0x88, Value::String(String::new())),
    ]);
    header.insert(0xb0, Struct(Fields::new()));
    let group = lucy_lobby::custom_game::links::LINK_UPLOAD;
    for (_, id, fields) in settings.transfer(group, header) {
        host.send(LINKS_IN, id, fields);
    }
    let shared = host.expect(LINKS, 40913);
    assert_eq!(shared[&0xa1], Unsigned(0), "done");
    let Value::String(code) = shared[&0x78].clone() else {
        panic!()
    };
    assert_eq!(code.len(), 5);

    guest.send(
        LINKS_IN,
        40809,
        Fields::from([
            (0x78, Value::String(code.to_lowercase())),
            (0xa0, Bool(false)),
        ]),
    );
    let link = guest.expect(LINKS, 40901);
    assert_eq!(link[&0x78], Unsigned(2));
    let sent = structure(&link[&0xb8]);
    assert_eq!(structure(&sent[&0x128])[&0x70], Value::String(code.clone()));
    assert_eq!(guest.expect(LINKS, 40902)[&0x1b8], Bool(true));
    assert_eq!(guest.expect(LINKS, 40903)[&0x1b0], Bool(false));

    guest.send(
        LINKS_IN,
        40809,
        Fields::from([(0x78, Value::String("ZZZZZ".into())), (0xa0, Bool(false))]),
    );
    assert_eq!(
        guest.expect(LINKS, 40912)[&0x78],
        Value::String("ZZZZZ".into())
    );

    // The script's code: its mode block and Hanamura's entry.
    guest.send(
        LINKS_IN,
        40809,
        Fields::from([(0x78, Value::String("test1".into())), (0xa0, Bool(false))]),
    );
    let link = guest.expect(LINKS, 40901);
    assert_eq!(
        structure(&structure(&link[&0xb8])[&0x128])[&0x70],
        Value::String("TEST1".into())
    );
    let entry = structure(&guest.expect(LINKS, 40902)[&0x78]).clone();
    assert_eq!(entry[&0x138], Unsigned(HANAMURA));
    assert_eq!(structure(&entry[&0x0])[&0xb0], Unsigned(ASSAULT));
    host.close();
    guest.close();
    server.stop().unwrap();
}

/// C32S2 through a normal custom game: the host's settings upload carries
/// the script as the client writes it (the compressed Workshop text in
/// lists pieces), and Start plays it with its rules, ruleset values and
/// Doomfist alone, with no `--workshop`.
#[test]
#[ignore = "requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn dpk_archive_c32s2_runs_through_a_custom_game_lobby() {
    use base64::Engine as _;

    let text = std::fs::read_to_string(
        lucy_testkit::dpk_archive().join("workshop.codes/C32S2_OG_doom_parkur_hanamura.txt"),
    )
    .unwrap();
    let data = lucy_testkit::runtime_dir().join("data");
    let catalog = lucy_workshop::Catalog::load(&data).unwrap();
    let names = crate::workshop_match::SettingNames::load(&data).unwrap();
    let script = crate::workshop_match::ScriptMatch::parse(&text, &catalog, &names).unwrap();
    let mut settings = super::codes::script_settings(&script, &catalog, |_| Vec::new());
    // The client's own upload: the text compressed and in a lists piece
    // (piece kind 1), its rules in the binary block too.
    let compressed = ruzstd::encoding::compress_to_vec(
        text.as_bytes(),
        ruzstd::encoding::CompressionLevel::Fastest,
    );
    settings.chunks.push(lucy_lobby::custom_game::Chunk {
        kind: lucy_lobby::custom_game::ChunkKind::Lists,
        fields: Fields::from([
            (
                0x168,
                Value::String(base64::engine::general_purpose::STANDARD.encode(compressed)),
            ),
            (0x190, Unsigned(1)),
        ]),
    });

    // The settings say what the script says: Assault's values, Doomfist
    // alone (General's list, every team).
    let rules = settings.rules(HANAMURA, ASSAULT);
    let values: std::collections::BTreeMap<u32, lucy_script::graph::ModeValue> = rules
        .values
        .iter()
        .map(|(&key, &value)| (key, super::custom::mode_value(value)))
        .collect();
    assert_eq!(values, script.games[0].values);
    let doomfist = catalog.hero("Doomfist").unwrap();
    let every = &rules.hero_lists[&None];
    assert!(every.whitelist && every.heroes == [doomfist]);
    let schemas = Schemas::lobby();
    let state = tempfile::tempdir().unwrap();
    let (mut server, _time) = server(&state);
    let mut host = Client::login(
        &schemas,
        SocketAddr::from(server.lobby_addr()),
        &GROUPS,
        "Host",
    );
    create(&mut host, "Doomfist Parkour", &settings);
    let roster = host.expect(MATCH_LOBBY, 53002);
    assert_eq!(array(&roster[&0x78]).len(), 1);
    host.send(GAME_REQUEST, 24018, Fields::from([(0x78, Unsigned(1))]));
    host.expect(HANDOFF, HANDOFF_MESSAGE);
    let games = server.game_service().unwrap().snapshot().unwrap();
    let [game] = games.as_slice() else {
        panic!("{games:?}")
    };
    assert_eq!(game.map, "Hanamura");
    assert_eq!(game.workshop_rules, Some(script.logic.rules.len()));
    host.close();
    server.stop().unwrap();
}
