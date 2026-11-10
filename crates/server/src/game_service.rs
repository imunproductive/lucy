//! The native UDP match worker. It owns seats, connections and frame timing.
//!
//! A match on any map of the catalog advances from loading into one world
//! for all its players, who see each other (ow174/game/match.py). Its scene,
//! with the map's collision, is prepared off the game thread while the
//! players load (match.py `_refresh_collision`); players can join it while it
//! runs (ow174/game/server.py `join_match`). In the PvP modes the first spawn
//! starts "assemble your team" (match.py `_end_assemble`).

use std::{
    collections::{HashMap, VecDeque},
    io,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use indexmap::IndexMap;
use mio::{Events, Interest, Poll, Token, Waker, net::UdpSocket};

use crate::{
    clock::{GameClock, Step, next_tick_after},
    connection::{GameConnections, MAX_CONNECTION_ID, Outgoing, SILENCE_TIME},
    ingress::{GameIngress, MapReadiness, MapReadyStep, MessageKind, WorldReadiness, pong},
};
use orpheus_content::{
    heroes::SOLDIER,
    maps::GameMap,
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_lobby::handoff::Handoff;
use orpheus_script::wire::chunk;
use orpheus_sim::{
    combat::{BotHit, BotReplacement, Combat, ShotContext},
    controller::ControllerFrame,
    egress::GameEgress,
    instance::{Instance, Member, RETAIL_QUANTA, TOURNAMENT_QUANTA, ownership},
    replication::{Record, RoomPlayer, WorldDelivery, show_players},
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::{CollisionBuild, PreparedScene},
};
use orpheus_wire::{
    combat_messages::CombatNotice,
    entity::{EntityUpdate, GAME_MODE_ENTITY, MAX_PAYLOAD, SkinTheme, frame},
    jam::codec::{Fields, Schemas, Value},
    link::{ACK, KIND_FRAME, Packet},
    messages::{Message, encode, locate},
};

const GAME_SOCKET: Token = Token(0);
const REQUESTS: Token = Token(1);
const RECEIVE_BATCH: usize = 64;
const INPUT_PER_PEER: usize = 16;
const INPUT_PER_TURN: usize = 64;
const MAX_PENDING_COMBAT_NOTICES: usize = 256;
/// "Assemble your team" in the PvP modes: 30 s shown on the hero select
/// screen, in whole seconds (ow174/game/match.py `ASSEMBLE_SECONDS`,
/// `COUNTDOWN_EVERY`).
const ASSEMBLE_TIME: Duration = Duration::from_secs(30);
const COUNTDOWN_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub struct MatchPlayer {
    pub name: String,
    pub team: u8,
    pub tournament: bool,
    pub ranked: bool,
}

/// A player of a match with a world: his roster entry, his card, and the
/// hero the lobby gives him until his first pick (ow174/game/server.py
/// `create_match`; Soldier: 76 for 0).
#[derive(Debug)]
pub struct WorldPlayer {
    pub player: MatchPlayer,
    pub card: PlayerCard,
    pub hero: u64,
}

/// Prepares a match's scene off the game thread: the map's collision may
/// have to be built from the install first.
pub type ScenePreparation = Box<dyn FnOnce() -> io::Result<PreparedScene> + Send>;

/// A match as the dashboard and the matchmaker see it (match.py `snapshot`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchSnapshot {
    pub id: MatchId,
    pub map: String,
    pub mode: String,
    /// Before hero select counts down: the players are still joining
    /// (match.py `waiting_for_players`).
    pub waiting: bool,
    pub players: Vec<PlayerSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub name: String,
    /// The hero he plays, or the lobby's before his first pick.
    pub hero: u64,
    pub team: u8,
    pub state: PlayerState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerState {
    /// In the world.
    Playing,
    /// His game connected and loads the map.
    Loading,
    /// His game has not connected, or he left.
    Waiting,
}

impl PlayerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Loading => "loading",
            Self::Waiting => "waiting",
        }
    }
}

