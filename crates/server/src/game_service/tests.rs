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

/// The id of the first reliable message the server sends.
fn first_reliable(client: &Client, schemas: &Schemas) -> u32 {
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        let packet = client.receive();
        if packet.kind != KIND_FRAME {
            continue;
        }
        if let Some(first) = messages(schemas, &packet).first() {
            return first.message.id;
        }
    }
    panic!("no reliable message in time");
}

/// tests/test_leaving.py `test_a_player_taken_out_before_his_game_connects_
/// leaves_at_once_and_only_gets_20304`.
#[test]
fn a_player_taken_out_before_his_game_connects_leaves_at_once_and_only_gets_20304() {
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let ticket = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap()
        .pop()
        .unwrap();
    assert!(server.release_seat(&ticket).unwrap());
    // He left at once: the seat is gone, and the match with its last player.
    assert!(!server.seat_exists(&ticket).unwrap());
    assert!(server.snapshot().unwrap().is_empty());

    let mut client = Client::new(server.local_addr(), &ticket);
    client.handshake();
    assert_eq!(first_reliable(&client, &schemas), 20304);
    thread::sleep(Duration::from_millis(100));
    // The game never joined a match.
    assert!(!server.seat_exists(&ticket).unwrap());
    assert!(server.snapshot().unwrap().is_empty());

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
    /// The channel-2 shot records' projectile ids (projectiles.py
    /// `shot_events`): a launch the other clients see.
    launches: Vec<u32>,
    base_health: Option<f32>,
    /// The first component of an update with one component.
    updated: Option<u8>,
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
                launches: Vec::new(),
                base_health: None,
                updated: None,
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
            if reader.bit().unwrap() {
                record.launches = shot_events(&mut reader);
            }
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
                        let count = components.bits(8).unwrap();
                        let index = components.bits(8).unwrap() as u8;
                        if count == 1 {
                            record.updated = Some(index);
                        }
                        if count == 1 && index == 51 {
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

/// The shot records after channel 2's events bit (projectiles.py
/// `shot_events`, `write_shot`): the projectile ids they start.
fn shot_events(reader: &mut BitReader<'_>) -> Vec<u32> {
    // 0 + `small` bits, else 1 + `middle` bits, escaped to 32 bits.
    fn escaped(reader: &mut BitReader<'_>, small: usize, middle: usize) -> u32 {
        if !reader.bit().unwrap() {
            return reader.bits(small).unwrap() as u32;
        }
        let value = reader.bits(middle).unwrap() as u32;
        if value == (1 << middle) - 1 {
            reader.bits(32).unwrap() as u32
        } else {
            value
        }
    }
    let mut ids = Vec::new();
    let mut first = true;
    while reader.bit().unwrap() {
        if first {
            reader.var_b().unwrap();
            first = false;
        } else if !reader.bit().unwrap() {
            reader.bits(4).unwrap();
        } else {
            let wide = reader.bit().unwrap();
            reader.bits(if wide { 32 } else { 8 }).unwrap();
        }
        escaped(reader, 4, 8); // the shot counter
        assert!(!reader.bit().unwrap(), "no spread");
        assert!(!reader.bit().unwrap(), "the volley form");
        reader.w_u16().unwrap(); // instance
        reader.w_u16().unwrap(); // state
        reader.bits(6).unwrap(); // activation
        if !reader.bit().unwrap() {
            reader.w_u16().unwrap(); // volleys
        }
        assert_eq!(reader.w_u16().unwrap(), 0);
        reader.advance(3 * 32 + 2 * 16).unwrap(); // origin, yaw and pitch
        if !reader.bit().unwrap() {
            continue;
        }
        let count = reader.w_u16().unwrap();
        let kind = reader.bits(2).unwrap() as u32;
        let mut id = 0x8000_0000 | kind << 29 | escaped(reader, 8, 16);
        ids.push(id);
        for _ in 1..count {
            if reader.bit().unwrap() {
                id += 1;
            } else {
                let negative = reader.bit().unwrap();
                let step = escaped(reader, 4, 8);
                id = if negative { id - step } else { id + step };
            }
            ids.push(id);
        }
    }
    ids
}

fn input_batch(first: u32, count: u8, yaw: i16) -> Vec<u8> {
    command_batch(first, count, yaw, 0, true)
}

fn command_batch(first: u32, count: u8, yaw: i16, buttons: u32, forward: bool) -> Vec<u8> {
    aimed_batch(first, count, yaw, 0, buttons, forward)
}

/// Soldier: 76's level shot from his 1.55 m eye meets a Training Bot's
/// chest: its idle pose centres the head sphere 1.98 m up. This pitch
/// (1/65536 turns, negative up) aims from the eye at the head 4 m away,
/// atan(0.43 / 4).
const AT_BOT_HEAD_4M: i16 = -1114;

fn aimed_batch(
    first: u32,
    count: u8,
    yaw: i16,
    pitch: i16,
    buttons: u32,
    forward: bool,
) -> Vec<u8> {
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
    input.signed(i64::from(pitch), 16).unwrap();
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
            map: GameMap::practice_range(),
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
    let commands = aimed_batch(frame, 1, -16384, AT_BOT_HEAD_4M, fire, false);
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
    let original = ProjectileLeases::new(0, frame, [(definition, 1)])
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
    // The board follows the hero (match.py `_send_board`: his hero's stats).
    assert_eq!(
        describe(&records),
        [
            (SECOND, Some(1), None),
            (OWNER, Some(2), None),
            (FIRST, Some(3), None),
            (OWNER, None, Some(4)),
            (OWNER, Some(2), None)
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
    /// The server tick of each datagram.
    ticks: Vec<u32>,
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
        self.ticks.push(self.tick);
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
                ticks: Vec::new(),
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

/// Two players on different teams in one Practice Range (combat.py): the
/// first shoots the second to death. The shooter gets his hits (25001) and
/// the elimination (25005); the victim gets "YOU WERE KILLED BY" (25004)
/// naming the shooter's body; both clients get frames of the dead body's
/// statescript, which carry its "died" kill feed line (003E); the shooter's
/// Tab board changes; and the victim gets a new body after the Practice
/// Range's 3 s, which the shooter's client gets too.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_player_kills_another_who_respawns_after_the_practice_range_delay() {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO, stats::GMP_STATS};
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::FIRE;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let spawns = [0, 1].map(|team| scene.map().spawn_point(team, 0).unwrap().0);
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let tags = ["shooter#1111", "target#2222"];
    let members = tags
        .iter()
        .enumerate()
        .map(|(index, tag)| WorldPlayer {
            player: MatchPlayer {
                name: (*tag).into(),
                team: index as u8,
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
    let tickets = server.open_prepared(MATCH, scene, members).unwrap();
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
                ticks: Vec::new(),
            }
        })
        .collect();
    let [shooter, target] = &mut watchers[..] else {
        unreachable!()
    };
    let (shooter_owner, target_owner) = (shooter.owner, target.owner);
    let (shooter_body, target_body) = (shooter_owner | 1, target_owner | 1);
    pump_until(
        &mut [shooter, target],
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
    for watcher in [&mut *shooter, &mut *target] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    pump_until(
        &mut [shooter, target],
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
    for watcher in [&mut *shooter, &mut *target] {
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
    // Each client has the other's body and his possession of it.
    let possessed = |watcher: &Watcher, other: u32| {
        watcher.created(other | 1).is_some()
            && watcher
                .find(0, |record| {
                    record.entity == other && record.operation == Some(2)
                })
                .is_some()
    };
    pump_until(&mut [shooter, target], &schemas, "the bodies", |watchers| {
        possessed(watchers[0], target_owner) && possessed(watchers[1], shooter_owner)
    });
    for _ in 0..10 {
        shooter.pump(&schemas);
        target.pump(&schemas);
    }

    // The shooter holds fire at the target's head height, level.
    let [dx, _, dz] = std::array::from_fn::<_, 3, _>(|axis| spawns[1][axis] - spawns[0][axis]);
    let yaw = (dx.atan2(dz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16;
    let messages_of = |watcher: &Watcher, id: u32| -> Vec<(usize, Fields)> {
        watcher
            .datagrams
            .iter()
            .enumerate()
            .flat_map(|(index, (messages, _))| {
                messages
                    .iter()
                    .filter(move |message| message.id == id)
                    .map(move |message| (index, message.fields.clone()))
            })
            .collect()
    };
    let fired = shooter.datagrams.len();
    let mut frame = shooter.tick + 1;
    let deadline = Instant::now() + Duration::from_secs(20);
    while messages_of(target, 25004).is_empty() {
        assert!(Instant::now() < deadline, "waiting for the kill");
        if frame <= shooter.tick + 8 {
            shooter
                .client
                .send(ACK, command_batch(frame, 16, yaw, FIRE, false));
            frame += 16;
        }
        shooter.pump(&schemas);
        target.pump(&schemas);
    }
    let (died, death) = messages_of(target, 25004).remove(0);
    let death_tick = target.ticks[died];
    assert_eq!(death[&0x78], Value::Unsigned(2), "killed by");
    assert_eq!(
        death[&0x7c],
        Value::Struct(Fields::from([(
            0,
            Value::Unsigned(u64::from(shooter_body))
        )]))
    );
    // Stop firing, and wait for the shooter's notices and board.
    let elimination = |watcher: &[&mut Watcher]| !messages_of(watcher[0], 25005).is_empty();
    pump_until(
        &mut [shooter, target],
        &schemas,
        "the elimination",
        elimination,
    );
    let hits = messages_of(shooter, 25001);
    assert!(!hits.is_empty());
    assert!(hits.iter().all(|(index, _)| *index >= fired));
    let (eliminated, notice) = messages_of(shooter, 25005).remove(0);
    assert_eq!(
        notice[&0x84],
        Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(target_body)))]))
    );
    assert!(hits.iter().all(|(index, _)| *index <= eliminated));
    pump_until(&mut [shooter, target], &schemas, "the board", |watchers| {
        watchers[0]
            .find(fired, |record| {
                record.entity == shooter_owner && record.updated == Some(GMP_STATS)
            })
            .is_some()
            && watchers[1]
                .find(died, |record| {
                    record.entity == target_owner && record.updated == Some(GMP_STATS)
                })
                .is_some()
    });
    // The dead body's statescript goes on to both clients while the dead
    // player's client keeps sending commands: its owner frames and the
    // shooter's observer frames carry the kill feed pulse. Soldier: 76's
    // death path reaches an action the VM doesn't implement (graph 01CA
    // node 118, `STU_4C3509FB`), which debug builds refuse and release
    // builds follow as Python does; so this part runs in release builds.
    let tolerant =
        orpheus_script::vm::Unknowns::for_build() == orpheus_script::vm::Unknowns::Tolerate;
    let owner_frames = |watcher: &Watcher| {
        watcher
            .find(died, |record| {
                record.entity == target_body && record.script.is_some()
            })
            .is_some()
    };
    let observer_frames = |watcher: &Watcher| {
        watcher
            .find(eliminated, |record| {
                record.entity == target_body && record.script.is_some()
            })
            .is_some()
    };
    let mut idle = target.tick + 1;
    let deadline = Instant::now() + Duration::from_secs(10);
    while tolerant && !(owner_frames(target) && observer_frames(shooter)) {
        assert!(
            Instant::now() < deadline,
            "waiting for the dead body's frames: owner {}, observer {}",
            owner_frames(target),
            observer_frames(shooter)
        );
        if idle <= target.tick + 8 {
            target
                .client
                .send(ACK, command_batch(idle, 16, 0, 0, false));
            idle += 16;
        }
        shooter.pump(&schemas);
        target.pump(&schemas);
    }
    // His new body comes after the Practice Range's 3 s, to both clients.
    let respawned = target_owner | 2;
    pump_until(
        &mut [shooter, target],
        &schemas,
        "the respawn",
        |watchers| {
            watchers[1].created(respawned).is_some() && watchers[0].created(respawned).is_some()
        },
    );
    let created = target.created(respawned).unwrap();
    let respawn_tick = target.ticks[created];
    assert!(created > died);
    let waited = respawn_tick - death_tick;
    assert!(
        (3000 / 16 - 4..3000 / 16 + 60).contains(&waited),
        "respawned after {waited} ticks"
    );
    assert!(
        target
            .find(died, |record| record.entity == target_body
                && record.operation == Some(3))
            .is_some()
    );
    server.stop().unwrap();
}

/// Players (BattleTag, team, hero name) in one prepared Practice Range, once
/// each client has every other body, its possession and, for a body that
/// runs a statescript (Soldier: 76's), its frames. Also where each spawned.
fn practice_players<const N: usize>(
    players: [(&str, u8, &str); N],
) -> (GameService, Arc<Schemas>, [Watcher; N], [[f64; 3]; N]) {
    let assets =
        orpheus_sim::scene_assets::SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    practice_world(players, &assets, |_| {})
}

/// [`practice_players`] with these assets, and `arrange` changing the
/// prepared scene first.
fn practice_world<const N: usize>(
    players: [(&str, u8, &str); N],
    assets: &orpheus_sim::scene_assets::SceneAssets,
    arrange: impl FnOnce(&mut orpheus_sim::scene_assets::PreparedScene),
) -> (GameService, Arc<Schemas>, [Watcher; N], [[f64; 3]; N]) {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};

    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    arrange(&mut scene);
    let heroes = players.map(|(_, _, name)| assets.heroes().find_hero(name).unwrap().guid);
    // match.py Player.spawn: the n-th player of a team.
    let spawns = std::array::from_fn(|slot| {
        let team = players[slot].1;
        let number = players[..slot]
            .iter()
            .filter(|(_, other, _)| *other == team)
            .count();
        scene.map().spawn_point(team, number).unwrap().0
    });
    let schemas = Arc::new(Schemas::game());
    let server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let members = players
        .iter()
        .enumerate()
        .map(|(index, &(tag, team, _))| WorldPlayer {
            player: MatchPlayer {
                name: tag.into(),
                team,
                tournament: false,
                ranked: false,
            },
            card: PlayerCard {
                account: [index as u64 + 7, 0x0100_0000_0000_0000],
                battle_tag: tag.into(),
                record: Fields::from([(0x40, Value::String(tag.into()))]),
                ..PlayerCard::default()
            },
            hero: 0,
        })
        .collect();
    let tickets = server.open_prepared(MATCH, scene, members).unwrap();
    let mut watchers: [Watcher; N] = tickets
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
                ticks: Vec::new(),
            }
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap_or_else(|_| unreachable!("one client a ticket"));
    let mut all: Vec<&mut Watcher> = watchers.iter_mut().collect();
    pump_until(&mut all, &schemas, "the instances", |watchers| {
        watchers.iter().all(|watcher| {
            watcher.datagrams.iter().any(|(messages, _)| {
                messages.iter().any(|message| {
                    message.id == 20300 && message.fields[&0x80] != Value::Struct(Fields::new())
                })
            })
        })
    });
    for watcher in all.iter_mut() {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    pump_until(&mut all, &schemas, "the controllers", |watchers| {
        watchers.iter().all(|watcher| {
            watcher
                .find(0, |record| {
                    record.entity == watcher.owner && record.script.is_some()
                })
                .is_some()
        })
    });
    for (watcher, hero) in all.iter_mut().zip(heroes) {
        let owner = watcher.owner;
        send_player_message(
            &mut watcher.client,
            &schemas,
            owner,
            1,
            PICK_HERO,
            hero.to_le_bytes().to_vec(),
        );
    }
    let owners: Vec<(u32, bool)> = all
        .iter()
        .zip(heroes)
        .map(|(watcher, hero)| (watcher.owner, hero == SOLDIER))
        .collect();
    let sees = |watcher: &Watcher, (other, scripted): (u32, bool)| {
        let body = other | 1;
        watcher.created(body).is_some()
            && watcher
                .find(0, |record| {
                    record.entity == other && record.operation == Some(2)
                })
                .is_some()
            && (!scripted
                || watcher
                    .find(0, |record| record.entity == body && record.script.is_some())
                    .is_some())
    };
    pump_until(&mut all, &schemas, "the other bodies", |watchers| {
        watchers.iter().all(|watcher| {
            owners
                .iter()
                .filter(|(other, _)| *other != watcher.owner)
                .all(|&other| sees(watcher, other))
        })
    });
    // The full frames' acknowledgements reach the server.
    for _ in 0..10 {
        for watcher in all.iter_mut() {
            watcher.pump(&schemas);
        }
    }
    (server, schemas, watchers, spawns)
}

/// Pump the clients until `done` while `player` keeps sending idle commands:
/// his body's script runs on his commands.
fn pump_commanding(
    watchers: &mut [&mut Watcher],
    schemas: &Schemas,
    what: &str,
    player: usize,
    next: &mut u32,
    done: impl Fn(&[&mut Watcher]) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !done(watchers) {
        assert!(Instant::now() < deadline, "waiting for {what}");
        let commander = &mut watchers[player];
        if *next <= commander.tick + 8 {
            commander
                .client
                .send(ACK, command_batch(*next, 16, 0, 0, false));
            *next += 16;
        }
        for watcher in watchers.iter_mut() {
            watcher.pump(schemas);
        }
    }
}

/// projectiles.py `_show_others`, `_ObservedProjectile`: another client
/// sees a player's Helix Rocket as a server entity of its definition under
/// the map (the room's first SEEN id), which a shot record in the shooter
/// body's channel 2 starts once its create arrived and the client has the
/// volley; it goes with the explosion's release when the rocket ends. The
/// shooter's own client gets none of it.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn another_client_sees_a_players_helix_rocket() {
    use orpheus_sim::projectiles::{SEEN_FIRST, SEEN_SPAN};
    use orpheus_wire::commands::SECONDARY;
    const HELIX: u32 = 0x072c;

    // Teammates, side by side: the rocket harms nobody.
    let (mut server, schemas, [mut shooter, mut viewer], spawns) = practice_players([
        ("shooter#1111", 1, "Soldier: 76"),
        ("viewer#2222", 1, "Soldier: 76"),
    ]);
    let shooter_body = shooter.owner | 1;
    // Away from the other player, level: the rocket ends on the range.
    let [dx, _, dz] = std::array::from_fn::<_, 3, _>(|axis| spawns[1][axis] - spawns[0][axis]);
    let away = ((-dx).atan2(-dz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16;
    let fired = viewer.datagrams.len();
    let mut next = shooter.tick + 1;
    shooter
        .client
        .send(ACK, command_batch(next, 1, away, SECONDARY, false));
    next += 1;
    let seen = |entity: u32| (SEEN_FIRST..SEEN_FIRST + SEEN_SPAN).contains(&entity);
    pump_commanding(
        &mut [&mut shooter, &mut viewer],
        &schemas,
        "the observed rocket's destroy",
        0,
        &mut next,
        |watchers| {
            watchers[1]
                .find(fired, |record| {
                    seen(record.entity) && record.operation == Some(3)
                })
                .is_some()
        },
    );
    let created = viewer
        .find(fired, |record| seen(record.entity) && record.is_create())
        .expect("the rocket's create");
    let (_, records) = &viewer.datagrams[created];
    let create = records
        .iter()
        .find(|record| seen(record.entity) && record.is_create())
        .unwrap();
    assert_eq!(create.entity, SEEN_FIRST);
    assert_eq!(create.definition, Some(HELIX));
    let launched = viewer
        .find(fired, |record| {
            record.entity == shooter_body && record.launches == [SEEN_FIRST]
        })
        .expect("the shot record");
    assert!(
        launched > created,
        "the launch waits for the create's arrival"
    );
    let destroyed = viewer
        .find(fired, |record| {
            record.entity == SEEN_FIRST && record.operation == Some(3)
        })
        .unwrap();
    assert!(destroyed >= launched);
    let (_, records) = &viewer.datagrams[destroyed];
    let destroy = records
        .iter()
        .find(|record| record.entity == SEEN_FIRST && record.operation == Some(3))
        .unwrap();
    assert!(
        destroy.projectile_release.is_some(),
        "a launched rocket that exploded is released"
    );
    for (_, records) in &viewer.datagrams {
        assert!(
            records
                .iter()
                .filter(|record| !record.launches.is_empty())
                .all(|record| record.entity == shooter_body)
        );
    }
    assert!(
        shooter
            .find(0, |record| seen(record.entity)
                || !record.launches.is_empty())
            .is_none()
    );
    server.stop().unwrap();
}

/// Tracer's Pulse Bomb (graph `0034` state 0; no Python reference): thrown
/// level at a training bot 3.3 m ahead, it falls on its arc onto the bot
/// and sticks (5 damage), and a second later explodes and kills it. The
/// other client sees the bomb as the room's first SEEN entity of definition
/// 0045, started by the shot record in Tracer's channel 2 and destroyed
/// with the explosion's release once it went off; Tracer's own client gets
/// the confirmation of the bomb she shot under her lease and its release
/// after the explosion, and none of the other client's copy. Her graphs
/// still reach content the debug VM refuses, so her body tolerates it as
/// release builds do, and her ultimate fills in a second.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn a_pulse_bomb_sticks_to_a_bot_kills_it_after_its_fuse_and_another_client_sees_it() {
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    const PULSE_BOMB: u32 = 0x0045;
    /// Q, the ultimate's button.
    const ULTIMATE: u32 = 0x20;
    const AHEAD: f64 = 3.3;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    // Her 1,260-point ultimate fills in a second.
    assets.set_ult_rates(orpheus_script::ultimate::ChargeRates {
        passive: 1260.0,
        combat: 1.0,
    });
    let mut aim = None;
    let (mut server, schemas, [mut tracer, mut viewer], _) = practice_world(
        [
            ("tracer#1111", 0, "Tracer"),
            ("viewer#2222", 0, "Soldier: 76"),
        ],
        &assets,
        |scene| {
            // A hostile training bot that neither fires nor walks, ahead of
            // her spawn along its facing.
            let ([x, y, z], yaw) = scene.map().spawn_point(0, 0).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
            aim = Some((bot.entity, (yaw * 65536.0 / 360.0).round() as i32 as i16));
        },
    );
    let (bot, yaw) = aim.unwrap();
    let tracer_body = tracer.owner | 1;
    let seen = |entity: u32| (SEEN_FIRST..SEEN_FIRST + SEEN_SPAN).contains(&entity);
    let messages_of = |watcher: &Watcher, id: u32| -> Vec<(usize, Fields)> {
        watcher
            .datagrams
            .iter()
            .enumerate()
            .flat_map(|(index, (messages, _))| {
                messages
                    .iter()
                    .filter(move |message| message.id == id)
                    .map(move |message| (index, message.fields.clone()))
            })
            .collect()
    };
    // Tracer faces the bot; her commands keep her script running.
    let mut next = tracer.tick + 1;
    let mut command = |tracer: &mut Watcher, buttons: u32| {
        let count = if buttons == 0 { 16 } else { 1 };
        if buttons != 0 || next <= tracer.tick + 8 {
            tracer
                .client
                .send(ACK, command_batch(next, count, yaw, buttons, false));
            next += u32::from(count);
        }
    };
    let charged = Instant::now() + Duration::from_secs(2);
    while Instant::now() < charged {
        command(&mut tracer, 0);
        tracer.pump(&schemas);
        viewer.pump(&schemas);
    }
    let thrown = (tracer.datagrams.len(), viewer.datagrams.len());
    command(&mut tracer, ULTIMATE);
    let released = |watcher: &Watcher, entity: &dyn Fn(u32) -> bool| {
        watcher
            .find(0, |record| {
                entity(record.entity)
                    && record.operation == Some(3)
                    && record.projectile_release.is_some()
            })
            .is_some()
    };
    let bomb = |entity: u32| entity & 0xf000_0000 == 0xa000_0000 && !seen(entity);
    let deadline = Instant::now() + Duration::from_secs(10);
    while messages_of(&tracer, 25005).is_empty()
        || !released(&viewer, &seen)
        || !released(&tracer, &bomb)
    {
        assert!(
            Instant::now() < deadline,
            "waiting for the Pulse Bomb: hits {:?}",
            messages_of(&tracer, 25001)
        );
        command(&mut tracer, 0);
        tracer.pump(&schemas);
        viewer.pump(&schemas);
    }

    // Her hits on the bot: the stick, then the explosion a second later.
    let hits: Vec<(u64, Value, Value)> = messages_of(&tracer, 25001)
        .into_iter()
        .filter(|(index, _)| *index >= thrown.0)
        .map(|(_, fields)| {
            let Value::Unsigned(frame) = fields[&0x84] else {
                panic!("impact frame")
            };
            let Value::Struct(impact) = &fields[&0x78] else {
                panic!("hit body")
            };
            let Value::Struct(target) = &impact[&0] else {
                panic!("hit target")
            };
            (frame, target[&0].clone(), impact[&4].clone())
        })
        .collect();
    let target = Value::Unsigned(u64::from(bot));
    assert_eq!(
        hits.iter()
            .map(|(_, target, amount)| (target.clone(), amount.clone()))
            .collect::<Vec<_>>(),
        [
            (target.clone(), Value::Unsigned(5)),
            (target.clone(), Value::Unsigned(195))
        ],
        "{hits:?}"
    );
    // STICK_FUSE: 1 s, 62.5 ticks of 16 ms.
    let fuse = hits[1].0 - hits[0].0;
    assert!((62..=63).contains(&fuse), "fuse of {fuse} ticks");
    let (_, notice) = messages_of(&tracer, 25005).remove(0);
    assert_eq!(notice[&0x84], Value::Struct(Fields::from([(0, target)])));
    let exploded = hits[1].0 as u32;

    // The other client's copy.
    let created = viewer
        .find(thrown.1, |record| seen(record.entity) && record.is_create())
        .expect("the bomb's create");
    let create = viewer.datagrams[created]
        .1
        .iter()
        .find(|record| seen(record.entity) && record.is_create())
        .unwrap();
    assert_eq!(
        (create.entity, create.definition),
        (SEEN_FIRST, Some(PULSE_BOMB))
    );
    let launched = viewer
        .find(created, |record| {
            record.entity == tracer_body && record.launches == [SEEN_FIRST]
        })
        .expect("the shot record");
    let destroyed = viewer
        .find(launched, |record| {
            record.entity == SEEN_FIRST && record.operation == Some(3)
        })
        .unwrap();
    assert!(viewer.ticks[destroyed] >= exploded);
    assert!(
        viewer.datagrams[destroyed]
            .1
            .iter()
            .any(|record| record.entity == SEEN_FIRST && record.projectile_release.is_some()),
        "the bomb that went off is released"
    );

    // Her own bomb: confirmed, then released after the explosion.
    let confirmed = tracer
        .find(thrown.0, |record| {
            bomb(record.entity) && record.definition == Some(PULSE_BOMB)
        })
        .expect("the bomb's confirmation");
    let release = tracer
        .find(confirmed, |record| {
            bomb(record.entity) && record.projectile_release.is_some()
        })
        .unwrap();
    assert!(tracer.ticks[release] >= exploded);
    assert!(
        tracer
            .find(0, |record| seen(record.entity)
                || !record.launches.is_empty())
            .is_none()
    );
    server.stop().unwrap();
}

/// combat.py `_shoot` for Genji's fan (graph `0304` state 11, his secondary
/// fire: three shuriken a shot, sync NONE): the server flies one shuriken
/// along the aim for its damage alone, as Python does whatever the count,
/// so a training bot 3.3 m ahead takes one hit of 29 (variable `01DC`,
/// which node 79 sets) a shot. Nobody sees a server projectile: Genji's
/// client gets no confirmation of a shuriken (definition `07CA`) and the
/// other client no copy or shot record.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn genjis_fan_hits_a_bot_once_a_shot_for_its_damage_alone_unseen() {
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    use orpheus_wire::commands::SECONDARY;
    const SHURIKEN: u32 = 0x07ca;
    const AHEAD: f64 = 3.3;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    let mut aim = None;
    let (mut server, schemas, [mut genji, mut viewer], _) = practice_world(
        [
            ("genji#1111", 0, "Genji"),
            ("viewer#2222", 0, "Soldier: 76"),
        ],
        &assets,
        |scene| {
            // A hostile training bot that neither fires nor walks, ahead of
            // his spawn along its facing.
            let ([x, y, z], yaw) = scene.map().spawn_point(0, 0).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
            aim = Some((bot.entity, (yaw * 65536.0 / 360.0).round() as i32 as i16));
        },
    );
    let (bot, yaw) = aim.unwrap();
    let seen = |entity: u32| (SEEN_FIRST..SEEN_FIRST + SEEN_SPAN).contains(&entity);
    let hits_of = |watcher: &Watcher, from: usize| -> Vec<(u64, Value, Value)> {
        watcher.datagrams[from..]
            .iter()
            .flat_map(|(messages, _)| messages.iter().filter(|message| message.id == 25001))
            .map(|message| {
                let Value::Unsigned(frame) = message.fields[&0x84] else {
                    panic!("impact frame")
                };
                let Value::Struct(impact) = &message.fields[&0x78] else {
                    panic!("hit body")
                };
                let Value::Struct(target) = &impact[&0] else {
                    panic!("hit target")
                };
                (frame, target[&0].clone(), impact[&4].clone())
            })
            .collect()
    };
    // Genji faces the bot; his commands keep his script running.
    let mut next = genji.tick + 1;
    let mut command = |genji: &mut Watcher, buttons: u32| {
        let count = if buttons == 0 { 16 } else { 1 };
        if buttons != 0 || next <= genji.tick + 8 {
            genji
                .client
                .send(ACK, command_batch(next, count, yaw, buttons, false));
            next += u32::from(count);
        }
    };
    let ready = Instant::now() + Duration::from_secs(1);
    while Instant::now() < ready {
        command(&mut genji, 0);
        genji.pump(&schemas);
        viewer.pump(&schemas);
    }
    let fired = (genji.datagrams.len(), viewer.datagrams.len());
    command(&mut genji, SECONDARY);
    let deadline = Instant::now() + Duration::from_secs(5);
    while hits_of(&genji, fired.0).is_empty() {
        assert!(Instant::now() < deadline, "waiting for the fan's hit");
        command(&mut genji, 0);
        genji.pump(&schemas);
        viewer.pump(&schemas);
    }
    // The other two shuriken of the shot never fly.
    let settled = Instant::now() + Duration::from_secs(1);
    while Instant::now() < settled {
        command(&mut genji, 0);
        genji.pump(&schemas);
        viewer.pump(&schemas);
    }

    let hits = hits_of(&genji, fired.0);
    assert_eq!(
        hits.iter()
            .map(|(_, target, amount)| (target.clone(), amount.clone()))
            .collect::<Vec<_>>(),
        [(Value::Unsigned(u64::from(bot)), Value::Unsigned(29))],
        "{hits:?}"
    );
    assert!(
        genji
            .find(fired.0, |record| record.definition == Some(SHURIKEN)
                || seen(record.entity)
                || !record.launches.is_empty())
            .is_none(),
        "Genji's client gets no shuriken"
    );
    assert!(
        viewer
            .find(fired.1, |record| seen(record.entity)
                || record.definition == Some(SHURIKEN)
                || !record.launches.is_empty())
            .is_none(),
        "the other client sees no shuriken"
    );
    server.stop().unwrap();
}

/// Doomfist's Hand Cannon (graph `0D0A` state 2: 11 projectiles a shot in
/// rings of 1, 4 and 6, 6 damage each, shots only): each pellet flies for
/// its damage (leafall903/ORPHEUS 7cb4018 `test_hand_cannon_sweeps_all_eleven_pellets_from_one_shot`),
/// so a training bot 2 m ahead takes eleven hits of 6, 66 in all, from one
/// shot; nobody sees a server projectile.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn doomfists_hand_cannon_hits_with_all_eleven_pellets() {
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    use orpheus_wire::commands::FIRE;
    const AHEAD: f64 = 2.0;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    let mut aim = None;
    let (mut server, schemas, [mut doomfist, mut viewer], _) = practice_world(
        [
            ("doomfist#1111", 0, "Doomfist"),
            ("viewer#2222", 0, "Soldier: 76"),
        ],
        &assets,
        |scene| {
            let ([x, y, z], yaw) = scene.map().spawn_point(0, 0).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
            aim = Some((bot.entity, (yaw * 65536.0 / 360.0).round() as i32 as i16));
        },
    );
    let (bot, yaw) = aim.unwrap();
    let seen = |entity: u32| (SEEN_FIRST..SEEN_FIRST + SEEN_SPAN).contains(&entity);
    let hits_of = |watcher: &Watcher, from: usize| -> Vec<(Value, Value)> {
        watcher.datagrams[from..]
            .iter()
            .flat_map(|(messages, _)| messages.iter().filter(|message| message.id == 25001))
            .map(|message| {
                let Value::Struct(impact) = &message.fields[&0x78] else {
                    panic!("hit body")
                };
                let Value::Struct(target) = &impact[&0] else {
                    panic!("hit target")
                };
                (target[&0].clone(), impact[&4].clone())
            })
            .collect()
    };
    let mut next = doomfist.tick + 1;
    let mut command = |doomfist: &mut Watcher, buttons: u32| {
        let count = if buttons == 0 { 16 } else { 1 };
        if buttons != 0 || next <= doomfist.tick + 8 {
            doomfist
                .client
                .send(ACK, command_batch(next, count, yaw, buttons, false));
            next += u32::from(count);
        }
    };
    let ready = Instant::now() + Duration::from_secs(1);
    while Instant::now() < ready {
        command(&mut doomfist, 0);
        doomfist.pump(&schemas);
        viewer.pump(&schemas);
    }
    let fired = (doomfist.datagrams.len(), viewer.datagrams.len());
    command(&mut doomfist, FIRE);
    let deadline = Instant::now() + Duration::from_secs(5);
    while hits_of(&doomfist, fired.0).len() < 11 {
        assert!(
            Instant::now() < deadline,
            "waiting for the pellets: {:?}",
            hits_of(&doomfist, fired.0)
        );
        command(&mut doomfist, 0);
        doomfist.pump(&schemas);
        viewer.pump(&schemas);
    }
    let settled = Instant::now() + Duration::from_millis(500);
    while Instant::now() < settled {
        command(&mut doomfist, 0);
        doomfist.pump(&schemas);
        viewer.pump(&schemas);
    }

    assert_eq!(
        hits_of(&doomfist, fired.0),
        vec![(Value::Unsigned(u64::from(bot)), Value::Unsigned(6)); 11]
    );
    assert!(
        viewer
            .find(fired.1, |record| seen(record.entity)
                || !record.launches.is_empty())
            .is_none(),
        "the other client sees no pellet"
    );
    server.stop().unwrap();
}

/// Echo's Tri-Shot (graph `19E5` state 3, her primary fire: three bolts a
/// shot, predicted): her body's lease gives definition `2CBF` three ids a
/// frame, so the three bolts she shot under the frame's consecutive ids
/// are each confirmed, and each released once the server's tracking bolt
/// (along the aim) has reached the training bot ahead of her; the other
/// client sees that one bolt as the room's first SEEN entity, started by
/// one shot record. Each of the three bolts of the volley's ring
/// (`STU_613BDDB0`, leafall903/ORPHEUS 7cb4018) flies for its damage and
/// hits the bot: three hits, where the one bolt along the aim hit once
/// before the ring layout.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn echos_tri_shot_confirms_and_releases_each_bolt_under_the_lease() {
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    use orpheus_wire::commands::FIRE;
    const BOLT: u32 = 0x2cbf;
    const AHEAD: f64 = 3.3;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    let mut aim = None;
    let (mut server, schemas, [mut echo, mut viewer], _) = practice_world(
        [("echo#1111", 0, "Echo"), ("viewer#2222", 0, "Soldier: 76")],
        &assets,
        |scene| {
            let ([x, y, z], yaw) = scene.map().spawn_point(0, 0).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
            aim = Some((bot.entity, (yaw * 65536.0 / 360.0).round() as i32 as i16));
        },
    );
    let (bot, yaw) = aim.unwrap();
    let echo_body = echo.owner | 1;
    let seen = |entity: u32| (SEEN_FIRST..SEEN_FIRST + SEEN_SPAN).contains(&entity);
    let bolt = |entity: u32| entity & 0xf000_0000 == 0xa000_0000 && !seen(entity);
    let mut next = echo.tick + 1;
    let mut command = |echo: &mut Watcher, buttons: u32| {
        let count = if buttons == 0 { 16 } else { 1 };
        if buttons != 0 || next <= echo.tick + 8 {
            echo.client
                .send(ACK, command_batch(next, count, yaw, buttons, false));
            next += u32::from(count);
        }
    };
    let ready = Instant::now() + Duration::from_secs(1);
    while Instant::now() < ready {
        command(&mut echo, 0);
        echo.pump(&schemas);
        viewer.pump(&schemas);
    }
    let fired = (echo.datagrams.len(), viewer.datagrams.len());
    command(&mut echo, FIRE);
    let records = |watcher: &Watcher, from: usize, pick: &dyn Fn(&WorldRecord) -> bool| {
        watcher.datagrams[from..]
            .iter()
            .flat_map(|(_, records)| records.iter().filter(|record| pick(record)))
            .map(|record| record.entity)
            .collect::<Vec<_>>()
    };
    let confirmed = |echo: &Watcher| {
        records(echo, fired.0, &|record| {
            bolt(record.entity) && record.definition == Some(BOLT)
        })
    };
    let released = |echo: &Watcher| {
        records(echo, fired.0, &|record| {
            bolt(record.entity) && record.projectile_release.is_some()
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let hits_of = |echo: &Watcher| -> Vec<Value> {
        echo.datagrams[fired.0..]
            .iter()
            .flat_map(|(messages, _)| messages.iter().filter(|message| message.id == 25001))
            .map(|message| {
                let Value::Struct(impact) = &message.fields[&0x78] else {
                    panic!("hit body")
                };
                let Value::Struct(target) = &impact[&0] else {
                    panic!("hit target")
                };
                target[&0].clone()
            })
            .collect()
    };
    while released(&echo).len() < 3
        || hits_of(&echo).len() < 3
        || viewer
            .find(fired.1, |record| {
                seen(record.entity) && record.operation == Some(3)
            })
            .is_none()
    {
        assert!(
            Instant::now() < deadline,
            "waiting for the Tri-Shot: confirmed {:x?}, released {:x?}",
            confirmed(&echo),
            released(&echo)
        );
        command(&mut echo, 0);
        echo.pump(&schemas);
        viewer.pump(&schemas);
    }

    // Three consecutive ids of one lease frame, each confirmed once and
    // released once.
    let mut ids = confirmed(&echo);
    ids.sort_unstable();
    assert_eq!(ids.len(), 3, "{ids:x?}");
    assert_eq!([ids[1] - ids[0], ids[2] - ids[1]], [1, 1], "{ids:x?}");
    let mut releases = released(&echo);
    releases.sort_unstable();
    assert_eq!(releases, ids);
    // Each bolt of the ring hit the bot.
    assert_eq!(hits_of(&echo), vec![Value::Unsigned(u64::from(bot)); 3]);
    // The other client's one copy.
    assert_eq!(
        records(&viewer, fired.1, &|record| seen(record.entity)
            && record.is_create()),
        [SEEN_FIRST]
    );
    assert_eq!(
        records(&viewer, fired.1, &|record| !record.launches.is_empty()),
        [echo_body]
    );
    server.stop().unwrap();
}

/// projectiles.py `_field`, `_deliver_field`: Soldier: 76's Biotic Field
/// (ABILITY_2: 0257) heals a wounded teammate standing in it, the healer's
/// Tab board counts the healing, and every client gets the field entity
/// (03DC.003), its frame once the create arrived, and its destroy when the
/// field ends, FIELD_TICKS after it opened. The teammate is Tracer, whose
/// body runs no statescript: a wounded Soldier: 76 body reaches an entity
/// filter (0033 node 29, `STU_AC829876`) that debug builds refuse.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_biotic_field_heals_a_wounded_teammate_and_every_client_sees_it() {
    use orpheus_content::stats::GMP_STATS;
    use orpheus_sim::projectiles::{FIELD_FIRST, FIELD_SPAN};
    use orpheus_wire::commands::FIRE;
    /// tests/test_game_projectiles.py `ABILITY_2`: 0257 st0's button.
    const ABILITY_2: u32 = 0x10;
    const FIELD: u32 = 0x03dc;
    /// projectiles.py `FIELD_TICKS`: 0258's 5 s.
    const FIELD_TICKS: u32 = 313;

    // The enemy and the patient share the spawn point; the healer stands
    // 1.5 m beside it, out of the enemy's line of fire.
    let (mut server, schemas, [mut enemy, mut patient, mut healer], spawns) = practice_players([
        ("enemy#1111", 0, "Soldier: 76"),
        ("patient#2222", 1, "Tracer"),
        ("healer#3333", 1, "Soldier: 76"),
    ]);
    let patient_body = patient.owner | 1;
    let health = |watcher: &Watcher| {
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| record.entity == patient_body)
            .filter_map(|record| record.base_health)
            .next_back()
    };
    // The enemy wounds the patient with short bursts away from the healer.
    let [dx, _, dz] = std::array::from_fn::<_, 3, _>(|axis| spawns[2][axis] - spawns[0][axis]);
    let away = ((-dx).atan2(-dz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16;
    let full = 150.0;
    let mut frame = enemy.tick + 1;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !health(&patient).is_some_and(|health| health < full) {
        assert!(Instant::now() < deadline, "waiting for the wound");
        if frame <= enemy.tick + 8 {
            enemy
                .client
                .send(ACK, command_batch(frame, 8, away, FIRE, false));
            frame += 8;
        }
        for watcher in [&mut enemy, &mut patient, &mut healer] {
            watcher.pump(&schemas);
        }
    }
    for _ in 0..20 {
        for watcher in [&mut enemy, &mut patient, &mut healer] {
            watcher.pump(&schemas);
        }
    }
    let wounded = health(&patient).unwrap();
    assert!(wounded < full && wounded > 0.0, "wounded to {wounded}");

    let cast = [&enemy, &patient, &healer].map(|watcher| watcher.datagrams.len());
    let mut next = healer.tick + 1;
    healer
        .client
        .send(ACK, command_batch(next, 1, 0, ABILITY_2, false));
    next += 1;
    let field =
        |record: &WorldRecord| (FIELD_FIRST..FIELD_FIRST + FIELD_SPAN).contains(&record.entity);
    let created = |watcher: &Watcher, from: usize| -> Vec<u32> {
        watcher.datagrams[from..]
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| field(record) && record.is_create())
            .map(|record| record.entity)
            .collect()
    };
    let destroyed = |watcher: &Watcher, entity: u32| {
        watcher
            .find(0, |record| {
                record.entity == entity && record.operation == Some(3)
            })
            .is_some()
    };
    pump_commanding(
        &mut [&mut enemy, &mut patient, &mut healer],
        &schemas,
        "the fields' destroys",
        2,
        &mut next,
        |watchers| {
            watchers.iter().zip(cast).all(|(watcher, from)| {
                let fields = created(watcher, from);
                !fields.is_empty() && fields.iter().all(|&entity| destroyed(watcher, entity))
            })
        },
    );
    for (watcher, from) in [(&enemy, cast[0]), (&patient, cast[1]), (&healer, cast[2])] {
        let fields = created(watcher, from);
        if watcher.owner != healer.owner {
            assert_eq!(fields.len(), 1, "another client's field, at the feet");
        }
        for entity in fields {
            let create = watcher
                .find(from, |record| record.entity == entity && record.is_create())
                .unwrap();
            let (_, records) = &watcher.datagrams[create];
            let record = records
                .iter()
                .find(|record| record.entity == entity && record.is_create())
                .unwrap();
            assert_eq!(record.definition, Some(FIELD));
            let frame = watcher
                .find(create, |record| {
                    record.entity == entity && record.script.is_some()
                })
                .expect("the field's frame");
            assert!(frame > create, "its frame follows the create's arrival");
            let gone = watcher
                .find(create, |record| {
                    record.entity == entity && record.operation == Some(3)
                })
                .unwrap();
            if watcher.owner != healer.owner {
                let lived = watcher.ticks[gone] - watcher.ticks[create];
                assert!(
                    (FIELD_TICKS - 4..=FIELD_TICKS + 30).contains(&lived),
                    "the field lived {lived} ticks"
                );
            }
        }
    }
    let healed = health(&patient).unwrap();
    assert!(healed > wounded, "healed from {wounded} to {healed}");
    assert!(
        healer
            .find(cast[2], |record| {
                record.entity == healer.owner && record.updated == Some(GMP_STATS)
            })
            .is_some(),
        "the healing reaches the healer's board"
    );
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
    use orpheus_script::wire::{Value as ScriptValue, Variables};
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
    let catalog = MapCatalog::load(runtime.join("data")).unwrap();
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
            None,
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
            ticks: Vec::new(),
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
    let clock = |seconds: f64| {
        scene
            .mode_frame(&Variables::from([(
                orpheus_sim::match_flow::CLOCK,
                ScriptValue::Float(seconds),
            )]))
            .unwrap()
    };
    let (counting, over) = (clock(12.5), clock(0.0));
    for watcher in [&*first, &*second] {
        let (closed, _) = controllers(watcher)
            .into_iter()
            .find(|(_, chunk)| shows(chunk, false, false, Some(SOLDIER)))
            .unwrap();
        let frames: Vec<_> = mode_frames(watcher)
            .into_iter()
            .filter(|(index, _)| *index <= closed)
            .collect();
        let (_, last) = frames.last().unwrap();
        // One frame a second for 30 s, the mode entity's first ones; then
        // the round's, whose clock Skirmish doesn't run.
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
    // His first frame may be the mode UI without variables (he was
    // presented after the heroes assembled); then the round's, with no
    // countdown (Skirmish has no round clock).
    let frames: Vec<_> = mode_frames(&third)
        .into_iter()
        .filter(|(_, chunk)| chunk.width != scene.mode_ui().bit_len())
        .collect();
    assert!(
        frames
            .iter()
            .all(|(_, chunk)| chunk.data.as_slice() == over.as_bytes()),
        "the round's frames, no countdown"
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

/// A Quick Play Assault match on Temple of Anubis runs its whole flow on a
/// time scale of 1/10 (orpheus_sim::match_flow): assemble, setup, the
/// round, the capture of A and then B by the attacker, the round's end,
/// the match's end; the result goes to the lobby (a win for the attackers'
/// team, a loss for the defenders') and both players go home (20304).
///
/// The test moves both points into the attackers' spawn room, so the
/// attacker captures them standing where he spawns: walking 125 m to A
/// needs the movement the test client doesn't play.
#[test]
#[ignore = "requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR); builds Anubis' collision once"]
fn an_assault_match_on_anubis_captures_a_and_b_and_sends_the_players_home() {
    use orpheus_content::{
        heroes::SOLDIER,
        maps::{MapCatalog, ObjectiveKind},
        parameters::PICK_HERO,
    };
    use orpheus_script::wire::{Value as ScriptValue, Variables};
    use orpheus_sim::{
        match_flow::{ASSAULT, ATTACKERS, BANNER, CLOCK, DEFENDERS, OBJECTIVE_NAME},
        scene_assets::SceneAssets,
    };

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const SCALE: f64 = 0.1;

    let runtime = orpheus_testkit::runtime_dir();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(collision_build(orpheus_testkit::game_dir())),
    );
    let catalog = MapCatalog::load(runtime.join("data")).unwrap();
    let mut map = catalog
        .game_map(ANUBIS, ASSAULT, "Assault", false, &[6, 6])
        .expect("Temple of Anubis in Assault");
    let names: Vec<_> = map
        .objectives_of(ObjectiveKind::Point)
        .map(|point| point.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(names, ["Point A", "Point B"]);
    let (attack_spawn, _) = map.spawn_point(1, 0).unwrap();
    let (defense_spawn, _) = map.spawn_point(0, 0).unwrap();
    let distance = |a: [f64; 3], b: [f64; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);
    assert!(distance(attack_spawn, defense_spawn) > 50.0);
    for objective in &mut map.objectives {
        objective.position = attack_spawn;
    }

    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(orpheus_sim::match_flow::Timers { scale: SCALE })
        .unwrap();
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
            vec![player("defender#1111", 0, 7), player("attacker#2222", 1, 8)],
            None,
            prepare,
        )
        .unwrap();
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
                ticks: Vec::new(),
            }
        })
        .collect();
    let [defender, attacker] = &mut watchers[..] else {
        unreachable!()
    };
    let received = |watcher: &Watcher, id: u32| {
        watcher
            .datagrams
            .iter()
            .position(|(messages, _)| messages.iter().any(|message| message.id == id))
    };
    pump_until(
        &mut [defender, attacker],
        &schemas,
        "the instances",
        |watchers| {
            watchers
                .iter()
                .all(|watcher| received(watcher, 20300).is_some())
        },
    );
    for watcher in [&mut *defender, &mut *attacker] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    pump_within(
        &mut [defender, attacker],
        &schemas,
        "the players in the world",
        Duration::from_secs(900),
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher.created(GAME_MODE_ENTITY).is_some()
                    && watcher
                        .find(0, |record| {
                            record.entity == watcher.owner && record.script.is_some()
                        })
                        .is_some()
            })
        },
    );
    for watcher in [&mut *defender, &mut *attacker] {
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
    pump_until(
        &mut [defender, attacker],
        &schemas,
        "the bodies",
        |watchers| {
            watchers
                .iter()
                .all(|watcher| watcher.created(watcher.owner | 1).is_some())
        },
    );
    // 3 s assemble, 4.5 s setup, A in 3 s, B in 1.8 s, 1.16 s of round end
    // and 2 s of match end.
    pump_within(
        &mut [defender, attacker],
        &schemas,
        "the end of the match",
        Duration::from_secs(60),
        |watchers| {
            watchers
                .iter()
                .all(|watcher| received(watcher, 20304).is_some())
        },
    );
    let results = server.take_results().unwrap();
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(
        (result.id, result.map, result.mode),
        (MATCH, ANUBIS, ASSAULT)
    );
    let outcomes: Vec<_> = result
        .players
        .iter()
        .map(|player| {
            (
                player.name.as_str(),
                player.team,
                player.outcome,
                player.finished,
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("defender#1111", 0, Outcome::Loss, true),
            ("attacker#2222", 1, Outcome::Win, true),
        ]
    );
    assert!(server.take_results().unwrap().is_empty());

    // The mode entity's frames, one a second and at each phase: the clock
    // with the phase banner and the objective's name while the round is
    // on, then the clock alone (no banner, no objective left) at the round's
    // and the match's end.
    let scene = assets.prepare(map.clone()).unwrap();
    let width = |extra: &[(u32, ScriptValue)]| {
        let mut variables = Variables::from([(CLOCK, ScriptValue::Float(1.0))]);
        variables.extend(extra.iter().cloned());
        variables.extend([
            (ATTACKERS, ScriptValue::Integer(1)),
            (DEFENDERS, ScriptValue::Integer(0)),
        ]);
        scene.mode_frame(&variables).unwrap().bit_len()
    };
    let playing = width(&[
        (BANNER, ScriptValue::Asset(0x0de0_0000_0000_6116)),
        (OBJECTIVE_NAME, ScriptValue::String("Point A".into())),
    ]);
    let over = width(&[]);
    assert_ne!(playing, over);
    for watcher in [&*defender, &*attacker] {
        let widths: Vec<usize> = watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| record.entity == GAME_MODE_ENTITY)
            .filter_map(|record| record.script.as_ref().map(|chunk| chunk.width))
            // A client presented after the heroes assembled first gets the
            // mode UI without variables.
            .filter(|&width| width != scene.mode_ui().bit_len())
            .collect();
        // The playing frames carry the banner and the objective's name; at
        // the round's and the match's end they're gone (the clock's compact
        // form changes the widths, so compare with the widest).
        assert!(widths.len() >= 5, "{widths:?}");
        let widest = widths.iter().copied().max().unwrap();
        assert!(widest > over, "{widths:?}");
        assert!(
            widths[widths.len() - 2..]
                .iter()
                .all(|&width| width < widest)
        );
    }
    server.stop().unwrap();
}

