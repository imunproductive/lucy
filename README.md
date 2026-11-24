# Lucy

**Lucy** is a private server for Overwatch 1, written in Rust. It runs the lobby, the game servers, a local Battle.net and a web dashboard for the 1.74 client (build 1.74.0.0.104319), using the game data from your own installation.

> *Unofficial fan project, not affiliated with or endorsed by Blizzard Entertainment. You need your own copy of the game.*

Lucy was originally forked from [ORPHEUS](https://github.com/leafall903/ORPHEUS) by leafall903 and contributors, and has since been largely rewritten. Everything it builds on is credited in [CREDITS.md](CREDITS.md).

## Why

Overwatch 1 was shut down in 2022 and replaced by Overwatch 2, and the game it was can no longer be played. Lucy is an attempt to preserve it: to keep the last version, 1.74, playable as it was, for the people who remember it and for anyone who wants to study it. It is non-commercial, and it ships no Blizzard content.

## Compared with retail Overwatch 1.74

| Area | Feature | Status | Notes |
|---|---|---|---|
| **Lobby** | Login, main menu, profile and career | Works | |
| | Loot boxes, collection, shop, events | Works | |
| | Friends, groups, chat, whispers | Works | |
| | Custom games | Works | Browser, create, join, teams, start, share codes; spectating a match not yet |
| **Modes** | Practice Range (Training Bots, Training Partner) | Works | |
| | Practice vs AI | Works | AI heroes on both teams |
| | Assault | Works | Runs on the game's own mode graph, with the end-of-match cards |
| | Escort, Hybrid | Partial | Payload runs on a stand-in; moving to the game's payload graph |
| | Control, Elimination, Deathmatch, Team Deathmatch | Partial | Not checked in the game |
| | Workshop | Works | Full rule engine; Doomfist parkour playable start to finish |
| | Play of the Game, replays | Not yet | |
| **Gameplay** | Movement matching the client's prediction | Works | Frame-exact for the heroes checked |
| | Abilities | Partial | The heroes' own scripts run on the server. Checked in detail: Doomfist, Soldier: 76, Reinhardt, Ana, Lúcio, Widowmaker |
| | Hit registration | Works | On the client's own hitboxes |
| | Damage, healing, deaths, respawns, ultimate charge | Works | |
| | Kill feed, death cam, scoreboard | Works | |
| | Map objects | Works | Moving platforms, boats and cars, which you can ride; spawn doors and lifts don't open or move yet |

## What Lucy adds

- **Everything on your PC:** the lobby, the game servers and a local Battle.net, started with one command (`lucy play`).
- **Hosting for friends:** `lucy host` on your PC, `lucy join` on theirs. The game reaches the server through a relay: `owwfd_relay.dll`, or the Python script `lucy_relay.py` instead of the DLL.
- **A host dashboard** in the browser: live matches, chat, system messages, kicking players, match controls (time, phases, stopping a match), and editing any account's profile, currencies, loot boxes and unlocks.
- **Workshop scripts from files:** **Import Code** in a custom game loads a script saved under its code, and `--workshop` sends every player into one script's match ([Workshop scripts](docs/user/playing.md#workshop-scripts)).
- **Your own game data:** the server reads what it needs from your Overwatch install on first start; nothing is downloaded.

## Getting started

You need your own copy of Overwatch, build 1.74.0.0.104319, on Windows.

### From a release

1. Download the latest `lucy-<version>.zip` from [Releases](https://github.com/imunproductive/lucy/releases) and unzip it.
2. Check your setup: `lucy.exe doctor --game C:\path\to\Overwatch`
3. Play: `lucy.exe play --game C:\path\to\Overwatch`

### From source

1. Install Rust (MSVC toolchain) and the Microsoft C++ Build Tools ([details](docs/user/building.md)).
2. `git clone https://github.com/imunproductive/lucy.git` and `cd lucy`
3. `cargo build --release --locked`
4. `target\release\lucy.exe play --game C:\path\to\Overwatch`

The first start reads the game data from your install, which takes a few minutes. To host for others or join someone, see [Hosting](docs/user/hosting.md). To play Workshop scripts such as the Doomfist parkour maps, see [Workshop scripts](docs/user/playing.md#workshop-scripts).

## Documentation

- [User guide](docs/user/README.md)
- [Architecture](docs/dev/architecture.md)
- [Testing](docs/dev/testing.md)
- [Research notes](docs/notes)

## License

MIT, see [LICENSE](LICENSE). Credits: [CREDITS.md](CREDITS.md). Third-party notices: [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

---

<sub>Overwatch and Blizzard Entertainment are trademarks or registered trademarks of Blizzard Entertainment, Inc. in the U.S. and/or other countries. Lucy is an independent, non-commercial fan project. It is not affiliated with, associated with, authorized, sponsored or endorsed by Blizzard Entertainment, Inc. It contains no Blizzard game files or data: you need your own legally obtained copy of Overwatch. All game content remains the property of Blizzard Entertainment, Inc.</sub>
