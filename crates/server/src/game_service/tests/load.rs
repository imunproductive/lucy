//! The load harness (`just load <install> <archive>`): headless games join
//! a Workshop match (C32S2, the Doomfist parkour of Hanamura) or a Practice
//! vs AI match (Temple of Anubis, the AI heroes filling both teams of six),
//! N players each, one or more matches at once, and the rooms' own timing
//! (`timing.rs`) is read over a measured span after a warm-up.
//!
//! - On the virtual clock (`Lockstep`): every room on one thread, each tick
//!   on time; the timing is the CPU each tick takes, by phase.
//! - On the real clock (`GameService::bind`, real UDP): each room on its
//!   thread, the games on threads of their own; the timing adds the
//!   achieved tick rate, the skipped frames and how late ticks start.
//!
//! Each game does what the client does: the link's handshake, 21602 once
//! the instance (20300) came, a hero pick once the server has it in the
//! world, then one datagram after each frame with its commands up to the
//! frame after it: running forward, turning a quarter turn every 3 s,
//! jumping for 4 frames and firing for 30 in every 192. Memory per room is
//! the process's private bytes after the warm-up less before the rooms
//! opened, over the rooms.
//!
//! Every test here is a measurement, not a check: it prints a table and
//! fails only when the games can't get into the match. The `load_` tests
//! stay out of `test-game` and `test-affected`.

use super::*;
use crate::game_service::lockstep::Network;
use crate::workshop_match::{ScriptMatch, SettingNames};
use lucy_content::{
    heroes::SOLDIER,
    maps::{GameModes, MODES_FILE, MapCatalog},
    parameters::PICK_HERO,
};
use lucy_core::phase::Phase;
use lucy_script::vm::Unknowns;
use lucy_sim::{ai::Difficulty, match_flow::ASSAULT, scene_assets::SceneAssets};
use lucy_wire::{
    commands::{FIRE, JUMP, MAX_COMMANDS},
    link::{LinkCipher, MAX_DATAGRAM, ReceiveWindow},
};
use std::{
    net::{Ipv4Addr, UdpSocket as GameSocket},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    thread,
    time::Instant,
};

const DOOMFIST: u64 = 0x02e0_0000_0000_012f;
const ANUBIS: u64 = 0x0800_0000_0000_005b;
/// A match ruleset of the harness's own (`custom.rs` `MATCH_RULESETS`).
const RULESET: u64 = 0x0fd0_8000_0000_0001;

/// Game time (virtual) or wall time (real) after every game picked its
/// hero, before the measured span.
const WARMUP: Duration = Duration::from_secs(10);
const MEASURE: Duration = Duration::from_secs(30);
/// The games must all be in the match by then.
const JOIN_WITHIN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// C32S2 in a Workshop match: every player a Doomfist on team 1.
    Parkour,
    /// Practice vs AI: the players alternate teams, Soldier: 76 each, and
    /// AI heroes fill both teams to six.
    PracticeVsAi,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Parkour => "C32S2",
            Self::PracticeVsAi => "Practice vs AI",
        }
    }

    fn hero(self) -> u64 {
        match self {
            Self::Parkour => DOOMFIST,
            Self::PracticeVsAi => SOLDIER,
        }
    }
}

/// The data a scenario's matches share.
struct Setup {
    assets: Arc<SceneAssets>,
    maps: MapCatalog,
    parkour: Option<Parkour>,
}

struct Parkour {
    map: GameMap,
    logic: Arc<lucy_workshop::Logic>,
    catalog: Arc<lucy_workshop::Catalog>,
}

impl Setup {
    fn load(kind: Kind) -> Self {
        let runtime = lucy_testkit::runtime_dir();
        let data = runtime.join("data");
        let mut assets = SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(collision_build(lucy_testkit::game_dir()));
        // The AI heroes' graphs still have gaps: they run as a release
        // build runs them (`slow_a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end`).
        assets.set_player_unknowns(Unknowns::Tolerate);
        let assets = Arc::new(assets);
        let maps = MapCatalog::load(&data)
            .unwrap()
            .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
        let parkour = (kind == Kind::Parkour).then(|| {
            let catalog = Arc::new(lucy_workshop::Catalog::load(&data).unwrap());
            let path = lucy_testkit::dpk_archive()
                .join("workshop.codes/C32S2_OG_doom_parkur_hanamura.txt");
            let script = ScriptMatch::parse(
                &std::fs::read_to_string(path).unwrap(),
                &catalog,
                &SettingNames::load(&data).unwrap(),
            )
            .unwrap();
            let [game] = script.games.as_slice() else {
                panic!("{:?}", script.games)
            };
            let hanamura = game.maps.as_ref().unwrap()[0];
            let mut map = maps
                .game_map(hanamura, game.mode, "Assault", false, &[12, 0])
                .expect("Hanamura in Assault");
            map.hero_pool = script.heroes.clone().unwrap();
            map.ruleset = RULESET;
            assets.graphs().add_match_ruleset(
                RULESET,
                map.mode_guid,
                game.values.clone(),
                script.bodies.clone(),
            );
            Parkour {
                map,
                logic: Arc::clone(&script.logic),
                catalog,
            }
        });
        Self {
            assets,
            maps,
            parkour,
        }
    }