/// Practice vs AI, Easy, on Temple of Anubis (orpheus_sim::ai) at a time
/// scale of 1/4: one player on the blue team (the defenders) and eleven AI
/// heroes, five with him and six attacking. His game gets them as other
/// players (their cards with their heroes' names, their bodies); the AI
/// heroes pick roster heroes, the attackers walk from their spawn toward
/// point A, the AI heroes of both teams fight (bodies lose health or die),
/// and the match flow ends the match: the result has the player only, and
/// his game goes home.
#[test]
#[ignore = "requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR); builds Anubis' collision once"]
fn a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end() {
    use orpheus_content::{
        heroes::SOLDIER,
        maps::{MapCatalog, ObjectiveKind},
        parameters::PICK_HERO,
    };
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        ai::{Difficulty, ROSTER},
        match_flow::ASSAULT,
        scene_assets::SceneAssets,
    };

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const SCALE: f64 = 0.25;

    let runtime = orpheus_testkit::runtime_dir();
    let mut assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(collision_build(orpheus_testkit::game_dir()));
    // The roster heroes' graphs still have gaps, and every body's graph
    // `0B8B` reaches TrackStatTime (`STU_441B50E4`) on the ultimate's
    // charge (`ULT_CHARGE` against `ULT_COST`): the bodies run them as a
    // release build does.
    assets.set_player_unknowns(Unknowns::Tolerate);
    let assets = Arc::new(assets);
    let catalog = MapCatalog::load(runtime.join("data")).unwrap();
    let map = catalog
        .game_map(ANUBIS, ASSAULT, "Assault", false, &[6, 6])
        .expect("Temple of Anubis in Assault");
    let point_a = map
        .objectives_of(ObjectiveKind::Point)
        .next()
        .expect("point A")
        .position;
    let flat = |a: [f64; 3], b: [f64; 3]| (a[0] - b[0]).hypot(a[2] - b[2]);

    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(orpheus_sim::match_flow::Timers { scale: SCALE })
        .unwrap();
    let player = WorldPlayer {
        player: MatchPlayer {
            name: "player#1111".into(),
            team: 0,
            tournament: false,
            ranked: false,
        },
        card: PlayerCard {
            account: [7, 0x0100_0000_0000_0000],
            battle_tag: "player#1111".into(),
            record: Fields::from([(0x40, Value::String("player#1111".into()))]),
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
            vec![player],
            Some(Difficulty::Easy),
            prepare,
        )
        .unwrap();
    assert_eq!(tickets.len(), 1);
    // Five AI heroes with the player, six against him, each a roster hero
    // of its own team.
    let roster = server.bodies(MATCH).unwrap();
    assert_eq!(roster.len(), 12);
    assert!(!roster[0].ai);
    for team in [0, 1] {
        let names: Vec<_> = roster
            .iter()
            .filter(|body| body.ai && body.team == team)
            .map(|body| body.name.as_str())
            .collect();
        assert_eq!(names.len(), 5 + usize::from(team));
        assert!(names.iter().all(|name| ROSTER.contains(name)));
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
    }

    let mut watcher = Watcher {
        client: Client::new(server.local_addr(), &tickets[0]),
        owner: player_entity(0),
        tick: 0,
        sequences: HashSet::new(),
        datagrams: Vec::new(),
        ticks: Vec::new(),
    };
    watcher.client.handshake();
    pump_until(&mut [&mut watcher], &schemas, "the instance", |watchers| {
        watchers[0]
            .datagrams
            .iter()
            .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
    });
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    watcher.client.send(ACK, payload.into_bytes());
    pump_within(
        &mut [&mut watcher],
        &schemas,
        "the player in the world",
        Duration::from_secs(900),
        |watchers| {
            let watcher = &watchers[0];
            watcher.created(GAME_MODE_ENTITY).is_some()
                && watcher
                    .find(0, |record| {
                        record.entity == watcher.owner && record.script.is_some()
                    })
                    .is_some()
        },
    );
    let owner = watcher.owner;
    send_player_message(
        &mut watcher.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );

    // The match: a sample of the bodies every quarter second until the
    // player's game goes home.
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut samples = Vec::new();
    let mut next_sample = Instant::now();
    let mut home = false;
    while !home {
        watcher.pump(&schemas);
        home = watcher
            .datagrams
            .last()
            .is_some_and(|(messages, _)| messages.iter().any(|message| message.id == 20304));
        if Instant::now() >= next_sample {
            let bodies = server.bodies(MATCH).unwrap();
            if !bodies.is_empty() {
                samples.push(bodies);
            }
            next_sample += Duration::from_millis(250);
        }
        assert!(
            Instant::now() < deadline,
            "waiting for the end of the match"
        );
    }

    // The AI heroes are other players to the player's game: a card with
    // each one's name, and his body.
    for (slot, body) in roster.iter().enumerate().skip(1) {
        assert!(
            watcher.card(0, &body.name).is_some(),
            "no card of {}",
            body.name
        );
        let entity = player_entity(slot);
        assert!(
            watcher
                .find(0, |record| record.entity & !0xff == entity
                    && record.entity != entity
                    && record.is_create())
                .is_some(),
            "no body of {}",
            body.name
        );
    }
    // Every AI hero played a hero with its weapons.
    for slot in 1..12 {
        assert!(
            samples
                .iter()
                .any(|bodies| bodies[slot].hero.is_some() && bodies[slot].scripted),
            "{} has no hero",
            roster[slot].name
        );
    }
    // The attackers walked toward point A: each came far closer than his
    // spawn is.
    let (attack_spawn, _) = map.spawn_point(1, 0).unwrap();
    let start = flat(attack_spawn, point_a);
    let closest = |slot: usize| {
        samples
            .iter()
            .filter_map(|bodies| bodies[slot].position)
            .map(|feet| flat(feet, point_a))
            .fold(f64::INFINITY, f64::min)
    };
    let attackers: Vec<usize> = (1..12).filter(|&slot| roster[slot].team == 1).collect();
    let approached = attackers
        .iter()
        .filter(|&&slot| closest(slot) < start - 40.0)
        .count();
    assert!(
        approached >= 3,
        "from {start:.0} m: {:?}",
        attackers
            .iter()
            .map(|&slot| closest(slot))
            .collect::<Vec<_>>()
    );
    // They fought: AI heroes were hurt or killed.
    let hurt = (1..12)
        .filter(|&slot| {
            samples.iter().any(|bodies| {
                bodies[slot].health.is_some_and(|health| health < 0.99)
                    || (bodies[slot].hero.is_some() && bodies[slot].position.is_none())
            })
        })
        .count();
    assert!(hurt >= 2, "{hurt} AI heroes hurt");

    // The result counts for the player only.
    let results = server.take_results().unwrap();
    assert_eq!(results.len(), 1);
    let result = &results[0];
    assert_eq!(
        (result.id, result.map, result.mode),
        (MATCH, ANUBIS, ASSAULT)
    );
    assert_eq!(result.players.len(), 1);
    let me = &result.players[0];
    assert_eq!(
        (me.slot, me.name.as_str(), me.team, me.finished),
        (0, "player#1111", 0, true)
    );
    server.stop().unwrap();
}

