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
    $env:ORPHEUS_RUNTIME_DIR = '{{runtime}}'; $env:ORPHEUS_BNET_DESCRIPTORS = '{{runtime}}\bnet\descriptors.pb'; cargo test --workspace --locked -- --ignored
