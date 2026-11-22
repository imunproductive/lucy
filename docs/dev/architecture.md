# Architecture

Lucy is one Cargo workspace. It builds one program, `lucy.exe`, and one DLL, `owwfd_relay.dll`. The server supports only the client build 1.74.0.0.104319.

## Layout

```
Lucy/
├─ Cargo.toml          workspace only
├─ crates/             the libraries (below)
├─ apps/lucy/       the lucy program: command line, doctor, modes
├─ web/dashboard/      the dashboard (Vite, Svelte, TypeScript): src/, and its build in dist/, which is committed and embedded into the binary
├─ research/ida/       the IDA script that extracted the statescript codecs
├─ tools/              the client tools (Python): index, game database, debugger (docs/dev/client-tools.md)
├─ docs/user/          for players and hosts
├─ docs/dev/           for developers
├─ docs/notes/         research notes: protocol, behavior, references
├─ justfile            the developer recipes
└─ START.cmd           builds, then runs lucy
```

## Crates

Each package is named `lucy-<folder>`.

| Crate | Responsibility |
| --- | --- |
| `core` | GUID text, bit readers and writers, the game tick constants |
| `fixed` | The fixed build data (`crates/fixed/data`), embedded with `include_bytes!` |
| `wire` | Byte-exact encodings: the JAM lobby protocol, the game link, entity records, input commands, TCP helpers |
| `content` | Typed game content and its loaders: catalog, heroes, maps, stats, hero-select parameters |
| `bnet` | Local Battle.net emulation: protocol, RPC, session keys, service |
| `physics` | Collision, the contact solver, the character mover, static rays, AI navigation |
| `script` | Statescript: graphs, expressions, the VM, payload codecs, snapshots and their wire format, vitals |
| `sim` | The match simulation: bodies, bots, combat, projectiles, replication, egress |
| `lobby` | Lobby services: accounts, collection, login content, parties, matchmaking rules, match handoff |
| `server` | Process wiring: the lobby threads, the game service, connections, ingress, the dashboard API |
| `casc` | Reads assets out of the install's CASC storage, with the build 104319 manifest |
| `stu` | Reads STUv2 structured assets with the 1.74 type table generated from OWLib |
| `assets` | Derives the server's data files from the install and keeps them in the cache |
| `launcher` | Windows: finds, checks, starts and injects the client |
| `relay` | Windows `cdylib`, `owwfd_relay.dll`: pipes the client's Battle.net stream to the local server |
| `testkit` | Test support: finds the game for ignored tests, builds synthetic storages |
| `apps/lucy` | The `lucy` program |

### Dependency rules

An arrow means "depends on". Only these edges exist (normal dependencies, from the `Cargo.toml` files):

```mermaid
graph TD
  app[apps/lucy] --> server
  app --> launcher
  app --> assets
  app --> lobby
  app --> sim
  server --> sim
  server --> lobby
  server --> bnet
  server --> script
  server --> assets
  assets --> casc
  assets --> stu
  launcher --> casc
  sim --> script
  sim --> physics
  sim --> content
  script --> physics
  script --> content
  lobby --> content
  lobby --> bnet
  physics --> wire
  content --> wire
  bnet --> wire
  wire --> core
  wire --> fixed
```

Most crates also use `core` and `fixed` directly; the graph leaves out those edges.

- `lobby` and `sim` never depend on each other. `server` connects them.
- `casc`, `stu`, `core`, `fixed` and `relay` depend on nothing in the workspace.
- `assets` produces files, not types. It does not depend on `content`, `script` or `physics`; the server's loaders read the files it writes.
- `testkit` is only a dev-dependency.
- Only `launcher`, `relay` and `probe` (the client probe DLL, `docs/dev/client-tools.md`) use `unsafe`. Every other crate, the app included, has `#![forbid(unsafe_code)]`. In those three, every `unsafe` block states why it is sound, and `unsafe_op_in_unsafe_fn` is denied.
- Library crates return typed errors or `io::Error`. `apps/lucy` reports them and sets the exit code.

When you add a dependency between crates, keep it in the direction of the graph. Cargo refuses cycles.

## Data flow

```mermaid
flowchart LR
  install[Overwatch 1.74 install] --> casc --> stu --> assets
  assets --> cache[(state/cache/104319)]
  cache --> loaders[content, script, physics loaders]
  fixed[fixed build data] --> loaders
  loaders --> server
  config[config.toml + flags] --> server
  state[(state folder)] <--> server
```