/// A Competitive Assault match on Temple of Anubis plays both sides on a
/// time scale of 1/50 (orpheus_sim::match_flow): team 1 attacks first and
/// takes A, then time runs out on B; before the second side hero select
/// reopens and both players get a new body in their new side's spawn room
/// (team 0 now in the attackers' room, team 1 in the defenders'), where
/// team 0 takes A too. Equal progress: a draw for both (rules 63-64).
///
/// A stands in the attackers' first room and B in the defenders' room, so
/// each attacker captures A where he spawns and no one can reach B: the
/// test client plays no movement. Team 0 can only take A if the side swap
/// moved him.
#[test]
#[ignore = "requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR); builds Anubis' collision once"]
fn a_competitive_assault_on_anubis_swaps_sides_and_spawn_rooms() {
    use orpheus_content::{
        heroes::SOLDIER,
        maps::{MapCatalog, ObjectiveKind, SpawnChoice},
        parameters::PICK_HERO,
    };
    use orpheus_sim::{match_flow::ASSAULT, scene_assets::SceneAssets};

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const SCALE: f64 = 0.02;

    let runtime = orpheus_testkit::runtime_dir();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(collision_build(orpheus_testkit::game_dir())),
    );
    let catalog = MapCatalog::load(runtime.join("data")).unwrap();
    let mut map = catalog
        .game_map(ANUBIS, ASSAULT, "Assault", false, &[6, 6])
        .expect("Temple of Anubis in Assault");
    let room = |attack| SpawnChoice::Objective {
        objective: 0,
        attack,
    };
    let (attack_spawn, _) = map.spawn_point_in(0, 0, room(true)).unwrap();
    let (defense_spawn, _) = map.spawn_point_in(1, 0, room(false)).unwrap();
    // The rooms of a side are the same whichever team plays it.
    assert_eq!(
        map.spawn_point_in(1, 0, room(true)).unwrap().0,
        attack_spawn
    );
    assert_eq!(map.spawn_point(1, 0).unwrap().0, attack_spawn);
    assert_eq!(map.spawn_point(0, 0).unwrap().0, defense_spawn);
    for objective in &mut map.objectives {
        if objective.kind == ObjectiveKind::Point {
            objective.position = if objective.order == 0 {
                attack_spawn
            } else {
                defense_spawn
            };
        }
    }

    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(orpheus_sim::match_flow::Timers { scale: SCALE })
        .unwrap();
    let player = |tag: &str, team, account| WorldPlayer {
        player: MatchPlayer {
            name: tag.into(),
            team,
            tournament: false,
            ranked: true,
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
            vec![player("blue#1111", 0, 7), player("red#2222", 1, 8)],
            None,
            prepare,
        )
        .unwrap();
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
                ticks: Vec::new(),
            }
        })
        .collect();
    let [blue, red] = &mut watchers[..] else {
        unreachable!()
    };
    let received = |watcher: &Watcher, id: u32| {
        watcher
            .datagrams
            .iter()
            .position(|(messages, _)| messages.iter().any(|message| message.id == id))
    };
    pump_until(&mut [blue, red], &schemas, "the instances", |watchers| {
        watchers
            .iter()
            .all(|watcher| received(watcher, 20300).is_some())
    });
    for watcher in [&mut *blue, &mut *red] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    pump_within(
        &mut [blue, red],
        &schemas,
        "the players in the world",
        Duration::from_secs(900),
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher.created(GAME_MODE_ENTITY).is_some()
                    && watcher
                        .find(0, |record| {
                            record.entity == watcher.owner && record.script.is_some()
                        })
                        .is_some()
            })
        },
    );
    for watcher in [&mut *blue, &mut *red] {
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
    pump_until(&mut [blue, red], &schemas, "the bodies", |watchers| {
        watchers
            .iter()
            .all(|watcher| watcher.created(watcher.owner | 1).is_some())
    });
    // Per side: 0.5-0.6 s of hero select, 0.9 s of setup, 7.8 s of round
    // (A in 0.6 s adds 3.6 s), 0.23 s of round end; then 0.4 s of match end.
    pump_within(
        &mut [blue, red],
        &schemas,
        "the end of the match",
        Duration::from_secs(120),
        |watchers| {
            watchers
                .iter()
                .all(|watcher| received(watcher, 20304).is_some())
        },
    );
    // The second side's bodies, in the swapped rooms.
    for watcher in [&*blue, &*red] {
        assert!(
            watcher.created(watcher.owner | 2).is_some(),
            "{:08X}: a new body for the second side",
            watcher.owner
        );
    }
    let results = server.take_results().unwrap();
    assert_eq!(results.len(), 1);
    let outcomes: Vec<_> = results[0]
        .players
        .iter()
        .map(|player| (player.team, player.outcome, player.finished))
        .collect();
    assert_eq!(
        outcomes,
        [(0, Outcome::Draw, true), (1, Outcome::Draw, true)]
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

/// The Practice Range with the training bots' scripts (ow174/game/match.py
/// `_prepare_bot_script`, server.py `_BotScriptDelivery`): the world waits
/// for every bot's script; each bot's first frame follows its create, the
/// nameplate follows the mode UI and carries the bot's name; a bot shot to
/// death sends its death script's frames, and its replacement 3 s later
/// starts a new script.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn practice_bots_run_their_scripts_for_a_client() {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        body_script::fixture_hitscan_factory, bots::BOT_ENTITY, scene_assets::SceneAssets,
    };
    use orpheus_wire::commands::FIRE;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    // The stock bot graphs hold content the VM doesn't know yet; the Python
    // reference ran them anyway.
    assets.set_bot_unknowns(Unknowns::Tolerate);
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    let [x, y, z] = scene.map().spawn_point(0, 0).unwrap().0;
    let bots: Vec<(u32, bool)> = scene
        .bots()
        .iter()
        .map(|bot| (bot.entity, bot.spawn.firing))
        .collect();
    let names: Vec<(u32, String)> = scene
        .bots()
        .iter()
        .map(|bot| (bot.entity, bot.spawn.name.clone()))
        .collect();
    // An enemy bot that neither fires nor walks, 4 m beside the spawn.
    let target = scene
        .bots_mut()
        .iter_mut()
        .find(|bot| !bot.spawn.firing && bot.spawn.team != 0 && bot.spawn.route.is_empty())
        .unwrap();
    target.position = [x - 4.0, y, z];
    let target = target.entity;
    let factory = fixture_hitscan_factory(100.0);
    scene.owner_body_factory = Some(Arc::new(move |hero, spawn| {
        if hero.guid != SOLDIER {
            return Ok(None);
        }
        Ok(Some(Box::new(factory.start(spawn)?)))
    }));
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let ticket = server
        .open_prepared_practice(MATCH, scene, player().remove(0), PlayerCard::default())
        .unwrap();
    let mut client = Client::new(server.local_addr(), &ticket);
    client.handshake();
    let mut watcher = Watcher {
        client,
        owner: player_entity(0),
        tick: 0,
        sequences: HashSet::new(),
        datagrams: Vec::new(),
        ticks: Vec::new(),
    };
    pump_until(&mut [&mut watcher], &schemas, "the instance", |watchers| {
        watchers[0]
            .datagrams
            .iter()
            .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
    });
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    watcher.client.send(ACK, payload.into_bytes());

    // Each bot's first frame, after its create; then, after the mode UI, a
    // bot that doesn't fire changes only by its nameplate.
    let script = |watcher: &Watcher, from: usize, entity: u32, full: bool| {
        watcher.find(from, |record| {
            record.entity == entity
                && record
                    .script
                    .as_ref()
                    .is_some_and(|script| (script.first == 0) == full)
        })
    };
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the bots' first frames and nameplates",
        |watchers| {
            let watcher = &watchers[0];
            let mode = watcher.find(0, |record| {
                record.entity == GAME_MODE_ENTITY && record.script.is_some()
            });
            bots.iter().all(|&(entity, firing)| {
                let created = watcher.created(entity);
                let full = script(watcher, 0, entity, true);
                assert!(full.is_none() || created < full);
                full.is_some()
                    && (firing
                        || mode.is_some_and(|mode| script(watcher, mode, entity, false).is_some()))
            })
        },
    );
    // Each bot's nameplate text, its hero's name, in its frames after the
    // mode UI (a firing bot's goes with a live frame).
    let texts: Vec<(u32, BitWriter)> = names
        .iter()
        .map(|(entity, name)| {
            let mut text = BitWriter::new();
            orpheus_script::wire::write_value(
                &mut text,
                &orpheus_script::wire::Value::String(name.clone()),
                orpheus_script::wire::ValueEncoding::Compact,
            )
            .unwrap();
            (*entity, text)
        })
        .collect();
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the bots' names in their nameplates",
        |watchers| {
            let watcher = &watchers[0];
            let Some(mode) = watcher.find(0, |record| {
                record.entity == GAME_MODE_ENTITY && record.script.is_some()
            }) else {
                return false;
            };
            texts.iter().all(|(entity, text)| {
                watcher
                    .find(mode, |record| {
                        record.entity == *entity
                            && record.script.as_ref().is_some_and(|chunk| {
                                orpheus_testkit::contains_bits(
                                    &chunk.data,
                                    chunk.width,
                                    text.as_bytes(),
                                    text.bit_len(),
                                )
                            })
                    })
                    .is_some()
            })
        },
    );

    // Shoot the target to death.
    let owner = watcher.owner;
    send_player_message(
        &mut watcher.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    pump_until(&mut [&mut watcher], &schemas, "the body", |watchers| {
        watchers[0].created(owner | 1).is_some()
    });
    let commands = aimed_batch(watcher.tick + 1, 1, -16384, AT_BOT_HEAD_4M, FIRE, false);
    watcher.client.send(ACK, commands.clone());
    watcher.client.send(ACK, commands);
    let replacement = BOT_ENTITY + 15;
    pump_until(&mut [&mut watcher], &schemas, "the kill", |watchers| {
        watchers[0]
            .datagrams
            .iter()
            .any(|(messages, _)| messages.iter().any(|message| message.id == 25005))
    });
    let killed_at = Instant::now();
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the replacement",
        |watchers| watchers[0].created(replacement).is_some(),
    );
    // combat.py `BOT_RESPAWN`: 3 s.
    assert!(killed_at.elapsed() >= Duration::from_millis(2_800));
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the replacement's first frame",
        |watchers| script(watchers[0], 0, replacement, true).is_some(),
    );
    // The death script's frames go with the terminal health, before the
    // destroy; the replacement's first frame after its create.
    let terminal = watcher
        .find(0, |record| {
            record.entity == target && record.base_health == Some(0.0)
        })
        .unwrap();
    let death = script(&watcher, terminal, target, false).unwrap();
    let destroyed = watcher
        .find(0, |record| {
            record.entity == target && record.operation == Some(3)
        })
        .unwrap();
    let created = watcher.created(replacement).unwrap();
    assert!(death <= destroyed && destroyed <= created);
    assert!(created < script(&watcher, 0, replacement, true).unwrap());
    server.stop().unwrap();
}