    /// Opens match `id` of `kind` with `players` players; their tickets in
    /// slot order.
    fn open(&self, server: &GameService, id: MatchId, kind: Kind, players: usize) -> Vec<Handoff> {
        let map = match kind {
            Kind::Parkour => self.parkour.as_ref().expect("C32S2").map.clone(),
            Kind::PracticeVsAi => self
                .maps
                .game_map(ANUBIS, ASSAULT, "Assault", false, &[6, 6])
                .expect("Temple of Anubis in Assault"),
        };
        let world_players = (0..players)
            .map(|index| {
                let name = format!("load{}x{index}#1000", id[1]);
                WorldPlayer {
                    player: MatchPlayer {
                        name: name.clone(),
                        team: match kind {
                            Kind::Parkour => 0,
                            Kind::PracticeVsAi => (index % 2) as u8,
                        },
                        tournament: false,
                        ranked: false,
                    },
                    card: PlayerCard {
                        account: [7, 0x0100_0000_0000_0000 | (id[1] << 8) | index as u64],
                        battle_tag: name.clone(),
                        record: Fields::from([(0x40, Value::String(name))]),
                        ..PlayerCard::default()
                    },
                    hero: 0,
                }
            })
            .collect();
        let prepare = {
            let (assets, map) = (Arc::clone(&self.assets), map.clone());
            Box::new(move || assets.prepare(map))
        };
        let ai = (kind == Kind::PracticeVsAi).then_some(Difficulty::Easy);
        let tickets = server
            .open_world(id, map, world_players, ai, prepare)
            .unwrap();
        if let Some(parkour) = &self.parkour
            && kind == Kind::Parkour
        {
            assert!(
                server
                    .attach_workshop(id, Arc::clone(&parkour.logic), Arc::clone(&parkour.catalog))
                    .unwrap()
            );
        }
        tickets
    }
}

/// Where a game's datagrams go.
enum Wire {
    Virtual {
        network: Network,
        address: SocketAddr,
    },
    Udp(GameSocket),
}

impl Wire {
    fn send(&self, datagram: Vec<u8>) {
        match self {
            Self::Virtual { network, address } => network.send(*address, datagram),
            Self::Udp(socket) => {
                let _ = socket.send(&datagram);
            }
        }
    }

