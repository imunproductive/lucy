# Doctor and troubleshooting

## orpheus doctor

```powershell
target\release\orpheus.exe doctor
```

`doctor` checks what the modes need. It takes the same settings as `serve` (`--game`, `--state`, the port flags, `config.toml`). It prints one line for each check:

```
ok    config   C:\Users\you\AppData\Roaming\orpheus\config.toml not present, using defaults
ok    state    C:\Users\you\AppData\Roaming\orpheus
ok    data     C:\Users\you\AppData\Roaming\orpheus\cache\104319 (Practice Range ready)
ok    account  Player (saved profile)
ok    ports    lobby 3724, stall 1119, rpc 21119, dashboard 3725 (TCP), game 3730 (UDP) are free
ok    client   C:\Games\Overwatch\_retail_\Overwatch.exe (1.74.0.0.104319, language enUS)
ok    relay    D:\ORPHEUS\target\release\owwfd_relay.dll
```

A problem shows `FAIL`, what is wrong, and a line `fix:` with what to do. The exit code is 0 when every check passes, else 1.

`doctor` changes nothing in the state folder, with one exception: it derives the game data if the cache is not ready. If no game is known it says so; it doesn't open the file dialog.

Run `doctor` while no server runs. A running server holds the ports, so the `ports` check fails.

## Every check and its messages

### config

| Result | Meaning and fix |
| --- | --- |
| `ok` `<path>` | `config.toml` was read. |
| `ok` `<path> not present, using defaults` | There is no `config.toml`. That is fine. |
| `FAIL` `<path>: <TOML error>` <br> fix: `correct config.toml; its keys are account, game, locale and [ports]` | The file is not valid TOML, or has an unknown key. See [configuration](configuration.md). When this check fails, `doctor` stops, because every other check needs the settings. |

### state

| Result | Meaning and fix |
| --- | --- |
| `ok` `<folder>` | The state folder can be written. |
| `FAIL` `<folder> is not writable: <error>` <br> fix: `choose another folder with --state or fix its permissions` | `orpheus` cannot create or write the folder. Fix the folder's permissions, or use `--state <folder>`. |

### data

| Result | Meaning and fix |
| --- | --- |
| `ok` `<cache> (Practice Range ready)` | The game data is derived and loads. |
| `FAIL` `cannot read game data from the install: <error>` <br> fix: `point game at an Overwatch 1.74.0.0.104319 install (--game or config.toml)` | The install could not be read. The error says why: for example, the game is another build, it has no `.build.info`, or its files are damaged. |
| `FAIL` `<cache> is missing <files>` <br> fix: `delete the folder so the next start derives it again` | The cache is incomplete. Delete the folder that the message names. |
| `FAIL` `<cache> has no Practice Range movement data` <br> fix: same | The cache has no movement data for the Practice Range. Delete the folder. |
| `FAIL` `<cache> cannot load the Practice Range: <error>` <br> fix: same | The cache is damaged. Delete the folder. |

### account

