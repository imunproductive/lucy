# Playing

## START.cmd

Double-click `START.cmd` in the checkout. It:

1. checks that Rust is installed;
2. builds `orpheus.exe` and `owwfd_relay.dll` (`cargo build --release --locked --workspace`), which does nothing if the build is current;
3. runs `target\release\orpheus.exe` with the arguments you gave it.

Without arguments, `orpheus` asks for the mode:

```
Which mode?
  1  Play in retail mode (default Overwatch mode)
  2  Play in tournament mode (used on LANs by pros)
  3  Host a server for others and play on it
  4  Host a server for others without playing here
  5  Join a server in retail mode (default Overwatch mode)
  6  Join a server in tournament mode (used on LANs by pros)
```

Press Enter for 1. If no console can answer, `orpheus` starts mode 1.

You can skip the question with a command, for example `START.cmd join --server 1.2.3.4:3724`, or with `--mode`, for example `orpheus --mode tournament`.

If something fails, `START.cmd` tells you to run `target\release\orpheus.exe doctor`.

## orpheus play

```powershell
target\release\orpheus.exe play
```

`play` (also called `retail`, mode 1) is the normal way to play alone on this PC. It:

1. finds the game and checks that it is build 1.74.0.0.104319;
2. closes a copy of this game that is still running from an earlier start (other installs are not touched);
3. derives the game data if the cache is not ready;
4. starts the lobby, the game server, the local Battle.net and the dashboard on `127.0.0.1`;
5. starts the game with `--BNetServer` and the game language, and loads the relay into it.

Keep the console window open while you play. When you close the game or the window, the server stops.

`--timeout <seconds>` (default 30) is how long `orpheus` waits for the game to be ready for the relay. A slow PC may need more.

`--account <name>` chooses the saved account the game logs in as. Without it, the account in `default_account.txt` is used. A name without a profile gets a new profile from the template on its first login.

### Game language

`locale` decides the game language (`--locale` or `config.toml`):

- `auto` (default): the system language if the install has it, else an installed language with the same first two letters, else `enUS`. A game started in a language its install lacks shows a black screen;
- `none`: pass no language; the game uses its own;
- a code such as `enUS`, `zhCN` or `deDE`.

## orpheus tournament

```powershell
target\release\orpheus.exe tournament
```

Mode 2 starts the server and the game in tournament mode (`--tank_TournamentMode`). The game has the simpler menu and connects to the lobby itself. There is no relay and no Battle.net. Ctrl+C or closing the window stops the server.

## orpheus serve

```powershell
target\release\orpheus.exe serve
```

`serve` runs the Battle.net, lobby, game and dashboard services on this PC, without starting a game. Use it if you start the game yourself. It runs until Ctrl+C. `START_SERVER_LOCAL.cmd` builds and runs `orpheus serve`.

## The banner

After the start, the log shows where the server listens:

```
OVERWATCH 1.74 LOBBY SERVER
 [*] Bind:           127.0.0.1:3724
 [*] Game server:    127.0.0.1:3730 (UDP)
 [*] Battle.net:     127.0.0.1:1119 (relay RPC 127.0.0.1:21119)
 [*] Dashboard:      http://127.0.0.1:3725
 [*] Accounts:       Player (profiles/)
 [*] New accounts:   copy of profile.json
 [*] Log file:       C:\Users\you\AppData\Roaming\orpheus\logs\orpheus.log
```

## The dashboard

While the server runs, open <http://127.0.0.1:3725> in a browser. The dashboard shows the server status, the matches and the saved accounts. With it you can:

- edit a profile, and apply a change to all profiles;
- give loot boxes, open them, buy items and give skins;
- choose the portrait frame;
- choose the default account and the account a game plays;
- change the matchmaking and mode settings and the map;
- end the running matches;
- start a second game in a window for another account.

The second game plays the account you choose. The local Battle.net tells the two games apart by the process that owns each connection.

`--dashboard-port 0` (or `dashboard = 0` in `config.toml`) turns the dashboard off.

## Matches with fewer players

By default a match starts only with full teams. `--test-players <n>` starts a match once `n` players search. This helps when you test alone or with few friends.

Next: [hosting and joining](hosting.md).
