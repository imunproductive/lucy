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

#[derive(Clone, Debug, Eq, PartialEq)]
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
#[ignore = "requires a derived 104319 Practice Range scene"]
fn busy_peers_resume_in_round_robin_order_after_the_global_input_budget() {
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::Command;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let mut state = State::new(Arc::new(fixture()));
    // Five synthetic peers share a prepared fixture solely to test the
    // worker scheduler.
    let players: Vec<_> = (0..5)
        .map(|_| WorldPlayer {
            player: player().pop().unwrap(),
            card: PlayerCard::default(),
            hero: 0,
        })
        .collect();
    let map = scene.map().clone();
    let world = WorldStart {
        scene: Box::new(scene),
        combat: None,
    };
    state
        .rooms
        .insert(MATCH, Room::new(map, players, Some(world)));
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
#[ignore = "requires a derived 104319 Practice Range scene"]
fn prepared_world_waits_for_ownership_and_retries_baselines_before_scripts() {
    exercise_prepared_world(PreparedFixture::Stock);
}

#[test]
#[ignore = "requires a derived 104319 Practice Range scene without bot VM graphs"]
fn prepared_worker_retries_a_concrete_owner_vm_on_encrypted_loopback() {
    exercise_prepared_world(PreparedFixture::Owner);
}

#[test]
#[ignore = "requires a derived 104319 Practice Range scene without bot VM graphs"]
fn prepared_worker_delivers_a_volley_hit_and_replaces_the_bot_over_encrypted_udp() {
    exercise_prepared_world(PreparedFixture::Combat);
}

#[test]
#[ignore = "requires a derived 104319 Practice Range scene without bot VM graphs"]
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
        [20404, 20402, 23310, HANDOFF_MESSAGE]
    );
    assert_eq!(outgoing[3].0, HANDOFF);
    let Value::Struct(wire) = &outgoing[3].2[&0x80] else {
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
    send_player_message(client, schemas, player_entity(0), ordinal, guid, parameters);
}

fn send_player_message(
    client: &mut Client,
    schemas: &Schemas,
    owner: u32,
    ordinal: u32,
    guid: u64,
    parameters: Vec<u8>,
) {
    let fields = Fields::from([
        (
            0x78,
            Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(owner)))])),
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
    let controller = orpheus_sim::controller::Controller::load(
        assets.graphs(),
        orpheus_content::maps::PRACTICE_RANGE_MODE,
    )
    .unwrap();
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
                // packet, as the last record. This run produces none before the
                // create is acked, so the create-first rule is tested by
                // `owner_correction_respects_create_budget_and_failed_transport`.
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

/// One client of a shared world and what it received, datagram by datagram.
struct Watcher {
    client: Client,
    owner: u32,
    tick: u32,
    sequences: HashSet<u32>,
    datagrams: Vec<(Vec<orpheus_wire::messages::Message>, Vec<WorldRecord>)>,
}

impl Watcher {
    fn pump(&mut self, schemas: &Schemas) {
        let packet = self.client.receive();
        if packet.kind != KIND_FRAME {
            return;
        }
        self.tick = BitReader::new(&packet.payload).var_a().unwrap();
        let messages = messages(schemas, &packet)
            .into_iter()
            .filter(|item| self.sequences.insert(item.sequence))
            .map(|item| item.message)
            .collect();
        let records = world_records(schemas, &packet);
        self.client.window.accept(packet.seq);
        self.client.send(ACK, Vec::new());
        self.datagrams.push((messages, records));
    }

    /// The first datagram from `from` with a record that passes `test`.
    fn find(&self, from: usize, test: impl Fn(&WorldRecord) -> bool) -> Option<usize> {
        (from..self.datagrams.len()).find(|&index| self.datagrams[index].1.iter().any(&test))
    }

    fn created(&self, entity: u32) -> Option<usize> {
        self.find(0, |record| record.entity == entity && record.is_create())
    }