/// Builds a map's collision cache from the game install, as the Python
/// server did when a match opened on a map without one
/// (ow174/game/collision.py `_read_cache`, `build`). The collision reads no
/// display strings, so any text locale does.
pub fn collision_build(install: PathBuf) -> Arc<CollisionBuild> {
    Arc::new(move |map, name| {
        let started = Instant::now();
        let opened = orpheus_assets::Install::open(&install, "enUS").map_err(io::Error::other)?;
        let output =
            orpheus_assets::collision_cache(&opened, map, name).map_err(io::Error::other)?;
        tracing::info!(
            "[game] built the collision of {name} in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        Ok(output.bytes)
    })
}

/// Retail puts a whole group in the Practice Range together, up to six on
/// one team (ow174/lobby/matchmaker.py `practice`).
pub const MAX_PRACTICE_PARTY: usize = 6;

pub struct GameService {
    address: SocketAddr,
    requests: mpsc::SyncSender<Request>,
    wake: Arc<Waker>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

pub type MatchId = [u64; 2];

enum Request {
    Open(Box<OpenRequest>),
    Pending {
        connection: u32,
        reply: mpsc::SyncSender<bool>,
    },
    Cancel {
        connection: u32,
        reply: mpsc::SyncSender<bool>,
    },
    Seat {
        connection: u32,
        id: MatchId,
        release: bool,
        reply: mpsc::SyncSender<bool>,
    },
    Skin {
        connection: u32,
        id: MatchId,
        hero: u64,
        skin: SkinTheme,
        reply: mpsc::SyncSender<()>,
    },
    /// The lobby's `.hero` chat command.
    Hero {
        connection: u32,
        id: MatchId,
        hero: u64,
        reply: mpsc::SyncSender<bool>,
    },
    /// More players for a match that is on (server.py `join_match`).
    Join {
        id: MatchId,
        players: Vec<WorldPlayer>,
        reply: mpsc::SyncSender<io::Result<Option<Vec<Handoff>>>>,
    },
    /// A match's scene came from its preparation thread.
    Scene {
        id: MatchId,
        scene: io::Result<Box<PreparedScene>>,
    },
    Snapshot {
        reply: mpsc::SyncSender<Vec<MatchSnapshot>>,
    },
}

struct OpenRequest {
    id: MatchId,
    map: GameMap,
    players: Vec<WorldPlayer>,
    world: Option<WorldStart>,
    reply: mpsc::SyncSender<io::Result<Vec<Handoff>>>,
}

struct WorldStart {
    scene: Box<PreparedScene>,
    combat: Option<Combat>,
}

impl GameService {
    pub fn bind(address: SocketAddr, schemas: Arc<Schemas>) -> io::Result<Self> {
        for id in [20300, 20301, 20302, 20304, 20306, 21601, 21602, 21610] {
            let (_, crc, _) = locate(id).map_err(invalid)?;
            schemas.fields(crc, id).map_err(invalid)?;
        }

        let mut socket = UdpSocket::bind(address)?;
        let address = socket.local_addr()?;
        let poll = Poll::new()?;
        poll.registry()
            .register(&mut socket, GAME_SOCKET, Interest::READABLE)?;
        let wake = Arc::new(Waker::new(poll.registry(), REQUESTS)?);
        let (requests, receiver) = mpsc::sync_channel(RECEIVE_BATCH);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name("orpheus-game".into())
            .spawn(move || run(socket, poll, receiver, worker_stopping, schemas))?;
        Ok(Self {
            address,
            requests,
            wake,
            stopping,
            worker: Some(worker),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    pub fn open_match(
        &self,
        id: [u64; 2],
        map: GameMap,
        players: Vec<MatchPlayer>,
    ) -> io::Result<Vec<Handoff>> {
        let players = players
            .into_iter()
            .map(|player| WorldPlayer {
                player,
                card: PlayerCard::default(),
                hero: SOLDIER,
            })
            .collect();
        self.request(|reply| {
            Request::Open(Box::new(OpenRequest {
                id,
                map,
                players,
                world: None,
                reply,
            }))
        })?
    }

    /// Open the currently ported world bootstrap for one player. The
    /// graph-backed NPC behavior from the enriched bundle is a separate
    /// simulation prerequisite.
    pub fn open_prepared_practice(
        &self,
        id: [u64; 2],
        scene: PreparedScene,
        player: MatchPlayer,
        card: PlayerCard,
    ) -> io::Result<Handoff> {
        let player = WorldPlayer {
            player,
            card,
            hero: SOLDIER,
        };
        let mut handoffs = self.open_prepared(id, scene, vec![player])?;
        handoffs
            .pop()
            .ok_or_else(|| invalid("a solo world requires a seat"))
    }

    /// A group in the Practice Range: everyone on the first team, whose
    /// size is the group's (ow174/lobby/matchmaker.py `_practice`). Seats in
    /// member order.
    pub fn open_practice_party(
        &self,
        id: [u64; 2],
        scene: PreparedScene,
        mut members: Vec<WorldPlayer>,
    ) -> io::Result<Vec<Handoff>> {
        if !(1..=MAX_PRACTICE_PARTY).contains(&members.len()) {
            return Err(invalid("a Practice Range group has one to six players"));
        }
        for member in &mut members {
            member.player.team = 0;
        }
        let mut map = scene.map().clone();
        map.team_sizes = vec![members.len() as u8, 0];
        self.open_ready(id, map, scene, members)
    }

    /// One world for every player of the match, each with his card. Seats
    /// in player order; the slot is the player's index.
    pub fn open_prepared(
        &self,
        id: [u64; 2],
        scene: PreparedScene,
        players: Vec<WorldPlayer>,
    ) -> io::Result<Vec<Handoff>> {
        let map = scene.map().clone();
        self.open_ready(id, map, scene, players)
    }

    /// A match on any map: the seats come back at once, and the players load
    /// the map while `prepare` makes the scene on a thread of its own; their
    /// world comes once it is ready (match.py `_refresh_collision`). When it
    /// fails, their games go back to the menu and the match ends
    /// (`_collision_failure`). Seats in player order.
    pub fn open_world(
        &self,
        id: MatchId,
        map: GameMap,
        players: Vec<WorldPlayer>,
        prepare: ScenePreparation,
    ) -> io::Result<Vec<Handoff>> {
        let handoffs = self.request(|reply| {
            Request::Open(Box::new(OpenRequest {
                id,
                map,
                players,
                world: None,
                reply,
            }))
        })??;
        let requests = self.requests.clone();
        let wake = Arc::clone(&self.wake);
        let spawned = thread::Builder::new()
            .name("orpheus-scene".into())
            .spawn(move || {
                let scene = prepare().map(Box::new);
                if requests.send(Request::Scene { id, scene }).is_ok() {
                    let _ = wake.wake();
                }
            });
        if let Err(error) = spawned {
            let _ = self.requests.try_send(Request::Scene {
                id,
                scene: Err(error),
            });
            self.wake.wake()?;
        }
        Ok(handoffs)
    }

    fn open_ready(
        &self,
        id: MatchId,
        map: GameMap,
        scene: PreparedScene,
        players: Vec<WorldPlayer>,
    ) -> io::Result<Vec<Handoff>> {
        let teams: Vec<u8> = players.iter().map(|entry| entry.player.team).collect();
        let cards: Vec<&PlayerCard> = players.iter().map(|entry| &entry.card).collect();
        check_scene(&scene, &map, &teams, &cards)?;
        self.request(|reply| {
            Request::Open(Box::new(OpenRequest {
                id,
                map,
                players,
                world: Some(WorldStart {
                    scene: Box::new(scene),
                    combat: None,
                }),
                reply,
            }))
        })?
    }

    /// Seats for more players in a match that is on, in player order
    /// (server.py `join_match`); `None` once it has ended or while the game
    /// clock rolls over.
    pub fn join_match(
        &self,
        id: MatchId,
        players: Vec<WorldPlayer>,
    ) -> io::Result<Option<Vec<Handoff>>> {
        self.request(|reply| Request::Join { id, players, reply })?
    }

    /// Every match with its players (server.py `snapshot`).
    pub fn snapshot(&self) -> io::Result<Vec<MatchSnapshot>> {
        self.request(|reply| Request::Snapshot { reply })
    }

    pub fn handoff_pending(&self, connection: u32) -> io::Result<bool> {
        self.request(|reply| Request::Pending { connection, reply })
    }

    pub fn cancel_handoff(&self, connection: u32) -> io::Result<bool> {
        self.request(|reply| Request::Cancel { connection, reply })
    }

    pub fn seat_exists(&self, handoff: &Handoff) -> io::Result<bool> {
        self.seat(handoff, false)
    }

    pub fn release_seat(&self, handoff: &Handoff) -> io::Result<bool> {
        self.seat(handoff, true)
    }

    pub(crate) fn update_skin(
        &self,
        handoff: &Handoff,
        hero: u64,
        skin: SkinTheme,
    ) -> io::Result<()> {
        self.request(|reply| Request::Skin {
            connection: handoff.conn,
            id: handoff.match_id,
            hero,
            skin,
            reply,
        })
    }

    /// Play another hero in the seat's match, as a pick on the hero select
    /// screen does (`ow174/game/server.py` switch_hero for the lobby's `.hero`
    /// command). False when the seat's game is not in the match's world.
    pub(crate) fn switch_hero(&self, handoff: &Handoff, hero: u64) -> io::Result<bool> {
        self.request(|reply| Request::Hero {
            connection: handoff.conn,
            id: handoff.match_id,
            hero,
            reply,
        })
    }

    fn seat(&self, handoff: &Handoff, release: bool) -> io::Result<bool> {
        self.request(|reply| Request::Seat {
            connection: handoff.conn,
            id: handoff.match_id,
            release,
            reply,
        })
    }

    fn request<T>(&self, request: impl FnOnce(mpsc::SyncSender<T>) -> Request) -> io::Result<T> {
        let (reply, receive) = mpsc::sync_channel(1);
        self.requests
            .try_send(request(reply))
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "game worker request queue is full",
                ),
                mpsc::TrySendError::Disconnected(_) => stopped(),
            })?;
        self.wake.wake()?;
        receive.recv().map_err(|_| stopped())
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            self.stopping.store(true, Ordering::Release);
            let _ = self.wake.wake();
            worker
                .join()
                .map_err(|_| io::Error::other("game worker panicked"))??;
        }
        Ok(())
    }
}

impl Drop for GameService {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Player {
    info: MatchPlayer,
    card: PlayerCard,
    /// The lobby's hero, shown until his first pick.
    hero: u64,
    entity: u32,
    gone: bool,
}

impl Player {
    fn quanta(&self) -> u32 {
        if self.info.tournament {
            TOURNAMENT_QUANTA
        } else {
            RETAIL_QUANTA
        }
    }
}

struct Room {
    map: GameMap,
    players: Vec<Player>,
    remaining: usize,
    world: Option<WorldStart>,
    /// PvP: when "assemble your team" ends (match.py `assemble_ends`).
    assemble_ends: Option<Duration>,
    assembled: bool,
    /// Its collision failed: it takes nobody more (match.py `ended`).
    ended: bool,
    /// When the match opened: the snapshot lists the oldest first.
    started: Instant,
}

impl Room {
    fn new(map: GameMap, players: Vec<WorldPlayer>, world: Option<WorldStart>) -> Self {
        let mut room = Self {
            map,
            players: Vec::with_capacity(players.len()),
            remaining: 0,
            world,
            assemble_ends: None,
            assembled: false,
            ended: false,
            started: Instant::now(),
        };
        room.add(players);
        room
    }

    /// The players after those already in the match, each in the next slot
    /// (match.py `add_player`).
    fn add(&mut self, players: Vec<WorldPlayer>) {
        self.remaining += players.len();
        for WorldPlayer { player, card, hero } in players {
            let slot = self.players.len();
            self.players.push(Player {
                info: player,
                card,
                hero: if hero == 0 { SOLDIER } else { hero },
                entity: player_entity(slot),
                gone: false,
            });
        }
    }

    fn assembling(&self) -> bool {
        self.assemble_ends.is_some() && !self.assembled
    }

    fn appearance(&self, slot: usize) -> Option<PlayerAppearance<'_>> {
        let player = &self.players[slot];
        let team = player.info.team;
        let earlier = self.players[..slot].iter().map(|player| player.info.team);
        Some(PlayerAppearance {
            entity: player.entity,
            slot: i8::try_from(slot).ok()?,
            team,
            spawn: spawn_number(&self.map, earlier, team),
            card: &player.card,
        })
    }

    fn message(&self, slot: usize, initial: bool) -> Message {
        let members: Vec<_> = self
            .players
            .iter()
            .map(|player| Member {
                entity: player.entity,
                name: &player.info.name,
                team: player.info.team,
                gone: player.gone,
                ranked: player.info.ranked,
            })
            .collect();
        let instance = Instance {
            map: &self.map,
            members: &members,
        };
        let player = &self.players[slot];
        if initial {
            instance.load(player.entity, player.quanta())
        } else {
            instance.update(player.entity)
        }
    }
}

struct Peer {
    ingress: GameIngress,
    egress: GameEgress<Arc<Record>>,
    loading: MapReadiness,
    ownership: Option<u32>,
    open: bool,
    world: WorldDelivery,
    combat_ui: CombatUi,
    /// The controller step ran (match.py `steps_done >= 4`).
    presented: bool,
    /// When the next countdown frame is due (match.py `next_countdown`).
    next_countdown: Duration,
}

#[derive(Default)]
struct CombatUi {
    notices: VecDeque<CombatNotice>,
    last_notice_by_entity: HashMap<u32, u32>,
    replacements: VecDeque<(Duration, BotReplacement)>,
}

fn initial_controller(
    scene: &PreparedScene,
    tick: u32,
) -> io::Result<orpheus_core::bits::BitWriter> {
    scene
        .controller()
        .controller_frame(
            tick,
            ControllerFrame {
                select_open: true,
                skins: true,
                hide_assemble: true,
                ..ControllerFrame::default()
            },
        )
        .map_err(invalid)
}

/// A scene the worker can open a world on, whose world baselines fit a
/// datagram for every player (`teams` and `cards` in slot order).
fn check_scene(
    scene: &PreparedScene,
    map: &GameMap,
    teams: &[u8],
    cards: &[&PlayerCard],
) -> io::Result<()> {
    if !scene.movement_ready() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the native world bootstrap requires a scene without bot VM graphs",
        ));
    }
    let controller = initial_controller(scene, FIRST_TICK)?;
    for (slot, card) in cards.iter().enumerate() {
        let team = teams[slot];
        let viewer = PlayerAppearance {
            entity: player_entity(slot),
            slot: i8::try_from(slot).map_err(invalid)?,
            team,
            spawn: spawn_number(map, teams[..slot].iter().copied(), team),
            card,
        };
        let creates = scene.initial_world(&viewer)?;
        let scripts = [
            script_record(viewer.entity, &controller)?,
            script_record(GAME_MODE_ENTITY, scene.mode_ui())?,
        ];
        for record in creates.iter().chain(&scripts) {
            let packet = frame(u32::MAX, u32::MAX, &[], &[], &[record]).map_err(invalid)?;
            if packet.len() > MAX_PAYLOAD {
                return Err(invalid("a world baseline exceeds the game datagram budget"));
            }
        }
    }
    Ok(())
}

