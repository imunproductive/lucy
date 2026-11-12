//! Game clients on the native lobby over loopback: friends, presence,
//! parties, chat, whispers, the bot, the group finder and the match chat.
//! The server runs on virtual clocks (`Server::start_virtual`), and each
//! client reads in step with its session (`lobby/test_client.rs`).

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
};

use orpheus_lobby::{
    accounts::{account_id_for, battle_tag},
    handoff::{CREATE_GAME, GAME_REQUEST, HANDOFF, HANDOFF_MESSAGE, Handoff, general_channel},
    login::OUT_CONNECT,
    party::PARTY,
    presence::FRIENDS,
    social::{
        array,
        messages::{CHAT_IN, CHAT_OUT, FRIENDS_OUT, GROUP_FINDER, GROUPS, PARTY_OUT, SENT_TEXT},
        structure,
    },
    templates::{IN_CONNECT, LOBBY},
};
use orpheus_wire::jam::codec::{Fields, Schemas, Value, id16};

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
    let runtime = orpheus_testkit::runtime_dir();
    let mut config = ServerConfig::new(runtime, state.path().into());
    config.lobby_port = 0;
    config.game_port = Some(0);
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = Some(0);
    Server::start_virtual(config).unwrap()
}

#[test]
#[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
fn two_players_befriend_party_chat_and_find_the_bot_group() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, _) = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let dashboard = server.dashboard_addr().unwrap();
    let schemas = Schemas::lobby();
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

#[test]
#[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
fn match_commands_and_general_again_after_the_match() {
    let state = tempfile::tempdir().unwrap();
    let (mut server, time) = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let schemas = Schemas::lobby();
    let mut alpha = login(&schemas, lobby, "Alpha");
    let joined_general = alpha.expect(CHAT_IN, 20402)[&0x78].clone();
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
    let Value::Struct(matched) = alpha.expect(CHAT_IN, 20402)[&0x78].clone() else {
        panic!("no match channel");
    };
    let handoff = alpha.expect(HANDOFF, HANDOFF_MESSAGE);
    // The match chat has the player.
    alpha.send(
        CHAT_OUT,
        21701,
        Fields::from([(0x78, Value::Struct(matched.clone()))]),
    );
    assert_eq!(array(alpha.expect(CHAT_IN, 20401).get(&0x90)).len(), 1);

    // The game connects, and the lobby's next sweep sees it in the match.
    let ticket = structure(handoff.get(&0x80)).unwrap();
    let Value::Unsigned(connection) = ticket[&0x28] else {
        panic!("no UDP connection");
    };
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
    run(&time.lobby, &mut [&mut alpha], TICK);
    // .leave sends it home; it stays, so the leave grace (`LEAVE_GRACE`)
    // drops it, and the lobby's next sweep takes the player out of the
    // match chat and back to General.
    say(&mut alpha, &matched, ".leave");
    time.game.run(LEAVE_GRACE);
    run(&time.lobby, &mut [&mut alpha], TICK);
    let left = alpha.expect(CHAT_IN, 20404);
    assert_eq!(left[&0x78], Value::Struct(matched));
    let joined = alpha.expect(CHAT_IN, 20402);
    assert_eq!(joined[&0x78], joined_general);
    alpha.expect(PARTY, 20700);

    alpha.close();
    server.stop().unwrap();
}
