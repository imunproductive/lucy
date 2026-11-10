use super::*;
use orpheus_content::maps::PRACTICE_RANGE_MAP;
use orpheus_core::bits::{BitReader, BitWriter};
use orpheus_wire::{
    link::{FIN, KIND_CONTROL, LinkCipher, MAX_DATAGRAM, ReceiveWindow, SYN},
    messages::{read_channel, write_channel},
};
use std::{collections::HashSet, net::UdpSocket as ClientSocket};

const MATCH: MatchId = [5, 8];

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn player() -> Vec<MatchPlayer> {
    vec![MatchPlayer {
        name: "Protocol fixture".into(),
        team: 0,
        tournament: false,
        ranked: false,
    }]
}

fn fixture() -> Schemas {
    Schemas::from_json(
        r#"{
                "C77F6403": {
                    "20300": [{"type":8,"off":1192,"size":8,"count":0,"array":false}],
                    "20301": [], "20302": [], "20304": [],
                    "20306": [
                        {"type":6,"off":120,"size":4,"count":0,"array":false},
                        {"type":8,"off":128,"size":8,"count":-1,"array":true}
                    ]
                },
                "A7FBBD0C": {
                    "21601": [], "21602": [],
                    "21610": [
                        {"type":6,"off":120,"size":4,"count":0,"array":false},
                        {"type":8,"off":128,"size":8,"count":-1,"array":true}
                    ]
                }
            }"#,
    )
    .unwrap()
}

struct Client {
    socket: ClientSocket,
    cipher: LinkCipher,
    connection: u32,
    next: u32,
    window: ReceiveWindow,
}

impl Client {
    fn new(address: SocketAddr, ticket: &Handoff) -> Self {
        let socket = ClientSocket::bind(local()).unwrap();
        socket.connect(address).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        Self {
            socket,
            cipher: LinkCipher::new(ticket.key_out, ticket.key_in),
            connection: ticket.conn,
            next: 0,
            window: ReceiveWindow::default(),
        }
    }

    fn send(&mut self, flags: u8, payload: Vec<u8>) {
        let packet = Packet {
            conn: self.connection,
            kind: if payload.is_empty() {
                KIND_CONTROL
            } else {
                KIND_FRAME
            },
            seq: self.next,
            ack: self.window.expected,
            ack_bits: self.window.bits,
            flags,
            payload,
        };
        self.next += 1;
        self.socket
            .send(&self.cipher.seal(&packet).unwrap())
            .unwrap();
    }

    fn receive(&self) -> Packet {
        let mut bytes = [0; MAX_DATAGRAM];
        let count = self.socket.recv(&mut bytes).unwrap();
        let packet = self.cipher.open(&bytes[..count]).unwrap();
        assert_eq!(packet.conn, self.connection);
        packet
    }

    fn handshake(&mut self) {
        self.send(SYN, Vec::new());
        let response = self.receive();
        assert_eq!((response.kind, response.flags), (KIND_CONTROL, SYN | ACK));
        assert!(self.window.accept(response.seq));
        self.send(ACK, Vec::new());
    }
}

fn messages(schemas: &Schemas, packet: &Packet) -> Vec<orpheus_wire::messages::ReliableMessage> {
    assert_eq!(packet.kind, KIND_FRAME);
    let mut reader = BitReader::new(&packet.payload);
    reader.var_a().unwrap();
    read_channel(&mut reader, schemas).unwrap().reliable
}

fn exercise_loading(schemas: Schemas) {
    let schemas = Arc::new(schemas);
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let mut tickets = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap();
    let ticket = tickets.pop().unwrap();
    assert!(server.handoff_pending(ticket.conn).unwrap());
    let mut client = Client::new(server.local_addr(), &ticket);
    client.handshake();
    assert!(!server.handoff_pending(ticket.conn).unwrap());

    let first = client.receive();
    let initial = messages(&schemas, &first);
    assert_eq!((initial[0].sequence, initial[0].message.id), (0, 20300));
    assert_eq!(
        initial[0].message.fields[&0x4a8],
        Value::Unsigned(PRACTICE_RANGE_MAP)
    );
    // Drop the first instance datagram, then ACK the following heartbeat.
    let heartbeat = client.receive();
    assert!(messages(&schemas, &heartbeat).is_empty());
    assert!(client.window.accept(heartbeat.seq));
    client.send(ACK, Vec::new());
    let until = Instant::now() + Duration::from_secs(2);
    let retry = loop {
        let packet = client.receive();
        if !messages(&schemas, &packet).is_empty() {
            break packet;
        }
        assert!(Instant::now() < until, "waiting for retransmitted instance");
    };
    assert_eq!(messages(&schemas, &retry), initial);
    assert!(client.window.accept(retry.seq));

    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    let ping = encode(
        &schemas,
        21610,
        &Fields::from([
            (0x78, Value::Unsigned(17)),
            (0x80, Value::Array(vec![Value::Unsigned(25)])),
        ]),
    )
    .unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[ping]).unwrap();
    let start = Instant::now();
    client.send(ACK, payload.into_bytes());
    let mut received = Vec::new();
    while received.len() < 3 && start.elapsed() < Duration::from_secs(2) {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        received.extend(messages(&schemas, &packet));
        client.send(ACK, Vec::new());
    }
    assert_eq!(
        received
            .iter()
            .map(|message| (message.sequence, message.message.id))
            .collect::<Vec<_>>(),
        [(1, 20306), (2, 20302), (3, 20301)]
    );
    assert!(start.elapsed() >= Duration::from_millis(400));
    assert_eq!(received[0].message.fields[&0x78], Value::Unsigned(17));
    let Value::Array(stamps) = &received[0].message.fields[&0x80] else {
        panic!("expected ping timestamps");
    };
    assert_eq!(stamps.len(), 3);
    assert_eq!(stamps[0], Value::Unsigned(25));

    client.send(FIN | ACK, Vec::new());
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let packet = client.receive();
        if packet.kind == KIND_CONTROL {
            assert_eq!(packet.flags, FIN | ACK);
            break;
        }
        assert!(Instant::now() < until, "waiting for FIN acknowledgement");
    }
    let address = server.local_addr();
    server.stop().unwrap();
    assert!(ClientSocket::bind(address).is_ok());
    assert_eq!(
        server
            .open_match(MATCH, GameMap::practice_range(), player())
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
}

