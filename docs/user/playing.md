# Playing

## START.cmd

Double-click `START.cmd` in the checkout. It:

1. checks that Rust is installed;
2. builds `lucy.exe` and `owwfd_relay.dll` (`cargo build --release --locked --workspace`), which does nothing if the build is current;
3. runs `target\release\lucy.exe` with the arguments you gave it.

Without arguments, `lucy` asks for the mode:

```
Which mode?
  1  Play in retail mode (default Overwatch mode)
  2  Play in tournament mode (used on LANs by pros)
  3  Host a server for others and play on it
  4  Host a server for others without playing here
  5  Join a server in retail mode (default Overwatch mode)
  6  Join a server in tournament mode (used on LANs by pros)
```

Press Enter for 1. If no console can answer, `lucy` starts mode 1.

You can skip the question with a command, for example `START.cmd join --server 1.2.3.4:3724`, or with `--mode`, for example `lucy --mode tournament`.

If something fails, `START.cmd` tells you to run `target\release\lucy.exe doctor`.

## lucy play

```powershell
target\release\lucy.exe play
```

`play` (also called `retail`, mode 1) is the normal way to play alone on this PC. It:

1. finds the game and checks that it is build 1.74.0.0.104319;
2. closes a copy of this game that is still running from an earlier start (other installs are not touched);
3. derives the game data if the cache is not ready;
4. starts the lobby, the game server, the local Battle.net and the dashboard on `127.0.0.1`;
5. starts the game with `--BNetServer` and the game language, and loads the relay into it.

Keep the console window open while you play. When you close the game or the window, the server stops.

`--timeout <seconds>` (default 30) is how long `lucy` waits for the game to be ready for the relay. A slow PC may need more.

