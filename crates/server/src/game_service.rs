//! The native UDP match worker. It owns seats, connections and frame timing.
//!
//! A match on any map of the catalog advances from loading into one world
//! for all its players, who see each other (ow174/game/match.py). Its scene,
//! with the map's collision, is prepared off the game thread while the
//! players load (match.py `_refresh_collision`); players can join it while it
//! runs (ow174/game/server.py `join_match`). In the PvP modes the first spawn
//! starts "assemble your team" (match.py `_end_assemble`).
//!
//! How a player gets into the world (client research, 1.74): when the map is
//! loaded the client sends 21601 (and later 21602). 0.3 s later the server
//! sends 20302 `{the player entity}`, "this is your entity" (0x7FF7896E6C10
//! writes world +0x298); it has to come before the player's create, or the
//! client does not add its command components 59 and 63 (0x7FF78955EE90).
//! At least 0.6 s after loading, and after the ownership message and its
//! predecessors are acknowledged, one frame creates the map's server-owned
//! placeables (the loading screen only ends once they all exist), the player
//! entity (000E.003, flags 0x3C) and the game mode entity (16B2.003). Once
//! that create is acknowledged the player's entity gets its controller's
//! statescript in owner mode, so small owner frames keep telling the client
//! which of its commands the server has (CmFD); otherwise it replays old key
//! presses. The hero's body comes with the pick, as in retail (OW2 traffic):
//! the body with its movement state and an update of the player entity whose
//! component 44 possesses it. A switch replaces the body under a new id in
//! the same frame (the client ignores a destroyed id for 30 s). In the PvP
//! modes the first spawn shows 30 s of "assemble your team" on the hero
//! select screen, which stays open after a pick. Retail sent a datagram every
//! third tick (48 ms, OW2 traffic).

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
    connection::{GameConnections, MAX_CONNECTION_ID, Outgoing, Received, SILENCE_TIME},
    ingress::{GameIngress, MapReadiness, MapReadyStep, MessageKind, WorldReadiness, pong},
};
use orpheus_content::{
    heroes::SOLDIER,
    maps::GameMap,
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
    stats::PlayerStats,
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_lobby::handoff::Handoff;
use orpheus_script::wire::chunk;
use orpheus_sim::{
    bot_script::{BotShotKind, SeenRocket},
    combat::{
        BotHit, BotReplacement, Combat, CombatPlayer, FieldHeal, PosedShot, Shooter, ShotContext,
        apply_player_hits, deliver_body_messages, deliver_projectiles, finish_bot_rockets,
        heal_players, show_bot_rockets, show_fired,
    },
    controller::ControllerFrame,
    egress::GameEgress,
    instance::{Instance, Member, RETAIL_QUANTA, TOURNAMENT_QUANTA, ownership},
    projectiles::FiredProjectile,
    replication::{Record, RoomPlayer, WorldDelivery, show_players},
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::{CollisionBuild, PreparedScene},
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

    /// Take the seat's player out of his match (server.py `take_out`): a
    /// connected game goes back to the menu (20304) and leaves when its link
    /// closes; a player whose game has not connected leaves now, and the game
    /// gets only 20304 if it still connects. False when the seat is gone.
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
                stats: PlayerStats::default(),
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

/// A hit on a bot reaches its script (combat.py `_damage`): the death
/// message for a killing hit (`_bot_died`), else its new health
/// (`_sync_body_health`).
fn bot_hit_script(scene: &mut PreparedScene, hit: &BotHit) {
    match hit.death {
        Some(death) => scene.bot_died(
            death.bot,
            death.killer,
            death.critical,
            death.loadout,
            death.tick,
        ),
        None => scene.bot_health(hit.bot, hit.remaining),
    }
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
            presented: false,
            next_countdown: Duration::ZERO,
        }
    }

    fn clear_world(&mut self) {
        self.world.clear();
        self.combat_ui = CombatUi::default();
        self.presented = false;
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
                && (has_notice
                    || waiting_for_reliable
                    || self.world.health_pending(entity)
                    || self.world.bot_frames_pending(entity))
            {
                self.world.hold_bot_destroy(entity);
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

    /// This tick's frame over the peer's link: the queued reliable messages
    /// and world records, with these movements.
    fn send_frame(
        &mut self,
        links: &mut GameConnections,
        socket: &UdpSocket,
        connection: u32,
        tick: u32,
        movements: Vec<EntityUpdate>,
        reliable_only: bool,
    ) {
        let _ = self.world.send_frame(
            tick,
            movements,
            reliable_only,
            &mut self.egress,
            |payload| {
                let packet = Packet {
                    conn: connection,
                    kind: KIND_FRAME,
                    flags: ACK,
                    payload,
                    ..Packet::default()
                };
                let outgoing = links
                    .transmit(packet)
                    .map_err(|error| io::Error::other(format!("game link: {error:?}")))?;
                send(socket, outgoing)
            },
        );
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
                .is_some_and(|world| world.scene.bot_scripts_ready()),
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
    /// Games taken out of their match before they connected (server.py
    /// `take_out`): the player has left, but the handoff stays reserved until
    /// its deadline, so a game that still connects is admitted and gets only
    /// 20304 (server.py `_datagram`, `Client.received`).
    homeward: HashMap<u32, Peer>,
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
            homeward: HashMap::new(),
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
            if self.seats.contains_key(&connection) || self.homeward.contains_key(&connection) {
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
                if release && exists {
                    if self.links.waiting(connection) {
                        self.take_out_waiting(connection);
                    } else {
                        self.send_home(connection, now);
                    }
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

    /// A player whose game has not connected leaves the match now; if his
    /// game still connects, it gets only 20304 (server.py `take_out`).
    fn take_out_waiting(&mut self, connection: u32) {
        let (id, slot) = self.seats[&connection];
        let player = &self.rooms[&id].players[slot];
        tracing::info!(
            "[game] match {:04X}: {} left before his game connected",
            id[0] & 0xFFFF,
            player.info.name
        );
        let peer = Peer::new(player.entity);
        self.retire(connection);
        self.homeward.insert(connection, peer);
    }

    fn retire(&mut self, connection: u32) {
        self.homeward.remove(&connection);
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
        if self.homeward.contains_key(&connection) {
            self.receive_homeward(socket, received, now);
            return;
        }
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

    /// A datagram of a game whose player was taken out before it connected:
    /// once its link opens it goes back to the menu (20304), and nothing it
    /// sends reaches a match (server.py `Client.received`, `player.gone`).
    /// While the clock drains it is sent home at its SYN (`_datagram`).
    fn receive_homeward(&mut self, socket: &UdpSocket, received: Received, now: Duration) {
        let connection = received.connection;
        let peer = self.homeward.get_mut(&connection).expect("homeward peer");
        if let Some((ack, bits)) = received.ack {
            let _ = peer.egress.acknowledge(ack, bits);
        }
        if let Some(Ok(control)) = received.control {
            let _ = send(socket, control);
        }
        if received.closed {
            self.homeward.remove(&connection);
            return;
        }
        peer.open |= received.opened;
        if received.opened || (received.admitted && self.clock.draining()) {
            self.send_home(connection, now);
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
        let mut fired = Vec::new();
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
            let projectiles = peer.world.take_fired();
            if !projectiles.is_empty() {
                fired.push((id, projectiles));
            }
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
        for (id, projectiles) in fired {
            self.show_fired(id, &projectiles, tick);
        }
        for (id, shooter, hits) in struck {
            self.deliver_bot_hits(id, shooter, hits);
            self.resolve_player_hits(id, tick, now, None);
        }
        self.peers.values().any(|peer| peer.world.has_input_work())
    }

    /// projectiles.py `_show_others`: the other clients of the room see the
    /// projectiles a player fired.
    fn show_fired(&mut self, id: MatchId, projectiles: &[FiredProjectile], tick: u32) {
        let Some(Room { players, world, .. }) = self.rooms.get_mut(&id) else {
            return;
        };
        let Some(world) = world.as_mut() else {
            return;
        };
        let combat = world
            .combat
            .get_or_insert_with(|| Combat::new(&world.scene));
        let mut peers = room_peers(&mut self.peers, &self.seats, id, players.len());
        let mut worlds: Vec<_> = peers
            .iter_mut()
            .map(|peer| peer.as_deref_mut().map(|peer| &mut peer.world))
            .collect();
        show_fired(combat, projectiles, &mut worlds, tick);
    }

    /// The room's combat and its clients' worlds, by slot, for the bots'
    /// rocket copies (projectiles.py `_show_others`, `flown`).
    fn bot_rockets(
        &mut self,
        id: MatchId,
        tick: u32,
        apply: impl FnOnce(&mut Combat, &mut [Option<&mut WorldDelivery>], u32),
    ) {
        let Some(Room { players, world, .. }) = self.rooms.get_mut(&id) else {
            return;
        };
        let Some(combat) = world.as_mut().and_then(|world| world.combat.as_mut()) else {
            return;
        };
        let mut peers = room_peers(&mut self.peers, &self.seats, id, players.len());
        let mut worlds: Vec<_> = peers
            .iter_mut()
            .map(|peer| peer.as_deref_mut().map(|peer| &mut peer.world))
            .collect();
        apply(combat, &mut worlds, tick);
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
        if let Some(world) = self.rooms.get_mut(&id).and_then(|room| room.world.as_mut()) {
            for hit in &hits {
                bot_hit_script(&mut world.scene, hit);
            }
        }
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
        let teams: Vec<u8> = players.iter().map(|player| player.info.team).collect();
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
                    let healer = Shooter {
                        source: heal.source,
                        team: teams[heal.slot],
                        bot: false,
                    };
                    let (bodies, bots) =
                        combat.heal_area(scene, healer, heal.centre, heal.radius, heal.amount);
                    let healed = bots.iter().map(|(_, given)| given).sum();
                    heal_players(
                        scene,
                        healer.source,
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
        for hit in &bot_heals {
            bot_hit_script(scene, hit);
        }
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
    /// bodies are, the bots' rockets, the players' projectiles and what they
    /// ask of the room (projectiles.py `Projectiles.update`), the hits on
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
            let (bot_hits, first_tick) = {
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
                let first_tick = combat.catch_up(tick);
                combat.record_poses(&world.scene, &targets, tick);
                let context = ShotContext {
                    quanta: RETAIL_QUANTA,
                    tick,
                    now,
                };
                let hits = combat
                    .step_bot_projectiles(&mut world.scene, context)
                    .unwrap_or_default();
                (hits, first_tick)
            };
            self.deliver_bot_hits(id, None, bot_hits);
            // projectiles.py `flown`: the copies of the rockets that landed.
            self.bot_rockets(id, tick, finish_bot_rockets);
            // combat.py `_bot_weapons`: the firing bots' scripts after the
            // rockets flew; their shots at the players and the other team's
            // bots.
            let (bot_hits, rockets) = {
                let world = self
                    .rooms
                    .get_mut(&id)
                    .and_then(|room| room.world.as_mut())
                    .expect("room with a world");
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                let mut hits = Vec::new();
                let mut rockets = Vec::new();
                for shot in world.scene.bot_weapons(tick) {
                    let shooter = Shooter {
                        source: shot.source,
                        team: shot.team,
                        bot: true,
                    };
                    let seen = SeenRocket::of(0, &shot);
                    match shot.kind {
                        BotShotKind::Projectile(projectile) => {
                            let fired = combat.fire_bot_projectile(shooter, projectile, shot.tick);
                            if let Some((rocket, mut seen)) = fired.zip(seen) {
                                seen.rocket = rocket;
                                rockets.push(seen);
                            }
                        }
                        BotShotKind::Hitscan {
                            origin,
                            direction,
                            reach,
                            hit,
                            ..
                        } => {
                            let context = ShotContext {
                                quanta: RETAIL_QUANTA,
                                tick: shot.tick,
                                now,
                            };
                            if let Ok(Some(hit)) = combat.bot_hitscan(
                                &mut world.scene,
                                shooter,
                                origin,
                                direction,
                                reach,
                                hit,
                                context,
                            ) {
                                hits.push(hit);
                            }
                        }
                    }
                }
                (hits, rockets)
            };
            self.deliver_bot_hits(id, None, bot_hits);
            self.bot_rockets(id, tick, |combat, worlds, tick| {
                show_bot_rockets(combat, &rockets, worlds, tick);
            });
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
                let _ = peer
                    .world
                    .advance_projectiles(tick, first_tick, |source, flight| {
                        let combat = world
                            .combat
                            .get_or_insert_with(|| Combat::new(&world.scene));
                        let shooter = Shooter {
                            source,
                            team,
                            bot: false,
                        };
                        combat.step_projectile(
                            &mut world.scene,
                            shooter,
                            flight,
                            context,
                            &mut hits,
                        )
                    });
                struck.push((connection, hits));
            }
            for (connection, hits) in struck {
                self.deliver_bot_hits(id, Some(connection), hits);
            }
            let heals = self.deliver_projectiles(id, tick);
            self.resolve_player_hits(id, tick, now, Some(heals));
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

    /// projectiles.py `Projectiles.update` for a room once its players'
    /// shots stepped: the other clients' copies, the Biotic Fields'
    /// entities, and the fields' heals for [`Self::resolve_player_hits`].
    fn deliver_projectiles(&mut self, id: MatchId, tick: u32) -> Vec<FieldHeal> {
        let Some(Room { players, world, .. }) = self.rooms.get_mut(&id) else {
            return Vec::new();
        };
        let Some(world) = world.as_mut() else {
            return Vec::new();
        };
        let combat = world
            .combat
            .get_or_insert_with(|| Combat::new(&world.scene));
        let mut peers = room_peers(&mut self.peers, &self.seats, id, players.len());
        let mut worlds: Vec<_> = peers
            .iter_mut()
            .map(|peer| peer.as_deref_mut().map(|peer| &mut peer.world))
            .collect();
        deliver_projectiles(&world.scene, combat, &mut worlds, tick)
    }

    fn send_home(&mut self, connection: u32, now: Duration) {
        if self.links.begin_leaving(connection, now) {
            let peer = self
                .peers
                .get_mut(&connection)
                .or_else(|| self.homeward.get_mut(&connection))
                .expect("admitted peer");
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
        let step = self.clock.step(missed, self.connections_pending());
        let (tick, send_frames, drain, force) = match step {
            Step::Simulate { frame, send_world } => (frame, send_world, false, false),
            Step::Drain {
                frame,
                entered,
                force,
                send_reliable,
            } => {
                if entered {
                    for (&connection, peer) in self.peers.iter_mut().chain(&mut self.homeward) {
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
        let mut bot_events = HashMap::new();
        if !drain {
            for room in self.rooms.values_mut() {
                if let Some(world) = &mut room.world {
                    world.scene.step_bots(tick);
                }
            }
            self.update_combat(tick, now);
            for (&id, room) in &mut self.rooms {
                if let Some(world) = &mut room.world {
                    // combat.py `update`: the dead bots' death scripts, then
                    // the respawns with new scripts.
                    world.scene.advance_bot_deaths(tick);
                    if let Some(combat) = &mut world.combat {
                        if let Ok(replacements) = combat.respawn_due(&mut world.scene, tick, now) {
                            if !replacements.is_empty() {
                                world.scene.prepare_respawned_bots(tick);
                                spawned.insert(id, replacements);
                            }
                        }
                    }
                    bot_events.insert(id, world.scene.take_bot_events());
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
            if !drain {
                if let Some(world) = &room.world {
                    let events = bot_events.get(&id).map_or(&[][..], Vec::as_slice);
                    peer.world
                        .update_bots(&world.scene, events, tick, send_frames);
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
                peer.send_frame(&mut self.links, socket, connection, tick, movements, drain);
            }
        }
        // Games taken out before they connected get nothing but their 20304
        // (server.py `_tick`, `Client.send_frame`).
        for (&connection, peer) in &mut self.homeward {
            if send_frames && peer.open {
                peer.send_frame(&mut self.links, socket, connection, tick, Vec::new(), true);
            }
        }
        self.clock.renew_if_idle(self.connections_pending(), false);
    }

    /// Seats and games still connected or awaited keep the clock from
    /// renewing (server.py `_drain_clock`: `self.clients or self.handoffs`).
    fn connections_pending(&self) -> bool {
        !self.seats.is_empty() || !self.homeward.is_empty()
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
