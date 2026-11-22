//! The native UDP match server. A router (`router.rs`) owns the socket and
//! the lobby's requests; each match is a room on a thread of its own
//! (`room.rs`), with its seats, its games' links and its frame timing.
//!
//! A match on any map of the catalog advances from loading into one world
//! for all its players, who see each other (ow174/game/match.py). Its scene,
//! with the map's collision, is prepared off the game thread while the
//! players load (match.py `_refresh_collision`); players can join it while it
//! runs (ow174/game/server.py `join_match`). In the PvP modes the first spawn
//! starts the match flow (lucy_sim::match_flow): "assemble your team"
//! (match.py `_end_assemble`), then the mode's setup, rounds, objectives and
//! scoring; at the match's end the result goes to the lobby and the players
//! go home. In Practice vs AI, AI heroes fill the teams: players without a
//! game, whose bodies the server plays (`ai.rs`, `lucy_sim::ai`).
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
//! select screen, which stays open after a pick. The server sends each client
//! a datagram every tick (16 ms), as retail PC's default did from August 2016
//! (GDC 2017 netcode talks); OW2 traffic shows every third tick (48 ms).

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
    time::{Duration, Instant},
};

use indexmap::IndexMap;
use mio::{Interest, Poll, Token, Waker, net::UdpSocket};

use crate::{
    clock::GameClock,
    connection::{GameConnections, MAX_CONNECTION_ID, Outgoing},
    ingress::{GameIngress, MapReadiness},
};
use lucy_content::{
    hero_settings,
    heroes::SOLDIER,
    maps::{GameMap, SpawnChoice},
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
    stats::PlayerStats,
};
use lucy_lobby::handoff::Handoff;
use lucy_script::{snapshot::ProjectedSnapshot, vm::GameMessage};
use lucy_sim::{
    ai::Difficulty,
    bot_plate::{BotPlate, PlateChange, PlateSample, PlateView},
    combat::{BotReplacement, Combat},
    egress::GameEgress,
    hurt_feedback::HurtFeedback,
    instance::{RETAIL_QUANTA, TOURNAMENT_QUANTA},
    match_flow::{MatchFlow, Timers},
    messages::{Backlog, HeldReach},
    mode_script::ModeRunner,
    replication::{Record, WorldDelivery},
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::{CollisionBuild, CollisionFiles, PreparedScene},
    workshop::WorkshopRun,
};
use lucy_wire::{
    combat_messages::CombatNotice,
    entity::{MAX_PAYLOAD, SkinTheme, frame},
    jam::codec::{Fields, Schemas, Value},
    messages::{Message, locate},
};

use router::Router;

const GAME_SOCKET: Token = Token(0);
const REQUESTS: Token = Token(1);
const RECEIVE_BATCH: usize = 64;
/// The AI players' seats (Practice vs AI, `lucy_sim::ai`) take
/// connection ids no game link has.
const FIRST_AI_CONNECTION: u32 = MAX_CONNECTION_ID + 1;

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
    /// A Workshop match (a custom game, or `--workshop`): how many rules
    /// its script has for the Workshop engine; `None` for every other match.
    pub workshop_rules: Option<usize>,
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

/// How a finished match went for a player's team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Win,
    Loss,
    Draw,
}

/// A match that ended by its rules, for the lobby's profiles: every seat of
/// the room in slot order, which is the matchmaker's seat order. Results
/// count for the players only: the AI heroes of a Practice vs AI match have
/// none and take no slot here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchResult {
    pub id: MatchId,
    pub map: u64,
    pub mode: u64,
    pub players: Vec<PlayerResult>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerResult {
    /// The player's place among the room's players (not its AI heroes).
    pub slot: usize,
    pub name: String,
    pub account: [u64; 2],
    pub team: u8,
    pub outcome: Outcome,
    /// Still in the match when it ended: a leaver's seat is listed, but
    /// gets no result of his own.
    pub finished: bool,
    /// His medals at the end, counted by tier: gold, silver, bronze (the
    /// Tab board's, `team_medals`).
    pub medals: [u32; 3],
}