#[test]
fn worker_serves_a_match_seat_and_retries_before_map_entry() {
    exercise_loading(fixture());
}

#[test]
fn worker_serves_loading_using_the_retail_schemas() {
    let schemas = Schemas::game();
    exercise_loading(schemas);
}

#[test]
fn canceled_seat_is_rejected_before_tick_and_released() {
    let mut server = GameService::bind(local(), Arc::new(fixture())).unwrap();
    let ticket = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap()
        .pop()
        .unwrap();
    assert!(server.handoff_pending(ticket.conn).unwrap());
    assert!(server.cancel_handoff(ticket.conn).unwrap());
    assert!(!server.handoff_pending(ticket.conn).unwrap());
    let mut client = Client::new(server.local_addr(), &ticket);
    client
        .socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    client.send(SYN, Vec::new());
    let mut buffer = [0; MAX_DATAGRAM];
    let error = client.socket.recv(&mut buffer).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ));
    let retry = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap()
        .pop()
        .unwrap();
    assert_ne!(ticket.conn, retry.conn);
    server.stop().unwrap();
}

#[derive(Debug)]
struct WorldRecord {
    entity: u32,
    operation: Option<u8>,
    definition: Option<u32>,
    movement: bool,
    skin: Option<(u32, bool)>,
    script: Option<ReceivedChunk>,
    projectile_release: Option<u32>,
    base_health: Option<f32>,
}

#[derive(Debug, Eq, PartialEq)]
struct ReceivedChunk {
    first: u32,
    span: u32,
    width: usize,
    data: Vec<u8>,
}

impl ReceivedChunk {
    fn bits(&self) -> BitReader<'_> {
        BitReader::with_range(&self.data, 0, self.width).unwrap()
    }
}

impl WorldRecord {
    fn is_create(&self) -> bool {
        self.operation == Some(1)
    }
}

fn world_records(schemas: &Schemas, packet: &Packet) -> Vec<WorldRecord> {
    let mut reader = BitReader::new(&packet.payload);
    reader.var_a().unwrap();
    read_channel(&mut reader, schemas).unwrap();
    reader.var_a().unwrap();
    assert_eq!(reader.bits(7).unwrap(), 0b010_0000);
    let count = reader.bits(7).unwrap();
    (0..count)
        .map(|_| {
            let mut record = WorldRecord {
                entity: reader.entity_id().unwrap().unwrap(),
                operation: None,
                definition: None,
                movement: false,
                skin: None,
                script: None,
                projectile_release: None,
                base_health: None,
            };
            assert!(reader.bit().unwrap());
            if !reader.bit().unwrap() {
                return record;
            }
            if reader.bit().unwrap() {
                assert!(!reader.bit().unwrap());
                let first = reader.w_u32().unwrap();
                let span = reader.w_var().unwrap();
                assert!(!reader.bit().unwrap());
                let width = reader.w_var().unwrap() as usize;
                record.script = Some(ReceivedChunk {
                    first,
                    span,
                    width,
                    data: reader.raw_bits(width).unwrap(),
                });
            }
            if !reader.bit().unwrap() {
                return record;
            }
            assert!(!reader.bit().unwrap()); // no channel-2 launch events
            if reader.bit().unwrap() {
                assert_eq!(reader.var_b().unwrap(), u32::MAX);
                record.projectile_release = Some(reader.var_b().unwrap());
                assert_eq!(reader.bits(2).unwrap(), 0);
                assert_eq!(
                    std::array::from_fn::<_, 3, _>(|_| reader.f32().unwrap()),
                    [0.0, -1000.0, 0.0]
                );
                assert_eq!(reader.bits(5).unwrap(), 0);
                assert!(reader.bit().unwrap()); // impact count is present
                assert!(reader.bit().unwrap()); // short count
                assert_eq!(reader.bits(4).unwrap(), 1);
                assert_eq!(reader.bits(6).unwrap(), 0);
            }
            assert!(reader.bit().unwrap());
            record.movement = reader.bit().unwrap();
            if record.movement {
                let length = reader.bits(12).unwrap() as usize;
                reader.advance(length - 12).unwrap();
            }
            if reader.bit().unwrap() {
                let operation = reader.bits(2).unwrap() as u8;
                record.operation = Some(operation);
                if operation != 3 {
                    let length = reader.bits(14).unwrap() as usize;
                    let start = reader.position();
                    if operation == 1 && !reader.bit().unwrap() {
                        record.definition = Some(reader.guid_index().unwrap());
                        let flags = reader.bits(8).unwrap() as u8;
                        if flags & orpheus_wire::entity::SKIN_THEME != 0 {
                            assert_ne!(flags & orpheus_wire::entity::WITH_SKIN, 0);
                            record.skin =
                                Some((reader.bits(32).unwrap() as u32, reader.bit().unwrap()));
                        }
                    } else if operation == 2 {
                        let mut components = reader;
                        if components.bits(8).unwrap() == 1 && components.bits(8).unwrap() == 51 {
                            components.align();
                            if components.bits(8).unwrap() == 7 && components.bits(56).unwrap() == 1
                            {
                                components.advance(64).unwrap(); // ID and maximum
                                record.base_health = Some(components.f32().unwrap());
                            }
                        }
                    }
                    reader
                        .advance(length - (reader.position() - start))
                        .unwrap();
                }
                assert!(!reader.bit().unwrap());
            }
            record
        })
        .collect()
}