fn script_record(entity: u32, payload: &orpheus_core::bits::BitWriter) -> io::Result<EntityUpdate> {
    Ok(EntityUpdate {
        entity,
        chunk: Some(chunk(payload, 0, 1).map_err(invalid)?),
        ..EntityUpdate::default()
    })
}

fn player_entity(slot: usize) -> u32 {
    0xa000_0000 | ((slot as u32 + 1) << 8)
}

/// The n-th player of a team takes the team's n-th spawn point; in a
/// free-for-all the n-th player of the match (ow174/game/match.py Player).
fn spawn_number(map: &GameMap, earlier: impl Iterator<Item = u8>, team: u8) -> usize {
    earlier
        .filter(|&other| map.free_for_all || other == team)
        .count()
}

impl Peer {
    fn new(owner: u32) -> Self {
        Self {
            ingress: GameIngress::default(),
            egress: GameEgress::default(),
            loading: MapReadiness::default(),
            ownership: None,
            open: false,
            world: WorldDelivery::new(owner),
            combat_ui: CombatUi::default(),
            presented: false,
            next_countdown: Duration::ZERO,
        }
    }

    fn clear_world(&mut self) {
        self.world.clear();
        self.combat_ui = CombatUi::default();
        self.presented = false;
    }

    fn bot_hit(&mut self, hit: BotHit) {
        self.world.queue_health(hit.health);
        for notice in hit.notices.into_iter().flatten() {
            if self.combat_ui.notices.len() == MAX_PENDING_COMBAT_NOTICES {
                self.combat_ui.notices.pop_front();
            }
            self.combat_ui.notices.push_back(notice);
        }
    }

