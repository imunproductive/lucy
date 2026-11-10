# ORPHEUS

A server for Overwatch 1 retail **1.74.0.0.104319**.

This fork is being restructured. The plan, the target layout and the progress are in [docs/REFACTOR.md](docs/REFACTOR.md). Research and behavior notes are in [docs/notes](docs/notes).

## Requirements

- Stable Rust with the MSVC toolchain
- [just](https://github.com/casey/just), for the developer recipes
- Node.js, for the dashboard test

## Run

The native server serves the solo Practice Range on loopback. It needs a prepared runtime pack.

```powershell
cargo build --release
target\release\orpheus.exe doctor --runtime C:\path\to\pack
target\release\orpheus.exe serve --runtime C:\path\to\pack
```

`doctor` checks the runtime pack, the saved profiles and the ports, and prints a fix for each problem. `serve` runs until Ctrl+C. The dashboard is at <http://127.0.0.1:3725>. `START_SERVER_LOCAL.cmd` builds and runs `orpheus serve`.

Profiles, logs and `config.toml` live in `%APPDATA%\orpheus`; pass `--state <dir>` or set `ORPHEUS_STATE_DIR` to use another folder. To reuse the profiles of an older checkout:

```powershell
target\release\orpheus.exe state import C:\path\to\old\checkout
```

`config.toml` holds the defaults; a command-line flag or environment variable overrides it:

```toml
runtime = 'C:\path\to\pack'   # or ORPHEUS_RUNTIME_DIR, or --runtime
account = "Player"            # the account the game logs in as
game = 'C:\path\to\Overwatch' # for play; or ORPHEUS_GAME, or --game
locale = "auto"               # auto, none, or a code such as enUS

[ports]
lobby = 3724
stall = 1119
rpc = 21119
game = 3730
dashboard = 3725
```

To start the game as well, build the whole workspace (it builds `owwfd_relay.dll` next to `orpheus.exe`) and use `play`:

```powershell
cargo build --release --workspace
target\release\orpheus.exe play --runtime C:\path\to\pack --game C:\path\to\Overwatch
```

`play` checks that the install is build 1.74.0.0.104319, starts the server and the game, and loads the relay into the game. `START.cmd` does the same with the settings from `config.toml` (`game = '...'`, and `locale = "auto"` by default).

`orpheus runtime prepare` assembles a runtime pack from a prepared research checkout. The frozen Python server is still the reference for everything the native server does not do yet; `py -3 -B -m ow174 --mode retail --bnet-port 1118 --test-players 1` runs it with the game, using the relay from `cargo build --release -p orpheus-relay`.

## Check

```powershell
just check
```

This runs `cargo fmt --check`, `cargo clippy` with warnings denied, `cargo test` and the dashboard test. `just` lists the other recipes. Tests that need a prepared runtime pack or the game are ignored by default; run them with `just test-runtime <pack>` and `just test-game <install>`.

## License

MIT, see [LICENSE](LICENSE). Upstream projects are listed in [CREDITS.md](CREDITS.md).