fn input_batch(first: u32, count: u8, yaw: i16) -> Vec<u8> {
    command_batch(first, count, yaw, 0, true)
}

fn command_batch(first: u32, count: u8, yaw: i16, buttons: u32, forward: bool) -> Vec<u8> {
    let mut input = BitWriter::new();
    input.bits(0, 24).unwrap(); // latency and key flags
    input.bits(u64::from(count), 8).unwrap();
    input.bits(u64::from(first), 32).unwrap();
    input.bit(forward); // new throttles
    if forward {
        input.bit(true); // compact throttle direction
        input.bit(true); // moving
        input.bits(2, 3).unwrap();
    }
    input.signed(i64::from(yaw), 16).unwrap();
    input.signed(0, 16).unwrap();
    input.bits(u64::from(buttons), 24).unwrap();
    input.bits(0, 8).unwrap(); // action
    input.bits(0, 16).unwrap(); // extra
    for _ in 1..count {
        input.bit(false); // next consecutive frame
        input.bit(false); // no changed fields
    }
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[], &[]).unwrap();
    payload.bit(true);
    payload.bits(input.bit_len() as u64, 32).unwrap();
    payload.raw_bits(input.as_bytes(), input.bit_len()).unwrap();
    payload.into_bytes()
}

fn input_ack(schemas: &Schemas, packet: &Packet) -> u32 {
    let mut reader = BitReader::new(&packet.payload);
    reader.var_a().unwrap();
    read_channel(&mut reader, schemas).unwrap();
    reader.var_a().unwrap()
}

#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn busy_peers_resume_in_round_robin_order_after_the_global_input_budget() {
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::Command;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let mut state = State::new(Arc::new(fixture()));
    // Five synthetic peers share a prepared fixture solely to test the
    // worker scheduler. The actual admission route remains solo.
    let players: Vec<_> = (0..5)
        .map(|slot| Player {
            info: player().pop().unwrap(),
            entity: player_entity(slot),
            gone: false,
        })
        .collect();
    state.rooms.insert(
        MATCH,
        Room {
            map: scene.map().clone(),
            remaining: players.len(),
            players,
            world: Some(WorldStart {
                scene: Box::new(scene),
                card: PlayerCard::default(),
                combat: None,
            }),
        },
    );
    let first = FIRST_TICK + 1;
    let commands: Vec<_> = (first..first + 32)
        .map(|frame| Command {
            frame,
            ..Command::default()
        })
        .collect();
    let room = &state.rooms[&MATCH];
    for slot in 0..room.players.len() {
        let connection = slot as u32 + 1;
        let viewer = room.appearance(slot).unwrap();
        let mut peer = Peer::new(viewer.entity);
        peer.world
            .start(&room.world.as_ref().unwrap().scene, &viewer, FIRST_TICK)
            .unwrap();
        peer.world.queue_commands(FIRST_TICK, &commands);
        state.seats.insert(connection, (MATCH, slot));
        state.peers.insert(connection, peer);
    }
    let acks = |state: &State| {
        state
            .peers
            .values()
            .map(|peer| peer.world.command_ack(FIRST_TICK))
            .collect::<Vec<_>>()
    };
    assert!(state.advance_input(Duration::ZERO));
    assert_eq!(
        acks(&state),
        [first + 15, first + 15, first + 15, first + 15, FIRST_TICK]
    );
    assert!(state.advance_input(Duration::ZERO));
    assert_eq!(
        acks(&state),
        [first + 31, first + 31, first + 31, first + 15, first + 15]
    );
    assert!(!state.advance_input(Duration::ZERO));
    assert_eq!(acks(&state), [first + 31; 5]);
}

#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn prepared_world_waits_for_ownership_and_retries_baselines_before_scripts() {
    exercise_prepared_world(PreparedFixture::Stock);
}

#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn prepared_worker_retries_a_concrete_owner_vm_on_encrypted_loopback() {
    exercise_prepared_world(PreparedFixture::Owner);
}

#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn prepared_worker_delivers_a_volley_hit_and_replaces_the_bot_over_encrypted_udp() {
    exercise_prepared_world(PreparedFixture::Combat);
}

#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn prepared_worker_delivers_helix_flight_damage_and_predicted_release_over_encrypted_udp() {
    exercise_prepared_world(PreparedFixture::Projectile);
}