    fn flush_combat(&mut self, schemas: &Schemas, now: Duration) {
        for _ in 0..self.combat_ui.notices.len() {
            let notice = self
                .combat_ui
                .notices
                .pop_front()
                .expect("pending combat notice");
            let entity = notice.required_identity();
            if entity.is_some_and(|entity| !self.world.identity_ready(entity))
                || (notice.needs_mode_ui() && !self.world.mode_ready())
            {
                self.combat_ui.notices.push_back(notice);
                continue;
            }
            if let Ok(bytes) = notice.encode(schemas) {
                if let (Ok(ordinal), Some(entity)) = (self.egress.queue_reliable(bytes), entity) {
                    self.combat_ui.last_notice_by_entity.insert(entity, ordinal);
                }
            }
        }
        for _ in 0..self.combat_ui.replacements.len() {
            let (expires, replacement) = self
                .combat_ui
                .replacements
                .pop_front()
                .expect("pending bot replacement");
            let entity = replacement.retired;
            let has_notice = self
                .combat_ui
                .notices
                .iter()
                .any(|notice| notice.required_identity() == Some(entity));
            let waiting_for_reliable = self
                .combat_ui
                .last_notice_by_entity
                .get(&entity)
                .is_some_and(|&ordinal| !self.egress.delivered_through(ordinal));
            if now < expires
                && (has_notice || waiting_for_reliable || self.world.health_pending(entity))
            {
                self.combat_ui
                    .replacements
                    .push_back((expires, replacement));
            } else {
                self.combat_ui
                    .notices
                    .retain(|notice| notice.required_identity() != Some(entity));
                self.combat_ui.last_notice_by_entity.remove(&entity);
                self.world.replace_bot(replacement);
            }
        }
    }

    fn reliable(&mut self, schemas: &Schemas, message: Message) -> io::Result<u32> {
        let bytes = encode(schemas, message.id, &message.fields).map_err(invalid)?;
        self.egress.queue_reliable(bytes).map_err(invalid)
    }

    /// The seconds left of "assemble your team" in the game mode entity's
    /// v1303, every second; the client shows them rounded up (match.py
    /// `_send_countdown`).
    fn send_countdown(
        &mut self,
        scene: &PreparedScene,
        ends: Duration,
        now: Duration,
    ) -> io::Result<()> {
        let frame = scene.countdown_frame(ends.saturating_sub(now).as_secs_f64())?;
        if self.world.queue_mode_frame(&frame)? {
            self.next_countdown = now.saturating_add(COUNTDOWN_EVERY);
        }
        Ok(())
    }

    /// The due spawn steps; true when this one put the player in the world.
    fn advance_loading(
        &mut self,
        schemas: &Schemas,
        room: &Room,
        slot: usize,
        now: Duration,
        tick: u32,
    ) -> io::Result<bool> {
        let mut spawned = false;
        let player = &room.players[slot];
        let ready = WorldReadiness {
            ownership_delivered: self
                .ownership
                .is_some_and(|id| self.egress.delivered_through(id)),
            collision_ready: room.world.is_some(),
            bot_scripts_ready: room
                .world
                .as_ref()
                .is_some_and(|world| world.scene.scripted_bots() == 0),
        };
        while let Some(step) = self.loading.take_due(now, ready) {
            match step {
                MapReadyStep::Ownership => {
                    self.ownership = Some(self.reliable(schemas, ownership(player.entity))?);
                }
                MapReadyStep::Game => {
                    self.reliable(schemas, room.message(slot, false))?;
                }
                MapReadyStep::World => {
                    let world = room.world.as_ref().expect("map-entry prerequisites");
                    let viewer = room.appearance(slot).expect("prepared player");
                    self.world.start(&world.scene, &viewer, tick)?;
                    spawned = true;
                }
                MapReadyStep::Controller => {
                    self.world.presentation_due = true;
                    self.presented = true;
                }
            }
        }
        if self.world.presentation_due {
            let world = room.world.as_ref().expect("world started");
            self.world.present(&world.scene, tick)?;
        }
        Ok(spawned)
    }

