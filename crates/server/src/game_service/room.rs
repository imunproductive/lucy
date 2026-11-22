//! A room: one match on its own loop and clock. Its thread runs it
//! (`router.rs`), or in the tests the lockstep (`lockstep.rs`), through the
//! same synchronous step ([`RoomLoop::pass`]): what came in (its games'
//! datagrams, the requests for it), the queued input, then the tick when it
//! is due.
//!
//! A panic in a pass (or an error of its loop) ends the match alone
//! ([`RoomLoop::fail`]): the room sends every game home (20304) as at the
//! end of the clock's frame range, and its seats go as the games close
//! their links or time out, which the lobby sees as their leaving. Only a
//! build that unwinds catches the panic: the release and `gametest` builds.
//! A debug build (Cranelift) aborts the process.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    ops::Range,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use indexmap::IndexMap;
use lucy_content::{
    hero_settings,
    heroes::SOLDIER,
    maps::{GameMap, SpawnChoice},
    stats::PlayerStats,
};
use lucy_core::phase::{self, Phase};
use lucy_lobby::handoff::Handoff;
use lucy_sim::{
    ai::Random,
    bot_plate::BotPlate,
    combat::Combat,
    instance::{Instance, Member},
    match_flow::{MatchFlow, Timers},
    messages::{Backlog, HeldReach},
    mode_script::ModeRunner,
    mode_view::ModeFrame,
    scene::{PlayerAppearance, PlayerCard},
    scene_assets::PreparedScene,
    workshop::WorkshopRun,
};
use lucy_wire::{
    entity::{MAX_PAYLOAD, frame},
    messages::{Message, encode},
};

use super::{
    DebugKill, DebugSeat, FIRST_AI_CONNECTION, LATENCY_STEP, MAX_PONGS, MatchSnapshot, OpenReply,
    Opening, Peer, Player, PlayerSnapshot, PlayerState, RECEIVE_BATCH, Room, RoomPlate,
    RoomRequest, RoomSettings, RoomSnapshot, RoomState, RoomWorkshop, Transport, WorldPlayer,
    WorldStart, ai, check_scene, invalid, leave_message, pick_message, player_entity,
    router::Inbound, send, spawn_number, take_placeable_removals, timing::TickTiming,
};
use crate::{
    clock::{GameClock, Step, next_tick_after},
    connection::{GameConnections, Received, SILENCE_TIME, Timeout},
    ingress::{IngressError, MessageKind, Pong},
};

/// A pass takes at most this many datagrams and requests; more make it
/// busy, so the next pass comes at once.
const INBOX_BATCH: usize = RECEIVE_BATCH;

/// A room's loop: its state, and when its next tick is due on the
/// service's clock.
pub(super) struct RoomLoop {
    state: RoomState,
    due: Duration,
    /// The answer to the request that opened the room, which its first
    /// pass sends.
    opened: Option<(OpenReply, Vec<Handoff>)>,
    /// The match stopped (a panic, an error of its loop): the room only
    /// sends its players home.
    failed: bool,
    /// It failed again while it did: it ends at once, and its games time
    /// out on their own.
    abandoned: bool,
}

/// What one pass left waiting.
#[derive(Default)]
pub(super) struct Pass {
    handled: usize,
    input_ready: bool,
}

impl Pass {
    /// More waits than one pass took: the next pass runs at once.
    pub(super) fn busy(&self) -> bool {
        self.handled == INBOX_BATCH || self.input_ready
    }
}

impl RoomLoop {
    /// The loop of a room that opened at `now`, whose opener gets `tickets`
    /// on `reply`.
    pub(super) fn new(
        state: RoomState,
        now: Duration,
        reply: OpenReply,
        tickets: Vec<Handoff>,
    ) -> Self {
        let (_, due) = GameClock::opened(now);
        Self {
            state,
            due,
            opened: Some((reply, tickets)),
            failed: false,
            abandoned: false,
        }
    }

    /// When the next tick is due, since the service started.
    pub(super) fn due(&self) -> Duration {
        self.due
    }

    /// The room is done: its last game left, or its match failed twice.
    pub(super) fn finished(&self) -> bool {
        self.abandoned || self.state.finished()
    }

    /// One pass: what came in, the queued input, then the tick if it is
    /// due. The input the tick gave (the AI players' commands, `drive_ai`)
    /// makes the pass busy, so they run in the next pass, at once. `clock`
    /// reads the time since the service started: the wall clock on a
    /// room's thread, a virtual one in the tests (`lockstep.rs`).
    pub(super) fn pass(
        &mut self,
        socket: &impl Transport,
        inbound: impl Iterator<Item = Inbound>,
        clock: impl Fn() -> Duration,
    ) -> Pass {
        if let Some((reply, tickets)) = self.opened.take()
            && let Err(mpsc::SendError(Ok(tickets))) = reply.send(Ok(tickets))
        {
            for ticket in tickets {
                self.state.links.cancel_handoff(ticket.conn);
            }
        }
        if self.abandoned {
            return Pass::default();
        }
        // What the thread did before the pass (its wait, another room's
        // pass on the tests' thread) isn't the room's.
        phase::take();
        let stepped = panic::catch_unwind(AssertUnwindSafe(|| self.step(socket, inbound, &clock)));
        self.state.timing.add(&phase::take());
        match stepped {
            Ok(Ok(pass)) => pass,
            Ok(Err(error)) => {
                self.fail(&error.to_string(), clock());
                Pass::default()
            }
            Err(panic) => {
                let why = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("a panic");
                self.fail(why, clock());
                Pass::default()
            }
        }
    }

