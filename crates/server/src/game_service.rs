//! The native UDP match worker. It owns seats, connections and frame timing.
//!
//! A prepared Practice Range scene can advance from loading into one world
//! for all its players, who see each other (ow174/game/match.py). Maps that
//! need the unported bot VM retain that requirement.

use std::{
    collections::{HashMap, VecDeque},
    io,
    net::SocketAddr,
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
    maps::{GameMap, PRACTICE_RANGE_MODE},
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
    stats::PlayerStats,
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_lobby::handoff::Handoff;
use orpheus_script::wire::chunk;
use orpheus_sim::{
    combat::{
        BotHit, BotReplacement, Combat, CombatPlayer, PosedShot, Shooter, ShotContext,
        apply_player_hits, deliver_body_messages, heal_players,
    },
    controller::ControllerFrame,
    egress::GameEgress,
    instance::{Instance, Member, RETAIL_QUANTA, TOURNAMENT_QUANTA, ownership},
    projectiles::HEAL_RADIUS,
    replication::{Record, RoomPlayer, WorldDelivery, show_players},
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::PreparedScene,
};
use orpheus_wire::entity::Component as WorldComponent;
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

#[derive(Debug)]
pub struct MatchPlayer {
    pub name: String,
    pub team: u8,
    pub tournament: bool,
    pub ranked: bool,
}

/// A player of a match with a world: his roster entry and his card.
#[derive(Debug)]
pub struct WorldPlayer {
    pub player: MatchPlayer,
    pub card: PlayerCard,
}

/// Retail puts a whole group in the Practice Range together, up to six on
/// one team (ow174/lobby/matchmaker.py `practice`).
pub const MAX_PRACTICE_PARTY: usize = 6;

pub struct GameService {
    address: SocketAddr,
    requests: mpsc::SyncSender<Request>,
    wake: Waker,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

type MatchId = [u64; 2];

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
    /// The lobby's `.leave` chat command.
    Home {
        connection: u32,
        id: MatchId,
        reply: mpsc::SyncSender<bool>,
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
        let wake = Waker::new(poll.registry(), REQUESTS)?;
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
        let mut handoffs = self.open_prepared(id, scene, vec![WorldPlayer { player, card }])?;
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
        self.open_world(id, map, scene, members)
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
        self.open_world(id, map, scene, players)
    }

    fn open_world(
        &self,
        id: [u64; 2],
        map: GameMap,
        scene: PreparedScene,
        players: Vec<WorldPlayer>,
    ) -> io::Result<Vec<Handoff>> {
        if scene.map().mode_guid != PRACTICE_RANGE_MODE || !scene.movement_ready() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the native world bootstrap requires the Practice Range without bot VM graphs",
            ));
        }
        let controller = initial_controller(&scene, FIRST_TICK)?;
        for (slot, entry) in players.iter().enumerate() {
            let team = entry.player.team;
            let viewer = PlayerAppearance {
                entity: player_entity(slot),
                slot: i8::try_from(slot).map_err(invalid)?,
                team,
                spawn: spawn_number(&map, players[..slot].iter().map(|p| p.player.team), team),
                card: &entry.card,
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

    /// Send the seat's game back to the menu with 20304 (`ow174/game/server.py`
    /// send_home for the lobby's `.leave` command). False when no game is
    /// connected to the seat.
    pub(crate) fn send_home(&self, handoff: &Handoff) -> io::Result<bool> {
        self.request(|reply| Request::Home {
            connection: handoff.conn,
            id: handoff.match_id,
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
    entity: u32,
    gone: bool,
    /// The numbers his Tab board shows (match.py `Player.stats`).
    stats: PlayerStats,
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
}

impl Room {
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

/// The connected players of a room, by slot.
fn room_peers<'p>(
    peers: &'p mut IndexMap<u32, Peer>,
    seats: &HashMap<u32, (MatchId, usize)>,
    id: MatchId,
    players: usize,
) -> Vec<Option<&'p mut Peer>> {
    let mut room: Vec<_> = (0..players).map(|_| None).collect();
    for (connection, peer) in peers.iter_mut() {
        let (match_id, slot) = seats[connection];
        if match_id == id {
            room[slot] = Some(peer);
        }
    }
    room
}

/// A Biotic Field's heal this tick (projectiles.py `_field`).
struct FieldHeal {
    healer: Shooter,
    centre: [f64; 3],
    amount: f64,
}

/// match.py `_board`: the player's board numbers, with their medals among
/// his teammates (every player in a free-for-all) and his hero's stats.
fn board(room: &Room, slot: usize, scene: &PreparedScene, hero: Option<u64>) -> WorldComponent {
    let me = &room.players[slot];
    let free_for_all = room.map.free_for_all;
    let mut team: Vec<usize> = (0..room.players.len())
        .filter(|&other| {
            let player = &room.players[other];
            !player.gone && (free_for_all || player.info.team == me.info.team)
        })
        .collect();
    if !team.contains(&slot) {
        team.push(slot);
    }
    let mode = room.map.mode_guid;
    let stats: Vec<_> = team
        .iter()
        .map(|&other| &room.players[other].stats)
        .collect();
    let medals = scene.stats().medals(&stats, mode);
    let index = team
        .iter()
        .position(|&other| other == slot)
        .expect("the player is on his team");
    scene
        .stats()
        .component(&me.stats, mode, hero, Some(&medals[index]))
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
        }
    }

    fn clear_world(&mut self) {
        self.world.clear();
        self.combat_ui = CombatUi::default();
    }

    /// A hit's notices for this client (the shooter's).
    fn notice(&mut self, notice: CombatNotice) {
        if self.combat_ui.notices.len() == MAX_PENDING_COMBAT_NOTICES {
            self.combat_ui.notices.pop_front();
        }
        self.combat_ui.notices.push_back(notice);
    }

    fn flush_combat(&mut self, schemas: &Schemas, now: Duration) {
        for _ in 0..self.combat_ui.notices.len() {
            let notice = self
                .combat_ui
                .notices
                .pop_front()
                .expect("pending combat notice");
            let entity = notice.required_identity();
            if entity.is_some_and(|entity| !self.world.combat_identity_ready(entity))
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

    fn advance_loading(
        &mut self,
        schemas: &Schemas,
        room: &Room,
        slot: usize,
        now: Duration,
        tick: u32,
    ) -> io::Result<()> {
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
                }
                MapReadyStep::Controller => self.world.presentation_due = true,
            }
        }
        if self.world.presentation_due {
            let world = room.world.as_ref().expect("world started");
            self.world.present(&world.scene, tick)?;
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
        let room = Room {
            map,
            players: players
                .into_iter()
                .enumerate()
                .map(|(slot, WorldPlayer { player, card })| Player {
                    info: player,
                    card,
                    entity: player_entity(slot),
                    gone: false,
                    stats: PlayerStats::default(),
                })
                .collect(),
            remaining,
            world,
        };
        let initial = room.message(0, true);
        let bytes = encode(&self.schemas, initial.id, &initial.fields).map_err(invalid)?;
        let payload = frame(u32::MAX, u32::MAX, &[(0, bytes)], &[], &[]).map_err(invalid)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(invalid("match roster exceeds the game datagram budget"));
        }

        let mut tickets = Vec::with_capacity(remaining);
        for _ in 0..remaining {
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
        for (slot, ticket) in tickets.iter().enumerate() {
            let reserved = self
                .links
                .reserve_handoff(ticket, u64::from(ticket.conn), now);
            assert!(reserved, "seat and connection tables are in sync");
            self.seats.insert(ticket.conn, (id, slot));
        }
        self.rooms.insert(id, room);
        Ok(tickets)
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
            Request::Home {
                connection,
                id,
                reply,
            } => {
                let home = !self.clock.draining()
                    && self.seats.get(&connection).is_some_and(|seat| seat.0 == id)
                    && self.peers.contains_key(&connection);
                if home {
                    self.send_home(connection, now);
                }
                let _ = reply.send(home);
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
        let mut struck = Vec::new();
        for offset in 0..count {
            let index = (start + offset) % count;
            let (&connection, peer) = self.peers.get_index(index).expect("active peer index");
            if !peer.world.has_input_work() {
                continue;
            }
            let (id, slot) = self.seats[&connection];
            let retained = self.retained_elsewhere(connection, now);
            let (_, peer) = self.peers.get_index_mut(index).expect("active peer index");
            peer.world.retain_elsewhere(retained);
            let room = self.rooms.get_mut(&id).expect("prepared room with input");
            let world = room.world.as_ref().expect("prepared world with input");
            let viewer = room.appearance(slot).expect("prepared player");
            let team = viewer.team;
            let context = ShotContext {
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
            let shots = peer.world.take_shots();
            if (!leave && peer.world.combat_body_active()) || !shots.is_empty() {
                let world = room.world.as_mut().expect("prepared world with combat");
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                // combat.py `Combat.command`: the shots, then quick melee.
                let mut hits = Vec::new();
                for shot in shots {
                    let visible = |entity| peer.world.identity_ready(entity);
                    match shot {
                        PosedShot::Hitscan(shot) => {
                            let shooter = Shooter {
                                source: shot.source,
                                team,
                                bot: false,
                            };
                            if let Ok(Some(hit)) =
                                combat.hitscan(&mut world.scene, shot, shooter, context, visible)
                            {
                                hits.push(hit);
                            }
                        }
                        PosedShot::Melee(melee) => {
                            let shooter = Shooter {
                                source: melee.source,
                                team,
                                bot: false,
                            };
                            hits.extend(combat.melee(
                                &mut world.scene,
                                melee,
                                shooter,
                                context,
                                visible,
                            ));
                        }
                        PosedShot::Projectile(_) => {}
                    }
                }
                struck.push((id, Some(connection), hits));
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
        for (id, shooter, hits) in struck {
            self.deliver_bot_hits(id, shooter, hits);
            self.resolve_player_hits(id, tick, now, None);
        }
        self.peers.values().any(|peer| peer.world.has_input_work())
    }

    /// The bodies of the player's ring another client of the room still
    /// holds: a new body takes none of them (ow174/game/match.py
    /// `_next_body_id`).
    fn retained_elsewhere(&self, connection: u32, now: Duration) -> Vec<u32> {
        let (id, slot) = self.seats[&connection];
        let owner = player_entity(slot);
        self.peers
            .iter()
            .filter(|&(&other, _)| other != connection && self.seats[&other].0 == id)
            .flat_map(|(_, other)| other.world.retained_bodies(owner, now))
            .collect()
    }

    /// combat.py `_damage` for bots and `_send_health`: the shooter's
    /// client gets the notices, every client of the room the bot's pools.
    fn deliver_bot_hits(&mut self, id: MatchId, shooter: Option<u32>, hits: Vec<BotHit>) {
        for hit in hits {
            for (&connection, peer) in &mut self.peers {
                if self.seats[&connection].0 != id {
                    continue;
                }
                peer.world.queue_health(hit.health.clone());
                if shooter == Some(connection) {
                    for notice in hit.notices.into_iter().flatten() {
                        peer.notice(notice);
                    }
                }
            }
        }
    }

    /// The hits on players' bodies of a room (combat.py `_damage`, `_died`).
    /// On a tick (`heals` given) also the Biotic Fields' heals
    /// (projectiles.py `_field`: `heal_area`) and the messages between
    /// bodies (match.py `deliver_body_messages`), in that order.
    fn resolve_player_hits(
        &mut self,
        id: MatchId,
        tick: u32,
        now: Duration,
        heals: Option<Vec<FieldHeal>>,
    ) {
        let Some(Room { players, world, .. }) = self.rooms.get_mut(&id) else {
            return;
        };
        let Some(WorldStart {
            scene,
            combat: Some(combat),
        }) = world.as_mut()
        else {
            return;
        };
        let hits = combat.take_player_hits();
        let mut peers = room_peers(&mut self.peers, &self.seats, id, players.len());
        let mut bot_heals = Vec::new();
        let notices = {
            let mut room: Vec<_> = players
                .iter_mut()
                .zip(peers.iter_mut())
                .map(|(player, peer)| CombatPlayer {
                    entity: player.entity,
                    world: peer.as_deref_mut().map(|peer| &mut peer.world),
                    stats: &mut player.stats,
                })
                .collect();
            let notices = apply_player_hits(scene, hits, &mut room, tick, now);
            if let Some(heals) = heals {
                for heal in heals {
                    let (bodies, bots) =
                        combat.heal_area(scene, heal.healer, heal.centre, HEAL_RADIUS, heal.amount);
                    let healed = bots.iter().map(|(_, given)| given).sum();
                    heal_players(
                        scene,
                        heal.healer.source,
                        &bodies,
                        heal.amount,
                        healed,
                        &mut room,
                    );
                    bot_heals.extend(bots.into_iter().map(|(hit, _)| hit));
                }
                let mut worlds: Vec<_> = room
                    .iter_mut()
                    .filter_map(|player| player.world.as_deref_mut())
                    .collect();
                deliver_body_messages(&mut worlds);
            }
            notices
        };
        for (peer, notices) in peers.into_iter().zip(notices) {
            if let Some(peer) = peer {
                for hit in &bot_heals {
                    peer.world.queue_health(hit.health.clone());
                }
                for notice in notices {
                    peer.notice(notice);
                }
            }
        }
    }

    /// combat.py `Combat.update` for every room with a world: where the
    /// bodies are, the bots' rockets, the players' projectiles, the hits on
    /// players, the Biotic Fields' heals and the messages between bodies,
    /// then the players' respawns and their spawn voice lines.
    fn update_combat(&mut self, tick: u32, now: Duration) {
        let ids: Vec<MatchId> = self
            .rooms
            .iter()
            .filter(|(_, room)| room.world.is_some())
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            let bot_hits = {
                let Room { players, world, .. } = self.rooms.get_mut(&id).expect("room");
                let world = world.as_mut().expect("room with a world");
                let targets: Vec<_> = room_peers(&mut self.peers, &self.seats, id, players.len())
                    .into_iter()
                    .enumerate()
                    .filter_map(|(slot, peer)| {
                        peer?
                            .world
                            .combat_target(&world.scene, players[slot].info.team)
                    })
                    .collect();
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                combat.record_poses(&world.scene, &targets, tick);
                let context = ShotContext {
                    quanta: RETAIL_QUANTA,
                    tick,
                    now,
                };
                combat
                    .step_bot_projectiles(&mut world.scene, context)
                    .unwrap_or_default()
            };
            self.deliver_bot_hits(id, None, bot_hits);
            let mut struck = Vec::new();
            for (&connection, peer) in &mut self.peers {
                let (match_id, slot) = self.seats[&connection];
                if match_id != id {
                    continue;
                }
                let room = self.rooms.get_mut(&id).expect("room");
                let player = &room.players[slot];
                let team = player.info.team;
                let context = ShotContext {
                    quanta: player.quanta(),
                    tick,
                    now,
                };
                let world = room.world.as_mut().expect("room with a world");
                let mut hits = Vec::new();
                let _ = peer.world.advance_projectiles(tick, |source, flight| {
                    let combat = world
                        .combat
                        .get_or_insert_with(|| Combat::new(&world.scene));
                    let shooter = Shooter {
                        source,
                        team,
                        bot: false,
                    };
                    combat.step_projectile(&mut world.scene, shooter, flight, context, &mut hits)
                });
                struck.push((connection, hits));
            }
            for (connection, hits) in struck {
                self.deliver_bot_hits(id, Some(connection), hits);
            }
            self.resolve_player_hits(id, tick, now, Some(Vec::new()));
            let connections: Vec<u32> = self
                .peers
                .keys()
                .copied()
                .filter(|connection| self.seats[connection].0 == id)
                .collect();
            for connection in connections {
                let retained = self.retained_elsewhere(connection, now);
                let slot = self.seats[&connection].1;
                let room = &self.rooms[&id];
                let world = room.world.as_ref().expect("room with a world");
                let viewer = room.appearance(slot).expect("seated player");
                let peer = self.peers.get_mut(&connection).expect("room peer");
                peer.world.retain_elsewhere(retained);
                if let Err(error) = peer.world.respawn_due(
                    &world.scene,
                    &viewer,
                    tick,
                    now,
                    room.players[slot].quanta(),
                ) {
                    eprintln!("{:08X} could not respawn ({error})", viewer.entity);
                }
                peer.world.expire_bark(now);
            }
        }
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
            self.update_combat(tick, now);
            for (&id, room) in &mut self.rooms {
                if let Some(world) = &mut room.world {
                    if let Some(combat) = &mut world.combat {
                        if let Ok(replacements) = combat.respawn_due(&mut world.scene, tick, now) {
                            if !replacements.is_empty() {
                                spawned.insert(id, replacements);
                            }
                        }
                    }
                }
            }
        }
        for (&connection, peer) in &mut self.peers {
            let (id, slot) = self.seats[&connection];
            let room = &self.rooms[&id];
            if !drain
                && peer
                    .advance_loading(&self.schemas, room, slot, now, tick)
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
            if let Some(world) = room.world.as_ref().filter(|_| !drain) {
                let hero = peer.world.current_hero();
                peer.world
                    .send_board(now, || board(room, slot, &world.scene, hero));
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
