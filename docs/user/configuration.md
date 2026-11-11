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
locale = "auto"               # auto, none, or a code such as enUS
bot_controller = "player"     # player, ai or off

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
| `bot_controller` | `--bot-controller` | `ORPHEUS_BOT_CONTROLLER` | `player` | How the training bots get a name in "ELIMINATED" and the kill feed. See [Bot controllers](#bot-controllers). |
| `ports.lobby` | `--lobby-port` | | 3724 | Lobby, TCP |
| `ports.stall` | `--stall-port` | | 1119 | Battle.net, TCP |
| `ports.rpc` | `--rpc-port` | | 21119 | Battle.net RPC, TCP |
| `ports.game` | `--game-port` | | 3730 | Game server, UDP. 0 turns it off: the lobby runs without matches. |
| `ports.dashboard` | `--dashboard-port` | | 3725 | Dashboard, TCP. 0 turns it off. |

Use single quotes for Windows paths in TOML, so the backslashes stay as they are.

An unknown key is an error. `orpheus doctor` shows it.

## Bot controllers

The game names a killed player by his player entity, the entity that controls his hero. A training bot has no such entity, so the server gives each bot a stand-in one. `bot_controller` sets what kind:

| Value | What the server sends for each bot |
| --- | --- |
| `player` | A player entity (definition `000E`) with the bot's team, a made-up account named after the bot ("Training Bot", "Friendly Bot") and a link to the bot's body. It is not a match participant, so it should not show in the team list or the Tab board. |
| `ai` | The bot body's own AI entity (definition `1404`, or `14AF` for two of the bodies) with the bot's team, a link to the body and the bot's hero. This is probably closer to what the original servers did, but the game may reject it. |
| `off` | Nothing. The bots stay unnamed: "ELIMINATED" without a name, and no kill feed line. |

If the game crashes or misbehaves near the bots, try `ai` or `off`. The setting applies to the matches that start after the server starts.

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