    /// This tick's steps of the player (match.py `update`): the end of
    /// "assemble your team", the spawn steps, and the countdown. The first
    /// spawn in a PvP mode starts the countdown (`_spawn_steps`).
    fn update(
        &mut self,
        schemas: &Schemas,
        room: &mut Room,
        slot: usize,
        assembled: bool,
        now: Duration,
        tick: u32,
    ) -> io::Result<()> {
        if let (true, Some(world), Some(ends)) =
            (assembled && self.presented, &room.world, room.assemble_ends)
        {
            self.send_countdown(&world.scene, ends, now)?;
            self.world.end_assemble(&world.scene, tick)?;
        }
        self.world.set_assembling(room.assembling());
        let spawned = self.advance_loading(schemas, room, slot, now, tick)?;
        let pvp = room
            .world
            .as_ref()
            .is_some_and(|world| !world.scene.controller().practice());
        if spawned && pvp && room.assemble_ends.is_none() {
            room.assemble_ends = Some(now.saturating_add(ASSEMBLE_TIME));
            tracing::info!(
                "[game] {}: assemble your team, {} s",
                room.map.name,
                ASSEMBLE_TIME.as_secs()
            );
        }
        if let (true, Some(world), Some(ends)) = (
            room.assembling() && self.presented && now >= self.next_countdown,
            &room.world,
            room.assemble_ends,
        ) {
            self.send_countdown(&world.scene, ends, now)?;
        }
        Ok(())
    }
}

struct State {
    schemas: Arc<Schemas>,
    clock: GameClock,
    links: GameConnections,
    rooms: HashMap<MatchId, Room>,
    seats: HashMap<u32, (MatchId, usize)>,
    peers: IndexMap<u32, Peer>,
    next_connection: u32,
    next_input_peer: usize,
}

impl State {
    fn new(schemas: Arc<Schemas>) -> Self {
        Self {
            schemas,
            clock: GameClock::default(),
            links: GameConnections::default(),
            rooms: HashMap::new(),
            seats: HashMap::new(),
            peers: IndexMap::new(),
            next_connection: 1,
            next_input_peer: 0,
        }
    }

