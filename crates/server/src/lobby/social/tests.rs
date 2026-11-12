//! Game clients on the native lobby over loopback: friends, presence,
//! parties, chat, whispers, the bot, the group finder and the match chat.

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket},
    time::{Duration, Instant},
};

use orpheus_lobby::{
    accounts::{account_id_for, battle_tag},
    handoff::{CREATE_GAME, GAME_REQUEST, HANDOFF, HANDOFF_MESSAGE, general_channel},
    login::{LOGIN_MESSAGE, OUT_CONNECT},
    party::PARTY,
    presence::FRIENDS,
    social::{
        array,
        messages::{CHAT_IN, CHAT_OUT, FRIENDS_OUT, GROUP_FINDER, GROUPS, PARTY_OUT, SENT_TEXT},
        structure,
    },
    templates::{IN_CONNECT, LOBBY},
};
use orpheus_wire::{
    jam::cipher::Jam,
    jam::codec::{Fields, Schemas, Value, id16},
    jam::framing::{self, FrameReader},
    jam::handshake::client_handshake,
    link::{ACK, LinkCipher, MAX_DATAGRAM, Packet, SYN},
};

use crate::server::{Server, ServerConfig};

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
const WAIT: Duration = Duration::from_secs(5);

struct Client<'a> {
    schemas: &'a Schemas,
    tcp: TcpStream,
    tx: Jam,
    reader: FrameReader,
    received: Vec<(u32, u32, Fields)>,
}

impl<'a> Client<'a> {
    fn login(schemas: &'a Schemas, lobby: SocketAddr, name: &str) -> Self {
        let mut tcp = TcpStream::connect(lobby).unwrap();
        tcp.set_read_timeout(Some(WAIT)).unwrap();
        let (mut tx, rx) = client_handshake(&mut tcp);
        tcp.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut announcement = vec![0, 0];
        announcement.extend_from_slice(&(ANNOUNCED.len() as u32).to_le_bytes());
        for group in ANNOUNCED {
            announcement.extend_from_slice(&group.to_le_bytes());
        }
        framing::send_frame(&mut tcp, &mut tx, &announcement).unwrap();
        let mut client = Self {
            schemas,
            tcp,
            tx,
            reader: FrameReader::new(rx),
            received: Vec::new(),
        };
        client.send(
            OUT_CONNECT,
            LOGIN_MESSAGE,
            Fields::from([(0x78, Value::String(name.into()))]),
        );
        client
    }

    fn send(&mut self, crc: u32, id: u32, fields: Fields) {
        let wire = ANNOUNCED.iter().position(|&group| group == crc).unwrap() as u8 + 1;
        let mut frame = self.schemas.header(crc, wire, id).unwrap();
        frame.extend(self.schemas.encode(crc, id, &fields).unwrap());
        framing::send_frame(&mut self.tcp, &mut self.tx, &frame).unwrap();
    }