/// Builds a map's collision caches from the game install, as the Python
/// server did when a match opened on a map without one
/// (ow174/game/collision.py `_read_cache`, `build`): the movement cache and
/// the ray cache the shots meet. The collision reads no display strings, so
/// any text locale does.
pub fn collision_build(install: PathBuf) -> Arc<CollisionBuild> {
    Arc::new(move |map, name| {
        let started = Instant::now();
        let opened = lucy_assets::Install::open(&install, "enUS").map_err(io::Error::other)?;
        let movement =
            lucy_assets::collision_cache(&opened, map, name).map_err(io::Error::other)?;
        let rays = lucy_assets::ray_cache(&opened, map, name).map_err(io::Error::other)?;
        tracing::info!(
            "[game] built the collision of {name} in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        Ok(CollisionFiles {
            movement: movement.bytes,
            rays: rays.bytes,
        })
    })
}

pub struct GameService {
    address: SocketAddr,
    requests: mpsc::SyncSender<Request>,
    wake: Arc<Waker>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

pub type MatchId = [u64; 2];

/// What the lobby, the dashboard, a scene's thread and the tests ask the
/// game service. The router answers it (`router.rs`), or passes it on to
/// the room it is for ([`Request::Room`]).
enum Request {
    Open(Box<OpenRequest>),
    /// More players for a match that is on (server.py `join_match`).
    Join {
        id: MatchId,
        players: Vec<WorldPlayer>,
        reply: mpsc::SyncSender<io::Result<Option<Vec<Handoff>>>>,
    },
    /// Every room's line of the snapshot comes on the receiver, which ends
    /// once each room has answered.
    Snapshot {
        reply: mpsc::SyncSender<mpsc::Receiver<RoomSnapshot>>,
    },
    /// The matches that ended since the last call, oldest first.
    Results {
        reply: mpsc::SyncSender<Vec<MatchResult>>,
    },
    /// The live nameplate tool: a change, if any, then what it shows.
    BotPlate {
        change: Option<PlateChange>,
        reply: mpsc::SyncSender<Result<PlateAnswers, String>>,
    },
    /// The live hurt feedback switch: a change, if any, then what is in
    /// force.
    Hurt {
        change: String,
        reply: mpsc::SyncSender<Result<HurtFeedback, String>>,
    },
    /// Tests: the match flow's timers for the matches that open later.
    #[cfg(test)]
    Timers {
        timers: Timers,
        reply: mpsc::SyncSender<()>,
    },
    /// For the room of a match or of a seat.
    Room(RoomRequest),
}

/// What a room answers. The router passes it to the room of its match or
/// of its seat's connection ([`RoomRequest::target`]), or answers it as a
/// room without that match or seat would ([`RoomRequest::refuse`]).
enum RoomRequest {
    /// More players, with the connections the router gave their seats.
    Join {
        id: MatchId,
        players: Vec<WorldPlayer>,
        connections: Vec<u32>,
        reply: mpsc::SyncSender<io::Result<Option<Vec<Handoff>>>>,
    },
    /// A match's scene came from its preparation thread.
    Scene {
        id: MatchId,
        scene: io::Result<Box<PreparedScene>>,
    },
    /// The Workshop script of the match that just opened.
    Workshop {
        id: MatchId,
        logic: Arc<lucy_workshop::Logic>,
        catalog: Arc<lucy_workshop::Catalog>,
        reply: mpsc::SyncSender<bool>,
    },
    /// Tests: where every player's body of a match stands.
    #[cfg(test)]
    Bodies {
        id: MatchId,
        reply: mpsc::SyncSender<Vec<ai::BodyState>>,
    },
    /// Tests: what a Workshop match's engine and action graphs met.
    #[cfg(test)]
    WorkshopReport {
        id: MatchId,
        reply: mpsc::SyncSender<Option<WorkshopReport>>,
    },
    /// Tests: the match's next tick panics (a build that unwinds only).
    #[cfg(all(test, unwinds))]
    Fail {
        id: MatchId,
        reply: mpsc::SyncSender<()>,
    },
    /// Tests: the room's thread waits, once it has answered, until
    /// `release` lets it go (the threads of `GameService::bind` only).
    #[cfg(test)]
    Hold {
        id: MatchId,
        release: mpsc::Receiver<()>,
        reply: mpsc::SyncSender<()>,
    },
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
    /// The marker messages (`LUCY_DEBUG_SEND`): the seat's entities.
    DebugSeat {
        connection: u32,
        id: MatchId,
        reply: mpsc::SyncSender<Option<DebugSeat>>,
    },
    /// The marker messages: encoded game messages for the seat's game,
    /// queued as the server's own reliable messages.
    DebugSend {
        connection: u32,
        id: MatchId,
        messages: Vec<Vec<u8>>,
        reply: mpsc::SyncSender<bool>,
    },
    /// The marker messages: the environment kills the seat's body.
    DebugKill {
        connection: u32,
        id: MatchId,
        reply: mpsc::SyncSender<DebugKill>,
    },
}

/// The room a [`RoomRequest`] is for.
enum Target {
    Match(MatchId),
    Connection(u32),
}

impl RoomRequest {
    fn target(&self) -> Target {
        match *self {
            Self::Join { id, .. } | Self::Scene { id, .. } | Self::Workshop { id, .. } => {
                Target::Match(id)
            }
            #[cfg(test)]
            Self::Bodies { id, .. } | Self::WorkshopReport { id, .. } | Self::Hold { id, .. } => {
                Target::Match(id)
            }
            #[cfg(all(test, unwinds))]
            Self::Fail { id, .. } => Target::Match(id),
            Self::Pending { connection, .. }
            | Self::Cancel { connection, .. }
            | Self::Seat { connection, .. }
            | Self::Skin { connection, .. }
            | Self::Hero { connection, .. }
            | Self::DebugSeat { connection, .. }
            | Self::DebugSend { connection, .. }
            | Self::DebugKill { connection, .. } => Target::Connection(connection),
        }
    }