    fn open_match(
        &mut self,
        id: MatchId,
        map: GameMap,
        players: Vec<WorldPlayer>,
        world: Option<WorldStart>,
        now: Duration,
    ) -> io::Result<Vec<Handoff>> {
        let remaining = players.len();
        if self.clock.draining() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "game clock is draining",
            ));
        }
        if self.rooms.contains_key(&id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "match already exists",
            ));
        }
        if remaining == 0 || remaining > usize::from(u8::MAX) {
            return Err(invalid("invalid match roster size"));
        }
        let room = Room::new(map, players, world);
        self.check_roster(&room)?;
        let tickets = self.reserve(id, 0..remaining, now)?;
        self.rooms.insert(id, room);
        Ok(tickets)
    }

    /// More players for a match that is on, in the next slots (server.py
    /// `join_match`); `None` once it has ended or while the clock drains.
    fn join_match(
        &mut self,
        id: MatchId,
        players: Vec<WorldPlayer>,
        now: Duration,
    ) -> io::Result<Option<Vec<Handoff>>> {
        if self.clock.draining() || self.rooms.get(&id).is_none_or(|room| room.ended) {
            return Ok(None);
        }
        let room = self.rooms.get_mut(&id).expect("running match");
        let first = room.players.len();
        if players.is_empty() || first + players.len() > usize::from(u8::MAX) {
            return Err(invalid("invalid match roster size"));
        }
        room.add(players);
        let room = &self.rooms[&id];
        let checked = self.check_roster(room).and_then(|()| {
            let Some(world) = &room.world else {
                return Ok(());
            };
            let teams: Vec<u8> = room.players.iter().map(|player| player.info.team).collect();
            let cards: Vec<&PlayerCard> = room.players.iter().map(|player| &player.card).collect();
            check_scene(&world.scene, &room.map, &teams, &cards)
        });
        let count = room.players.len();
        let reserved = checked.and_then(|()| self.reserve(id, first..count, now));
        if reserved.is_err() {
            let room = self.rooms.get_mut(&id).expect("running match");
            room.players.truncate(first);
            room.remaining -= count - first;
        }
        reserved.map(Some)
    }

    /// The roster's 20300 fits a datagram.
    fn check_roster(&self, room: &Room) -> io::Result<()> {
        let initial = room.message(0, true);
        let bytes = encode(&self.schemas, initial.id, &initial.fields).map_err(invalid)?;
        let payload = frame(u32::MAX, u32::MAX, &[(0, bytes)], &[], &[]).map_err(invalid)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(invalid("match roster exceeds the game datagram budget"));
        }
        Ok(())
    }

    /// A seat for each of these slots of the match.
    fn reserve(
        &mut self,
        id: MatchId,
        slots: std::ops::Range<usize>,
        now: Duration,
    ) -> io::Result<Vec<Handoff>> {
        let mut tickets = Vec::with_capacity(slots.len());
        for _ in slots.clone() {
            let connection = self.next_connection;
            self.next_connection = self.next_connection % MAX_CONNECTION_ID + 1;
            if self.seats.contains_key(&connection) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "game seat ID is still in use",
                ));
            }
            tickets.push(Handoff::new(connection, id)?);
        }
        for (slot, ticket) in slots.zip(&tickets) {
            let reserved = self
                .links
                .reserve_handoff(ticket, u64::from(ticket.conn), now);
            assert!(reserved, "seat and connection tables are in sync");
            self.seats.insert(ticket.conn, (id, slot));
        }
        Ok(tickets)
    }

    /// The match's scene is ready, or its collision failed: then every game
    /// goes back to the menu, those not connected yet lose their seats, and
    /// the match takes nobody more (match.py `_collision_failure`).
    fn scene_ready(&mut self, id: MatchId, scene: io::Result<Box<PreparedScene>>, now: Duration) {
        let Some(room) = self.rooms.get_mut(&id) else {
            return;
        };
        let scene = scene.and_then(|scene| {
            let teams: Vec<u8> = room.players.iter().map(|player| player.info.team).collect();
            let cards: Vec<&PlayerCard> = room.players.iter().map(|player| &player.card).collect();
            check_scene(&scene, &room.map, &teams, &cards)?;
            Ok(scene)
        });
        let error = match scene {
            Ok(scene) => {
                room.world = Some(WorldStart {
                    scene,
                    combat: None,
                });
                return;
            }
            Err(error) => error,
        };
        tracing::error!(
            "[game] match {:04X} on {}: collision unavailable: {error}",
            id[0] & 0xFFFF,
            room.map.name
        );
        room.ended = true;
        let seated: Vec<u32> = self
            .seats
            .iter()
            .filter(|(_, seat)| seat.0 == id)
            .map(|(&connection, _)| connection)
            .collect();
        for connection in seated {
            if !self.links.cancel_handoff(connection) && self.peers.contains_key(&connection) {
                self.send_home(connection, now);
            }
        }
    }

    /// Every match with its players and their state (match.py `snapshot`).
    fn snapshot(&self) -> Vec<MatchSnapshot> {
        let mut peers: HashMap<MatchId, HashMap<usize, &Peer>> = HashMap::new();
        for (connection, peer) in &self.peers {
            let (id, slot) = self.seats[connection];
            peers.entry(id).or_default().insert(slot, peer);
        }
        let mut rooms: Vec<_> = self.rooms.iter().collect();
        rooms.sort_by_key(|(_, room)| room.started);
        rooms
            .into_iter()
            .map(|(&id, room)| MatchSnapshot {
                id,
                map: room.map.name.clone(),
                mode: room.map.mode_name.clone(),
                waiting: room.assemble_ends.is_none(),
                players: room
                    .players
                    .iter()
                    .enumerate()
                    .map(|(slot, player)| {
                        let peer = peers.get(&id).and_then(|peers| peers.get(&slot));
                        let state = match peer {
                            _ if player.gone => PlayerState::Waiting,
                            Some(peer) if peer.world.world_started() => PlayerState::Playing,
                            Some(_) => PlayerState::Loading,
                            None => PlayerState::Waiting,
                        };
                        PlayerSnapshot {
                            name: player.info.name.clone(),
                            hero: peer
                                .and_then(|peer| peer.world.hero())
                                .unwrap_or(player.hero),
                            team: player.info.team,
                            state,
                        }
                    })
                    .collect(),
            })
            .collect()
    }

    fn handle_request(&mut self, request: Request, now: Duration) {
        match request {
            Request::Open(request) => {
                let OpenRequest {
                    id,
                    map,
                    players,
                    world,
                    reply,
                } = *request;
                let result = self.open_match(id, map, players, world, now);
                if let Err(mpsc::SendError(Ok(tickets))) = reply.send(result) {
                    for ticket in tickets {
                        self.links.cancel_handoff(ticket.conn);
                    }
                }
            }
            Request::Join { id, players, reply } => {
                let result = self.join_match(id, players, now);
                if let Err(mpsc::SendError(Ok(Some(tickets)))) = reply.send(result) {
                    for ticket in tickets {
                        self.links.cancel_handoff(ticket.conn);
                    }
                }
            }
            Request::Scene { id, scene } => self.scene_ready(id, scene, now),
            Request::Snapshot { reply } => {
                let _ = reply.send(self.snapshot());
            }
            Request::Pending { connection, reply } => {
                let _ = reply.send(self.links.handoff_pending(connection, now));
            }
            Request::Cancel { connection, reply } => {
                let _ = reply.send(self.links.cancel_handoff(connection));
            }
            Request::Seat {
                connection,
                id,
                release,
                reply,
            } => {
                let exists = self
                    .seats
                    .get(&connection)
                    .is_some_and(|&(match_id, _)| match_id == id);
                if release && exists && !self.links.cancel_handoff(connection) {
                    self.send_home(connection, now);
                }
                let _ = reply.send(exists);
            }
            Request::Skin {
                connection,
                id,
                hero,
                skin,
                reply,
            } => {
                if let Some(&(_, slot)) = self.seats.get(&connection).filter(|seat| seat.0 == id) {
                    let room = self.rooms.get_mut(&id).expect("reserved match");
                    if room.world.is_some() {
                        room.players[slot].card.skins.insert(hero, skin);
                        if let Some(peer) = self.peers.get_mut(&connection) {
                            peer.world.queue_skin(hero, self.clock.frame(), now);
                        }
                    }
                }
                let _ = reply.send(());
            }
            Request::Hero {
                connection,
                id,
                hero,
                reply,
            } => {
                let _ = reply.send(self.switch_hero(connection, id, hero, now));
            }
        }
    }

    /// A pick of the hero, as the client's own pick message (21616) carries
    /// it, queued with the seat's other game input.
    fn switch_hero(&mut self, connection: u32, id: MatchId, hero: u64, now: Duration) -> bool {
        if self.clock.draining() || !self.seats.get(&connection).is_some_and(|seat| seat.0 == id) {
            return false;
        }
        let (_, slot) = self.seats[&connection];
        let room = &self.rooms[&id];
        let (Some(world), Some(peer)) = (&room.world, self.peers.get_mut(&connection)) else {
            return false;
        };
        if world.scene.heroes().get(hero).is_none() || !peer.world.world_started() {
            return false;
        }
        let pick = Message {
            id: CLIENT_GAME_MESSAGE,
            fields: Fields::from([
                (
                    0x78,
                    Value::Struct(Fields::from([(
                        0,
                        Value::Unsigned(u64::from(room.players[slot].entity)),
                    )])),
                ),
                (0x80, Value::Blob(hero.to_le_bytes().to_vec())),
                (0xa8, Value::Unsigned(PICK_HERO)),
                (0xb0, Value::Signed(0)),
            ]),
        };
        peer.world.queue_game_message(self.clock.frame(), now, pick);
        true
    }

    fn retire(&mut self, connection: u32) {
        self.peers.shift_remove(&connection);
        if let Some((id, slot)) = self.seats.remove(&connection) {
            let room = self.rooms.get_mut(&id).expect("reserved match");
            room.players[slot].gone = true;
            room.remaining -= 1;
            if room.remaining == 0 {
                self.rooms.remove(&id);
            }
        }
    }

    fn receive(&mut self, socket: &UdpSocket, bytes: &[u8], address: SocketAddr, now: Duration) {
        let Ok(received) = self.links.receive(bytes, address, now) else {
            return;
        };
        let connection = received.connection;
        let (id, slot) = self.seats[&connection];
        if received.admitted {
            self.peers
                .insert(connection, Peer::new(self.rooms[&id].players[slot].entity));
        }
        let peer = self.peers.get_mut(&connection).expect("admitted peer");
        if let Some((ack, bits)) = received.ack {
            let report = peer.egress.acknowledge(ack, bits);
            if !self.clock.draining() {
                peer.world.acknowledge(report, now);
            }
        }
        if let Some(Ok(control)) = received.control {
            let _ = send(socket, control);
        }
        if received.closed {
            self.retire(connection);
            return;
        }
        peer.open |= received.opened;
        if self.clock.draining() {
            if received.admitted {
                self.send_home(connection, now);
            }
            return;
        }
        let room = &self.rooms[&id];
        if received.opened {
            let _ = peer.reliable(&self.schemas, room.message(slot, true));
        }
        if let Some(payload) = received.payload {
            let Peer {
                ingress,
                egress,
                loading,
                world,
                ..
            } = peer;
            let commands = ingress.receive(&self.schemas, &payload, |message| {
                match MessageKind::from(message.id) {
                    MessageKind::Ping => {
                        let received = wall_nanoseconds();
                        if let Ok(pong) = pong(&message.fields, received, wall_nanoseconds()) {
                            if let Ok(bytes) = encode(&self.schemas, pong.id, &pong.fields) {
                                let _ = egress.queue_reliable(bytes);
                            }
                        }
                    }
                    MessageKind::MapLoaded => {
                        loading.mark_loaded(now);
                    }
                    MessageKind::GameMessage if room.world.is_some() => {
                        world.queue_game_message(self.clock.frame(), now, message);
                    }
                    _ => {}
                }
            });
            if let (Ok(commands), Some(_)) = (commands, &room.world) {
                world.queue_commands(self.clock.frame(), &commands);
            }
        }
    }

    /// Bounded round-robin input work between receive batches and tick checks.
    /// A busy peer cannot consume the next peer's whole turn.
    fn advance_input(&mut self, now: Duration) -> bool {
        let count = self.peers.len();
        if count == 0 || self.clock.draining() {
            return false;
        }
        let start = self.next_input_peer % count;
        let tick = self.clock.frame();
        let mut remaining = INPUT_PER_TURN;
        for offset in 0..count {
            let index = (start + offset) % count;
            let (&connection, peer) = self.peers.get_index(index).expect("active peer index");
            if !peer.world.has_input_work() {
                continue;
            }
            let (id, slot) = self.seats[&connection];
            // A new body takes no id another client of the room still holds
            // (ow174/game/match.py `_next_body_id`).
            let owner = player_entity(slot);
            let retained: Vec<u32> = self
                .peers
                .iter()
                .filter(|&(&other, _)| other != connection && self.seats[&other].0 == id)
                .flat_map(|(_, other)| other.world.retained_bodies(owner, now))
                .collect();
            let (_, peer) = self.peers.get_index_mut(index).expect("active peer index");
            peer.world.retain_elsewhere(retained);
            let room = self.rooms.get_mut(&id).expect("prepared room with input");
            let world = room.world.as_ref().expect("prepared world with input");
            let viewer = room.appearance(slot).expect("prepared player");
            let context = ShotContext {
                team: viewer.team,
                quanta: room.players[slot].quanta(),
                tick,
                now,
            };
            let budget = remaining.min(INPUT_PER_PEER);
            let (used, leave) = peer
                .world
                .advance_input(&world.scene, &viewer, context.quanta, budget)
                .unwrap_or((budget, true));
            remaining -= used;
            self.next_input_peer = (index + 1) % count;
            let shots = peer.world.take_hitscan();
            if (!leave && peer.world.combat_body_active()) || !shots.is_empty() {
                let world = room.world.as_mut().expect("prepared world with combat");
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                for shot in shots {
                    if let Ok(Some(hit)) =
                        combat.hitscan(&mut world.scene, shot, context, |entity| {
                            peer.world.identity_ready(entity)
                        })
                    {
                        peer.bot_hit(hit);
                    }
                }
            }
            if leave {
                peer.clear_world();
                peer.loading = MapReadiness::default();
                if self.links.begin_leaving(connection, now) {
                    let _ = peer.reliable(&self.schemas, leave_message());
                }
            }
            if remaining == 0 {
                break;
            }
        }
        self.peers.values().any(|peer| peer.world.has_input_work())
    }

    fn send_home(&mut self, connection: u32, now: Duration) {
        if self.links.begin_leaving(connection, now) {
            let peer = self.peers.get_mut(&connection).expect("admitted peer");
            let _ = peer.reliable(&self.schemas, leave_message());
        }
    }

    /// Every client of a room learns about the other players this tick
    /// (ow174/game/match.py `update`); their cards go reliably.
    fn show_players(&mut self, tick: u32, now: Duration, frames: bool) {
        for (&id, room) in &self.rooms {
            let Some(world) = &room.world else {
                continue;
            };
            let mut connections = vec![None; room.players.len()];
            let mut players: Vec<_> = (0..room.players.len())
                .map(|slot| RoomPlayer {
                    appearance: room.appearance(slot).expect("seated player"),
                    world: None,
                })
                .collect();
            for (&connection, peer) in &mut self.peers {
                let (match_id, slot) = self.seats[&connection];
                if match_id == id {
                    connections[slot] = Some(connection);
                    players[slot].world = Some(&mut peer.world);
                }
            }
            if connections.iter().all(Option::is_none) {
                continue;
            }
            let cards = show_players(&world.scene, &mut players, tick, now, frames);
            drop(players);
            for (connection, messages) in connections.into_iter().zip(cards) {
                let Some(peer) = connection.and_then(|connection| self.peers.get_mut(&connection))
                else {
                    continue;
                };
                for message in messages {
                    let _ = peer.reliable(&self.schemas, message);
                }
            }
        }
    }

    fn tick(&mut self, socket: &UdpSocket, now: Duration, missed: u64) {
        let step = self.clock.step(missed, !self.seats.is_empty());
        let (tick, send_frames, drain, force) = match step {
            Step::Simulate { frame, send_world } => (frame, send_world, false, false),
            Step::Drain {
                frame,
                entered,
                force,
                send_reliable,
            } => {
                if entered {
                    for (&connection, peer) in &mut self.peers {
                        peer.clear_world();
                        if self.links.begin_leaving(connection, now) {
                            let _ = peer.reliable(&self.schemas, leave_message());
                        }
                    }
                }
                (frame, send_reliable, true, force)
            }
        };
        for expired in self.links.expire(now, force) {
            self.retire(expired.connection);
        }
        let mut spawned = HashMap::new();
        if !drain {
            for room in self.rooms.values_mut() {
                if let Some(world) = &mut room.world {
                    world.scene.step_bots(tick);
                }
            }
            for (&connection, peer) in &mut self.peers {
                let (id, slot) = self.seats[&connection];
                let room = self.rooms.get_mut(&id).expect("admitted room");
                let Some(world) = &mut room.world else {
                    continue;
                };
                let player = &room.players[slot];
                let context = ShotContext {
                    team: player.info.team,
                    quanta: player.quanta(),
                    tick,
                    now,
                };
                let mut hits = Vec::new();
                let _ = peer.world.advance_projectiles(tick, |source, flight| {
                    let combat = world
                        .combat
                        .get_or_insert_with(|| Combat::new(&world.scene));
                    combat.step_projectile(&mut world.scene, source, flight, context, &mut hits)
                });
                for hit in hits {
                    peer.bot_hit(hit);
                }
            }
            for (&id, room) in &mut self.rooms {
                if let Some(world) = &mut room.world {
                    if let Some(combat) = &mut world.combat {
                        if let Ok(replacements) = combat.respawn_due(&mut world.scene, tick, now) {
                            if !replacements.is_empty() {
                                spawned.insert(id, replacements);
                            }
                        }
                        combat.record_poses(&world.scene, tick);
                    }
                }
            }
        }
        // match.py `update`: the countdown ends before the players' steps.
        let mut assembled = Vec::new();
        if !drain {
            for (&id, room) in &mut self.rooms {
                if room
                    .assemble_ends
                    .is_some_and(|ends| !room.assembled && now >= ends)
                {
                    room.assembled = true;
                    tracing::info!("[game] match {:04X}: heroes assembled", id[0] & 0xFFFF);
                    assembled.push(id);
                }
            }
        }
        for (&connection, peer) in &mut self.peers {
            let (id, slot) = self.seats[&connection];
            let room = self.rooms.get_mut(&id).expect("admitted room");
            let ending = assembled.contains(&id);
            if !drain
                && peer
                    .update(&self.schemas, room, slot, ending, now, tick)
                    .is_err()
            {
                peer.clear_world();
                peer.loading = MapReadiness::default();
                if self.links.begin_leaving(connection, now) {
                    let _ = peer.reliable(&self.schemas, leave_message());
                }
            }
            if let Some(replacements) = spawned.get(&id).filter(|_| peer.world.world_started()) {
                peer.combat_ui.replacements.extend(
                    replacements
                        .iter()
                        .cloned()
                        .map(|bot| (now.saturating_add(SILENCE_TIME), bot)),
                );
            }
        }
        if !drain {
            self.show_players(tick, now, send_frames);
        }
        for (&connection, peer) in &mut self.peers {
            let (id, _) = self.seats[&connection];
            let room = &self.rooms[&id];
            if send_frames && peer.open {
                if !drain {
                    peer.flush_combat(&self.schemas, now);
                }
                let movements = if drain {
                    Vec::new()
                } else {
                    room.world
                        .as_ref()
                        .map_or_else(Vec::new, |world| peer.world.movement(&world.scene, tick))
                };
                let _ =
                    peer.world
                        .send_frame(tick, movements, drain, &mut peer.egress, |payload| {
                            let packet = Packet {
                                conn: connection,
                                kind: KIND_FRAME,
                                flags: ACK,
                                payload,
                                ..Packet::default()
                            };
                            let outgoing = self.links.transmit(packet).map_err(|error| {
                                io::Error::other(format!("game link: {error:?}"))
                            })?;
                            send(socket, outgoing)
                        });
            }
        }
        self.clock.renew_if_idle(!self.seats.is_empty(), false);
    }
}

