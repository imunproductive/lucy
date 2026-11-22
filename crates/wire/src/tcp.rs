//! Shared, cancellable TCP peers for the local Battle.net, lobby and
//! dashboard services: a stream with deadlines on the service's clock, and
//! the workers that serve each accepted connection on its own thread within
//! a listener's limits.

use std::{
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// How long one read or write blocks before the stream checks its stop flag
/// and deadline again.
const IO_POLL: Duration = Duration::from_millis(250);
/// How long a write without a deadline may make no progress.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// What deadlines read: the time since some start, the service's clock (a
/// test's may be virtual).
pub type Now = Arc<dyn Fn() -> Duration + Send + Sync>;

/// The wall clock, from now.
pub fn wall_clock() -> Now {
    let start = Instant::now();
    Arc::new(move || start.elapsed())
}

pub struct PeerStream {
    socket: TcpStream,
    stopping: Arc<AtomicBool>,
    clock: Now,
    /// When reads and writes start failing with `TimedOut`, on `clock`.
    deadline: Option<Duration>,
}

impl PeerStream {
    /// A stream on the wall clock.
    pub fn new(socket: TcpStream, stopping: Arc<AtomicBool>) -> io::Result<Self> {
        socket.set_nonblocking(false)?;
        socket.set_read_timeout(Some(IO_POLL))?;
        socket.set_write_timeout(Some(IO_POLL))?;
        Ok(Self {
            socket,
            stopping,
            clock: wall_clock(),
            deadline: None,
        })
    }

    /// Read deadlines from `now` instead of the wall clock. Clears the
    /// deadline.
    pub fn set_clock(&mut self, now: Now) {
        self.clock = now;
        self.deadline = None;
    }

    /// Reads and writes fail with `TimedOut` once `limit` has passed on the
    /// stream's clock, and until then retry instead of returning
    /// `WouldBlock`. `None`: a read returns `WouldBlock` or `TimedOut` after
    /// each poll, and a write fails after [`WRITE_TIMEOUT`] of real time
    /// without progress.
    pub fn set_time_limit(&mut self, limit: Option<Duration>) {
        self.deadline = limit.map(|limit| (self.clock)() + limit);
    }

    pub fn deadline_expired(&self) -> bool {
        self.deadline.is_some_and(|end| (self.clock)() >= end)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.socket.peer_addr()
    }

    pub fn peek(&self, data: &mut [u8]) -> io::Result<usize> {
        self.transfer(None, |socket| socket.peek(data))
    }

    /// Run `operation` until it does something, retrying a poll that timed
    /// out while a deadline (the stream's, or the real-time `until`) has not
    /// passed.
    fn transfer<T>(
        &self,
        until: Option<Instant>,
        mut operation: impl FnMut(&TcpStream) -> io::Result<T>,
    ) -> io::Result<T> {
        loop {
            if self.stopping.load(Ordering::Acquire) {
                return Err(io::ErrorKind::ConnectionAborted.into());
            }
            if self.deadline_expired() || until.is_some_and(|end| Instant::now() >= end) {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match operation(&self.socket) {
                Err(error) if (self.deadline.is_some() || until.is_some()) && retryable(&error) => {
                    continue;
                }
                result => return result,
            }
        }
    }
}

impl Read for PeerStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.transfer(None, |mut socket| socket.read(buffer))
    }
}

impl Write for PeerStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let until = self
            .deadline
            .is_none()
            .then(|| Instant::now() + WRITE_TIMEOUT);
        self.transfer(until, |mut socket| socket.write(buffer))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

pub fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// How many connections a listener serves at once.
#[derive(Clone, Copy, Debug)]
pub struct PeerLimits {
    pub total: usize,
    pub per_address: usize,
}

/// The limit that refused a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// The listener serves [`PeerLimits::total`] connections.
    Total,
    /// The address has [`PeerLimits::per_address`] connections.
    Address,
}

