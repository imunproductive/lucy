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
    node --test tests/test_dashboard_client.mjs

build:
    cargo build --workspace --release --locked

# Ignored tests that need a prepared runtime pack.
test-runtime runtime:
    cargo build -p orpheus-relay --locked; $env:ORPHEUS_RUNTIME_DIR = '{{runtime}}'; cargo test --workspace --locked -- --ignored

# Ignored tests that read an Overwatch 1.74 install.
test-game game:
    $env:ORPHEUS_GAME_DIR = '{{game}}'; cargo test -p orpheus-casc --release --locked -- --ignored

# Windows-only tests that load the relay DLL into a stand-in process.
test-windows:
    cargo build -p orpheus-relay --locked; cargo test -p orpheus-launcher --locked -- --ignored