1. **Install.** The user names the game with `--game`, `LUCY_GAME` or `game` in `config.toml`. The launcher reads `.build.info` and refuses any build other than 104319.
2. **casc.** Reads the build config, the local indices and `data.NNN` archives, the encoding table and the 104319 manifest. It decodes BLTE blocks, including Salsa20-encrypted blocks. One key the install's keyring lacks comes from TACTLib's static keyring.
3. **stu.** Decodes STUv2 assets (graphs, entity definitions, game modes and more) with the type table in `crates/stu/data/types.txt`.
4. **assets.** Runs the derivations in dependency order (`DERIVATIONS`). Each writes one file in the format of the old prepared runtime pack. Collision for a map other than the Practice Range is built when a match on that map first starts.
5. **Cache.** The files go to `<state>/cache/104319/`. A stamp (`derived.txt`) holds the build version, build config key, locale and derivations version. Another stamp makes the next start derive again. A full derivation takes about two minutes in a release build.
6. **Loaders.** `content`, `script`, `physics` and `sim::scene_assets` load the cache files. `server::game_data` lists the files the server needs.
7. **Fixed data.** Some inputs are not in the install: message schemas, statescript payload codecs, Battle.net descriptors, the lobby state-blob key, the captured server templates, resource keys and the research names. They are embedded from `crates/fixed/data`.

[data-sources.md](data-sources.md) lists each file, its inputs and the oracle it was checked against.

## The server

`lucy_server::server::Server::start` starts these services. Each has its own threads; there is no async runtime.

| Service | Transport | Default port |
| --- | --- | --- |
| Battle.net, the address the client dials | TCP | 1119 (`stall`) |
| Battle.net RPC, where the relay pipes the stream | TCP | 21119 (`rpc`) |
| Lobby (JAM protocol) | TCP | 3724 |
| Game server | UDP | 3730 |
| Dashboard (HTTP/1.1, `server/dashboard/http.rs`) | TCP | 3725 |

### TCP connections

The lobby, Battle.net and the dashboard serve their TCP connections the same way (`wire::tcp`):

- **Accept loop.** One thread per listener (`lucy-lobby`, `bnet`, `lucy-dashboard`) polls its non-blocking listener every 10 ms and stops when `Server::stop` asks.
- **One thread per connection** (`PeerWorkers`), within the listener's limits: so many connections in all and so many from one address (`PeerLimits`; the numbers are in [behavior.md](../notes/behavior.md), "Lobby, Battle.net and dashboard connections"). A connection over a limit is closed at once, and the first refusal is logged. The accept loop joins the thread of each connection that ended before its next accept, so no thread outlives its connection by more than a poll.
- **Deadlines.** Each stream (`PeerStream`) reads its deadlines from the service's clock: the lobby's clock for the lobby and the dashboard (virtual in tests), the wall clock for Battle.net. A connection's first deadline (its handshake's) starts at its accept; the service sets the next ones (login, idle, keep-alive). Reads and writes poll every 250 ms, so a deadline or a stop is seen within one poll.
- **Shutdown.** Stopping a service closes its listener, sets the workers' stop flag (a blocked read sees it within a poll) and joins every connection's thread. `Server::stop` stops the dashboard, Battle.net, the lobby, then the game service.

### Lobby

The lobby lives in `crates/server/src/lobby` and uses the services of the `lobby` crate.