#[derive(Clone, Copy)]
enum PreparedFixture {
    Stock,
    Owner,
    Combat,
    Projectile,
}

fn exercise_prepared_world(fixture: PreparedFixture) {
    use crate::practice::{SoloPracticeHandoff, dispatch_scene};
    use orpheus_content::catalog::ItemDb;
    use orpheus_lobby::{
        accounts::Accounts,
        collection::Collection,
        handoff::{CREATE_GAME, GAME_REQUEST},
        handoff::{HANDOFF, HANDOFF_MESSAGE},
        login::Identity,
        session::PreparedLogin,
        templates::RetailTemplates,
    };
    use orpheus_sim::{
        body_script::{fixture_body_factory, fixture_hitscan_factory, fixture_projectile_factory},
        bots::BOT_ENTITY,
        scene_assets::SceneAssets,
    };
    use orpheus_wire::jam::codec::id16;

    const CONTROLLER: u32 = 0xa000_0100;
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let schemas = Arc::new(Schemas::game());
    let lobby = Schemas::lobby();
    let assets = SceneAssets::load(runtime).unwrap();
    let scripted_bots = assets.graphs().entity_body(0x0400_0000_0000_143e).is_some();
    let collection = Collection::new(
        &RetailTemplates::captured(),
        Arc::new(ItemDb::load(&data).unwrap()),
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let accounts = Accounts::new(
        temporary.path().join("profiles"),
        temporary.path().join("seed"),
    )
    .unwrap();
    let account = accounts.get("world fixture").unwrap();
    let login = PreparedLogin {
        identity: Identity::create(account.account_lo, 1).unwrap(),
        account,
        tournament: false,
    };
    let fields = lobby
        .decode(GAME_REQUEST, CREATE_GAME, b"\x02\x00\x04\x00\x00\x00", true)
        .unwrap();
    let request = (GAME_REQUEST, CREATE_GAME, fields);
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let channel = |id: MatchId, kind| {
        Fields::from([
            (0, Value::Struct(id16(id[0], id[1]))),
            (0x10, Value::Unsigned(kind)),
        ])
    };
    let mut outgoing = Vec::new();
    let result = dispatch_scene(
        &request,
        &login,
        &server,
        SoloPracticeHandoff {
            match_id: MATCH,
            host: "127.0.0.1",
            general: channel([7, 8], 7),
            match_channel: channel(MATCH, 6),
        },
        (&assets, &collection),
        |map| {
            let mut scene = assets.prepare(map)?;
            if !matches!(fixture, PreparedFixture::Stock) {
                assert!(!scripted_bots);
                let factory = match fixture {
                    PreparedFixture::Combat => {
                        let [x, y, z] = scene.map().spawn_point(0, 0).unwrap().0;
                        let bot = scene
                            .bots_mut()
                            .iter_mut()
                            .find(|bot| bot.entity == BOT_ENTITY + 1)
                            .unwrap();
                        bot.position = [x - 4.0, y, z];
                        fixture_hitscan_factory(100.0)
                    }
                    PreparedFixture::Projectile => {
                        let [x, y, z] = scene.map().spawn_point(0, 0).unwrap().0;
                        let bots = scene.bots_mut();
                        assert_eq!(
                            bots[..3]
                                .iter()
                                .map(|bot| bot.spawn.team)
                                .collect::<Vec<_>>(),
                            [0, 1, 1]
                        );
                        assert!(bots[..3].iter().all(|bot| bot.spawn.route.is_empty()));
                        bots[0].position = [x - 3.5, y, z];
                        bots[1].position = [x - 5.0, y, z];
                        bots[2].position = [x - 5.0, y, z + 0.8];
                        fixture_projectile_factory(40.0)
                    }
                    _ => fixture_body_factory().0,
                };
                scene.owner_body_factory = Some(Arc::new(move |hero, spawn| {
                    if hero.guid != orpheus_content::heroes::SOLDIER {
                        return Ok(None);
                    }
                    Ok(Some(Box::new(factory.start(spawn)?)))
                }));
            }
            Ok(scene)
        },
        |(crc, id, fields)| {
            let bytes = lobby.encode(*crc, *id, fields).unwrap();
            outgoing.push((*crc, *id, lobby.decode(*crc, *id, &bytes, true).unwrap()));
            Ok(true)
        },
    );
    if scripted_bots {
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Unsupported);
        assert!(outgoing.is_empty());
        return;
    }
    let ticket = result.unwrap().unwrap().handoff;
    assert_eq!(
        outgoing.iter().map(|(_, id, _)| *id).collect::<Vec<_>>(),
        [20404, 20402, HANDOFF_MESSAGE]
    );
    assert_eq!(outgoing[2].0, HANDOFF);
    let Value::Struct(wire) = &outgoing[2].2[&0x80] else {
        panic!("the decoded handoff has no seat body");
    };
    let key = |offset| {
        let Value::Array(bytes) = &wire[&offset] else {
            panic!("the decoded handoff has no direction key");
        };
        std::array::from_fn(|index| match bytes[index] {
            Value::Unsigned(byte) => u8::try_from(byte).unwrap(),
            _ => panic!("the handoff key contains an invalid byte"),
        })
    };
    assert_eq!(wire[&0x28], Value::Unsigned(u64::from(ticket.conn)));
    assert_eq!(
        wire[&0x2c],
        Value::Unsigned(u64::from(server.local_addr().port()))
    );
    let client_ticket = Handoff {
        key_in: key(0xae),
        key_out: key(0xce),
        ..ticket
    };
    let mut client = Client::new(server.local_addr(), &client_ticket);
    client.handshake();
    let load = client.receive();
    assert_eq!(messages(&schemas, &load)[0].message.id, 20300);
    assert!(client.window.accept(load.seq));
    let mut payload = BitWriter::new();
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    let started = Instant::now();
    client.send(ACK, payload.into_bytes());

    let mut ownership = false;
    while started.elapsed() < Duration::from_millis(650) {
        let frame = client.receive();
        assert!(client.window.accept(frame.seq));
        assert!(world_records(&schemas, &frame).is_empty());
        ownership |= messages(&schemas, &frame)
            .iter()
            .any(|message| message.message.id == 20302);
    }
    assert!(ownership);
    client.send(ACK, Vec::new());

    let deadline = Instant::now() + Duration::from_secs(3);
    let lost = loop {
        let frame = client.receive();
        let records = world_records(&schemas, &frame);
        let creates: Vec<_> = records
            .iter()
            .filter(|record| record.is_create())
            .map(|record| record.entity)
            .collect();
        if !creates.is_empty() {
            assert!(creates.contains(&CONTROLLER) && creates.contains(&GAME_MODE_ENTITY));
            break creates; // Deliberately leave the first baseline out of the ACK window.
        }
        assert!(client.window.accept(frame.seq));
        client.send(ACK, Vec::new());
        assert!(Instant::now() < deadline, "waiting for first world creates");
    };
    let mut creates = HashSet::new();
    loop {
        let frame = client.receive();
        assert!(client.window.accept(frame.seq));
        let records = world_records(&schemas, &frame);
        assert!(records.iter().all(|record| record.script.is_none()));
        for record in records.iter().filter(|record| record.is_create()) {
            assert!(creates.insert(record.entity));
        }
        if lost.iter().all(|id| creates.contains(id)) {
            break;
        }
        client.send(ACK, Vec::new());
        assert!(Instant::now() < deadline, "waiting for baseline retransmit");
    }
    assert_eq!(creates.len(), 21);

    // Even after the retransmission, the script stays behind its create ACK.
    let withheld = client.receive();
    assert!(client.window.accept(withheld.seq));
    assert!(
        world_records(&schemas, &withheld)
            .iter()
            .all(|record| record.script.is_none())
    );
    client.send(ACK, Vec::new());

    let mut mode = false;
    let mut controller = false;
    let mut moving_bot = false;
    while !(mode && controller && moving_bot) && Instant::now() < deadline {
        let frame = client.receive();
        assert!(client.window.accept(frame.seq));
        for record in world_records(&schemas, &frame) {
            assert!(!record.is_create());
            if record.movement {
                assert!((BOT_ENTITY..BOT_ENTITY + 15).contains(&record.entity));
                assert!(creates.contains(&record.entity));
                moving_bot = true;
            }
            if let Some(script) = record.script {
                assert_eq!((script.first, script.span), (0, 1));
                if record.entity == GAME_MODE_ENTITY {
                    assert_eq!(script.data, [0x08, 0x5b, 0x06, 0x00, 0x0c]);
                    mode = true;
                } else {
                    assert_eq!(record.entity, CONTROLLER);
                    let mut bits = script.bits();
                    assert_eq!(bits.bits(3).unwrap(), 2);
                    assert!(bits.w_var().unwrap() >= orpheus_core::tick::FIRST_TICK);
                    controller = true;
                }
            }
        }
        client.send(ACK, Vec::new());
    }
    assert!(mode && controller && moving_bot);
    match fixture {
        PreparedFixture::Owner => exercise_owner_vm(&mut client, &schemas),
        PreparedFixture::Combat => {
            exercise_combat(&mut client, &schemas, BOT_ENTITY + 1, BOT_ENTITY + 15)
        }
        PreparedFixture::Projectile => exercise_projectile(&mut client, &schemas),
        PreparedFixture::Stock => {
            exercise_hero_selection(&mut client, &schemas, &assets, &server, &ticket)
        }
    }
    server.stop().unwrap();
}

