//! The game service's router and its threads.
//!
//! One thread (`lucy-game`, [`run`]) owns the UDP socket and the requests
//! of the lobby, the dashboard and the scenes' threads. It routes each
//! datagram by the connection id in its header to the room that has that
//! connection, and drops at once a datagram that isn't a game's or whose
//! connection no room has (its match closed, or it never had one). It
//! answers the requests that are about no single room (opening a match,
//! the results, the live tools' settings) and passes the others to their
//! room. Each room runs on a thread of its own (`lucy-room-XXXX`,
//! [`run_room`]) with its own loop and clock (`room.rs`), and sends through
//! a clone of the socket. Rooms share only what never changes: the schemas
//! and the prepared scenes' assets (`Arc`).
//!
//! A room tells the router what it must know through [`RoomEvent`]s: a link
//! closed, a match ended by its rules, the room's thread ended. A room
//! whose match panics sends its players home itself (`RoomLoop::fail`); a
//! room whose thread ends anyway loses its routes when the router hears of
//! it, so its seats are gone for the lobby and its games' datagrams are
//! dropped.
//!
//! The tests run the router and the rooms on one thread on a virtual clock
//! (`lockstep.rs`): the same [`Router`] and [`RoomLoop`], without threads.

use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use lucy_sim::{
    bot_plate::{BotPlate, PlateChange},
    hurt_feedback::HurtFeedback,
    match_flow::Timers,
};
use lucy_wire::{
    jam::codec::Schemas,
    link::{MAX_DATAGRAM, peek_connection},
};
use mio::{Events as Readiness, Poll, Waker, net::UdpSocket};

use super::{
    GAME_SOCKET, MatchId, MatchResult, OpenRequest, PlateAnswers, RECEIVE_BATCH, Request,
    RoomPlate, RoomRequest, RoomSettings, RoomSnapshot, RoomState, Target, WorldPlayer,
    room::RoomLoop,
};
use crate::connection::MAX_CONNECTION_ID;
use lucy_lobby::handoff::Handoff;

/// A game's datagrams wait for its room at most this many at a time; the
/// router drops the ones over it. A game sends one a tick and a room reads
/// what came every tick, so this is a second of a game's datagrams: input
/// a room that fell so far behind would run late anyway.
const PEER_DATAGRAMS: usize = 64;

/// What a room reads: its games' datagrams, the requests for it, and what
/// the router asks of every room.
pub(super) enum Inbound {
    Datagram(Datagram),
    Request(RoomRequest),
    /// Its line of the snapshot ([`super::GameService::snapshot`]).
    Snapshot(mpsc::Sender<RoomSnapshot>),
    /// The live nameplate tool's plate, the messages it has for the live
    /// bots, and where the room's part of the answer goes
    /// ([`super::GameService::bot_plate`]).
    BotPlate {
        plate: Arc<BotPlate>,
        messages: Vec<u64>,
        reply: mpsc::Sender<RoomPlate>,
    },
    /// The live hurt feedback switch now in force.
    Hurt(HurtFeedback),
}

impl Inbound {
    /// Nothing will read it: a request gets the answer of a room that has
    /// no such match or seat, and a broadcast's answer goes without this
    /// room's part.
    pub(super) fn refuse(self) {
        if let Self::Request(request) = self {
            request.refuse();
        }
    }
}

/// A datagram for one of a room's connections. It counts against its
/// game's [`PEER_DATAGRAMS`] until the room takes it or drops it.
pub(super) struct Datagram {
    pub(super) bytes: Vec<u8>,
    pub(super) from: SocketAddr,
    queued: Arc<AtomicUsize>,
}

