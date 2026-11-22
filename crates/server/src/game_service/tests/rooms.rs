//! The rooms of the game service: each match on a loop and clock of its
//! own (`room.rs`), and with the service's threads on a thread of its own
//! (`router.rs`). A game that goes silent leaves its match, a match that
//! ended answers nothing, a match that panics ends alone, and two matches
//! run side by side.

use super::*;
use crate::connection::SILENCE_TIME;
use lucy_wire::link::{LinkCipher, MAX_DATAGRAM};
use std::net::{Ipv4Addr, UdpSocket as GameSocket};

const OTHER: MatchId = [6, 9];

fn open(server: &GameService, id: MatchId) -> Handoff {
    server
        .open_match(id, GameMap::practice_range(), player())
        .unwrap()
        .pop()
        .unwrap()
}

/// A game that goes silent leaves its match once more than 10 s pass
/// without a datagram from it (`SILENCE_TIME`). The match had only him: it
/// is gone with him, and a datagram for it gets no answer.
#[test]
fn a_silent_game_leaves_its_match_and_the_closed_match_answers_nothing() {
    let server = Lockstep::new(Arc::new(fixture())).unwrap();
    let ticket = open(&server, MATCH);
    let mut client = Client::new(&server, &ticket);
    // His last datagram, the handshake's ACK, arrives at the last tick's
    // time. His link expires at the first tick more than 10 s after it:
    // 10 s are 625 ticks of 16 ms, so the 626th (`GameConnections::expire`).
    client.handshake();
    server.run(SILENCE_TIME);
    assert!(server.seat_exists(&ticket).unwrap());
    assert_eq!(server.snapshot().unwrap().len(), 1);
    client.discard();
    server.step();
    assert!(!server.seat_exists(&ticket).unwrap());
    assert!(server.snapshot().unwrap().is_empty());
    // The tick that took him out sent him nothing, and a new SYN for the
    // match that ended gets no SYN|ACK.
    client.send(SYN, Vec::new());
    client.hears_nothing(Duration::from_millis(100));
}

/// A match whose tick panics ends alone (`RoomLoop::fail`): the tick that
/// failed sends its game nothing, the next sends it 20304 as the drain at
/// the end of the clock's frames does, its seat goes (for the lobby too)
/// once the game closes its link, and the match then answers nothing. The
/// other match ticks through it all.
#[cfg(unwinds)]
#[test]
#[ignore = "unwinds: a room catches a panic only in a build that unwinds (release, gametest), and a debug build aborts on one; just test-game runs it"]
fn a_match_that_panics_sends_its_game_home_and_the_other_plays_on() {
    let schemas = Arc::new(fixture());
    let server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let failing = open(&server, MATCH);
    let playing = open(&server, OTHER);
    let mut failed = Client::new(&server, &failing);
    failed.handshake();
    let mut other = Client::new(&server, &playing);
    other.handshake();
    assert_eq!(first_reliable(&failed, &schemas), 20300);
    assert_eq!(first_reliable(&other, &schemas), 20300);

    server.fail_match(MATCH).unwrap();
    let armed = server.now();
    failed.discard();
    other.discard();
    // Each tick sends what it queued: the failing tick (the next) nothing,
    // the drain's first tick the 20304 the failure queued.
    let home = failed.receive();
    assert_eq!(tick_of(&home), tick_at(armed) + 2);
    assert!(
        messages(&schemas, &home)
            .iter()
            .any(|message| message.message.id == 20304)
    );
    assert_eq!(tick_of(&other.receive()), tick_at(armed) + 1);
    assert_eq!(tick_of(&other.receive()), tick_at(armed) + 2);

    // His game goes back to the menu and closes its link.
    failed.send(FIN | ACK, Vec::new());
    while failed.receive().kind != KIND_CONTROL {}
    assert!(!server.seat_exists(&failing).unwrap());
    let ids: Vec<MatchId> = server
        .snapshot()
        .unwrap()
        .iter()
        .map(|found| found.id)
        .collect();
    assert_eq!(ids, [OTHER]);
    failed.send(SYN, Vec::new());
    failed.hears_nothing(Duration::from_millis(100));
    other.discard();
    let frame = other.receive();
    assert_eq!(tick_of(&frame), tick_at(other.now()));
}

/// A game on the UDP socket of a service with its own threads.
struct UdpGame {
    socket: GameSocket,
    cipher: LinkCipher,
    connection: u32,
    next: u32,
}

impl UdpGame {
    fn connect(server: &GameService, ticket: &Handoff) -> Self {
        let socket = GameSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket.connect(server.local_addr()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut game = Self {
            socket,
            cipher: LinkCipher::new(ticket.key_out, ticket.key_in),
            connection: ticket.conn,
            next: 0,
        };
        game.send(SYN);
        let answer = game.receive();
        assert_eq!((answer.kind, answer.flags), (KIND_CONTROL, SYN | ACK));
        game.send(ACK);
        game
    }

    fn send(&mut self, flags: u8) {
        let packet = Packet {
            conn: self.connection,
            kind: KIND_CONTROL,
            seq: self.next,
            flags,
            ..Packet::default()
        };
        self.next += 1;
        self.socket
            .send(&self.cipher.seal(&packet).unwrap())
            .unwrap();
    }

    fn receive(&self) -> Packet {
        let mut buffer = [0; MAX_DATAGRAM];
        let count = self.socket.recv(&mut buffer).unwrap();
        let packet = self.cipher.open(&buffer[..count]).unwrap();
        assert_eq!(packet.conn, self.connection);
        packet
    }

    fn frame(&self) -> Packet {
        loop {
            let packet = self.receive();
            if packet.kind == KIND_FRAME {
                return packet;
            }
        }
    }

    /// Drops the datagrams that came and weren't read.
    fn discard(&self) {
        self.socket.set_nonblocking(true).unwrap();
        while self.socket.recv(&mut [0; MAX_DATAGRAM]).is_ok() {}
        self.socket.set_nonblocking(false).unwrap();
    }
}

/// The service's own threads (`GameService::bind`): each match's room on a
/// thread of its own. While one room's thread is held, as a long tick would
/// hold it, the other match's game still gets a frame every tick; let go,
/// the held room goes on at the service's clock, past the ticks it missed.
/// Nothing here depends on how fast the threads run.
#[test]
fn two_matches_run_side_by_side_on_their_threads() {
    let mut server =
        GameService::bind((Ipv4Addr::LOCALHOST, 0).into(), Arc::new(fixture())).unwrap();
    let held = UdpGame::connect(&server, &open(&server, MATCH));
    let playing = UdpGame::connect(&server, &open(&server, OTHER));
    held.frame();
    playing.frame();

    let hold = server.hold_match(MATCH).unwrap();
    // A frame or two of the other match's may have been in flight: five
    // frames a tick apart or more show it ticks while the other waits.
    playing.discard();
    let ticks: Vec<u32> = (0..5).map(|_| tick_of(&playing.frame())).collect();
    assert!(ticks.windows(2).all(|pair| pair[0] < pair[1]), "{ticks:?}");
    held.discard();
    drop(hold);
    let resumed = tick_of(&held.frame());
    assert!(resumed >= ticks[4], "{resumed} after {ticks:?}");
    server.stop().unwrap();
}