fn send_game_message(
    client: &mut Client,
    schemas: &Schemas,
    ordinal: u32,
    guid: u64,
    parameters: Vec<u8>,
) {
    let fields = Fields::from([
        (
            0x78,
            Value::Struct(Fields::from([(
                0,
                Value::Unsigned(u64::from(player_entity(0))),
            )])),
        ),
        (0x80, Value::Blob(parameters)),
        (0xa8, Value::Unsigned(guid)),
        (0xb0, Value::Signed(3)),
    ]);
    let message = encode(
        schemas,
        orpheus_content::parameters::CLIENT_GAME_MESSAGE,
        &fields,
    )
    .unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(ordinal, message)], &[]).unwrap();
    client.send(ACK, payload.into_bytes());
}

fn pick_soldier_for_weapon(client: &mut Client, schemas: &Schemas) -> u32 {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};
    let body = player_entity(0) | 1;
    send_game_message(
        client,
        schemas,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        client.send(ACK, Vec::new());
        if world_records(schemas, &packet)
            .iter()
            .any(|record| record.entity == body && record.is_create())
        {
            return BitReader::new(&packet.payload).var_a().unwrap() + 1;
        }
        assert!(Instant::now() < deadline, "waiting for weapon fixture body");
    }
}

fn exercise_combat(client: &mut Client, schemas: &Schemas, target: u32, replacement: u32) {
    let frame = pick_soldier_for_weapon(client, schemas);
    let fire = orpheus_wire::commands::FIRE;
    let commands = command_batch(frame, 1, -16384, fire, false);
    client.send(ACK, commands.clone());
    client.send(ACK, commands);

    let deadline = Instant::now() + Duration::from_secs(7);
    let mut lost = None;
    let mut delivered_hit = false;
    let mut delivered_kill = false;
    let mut delivered_health = false;
    let mut destroyed = false;
    let mut created = false;
    while !created {
        let packet = client.receive();
        let reliable = messages(schemas, &packet);
        let hit = reliable.iter().find(|item| item.message.id == 25001);
        if lost.is_none() {
            if let Some(hit) = hit {
                lost = Some((hit.sequence, hit.message.fields.clone()));
                continue; // let a following ACK reveal loss of the original hit
            }
        }
        for item in reliable {
            match item.message.id {
                25001 => {
                    let (ordinal, fields) = lost.as_ref().unwrap();
                    assert_eq!((&item.sequence, &item.message.fields), (ordinal, fields));
                    assert_eq!(fields[&0x84], Value::Unsigned(u64::from(frame)));
                    let Value::Struct(impact) = &fields[&0x78] else {
                        panic!("hit body")
                    };
                    assert_eq!(impact[&4], Value::Unsigned(200));
                    assert_eq!(impact[&8], Value::Unsigned(3));
                    delivered_hit = true;
                }
                25005 => {
                    let Value::Struct(victim) = &item.message.fields[&0x84] else {
                        panic!("elimination body")
                    };
                    assert_eq!(victim[&0], Value::Unsigned(u64::from(target)));
                    delivered_kill = true;
                }
                _ => {}
            }
        }
        for record in world_records(schemas, &packet) {
            if record.entity == target {
                if record.operation == Some(2) {
                    delivered_health = true;
                }
                if record.operation == Some(3) {
                    assert!(delivered_hit && delivered_kill && delivered_health);
                    destroyed = true;
                }
            } else if record.entity == replacement && record.is_create() {
                assert!(destroyed);
                created = true;
            }
        }
        assert!(client.window.accept(packet.seq));
        client.send(ACK, Vec::new());
        assert!(
            Instant::now() < deadline,
            "waiting for combat recovery and bot respawn"
        );
    }
}

