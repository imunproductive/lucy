use super::*;
use lockstep::{Client, Lockstep};
use orpheus_content::maps::PRACTICE_RANGE_MAP;
use orpheus_core::bits::{BitReader, BitWriter};
use orpheus_wire::{
    link::{FIN, KIND_CONTROL, SYN},
    messages::{read_channel, write_channel},
};
use std::collections::HashSet;

const MATCH: MatchId = [5, 8];

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

/// The server tick a frame datagram was sent at.
fn tick_of(packet: &Packet) -> u32 {
    BitReader::new(&packet.payload).var_a().unwrap()
}

/// The tick the service ran at `time`: the first runs at time 0
/// (`GameClock`).
fn tick_at(time: Duration) -> u32 {
    FIRST_TICK + 1 + lockstep::ticks(time)
}

fn messages(schemas: &Schemas, packet: &Packet) -> Vec<orpheus_wire::messages::ReliableMessage> {
    assert_eq!(packet.kind, KIND_FRAME);
    let mut reader = BitReader::new(&packet.payload);
    reader.var_a().unwrap();
    read_channel(&mut reader, schemas).unwrap().reliable
}

fn exercise_loading(schemas: Schemas) {
    let schemas = Arc::new(schemas);
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let mut tickets = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap();
    let ticket = tickets.pop().unwrap();
    assert!(server.handoff_pending(ticket.conn).unwrap());
    let mut client = Client::new(&server, &ticket);
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
    // The ACK leaves the first datagram out: its reliable messages are
    // lost, and go again in the next frame (`GameEgress::acknowledge`).
    let retry = client.receive();
    // A datagram every tick: the retry is the heartbeat's next tick.
    assert_eq!(tick_of(&retry), tick_of(&heartbeat) + 1);
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
    // The map is loaded at the last tick's time, when the datagram arrives.
    let loaded_at = client.now();
    client.send(ACK, payload.into_bytes());
    let mut received = Vec::new();
    while received.len() < 3 && client.now() < loaded_at + Duration::from_secs(2) {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        let tick = tick_of(&packet);
        received.extend(
            messages(&schemas, &packet)
                .into_iter()
                .map(|item| (tick, item)),
        );
        client.send(ACK, Vec::new());
    }
    assert_eq!(
        received
            .iter()
            .map(|(_, message)| (message.sequence, message.message.id))
            .collect::<Vec<_>>(),
        [(1, 20306), (2, 20302), (3, 20301)]
    );
    // The pong goes with the next tick's datagram; the ownership (20302)
    // and the game (20301) with the tick 0.3 s and 0.4 s after the map
    // loaded (`MapReadyStep::delay`): 19 and 25 ticks of 16 ms. Each tick
    // sends what it queued.
    let loaded = tick_at(loaded_at);
    assert_eq!(
        received.iter().map(|(tick, _)| *tick).collect::<Vec<_>>(),
        [loaded + 1, loaded + 19, loaded + 25]
    );
    let received: Vec<_> = received.into_iter().map(|(_, message)| message).collect();
    assert_eq!(received[0].message.fields[&0x78], Value::Unsigned(17));
    let Value::Array(stamps) = &received[0].message.fields[&0x80] else {
        panic!("expected ping timestamps");
    };
    assert_eq!(stamps.len(), 3);
    assert_eq!(stamps[0], Value::Unsigned(25));

    client.send(FIN | ACK, Vec::new());
    let until = client.now() + Duration::from_secs(2);
    loop {
        let packet = client.receive();
        if packet.kind == KIND_CONTROL {
            assert_eq!(packet.flags, FIN | ACK);
            break;
        }
        assert!(client.now() < until, "waiting for FIN acknowledgement");
    }
    server.stop().unwrap();
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
    let mut server = Lockstep::new(Arc::new(fixture())).unwrap();
    let ticket = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap()
        .pop()
        .unwrap();
    assert!(server.handoff_pending(ticket.conn).unwrap());
    assert!(server.cancel_handoff(ticket.conn).unwrap());
    assert!(!server.handoff_pending(ticket.conn).unwrap());
    let mut client = Client::new(&server, &ticket);
    client.send(SYN, Vec::new());
    client.hears_nothing(Duration::from_millis(100));
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
    let until = client.now() + Duration::from_secs(3);
    while client.now() < until {
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let ticket = server
        .open_match(MATCH, GameMap::practice_range(), player())
        .unwrap()
        .pop()
        .unwrap();
    assert!(server.release_seat(&ticket).unwrap());
    // He left at once: the seat is gone, and the match with its last player.
    assert!(!server.seat_exists(&ticket).unwrap());
    assert!(server.snapshot().unwrap().is_empty());

    let mut client = Client::new(&server, &ticket);
    client.handshake();
    assert_eq!(first_reliable(&client, &schemas), 20304);
    server.run(Duration::from_millis(100));
    // The game never joined a match.
    assert!(!server.seat_exists(&ticket).unwrap());
    assert!(server.snapshot().unwrap().is_empty());

    client.send(FIN | ACK, Vec::new());
    let until = client.now() + Duration::from_secs(2);
    loop {
        let packet = client.receive();
        if packet.kind == KIND_CONTROL {
            assert_eq!(packet.flags, FIN | ACK);
            break;
        }
        assert!(client.now() < until, "waiting for FIN acknowledgement");
    }
    server.stop().unwrap();
}

#[derive(Debug)]
struct WorldRecord {
    entity: u32,
    operation: Option<u8>,
    definition: Option<u32>,
    movement: bool,
    /// The movement record's head, when it has one.
    moved: Option<MovementHead>,
    skin: Option<(u32, bool)>,
    script: Option<ReceivedChunk>,
    projectile_release: Option<u32>,
    /// The release state's position: where an explosion the client's own
    /// projectile hasn't had yet plays.
    release_position: Option<[f32; 3]>,
    /// The channel-2 shot records' projectile ids (projectiles.py
    /// `shot_events`): a launch the other clients see.
    launches: Vec<u32>,
    base_health: Option<f32>,
    /// The first component of an update with one component.
    updated: Option<u8>,
    /// A create's or update's components, when they are all of the kinds
    /// [`decode_components`] reads.
    components: Option<Vec<DecodedComponent>>,
}

/// A component's index and its fields (absent ones `None`).
type DecodedComponent = (u8, Vec<Option<DecodedField>>);

#[derive(Clone, Debug, PartialEq)]
enum DecodedField {
    Id(u64),
    DbId(u64, u64),
    Entity(u32),
    Text(String),
    /// A field of plain bytes (a health pool's 20).
    Bytes(Vec<u8>),
}

/// The components of a create or update (`orpheus_wire::entity`
/// `write_components`) when each is a 26, 29 (id and BattleTag), 44, 51
/// (the pools), 75 or 123, the components of a player's or a bot's stand-in
/// controller and of an observed or a bot's body; `None` for any other.
fn decode_components(mut reader: BitReader<'_>) -> Option<Vec<DecodedComponent>> {
    #[derive(Clone, Copy)]
    enum Kind {
        Id,
        DbId,
        Entity,
        Text,
        Bytes(usize),
    }
    /// `orpheus_wire::entity` HEALTH: 48 pools, then two u32, the killer
    /// and two u8.
    const HEALTH: [Kind; 53] = {
        let mut fields = [Kind::Bytes(20); 53];
        fields[48] = Kind::Bytes(4);
        fields[49] = Kind::Bytes(4);
        fields[50] = Kind::Entity;
        fields[51] = Kind::Bytes(1);
        fields[52] = Kind::Bytes(1);
        fields
    };
    fn byte_masked(reader: &mut BitReader<'_>) -> Option<u64> {
        let present = reader.bits(8).ok()?;
        let mut value = 0;
        for byte in 0..8 {
            if present & (1 << byte) != 0 {
                value |= reader.bits(8).ok()? << (8 * byte);
            }
        }
        Some(value)
    }
    let count = reader.bits(8).ok()?;
    let mut components = Vec::new();
    for _ in 0..count {
        let index = reader.bits(8).ok()? as u8;
        let layout: &[Kind] = match index {
            26 | 123 => &[Kind::Id],
            29 => &[Kind::DbId, Kind::Text],
            44 => &[Kind::Entity; 3],
            51 => &HEALTH,
            75 => &[Kind::Id; 6],
            _ => return None,
        };
        reader.align();
        let mask_bytes = reader.bits(8).ok()? as usize;
        let mask: Vec<u64> = (0..mask_bytes)
            .map(|_| reader.bits(8))
            .collect::<Result<_, _>>()
            .ok()?;
        let fields_present = mask_bytes * 8;
        let mut fields = Vec::new();
        for field in 0..fields_present {
            if mask[field / 8] & (1 << (field % 8)) == 0 {
                fields.push(None);
                continue;
            }
            let value = match layout.get(field)? {
                Kind::Id => {
                    reader.align();
                    DecodedField::Id(byte_masked(&mut reader)?)
                }
                Kind::DbId => {
                    DecodedField::DbId(byte_masked(&mut reader)?, byte_masked(&mut reader)?)
                }
                Kind::Entity => DecodedField::Entity(reader.bits(32).ok()? as u32),
                Kind::Text => {
                    reader.align();
                    let length = reader.bits(32).ok()? as usize;
                    let bytes: Vec<u8> = (0..length)
                        .map(|_| reader.bits(8).map(|byte| byte as u8))
                        .collect::<Result<_, _>>()
                        .ok()?;
                    DecodedField::Text(String::from_utf8(bytes).ok()?)
                }
                Kind::Bytes(length) => DecodedField::Bytes(
                    (0..*length)
                        .map(|_| reader.bits(8).map(|byte| byte as u8))
                        .collect::<Result<_, _>>()
                        .ok()?,
                ),
            };
            fields.push(Some(value));
        }
        while fields.last() == Some(&None) {
            fields.pop();
        }
        components.push((index, fields));
    }
    Some(components)
}

/// The current amount of pool 0 in decoded components that start with the
/// pools (51): its bytes 8 to 12 (after its id and maximum).
fn first_pool(components: &[DecodedComponent]) -> Option<f32> {
    let (51, fields) = components.first()? else {
        return None;
    };
    let Some(DecodedField::Bytes(pool)) = fields.first()? else {
        return None;
    };
    Some(f32::from_le_bytes(pool.get(8..12)?.try_into().ok()?))
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

/// What a movement record says of the body's place and facing
/// (`orpheus_wire::entity::write_movement`): its frame (frames before the
/// packet's), discontinuity counter, aim pitch, Euler yaw (s16 angle units)
/// and position (1/1024 m).
#[derive(Clone, Copy, Debug, PartialEq)]
struct MovementHead {
    frame_back: u32,
    discontinuity: u32,
    pitch: u16,
    yaw: u16,
    position: [i32; 3],
}

/// Reads a movement record after its 12-bit length.
fn movement_head(mut reader: BitReader<'_>) -> MovementHead {
    // A present value: the first of `widths` its bit picks, else 32 bits.
    fn sized(reader: &mut BitReader<'_>, widths: &[usize]) -> u64 {
        for &width in widths {
            if reader.bit().unwrap() {
                return reader.bits(width).unwrap();
            }
        }
        reader.bits(32).unwrap()
    }
    fn optional(reader: &mut BitReader<'_>, widths: &[usize]) -> u64 {
        if reader.bit().unwrap() {
            sized(reader, widths)
        } else {
            0
        }
    }
    assert!(!reader.bit().unwrap(), "against the reference state");
    if reader.bit().unwrap() {
        for _ in 0..4 {
            if reader.bit().unwrap() {
                reader.bits(8).unwrap();
            }
        }
    }
    assert!(!reader.bit().unwrap(), "the 1/1024 grid");
    let frame_back = optional(&mut reader, &[4, 10]) as u32;
    let discontinuity = if reader.bit().unwrap() {
        assert!(!reader.bit().unwrap(), "no short width");
        reader.bits(32).unwrap() as u32
    } else {
        0
    };
    let pitch = optional(&mut reader, &[8, 10, 16]) as u16;
    optional(&mut reader, &[4, 10]); // ticks in the air
    assert!(!reader.bit().unwrap()); // no timed movement event
    optional(&mut reader, &[8, 10, 24]); // gravity scale
    assert!(!reader.bit().unwrap()); // move-speed scale 1
    let input_frame = reader.bit().unwrap();
    if input_frame && reader.bit().unwrap() {
        assert!(!reader.bit().unwrap());
        reader.bits(32).unwrap();
    }
    if reader.bit().unwrap() {
        assert!(!reader.bit().unwrap());
        reader.bits(32).unwrap(); // the last stand-up
    }
    for _ in 0..if input_frame { 4 } else { 2 } {
        if reader.bit().unwrap() {
            reader.bits(8).unwrap(); // throttles
        }
    }
    let yaw = optional(&mut reader, &[8, 10, 16]) as u16;
    optional(&mut reader, &[8, 10, 16]); // Euler pitch
    optional(&mut reader, &[8, 10, 16]); // and roll
    let position = std::array::from_fn(|_| {
        assert!(reader.bit().unwrap());
        // Every axis goes at the 24-bit width, signed.
        let value = sized(&mut reader, &[8, 10, 24]);
        ((value << 40) as i64 >> 40) as i32
    });
    MovementHead {
        frame_back,
        discontinuity,
        pitch,
        yaw,
        position,
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
                moved: None,
                skin: None,
                script: None,
                projectile_release: None,
                release_position: None,
                launches: Vec::new(),
                base_health: None,
                updated: None,
                components: None,
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
                record.release_position = Some(std::array::from_fn(|_| reader.f32().unwrap()));
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
                record.moved = Some(movement_head(reader));
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
                        if flags & orpheus_wire::entity::WITH_MOVEMENT == 0 {
                            use orpheus_wire::entity::{WITH_SCALE, WITH_TRANSFORM};
                            let mut components = reader;
                            if flags & WITH_TRANSFORM != 0 {
                                components.advance(7 * 32).unwrap();
                            }
                            if flags & (WITH_TRANSFORM | WITH_SCALE) != 0 {
                                components.advance(3 * 32).unwrap();
                            }
                            record.components = decode_components(components);
                        }
                    } else if operation == 2 {
                        record.components = decode_components(reader);
                        let mut components = reader;
                        let count = components.bits(8).unwrap();
                        let index = components.bits(8).unwrap() as u8;
                        if count == 1 {
                            record.updated = Some(index);
                        }
                        // Pool 0 of an update that starts with the pools.
                        if index == 51 {
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

/// What a client sends after a datagram, as the game does: its commands up
/// to the frame of the next tick (its command clock a frame ahead of the
/// packet frame), from `*next` on (more than one after frames it missed),
/// with this input. The server runs one a tick (`InputQueue`).
fn send_input(
    watcher: &mut Watcher,
    next: &mut u32,
    yaw: i16,
    pitch: i16,
    buttons: u32,
    forward: bool,
) {
    let newest = watcher.tick + 1;
    if *next > newest {
        return;
    }
    let count = (newest - *next + 1).min(u32::from(orpheus_wire::commands::MAX_COMMANDS));
    watcher.client.send(
        ACK,
        aimed_batch(*next, count as u8, yaw, pitch, buttons, forward),
    );
    *next += count;
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
    // A tick runs all but the last MAX_BUFFERED (4) of a long input buffer
    // at once (`InputQueue::release`): 36 commands give each peer 32 to run.
    let commands: Vec<_> = (first..first + 36)
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
        peer.world.release_input(1);
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
#[ignore = "requires a derived 104319 Practice Range scene"]
fn prepared_worker_delivers_helix_flight_damage_and_predicted_release_over_encrypted_udp() {
    exercise_prepared_world(PreparedFixture::Projectile);
}

#[derive(Clone, Copy)]
enum PreparedFixture {
    Stock,
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
        body_script::fixture_projectile_factory, bots::BOT_ENTITY, scene_assets::SceneAssets,
    };
    use orpheus_wire::jam::codec::id16;

    const CONTROLLER: u32 = 0xa000_0100;
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let schemas = Arc::new(Schemas::game());
    let lobby = Schemas::lobby();
    let assets = SceneAssets::load(runtime).unwrap();
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
            if let PreparedFixture::Projectile = fixture {
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
                let factory = fixture_projectile_factory(40.0);
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
    let mut client = Client::new(&server, &client_ticket);
    client.handshake();
    let load = client.receive();
    assert_eq!(messages(&schemas, &load)[0].message.id, 20300);
    assert!(client.window.accept(load.seq));
    let mut payload = BitWriter::new();
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    let loaded_at = client.now();
    client.send(ACK, payload.into_bytes());

    let mut ownership = false;
    while client.now() < loaded_at + Duration::from_millis(650) {
        let frame = client.receive();
        assert!(client.window.accept(frame.seq));
        assert!(world_records(&schemas, &frame).is_empty());
        ownership |= messages(&schemas, &frame)
            .iter()
            .any(|message| message.message.id == 20302);
    }
    assert!(ownership);
    client.send(ACK, Vec::new());

    let deadline = client.now() + Duration::from_secs(3);
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
        assert!(client.now() < deadline, "waiting for first world creates");
    };
    let mut creates = HashSet::new();
    // The creates of the datagrams the client acknowledged: a script may
    // follow only its own entity's acknowledged create.
    let mut acknowledged = HashSet::new();
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
        acknowledged.clone_from(&creates);
        client.send(ACK, Vec::new());
        assert!(client.now() < deadline, "waiting for baseline retransmit");
    }
    let mut mode = false;
    let mut controller = false;
    // The mode's and the controller's first script chunks.
    let first_script = |record: &WorldRecord, mode: &mut bool, controller: &mut bool| {
        let Some(script) = &record.script else {
            return;
        };
        assert_eq!((script.first, script.span), (0, 1));
        if record.entity == GAME_MODE_ENTITY {
            assert_eq!(script.data, [0x08, 0x5b, 0x06, 0x00, 0x0c]);
            *mode = true;
        } else {
            assert_eq!(record.entity, CONTROLLER);
            let mut bits = script.bits();
            assert_eq!(bits.bits(3).unwrap(), 2);
            assert!(bits.w_var().unwrap() >= orpheus_core::tick::FIRST_TICK);
            *controller = true;
        }
    };
    // The rest of the world follows, with no script whose create the
    // client hasn't acknowledged: 4 placeables, the player, the mode, the
    // 6 spawn door barriers, 15 bots and their 15 stand-in controllers.
    while creates.len() < 42 {
        let frame = client.receive();
        assert!(client.window.accept(frame.seq));
        let records = world_records(&schemas, &frame);
        assert!(
            records
                .iter()
                .all(|record| record.script.is_none() || acknowledged.contains(&record.entity))
        );
        for record in &records {
            first_script(record, &mut mode, &mut controller);
        }
        for record in records.iter().filter(|record| record.is_create()) {
            assert!(creates.insert(record.entity));
        }
        assert!(client.now() < deadline, "waiting for the rest of the world");
    }
    assert_eq!(creates.len(), 42);

    // Even after the retransmission, a script stays behind its create ACK.
    let withheld = client.receive();
    assert!(client.window.accept(withheld.seq));
    for record in world_records(&schemas, &withheld) {
        assert!(record.script.is_none() || acknowledged.contains(&record.entity));
        first_script(&record, &mut mode, &mut controller);
    }
    client.send(ACK, Vec::new());

    let mut moving_bot = false;
    while !(mode && controller && moving_bot) && client.now() < deadline {
        let frame = client.receive();
        assert!(client.window.accept(frame.seq));
        for record in world_records(&schemas, &frame) {
            assert!(!record.is_create());
            if record.movement {
                assert!((BOT_ENTITY..BOT_ENTITY + 15).contains(&record.entity));
                assert!(creates.contains(&record.entity));
                moving_bot = true;
            }
            first_script(&record, &mut mode, &mut controller);
        }
        client.send(ACK, Vec::new());
    }
    assert!(mode && controller && moving_bot);
    match fixture {
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
    let deadline = client.now() + Duration::from_secs(3);
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
        assert!(client.now() < deadline, "waiting for weapon fixture body");
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

    let deadline = client.now() + Duration::from_secs(3);
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
            client.now() < deadline,
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
        let deadline = client.now() + Duration::from_secs(2);
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
            assert!(client.now() < deadline, "waiting for the selected body");
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
        let deadline = client.now() + Duration::from_secs(2);
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
                client.now() < deadline,
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
    let deadline = client.now() + Duration::from_secs(2);
    let reopen_cmfd = loop {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        let records = world_records(schemas, &packet);
        // The owner's body may move by a correction in any datagram, now
        // that one goes every tick (see `await_input`); a movement-only
        // record of it is no part of the reopen.
        let records: Vec<_> = records
            .into_iter()
            .filter(|record| {
                !(record.entity == FIRST
                    && record.movement
                    && record.operation.is_none()
                    && record.script.is_none())
            })
            .collect();
        if !describe(&records).is_empty() {
            assert_eq!(describe(&records), [(OWNER, None, Some(3))]);
            break verify_controller(&records, SOLDIER, true);
        }
        assert!(
            client.now() < deadline,
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
        self.record(schemas, &packet);
    }

    /// Takes in a datagram the client received, and acknowledges it.
    fn record(&mut self, schemas: &Schemas, packet: &Packet) {
        if packet.kind != KIND_FRAME {
            return;
        }
        self.tick = tick_of(packet);
        let messages = messages(schemas, packet)
            .into_iter()
            .filter(|item| self.sequences.insert(item.sequence))
            .map(|item| item.message)
            .collect();
        let records = world_records(schemas, packet);
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
    let deadline = watchers[0].client.now() + within;
    while !done(watchers) {
        for watcher in watchers.iter_mut() {
            watcher.pump(schemas);
        }
        assert!(watchers[0].client.now() < deadline, "waiting for {what}");
    }
}

/// Two clients in one Practice Range see each other, in the order of
/// ow174/game/match.py: the other's card and controller entity, his team
/// list entry once the entity arrived, his body, his possession once the
/// body arrived, a movement record for his body in every datagram from the
/// one after its create, his body's statescript once the create arrived;
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
            let mut client = Client::new(&server, ticket);
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
    let deadline = server.now() + Duration::from_secs(2);
    while server.seat_exists(&tickets[1]).unwrap() {
        assert!(server.now() < deadline, "waiting for the match to end");
        server.step();
    }
    server.stop().unwrap();
}

/// Two players on different teams in one Practice Range (combat.py): the
/// first shoots the second to death. The shooter gets his hits (25001) and
/// the elimination (25005); the victim gets "YOU WERE KILLED BY" (25004)
/// naming the shooter's body; both clients get frames of the dead body's
/// statescript, which carry its "died" kill feed line (003E); the shooter's
/// Tab board changes; and the victim gets a new body after the Practice
/// Range's 3 s, which the shooter's client gets too, of the hero he picked
/// while dead. Each of his bodies turns his view to the spawn's facing. The
/// target's body is a hero that can be hit, and its pools of 0 come with
/// its dead bit.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_player_kills_another_who_respawns_after_the_practice_range_delay() {
    use orpheus_content::{
        heroes::{ANA, SOLDIER},
        parameters::PICK_HERO,
        stats::GMP_STATS,
    };
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::FIRE;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    // The target's spawn: team 1's first point.
    let ([spawn_x, _, spawn_z], facing) = scene.map().spawn_point(1, 0).unwrap();
    let spawn_yaw = (facing * 65536.0 / 360.0).round_ties_even() as i32 as u16;
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
            let mut client = Client::new(&server, ticket);
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

    // The shooter holds fire at the target's head height, level, from where
    // he stands: the Practice Range spawn is both teams', and the enemies'
    // capsules push them apart. Bodies: the shooter's, then the target's.
    let yaw = |server: &Lockstep| {
        let bodies = server.bodies(MATCH).unwrap();
        let [from, to] = [0, 1].map(|slot| bodies[slot].position.unwrap());
        let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
        (dx.atan2(dz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16
    };
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
    let deadline = server.now() + Duration::from_secs(20);
    while messages_of(target, 25004).is_empty() {
        assert!(server.now() < deadline, "waiting for the kill");
        if frame <= shooter.tick + 8 {
            shooter
                .client
                .send(ACK, command_batch(frame, 16, yaw(&server), FIRE, false));
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
    // The target's body is a hero that can be hit on team 1 (its filter
    // bits at its create, `orpheus_sim::scene` module notes), and each
    // client gets its pools of 0 with its dead bit.
    let filter = |components: &[DecodedComponent]| {
        components.iter().find(|(index, _)| *index == 26).cloned()
    };
    let created = shooter
        .find(0, |record| {
            record.entity == target_body && record.is_create()
        })
        .unwrap();
    let create = shooter.datagrams[created]
        .1
        .iter()
        .find(|record| record.entity == target_body && record.is_create())
        .unwrap();
    assert_eq!(
        create.components.as_deref().and_then(filter),
        Some((26, vec![Some(DecodedField::Id(0x0100_0041))]))
    );
    let dead = |watcher: &Watcher| {
        watcher
            .find(0, |record| {
                record.entity == target_body
                    && record.operation == Some(2)
                    && record.components.as_deref().and_then(first_pool) == Some(0.0)
                    && record.components.as_deref().and_then(filter)
                        == Some((26, vec![Some(DecodedField::Id(0x0100_0051))]))
            })
            .is_some()
    };
    pump_until(
        &mut [shooter, target],
        &schemas,
        "the dead body's filter bits",
        |watchers| watchers.iter().all(|watcher| dead(watcher)),
    );
    // While dead, the target picks Ana: she takes his respawn.
    send_player_message(
        &mut target.client,
        &schemas,
        target_owner,
        2,
        PICK_HERO,
        ANA.to_le_bytes().to_vec(),
    );
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
    // death path now runs its array inserts (graph 01CA nodes 118 and 121,
    // `STU_4C3509FB`), then reaches a config the VM doesn't compute (graph
    // 01D8 node 98, `STU_309B0570`, a velocity), which debug builds refuse
    // and release builds read as null as Python does; so this part runs in
    // release builds.
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
    let deadline = server.now() + Duration::from_secs(10);
    while tolerant && !(owner_frames(target) && observer_frames(shooter)) {
        assert!(
            server.now() < deadline,
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
    // The dead body's frames raise its dead flag (entity variable #478),
    // which hides its health bar and name tag from the others (0D24's
    // #17549 reads `!#478e`); the shooter's observer frames carry it.
    if tolerant {
        let mut dead_flag = BitWriter::new();
        dead_flag.w_var(orpheus_script::bot::DEAD_FLAG);
        orpheus_script::wire::write_value(
            &mut dead_flag,
            &orpheus_script::wire::Value::Bool(true),
            orpheus_script::wire::ValueEncoding::Compact,
        )
        .unwrap();
        dead_flag.bit(false); // no stack binding
        let raised = shooter.find(0, |record| {
            record.entity == target_body
                && record.script.as_ref().is_some_and(|chunk| {
                    orpheus_testkit::contains_bits(
                        &chunk.data,
                        chunk.width,
                        dead_flag.as_bytes(),
                        dead_flag.bit_len(),
                    )
                })
        });
        assert!(
            raised.is_some_and(|index| index >= fired),
            "the dead body's observer frames carry its dead flag"
        );
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
    // combat.py `PRACTICE_RESPAWN`: 3 s. The killing command runs from the
    // shooter's input buffer as tick T is due, at T's time, before the tick
    // (`InputQueue`): the death notice goes in that tick's datagram, T. The
    // respawn is due at the first tick 3 s after the death, T + 188
    // (3000 / 16 = 187.5), and the new body goes in that tick's datagram,
    // T + 188: 188 ticks after the death notice. (Before the input buffer
    // the command ran on arrival, at the time of the tick before: 187.)
    let waited = respawn_tick - death_tick;
    assert_eq!(waited, 188, "respawned after {waited} ticks");
    assert!(
        target
            .find(died, |record| record.entity == target_body
                && record.operation == Some(3))
            .is_some()
    );
    // Each body placed at the spawn point turns its player's view to the
    // spawn's facing (`SpawnTurn` in `orpheus_sim::replication`): the create
    // and its own movement record start the body's discontinuity counter at
    // 0; the next owner movement record, once the create arrived, is the
    // pose it was made with (the spawn point, the spawn's yaw, a level aim)
    // at the frame it was made, with a new counter; the first body's as the
    // respawned one's. The hero picked while dead (Ana) is the respawned
    // body's.
    let head = |watcher: &Watcher, index: usize, body: u32| {
        watcher.datagrams[index]
            .1
            .iter()
            .find_map(|record| record.moved.filter(|_| record.entity == body))
    };
    let turn = |watcher: &Watcher, body: u32| {
        let create = watcher.created(body).unwrap();
        assert_eq!(head(watcher, create, body).unwrap().discontinuity, 0);
        let index = watcher
            .find(create + 1, |record| {
                record.entity == body && record.moved.is_some()
            })
            .unwrap();
        (index, head(watcher, index, body).unwrap())
    };
    pump_until(
        &mut [shooter, target],
        &schemas,
        "the spawn turn",
        |watchers| {
            watchers[1]
                .find(created + 1, |record| {
                    record.entity == respawned && record.moved.is_some()
                })
                .is_some()
        },
    );
    for (body, made_at) in [(target_body, None), (respawned, Some(created))] {
        let (index, head) = turn(target, body);
        assert_eq!(head.discontinuity, 1);
        assert_eq!((head.yaw, head.pitch), (spawn_yaw, 0));
        let [x, _, z] = head.position.map(|axis| f64::from(axis) / 1024.0);
        assert!((x - spawn_x).abs() < 0.002 && (z - spawn_z).abs() < 0.002);
        if let Some(created) = made_at {
            let frame = target.ticks[index] - head.frame_back;
            assert!(frame <= target.ticks[created], "made at {frame}");
        }
    }
    let definition = |body: u32| {
        target.datagrams[target.created(body).unwrap()]
            .1
            .iter()
            .find(|record| record.entity == body && record.is_create())
            .unwrap()
            .definition
    };
    assert_ne!(definition(respawned), definition(target_body), "Ana's body");
    // Whatever owner records of Ana's body follow her turn (her
    // corrections) carry its counter; the wait and resumption of the
    // corrections themselves is `spawn_turn_goes_after_the_create_again_
    // when_lost_and_corrections_carry_its_counter` in `orpheus_sim`.
    let (turned, _) = turn(target, respawned);
    let mut frame = target.tick + 1;
    let end = server.now() + Duration::from_secs(2);
    while server.now() < end {
        if frame <= target.tick + 8 {
            target
                .client
                .send(ACK, command_batch(frame, 16, 0, 0, false));
            frame += 16;
        }
        shooter.pump(&schemas);
        target.pump(&schemas);
    }
    assert!(
        target.datagrams[turned + 1..]
            .iter()
            .flat_map(|(_, records)| records)
            .filter_map(|record| record.moved.filter(|_| record.entity == respawned))
            .all(|head| head.discontinuity == 1)
    );
    server.stop().unwrap();
}

/// The Practice Range's spawn room (`orpheus_sim::map_areas`): a player
/// who walks out of it toward the bots gets his player entity's component
/// 74 (field 6, the spawn room flag the client checks before H opens hero
/// select) updated once, and no more while he stays out.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn walking_out_of_the_spawn_room_clears_the_hero_select_flag() {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use orpheus_sim::scene_assets::SceneAssets;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let facing = scene.map().spawn_point(0, 0).unwrap().1;
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let tag = "walker#1111";
    let members = vec![WorldPlayer {
        player: MatchPlayer {
            name: tag.into(),
            team: 0,
            tournament: false,
            ranked: false,
        },
        card: PlayerCard {
            account: [7, 0x0100_0000_0000_0000],
            battle_tag: tag.into(),
            record: Fields::from([(0x40, Value::String(tag.into()))]),
            ..PlayerCard::default()
        },
        hero: 0,
    }];
    let tickets = server.open_prepared(MATCH, scene, members).unwrap();
    let mut client = Client::new(&server, &tickets[0]);
    client.handshake();
    let mut walker = Watcher {
        client,
        owner: player_entity(0),
        tick: 0,
        sequences: HashSet::new(),
        datagrams: Vec::new(),
        ticks: Vec::new(),
    };
    let owner = walker.owner;
    pump_until(&mut [&mut walker], &schemas, "the instance", |watchers| {
        watchers[0].datagrams.iter().any(|(messages, _)| {
            messages.iter().any(|message| {
                message.id == 20300 && message.fields[&0x80] != Value::Struct(Fields::new())
            })
        })
    });
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    walker.client.send(ACK, payload.into_bytes());
    pump_until(&mut [&mut walker], &schemas, "the controller", |watchers| {
        watchers[0]
            .find(0, |record| {
                record.entity == owner && record.script.is_some()
            })
            .is_some()
    });
    send_player_message(
        &mut walker.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    pump_until(&mut [&mut walker], &schemas, "the body", |watchers| {
        watchers[0].created(owner | 1).is_some()
    });
    let spawned = walker.datagrams.len();
    let flag = |watcher: &Watcher| {
        watcher.find(spawned, |record| {
            record.entity == owner && record.updated == Some(74)
        })
    };
    // He runs the way the spawn point faces, out of the room.
    let yaw = (facing * 65536.0 / 360.0).round() as i32 as i16;
    let mut frame = walker.tick + 1;
    let deadline = server.now() + Duration::from_secs(30);
    while flag(&walker).is_none() {
        assert!(server.now() < deadline, "waiting for the flag");
        if frame <= walker.tick + 8 {
            walker
                .client
                .send(ACK, command_batch(frame, 16, yaw, 0, true));
            frame += 16;
        }
        walker.pump(&schemas);
    }
    let cleared = flag(&walker).unwrap();
    for _ in 0..60 {
        if frame <= walker.tick + 8 {
            walker
                .client
                .send(ACK, command_batch(frame, 16, yaw, 0, true));
            frame += 16;
        }
        walker.pump(&schemas);
    }
    assert!(
        walker
            .find(cleared + 1, |record| record.entity == owner
                && record.updated == Some(74))
            .is_none(),
        "one update while he stays out"
    );
    server.stop().unwrap();
}

/// Players (BattleTag, team, hero name) in one prepared Practice Range, once
/// each client has every other body, its possession and, for a body that
/// runs a statescript (Soldier: 76's), its frames. Also where each spawned.
fn practice_players<const N: usize>(
    players: [(&str, u8, &str); N],
) -> (Lockstep, Arc<Schemas>, [Watcher; N], [[f64; 3]; N]) {
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
) -> (Lockstep, Arc<Schemas>, [Watcher; N], [[f64; 3]; N]) {
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
    let server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
            let mut client = Client::new(&server, ticket);
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

/// Team 0's first player (slot 0 of the room) runs out of his spawn room
/// the way the first spawn point faces, as a player leaves it: retail
/// spawn rooms keep him invulnerable (`orpheus_sim::map_areas`). He runs
/// until the server's spawn room flag cleared and his feet are `there`,
/// then `beyond` more frames, then stands still for a second. Returns his
/// next command frame.
fn run_out_of_the_spawn_room(
    server: &Lockstep,
    watcher: &mut Watcher,
    schemas: &Schemas,
    there: impl Fn([f64; 3]) -> bool,
    beyond: u32,
) -> u32 {
    let facing = GameMap::practice_range().spawn_point(0, 0).unwrap().1;
    let yaw = (facing * 65536.0 / 360.0).round() as i32 as i16;
    let out = |server: &Lockstep| server.bodies(MATCH).unwrap()[0].spawn_room == Some(false);
    let arrived = |server: &Lockstep| {
        let body = server.bodies(MATCH).unwrap().remove(0);
        body.spawn_room == Some(false) && body.position.is_some_and(&there)
    };
    assert_eq!(server.bodies(MATCH).unwrap()[0].spawn_room, Some(true));
    let mut frame = watcher.tick + 1;
    let deadline = server.now() + Duration::from_secs(30);
    let mut stop = None;
    loop {
        assert!(server.now() < deadline, "running out of the spawn room");
        if stop.is_none() && arrived(server) {
            stop = Some(frame + beyond);
        }
        if stop.is_some_and(|stop| watcher.tick > stop + 64) {
            break;
        }
        let running = stop.is_none_or(|stop| frame < stop);
        send_input(watcher, &mut frame, yaw, 0, 0, running);
        watcher.pump(schemas);
    }
    assert!(out(server), "stopped back in the spawn room");
    frame
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
    let deadline = watchers[0].client.now() + Duration::from_secs(15);
    while !done(watchers) {
        assert!(watchers[0].client.now() < deadline, "waiting for {what}");
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

/// The client's hero settings (21604) reach his body in the world and his
/// owner frames: with ALLIED HEALTH BARS at ALWAYS, Soldier: 76's body graph
/// `01CF` gives him the support tag (st117) that `0D24` asks of the viewer
/// for an ally's health bar, and the client acknowledges a frame with st117
/// and `#8336e`; back at DEFAULT the client sends an empty list (seen in the
/// game) and the tag and the variable go, in his frames too.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_clients_allied_health_bars_setting_reaches_his_body() {
    use orpheus_content::hero_settings::{
        ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS, HERO_SETTINGS,
    };
    use orpheus_script::wire::Value as ScriptValue;

    let (mut server, schemas, [mut soldier], _) =
        practice_players([("soldier#1111", 0, "Soldier: 76")]);
    assert!(!server.bodies(MATCH).unwrap()[0].support_tag_given);
    // practice_world sent records 0 (map loaded) and 1 (the pick).
    let send = |watcher: &mut Watcher, ordinal: u32, entries: Vec<Value>| {
        let settings = Fields::from([
            (0x78, Value::Array(entries)),
            (0x90, Value::Bool(false)),
            (0x91, Value::Bool(false)),
        ]);
        let message = encode(&schemas, HERO_SETTINGS, &settings).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(ordinal, message)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    };
    // His body's script runs, and its owner frames go out, on his commands.
    let mut next = soldier.tick + 1;
    let mut wait_for = |watcher: &mut Watcher, given: bool, variable: ScriptValue| {
        let deadline = server.now() + Duration::from_secs(15);
        loop {
            let body = server.bodies(MATCH).unwrap().remove(0);
            if body.support_tag_given == given
                && body.support_tag_delivered == (given, variable.clone())
            {
                break;
            }
            assert!(server.now() < deadline, "the tag is not {given}: {body:?}");
            if next <= watcher.tick + 8 {
                watcher
                    .client
                    .send(ACK, command_batch(next, 16, 0, 0, false));
                next += 16;
            }
            watcher.pump(&schemas);
        }
    };
    let always = Value::Struct(Fields::from([
        (0, Value::Unsigned(0)),
        (8, Value::Unsigned(ALLIED_HEALTH_BARS)),
        (0x10, Value::Unsigned(ALLIED_HEALTH_BARS_ALWAYS)),
    ]));
    send(&mut soldier, 2, vec![always]);
    wait_for(
        &mut soldier,
        true,
        ScriptValue::Asset(ALLIED_HEALTH_BARS_ALWAYS),
    );
    send(&mut soldier, 3, Vec::new());
    wait_for(&mut soldier, false, ScriptValue::Null);
    server.stop().unwrap();
}

/// Whether a body script frame ends with a remote message group of one short
/// entry for `node` (`bits` wide) of any instance, then no more entries,
/// groups or ungrouped entries (104319 A0CCA0; `SnapshotEncoder` frame
/// actions).
fn ends_with_frame_action(chunk: &ReceivedChunk, node: u16, bits: u8) -> bool {
    let bit = |data: &[u8], index: usize| data[index / 8] >> (index % 8) & 1;
    (1..=512).any(|instance| {
        let mut entry = BitWriter::new();
        entry.bit(true);
        entry.w_u16(instance);
        entry.bits(u64::from(node), usize::from(bits)).unwrap();
        let mut tail = BitWriter::new();
        tail.bit(true);
        tail.bit(false);
        tail.bits(8 + entry.bit_len() as u64, 8).unwrap();
        tail.append(&entry);
        tail.bits(0, 3).unwrap();
        let length = tail.bit_len();
        chunk.width >= length
            && (0..length).all(|index| {
                bit(&chunk.data, chunk.width - length + index) == bit(tail.as_bytes(), index)
            })
    })
}

/// A player's communication wheel choice (21608, Need Healing) reaches his
/// body as `02A3`; `05AD` plays `0686`, whose effect 40 goes to his teammate
/// in his body's observer frames (remote index 8) and to his own client in
/// its owner frames (raw index 36), each as the frame's remote message
/// (`orpheus_sim::wheel`).
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_need_healing_callout_reaches_a_teammate() {
    let graphs = orpheus_script::graph::GraphStore::load_runtime(
        orpheus_testkit::runtime_dir().join("data"),
        "game_graphs_174.json.gz",
    )
    .unwrap();
    let callouts = graphs.graph(0x0686).unwrap();
    let effect = callouts.node(40).unwrap();
    assert_eq!((effect.remote, effect.rid), (8, 36));
    let (remote_bits, raw_bits) = (callouts.header.remote_bits, callouts.header.nodes_bits);
    let (mut server, schemas, [mut speaker, mut teammate], _) = practice_players([
        ("speaker#1111", 0, "Soldier: 76"),
        ("teammate#2222", 0, "Soldier: 76"),
    ]);
    let body = speaker.owner | 1;
    let (speaker_from, teammate_from) = (speaker.datagrams.len(), teammate.datagrams.len());
    // practice_world sent records 0 (map loaded) and 1 (the pick).
    let choice = Fields::from([
        (0x78, Value::Unsigned(0x0a90_0000_0000_016c)),
        (0x80, Value::Bool(false)),
        (0x84, Value::Struct(Fields::from([(0, Value::Unsigned(0))]))),
    ]);
    let message = encode(&schemas, orpheus_sim::wheel::COMMUNICATION_ACTION, &choice).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(2, message)], &[]).unwrap();
    speaker.client.send(ACK, payload.into_bytes());
    let carries = |watcher: &Watcher, from: usize, node: u16, bits: u8| {
        watcher
            .find(from, |record| {
                record.entity == body
                    && record
                        .script
                        .as_ref()
                        .is_some_and(|chunk| ends_with_frame_action(chunk, node, bits))
            })
            .is_some()
    };
    let mut next = speaker.tick + 1;
    let mut all = [&mut speaker, &mut teammate];
    pump_commanding(
        &mut all,
        &schemas,
        "the callout",
        0,
        &mut next,
        |watchers| {
            carries(watchers[1], teammate_from, 8, remote_bits)
                && carries(watchers[0], speaker_from, 36, raw_bits)
        },
    );
    server.stop().unwrap();
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

/// Beyond Python (which never hurt or pushed the shooter;
/// `ProjectileFlight::own_blast`, `Mover::push`): a Helix Rocket fired at
/// the floor just ahead of Soldier: 76's feet (70° down) hurts and lifts
/// him as graph `0254` node 35 says: 80 × 1 (within 1 m) × the self-damage
/// scale 0.5 = 40 of his 200, and within the 0.5 m knockback ring 4 m/s
/// along the line from the burst to his centre (up and back) plus 6 m/s
/// up. His server mover flies it as a jump: more than 2 m up, at most
/// (4 + 6)² / 2g = 100/35 m, and he lands behind where he stood, away from
/// the burst. He gets no hit notice (25001) for it. He fires outside his
/// spawn room, where he is not invulnerable.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_helix_at_soldiers_feet_hurts_him_and_lifts_him() {
    use orpheus_wire::commands::SECONDARY;
    /// 70° down (1/65536 turns, positive down).
    const DOWN: i16 = 12743;

    let (mut server, schemas, [mut soldier], _) =
        practice_players([("soldier#1111", 0, "Soldier: 76")]);
    let mut next = run_out_of_the_spawn_room(&server, &mut soldier, &schemas, |_| true, 32);
    let start = server.bodies(MATCH).unwrap().remove(0);
    assert_eq!(start.health, Some(1.0));
    let [_, floor, start_z] = start.position.unwrap();
    let fired = soldier.datagrams.len();
    // Yaw 0 faces +z.
    soldier
        .client
        .send(ACK, aimed_batch(next, 1, 0, DOWN, SECONDARY, false));
    next += 1;
    let mut top = floor;
    let deadline = server.now() + Duration::from_secs(15);
    let landed = loop {
        let body = server.bodies(MATCH).unwrap().remove(0);
        let height = body.position.unwrap()[1];
        top = top.max(height);
        if top - floor > 0.5 && (height - floor).abs() < 0.05 {
            break body;
        }
        assert!(
            server.now() < deadline,
            "waiting for the rocket jump: {:.3} m up at most, health {:?}",
            top - floor,
            body.health
        );
        send_input(&mut soldier, &mut next, 0, 0, 0, false);
        soldier.pump(&schemas);
    };
    assert!(
        top - floor > 2.0 && top - floor <= 100.0 / 35.0 + 0.05,
        "{} m up",
        top - floor
    );
    let back = start_z - landed.position.unwrap()[2];
    assert!(back > 0.3, "landed {back} m back");
    assert_eq!(landed.health, Some(0.8));
    assert!(
        soldier.datagrams[fired..]
            .iter()
            .all(|(messages, _)| messages.iter().all(|message| message.id != 25001))
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
    let charged = server.now() + Duration::from_secs(2);
    while server.now() < charged {
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
    let deadline = server.now() + Duration::from_secs(10);
    while messages_of(&tracer, 25005).is_empty()
        || !released(&viewer, &seen)
        || !released(&tracer, &bomb)
    {
        assert!(
            server.now() < deadline,
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
    // STICK_FUSE: 1 s, 62.5 ticks of 16 ms: the bomb explodes at the
    // first tick 1 s after it stuck, 63 ticks later.
    let fuse = hits[1].0 - hits[0].0;
    assert_eq!(fuse, 63, "fuse of {fuse} ticks");
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

/// The owner's Helix Rocket explodes on his screen soon after it hit. His
/// client holds the explosion until the destroy with the release state
/// arrives (072C: `m_delayCollisionEffectsBasedOnLatency`), so that destroy
/// must follow the server's impact (25001's impact frame `0x84`, the server
/// tick of the hit) closely. The client leads the packet frame by `lead`
/// frames, as the real client does (`correction.rs`: at most 16), and runs
/// a command a datagram, as the server sends one every tick.
///
/// The timeline of a shot, from the server tick T of the datagram after
/// which the client sent the shot's command (the client simulates that
/// frame then): the server's input buffer holds the client's next command
/// in hand (`InputQueue`, one tick), so the command runs as tick T + 2 is
/// due, before it; its first flight step flies a whole tick, so its rocket
/// leaves at T + 1, a tick after the client's; the confirmation goes in the
/// datagram of the tick that queued it, T + 2; the rocket hits the bot 8 m
/// ahead (7.6 m to its shapes, 9.1 ticks at 50 m/s) in tick T + 11, whatever
/// the lead, and the release goes in that tick's own datagram, T + 11, as
/// the confirmation's ACK came back before T + 3. So the server adds a tick
/// and less than another to the client's own flight (the buffer's tick, as
/// retail's one buffered command), and the rest is the trip to the client.
/// Before the buffer the command ran on arrival, before T + 1, and each of
/// these came a tick sooner. Before that, the release waited for the
/// shot's command frame plus the flight plus 6, compared with server
/// ticks: `lead` + 6 ticks after the impact at least.
///
/// The second shot, 16 frames ahead, loses the datagram with its
/// confirmation (T + 2): the ACK sent after T + 3's datagram shows it lost,
/// so the create goes again at T + 4, arrives before the impact, and the
/// release still goes at T + 11 with its explosion. The client's lead
/// grows from 8 to 16 frames in one burst of 8 commands: the buffer runs
/// all but 4 at once and drains back to one waiting command in 3 × 20
/// ticks, long before the shot. Each release carries
/// the server's impact point, on the bot, where an explosion the client's
/// rocket hasn't had yet plays (before, out of sight at y −1000). Each shot
/// has a bot of its own, 8 m out, the second 30° round from the first
/// (4 m from it, beyond the blast's 3 m): the first shot's explosion throws
/// its bot back.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn the_owners_helix_release_follows_its_impact_with_a_leading_client() {
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::SECONDARY;
    const HELIX: u32 = 0x072c;
    const AHEAD: f64 = 8.0;
    const APART: f64 = 30.0;

    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    // Confirmation, its resend and release after the shot's tick T; the
    // derivations are in the notes above.
    let (confirm_at, resend_at, release_at) = (2, 4, 11);
    let mut aims = Vec::new();
    let (mut server, schemas, [mut soldier], _) =
        practice_world([("soldier#1111", 0, "Soldier: 76")], &assets, |scene| {
            // Two hostile training bots that neither fire nor walk, ahead
            // of his spawn: along its facing and 30° round from it, the
            // second turned 30° from the first's facing too, so each
            // shot meets its bot alike.
            let ([x, y, z], facing) = scene.map().spawn_point(0, 0).unwrap();
            let mut bots = scene.bots_mut().iter_mut().filter(|bot| {
                bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty()
            });
            let mut turned = None;
            for (turn, yaw) in [(0.0, facing), (APART, facing + APART)] {
                let bot = bots.next().unwrap();
                let first = *turned.get_or_insert(bot.yaw);
                bot.yaw = (first + turn * 65536.0 / 360.0 + 32768.0).rem_euclid(65536.0) - 32768.0;
                let (sin, cos) = yaw.to_radians().sin_cos();
                bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
                aims.push((
                    bot.entity,
                    bot.position,
                    (yaw * 65536.0 / 360.0).round() as i32 as i16,
                ));
            }
        });
    let leased = |entity: u32| (0xa010_0000..0xc000_0000).contains(&entity);
    let hit = |bot: u32, fields: &Fields| {
        let Value::Struct(impact) = &fields[&0x78] else {
            return false;
        };
        matches!(&impact[&0], Value::Struct(target)
            if target[&0] == Value::Unsigned(u64::from(bot)))
    };
    let mut next = soldier.tick + 1;
    // The client's command frames run `lead` ahead of the last packet frame.
    // The second shot comes after Helix's 6 s cooldown, and its client
    // misses the datagram with the confirmation.
    for ((lead, lose_confirmation), (bot, at, yaw)) in
        [(8, false), (16, true)].into_iter().zip(aims)
    {
        let hit_bot = |fields: &Fields| hit(bot, fields);
        let fire = soldier.tick + lead + 60;
        let from = soldier.datagrams.len();
        let mut sent = None;
        let mut lost = None;
        let deadline = server.now() + Duration::from_secs(10);
        let (impact, confirmed, release) = loop {
            assert!(server.now() < deadline, "waiting for the Helix release");
            while next <= soldier.tick + lead {
                let buttons = if next == fire { SECONDARY } else { 0 };
                if next == fire {
                    sent = Some(soldier.tick);
                }
                soldier
                    .client
                    .send(ACK, command_batch(next, 1, yaw, buttons, false));
                next += 1;
            }
            let packet = soldier.client.receive();
            if lose_confirmation
                && lost.is_none()
                && world_records(&schemas, &packet)
                    .iter()
                    .any(|record| leased(record.entity) && record.definition == Some(HELIX))
            {
                lost = Some(tick_of(&packet));
                continue;
            }
            soldier.record(&schemas, &packet);
            let impact = soldier.datagrams[from..].iter().find_map(|(messages, _)| {
                messages
                    .iter()
                    .find(|message| message.id == 25001 && hit_bot(&message.fields))
                    .map(|message| message.fields[&0x84].clone())
            });
            let confirmed = soldier.find(from, |record| {
                leased(record.entity) && record.definition == Some(HELIX)
            });
            let release = confirmed.and_then(|confirmed| {
                soldier.find(confirmed, |record| {
                    leased(record.entity)
                        && record.operation == Some(3)
                        && record.projectile_release.is_some()
                })
            });
            if let (Some(impact), Some(confirmed), Some(release)) = (impact, confirmed, release) {
                break (impact, confirmed, release);
            }
        };
        let Value::Unsigned(impact) = impact else {
            panic!("impact frame")
        };
        let sent = sent.unwrap();
        let position = soldier.datagrams[release]
            .1
            .iter()
            .find_map(|record| record.release_position)
            .unwrap();
        let resent = lost.map(|lost| (lost, soldier.ticks[confirmed]));
        assert_eq!(
            (
                lost.is_none().then(|| soldier.ticks[confirmed]),
                resent,
                u32::try_from(impact).unwrap(),
                soldier.ticks[release]
            ),
            (
                (!lose_confirmation).then_some(sent + confirm_at),
                lose_confirmation.then_some((sent + confirm_at, sent + resend_at)),
                sent + release_at,
                sent + release_at
            ),
            "lead {lead}: the command sent after tick {sent}"
        );
        // On the bot's shapes, as the rocket's centre meets them: its
        // 0.2 m radius beyond them (within 0.6 m of the bot's axis, below
        // its 2.6 m top).
        let [x, y, z] = position.map(f64::from);
        assert!(
            (x - at[0]).hypot(z - at[2]) < 0.6 + 0.2 && (at[1]..at[1] + 2.6 + 0.2).contains(&y),
            "released at {position:?}, the bot at {at:?}"
        );
        // Helix Rockets' 6 s cooldown.
        let ready = server.now() + Duration::from_millis(6500);
        while server.now() < ready {
            while next <= soldier.tick + lead {
                soldier
                    .client
                    .send(ACK, command_batch(next, 1, yaw, 0, false));
                next += 1;
            }
            soldier.pump(&schemas);
        }
    }
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
    let ready = server.now() + Duration::from_secs(1);
    while server.now() < ready {
        command(&mut genji, 0);
        genji.pump(&schemas);
        viewer.pump(&schemas);
    }
    let fired = (genji.datagrams.len(), viewer.datagrams.len());
    command(&mut genji, SECONDARY);
    let deadline = server.now() + Duration::from_secs(5);
    while hits_of(&genji, fired.0).is_empty() {
        assert!(server.now() < deadline, "waiting for the fan's hit");
        command(&mut genji, 0);
        genji.pump(&schemas);
        viewer.pump(&schemas);
    }
    // The other two shuriken of the shot never fly.
    let settled = server.now() + Duration::from_secs(1);
    while server.now() < settled {
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
    let ready = server.now() + Duration::from_secs(1);
    while server.now() < ready {
        command(&mut doomfist, 0);
        doomfist.pump(&schemas);
        viewer.pump(&schemas);
    }
    let fired = (doomfist.datagrams.len(), viewer.datagrams.len());
    command(&mut doomfist, FIRE);
    let deadline = server.now() + Duration::from_secs(5);
    while hits_of(&doomfist, fired.0).len() < 11 {
        assert!(
            server.now() < deadline,
            "waiting for the pellets: {:?}",
            hits_of(&doomfist, fired.0)
        );
        command(&mut doomfist, 0);
        doomfist.pump(&schemas);
        viewer.pump(&schemas);
    }
    let settled = server.now() + Duration::from_millis(500);
    while server.now() < settled {
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
    let ready = server.now() + Duration::from_secs(1);
    while server.now() < ready {
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
    let deadline = server.now() + Duration::from_secs(5);
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
            server.now() < deadline,
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

/// Ana's Biotic Rifle (graph `0B94`) over the wire, after leafall903/ORPHEUS
/// (MIT) `test_game_ana.py` (c239a84): a Friendly Bot below full health
/// heals 70 a dart (to full at most), with no 25001 to Ana; an enemy takes
/// the dart's 70 with her 25001. An enemy Soldier: 76 wounds the bot: he
/// walks ahead of the spawn both share and fires at it, 8 m out along their
/// facing. They share the practice spawn, and as enemies their capsules
/// push them apart (Ana about 1 m sideways), so Ana aims from where she
/// then stands: at him until a dart strikes him (the enemy), then at the
/// bot. Ana's client
/// gets her own body's state, as Soldier: 76's does (owner corrections,
/// `replication.rs` `CORRECTED_HEROES`).
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn anas_rifle_heals_a_hurt_friendly_bot_and_damages_an_enemy() {
    use orpheus_script::vm::Unknowns;
    use orpheus_sim::scene_assets::SceneAssets;
    use orpheus_wire::commands::FIRE;
    const AHEAD: f64 = 8.0;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    let mut aim = None;
    let (mut server, schemas, [mut ana, mut soldier], _) = practice_world(
        [("ana#1111", 0, "Ana"), ("soldier#2222", 1, "Soldier: 76")],
        &assets,
        |scene| {
            // A Friendly Bot (Ana's team) that neither fires nor walks,
            // ahead of the spawn along its facing.
            let ([x, y, z], yaw) = scene.map().spawn_point(0, 0).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| bot.spawn.team == 0 && !bot.spawn.firing && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
            aim = Some((
                bot.entity,
                bot.position,
                bot.spawn.health,
                (yaw * 65536.0 / 360.0).round() as i32 as i16,
            ));
        },
    );
    let (bot, bot_at, full, yaw) = aim.unwrap();
    let bot_health = |watcher: &Watcher| {
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| record.entity == bot)
            .filter_map(|record| record.base_health)
            .next_back()
    };
    // Ana's 25001s from `from` on: whom each struck and for how much.
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
    /// One player's commands from frame `next` on: an idle batch keeps his
    /// script running, a press goes alone.
    fn command(watcher: &mut Watcher, next: &mut u32, yaw: i16, buttons: u32) {
        let count = if buttons == 0 { 16 } else { 1 };
        if buttons != 0 || *next <= watcher.tick + 8 {
            watcher
                .client
                .send(ACK, command_batch(*next, count, yaw, buttons, false));
            *next += u32::from(count);
        }
    }
    /// Both idle for `seconds`.
    fn settle(
        watchers: [&mut Watcher; 2],
        frames: &mut [u32; 2],
        yaw: i16,
        schemas: &Schemas,
        seconds: f64,
    ) {
        let [first, second] = watchers;
        let until = first.client.now() + Duration::from_secs_f64(seconds);
        while first.client.now() < until {
            command(first, &mut frames[0], yaw, 0);
            command(second, &mut frames[1], yaw, 0);
            first.pump(schemas);
            second.pump(schemas);
        }
    }
    let full = full as f32;
    let mut frames = [ana.tick + 1, soldier.tick + 1];
    settle([&mut ana, &mut soldier], &mut frames, yaw, &schemas, 1.0);
    // No health update for the bot yet: it is at full health.
    assert_eq!(bot_health(&ana), None);

    // The Soldier walks about 2 m ahead (25 ticks), then fires at the bot.
    soldier
        .client
        .send(ACK, command_batch(frames[1], 25, yaw, 0, true));
    frames[1] += 25;
    settle([&mut ana, &mut soldier], &mut frames, yaw, &schemas, 0.5);
    let deadline = server.now() + Duration::from_secs(10);
    while !bot_health(&ana).is_some_and(|health| health < full) {
        assert!(server.now() < deadline, "waiting for the bot's wound");
        command(&mut soldier, &mut frames[1], yaw, FIRE);
        command(&mut ana, &mut frames[0], yaw, 0);
        ana.pump(&schemas);
        soldier.pump(&schemas);
    }
    settle([&mut ana, &mut soldier], &mut frames, yaw, &schemas, 0.5);
    let wounded = bot_health(&ana).unwrap();
    assert!(wounded < full, "wounded to {wounded}");

    // Ana fires from where the push left her: at the Soldier until a dart
    // strikes him, then at the bot. Bodies: Ana's, then the Soldier's.
    let fired = ana.datagrams.len();
    let soldier_body = Value::Unsigned(u64::from(soldier.owner | 1));
    let facing = |from: [f64; 3], to: [f64; 3]| {
        let degrees = (to[0] - from[0]).atan2(to[2] - from[2]).to_degrees();
        (degrees * 65536.0 / 360.0).round() as i32 as i16
    };
    let deadline = server.now() + Duration::from_secs(20);
    while !bot_health(&ana).is_some_and(|health| health > wounded) {
        assert!(
            server.now() < deadline,
            "waiting for the heal: hits {:?}",
            hits_of(&ana, fired)
        );
        let bodies = server.bodies(MATCH).unwrap();
        let ana_at = bodies[0].position.unwrap();
        let target = if hits_of(&ana, fired).is_empty() {
            bodies[1].position.unwrap()
        } else {
            bot_at
        };
        let aim = facing(ana_at, target);
        command(&mut ana, &mut frames[0], aim, FIRE);
        settle([&mut ana, &mut soldier], &mut frames, aim, &schemas, 1.0);
    }
    assert_eq!(bot_health(&ana), Some((wounded + 70.0).min(full)));
    let hits = hits_of(&ana, fired);
    assert!(
        hits.contains(&(soldier_body.clone(), Value::Unsigned(70))),
        "the enemy takes 70: {hits:?}"
    );
    assert!(
        hits.iter().all(|(target, _)| *target == soldier_body),
        "no 25001 for the heal: {hits:?}"
    );
    // Her body's state: pure movement records on her body, at most one a
    // datagram, each the datagram's last record.
    let own = ana.owner | 1;
    let mut corrections = 0;
    for (_, records) in &ana.datagrams {
        let moves: Vec<_> = records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record.entity == own && record.movement && record.operation.is_none()
            })
            .map(|(index, _)| index)
            .collect();
        assert!(moves.len() <= 1, "{moves:?}");
        assert!(moves.iter().all(|&index| index + 1 == records.len()));
        corrections += moves.len();
    }
    assert!(corrections > 0, "no correction of Ana's body");
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
    // 1.5 m beside it.
    let (mut server, schemas, [mut enemy, mut patient, mut healer], _) = practice_players([
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
    // The enemy wounds the patient with short bursts. They start on the
    // same spawn point and, as enemies, push each other apart, so each
    // burst aims from where the two bodies stand (bodies: the enemy's, the
    // patient's, the healer's).
    let full = 150.0;
    let mut frame = enemy.tick + 1;
    let deadline = server.now() + Duration::from_secs(10);
    while !health(&patient).is_some_and(|health| health < full) {
        assert!(server.now() < deadline, "waiting for the wound");
        if frame <= enemy.tick + 8 {
            let bodies = server.bodies(MATCH).unwrap();
            let [from, to] = [0, 1].map(|index| bodies[index].position.unwrap());
            let aim = ((to[0] - from[0]).atan2(to[2] - from[2]) * 32768.0 / std::f64::consts::PI)
                .round() as i32 as i16;
            enemy
                .client
                .send(ACK, command_batch(frame, 8, aim, FIRE, false));
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
                // FieldSchedule::step: the field opens at its start tick S
                // (the cast's first update + FIELD_DELAY) and expires at
                // S + FIELD_TICKS. The create is queued at S and the destroy
                // at S + FIELD_TICKS, and each goes in its own tick's
                // datagram: FIELD_TICKS ticks apart.
                assert_eq!(lived, FIELD_TICKS, "the field lived {lived} ticks");
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
#[ignore = "builds Workshop Chamber's collision; requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR)"]
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
    // The scene is prepared on its own thread: the clock waits for it.
    server.await_scenes();
    let watch = |ticket: &Handoff, slot| {
        let mut client = Client::new(&server, ticket);
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
    // The world on the collision built from the install.
    pump_until(
        &mut [first, second],
        &schemas,
        "the controllers on the built collision",
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
        // the round's, whose clock Skirmish doesn't run. The flow starts at
        // the first spawn (the World step), before the clients take mode
        // frames (the Controller step, 0.1 s later): the frame at 0 s goes
        // to nobody, so each gets those at 1 to 29 s and the one at 30 s
        // that ends the assembly.
        assert_eq!(frames.len(), 30, "countdown frames");
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
#[ignore = "plays a whole Assault match at 1/10 scale on Anubis' collision (built once); requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR)"]
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
    // The scene is prepared on its own thread: the clock waits for it.
    server.await_scenes();
    let mut watchers: Vec<_> = tickets
        .iter()
        .enumerate()
        .map(|(slot, ticket)| {
            let mut client = Client::new(&server, ticket);
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
    pump_until(
        &mut [defender, attacker],
        &schemas,
        "the players in the world",
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
#[ignore = "slow: simulates a whole match at 1/4 scale with twelve bodies, about 40 s optimised; requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR)"]
fn slow_a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end() {
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
    // The scene is prepared on its own thread: the clock waits for it.
    server.await_scenes();
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
        client: Client::new(&server, &tickets[0]),
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
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the player in the world",
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
    let deadline = server.now() + Duration::from_secs(900);
    let mut samples = Vec::new();
    let mut next_sample = server.now();
    let mut home = false;
    while !home {
        watcher.pump(&schemas);
        home = watcher
            .datagrams
            .last()
            .is_some_and(|(messages, _)| messages.iter().any(|message| message.id == 20304));
        if server.now() >= next_sample {
            let bodies = server.bodies(MATCH).unwrap();
            if !bodies.is_empty() {
                samples.push(bodies);
            }
            next_sample += Duration::from_millis(250);
        }
        assert!(server.now() < deadline, "waiting for the end of the match");
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
#[ignore = "plays both sides of a whole Competitive Assault match at 1/50 scale on Anubis' collision (built once); requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR)"]
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
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
    // The scene is prepared on its own thread: the clock waits for it.
    server.await_scenes();
    let mut watchers: Vec<_> = tickets
        .iter()
        .enumerate()
        .map(|(slot, ticket)| {
            let mut client = Client::new(&server, ticket);
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
    pump_until(
        &mut [blue, red],
        &schemas,
        "the players in the world",
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

    let mut server = Lockstep::new(Arc::new(fixture())).unwrap();
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
/// `_prepare_bot_script`, server.py `_BotScriptDelivery`), over the wire: a
/// hit the bot survives sends a frame of its script, its pain line and no
/// elimination; the next kills it: its hit notice (25001), lost once,
/// goes again under its sequence, the elimination (25005) names it, its
/// death script's frames go before its destroy, and its replacement comes
/// 3 s later with a new script. No card names a bot: the team list and the
/// Tab board stay the players'.
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
    let names: Vec<String> = scene
        .bots()
        .iter()
        .map(|bot| bot.spawn.name.clone())
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
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let ticket = server
        .open_prepared_practice(MATCH, scene, player().remove(0), PlayerCard::default())
        .unwrap();
    let mut client = Client::new(&server, &ticket);
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
    // A bot's script frame from `from` on, full (its first) or a delta.
    let script = |watcher: &Watcher, from: usize, entity: u32, full: bool| {
        watcher.find(from, |record| {
            record.entity == entity
                && record
                    .script
                    .as_ref()
                    .is_some_and(|script| (script.first == 0) == full)
        })
    };
    // The world waits for every bot's script (match.py `_spawn_steps`).
    pump_until(&mut [&mut watcher], &schemas, "the world", |watchers| {
        let watcher = &watchers[0];
        watcher
            .find(0, |record| {
                record.entity == watcher.owner && record.script.is_some()
            })
            .is_some()
    });

    // Shoot the target: the fixture gun (`fixture_hitscan_factory`) fires
    // shots of 100, 9 a second (every 7 frames) while the button is held:
    // the commands of 16 frames with it held fire three. The bot (200)
    // survives the first and dies of the second.
    let owner = watcher.owner;
    send_player_message(
        &mut watcher.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    // A shot meets only a bot the client was shown (`WorldDelivery::
    // bot_shown`): a datagram with its first movement record arrived, and
    // the shot's command came in a later datagram than the one whose ACK
    // said so. The server reads that ACK with the commands of the datagram
    // that carries it (an ACK with no payload carries no input), and those
    // commands were made before the client could draw the bot. So: wait
    // for the target's first movement record, then send one idle command;
    // the shot goes in the next one.
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the body and the target",
        |watchers| {
            watchers[0].created(owner | 1).is_some()
                && watchers[0]
                    .find(0, |record| record.entity == target && record.movement)
                    .is_some()
        },
    );
    let idle = watcher.tick + 1;
    watcher
        .client
        .send(ACK, aimed_batch(idle, 1, -16384, 0, 0, false));
    // The first hit's 25001, and with it or after it a frame of the bot's
    // script, which doesn't fire: its nameplate enabled (upstream
    // `show_damage_nameplate`); no elimination yet.
    let hits_from = |watcher: &Watcher, from: usize, id: u32| {
        (from..watcher.datagrams.len())
            .find(|&index| watcher.datagrams[index].0.iter().any(|item| item.id == id))
    };
    let fired = watcher.datagrams.len();
    // The client sends each frame's command as its frame comes, the next
    // one after each datagram.
    let shot_frame = idle + 1;
    let mut next = shot_frame;
    let mut command = |watcher: &mut Watcher, buttons: u32| {
        while next <= watcher.tick + 1 {
            watcher
                .client
                .send(ACK, aimed_batch(next, 1, -16384, 0, buttons, false));
            next += 1;
        }
    };
    let deadline = server.now() + Duration::from_secs(2);
    while !hits_from(&watcher, fired, 25001)
        .is_some_and(|hit| script(&watcher, hit, target, false).is_some())
    {
        assert!(
            server.now() < deadline,
            "waiting for the surviving hit's nameplate frame"
        );
        command(&mut watcher, FIRE);
        watcher.pump(&schemas);
    }
    assert_eq!(hits_from(&watcher, fired, 25005), None);

    // The second shot kills the bot. The client leaves the datagram with
    // its hit notice out of its ACKs; the next ACK shows it lost, and the
    // notice goes again under the same sequence (`GameEgress::acknowledge`).
    let killed = watcher.datagrams.len();
    let hit_in = |packet: &Packet, known: &HashSet<u32>| {
        (packet.kind == KIND_FRAME)
            .then(|| {
                messages(&schemas, packet)
                    .into_iter()
                    .find(|item| item.message.id == 25001 && !known.contains(&item.sequence))
            })
            .flatten()
    };
    let (lost, kill_sent) = loop {
        assert!(server.now() < deadline, "waiting for the kill's hit");
        command(&mut watcher, FIRE);
        let packet = watcher.client.receive();
        if let Some(hit) = hit_in(&packet, &watcher.sequences) {
            break (hit, tick_of(&packet));
        }
        watcher.record(&schemas, &packet);
    };
    // Each hit names the frame its shot left the gun: shot k leaves
    // round(k / 9 s) after the press (`vm/volleys.rs`), the second 111 ms
    // later, in the 16 ms frame from 96 ms on: the press's frame + 6. 100
    // dealt, killing (bit 2), not critical.
    let first = &watcher.datagrams[hits_from(&watcher, fired, 25001).unwrap()]
        .0
        .iter()
        .find(|message| message.id == 25001)
        .unwrap()
        .fields;
    assert_eq!(first[&0x84], Value::Unsigned(u64::from(shot_frame)));
    assert_eq!(
        lost.message.fields[&0x84],
        Value::Unsigned(u64::from(shot_frame + 111 / 16))
    );
    let Value::Struct(impact) = &lost.message.fields[&0x78] else {
        panic!("hit body")
    };
    let entity = |id: u32| Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(id)))]));
    assert_eq!(impact[&0], entity(target));
    assert_eq!(impact[&4], Value::Unsigned(100), "damage");
    assert_eq!(impact[&8], Value::Unsigned(2), "killing");
    // The button comes up.
    let resent = loop {
        assert!(server.now() < deadline, "waiting for the hit's resend");
        command(&mut watcher, 0);
        let packet = watcher.client.receive();
        let hit = hit_in(&packet, &watcher.sequences);
        watcher.record(&schemas, &packet);
        if let Some(hit) = hit {
            break hit;
        }
    };
    assert_eq!(resent, lost);

    // 0.1 s after the first hit the bot's 0C17 raises its pain criteria
    // (100 of the 100 left: "severe", 0609); the client gets the bot's
    // voice set's line for it (ClientInVoice 27500): the bot speaks, to the
    // player's body, stimulus 0004.078 ("PnSvr"), its first line (key
    // 000B).
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the bot's pain line",
        |watchers| hits_from(watchers[0], fired, 27500).is_some(),
    );
    let hit = hits_from(&watcher, fired, 25001).unwrap();
    let said = hits_from(&watcher, fired, 27500).unwrap();
    assert!(said >= hit);
    let voice = &watcher.datagrams[said]
        .0
        .iter()
        .find(|message| message.id == 27500)
        .unwrap()
        .fields;
    assert_eq!(voice[&0x78], entity(target));
    assert_eq!(voice[&0x7c], entity(owner | 1));
    assert_eq!(voice[&0x88], Value::Unsigned(0x0ee0_0000_0000_0004));
    assert_eq!(voice[&0x90], Value::Unsigned(0x0760_0000_0000_000b));
    let replacement = BOT_ENTITY + 15;
    pump_until(&mut [&mut watcher], &schemas, "the kill", |watchers| {
        hits_from(watchers[0], killed, 25005).is_some()
    });
    let kill = hits_from(&watcher, killed, 25005).unwrap();
    let elimination = watcher.datagrams[kill]
        .0
        .iter()
        .find(|message| message.id == 25005)
        .unwrap();
    assert_eq!(elimination.fields[&0x84], entity(target));
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the replacement",
        |watchers| watchers[0].created(replacement).is_some(),
    );
    // combat.py `BOT_RESPAWN`: 3 s. The killing command runs from the
    // input buffer as tick T is due, at T's time, before the tick
    // (`InputQueue`): the kill's notices go in that tick's datagram, T. The
    // replacement is due at the first tick 3 s after the kill, T + 188
    // (3000 / 16 = 187.5), and goes in that tick's datagram, T + 188: 188
    // ticks after the kill's notices. (Before the input buffer the command
    // ran on arrival, at the time of the tick before: 187.)
    let created = watcher.created(replacement).unwrap();
    let respawned = watcher.ticks[created] - kill_sent;
    assert_eq!(respawned, 188, "replaced after {respawned} ticks");
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
    assert!(death <= destroyed && destroyed <= created);
    assert!(created < script(&watcher, 0, replacement, true).unwrap());
    // No card names a bot.
    assert!(!watcher.datagrams.iter().any(|(messages, _)| {
        messages.iter().any(|message| {
            message.id == 20308
                && matches!(
                    message.fields.get(&0x80),
                    Some(Value::Struct(card))
                        if matches!(card.get(&0x40), Some(Value::String(tag)) if names.contains(tag))
                )
        })
    }));
    server.stop().unwrap();
}

/// combat.py `_bot_weapons`: a firing training bot's script fires its
/// rockets along its yaw, and they damage the player in front of it, once
/// he ran out of his spawn room (where he is invulnerable).
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
    let ([x, y, z], facing) = scene.map().spawn_point(0, 0).unwrap();
    // The way the spawn point faces (yaw 0 faces +z, a quarter turn +x),
    // 3 m past where the spawn room ends.
    let (sin, cos) = facing.to_radians().sin_cos();
    let along = move |[px, _, pz]: [f64; 3]| (px - x) * sin + (pz - z) * cos;
    let exit = (0..400)
        .map(|step| f64::from(step) * 0.25)
        .find(|&reach| {
            let point = [x + reach * sin, y, z + reach * cos];
            scene.in_spawn_room(0, orpheus_content::maps::SpawnChoice::First, point) == Some(false)
        })
        .unwrap();
    let reach = exit + 3.0;
    let [x, z] = [x + reach * sin, z + reach * cos];
    // An enemy firing bot 4 m beside that spot, facing it (a quarter turn:
    // along +x), where the other bots leave the line clear.
    let shooter = scene
        .bots_mut()
        .iter_mut()
        .find(|bot| bot.spawn.firing && bot.spawn.team != 0)
        .unwrap();
    shooter.position = [x - 4.0, y, z];
    shooter.yaw = 16384.0;
    let shooter = shooter.entity;
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let ticket = server
        .open_prepared_practice(MATCH, scene, player().remove(0), PlayerCard::default())
        .unwrap();
    let mut client = Client::new(&server, &ticket);
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
    pump_until(&mut [&mut watcher], &schemas, "the body", |watchers| {
        watchers[0].created(body).is_some()
    });
    run_out_of_the_spawn_room(
        &server,
        &mut watcher,
        &schemas,
        |feet| along(feet) >= reach,
        0,
    );
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

/// A living enemy training bot blocks Soldier: 76's body on the server
/// (`orpheus_sim::replication::blocks`), as the client's own mover stops at
/// the bot's capsule: he runs out of his spawn room at a Training Bot
/// standing 3 m past its end, turning toward the bot every 4 frames, and
/// stops against it, their axes the sum of their radii apart (0.5 + 0.5,
/// less at most the solver's 5 mm slop and 5 mm tolerance), never nearer.
/// His owner corrections are his server body's poses, so none pulls his
/// client through the bot.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn soldier_runs_into_an_enemy_bot_and_stops_short_of_it() {
    let assets =
        orpheus_sim::scene_assets::SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let mut placed = None;
    let (server, schemas, [mut soldier], _) =
        practice_world([("soldier#1111", 0, "Soldier: 76")], &assets, |scene| {
            let ([x, y, z], facing) = scene.map().spawn_point(0, 0).unwrap();
            // The way the spawn point faces (yaw 0 faces +z, a quarter turn
            // +x), 3 m past where the spawn room ends, as
            // `a_firing_practice_bot_shoots_the_player` finds it clear.
            let (sin, cos) = facing.to_radians().sin_cos();
            let exit = (0..400)
                .map(|step| f64::from(step) * 0.25)
                .find(|&reach| {
                    let point = [x + reach * sin, y, z + reach * cos];
                    scene.in_spawn_room(0, orpheus_content::maps::SpawnChoice::First, point)
                        == Some(false)
                })
                .unwrap();
            let reach = exit + 3.0;
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| !bot.spawn.firing && bot.spawn.team != 0 && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + reach * sin, y, z + reach * cos];
            placed = Some((bot.position, facing));
        });
    let ([bx, _, bz], facing) = placed.unwrap();
    let gap = |[px, _, pz]: [f64; 3]| (bx - px).hypot(bz - pz);
    // Yaw 0 faces +z, a quarter turn +x.
    let toward = |[px, _, pz]: [f64; 3]| {
        ((bx - px).atan2(bz - pz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16
    };
    let mut yaw = (facing * 65536.0 / 360.0).round() as i32 as i16;
    // He runs for 8 s, at most 44 m at 5.5 m/s, far more than the way to
    // the bot; his body's feet after every datagram.
    let mut frame = soldier.tick + 1;
    let end = server.now() + Duration::from_secs(8);
    let mut feet = Vec::new();
    while server.now() < end {
        if frame <= soldier.tick + 8 {
            soldier
                .client
                .send(ACK, command_batch(frame, 4, yaw, 0, true));
            frame += 4;
        }
        soldier.pump(&schemas);
        let at = server.bodies(MATCH).unwrap()[0].position;
        if let Some(at) = at.filter(|&at| gap(at) < 6.0) {
            yaw = toward(at);
        }
        feet.extend(at);
    }
    let wrong = feet.iter().position(|&at| gap(at) < 0.99);
    assert_eq!(
        wrong,
        None,
        "the bot at {:?}: {:?}",
        [bx, bz],
        wrong.map(|first| &feet[first.saturating_sub(3)..(first + 3).min(feet.len())])
    );
    let last = *feet.last().unwrap();
    assert!(gap(last) <= 1.0, "{last:?}");
}