/// What became of an accepted connection.
#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    /// A worker thread serves it.
    Served,
    /// Closed at once. `first`: the first refusal since this limit was last
    /// reached, the one to log.
    Refused { limit: Limit, first: bool },
}

/// The number of workers a listener has not joined yet, for whoever watches
/// the listener (tests, the dashboard).
#[derive(Clone, Default)]
pub struct PeerCount(Arc<AtomicUsize>);

impl PeerCount {
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

/// The threads serving a listener's connections, one each. The accept loop
/// calls [`reap`](Self::reap) between accepts, so a thread whose connection
/// ended is joined within one accept poll; dropping the workers stops and
/// joins the rest.
pub struct PeerWorkers {
    limits: PeerLimits,
    stopping: Arc<AtomicBool>,
    workers: Vec<(IpAddr, JoinHandle<()>)>,
    count: PeerCount,
    /// The limits refused since they were last reached: the whole listener,
    /// and the addresses.
    full_noted: bool,
    noted: Vec<IpAddr>,
    /// The streams' clock, and the time limit each has from its accept.
    clock: Now,
    accept_limit: Option<Duration>,
}

impl PeerWorkers {
    pub fn new(limits: PeerLimits) -> Self {
        Self {
            limits,
            stopping: Arc::default(),
            workers: Vec::new(),
            count: PeerCount::default(),
            full_noted: false,
            noted: Vec::new(),
            clock: wall_clock(),
            accept_limit: None,
        }
    }

    /// Each accepted stream reads its deadlines from `now`, and its reads and
    /// writes fail once `limit` has passed since its accept (its handshake's
    /// deadline), until its worker sets another
    /// ([`PeerStream::set_time_limit`]).
    pub fn with_accept_deadline(mut self, now: Now, limit: Duration) -> Self {
        self.clock = now;
        self.accept_limit = Some(limit);
        self
    }

    /// Serve an accepted connection from `peer` on a thread of its own, or
    /// close it when the listener is at a limit.
    pub fn spawn(
        &mut self,
        name: &str,
        socket: TcpStream,
        peer: SocketAddr,
        handle: impl FnOnce(PeerStream) + Send + 'static,
    ) -> io::Result<Admission> {
        let address = peer.ip();
        let refused = if self.workers.len() >= self.limits.total {
            Some((Limit::Total, !std::mem::replace(&mut self.full_noted, true)))
        } else if self.from(address) >= self.limits.per_address {
            let first = !self.noted.contains(&address);
            if first {
                self.noted.push(address);
            }
            Some((Limit::Address, first))
        } else {
            None
        };
        if let Some((limit, first)) = refused {
            drop(socket);
            return Ok(Admission::Refused { limit, first });
        }
        let mut stream = PeerStream::new(socket, Arc::clone(&self.stopping))?;
        stream.set_clock(Arc::clone(&self.clock));
        stream.set_time_limit(self.accept_limit);
        let worker = thread::Builder::new()
            .name(name.into())
            .spawn(move || handle(stream))?;
        self.workers.push((address, worker));
        self.count.0.store(self.workers.len(), Ordering::Release);
        Ok(Admission::Served)
    }

    /// The connections served from `address`.
    fn from(&self, address: IpAddr) -> usize {
        self.workers
            .iter()
            .filter(|(from, _)| *from == address)
            .count()
    }