fn exercise_projectile(client: &mut Client, schemas: &Schemas) {
    use orpheus_sim::{
        body_script::FIXTURE_PROJECTILE_DEFINITION, bots::BOT_ENTITY, projectiles::ProjectileLeases,
    };
    use orpheus_wire::commands::SECONDARY;

    let frame = pick_soldier_for_weapon(client, schemas);
    let definition = FIXTURE_PROJECTILE_DEFINITION;
    let original = ProjectileLeases::new(0, frame, [definition])
        .unwrap()
        .entity(definition, frame, 0)
        .unwrap();
    let commands = command_batch(frame, 1, -16384, SECONDARY, false);
    client.send(ACK, commands.clone());
    client.send(ACK, commands);

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut noticed = HashSet::new();
    let mut impacts = Vec::new();
    let mut health = HashMap::new();
    let mut creates = HashSet::new();
    let mut releases = HashSet::new();
    let mut lost_original = false;
    while !releases.contains(&original) || health.len() < 2 || impacts.len() < 3 {
        let packet = client.receive();
        let tick = BitReader::new(&packet.payload).var_a().unwrap();
        let records = world_records(schemas, &packet);
        if !creates.contains(&original) {
            assert!(!records.iter().any(|record| {
                record.entity == original && record.projectile_release.is_some()
            }));
        }
        if !lost_original
            && records.iter().any(|record| {
                record.entity == original && record.definition == Some(definition as u32)
            })
        {
            lost_original = true;
            continue; // a later ACK will expose the lost confirmation datagram
        }
        for item in messages(schemas, &packet) {
            assert_ne!(
                item.message.id, 25005,
                "one Helix flight cannot eliminate this target"
            );
            if item.message.id != 25001 || !noticed.insert(item.sequence) {
                continue;
            }
            let fields = &item.message.fields;
            let Value::Unsigned(impact_frame) = fields[&0x84] else {
                panic!("impact frame")
            };
            assert!(impact_frame > u64::from(frame));
            let Value::Struct(impact) = &fields[&0x78] else {
                panic!("hit body")
            };
            let Value::Struct(target) = &impact[&0] else {
                panic!("hit target")
            };
            assert_eq!(impact[&8], Value::Unsigned(0));
            impacts.push((target[&0].clone(), impact[&4].clone()));
        }
        for record in records {
            if let Some(current) = record.base_health {
                health.insert(record.entity, current);
            }
            if record.definition == Some(definition as u32) {
                creates.insert(record.entity);
            }
            if let Some(release_frame) = record.projectile_release {
                assert!(creates.contains(&record.entity));
                assert_eq!(record.operation, Some(3));
                assert!(release_frame > tick);
                releases.insert(record.entity);
            }
        }
        assert!(client.window.accept(packet.seq));
        client.send(ACK, Vec::new());
        assert!(
            Instant::now() < deadline,
            "waiting for Helix: shot {frame}, entity {original:#x}, impacts {impacts:?}, health {health:?}, creates {creates:?}, releases {releases:?}"
        );
    }
    assert!(lost_original);
    let impact = |entity, damage| (Value::Unsigned(u64::from(entity)), Value::Unsigned(damage));
    assert_eq!(
        impacts,
        [
            impact(BOT_ENTITY + 1, 40),
            impact(BOT_ENTITY + 1, 80),
            impact(BOT_ENTITY + 2, 80)
        ]
    );
    assert_eq!(
        health,
        HashMap::from([(BOT_ENTITY + 1, 80.0), (BOT_ENTITY + 2, 120.0)])
    );
}