    fn step(
        &mut self,
        socket: &impl Transport,
        inbound: impl Iterator<Item = Inbound>,
        clock: &impl Fn() -> Duration,
    ) -> io::Result<Pass> {
        let mut handled = 0;
        for inbound in inbound.take(INBOX_BATCH) {
            self.state.take(socket, inbound, clock());
            handled += 1;
        }
        let now = clock();
        let _simulate = phase::enter(Phase::Simulate);
        // The tick that is due runs each player's command of its own
        // (`RoomState::release_input`) before it simulates and sends.
        let due = if now >= self.due {
            let late = now - self.due;
            let (next, missed) = next_tick_after(self.due, now)
                .ok_or_else(|| io::Error::other("game tick clock exhausted"))?;
            self.due = next;
            let ticks = u32::try_from(missed).unwrap_or(u32::MAX).saturating_add(1);
            self.state.release_input(ticks);
            Some((missed, late))
        } else {
            None
        };
        let mut input_ready = self.state.advance_input(now);
        if let Some((missed, late)) = due {
            self.state.tick(socket, now, missed, clock);
            // Each tick's frame and its start against its time on the grid:
            // the wake-up and the datagrams taken first (`timer.rs`).
            tracing::debug!(
                target: "lucy::tick",
                frame = self.state.clock.frame(),
                late_us = late.as_micros(),
                missed,
                "tick"
            );
            // An AI player's command runs right after the tick that gave
            // it, before whatever comes in next: not when the next
            // datagram, request or tick wakes the loop.
            input_ready = self.state.input_waits();
            self.state.timing.add(&phase::take());
            if let Some(window) = self.state.timing.tick(now, late, missed) {
                tracing::info!(
                    target: "lucy::timing",
                    "[game] match {:04X}: {window}",
                    self.state.id[0] & 0xFFFF
                );
            }
        }
        Ok(Pass {
            handled,
            input_ready,
        })
    }

    /// The match stopped: its players go home ([`RoomState::abandon`]).
    /// If it stops again while they do, the room ends at once.
    fn fail(&mut self, why: &str, now: Duration) {
        let id = self.state.id[0] & 0xFFFF;
        if self.failed {
            tracing::error!(
                "[game] match {id:04X} failed again while it sent its players home ({why}): it ends"
            );
            self.abandoned = true;
            return;
        }
        tracing::error!("[game] match {id:04X} stopped ({why}): its players go home");
        self.failed = true;
        if panic::catch_unwind(AssertUnwindSafe(|| self.state.abandon(now))).is_err() {
            self.abandoned = true;
        }
    }

    /// Tests: the match waits for the scene its thread prepares.
    #[cfg(test)]
    pub(super) fn awaits_scene(&self) -> bool {
        let room = &self.state.room;
        !room.closed() && !room.ended && room.world.is_none()
    }
}

impl RoomState {
    /// A room for a new match (match.py `create_match`): its players'
    /// seats on the connections `allocate` gives, and with `ai` the AI
    /// heroes that fill its teams. Its clock is on the service's grid at
    /// `now` (`GameClock::opened`).
    pub(super) fn open(
        opening: Opening,
        settings: RoomSettings,
        now: Duration,
        allocate: impl FnOnce(usize) -> io::Result<Vec<u32>>,
    ) -> io::Result<(Self, Vec<Handoff>)> {
        let Opening {
            id,
            map,
            players,
            world,
            ai,
        } = opening;
        let remaining = players.len();
        if remaining == 0 || remaining > usize::from(u8::MAX) {
            return Err(invalid("invalid match roster size"));
        }
        let mut room = Room::new(map, players, world);
        let seats = ai.map_or_else(Vec::new, |difficulty| {
            let mut random = Random::new(ai::seed());
            room.add_ai(difficulty, &mut random)
        });
        let RoomSettings {
            schemas,
            timers,
            bot_plate,
            hurt,
            events,
        } = settings;
        let (clock, _) = GameClock::opened(now);
        let mut state = Self {
            id,
            schemas,
            clock,
            links: GameConnections::default(),
            room,
            seats: HashMap::new(),
            peers: IndexMap::new(),
            homeward: HashMap::new(),
            next_ai_connection: FIRST_AI_CONNECTION,
            next_input_peer: 0,
            timers,
            bot_plate,
            hurt,
            events,
            timing: TickTiming::default(),
            #[cfg(all(test, unwinds))]
            fail: false,
        };
        state.check_roster()?;
        let connections = allocate(remaining)?;
        let tickets = state.reserve(0..remaining, &connections, now)?;
        state.seat_ai(seats);
        Ok((state, tickets))
    }

    /// The room is done: its last player left and every link it had is
    /// gone.
    fn finished(&self) -> bool {
        self.room.closed() && self.links.is_empty()
    }

    /// What came in for the room.
    fn take(&mut self, socket: &impl Transport, inbound: Inbound, now: Duration) {
        match inbound {
            Inbound::Datagram(datagram) => {
                let _receive = phase::enter(Phase::Receive);
                self.receive(
                    socket,
                    &datagram.bytes,
                    datagram.from,
                    datagram.arrived,
                    now,
                );
            }
            Inbound::Request(request) => self.handle_request(request, now),
            Inbound::Snapshot(reply) => {
                if let Some(snapshot) = self.snapshot() {
                    let _ = reply.send(snapshot);
                }
            }
            Inbound::BotPlate {
                plate,
                messages,
                reply,
            } => {
                let _ = reply.send(self.tune_bot_plate(plate, &messages));
            }
            Inbound::Hurt(hurt) => self.hurt = hurt,
        }
    }

