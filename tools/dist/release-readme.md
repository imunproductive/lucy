# Lucy {version}

Lucy is a server for the Overwatch client of build **1.74.0.0.104319**. It runs the Battle.net login, the lobby and the matches on your PC, so you can play alone or host games for friends. You need your own copy of that game install; Lucy contains no game files. Lucy runs on Windows.

## The files

| File | What it is |
| --- | --- |
| `lucy.exe` | The program: server, game launcher and dashboard |
| `owwfd_relay.dll` | The relay that `lucy.exe` loads into the game. Keep it next to `lucy.exe`. |
| `START.cmd` | Runs `lucy.exe`, which asks which mode to start |
| `START_SERVER_LOCAL.cmd` | Runs `lucy.exe serve`: the services only; you start the game yourself |
| `lucy_relay.py` | The relay without the DLL, for Python 3.8 or newer (64-bit). The top of the file tells how to use it. |
| `docs\` | The user guide. Start with `docs\README.md`. |
| `LICENSE`, `CREDITS.md`, `THIRD-PARTY-NOTICES.md` | The license, the credits and the notices of the projects Lucy contains code from |
| `SHA256SUMS.txt` | The SHA-256 of every file here |

## First start

1. Unzip `lucy-{version}.zip` into a new folder you can write to, for example `C:\Lucy`. Do not use `Program Files`. The files have no folder of their own in the zip.
2. Open a terminal in that folder and check your setup. Use the folder of your game install:

   ```powershell
   .\lucy.exe doctor --game C:\Games\Overwatch
   ```

   Each `FAIL` line tells you how to fix it. The first check derives the game data from your install; this takes about two minutes, once.
3. Play alone: `.\lucy.exe play --game C:\Games\Overwatch`. Or double-click `START.cmd` and choose a mode (Enter plays alone); it uses the game you gave last time.

Lucy keeps your accounts, settings and logs in `%APPDATA%\lucy`, not in this folder. To update, unzip the new version and use it instead of this one: your accounts and settings stay.

## Hosting

To host for other PCs, choose mode 3 (host and play) or 4 (server only), or run `.\lucy.exe host`. Players on the internet need TCP port 3724 and UDP port 3730 forwarded to your PC, and your public address in `--game-host`. Each player needs their own game install and this version of Lucy: `.\lucy.exe join --server <address>:3724 --name <name>`. See `docs\hosting.md`.

## More

- `docs\first-run.md`: the game install, the game data and the state folder.
- `docs\playing.md`: the modes, custom games, Workshop scripts (such as the Doomfist parkour maps) and the dashboard.
- `docs\troubleshooting.md`: every `doctor` message and common problems.
- `docs\commands.md`: every command and option.

The user guide also describes the build from source (`docs\building.md`); this download does not need it. Where the guide shows a `.cmd` file building Lucy, the files here only start it.
