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
- **Messages**: buttons that send one game message to every live bot's nameplate.

The commands:

| Command | What it changes |
|---|---|
| `<array>[.<slot>].<attribute> <value>` | One attribute of a pool part. `<array>` is `health`, `armour` or `shields`; `<slot>` is 0 to 15 (0 if you leave it out). `<attribute>` is `id`, `max`, `now`, `kind`, or one of the four flag bytes `b16`, `b17`, `b18`, `b19`. A part the bot doesn't have is added, with id `256 + field` |
| `<array>[.<slot>].tail <b16>.<b17>.<b18>.<b19>` | The four flag bytes at once, for example `health.tail 0.1.0.1` |
| `<array>[.<slot>] default` | Takes back every change of that part |
| `c51.<48 to 52> <value>` | Component 51's five fields after the pools (`u32`, `u32`, an entity, `u8`, `u8`). We don't send them; once set, a field goes back as 0 |
| `name`, `enabled`, `nameonly`, `offset` `<value>` | The nameplate's own variables: the name text (`#8812`; we send the bot's name), the plate on (`#8807`), the name alone instead of the full plate (`#14680`), the root layout's offset (`#8813`) |
| `meter`, `status`, `dead` `<value>` | The body's variables that `0D24` reads: the name's meter (`#6036`; it shows 1 minus this), the status icon (`#2143`), the dead flag (`#478`; true hides the bar) |
| `entity.<id> <value>`, `plate.<id> <value>` | Any variable of the bot body, or of its nameplate instances |
| `message <id>` | A game message to every live bot's nameplate, by its `025` index (`0744`) or whole GUID |
| `reset` | Takes back every change |

A variable's value is `null`, `true`, `false`, a whole number, a number with a point, `0x` with 16 hex digits (an asset), or text. Put text in quotes when it reads as one of the others (`name "null"`). `default` as a value takes a change back. When you take a variable back, the bot gets its old value; a variable it never had becomes `null`.

Our bots send one pool part: `health.0`, id `1`, kind `0`, maximum and current health, and flag bytes `0.1.0.1`. Byte 17 set and byte 18 clear draws the part as health; any other pair draws it as over-health, in another colour (your tests with `0.0.0.0` and `1.1.1.1` turned the segments turquoise). Nobody has tested bytes 16 and 19 one at a time. They are the first things to try for the outlines. To find the input, change one thing at a time in the Practice Range and compare the bar with retail's:

1. `.plate health.b16 1`, then `.plate health.b16 0`.
2. `.plate health.b19 0`, then `.plate health.b19 default`.
3. `.plate health.tail 1.1.0.0`, then `.plate reset`.
4. `.plate c51.51 1`, then `.plate c51.52 1`, then `.plate reset`.
5. `.plate nameonly true` (the name alone, without the full plate), then `.plate nameonly default`.
6. `.plate entity.9824 true`, `.plate entity.8838 true`, `.plate meter 1` and `.plate status true`, one at a time, each followed by `.plate reset`.
7. `.plate name null`: the name then comes from the bot's controller. Then `.plate reset`.
8. Last, `.plate message 0744` (hides the name). `reset` does not take back a message: its effect stays until the bot respawns (kill it).

After each step, look at the segment outlines and at the `▌` before the name.

## Matches with fewer players

By default a match starts only with full teams. `--test-players <n>` starts a match once `n` players search. This helps when you test alone or with few friends.

Add `--test-ai <level>` (`easy`, `medium` or `hard`) to fill the missing players of such a match with AI heroes.

## Practice vs AI

**Training > Practice vs AI** works. The server picks an Assault, Control, Escort or Hybrid map at random. Your group plays on one team, and the server fills both teams with AI heroes at the level you chose. The AI heroes walk to the objective, fight, fall back when they are hurt and respawn. Easy AI heroes deal 25% of their damage, Medium 50% and Hard 100%. The match counts in your profile; the AI heroes get no result.

Next: [hosting and joining](hosting.md).