impl Drop for Datagram {
    fn drop(&mut self) {
        self.queued.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What a room tells the router.
enum RoomEvent {
    /// A connection's link is gone (its game closed it, or it timed out):
    /// its datagrams are dropped from now.
    Closed { id: MatchId, connection: u32 },
    /// A match ended by its rules, for the lobby's profiles.
    Result(MatchResult),
    /// The room's thread ended: so did every connection it had.
    Ended(MatchId),
}

/// A room's way to the router. The router wakes for each event; the tests'
/// single thread (`lockstep.rs`) needs no wake-up, as it takes the events
/// after each pass.
#[derive(Clone)]
pub(super) struct Events {
    id: MatchId,
    sender: mpsc::Sender<RoomEvent>,
    wake: Option<Arc<Waker>>,
}

impl Events {
    /// The link of `connection` is gone.
    pub(super) fn closed(&self, connection: u32) {
        self.send(RoomEvent::Closed {
            id: self.id,
            connection,
        });
    }

    /// The match ended by its rules.
    pub(super) fn result(&self, result: MatchResult) {
        self.send(RoomEvent::Result(result));
    }

    fn send(&self, event: RoomEvent) {
        if self.sender.send(event).is_ok()
            && let Some(wake) = &self.wake
        {
            let _ = wake.wake();
        }
    }

    /// Tests: the events of a room no router hears.
    #[cfg(test)]
    pub(super) fn nowhere(id: MatchId) -> Self {
        let (sender, _) = mpsc::channel();
        Self {
            id,
            sender,
            wake: None,
        }
    }
}

/// Tells the router that its room's thread ended, however it ends.
struct Ended(Events);

impl Drop for Ended {
    fn drop(&mut self) {
        self.0.send(RoomEvent::Ended(self.0.id));
    }
}

/// The room of a connection, and how many of its game's datagrams wait
/// there.
struct Route {
    id: MatchId,
    queued: Arc<AtomicUsize>,
}

/// A room the router made, for its runner to run: its loop and what it
/// reads.
pub(super) struct NewRoom {
    pub(super) id: MatchId,
    pub(super) game: RoomLoop,
    pub(super) inbox: mpsc::Receiver<Inbound>,
}

pub(super) struct Router {
    schemas: Arc<Schemas>,
    wake: Option<Arc<Waker>>,
    events: mpsc::Sender<RoomEvent>,
    heard: mpsc::Receiver<RoomEvent>,
    /// Each room's inbox, by match.
    rooms: HashMap<MatchId, mpsc::Sender<Inbound>>,
    /// The room of each connection a room has reserved or open.
    routes: HashMap<u32, Route>,
    next_connection: u32,
    /// Matches that ended by their rules, until the lobby takes them.
    results: Vec<MatchResult>,
    /// The match flow's timers for the rooms that open (tests shorten
    /// them).
    timers: Timers,
    /// The live nameplate tool's changes, which every room takes.
    bot_plate: Arc<BotPlate>,
    /// The live hurt feedback switch, which every room takes.
    hurt: HurtFeedback,
}

impl Router {
    /// A router whose rooms wake it with `wake` (none in the tests).
    pub(super) fn new(schemas: Arc<Schemas>, wake: Option<Arc<Waker>>) -> Self {
        let (events, heard) = mpsc::channel();
        Self {
            schemas,
            wake,
            events,
            heard,
            rooms: HashMap::new(),
            routes: HashMap::new(),
            next_connection: 1,
            results: Vec::new(),
            timers: Timers::default(),
            bot_plate: Arc::default(),
            hurt: HurtFeedback::default(),
        }
    }

    /// A request, at `now` since the service started: answered here, or
    /// passed on to its room. A match that opens comes back as a room for
    /// the runner to start.
    pub(super) fn handle(&mut self, request: Request, now: Duration) -> Option<NewRoom> {
        match request {
            Request::Open(request) => return self.open(*request, now),
            Request::Join { id, players, reply } => self.join(id, players, reply),
            Request::Snapshot { reply } => {
                let (answers, rooms) = mpsc::channel();
                self.broadcast(|| Inbound::Snapshot(answers.clone()));
                let _ = reply.send(rooms);
            }
            Request::Results { reply } => {
                let _ = reply.send(std::mem::take(&mut self.results));
            }
            Request::BotPlate { change, reply } => {
                let _ = reply.send(self.tune_bot_plate(change));
            }
            Request::Hurt { change, reply } => {
                let mut hurt = self.hurt;
                let changed = hurt.change(&change).map(|()| {
                    self.hurt = hurt;
                    self.broadcast(|| Inbound::Hurt(hurt));
                    hurt
                });
                let _ = reply.send(changed);
            }
            #[cfg(test)]
            Request::Timers { timers, reply } => {
                self.timers = timers;
                let _ = reply.send(());
            }
            Request::Room(request) => self.pass_on(request),
        }
        None
    }

    /// A datagram from `from` goes to the room that has its connection. It
    /// is dropped here when it can't be a game's, when no room has its
    /// connection (its match closed), and while its game has
    /// [`PEER_DATAGRAMS`] waiting.
    pub(super) fn route(&self, datagram: &[u8], from: SocketAddr) {
        if datagram.len() > MAX_DATAGRAM {
            return;
        }
        let Some(route) = peek_connection(datagram).and_then(|id| self.routes.get(&id)) else {
            return;
        };
        let Some(room) = self.rooms.get(&route.id) else {
            return;
        };
        // Only the router adds; the room's takes can only make room.
        if route.queued.load(Ordering::Relaxed) >= PEER_DATAGRAMS {
            return;
        }
        route.queued.fetch_add(1, Ordering::Relaxed);
        let _ = room.send(Inbound::Datagram(Datagram {
            bytes: datagram.to_vec(),
            from,
            queued: Arc::clone(&route.queued),
        }));
    }

    /// What the rooms told since the last call; the rooms whose threads
    /// ended.
    pub(super) fn take_events(&mut self) -> Vec<MatchId> {
        let mut ended = Vec::new();
        while let Ok(event) = self.heard.try_recv() {
            match event {
                RoomEvent::Closed { id, connection } => {
                    if self
                        .routes
                        .get(&connection)
                        .is_some_and(|route| route.id == id)
                    {
                        self.routes.remove(&connection);
                    }
                }
                RoomEvent::Result(result) => self.results.push(result),
                RoomEvent::Ended(id) => {
                    self.end(id);
                    ended.push(id);
                }
            }
        }
        ended
    }

    /// The room of match `id` has ended: its connections go, and the
    /// requests for it get their answers here.
    pub(super) fn end(&mut self, id: MatchId) {
        self.rooms.remove(&id);
        self.routes.retain(|_, route| route.id != id);
    }

    /// The service stops: every room's inbox closes, and with it the room.
    fn close(&mut self) {
        self.rooms.clear();
        self.routes.clear();
    }

    /// A room for a new match, whose seats get the next free connections.
    fn open(&mut self, request: OpenRequest, now: Duration) -> Option<NewRoom> {
        let OpenRequest { opening, reply } = request;
        let id = opening.id;
        if self.rooms.contains_key(&id) {
            let _ = reply.send(Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "match already exists",
            )));
            return None;
        }
        let settings = RoomSettings {
            schemas: Arc::clone(&self.schemas),
            timers: self.timers,
            bot_plate: Arc::clone(&self.bot_plate),
            hurt: self.hurt,
            events: Events {
                id,
                sender: self.events.clone(),
                wake: self.wake.clone(),
            },
        };
        let opened = RoomState::open(opening, settings, now, |count| self.allocate(count));
        let (state, tickets) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                let _ = reply.send(Err(error));
                return None;
            }
        };
        let connections: Vec<u32> = tickets.iter().map(|ticket| ticket.conn).collect();
        self.add_routes(id, &connections);
        let (inbox, read) = mpsc::channel();
        self.rooms.insert(id, inbox);
        Some(NewRoom {
            id,
            game: RoomLoop::new(state, now, reply, tickets),
            inbox: read,
        })
    }

    /// More players for match `id`: the room seats them on the next free
    /// connections, and gives back the ones it doesn't take.
    fn join(
        &mut self,
        id: MatchId,
        players: Vec<WorldPlayer>,
        reply: mpsc::SyncSender<io::Result<Option<Vec<Handoff>>>>,
    ) {
        if !self.rooms.contains_key(&id) {
            let _ = reply.send(Ok(None));
            return;
        }
        let connections = match self.allocate(players.len()) {
            Ok(connections) => connections,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        self.add_routes(id, &connections);
        let request = RoomRequest::Join {
            id,
            players,
            connections,
            reply,
        };
        if let Err(mpsc::SendError(Inbound::Request(request))) =
            self.rooms[&id].send(Inbound::Request(request))
        {
            if let RoomRequest::Join { connections, .. } = &request {
                for connection in connections {
                    self.routes.remove(connection);
                }
            }
            request.refuse();
        }
    }

    /// A request for one room, to the room of its match or seat; answered
    /// here as by a room without the match or seat when there is none.
    fn pass_on(&self, request: RoomRequest) {
        let id = match request.target() {
            Target::Match(id) => Some(id),
            Target::Connection(connection) => self.routes.get(&connection).map(|route| route.id),
        };
        let Some(room) = id.and_then(|id| self.rooms.get(&id)) else {
            request.refuse();
            return;
        };
        if let Err(mpsc::SendError(inbound)) = room.send(Inbound::Request(request)) {
            inbound.refuse();
        }
    }

    /// The live nameplate tool's change, if any: the plate every room takes
    /// now, and the messages it has for their live bots. Each room's part
    /// of the answer comes on the receiver; a change the tool refuses comes
    /// back as its reason.
    fn tune_bot_plate(&mut self, change: Option<PlateChange>) -> Result<PlateAnswers, String> {
        let mut messages = Vec::new();
        if let Some(change) = change {
            messages = change.messages();
            let mut plate = (*self.bot_plate).clone();
            plate.change(change)?;
            if plate != *self.bot_plate {
                self.bot_plate = Arc::new(plate);
            }
        }
        let (answers, rooms) = mpsc::channel();
        self.broadcast(|| Inbound::BotPlate {
            plate: Arc::clone(&self.bot_plate),
            messages: messages.clone(),
            reply: answers.clone(),
        });
        Ok(PlateAnswers {
            commands: self.bot_plate.commands(),
            messages,
            rooms,
        })
    }

    fn broadcast(&self, inbound: impl Fn() -> Inbound) {
        for room in self.rooms.values() {
            let _ = room.send(inbound());
        }
    }

    /// Connection ids for `count` new seats: the next ones in turn, 1 to
    /// [`MAX_CONNECTION_ID`], none still in use.
    fn allocate(&mut self, count: usize) -> io::Result<Vec<u32>> {
        let mut connections = Vec::with_capacity(count);
        for _ in 0..count {
            let connection = self.next_connection;
            self.next_connection = self.next_connection % MAX_CONNECTION_ID + 1;
            if self.routes.contains_key(&connection) {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "game seat ID is still in use",
                ));
            }
            connections.push(connection);
        }
        Ok(connections)
    }

    fn add_routes(&mut self, id: MatchId, connections: &[u32]) {
        for &connection in connections {
            self.routes.insert(
                connection,
                Route {
                    id,
                    queued: Arc::default(),
                },
            );
        }
    }

    /// The guard a room's thread holds: it tells the router when the
    /// thread ends.
    fn ended(&self, id: MatchId) -> Ended {
        Ended(Events {
            id,
            sender: self.events.clone(),
            wake: self.wake.clone(),
        })
    }
}