/// combat.py `_bot_weapons`: a firing training bot's script fires its
/// rockets along its yaw, and they damage the player in front of it.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_firing_practice_bot_shoots_the_player() {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_bot_unknowns(Unknowns::Tolerate);
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    let [x, y, z] = scene.map().spawn_point(0, 0).unwrap().0;
    // An enemy firing bot 4 m beside the player's spawn, facing it (a
    // quarter turn: along +x), where the other bots leave the line clear.
    let shooter = scene
        .bots_mut()
        .iter_mut()
        .find(|bot| bot.spawn.firing && bot.spawn.team != 0)
        .unwrap();
    shooter.position = [x - 4.0, y, z];
    shooter.yaw = 16384.0;
    let shooter = shooter.entity;
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let ticket = server
        .open_prepared_practice(MATCH, scene, player().remove(0), PlayerCard::default())
        .unwrap();
    let mut client = Client::new(server.local_addr(), &ticket);
    client.handshake();
    let mut watcher = Watcher {
        client,
        owner: player_entity(0),
        tick: 0,
        sequences: HashSet::new(),
        datagrams: Vec::new(),
        ticks: Vec::new(),
    };
    pump_until(&mut [&mut watcher], &schemas, "the instance", |watchers| {
        watchers[0]
            .datagrams
            .iter()
            .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
    });
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    watcher.client.send(ACK, payload.into_bytes());
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the controller",
        |watchers| {
            watchers[0]
                .find(0, |record| {
                    record.entity == watchers[0].owner && record.script.is_some()
                })
                .is_some()
        },
    );
    let owner = watcher.owner;
    send_player_message(
        &mut watcher.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    let body = owner | 1;
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the bot's rocket",
        |watchers| {
            watchers[0]
                .find(0, |record| {
                    record.entity == body && record.base_health.is_some_and(|health| health < 200.0)
                })
                .is_some()
        },
    );
    // The bot's script is what fires: its live frames went to the client.
    assert!(
        watcher
            .find(0, |record| {
                record.entity == shooter
                    && record
                        .script
                        .as_ref()
                        .is_some_and(|script| script.first != 0)
            })
            .is_some()
    );
    // projectiles.py `_show_others` for the bot: the client gets a rocket
    // entity of the bot's projectile definition (1476), the bot's shot record
    // naming it once its create arrived (server.py `retain_bot_volley`
    // keeps the volley in the bot's frames until then), and its destroy
    // after it landed. The first rocket created gets its own launch.
    let seen = |entity: u32| (SEEN_FIRST..SEEN_FIRST + SEEN_SPAN).contains(&entity);
    let first = |watcher: &Watcher| {
        watcher
            .find(0, |record| seen(record.entity) && record.is_create())
            .map(|created| {
                let rocket = watcher.datagrams[created]
                    .1
                    .iter()
                    .find(|record| seen(record.entity) && record.is_create())
                    .unwrap()
                    .entity;
                (created, rocket)
            })
    };
    // The firing bots' shot records: the first rocket may be another bot's.
    let launch = |watcher: &Watcher, rocket: u32| {
        watcher.find(0, |record| record.launches.contains(&rocket))
    };
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the first rocket's launch and destroy",
        |watchers| {
            first(watchers[0]).is_some_and(|(_, rocket)| {
                launch(watchers[0], rocket).is_some_and(|launched| {
                    watchers[0]
                        .find(launched, |record| {
                            record.entity == rocket && record.operation == Some(3)
                        })
                        .is_some()
                })
            })
        },
    );
    let (created, rocket) = first(&watcher).unwrap();
    let launched = launch(&watcher, rocket).unwrap();
    let (_, records) = &watcher.datagrams[created];
    let create = records
        .iter()
        .find(|record| record.entity == rocket && record.is_create())
        .unwrap();
    assert_eq!(create.definition, Some(0x1476));
    assert!(
        launched > created,
        "the launch waits for the create's arrival"
    );
    let destroyed = watcher
        .find(launched, |record| {
            record.entity == rocket && record.operation == Some(3)
        })
        .expect("the rocket's destroy");
    assert!(destroyed >= launched);
    server.stop().unwrap();
}

mod collision_ready;
