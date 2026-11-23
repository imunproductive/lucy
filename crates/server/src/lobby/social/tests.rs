//! Game clients on the native lobby over loopback: friends, presence,
//! parties, chat, whispers, the bot, the group finder and the match chat.
//! The server runs on virtual clocks (`Server::start_virtual`), and each
//! client reads in step with its session (`lobby/test_client.rs`).

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
};

use lucy_lobby::{
    accounts::{account_id_for, battle_tag},
    handoff::{CREATE_GAME, GAME_REQUEST, HANDOFF, HANDOFF_MESSAGE, Handoff, general_channel},
    login::OUT_CONNECT,
    party::PARTY,
    presence::FRIENDS,
    social::{
        array,
        messages::{CHAT_IN, CHAT_OUT, FRIENDS_OUT, GROUP_FINDER, GROUPS, PARTY_OUT, SENT_TEXT},
        structure, unsigned,
    },
    templates::{IN_CONNECT, LOBBY},
};
use lucy_wire::jam::codec::{Fields, Schemas, Value, id16};

use crate::{
    connection::LEAVE_GRACE,
    game_service::lockstep::Client as GameClient,
    lobby::{
        TICK,
        test_client::{Client, run},
    },
    server::{Server, ServerConfig, VirtualTime},
};

mod bot_chat;

const ANNOUNCED: [u32; 13] = [
    IN_CONNECT,
    OUT_CONNECT,
    LOBBY,
    FRIENDS,
    FRIENDS_OUT,
    PARTY,
    PARTY_OUT,
    CHAT_IN,
    CHAT_OUT,
    GROUPS,
    GROUP_FINDER,
    GAME_REQUEST,
    HANDOFF,
];

fn login<'a>(schemas: &'a Schemas, lobby: SocketAddr, name: &str) -> Client<'a> {
    Client::login(schemas, lobby, &ANNOUNCED, name)
}

fn id(name: &str) -> Value {
    Value::Struct(id16(u64::from(account_id_for(name)), 1 << 56))
}

fn members(state: &Fields) -> Vec<u64> {
    let party = structure(state.get(&0x78)).unwrap();
    array(party.get(&0))
        .iter()
        .map(|tile| {
            let card = structure(structure(Some(tile)).unwrap().get(&0)).unwrap();
            let Some(Value::Unsigned(account)) = structure(card.get(&0)).unwrap().get(&0) else {
                panic!("a party tile without an account");
            };
            *account
        })
        .collect()
}

