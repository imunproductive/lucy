set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# List the recipes.
default:
    @just --list

# Everything a change must pass.
check: fmt-check lint test dashboard-test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Tests run with cargo-nextest (.config/nextest.toml): every crate's tests in one pool, each in its own
# process; then the doc tests, which nextest doesn't run.
test:
    cargo nextest run --workspace --locked -j $env:RUST_TEST_THREADS; $units = $LASTEXITCODE; cargo test --workspace --locked --doc; exit [Math]::Max($units, $LASTEXITCODE)

dashboard-test:
    node --test web/tests/test_dashboard_client.mjs

build:
    cargo build --workspace --release --locked

# Every recipe builds and tests with a quarter of the PC's CPU threads (4 on a 16-thread PC; cargo's
# default is all of them): other agents and the user share the machine. The game-install tests also
# need the cap for memory: each keeps a game server thread busy in real time beside its own and holds
# about 0.7 GB, so one per CPU thread ran a 16-thread, 16 GB PC out of memory and its tests' waits for
# datagrams timed out. A `CARGO_BUILD_JOBS` / `RUST_TEST_THREADS` already set in the environment wins
# (when the PC is free).
export CARGO_BUILD_JOBS := env_var_or_default("CARGO_BUILD_JOBS", `[Math]::Max(1, [Math]::Floor([Environment]::ProcessorCount / 4))`)
export RUST_TEST_THREADS := env_var_or_default("RUST_TEST_THREADS", CARGO_BUILD_JOBS)

# Ignored tests that read an Overwatch 1.74 install or the game data derived from it, without the slow group; then the serial_ tests, one at a time.
test-game game:
    cargo build -p orpheus-relay --locked; $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo nextest run --workspace --exclude orpheus-launcher --locked --cargo-profile gametest --run-ignored only -j $env:RUST_TEST_THREADS -E 'not (test(/datatool/) | test(/matches_the_pack/) | test(/dpk_archive/) | test(/(^|::)slow_/) | test(/(^|::)serial_/))'; $parallel = $LASTEXITCODE; cargo nextest run --workspace --exclude orpheus-launcher --locked --cargo-profile gametest --run-ignored only -j 1 -E 'test(/(^|::)serial_/) & not test(/(^|::)slow_/)'; exit [Math]::Max($parallel, $LASTEXITCODE)

# The game-install tests that play a whole match or run over a minute: their names start with slow_.
test-slow game:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo nextest run --workspace --exclude orpheus-launcher --locked --cargo-profile gametest --run-ignored only -j $env:RUST_TEST_THREADS -E 'test(/(^|::)slow_/)'

# The game-install tests of one crate whose names contain the filter (while working on a feature).
test-named game crate filter profile="dev":
    $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo nextest run -p {{crate}} --locked --cargo-profile {{profile}} --run-ignored only -j $env:RUST_TEST_THREADS {{filter}}

# The tests of the crates a change can affect (tools/affected.py), game-install tests included: an
# agent's own check before reporting. Merges run `test` and `test-game`.
test-affected game:
    cargo build -p orpheus-relay -p orpheus-probe --locked; $env:ORPHEUS_GAME_DIR = '{{game}}'; $crates = (python tools/affected.py --cargo-args) -split ' '; if ($crates.Count -lt 2) { 'no crate affected'; exit 0 }; cargo nextest run @crates --locked --run-ignored all -j $env:RUST_TEST_THREADS -E 'not (test(/datatool/) | test(/matches_the_pack/) | test(/dpk_archive/) | test(/(^|::)slow_/) | test(/(^|::)serial_/))'

# Ignored tests that derive the runtime pack's files from an install and compare them with the pack.
test-assets game runtime:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; $env:ORPHEUS_RUNTIME_DIR = '{{runtime}}'; cargo test -p orpheus-assets --release --locked -- --ignored matches_the_pack

# Ignored tests that compare the asset reader with a DataTool XML export of the install.
test-datatool game dump:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; $env:ORPHEUS_DATATOOL_DUMP = '{{dump}}'; cargo test -p orpheus-stu --release --locked -- --ignored datatool

# Ignored tests that run the Workshop parser and engine over the Doomfist parkour script archive, and
# C32S2 in a simulated match and in a match the game service hosts (optimised).
test-dpk game archive:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; $env:ORPHEUS_DPK_ARCHIVE = '{{archive}}'; cargo nextest run -p orpheus-workshop -p orpheus-sim -p orpheus-server --locked --cargo-profile gametest --run-ignored only -j $env:RUST_TEST_THREADS dpk_archive

# Windows-only tests that load the relay and probe DLLs into a stand-in process.
test-windows:
    cargo build -p orpheus-relay -p orpheus-probe --locked; cargo test -p orpheus-launcher --locked -- --ignored
