# Commands reference

`lucy --help` lists the commands. `lucy <command> --help` shows the options of one command. This page is made from that help.

```
Usage: lucy.exe [OPTIONS] [COMMAND]

Commands:
  play             Run the server, start the game and load the relay into it (retail mode)
  host             Host for other PCs and play here: retail with the lobby open to other PCs
  tournament       Run the server and the game in tournament mode, with no relay or Battle.net
  serve            Run the Battle.net, lobby and game services until Ctrl+C, without starting a game
  server           Host for other PCs without playing here: the lobby and game server only
  join             Play on someone else's server with the full menu (--server, --name)
  join-tournament  Play on someone else's server in tournament mode (--server)
  doctor           Check the setup the modes need and say how to fix each problem
  state            Manage saved player state
  assets           Game data derived from the install
  script           Statescript research tools
  help             Print this message or the help of the given subcommand(s)
```

## Without a command

`lucy` without a command asks for the mode (see [playing](playing.md#startcmd)). Enter picks retail. Without a console, it starts retail.

`--mode <mode>` starts a mode without asking. The modes are `retail`, `tournament`, `host`, `server`, `join` and `join-tournament`. Without a command, the options follow `lucy` directly: `lucy --mode join --server 1.2.3.4:3724`.

With a command, the options go after the command: `lucy play --game ...`. Only `--state` can go anywhere.

## Options

### Everywhere

| Option | Meaning |
| --- | --- |
| `--state <DIR>` | Folder for profiles, logs, `config.toml` and remembered answers. Default `%APPDATA%\lucy`. Environment: `LUCY_STATE_DIR`. |

### Server options

`play`, `host`, `tournament`, `serve`, `server`, `join`, `join-tournament`, `doctor`, `assets build`, `assets diff` and `script query` take these. [Configuration](configuration.md) has the defaults.

| Option | Meaning |
| --- | --- |
| `--account <NAME>` | Saved account the game logs in as |
| `--game <PATH>` | `Overwatch.exe`, its installation folder or the official launcher. The server reads its game data from this install. Environment: `LUCY_GAME`. |
| `--locale <CODE>` | Game language: `auto`, `none`, or a code such as `enUS` |
| `--lobby-port <PORT>` | Lobby TCP port |
| `--stall-port <PORT>` | Battle.net TCP port the client dials |
| `--rpc-port <PORT>` | Battle.net RPC TCP port the relay pipes to |
| `--game-port <PORT>` | Game UDP port; 0 turns the game server off (the lobby runs without matches) |
| `--dashboard-port <PORT>` | Dashboard TCP port; 0 turns the dashboard off |
| `--test-players <PLAYERS>` | Start a match once this many players search (default 0: full teams only) |
| `--test-ai <LEVEL>` | Fill the missing players of a match `--test-players` starts with AI heroes of this level (`easy`, `medium`, `hard`) |
| `--save <FILE>` | Template profile for new accounts (default `profile.json` in the state folder) |
| `--dev-console` | Turn on the game's developer features and its own engine console: Alt+backtick opens it in the game. Its commands run on the client; nothing goes to the server. Use this, never `--tank_Console` or `--tank_ShowMode`. Environment: `LUCY_DEV_CONSOLE`. |
| `--max-players <PLAYERS>` | Players the lobby serves at once (default 128) |
| `--max-players-per-address <PLAYERS>` | Players the lobby serves at once from one address (default 16, or `--max-players` if lower) |

### Mode options

The mode commands (`play`, `host`, `tournament`, `serve`, `server`, `join`, `join-tournament`) also take these. Each mode uses the ones that apply to it.

| Option | Meaning |
| --- | --- |
| `--host <ADDRESS>` | Address the lobby and the game server listen on. Default `0.0.0.0` for `host` and `server`, else `127.0.0.1`. |
| `--game-host <ADDRESS>` | Address games are sent to for a match, if not the one they reach the lobby at (behind a router's port forward: the public address) |
| `--server <HOST:PORT>` | Address of the server to join, such as `1.2.3.4:3724` |
| `--name <NAME>` | Your name on the server you join with the full menu |
| `--timeout <SECONDS>` | Seconds to wait for the game to be ready for the relay. Default 30. |
| `-- <ARGUMENTS>` | Everything after `--` goes to `Overwatch.exe` as it is, after the arguments `lucy` passes itself. For example `lucy play --game D:\Games\Overwatch -- --tank_Locale=enUS`. Only the modes that start the game use them. Don't pass `--tank_Console` or `--tank_ShowMode`: one of them opens a full-screen window that can't be hidden or closed. |

## Modes

| Command | Menu | What runs | Guide |
| --- | --- | --- | --- |
| `play` (alias `retail`) | 1 | The server for this PC, the local Battle.net and the game with the relay | [playing](playing.md#lucy-play) |
| `tournament` | 2 | The server for this PC and the game in tournament mode. No relay or Battle.net. | [playing](playing.md#lucy-tournament) |
| `host` | 3 | `play`, with the lobby and game server open to other PCs | [hosting](hosting.md) |
| `server` | 4 | The lobby and game server for other PCs. No game or Battle.net here. | [hosting](hosting.md) |
| `join` | 5 | The game with the full menu on someone else's server | [hosting](hosting.md#join-with-the-full-menu-lucy-join) |
| `join-tournament` | 6 | The game in tournament mode on someone else's server | [hosting](hosting.md#join-in-tournament-mode-lucy-join-tournament) |
| `serve` | | The services for this PC without a game. Battle.net stays on, for a game started by hand. | [playing](playing.md#lucy-serve) |

## doctor

`lucy doctor [server options]` checks the setup the modes need and says how to fix each problem. See [doctor and troubleshooting](troubleshooting.md).

## state import

```
lucy state import [--overwrite] <CHECKOUT>
```

Copies `profiles\*.json`, `profile.json` and `default_account.txt` from an older checkout's root folder into the state folder. `--overwrite` replaces files that already exist. See [first run](first-run.md#5-profiles-from-an-older-checkout).

## assets

| Command | Meaning |
| --- | --- |
| `lucy assets build [server options]` | Derive the game data now and print the cache folder |
| `lucy assets diff --pack <DIR> [server options]` | Derive the game data into a scratch folder and compare every file with a prepared runtime pack. For developers; see [testing](../dev/testing.md). |

## script query

`lucy script query <command>` answers questions about the statescript graphs in the derived game data. It is a research tool.

| Command | Meaning |
| --- | --- |
| `graphs` | Graphs with their role and size, for example `graphs --hero soldier` |
| `show` | A graph as a tree, for example `show 0255 --state-index 1 --depth 1`. `--state-index` is the `m_states` index to start at (`--state` is the state folder, as everywhere) |
| `var` | Every use of a variable, for example `var 699 --graph 0255` |
| `class` | Where a class is used and its fields, for example `class MovementMod` |
| `find` | Names, aliases, hashes, GUIDs, for example `find "hero select"` |
| `hero` | A hero's body, graphs, classes and shared variables |

`--owlib <DIR>` (environment `OWLIB_TYPES`) points to `TankLib/STU/Types` of an OWLib checkout, for each field's declared type and offset. `lucy script query --help` explains the notation.
