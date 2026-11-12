//! Tests: a game's client of the lobby over loopback TCP, in step with the
//! lobby's virtual clock ([`VirtualClock`]).
//!
//! A session handles its client's frames in order. A ping (a control frame)
//! is answered once the session handled what came before it and sent
//! everything it had to send by then: the replies, the mail other sessions
//! posted to it, and what its tick sends once the clock has passed a lobby
//! tick ([`TICK`]) since its last wake. So after [`Client::sync`] the client
//! holds all the lobby sent it; nothing more comes until a client sends
//! again or the clock moves. [`Client::send`] syncs, so another session's
//! mail is posted before the test reads it. A test never waits in real time:
//! the socket's read timeout only fails a test whose lobby hangs.

use std::{
    io::{ErrorKind, Read},
    net::{Shutdown, SocketAddr, TcpStream},
    time::Duration,
};

use orpheus_wire::jam::{
    cipher::Jam,
    codec::{Fields, Schemas},
    framing::{self, FrameReader},
    handshake::client_handshake,
};

use super::{TICK, clock::VirtualClock};

/// How long a read may block before the test fails: the lobby hangs.
const HANG: Duration = Duration::from_secs(10);

pub(crate) struct Client<'a> {
    schemas: &'a Schemas,
    /// The groups the client announced, by wire index - 1.
    groups: &'a [u32],
    tcp: TcpStream,
    tx: Jam,
    reader: FrameReader,
    /// What the client received and has not consumed: group, id, fields.
    pub(crate) received: Vec<(u32, u32, Fields)>,
}

impl<'a> Client<'a> {
    /// A connection that announced `groups`, not logged in yet.
    pub(crate) fn connect(schemas: &'a Schemas, lobby: SocketAddr, groups: &'a [u32]) -> Self {
        let mut tcp = TcpStream::connect(lobby).unwrap();
        tcp.set_read_timeout(Some(HANG)).unwrap();
        let (mut tx, rx) = client_handshake(&mut tcp);
        let mut announcement = vec![0, 0];
        announcement.extend_from_slice(&(groups.len() as u32).to_le_bytes());
        for group in groups {
            announcement.extend_from_slice(&group.to_le_bytes());
        }
        framing::send_frame(&mut tcp, &mut tx, &announcement).unwrap();
        Self {
            schemas,
            groups,
            tcp,
            tx,
            reader: FrameReader::new(rx),
            received: Vec::new(),
        }
    }

    /// Connects and logs in as `name`; returns with the login's messages
    /// received.
    pub(crate) fn login(
        schemas: &'a Schemas,
        lobby: SocketAddr,
        groups: &'a [u32],
        name: &str,
    ) -> Self {
        use orpheus_lobby::login::{LOGIN_MESSAGE, OUT_CONNECT};
        use orpheus_wire::jam::codec::Value;

        let mut client = Self::connect(schemas, lobby, groups);
        client.send(
            OUT_CONNECT,
            LOGIN_MESSAGE,
            Fields::from([(0x78, Value::String(name.into()))]),
        );
        client
    }

    /// Sends a message and returns once the lobby handled it, with what it
    /// sent this client by then ([`sync`](Self::sync)).
    pub(crate) fn send(&mut self, crc: u32, id: u32, fields: Fields) {
        let wire = self
            .groups
            .iter()
            .position(|&group| group == crc)
            .expect("an announced group") as u8
            + 1;
        let mut frame = self.schemas.header(crc, wire, id).unwrap();
        frame.extend(self.schemas.encode(crc, id, &fields).unwrap());
        framing::send_frame(&mut self.tcp, &mut self.tx, &frame).unwrap();
        self.sync();
    }

    /// Pings the session and reads until its answer: everything the lobby
    /// sent this client by then is in `received` (the module notes).
    pub(crate) fn sync(&mut self) {
        framing::send_frame(
            &mut self.tcp,
            &mut self.tx,
            &[framing::CONTROL_WIRE, framing::PING],
        )
        .unwrap();
        let mut buffer = vec![0; 65_536];
        loop {
            let count = self
                .tcp
                .read(&mut buffer)
                .unwrap_or_else(|error| panic!("the lobby did not answer the ping: {error}"));
            assert_ne!(count, 0, "the lobby closed the connection");
            let mut answered = false;
            for frame in self.reader.feed(&buffer[..count]) {
                match frame.first() {
                    Some(&framing::CONTROL_WIRE) => answered |= frame == framing::PONG,
                    Some(&wire) => {
                        let crc = self.groups[usize::from(wire) - 1];
                        let (id, body) = self.schemas.split(crc, &frame).unwrap();
                        let fields = self.schemas.decode(crc, id, body, true).unwrap();
                        self.received.push((crc, id, fields));
                    }
                    None => {}
                }
            }
            if answered {
                return;
            }
        }
    }

    /// The first message of this kind received so far, or by the next
    /// sync; it and everything before it are consumed.
    pub(crate) fn expect(&mut self, crc: u32, id: u32) -> Fields {
        if self.position(crc, id).is_none() {
            self.sync();
        }
        let Some(index) = self.position(crc, id) else {
            let ids: Vec<u32> = self.received.iter().map(|&(_, id, _)| id).collect();
            panic!("no {crc:08X}/{id}, got {ids:?}");
        };
        self.received.drain(..=index).next_back().unwrap().2
    }

    /// Everything received by the next sync; the ids.
    pub(crate) fn settle(&mut self) -> Vec<u32> {
        self.take().into_iter().map(|(_, id, _)| id).collect()
    }

    /// Everything received by the next sync.
    pub(crate) fn take(&mut self) -> Vec<(u32, u32, Fields)> {
        self.sync();
        std::mem::take(&mut self.received)
    }

    /// Closes the connection and returns once the lobby ended the session:
    /// it closes its end after the session's logout.
    pub(crate) fn close(mut self) {
        self.tcp.shutdown(Shutdown::Write).unwrap();
        let mut buffer = vec![0; 65_536];
        loop {
            match self.tcp.read(&mut buffer) {
                Ok(0) => return,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                    ) =>
                {
                    return;
                }
                Err(error) => panic!("the lobby did not end the session: {error}"),
            }
        }
    }

    fn position(&self, crc: u32, id: u32) -> Option<usize> {
        self.received
            .iter()
            .position(|&(group, message, _)| (group, message) == (crc, id))
    }
}

/// Runs the lobby's clock for `time`, a lobby tick ([`TICK`]) at a time;
/// after each, every client's session ticks ([`Client::sync`]).
pub(crate) fn run(clock: &VirtualClock, clients: &mut [&mut Client<'_>], time: Duration) {
    assert!(
        time.as_nanos().is_multiple_of(TICK.as_nanos()),
        "{time:?} is not a number of lobby ticks"
    );
    for _ in 0..time.as_nanos() / TICK.as_nanos() {
        clock.advance(TICK);
        for client in clients.iter_mut() {
            client.sync();
        }
    }
}