    /// More players for the match, in the next slots, on the connections
    /// the router gave them (server.py `join_match`); `None` once it has
    /// ended or while its clock drains. The connections it doesn't take go
    /// back to the router.
    fn join_match(
        &mut self,
        players: Vec<WorldPlayer>,
        connections: Vec<u32>,
        now: Duration,
    ) -> io::Result<Option<Vec<Handoff>>> {
        let joined = self.join(players, &connections, now);
        if !matches!(joined, Ok(Some(_))) {
            for connection in connections {
                self.events.closed(connection);
            }
        }
        joined
    }

    fn join(
        &mut self,
        players: Vec<WorldPlayer>,
        connections: &[u32],
        now: Duration,
    ) -> io::Result<Option<Vec<Handoff>>> {
        if self.clock.draining() || self.room.closed() || self.room.ended {
            return Ok(None);
        }
        let first = self.room.players.len();
        if players.is_empty() || first + players.len() > usize::from(u8::MAX) {
            return Err(invalid("invalid match roster size"));
        }
        self.room.add(players);
        let room = &self.room;
        let checked = self.check_roster().and_then(|()| {
            let Some(world) = &room.world else {
                return Ok(());
            };
            let teams: Vec<u8> = room.players.iter().map(|player| player.info.team).collect();
            let cards: Vec<&PlayerCard> = room.players.iter().map(|player| &player.card).collect();
            check_scene(&world.scene, &room.map, &teams, &cards)
        });
        let count = room.players.len();
        let reserved = checked.and_then(|()| self.reserve(first..count, connections, now));
        if reserved.is_err() {
            self.room.players.truncate(first);
            self.room.remaining -= count - first;
        }
        reserved.map(Some)
    }

