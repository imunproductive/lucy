# Lucy

**Lucy** is a private server for Overwatch 1, written in Rust. It runs the lobby, the game servers, a local Battle.net and a web dashboard for the 1.74 client (build 1.74.0.0.104319), using the game data from your own installation.

> *Unofficial fan project, not affiliated with or endorsed by Blizzard Entertainment. You need your own copy of the game.*

Lucy was originally forked from [ORPHEUS](https://github.com/leafall903/ORPHEUS) by leafall903 and contributors, and has since been largely rewritten. Everything it builds on is credited in [CREDITS.md](CREDITS.md).

## What works

Compared with retail Overwatch 1.74:

| Area | Feature | Status | Notes |
|---|---|---|---|
| **Lobby** | Login, main menu, profile and career | Works | |
| | Loot boxes, collection, shop, events | Works | Edit any profile from the dashboard |
| | Friends, groups, chat, whispers | Works | |
| | Custom games | Works | Browser, create, join, teams, start, share codes; spectating a match not yet |
| **Modes** | Practice Range (Training Bots, Training Partner) | Works | |
| | Practice vs AI | Works | AI heroes on both teams |
| | Assault | Works | Runs on the game's own mode graph, with the end-of-match cards |
| | Escort, Hybrid | Partial | Payload runs on a stand-in; moving to the game's payload graph |
| | Control, Elimination, Deathmatch, Team Deathmatch | Partial | Not checked in the game |
| | Workshop scripts | Works | Full rule engine; Doomfist parkour playable start to finish |
| | Play of the Game, replays | Not yet | |
| **Gameplay** | Movement matching the client's prediction | Works | Frame-exact for the heroes checked |
| | Abilities | Partial | The heroes' own scripts run on the server. Checked in detail: Doomfist, Soldier: 76, Reinhardt, Ana, Lúcio, Widowmaker |
| | Hit registration | Works | On the client's own hitboxes |
| | Damage, healing, deaths, respawns, ultimate charge | Works | |
| | Kill feed, death cam, scoreboard | Works | |
| | Map objects (cars, consoles) | Works | Spawn doors not yet |
| **Hosting** | Play on your own PC | Works | |
| | Host for other players (`lucy host`, `lucy join`) | Works | |
| | Relay | Works | `owwfd_relay.dll`, or the Python script `lucy_relay.py` |
| | Host dashboard | Works | Live matches, chat, system messages, kick, match controls |

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