- **Sessions.** Each client connection has its own thread. It runs the handshake and login, then a loop that reads messages and answers them. The loop wakes at least every lobby tick (250 ms on the lobby's clock) and then checks the session's mailbox and how long the game has been quiet.
- **Router.** `router.rs` decides which handler answers each client message after login.
- **Modules.** Each feature (chat, friends, party, matchmaking, shop, rewards, settings and more) is a module under `handlers/`. A module declares the messages it answers and its session hooks. `handlers::MODULES` lists all modules. To add a feature, add a module and list it there.
- **Hub.** `hub.rs` keeps the sessions that are online. A session that must tell another session something (a whisper, an invite, a friend's status) posts it to that session's mailbox. The other session sends it at its next wake. A mailbox holds at most 256 messages; one that overflows shuts, refuses further mail as for an offline account, and its session ends so the game logs in again with the whole state.
- **Matchmaker.** Forms matches from the queues and hands the players to the game service.

### Game service and simulation

`server::game_service::GameService` is the UDP match server. Other threads (the lobby's sessions and matchmaker, the dashboard, a scene's preparation) talk to it through a request channel.

- **Threads.** A router thread (`lucy-game`, `game_service/router.rs`) owns the UDP socket (one port, 3730) and the request channel. It routes each datagram by the connection id in its header to the channel of the room that has that connection. A datagram that can't be a game's, whose connection no room has (its match ended, or never had it), or whose game already has 64 waiting is dropped there. The router answers the requests about no single room (opening a match, the results of the matches that ended) and passes the others to the room of their match or seat; for the snapshot each room answers its own part to the caller. Each match is a room on a thread of its own (`lucy-room-XXXX`, `game_service/room.rs`) with its own loop and clock; it sends through a clone of the socket. Rooms share only what never changes (`Arc`: the schemas, the scene assets their scenes come from). A room tells the router through events when one of its links closes, when its match ends by its rules, and when its thread ends.
- **Clock.** Every room ticks on the service's grid (`clock.rs` `GameClock::opened`): a tick every 16 ms from the service's start, with the same frame number in every room. A room that falls behind skips the ticks it missed. A room waits for its next tick in `router.rs` `wait_until`; while a room thread runs it holds the process's system timer at 1 ms (`game_service/timer.rs`, `timeBeginPeriod` on Windows, the server crate's only `unsafe`), so its ticks start within 1-2 ms of their time instead of up to 16 ms late (`refs/reports/Latency.md`).
- **Faults.** A room catches a panic of its loop (release and `gametest` builds; a Cranelift debug build aborts the process): its match stops, its games go back to the menu (20304) as at the end of the clock's frames, and its seats go as they close their links or time out, which the lobby sees as their leaving (`seat_exists`). If it fails again meanwhile, its thread ends; the router drops its connections, so its seats are gone and its games' datagrams are dropped.
- **Tests.** The lockstep (`game_service/lockstep.rs`) runs the same router and rooms on one thread on a virtual clock ([testing](testing.md#game-service-tests)); only `GameService::bind` starts threads.
- **Rooms.** Each match is a room with its map, its players and its world. The room's scene, with the map's collision, is prepared on another thread while the players load. Players can join a running match.
- **Connections.** The lobby sends each player the server address, a connection id and two keys (message 20600). `server::connection` admits the player and keeps the encrypted link; the room of the match keeps its players' links. A room acknowledges each datagram with content as it takes it, with a bare link ACK of its own (`GameConnections::acknowledge`); its frames go at the tick. `server::ingress` orders incoming messages, builds the pongs (filled with their send time as their frame leaves) and tracks the loading steps.
- **Simulation.** The `sim` crate holds bodies, bots, combat, hitscan, projectiles and pools. It runs at the game tick in `core::tick`.
- **AI heroes.** In Practice vs AI (and with `--test-ai`) the room fills its teams with AI players: a seat without a game, a real hero body in a headless `WorldDelivery` whose records arrive at once, and `sim::ai::AiController`, which makes each tick's command from what the AI sees (navigation from `physics::nav`, line of sight through the collision). Other clients see them as any other player.
- **Replication.** `sim::replication` builds what each peer sees: entity creates, updates and the owning player's body. `sim::egress` packs it into datagrams within the size limits in [protocol.md](../notes/protocol.md).
- **Statescript VM.** `script::vm` runs the authored statescript graphs of heroes, modes and bots: entry, variable writes, links, sub-scripts, waits, messages, abilities and timers. `script::snapshot` and `script::stream` encode the VM state for the client with the payload codecs from the fixed data.

### Unknowns policy

The VM does not implement every node of every graph. `script::vm::Unknowns` decides what happens with graph content it does not know:

- **Refuse** (debug builds and tests): record a diagnostic and refuse the body. A gap cannot hide.
- **Tolerate** (release builds): do what the Python reference did, and record each case in the snapshot's assumptions. An unknown state is a plain state. An unknown action follows its out plug. An unknown config value or opcode is null. An active state whose payload cannot be written is left out of frames and logged once per class (`script::snapshot`).

Every verified class of the client's codec table must have a payload family in `script::codecs`: `CodecTable::client()` panics in a debug build when one has none.

So a debug build can stop a hero that a release build plays. Use a release build to play and a debug build to find gaps.

### Dashboard

The dashboard backend is in `server` and serves the dashboard's build. It shows the server state and the saved profiles and can change them. It can also start a second game for another account. Its HTTP/1.1 server (`dashboard/http.rs`) is the shared TCP model above; it handles the requests one at a time, whichever connection they come on.

The front end is in `web/dashboard`: a Svelte 5 and TypeScript single-page app that Vite builds into `web/dashboard/dist/` (`just dashboard-build`). The build is committed, so `cargo build` needs no Node. While you work on it, `npm run dev` there serves the page with hot reload and sends `/api` to a running `lucy serve` (port 3725, or the address in `LUCY_DASHBOARD`). `server/build.rs` lists the files of `dist/` and embeds them with `include_bytes!`, with their MIME types. The server answers a GET this way:

- `/api/...`: the JSON API (`dashboard/api*.rs`).
- A file of the build: the scripts and styles under `/assets/` have a content hash in their names and are cached for a year (`immutable`). `index.html` and `favicon.svg` are sent with `no-cache`.
- `/assets/boxes/*.png` and `/assets/previews/*.webp`: optional pictures from the assets folder.
- Any other path without a file extension: `index.html`. The page routes itself (`/events`, `/matches/<id>` and so on, `src/lib/routes.ts`).

In the front end, `src/lib/api.ts` is the typed client of the API and `src/lib/types.ts` holds the reply types. `src/lib/live.ts` is the client of the live-match API. The page reads `/api/state` every 10 s while it is visible.

#### Live matches and the host's controls

The live pages (`/matches`, `/matches/<id>`) poll `GET /api/live?rev=N` once a second (`dashboard/live.rs`): the online accounts and every match (mode, map, the mode graph and what drives the flow, phase, clock, pause, round, winner, players with their hero, team, state and whether the server plays them, the last chat line's number, and the flow controls the match allows now), or only `{rev}` when nothing changed since revision N. Polling, not server-sent events: the dashboard's server serves one request at a time and gives each response a 30 s deadline, so a held stream would need its own path around both.

- **Chat.** A match's players are in its match channel (type 6) and their team's (type 5, `handoff::team_channel`), which the handoff joins after the match's (`handoff::enter_game`); `SocialState::channel_members` sends a team line to that team only. `lobby/chat_log.rs` keeps each match's lines (players' match and team lines, the host's system lines) while anyone is in its chat and 30 min after, and General's; `GET /api/matches/<id>/chat?after=` and `/api/lobby/chat?after=` page them by sequence number.
- **System lines** (`lobby/host.rs`): a 20400 with an empty sender card, which the client draws alone in the channel's colour. `POST /api/matches/<id>/chat {text, channel}` sends one to a match or a team; `POST /api/broadcast {text, scope}` to everyone online, a player in a match on his match channel, one in the menu on General.
- **Kick and stop.** `POST /api/matches/<id>/kick {account, reason}` takes the player out through the matchmaker's seat (`Matchmaker::send_home` with `Home::Player`), or the game service's for a seat the matchmaker doesn't hold (`GameService::kick`, the solo Practice Range): his game gets 20304, the others "left the game" once his link closes, the match a system line, and he gets the reason on General once his game is in the menu (`ChatLog::leave_note`, delivered by `Turn::game_left`). `POST /api/matches/<id>/stop {result}` declares the winner or a draw (then the match ends as its mode does: cards, results, home) or, with `none`, sends every game home now (`Home::Match` and `GameService::stop_match`).
- **Flow controls** (`game_service/controls.rs`): `POST /api/matches/<id>/flow {action}` runs the Workshop action that does it on the mode entity's script (Set Match Time `1ABC`, Pause `1D53`, Unpause `1D54`, Declare Team Victory `1384`, Declare Match Draw `1D55`, Declare Round Victory `1E2D`, Round Draw `272A`, Go To Assemble Heroes `1EF9`, Disable/Enable Built-In Game Mode Completion `1ABA`/`1AB9`), so the mode's own graph takes its message. A match offers the controls whose message its graph has an entry for (`ModeRunner::takes`). Where the match flow stands in for the graph (Escort, Hybrid, a graph that doesn't run whole), the flow takes Set Match Time and the declared winner as the graph would (`MatchFlow::receive`): from the action's own message on a Workshop match's mode entity, else directly. Restart Match `2FD8` (the server's own `STU_5E019216`) is not offered.

## The program

`apps/lucy` reads the command line with `clap`, resolves the settings, starts logging and runs a mode. Settings come from, in this order: the command-line flag, the `LUCY_*` environment variable, `config.toml`, the default. See the [commands reference](../user/commands.md) and [configuration](../user/configuration.md).

`play` and `host` start the server, then the launcher starts the game with `--BNetServer` and `--tank_Locale`, and injects the relay. The relay must be the `owwfd_relay.dll` next to `lucy.exe`, so the DLL always comes from the same build. The launcher tells the relay its log file and the RPC port through environment variables.

## State folder

The state folder is `%APPDATA%\lucy` on Windows (from the `directories` crate). `--state` or `LUCY_STATE_DIR` chooses another folder. Nothing is written into the checkout.

| Path | Content |
| --- | --- |
| `config.toml` | Settings |
| `profiles/` | One JSON file per saved account |
| `profile.json` | The template for new accounts |
| `default_account.txt` | The account the local game logs in as |
| `game_path.txt` | The game picked in the file dialog |
| `public_address.txt`, `server_address.txt`, `player_name.txt` | Answers the modes remember |
| `logs/lucy.log` | Server log, 2 MB, with two older files |
| `logs/game.log`, `logs/relay.log` | Game output and the relay log |
| `cache/104319/` | Data derived from the install |

## Network

The program opens only its own listeners, the connections a join needs, and the relay pipe. It never downloads anything.