    /// The first 20308 card with this BattleTag from `from` on, and its flags.
    fn card(&self, from: usize, tag: &str) -> Option<(usize, [bool; 4])> {
        (from..self.datagrams.len()).find_map(|index| {
            self.datagrams[index].0.iter().find_map(|message| {
                let Value::Struct(card) = message.fields.get(&0x80)? else {
                    return None;
                };
                (message.id == 20308 && card.get(&0x40) == Some(&Value::String(tag.into()))).then(
                    || {
                        let flag = |offset| message.fields.get(&offset) == Some(&Value::Bool(true));
                        (index, [0x78, 0x79, 0x7a, 0x7b].map(flag))
                    },
                )
            })
        })
    }
}

fn pump_until(
    watchers: &mut [&mut Watcher],
    schemas: &Schemas,
    what: &str,
    done: impl Fn(&[&mut Watcher]) -> bool,
) {
    pump_within(watchers, schemas, what, Duration::from_secs(10), done);
}

fn pump_within(
    watchers: &mut [&mut Watcher],
    schemas: &Schemas,
    what: &str,
    within: Duration,
    done: impl Fn(&[&mut Watcher]) -> bool,
) {
    let deadline = Instant::now() + within;
    while !done(watchers) {
        for watcher in watchers.iter_mut() {
            watcher.pump(schemas);
        }
        assert!(Instant::now() < deadline, "waiting for {what}");
    }
}