fn exercise_owner_vm(client: &mut Client, schemas: &Schemas) {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};

    let owner = player_entity(0);
    let body = owner | 1;
    send_game_message(
        client,
        schemas,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let start = loop {
        let packet = client.receive();
        let records = world_records(schemas, &packet);
        assert!(
            records
                .iter()
                .all(|record| record.entity != body || record.script.is_none())
        );
        assert!(client.window.accept(packet.seq));
        client.send(ACK, Vec::new());
        if records
            .iter()
            .any(|record| record.entity == body && record.is_create())
        {
            let controller = records
                .iter()
                .find(|record| record.entity == owner && record.script.is_some())
                .unwrap();
            let mut bits = controller.script.as_ref().unwrap().bits();
            assert_eq!(bits.bits(3).unwrap(), 2);
            break bits.w_var().unwrap();
        }
        assert!(Instant::now() < deadline, "waiting for fixture body create");
    };
    let wait_chunk = |client: &mut Client| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let packet = client.receive();
            if let Some(script) = world_records(schemas, &packet)
                .into_iter()
                .find(|record| record.entity == body && record.script.is_some())
                .and_then(|record| record.script)
            {
                return (packet.seq, script);
            }
            assert!(client.window.accept(packet.seq));
            client.send(ACK, Vec::new());
            assert!(Instant::now() < deadline, "waiting for fixture body script");
        }
    };
    let skip_chunk = |client: &mut Client, payload| {
        let packet = client.receive();
        assert!(
            world_records(schemas, &packet)
                .iter()
                .all(|record| record.entity != body || record.script.is_none())
        );
        assert!(client.window.accept(packet.seq));
        client.send(ACK, payload);
    };
    let accept_chunk = |client: &mut Client, sequence| {
        assert!(client.window.accept(sequence));
        client.send(ACK, Vec::new());
    };
    let check_delta = |chunk: &ReceivedChunk, corrected, command_frame| {
        let mut bits = chunk.bits();
        assert_eq!(bits.bits(3).unwrap(), if corrected { 6 } else { 2 });
        assert_eq!(bits.w_var().unwrap(), command_frame);
    };

    let (_, h0) = wait_chunk(client);
    assert_eq!((h0.first, h0.span), (0, 1));
    assert_eq!(h0.bits().bits(2).unwrap(), 0);
    skip_chunk(client, Vec::new());
    let (sequence, retry) = wait_chunk(client);
    assert_eq!(retry, h0);
    accept_chunk(client, sequence);

    let (_, c0) = wait_chunk(client);
    assert_eq!((c0.first, c0.span), (2, 0));
    check_delta(&c0, false, start);
    skip_chunk(client, input_batch(start + 1, 16, 0));
    let (sequence, retry) = wait_chunk(client);
    assert_eq!(retry, c0);
    accept_chunk(client, sequence);

    let (_, c1) = wait_chunk(client);
    assert_eq!((c1.first, c1.span), (3, 0));
    check_delta(&c1, true, start + 16);
    skip_chunk(client, input_batch(start + 17, 1, 0));
    let (sequence, cumulative) = wait_chunk(client);
    assert_eq!((cumulative.first, cumulative.span), (3, 1));
    check_delta(&cumulative, true, start + 17);
    accept_chunk(client, sequence);
}