/// The router's thread: the requests, the rooms' events and the socket's
/// datagrams, until the service stops; then every room's thread stops too.
pub(super) fn run(
    mut router: Router,
    socket: UdpSocket,
    rooms_socket: Arc<std::net::UdpSocket>,
    mut poll: Poll,
    requests: mpsc::Receiver<Request>,
    stopping: Arc<AtomicBool>,
) -> io::Result<()> {
    // The service's clock: every room's ticks fall on its grid
    // (`GameClock::opened`).
    let start = Instant::now();
    let mut threads = HashMap::new();
    let routed = route_until_stopped(
        &mut router,
        &socket,
        &rooms_socket,
        &mut poll,
        &requests,
        &stopping,
        start,
        &mut threads,
    );
    router.close();
    for (_, thread) in threads {
        let _ = thread.join();
    }
    routed
}

#[allow(clippy::too_many_arguments)]
fn route_until_stopped(
    router: &mut Router,
    socket: &UdpSocket,
    rooms_socket: &Arc<std::net::UdpSocket>,
    poll: &mut Poll,
    requests: &mpsc::Receiver<Request>,
    stopping: &AtomicBool,
    start: Instant,
    threads: &mut HashMap<MatchId, JoinHandle<()>>,
) -> io::Result<()> {
    // An oversized datagram must never be accepted as a truncated valid one.
    let mut buffer = vec![0; 65_536];
    let mut readiness = Readiness::with_capacity(2);
    let mut socket_ready = false;
    while !stopping.load(Ordering::Acquire) {
        let now = start.elapsed();
        let mut handled = 0;
        for request in requests.try_iter().take(RECEIVE_BATCH) {
            if let Some(room) = router.handle(request, now) {
                start_room(room, router, rooms_socket, start, threads);
            }
            handled += 1;
        }
        for id in router.take_events() {
            if let Some(thread) = threads.remove(&id) {
                let _ = thread.join();
            }
        }
        if socket_ready {
            for _ in 0..RECEIVE_BATCH {
                match socket.recv_from(&mut buffer) {
                    Ok((count, from)) => router.route(&buffer[..count], from),
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
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let busy = handled == RECEIVE_BATCH || socket_ready;
        match poll.poll(&mut readiness, busy.then_some(Duration::ZERO)) {
            Ok(()) => {
                socket_ready |= readiness.iter().any(|event| event.token() == GAME_SOCKET);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// A new room's thread. When it can't start, the room ends at once: its
/// opener gets no seats.
fn start_room(
    room: NewRoom,
    router: &mut Router,
    socket: &Arc<std::net::UdpSocket>,
    start: Instant,
    threads: &mut HashMap<MatchId, JoinHandle<()>>,
) {
    let NewRoom { id, game, inbox } = room;
    let ended = router.ended(id);
    let socket = Arc::clone(socket);
    let spawned = thread::Builder::new()
        .name(format!("lucy-room-{:04X}", id[0] & 0xFFFF))
        .spawn(move || run_room(game, &socket, inbox, start, ended));
    match spawned {
        Ok(thread) => {
            threads.insert(id, thread);
        }
        Err(error) => {
            tracing::error!("[game] match {:04X} has no thread: {error}", id[0] & 0xFFFF);
            router.end(id);
        }
    }
}

/// A room's thread: its loop on the service's clock, woken by what comes in
/// (so a datagram is taken as it arrives) or by its next tick. It ends once
/// its room has finished (its last game left, or its match failed twice),
/// or when the service stops.
fn run_room(
    mut game: RoomLoop,
    socket: &std::net::UdpSocket,
    inbox: mpsc::Receiver<Inbound>,
    start: Instant,
    ended: Ended,
) {
    let mut next = None;
    loop {
        let inbound = next.take().into_iter().chain(inbox.try_iter());
        let pass = game.pass(socket, inbound, || start.elapsed());
        if game.finished() {
            break;
        }
        let due = if pass.busy() {
            Duration::ZERO
        } else {
            game.due()
        };
        match wait_until(&inbox, start, due) {
            Woken::Inbound(inbound) => next = Some(inbound),
            Woken::Due => {}
            Woken::Closed => return,
        }
    }
    // Until the router lets the room go, what still comes is refused.
    drop(ended);
    for inbound in inbox {
        inbound.refuse();
    }
}

/// What ended a room's wait ([`wait_until`]).
enum Woken {
    Inbound(Inbound),
    Due,
    /// The router let the room go: the service stops.
    Closed,
}

/// A room's wait for what comes in, until `due` on the service's clock
/// (`start`). Every room's wait for its next tick goes through here, so a
/// precise timer belongs here: on Windows a plain wait rounds up to the
/// system timer's 15.6 ms (`refs/reports/Latency.md`).
fn wait_until(inbox: &mpsc::Receiver<Inbound>, start: Instant, due: Duration) -> Woken {
    match inbox.recv_timeout(due.saturating_sub(start.elapsed())) {
        Ok(inbound) => Woken::Inbound(inbound),
        Err(mpsc::RecvTimeoutError::Timeout) => Woken::Due,
        Err(mpsc::RecvTimeoutError::Disconnected) => Woken::Closed,
    }
}
