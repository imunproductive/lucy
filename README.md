# ORPHEUS

A private server for the Overwatch 1 client, version **1.74.0.0.104319**, written in Rust. It runs the lobby, the game server, a local Battle.net and a web dashboard on your PC. It reads the game data from your own Overwatch install. It never downloads anything.

You need your own copy of the game, build 1.74.0.0.104319, on Windows.

## Quick start

1. Build (you need Rust with the MSVC toolchain and the Microsoft C++ Build Tools; see [building](docs/user/building.md)):

   ```powershell
   cargo build --release --locked --workspace
   ```

2. Check the setup:

   ```powershell
   target\release\orpheus.exe doctor --game C:\path\to\Overwatch
   ```

3. Play:

   ```powershell
   target\release\orpheus.exe play --game C:\path\to\Overwatch
   ```

The first start derives the game data from the install, which takes about two minutes. `START.cmd` does the build and then asks which mode to start.

## Documentation

- [User guide](docs/user/README.md): building, first run, playing, hosting and joining, troubleshooting, commands, configuration.
- [Architecture](docs/dev/architecture.md): crates, data flow, the lobby, the game service, the state folder.
- [Testing](docs/dev/testing.md): the test tiers and the `just` recipes. Run `just check` before every change.
- [Data sources](docs/dev/data-sources.md): where each data file comes from.
- [Decisions](docs/dev/decisions.md): conventions, fixed data, decisions, and how the Python tests were retired.
- [Hardcoded values](docs/dev/hardcoded-values.md): game values still kept in code, and whether the install has the real value.
- [Research notes](docs/notes): protocol, behavior and acceptance records, references.

## License

MIT, see [LICENSE](LICENSE). Upstream projects are listed in [CREDITS.md](CREDITS.md).
