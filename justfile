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

test:
    cargo test --workspace --locked

dashboard-test:
    node --test web/tests/test_dashboard_client.mjs

build:
    cargo build --workspace --release --locked

# Ignored tests that read an Overwatch 1.74 install or the game data derived from it, without the slow group.
test-game game:
    cargo build -p orpheus-relay --locked; $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo test --workspace --exclude orpheus-launcher --locked --profile gametest --no-fail-fast -- --ignored --skip datatool --skip matches_the_pack --skip slow_ --test-threads=1

# The game-install tests that run over a minute: their names start with slow_.
test-slow game:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo test --workspace --exclude orpheus-launcher --locked --profile gametest --no-fail-fast -- --ignored slow_ --test-threads=1

# The game-install tests of one crate whose names contain the filter (while working on a feature).
test-named game crate filter profile="dev":
    $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo test -p {{crate}} --locked --profile {{profile}} -- --ignored {{filter}} --test-threads=1

# Ignored tests that derive the runtime pack's files from an install and compare them with the pack.
test-assets game runtime:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; $env:ORPHEUS_RUNTIME_DIR = '{{runtime}}'; cargo test -p orpheus-assets --release --locked -- --ignored matches_the_pack

# Ignored tests that compare the asset reader with a DataTool XML export of the install.
test-datatool game dump:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; $env:ORPHEUS_DATATOOL_DUMP = '{{dump}}'; cargo test -p orpheus-stu --release --locked -- --ignored datatool

# Windows-only tests that load the relay DLL into a stand-in process.
test-windows:
    cargo build -p orpheus-relay --locked; cargo test -p orpheus-launcher --locked -- --ignored
