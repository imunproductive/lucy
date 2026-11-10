# ORPHEUS

A server for Overwatch 1 retail **1.74.0.0.104319**.

This fork is being restructured. The plan, the target layout and the progress are in [docs/REFACTOR.md](docs/REFACTOR.md). Research and behavior notes are in [docs/notes](docs/notes).

## Requirements

- Stable Rust with the MSVC toolchain
- [just](https://github.com/casey/just)
- Node.js, for the dashboard test

## Check

```powershell
just check
```

This runs `cargo fmt --check`, `cargo clippy` with warnings denied, `cargo test` and the dashboard test. `just` lists the other recipes. Tests that need a prepared runtime pack are ignored by default; run them with `just test-runtime <pack folder>`.

## Run

The native server serves the solo Practice Range on loopback. It needs a prepared runtime pack and a state folder for profiles:

```powershell
cargo run --release --bin orpheus_server -- --runtime <pack folder> --data-dir <state folder>
```

The frozen Python server is still the reference for everything else. `START_SERVER_LOCAL.cmd` runs it without the game. `START_LOCAL.cmd` also starts the client, which needs the relay DLL built with `relay\build.bat` from an x64 Native Tools Command Prompt for Visual Studio.

## License

MIT, see [LICENSE](LICENSE). Upstream projects are listed in [CREDITS.md](CREDITS.md).
