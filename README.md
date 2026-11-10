# ORPHEUS

A server for Overwatch 1 retail **1.74.0.0.104319**.

This fork is being restructured. The plan, the target layout and the progress are in [docs/REFACTOR.md](docs/REFACTOR.md). Research and behavior notes are in [docs/notes](docs/notes).

## Requirements

- Stable Rust with the MSVC toolchain
- [just](https://github.com/casey/just), for the developer recipes
- Node.js, for the dashboard test

## Run

The native server serves the Practice Range. It reads its game data from your Overwatch 1.74 install: on the first start it derives the files it needs into `%APPDATA%\orpheus\cache\104319` (this takes a few minutes) and reuses them afterwards. The fixed build data (message layouts, protocol descriptors, captured server templates) is embedded in the binary.

```powershell
cargo build --release
target\release\orpheus.exe doctor --game C:\path\to\Overwatch
target\release\orpheus.exe serve --game C:\path\to\Overwatch
```

`doctor` checks the install, the derived data, the saved profiles and the ports, and prints a fix for each problem. `serve` runs until Ctrl+C. The dashboard is at <http://127.0.0.1:3725> (`--dashboard-port 0` turns it off). `--game-port 0` turns the game server off: the lobby runs without matches. `START_SERVER_LOCAL.cmd` builds and runs `orpheus serve`.

Profiles, logs (`logs\orpheus.log`, kept at 2 MB with two older files), `config.toml` and the answers the modes remember live in `%APPDATA%\orpheus`; pass `--state <dir>` or set `ORPHEUS_STATE_DIR` to use another folder. To reuse the profiles of an older checkout:

```powershell
target\release\orpheus.exe state import C:\path\to\old\checkout
```

`config.toml` holds the defaults; a command-line flag or environment variable overrides it:

```toml
game = 'C:\path\to\Overwatch' # or ORPHEUS_GAME, or --game
account = "Player"            # the account the game logs in as
locale = "auto"               # auto, none, or a code such as enUS

[ports]
lobby = 3724
stall = 1119
rpc = 21119
game = 3730
dashboard = 3725              # 0 turns the dashboard off
```

To start the game as well, build the whole workspace (it builds `owwfd_relay.dll` next to `orpheus.exe`) and use `play`:

```powershell
cargo build --release --workspace
target\release\orpheus.exe play --game C:\path\to\Overwatch
```

`play` checks that the install is build 1.74.0.0.104319, starts the server and the game, and loads the relay into the game. Without `--game`, `ORPHEUS_GAME` or `game` in `config.toml`, it uses the game picked last time (`game_path.txt` in the state folder), an unpacked copy in the current folder's `game`, or asks for `Overwatch.exe` in a file dialog and remembers it. `--timeout <seconds>` (30 by default) is how long the game may take before the relay is in it. The dashboard's "start a second game" opens another game in a window (`--tank_NoFullScreen=1`) that plays the chosen account; Battle.net tells the games apart by the process that owns each connection.

### Modes

`START.cmd` builds the workspace and runs `orpheus` without a command, which asks for the mode (Enter is retail); with no console to answer, it starts retail. `orpheus --mode <mode>` or a command skips the question:

| Command | Mode | What runs |
|---|---|---|
| `play` (`retail`) | 1 | The server for this PC, the local Battle.net and the game with the relay. |
| `tournament` | 2 | The server for this PC and the game in tournament mode (`--tank_TournamentMode`), no relay or Battle.net. |
| `host` | 3 | `play`, with the lobby and the game server on every address (`0.0.0.0`) for other PCs. |
| `server` | 4 | The lobby and the game server for other PCs, without a game or Battle.net here. |
| `join` | 5 | The game with the full menu on someone else's server: `--server host:port`, `--name <name>`. |
| `join-tournament` | 6 | The game in tournament mode on someone else's server: `--server host:port`. |
| `serve` | | The services for this PC without a game (Battle.net stays for a game started by hand). |

`--host <address>` sets where the lobby and game server listen. A host behind a router gives its public address with `--game-host` (the menu asks for it and remembers it); players from outside the network get it in the handoff, players on the host's network the address they reached. The banner lists the LAN addresses and the TCP and UDP ports to forward. `join` asks the server for the name's BattleTag, runs a local Battle.net that sends the game to that server, and starts the game with the relay; the address and name it asks for are remembered.

`orpheus assets build` derives the game data now and prints the cache folder, and `orpheus assets diff --pack <dir>` compares the derived files with a prepared runtime pack. `orpheus runtime prepare` still assembles a pack for the frozen Python server, which remains the reference for everything the native server does not do yet; `py -3 -B -m ow174 --mode retail --bnet-port 1118 --test-players 1` runs it with the game, using the relay from `cargo build --release -p orpheus-relay`.

`orpheus script query` answers questions about the statescript graphs in the derived data: `graphs` (which graphs a hero or a mode runs), `show` (a graph as a tree), `var` (who reads and writes a variable), `class` (where a node class is used and its fields), `find` (names, aliases, hashes, GUIDs) and `hero` (a hero's graphs, classes and shared variables). `orpheus script query --help` explains the notation.

## Check

```powershell
just check
```

This runs `cargo fmt --check`, `cargo clippy` with warnings denied, `cargo test` and the dashboard test. `just` lists the other recipes. Tests that need the game are ignored by default; run them with `just test-game <install>`. `just test-assets <install> <pack>` compares each derivation with a prepared pack, and `just test-datatool <install> <dump>` compares the asset reader with a DataTool XML export of the install.

## License

MIT, see [LICENSE](LICENSE). Upstream projects are listed in [CREDITS.md](CREDITS.md).
