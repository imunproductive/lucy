# Testing

## Before every change

```powershell
just check
```

`just check` runs four recipes: `fmt-check`, `lint`, `test` and `dashboard-test`. A change must pass it. It needs no game, no network (except Cargo's crate download) and no prepared data.

You need [just](https://github.com/casey/just) and Node.js in addition to the build tools in [building](../user/building.md). The `justfile` uses Windows PowerShell. Each recipe is a short `cargo` or `node` command, so you can also run it by hand.

## What to run, and when

While you work on a feature, run only the tests that are directly involved:

- `just check` (or `cargo test --workspace` and `just lint`). These are cheap.
- The game-install tests of the area you touched, by name:

  ```powershell
  just test-named C:\Games\Overwatch orpheus-sim body_script
  ```

  The last argument is a name filter, as in `cargo test`. `test-named` uses the debug build, which `cargo test --workspace` has already compiled. For the long match tests of `orpheus-server`, add `gametest` to use the optimised build: `just test-named C:\Games\Overwatch orpheus-server assault gametest`. That build takes about two minutes after a change in `sim`, but the tests then run two to three times faster.

Do not run the full game-install suites (`just test-game`, `just test-slow`) for a feature. The integrator (whoever merges) runs them once, after a batch of merges. An agent does not run them before it reports.

## Adding a test

- Think carefully before you add a test. Add only tests that you are sure are useful.
- If a test of the same code path exists, extend it instead of adding a new one.
- Test the logic at the lowest level that can show the bug. Fast unit tests are best.
- Add a loopback or game-install test only for behaviour that you cannot check below the network: what the client receives, and in what order.
- Never add a test that another test already covers or could cover.
- Test behaviour, not wiring. Delete tests that only check wording or wiring.
- Never weaken a test to make it pass: do not loosen an assertion or change an expected value to match the output.

## Recipes

| Recipe | Command | Use |
| --- | --- | --- |
| `just` | `just --list` | Lists the recipes |
| `just check` | `fmt-check`, `lint`, `test`, `dashboard-test` | Before every change |
| `just fmt` | `cargo fmt --all` | Formats the code |
| `just fmt-check` | `cargo fmt --all --check` | Fails on unformatted code |
| `just lint` | `cargo clippy --workspace --all-targets --locked -- -D warnings` | Clippy; every warning is an error |
| `just test` | `cargo test --workspace --locked` | The public tests |
| `just dashboard-test` | `node --test web/tests/test_dashboard_client.mjs` | The dashboard JavaScript test |
| `just build` | `cargo build --workspace --release --locked` | Release build of `orpheus.exe` and `owwfd_relay.dll` |
| `just test-game <install>` | ignored tests with `ORPHEUS_GAME_DIR`, optimised, without the slow group, side by side; then the `serial_` tests one at a time | Tests that read the install; the integrator runs them after a batch of merges |
| `just test-slow <install>` | ignored `slow_` tests, optimised | The game-install tests that play a whole match or run over a minute; the integrator runs them with `test-game` |
| `just test-named <install> <crate> <filter> [gametest]` | ignored tests of one crate whose names contain the filter; debug build unless `gametest` is given | The game-install tests of the area you work on |
| `just test-assets <install> <pack>` | ignored `matches_the_pack` tests of `orpheus-assets` | Derivations against a prepared pack |
| `just test-datatool <install> <dump>` | ignored `datatool` tests of `orpheus-stu` | The asset reader against a DataTool export |
| `just test-windows` | ignored tests of `orpheus-launcher` | Loads the relay DLL into a stand-in process |

## Test tiers

### 1. Public

`cargo test` (`just test`). Synthetic fixtures only. These tests must pass on a fresh clone with no game installed. `testkit` builds a miniature CASC storage for the `casc` tests.

### 2. Game

Tests that need the Overwatch 1.74 install are marked `#[ignore = "..."]`, so they still show in test lists. Run them with:

```powershell
just test-game C:\Games\Overwatch
```

The recipe builds the relay, sets `ORPHEUS_GAME_DIR` and runs every ignored test except the pack and DataTool comparisons and the slow group. Test code never reads environment variables itself; it asks `testkit`:

- `testkit::game_dir()` returns the install from `ORPHEUS_GAME_DIR`.
- `testkit::runtime_dir()` derives the game data from that install once and keeps it in `target/orpheus-test-data`. Later runs reuse it.

`just test-game` and `just test-slow` build with the Cargo profile `gametest` (in `Cargo.toml`). It optimises every crate as the release profile does, but it keeps the debug assertions and overflow checks. So the VM still refuses statescript content it does not implement, as in `cargo test`. Each game-install test, run alone, takes about half the time of a debug build; the `sim` tests take a third. The build goes to `target/gametest`, apart from the debug build. The first `gametest` build compiles everything once (about 7 minutes); after a change in `sim` it takes about two minutes.

The game-install recipes run the tests side by side, half as many at a time as the PC has CPU threads (`RUST_TEST_THREADS`). Each such test keeps a game server thread busy in real time beside its own, and holds about 0.7 GB; with one test per CPU thread, a PC with 16 threads and 16 GB ran out of memory, and the tests' waits for datagrams timed out. A loopback test binds `127.0.0.1:0`, so the system gives it a free port, and writes only to its own temporary folder (`tempfile::tempdir()`). So the tests don't take each other's ports or files, and they run while a server listens on the default ports. A new test must do the same. Only the derived game data in `target/orpheus-test-data` and its collision caches are shared; a map's collision is built once per process, whichever test asks first.

#### Slow tests

A game-install test that plays a whole match in real time, or runs for more than about 60 seconds in the `gametest` build, is in the slow group:

- Its name starts with `slow_`. `just test-game` skips the names that contain `slow_` (`--skip slow_`); `just test-slow` runs only them. Do not use `slow_` in the name of another ignored test.
- Its `#[ignore]` reason starts with `slow:` and says why it is slow.

```powershell
just test-slow C:\Games\Overwatch
```

#### Serial tests

The load of other tests stretches real time: the game server skips the ticks it falls behind on. A game-install test whose assertion depends on that timing (a window of ticks or milliseconds, or the exact contents of a datagram) is in the serial group:

- Its name starts with `serial_`. `just test-game` runs the other tests first, then the `serial_` tests one at a time (`--test-threads=1`). Do not use `serial_` in the name of another ignored test.
- Its `#[ignore]` reason starts with `serial:` and says which timing it depends on.

Do not lengthen such a window to make a test pass in parallel. A wait that only gives something time to happen, with no timing asserted, is not a window: make it longer instead. `just test-named` runs a `serial_` test together with the others its filter matches; give its full name to run it alone.

### 3. Assets oracle

Each derivation in `crates/assets` is checked against the matching file of a prepared runtime pack, the data the inherited Python server read:

```powershell
just test-assets C:\Games\Overwatch D:\packs\runtime
```

This sets `ORPHEUS_GAME_DIR` and `ORPHEUS_RUNTIME_DIR` and runs the `matches_the_pack` tests in a release build. Where the derived file differs from the pack for a known reason, the test pins the difference and names the cause. [data-sources.md](data-sources.md) lists them. `orpheus assets diff --pack <dir>` does the same comparison from the command line.

### 4. DataTool comparison

The `stu` reader is compared with a DataTool XML export of the same install:

```powershell
just test-datatool C:\Games\Overwatch D:\dumps\datatool
```

This sets `ORPHEUS_DATATOOL_DUMP` and runs the `datatool` tests of `orpheus-stu`. The DataTool settings and revisions are in [data-sources.md](data-sources.md).

### 5. Dashboard

`just dashboard-test` runs `web/tests/test_dashboard_client.mjs` with Node's test runner. It is part of `just check`.

### 6. Windows relay

`just test-windows` builds the relay and loads it into a stand-in process. It does not need the game.

### 7. Client acceptance

Some behavior can only be checked with the original client. These checks are manual. Record each one in [behavior.md](../notes/behavior.md#client-acceptance-records): date, commit, client build and locale, hero, map, inputs, expected and observed result, and the number of runs.

## Debug and release builds

Debug builds refuse statescript content the VM does not implement; release builds tolerate it (see [architecture](architecture.md#unknowns-policy)). A test that runs a hero graph in a debug build shows each gap as a diagnostic.