    fn read(&mut self) {
        let mut buffer = [0; 65_536];
        match self.tcp.read(&mut buffer) {
            Ok(0) => panic!("the lobby closed the connection"),
            Ok(count) => {
                for frame in self.reader.feed(&buffer[..count]) {
                    let Some(&wire) = frame.first().filter(|&&wire| wire != 0) else {
                        continue;
                    };
                    let crc = ANNOUNCED[usize::from(wire) - 1];
                    let (id, body) = self.schemas.split(crc, &frame).unwrap();
                    let fields = self.schemas.decode(crc, id, body, true).unwrap();
                    self.received.push((crc, id, fields));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("{error}"),
        }
    }

    /// The first message of this kind received so far or within a while;
    /// it and everything before it are consumed.
    fn expect(&mut self, crc: u32, id: u32) -> Fields {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(index) = self
                .received
                .iter()
                .position(|&(group, message, _)| (group, message) == (crc, id))
            {
                return self.received.drain(..=index).next_back().unwrap().2;
            }
            assert!(Instant::now() < deadline, "no {crc:08X}/{id}");
            self.read();
        }
    }

    /// Everything that arrives within a short while.
    fn settle(&mut self) -> Vec<u32> {
        let until = Instant::now() + Duration::from_millis(700);
        while Instant::now() < until {
            self.read();
        }
        self.received.drain(..).map(|(_, id, _)| id).collect()
    }
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

fn start(state: &tempfile::TempDir) -> Server {
    let runtime = orpheus_testkit::runtime_dir();
    let mut config = ServerConfig::new(runtime, state.path().into());
    config.lobby_port = 0;
    config.game_port = Some(0);
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = Some(0);
    Server::start(config).unwrap()
}

#[test]
#[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
fn two_players_befriend_party_chat_and_find_the_bot_group() {
    let state = tempfile::tempdir().unwrap();
    let mut server = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let dashboard = server.dashboard_addr().unwrap();
    let schemas = Schemas::lobby();
    let bot = account_id_for("Bot");
    let (alpha_lo, beta_lo) = (account_id_for("Alpha"), account_id_for("Beta"));

    // Login: the party state, the settings, the friends list and the cards,
    // then General.
    let mut alpha = Client::login(&schemas, lobby, "Alpha");
    let solo = alpha.expect(PARTY, 20700);
    assert_eq!(members(&solo), [u64::from(alpha_lo)]);
    alpha.expect(LOBBY, 20802);
    let friends = alpha.expect(FRIENDS, 27100);
    assert_eq!(array(friends.get(&0x78)).len(), 1);
    let cards = alpha.expect(LOBBY, 20809);
    assert_eq!(array(cards.get(&0x78)).len(), 1);
    alpha.expect(CHAT_IN, 20402);
    let mut beta = Client::login(&schemas, lobby, "Beta");
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
    drop(beta);
    let state = alpha.expect(PARTY, 20700);
    assert_eq!(members(&state), [u64::from(alpha_lo)]);
    alpha.expect(CHAT_IN, 20404);
    alpha.expect(FRIENDS, 27113);
    alpha.expect(LOBBY, 20809);

    // The bot accepts an invite at once.
    alpha.send(PARTY_OUT, 22102, Fields::from([(0x78, id("Bot"))]));
    let state = alpha.expect(PARTY, 20700);
    assert_eq!(members(&state), [u64::from(alpha_lo), u64::from(bot)]);

    drop(alpha);
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
    let mut server = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let schemas = Schemas::lobby();
    let mut alpha = Client::login(&schemas, lobby, "Alpha");
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

    // The game connects; .leave sends it home, and once it is gone the
    // player leaves the match chat and is in General again.
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
    let cipher = LinkCipher::new(key(0xce), key(0xae));
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    udp.connect(server.game_addr().unwrap()).unwrap();
    udp.set_read_timeout(Some(WAIT)).unwrap();
    let mut packet = Packet {
        conn: u32::try_from(connection).unwrap(),
        flags: SYN,
        ..Packet::default()
    };
    udp.send(&cipher.seal(&packet).unwrap()).unwrap();
    let mut buffer = [0; MAX_DATAGRAM];
    let count = udp.recv(&mut buffer).unwrap();
    let syn_ack = cipher.open(&buffer[..count]).unwrap();
    packet.seq += 1;
    packet.flags = ACK;
    packet.ack = syn_ack.seq.wrapping_add(1);
    udp.send(&cipher.seal(&packet).unwrap()).unwrap();
    udp.recv(&mut buffer).unwrap();
    say(&mut alpha, &matched, ".leave");
    let left = alpha.expect(CHAT_IN, 20404);
    assert_eq!(left[&0x78], Value::Struct(matched));
    let joined = alpha.expect(CHAT_IN, 20402);
    assert_eq!(joined[&0x78], joined_general);
    alpha.expect(PARTY, 20700);

    drop(alpha);
    server.stop().unwrap();
}
