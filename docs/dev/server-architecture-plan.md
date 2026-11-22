# Plan: server architecture, then profiling and latency

Goal (2026-11-22): the server hosts a few matches at once (for example two or three Doomfist parkour lobbies) on one modest machine, with matches isolated from each other and connections handled cleanly. Not a goal: production operations, containers, auto-scaling, many hosts.

## Where it stands

- One process, plain threads, no async runtime (`architecture.md`, "Services").
- **Lobby:** one TCP listener (3724), one thread per connection, at most 32 peers (`server/src/lobby/mod.rs` `MAX_LOBBY_PEERS`), a mailbox hub between sessions. Battle.net (1119, 21119) and the dashboard (3725) have their own listeners and threads.
- **Game:** one UDP socket (3730) and one thread (`lucy-game`, `game_service.rs` `run`, a `mio` loop) for every match. Rooms tick one after another every 16 ms; a room's scene is prepared on a `lucy-scene` thread. A panic in any room ends the thread and every match on it.
- **Speed, measured once:** the slow Practice vs AI test (12 bodies, 11 AI) simulates 570 s of match in 240 s, about 6.7 ms per 16 ms tick, in the `gametest` build with debug assertions, under load. Nothing has been profiled.
- **Latency seen by the user** with the server on the same PC: the client's stats show 15-20 ms ping and 20-25 ms interpolation delay (IND). Cause unknown.

## Stage 1: architecture

1. **One thread per room.** A receiver thread owns the UDP socket and routes each datagram by connection id to its room's channel (unknown or malformed datagrams are dropped cheaply there); each room runs its own loop and clock on its own thread and sends through a clone of the socket. Rooms share only read-only data (`Arc`: schemas, prepared scene assets). The lobby talks to the game side through the existing request channel, now answered by the router and the room that owns the match. One port stays (3730): no firewall change.
2. **Fault isolation.** A room that panics ends alone: the router sees its channel close, sends its players home and tells the lobby the match ended. (Release and `gametest` builds unwind; Cranelift debug builds abort on a panic, so this holds for release builds.)
3. **Deterministic tests stay.** Rooms keep a synchronous step API, so `lockstep` and the virtual-clock tests drive them without threads, with the same exact assertions. Only the production runner spawns threads.
4. **Connection hygiene.**
   - Lobby, Battle.net, dashboard: deadlines for the handshake and login, idle timeouts, a per-address connection limit, threads that end and are joined when their connection ends, bounded mailboxes, a clean shutdown.
   - Game links: a peer that goes silent times out and leaves its match; datagrams for a closed match are refused; per-peer limits on queued input.
5. **Docs:** `architecture.md` describes the new threads and channels; `behavior.md` the timeouts and limits.

## Stage 2: profiling, optimisation and latency

1. **Measure.** Per-room tick time by phase (receive, simulation, statescript, replication, egress, encryption), shown in the log and the dashboard. A load harness: headless clients that join a Workshop C32S2 match and a Practice vs AI match, N players each, on the virtual clock (CPU per tick, memory per room) and on the real clock.
2. **Profile and optimise** the hot spots in a release build, with no change to what goes on the wire (every exact test unchanged).
3. **Latency.** Find why a local server shows 15-20 ms ping and 20-25 ms IND: what the client measures for each (client code), when the server reads input and sends acknowledgements and frames relative to its tick, the input buffer it keeps, and Windows timer resolution (a 15.6 ms default granularity would bunch the 16 ms ticks). Compare with retail where the research has numbers (`refs/reports/GDC-Netcode.md`). Fix what the server causes; record what is the client's design.

## Status

- 2026-11-22: plan written. Stage 1 started (RoomThreads: per-room threads and game links; LobbyConns: lobby, Battle.net and dashboard connections). A latency baseline on the current code runs beside it (LatencyBaseline, measurement only), for stage 2.
- 2026-11-22, RoomThreads (`day/roomthreads`): items 1-3 and the game links of item 4 done, with item 5's game half (`architecture.md` "Game service and simulation", `behavior.md` "One thread per match"). The router thread (`game_service/router.rs`) owns the socket and the request channel, routes each datagram by connection id to its room's channel and drops at once what no room has; each room runs on its own thread (`room.rs`, `RoomLoop::pass`) on the service's tick grid (`clock.rs` `GameClock::opened`, so frame numbers are unchanged) and sends through a clone of the socket; rooms share only `Arc` data. A room catches a panic of its pass (release, gametest): its games get 20304 as at the end of the clock's frames, and the lobby sees the seats go. Game links: the 10 s silence timeout as before, now per room; datagrams of closed matches dropped at the router; at most 64 datagrams waiting per game; at most 256 reliable messages waiting for a lost one (then the game leaves). `game_service.rs` is split into `router`, `room`, `peer`, `combat`, `flow`, `messages`. The lockstep runs the same router and rooms on one thread: every virtual-clock test passes unchanged (`lucy-server`'s 76 game-install tests, the 3 slow match tests on `gametest`). New: `rooms::two_matches_run_side_by_side_on_their_threads` (the service's threads: one match's game gets its frames while the other's room is held), `rooms::a_silent_game_leaves_its_match_and_the_closed_match_answers_nothing`, `rooms::a_match_that_panics_sends_its_game_home_and_the_other_plays_on` (`cfg(unwinds)`, gametest). For stage 2: a room takes each datagram as it arrives, and every room's wait for its next tick goes through `router.rs` `wait_until`. Left: `loose-ends.md` "Room threads: what RoomThreads left".