    /// The roster's 20300 fits a datagram.
    fn check_roster(&self) -> io::Result<()> {
        let initial = self.room.message(0, true);
        let bytes = encode(&self.schemas, initial.id, &initial.fields).map_err(invalid)?;
        let payload = frame(u32::MAX, u32::MAX, 0, &[(0, bytes)], &[], &[]).map_err(invalid)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(invalid("match roster exceeds the game datagram budget"));
        }
        Ok(())
    }

    /// A seat for each of these slots of the match, on these connections.
    fn reserve(
        &mut self,
        slots: Range<usize>,
        connections: &[u32],
        now: Duration,
    ) -> io::Result<Vec<Handoff>> {
        let tickets = connections
            .iter()
            .map(|&connection| Handoff::new(connection, self.id))
            .collect::<io::Result<Vec<_>>>()?;
        for (slot, ticket) in slots.zip(&tickets) {
            let reserved = self
                .links
                .reserve_handoff(ticket, u64::from(ticket.conn), now);
            assert!(reserved, "seat and connection tables are in sync");
            self.seats.insert(ticket.conn, slot);
        }
        Ok(tickets)
    }

    /// The match's scene is ready, or its collision failed: then every game
    /// goes back to the menu, those not connected yet lose their seats, and
    /// the match takes nobody more (match.py `_collision_failure`).
    fn scene_ready(&mut self, scene: io::Result<Box<PreparedScene>>, now: Duration) {
        if self.room.closed() {
            return;
        }
        let room = &mut self.room;
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
            self.id[0] & 0xFFFF,
            room.map.name
        );
        room.ended = true;
        let seated: Vec<u32> = self.seats.keys().copied().collect();
        for connection in seated {
            if !self.links.cancel_handoff(connection) && self.peers.contains_key(&connection) {
                self.send_home(connection, now);
            }
        }
    }

    /// The match with its players and their state (match.py `snapshot`);
    /// none once its last player left.
    fn snapshot(&self) -> Option<RoomSnapshot> {
        let room = &self.room;
        if room.closed() {
            return None;
        }
        let peers: HashMap<usize, &Peer> = self
            .peers
            .iter()
            .map(|(connection, peer)| (self.seats[connection], peer))
            .collect();
        let snapshot = MatchSnapshot {
            id: self.id,
            map: room.map.name.clone(),
            mode: room.map.mode_name.clone(),
            workshop_rules: room
                .workshop
                .as_ref()
                .map(|workshop| workshop.logic.rules.len()),
            timing: self.timing.snapshot(),
            waiting: room
                .flow
                .as_ref()
                .is_none_or(|flow| flow.phase() <= lucy_sim::match_flow::Phase::Assemble),
            players: room
                .players
                .iter()
                .enumerate()
                .map(|(slot, player)| {
                    let peer = peers.get(&slot);
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
        };
        Some(RoomSnapshot {
            started: room.started,
            snapshot,
        })
    }

    fn handle_request(&mut self, request: RoomRequest, now: Duration) {
        match request {
            RoomRequest::Join {
                players,
                connections,
                reply,
                ..
            } => {
                let result = self.join_match(players, connections, now);
                if let Err(mpsc::SendError(Ok(Some(tickets)))) = reply.send(result) {
                    for ticket in tickets {
                        self.links.cancel_handoff(ticket.conn);
                    }
                }
            }
            RoomRequest::Scene { scene, .. } => self.scene_ready(scene, now),
            RoomRequest::Workshop {
                logic,
                catalog,
                reply,
                ..
            } => {
                let found = !self.room.closed();
                if found {
                    self.room.workshop = Some(RoomWorkshop {
                        logic,
                        catalog,
                        run: None,
                    });
                }
                let _ = reply.send(found);
            }
            #[cfg(test)]
            RoomRequest::Bodies { reply, .. } => {
                let _ = reply.send(self.bodies());
            }
            #[cfg(test)]
            RoomRequest::WorkshopReport { reply, .. } => {
                let _ = reply.send(self.workshop_report());
            }
            #[cfg(all(test, unwinds))]
            RoomRequest::Fail { reply, .. } => {
                self.fail = true;
                let _ = reply.send(());
            }
            #[cfg(test)]
            RoomRequest::Hold { release, reply, .. } => {
                let _ = reply.send(());
                let _ = release.recv();
            }
            RoomRequest::Pending { connection, reply } => {
                let _ = reply.send(self.links.handoff_pending(connection, now));
            }
            RoomRequest::Cancel { connection, reply } => {
                let _ = reply.send(self.links.cancel_handoff(connection));
            }
            RoomRequest::Seat {
                connection,
                id,
                release,
                reply,
            } => {
                let exists = self.seat(connection, id).is_some();
                if release && exists {
                    if self.links.waiting(connection) {
                        self.take_out_waiting(connection);
                    } else {
                        self.send_home(connection, now);
                    }
                }
                let _ = reply.send(exists);
            }
            RoomRequest::Skin {
                connection,
                id,
                hero,
                skin,
                reply,
            } => {
                if let Some(slot) = self.seat(connection, id)
                    && self.room.world.is_some()
                {
                    self.room.players[slot].card.skins.insert(hero, skin);
                    if let Some(peer) = self.peers.get_mut(&connection) {
                        peer.world.queue_skin(hero, self.clock.frame(), now);
                    }
                }
                let _ = reply.send(());
            }
            RoomRequest::Hero {
                connection,
                id,
                hero,
                reply,
            } => {
                let _ = reply.send(self.switch_hero(connection, id, hero, now));
            }
            RoomRequest::DebugSeat {
                connection,
                id,
                reply,
            } => {
                let _ = reply.send(self.debug_seat(connection, id));
            }
            RoomRequest::DebugSend {
                connection,
                id,
                messages,
                reply,
            } => {
                let sent = self.debug_seat(connection, id).is_some_and(|_| {
                    let peer = &mut self.peers[&connection];
                    messages
                        .into_iter()
                        .all(|message| peer.egress.queue_reliable(message).is_ok())
                });
                let _ = reply.send(sent);
            }
            RoomRequest::DebugKill {
                connection,
                id,
                reply,
            } => {
                let _ = reply.send(self.debug_kill(connection, id));
            }
        }
    }

    /// The slot of the seat of `connection` in match `id`.
    fn seat(&self, connection: u32, id: super::MatchId) -> Option<usize> {
        if id != self.id {
            return None;
        }
        self.seats.get(&connection).copied()
    }

    /// Tests: what the Workshop match's engine and action graphs met.
    #[cfg(test)]
    fn workshop_report(&mut self) -> Option<super::WorkshopReport> {
        if self.room.closed() {
            return None;
        }
        let room = &mut self.room;
        let workshop = room.workshop.as_ref()?;
        let run = workshop.run.as_ref();
        Some(super::WorkshopReport {
            running: run.is_some(),
            unsupported: run
                .map(|run| run.engine().unsupported())
                .unwrap_or_default(),
            faults: run
                .map(|run| run.engine().faults().to_vec())
                .unwrap_or_default(),
            gaps: room
                .mode
                .as_mut()
                .and_then(ModeRunner::script_mut)
                .map(|script| script.gaps().into_iter().collect())
                .unwrap_or_default(),
        })
    }

    /// The entities of a seat of match `id` whose game is connected.
    fn debug_seat(&self, connection: u32, id: super::MatchId) -> Option<DebugSeat> {
        let slot = self.seat(connection, id)?;
        let peer = self.peers.get(&connection)?;
        Some(DebugSeat {
            controller: self.room.players[slot].entity,
            body: peer.world.current_body(),
        })
    }

    /// The environment's kill of a seat's living body ([`DebugKill`]),
    /// resolved with the next tick's hits.
    fn debug_kill(&mut self, connection: u32, id: super::MatchId) -> DebugKill {
        let Some(seat) = self.debug_seat(connection, id) else {
            return DebugKill::NotConnected;
        };
        let Some(body) = seat.body else {
            return DebugKill::NoBody;
        };
        let world = &self.peers[&connection].world;
        if world.body_dead() {
            return DebugKill::Dead;
        }
        if world.in_spawn_room() == Some(true) {
            return DebugKill::InSpawnRoom;
        }
        let tick = self.clock.frame();
        let slot = self.seats[&connection];
        let team = self.room.players[slot].info.team;
        let Some(world) = self.room.world.as_mut() else {
            return DebugKill::NotConnected;
        };
        world
            .combat
            .get_or_insert_with(|| Combat::new(&world.scene))
            .environment_kill(body, team, tick);
        DebugKill::Killed(body)
    }

    /// A pick of the hero, as the client's own pick message (21616) carries
    /// it, queued with the seat's other game input.
    fn switch_hero(
        &mut self,
        connection: u32,
        id: super::MatchId,
        hero: u64,
        now: Duration,
    ) -> bool {
        if self.clock.draining() {
            return false;
        }
        let Some(slot) = self.seat(connection, id) else {
            return false;
        };
        let room = &self.room;
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

    /// The live nameplate tool's plate, which the room takes now
    /// ([`Self::sync_bot_plates`]), and its messages to the live bots: how
    /// many of the room's took each, and what its first live bot sends now.
    fn tune_bot_plate(&mut self, plate: Arc<BotPlate>, messages: &[u64]) -> RoomPlate {
        self.bot_plate = plate;
        self.sync_bot_plates();
        let mut world = self.room.world.as_mut();
        let taken = messages
            .iter()
            .map(|&message| {
                world
                    .as_mut()
                    .map_or(0, |world| world.scene.message_bot_plates(message))
            })
            .collect();
        let sample = self.room.world.as_ref().and_then(|world| {
            let combat = world.combat.as_ref();
            world
                .scene
                .bot_plate_sample(|slot| combat.and_then(|combat| combat.bot_vitals(slot)))
        });
        RoomPlate {
            started: self.room.started,
            taken,
            sample,
        }
    }

    /// The room takes the live nameplate tool's plate if it hasn't yet: its
    /// bots' scripts at once, their pools as an update to each of its
    /// clients.
    fn sync_bot_plates(&mut self) {
        let Some(WorldStart { scene, combat }) = &mut self.room.world else {
            return;
        };
        if Arc::ptr_eq(scene.bot_plate(), &self.bot_plate) {
            return;
        }
        let combat = combat.as_ref();
        let updates = scene.set_bot_plate(Arc::clone(&self.bot_plate), |slot| {
            combat.and_then(|combat| combat.bot_vitals(slot))
        });
        for peer in self.peers.values_mut() {
            if peer.ai.is_some() {
                continue;
            }
            for update in &updates {
                peer.world.queue_health(update.clone());
            }
        }
    }

    /// A player whose game has not connected leaves the match now; if his
    /// game still connects, it gets only 20304 (server.py `take_out`).
    fn take_out_waiting(&mut self, connection: u32) {
        let slot = self.seats[&connection];
        let player = &self.room.players[slot];
        tracing::info!(
            "[game] match {:04X}: {} left before his game connected",
            self.id[0] & 0xFFFF,
            player.info.name
        );
        let peer = Peer::new(player.entity);
        self.retire(connection);
        self.homeward.insert(connection, peer);
    }

    /// Logs why a seated player's link ends (a silent drop once looked
    /// like a stopped match).
    fn report_leave(&self, connection: u32, why: &str) {
        let Some(&slot) = self.seats.get(&connection) else {
            return;
        };
        tracing::info!(
            "[game] match {:04X}: {} left: {why}",
            self.id[0] & 0xFFFF,
            self.room.players[slot].info.name
        );
    }

    fn retire(&mut self, connection: u32) {
        self.homeward.remove(&connection);
        self.peers.shift_remove(&connection);
        if let Some(slot) = self.seats.remove(&connection) {
            self.room.players[slot].gone = true;
            self.room.remaining -= 1;
            if self.room.closed() {
                self.close();
            }
        }
    }

    /// The match's last player left: its AI players go, and its world with
    /// them. The room only finishes the links of the games taken out
    /// before they connected.
    fn close(&mut self) {
        self.unseat_ai();
        self.room.world = None;
    }

    /// A datagram of one of the room's games, which the router read at
    /// `arrived` and the room takes at `now`.
    fn receive(
        &mut self,
        socket: &impl Transport,
        bytes: &[u8],
        address: SocketAddr,
        arrived: Duration,
        now: Duration,
    ) {
        let Ok(received) = self.links.receive(bytes, address, now) else {
            return;
        };
        let connection = received.connection;
        if received.closed {
            self.events.closed(connection);
        }
        if self.homeward.contains_key(&connection) {
            self.receive_homeward(socket, received, now);
            return;
        }
        let slot = self.seats[&connection];
        if received.admitted {
            self.peers
                .insert(connection, Peer::new(self.room.players[slot].entity));
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
        // A datagram with content is acknowledged at once, on its own: the
        // client's link round trip (LATENCY) ends when its frame loop reads
        // the first datagram that acknowledges the one it timed, so an
        // acknowledgement that waited for the next tick's frame added up
        // to a tick and its work (refs/reports/Latency.md §2, §5: 17 ms
        // → 10 ms at 60 FPS). The frames go as before.
        if received.payload.is_some()
            && let Ok(ack) = self.links.acknowledge(connection)
        {
            let _ = send(socket, ack);
        }
        if received.closed {
            self.report_leave(connection, "his game closed its link");
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
        let room = &self.room;
        if received.opened {
            let _ = peer.reliable(&self.schemas, room.message(slot, true));
        }
        let Some(payload) = received.payload else {
            return;
        };
        let Peer {
            ingress,
            loading,
            world,
            reported_latency,
            hero_settings: sent_settings,
            pongs,
            ..
        } = peer;
        let incoming = ingress.receive(&self.schemas, &payload, |message| {
            match MessageKind::from(message.id) {
                MessageKind::Ping => {
                    if let Ok(pong) = Pong::new(&message.fields, arrived, now) {
                        if pongs.len() == MAX_PONGS {
                            pongs.remove(0);
                        }
                        pongs.push(pong);
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
                        Some(part) => {
                            let listed: Vec<_> = part
                                .entries
                                .iter()
                                .map(|entry| {
                                    format!(
                                        "{:X}/{:X}={:X}",
                                        entry.hero, entry.setting, entry.value
                                    )
                                })
                                .collect();
                            tracing::info!(
                                "[game] hero settings of {name} (hero/setting=value; first {}, last {}): {}",
                                part.first,
                                part.last,
                                listed.join(" ")
                            );
                            if let Some(entries) = sent_settings.take(part) {
                                world.update_hero_settings(&entries);
                            }
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
        match incoming {
            Ok(input) => {
                if let Some(latency) = input.latency.filter(|&latency| {
                    reported_latency.is_none_or(|last| last.abs_diff(latency) >= LATENCY_STEP)
                }) {
                    *reported_latency = Some(latency);
                    let view_delay = input.commands.last().map(|command| command.extra);
                    tracing::info!(
                        target: "lucy::latency",
                        "[game] {} reports a {latency} ms round trip (view delay {view_delay:?} ms)",
                        room.players[slot].info.name
                    );
                }
                if room.world.is_some() {
                    world.queue_commands(self.clock.frame(), &input.commands);
                }
            }
            // Its reliable messages pile up behind a lost one: the game is
            // broken, and its player leaves.
            Err(error @ IngressError::Backlog) => {
                tracing::warn!(
                    "[game] match {:04X}: {} is sent home: {error}",
                    self.id[0] & 0xFFFF,
                    room.players[slot].info.name
                );
                peer.leave(&mut self.links, &self.schemas, connection, now);
            }
            Err(_) => {}
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

    pub(super) fn send_home(&mut self, connection: u32, now: Duration) {
        if self.links.begin_leaving(connection, now) {
            let peer = self
                .peers
                .get_mut(&connection)
                .or_else(|| self.homeward.get_mut(&connection))
                .expect("admitted peer");
            let _ = peer.reliable(&self.schemas, leave_message());
        }
    }

    /// Every game goes back to the menu (20304): the match stops, as the
    /// clock's frame range ran out or the match failed.
    fn send_everyone_home(&mut self, now: Duration) {
        for (&connection, peer) in self.peers.iter_mut().chain(&mut self.homeward) {
            peer.clear_world();
            if self.links.begin_leaving(connection, now) {
                let _ = peer.reliable(&self.schemas, leave_message());
            }
        }
    }

    /// The match failed ([`RoomLoop::fail`]): it takes nobody more, the
    /// players whose games haven't connected leave now (a game that still
    /// connects gets only 20304), every connected game goes back to the
    /// menu, and the clock drains: the room's ticks only finish the links,
    /// until each game has closed its link or timed out.
    fn abandon(&mut self, now: Duration) {
        self.room.ended = true;
        let waiting: Vec<u32> = self
            .seats
            .keys()
            .copied()
            .filter(|&connection| self.links.waiting(connection))
            .collect();
        for connection in waiting {
            self.take_out_waiting(connection);
        }
        self.send_everyone_home(now);
        self.clock.drain();
    }

    /// The tick at `now`; `clock` reads the time each game's frame leaves.
    fn tick(
        &mut self,
        socket: &impl Transport,
        now: Duration,
        missed: u64,
        clock: &impl Fn() -> Duration,
    ) {
        let step = self.clock.step(missed, self.connections_pending());
        let (tick, drain, force) = match step {
            Step::Simulate { frame } => (frame, false, false),
            Step::Drain {
                frame,
                entered,
                force,
            } => {
                if entered {
                    self.send_everyone_home(now);
                }
                (frame, true, force)
            }
        };
        for expired in self.links.expire(now, force) {
            let why = match expired.reason {
                Timeout::Handoff => "his game did not connect in time",
                Timeout::Silence => "nothing from his game for 10 s",
                Timeout::LeaveGrace => "his game did not close its link after the leave",
                Timeout::Drain => "the server is draining",
            };
            self.events.closed(expired.connection);
            self.report_leave(expired.connection, why);
            self.retire(expired.connection);
        }
        // A closed room only finishes its links.
        let simulate = !drain && !self.room.closed();
        #[cfg(all(test, unwinds))]
        if simulate && self.fail {
            panic!("a test stops match {:04X}", self.id[0] & 0xFFFF);
        }
        let mut spawned = Vec::new();
        let mut bot_events = Vec::new();
        let mut bot_voices = Vec::new();
        if simulate {
            // A match that started since takes the live nameplate tool's
            // plate before its bots' scripts start.
            self.sync_bot_plates();
            if let Some(world) = &mut self.room.world {
                world.scene.set_hurt_feedback(self.hurt);
                world.scene.step_bots(tick);
            }
            self.update_combat(tick, now);
            let mut dummies = Vec::new();
            let mut scopes = Vec::new();
            let room = &mut self.room;
            let next = room.next_dummy();
            if let Some(world) = &mut room.world {
                // combat.py `update`: the dead bots' death scripts, then
                // the respawns with new scripts.
                world.scene.advance_bot_deaths(tick);
                // The live bots' reactions to their hits, and the voice
                // lines they call for.
                world.scene.advance_live_bots(tick);
                // The server placeables' scripts (placed_script.rs); their
                // messages (the spawn rooms' `0723`) go with the others
                // (`route_messages`). A mode graph that drives the mode
                // entity holds the spawn rooms' `0724` itself; else the
                // rooms the match flow's spawns hold get it here, from
                // the tick after the mode started (the placeables run
                // their first tick before it). The scripts' dummy bots
                // (the Training Partner) are AI players of the room
                // (`ai.rs`).
                let holds = match room.mode.as_ref() {
                    None => Vec::new(),
                    Some(mode) if mode.holds_rooms() => Vec::new(),
                    Some(_) => {
                        let flow = room.flow.as_ref();
                        world.scene.room_holds(|team| {
                            flow.map_or(SpawnChoice::First, |flow| flow.spawn_choice(team))
                        })
                    }
                };
                world.scene.set_next_dummy(Some(next));
                // The placeables the mode entity's graphs removed (`0CBB`
                // st34: the health packs, when the ruleset turns them
                // off) go before their scripts run again.
                if let Some(mode) = room.mode.as_mut() {
                    take_placeable_removals(mode, &mut world.scene);
                }
                world.scene.advance_placed(tick, &holds);
                dummies = world.scene.take_dummy_changes();
                scopes = world.scene.take_scope_changes();
                bot_voices = world.scene.take_bot_voices();
                if let Some(combat) = &mut world.combat
                    && let Ok(replacements) = combat.respawn_due(&mut world.scene, tick, now)
                    && !replacements.is_empty()
                {
                    world.scene.prepare_respawned_bots(tick);
                    spawned = replacements;
                }
                bot_events = world.scene.take_bot_events();
            }
            // The mode starts after the placeables' first tick, with
            // what they reported (`mode_script::reported_objectives`).
            room.begin_mode(tick);
            self.change_dummies(dummies);
            self.change_scopes(tick, scopes);
            // match.py `update`: the countdown (now the match flow) runs
            // before the players' steps.
            self.update_flows(tick, now);
            // The AI heroes' input for this tick (ai.rs).
            let _ai = phase::enter(Phase::Ai);
            self.drive_ai(tick, now);
        }
        // What every player's graphs counted (the AI heroes' too), what the
        // entities they spawned counted for them (the Biotic Field's `0258`
        // rid14) and what the room counts natively (shots, the training
        // bots' damage, heals and kills) go to the stat tables before the
        // boards go out.
        let Room { players, world, .. } = &mut self.room;
        for peer in self.peers.values_mut() {
            let tracked = peer.world.take_tracked_stats();
            if let Some(world) = world {
                super::flow::credit_tracked_stats(players, world.scene.stats(), tracked);
            }
        }
        if let Some(world) = world {
            let tracked = world
                .combat
                .as_mut()
                .map(Combat::take_tracked_stats)
                .unwrap_or_default();
            super::flow::credit_tracked_stats(players, world.scene.stats(), tracked);
        }
        let timers = self.timers;
        for (&connection, peer) in &mut self.peers {
            if peer.ai.is_some() {
                continue;
            }
            let slot = self.seats[&connection];
            let room = &mut self.room;
            if simulate
                && let Err(error) = peer.update(&self.schemas, room, slot, timers, now, tick)
            {
                tracing::warn!(
                    "[game] match {:04X}: {} is sent home: {error}",
                    self.id[0] & 0xFFFF,
                    room.players[slot].info.name
                );
                peer.leave(&mut self.links, &self.schemas, connection, now);
            }
            if simulate {
                if let Some(world) = &room.world {
                    peer.world.update_bots(&world.scene, &bot_events, tick);
                    peer.world.update_placed(&world.scene);
                }
                // ClientInVoice 27500: each bot line to every client of the
                // room whose body hears it: its audience lists the body
                // (a pain line's: the bot and its attacker), within the
                // stimulus's radius of the bot (voice_lines.rs).
                if let (Some(body), Some(position)) =
                    (peer.world.current_body(), peer.world.hearing_position())
                {
                    for (voice, speaker) in &bot_voices {
                        if voice.heard(body, *speaker, position) {
                            peer.notice(voice.notice(body));
                        }
                    }
                }
            }
            if peer.world.world_started() {
                peer.combat_ui.replacements.extend(
                    spawned
                        .iter()
                        .cloned()
                        .map(|bot| (now.saturating_add(SILENCE_TIME), bot)),
                );
            }
            if let Some(world) = room.world.as_ref().filter(|_| simulate) {
                let hero = peer.world.current_hero();
                peer.world
                    .send_board(now, || super::flow::board(room, slot, &world.scene, hero));
            }
        }
        if simulate {
            self.say_player_lines(tick);
            self.route_messages(tick);
            let _replicate = phase::enter(Phase::Replicate);
            self.show_players(tick, now);
        }
        for (&connection, peer) in &mut self.peers {
            if peer.ai.is_some() {
                // What a client would get arrives at once (ai.rs).
                if simulate {
                    let _ai = phase::enter(Phase::Ai);
                    peer.world.deliver_locally(tick, now);
                }
                continue;
            }
            let _replicate = phase::enter(Phase::Replicate);
            // Each tick sends every client a datagram (62.5 a second, as
            // retail's PC default); the forced last drain step only retires.
            if !force && peer.open {
                if simulate {
                    if let Some(view) = peer.world.take_spawn_view() {
                        peer.notice(view);
                    }
                    peer.flush_combat(&self.schemas, now);
                }
                let movements = match &self.room.world {
                    Some(world) if simulate => peer.world.movement(&world.scene, tick),
                    _ => Vec::new(),
                };
                peer.send_frame(
                    &self.schemas,
                    &mut self.links,
                    socket,
                    connection,
                    tick,
                    movements,
                    drain,
                    clock(),
                );
            }
        }
        // Games taken out before they connected get nothing but their 20304
        // (server.py `_tick`, `Client.send_frame`).
        for (&connection, peer) in &mut self.homeward {
            if !force && peer.open {
                peer.send_frame(
                    &self.schemas,
                    &mut self.links,
                    socket,
                    connection,
                    tick,
                    Vec::new(),
                    true,
                    clock(),
                );
            }
        }
    }

    /// Seats and games still connected or awaited keep the clock from
    /// draining its last step (server.py `_drain_clock`: `self.clients or
    /// self.handoffs`).
    fn connections_pending(&self) -> bool {
        !self.seats.is_empty() || !self.homeward.is_empty()
    }
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
            mail: Vec::new(),
            held_reach: HeldReach::default(),
            backlog: Backlog::default(),
            flow_at: Duration::ZERO,
            flow_tick: 0,
            mode_frame: None,
            next_mode_frame: Duration::ZERO,
            graph_frame: None,
            assembled: false,
            ended: false,
            started: Instant::now(),
            ai: None,
            dummies: Vec::new(),
            workshop: None,
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
                dummy: false,
            });
        }
    }

    /// Its last player left: the match is over for good. Its room only
    /// finishes the links of games taken out before they connected.
    pub(super) fn closed(&self) -> bool {
        self.remaining == 0
    }

    pub(super) fn assembling(&self) -> bool {
        self.flow.as_ref().is_some_and(MatchFlow::assembling)
    }

    /// Whether the mode's own graph drives the match (`MatchFlow::follow`).
    pub(super) fn follows_graph(&self) -> bool {
        self.flow.as_ref().is_some_and(MatchFlow::follows_graph)
    }

    /// The mode entity's first frame for a client: the mode graph's newest
    /// while it drives the mode entity, else the scene's hand-built one.
    pub(super) fn first_mode_frame<'a>(&'a self, scene: &'a PreparedScene) -> ModeFrame<'a> {
        self.graph_mode_frame()
            .unwrap_or(ModeFrame::Full(scene.mode_ui()))
    }

    /// The mode entity's frame every client gets this tick: the hand-built
    /// one when due, else the mode graph's while it drives the mode entity.
    pub(super) fn mode_frame_now(&self) -> Option<ModeFrame<'_>> {
        match &self.mode_frame {
            Some(frame) => Some(ModeFrame::Full(frame)),
            None => self.graph_mode_frame(),
        }
    }

    fn graph_mode_frame(&self) -> Option<ModeFrame<'_>> {
        Some(ModeFrame::Graph {
            encoder: self.mode.as_ref()?.encoder()?,
            projected: self.graph_frame.as_ref()?,
        })
    }

    /// The mode entity's graph starts with the room's first tick, before
    /// its placed scripts' first, as the map's entities start with the mode
    /// entity in retail: the placed scripts find its held `0724` as they
    /// start, and the mode graph takes the objectives' `0813` (`0F4C`, sent
    /// once as they start). A Workshop match runs its actions on the mode
    /// entity, so a graph must drive it (`ModeRunner::workshop`). The mode
    /// graph removes placeables as it starts (`0CBB` st34, the health
    /// packs), before any world reaches a client.
    fn begin_mode(&mut self, tick: u32) {
        let Some(world) = &mut self.world else {
            return;
        };
        if self.mode.is_some() {
            return;
        }
        let mode = if self.workshop.is_some() {
            ModeRunner::workshop(&world.scene, tick, Vec::new())
        } else {
            Some(ModeRunner::start(&world.scene, tick, Vec::new()))
        };
        if let Some(mode) = mode {
            take_placeable_removals(self.mode.insert(mode), &mut world.scene);
        }
    }

    /// The players are in a PvP world: the match starts. Where the mode's
    /// own graph drives the match (`ModeRunner::holds_rooms`) the server
    /// starts it (`ModeRunner::start_match`) and the flow follows the
    /// graph; else the flow starts with "assemble your team".
    pub(super) fn begin_flow(
        &mut self,
        scene: &mut PreparedScene,
        timers: Timers,
        now: Duration,
        tick: u32,
    ) {
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
                mode_guid: lucy_sim::match_flow::SKIRMISH,
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
        if self.mode.is_none() {
            let mode = self.mode.insert(ModeRunner::start(scene, tick, Vec::new()));
            // The mode graph removes placeables as it starts (`0CBB` st34,
            // the health packs): before the world that just started
            // reaches the client, so their creates never go.
            take_placeable_removals(mode, scene);
        }
        match self.mode.as_mut() {
            Some(mode) if mode.holds_rooms() => mode.start_match(),
            // Nothing stands in for a mode its graph drives: the match
            // waits, and says why.
            mode if flow.mode().graph_driven() => tracing::error!(
                "[game] {}: {:?} plays only on its mode graph, which doesn't run here: {:?}",
                self.map.name,
                flow.mode(),
                mode.and_then(|mode| mode.gaps())
            ),
            _ => flow.start(),
        }
        tracing::info!(
            "[game] {}: {:?} match flow, assemble your team",
            self.map.name,
            flow.mode()
        );
        self.flow = Some(flow);
        // The players reach the graph with the next tick's flow
        // (`update_flows`), as they join. A Workshop match's graph started
        // with the room (`Self::begin_mode`); its rules start now, against
        // the players in the world.
        if let (Some(workshop), true) = (
            self.workshop.as_mut(),
            self.mode.as_ref().is_some_and(ModeRunner::graph_driven),
        ) {
            workshop.run = Some(WorkshopRun::new(
                &workshop.logic,
                Arc::clone(&workshop.catalog),
                u64::from(tick),
            ));
        }
        self.flow_at = now;
        self.flow_tick = tick;
        self.next_mode_frame = now;
    }

    pub(super) fn appearance(&self, slot: usize) -> Option<PlayerAppearance<'_>> {
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

    pub(super) fn message(&self, slot: usize, initial: bool) -> Message {
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
