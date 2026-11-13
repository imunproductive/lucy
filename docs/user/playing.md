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
- start a second game in a window for another account;
- change the training bots' health bar and nameplate while a match runs (the **Bot plate** page, below).

The second game plays the account you choose. The local Battle.net tells the two games apart by the process that owns each connection.

`--dashboard-port 0` (or `dashboard = 0` in `config.toml`) turns the dashboard off.

### Bot plate

The **Bot plate** page is a debugging tool. It looks for the input that makes our Training Bot's health bar look unlike retail's: black outlines around the segments and a small bar (`▌`) before the name. It changes what the server sends about every training bot, in every running match. A change reaches the games at once as an update. Nothing respawns, and you don't restart anything. A bot that respawns gets the changes too. The changes stay until you reset them or stop the server.

In a match you can type the same commands in any chat: `.plate <command>`. The bot in the chat answers with what is in force and with the first live bot's pool parts. `.plate` alone (or `.plate show`) only shows them.

The page shows:

- **Command**: a box for one or more commands, separated by `;`. **Reset** takes back every change.
- **Pool parts**: component 51 (the health pools) as the first live bot sends it now. Change a value and press Enter. An empty box takes that change back. Changed parts are in bold.
- **Nameplate variables**: the inputs of the nameplate graph `0D24`, what each reads now on that bot, and what the graph does with it. Change one the same way.
- **Messages**: a button that sends `0C3C` (the plate on) to every live bot's nameplate. It is the only message the server can send to `0D24`: the graph waits for its other messages only on the client.

The commands:

| Command | What it changes |
|---|---|
| `<array>[.<slot>].<attribute> <value>` | One attribute of a pool part. `<array>` is `health`, `armour` or `shields`; `<slot>` is 0 to 15 (0 if you leave it out). `<attribute>` is `id`, `max`, `now`, `kind`, or one of the four flag bytes `b16`, `b17`, `b18`, `b19`. A part the bot doesn't have is added, with id `256 + field` |
| `<array>[.<slot>].tail <b16>.<b17>.<b18>.<b19>` | The four flag bytes at once, for example `health.tail 0.1.0.1` |
| `<array>[.<slot>] default` | Takes back every change of that part |
| `c51.<48 to 52> <value>` | Component 51's five fields after the pools (`u32`, `u32`, an entity, `u8`, `u8`). We don't send them; once set, a field goes back as 0 |
| `name`, `enabled`, `nameonly`, `offset` `<value>` | The nameplate's own variables: the name text (`#8812`; we send the bot's name), the plate on (`#8807`), the name alone instead of the full plate (`#14680`), the root layout's offset (`#8813`) |
| `meter`, `status`, `dead` `<value>` | The body's variables that `0D24` reads: the name's meter (`#6036`; it shows 1 minus this), the status icon (`#2143`), the dead flag (`#478`; true hides the bar) |
| `antiheal`, `healboost` `<value>` | The body's statuses that every health bar reads (`#8202`, `#8203`): Biotic Grenade sets the first on an enemy (purple outline) and the second on an ally (yellow outline). Neither set is "no status" |
| `entity.<id> <value>`, `plate.<id> <value>` | Any variable of the bot body, or of its nameplate instances |
| `hero <bot, a hero, or default>` | The hero of each bot's stand-in controller (component 75), where the plate takes its hero icon: `bot` (the bot's own, a Training Bot portrait), a hero's index or GUID (`013B` is Ana), `default` (none, as we send) |
| `message <id>` | A game message to every live bot's nameplate, by its `025` index (`0C3C`) or whole GUID. The messages `0D24` waits for only on the client (`0744` and the others) are refused |
| `reset` | Takes back every change |

A variable's value is `null`, `true`, `false`, a whole number, a number with a point, `0x` with 16 hex digits (an asset), or text. Put text in quotes when it reads as one of the others (`name "null"`). `default` as a value takes a change back. When you take a variable back, the bot gets its old value; a variable it never had becomes `null`.

Our bots send one pool part: `health.0`, id `1`, kind `0`, maximum and current health, and flag bytes `0.1.0.1`. Byte 17 set and byte 18 clear draws the part as health; any other pair draws it as over-health, in another colour (your tests with `0.0.0.0` and `1.1.1.1` turned the segments turquoise). Nobody has tested bytes 16 and 19 one at a time. They are the first things to try for the outlines. To find the input, change one thing at a time in the Practice Range and compare the bar with retail's:

1. `.plate hero bot`, then `.plate hero 013B`, then `.plate hero default`: if the `▌` turns into a portrait (the Training Bot's head, then Ana's), it is the hero icon slot, empty because our bots' controllers have no hero.
2. `.plate antiheal true` (the outline should turn purple), then `.plate antiheal default`; `.plate healboost true` (yellow), then `.plate healboost default`. If the outline changes colour, the status slot works and the black is the "no status" outline (bar type `32F4`), coloured by the bot's team.
3. `.plate entity.19452 100`, then `.plate entity.19452 default`.
4. Compare with an enemy player's bar in a PvP match: if it has black outlines too, the cause is in what every body sends, not in the bots.

After each step, look at the segment outlines and at the `▌` before the name.

## Matches with fewer players

By default a match starts only with full teams. `--test-players <n>` starts a match once `n` players search. This helps when you test alone or with few friends.

Add `--test-ai <level>` (`easy`, `medium` or `hard`) to fill the missing players of such a match with AI heroes.

## Practice vs AI

**Training > Practice vs AI** works. The server picks an Assault, Control, Escort or Hybrid map at random. Your group plays on one team, and the server fills both teams with AI heroes at the level you chose. The AI heroes walk to the objective, fight, fall back when they are hurt and respawn. Easy AI heroes deal 25% of their damage, Medium 50% and Hard 100%. The match counts in your profile; the AI heroes get no result.

Next: [hosting and joining](hosting.md).
