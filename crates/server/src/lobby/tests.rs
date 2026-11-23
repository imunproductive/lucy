//! The lobby's connections over loopback, on the virtual clocks
//! (`Server::start_virtual`): their threads, limits and deadlines.

use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    thread,
    time::{Duration, Instant},
};

use lucy_lobby::{
    handoff::{CREATE_GAME, GAME_REQUEST, HANDOFF, HANDOFF_MESSAGE},
    login::OUT_CONNECT,
    session::{HANDSHAKE_TIMEOUT, LOGIN_TIMEOUT},
    templates::IN_CONNECT,
};
use lucy_wire::jam::codec::{Fields, Schemas, Value};

use super::{IDLE_TIMEOUT, LIMITS, TICK, test_client::Client};
use crate::server::{Server, ServerConfig, VirtualTime};

const ANNOUNCED: [u32; 4] = [IN_CONNECT, OUT_CONNECT, GAME_REQUEST, HANDOFF];

fn start(state: &tempfile::TempDir) -> (Server, VirtualTime) {
    let mut config = ServerConfig::new(lucy_testkit::runtime_dir(), state.path().into());
    config.lobby_port = 0;
    config.game_port = Some(0);
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = None;
    Server::start_virtual(config).unwrap()
}

/// Waits in real time for the lobby's threads to catch up; fails after
/// 10 s.
fn settle(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn connect(lobby: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(lobby).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
}

/// The lobby closed `stream`.
fn closed(stream: &mut TcpStream) -> bool {
    matches!(stream.read(&mut [0]), Ok(0) | Err(_))
}

#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn every_lobby_connection_thread_ends_with_its_connection() {
    let state = tempfile::tempdir().unwrap();
    let (server, _time) = start(&state);
    let lobby = SocketAddr::from(server.lobby_addr());
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    for _ in 0..5 {
        // Bare connections and logged-in games, up to the most from one
        // address.
        let bare: Vec<_> = (0..LIMITS.per_address / 2)
            .map(|_| connect(lobby))
            .collect();
        let games: Vec<_> = (0..LIMITS.per_address / 2)
            .map(|index| Client::login(&schemas, lobby, &ANNOUNCED, &format!("Game{index}")))
            .collect();
        settle("every connection is served", || {
            server.lobby_peers() == LIMITS.per_address
        });
        // One more from this address is closed at once.
        assert!(closed(&mut connect(lobby)));
        assert_eq!(server.lobby_peers(), LIMITS.per_address);
        drop((bare, games));
        settle("a thread outlived its connection", || {
            server.lobby_peers() == 0
        });
    }
}

#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn quiet_links_close_at_their_deadlines_unless_the_game_holds_a_seat() {
    let state = tempfile::tempdir().unwrap();
    let (server, time) = start(&state);
    let clock = &time.lobby;
    let lobby = SocketAddr::from(server.lobby_addr());
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();

    // A connection that never starts the handshake, and a game that never
    // logs in; the ping shows its session waits for the login.
    let mut silent = connect(lobby);
    let mut unnamed = Client::connect(&schemas, lobby, &ANNOUNCED);
    unnamed.sync();
    settle("both are served", || server.lobby_peers() == 2);
    // Both deadlines pass at 30 s: the handshake's from the accept, the
    // login's from the handshake, both at 0 s.
    assert_eq!(HANDSHAKE_TIMEOUT, LOGIN_TIMEOUT);
    clock.advance(HANDSHAKE_TIMEOUT);
    assert!(closed(&mut silent));
    unnamed.closed_by_lobby();
    settle("their threads end", || server.lobby_peers() == 0);

    // A game in the menu: its last bytes are this ping, at 30 s. At 89.75 s
    // it has been quiet 59.75 s and its link stays (the session's keepalive
    // comes after its idle check); at 90 s it closes.
    let mut menu = Client::login(&schemas, lobby, &ANNOUNCED, "Alpha");
    menu.settle();
    clock.advance(IDLE_TIMEOUT - TICK);
    menu.keepalive();
    clock.advance(TICK);
    menu.closed_by_lobby();
    settle("its thread ends", || server.lobby_peers() == 0);

    // A game with a seat in a match (the Practice Range it asked for; its
    // game service's clock stands, so the seat stays reserved) keeps its
    // quiet link.
    let mut player = Client::login(&schemas, lobby, &ANNOUNCED, "Beta");
    player.send(
        GAME_REQUEST,
        CREATE_GAME,
        Fields::from([
            (0x78, Value::Unsigned(2)),
            (0x80, Value::String(String::new())),
            (0xa8, Value::Unsigned(4)),
        ]),
    );
    player.expect(HANDOFF, HANDOFF_MESSAGE);
    player.settle();
    clock.advance(2 * IDLE_TIMEOUT);
    player.keepalive();
    assert_eq!(server.lobby_peers(), 1);
    player.close();
}
