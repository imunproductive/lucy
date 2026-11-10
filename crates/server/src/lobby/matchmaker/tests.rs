use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use orpheus_lobby::{
    handoff::{HANDOFF, HANDOFF_MESSAGE},
    lineup::MATCH_LOBBY,
    login::{LOGIN_MESSAGE, OUT_CONNECT},
    matchmaker::QUICK_PLAY,
    party::PARTY,
    queue::{ENTER_QUEUE, MATCHMAKE, QUEUE, QUEUE_WAITS, SET_ROLES, TANK_ROLE},
    templates::IN_CONNECT,
};
use orpheus_wire::jam::{
    codec::{Fields, Schemas, Value},
    framing::{self, FrameReader},
    handshake::client_handshake,
};

use crate::server::{Server, ServerConfig};

const CHAT_IN: u32 = 0x5f91_3f6b;

fn post(dashboard: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(dashboard).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.0\r\nHost: {dashboard}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let status = reply[9..12].parse().unwrap();
    (status, reply.split_once("\r\n\r\n").unwrap().1.to_owned())
}

fn structure(value: &Value) -> &Fields {
    let Value::Struct(fields) = value else {
        panic!("expected a struct, got {value:?}")
    };
    fields
}

/// One player searches Quick Play with one player to start and the
/// Practice Range forced: the queue pops, "Game Found!" shows, and the game
/// gets its 20600 after it.
#[test]
#[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
fn one_player_to_start_pops_the_queue_and_hands_the_game_off() {
    let runtime = orpheus_testkit::runtime_dir();
    let state = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::new(runtime, state.path().into());
    config.lobby_port = 0;
    config.game_port = 0;
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = Some(0);
    let mut server = Server::start(config).unwrap();
    let dashboard = server.dashboard_addr().unwrap();
    let card = format!("0x{QUICK_PLAY:016X}");
    assert_eq!(
        post(
            dashboard,
            "/api/mode_settings",
            &format!(r#"{{"card":"{card}","players_to_start":1}}"#)
        ),
        (
            200,
            r#"{"message":"Quick Play: matches start with 1 player(s)."}"#.into()
        )
    );
    let saved = std::fs::read_to_string(state.path().join("matchmaking.json")).unwrap();
    assert!(saved.contains("\"players_to_start\": 1"), "{saved}");
    assert_eq!(
        post(dashboard, "/api/set_map", r#"{"map":"0x0800000000000688"}"#),
        (
            200,
            r#"{"message":"Matches load Practice Range when their queue's modes allow it."}"#
                .into()
        )
    );

    let schemas = Schemas::lobby();
    let mut tcp = TcpStream::connect(server.lobby_addr()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let (mut tx, rx) = client_handshake(&mut tcp);
    let groups = [
        IN_CONNECT,
        OUT_CONNECT,
        MATCHMAKE,
        QUEUE,
        QUEUE_WAITS,
        PARTY,
        MATCH_LOBBY,
        HANDOFF,
        CHAT_IN,
    ];
    let wire = |group: u32| groups.iter().position(|&known| known == group).unwrap() as u8 + 1;
    let mut announcement = vec![0, 0];
    announcement.extend_from_slice(&(groups.len() as u32).to_le_bytes());
    for group in groups {
        announcement.extend_from_slice(&group.to_le_bytes());
    }
    framing::send_frame(&mut tcp, &mut tx, &announcement).unwrap();
    let key = Fields::from([(
        0,
        Value::Struct(Fields::from([(0, Value::Unsigned(QUICK_PLAY))])),
    )]);
    let mut send = |tcp: &mut TcpStream, group: u32, id: u32, fields: Fields| {
        let mut frame = schemas.header(group, wire(group), id).unwrap();
        frame.extend(schemas.encode(group, id, &fields).unwrap());
        framing::send_frame(tcp, &mut tx, &frame).unwrap();
    };
    send(&mut tcp, OUT_CONNECT, LOGIN_MESSAGE, Fields::new());
    send(
        &mut tcp,
        MATCHMAKE,
        ENTER_QUEUE,
        Fields::from([(0x78, Value::Struct(key.clone()))]),
    );

    let wanted = [
        (QUEUE, 44201),
        (QUEUE, 44202),
        (QUEUE_WAITS, 56200),
        (PARTY, 20700),
        (MATCH_LOBBY, 53000),
        (MATCH_LOBBY, 53003),
        (CHAT_IN, 20402),
        (CHAT_IN, 20404),
        (HANDOFF, HANDOFF_MESSAGE),
    ];
    let mut received: Vec<(u32, Fields, Instant)> = Vec::new();
    let mut reader = FrameReader::new(rx);
    let mut buf = [0; 16384];
    let limit = Instant::now() + Duration::from_secs(15);
    let mut roles_sent = false;
    while !received.iter().any(|(id, ..)| *id == HANDOFF_MESSAGE) {
        assert!(
            Instant::now() < limit,
            "no handoff: {:?}",
            received.iter().map(|(id, ..)| id).collect::<Vec<_>>()
        );
        let count = match tcp.read(&mut buf) {
            Ok(0) => panic!("the lobby closed the connection"),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        for frame in reader.feed(&buf[..count]) {
            for (group, id) in wanted {
                if let Some(body) =
                    frame.strip_prefix(schemas.header(group, wire(group), id).unwrap().as_slice())
                {
                    received.push((
                        id,
                        schemas.decode(group, id, body, true).unwrap(),
                        Instant::now(),
                    ));
                }
            }
        }
        // The role screen: Ready as a tank once the role check started.
        if !roles_sent && received.iter().any(|(id, ..)| *id == 56200) {
            roles_sent = true;
            send(
                &mut tcp,
                MATCHMAKE,
                SET_ROLES,
                Fields::from([(
                    0x78,
                    Value::Array(vec![Value::Unsigned(u64::from(TANK_ROLE))]),
                )]),
            );
        }
    }
    let order: Vec<u32> = received
        .iter()
        .map(|(id, ..)| *id)
        .filter(|id| *id != 20700)
        .collect();
    // 20402 General at login; then the queue, the role check, the search, the
    // pop and the game.
    assert_eq!(
        order,
        [
            20402,
            44201,
            56200,
            44201,
            44202,
            53000,
            53003,
            20404,
            20402,
            HANDOFF_MESSAGE
        ]
    );
    let find = |wanted: u32, nth: usize| {
        received
            .iter()
            .filter(|(id, ..)| *id == wanted)
            .nth(nth)
            .unwrap()
    };
    // The role check shows the queue entry in the party state.
    let party_states: Vec<&Fields> = received
        .iter()
        .filter(|(id, ..)| *id == 20700)
        .map(|(_, fields, _)| fields)
        .collect();
    let queue_state = |fields: &Fields| match &structure(&fields[&0x78])[&0x18] {
        Value::Array(entries) => entries.first().map(|entry| structure(entry)[&0x54].clone()),
        other => panic!("{other:?}"),
    };
    let states: Vec<_> = party_states
        .iter()
        .map(|fields| queue_state(fields))
        .collect();
    // Login, the role check, the search, the pop.
    assert_eq!(
        states,
        [None, Some(Value::Signed(1)), Some(Value::Signed(3)), None]
    );

    let (_, popped, _) = find(44202, 0);
    assert_eq!(popped[&0xa8], Value::Unsigned(1));
    let (_, found, found_at) = find(53003, 0);
    assert_eq!(found[&0xa0], Value::Signed(4));
    let (_, handoff, handoff_at) = find(HANDOFF_MESSAGE, 0);
    assert!(handoff_at.duration_since(*found_at) >= Duration::from_millis(3500));
    let ticket = structure(&handoff[&0x80]);
    assert_eq!(
        ticket[&0x2c],
        Value::Unsigned(u64::from(server.game_addr().port()))
    );
    let (_, lobby, _) = find(53000, 0);
    let match_id = &structure(&lobby[&0x78])[&0x30];
    assert_eq!(
        structure(match_id)[&0],
        Value::Array(match &structure(&ticket[&0])[&0] {
            Value::Unsigned(low) => vec![Value::Unsigned(*low), structure(&ticket[&0])[&8].clone()],
            other => panic!("{other:?}"),
        })
    );
    server.stop().unwrap();
}