    /// The next datagram from the service: at once on the virtual clock,
    /// within 100 ms over UDP.
    fn take(&self) -> Option<Vec<u8>> {
        match self {
            Self::Virtual { network, address } => network.take(*address),
            Self::Udp(socket) => {
                let mut buffer = [0; MAX_DATAGRAM];
                let count = socket.recv(&mut buffer).ok()?;
                Some(buffer[..count].to_vec())
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Connecting,
    Loading,
    Playing,
}

/// A headless game (the module notes).
struct Game {
    wire: Wire,
    cipher: LinkCipher,
    connection: u32,
    next: u32,
    window: ReceiveWindow,
    slot: usize,
    hero: u64,
    stage: Stage,
    /// The newest frame's tick.
    tick: u32,
    /// Its next command's frame.
    command: u32,
    /// Its next reliable message's ordinal.
    ordinal: u32,
    picks: u32,
}

impl Game {
    fn new(wire: Wire, ticket: &Handoff, slot: usize, hero: u64) -> Self {
        let mut game = Self {
            wire,
            cipher: LinkCipher::new(ticket.key_out, ticket.key_in),
            connection: ticket.conn,
            next: 0,
            window: ReceiveWindow::default(),
            slot,
            hero,
            stage: Stage::Connecting,
            tick: 0,
            command: 0,
            ordinal: 0,
            picks: 0,
        };
        game.send(SYN, Vec::new());
        game
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
        self.wire.send(self.cipher.seal(&packet).unwrap());
    }

    fn reliable(&mut self, message: Vec<u8>) {
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(self.ordinal, message)], &[]).unwrap();
        self.ordinal += 1;
        self.send(ACK, payload.into_bytes());
    }

    /// Takes what came, answering each frame; `picks` is how many hero
    /// picks the harness wants it to have sent.
    fn pump(&mut self, schemas: &Schemas, picks: u32) {
        while let Some(datagram) = self.wire.take() {
            self.receive(schemas, &datagram, picks);
            if matches!(self.wire, Wire::Udp(_)) {
                break;
            }
        }
    }

    fn receive(&mut self, schemas: &Schemas, datagram: &[u8], picks: u32) {
        let Some(packet) = self.cipher.open(datagram) else {
            return;
        };
        if packet.kind == KIND_CONTROL {
            if self.stage == Stage::Connecting && packet.flags == SYN | ACK {
                self.window.accept(packet.seq);
                self.send(ACK, Vec::new());
                self.stage = Stage::Loading;
            }
            return;
        }
        if packet.kind != KIND_FRAME {
            return;
        }
        self.window.accept(packet.seq);
        self.tick = tick_of(&packet);
        match self.stage {
            Stage::Connecting => {}
            Stage::Loading => {
                if messages(schemas, &packet)
                    .iter()
                    .any(|item| item.message.id == 20300)
                {
                    self.reliable(encode(schemas, 21602, &Fields::new()).unwrap());
                    self.stage = Stage::Playing;
                    self.command = self.tick + 1;
                } else {
                    self.send(ACK, Vec::new());
                }
            }
            Stage::Playing => {
                if self.picks < picks {
                    self.pick(schemas);
                }
                self.commands();
            }
        }
    }

    /// 21616 to his controller's instance 3: the hero pick
    /// (`send_player_message`).
    fn pick(&mut self, schemas: &Schemas) {
        let fields = Fields::from([
            (
                0x78,
                Value::Struct(Fields::from([(
                    0,
                    Value::Unsigned(u64::from(player_entity(self.slot))),
                )])),
            ),
            (0x80, Value::Blob(self.hero.to_le_bytes().to_vec())),
            (0xa8, Value::Unsigned(PICK_HERO)),
            (0xb0, Value::Signed(3)),
        ]);
        let message = encode(
            schemas,
            lucy_content::parameters::CLIENT_GAME_MESSAGE,
            &fields,
        )
        .unwrap();
        self.reliable(message);
        self.picks += 1;
    }

    /// His commands up to the frame after the newest (`send_input`).
    fn commands(&mut self) {
        let newest = self.tick + 1;
        if self.command > newest {
            self.send(ACK, Vec::new());
            return;
        }
        let count = (newest - self.command + 1).min(u32::from(MAX_COMMANDS));
        let time = self.command + self.slot as u32 * 37;
        // A quarter turn every 3 s (188 frames).
        let yaw = (time.wrapping_mul(87) & 0xFFFF) as u16 as i16;
        let buttons = match time % 192 {
            0..4 => JUMP,
            96..126 => FIRE,
            _ => 0,
        };
        let payload = aimed_batch(self.command, count as u8, yaw, 0, buttons, true);
        self.send(ACK, payload);
        self.command += count;
    }
}

/// The process's private bytes (PowerShell's `Get-Process`).
fn private_bytes() -> u64 {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Get-Process -Id {}).PrivateMemorySize64",
                std::process::id()
            ),
        ])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap()
}

/// How many picks each game should have sent: one more whenever the
/// server has him in the world without the hero, 2 s after the last.
struct Picks {
    wanted: Vec<Vec<Arc<AtomicU32>>>,
    last: Vec<Vec<Option<Duration>>>,
}

impl Picks {
    fn new(rooms: &[Vec<Handoff>]) -> Self {
        Self {
            wanted: rooms
                .iter()
                .map(|tickets| tickets.iter().map(|_| Arc::default()).collect())
                .collect(),
            last: rooms
                .iter()
                .map(|tickets| vec![None; tickets.len()])
                .collect(),
        }
    }

    /// True once every player plays his hero.
    fn update(&mut self, server: &GameService, ids: &[MatchId], hero: u64, now: Duration) -> bool {
        let snapshots = server.snapshot().unwrap();
        let mut ready = true;
        for (room, id) in ids.iter().enumerate() {
            let snapshot = snapshots
                .iter()
                .find(|snapshot| snapshot.id == *id)
                .expect("the match runs");
            for (slot, wanted) in self.wanted[room].iter().enumerate() {
                let player = &snapshot.players[slot];
                if player.state == PlayerState::Playing && player.hero == hero {
                    continue;
                }
                ready = false;
                let last = &mut self.last[room][slot];
                if player.state == PlayerState::Playing
                    && last.is_none_or(|last| now >= last + Duration::from_secs(2))
                {
                    wanted.fetch_add(1, Ordering::Relaxed);
                    *last = Some(now);
                }
            }
        }
        ready
    }
}

