//! Tests: the game service on a virtual clock, with an in-memory network in
//! place of its UDP socket. The loop is the real one ([`GameLoop::pass`]);
//! only its clock and its socket are the test's.
//!
//! The clock moves in lockstep with the test's clients: they send what they
//! send for the next tick, the test steps the clock, and the loop takes the
//! requests, datagrams and input that came since the last tick (at the last
//! tick's time, as a server that keeps up with its clients does between two
//! ticks), then runs the next tick exactly when it is due, so no tick is ever
//! skipped; the clients then read what it sent. Between steps the loop
//! answers the service's requests (`snapshot` and the like) at the last
//! tick's time. Nothing in a test's run depends on the PC's speed.
//!
//! A match's scene is still prepared on a thread of its own, in real time
//! ([`GameService::open_world`]): [`Lockstep::await_scenes`] holds the clock
//! until every match has its scene, so it comes at a known tick.
//!
//! A test's game is a [`Client`] on the network: waiting for its next
//! datagram runs the service's ticks. A lobby test gives the lobby the
//! service and keeps the network ([`Lockstep::into_parts`]).

use std::{
    collections::{HashMap, VecDeque},
    io,
    net::{Ipv4Addr, SocketAddr},
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use mio::{Events, Poll, Waker};
use orpheus_core::tick::TICK_PERIOD;
use orpheus_lobby::handoff::Handoff;
use orpheus_wire::{
    jam::codec::Schemas,
    link::{ACK, KIND_CONTROL, KIND_FRAME, LinkCipher, Packet, ReceiveWindow, SYN},
};

use super::{
    GameLoop, GameService, RECEIVE_BATCH, REQUESTS, Request, State, Transport, check_schemas,
    stopped,
};

/// The service's address on the in-memory network.
const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 3_074);

/// A game service on a virtual clock (the module notes). It is the
/// [`GameService`] it derefs to, plus the clock and the network.
pub(crate) struct Lockstep {
    service: GameService,
    network: Network,
}

impl Lockstep {
    pub(crate) fn new(schemas: Arc<Schemas>) -> io::Result<Self> {
        check_schemas(&schemas)?;
        let poll = Poll::new()?;
        let wake = Arc::new(Waker::new(poll.registry(), REQUESTS)?);
        let (requests, receiver) = mpsc::sync_channel(RECEIVE_BATCH);
        let (controls, steps) = mpsc::channel();
        let wire = Wire::default();
        // The first tick runs at once, as the real loop's first pass does,
        // before any request.
        let mut game = GameLoop::new(schemas);
        settle(&mut game, &wire, &receiver, Duration::ZERO)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let worker = thread::Builder::new().name("orpheus-game".into()).spawn({
            let (wire, stopping) = (wire.clone(), Arc::clone(&stopping));
            move || run(game, &wire, poll, &receiver, &steps, &stopping)
        })?;
        Ok(Self {
            service: GameService {
                address: SERVER,
                requests,
                wake: Arc::clone(&wake),
                stopping,
                worker: Some(worker),
            },
            network: Network {
                wire,
                controls,
                wake,
                now: Arc::new(Mutex::new(Duration::ZERO)),
            },
        })
    }

    pub(crate) fn network(&self) -> &Network {
        &self.network
    }

    /// The service alone, for a lobby that holds it, and the network that
    /// runs its clock.
    pub(crate) fn into_parts(self) -> (GameService, Network) {
        (self.service, self.network)
    }

    /// Runs the next tick.
    pub(crate) fn step(&self) {
        self.network.step();
    }

    /// Runs the ticks of `time`.
    pub(crate) fn run(&self, time: Duration) {
        self.network.run(time);
    }

    /// The time of the last tick since the service started.
    pub(crate) fn now(&self) -> Duration {
        self.network.now()
    }

    /// Holds the clock until every match has the scene its thread prepares
    /// (or knows that it failed).
    pub(crate) fn await_scenes(&self) {
        self.network.await_scenes();
    }
}

impl Deref for Lockstep {
    type Target = GameService;

    fn deref(&self) -> &GameService {
        &self.service
    }
}