`--account <name>` chooses the saved account the game logs in as. Without it, the account in `default_account.txt` is used. A name without a profile gets a new profile from the template on its first login, if it follows the [rule for player names](hosting.md#player-names).

### Game language

`locale` decides the game language (`--locale` or `config.toml`):

- `auto` (default): the system language if the install has it, else an installed language with the same first two letters, else `enUS`. A game started in a language its install lacks shows a black screen;
- `none`: pass no language; the game uses its own;
- a code such as `enUS`, `zhCN` or `deDE`.

## lucy tournament

```powershell
target\release\lucy.exe tournament
```

Mode 2 starts the server and the game in tournament mode (`--tank_TournamentMode`). The game has the simpler menu and connects to the lobby itself. There is no relay and no Battle.net. Ctrl+C or closing the window stops the server.

## lucy serve

```powershell
target\release\lucy.exe serve
```

`serve` runs the Battle.net, lobby, game and dashboard services on this PC, without starting a game. Use it if you start the game yourself. It runs until Ctrl+C. `START_SERVER_LOCAL.cmd` builds and runs `lucy serve`.

### Relay without the DLL: lucy_relay.py

A game you start yourself needs the relay too. `tools\relay\lucy_relay.py` does what `owwfd_relay.dll` does, without loading a DLL into the game. It is one file that needs only 64-bit Python 3.8 or newer, so you can give it to other players as it is.

```powershell
target\release\lucy.exe serve
python tools\relay\lucy_relay.py D:\Games\Overwatch\Overwatch.exe
```

- If the game runs, the script attaches to it. Else it starts the game frozen with `--BNetServer=127.0.0.1:1119`, prepares the relay and unfreezes it. Without a path it uses `.\Overwatch.exe`.
- A game you start yourself needs `--BNetServer=127.0.0.1:1119` too. If it showed "unable to log you in" before the script attached, click **LOGIN**: the new attempt goes through the relay.
- Keep the script open while you play: it relays each new connection the game makes. A connection it relayed keeps working if you close the script.
- Arguments after `--` go to the game: `python lucy_relay.py -- --tank_Locale=enUS`. `--port` and `--host` choose the server's RPC port (default `127.0.0.1:21119`), `--pid` a game when several run, `--log` a log file.

The script writes three small functions (send, receive and "connected") and two copies of the client's vtables into the game's memory, and points the client's Battle.net streams at them. The functions' assembly source is in the script. It changes no code of the game.

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
 [*] Log file:       C:\Users\you\AppData\Roaming\lucy\logs\lucy.log
```

## The dashboard

While the server runs, open <http://127.0.0.1:3725> in a browser. The account list at the top picks the account that the pages show and change. It is also the account that a game logs in as next. The pages are:

| Page | Address | What you can do |
|---|---|---|
| **Profile** | `/` | Edit the profile: nickname, regions, level and portrait frame, currencies, competitive ratings, priority passes, lobby hero, server date and options. |
| **Events** | `/events` | Choose the lobby event and the hero challenge, for this account or for all accounts. |
| **Loot boxes** | `/boxes` | Give loot boxes, or open all of them at once. |
| **Collection** | `/collection` | Buy or unlock items, remove unlocked items, choose the portrait frame. |
| **Server** | `/server` | See the server status. Change the matchmaking, the mode settings, the map and the Workshop script. End the running matches. Choose the default account and the account the game plays. Start a second game for another account. Use the group finder bot. |
| **Live matches** | `/matches` | See every running match (map, mode, phase, time, players) and the lobby's chat, updated each second. Send a message to everyone online, to the players in the menu only, or to the players in matches only. Open a match. |
| **Match** | `/matches/<id>` | See the match's chat (match and team lines) and send lines as the system, to the match or one team. Kick a player, with a reason he sees in the menu. Control the match where its mode allows it: set the time left in this phase, skip the phase, pause and resume, go back to the hero assembly, declare a round's winner, keep the match on at clock 0. Stop the match with a winner or a draw (the result screens play, then everyone goes home) or with no result (everyone goes home now). |

System lines and messages show in the players' chat without a name, in the colour of the channel. The match's chat stays on the page for 30 minutes after the match ends. The controls a match offers depend on its mode: Assault and Skirmish on their own game mode scripts have most of them; Escort, Hybrid and Control offer setting the time and stopping with a winner; the Practice Range has none, but its players can be messaged and kicked.

The button with the sun or moon in the top bar changes between the light and the dark theme. The page keeps your choice. On a narrow window, the menu button opens the page list.

The second game plays the account you choose. The local Battle.net tells the two games apart by the process that owns each connection.

`--dashboard-port 0` (or `dashboard = 0` in `config.toml`) turns the dashboard off.

## Matches with fewer players

By default a match starts only with full teams. `--test-players <n>` starts a match once `n` players search. This helps when you test alone or with few friends.

Add `--test-ai <level>` (`easy`, `medium` or `hard`) to fill the missing players of such a match with AI heroes.

## Practice vs AI

**Training > Practice vs AI** works. The server picks an Assault, Control, Escort or Hybrid map at random. Your group plays on one team, and the server fills both teams with AI heroes at the level you chose. The AI heroes walk to the objective, fight, fall back when they are hurt and respawn. Easy AI heroes deal 25% of their damage, Medium 50% and Hard 100%. The match counts in your profile; the AI heroes get no result.

## Custom games

**Play > Game Browser > Create** makes a custom game with you as its owner. Change the settings as in Overwatch: modes, maps, heroes, lobby, the Workshop. The settings are the game's, and every player in the lobby sees them.

- **Find a game**: **Play > Game Browser** lists the custom games on the server. The filters work. Select a game to see its settings and players, then press **Join**, or join as a spectator. A friend's **Join Game** works too.
- **Invite**: click an empty slot in your lobby, or use **Invite** on a player's card. The player gets an invitation in his chat and accepts or declines it.
- **The lobby**: as the owner, move players between Team 1, Team 2, the spectators and the queue (drag and drop, or a player's menu), swap two players, remove a player, add or edit AI bots, rename the game and the teams, and make another player the owner. A player who joins a full game waits in the queue and takes the first free seat.
- **Start** sends everyone on a team into the match of the first map of the settings that the server hosts, with the settings applied: the mode and hero settings, each team's and each hero's settings, the teams' hero lists and the Workshop rules. AI bots play as AI heroes at the bots' highest difficulty. Spectators and players in the queue stay in the lobby: watching a match is not done yet.
- **During the match**, the owner can go **Back to Lobby**, **Restart** the match, or **Start Game Mode** (end the waiting phase). A player who joins a game in a match goes into the match. Settings you change in a match take effect in the next one.
- **After the match** everyone is back in the lobby. **Leave Game** leaves the custom game. When the owner leaves, the next player owns the game; it closes when the last player leaves.

**Share Code** gives the settings a code of five characters, kept on the server. **Import Code** loads the settings of a code that the server knows: a code someone shared on this server, or a Workshop script you put in the folder `workshop_codes` in the state folder (or a folder that you give with `--workshop-codes`), named by its code: `C32S2.txt` or `C32S2_doomfist_parkour.txt`. The Doomfist parkour archive's `workshop.codes` folder can be used as it is. **Save** and **Load** in the settings keep your own presets on the server.

Next: [hosting and joining](hosting.md).