| Result | Meaning and fix |
| --- | --- |
| `ok` `<name> (saved profile)` | The game logs in as this saved account. |
| `ok` `<name> (created from the template on first login)` | The name has no profile yet. It is made on the first login. |
| `ok` `a new profile from the template on first login` | No account is chosen. The first login makes one. |
| `FAIL` `cannot read the saved profiles: <error>` <br> fix: `repair or remove the broken file in the profiles folder, or import them with orpheus state import` | A file in `profiles\` cannot be read, or `--account` is empty. Repair or remove the file the error names. |

### ports

| Result | Meaning and fix |
| --- | --- |
| `ok` `lobby 3724, stall 1119, rpc 21119, dashboard 3725 (TCP), game 3730 (UDP) are free` | All ports are free on `127.0.0.1`. |
| `FAIL` `in use: <name> TCP <port>, ...` <br> fix: `stop the other server, or choose free ports with the --*-port flags or [ports] in config.toml` | Another program uses these ports. Often it is another `orpheus` or an old Python server. Stop it, or choose other ports. |

### client

| Result | Meaning and fix |
| --- | --- |
| `ok` `<Overwatch.exe> (1.74.0.0.104319, language <code>)` | The game is found and has the right build. |
| `ok` `not chosen yet; the modes that start the game ask for it and remember it` | No game is known yet. |
| `FAIL` `<path> is build <version>, not 1.74.0.0.104319` <br> fix: `point game at the 1.74.0.0.104319 install with --game or game = '<path>' in config.toml` | The game is another version. ORPHEUS works only with 1.74.0.0.104319. If the version is `unknown`, the `.build.info` file is missing or cannot be read. |

### relay

| Result | Meaning and fix |
| --- | --- |
| `ok` `<path>\owwfd_relay.dll` | The relay is next to `orpheus.exe`. |
| `FAIL` `<path>\owwfd_relay.dll is missing; build it with cargo build --workspace` <br> fix: `run cargo build --workspace (orpheus play loads it)` | Build with `--workspace`: `cargo build --release --locked --workspace`. `START.cmd` does this. |

## Other problems

| Problem | What to do |
| --- | --- |
| `No game selected. Start again and pick Overwatch.exe.` | You closed the file dialog. Start again and pick `_retail_\Overwatch.exe`, or give `--game`. |
| `Cannot find the game there. Select _retail_\Overwatch.exe.` | The file you picked is not the game. Pick `Overwatch.exe` in the `_retail_` folder, not a shortcut. |
| `<path> has no .build.info next to it` | The path is not inside an install. Give the install folder or its `Overwatch.exe`. |
| The game shows a black screen at start | The game language is not installed. Set `locale` to a language the install has, or to `auto`. |
| The relay does not load before the timeout | The game was slow to start. Use `--timeout 60`. Look in `logs\relay.log` and `logs\game.log`. |
| The first start takes minutes | The game data is derived once (about two minutes). Later starts are fast. |
| A new map loads slowly the first time | Its collision is built at the first match on it. |
| `the option ... goes after the command, or use it without one` | Write options after the command: `orpheus play --game ...`, not `orpheus --game ... play`. Only `--state` can go anywhere. |
| `No answer to the question. Give it as an option instead, see orpheus --help.` | `orpheus` asked a question but no one could answer (no console). Give the answer as an option, for example `--server` or `--name`. |
| `the Overwatch 1.74 client runs on Windows only` | The modes that start the game need Windows. `serve` and `server` work on other systems. |
| Something else | Read `logs\orpheus.log` in the state folder. |

For problems when joining, see [hosting and joining](hosting.md#join-messages).

## Debugging switches

These environment variables are for research. Leave them unset to play.

| Variable | What it does |
| --- | --- |
| `ORPHEUS_DEBUG_DROP_NOTICES` | A comma list of combat notices the server leaves out: `hit`, `kill-flag`, `elimination`, `death` |
| `ORPHEUS_DEBUG_BOT_BODY=<hex GUID>` | Creates every training bot with this body definition in place of its own (`1403`/`14ED`), for when your own bullets still go through the bots: try `04000000000014D2`, the body whose bots took bullets in ow1-preservation's tests. The server still runs the bots' own scripts. It logs a warning when it is set |
| `ORPHEUS_DEBUG_HELLO_FLAGS=1` | The lobby's hello message (20500) sends its six unknown flags true, to look for hidden admin or developer features |
| `ORPHEUS_DEBUG_SEND=1` | The dashboard gets two routes that send a game link or lobby message with chosen field values to a connected game, to find which of the client's code reads a field (see [marker messages](#marker-messages)). It logs a warning when it is set. Without it the routes do not exist (404) |

### Marker messages

With `ORPHEUS_DEBUG_SEND=1`, the dashboard (`http://127.0.0.1:3725`) answers:

- `GET /api/debug/messages?id=25000`: the message's fields in the order of its schema, as `{"id", "link": "game" or "lobby", "name": "<protocol CRC>/<id>", "fields": [{"index", "name", "off", "type"}]}`. A field's name is `+0x<off>`; a struct field lists its members the same way in `"fields"`. Types: `bool`, `u8` to `u64`, `i8` to `i64`, `f32`, `f64`, `string`, `blob`, `struct`; `[]` marks a counted array, `[N]` a fixed one.
- `POST /api/debug/send` with a JSON body `{"id": 25000, "account": "Name", "fields": {"#0": 1, "+0x7C": "self"}, "count": 1}`. Only `id` is required. `account` is a connected account; without it, the only connected account. `fields` names a field by `+0x<off>` or `#<index>`, a struct member by a dotted path (`#1.#0`). Fields not given are zero or empty. Values: integers (numbers, or text in decimal or `0x` hex), floats, `true`/`false`, text for strings, hex text for blobs, a list for an array, an object of members for a struct. For integers, `"self"` is the account's hero body entity and `"controller"` its player entity; a struct with one integer member, such as an entity reference, takes the value directly. `count` (1 to 64) sends the message that many times. A game link message goes to the account's game in its match, as the server's own reliable messages; a lobby message goes through the account's lobby session. The answer is `{"sent", "link", "id", "account", "fields": [{"index", "name", "off", "value"}]}` with every field's value as sent, or `{"error": "..."}`: an unknown id or field, a wrong value, an account that is not connected, or one that is not in a match (game link messages, and `"self"` or `"controller"`).

```powershell
$env:ORPHEUS_DEBUG_SEND = 1; orpheus serve
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:3725/api/debug/send -ContentType application/json -Body '{"id": 25004, "fields": {"#0": 2, "#1": "self"}}'
```
