# Configuration

`orpheus` reads `config.toml` in the state folder (`%APPDATA%\orpheus\config.toml`). The file is optional. Each setting comes from the first of:

1. the command-line flag;
2. the environment variable, where one exists;
3. `config.toml`;
4. the default.

## Keys

```toml
game = 'C:\Games\Overwatch'   # the install, Overwatch.exe or the official launcher
account = "Player"            # the saved account the game logs in as
dev_console = false           # the game's own engine console (Alt+backtick)
locale = "auto"               # auto, none, or a code such as enUS

[ports]
lobby = 3724                  # TCP
stall = 1119                  # TCP, the Battle.net address the game dials
rpc = 21119                   # TCP, where the relay pipes the Battle.net stream
game = 3730                   # UDP
dashboard = 3725              # TCP; 0 turns the dashboard off
```

| Key | Flag | Environment | Default | Meaning |
| --- | --- | --- | --- | --- |
| `game` | `--game` | `ORPHEUS_GAME` | the last picked or given game, then `game\` in the current folder, then a file dialog (only the modes that start the game) | The Overwatch 1.74 install. The server also reads its game data from it. |
| `account` | `--account` | | `default_account.txt` | The saved account the game on this PC logs in as |
| `locale` | `--locale` | | `auto` | The game language: `auto`, `none` or a code such as `enUS` (see [playing](playing.md#game-language)) |
| `dev_console` | `--dev-console` | `ORPHEUS_DEV_CONSOLE` | off | Turns on the game's developer features and its own engine console: in the game, Alt+backtick opens it (a `>:` prompt; what works there: [the engine console](#the-engine-console)). The console is the client's: nothing you type there goes to the server. Don't use the `--tank_Console` or `--tank_ShowMode` game options for this: one of them opens a full-screen window that can't be hidden or closed. The flag or `ORPHEUS_DEV_CONSOLE=1` turns it on even when `config.toml` says `false`. |
| `ports.lobby` | `--lobby-port` | | 3724 | Lobby, TCP |
| `ports.stall` | `--stall-port` | | 1119 | Battle.net, TCP |
| `ports.rpc` | `--rpc-port` | | 21119 | Battle.net RPC, TCP |
| `ports.game` | `--game-port` | | 3730 | Game server, UDP. 0 turns it off: the lobby runs without matches. |
| `ports.dashboard` | `--dashboard-port` | | 3725 | Dashboard, TCP. 0 turns it off. |

Use single quotes for Windows paths in TOML, so the backslashes stay as they are.

An unknown key is an error. `orpheus doctor` shows it.

## The engine console

With `dev_console` on, the console in the game is open but has **no commands**. The 1.74 retail client keeps only one command, `runCommand`, and that one is set up while the game starts, before the server turns the console on. So every line you type is only echoed (`>: text`), and `help` prints nothing. These things work:

| Key | Effect |
| --- | --- |
| Alt+backtick | Opens and closes the console. Its pane also shows the game's log lines (map loads, "Currency: …", "You should connect to …"). |
| Tab | Completes a command name. With no commands it does nothing. |
| Alt+C | Runs the last console line again. |

Alt+Shift+F and Alt+Shift+R are developer keys too, but in the Practice Range they change nothing you can see. The research report `AdminMode.md` (section "Console commands", in the research reports folder) has the rest.

## Settings that are not in config.toml

| Flag | Environment | Meaning |
| --- | --- | --- |
| `--state <folder>` | `ORPHEUS_STATE_DIR` | The state folder. Default `%APPDATA%\orpheus`. |
| `--host <address>` | | Where the lobby and game server listen |
| `--game-host <address>` | | The public address for players on the internet. Remembered in `public_address.txt` when the menu asks for it. |
| `--server <host:port>` | | The server to join. Remembered in `server_address.txt`. |
| `--name <name>` | | Your name on the server you join. Remembered in `player_name.txt`. |
| `--timeout <seconds>` | | How long to wait for the game before the relay is loaded. Default 30. |
| `--test-players <n>` | | Start a match once `n` players search. Default 0: full teams only. |
| `--test-ai <level>` | | Fill the missing players of a match `--test-players` starts with AI heroes: `easy`, `medium` or `hard`. Default: no AI heroes. |
| `--save <file>` | | The template profile for new accounts. Default `profile.json` in the state folder. |

See the [commands reference](commands.md) for which command takes which option.