impl DerefMut for Lockstep {
    fn deref_mut(&mut self) -> &mut GameService {
        &mut self.service
    }
}

/// The test's end of the in-memory network: its clients' datagrams, and the
/// clock's controls.
#[derive(Clone)]
pub(crate) struct Network {
    wire: Wire,
    controls: mpsc::Sender<Control>,
    wake: Arc<Waker>,
    now: Arc<Mutex<Duration>>,
}

impl Network {
    /// A new client's address.
    pub(crate) fn bind(&self) -> SocketAddr {
        let mut datagrams = self.wire.lock();
        datagrams.next_port += 1;
        let address = SocketAddr::new(SERVER.ip(), 50_000 + datagrams.next_port);
        datagrams.clients.insert(address, VecDeque::new());
        address
    }

    /// A datagram from the client at `from` to the service; it arrives
    /// before the next tick.
    pub(crate) fn send(&self, from: SocketAddr, datagram: Vec<u8>) {
        self.wire.lock().server.push_back((from, datagram));
    }

    /// The next datagram the service sent to the client at `to`, if any.
    pub(crate) fn take(&self, to: SocketAddr) -> Option<Vec<u8>> {
        self.wire.lock().clients.get_mut(&to)?.pop_front()
    }

    /// Runs the next tick.
    pub(crate) fn step(&self) {
        let now = self.control(|reply| Control::Step { reply });
        *self.now.lock().expect("clock") = now;
    }

    /// Runs the ticks of `time`.
    pub(crate) fn run(&self, time: Duration) {
        for _ in 0..ticks(time) {
            self.step();
        }
    }

    /// The time of the last tick since the service started.
    pub(crate) fn now(&self) -> Duration {
        *self.now.lock().expect("clock")
    }

    /// Holds the clock until every match has the scene its thread prepares
    /// (or knows that it failed).
    pub(crate) fn await_scenes(&self) {
        self.control(|reply| Control::AwaitScenes { reply });
    }

    fn control<T>(&self, control: impl FnOnce(mpsc::SyncSender<T>) -> Control) -> T {
        let (reply, receive) = mpsc::sync_channel(1);
        self.controls
            .send(control(reply))
            .expect("the game worker runs");
        self.wake.wake().expect("the game worker wakes");
        receive.recv().expect("the game worker answers")
    }
}

/// The ticks that `time` takes, the last one partly.
pub(crate) fn ticks(time: Duration) -> u32 {
    u32::try_from(time.as_nanos().div_ceil(TICK_PERIOD.as_nanos())).expect("ticks")
}

/// How long a client waits for its next datagram: the 2 s read timeout its
/// socket had over UDP, in ticks.
pub(crate) const RECEIVE_TICKS: u32 = 125;

/// A game on the service's in-memory network. Waiting for a datagram runs
/// the service's ticks.
pub(crate) struct Client {
    network: Network,
    address: SocketAddr,
    cipher: LinkCipher,
    connection: u32,
    next: u32,
    pub(crate) window: ReceiveWindow,
}

impl Client {
    pub(crate) fn new(server: &Lockstep, ticket: &Handoff) -> Self {
        Self::on(server.network(), ticket)
    }

    /// A game on `network` with the lobby's `ticket`.
    pub(crate) fn on(network: &Network, ticket: &Handoff) -> Self {
        let network = network.clone();
        let address = network.bind();
        Self {
            network,
            address,
            cipher: LinkCipher::new(ticket.key_out, ticket.key_in),
            connection: ticket.conn,
            next: 0,
            window: ReceiveWindow::default(),
        }
    }

    /// A datagram that arrives before the service's next tick.
    pub(crate) fn send(&mut self, flags: u8, payload: Vec<u8>) {
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
        self.network
            .send(self.address, self.cipher.seal(&packet).unwrap());
    }

    /// The next datagram, running ticks until one comes (at most
    /// [`RECEIVE_TICKS`]).
    pub(crate) fn receive(&self) -> Packet {
        for _ in 0..=RECEIVE_TICKS {
            if let Some(bytes) = self.network.take(self.address) {
                let packet = self.cipher.open(&bytes).unwrap();
                assert_eq!(packet.conn, self.connection);
                return packet;
            }
            self.network.step();
        }
        panic!("no datagram in {RECEIVE_TICKS} ticks");
    }