/// Two clients in one Practice Range see each other, in the order of
/// ow174/game/match.py: the other's card and controller entity, his team
/// list entry once the entity arrived, his body, his possession once the
/// body arrived, a movement record for his body in every send frame from the
/// frame after its create, his body's statescript once the create arrived;
/// a hero switch replaces his body, and leaving posts "left the game" and
/// destroys his body and entity.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn two_players_in_one_practice_range_see_each_other() {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::FIRE;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    assert!(scene.movement_ready());
    let tracer = assets.heroes().find_hero("Tracer").unwrap().guid;
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let tags = ["first#1111", "second#2222"];
    let members = tags
        .iter()
        .enumerate()
        .map(|(index, tag)| WorldPlayer {
            player: MatchPlayer {
                name: (*tag).into(),
                team: 1,
                tournament: false,
                ranked: false,
            },
            card: PlayerCard {
                account: [index as u64 + 7, 0x0100_0000_0000_0000],
                battle_tag: (*tag).into(),
                record: Fields::from([(0x40, Value::String((*tag).into()))]),
                ..PlayerCard::default()
            },
            hero: 0,
        })
        .collect();
    let tickets = server.open_practice_party(MATCH, scene, members).unwrap();
    assert_eq!(tickets.len(), 2);
    let mut watchers: Vec<_> = tickets
        .iter()
        .enumerate()
        .map(|(slot, ticket)| {
            let mut client = Client::new(server.local_addr(), ticket);
            client.handshake();
            Watcher {
                client,
                owner: player_entity(slot),
                tick: 0,
                sequences: HashSet::new(),
                datagrams: Vec::new(),
            }
        })
        .collect();
    let [first, second] = &mut watchers[..] else {
        unreachable!()
    };
    let (first_owner, second_owner) = (first.owner, second.owner);
    pump_until(
        &mut [first, second],
        &schemas,
        "the instances",
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher.datagrams.iter().any(|(messages, _)| {
                    messages.iter().any(|message| {
                        message.id == 20300 && message.fields[&0x80] != Value::Struct(Fields::new())
                    })
                })
            })
        },
    );
    for watcher in [&mut *first, &mut *second] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    pump_until(
        &mut [first, second],
        &schemas,
        "the controllers",
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher
                    .find(0, |record| {
                        record.entity == watcher.owner && record.script.is_some()
                    })
                    .is_some()
            })
        },
    );
    for watcher in [&mut *first, &mut *second] {
        let owner = watcher.owner;
        send_player_message(
            &mut watcher.client,
            &schemas,
            owner,
            1,
            PICK_HERO,
            SOLDIER.to_le_bytes().to_vec(),
        );
    }
    let sees = |watcher: &Watcher, other: u32| {
        let body = other | 1;
        watcher.created(other).is_some()
            && watcher
                .find(0, |record| {
                    record.entity == other && record.script.is_some()
                })
                .is_some()
            && watcher.created(body).is_some()
            && watcher
                .find(0, |record| {
                    record.entity == other && record.operation == Some(2)
                })
                .is_some()
            && watcher
                .find(0, |record| record.entity == body && record.movement)
                .is_some()
            && watcher
                .find(0, |record| record.entity == body && record.script.is_some())
                .is_some()
    };
    pump_until(
        &mut [first, second],
        &schemas,
        "the other players",
        |watchers| sees(watchers[0], second_owner) && sees(watchers[1], first_owner),
    );

    // The second player fires: his body's statescript changes, and the first
    // client gets a delta of it.
    let frame = second.tick + 1;
    second
        .client
        .send(ACK, command_batch(frame, 16, 0, FIRE, false));
    let second_body = second_owner | 1;
    pump_until(
        &mut [first, second],
        &schemas,
        "an observer delta",
        |watchers| {
            watchers[0]
                .find(0, |record| {
                    record.entity == second_body
                        && record
                            .script
                            .as_ref()
                            .is_some_and(|script| script.first != 0)
                })
                .is_some()
        },
    );

    for (viewer, other, tag) in [
        (&*first, second_owner, tags[1]),
        (&*second, first_owner, tags[0]),
    ] {
        let body = other | 1;
        // Its own card first, then the other's card with his entity.
        let own = tags[usize::from(viewer.owner == second_owner)];
        let (own_card, own_flags) = viewer.card(0, own).expect("own card");
        assert_eq!(own_flags, [false; 4]);
        let (card, _) = viewer.card(0, tag).expect("the other's card");
        let created = viewer.created(other).unwrap();
        assert!(own_card <= card && card <= created);
        // 288D after the entity's create arrived.
        let entry = viewer
            .find(0, |record| {
                record.entity == other && record.script.is_some()
            })
            .unwrap();
        assert!(entry > created);
        let body_created = viewer.created(body).unwrap();
        assert!(body_created >= created);
        // 44/75 once both creates arrived.
        let possession = viewer
            .find(0, |record| {
                record.entity == other && record.operation == Some(2)
            })
            .unwrap();
        assert!(possession > body_created && possession > created);
        // A movement record in every frame after the create's.
        let moved = viewer
            .find(0, |record| record.entity == body && record.movement)
            .unwrap();
        assert!(moved > body_created);
        for (index, (_, records)) in viewer.datagrams.iter().enumerate().skip(moved) {
            assert!(
                records
                    .iter()
                    .any(|record| record.entity == body && record.movement),
                "datagram {index} has no movement record of {body:08X}"
            );
        }
        // The statescript: a plain full frame once the create arrived.
        let full = viewer
            .find(0, |record| record.entity == body && record.script.is_some())
            .unwrap();
        assert!(full > body_created);
        let (_, records) = &viewer.datagrams[full];
        let chunk = records
            .iter()
            .find_map(|record| record.script.as_ref().filter(|_| record.entity == body))
            .unwrap();
        assert_eq!(chunk.first, 0);
        assert_eq!(chunk.bits().bits(2).unwrap(), 0, "an H=0 frame");
    }

    // The first player switches to Tracer: the second client loses his old
    // body and gets the new one, its possession and a new team list entry.
    // Tracer's body runs no statescript.
    let switched = second.datagrams.len();
    send_player_message(
        &mut first.client,
        &schemas,
        first_owner,
        2,
        PICK_HERO,
        tracer.to_le_bytes().to_vec(),
    );
    let (old_body, new_body) = (first_owner | 1, first_owner | 2);
    pump_until(&mut [first, second], &schemas, "the switch", |watchers| {
        let viewer = &watchers[1];
        viewer.created(new_body).is_some_and(|created| {
            viewer
                .find(created + 1, |record| {
                    record.entity == first_owner && record.operation == Some(2)
                })
                .is_some()
        })
    });
    let destroyed = second
        .find(switched, |record| {
            record.entity == old_body && record.operation == Some(3)
        })
        .unwrap();
    let created = second.created(new_body).unwrap();
    assert!(destroyed <= created);
    assert!(
        second
            .find(switched, |record| record.entity == first_owner
                && record.script.is_some())
            .is_some()
    );
    for _ in 0..20 {
        first.pump(&schemas);
        second.pump(&schemas);
    }
    assert!(
        second
            .find(created, |record| record.entity == new_body
                && record.movement)
            .is_some()
    );
    assert!(
        second
            .find(0, |record| record.entity == new_body
                && record.script.is_some())
            .is_none()
    );

    // The first player leaves: "left the game", then his body and entity go.
    let left = second.datagrams.len();
    first.client.send(FIN | ACK, Vec::new());
    pump_until(&mut [second], &schemas, "the leave", |watchers| {
        watchers[0]
            .find(left, |record| {
                record.entity == first_owner && record.operation == Some(3)
            })
            .is_some()
    });
    let (notice, flags) = second.card(left, tags[0]).expect("left the game");
    assert_eq!(flags, [true, false, false, true]);
    let body_gone = second
        .find(left, |record| {
            record.entity == new_body && record.operation == Some(3)
        })
        .unwrap();
    let entity_gone = second
        .find(left, |record| {
            record.entity == first_owner && record.operation == Some(3)
        })
        .unwrap();
    assert!(notice <= body_gone && body_gone <= entity_gone);
    let (_, records) = &second.datagrams[entity_gone];
    if body_gone == entity_gone {
        let position = |entity| {
            records
                .iter()
                .position(|record| record.entity == entity && record.operation == Some(3))
        };
        assert!(position(new_body) < position(first_owner));
    }
    assert!(server.seat_exists(&tickets[1]).unwrap());

    // The match ends with its last player.
    second.client.send(FIN | ACK, Vec::new());
    let deadline = Instant::now() + Duration::from_secs(2);
    while server.seat_exists(&tickets[1]).unwrap() {
        assert!(Instant::now() < deadline, "waiting for the match to end");
        thread::sleep(Duration::from_millis(10));
    }
    server.stop().unwrap();
}

