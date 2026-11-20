# Building from source

ORPHEUS has no ready-made download. You build it from the source code on your own PC. The build makes two files in `target\release`:

- `orpheus.exe`, the program;
- `owwfd_relay.dll`, the relay that `orpheus` loads into the game.

They must stay in the same folder. `orpheus` only loads the relay that is next to it.

## What you need

1. **Windows.** The game, the launcher and the relay work only on Windows. The server code also builds on other systems, but it cannot start the game there.
2. **Microsoft C++ Build Tools.** Install [Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/) and select the workload **Desktop development with C++**. It gives the MSVC linker and the Windows SDK, which Rust needs.
3. **Rust with the MSVC toolchain.** Install it from <https://rustup.rs>. On Windows the default toolchain is `stable-x86_64-pc-windows-msvc`. Rust 1.85 or newer is needed.
4. **Git**, to get the source.

You do not need Node.js or `just` to play. Developers need them; see [testing](../dev/testing.md).

## Build

```powershell
git clone <repository> ORPHEUS
cd ORPHEUS
cargo build --release --locked --workspace
```

`--workspace` is important: without it, Cargo builds `orpheus.exe` but not the relay DLL.

The first build downloads the Rust crates and takes some minutes. Later builds only compile what changed. The build is the only time anything is downloaded; `orpheus` itself never uses the internet.

`START.cmd` runs the same build and then starts `orpheus` (see [playing](playing.md)). If the build is current, it does nothing.

## Update

```powershell
git pull
cargo build --release --locked --workspace
```

Your profiles and settings are in the state folder, not in the checkout, so an update keeps them.

## Build problems

| Message | Fix |
| --- | --- |
| `linker 'link.exe' not found` | Install the Build Tools with **Desktop development with C++**, then open a new terminal. |
| `the lock file needs to be updated but --locked was passed` | Your checkout is not clean. Run `git status` and restore `Cargo.lock`. |
| `package requires rustc 1.85 or newer` | Run `rustup update`. |
| `orpheus.exe` cannot be written | `orpheus` is still running. Close it, then build again. |

Next: [first run](first-run.md).
