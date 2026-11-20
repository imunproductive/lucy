# Architecture

ORPHEUS is one Cargo workspace. It builds one program, `orpheus.exe`, and one DLL, `owwfd_relay.dll`. The server supports only the client build 1.74.0.0.104319.

## Layout

```
ORPHEUS/
├─ Cargo.toml          workspace only
├─ crates/             the libraries (below)
├─ apps/orpheus/       the orpheus program: command line, doctor, modes
├─ web/dashboard/      dashboard front end (JavaScript), embedded into the binary
├─ web/tests/          the dashboard JavaScript test
├─ research/ida/       the IDA script that extracted the statescript codecs
├─ tools/              the client tools (Python): index, game database, debugger (docs/dev/client-tools.md)
├─ docs/user/          for players and hosts
├─ docs/dev/           for developers
├─ docs/notes/         research notes: protocol, behavior, references
├─ justfile            the developer recipes
└─ START.cmd           builds, then runs orpheus
```

## Crates

Each package is named `orpheus-<folder>`.

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
| `apps/orpheus` | The `orpheus` program |

### Dependency rules

An arrow means "depends on". Only these edges exist (normal dependencies, from the `Cargo.toml` files):

```mermaid
graph TD
  app[apps/orpheus] --> server
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
- Library crates return typed errors or `io::Error`. `apps/orpheus` reports them and sets the exit code.

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

1. **Install.** The user names the game with `--game`, `ORPHEUS_GAME` or `game` in `config.toml`. The launcher reads `.build.info` and refuses any build other than 104319.
2. **casc.** Reads the build config, the local indices and `data.NNN` archives, the encoding table and the 104319 manifest. It decodes BLTE blocks, including Salsa20-encrypted blocks. One key the install's keyring lacks comes from TACTLib's static keyring.
3. **stu.** Decodes STUv2 assets (graphs, entity definitions, game modes and more) with the type table in `crates/stu/data/types.txt`.
4. **assets.** Runs the derivations in dependency order (`DERIVATIONS`). Each writes one file in the format of the old prepared runtime pack. Collision for a map other than the Practice Range is built when a match on that map first starts.
5. **Cache.** The files go to `<state>/cache/104319/`. A stamp (`derived.txt`) holds the build version, build config key, locale and derivations version. Another stamp makes the next start derive again. A full derivation takes about two minutes in a release build.
6. **Loaders.** `content`, `script`, `physics` and `sim::scene_assets` load the cache files. `server::game_data` lists the files the server needs.
7. **Fixed data.** Some inputs are not in the install: message schemas, statescript payload codecs, Battle.net descriptors, the lobby state-blob key, the captured server templates, resource keys and the research names. They are embedded from `crates/fixed/data`.

[data-sources.md](data-sources.md) lists each file, its inputs and the oracle it was checked against.

## The server

`orpheus_server::server::Server::start` starts these services. Each has its own threads; there is no async runtime.

| Service | Transport | Default port |
| --- | --- | --- |
| Battle.net, the address the client dials | TCP | 1119 (`stall`) |
| Battle.net RPC, where the relay pipes the stream | TCP | 21119 (`rpc`) |
| Lobby (JAM protocol) | TCP | 3724 |
| Game server | UDP | 3730 |
| Dashboard (HTTP, `tiny_http`) | TCP | 3725 |

### Lobby

The lobby lives in `crates/server/src/lobby` and uses the services of the `lobby` crate.

- **Sessions.** Each client connection has its own thread. It runs the handshake and login, then a loop that reads messages and answers them.
- **Router.** `router.rs` decides which handler answers each client message after login.
- **Modules.** Each feature (chat, friends, party, matchmaking, shop, rewards, settings and more) is a module under `handlers/`. A module declares the messages it answers and its session hooks. `handlers::MODULES` lists all modules. To add a feature, add a module and list it there.
- **Hub.** `hub.rs` keeps the sessions that are online. A session that must tell another session something (a whisper, an invite, a friend's status) posts it to that session's mailbox. The other session sends it at its next wake.
- **Matchmaker.** Forms matches from the queues and hands the players to the game service.

### Game service and simulation

`server::game_service::GameService` is the UDP match worker. It runs on one thread with a `mio` event loop. Other threads talk to it through a request channel.

- **Rooms.** Each match is a room with its map, its players and its world. The room's scene, with the map's collision, is prepared on another thread while the players load. Players can join a running match.
- **Connections.** The lobby sends each player the server address, a connection id and two keys (message 20600). `server::connection` admits the player and keeps the encrypted link. `server::ingress` orders incoming messages and tracks the loading steps.
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

The dashboard backend is in `server` and serves the files of `web/dashboard`, which are embedded into the binary. It shows the server state and the saved profiles and can change them. It can also start a second game for another account.

With `ORPHEUS_DEBUG_SEND=1` it also has the debug routes (`server/dashboard/debug.rs`, [troubleshooting](../user/troubleshooting.md#marker-messages)). The marker message routes encode a message with the schema encoder and send it on the normal paths: a game link message goes to the game service as a request (`GameService::debug_send`), which queues it on the seat's reliable channel like the server's own messages; a lobby message is posted to the account's session through the hub. `POST /api/debug/kill` asks the game service to kill an account's hero through the normal death path (`Combat::environment_kill`); the client tools' `die` action uses it.

## The program

`apps/orpheus` reads the command line with `clap`, resolves the settings, starts logging and runs a mode. Settings come from, in this order: the command-line flag, the `ORPHEUS_*` environment variable, `config.toml`, the default. See the [commands reference](../user/commands.md) and [configuration](../user/configuration.md).

`play` and `host` start the server, then the launcher starts the game with `--BNetServer` and `--tank_Locale`, and injects the relay. The relay must be the `owwfd_relay.dll` next to `orpheus.exe`, so the DLL always comes from the same build. The launcher tells the relay its log file and the RPC port through environment variables.

## State folder

The state folder is `%APPDATA%\orpheus` on Windows (from the `directories` crate). `--state` or `ORPHEUS_STATE_DIR` chooses another folder. Nothing is written into the checkout.

| Path | Content |
| --- | --- |
| `config.toml` | Settings |
| `profiles/` | One JSON file per saved account |
| `profile.json` | The template for new accounts |
| `default_account.txt` | The account the local game logs in as |
| `game_path.txt` | The game picked in the file dialog |
| `public_address.txt`, `server_address.txt`, `player_name.txt` | Answers the modes remember |
| `logs/orpheus.log` | Server log, 2 MB, with two older files |
| `logs/game.log`, `logs/relay.log` | Game output and the relay log |
| `cache/104319/` | Data derived from the install |

## Network

The program opens only its own listeners, the connections a join needs, and the relay pipe. It never downloads anything.