fn exercise_hero_selection(
    client: &mut Client,
    schemas: &Schemas,
    assets: &orpheus_sim::scene_assets::SceneAssets,
    server: &GameService,
    ticket: &Handoff,
) {
    use orpheus_content::{
        heroes::SOLDIER,
        parameters::{OPEN_OR_CLOSE, PICK_HERO},
    };
    const OWNER: u32 = 0xa000_0100;
    const FIRST: u32 = OWNER | 1;
    const SECOND: u32 = OWNER | 2;

    let send = |client: &mut Client, ordinal, guid, parameters| {
        send_game_message(client, schemas, ordinal, guid, parameters);
    };
    let next_body_frame = |client: &mut Client, body| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let packet = client.receive();
            let records = world_records(schemas, &packet);
            if records
                .iter()
                .any(|record| record.entity == body && record.is_create())
            {
                break (packet, records);
            }
            assert!(client.window.accept(packet.seq));
            client.send(ACK, Vec::new());
            assert!(Instant::now() < deadline, "waiting for the selected body");
        }
    };
    let describe = |records: &[WorldRecord]| {
        records
            .iter()
            .filter(|record| matches!(record.entity, OWNER | FIRST | SECOND))
            .map(|record| {
                (
                    record.entity,
                    record.operation,
                    record.script.as_ref().map(|script| script.span),
                )
            })
            .collect::<Vec<_>>()
    };
    let controller =
        orpheus_sim::controller::Controller::load(assets.graphs(), PRACTICE_RANGE_MODE).unwrap();
    let verify_controller = |records: &[WorldRecord], hero, open| {
        let script = records
            .iter()
            .find(|record| record.entity == OWNER && record.script.is_some())
            .and_then(|record| record.script.as_ref())
            .expect("hero selection controller frame");
        assert_eq!(script.first, 0);
        let mut reader = script.bits();
        assert_eq!(reader.bits(3).unwrap(), 2);
        let cmfd = reader.w_var().unwrap();
        let expected = controller
            .controller_frame(
                cmfd,
                ControllerFrame {
                    select_open: open,
                    skins: open,
                    hide_assemble: true,
                    hero: Some(hero),
                    ..ControllerFrame::default()
                },
            )
            .unwrap();
        assert_eq!(script.width, expected.bit_len());
        assert_eq!(script.data, expected.as_bytes());
        cmfd
    };

    send(client, 1, PICK_HERO, SOLDIER.to_le_bytes().to_vec());
    let (_lost, records) = next_body_frame(client, FIRST);
    let expected = [
        (FIRST, Some(1), None),
        (OWNER, Some(2), None),
        (OWNER, None, Some(2)),
    ];
    assert_eq!(describe(&records), expected);
    assert!(
        records
            .iter()
            .any(|record| record.entity == FIRST && record.movement)
    );
    let first_cmfd = verify_controller(&records, SOLDIER, false);

    // A later datagram ACK identifies the first body batch as lost.
    let following = client.receive();
    assert!(client.window.accept(following.seq));
    assert!(describe(&world_records(schemas, &following)).is_empty());
    client.send(ACK, Vec::new());
    let (retry, records) = next_body_frame(client, FIRST);
    assert_eq!(describe(&records), expected);
    assert_eq!(verify_controller(&records, SOLDIER, false), first_cmfd);
    assert!(client.window.accept(retry.seq));

    let tick = BitReader::new(&retry.payload).var_a().unwrap();
    let first_input = tick + 32;
    let mut newest = first_input + 4;
    let await_input = |client: &mut Client, expected| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let packet = client.receive();
            assert!(client.window.accept(packet.seq));
            if input_ack(schemas, &packet) == expected {
                // The stock Soldier: 76 script turns on owner corrections. The
                // owner's own body moves only by a correction: at most one per
                // packet, the last record, and only after its create was acked
                // (the ACK that followed the retried body frame).
                let records = world_records(schemas, &packet);
                let own: Vec<_> = records
                    .iter()
                    .enumerate()
                    .filter(|(_, record)| record.entity == FIRST && record.movement)
                    .map(|(index, _)| index)
                    .collect();
                assert!(own.len() <= 1, "{own:?}");
                assert!(own.iter().all(|&index| index + 1 == records.len()));
                return;
            }
            client.send(ACK, Vec::new());
            assert!(
                Instant::now() < deadline,
                "waiting for command acknowledgement"
            );
        }
    };
    client.send(ACK, input_batch(first_input, 5, -31010));
    await_input(client, newest);
    client.send(ACK, input_batch(newest + 2_000, 1, -31010));
    for _ in 0..2 {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        assert_eq!(input_ack(schemas, &packet), newest);
        client.send(ACK, Vec::new());
    }
    newest += 48;
    let recoverable = input_batch(newest, 1, -31010);
    for _ in 0..=RECEIVE_BATCH {
        client.send(ACK, recoverable.clone());
    }
    await_input(client, newest);

    let mut open = vec![1, 1];
    open.extend(OWNER.to_le_bytes());
    open.extend(OWNER.to_le_bytes());
    send(client, 2, OPEN_OR_CLOSE, open);
    let deadline = Instant::now() + Duration::from_secs(2);
    let reopen_cmfd = loop {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        let records = world_records(schemas, &packet);
        if !describe(&records).is_empty() {
            assert_eq!(describe(&records), [(OWNER, None, Some(3))]);
            break verify_controller(&records, SOLDIER, true);
        }
        assert!(
            Instant::now() < deadline,
            "waiting for hero selection to reopen"
        );
    };
    assert!(reopen_cmfd > first_cmfd);

    let tracer = assets.heroes().find_hero("Tracer").unwrap().guid;
    send(client, 3, PICK_HERO, tracer.to_le_bytes().to_vec());
    let (swap, records) = next_body_frame(client, SECOND);
    assert_eq!(
        describe(&records),
        [
            (SECOND, Some(1), None),
            (OWNER, Some(2), None),
            (FIRST, Some(3), None),
            (OWNER, None, Some(4))
        ],
    );
    assert!(verify_controller(&records, tracer, false) > reopen_cmfd);
    assert!(client.window.accept(swap.seq));
    client.send(ACK, Vec::new());

    let skin = SkinTheme {
        guid: 0x0a50_0000_0000_1234,
        golden: true,
    };
    server.update_skin(ticket, tracer, skin).unwrap();
    let third = OWNER | 3;
    let (reskin, records) = next_body_frame(client, third);
    assert_eq!(
        records
            .iter()
            .find(|record| record.entity == third)
            .unwrap()
            .skin,
        Some((0x1234, true))
    );
    assert!(
        records
            .iter()
            .any(|record| record.entity == SECOND && record.operation == Some(3))
    );
    assert!(client.window.accept(reskin.seq));
    client.send(ACK, Vec::new());
}