    pub fn len(&self) -> usize {
        self.workers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// A handle that reads [`len`](Self::len).
    pub fn count(&self) -> PeerCount {
        self.count.clone()
    }

    /// Join the workers whose connection ended.
    pub fn reap(&mut self) {
        let mut index = 0;
        while index < self.workers.len() {
            if self.workers[index].1.is_finished() {
                let _ = self.workers.swap_remove(index).1.join();
            } else {
                index += 1;
            }
        }
        self.count.0.store(self.workers.len(), Ordering::Release);
        self.full_noted &= self.workers.len() >= self.limits.total;
        let mut noted = std::mem::take(&mut self.noted);
        noted.retain(|&address| self.from(address) >= self.limits.per_address);
        self.noted = noted;
    }
}

impl Drop for PeerWorkers {
    fn drop(&mut self) {
        // On Windows, closing a duplicated socket need not wake another
        // thread's blocking read. Peers check this flag between bounded I/O.
        self.stopping.store(true, Ordering::Release);
        for (_, worker) in self.workers.drain(..) {
            let _ = worker.join();
        }
        self.count.0.store(0, Ordering::Release);
    }
}

pub fn accept(listener: &TcpListener) -> io::Result<Option<(TcpStream, SocketAddr)>> {
    match listener.accept() {
        Ok(peer) => Ok(Some(peer)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock
                    | io::ErrorKind::Interrupted
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Shutdown};

    /// Serves `count` pending connections of `listener`; each worker reads
    /// until its client closes.
    fn serve_pending(
        listener: &TcpListener,
        workers: &mut PeerWorkers,
        count: usize,
    ) -> Vec<Admission> {
        let mut admissions = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while admissions.len() < count && Instant::now() < deadline {
            match accept(listener).unwrap() {
                Some((socket, peer)) => admissions.push(
                    workers
                        .spawn("test-peer", socket, peer, |mut stream| {
                            let mut buffer = [0; 16];
                            loop {
                                match stream.read(&mut buffer) {
                                    Ok(0) => return,
                                    Ok(_) => (),
                                    Err(error) if retryable(&error) => (),
                                    Err(_) => return,
                                }
                            }
                        })
                        .unwrap(),
                ),
                None => thread::sleep(Duration::from_millis(1)),
            }
        }
        admissions
    }

    #[test]
    fn workers_keep_the_limits_and_are_joined_when_their_connection_ends() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let mut workers = PeerWorkers::new(PeerLimits {
            total: 3,
            per_address: 2,
        });
        let count = workers.count();
        let connect = || TcpStream::connect(address).unwrap();

        // Two connections from loopback, then that address is full: the
        // first refusal is the one to log.
        let mut clients = vec![connect(), connect(), connect(), connect()];
        assert_eq!(
            serve_pending(&listener, &mut workers, 4),
            [
                Admission::Served,
                Admission::Served,
                Admission::Refused {
                    limit: Limit::Address,
                    first: true
                },
                Admission::Refused {
                    limit: Limit::Address,
                    first: false
                },
            ]
        );
        assert_eq!(count.get(), 2);
        // A refused connection is closed at once.
        let mut byte = [0];
        clients[2]
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        assert!(matches!(clients[2].read(&mut byte), Ok(0) | Err(_)));

        // Many connections opened and dropped: every worker ends and is
        // joined, and the address may connect again.
        for _ in 0..25 {
            clients.clear();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !workers.is_empty() && Instant::now() < deadline {
                workers.reap();
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(count.get(), 0, "a worker outlived its connection");
            clients.extend([connect(), connect()]);
            let admissions = serve_pending(&listener, &mut workers, 2);
            assert_eq!(admissions, [Admission::Served, Admission::Served]);
        }
        for client in &clients {
            client.shutdown(Shutdown::Both).unwrap();
        }
        drop(clients);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !workers.is_empty() && Instant::now() < deadline {
            workers.reap();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(count.get(), 0);
    }

    #[test]
    fn a_full_listener_refuses_connections_and_dropping_it_stops_its_workers() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let mut workers = PeerWorkers::new(PeerLimits {
            total: 2,
            per_address: 5,
        });
        let count = workers.count();
        let _clients: Vec<_> = (0..4)
            .map(|_| TcpStream::connect(address).unwrap())
            .collect();
        assert_eq!(
            serve_pending(&listener, &mut workers, 4),
            [
                Admission::Served,
                Admission::Served,
                Admission::Refused {
                    limit: Limit::Total,
                    first: true
                },
                Admission::Refused {
                    limit: Limit::Total,
                    first: false
                },
            ]
        );
        // The workers still read from open connections; dropping them stops
        // and joins them within a poll.
        let start = Instant::now();
        drop(workers);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(count.get(), 0);
    }
}