fn post(dashboard: SocketAddr, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(dashboard).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.0\r\nHost: {dashboard}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

fn start(state: &tempfile::TempDir) -> (Server, VirtualTime) {
    let runtime = lucy_testkit::runtime_dir();
    let mut config = ServerConfig::new(runtime, state.path().into());
    config.lobby_port = 0;
    config.game_port = Some(0);
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = Some(0);
    Server::start_virtual(config).unwrap()
}

#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn two_players_befriend_party_chat_and_find_the_bot_group() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _) = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let dashboard = server.dashboard_addr().unwrap();
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let bot = account_id_for("Bot");
    let (alpha_lo, beta_lo) = (account_id_for("Alpha"), account_id_for("Beta"));

    // Login: the party state, the settings, the friends list and the cards,
    // then General.
    let mut alpha = login(&schemas, lobby, "Alpha");
    let solo = alpha.expect(PARTY, 20700);
    assert_eq!(members(&solo), [u64::from(alpha_lo)]);
    alpha.expect(LOBBY, 20802);
    let friends = alpha.expect(FRIENDS, 27100);
    assert_eq!(array(friends.get(&0x78)).len(), 1);
    let cards = alpha.expect(LOBBY, 20809);
    assert_eq!(array(cards.get(&0x78)).len(), 1);
    alpha.expect(CHAT_IN, 20402);
    let mut beta = login(&schemas, lobby, "Beta");
    beta.expect(CHAT_IN, 20402);
    // Not friends yet: nobody hears of the other's login.
    assert!(!alpha.settle().contains(&27113));

    // A friend request by BattleTag, accepted in game.
    let alpha_tag = battle_tag("Alpha", alpha_lo);
    beta.send(
        FRIENDS_OUT,
        27000,
        Fields::from([
            (0x78, Value::Unsigned(5)),
            (0x80, Value::Unsigned(1)),
            (0x88, Value::String(alpha_tag)),
        ]),
    );
    let result = beta.expect(FRIENDS, 27110);
    assert_eq!(result[&0x80], Value::Unsigned(SENT_TEXT));
    alpha.expect(FRIENDS, 27107);
    alpha.send(
        FRIENDS_OUT,
        27001,
        Fields::from([
            (0x78, Value::Unsigned(6)),
            (0x80, id("Beta")),
            (0x90, Value::Unsigned(0)),
        ]),
    );
    assert_eq!(alpha.expect(FRIENDS, 27111)[&0x80], Value::Unsigned(0));
    alpha.expect(FRIENDS, 27108);
    alpha.expect(FRIENDS, 27105);
    alpha.expect(FRIENDS, 27113);
    assert_eq!(array(alpha.expect(LOBBY, 20809).get(&0x78)).len(), 2);
    beta.expect(FRIENDS, 27105);
    beta.expect(FRIENDS, 27113);
    beta.expect(LOBBY, 20809);

    // The status dropdown: the own copy, the party state, and the friend.
    alpha.send(
        FRIENDS_OUT,
        27011,
        Fields::from([(0x78, Value::Unsigned(2))]),
    );
    alpha.expect(FRIENDS, 27113);
    alpha.expect(PARTY, 20700);
    beta.expect(FRIENDS, 27113);

    // An invite shows a pending tile; accepted, both are in the party and
    // its chat.
    alpha.send(PARTY_OUT, 22102, Fields::from([(0x78, id("Beta"))]));
    let invite = beta.expect(PARTY, 20704);
    assert_eq!(invite[&0x78], id("Alpha"));
    let pending = alpha.expect(PARTY, 20700);
    assert_eq!(members(&pending), [u64::from(alpha_lo), u64::from(beta_lo)]);
    beta.send(PARTY_OUT, 22108, Fields::from([(0x78, id("Alpha"))]));
    let mut joined = Vec::new();
    for client in [&mut alpha, &mut beta] {
        let state = client.expect(PARTY, 20700);
        assert_eq!(members(&state), [u64::from(alpha_lo), u64::from(beta_lo)]);
        joined.push(client.expect(CHAT_IN, 20402)[&0x78].clone());
    }
    assert_eq!(joined[0], joined[1]);

    // Party chat reaches every member; General lists both and the bot.
    alpha.send(
        CHAT_OUT,
        21700,
        Fields::from([
            (0x78, joined[0].clone()),
            (0x90, Value::String("hello".into())),
        ]),
    );
    for client in [&mut alpha, &mut beta] {
        let said = client.expect(CHAT_IN, 20400);
        assert_eq!(said[&0x100], Value::String("hello".into()));
    }
    let general = general_channel();
    alpha.send(
        CHAT_OUT,
        21701,
        Fields::from([(0x78, Value::Struct(general))]),
    );
    assert_eq!(array(alpha.expect(CHAT_IN, 20401).get(&0x90)).len(), 3);
    let unexpected = beta.settle();
    assert!(unexpected.is_empty(), "{unexpected:?}");

    // A whisper: 27116 to the sender, 27117 to the target.
    alpha.send(
        FRIENDS_OUT,
        27004,
        Fields::from([
            (0x78, Value::Unsigned(9)),
            (0x80, id("Beta")),
            (0xa0, Value::String("psst".into())),
        ]),
    );
    assert_eq!(alpha.expect(FRIENDS, 27116)[&0x78], Value::Unsigned(9));
    let whisper = beta.expect(FRIENDS, 27117);
    assert_eq!(whisper[&0x88], Value::String("psst".into()));

    // The bot's group in the group finder, from the dashboard.
    assert!(post(dashboard, "/api/bot_group", r#"{"action":"list"}"#).contains("group finder"));
    alpha.send(GROUP_FINDER, 52200, Fields::new());
    let found = alpha.expect(GROUPS, 52300);
    assert_eq!(array(found.get(&0x78)).len(), 1);

    // Beta leaves: the party is Alpha's alone again and its chat goes.
    beta.close();
    let state = alpha.expect(PARTY, 20700);
    assert_eq!(members(&state), [u64::from(alpha_lo)]);
    alpha.expect(CHAT_IN, 20404);
    alpha.expect(FRIENDS, 27113);
    alpha.expect(LOBBY, 20809);

    // The bot accepts an invite at once.
    alpha.send(PARTY_OUT, 22102, Fields::from([(0x78, id("Bot"))]));
    let state = alpha.expect(PARTY, 20700);
    assert_eq!(members(&state), [u64::from(alpha_lo), u64::from(bot)]);

    alpha.close();
    server.stop().unwrap();
}

/// Says `text` in `channel` and waits for the server's chat line back.
fn say(client: &mut Client<'_>, channel: &Fields, text: &str) {
    client.send(
        CHAT_OUT,
        21700,
        Fields::from([
            (0x78, Value::Struct(channel.clone())),
            (0x90, Value::String(text.into())),
        ]),
    );
    client.expect(CHAT_IN, 20400);
}

/// A dashboard GET, and its JSON body.
fn get(dashboard: SocketAddr, path: &str) -> serde_json::Value {
    let mut stream = TcpStream::connect(dashboard).unwrap();
    write!(stream, "GET {path} HTTP/1.0\r\nHost: {dashboard}\r\n\r\n").unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{reply}");
    serde_json::from_str(body).unwrap()
}

/// Every value of a decoded struct is its default: an empty card.
fn blank(value: &Value) -> bool {
    match value {
        Value::Unsigned(0) | Value::Signed(0) | Value::Bool(false) => true,
        Value::Float32(value) => *value == 0.0,
        Value::Float64(value) => *value == 0.0,
        Value::String(text) => text.is_empty(),
        Value::Struct(fields) => fields.values().all(blank),
        Value::Array(items) => items.iter().all(blank),
        Value::Blob(bytes) => bytes.iter().all(|&byte| byte == 0),
        _ => false,
    }
}

/// A system line: a 20400 on `channel` with `text`, from an empty card
/// (the client draws it alone, in the channel's colour).
fn assert_system_line(said: &Fields, channel: &Value, text: &str) {
    assert_eq!(&said[&0x78], channel);
    assert_eq!(said[&0x100], Value::String(text.into()));
    let member = structure(said.get(&0x90)).unwrap();
    assert!(blank(&member[&0]), "{member:?}");
}

/// Alpha plays the Practice Range; Beta stays in the menu. The handoff
/// joins Alpha's team channel (type 5) after the match's: his team line
/// reaches his team (himself) and not Beta. The dashboard's system line to
/// the match and its global message come as authorless 20400s, Alpha's on
/// the match channel and Beta's on General, and the match's chat log has
/// the team line and both system lines. The host's kick sends Alpha's game
/// home: once the leave grace (`LEAVE_GRACE`) drops it, the lobby's next
/// sweep takes him out of the team and match channels, back into General,
/// and gives him the host's reason there.
#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn team_chat_system_lines_and_a_kick_from_the_dashboard() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, time) = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let dashboard = server.dashboard_addr().unwrap();
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let mut alpha = login(&schemas, lobby, "Alpha");
    let joined_general = alpha.expect(CHAT_IN, 20402)[&0x78].clone();
    let mut beta = login(&schemas, lobby, "Beta");
    assert_eq!(beta.expect(CHAT_IN, 20402)[&0x78], joined_general);
    alpha.settle();
    alpha.send(
        GAME_REQUEST,
        CREATE_GAME,
        Fields::from([
            (0x78, Value::Unsigned(2)),
            (0x80, Value::String(String::new())),
            (0xa8, Value::Unsigned(4)),
        ]),
    );
    let left = alpha.expect(CHAT_IN, 20404);
    assert_eq!(left[&0x78], joined_general);
    let matched = alpha.expect(CHAT_IN, 20402)[&0x78].clone();
    let team = alpha.expect(CHAT_IN, 20402)[&0x78].clone();
    let kind = |channel: &Value| unsigned(structure(Some(channel)).unwrap().get(&0x10));
    assert_eq!((kind(&matched), kind(&team)), (6, 5));
    let handoff = alpha.expect(HANDOFF, HANDOFF_MESSAGE);

    // The game connects, and the lobby's next sweep sees it in the match.
    let ticket = structure(handoff.get(&0x80)).unwrap();
    let Value::Unsigned(connection) = ticket[&0x28] else {
        panic!("no UDP connection");
    };
    let match_id = structure(ticket.get(&0)).unwrap();
    let (Value::Unsigned(low), Value::Unsigned(high)) = (&match_id[&0], &match_id[&8]) else {
        panic!("no match id");
    };
    let id = format!("{low:016x}{high:016x}");
    let key = |offset| -> [u8; 32] {
        std::array::from_fn(|index| match &array(ticket.get(&offset))[index] {
            Value::Unsigned(byte) => u8::try_from(*byte).unwrap(),
            _ => panic!("invalid key byte"),
        })
    };
    time.game.await_scenes();
    let mut game = GameClient::on(
        &time.game,
        // The game needs only its connection and keys.
        &Handoff {
            conn: u32::try_from(connection).unwrap(),
            key_in: key(0xae),
            key_out: key(0xce),
            match_id: [0; 2],
        },
    );
    game.handshake();
    game.receive();
    run(&time.lobby, &mut [&mut alpha, &mut beta], TICK);

    // Team chat reaches the team only, not everyone online.
    let Value::Struct(team_fields) = &team else {
        unreachable!()
    };
    say(&mut alpha, team_fields, "hi team");
    let unexpected = beta.settle();
    assert!(unexpected.is_empty(), "{unexpected:?}");

    // The live view has the match, with Alpha in it.
    let alpha_lo = account_id_for("Alpha");
    let live = get(dashboard, "/api/live");
    let found = &live["matches"][0];
    assert_eq!(found["id"], id.as_str());
    assert_eq!(found["players"][0]["account"], alpha_lo.to_string());
    assert_eq!(found["chat_seq"], 1);
    let rev = live["rev"].as_u64().unwrap();
    assert_eq!(
        get(dashboard, &format!("/api/live?rev={rev}")),
        serde_json::json!({"rev": rev})
    );

    // A system line to the match, then a message to everyone.
    let reply = post(
        dashboard,
        &format!("/api/matches/{id}/chat"),
        r#"{"text":"Welcome to the Range"}"#,
    );
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert_system_line(
        &alpha.expect(CHAT_IN, 20400),
        &matched,
        "Welcome to the Range",
    );
    let reply = post(
        dashboard,
        "/api/broadcast",
        r#"{"text":"Restart in 5 minutes","scope":"all"}"#,
    );
    assert!(reply.contains(r#""told":2"#), "{reply}");
    assert_system_line(
        &alpha.expect(CHAT_IN, 20400),
        &matched,
        "Restart in 5 minutes",
    );
    assert_system_line(
        &beta.expect(CHAT_IN, 20400),
        &joined_general,
        "Restart in 5 minutes",
    );
    let chat = get(dashboard, &format!("/api/matches/{id}/chat"));
    let lines: Vec<_> = chat["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| {
            (
                line["kind"].as_str().unwrap().to_owned(),
                line["channel"].as_str().unwrap().to_owned(),
                line["from"]["name"].as_str().map(str::to_owned),
                line["text"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let line = |kind: &str, channel: &str, from: Option<&str>, text: &str| {
        (
            kind.to_owned(),
            channel.to_owned(),
            from.map(str::to_owned),
            text.to_owned(),
        )
    };
    assert_eq!(
        lines,
        [
            line("chat", "team1", Some("Alpha"), "hi team"),
            line("system", "match", None, "Welcome to the Range"),
            line("system", "match", None, "Restart in 5 minutes"),
        ]
    );
    assert_eq!(chat["next"], 3);

    // The kick: his game goes home; once the leave grace drops it, he is
    // back in General with the host's reason.
    let reply = post(
        dashboard,
        &format!("/api/matches/{id}/kick"),
        &format!(r#"{{"account":"{alpha_lo}","reason":"AFK"}}"#),
    );
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    time.game.run(LEAVE_GRACE);
    run(&time.lobby, &mut [&mut alpha], TICK);
    assert_eq!(alpha.expect(CHAT_IN, 20404)[&0x78], team);
    assert_eq!(alpha.expect(CHAT_IN, 20404)[&0x78], matched);
    assert_eq!(alpha.expect(CHAT_IN, 20402)[&0x78], joined_general);
    assert_system_line(
        &alpha.expect(CHAT_IN, 20400),
        &joined_general,
        "The host removed you from the match: AFK",
    );
    alpha.expect(PARTY, 20700);
    assert_eq!(
        get(dashboard, "/api/live")["matches"],
        serde_json::json!([])
    );

    alpha.close();
    beta.close();
    server.stop().unwrap();
}