    /// The answer of a room that has no such match or seat: the match has
    /// ended, the seat is gone.
    fn refuse(self) {
        match self {
            Self::Join { reply, .. } => {
                let _ = reply.send(Ok(None));
            }
            Self::Scene { .. } => {}
            Self::Workshop { reply, .. }
            | Self::Pending { reply, .. }
            | Self::Cancel { reply, .. }
            | Self::Seat { reply, .. }
            | Self::Hero { reply, .. }
            | Self::DebugSend { reply, .. } => {
                let _ = reply.send(false);
            }
            #[cfg(test)]
            Self::Bodies { reply, .. } => {
                let _ = reply.send(Vec::new());
            }
            #[cfg(test)]
            Self::WorkshopReport { reply, .. } => {
                let _ = reply.send(None);
            }
            #[cfg(test)]
            Self::Hold { reply, .. } => {
                let _ = reply.send(());
            }
            #[cfg(all(test, unwinds))]
            Self::Fail { reply, .. } => {
                let _ = reply.send(());
            }
            Self::Skin { reply, .. } => {
                let _ = reply.send(());
            }
            Self::DebugSeat { reply, .. } => {
                let _ = reply.send(None);
            }
            Self::DebugKill { reply, .. } => {
                let _ = reply.send(DebugKill::NotConnected);
            }
        }
    }
}

/// A room's line of the snapshot, with when its match opened: the snapshot
/// lists the oldest first.
struct RoomSnapshot {
    started: Instant,
    snapshot: MatchSnapshot,
}

/// The live nameplate tool's answer: the plate's commands, the messages it
/// sent the live bots, and each room's part on the receiver.
struct PlateAnswers {
    commands: Vec<String>,
    messages: Vec<u64>,
    rooms: mpsc::Receiver<RoomPlate>,
}

/// A room's part of the nameplate tool's answer: how many of its live bots
/// took each message, and what its first live bot sends now.
struct RoomPlate {
    started: Instant,
    taken: Vec<usize>,
    sample: Option<PlateSample>,
}

/// Tests: what a Workshop match's rules met so far
/// ([`GameService::workshop_report`]).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct WorkshopReport {
    /// Whether the engine runs (the mode's graph drives the mode entity).
    pub running: bool,
    /// Values it doesn't evaluate ([`lucy_workshop::Engine::unsupported`]).
    pub unsupported: Vec<String>,
    /// Executions it stopped.
    pub faults: Vec<String>,
    /// The mode entity's graphs' refused or assumed classes, by graph
    /// (`lucy_script::mode::ModeScript::gaps`).
    pub gaps: Vec<((u32, String), usize)>,
}

