use super::*;
use lockstep::{Client, Lockstep};
use lucy_content::maps::PRACTICE_RANGE_MAP;
use lucy_core::bits::{BitReader, BitWriter};
use lucy_core::tick::{FIRST_TICK, TICK_PERIOD};
use lucy_wire::{
    entity::GAME_MODE_ENTITY,
    link::{ACK, FIN, KIND_CONTROL, KIND_FRAME, Packet, SYN},
    messages::{encode, read_channel, write_channel},
};
use std::collections::HashSet;

const MATCH: MatchId = [5, 8];
/// The match tests' scene seed (`SceneAssets::with_seed`): the same mode
/// random value and match seed each run, as the AI heroes' fixed seed on
/// the virtual clock (`lockstep.rs`).
const SCENE_SEED: u64 = 0x0f4a_1225;

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

fn messages(schemas: &Schemas, packet: &Packet) -> Vec<lucy_wire::messages::ReliableMessage> {
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
    // The room acknowledges the datagram on its own as it takes it, at the
    // last tick's time, a tick before the next frame
    // (`GameConnections::acknowledge`). It was the client's fourth (SYN,
    // ACK, the heartbeat's ACK, this one: sequence 3), so the next
    // expected is 4, with the three before it arrived.
    let (acked_at, ack) = client.receive_any();
    assert!(lockstep::is_bare_ack(&ack), "{ack:?}");
    assert_eq!((acked_at, ack.ack, ack.ack_bits), (loaded_at, 4, 0b1111));
    let mut received = Vec::new();
    let mut first_frame = None;
    while received.len() < 3 && client.now() < loaded_at + Duration::from_secs(2) {
        let packet = client.receive();
        assert!(client.window.accept(packet.seq));
        first_frame.get_or_insert(packet.seq);
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
    // The frame after the ACK, which took the sequence before it.
    assert_eq!(first_frame, Some(ack.seq + 1));
    // The pong: the ping's index and stamp, then the server's received
    // (the datagram's arrival), made (the room took it) and sent (its
    // frame left at the next tick, 16 ms later), in nanoseconds of the
    // service's clock (`ingress::Pong`).
    assert_eq!(received[0].message.fields[&0x78], Value::Unsigned(17));
    let nanos = |time: Duration| Value::Unsigned(u64::try_from(time.as_nanos()).unwrap());
    assert_eq!(
        received[0].message.fields[&0x80],
        Value::Array(vec![
            Value::Unsigned(25),
            nanos(loaded_at),
            nanos(loaded_at),
            nanos(loaded_at + TICK_PERIOD),
        ])
    );

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
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn worker_serves_loading_using_the_retail_schemas() {
    let schemas = Schemas::game(&lucy_testkit::runtime_dir()).unwrap();
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
#[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
fn a_player_taken_out_before_his_game_connects_leaves_at_once_and_only_gets_20304() {
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
    /// A create's movement state's frame (counted back from the create's
    /// base, `CREATE_BASE`), when it carries one.
    create_frame: Option<u32>,
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
    /// A list of u32 (a target tag list of 76).
    U32s(Vec<u32>),
}

/// The components of a create or update (`lucy_wire::entity`
/// `write_components`) when each is a 26, 29 (id and BattleTag), 44, 51
/// (the pools), 75, 76 or 123, the components of an observed or a bot's
/// body; `None` for any other (a player's or a bot's stand-in controller,
/// with its 74).
fn decode_components(mut reader: BitReader<'_>) -> Option<Vec<DecodedComponent>> {
    #[derive(Clone, Copy)]
    enum Kind {
        Id,
        DbId,
        Entity,
        Text,
        Bytes(usize),
        U32s,
    }
    /// `lucy_wire::entity` HEALTH: 48 pools, then two u32, the killer
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
            76 => &[Kind::U32s; 2],
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
                Kind::U32s => {
                    reader.align();
                    let count = reader.bits(32).ok()? as usize;
                    DecodedField::U32s(
                        (0..count)
                            .map(|_| reader.bits(32).map(|value| value as u32))
                            .collect::<Result<_, _>>()
                            .ok()?,
                    )
                }
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
    /// A part's number and whether it is the last (`mode_view.rs`).
    part: Option<(u16, bool)>,
    width: usize,
    data: Vec<u8>,
}

impl ReceivedChunk {
    fn bits(&self) -> BitReader<'_> {
        BitReader::with_range(&self.data, 0, self.width).unwrap()
    }
}

/// The whole chunks among `chunks` (one entity's, in arrival order): each
/// whole one, and each chunk in parts once its parts up to the last are
/// all there, joined as the client's joiner does (`0x7FF78991FE30`).
fn joined_chunks<'a>(chunks: impl IntoIterator<Item = &'a ReceivedChunk>) -> Vec<ReceivedChunk> {
    let mut parts: HashMap<(u32, u32), std::collections::BTreeMap<u16, &ReceivedChunk>> =
        HashMap::new();
    let mut whole = Vec::new();
    for chunk in chunks {
        let Some((number, _)) = chunk.part else {
            whole.push(chunk.clone());
            continue;
        };
        let held = parts.entry((chunk.first, chunk.span)).or_default();
        held.insert(number, chunk);
        let Some((&last, _)) = held
            .iter()
            .find(|(_, part)| part.part.is_some_and(|(_, last)| last))
        else {
            continue;
        };
        if (0..=last).all(|number| held.contains_key(&number)) {
            let mut data = BitWriter::new();
            for part in held.values() {
                data.raw_bits(&part.data, part.width).unwrap();
            }
            whole.push(ReceivedChunk {
                first: chunk.first,
                span: chunk.span,
                part: None,
                width: data.bit_len(),
                data: data.into_bytes(),
            });
            parts.remove(&(chunk.first, chunk.span));
        }
    }
    whole
}

/// The graphs of the instances a mode entity's frame lists: a whole
/// frame's, or the new ones of a delta.
fn listed_graphs(graphs: &lucy_script::graph::GraphStore, chunk: &ReceivedChunk) -> Vec<u16> {
    use lucy_script::wire::{DeltaInstance, read_delta_instances, read_full_head};
    if chunk.first == 0 {
        let head = read_full_head(&chunk.data, |graph| {
            graphs
                .graph(u32::from(graph))
                .map(|graph| graph.header.states_bits)
        })
        .expect("a full frame's head");
        return head
            .instances
            .iter()
            .map(|&(_, graph, _, _)| graph)
            .collect();
    }
    read_delta_instances(&chunk.data)
        .expect("a delta's instances")
        .into_iter()
        .filter_map(|instance| match instance {
            DeltaInstance::Described((_, graph, _, _)) => Some(graph),
            _ => None,
        })
        .collect()
}

/// Hero select open (#2438e), the pick (#13161e while #2114e) and the hero
/// he plays (#940e), as a full owner controller frame shows them.
type ControllerShows = (bool, Option<u64>, Option<u64>);

/// What a full owner controller frame shows (its head: the controller
/// script's projection, `lucy_sim::controller`): [`ControllerShows`].
/// None for another chunk. `graphs` give the states of the variables' Stack
/// links (`0C90`'s mode ruleset switch st47 binds #8430e or #8431e).
fn controller_shows(
    graphs: &lucy_script::graph::GraphStore,
    chunk: &ReceivedChunk,
) -> Option<ControllerShows> {
    let (shown, instances) = controller_head(graphs, chunk)?;
    // On the Practice Range the Training Partner console's scope 126
    // (`265F`) opens on every controller once the console runs.
    let scoped = instances.len() == 6 && instances[5] == 0x265f;
    assert_eq!(
        &instances[..instances.len() - usize::from(scoped)],
        [0x0c90, 0x288a, 0x0215, 0x288b, 0x288d]
    );
    Some(shown)
}

/// [`controller_shows`]'s reading of a full owner controller frame, with
/// the graphs of the instances it lists.
fn controller_head(
    graphs: &lucy_script::graph::GraphStore,
    chunk: &ReceivedChunk,
) -> Option<(ControllerShows, Vec<u16>)> {
    use lucy_script::wire::{Value as ScriptValue, read_full_head};
    if chunk.first != 0 {
        return None;
    }
    let head = read_full_head(&chunk.data, |graph| {
        graphs
            .graph(u32::from(graph))
            .map(|graph| graph.header.states_bits)
    })
    .expect("a full frame's head");
    head.command_frame?;
    let flag = |id| head.variables.get(&id) == Some(&ScriptValue::Bool(true));
    let asset = |id| match head.variables.get(&id) {
        Some(&ScriptValue::Asset(guid)) if guid != 0 => Some(guid),
        _ => None,
    };
    assert_eq!(flag(2438), flag(31435), "#31435 follows #2438");
    let instances = head
        .instances
        .iter()
        .map(|&(_, graph, _, _)| graph)
        .collect();
    Some((
        (flag(2438), asset(13161).filter(|_| flag(2114)), asset(940)),
        instances,
    ))
}

/// The Practice Range's first mode frame: `13C0`'s full observer frame as
/// the room runs the graph (`Room::first_mode_frame`, the graph's newest).
/// It carries what opened the Tab board when it was built by hand
/// (`scene::BOARD_POSTERS` before 2026-11-18: `13C0` with remote slot 4,
/// the `06DF.025` poster st6, on and `0CB6` under st21, the frame upstream
/// kjs saw open the board, `254fa913`), and the rest of the graph's tree.
/// The layouts are `lucy script query show 13C0` and `0CBB`.
fn assert_range_mode_frame(graphs: &lucy_script::graph::GraphStore, chunk: &ReceivedChunk) {
    use lucy_script::wire::{Value as ScriptValue, read_full_head, read_value};
    let head = read_full_head(&chunk.data, |graph| {
        graphs
            .graph(u32::from(graph))
            .map(|graph| graph.header.states_bits)
    })
    .expect("a full frame's head");
    // An observer frame: no CmFD.
    assert_eq!(head.command_frame, None);
    // The instances (id, graph, creator, parent instance and state): the
    // root `13C0`; its SubScripts `0CBB` (st5, the PvP modes' shared
    // subgraph), `0CB6` (st21, the mode UI) and `0C8F` (st16, under
    // st15's `06FD`); and `0CBB`'s `11A6` (st43), `03F2` (st44), `180A`
    // (st45) and `1B4A` (st50).
    assert_eq!(
        head.instances,
        [
            (1, 0x13c0, None, None),
            (2, 0x0cbb, None, Some((1, 5))),
            (3, 0x0cb6, None, Some((1, 21))),
            (4, 0x0c8f, None, Some((1, 16))),
            (5, 0x11a6, None, Some((2, 43))),
            (6, 0x03f2, None, Some((2, 44))),
            (7, 0x180a, None, Some((2, 45))),
            (8, 0x1b4a, None, Some((2, 50))),
        ]
    );
    // Mode `005A`'s own variable (the mode's `m_3CE93B76`).
    assert_eq!(head.variables.get(&27534), Some(&ScriptValue::Bool(true)));
    // Then instance 1's sections. First its three presence slots (the sync
    // vars with a presence bit), each a present bit, then the value and no
    // Stack link: #27536 = !#27534 (st4's Stack) false, so st2 takes its
    // false subgraph, the poster st6; #12581 true (st17's false subgraph,
    // rid31); #375 none (st10's FilterArray, on st17's true subgraph, never
    // begins).
    let mut bits = BitReader::with_range(&chunk.data, head.end, chunk.width).unwrap();
    for expected in [
        Some(ScriptValue::Bool(false)),
        Some(ScriptValue::Bool(true)),
        None,
    ] {
        let value = bits.bit().unwrap().then(|| read_value(&mut bits).unwrap());
        if value.is_some() {
            assert!(!bits.bit().unwrap(), "a presence slot with a link");
        }
        assert_eq!(value, expected);
    }
    // No instance variables.
    assert!(!bits.bit().unwrap());
    // Its ten remote states, in `m_remoteSyncNodes` order. A running state
    // is a present bit and its active bit, then its payload: a
    // SendGameMessage (`STU_38EE1100`) its reply flag, off; a Stack or the
    // UX presenter `STU_62755D14` (st0; the mode entity's presenters go in
    // its frames, `SnapshotEncoder::with_presenters`) nothing in an
    // observer frame. A state left out is a bit 0: st8, `0702` on `0071`,
    // which nobody sent; st12, #3179's Stack on st17's true subgraph.
    let send = Some(1);
    let stack = Some(0);
    for (state, payload) in [
        (0, stack),
        (1, send),  // `0727`: the controllers open hero select
        (3, send),  // `0694`
        (4, stack), // #27536
        (6, send),  // `06DF.025`: the board poster
        (8, None),
        (9, send), // `0724`: the spawn rooms
        (12, None),
        (15, send), // `06FD`: the bodies' respawn seconds
        (18, send), // `0702`: the doors open
    ] {
        assert_eq!(bits.bit().unwrap(), payload.is_some(), "13C0 st{state}");
        if let Some(width) = payload {
            assert!(bits.bit().unwrap(), "13C0 st{state} runs");
            if width != 0 {
                assert_eq!(bits.bits(width).unwrap(), 0, "13C0 st{state} payload");
            }
        }
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
    // The ch3 clock flags (any), then no ping, keep ch4, an empty ch5.
    reader.bits(4).unwrap();
    assert_eq!(reader.bits(3).unwrap(), 0b010);
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
                release_position: None,
                launches: Vec::new(),
                base_health: None,
                updated: None,
                components: None,
                create_frame: None,
            };
            assert!(reader.bit().unwrap());
            if !reader.bit().unwrap() {
                return record;
            }
            if reader.bit().unwrap() {
                assert!(!reader.bit().unwrap());
                let first = reader.w_u32().unwrap();
                let span = reader.w_var().unwrap();
                let part = reader
                    .bit()
                    .unwrap()
                    .then(|| (reader.w_u16().unwrap(), reader.bit().unwrap()));
                let width = reader.w_var().unwrap() as usize;
                record.script = Some(ReceivedChunk {
                    first,
                    span,
                    part,
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
                        if flags & lucy_wire::entity::SKIN_THEME != 0 {
                            assert_ne!(flags & lucy_wire::entity::WITH_SKIN, 0);
                            record.skin =
                                Some((reader.bits(32).unwrap() as u32, reader.bit().unwrap()));
                        }
                        use lucy_wire::entity::{WITH_MOVEMENT, WITH_SCALE, WITH_TRANSFORM};
                        let mut components = reader;
                        if flags & WITH_TRANSFORM != 0 {
                            components.advance(7 * 32).unwrap();
                        }
                        if flags & (WITH_TRANSFORM | WITH_SCALE) != 0 {
                            components.advance(3 * 32).unwrap();
                        }
                        if flags & WITH_MOVEMENT != 0 {
                            record.create_frame = Some(movement_state(&mut components));
                        }
                        record.components = decode_components(components);
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

/// A create's movement state as `lucy_wire::entity::write_movement`
/// writes it, read past; its frame, counted back from the create's base.
fn movement_state(reader: &mut BitReader<'_>) -> u32 {
    fn selector(reader: &mut BitReader<'_>, width: usize) {
        if reader.bit().unwrap() {
            assert_eq!(reader.bits(3).unwrap(), 0b100);
            reader.advance(width).unwrap();
        }
    }
    fn delta(reader: &mut BitReader<'_>, widths: &[usize]) {
        if !reader.bit().unwrap() {
            return;
        }
        let width = widths.iter().find(|_| reader.bit().unwrap()).unwrap_or(&32);
        reader.advance(*width).unwrap();
    }
    fn raw_frame(reader: &mut BitReader<'_>) {
        assert!(!reader.bit().unwrap());
        reader.advance(32).unwrap();
    }
    if reader.bit().unwrap() {
        for _ in 0..4 {
            if reader.bit().unwrap() {
                reader.advance(8).unwrap();
            }
        }
    }
    assert!(!reader.bit().unwrap(), "the 1/1024 grid");
    let frame_back = if !reader.bit().unwrap() {
        0
    } else if reader.bit().unwrap() {
        reader.bits(4).unwrap()
    } else if reader.bit().unwrap() {
        reader.bits(10).unwrap()
    } else {
        reader.bits(32).unwrap()
    } as u32;
    if reader.bit().unwrap() {
        raw_frame(reader); // The discontinuity counter.
    }
    selector(reader, 16); // Pitch.
    delta(reader, &[4, 10]); // Air ticks.
    assert!(!reader.bit().unwrap(), "a timed movement event");
    delta(reader, &[8, 10, 24]); // Gravity.
    assert!(!reader.bit().unwrap(), "a move-speed scale");
    let input = reader.bit().unwrap();
    if input && reader.bit().unwrap() {
        raw_frame(reader);
    }
    if reader.bit().unwrap() {
        raw_frame(reader); // The crouch frame.
    }
    for _ in 0..if input { 4 } else { 2 } {
        if reader.bit().unwrap() {
            reader.advance(8).unwrap();
        }
    }
    selector(reader, 16); // Yaw.
    assert_eq!(reader.bits(2).unwrap(), 0, "Euler pitch or roll");
    for _ in 0..3 {
        selector(reader, lucy_wire::entity::MOVEMENT_AXIS_BITS);
    }
    if reader.bit().unwrap() {
        for _ in 0..3 {
            selector(reader, lucy_wire::entity::MOVEMENT_AXIS_BITS);
        }
    }
    assert_eq!(reader.bits(9).unwrap(), 0);
    if reader.bit().unwrap() {
        reader.advance(64).unwrap(); // The parent and its matrix index.
    }
    delta(reader, &[8, 12, 17]);
    delta(reader, &[8, 10, 24]);
    delta(reader, &[8, 12, 17]);
    assert!(!reader.bit().unwrap(), "+384");
    if reader.bit().unwrap() {
        assert_eq!(reader.bits(2).unwrap(), 0b10);
        reader.advance(33).unwrap(); // The fall, then a clear bit.
    }
    lucy_wire::entity::CREATE_BASE.wrapping_sub(frame_back)
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
    acting_batch(first, count, yaw, pitch, buttons, forward, 0)
}

/// [`aimed_batch`] with each command's action byte (`timeline.rs`: 42 is
/// the hero select key, H).
fn acting_batch(
    first: u32,
    count: u8,
    yaw: i16,
    pitch: i16,
    buttons: u32,
    forward: bool,
    action: u8,
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
    input.bits(u64::from(action), 8).unwrap();
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
    let count = (newest - *next + 1).min(u32::from(lucy_wire::commands::MAX_COMMANDS));
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
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::Command;

    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
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
    let opening = Opening {
        id: MATCH,
        map,
        players,
        world: Some(world),
        ai: None,
    };
    let settings = RoomSettings {
        schemas: Arc::new(fixture()),
        timers: Timers::default(),
        ai_seed: None,
        events: router::Events::nowhere(MATCH),
    };
    // The seats take connections 1 to 5, in slot order.
    let allocate = |count: usize| Ok((1..=count as u32).collect());
    let (mut state, _) = RoomState::open(opening, settings, Duration::ZERO, allocate).unwrap();
    let first = FIRST_TICK + 1;
    // A tick runs all but the last MAX_BUFFERED (4) of a long input buffer
    // at once (`InputQueue::release`): 36 commands give each peer 32 to run.
    let commands: Vec<_> = (first..first + 36)
        .map(|frame| Command {
            frame,
            ..Command::default()
        })
        .collect();
    let room = &state.room;
    for slot in 0..room.players.len() {
        let connection = slot as u32 + 1;
        let viewer = room.appearance(slot).unwrap();
        let mut peer = Peer::new(viewer.entity);
        peer.world
            .start(&room.world.as_ref().unwrap().scene, &viewer, FIRST_TICK)
            .unwrap();
        peer.world.queue_commands(FIRST_TICK, &commands);
        peer.world.release_input(1);
        state.peers.insert(connection, peer);
    }
    let acks = |state: &RoomState| {
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
    use lucy_lobby::{
        accounts::Accounts,
        collection::Collection,
        handoff::{CREATE_GAME, GAME_REQUEST},
        handoff::{HANDOFF, HANDOFF_MESSAGE},
        login::Identity,
        session::PreparedLogin,
    };
    use lucy_sim::{
        body_script::fixture_projectile_factory, bots::BOT_ENTITY, scene_assets::SceneAssets,
    };
    use lucy_wire::jam::codec::id16;

    const CONTROLLER: u32 = 0xa000_0100;
    let runtime = lucy_testkit::runtime_dir();
    let data = runtime.join("data");
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
    let lobby = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let assets = SceneAssets::load(runtime).unwrap();
    let collection = Collection::load(&data).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let accounts = Accounts::new(
        temporary.path().join("profiles"),
        temporary.path().join("seed"),
    )
    .unwrap();
    let account = accounts.get("world_fixture").unwrap();
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
            team_channel: channel(MATCH, 5),
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
                scene.owner_body_factory = Some(Arc::new(move |hero, spawn, _| {
                    if hero.guid != lucy_content::heroes::SOLDIER {
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
        [20404, 20402, 20402, HANDOFF_MESSAGE]
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
    // The mode's and the controller's first script chunks (the Training
    // Partner's player, `player_entity(1)`, and his body have their own,
    // as do the server placeables whose scripts run whole and the
    // entities their graphs make (the health packs' visuals, running
    // `0EB9`), once their creates arrived (`placed_script.rs`): not
    // checked here).
    let first_script = |record: &WorldRecord, mode: &mut bool, controller: &mut bool| {
        let Some(script) = &record.script else {
            return;
        };
        let placeable = record.entity & 0xFFFF_F000 == 0x8000_0000;
        let made = (lucy_script::placed::MADE_FIRST
            ..lucy_script::placed::MADE_FIRST + lucy_script::placed::MADE_SPAN)
            .contains(&record.entity);
        if record.entity & !0xFF == player_entity(1) || placeable || made {
            return;
        }
        if record.entity == GAME_MODE_ENTITY {
            // The whole frame first, then the graph's changes as deltas
            // from the frame the client acknowledged
            // (`lucy_sim::mode_view`).
            if *mode {
                assert_ne!(script.first, 0, "a delta after the whole frame");
            } else {
                assert_eq!((script.first, script.span), (0, 1));
                assert_range_mode_frame(assets.graphs(), script);
            }
            *mode = true;
        } else {
            assert_eq!(record.entity, CONTROLLER);
            // Whole frames (first 0), and after the first one owner acks
            // with the command frame every 100 ms
            // (`WorldDelivery::send_controller_ack`).
            if script.first == 0 {
                assert_eq!(script.span, 1);
            } else {
                assert!(*controller, "an owner ack only after the whole frame");
            }
            let mut bits = script.bits();
            assert_eq!(bits.bits(3).unwrap(), 2);
            assert!(bits.w_var().unwrap() >= lucy_core::tick::FIRST_TICK);
            *controller = true;
        }
    };
    // The rest of the world follows, with no script whose create the
    // client hasn't acknowledged: 4 placeables, the player, the mode, the
    // map's 26 server placeables (`game_map_areas` `server_placeables`),
    // 15 bots and their 15 stand-in controllers, the Training Partner
    // the console `265F` makes (his player and body, `ai.rs`), and the
    // visuals of the Range's 4 health packs (`0046` st15
    // StateCreateEntity, made by the server's instance of the pack):
    // 4 + 2 + 26 + 15 + 15 + 2 + 4 = 68.
    while creates.len() < 68 {
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
    assert_eq!(creates.len(), 68);

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
            // The bots move, and so may the Training Partner's body (a
            // player's body: the console `265F`'s dummy, `ai.rs`).
            let partner = record.entity & !0xFF == player_entity(1);
            if record.movement {
                assert!(creates.contains(&record.entity));
                if !partner {
                    assert!((BOT_ENTITY..BOT_ENTITY + 15).contains(&record.entity));
                    moving_bot = true;
                }
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
    send_instance_message(client, schemas, owner, ordinal, guid, 3, parameters);
}

/// A 21616 `{owner, parameters, guid, instance}` as the client's
/// `STU_79A131B4` sends it from its instance `instance` on `owner`.
fn send_instance_message(
    client: &mut Client,
    schemas: &Schemas,
    owner: u32,
    ordinal: u32,
    guid: u64,
    instance: i64,
    parameters: Vec<u8>,
) {
    let fields = Fields::from([
        (
            0x78,
            Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(owner)))])),
        ),
        (0x80, Value::Blob(parameters)),
        (0xa8, Value::Unsigned(guid)),
        (0xb0, Value::Signed(instance)),
    ]);
    let message = encode(
        schemas,
        lucy_content::parameters::CLIENT_GAME_MESSAGE,
        &fields,
    )
    .unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(ordinal, message)], &[]).unwrap();
    client.send(ACK, payload.into_bytes());
}

fn pick_soldier_for_weapon(client: &mut Client, schemas: &Schemas) -> u32 {
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};
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
    use lucy_sim::{
        body_script::FIXTURE_PROJECTILE_DEFINITION, bots::BOT_ENTITY, projectiles::ProjectileLeases,
    };
    use lucy_wire::commands::SECONDARY;

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
    assets: &lucy_sim::scene_assets::SceneAssets,
    server: &GameService,
    ticket: &Handoff,
) {
    use lucy_content::{
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
    // The controller's whole frames (first 0); its owner acks (the
    // command frame every 100 ms, `WorldDelivery::send_controller_ack`)
    // are left out, and they move the whole frames' chunk numbers on.
    let ack = |record: &WorldRecord| {
        record.entity == OWNER
            && record
                .script
                .as_ref()
                .is_some_and(|script| script.first != 0)
    };
    let describe = |records: &[WorldRecord]| {
        records
            .iter()
            .filter(|record| matches!(record.entity, OWNER | FIRST | SECOND) && !ack(record))
            .map(|record| {
                (
                    record.entity,
                    record.operation,
                    record.script.as_ref().map(|script| script.first),
                )
            })
            .collect::<Vec<_>>()
    };
    // The frame the controller script's projection gives: hero select open
    // or not, the pick and the hero he plays.
    let verify_controller = |records: &[WorldRecord], open, pick: Option<u64>, hero| {
        let script = records
            .iter()
            .find(|record| record.entity == OWNER && record.script.is_some() && !ack(record))
            .and_then(|record| record.script.as_ref())
            .expect("hero selection controller frame");
        let mut reader = script.bits();
        assert_eq!(reader.bits(3).unwrap(), 2);
        let cmfd = reader.w_var().unwrap();
        assert_eq!(
            controller_shows(assets.graphs(), script),
            Some((open, pick, Some(hero)))
        );
        cmfd
    };

    send(client, 1, PICK_HERO, SOLDIER.to_le_bytes().to_vec());
    let (_lost, records) = next_body_frame(client, FIRST);
    let expected = [
        (FIRST, Some(1), None),
        (OWNER, Some(2), None),
        (OWNER, None, Some(0)),
    ];
    // The pick's frame names the new hero with the screen still open;
    // `288B` closes it once the body lives (st95), a tick later: that
    // frame may share the datagram, after the possession, replaced by a
    // newer frame queued before it went (the controller's other states
    // change after the spawn too, and each change sends a frame).
    let pick_batch = |records: &[WorldRecord]| {
        let described = describe(records);
        assert_eq!(described[..3], expected);
        match described[3..] {
            [] => {}
            [(OWNER, None, Some(0))] => {
                let close = records
                    .iter()
                    .filter(|record| record.entity == OWNER && !ack(record))
                    .filter_map(|record| record.script.as_ref())
                    .nth(1)
                    .unwrap();
                assert_eq!(
                    controller_shows(assets.graphs(), close),
                    Some((false, Some(SOLDIER), Some(SOLDIER)))
                );
            }
            ref rest => panic!("{rest:?}"),
        }
    };
    pick_batch(&records);
    assert!(
        records
            .iter()
            .any(|record| record.entity == FIRST && record.movement)
    );
    let first_cmfd = verify_controller(&records, true, Some(SOLDIER), SOLDIER);

    // A later datagram ACK identifies the first body batch as lost.
    let following = client.receive();
    assert!(client.window.accept(following.seq));
    assert!(describe(&world_records(schemas, &following)).is_empty());
    client.send(ACK, Vec::new());
    let (retry, records) = next_body_frame(client, FIRST);
    pick_batch(&records);
    assert_eq!(
        verify_controller(&records, true, Some(SOLDIER), SOLDIER),
        first_cmfd
    );
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
        // The owner's body runs a frame every tick, this silent client's
        // too (past the input buffer's wait and repeats, neutral frames:
        // `InputQueue`), so its movement (a correction goes every tick, see
        // `await_input`) and its owner frame may come in any datagram, the
        // reopen's too; neither is part of the reopen.
        let records: Vec<_> = records
            .into_iter()
            .filter(|record| !(record.entity == FIRST && record.operation.is_none()))
            .collect();
        // The close of the pick (st95) may come first.
        let closed = records.iter().any(|record| {
            record.entity == OWNER
                && record.script.as_ref().is_some_and(|script| {
                    controller_shows(assets.graphs(), script).is_some_and(|(open, ..)| !open)
                })
        });
        if !closed && !describe(&records).is_empty() {
            let described: Vec<_> = describe(&records)
                .into_iter()
                .map(|(entity, operation, script)| (entity, operation, script.is_some()))
                .collect();
            assert_eq!(described, [(OWNER, None, true)]);
            // An open clears the pick (`288B`).
            break verify_controller(&records, true, None, SOLDIER);
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
    let described: Vec<_> = describe(&records)
        .into_iter()
        .map(|(entity, operation, script)| (entity, operation, script.is_some()))
        .collect();
    assert_eq!(
        described[..5],
        [
            (SECOND, Some(1), false),
            (OWNER, Some(2), false),
            (FIRST, Some(3), false),
            (OWNER, None, true),
            (OWNER, Some(2), false)
        ],
    );
    // `288B`'s close (st95) may follow in the same datagram.
    assert!(
        described[5..]
            .iter()
            .all(|&entry| entry == (OWNER, None, true))
    );
    assert!(verify_controller(&records, true, Some(tracer), tracer) > reopen_cmfd);
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
    datagrams: Vec<(Vec<lucy_wire::messages::Message>, Vec<WorldRecord>)>,
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
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::FIRE;

    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    assert!(scene.movement_ready());
    let tracer = assets.heroes().find_hero("Tracer").unwrap().guid;
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
/// naming the shooter's body, and before it his pain line, which the
/// shooter hears too; both clients get frames of the dead body's
/// statescript, which carry its "died" kill feed line (003E); the shooter's
/// Tab board changes; and the victim gets a new body after the Practice
/// Range's 3 s, which the shooter's client gets too, of the hero he picked
/// while dead. The target's body is a hero that can be hit, and its pools
/// of 0 come with its dead bit.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_player_kills_another_who_respawns_after_the_practice_range_delay() {
    use lucy_content::{
        heroes::{ANA, SOLDIER},
        parameters::PICK_HERO,
        stats::GMP_STATS,
    };
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::FIRE;

    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(two_team_range()).unwrap();
    let facing = scene.map().spawn_point(0, 0).unwrap().1;
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
    let heard_from = target.datagrams.len();
    let mut frame = shooter.tick + 1;
    // The target's client goes on sending its commands, which run his
    // body's script (its frames, and with them the waits after a hit).
    let mut idle = target.tick + 1;
    let deadline = server.now() + Duration::from_secs(20);
    while messages_of(target, 25004).is_empty() {
        assert!(server.now() < deadline, "waiting for the kill");
        if frame <= shooter.tick + 8 {
            shooter
                .client
                .send(ACK, command_batch(frame, 16, yaw(&server), FIRE, false));
            frame += 16;
        }
        if idle <= target.tick + 8 {
            target
                .client
                .send(ACK, command_batch(idle, 16, 0, 0, false));
            idle += 16;
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
    // Each hit the target survives reaches his body's script as the damage
    // message (`lucy_sim::combat::damage_message`), and 0.15 s after
    // the first his graph 01CA raises his pain criteria, whose audience
    // (#1839) is his body and the shooter's: before he dies, his client
    // hears his body's pain line (ClientInVoice 27500), said to his body,
    // and the shooter's client hears it said to the shooter's.
    let body_id = |id: u32| Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(id)))]));
    let pain = messages_of(target, 27500)
        .into_iter()
        .find(|(_, voice)| voice[&0x78] == body_id(target_body));
    assert!(
        pain.as_ref()
            .is_some_and(|(index, voice)| (heard_from..died).contains(index)
                && voice[&0x7c] == body_id(target_body)),
        "the target's pain line: {pain:?}"
    );
    let line = pain.unwrap().1[&0x90].clone();
    let heard_by_shooter = |watchers: &[&mut Watcher]| {
        messages_of(watchers[0], 27500)
            .iter()
            .any(|(index, voice)| {
                *index >= fired
                    && voice[&0x78] == body_id(target_body)
                    && voice[&0x7c] == body_id(shooter_body)
                    && voice[&0x90] == line
            })
    };
    pump_until(
        &mut [shooter, target],
        &schemas,
        "the shooter hears the target's pain line",
        heard_by_shooter,
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
    // bits at its create, `lucy_sim::scene` module notes), and each
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
        Some((26, vec![Some(DecodedField::Id(0x0101_0041))]))
    );
    // The shooter's create of it carries its movement state at frame 0,
    // below every record of it that follows, so his client's sphere
    // history of it (`0x7FF789B80F70`) appends them; the target's own
    // create puts it 18 frames past its datagram's frame, ahead of his
    // client's command frames and below the records that follow
    // (`lucy_sim::body::body_create`).
    assert_eq!(create.create_frame, Some(0));
    let own_index = target
        .find(0, |record| {
            record.entity == target_body && record.is_create()
        })
        .unwrap();
    let own = target.datagrams[own_index]
        .1
        .iter()
        .find(|record| record.entity == target_body && record.is_create())
        .unwrap();
    assert_eq!(
        own.create_frame,
        Some(target.ticks[own_index] + lucy_sim::body::OWN_CREATE_LEAD)
    );
    let dead = |watcher: &Watcher| {
        watcher
            .find(0, |record| {
                record.entity == target_body
                    && record.operation == Some(2)
                    && record.components.as_deref().and_then(first_pool) == Some(0.0)
                    && record.components.as_deref().and_then(filter)
                        == Some((26, vec![Some(DecodedField::Id(0x0101_0051))]))
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
    // `STU_4C3509FB`) and the attacker's position (graph 01D8 node 98,
    // `STU_309B0570`), then reaches a state the VM doesn't run (graph 01C7
    // node 68, the aim speed `STU_3716E1D4`), which debug builds refuse
    // and release builds run as a plain state as Python does; so this part
    // runs in release builds.
    let tolerant = lucy_script::vm::Unknowns::for_build() == lucy_script::vm::Unknowns::Tolerate;
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
        dead_flag.w_var(lucy_script::bot::DEAD_FLAG);
        lucy_script::wire::write_value(
            &mut dead_flag,
            &lucy_script::wire::Value::Bool(true),
            lucy_script::wire::ValueEncoding::Compact,
        )
        .unwrap();
        dead_flag.bit(false); // no stack binding
        let raised = shooter.find(0, |record| {
            record.entity == target_body
                && record.script.as_ref().is_some_and(|chunk| {
                    lucy_testkit::contains_bits(
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
    // The respawn comes from the dead body's `0C8E`: it counts #384 down
    // from #9575 = `06FD`'s 3 s (#2153; no overtime #3392e, ruleset scale
    // #9531e 1), a 16 ms frame at a time from the frame that took the
    // death, and sends `0685` on the 188th (187 frames make 2992 ms, 188
    // make 3008). The killing command runs from the shooter's input buffer
    // as tick T is due, before the tick (`InputQueue`): the death notice
    // goes in that tick's datagram, T, and the dead body's frames follow
    // the target's commands, one a tick. The match takes the `0685` among
    // the bodies' messages and respawns him in the same tick: the new body
    // goes in datagram T + 188.
    let waited = respawn_tick - death_tick;
    assert_eq!(waited, 188, "respawned after {waited} ticks");
    assert!(
        target
            .find(died, |record| record.entity == target_body
                && record.operation == Some(3))
            .is_some()
    );
    // The hero picked while dead (Ana) is the respawned body's.
    let definition = |body: u32| {
        target.datagrams[target.created(body).unwrap()]
            .1
            .iter()
            .find(|record| record.entity == body && record.is_create())
            .unwrap()
            .definition
    };
    assert_ne!(definition(respawned), definition(target_body), "Ana's body");
    // Each spawn turns the view to the spawn's facing (25008, the rotation
    // about Y) once the client has the new body's possession: the first
    // spawn before the death, the respawn after its possession
    // (`WorldDelivery::take_spawn_view`). The Practice Range has one spawn,
    // so both face its way.
    let half = f64::from((facing * 65536.0 / 360.0).round_ties_even() as i32)
        * std::f64::consts::PI
        / 65536.0;
    let rotation = Value::Struct(Fields::from([
        (0, Value::Float32(0.0)),
        (4, Value::Float32(half.sin() as f32)),
        (8, Value::Float32(0.0)),
        (12, Value::Float32(half.cos() as f32)),
    ]));
    pump_until(
        &mut [shooter, target],
        &schemas,
        "the respawn's view turn",
        |watchers| messages_of(watchers[1], 25008).len() == 2,
    );
    let views = messages_of(target, 25008);
    assert!(views[0].0 < died, "the first spawn's view turn");
    let possessed = target
        .find(created, |record| {
            record.entity == target_owner && record.operation == Some(2)
        })
        .unwrap();
    assert!(views[1].0 > possessed, "after the respawn's possession");
    for (_, view) in views {
        assert_eq!(view, Fields::from([(0x78, rotation.clone())]));
    }
    assert_eq!(messages_of(shooter, 25008).len(), 1, "the shooter's own");
    server.stop().unwrap();
}

/// The Practice Range's spawn room (`lucy_sim::map_areas`): a player
/// who walks out of it toward the bots gets his player entity's component
/// 74 (field 6, the spawn room flag the client checks before H opens hero
/// select) updated once, and no more while he stays out.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn walking_out_of_the_spawn_room_clears_the_hero_select_flag() {
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use lucy_sim::scene_assets::SceneAssets;

    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let facing = scene.map().spawn_point(0, 0).unwrap().1;
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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

/// The Practice Range for players of both teams. Its mode has one blue
/// team (retail puts no red player there): both its spawn points and its
/// spawn room `0D62` (126) are blue, so a red player at the blue spawn
/// stands in the room as an intruder, and the room's `0B7B` (`0C8E` st45,
/// 400 a second) kills him again after every respawn. The tests that pit
/// two teams' players against each other spawn everyone near the Range's
/// other blue point (placeable 649), in the open outside the room. That
/// point is on the Training Partner's pad `3992`, where the console `265F`
/// puts its Tracer, so they spawn 1.5 m to its side (along (0.8157, 0,
/// 0.5785), square to its facing).
fn two_team_range() -> GameMap {
    GameMap {
        spawns: vec![lucy_content::maps::Spawn {
            team: None,
            position: [35.46356, 1.991726, 23.83998],
            yaw: 144.6566,
        }],
        ..GameMap::practice_range()
    }
}

/// Players (BattleTag, team, hero name) in one prepared Practice Range, once
/// each client has every other body, its possession and, for a body that
/// runs a statescript (Soldier: 76's), its frames. Also where each spawned.
fn practice_players<const N: usize>(
    players: [(&str, u8, &str); N],
) -> (Lockstep, Arc<Schemas>, [Watcher; N], [[f64; 3]; N]) {
    let assets = lucy_sim::scene_assets::SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    practice_world(players, &assets, |_| {})
}

/// [`practice_players`] with these assets, and `arrange` changing the
/// prepared scene first.
fn practice_world<const N: usize>(
    players: [(&str, u8, &str); N],
    assets: &lucy_sim::scene_assets::SceneAssets,
    arrange: impl FnOnce(&mut lucy_sim::scene_assets::PreparedScene),
) -> (Lockstep, Arc<Schemas>, [Watcher; N], [[f64; 3]; N]) {
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};

    let map = if players.iter().any(|&(_, team, _)| team != 0) {
        two_team_range()
    } else {
        GameMap::practice_range()
    };
    let mut scene = assets.prepare(map).unwrap();
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
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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

/// A hero change in the Practice Range and back (the user's report of
/// 2026-11-19, release 385edf5): Soldier: 76 (the first pick), reopen,
/// Tracer, reopen, Soldier: 76. Each pick replaces the body (the old one
/// destroyed), the reopened screen shows the hero he plays (#940e) and no
/// pick, and the first hero is his to pick again. Since the Range's mode
/// `13C0` runs, its ruleset puts the controller's hero limit receiver
/// `0C90` st4 on, and the controller's own `08BE` broadcast reached it
/// (the VM didn't test message filters): his first hero became a
/// teammate's (#10476), and the pick back was refused.
#[test]
#[ignore = "requires the derived 104319 data"]
fn practice_range_hero_switch_and_back() {
    use lucy_content::{
        heroes::SOLDIER,
        parameters::{OPEN_OR_CLOSE, PICK_HERO},
    };
    let assets = lucy_sim::scene_assets::SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let tracer = assets.heroes().find_hero("Tracer").unwrap().guid;
    let (mut server, schemas, [mut player], _) =
        practice_world([("player#1111", 0, "Soldier: 76")], &assets, |_| {});
    let owner = player.owner;
    let mut open = vec![1, 1];
    open.extend(owner.to_le_bytes());
    open.extend(owner.to_le_bytes());
    let mut ordinal = 2;
    // Sends the request, then takes datagrams until `done` holds of those
    // from the first after it on.
    let mut request = |player: &mut Watcher,
                       what: &str,
                       guid: u64,
                       parameters: Vec<u8>,
                       done: &dyn Fn(&Watcher, usize) -> bool| {
        let from = player.datagrams.len();
        send_player_message(
            &mut player.client,
            &schemas,
            owner,
            ordinal,
            guid,
            parameters,
        );
        ordinal += 1;
        let deadline = player.client.now() + Duration::from_secs(2);
        while !done(player, from) {
            assert!(player.client.now() < deadline, "waiting for {what}");
            player.pump(&schemas);
        }
    };
    // A controller frame from `from` on shows (open, pick, #940e).
    let shows = |player: &Watcher, from: usize, shown: (bool, Option<u64>, Option<u64>)| {
        player
            .find(from, |record| {
                record.entity == owner
                    && record
                        .script
                        .as_ref()
                        .and_then(|script| controller_shows(assets.graphs(), script))
                        == Some(shown)
            })
            .is_some()
    };
    // The pick of `hero` from `from` on: body `new` made, `old` destroyed,
    // the screen closed on him.
    let switched = |player: &Watcher, from: usize, hero: u64, old: u32, new: u32| {
        player
            .find(from, |record| record.entity == new && record.is_create())
            .is_some()
            && player
                .find(from, |record| {
                    record.entity == old && record.operation == Some(3)
                })
                .is_some()
            && shows(player, from, (false, Some(hero), Some(hero)))
    };
    let reopened =
        |hero| move |player: &Watcher, from: usize| shows(player, from, (true, None, Some(hero)));
    request(
        &mut player,
        "the reopen",
        OPEN_OR_CLOSE,
        open.clone(),
        &reopened(SOLDIER),
    );
    request(
        &mut player,
        "Tracer",
        PICK_HERO,
        tracer.to_le_bytes().to_vec(),
        &|player, from| switched(player, from, tracer, owner | 1, owner | 2),
    );
    // He plays Tracer: the reopened screen says so.
    request(
        &mut player,
        "the second reopen",
        OPEN_OR_CLOSE,
        open,
        &reopened(tracer),
    );
    request(
        &mut player,
        "Soldier: 76 again",
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
        &|player, from| switched(player, from, SOLDIER, owner | 2, owner | 3),
    );
    server.stop().unwrap();
}

/// The Practice Range's Training Partner console `265F` makes its default
/// partner at its start (`STU_FC3921B1`, `lucy_sim::placed_script`): a
/// Tracer (`0003.075`) on the red team, an AI player in the room's next
/// slot (`ai.rs`), whose body stands still on the console's pad `3992`
/// (34.24, 1.99, 22.97) facing the pad's way, and whose body the player's
/// client gets.
///
/// Then the player walks from his spawn to the console (34.62, 1.99,
/// 17.86) along the map's navigation: within its 1.75 m search his body
/// gets the console's scope 12, whose H button (st9, action byte 42)
/// raises the picker's condition `#25456e` in his owner frames. His client
/// sends the pick as its picker's `STU_79A131B4` does: a 21616 to his
/// controller's scope 126 instance with `1213` {`08E6` Ana, `08E7`
/// true}. The console's chain (`1213` → `1214` on his body → remove red
/// slot 0, make Ana there) destroys the Tracer's body and seats an Ana
/// dummy, the room's next slot (2), on the pad, and the picker closes.
/// The console sends its own frames; the partners' bodies come as
/// another player's (observer frames only).
#[test]
#[ignore = "requires the derived 104319 data"]
fn the_range_console_puts_a_training_partner_on_its_pad_and_a_pick_replaces_it() {
    use lucy_content::heroes::ANA;
    let PartnerPick {
        server,
        schemas,
        mut watcher,
        mut frame,
        yaw,
        ..
    } = pick_training_partner(
        &lucy_sim::scene_assets::SceneAssets::load(lucy_testkit::runtime_dir()).unwrap(),
        ANA,
    );
    let tracer = player_entity(1) | 1;
    let ana = player_entity(2) | 1;
    let bodies = server.bodies(MATCH).unwrap();
    let [me, tracer_seat, partner] = bodies.as_slice() else {
        panic!("{bodies:?}");
    };
    assert_eq!(tracer_seat.position, None, "{tracer_seat:?}");
    assert!(partner.ai);
    assert_eq!((partner.team, partner.hero), (1, Some(ANA)));
    assert!(on_the_pad(partner.position.unwrap()), "{partner:?}");
    // The picker closes (`1214` ends StateStack rid40).
    let deadline = server.now() + Duration::from_secs(2);
    while server.bodies(MATCH).unwrap()[0].picker_raised {
        assert!(server.now() < deadline, "the picker closes: {me:?}");
        send_input(&mut watcher, &mut frame, yaw, 0, 0, false);
        watcher.pump(&schemas);
    }
    // His HUD comes back with it (`265F` st13 false on his controller).
    assert!(!server.bodies(MATCH).unwrap()[0].picker_hides_hud);
    assert!(
        watcher
            .find(0, |record| record.entity == CONSOLE
                && record.script.is_some())
            .is_some(),
        "the console's frames (created {:?}, pad framed {:?}, placed framed {:?})",
        watcher.created(CONSOLE),
        watcher.find(0, |record| record.entity == 0x8000_024A
            && record.script.is_some()),
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| record.entity >> 29 == 4 && record.script.is_some())
            .map(|record| record.entity)
            .collect::<std::collections::BTreeSet<_>>()
    );
    // The partners' bodies are another player's to this client: any of
    // their statescript comes as observer frames (H=0), never as owner
    // frames (two more seconds of datagrams).
    let idle = watcher.datagrams.len();
    for _ in 0..125 {
        send_input(&mut watcher, &mut frame, yaw, 0, 0, false);
        watcher.pump(&schemas);
    }
    let controller_frames = watcher.datagrams[idle..]
        .iter()
        .flat_map(|(_, records)| records)
        .filter(|record| record.entity == watcher.owner)
        .filter_map(|record| record.script.as_ref())
        .filter(|script| script.first == 0)
        .count();
    // His controller's projection holds while he stands: no whole
    // controller frame goes (one goes only when what it carries changed;
    // the owner acks with his command frame go on every 100 ms).
    assert_eq!(controller_frames, 0);
    for (_, records) in &watcher.datagrams {
        for chunk in records
            .iter()
            .filter(|record| [tracer, ana].contains(&record.entity))
            .filter_map(|record| record.script.as_ref())
        {
            if chunk.first == 0 {
                assert_eq!(chunk.bits().bits(2).unwrap(), 0, "an H=0 frame");
            }
        }
    }
    // The swap posts no chat line: the partners are dummy bots, whose
    // cards come without "joined the game" or "left the game".
    let lines: Vec<_> = watcher
        .datagrams
        .iter()
        .flat_map(|(messages, _)| messages)
        .filter(|message| {
            message.id == 20308
                && [0x78, 0x7b]
                    .iter()
                    .all(|offset| message.fields.get(offset) == Some(&Value::Bool(true)))
        })
        .collect();
    assert!(lines.is_empty(), "{lines:?}");
}

/// A Reinhardt picked at the console: his body comes once, with his hero's
/// body definition (`0024.003`) and his script, and the console's scope 197
/// (`265F` st20, on the picker's body, over `STU_00707E1E(#29454)`, the
/// body the new partner's player possesses) opens its instance on it: the
/// 2 s Wait st21 with the spawn effect `1F3F.00D` (st22). The partner's
/// full frame lists that instance and the picker's frame record of st20
/// names it, so the picker's client, which predicts his scope 12, makes
/// none of its own there; it ends after the Wait and never opens again.
/// Before (TrainingPartner.md "DayPartnerRein"), the AI partner's body was
/// found by no player (its controller is his AI entity), st20 listed no
/// target and the instance never opened on the server.
#[test]
#[ignore = "requires the derived 104319 data"]
fn a_reinhardt_training_partner_comes_once_with_the_consoles_spawn_effect() {
    use lucy_core::tick::TICK_PERIOD;
    let assets = lucy_sim::scene_assets::SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let reinhardt = assets.heroes().find_hero("Reinhardt").unwrap().clone();
    let PartnerPick {
        server,
        schemas,
        mut watcher,
        mut frame,
        yaw,
        picked,
    } = pick_training_partner(&assets, reinhardt.guid);
    let partner = player_entity(2) | 1;
    // The Wait's 2 s: the instance opens as the room routes its messages,
    // before the tick's scripts run, on the clock of the tick before the
    // one that first shows it; it ends at the first tick 2 s (125 ticks of
    // 16 ms) after that, 124 ticks after it was first seen.
    let wait = TICK_PERIOD * ((2.0 / TICK_PERIOD.as_secs_f64()).ceil() as u32 - 1);
    let mut opened = None;
    let mut spans = Vec::new();
    // Ten seconds of the partner on the pad, a server tick at a time; the
    // pick's tick had no instance yet.
    assert!(!server.bodies(MATCH).unwrap()[2].spawn_effect);
    for _ in 0..(10 * 1000 / TICK_PERIOD.as_millis()) {
        send_input(&mut watcher, &mut frame, yaw, 0, 0, false);
        watcher.pump(&schemas);
        let bodies = server.bodies(MATCH).unwrap();
        assert!(bodies[2].scripted, "{:?}", bodies[2]);
        match (opened, bodies[2].spawn_effect) {
            (None, true) => opened = Some(server.now()),
            (Some(begin), false) => {
                spans.push((begin, server.now()));
                opened = None;
            }
            _ => (),
        }
    }
    assert_eq!(opened, None, "the spawn effect still runs");
    let [(begin, end)] = spans[..] else {
        panic!("the spawn effect ran {spans:?}");
    };
    assert_eq!(end - begin, wait);
    // His record of st20 names the instance on the partner's body.
    let bodies = server.bodies(MATCH).unwrap();
    let [(target, Some(instance))] = bodies[0].spawn_scope[..] else {
        panic!("{:?}", bodies[0].spawn_scope);
    };
    assert_eq!(target, partner);
    // One create, of his body's definition, then one full frame: his
    // graphs and the console's scope instance.
    let records: Vec<&WorldRecord> = watcher.datagrams[picked..]
        .iter()
        .flat_map(|(_, records)| records)
        .filter(|record| record.entity == partner)
        .collect();
    let creates: Vec<_> = records.iter().filter(|record| record.is_create()).collect();
    let [create] = creates[..] else {
        panic!("{} creates", creates.len());
    };
    assert_eq!(
        create.definition.map(u64::from),
        Some(reinhardt.body & 0xffff_ffff)
    );
    let fulls: Vec<_> = records
        .iter()
        .filter_map(|record| record.script.as_ref())
        .filter(|chunk| chunk.first == 0)
        .collect();
    let [full] = fulls[..] else {
        panic!("{} full frames", fulls.len());
    };
    let head = lucy_script::wire::read_full_head(&full.data, |graph| {
        assets
            .graphs()
            .graph(u32::from(graph))
            .map(|graph| graph.header.states_bits)
    })
    .unwrap();
    // Reinhardt's first graph `0033`, his Charge `007D`.
    assert_eq!(head.instances[0].1, 0x0033);
    assert_eq!(head.instances[1].1, 0x007d);
    let scoped: Vec<u16> = head
        .instances
        .iter()
        .filter(|&&(_, graph, ..)| graph == 0x265f)
        .map(|&(id, ..)| id)
        .collect();
    assert_eq!(scoped, [instance]);
}

/// The Range's Training Partner console (`265F`).
const CONSOLE: u32 = 0x8000_010B;

/// A partner's feet on the console's pad `3992` (placed at (34.24, 1.99,
/// 22.97), scale 0.6): x and z within 1 mm of its placement, y within
/// 0.1 m of its top. The pad's mesh (category 1, mask 0x119A: both teams'
/// movers) rises 0.6 × 0.2505 m over its placement, and the mover's ground
/// ray stands a body on it as the client's does (refs `MapEntities.md`
/// "RangeConsole"): the partner's feet rest at 2.1416 m.
fn on_the_pad(feet: [f64; 3]) -> bool {
    const PAD: [f64; 3] = [34.24001, 1.9917264, 22.972233];
    const TOP: f64 = 1.9917264 + 0.6 * 0.2504736;
    (feet[0] - PAD[0]).abs() < 1e-3
        && (feet[2] - PAD[2]).abs() < 1e-3
        && (feet[1] - TOP).abs() < 0.1
}

/// A Range whose player picked a Training Partner at the console
/// ([`pick_training_partner`]).
struct PartnerPick {
    server: Lockstep,
    schemas: Arc<Schemas>,
    watcher: Watcher,
    /// His next command frame, and the yaw he stands at.
    frame: u32,
    yaw: i16,
    /// The datagram that brought the new partner's create.
    picked: usize,
}

/// A Range with one Soldier: 76, whose Tracer partner (room slot 1)
/// stands on the pad; he walks to the console, presses H and picks `hero`
/// as the picker does, until the Tracer's body is gone and the new
/// partner's body (room slot 2) arrived.
fn pick_training_partner(assets: &lucy_sim::scene_assets::SceneAssets, hero: u64) -> PartnerPick {
    const TRACER: u64 = 0x02E0_0000_0000_0003;
    const CONSOLE_AT: [f32; 3] = [34.62, 1.99, 17.86];
    const PICK: u64 = 0x0240_0000_0000_1213;
    let spawn = GameMap::practice_range().spawn_point(0, 0).unwrap().0;
    // The walk to the console: the server's own path query, its legs
    // checked with Soldier: 76's standing capsule on the map's collision
    // (`lucy_sim::navigate`), as an AI hero's are.
    let mut walk = None;
    let (server, schemas, [mut watcher], _) =
        practice_world([("player#1111", 0, "Soldier: 76")], assets, |scene| {
            let body = scene
                .heroes()
                .get(lucy_content::heroes::SOLDIER)
                .unwrap()
                .body;
            let to = CONSOLE_AT.map(f64::from);
            let path = lucy_sim::navigate::path(
                scene.navigation(),
                spawn,
                to,
                scene.walk_test(Some(body), 0),
            )
            .unwrap();
            walk = Some(
                std::iter::once(spawn)
                    .chain(path)
                    .map(|point| point.map(|axis| axis as f32))
                    .collect::<Vec<_>>(),
            );
        });
    let waypoints = walk.unwrap();
    let tracer = player_entity(1) | 1;
    let partner = player_entity(2) | 1;
    pump_until(
        &mut [&mut watcher],
        &schemas,
        "the Training Partner's body",
        |watchers| watchers[0].created(tracer).is_some(),
    );
    let bodies = server.bodies(MATCH).unwrap();
    let [_, dummy] = bodies.as_slice() else {
        panic!("{bodies:?}");
    };
    assert!(dummy.ai);
    assert_eq!((dummy.team, dummy.hero), (1, Some(TRACER)));
    assert!(on_the_pad(dummy.position.unwrap()), "{dummy:?}");
    // The walk: towards the next waypoint not yet reached, until his feet
    // are within a metre of the console's (horizontally).
    let flat = |feet: [f64; 3], point: [f32; 3]| {
        (feet[0] - f64::from(point[0])).hypot(feet[2] - f64::from(point[2]))
    };
    let yaw_to = |feet: [f64; 3], point: [f32; 3]| {
        let degrees = (f64::from(point[0]) - feet[0])
            .atan2(f64::from(point[2]) - feet[2])
            .to_degrees();
        (degrees * 65536.0 / 360.0).round() as i32 as i16
    };
    let mut frame = watcher.tick + 1;
    let mut next = 1;
    let deadline = server.now() + Duration::from_secs(30);
    let yaw = loop {
        let feet = server.bodies(MATCH).unwrap()[0].position.unwrap();
        while next + 1 < waypoints.len() && flat(feet, waypoints[next]) < 0.5 {
            next += 1;
        }
        let yaw = yaw_to(feet, waypoints[next]);
        if flat(feet, CONSOLE_AT) < 1.0 {
            break yaw;
        }
        assert!(
            server.now() < deadline,
            "walking to the console: {feet:?}, waypoint {next} of {waypoints:?}"
        );
        send_input(&mut watcher, &mut frame, yaw, 0, 0, true);
        watcher.pump(&schemas);
    };
    // He stands; his owner frames carry the console's scope 12 instance.
    let deadline = server.now() + Duration::from_secs(5);
    while !server.bodies(MATCH).unwrap()[0].console_button {
        assert!(server.now() < deadline, "the console's scope on his body");
        send_input(&mut watcher, &mut frame, yaw, 0, 0, false);
        watcher.pump(&schemas);
    }
    // H: one command with the hero select key's action byte.
    watcher
        .client
        .send(ACK, acting_batch(frame, 1, yaw, 0, 0, false, 42));
    frame += 1;
    let deadline = server.now() + Duration::from_secs(3);
    while !server.bodies(MATCH).unwrap()[0].picker_raised {
        assert!(server.now() < deadline, "the picker's condition");
        send_input(&mut watcher, &mut frame, yaw, 0, 0, false);
        watcher.pump(&schemas);
    }
    // His controller's scope 126 instance follows his body's `#25456e`
    // (st13 reads it through `STU_00707E1E`) and his controller frame
    // carries it: st33, whose client-only st12 hides his HUD while the
    // picker is open, which his client's rollbacks restore from that frame.
    assert!(server.bodies(MATCH).unwrap()[0].picker_hides_hud);
    // The pick, from his controller's scope 126 instance: `08E6` an array
    // of the hero, `08E7` an array of true, then two entities
    // (`lucy_content::parameters` `parameter_rows_read_as_the_client_writes_them`).
    let picker = server.bodies(MATCH).unwrap()[0].picker.unwrap();
    let owner = watcher.owner;
    let mut blob = vec![0x00, 0x00, 0x00, 0x00, 0x02];
    blob.extend([0xc3, 0xe6, 0x08, 0x40, 0x0a, 0x0c, 0x01, 0xff, 0x04]);
    blob.extend(hero.to_le_bytes());
    blob.extend([0xc3, 0xe7, 0x08, 0x40, 0x0a, 0x0c, 0x01, 0xff, 0x01, 0x01]);
    blob.extend(owner.to_le_bytes());
    blob.extend(owner.to_le_bytes());
    send_instance_message(
        &mut watcher.client,
        &schemas,
        owner,
        2,
        PICK,
        i64::from(picker),
        blob,
    );
    let deadline = server.now() + Duration::from_secs(5);
    while watcher.created(partner).is_none()
        || watcher
            .find(0, |record| {
                record.entity == tracer && record.operation == Some(3)
            })
            .is_none()
    {
        assert!(
            server.now() < deadline,
            "the Tracer's destroy, the new partner's create"
        );
        send_input(&mut watcher, &mut frame, yaw, 0, 0, false);
        watcher.pump(&schemas);
    }
    let picked = watcher.created(partner).unwrap();
    PartnerPick {
        server,
        schemas,
        watcher,
        frame,
        yaw,
        picked,
    }
}

/// Team 0's first player (slot 0 of the room) runs out of his spawn room
/// the way the first spawn point faces, as a player leaves it: retail
/// spawn rooms keep him invulnerable (`lucy_sim::map_areas`). He runs
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
/// owner frames. The client sends its list in parts of at most 4 entries
/// (`+0x90` the first, `+0x91` the last; `lucy_content::hero_settings`),
/// here 5: ALLIED HEALTH BARS at ALWAYS and three other heroes' settings,
/// then Soldier: 76's HOLD TO SPRINT on. The whole list counts: with ALLIED
/// HEALTH BARS at ALWAYS, Soldier: 76's body graph `01CF` gives him the
/// support tag (st117) that `0D24` asks of the viewer for an ally's health
/// bar, and the client acknowledges a frame with st117 and `#8336e`, with
/// `#7644e` (HOLD TO SPRINT) true, and its body's target tags (component 76)
/// with `0042` added, which the client takes its tags from (the tag state
/// has no client code); back at DEFAULT the client sends an empty list (one
/// 21604, first and last; seen in the game) and the tag and the variables
/// go back, in his frames and his component 76 too.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_clients_hero_settings_list_reaches_his_body() {
    use lucy_content::hero_settings::{
        ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS, HERO_SETTINGS,
    };
    use lucy_content::heroes::{ANA, SOLDIER};
    use lucy_script::wire::Value as ScriptValue;

    const HOLD_TO_SPRINT: u64 = 0x0d80_0000_0000_1ddc;
    let (mut server, schemas, [mut soldier], _) =
        practice_players([("soldier#1111", 0, "Soldier: 76")]);
    assert!(!server.bodies(MATCH).unwrap()[0].support_tag_given);
    // practice_world sent records 0 (map loaded) and 1 (the pick).
    let send = |watcher: &mut Watcher, parts: &[(u32, Vec<Value>, bool, bool)]| {
        let records: Vec<_> = parts
            .iter()
            .map(|(ordinal, entries, first, last)| {
                let settings = Fields::from([
                    (0x78, Value::Array(entries.clone())),
                    (0x90, Value::Bool(*first)),
                    (0x91, Value::Bool(*last)),
                ]);
                (
                    *ordinal,
                    encode(&schemas, HERO_SETTINGS, &settings).unwrap(),
                )
            })
            .collect();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &records, &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    };
    // His body's script runs, and its owner frames go out, on his commands.
    let body = soldier.owner | 1;
    let mut next = soldier.tick + 1;
    let mut wait_for = |watcher: &mut Watcher, given: bool, variable: ScriptValue| {
        let deadline = server.now() + Duration::from_secs(15);
        let from = watcher.datagrams.len();
        // His body's own tag states add other tags too (0x422 from the
        // start): `0042` is in the added list exactly while ALWAYS holds,
        // and nothing is suppressed.
        let tags = |components: &[DecodedComponent]| match components {
            [(76, fields)] => match fields.as_slice() {
                [
                    Some(DecodedField::U32s(added)),
                    Some(DecodedField::U32s(suppressed)),
                ] => added.contains(&0x42) == given && suppressed.is_empty(),
                _ => false,
            },
            _ => false,
        };
        loop {
            let state = server.bodies(MATCH).unwrap().remove(0);
            if state.support_tag_given == given
                && state.support_tag_delivered == (given, variable.clone())
                && state.hold_to_sprint_delivered == ScriptValue::Bool(given)
                && watcher
                    .find(from, |record| {
                        record.entity == body && record.components.as_deref().is_some_and(tags)
                    })
                    .is_some()
            {
                break;
            }
            assert!(server.now() < deadline, "the tag is not {given}: {state:?}");
            if next <= watcher.tick + 8 {
                watcher
                    .client
                    .send(ACK, command_batch(next, 16, 0, 0, false));
                next += 16;
            }
            watcher.pump(&schemas);
        }
    };
    let entry = |hero, setting, value| {
        Value::Struct(Fields::from([
            (0, Value::Unsigned(hero)),
            (8, Value::Unsigned(setting)),
            (0x10, Value::Unsigned(value)),
        ]))
    };
    let first = vec![
        entry(0, ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS),
        // NANO BOOST SENSITIVITY, TOGGLE GUARDIAN ANGEL off, ORB OF HARMONY
        // SENSITIVITY: other heroes', not on his body.
        entry(ANA, 0x0d80_0000_0000_2582, 0.5f64.to_bits()),
        entry(0x02e0_0000_0000_0004, 0x0d80_0000_0000_1e64, 0),
        entry(
            0x02e0_0000_0000_0020,
            0x0d80_0000_0000_257c,
            0.75f64.to_bits(),
        ),
    ];
    let last = vec![entry(SOLDIER, HOLD_TO_SPRINT, 1)];
    send(
        &mut soldier,
        &[(2, first, true, false), (3, last, false, true)],
    );
    wait_for(
        &mut soldier,
        true,
        ScriptValue::Asset(ALLIED_HEALTH_BARS_ALWAYS),
    );
    send(&mut soldier, &[(4, Vec::new(), true, true)]);
    wait_for(&mut soldier, false, ScriptValue::Null);
    server.stop().unwrap();
}

/// Whether a body script frame carries a short remote message entry for
/// `node` (`bits` wide) of any instance (104319 A0CCA0; `SnapshotEncoder`
/// frame actions): its present bit, the short form, the 8-bit length, the
/// target bit, the instance and the node. Since every networked action
/// goes in the frames, a group holds the other actions that ran with it
/// too, so the entry need not be the frame's last.
fn carries_frame_action(chunk: &ReceivedChunk, node: u16, bits: u8) -> bool {
    let bit = |data: &[u8], index: usize| data[index / 8] >> (index % 8) & 1;
    (1..=512).any(|instance| {
        let mut entry = BitWriter::new();
        entry.bit(true);
        entry.w_u16(instance);
        entry.bits(u64::from(node), usize::from(bits)).unwrap();
        let mut pattern = BitWriter::new();
        pattern.bit(true);
        pattern.bit(false);
        pattern.bits(8 + entry.bit_len() as u64, 8).unwrap();
        pattern.append(&entry);
        let length = pattern.bit_len();
        chunk.width > length
            && (0..chunk.width - length).any(|start| {
                (0..length)
                    .all(|index| bit(&chunk.data, start + index) == bit(pattern.as_bytes(), index))
            })
    })
}

/// A player's communication wheel choice (21608, Need Healing) reaches his
/// body as `02A3`; `05AD` plays `0686`, whose effect 40 goes to his teammate
/// in his body's observer frames (remote index 8) and to his own client in
/// its owner frames (raw index 36), each as the frame's remote message
/// (`lucy_sim::wheel`).
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_need_healing_callout_reaches_a_teammate() {
    let graphs = lucy_script::graph::GraphStore::load_runtime(
        lucy_testkit::runtime_dir().join("data"),
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
    let message = encode(&schemas, lucy_sim::wheel::COMMUNICATION_ACTION, &choice).unwrap();
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
                        .is_some_and(|chunk| carries_frame_action(chunk, node, bits))
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
    use lucy_sim::projectiles::{SEEN_FIRST, SEEN_SPAN};
    use lucy_wire::commands::SECONDARY;
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
/// scale 0.5 = 40 of his 200, and within the 0.5 m knockback ring the
/// client's push (`Knockback::push`, `0x7FF789C7B3C0`): 4 m/s back along the
/// ground and the line's vertical part × 6 m/s up, times the self-push
/// scale 1. His server mover flies it as a jump: more than 0.5 m up, at
/// most 6² / 2g = 36/35 m (the game, 2026-11-23: about 1 m, as his client
/// predicts it; before the client's formula, 4 + 6 m/s gave over 2 m), and
/// he lands behind where he stood, away from
/// the burst. He gets no hit notice (25001) for it. He fires outside his
/// spawn room, where he is not invulnerable.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_helix_at_soldiers_feet_hurts_him_and_lifts_him() {
    use lucy_wire::commands::SECONDARY;
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
        top - floor > 0.5 && top - floor <= 36.0 / 35.0 + 0.05,
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
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    const PULSE_BOMB: u32 = 0x0045;
    /// Q, the ultimate's button.
    const ULTIMATE: u32 = 0x20;
    const AHEAD: f64 = 3.3;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    // Her 1,260-point ultimate fills in a second: `0B8B`'s passive loop
    // adds 5 points times "Ultimate Generation - Passive" after its 1 s Wait.
    assets.set_ult_generation(lucy_script::wire::Variables::from([(
        lucy_script::ultimate::PASSIVE_GENERATION,
        lucy_script::wire::Value::Float(252.0),
    )]));
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
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::SECONDARY;
    const HELIX: u32 = 0x072c;
    const AHEAD: f64 = 8.0;
    const APART: f64 = 30.0;

    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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

/// Genji's fan (graph `0304` state 11, his secondary fire: three shuriken
/// a shot, sync NONE, launch `STU_1145C42E`): the server flies each
/// shuriken along the client's fan (`weapon_spread::fan_directions`:
/// level, 4.5° to either side of the aim and along it) for its damage
/// alone, so a training bot 3.3 m ahead takes three hits of 29 (variable
/// `01DC`, which node 79 sets) a shot, where the one shuriken along the
/// aim hit once before the fan layout. Nobody sees a server projectile:
/// Genji's client gets no confirmation of a shuriken (definition `07CA`)
/// and the other client no copy or shot record.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn genjis_fan_hits_a_bot_with_each_shuriken_for_its_damage_alone_unseen() {
    genjis_fan_hits(3.3, 0.0, 3);
}

/// A side shuriken of Genji's fan hits on its own: aimed 4.5° beside a
/// training bot 14 m ahead, the shuriken on the bot's side flies straight
/// at it, while the middle one passes 1.1 m from its axis, beyond its arms
/// and shoulders (0.64 m and 0.8 m out; at 8 m the middle one, 0.63 m
/// off, struck an arm), and the far one 2.2 m: one hit of 29.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn genjis_side_shuriken_hits_a_bot_beside_the_aim() {
    genjis_fan_hits(14.0, 4.5, 1);
}

/// One fan of Genji's at a hostile training bot `ahead` metres along his
/// spawn's facing, aimed `turn` degrees beside it: the bot takes
/// `hits_wanted` hits of 29, and no client sees a shuriken.
fn genjis_fan_hits(ahead: f64, turn: f64, hits_wanted: usize) {
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    use lucy_wire::commands::SECONDARY;
    const SHURIKEN: u32 = 0x07ca;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
            bot.position = [x + ahead * sin, y, z + ahead * cos];
            aim = Some((
                bot.entity,
                ((yaw + turn) * 65536.0 / 360.0).round() as i32 as i16,
            ));
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
    // The rest of the shot's shuriken land, or miss, within a second.
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
        vec![(Value::Unsigned(u64::from(bot)), Value::Unsigned(29)); hits_wanted],
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
    every_pellet_hits("Doomfist", 11, 6);
}

/// Reaper's Hellfire Shotguns (graph `0019` node 10, hitscan: 20 pellets
/// in rings, 6 damage each): a ray per pellet, each hit its own 25001
/// (upstream kjs `1e2e10c3`, `1dc5b995`: "20 pellets of 6" at 3 m), so a
/// training bot 2 m ahead takes twenty hits of 6 from one shot.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn reapers_shotgun_hits_with_every_hitscan_pellet() {
    every_pellet_hits("Reaper", 20, 6);
}

fn every_pellet_hits(hero: &str, pellets: usize, damage: u64) {
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    use lucy_wire::commands::FIRE;
    const AHEAD: f64 = 2.0;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    let mut aim = None;
    let (mut server, schemas, [mut doomfist, mut viewer], _) = practice_world(
        [
            ("doomfist#1111", 0, hero),
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
    while hits_of(&doomfist, fired.0).len() < pellets {
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

    // Every pellet hits the bot; one of a wide ring may strike its head
    // for double damage (Reaper's 6° rings reach it at 2 m).
    let hits = hits_of(&doomfist, fired.0);
    assert_eq!(hits.len(), pellets);
    for (target, amount) in hits {
        assert_eq!(target, Value::Unsigned(u64::from(bot)));
        assert!(
            [damage, 2 * damage].contains(&match amount {
                Value::Unsigned(amount) => amount,
                other => panic!("damage {other:?}"),
            }),
            "pellet damage"
        );
    }
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
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };
    use lucy_wire::commands::FIRE;
    const BOLT: u32 = 0x2cbf;
    const AHEAD: f64 = 3.3;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
/// push them apart (one of them about 1 m sideways: which one depends on
/// the spawn's neighbours, such as the Training Partner on his pad), so
/// each aims from where he then stands: the Soldier at the bot, Ana at him
/// until a dart strikes him (the enemy), then at the bot. Ana's client
/// gets her own body's state, as Soldier: 76's does (owner corrections,
/// `replication.rs` `CORRECTED_HEROES`).
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn anas_rifle_heals_a_hurt_friendly_bot_and_damages_an_enemy() {
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::FIRE;
    const AHEAD: f64 = 8.0;

    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
    let facing = |from: [f64; 3], to: [f64; 3]| {
        let degrees = (to[0] - from[0]).atan2(to[2] - from[2]).to_degrees();
        (degrees * 65536.0 / 360.0).round() as i32 as i16
    };
    let deadline = server.now() + Duration::from_secs(10);
    while !bot_health(&ana).is_some_and(|health| health < full) {
        assert!(server.now() < deadline, "waiting for the bot's wound");
        let soldier_at = server.bodies(MATCH).unwrap()[1].position.unwrap();
        command(
            &mut soldier,
            &mut frames[1],
            facing(soldier_at, bot_at),
            FIRE,
        );
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

/// Ana's Biotic Grenade (ABILITY_2: `0B97` st0) through her real body
/// script, which starts whole in debug builds (`STU_D1BF5FA6`). Her client
/// doesn't shoot the volley itself (`m_45271E8C` 0), so the server flies it
/// and shows it to her own client as to the others
/// (`lucy_sim::projectiles::ServerCopies`): the copy's create
/// (`1426.003`), then the shot record naming it on her body, then its
/// destroy when the server's flight bursts. She throws it at the enemy
/// Soldier: 76 beside her once he has wounded her, so its 4 m ring takes
/// them both: the aura (`053:0267`, graph `0C71`) plays the enemy aura
/// `0C72` on his body (60 from Ana) and the ally aura `0B99` on hers
/// (a heal of 100), each on its body's own graphs. His world answers
/// `0C72` rid21's `STU_F7262B36(#40)` (Ana's body's possessor: her
/// controller, `lucy_script::vm::listed_relation`); unanswered, it
/// stopped his whole script.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn anas_biotic_grenade_shows_to_her_own_client_and_its_aura_hurts_an_enemy_and_heals_her() {
    use lucy_wire::commands::FIRE;
    /// `0B97` st4's button (36).
    const ABILITY_2: u32 = 0x10;
    const GRENADE: u32 = 0x1426;
    const FULL: f32 = 200.0;

    let (mut server, schemas, [mut ana, mut soldier], _) =
        practice_players([("ana#1111", 0, "Ana"), ("soldier#2222", 1, "Soldier: 76")]);
    let ana_body = ana.owner | 1;
    let soldier_body = soldier.owner | 1;
    let health = |watcher: &Watcher, body: u32| {
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| record.entity == body)
            .filter_map(|record| record.base_health)
            .next_back()
    };
    // The way from one body to the other (bodies: Ana's, the Soldier's).
    let aim = |server: &Lockstep, from: usize, to: usize| {
        let bodies = server.bodies(MATCH).unwrap();
        let [from, to] = [from, to].map(|index| bodies[index].position.unwrap());
        ((to[0] - from[0]).atan2(to[2] - from[2]) * 32768.0 / std::f64::consts::PI).round() as i32
            as i16
    };
    let mut frames = [ana.tick + 1, soldier.tick + 1];
    // Both play on (a body's graphs run at its client's command frames).
    let idle = |watchers: [&mut Watcher; 2], frames: &mut [u32; 2], yaw: i16, pumps: usize| {
        let [ana, soldier] = watchers;
        for _ in 0..pumps {
            send_input(ana, &mut frames[0], yaw, 0, 0, false);
            send_input(soldier, &mut frames[1], 0, 0, 0, false);
            ana.pump(&schemas);
            soldier.pump(&schemas);
        }
    };

    // The Soldier wounds Ana, aiming from where the two bodies stand (as
    // enemies on one spawn point they push each other apart).
    let deadline = server.now() + Duration::from_secs(10);
    while !health(&ana, ana_body).is_some_and(|health| health < FULL) {
        assert!(server.now() < deadline, "waiting for Ana's wound");
        let toward = aim(&server, 1, 0);
        send_input(&mut soldier, &mut frames[1], toward, 0, FIRE, false);
        send_input(&mut ana, &mut frames[0], 0, 0, 0, false);
        ana.pump(&schemas);
        soldier.pump(&schemas);
    }
    idle([&mut ana, &mut soldier], &mut frames, 0, 30);
    let wounded = health(&ana, ana_body).unwrap();
    let unhurt = health(&soldier, soldier_body).unwrap_or(FULL);

    // Ana throws at him.
    let thrown = [&ana, &soldier].map(|watcher| watcher.datagrams.len());
    let toward = aim(&server, 0, 1);
    send_input(&mut ana, &mut frames[0], toward, 0, ABILITY_2, false);
    let grenade = |watcher: &Watcher, from: usize| {
        watcher.datagrams[from..]
            .iter()
            .flat_map(|(_, records)| records)
            .find(|record| record.is_create() && record.definition == Some(GRENADE))
            .map(|record| record.entity)
    };
    let gone = |watcher: &Watcher, from: usize| {
        grenade(watcher, from).is_some_and(|entity| {
            watcher
                .find(from, |record| {
                    record.entity == entity && record.operation == Some(3)
                })
                .is_some()
        })
    };
    let deadline = server.now() + Duration::from_secs(10);
    while !(gone(&ana, thrown[0]) && gone(&soldier, thrown[1])) {
        assert!(server.now() < deadline, "waiting for the grenade's burst");
        idle([&mut ana, &mut soldier], &mut frames, toward, 1);
    }
    idle([&mut ana, &mut soldier], &mut frames, toward, 30);

    // Her own client: the create, the launch on her body, the destroy.
    let copy = grenade(&ana, thrown[0]).unwrap();
    let created = ana
        .find(thrown[0], |record| {
            record.entity == copy && record.is_create()
        })
        .unwrap();
    let launched = ana
        .find(created, |record| {
            record.entity == ana_body && record.launches.contains(&copy)
        })
        .expect("the shot record naming her copy");
    let destroyed = ana
        .find(created, |record| {
            record.entity == copy && record.operation == Some(3)
        })
        .unwrap();
    assert!(created < launched && launched <= destroyed);
    // The aura on both bodies' graphs.
    assert_eq!(health(&soldier, soldier_body), Some(unhurt - 60.0));
    assert_eq!(health(&ana, ana_body), Some((wounded + 100.0).min(FULL)));
    server.stop().unwrap();
}

/// Soldier: 76's Biotic Field (ABILITY_2: 0257) heals a wounded teammate
/// standing in it, the healer's Tab board counts the healing, and every
/// client gets the field entity (03DC.003), its frame (graph `0258`'s, run
/// on the server, `lucy_sim::spawned`) once the create arrived, and its
/// destroy when the field removes itself. The heal is the aura `025F`'s
/// graph `0B3C` on the patient's own body, so the patient is a hero whose
/// body script runs: another Soldier: 76.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_biotic_field_heals_a_wounded_teammate_and_every_client_sees_it() {
    use lucy_content::stats::GMP_STATS;
    use lucy_sim::projectiles::{FIELD_FIRST, FIELD_SPAN};
    use lucy_wire::commands::FIRE;
    /// tests/test_game_projectiles.py `ABILITY_2`: 0257 st0's button.
    const ABILITY_2: u32 = 0x10;
    const FIELD: u32 = 0x03dc;
    /// `0258` st4 removes the field 5 s after it opened; a frame runs the
    /// events due in its 16 ms, so 5000 ms falls in frame 312 (4992-5007
    /// ms after the opening frame's start).
    const FIELD_TICKS: u32 = 312;

    // The enemy and the patient share the spawn point; the healer stands
    // 1.5 m beside it.
    let (mut server, schemas, [mut enemy, mut patient, mut healer], _) = practice_players([
        ("enemy#1111", 0, "Soldier: 76"),
        ("patient#2222", 1, "Soldier: 76"),
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
    let full = 200.0;
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
    // The healer and the patient both play on: a body's graphs (the aura's
    // heal `0B3C` among them) run at its client's command frames.
    let mut patient_next = patient.tick + 1;
    let deadline = server.now() + Duration::from_secs(15);
    loop {
        let watchers = [&enemy, &patient, &healer];
        let done = watchers.iter().zip(cast).all(|(watcher, from)| {
            let fields = created(watcher, from);
            !fields.is_empty() && fields.iter().all(|&entity| destroyed(watcher, entity))
        });
        if done {
            break;
        }
        assert!(server.now() < deadline, "waiting for the fields' destroys");
        for (watcher, next) in [(&mut healer, &mut next), (&mut patient, &mut patient_next)] {
            if *next <= watcher.tick + 8 {
                watcher
                    .client
                    .send(ACK, command_batch(*next, 16, 0, 0, false));
                *next += 16;
            }
        }
        for watcher in [&mut enemy, &mut patient, &mut healer] {
            watcher.pump(&schemas);
        }
    }
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
                // The field opens at its start tick S (the cast's first
                // update + FIELD_DELAY), where the room spawns it and queues
                // its create; its graph removes it in tick S + FIELD_TICKS,
                // which queues the destroy. Each goes in its own tick's
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

/// The health records of `entity` a client got from datagram `from` on,
/// each with its datagram's tick.
fn health_records(watcher: &Watcher, entity: u32, from: usize) -> Vec<(u32, f32)> {
    watcher.datagrams[from..]
        .iter()
        .zip(&watcher.ticks[from..])
        .flat_map(|((_, records), &tick)| {
            records
                .iter()
                .filter(move |record| record.entity == entity)
                .filter_map(move |record| Some((tick, record.base_health?)))
        })
        .collect()
}

/// Moira throws a Biotic Orb (E, then a fire button: `10E4` rid18/rid19)
/// with her view at `pitch` (1/65536 turns, positive down), every command
/// at that view; then she (and `others`, whose graphs run at their own
/// commands) plays on until her client saw the copy of the orb of
/// `definition` go. Returns the datagram of hers the throw went with, and
/// the copy.
fn throw_biotic_orb(
    server: &Lockstep,
    schemas: &Schemas,
    moira: &mut Watcher,
    others: &mut [&mut Watcher],
    (pitch, button, definition): (i16, u32, u32),
) -> (usize, u32) {
    /// `10E4`'s button (E).
    const ABILITY_2: u32 = 0x10;
    let mut frames: Vec<u32> = others.iter().map(|watcher| watcher.tick + 1).collect();
    let mut moira_frame = moira.tick + 1;
    let mut play = |moira: &mut Watcher, others: &mut [&mut Watcher], buttons| {
        send_input(moira, &mut moira_frame, 0, pitch, buttons, false);
        for (watcher, frame) in others.iter_mut().zip(&mut frames) {
            send_input(watcher, frame, 0, 0, 0, false);
        }
        moira.pump(schemas);
        for watcher in others.iter_mut() {
            watcher.pump(schemas);
        }
    };
    for buttons in [0; 10].into_iter().chain([ABILITY_2]).chain([0; 20]) {
        play(moira, others, buttons);
    }
    let thrown = moira.datagrams.len();
    play(moira, others, button);
    let copy = |moira: &Watcher| {
        moira.datagrams[thrown..]
            .iter()
            .flat_map(|(_, records)| records)
            .find(|record| record.is_create() && record.definition == Some(definition))
            .map(|record| record.entity)
    };
    let deadline = server.now() + Duration::from_secs(12);
    while !copy(moira).is_some_and(|entity| {
        moira
            .find(thrown, |record| {
                record.entity == entity && record.operation == Some(3)
            })
            .is_some()
    }) {
        assert!(server.now() < deadline, "waiting for the orb to go");
        play(moira, others, 0);
    }
    for _ in 0..30 {
        play(moira, others, 0);
    }
    (thrown, copy(moira).unwrap())
}

/// The copy of Moira's orb on her client: its create, then its frame (the
/// orb's graph on the server, `lucy_sim::spawned`, whose remote speed
/// state `STU_D3F71FCF` the client begins only from it), then its destroy.
fn assert_orb_copy_framed(moira: &Watcher, thrown: usize, copy: u32) {
    let created = moira
        .find(thrown, |record| record.entity == copy && record.is_create())
        .unwrap();
    let framed = moira
        .find(created, |record| {
            record.entity == copy && record.script.is_some()
        })
        .expect("the orb's frame on its copy");
    let gone = moira
        .find(created, |record| {
            record.entity == copy && record.operation == Some(3)
        })
        .unwrap();
    assert!(
        created < framed && framed <= gone,
        "{created} {framed} {gone}"
    );
}

/// Moira's damaging Biotic Orb (E, then the secondary fire: `10E4` rid19,
/// `1E61.003`), thrown straight up in the Practice Range's spawn room
/// between two enemy Training Bots 1.5 m to either side of her. The
/// server flies it (`lucy_sim::spawned`) between the room's ceiling,
/// about 10 m up, and its floor (99 mirror bounces), slowed to 0.275 while
/// it has targets; its graph `1100` puts its aura `02D5` on both bots
/// while they are within 4 m of it, and their own scripts run the aura's
/// `1101` (from the bot bundle), which hurts each by 50 a second for
/// Moira. What they take comes back to the orb as its damage dealt, and
/// once its pool of 200 is spent it removes itself, and its copy on her
/// client goes. Each bot loses 0.8 a tick while it is a target. The two
/// shares of the tick that fills the pool (250 shares, 125 a bot) reach
/// the orb as messages the next tick (`lucy_sim::messages` routes them
/// once a tick), when the bots take one more share each before the orb's
/// removal takes its auras off: 126 shares, 100.8 a bot.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn moiras_damaging_orb_hurts_two_bots_at_its_rate_until_its_pool_is_spent() {
    use lucy_script::vm::Unknowns;
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::SECONDARY;
    /// Straight up; the mover holds the view at 89°.
    const UP: i16 = -16384;
    /// 50 a second over a 16 ms tick.
    const TICK_DAMAGE: f32 = 0.8;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    assets.set_bot_unknowns(Unknowns::Tolerate);
    let mut targets = Vec::new();
    let (mut server, schemas, [mut moira], _) =
        practice_world([("moira#1111", 0, "Moira")], &assets, |scene| {
            let ([x, y, z], _) = scene.map().spawn_point(0, 0).unwrap();
            let standing: Vec<usize> = scene
                .bots()
                .iter()
                .enumerate()
                .filter(|(_, bot)| {
                    bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty()
                })
                .map(|(slot, _)| slot)
                .take(2)
                .collect();
            assert_eq!(standing.len(), 2, "two standing enemy bots");
            for (slot, side) in standing.into_iter().zip([-1.5, 1.5]) {
                let bot = &mut scene.bots_mut()[slot];
                bot.position = [x + side, y, z];
                targets.push((bot.entity, bot.spawn.health as f32));
            }
        });
    let (thrown, copy) = throw_biotic_orb(
        &server,
        &schemas,
        &mut moira,
        &mut [],
        (UP, SECONDARY, 0x1e61),
    );
    assert_orb_copy_framed(&moira, thrown, copy);
    for (bot, full) in targets {
        let healths = health_records(&moira, bot, thrown);
        let mut last = (moira.ticks[thrown], full);
        for &(tick, health) in &healths {
            assert!(
                last.1 - health <= TICK_DAMAGE * (tick - last.0) as f32 + 1e-3,
                "faster than 50 a second: {last:?} to {:?}",
                (tick, health)
            );
            last = (tick, health);
        }
        assert!((last.1 - (full - 100.8)).abs() < 1e-3, "{healths:?}");
    }
    server.stop().unwrap();
}

/// Moira's healing Biotic Orb (E, then the primary fire: `10E4` rid18,
/// `1E60.003`), thrown at her feet beside her wounded teammate (an enemy
/// Soldier: 76 wounds him first). The server flies it, its graph `10FF`
/// puts its aura `02A1` on the hurt ally within 5 m, and his body's own
/// graphs run the aura's `1102`, which heals him by 65 a second for Moira
/// up to his full health. Moira's body tolerates unknown content: a debug
/// build refuses `10E4` node 87's `STU_0340A42A` (loose ends), which stops
/// her script and with it her orb's flight.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn moiras_healing_orb_heals_a_wounded_teammate_at_its_rate() {
    use lucy_script::vm::Unknowns;
    use lucy_sim::scene_assets::SceneAssets;
    use lucy_wire::commands::FIRE;
    /// Straight down; the mover holds the view at 89°.
    const DOWN: i16 = 16384;
    /// 65 a second over a 16 ms tick.
    const TICK_HEAL: f32 = 1.04;
    const FULL: f32 = 200.0;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    let (mut server, schemas, [mut moira, mut ally, mut enemy], _) = practice_world(
        [
            ("moira#1111", 0, "Moira"),
            ("ally#2222", 0, "Soldier: 76"),
            ("enemy#3333", 1, "Soldier: 76"),
        ],
        &assets,
        |_| {},
    );
    let ally_body = ally.owner | 1;
    let health = |watcher: &Watcher| {
        health_records(watcher, ally_body, 0)
            .last()
            .map(|&(_, health)| health)
    };
    // The enemy wounds the ally, aiming from where the two bodies stand
    // (bodies: Moira's, the ally's, the enemy's).
    let mut frames = [moira.tick + 1, ally.tick + 1, enemy.tick + 1];
    let deadline = server.now() + Duration::from_secs(10);
    while !health(&ally).is_some_and(|health| health < FULL) {
        assert!(server.now() < deadline, "waiting for the wound");
        let bodies = server.bodies(MATCH).unwrap();
        let [from, to] = [2, 1].map(|index| bodies[index].position.unwrap());
        let aim = ((to[0] - from[0]).atan2(to[2] - from[2]) * 32768.0 / std::f64::consts::PI)
            .round() as i32 as i16;
        send_input(&mut enemy, &mut frames[2], aim, 0, FIRE, false);
        send_input(&mut moira, &mut frames[0], 0, 0, 0, false);
        send_input(&mut ally, &mut frames[1], 0, 0, 0, false);
        for watcher in [&mut moira, &mut ally, &mut enemy] {
            watcher.pump(&schemas);
        }
    }
    for _ in 0..20 {
        send_input(&mut moira, &mut frames[0], 0, 0, 0, false);
        send_input(&mut ally, &mut frames[1], 0, 0, 0, false);
        send_input(&mut enemy, &mut frames[2], 0, 0, 0, false);
        for watcher in [&mut moira, &mut ally, &mut enemy] {
            watcher.pump(&schemas);
        }
    }
    let wounded = health(&ally).unwrap();
    assert!(wounded < FULL, "wounded to {wounded}");

    let healing = ally.datagrams.len();
    let (thrown, copy) = throw_biotic_orb(
        &server,
        &schemas,
        &mut moira,
        &mut [&mut ally, &mut enemy],
        (DOWN, FIRE, 0x1e60),
    );
    assert_orb_copy_framed(&moira, thrown, copy);
    let healths = health_records(&ally, ally_body, healing);
    let mut last = (ally.ticks[healing], wounded);
    for &(tick, health) in &healths {
        assert!(
            health - last.1 <= TICK_HEAL * (tick - last.0) as f32 + 1e-3,
            "faster than 65 a second: {last:?} to {:?}",
            (tick, health)
        );
        last = (tick, health);
    }
    assert_eq!(last.1, FULL, "{healths:?}");
    server.stop().unwrap();
}

/// A Skirmish match on a map other than the Practice Range (ow174/game/
/// match.py, server.py): its collision is built from the install while two
/// players load, and both spawn on it. Skirmish's own graph `0D76` drives
/// the mode entity (its root in every whole mode frame) and has no
/// assembly: it never sets a phase, and its clock (#1303e) counts down
/// from 1800 s (rid91) from its start. So hero select opens for a player
/// with no hero (the graph's held `0727`, `0C90` st28) and closes once his
/// pick's body is there, not after 30 s. A third player then joins the
/// running match and sees the others.
#[test]
#[ignore = "builds Workshop Chamber's collision; requires an Overwatch 1.74 install (LUCY_GAME_DIR)"]
fn a_skirmish_match_on_another_map_plays_at_once_and_takes_a_third_player() {
    use lucy_content::{heroes::SOLDIER, maps::MapCatalog, parameters::PICK_HERO};
    use lucy_script::wire::read_full_head;
    use lucy_sim::scene_assets::SceneAssets;

    const WORKSHOP_CHAMBER: u64 = 0x0800_0000_0000_0c48;
    const SKIRMISH: u64 = 0x0230_0000_0000_0010;

    let runtime = lucy_testkit::runtime_dir();
    let collision = tempfile::tempdir().unwrap();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_folder(collision.path())
            .with_seed(SCENE_SEED)
            .with_collision_build(collision_build(lucy_testkit::game_dir())),
    );
    let catalog = MapCatalog::load(runtime.join("data")).unwrap();
    let map = catalog
        .game_map(WORKSHOP_CHAMBER, SKIRMISH, "Skirmish", false, &[6, 6])
        .expect("Workshop Chamber in Skirmish");
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
    // Whether a controller frame shows hero select so, with the hero he
    // plays.
    let shows = |chunk: &ReceivedChunk, open: bool, hero: Option<u64>| {
        controller_shows(assets.graphs(), chunk)
            .is_some_and(|(shown, _, played)| (shown, played) == (open, hero))
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
        // Hero select opens for a player with no hero.
        let (_, first_frame) = &controllers(watcher)[0];
        assert!(shows(first_frame, true, None));
    }

    // A pick creates the body, and hero select closes with it: no
    // assembly keeps it open.
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
    pump_within(
        &mut [first, second],
        &schemas,
        "the bodies, with hero select closed",
        Duration::from_secs(5),
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher.created(watcher.owner | 1).is_some_and(|created| {
                    controllers(watcher).iter().any(|(index, chunk)| {
                        *index >= created && shows(chunk, false, Some(SOLDIER))
                    })
                })
            })
        },
    );
    // The mode entity's whole frames are `0D76`'s, its clock counting
    // down from 1800 s.
    let graphs = assets.graphs();
    for watcher in [&*first, &*second] {
        let heads: Vec<_> = joined_chunks(mode_frames(watcher).iter().map(|(_, chunk)| chunk))
            .iter()
            .filter(|chunk| chunk.first == 0)
            .map(|chunk| {
                read_full_head(&chunk.data, |graph| {
                    graphs
                        .graph(u32::from(graph))
                        .map(|graph| graph.header.states_bits)
                })
                .expect("a full frame's head")
            })
            .collect();
        assert!(!heads.is_empty(), "no full mode frame");
        for head in &heads {
            let root = head
                .instances
                .iter()
                .find(|&&(instance, ..)| instance == 1)
                .expect("the root instance")
                .1;
            assert_eq!(root, 0x0d76);
            let clock = head
                .variables
                .get(&lucy_sim::match_flow::CLOCK)
                .map(lucy_script::expr::to_float);
            assert!(
                clock.is_some_and(|clock| clock > 1700.0 && clock <= 1800.0),
                "{clock:?}"
            );
        }
    }

    // A third player joins the running match: hero select open, the
    // others in his world, and the graph's frames from his first one.
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
                && !mode_frames(watchers[2]).is_empty()
                && watchers[2].created(first_owner).is_some()
                && watchers[2].created(second_owner).is_some()
                && watchers[0].created(third_owner).is_some()
                && watchers[1].created(third_owner).is_some()
        },
    );
    assert!(shows(&controllers(&third)[0].1, true, None));
    let (_, first_mode) = mode_frames(&third).into_iter().next().unwrap();
    assert_eq!(first_mode.first, 0, "his first mode frame is whole");

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

/// A Quick Play Assault match on Temple of Anubis, driven by its mode
/// graph `0F4A` in the graph's own time: assemble, setup, the round, the
/// capture of A and then B by the attacker (the capture area aura's `0F00`
/// on his body reports him on the point, `08AF` for team 1; the point's
/// `0E8E` counts him), the round's end, the match's end; the result goes to the
/// lobby (a win for the attackers' team, a loss for the defenders') and
/// both players go home (20304) once `0F4A`'s `180A` has shown the match
/// end's cards (56.4 s) and ended the game.
///
/// The test moves both points into the attackers' spawn room, so the
/// attacker captures them standing where he spawns: walking 125 m to A
/// needs the movement the test client doesn't play.
#[test]
#[ignore = "slow: plays a whole Assault match in the mode graph's time on Anubis' collision (built once), about 90 s optimised; requires an Overwatch 1.74 install (LUCY_GAME_DIR)"]
fn slow_an_assault_match_on_anubis_captures_a_and_b_and_sends_the_players_home() {
    use lucy_content::{
        heroes::SOLDIER,
        maps::{MapCatalog, ObjectiveKind},
        parameters::PICK_HERO,
    };
    use lucy_script::wire::read_full_head;
    use lucy_sim::{match_flow::ASSAULT, scene_assets::SceneAssets};

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const SCALE: f64 = 0.1;

    let runtime = lucy_testkit::runtime_dir();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_seed(SCENE_SEED)
            .with_collision_build(collision_build(lucy_testkit::game_dir())),
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

    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(lucy_sim::match_flow::Timers { scale: SCALE })
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
    // `0F4A`'s times: 30 s to assemble, 45 s of setup, A in 30 s and B in
    // 18 s with one attacker on them, the round's end, then the match's
    // end's cards (56.4 s). The clients send their (idle) commands, which
    // run their bodies' scripts, the aura's `0F00` among them.
    let (mut defending, mut attacking) = (defender.tick + 1, attacker.tick + 1);
    let deadline = server.now() + Duration::from_secs(210);
    while [&*defender, &*attacker]
        .iter()
        .any(|watcher| received(watcher, 20304).is_none())
    {
        assert!(server.now() < deadline, "the match didn't end");
        send_input(defender, &mut defending, 0, 0, 0, false);
        send_input(attacker, &mut attacking, 0, 0, 0, false);
        defender.pump(&schemas);
        attacker.pump(&schemas);
    }
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

    // The mode entity's frames are `0F4A`'s, not the hand-built `0CB6`
    // one: its root instance (1) in each whole frame.
    let graphs = assets.graphs();
    for watcher in [&*defender, &*attacker] {
        let roots: Vec<u16> = joined_chunks(
            watcher
                .datagrams
                .iter()
                .flat_map(|(_, records)| records)
                .filter(|record| record.entity == GAME_MODE_ENTITY)
                .filter_map(|record| record.script.as_ref()),
        )
        .iter()
        .filter(|chunk| chunk.first == 0)
        .map(|chunk| {
            let head = read_full_head(&chunk.data, |graph| {
                graphs
                    .graph(u32::from(graph))
                    .map(|graph| graph.header.states_bits)
            })
            .expect("a full frame's head");
            head.instances
                .iter()
                .find(|&&(instance, ..)| instance == 1)
                .expect("the root instance")
                .1
        })
        .collect();
        assert!(!roots.is_empty(), "no full mode frame");
        assert!(roots.iter().all(|&graph| graph == 0x0f4a), "{roots:04X?}");
    }
    server.stop().unwrap();
}

/// Practice vs AI, Easy, on Temple of Anubis (lucy_sim::ai), the mode
/// graph `0F4A` running the match on its own clocks: one player on the
/// blue team (the defenders) and eleven AI heroes, five with him and six
/// attacking. His game gets them as other players (their cards with their
/// heroes' names, their bodies); the AI heroes pick roster heroes, the
/// attackers walk from their spawn toward point A, the AI heroes of both
/// teams fight (bodies lose health or die; the defenders leave their
/// spawn room only once the spawn walls' `0FA8` reads the attacking team
/// from the mode graph, and the point counts who stands on it only once
/// the bodies' `08AE`/`08AF` end as they leave), and the mode graph ends
/// the match: the result has the player only, his game gets the match
/// end's cards (`180A`) and goes home once they end the game.
#[test]
#[ignore = "slow: simulates a whole match on the mode graph's clocks with twelve bodies, about 8 min optimised; requires an Overwatch 1.74 install (LUCY_GAME_DIR)"]
fn slow_a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end() {
    use lucy_content::{
        heroes::SOLDIER,
        maps::{MapCatalog, ObjectiveKind},
        parameters::PICK_HERO,
    };
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        ai::{Difficulty, ROSTER},
        match_flow::ASSAULT,
        scene_assets::SceneAssets,
    };

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const SCALE: f64 = 0.25;

    let runtime = lucy_testkit::runtime_dir();
    let mut assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_seed(SCENE_SEED)
        .with_collision_build(collision_build(lucy_testkit::game_dir()));
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

    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(lucy_sim::match_flow::Timers { scale: SCALE })
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

    // The result phase: `0F4A`'s `180A` shows the match end's cards, each
    // a list of graphs it plays on the mode entity, and the player's game
    // gets them in the mode entity's frames (the graphs the lucy-sim
    // test `the_match_end_plays_its_cards_and_then_ends_the_game` lists).
    // He goes home only when the last card ends the game: 56368 ms after
    // `0F4A` began the match's end at a frame's end, the first card's
    // graphs 240 ms after that, so 56128 ms (3508 ticks) after them.
    const CARD_GRAPHS: [u16; 15] = [
        0x027f, 0x0281, 0x0282, 0x0283, 0x0b46, 0x0d94, 0x17ca, 0x17df, 0x17e2, 0x17e3, 0x17e4,
        0x1818, 0x181e, 0x184c, 0x184d,
    ];
    let graphs = assets.graphs();
    let mode_chunks = |records: &[WorldRecord]| -> Vec<ReceivedChunk> {
        records
            .iter()
            .filter(|record| record.entity == GAME_MODE_ENTITY)
            .filter_map(|record| record.script.clone())
            .collect()
    };
    let shown: std::collections::BTreeSet<u16> = joined_chunks(
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| mode_chunks(records))
            .collect::<Vec<_>>()
            .iter(),
    )
    .iter()
    .flat_map(|chunk| listed_graphs(graphs, chunk))
    .filter(|graph| CARD_GRAPHS.contains(graph))
    .collect();
    assert_eq!(shown, std::collections::BTreeSet::from(CARD_GRAPHS));
    let first_card = watcher
        .datagrams
        .iter()
        .position(|(_, records)| {
            mode_chunks(records)
                .iter()
                .filter(|chunk| chunk.part.is_none())
                .any(|chunk| listed_graphs(graphs, chunk).contains(&0x027f))
        })
        .expect("the first card's graphs");
    let home = watcher.datagrams.len() - 1;
    assert_eq!(watcher.ticks[home] - watcher.ticks[first_card], 3508);
    server.stop().unwrap();
}

/// A Competitive Assault match on Temple of Anubis, driven by `0F4A` in
/// its own time, plays both sides: team 1 attacks first and takes A, then
/// time runs out on B; before the second side both players' bodies revive
/// in their new side's spawn room (team 0 now in the attackers' room, team
/// 1 in the defenders'), where team 0 takes A too. Equal progress with no
/// time left: a draw for both. The server's wait after the match's end is
/// 1/50 of it (lucy_sim::match_flow `Timers`).
///
/// A stands in the attackers' first room and B in the defenders' room, so
/// each attacker captures A where he spawns and no one can reach B: the
/// test client plays no movement. Team 0 can only take A if the side swap
/// moved him.
#[test]
#[ignore = "slow: plays both sides of a whole Competitive Assault match in the mode graph's time on Anubis' collision (built once); requires an Overwatch 1.74 install (LUCY_GAME_DIR)"]
fn slow_a_competitive_assault_on_anubis_swaps_sides_and_spawn_rooms() {
    use lucy_content::{
        heroes::SOLDIER,
        maps::{MapCatalog, ObjectiveKind, SpawnChoice},
        parameters::PICK_HERO,
    };
    use lucy_sim::{match_flow::ASSAULT, scene_assets::SceneAssets};

    const ANUBIS: u64 = 0x0800_0000_0000_005b;
    const SCALE: f64 = 0.02;
    /// "Assault - Competitive", the Competitive cards' ruleset
    /// (`game_queues_174.json`), as the matchmaker gives the map.
    const COMPETITIVE: u64 = 0x0fd0_0000_0000_002b;

    let runtime = lucy_testkit::runtime_dir();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_seed(SCENE_SEED)
            .with_collision_build(collision_build(lucy_testkit::game_dir())),
    );
    let catalog = MapCatalog::load(runtime.join("data")).unwrap();
    let mut map = catalog
        .game_map(ANUBIS, ASSAULT, "Assault", false, &[6, 6])
        .expect("Temple of Anubis in Assault");
    map.ruleset = COMPETITIVE;
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

    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(lucy_sim::match_flow::Timers { scale: SCALE })
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
    // `0F4A`'s times; the clients send their (idle) commands, which run
    // their bodies' scripts, the capture area aura's `0F00` among them.
    let (mut blue_input, mut red_input) = (blue.tick + 1, red.tick + 1);
    let deadline = server.now() + Duration::from_secs(1200);
    while [&*blue, &*red]
        .iter()
        .any(|watcher| received(watcher, 20304).is_none())
    {
        assert!(server.now() < deadline, "the match didn't end");
        send_input(blue, &mut blue_input, 0, 0, 0, false);
        send_input(red, &mut red_input, 0, 0, 0, false);
        blue.pump(&schemas);
        red.pump(&schemas);
    }
    // The second side moves the same bodies into the swapped rooms. The
    // draw below proves team 0 reached A, placed at the attacking spawn.
    for watcher in [&*blue, &*red] {
        assert!(watcher.created(watcher.owner | 1).is_some());
        assert!(
            watcher.created(watcher.owner | 2).is_none(),
            "{:08X}: a round restart must retain the possessed body",
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
    use lucy_content::heroes::SOLDIER;

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
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        body_script::fixture_hitscan_factory, bots::BOT_ENTITY, scene_assets::SceneAssets,
    };
    use lucy_wire::commands::FIRE;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
    scene.owner_body_factory = Some(Arc::new(move |hero, spawn, _| {
        if hero.guid != SOLDIER {
            return Ok(None);
        }
        Ok(Some(Box::new(factory.start(spawn)?)))
    }));
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
    // script, which doesn't fire: its flinch (`0011.025`); no elimination
    // yet.
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
            "waiting for the surviving hit's flinch frame"
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

/// The Practice Range's mode frames leave room for the movement records
/// (`lucy_sim::mode_view`): the first is whole (about 920 bytes), and
/// once the bots stand in the world, every datagram for 5 s carries a
/// movement record of every body that has one in the datagrams around it,
/// and the mode frame's change of the tick as a delta of a few bytes.
/// Before, the whole frame went once a second and took the place of 9 of
/// the 16 movement records.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn the_range_mode_frames_leave_room_for_every_movement_record() {
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use lucy_script::vm::Unknowns;
    use lucy_sim::scene_assets::SceneAssets;

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    assets.set_bot_unknowns(Unknowns::Tolerate);
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
    pump_until(&mut [&mut watcher], &schemas, "the world", |watchers| {
        let watcher = &watchers[0];
        watcher
            .find(0, |record| {
                record.entity == watcher.owner && record.script.is_some()
            })
            .is_some()
    });
    let owner = watcher.owner;
    send_player_message(
        &mut watcher.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    // The body, the Range's first mode frame, and every bot moving.
    pump_within(
        &mut [&mut watcher],
        &schemas,
        "the body and the bots",
        Duration::from_secs(20),
        |watchers| {
            let watcher = &watchers[0];
            watcher.created(owner | 1).is_some()
                && watcher
                    .find(0, |record| {
                        record.entity == GAME_MODE_ENTITY && record.script.is_some()
                    })
                    .is_some()
                && watcher.datagrams.last().is_some_and(|(_, records)| {
                    records.iter().filter(|record| record.movement).count() >= 15
                })
        },
    );
    let from = watcher.datagrams.len();
    let until = watcher.client.now() + Duration::from_secs(5);
    while watcher.client.now() < until {
        watcher.pump(&schemas);
    }
    // The first mode frame is whole and lists what the graph runs.
    let mode_chunks = |index: usize| -> Vec<&ReceivedChunk> {
        watcher.datagrams[index]
            .1
            .iter()
            .filter(|record| record.entity == GAME_MODE_ENTITY)
            .filter_map(|record| record.script.as_ref())
            .collect()
    };
    let full = watcher
        .find(0, |record| {
            record.entity == GAME_MODE_ENTITY && record.script.is_some()
        })
        .unwrap();
    let [first] = mode_chunks(full)[..] else {
        panic!("one mode frame")
    };
    assert_eq!(first.first, 0);
    assert_range_mode_frame(assets.graphs(), first);
    let moving = |index: usize| -> HashSet<u32> {
        watcher.datagrams[index]
            .1
            .iter()
            .filter(|record| record.movement)
            .map(|record| record.entity)
            .collect()
    };
    let mut dropped = Vec::new();
    let mut frames = Vec::new();
    let last = watcher.datagrams.len() - 1;
    for (index, &tick) in watcher.ticks.iter().enumerate().take(last).skip(from + 1) {
        let around: HashSet<u32> = moving(index - 1)
            .intersection(&moving(index + 1))
            .copied()
            .collect();
        let missing = around.difference(&moving(index)).count();
        if missing != 0 {
            dropped.push((tick, missing));
        }
        frames.extend(
            mode_chunks(index)
                .iter()
                .map(|chunk| (chunk.first, chunk.width.div_ceil(8))),
        );
    }
    assert!(dropped.is_empty(), "movement records left out: {dropped:?}");
    // `0CBB` st58's timed ChaseVar (#19530e) changes the frame every tick:
    // each change goes as a delta of a few bytes.
    assert!(frames.len() > 250, "{frames:?}");
    assert!(
        frames
            .iter()
            .all(|&(first, bytes)| first != 0 && bytes <= 32),
        "{frames:?}"
    );
    server.stop().unwrap();
}

/// combat.py `_bot_weapons`: a firing training bot's script fires its
/// rockets along its yaw, and they damage the player in front of it, once
/// he ran out of his spawn room (where he is invulnerable); the hit he
/// survives gives him his pain line.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_firing_practice_bot_shoots_the_player() {
    use lucy_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use lucy_script::vm::Unknowns;
    use lucy_sim::{
        projectiles::{SEEN_FIRST, SEEN_SPAN},
        scene_assets::SceneAssets,
    };

    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
            scene.in_spawn_room(0, lucy_content::maps::SpawnChoice::First, point) == Some(false)
        })
        .unwrap();
    let reach = exit + 3.0;
    let [x, z] = [x + reach * sin, z + reach * cos];
    // An enemy firing bot 4 m further along the player's way, facing back
    // at that spot, so its rockets fly along the line he ran whatever he
    // overshot it by: his body is hit by its hero's shapes, a torso a
    // few decimetres wide, where the capsule was a metre wide.
    let shooter = scene
        .bots_mut()
        .iter_mut()
        .find(|bot| bot.spawn.firing && bot.spawn.team != 0)
        .unwrap();
    let [bot_x, bot_z] = [x + 4.0 * sin, z + 4.0 * cos];
    shooter.position = [bot_x, y, bot_z];
    shooter.yaw = ((-sin).atan2(-cos) * 65536.0 / std::f64::consts::TAU).rem_euclid(65536.0);
    let shooter = shooter.entity;
    let schemas = Arc::new(Schemas::game(&lucy_testkit::runtime_dir()).unwrap());
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
    let mut next = run_out_of_the_spawn_room(
        &server,
        &mut watcher,
        &schemas,
        |feet| along(feet) >= reach,
        0,
    );
    // The rocket's hit, then his pain line: 0.15 s after the hit his body's
    // `01CA` raises it (his script runs on his commands, which go on). Its
    // audience is his body and the bot that hit him, so his client gets the
    // line (ClientInVoice 27500), his body speaking to his body.
    let hurt = |watcher: &Watcher| {
        watcher.find(0, |record| {
            record.entity == body && record.base_health.is_some_and(|health| health < 200.0)
        })
    };
    let body_id = |id: u32| Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(id)))]));
    let pain = |watcher: &Watcher| {
        let hit = hurt(watcher)?;
        (hit..watcher.datagrams.len()).find(|&index| {
            watcher.datagrams[index].0.iter().any(|message| {
                message.id == 27500
                    && message.fields[&0x78] == body_id(body)
                    && message.fields[&0x7c] == body_id(body)
            })
        })
    };
    pump_commanding(
        &mut [&mut watcher],
        &schemas,
        "the bot's rocket and the player's pain line",
        0,
        &mut next,
        |watchers| pain(watchers[0]).is_some(),
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
    // The first rocket takes 8 of his 200: the Training Bot's AI damage,
    // one direct hit, no splash and no critical (sim
    // `a_firing_bots_rocket_hits_the_player_in_front_of_it`).
    let first_hurt = hurt(&watcher).unwrap();
    let health = watcher.datagrams[first_hurt]
        .1
        .iter()
        .filter(|record| record.entity == body)
        .find_map(|record| record.base_health.filter(|health| *health < 200.0));
    assert_eq!(health, Some(192.0));
    // His client's damage direction arc: the hurt notice (25000) of the
    // hit, damage taken (kind 1) by his body from the bot standing where it
    // was placed, for those 8.
    let notice = watcher
        .datagrams
        .iter()
        .flat_map(|(messages, _)| messages)
        .find(|message| message.id == 25000)
        .expect("the hurt notice");
    let place = Value::Struct(Fields::from([
        (0, Value::Float32(bot_x as f32)),
        (4, Value::Float32(y as f32)),
        (8, Value::Float32(bot_z as f32)),
    ]));
    assert_eq!(
        [0x78, 0x7c, 0x80, 0x9c, 0xc0].map(|at| notice.fields[&at].clone()),
        [
            Value::Signed(1),
            body_id(body),
            body_id(shooter),
            place,
            Value::Float32(8.0),
        ]
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

mod assault;
mod collision_ready;
mod host;
mod load;
mod rooms;
mod victims;
mod workshop;

/// A living enemy training bot blocks Soldier: 76's body on the server
/// (`lucy_sim::replication::blocks`), as the client's own mover stops at
/// the bot's capsule: he runs out of his spawn room at a Training Bot
/// standing 3 m past its end, turning toward the bot every 4 frames, and
/// stops against it, their axes the sum of their radii apart (0.5 + 0.5,
/// less at most the solver's 5 mm slop and 5 mm tolerance), never nearer.
/// His owner corrections are his server body's poses, so none pulls his
/// client through the bot.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn soldier_runs_into_an_enemy_bot_and_stops_short_of_it() {
    let assets = lucy_sim::scene_assets::SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
                    scene.in_spawn_room(0, lucy_content::maps::SpawnChoice::First, point)
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