/// One room's measured span.
struct Measured {
    stats: TickStats,
}

fn timings(server: &GameService, ids: &[MatchId]) -> Vec<TickStats> {
    let snapshots = server.snapshot().unwrap();
    ids.iter()
        .map(|id| {
            snapshots
                .iter()
                .find(|snapshot| snapshot.id == *id)
                .expect("the match runs")
                .timing
                .total
                .clone()
        })
        .collect()
}

fn match_id(room: usize) -> MatchId {
    [5, 8 + room as u64]
}

/// `rooms` matches of `kind` with `players` each on the virtual clock.
fn virtual_clock(setup: &Setup, kind: Kind, players: usize, rooms: usize) -> (Vec<Measured>, u64) {
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(lucy_sim::match_flow::Timers { scale: 0.25 })
        .unwrap();
    let before = private_bytes();
    let ids: Vec<MatchId> = (0..rooms).map(match_id).collect();
    let tickets: Vec<Vec<Handoff>> = ids
        .iter()
        .map(|&id| setup.open(&server, id, kind, players))
        .collect();
    server.await_scenes();
    let network = server.network().clone();
    let mut games: Vec<Vec<Game>> = tickets
        .iter()
        .map(|tickets| {
            tickets
                .iter()
                .enumerate()
                .map(|(slot, ticket)| {
                    let wire = Wire::Virtual {
                        network: network.clone(),
                        address: network.bind(),
                    };
                    Game::new(wire, ticket, slot, kind.hero())
                })
                .collect()
        })
        .collect();
    let mut picks = Picks::new(&tickets);
    let step = |games: &mut Vec<Vec<Game>>, picks: &Picks| {
        server.step();
        for (room, games) in games.iter_mut().enumerate() {
            for (slot, game) in games.iter_mut().enumerate() {
                game.pump(&schemas, picks.wanted[room][slot].load(Ordering::Relaxed));
            }
        }
    };
    let joined = server.now() + JOIN_WITHIN;
    let mut poll = server.now();
    loop {
        step(&mut games, &picks);
        if server.now() >= poll {
            if picks.update(&server, &ids, kind.hero(), server.now()) {
                break;
            }
            poll += Duration::from_millis(250);
        }
        assert!(server.now() < joined, "the games didn't get into the match");
    }
    for _ in 0..lockstep::ticks(WARMUP) {
        step(&mut games, &picks);
    }
    let after = private_bytes();
    let start = timings(&server, &ids);
    for _ in 0..lockstep::ticks(MEASURE) {
        step(&mut games, &picks);
    }
    let end = timings(&server, &ids);
    server.stop().unwrap();
    let measured = end
        .iter()
        .zip(&start)
        .map(|(end, start)| Measured {
            stats: end.since(start),
        })
        .collect();
    (measured, after.saturating_sub(before) / rooms as u64)
}

/// `rooms` matches of `kind` with `players` each on the service's own
/// threads and real UDP, each game on a thread of its own.
fn real_clock(setup: &Setup, kind: Kind, players: usize, rooms: usize) -> (Vec<Measured>, u64) {
    let schemas = Arc::new(Schemas::game());
    let mut server =
        GameService::bind((Ipv4Addr::LOCALHOST, 0).into(), Arc::clone(&schemas)).unwrap();
    server
        .set_match_timers(lucy_sim::match_flow::Timers { scale: 0.25 })
        .unwrap();
    let before = private_bytes();
    let ids: Vec<MatchId> = (0..rooms).map(match_id).collect();
    let tickets: Vec<Vec<Handoff>> = ids
        .iter()
        .map(|&id| setup.open(&server, id, kind, players))
        .collect();
    let mut picks = Picks::new(&tickets);
    let stop = Arc::new(AtomicBool::new(false));
    let threads: Vec<_> = tickets
        .iter()
        .enumerate()
        .flat_map(|(room, tickets)| {
            tickets.iter().enumerate().map({
                let (server, schemas, stop, picks) = (&server, &schemas, &stop, &picks);
                move |(slot, ticket)| {
                    let socket = GameSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
                    socket.connect(server.local_addr()).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .unwrap();
                    let mut game = Game::new(Wire::Udp(socket), ticket, slot, kind.hero());
                    let (schemas, stop) = (Arc::clone(schemas), Arc::clone(stop));
                    let wanted = Arc::clone(&picks.wanted[room][slot]);
                    thread::Builder::new()
                        .name(format!("load-game-{room}-{slot}"))
                        .spawn(move || {
                            while !stop.load(Ordering::Relaxed) {
                                game.pump(&schemas, wanted.load(Ordering::Relaxed));
                            }
                        })
                        .unwrap()
                }
            })
        })
        .collect();
    let started = Instant::now();
    while !picks.update(&server, &ids, kind.hero(), started.elapsed()) {
        assert!(
            started.elapsed() < JOIN_WITHIN,
            "the games didn't get into the match"
        );
        thread::sleep(Duration::from_millis(250));
    }
    thread::sleep(WARMUP);
    let after = private_bytes();
    let start = timings(&server, &ids);
    thread::sleep(MEASURE);
    let end = timings(&server, &ids);
    stop.store(true, Ordering::Relaxed);
    for thread in threads {
        thread.join().unwrap();
    }
    server.stop().unwrap();
    let measured = end
        .iter()
        .zip(&start)
        .map(|(end, start)| Measured {
            stats: end.since(start),
        })
        .collect();
    (measured, after.saturating_sub(before) / rooms as u64)
}