/// A PvP match on a map other than the Practice Range (ow174/game/match.py,
/// server.py): its collision is built from the install while two players
/// load, and both spawn on it. The first spawn starts "assemble your team":
/// the game mode entity counts v1303 down every second, hero select stays
/// open after a pick with the skin selector on, and closes when the 30 s are
/// up. A third player then joins the running match and sees the others.
#[test]
#[ignore = "requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR); runs over 30 s"]
fn a_pvp_match_on_another_map_assembles_its_team_and_takes_a_third_player() {
    use orpheus_content::{heroes::SOLDIER, maps::MapCatalog, parameters::PICK_HERO};
    use orpheus_sim::{controller::Controller, scene_assets::SceneAssets};

    const WORKSHOP_CHAMBER: u64 = 0x0800_0000_0000_0c48;
    const SKIRMISH: u64 = 0x0230_0000_0000_0010;

    let runtime = orpheus_testkit::runtime_dir();
    let collision = tempfile::tempdir().unwrap();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_folder(collision.path())
            .with_collision_build(collision_build(orpheus_testkit::game_dir())),
    );
    let catalog = MapCatalog::load(runtime.join("data/game_maps_174.json")).unwrap();
    let map = catalog
        .game_map(WORKSHOP_CHAMBER, SKIRMISH, "Skirmish", false, &[6, 6])
        .expect("Workshop Chamber in Skirmish");
    let controller = Controller::load(assets.graphs(), SKIRMISH).unwrap();
    assert!(!controller.practice());
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let player = |tag: &str, team, account| WorldPlayer {
        player: MatchPlayer {
            name: tag.into(),
            team,
            tournament: false,
            ranked: false,
        },
        card: PlayerCard {
            account: [account, 0x0100_0000_0000_0000],
            battle_tag: tag.into(),
            record: Fields::from([(0x40, Value::String(tag.into()))]),
            ..PlayerCard::default()
        },
        hero: 0,
    };
    let prepare = {
        let (assets, map) = (Arc::clone(&assets), map.clone());
        Box::new(move || assets.prepare(map))
    };
    let tickets = server
        .open_world(
            MATCH,
            map.clone(),
            vec![player("first#1111", 0, 7), player("second#2222", 1, 8)],
            prepare,
        )
        .unwrap();
    let watch = |ticket: &Handoff, slot| {
        let mut client = Client::new(server.local_addr(), ticket);
        client.handshake();
        Watcher {
            client,
            owner: player_entity(slot),
            tick: 0,
            sequences: HashSet::new(),
            datagrams: Vec::new(),
        }
    };
    let instance = |watcher: &Watcher| {
        watcher.datagrams.iter().any(|(messages, _)| {
            messages.iter().any(|message| {
                message.id == 20300 && message.fields[&0x80] != Value::Struct(Fields::new())
            })
        })
    };
    let load = |watcher: &mut Watcher| {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    };
    // The owner's controller frames, in order, with their datagram.
    let controllers = |watcher: &Watcher| -> Vec<(usize, ReceivedChunk)> {
        let mut frames = Vec::new();
        for (index, (_, records)) in watcher.datagrams.iter().enumerate() {
            for record in records {
                if let (true, Some(script)) = (record.entity == watcher.owner, &record.script) {
                    frames.push((index, script.clone()));
                }
            }
        }
        frames
    };
    let mode_frames = |watcher: &Watcher| -> Vec<(usize, ReceivedChunk)> {
        let mut frames = Vec::new();
        for (index, (_, records)) in watcher.datagrams.iter().enumerate() {
            for record in records {
                if let (GAME_MODE_ENTITY, Some(script)) = (record.entity, &record.script) {
                    frames.push((index, script.clone()));
                }
            }
        }
        frames
    };
    // Whether a controller frame shows hero select so (match.py
    // `send_controller`: no assemble title hidden in the PvP modes).
    let shows = |chunk: &ReceivedChunk, open: bool, skins: bool, hero: Option<u64>| {
        let mut reader = chunk.bits();
        if chunk.first != 0 || reader.bits(3).unwrap() != 2 {
            return false;
        }
        let cmfd = reader.w_var().unwrap();
        let expected = controller
            .controller_frame(
                cmfd,
                ControllerFrame {
                    select_open: open,
                    skins,
                    hide_assemble: false,
                    hero,
                    ..ControllerFrame::default()
                },
            )
            .unwrap();
        (chunk.width, chunk.data.as_slice()) == (expected.bit_len(), expected.as_bytes())
    };

    let mut watchers: Vec<_> = tickets
        .iter()
        .enumerate()
        .map(|(slot, ticket)| watch(ticket, slot))
        .collect();
    let [first, second] = &mut watchers[..] else {
        unreachable!()
    };
    pump_until(
        &mut [first, second],
        &schemas,
        "the instances",
        |watchers| watchers.iter().all(|watcher| instance(watcher)),
    );
    load(first);
    load(second);
    // The world waits for the collision, which is built on its own thread.
    pump_within(
        &mut [first, second],
        &schemas,
        "the controllers on the built collision",
        Duration::from_secs(600),
        |watchers| {
            watchers
                .iter()
                .all(|watcher| !controllers(watcher).is_empty())
        },
    );
    assert!(collision.path().join("0C48.col").is_file());
    for watcher in [&*first, &*second] {
        assert!(watcher.created(0x8000_0000 | map.placeables[0]).is_some());
        assert!(watcher.created(watcher.owner).is_some());
        assert!(watcher.created(GAME_MODE_ENTITY).is_some());
        // Assembling already: hero select opens with the skin selector.
        let (_, first_frame) = &controllers(watcher)[0];
        assert!(shows(first_frame, true, true, None));
    }

    // A pick creates the body and keeps the screen open while the heroes
    // assemble.
    for watcher in [&mut *first, &mut *second] {
        let owner = watcher.owner;
        send_player_message(
            &mut watcher.client,
            &schemas,
            owner,
            1,
            PICK_HERO,
            SOLDIER.to_le_bytes().to_vec(),
        );
    }
    pump_until(&mut [first, second], &schemas, "the bodies", |watchers| {
        watchers.iter().all(|watcher| {
            watcher.created(watcher.owner | 1).is_some_and(|created| {
                controllers(watcher).iter().any(|(index, chunk)| {
                    *index >= created && shows(chunk, true, true, Some(SOLDIER))
                })
            })
        })
    });

    // The countdown ends: hero select closes for the players with a hero.
    pump_within(
        &mut [first, second],
        &schemas,
        "the end of the assembly",
        Duration::from_secs(40),
        |watchers| {
            watchers.iter().all(|watcher| {
                controllers(watcher)
                    .iter()
                    .any(|(_, chunk)| shows(chunk, false, false, Some(SOLDIER)))
            })
        },
    );
    let scene = assets.prepare(map.clone()).unwrap();
    let counting = scene.countdown_frame(12.5).unwrap();
    let over = scene.countdown_frame(0.0).unwrap();
    for watcher in [&*first, &*second] {
        let frames = mode_frames(watcher);
        let (last_index, last) = frames.last().unwrap();
        // One frame a second for 30 s, the mode entity's first ones.
        assert!(
            (25..=32).contains(&frames.len()),
            "{} countdown frames",
            frames.len()
        );
        for (_, chunk) in &frames[..frames.len() - 1] {
            assert_eq!(chunk.width, counting.bit_len());
        }
        assert_eq!(
            (last.width, last.data.as_slice()),
            (over.bit_len(), over.as_bytes())
        );
        let (closed, _) = controllers(watcher)
            .into_iter()
            .find(|(_, chunk)| shows(chunk, false, false, Some(SOLDIER)))
            .unwrap();
        assert!(closed >= *last_index);
    }

    // A third player joins the running match: no countdown, hero select
    // open without the skin selector, and the others in his world.
    let joined = server
        .join_match(MATCH, vec![player("third#3333", 0, 9)])
        .unwrap()
        .expect("the running match takes a player");
    assert_eq!(joined.len(), 1);
    let mut third = watch(&joined[0], 2);
    pump_until(
        &mut [first, second, &mut third],
        &schemas,
        "his instance",
        |watchers| instance(watchers[2]),
    );
    load(&mut third);
    let (first_owner, second_owner, third_owner) = (first.owner, second.owner, third.owner);
    pump_until(
        &mut [first, second, &mut third],
        &schemas,
        "the third player in the world",
        |watchers| {
            !controllers(watchers[2]).is_empty()
                && watchers[2].created(first_owner).is_some()
                && watchers[2].created(second_owner).is_some()
                && watchers[0].created(third_owner).is_some()
                && watchers[1].created(third_owner).is_some()
        },
    );
    assert!(shows(&controllers(&third)[0].1, true, false, None));
    assert!(
        mode_frames(&third)
            .iter()
            .all(|(_, chunk)| chunk.width != counting.bit_len())
    );

    let snapshot = server.snapshot().unwrap();
    assert_eq!(snapshot.len(), 1);
    let found = &snapshot[0];
    assert_eq!(
        (found.map.as_str(), found.waiting),
        ("Workshop Chamber", false)
    );
    let players: Vec<_> = found
        .players
        .iter()
        .map(|player| (player.name.as_str(), player.hero, player.team, player.state))
        .collect();
    assert_eq!(
        players,
        [
            ("first#1111", SOLDIER, 0, PlayerState::Playing),
            ("second#2222", SOLDIER, 1, PlayerState::Playing),
            ("third#3333", SOLDIER, 0, PlayerState::Playing),
        ]
    );
    server.stop().unwrap();
}

/// Players join a match that is on, in the next slots, and a match that is
/// over takes nobody (ow174/game/server.py `join_match`); the snapshot lists
/// every player, with the lobby's hero until his pick (match.py `snapshot`).
#[test]
fn players_join_a_running_match_in_the_next_slots() {
    use orpheus_content::heroes::SOLDIER;

    let mut server = GameService::bind(local(), Arc::new(fixture())).unwrap();
    let opened = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap();
    let joiner = || WorldPlayer {
        player: player().pop().unwrap(),
        card: PlayerCard::default(),
        hero: 0,
    };
    let joined = server
        .join_match(MATCH, vec![joiner()])
        .unwrap()
        .expect("a running match");
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].match_id, MATCH);
    assert_ne!(joined[0].conn, opened[0].conn);
    assert!(server.seat_exists(&joined[0]).unwrap());
    let snapshot = server.snapshot().unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot[0].waiting);
    let players: Vec<_> = snapshot[0]
        .players
        .iter()
        .map(|player| (player.hero, player.state))
        .collect();
    assert_eq!(players, [(SOLDIER, PlayerState::Waiting); 2]);
    assert!(server.join_match([9, 9], vec![joiner()]).unwrap().is_none());
    server.stop().unwrap();
}
