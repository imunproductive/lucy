//! The native UDP match worker. It owns seats, connections and frame timing.
//!
//! A match on any map of the catalog advances from loading into one world
//! for all its players, who see each other (ow174/game/match.py). Its scene,
//! with the map's collision, is prepared off the game thread while the
//! players load (match.py `_refresh_collision`); players can join it while it
//! runs (ow174/game/server.py `join_match`). In the PvP modes the first spawn
//! starts the match flow (orpheus_sim::match_flow): "assemble your team"
//! (match.py `_end_assemble`), then the mode's setup, rounds, objectives and
//! scoring; at the match's end the result goes to the lobby and the players
//! go home. In Practice vs AI, AI heroes fill the teams: players without a
//! game, whose bodies the server plays (`ai.rs`, `orpheus_sim::ai`).
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
    clock::{GameClock, Step, next_tick_after, send_every},
    connection::{GameConnections, MAX_CONNECTION_ID, Outgoing, Received, SILENCE_TIME},
    ingress::{GameIngress, MapReadiness, MapReadyStep, MessageKind, WorldReadiness, pong},
};
use orpheus_content::{
    hero_settings,
    heroes::SOLDIER,
    maps::{GameMap, SpawnChoice},
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
    stats::{BRONZE, GOLD, Medals, PlayerStats, SILVER, StatsCatalog},
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_lobby::handoff::Handoff;
use orpheus_script::{vm::GameMessage, wire::chunk};
use orpheus_sim::{
    ai::{Difficulty, Random},
    bot_script::{BotShotKind, SeenRocket},
    combat::{
        BotHit, BotReplacement, Combat, CombatPlayer, FieldHeal, PosedShot, Shooter, ShotContext,
        apply_player_hits, deliver_body_messages, deliver_projectiles, finish_bot_rockets,
        heal_players, show_bot_rockets, show_fired,
    },
    controller::ControllerFrame,
    egress::GameEgress,
    instance::{Instance, Member, RETAIL_QUANTA, TOURNAMENT_QUANTA, ownership},
    match_flow::{FlowEvent, MatchFlow, Mode, Phase, Standing, Timers},
    mode_script::ModeRunner,
    objectives::Participant,
    projectiles::FiredProjectile,
    replication::{Record, RoomPlayer, WorldDelivery, show_players},
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::{CollisionBuild, PreparedScene},
    voice_lines::VoiceLine,
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
/// The game mode entity gets a frame with the match flow's clock every
/// second, and at each phase change (ow174/game/match.py `COUNTDOWN_EVERY`).
const MODE_FRAME_EVERY: Duration = Duration::from_secs(1);
/// The AI players' seats (Practice vs AI, `orpheus_sim::ai`) take
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
    /// The matches that ended since the last call, oldest first.
    Results {
        reply: mpsc::SyncSender<Vec<MatchResult>>,
    },
    /// Tests: the match flow's timers for the matches that start later.
    #[cfg(test)]
    Timers {
        timers: Timers,
        reply: mpsc::SyncSender<()>,
    },
    /// Tests: where every player's body of a match stands.
    #[cfg(test)]
    Bodies {
        id: MatchId,
        reply: mpsc::SyncSender<Vec<ai::BodyState>>,
    },
}

struct OpenRequest {
    id: MatchId,
    map: GameMap,
    players: Vec<WorldPlayer>,
    world: Option<WorldStart>,
    /// Fill the teams with AI heroes of this level (Practice vs AI).
    ai: Option<Difficulty>,
    reply: mpsc::SyncSender<io::Result<Vec<Handoff>>>,
}

struct WorldStart {
    scene: Box<PreparedScene>,
    combat: Option<Combat>,
}

impl GameService {
    pub fn bind(address: SocketAddr, schemas: Arc<Schemas>) -> io::Result<Self> {
        check_schemas(&schemas)?;

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
                ai: None,
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
    /// (`_collision_failure`). Seats in player order. With `ai`, AI heroes
    /// of that level fill the places the players leave free on the teams,
    /// in the slots after theirs (Practice vs AI, `orpheus_sim::ai`).
    pub fn open_world(
        &self,
        id: MatchId,
        map: GameMap,
        players: Vec<WorldPlayer>,
        ai: Option<Difficulty>,
        prepare: ScenePreparation,
    ) -> io::Result<Vec<Handoff>> {
        let handoffs = self.request(|reply| {
            Request::Open(Box::new(OpenRequest {
                id,
                map,
                players,
                world: None,
                ai,
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
                ai: None,
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

    /// The matches that ended by their rules since the last call, oldest
    /// first; the lobby records them in the players' profiles.
    pub fn take_results(&self) -> io::Result<Vec<MatchResult>> {
        self.request(|reply| Request::Results { reply })
    }

    /// Tests: shorten the match flow of the matches that start from now.
    #[cfg(test)]
    pub(crate) fn set_match_timers(&self, timers: Timers) -> io::Result<()> {
        self.request(|reply| Request::Timers { timers, reply })
    }

    /// Tests: every player of the match and his body, in slot order.
    #[cfg(test)]
    pub(crate) fn bodies(&self, id: MatchId) -> io::Result<Vec<ai::BodyState>> {
        self.request(|reply| Request::Bodies { id, reply })
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
    /// When the flow last advanced.
    flow_at: Duration,
    /// The tick the flow started: the first player came into the world,
    /// and the AI heroes with him.
    flow_tick: u32,
    /// The mode entity's frame every client gets this tick, and when the
    /// next is due.
    mode_frame: Option<orpheus_core::bits::BitWriter>,
    next_mode_frame: Duration,
    /// Hero select closed this tick: the heroes are assembled.
    assembled: bool,
    /// Its collision failed: it takes nobody more (match.py `ended`).
    ended: bool,
    /// When the match opened: the snapshot lists the oldest first.
    started: Instant,
    /// The level of the AI heroes that fill its teams, if any.
    ai: Option<Difficulty>,
}

impl Room {
    fn new(map: GameMap, players: Vec<WorldPlayer>, world: Option<WorldStart>) -> Self {
        let mut room = Self {
            map,
            players: Vec::with_capacity(players.len()),
            remaining: 0,
            world,
            flow: None,
            mode: None,
            flow_at: Duration::ZERO,
            flow_tick: 0,
            mode_frame: None,
            next_mode_frame: Duration::ZERO,
            assembled: false,
            ended: false,
            started: Instant::now(),
            ai: None,
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
                ai: false,
            });
        }
    }

    fn assembling(&self) -> bool {
        self.flow.as_ref().is_some_and(MatchFlow::assembling)
    }

    /// The players are in a PvP world: the flow starts with "assemble your
    /// team", and the mode graph once to see whether it can drive the mode
    /// entity (`mode_script.rs`).
    fn begin_flow(&mut self, scene: &PreparedScene, timers: Timers, now: Duration, tick: u32) {
        if self.flow.is_some() {
            return;
        }
        let competitive = self.players.iter().any(|player| player.info.ranked);
        let graph_values = scene.mode_graph_values();
        let rules = scene.mode_rules();
        let objectives = scene.map_objectives();
        // A PvP mode without a flow of its own (a brawl) keeps the
        // assemble phase and plays on like Skirmish.
        let skirmish = || {
            let map = GameMap {
                mode_guid: orpheus_sim::match_flow::SKIRMISH,
                ..self.map.clone()
            };
            MatchFlow::new(
                &map,
                competitive,
                graph_values,
                rules,
                Arc::clone(&objectives),
                timers,
            )
        };
        let Some(mut flow) = MatchFlow::new(
            &self.map,
            competitive,
            graph_values,
            rules,
            Arc::clone(&objectives),
            timers,
        )
        .or_else(skirmish) else {
            return;
        };
        flow.start();
        tracing::info!(
            "[game] {}: {:?} match flow, assemble your team",
            self.map.name,
            flow.mode()
        );
        self.flow = Some(flow);
        self.mode = Some(ModeRunner::start(scene, tick));
        self.flow_at = now;
        self.flow_tick = tick;
        self.next_mode_frame = now;
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
            room: self
                .flow
                .as_ref()
                .map_or(SpawnChoice::First, |flow| flow.spawn_choice(team)),
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
    /// An AI player's seat: no game connects to it; the server plays its
    /// hero (`ai.rs`).
    ai: Option<Box<ai::AiSeat>>,
    /// The round trip the game last reported in its input record, as
    /// logged ([`LATENCY_STEP`]).
    reported_latency: Option<u16>,
}

/// A game's reported round trip is logged when it moves this far (ms) from
/// the last one logged: the client predicts no impact effects on other
/// bodies above about 220 ms (`orpheus_wire::commands::InputRecord`).
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
            room: SpawnChoice::First,
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

/// A hit on a bot reaches its script (combat.py `_damage`): the death
/// message for a killing hit (`_bot_died`), else its new health
/// (`_sync_body_health`) and, for damage, the damage message `0011.025`
/// (its flinch and pain line).
fn bot_hit_script(scene: &mut PreparedScene, hit: &BotHit) {
    match hit.death {
        Some(death) => scene.bot_died(
            death.bot,
            death.killer,
            death.critical,
            death.loadout,
            death.tick,
        ),
        None => {
            scene.bot_health(hit.bot, hit.remaining);
            if let Some(damage) = &hit.damage {
                scene.bot_hurt(hit.bot, damage);
            }
        }
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
    let mode = room.map.mode_guid;
    let medals = team_medals(
        &room.players,
        room.map.free_for_all,
        mode,
        slot,
        scene.stats(),
    );
    scene
        .stats()
        .component(&room.players[slot].stats, mode, hero, Some(&medals))
}

/// The player's medals among his teammates still in the match (every
/// player in a free-for-all), as the Tab board shows them.
fn team_medals(
    players: &[Player],
    free_for_all: bool,
    mode: u64,
    slot: usize,
    catalog: &StatsCatalog,
) -> Medals {
    let me = &players[slot];
    let mut team: Vec<usize> = (0..players.len())
        .filter(|&other| {
            let player = &players[other];
            !player.gone && (free_for_all || player.info.team == me.info.team)
        })
        .collect();
    if !team.contains(&slot) {
        team.push(slot);
    }
    let stats: Vec<_> = team.iter().map(|&other| &players[other].stats).collect();
    let index = team
        .iter()
        .position(|&other| other == slot)
        .expect("the player is on his team");
    catalog.medals(&stats, mode).swap_remove(index)
}

/// Medals counted by tier: gold, silver, bronze.
fn medal_counts(medals: &Medals) -> [u32; 3] {
    let mut counts = [0; 3];
    for &medal in medals.values() {
        match medal {
            GOLD => counts[0] += 1,
            SILVER => counts[1] += 1,
            BRONZE => counts[2] += 1,
            _ => {}
        }
    }
    counts
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
            ai: None,
            reported_latency: None,
        }
    }

    /// An AI player's peer: a world no client receives, and the controller
    /// that plays it.
    fn ai(owner: u32, seat: ai::AiSeat) -> Self {
        Self {
            world: WorldDelivery::headless(owner),
            ai: Some(Box::new(seat)),
            ..Self::new(owner)
        }
    }

    fn clear_world(&mut self) {
        self.world.clear();
        self.combat_ui = CombatUi::default();
        self.presented = false;
    }

    /// A hit's notices for this client (the shooter's).
    fn notice(&mut self, notice: CombatNotice) {
        if self.ai.is_some() {
            return;
        }
        let Some(notice) = DROPPED_NOTICES.filter(notice) else {
            return;
        };
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
            let notice = match notice {
                CombatNotice::Hit {
                    target,
                    dealt,
                    critical,
                    killed,
                    shot_frame,
                } => {
                    let shot_frame = shot_frame.max(self.combat_ui.last_hit_frame);
                    self.combat_ui.last_hit_frame = shot_frame;
                    CombatNotice::Hit {
                        target,
                        dealt,
                        critical,
                        killed,
                        shot_frame,
                    }
                }
                notice => notice,
            };
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
        socket: &impl Transport,
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

    /// This tick's steps of the player (match.py `update`): the mode
    /// entity's frame and the end of "assemble your team", then the spawn
    /// steps. The first spawn in a PvP mode starts the match flow
    /// (`_spawn_steps`).
    fn update(
        &mut self,
        schemas: &Schemas,
        room: &mut Room,
        slot: usize,
        timers: Timers,
        now: Duration,
        tick: u32,
    ) -> io::Result<()> {
        if let (true, Some(frame)) = (self.presented, &room.mode_frame) {
            self.world.queue_mode_frame(frame)?;
        }
        if let (true, Some(world)) = (room.assembled && self.presented, &room.world) {
            self.world.end_assemble(&world.scene, tick)?;
        }
        self.world.set_assembling(room.assembling());
        let spawned = self.advance_loading(schemas, room, slot, now, tick)?;
        if let Some(world) = room.world.take() {
            if spawned && !world.scene.controller().practice() {
                room.begin_flow(&world.scene, timers, now, tick);
            }
            room.world = Some(world);
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
    /// The next AI player's seat id ([`FIRST_AI_CONNECTION`] and up).
    next_ai_connection: u32,
    next_input_peer: usize,
    /// Matches that ended by their rules, until the lobby takes them.
    results: Vec<MatchResult>,
    /// The match flow's timers (tests shorten them).
    timers: Timers,
}

impl State {
    fn new(schemas: Arc<Schemas>) -> Self {
        Self {
            schemas,
            clock: GameClock::new(send_every()),
            links: GameConnections::default(),
            rooms: HashMap::new(),
            seats: HashMap::new(),
            peers: IndexMap::new(),
            homeward: HashMap::new(),
            next_connection: 1,
            next_ai_connection: FIRST_AI_CONNECTION,
            next_input_peer: 0,
            results: Vec::new(),
            timers: Timers::default(),
        }
    }

    fn open_match(
        &mut self,
        id: MatchId,
        map: GameMap,
        players: Vec<WorldPlayer>,
        world: Option<WorldStart>,
        ai: Option<Difficulty>,
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
        let mut room = Room::new(map, players, world);
        let seats = ai.map_or_else(Vec::new, |difficulty| {
            let mut random = Random::new(ai::seed());
            room.add_ai(difficulty, &mut random)
        });
        self.check_roster(&room)?;
        let tickets = self.reserve(id, 0..remaining, now)?;
        self.rooms.insert(id, room);
        self.seat_ai(id, seats);
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
                waiting: room
                    .flow
                    .as_ref()
                    .is_none_or(|flow| flow.phase() <= Phase::Assemble),
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
                    ai,
                    reply,
                } = *request;
                let result = self.open_match(id, map, players, world, ai, now);
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
            Request::Results { reply } => {
                let _ = reply.send(std::mem::take(&mut self.results));
            }
            #[cfg(test)]
            Request::Timers { timers, reply } => {
                self.timers = timers;
                let _ = reply.send(());
            }
            #[cfg(test)]
            Request::Bodies { id, reply } => {
                let _ = reply.send(self.bodies(id));
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
        let pick = pick_message(room.players[slot].entity, hero);
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
                self.unseat_ai(id);
            }
        }
    }

    fn receive(
        &mut self,
        socket: &impl Transport,
        bytes: &[u8],
        address: SocketAddr,
        now: Duration,
    ) {
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
                peer.world.acknowledge_movement(ack, bits);
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
                reported_latency,
                ..
            } = peer;
            let incoming = ingress.receive(&self.schemas, &payload, |message| {
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
                    MessageKind::GameMessage | MessageKind::CommunicationAction
                        if room.world.is_some() =>
                    {
                        world.queue_game_message(self.clock.frame(), now, message);
                    }
                    MessageKind::HeroSettings => {
                        let name = &room.players[slot].info.name;
                        match hero_settings::read(&message) {
                            Some(entries) => {
                                let listed: Vec<_> = entries
                                    .iter()
                                    .map(|entry| {
                                        format!(
                                            "{:X}/{:X}={:X}",
                                            entry.hero, entry.setting, entry.value
                                        )
                                    })
                                    .collect();
                                tracing::info!(
                                    "[game] hero settings of {name} (hero/setting=value): {}",
                                    listed.join(" ")
                                );
                                world.update_hero_settings(&entries);
                            }
                            None => tracing::info!(
                                "[game] unreadable hero settings of {name}: {:?}",
                                message.fields
                            ),
                        }
                    }
                    _ => {}
                }
            });
            if let Ok(input) = incoming {
                if let Some(latency) = input.latency.filter(|&latency| {
                    reported_latency.is_none_or(|last| last.abs_diff(latency) >= LATENCY_STEP)
                }) {
                    *reported_latency = Some(latency);
                    let view_delay = input.commands.last().map(|command| command.extra);
                    tracing::info!(
                        target: "orpheus::latency",
                        "[game] {} reports a {latency} ms round trip (view delay {view_delay:?} ms)",
                        room.players[slot].info.name
                    );
                }
                if room.world.is_some() {
                    world.queue_commands(self.clock.frame(), &input.commands);
                }
            }
        }
    }

    /// A datagram of a game whose player was taken out before it connected:
    /// once its link opens it goes back to the menu (20304), and nothing it
    /// sends reaches a match (server.py `Client.received`, `player.gone`).
    /// While the clock drains it is sent home at its SYN (`_datagram`).
    fn receive_homeward(&mut self, socket: &impl Transport, received: Received, now: Duration) {
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
                    match shot {
                        PosedShot::Hitscan(shot) => {
                            let frame = shot.command.frame;
                            let visible = |entity| peer.world.bot_shown(entity, frame);
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
                            let frame = melee.command.frame;
                            let visible = |entity| peer.world.bot_shown(entity, frame);
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
    /// The shooter's ultimate charges for the damage (assumption: training
    /// bots charge it as heroes do) and for the healing of allied bots
    /// (combat.py `_heal`), which his Healing Done counts too.
    fn deliver_bot_hits(&mut self, id: MatchId, shooter: Option<u32>, hits: Vec<BotHit>) {
        let healed: f64 = hits.iter().map(|hit| hit.healed).sum();
        if let Some(room) = self.rooms.get_mut(&id) {
            if let Some(world) = room.world.as_mut() {
                for hit in &hits {
                    bot_hit_script(&mut world.scene, hit);
                }
                if let Some(slot) = shooter
                    .filter(|_| healed > 0.0)
                    .map(|shooter| self.seats[&shooter].1)
                {
                    let healing = world.scene.board_stats().healing;
                    room.players[slot].stats.add(healing, healed);
                }
            }
        }
        let points = hits
            .iter()
            .flat_map(|hit| &hit.notices)
            .filter_map(|notice| match notice {
                Some(CombatNotice::Hit { dealt, .. }) => Some(*dealt),
                _ => None,
            })
            .sum::<f64>()
            + healed;
        if let Some(peer) = shooter
            .filter(|_| points > 0.0)
            .and_then(|shooter| self.peers.get_mut(&shooter))
        {
            peer.world.gain_ult(points);
        }
        for hit in hits {
            for (&connection, peer) in &mut self.peers {
                if self.seats[&connection].0 != id || peer.ai.is_some() {
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
        let Some(Room {
            players,
            world,
            flow,
            ai,
            ..
        }) = self.rooms.get_mut(&id)
        else {
            return;
        };
        let Some(WorldStart {
            scene,
            combat: Some(combat),
        }) = world.as_mut()
        else {
            return;
        };
        let mut hits = combat.take_player_hits();
        if let Some(difficulty) = *ai {
            ai::scale_damage(&mut hits, players, difficulty);
        }
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
                    let healed = bots.iter().map(|hit| hit.healed).sum();
                    heal_players(
                        scene,
                        healer.source,
                        &bodies,
                        heal.amount,
                        healed,
                        &mut room,
                    );
                    bot_heals.extend(bots);
                }
                let mut worlds: Vec<_> = room
                    .iter_mut()
                    .filter_map(|player| player.world.as_deref_mut())
                    .collect();
                deliver_body_messages(&mut worlds);
            }
            notices
        };
        // The kills count for the match flow's score (Deathmatch, Team
        // Deathmatch): the victim's death notice names the killer's body.
        if let Some(flow) = flow {
            for (victim, notices) in notices.iter().enumerate() {
                for notice in notices {
                    if let CombatNotice::Death { source, killed_by } = *notice {
                        let killer = killed_by
                            .then(|| {
                                players
                                    .iter()
                                    .position(|player| player.entity == source & !0xff)
                            })
                            .flatten();
                        flow.kill(
                            killer.map(|slot| (slot, teams[slot])),
                            (victim, teams[victim]),
                        );
                    }
                }
            }
        }
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
                // The bodies the statescript's target searches (`TrackTargets`)
                // find until the next update, as placed this tick.
                let candidates = combat.target_candidates(&world.scene);
                world.scene.set_bot_target_candidates(&candidates);
                for peer in room_peers(&mut self.peers, &self.seats, id, players.len())
                    .into_iter()
                    .flatten()
                {
                    peer.world.set_target_candidates(&candidates);
                }
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
            // The environment kills a living body that fell onto the map's
            // catch floor (`orpheus_sim::map_areas`), with the tick's hits.
            {
                let Room { players, world, .. } = self.rooms.get_mut(&id).expect("room");
                let world = world.as_mut().expect("room with a world");
                let combat = world
                    .combat
                    .get_or_insert_with(|| Combat::new(&world.scene));
                for (slot, peer) in room_peers(&mut self.peers, &self.seats, id, players.len())
                    .into_iter()
                    .enumerate()
                {
                    let Some(peer) = peer else { continue };
                    if peer.world.body_dead() {
                        continue;
                    }
                    let fell = peer
                        .world
                        .body_position()
                        .is_some_and(|position| world.scene.fell_out(position));
                    if let Some(body) = peer.world.current_body().filter(|_| fell) {
                        combat.environment_kill(body, players[slot].info.team, tick);
                    }
                }
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
                // The match flow holds the dead within an Elimination round
                // and once the match is decided.
                let respawns = room.flow.as_ref().is_none_or(MatchFlow::respawns);
                if let Err(error) = respawns
                    .then(|| {
                        peer.world.respawn_due(
                            &world.scene,
                            &viewer,
                            tick,
                            now,
                            room.players[slot].quanta(),
                        )
                    })
                    .transpose()
                {
                    eprintln!("{:08X} could not respawn ({error})", viewer.entity);
                }
                peer.world.expire_bark(now);
                peer.world.area_step(&world.scene, &viewer);
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

    /// Every PvP room's match flow this tick (orpheus_sim::match_flow): it
    /// advances with where the players' bodies stand, the game mode
    /// entity's frame is made when due, hero select closes once the heroes
    /// are assembled, the phase messages and the mode graph's go to the
    /// players' bodies, a new side, Control round or Elimination round puts
    /// everyone back in spawn, overtime lengthens respawns, a
    /// decided match goes to the lobby's results, and once its end is over
    /// everyone goes home.
    fn update_flows(&mut self, tick: u32, now: Duration) {
        let mut home = Vec::new();
        for (&id, room) in &mut self.rooms {
            room.assembled = false;
            room.mode_frame = None;
            let mut connections: Vec<(u32, usize)> = self
                .seats
                .iter()
                .filter(|(connection, seat)| seat.0 == id && self.peers.contains_key(*connection))
                .map(|(&connection, &(_, slot))| (connection, slot))
                .collect();
            connections.sort_by_key(|&(_, slot)| slot);
            let (Some(flow), Some(world)) = (room.flow.as_mut(), room.world.as_ref()) else {
                continue;
            };
            let participants: Vec<Participant> = connections
                .iter()
                .filter(|&&(_, slot)| !room.players[slot].gone)
                .map(|&(connection, slot)| Participant {
                    slot,
                    team: room.players[slot].info.team,
                    position: self.peers[&connection].world.body_position(),
                })
                .collect();
            let seconds = now.saturating_sub(room.flow_at).as_secs_f64();
            room.flow_at = now;
            let was = flow.phase();
            let events = flow.advance(seconds, &participants);
            let variables = flow.variables();
            let mut messages = room
                .mode
                .as_mut()
                .map_or_else(Vec::new, |mode| mode.advance(tick, &variables));
            let changed = events
                .iter()
                .any(|event| matches!(event, FlowEvent::Phase(_)));
            if changed || now >= room.next_mode_frame {
                if let Some(mode) = &room.mode {
                    match mode.frame(&world.scene, &variables) {
                        Ok(frame) => room.mode_frame = Some(frame),
                        Err(error) => tracing::warn!("[game] no game mode frame: {error}"),
                    }
                }
                room.next_mode_frame = now.saturating_add(MODE_FRAME_EVERY);
            }
            room.assembled = was == Phase::Assemble && flow.phase() != Phase::Assemble;
            let mut respawn = None;
            for event in &events {
                match event {
                    FlowEvent::Phase(phase) => {
                        tracing::info!(
                            "[game] match {:04X}: {phase:?}, {:.1} s",
                            id[0] & 0xFFFF,
                            flow.clock()
                        );
                        // A new side (after the first) reopens hero select
                        // in its spawn rooms; a new Control round after the
                        // first and every Elimination round put everyone
                        // back in spawn.
                        let sided = flow.attackers().is_some();
                        match phase {
                            Phase::Assemble if sided && flow.round() > 0 => {
                                respawn = Some(true);
                            }
                            Phase::Round
                                if flow.mode() == Mode::Elimination
                                    || (flow.mode() == Mode::Control && flow.round() > 1) =>
                            {
                                respawn = Some(false);
                            }
                            _ => {}
                        }
                    }
                    FlowEvent::Message {
                        message,
                        parameters,
                    } => messages.extend(connections.iter().filter_map(|(connection, _)| {
                        Some(GameMessage {
                            sender: GAME_MODE_ENTITY,
                            target: self.peers[connection].world.current_body()?,
                            message: *message,
                            parameters: parameters.clone(),
                        })
                    })),
                    FlowEvent::Objective(objective) => tracing::info!(
                        "[game] match {:04X}: objective {objective} taken",
                        id[0] & 0xFFFF
                    ),
                    FlowEvent::Overtime(on) => {
                        tracing::info!("[game] match {:04X}: overtime {on}", id[0] & 0xFFFF)
                    }
                    FlowEvent::RoundOver(winner) => tracing::info!(
                        "[game] match {:04X}: round {} won by {winner:?}",
                        id[0] & 0xFFFF,
                        flow.round()
                    ),
                    FlowEvent::MatchOver => {
                        room.ended = true;
                        // The AI heroes get no result: the slots of the
                        // result count the players (the matchmaker's seats)
                        // only.
                        let players = room
                            .players
                            .iter()
                            .enumerate()
                            .filter(|(_, player)| !player.ai)
                            .enumerate()
                            .map(|(seat, (slot, player))| PlayerResult {
                                slot: seat,
                                name: player.info.name.clone(),
                                account: player.card.account,
                                team: player.info.team,
                                outcome: match flow.standing(slot, player.info.team) {
                                    Some(Standing::Won) => Outcome::Win,
                                    Some(Standing::Drew) => Outcome::Draw,
                                    _ => Outcome::Loss,
                                },
                                finished: !player.gone
                                    && connections.iter().any(|&(_, seated)| seated == slot),
                                medals: medal_counts(&team_medals(
                                    &room.players,
                                    room.map.free_for_all,
                                    room.map.mode_guid,
                                    slot,
                                    world.scene.stats(),
                                )),
                            })
                            .collect();
                        tracing::info!("[game] match {:04X} is over", id[0] & 0xFFFF);
                        self.results.push(MatchResult {
                            id,
                            map: room.map.map_guid,
                            mode: room.map.mode_guid,
                            players,
                        });
                    }
                    FlowEvent::Finished => {
                        home.extend(connections.iter().map(|&(connection, _)| connection));
                    }
                }
            }
            let extra = Duration::from_secs_f64(flow.respawn_extra());
            for message in messages {
                for (connection, _) in &connections {
                    let peer = self.peers.get_mut(connection).expect("room peer");
                    if peer.world.receive_body_message(&message) {
                        break;
                    }
                }
            }
            if let Some(select) = respawn {
                let world = room.world.as_ref().expect("room with a world");
                for &(connection, slot) in &connections {
                    let viewer = room.appearance(slot).expect("seated player");
                    let peer = self.peers.get_mut(&connection).expect("room peer");
                    if let Err(error) = peer.world.spawn_again(
                        &world.scene,
                        &viewer,
                        tick,
                        now,
                        room.players[slot].quanta(),
                        select,
                    ) {
                        eprintln!("{:08X} could not respawn ({error})", viewer.entity);
                    }
                }
            }
            for (connection, _) in &connections {
                if let Some(peer) = self.peers.get_mut(connection) {
                    peer.world.set_respawn_extra(extra);
                }
            }
        }
        for connection in home {
            if !self.links.cancel_handoff(connection) && self.peers.contains_key(&connection) {
                self.send_home(connection, now);
            }
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

    fn tick(&mut self, socket: &impl Transport, now: Duration, missed: u64) {
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
        let mut bot_voices = HashMap::new();
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
                    // The live bots' reactions to their hits, and the voice
                    // lines they call for.
                    world.scene.advance_live_bots(tick);
                    bot_voices.insert(id, world.scene.take_bot_voices());
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
        // match.py `update`: the countdown (now the match flow) runs before
        // the players' steps.
        if !drain {
            self.update_flows(tick, now);
            // The AI heroes' input for this tick (ai.rs).
            self.drive_ai(tick, now);
        }
        let timers = self.timers;
        for (&connection, peer) in &mut self.peers {
            if peer.ai.is_some() {
                continue;
            }
            let (id, slot) = self.seats[&connection];
            let room = self.rooms.get_mut(&id).expect("admitted room");
            if !drain
                && peer
                    .update(&self.schemas, room, slot, timers, now, tick)
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
                // ClientInVoice 27500: each bot line to every client of the
                // room with a body (voice_lines.rs).
                if let Some(body) = peer.world.current_body() {
                    for voice in bot_voices.get(&id).into_iter().flatten() {
                        peer.notice(voice.notice(body));
                    }
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
            self.say_player_lines(tick);
            self.show_players(tick, now, send_frames);
        }
        for (&connection, peer) in &mut self.peers {
            if peer.ai.is_some() {
                // What a client would get arrives at once (ai.rs).
                if send_frames && !drain {
                    peer.world.deliver_locally(tick, now);
                }
                continue;
            }
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

    /// ClientInVoice 27500 for the lines the players' bodies said this tick
    /// (the communication wheel's, `orpheus_sim::voice_lines`): each goes to
    /// every client of the room whose body is within the stimulus's radius
    /// of the speaker's (35 m for the wheel's), the speaker's own client
    /// too. Assumption: the client doesn't apply the radius itself (the
    /// bots' lines go to every client, which leaves it to the client).
    fn say_player_lines(&mut self, tick: u32) {
        let mut said: HashMap<MatchId, Vec<(VoiceLine, [f64; 3])>> = HashMap::new();
        for (connection, peer) in &mut self.peers {
            let Some(&(id, _)) = self.seats.get(connection) else {
                continue;
            };
            let Some(world) = self.rooms.get(&id).and_then(|room| room.world.as_ref()) else {
                continue;
            };
            let lines = peer.world.take_voice_lines(&world.scene, tick);
            if let Some(position) = peer.world.hearing_position() {
                said.entry(id)
                    .or_default()
                    .extend(lines.into_iter().map(|line| (line, position)));
            }
        }
        if said.is_empty() {
            return;
        }
        for (connection, peer) in &mut self.peers {
            if peer.ai.is_some() {
                continue;
            }
            let Some(lines) = self.seats.get(connection).and_then(|(id, _)| said.get(id)) else {
                continue;
            };
            let (Some(body), Some(position)) =
                (peer.world.current_body(), peer.world.hearing_position())
            else {
                continue;
            };
            for (line, speaker) in lines {
                if line.heard(*speaker, position) {
                    peer.notice(line.notice(body));
                }
            }
        }
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

/// Where the game's datagrams go and come from: the UDP socket, or in tests
/// an in-memory network on a virtual clock (`lockstep.rs`).
trait Transport {
    fn send_to(&self, datagram: &[u8], address: SocketAddr) -> io::Result<usize>;
    fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)>;
}

impl Transport for UdpSocket {
    fn send_to(&self, datagram: &[u8], address: SocketAddr) -> io::Result<usize> {
        UdpSocket::send_to(self, datagram, address)
    }

    fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        UdpSocket::recv_from(self, buffer)
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

/// The game thread's loop: the matches and when the next tick is due.
struct GameLoop {
    state: State,
    due: Duration,
    /// An oversized datagram must never be accepted as a truncated valid one.
    buffer: [u8; 65_536],
}

/// What one pass of the loop left waiting.
struct Pass {
    handled: usize,
    socket_ready: bool,
    input_ready: bool,
}

impl Pass {
    /// More waits than one pass took: the next pass runs at once.
    fn busy(&self) -> bool {
        self.socket_ready || self.input_ready || self.handled == RECEIVE_BATCH
    }
}

impl GameLoop {
    fn new(schemas: Arc<Schemas>) -> Self {
        Self {
            state: State::new(schemas),
            due: Duration::ZERO,
            buffer: [0; 65_536],
        }
    }

    /// One pass of the loop: a batch of requests, a batch of datagrams if
    /// the socket is readable, the queued input, then the tick if it is
    /// due. `clock` reads the time since the loop started; the real loop
    /// (`run`) reads the wall clock, the tests a virtual one (`lockstep.rs`).
    fn pass(
        &mut self,
        socket: &impl Transport,
        requests: &mpsc::Receiver<Request>,
        mut socket_ready: bool,
        clock: impl Fn() -> Duration,
    ) -> io::Result<Pass> {
        let mut handled = 0;
        for request in requests.try_iter().take(RECEIVE_BATCH) {
            self.state.handle_request(request, clock());
            handled += 1;
        }
        if socket_ready {
            for _ in 0..RECEIVE_BATCH {
                match socket.recv_from(&mut self.buffer) {
                    Ok((count, address)) => {
                        self.state
                            .receive(socket, &self.buffer[..count], address, clock());
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
        let input_ready = self.state.advance_input(clock());
        let now = clock();
        if now >= self.due {
            let (next, missed) = next_tick_after(self.due, now)
                .ok_or_else(|| io::Error::other("game tick clock exhausted"))?;
            self.due = next;
            self.state.tick(socket, now, missed);
        }
        Ok(Pass {
            handled,
            socket_ready,
            input_ready,
        })
    }
}

fn run(
    socket: UdpSocket,
    mut poll: Poll,
    requests: mpsc::Receiver<Request>,
    stopping: Arc<AtomicBool>,
    schemas: Arc<Schemas>,
) -> io::Result<()> {
    let mut game = GameLoop::new(schemas);
    let start = Instant::now();
    let mut events = Events::with_capacity(2);
    let mut socket_ready = false;
    while !stopping.load(Ordering::Acquire) {
        let pass = game.pass(&socket, &requests, socket_ready, || start.elapsed())?;
        socket_ready = pass.socket_ready;
        let timeout = if pass.busy() {
            Duration::ZERO
        } else {
            game.due.saturating_sub(start.elapsed())
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

/// A debugging switch: `ORPHEUS_DEBUG_DROP_NOTICES` lists combat notices the
/// server leaves out (`hit`, `kill-flag` (25001 sent without its kill bit),
/// `elimination` (25005), `death` (25004)), to find which one stalls the
/// client's own body after a kill (leafall903/ORPHEUS issue 14).
#[derive(Default)]
struct DroppedNotices {
    hit: bool,
    kill_flag: bool,
    elimination: bool,
    death: bool,
}

static DROPPED_NOTICES: std::sync::LazyLock<DroppedNotices> = std::sync::LazyLock::new(|| {
    let Ok(list) = std::env::var("ORPHEUS_DEBUG_DROP_NOTICES") else {
        return DroppedNotices::default();
    };
    let mut dropped = DroppedNotices::default();
    for item in list
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        match item {
            "hit" => dropped.hit = true,
            "kill-flag" => dropped.kill_flag = true,
            "elimination" => dropped.elimination = true,
            "death" => dropped.death = true,
            other => tracing::warn!("ORPHEUS_DEBUG_DROP_NOTICES: unknown notice {other:?}"),
        }
    }
    tracing::warn!("debugging: dropping combat notices {list:?}");
    dropped
});

impl DroppedNotices {
    fn filter(&self, notice: CombatNotice) -> Option<CombatNotice> {
        match notice {
            CombatNotice::Hit { .. } if self.hit => None,
            CombatNotice::Hit {
                target,
                dealt,
                critical,
                shot_frame,
                ..
            } if self.kill_flag => Some(CombatNotice::Hit {
                target,
                dealt,
                critical,
                killed: false,
                shot_frame,
            }),
            CombatNotice::Elimination { .. } if self.elimination => None,
            CombatNotice::Death { .. } if self.death => None,
            notice => Some(notice),
        }
    }
}

mod ai;
#[cfg(test)]
pub(crate) mod lockstep;

#[cfg(test)]
mod tests;