const HEADER: &str = "| clock | match | players | rooms | room | ticks/s | skipped | work mean | p99 | max | receive | simulate | ai | script | workshop | replicate | crypto | send | other | late mean | late p99 | late max | MB/room |\n|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|";

/// One table row a room (ms, the phases as their mean a tick).
fn rows(clock: &str, kind: Kind, players: usize, measured: &[Measured], memory: u64) -> String {
    let ms = |time: Duration| format!("{:.3}", time.as_secs_f64() * 1000.0);
    let mut table = String::new();
    for (room, Measured { stats }) in measured.iter().enumerate() {
        let phases: Vec<String> = Phase::ALL
            .iter()
            .filter(|&&phase| phase != Phase::Other)
            .chain(std::iter::once(&Phase::Other))
            .map(|&phase| ms(stats.phase_mean(phase)))
            .collect();
        table += &format!(
            "| {clock} | {} | {players} | {} | {room} | {:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {:.0} |\n",
            kind.name(),
            measured.len(),
            stats.rate(),
            stats.skipped,
            ms(stats.work.mean()),
            ms(stats.work.quantile(0.99)),
            ms(stats.work.max()),
            phases.join(" | "),
            ms(stats.late.mean()),
            ms(stats.late.quantile(0.99)),
            ms(stats.late.max()),
            memory as f64 / 1e6,
        );
    }
    table
}

fn run(kind: Kind, clock: &str, scenarios: &[(usize, usize)]) {
    let setup = Setup::load(kind);
    let mut table = String::from(HEADER);
    table.push('\n');
    for &(players, rooms) in scenarios {
        let (measured, memory) = match clock {
            "virtual" => virtual_clock(&setup, kind, players, rooms),
            _ => real_clock(&setup, kind, players, rooms),
        };
        let rows = rows(clock, kind, players, &measured, memory);
        // Each scenario's rows as they come, then the whole table.
        print!("{rows}");
        table += &rows;
    }
    println!("\n{table}");
}

#[test]
#[ignore = "load: measures C32S2 matches on the virtual clock; requires an Overwatch 1.74 install and the DPK archive (just load <install> <archive>)"]
fn load_parkour_on_the_virtual_clock() {
    run(
        Kind::Parkour,
        "virtual",
        &[(1, 1), (4, 1), (8, 1), (12, 1), (12, 3)],
    );
}

#[test]
#[ignore = "load: measures Practice vs AI matches on the virtual clock; requires an Overwatch 1.74 install (just load <install> <archive>)"]
fn load_practice_vs_ai_on_the_virtual_clock() {
    run(
        Kind::PracticeVsAi,
        "virtual",
        &[(1, 1), (4, 1), (8, 1), (12, 1), (1, 3)],
    );
}

#[test]
#[ignore = "load: measures concurrent C32S2 matches on the real clock; requires an Overwatch 1.74 install and the DPK archive (just load <install> <archive>)"]
fn load_parkour_on_the_real_clock() {
    run(Kind::Parkour, "real", &[(12, 1), (12, 2), (12, 3)]);
}

#[test]
#[ignore = "load: measures concurrent Practice vs AI matches on the real clock; requires an Overwatch 1.74 install (just load <install> <archive>)"]
fn load_practice_vs_ai_on_the_real_clock() {
    run(Kind::PracticeVsAi, "real", &[(1, 1), (1, 2), (1, 3)]);
}