fn leave_message() -> Message {
    Message {
        id: 20304,
        fields: Fields::from([(0x78, Value::Unsigned(0)), (0x79, Value::Bool(false))]),
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "game worker has stopped")
}

fn wall_nanoseconds() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.as_nanos()).unwrap_or(u64::MAX)
}

fn send(socket: &UdpSocket, outgoing: Outgoing) -> io::Result<u32> {
    let count = socket.send_to(&outgoing.datagram, outgoing.address)?;
    if count != outgoing.datagram.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "partial game datagram",
        ));
    }
    Ok(outgoing.sequence)
}

fn run(
    socket: UdpSocket,
    mut poll: Poll,
    requests: mpsc::Receiver<Request>,
    stopping: Arc<AtomicBool>,
    schemas: Arc<Schemas>,
) -> io::Result<()> {
    let mut state = State::new(schemas);
    let start = Instant::now();
    let mut due = Duration::ZERO;
    let mut events = Events::with_capacity(2);
    // An oversized datagram must never be accepted as a truncated valid one.
    let mut buffer = [0; 65_536];
    let mut socket_ready = false;
    while !stopping.load(Ordering::Acquire) {
        let mut handled = 0;
        for request in requests.try_iter().take(RECEIVE_BATCH) {
            state.handle_request(request, start.elapsed());
            handled += 1;
        }
        if socket_ready {
            for _ in 0..RECEIVE_BATCH {
                match socket.recv_from(&mut buffer) {
                    Ok((count, address)) => {
                        state.receive(&socket, &buffer[..count], address, start.elapsed());
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        socket_ready = false;
                        break;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted
                                | io::ErrorKind::ConnectionReset
                                | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let input_ready = state.advance_input(start.elapsed());
        let now = start.elapsed();
        if now >= due {
            let (next, missed) = next_tick_after(due, now)
                .ok_or_else(|| io::Error::other("game tick clock exhausted"))?;
            due = next;
            state.tick(&socket, now, missed);
        }
        let timeout = if socket_ready || input_ready || handled == RECEIVE_BATCH {
            Duration::ZERO
        } else {
            due.saturating_sub(start.elapsed())
        };
        match poll.poll(&mut events, Some(timeout)) {
            Ok(()) => {
                socket_ready |= events.iter().any(|event| event.token() == GAME_SOCKET);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
