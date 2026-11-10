# First run

## 1. The game

You need your own copy of the Overwatch client, version **1.74.0.0.104319**. `orpheus` refuses every other version. It reads the version from the `.build.info` file in the install folder.

Tell `orpheus` where the game is. Any of these works:

- `--game C:\Games\Overwatch` on the command line (the install folder, `Overwatch.exe` or the official launcher);
- the environment variable `ORPHEUS_GAME`;
- `game = 'C:\Games\Overwatch'` in `config.toml` (see [configuration](configuration.md)).

If you give none of these, `orpheus` looks for:

1. the game you picked last time (`game_path.txt` in the state folder);
2. an unpacked copy in the folder `game` under the current folder.

If it still finds nothing, the modes that start the game open a file dialog. Pick `_retail_\Overwatch.exe`. `orpheus` remembers your choice.

## 2. Check the setup

```powershell
target\release\orpheus.exe doctor --game C:\Games\Overwatch
```

`doctor` checks the config, the state folder, the game data, the account, the ports, the game and the relay. Each line starts with `ok` or `FAIL`. Each `FAIL` line has a fix below it. See [doctor and troubleshooting](troubleshooting.md).

## 3. The game data cache

The server reads its game data (heroes, maps, statescript graphs, collision) from your install. The first time, it derives the files it needs and stores them in:

```
%APPDATA%\orpheus\cache\104319
```

This takes about two minutes. Later starts reuse the cache and are fast. `doctor` also derives the data if it is not there yet.

The cache is derived again by itself when the install's build or locale changes, or when a new version of ORPHEUS derives the data differently. If the cache is damaged, delete the folder; the next start derives it again. You can also derive it now:

```powershell
target\release\orpheus.exe assets build
```

This prints the cache folder.

A map other than the Practice Range gets its collision the first time a match on that map starts. That first match loads more slowly.

## 4. The state folder

Everything `orpheus` writes goes to the state folder, `%APPDATA%\orpheus`. Nothing is written into the checkout. Use `--state <folder>` or `ORPHEUS_STATE_DIR` to use another folder.

| File or folder | Content |
| --- | --- |
| `config.toml` | Your settings ([configuration](configuration.md)) |
| `profiles\` | One file per saved account |
| `profile.json` | The template for new accounts |
| `default_account.txt` | The account your game logs in as |
| `game_path.txt` | The game you picked in the file dialog |
| `public_address.txt`, `server_address.txt`, `player_name.txt` | Answers the modes remember |
| `logs\orpheus.log` | The server log. It is kept at 2 MB, with two older files. |
| `logs\game.log`, `logs\relay.log` | Game output and the relay log |
| `cache\104319\` | Game data derived from your install |

## 5. Profiles from an older checkout

The old Python server kept profiles in its checkout. To copy them into the state folder:

```powershell
target\release\orpheus.exe state import C:\path\to\old\checkout
```

This copies `profiles\*.json`, `profile.json` and `default_account.txt`. It keeps files that already exist in the state folder and says so. Add `--overwrite` to replace them.

Without any profile, the first login creates one from the template.

Next: [playing](playing.md).