    /// Nothing comes for the ticks of `time`.
    pub(crate) fn hears_nothing(&self, time: Duration) {
        self.network.run(time);
        assert!(self.network.take(self.address).is_none());
    }

    /// The time of the service's last tick.
    pub(crate) fn now(&self) -> Duration {
        self.network.now()
    }

    pub(crate) fn handshake(&mut self) {
        self.send(SYN, Vec::new());
        let response = self.receive();
        assert_eq!((response.kind, response.flags), (KIND_CONTROL, SYN | ACK));
        assert!(self.window.accept(response.seq));
        self.send(ACK, Vec::new());
    }
}

enum Control {
    Step { reply: mpsc::SyncSender<Duration> },
    AwaitScenes { reply: mpsc::SyncSender<()> },
}

/// The datagrams in flight, each delivered in order.
#[derive(Clone, Default)]
struct Wire(Arc<Mutex<Datagrams>>);

#[derive(Default)]
struct Datagrams {
    /// To the service, with their sender.
    server: VecDeque<(SocketAddr, Vec<u8>)>,
    /// To each client.
    clients: HashMap<SocketAddr, VecDeque<Vec<u8>>>,
    next_port: u16,
}

impl Wire {
    fn lock(&self) -> std::sync::MutexGuard<'_, Datagrams> {
        self.0.lock().expect("network")
    }
}

impl Transport for Wire {
    /// A datagram to an address no client has is lost, as over UDP.
    fn send_to(&self, datagram: &[u8], address: SocketAddr) -> io::Result<usize> {
        if let Some(inbox) = self.lock().clients.get_mut(&address) {
            inbox.push_back(datagram.to_vec());
        }
        Ok(datagram.len())
    }

    fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let (from, datagram) = self
            .lock()
            .server
            .pop_front()
            .ok_or(io::ErrorKind::WouldBlock)?;
        let count = datagram.len().min(buffer.len());
        buffer[..count].copy_from_slice(&datagram[..count]);
        Ok((count, from))
    }
}

impl State {
    /// A match waits for the scene its thread prepares.
    fn awaits_scene(&self) -> bool {
        self.rooms
            .values()
            .any(|room| room.world.is_none() && !room.ended)
    }
}

/// The game thread on the virtual clock: `run` in `game_service.rs` with
/// the test's clock and network.
fn run(
    mut game: GameLoop,
    wire: &Wire,
    mut poll: Poll,
    requests: &mpsc::Receiver<Request>,
    steps: &mpsc::Receiver<Control>,
    stopping: &AtomicBool,
) -> io::Result<()> {
    // The last tick's time: the first ran before the thread started.
    let mut now = Duration::ZERO;
    let mut events = Events::with_capacity(2);
    while !stopping.load(Ordering::Acquire) {
        // Between steps the loop answers requests; the datagrams wait for
        // the next step, so they all arrive at a known time.
        while game.pass(wire, requests, false, || now)?.busy() {}
        match steps.try_recv() {
            Ok(Control::Step { reply }) => {
                // What the clients sent since the last tick arrives before
                // the next.
                settle(&mut game, wire, requests, now)?;
                now = game.due;
                settle(&mut game, wire, requests, now)?;
                let _ = reply.send(now);
            }
            Ok(Control::AwaitScenes { reply }) => {
                while game.state.awaits_scene() {
                    let request = requests.recv().map_err(|_| stopped())?;
                    game.state.handle_request(request, now);
                }
                let _ = reply.send(());
            }
            Err(mpsc::TryRecvError::Empty) => match poll.poll(&mut events, None) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            },
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
    Ok(())
}

/// Passes of the loop at `now` until nothing waits.
fn settle(
    game: &mut GameLoop,
    wire: &Wire,
    requests: &mpsc::Receiver<Request>,
    now: Duration,
) -> io::Result<()> {
    while game.pass(wire, requests, true, || now)?.busy() {}
    Ok(())
}