struct OpenRequest {
    opening: Opening,
    reply: OpenReply,
}

/// Where the seats of a match that opens go.
type OpenReply = mpsc::SyncSender<io::Result<Vec<Handoff>>>;

/// A match to open: its map, its players, its world when its scene is
/// ready, and the level of the AI heroes that fill its teams (Practice vs
/// AI).
struct Opening {
    id: MatchId,
    map: GameMap,
    players: Vec<WorldPlayer>,
    world: Option<WorldStart>,
    ai: Option<Difficulty>,
}

struct WorldStart {
    scene: Box<PreparedScene>,
    combat: Option<Combat>,
}

/// The entities of a seat whose game is connected to its match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugSeat {
    /// The player entity: the controller.
    pub controller: u32,
    /// The hero body it possesses; none before the first pick.
    pub body: Option<u32>,
}

/// What `/api/debug/kill` did to a seat's body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugKill {
    /// The body dies at the next tick.
    Killed(u32),
    /// The seat's game is not connected to the match.
    NotConnected,
    /// No hero picked yet.
    NoBody,
    /// The body is dead already (waiting to respawn).
    Dead,
    /// The body is in its team's spawn room, where nothing hurts it.
    InSpawnRoom,
}

impl GameService {
    /// The service on a UDP socket at `address`: the router's thread
    /// (`router.rs`), which starts a thread for each match.
    pub fn bind(address: SocketAddr, schemas: Arc<Schemas>) -> io::Result<Self> {
        check_schemas(&schemas)?;

        let socket = std::net::UdpSocket::bind(address)?;
        let address = socket.local_addr()?;
        socket.set_nonblocking(true)?;
        // The rooms send through a clone of the socket the router reads.
        let rooms_socket = Arc::new(socket.try_clone()?);
        let mut socket = UdpSocket::from_std(socket);
        let poll = Poll::new()?;
        poll.registry()
            .register(&mut socket, GAME_SOCKET, Interest::READABLE)?;
        let wake = Arc::new(Waker::new(poll.registry(), REQUESTS)?);
        let (requests, receiver) = mpsc::sync_channel(RECEIVE_BATCH);
        let stopping = Arc::new(AtomicBool::new(false));
        let router = Router::new(schemas, Some(Arc::clone(&wake)));
        let worker_stopping = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name("lucy-game".into())
            .spawn(move || {
                router::run(
                    router,
                    socket,
                    rooms_socket,
                    poll,
                    receiver,
                    worker_stopping,
                )
            })?;
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
        self.open(Opening {
            id,
            map,
            players,
            world: None,
            ai: None,
        })
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
    /// size is the group's (ow174/lobby/matchmaker.py `_practice`), up to
    /// the mode's blue team (six in `005A`). Seats in member order.
    pub fn open_practice_party(
        &self,
        id: [u64; 2],
        scene: PreparedScene,
        mut members: Vec<WorldPlayer>,
    ) -> io::Result<Vec<Handoff>> {
        let seats = usize::from(scene.map().team_sizes.first().copied().unwrap_or(0));
        if !(1..=seats).contains(&members.len()) {
            return Err(invalid(format!(
                "a Practice Range group has one to {seats} players"
            )));
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
    /// (`_collision_failure`). Seats in player order. With `ai`, AI heroes
    /// of that level fill the places the players leave free on the teams,
    /// in the slots after theirs (Practice vs AI, `lucy_sim::ai`).
    pub fn open_world(
        &self,
        id: MatchId,
        map: GameMap,
        players: Vec<WorldPlayer>,
        ai: Option<Difficulty>,
        prepare: ScenePreparation,
    ) -> io::Result<Vec<Handoff>> {
        let handoffs = self.open(Opening {
            id,
            map,
            players,
            world: None,
            ai,
        })?;
        let requests = self.requests.clone();
        let wake = Arc::clone(&self.wake);
        let spawned = thread::Builder::new()
            .name("lucy-scene".into())
            .spawn(move || {
                let scene = prepare().map(Box::new);
                if requests
                    .send(Request::Room(RoomRequest::Scene { id, scene }))
                    .is_ok()
                {
                    let _ = wake.wake();
                }
            });
        if let Err(error) = spawned {
            let _ = self.requests.try_send(Request::Room(RoomRequest::Scene {
                id,
                scene: Err(error),
            }));
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
        self.open(Opening {
            id,
            map,
            players,
            world: Some(WorldStart {
                scene: Box::new(scene),
                combat: None,
            }),
            ai: None,
        })
    }

    fn open(&self, opening: Opening) -> io::Result<Vec<Handoff>> {
        self.request(|reply| Request::Open(Box::new(OpenRequest { opening, reply })))?
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

    /// Every match with its players (server.py `snapshot`), the oldest
    /// first. Each room answers for its own.
    pub fn snapshot(&self) -> io::Result<Vec<MatchSnapshot>> {
        let rooms = self.request(|reply| Request::Snapshot { reply })?;
        let mut rooms: Vec<RoomSnapshot> = rooms.iter().collect();
        rooms.sort_by_key(|room| room.started);
        Ok(rooms.into_iter().map(|room| room.snapshot).collect())
    }

    /// Give a match that just opened (before its scene is ready: right
    /// after `open_world`) its Workshop script and the catalog it reads,
    /// for the Workshop engine. False when the match is gone.
    pub fn attach_workshop(
        &self,
        id: MatchId,
        logic: Arc<lucy_workshop::Logic>,
        catalog: Arc<lucy_workshop::Catalog>,
    ) -> io::Result<bool> {
        self.room_request(|reply| RoomRequest::Workshop {
            id,
            logic,
            catalog,
            reply,
        })
    }

    /// The matches that ended by their rules since the last call, oldest
    /// first; the lobby records them in the players' profiles.
    pub fn take_results(&self) -> io::Result<Vec<MatchResult>> {
        self.request(|reply| Request::Results { reply })
    }

    /// Tests: shorten the match flow of the matches that open from now.
    #[cfg(test)]
    pub(crate) fn set_match_timers(&self, timers: Timers) -> io::Result<()> {
        self.request(|reply| Request::Timers { timers, reply })
    }

    /// Tests: every player of the match and his body, in slot order.
    #[cfg(test)]
    pub(crate) fn bodies(&self, id: MatchId) -> io::Result<Vec<ai::BodyState>> {
        self.room_request(|reply| RoomRequest::Bodies { id, reply })
    }

    /// Tests: what a Workshop match's rules met so far; none for another
    /// match.
    #[cfg(test)]
    pub(crate) fn workshop_report(&self, id: MatchId) -> io::Result<Option<WorkshopReport>> {
        self.room_request(|reply| RoomRequest::WorkshopReport { id, reply })
    }

    /// Tests: the match's next tick panics, as a bug in its simulation
    /// would (`RoomLoop::fail`).
    #[cfg(all(test, unwinds))]
    pub(crate) fn fail_match(&self, id: MatchId) -> io::Result<()> {
        self.room_request(|reply| RoomRequest::Fail { id, reply })
    }

    /// Tests: the thread of the match's room waits until the sender this
    /// gives is used or dropped (the threads of [`Self::bind`] only: the
    /// lockstep's one thread would wait for good).
    #[cfg(test)]
    pub(crate) fn hold_match(&self, id: MatchId) -> io::Result<mpsc::Sender<()>> {
        let (hold, release) = mpsc::channel();
        self.room_request(|reply| RoomRequest::Hold { id, release, reply })?;
        Ok(hold)
    }

    pub fn handoff_pending(&self, connection: u32) -> io::Result<bool> {
        self.room_request(|reply| RoomRequest::Pending { connection, reply })
    }

    pub fn cancel_handoff(&self, connection: u32) -> io::Result<bool> {
        self.room_request(|reply| RoomRequest::Cancel { connection, reply })
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
        self.room_request(|reply| RoomRequest::Skin {
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
        self.room_request(|reply| RoomRequest::Hero {
            connection: handoff.conn,
            id: handoff.match_id,
            hero,
            reply,
        })
    }

    /// The live nameplate tool (`lucy_sim::bot_plate`): apply a change,
    /// if any, to every match's training bots at once, and show the changes
    /// in force with what the first live bot sends now. A change the tool
    /// refuses comes back as its reason, as does a message no live bot of
    /// any match took.
    pub fn bot_plate(&self, change: Option<PlateChange>) -> io::Result<Result<PlateView, String>> {
        let answers = match self.request(|reply| Request::BotPlate { change, reply })? {
            Ok(answers) => answers,
            Err(reason) => return Ok(Err(reason)),
        };
        let mut rooms: Vec<RoomPlate> = answers.rooms.iter().collect();
        rooms.sort_by_key(|room| room.started);
        for (index, message) in answers.messages.iter().enumerate() {
            if rooms.iter().all(|room| room.taken[index] == 0) {
                return Ok(Err(format!(
                    "No live training bot took message {message:016X}."
                )));
            }
        }
        Ok(Ok(PlateView {
            commands: answers.commands,
            bot: rooms.into_iter().find_map(|room| room.sample),
        }))
    }

    /// The live hurt feedback switch (`lucy_sim::hurt_feedback`): apply
    /// a change given as the chat command's text, if any, to every match's
    /// players at once, and show what is in force. A change it refuses
    /// comes back as its reason.
    pub fn hurt_feedback(&self, change: &str) -> io::Result<Result<HurtFeedback, String>> {
        let change = change.to_owned();
        self.request(|reply| Request::Hurt { change, reply })
    }

    /// The marker messages (`LUCY_DEBUG_SEND`): the seat's controller and
    /// body, or none when its game is not connected to the match.
    pub(crate) fn debug_seat(&self, handoff: &Handoff) -> io::Result<Option<DebugSeat>> {
        self.room_request(|reply| RoomRequest::DebugSeat {
            connection: handoff.conn,
            id: handoff.match_id,
            reply,
        })
    }

    /// The marker messages: queue encoded game messages for the seat's game
    /// as reliable messages, in order. False when its game is not connected
    /// to the match.
    pub(crate) fn debug_send(&self, handoff: &Handoff, messages: Vec<Vec<u8>>) -> io::Result<bool> {
        self.room_request(|reply| RoomRequest::DebugSend {
            connection: handoff.conn,
            id: handoff.match_id,
            messages,
            reply,
        })
    }

    /// The marker messages: the environment kills the seat's living body,
    /// as the map's catch floor does ([`Combat::environment_kill`]): the
    /// hit goes the normal way (death message, board, respawn).
    pub(crate) fn debug_kill(&self, handoff: &Handoff) -> io::Result<DebugKill> {
        self.room_request(|reply| RoomRequest::DebugKill {
            connection: handoff.conn,
            id: handoff.match_id,
            reply,
        })
    }

    fn seat(&self, handoff: &Handoff, release: bool) -> io::Result<bool> {
        self.room_request(|reply| RoomRequest::Seat {
            connection: handoff.conn,
            id: handoff.match_id,
            release,
            reply,
        })
    }

    fn room_request<T>(
        &self,
        request: impl FnOnce(mpsc::SyncSender<T>) -> RoomRequest,
    ) -> io::Result<T> {
        self.request(|reply| Request::Room(request(reply)))
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

/// The schemas know the messages the worker sends and reads.
fn check_schemas(schemas: &Schemas) -> io::Result<()> {
    for id in [20300, 20301, 20302, 20304, 20306, 21601, 21602, 21610] {
        let (_, crc, _) = locate(id).map_err(invalid)?;
        schemas.fields(crc, id).map_err(invalid)?;
    }
    Ok(())
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
    /// An AI hero the server plays (Practice vs AI): no game, no result.
    ai: bool,
    /// A dummy bot a placed script made (the Training Partner, `ai.rs`):
    /// an AI player whose coming and going the chat doesn't post.
    dummy: bool,
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
    /// PvP: the match flow, from the first spawn (match.py
    /// `assemble_ends`).
    flow: Option<MatchFlow>,
    /// The game mode entity's frames: its graph's or the flow's.
    mode: Option<ModeRunner>,
    /// What the mode graph and the match flow sent this tick, for
    /// `route_messages`.
    mail: Vec<GameMessage>,
    /// Who each of the mode graph's held broadcasts reached.
    held_reach: HeldReach,
    /// The messages over a tick's cap, for the next tick's `route_messages`.
    backlog: Backlog,
    /// When the flow last advanced.
    flow_at: Duration,
    /// The tick the flow started: the first player came into the world,
    /// and the AI heroes with him.
    flow_tick: u32,
    /// The hand-built mode entity frame every client gets this tick, and
    /// when the next is due.
    mode_frame: Option<lucy_core::bits::BitWriter>,
    next_mode_frame: Duration,
    /// The mode graph's observer frame while it drives the mode entity:
    /// what each client's frames bring him (`lucy_sim::mode_view`).
    graph_frame: Option<Arc<ProjectedSnapshot>>,
    /// Hero select closed this tick: the heroes are assembled.
    assembled: bool,
    /// Its collision failed, its match is over, or the match failed: it
    /// takes nobody more (match.py `ended`).
    ended: bool,
    /// When the match opened: the snapshot lists the oldest first.
    started: Instant,
    /// The level of the AI heroes that fill its teams, if any.
    ai: Option<Difficulty>,
    /// Its dummy bots (`ai.rs`): the Practice Range's Training Partner.
    dummies: Vec<ai::DummySeat>,
    /// Its Workshop script (`attach_workshop`) and, once the mode starts,
    /// its engine (`lucy_sim::workshop`).
    workshop: Option<RoomWorkshop>,
}

/// A match's Workshop script and its engine.
struct RoomWorkshop {
    logic: Arc<lucy_workshop::Logic>,
    catalog: Arc<lucy_workshop::Catalog>,
    /// Runs from the mode's start (`Room::begin_flow`).
    run: Option<WorkshopRun>,
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
    /// An AI player's seat: no game connects to it; the server plays its
    /// hero (`ai.rs`).
    ai: Option<Box<ai::AiSeat>>,
    /// The round trip the game last reported in its input record, as
    /// logged ([`LATENCY_STEP`]).
    reported_latency: Option<u16>,
    /// The hero settings list the game is sending (21604 parts).
    hero_settings: hero_settings::SentList,
    /// Why the last frame couldn't be sent, as logged: a record that never
    /// fits or encodes stops every frame after it.
    send_failure: Option<String>,
    /// Pongs for the game's pings, until the next frame carries them with
    /// its send time ([`crate::ingress::Pong`]); at most [`MAX_PONGS`].
    pongs: Vec<crate::ingress::Pong>,
}

/// A game pings every 0.25 s and a frame goes every tick, so pongs only
/// pile up while its frames don't go (before its link opens); the oldest
/// go first.
const MAX_PONGS: usize = 4;

/// A game's reported round trip is logged when it moves this far (ms) from
/// the last one logged: the client predicts no impact effects on other
/// bodies above about 220 ms (`lucy_wire::commands::InputRecord`).
const LATENCY_STEP: u16 = 20;

#[derive(Default)]
struct CombatUi {
    notices: VecDeque<CombatNotice>,
    last_notice_by_entity: HashMap<u32, u32>,
    replacements: VecDeque<(Duration, BotReplacement)>,
    /// The frame of the last hit notice queued: the client's hit list drops
    /// a hit older than its last entry (leafall903/ORPHEUS `server.py`
    /// `_promote_combat_notice`), and held notices can leave out of order.
    last_hit_frame: u32,
}

/// The placeables the mode entity's graphs removed (`0CBB` st34: the
/// health packs, when the ruleset turns them off), out of the scene.
fn take_placeable_removals(mode: &mut ModeRunner, scene: &mut PreparedScene) {
    if let Some(script) = mode.script_mut() {
        for identifier in script.take_placeable_removals() {
            scene.remove_placeables(identifier);
        }
    }
}

/// The mode graph's frame this tick, while it drives the mode entity: the
/// `last` one while nothing in it changed, so each client's view sees it
/// is the same (`lucy_sim::mode_view`).
fn graph_frame(
    mode: &mut ModeRunner,
    last: Option<Arc<ProjectedSnapshot>>,
) -> Option<Arc<ProjectedSnapshot>> {
    let projected = mode.project(last.as_deref())?;
    Some(match last {
        Some(last) if *last == projected => last,
        _ => Arc::new(projected),
    })
}

/// A scene the worker can open a world on, whose world creates fit a
/// datagram for every player (`teams` and `cards` in slot order). A
/// statescript chunk too big for one goes in parts
/// (`lucy_sim::replication`).
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

    for (slot, card) in cards.iter().enumerate() {
        let team = teams[slot];
        let viewer = PlayerAppearance {
            entity: player_entity(slot),
            slot: i8::try_from(slot).map_err(invalid)?,
            team,
            spawn: spawn_number(map, teams[..slot].iter().copied(), team),
            room: SpawnChoice::First,
            card,
        };
        for record in scene.initial_world(&viewer)? {
            let packet = frame(u32::MAX, u32::MAX, 0, &[], &[], &[&record]).map_err(invalid)?;
            if packet.len() > MAX_PAYLOAD {
                return Err(invalid("a world baseline exceeds the game datagram budget"));
            }
        }
    }
    Ok(())
}

fn player_entity(slot: usize) -> u32 {
    0xa000_0000 | ((slot as u32 + 1) << 8)
}

/// A pick of the hero for the player entity, as the client's own pick
/// message (21616) carries it.
fn pick_message(entity: u32, hero: u64) -> Message {
    Message {
        id: CLIENT_GAME_MESSAGE,
        fields: Fields::from([
            (
                0x78,
                Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(entity)))])),
            ),
            (0x80, Value::Blob(hero.to_le_bytes().to_vec())),
            (0xa8, Value::Unsigned(PICK_HERO)),
            (0xb0, Value::Signed(0)),
        ]),
    }
}

/// The n-th player of a team takes the team's n-th spawn point; in a
/// free-for-all the n-th player of the match (ow174/game/match.py Player).
fn spawn_number(map: &GameMap, earlier: impl Iterator<Item = u8>, team: u8) -> usize {
    earlier
        .filter(|&other| map.free_for_all || other == team)
        .count()
}

/// A room: its match, the seats of its players and the links of their
/// games, on the room's own clock. Its thread (`router.rs`), or the tests'
/// lockstep, runs it (`room.rs`); nothing in it is shared with another
/// room.
struct RoomState {
    id: MatchId,
    schemas: Arc<Schemas>,
    clock: GameClock,
    links: GameConnections,
    room: Room,
    /// Each seat's slot in the match, by connection (the AI players' too).
    seats: HashMap<u32, usize>,
    peers: IndexMap<u32, Peer>,
    /// Games taken out of their match before they connected (server.py
    /// `take_out`): the player has left, but the handoff stays reserved until
    /// its deadline, so a game that still connects is admitted and gets only
    /// 20304 (server.py `_datagram`, `Client.received`).
    homeward: HashMap<u32, Peer>,
    /// The next AI player's seat id ([`FIRST_AI_CONNECTION`] and up).
    next_ai_connection: u32,
    next_input_peer: usize,
    /// The match flow's timers (tests shorten them).
    timers: Timers,
    /// The live nameplate tool's changes.
    bot_plate: Arc<BotPlate>,
    /// The live hurt feedback switch.
    hurt: HurtFeedback,
    /// What the router learns from the room.
    events: router::Events,
    /// Tests: the next tick panics ([`GameService::fail_match`]).
    #[cfg(all(test, unwinds))]
    fail: bool,
}

/// What a new room takes from the service: the schemas, and the settings
/// in force.
struct RoomSettings {
    schemas: Arc<Schemas>,
    timers: Timers,
    bot_plate: Arc<BotPlate>,
    hurt: HurtFeedback,
    events: router::Events,
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

/// Where a room's datagrams go: the UDP socket, or in tests an in-memory
/// network on a virtual clock (`lockstep.rs`). The router reads what comes
/// (`router.rs`).
trait Transport {
    fn send_to(&self, datagram: &[u8], address: SocketAddr) -> io::Result<usize>;
}

impl Transport for std::net::UdpSocket {
    fn send_to(&self, datagram: &[u8], address: SocketAddr) -> io::Result<usize> {
        std::net::UdpSocket::send_to(self, datagram, address)
    }
}

fn send(socket: &impl Transport, outgoing: Outgoing) -> io::Result<u32> {
    let count = socket.send_to(&outgoing.datagram, outgoing.address)?;
    if count != outgoing.datagram.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "partial game datagram",
        ));
    }
    Ok(outgoing.sequence)
}

mod ai;
mod combat;
mod flow;
mod messages;
mod peer;
mod room;
mod router;
mod timer;

#[cfg(test)]
pub(crate) mod lockstep;

#[cfg(test)]
mod tests;
