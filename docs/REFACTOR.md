# Refactoring plan

Status: proposed, 2026-11-10.

This plan covers the repository layout, the build, the developer workflow, the user entry points, how the server gets game data, the client relay, the removal of the Python server and the replacement of the inherited documentation. This repository is a fork that is being rewritten. Nothing in this plan pushes to, or opens pull requests against, the upstream repository. Gameplay porting continues alongside this plan; after [phase D](#phase-d-replace-the-inherited-documentation), its open items live in `docs/notes/parity.md` and `docs/notes/behavior.md`. This plan changes where that work lives and how it is built and run. It does not change what the work must verify.

## 1. Goals

1. **Rust for everything that ships.** Two exceptions remain: the dashboard front end (JavaScript that runs in the browser) and one IDA research script (it runs inside IDA's Python).
2. **One program.** One `orpheus` binary with subcommands, one config file and one per-user state folder. Nothing is written into the checkout at run time.
3. **No prepared runtime pack.** The server reads game assets from the user's own installation (CASC) at run time. Only the fixed build data in [section 7](#7-fixed-data) stays as data files.
4. **Users build from source and run what they built.** The program never downloads anything. Cargo fetching crates during the build is the only network access.
5. **A layered Cargo workspace.** Crate boundaries enforce the dependency rules.
6. **One local check command,** `just check`, run before every change is accepted.
7. **Behavior is preserved.** After every phase, the existing tests pass and the server still starts.

## 2. Out of scope

- **CI and CD.** Phase 0 disables the workflow. Automated checks and published release binaries are a separate, later plan.
- New gameplay behavior. It continues as separate work (see `docs/notes/behavior.md` after phase D).
- Any client build other than **1.74.0.0.104319**. The code refuses other builds instead of trying to adapt.
- A WebAssembly dashboard. The existing JavaScript front end stays. This drops that item from the inherited migration plan.
- A switch to an async runtime. The `mio` event loop and worker threads stay.
- Rewriting verified logic. Phases 1 and 3 only move and split code. Phase 2 makes the small, listed changes needed to break dependency cycles.

## 3. Starting point

Measured on 2026-11-10 at upstream commit `1010ca4`. The repository was then reinitialized as a local fork. The full upstream history is saved outside the checkout as `../ORPHEUS-upstream.bundle`. Work happens in local checkpoint commits, which are squashed into one initial commit once the refactor is complete. Nothing is pushed.

| Area | Current state |
| --- | --- |
| Rust | 59,268 lines in 99 files, all in one flat `rust/src` folder of one crate. Name prefixes stand in for modules: `game_*` (21 files), `contact_*` (12), `lobby_*` (15), `script_*` (9), `mover_*` (7). `script_init.rs` has 7,539 lines. |
| Rust dependency cycles | `game ↔ script`, `game ↔ match`, `world ↔ vitals`, `runtime ↔ script_graph` and `retail_templates ↔ lobby_*`. Physics and script code import `parse_guid` from `accounts`. |
| Rust tests | 381 `#[test]` functions, 61 of them `#[ignore]`d because they need a prepared runtime. Test code reads `ORPHEUS_RUNTIME_DIR` directly in 54 places across 42 files. |
| Public build | A test fixture does `include_str!("../../data/statescript_codecs_174.json")`. That file is not in the repository, so `cargo test` cannot compile on a fresh checkout. |
| Python | `ow174/` has 38,284 lines (including the dashboard web files). There are 114 test files (29,976 lines) and 22 tool scripts plus one PowerShell script (6,119 lines). 12 tools import `ow174`. |
| Test selection | CI runs 14 hand-picked Python test modules. CONTRIBUTING lists 6 different ones. |
| Entry points | `START.bat`, `START_LOCAL.cmd` and `START_SERVER_LOCAL.cmd` all start the Python server. The Rust server needs `cargo run`, the `ORPHEUS_RUNTIME_DIR` environment variable and `--data-dir`. |
| Local state | `paths.py` writes `profile.json`, `profiles/`, `default_account.txt`, `matchmaking.json`, `game_path.txt`, `server_address.txt`, `player_name.txt`, `public_address.txt`, `logs/` and `cache/` into the checkout root. |
| Relay | The source is `relay/owwfd_relay.cpp`. `relay\build.bat` needs a Visual Studio command prompt. `relay.py` downloads a prebuilt DLL from `squeeeezy/overwatch-1.74-lobby-server`, which currently returns 404. |
| Game data | About 21 prepared files plus collision caches. They are produced by DataTool XML dumps, Python extractors, memory-dump tools and a captured server template. |
| Docs | Every document was written by the upstream authors. 16 files in `docs/` (3 Markdown, 13 plain text), plus README, a Chinese README, CHANGELOG, CONTRIBUTING, CONTRIBUTORS, CREDITS, `WIP.md`, `data/README.md` and the GitHub issue and pull request templates. Status is repeated across README, `RUST_MIGRATION.md`, `STATE.md` and `WIP.md`. `CHANGELOG.md` is 939 lines (124 KB) of daily notes. There is no architecture document. |

## 4. Target

### 4.1 Repository layout

```
ORPHEUS/
├─ Cargo.toml                  # [workspace] only
├─ crates/
│  ├─ core/                    # Guid, bit streams, tick constants
│  ├─ wire/                    # JAM lobby protocol, game link, entity records, input commands, TCP helpers
│  ├─ content/                 # typed game content: catalog, heroes, maps, stats, parameters
│  ├─ bnet/                    # Battle.net protocol, RPC, session keys, service
│  ├─ physics/                 # collision, contact solver, mover, static rays
│  ├─ script/                  # statescript graph, expressions, VM, codecs, script wire, vitals, target tags
│  ├─ sim/                     # match simulation: bodies, bots, combat, projectiles, replication, egress
│  ├─ lobby/                   # accounts, collection, templates, lobby services, match handoff
│  ├─ server/                  # process wiring: game service, connections, ingress, practice handoff, dashboard API
│  ├─ casc/                    # CASC storage and the build-104319 manifest            (phase 6)
│  ├─ stu/                     # STUv2 asset reader with the 1.74 type table           (phase 6)
│  ├─ assets/                  # game install -> content, graphs, collision; cache     (phase 6)
│  ├─ launcher/                # Windows: find, start and inject the client             (phase 5)
│  ├─ relay/                   # Windows cdylib: owwfd_relay.dll                        (phase 5)
│  ├─ fixed/                   # fixed build data, committed and embedded (section 7)
│  └─ testkit/                 # fixtures, synthetic vectors, test client, gated helpers
├─ apps/orpheus/               # the single binary
├─ justfile                    # check, lint, test, test-game, build, ...
├─ web/dashboard/              # existing JS/HTML/CSS, embedded into the binary
├─ research/ida/               # extract_statescript_codecs.py
├─ docs/
│  ├─ notes/                   # facts saved from the inherited docs (phase D)
│  ├─ user/                    # build, play, host, troubleshooting (phase 9)
│  └─ dev/                     # architecture, testing, data sources (phase 9)
├─ README.md, CREDITS.md, LICENSE
└─ START.cmd                   # builds if needed, then runs `orpheus play`
```

During the transition, `ow174/`, `tests/*.py`, `tools/`, `relay/*.cpp`, `requirements.txt` and `pyproject.toml` stay where they are. Phase 8 deletes them.

### 4.2 Crates

| Crate | Responsibility | Comes from (today's `rust/src` modules) | Lines today |
| --- | --- | --- | --- |
| `core` | `parse_guid`, `BitReader`/`BitWriter`, tick constants | `bits`, `guid`, `tick` (the last two created in phase 2) | ~550 |
| `wire` | Byte-exact encodings | `jam_cipher`, `jam_codec`, `jam_framing`, `jam_handshake`, `game_link`, `game_messages`, `game_combat_messages`, `commands`, `world` (renamed `entity`), `service_tcp`, `protocol_tests` | 4,258 |
| `content` | Typed game content and its loaders | `catalog`, `game_content`, `game_heroes`, `game_stats`, `game_parameters` | 2,184 |
| `bnet` | Battle.net emulation | `bnet_protocol`, `bnet_rpc`, `bnet_service`, `bnet_session_key` | 1,264 |
| `physics` | Collision and movement | `collision_cache`, `collision_grid`, `collision_index`, `collision_scene`, `static_ray`, `contact_*` (12), `mover`, `mover_*` (6) | 11,147 |
| `script` | Statescript | `script_graph`, `script_expr`, `script_init` (split up), `script_runtime`, `script_wire`, `script_codecs`, `script_stream`, `script_snapshot_wire`, `script_weapons`, `vitals` | ~16,200 |
| `sim` | Match simulation | `game_body`, `game_bots`, `game_combat`, `game_controller`, `game_hitscan`, `game_pools`, `game_projectiles`, `game_replication`, `game_instance`, `game_egress`, `match_scene`, `scene_assets`, `script_player` (renamed `body_script`), `projectile_flight`, `projectile_wire` | 10,321 |
| `lobby` | Lobby services | `accounts`, `collection`, `retail_templates`, `battle_tag_query`, `match_handoff`, `lobby_*` except `lobby_practice` | 8,015 |
| `server` | Wires everything into running services | `server`, `game_service`, `game_clock` (the `GameClock` type), `game_connection`, `game_ingress`, `lobby_practice`, `runtime` with `runtime/target_tags` (until phase 8) | ~5,500 |
| `casc`, `stu`, `assets` | Reading game data from the install | new (phase 6) | — |
| `fixed` | The fixed data of section 7, embedded with `include_bytes!` | new | — |
| `launcher`, `relay` | Client start and injection | new, ported from `ow174/launcher/` and `relay/owwfd_relay.cpp` (phase 5) | — |
| `testkit` | Test support; dev-dependency only | new | — |
| `apps/orpheus` | CLI | `bin/orpheus_server.rs` and `bin/prepare_server_runtime.rs`, now the `serve` and `runtime prepare` subcommands | — |

`world.rs` is renamed `entity` because it encodes entity create, update and movement records and the datagram limits. It is part of the wire format, not a simulation world.

### 4.3 Dependency rules

An arrow means "depends on". Cargo rejects anything else.

```mermaid
graph TD
  app[apps/orpheus] --> server
  app --> launcher
  app --> assets
  server --> sim
  server --> lobby
  server --> bnet
  assets --> casc
  assets --> stu
  assets --> script
  assets --> physics
  assets --> content
  sim --> script
  sim --> physics
  sim --> content
  script --> physics
  script --> content
  lobby --> content
  lobby --> bnet
  physics --> wire
  content --> wire
  bnet --> wire
  wire --> core
  wire --> fixed
  bnet --> fixed
  script --> fixed
  lobby --> fixed
  casc --> core
  stu --> core
  launcher --> core
```

- `lobby` and `sim` never depend on each other. `server` connects them.
- `relay` depends on nothing in the workspace. `launcher` depends only on `core`.
- Every crate except `launcher` and `relay` keeps `#![forbid(unsafe_code)]`. In those two, every `unsafe` block carries a `// SAFETY:` comment, and `unsafe_op_in_unsafe_fn` is denied.
- `testkit` is only ever a dev-dependency.

### 4.4 How the program runs

```mermaid
flowchart LR
  install[Overwatch 1.74 install] --> casc --> stu --> assets
  assets --> cache[(state/cache/104319)]
  cache --> content
  fixed[Fixed build data] --> content
  content --> server
  config[config.toml + CLI flags] --> server
  state[(state folder: profiles, logs)] <--> server
```

**State folder.** `%APPDATA%\orpheus` on Windows, found with the `directories` crate so the server still builds on other systems. It contains:

| Path | Content |
| --- | --- |
| `config.toml` | Settings (below) |
| `profiles/`, `profile.json`, `default_account.txt`, `matchmaking.json` | Same names and formats as today, so existing profiles keep working |
| `logs/orpheus.log`, `logs/relay.log` | Server log. The launcher passes the relay log path to the game through an environment variable the DLL reads. |
| `cache/104319/` | Content derived from the install, keyed by content key (phase 6) |

**Settings precedence:** CLI flag > `ORPHEUS_*` environment variable > `config.toml` > default.

```toml
game = 'C:\Games\Overwatch\_retail_\Overwatch.exe'
locale = "auto"            # or zhCN, enUS, ...
account = "Player"         # replaces default_account.txt after phase 8
bind = "127.0.0.1"

[ports]
lobby = 3724               # TCP
stall = 1119               # TCP, Battle.net connection the client dials
rpc = 21119                # TCP, relay target
game = 3730                # UDP
dashboard = 3725           # TCP
```

`game_path.txt`, `server_address.txt`, `player_name.txt` and `public_address.txt` become config keys.

### 4.5 Command line

Each command ships together with the feature behind it. There are no placeholder commands.

| Command | Phase | Behavior |
| --- | --- | --- |
| `orpheus serve` | 4 | Battle.net, lobby, game and dashboard services. Stops on Ctrl+C. |
| `orpheus doctor` | 4, extended in 5 and 6 | Checks the config, the state folder, the runtime pack, the login account, free ports, the client path and build, and the relay DLL next to the binary; phase 6 adds the fixed data and the derived cache. Prints one fix for each problem and changes nothing in the state folder. |
| `orpheus state import <old checkout>` | 4 | Copies `profiles/`, `profile.json` and `default_account.txt` from a checkout into the state folder; existing files are kept unless `--overwrite`. |
| `orpheus runtime prepare` | 4 | The former `prepare_server_runtime` binary. Removed in phase 8 with the pack. |
| `orpheus play` | 5 | `serve` plus starting the client (`--BNetServer`, `--tank_Locale`) and injecting the relay built next to this binary. The game's environment tells the relay its log file and the server's RPC port. |
| `orpheus assets diff --pack <dir>` | 6 | Developer check: compares derived content with a prepared pack. |
| `orpheus script query` | 7 | Port of `tools/ssq.py`. |
| `orpheus host`, `orpheus join <address>`, `--tournament` | with their features | LAN or remote hosting and joining. They need bind addresses other than loopback, the local Battle.net emulator for joining, and matchmaking. All are migration work. |

`START.cmd` runs `cargo build --release --locked --workspace`, which also builds the relay and does nothing when the build is current, then runs `target\release\orpheus.exe play %*`. `START_SERVER_LOCAL.cmd` does the same with `serve`.

## 5. Conventions

- **Module names** drop the old prefixes: `game_bots` becomes `sim::bots`, `contact_gjk` becomes `physics::contact::gjk`, `jam_codec` becomes `wire::jam::codec`.
- **File size:** aim for under 1,500 lines. Split by concern and keep tests in separate `tests.rs` or `tests/` modules, not at the bottom of a 7,000-line file.
- **Errors:** library crates keep their typed or `io::Error` errors. Only `apps/orpheus` uses `anyhow`.
- **Logging:** `tracing`, with a file appender in the state folder. No `println!` outside the CLI.
- **CLI parsing:** `clap` with the derive API.
- **Dashboard HTTP:** `tiny_http`. It is synchronous and fits the existing thread model. The JavaScript files are embedded into the binary.
- **Developer tools:** stable Rust with the MSVC toolchain, Node.js (only for the dashboard test) and [`just`](https://github.com/casey/just). The `justfile` uses PowerShell on Windows. Every recipe is a short list of plain `cargo` or `node` commands, so each can also be run by hand.
- **Network:** the program opens only its own listeners and the relay pipe. It never downloads anything.
- **Target build:** `.build.info` is checked at startup, and any build other than 104319 is refused with a clear message.
- **Test tiers:**
  1. **Public:** `cargo test`. Synthetic fixtures only. Must pass on a fresh clone with no game installed.
  2. **Game:** marked `#[ignore = "needs game install"]` so they still appear in test listings. `just test-game` runs them (`cargo test --workspace -- --ignored`). They get the game path only through `testkit`, which reads `ORPHEUS_GAME_DIR` or the `game` key from the user's `config.toml`. Test code never reads environment variables itself. Until phase 6d, `just test-runtime` does the same with the prepared pack in `ORPHEUS_RUNTIME_DIR`.
  3. **Client acceptance:** manual, recorded in `docs/notes/behavior.md`.
- **Public repo data policy:** no client binaries, dumps, captures, keys or prepared data unless decision D1 explicitly allows a fixed-data item.

## 6. Phases

```mermaid
flowchart LR
  P0[0 Groundwork] --> PD[D Replace inherited docs] --> P1[1 Module folders] --> P2[2 Cut cycles] --> P3[3 Workspace crates] --> P4[4 orpheus app] --> P5[5 Launcher and relay]
  P0 --> S[6.0 CASC spike]
  S --> P6[6 Game data from CASC]
  P3 --> P6
  P3 --> P7[7 Tools]
  P5 --> P8[8 Remove Python]
  P6 --> P8
  P7 --> P8
  M[Gameplay parity, docs/notes/parity.md] --> P8
  P8 --> P9[9 New docs]
```

### Progress

| Phase | Status |
| --- | --- |
| 0 Groundwork | Done |
| D Docs | Done |
| 1 Module folders | Done, merged with phase 3: files moved straight into the crates, large test modules moved to `tests.rs` files, the statescript VM split into 11 files by concern |
| 2 Cut cycles | Done, before phase 1 |
| 3 Workspace crates | Done |
| 4 `orpheus` app | Done. The dashboard serves its profile, account, item, frame and loot box actions; the rest answer 501 (see `docs/notes/parity.md`). |
| 5 Launcher and relay | Done. The original client reached the lobby through the Rust relay (`docs/notes/behavior.md`, 2026-11-10). The C++ relay, `build.bat`, `START.bat`, `START_LOCAL.cmd` and `--relay-dll` are deleted. `ow174/launcher` stays until phase 8, because the frozen Python server imports it; it now loads the Rust relay. |
| 6 Game data from CASC | Spike passed. 6a done: `crates/casc` reads every stored graph and definition from the install by GUID (`just test-game`). 6b done: `crates/stu` decodes every stored graph, definition and game mode, and matches a DataTool export of the install (`just test-datatool`). 6c and 6d not started. |
| 7 Tools | IDA script moved; `fake_client` covered by test fixtures; `ssq` deferred (see phase 7) |
| 8 Remove Python | Waits for the phase 5 and 6 gates and gameplay parity |
| 9 New documentation | Not started |

Phases 1 to 3 are mostly mechanical, but they touch every Rust file. Each step lands as a local checkpoint commit. Gameplay work continues in the new layout afterwards. The Python server stays frozen throughout and serves as the behavioral reference until phase 8.

### Phase 0: Groundwork

0. Reinitialize the repository (done 2026-11-10; see [section 3](#3-starting-point)) and record the imported tree as the first local checkpoint commit. `git mv` in phase 1 needs that baseline.
1. Delete `.github/workflows/public-checks.yml`.
2. Remove the relay download: `RELEASE_URL`, `RELEASE_SHA256` and the download path in `ow174/launcher/relay.py`, and the "downloaded by START.bat" comment in `.gitignore`. A missing DLL produces an error that points to `relay\build.bat`.
3. Replace the `include_str!` of `data/statescript_codecs_174.json` in `script_player.rs` with a small synthetic codec table committed under `rust/fixtures/`. It covers only the classes the fixture graph uses.
4. Add a `justfile` with Windows PowerShell as its shell. Its recipes:
   - `check`: `fmt-check`, `lint`, `test` and `dashboard-test`;
   - `fmt-check`: `cargo fmt --all --check`;
   - `lint`: `cargo clippy --workspace --all-targets --locked -- -D warnings`;
   - `test`: `cargo test --workspace --locked`;
   - `dashboard-test`: `node --test tests/test_dashboard_client.mjs`;
   - `test-runtime`: the ignored tests against a prepared pack (until phase 6d).

   Python is not part of `just check`. When a phase changes Python code (phase 0 changes `relay.py`), the affected Python tests are run by hand once.

   Phase D's new README points to `just check`.

**Done when:** `just check` passes on a fresh clone with no game, no runtime pack and no network access beyond Cargo's crate fetch.

### Phase D: Replace the inherited documentation

All existing documentation was written by the upstream authors, and the fork keeps none of it. This phase saves the facts the rewrite needs into a few short notes and deletes everything else. It runs right after phase 0, so every later phase works from the new notes. The deleted files remain in the upstream history bundle (`../ORPHEUS-upstream.bundle`) if something turns out to be missing.

**What to keep:** facts the code or this plan depends on. That means numbers and limits, client addresses and offsets, wire layouts, pinned tool revisions, what each data file is for and how it is made, verified behavior, known defects, acceptance targets, and the legal notices.

**What to drop:** upstream process (contribution lanes, review and audit workflow, AI-disclosure rules, issue forms), status narratives, the daily changelog, conclusions the upstream authors themselves retracted or superseded, and the specs of Python-only research formats.

Each kept fact is rewritten briefly, in our own words. Where it matters, the note says what the fact was observed on (for example "client 104319, offline extraction"). Upstream session names and dates are not kept.

**New notes in `docs/notes/`:**

| Note | Content | Taken from |
| --- | --- | --- |
| `protocol.md` | Datagram and payload limits, the verified raw statescript payload layouts, handshake facts, default ports, relay offsets, client structural evidence | `PACKET_BUDGET.txt`, `RAW_PAYLOADS_174.txt`, `RESEARCH_EVIDENCE.txt` §A–B, the rules in `SIMULATION_CONTRACT.txt` that the Rust worker implements, the port list in `RUST_MIGRATION.md` |
| `data-sources.md` | Each data file: its purpose, how it is derived and from which asset types. The fixed-data list. The pinned OWLib and TACTLib revisions, the DataTool offline settings and the graph bundle hash. | `data/README.md`, `RESEARCH_EVIDENCE.txt` §I, [section 7](#7-fixed-data) |
| `behavior.md` | Acceptance targets M0–M6, behavior seen with the original client, known defects and open questions. Client acceptance records are added here from now on. | `docs/ROADMAP.md`, `docs/STATE.md`, `WIP.md`, the README status table |
| `parity.md` | What the Python server does that the Rust server doesn't yet, plus the cutover criteria. Deleted in phase 8. | `docs/RUST_MIGRATION.md`, the Python CLI modes |
| `references.md` | External projects and notes still worth consulting, one line each on what they're useful for | `RESEARCH_EVIDENCE.txt` §C–H (without the retracted conclusions), `CREDITS.md` |
| `related-projects.md` | Every useful finding from the other 1.74 projects (match flow, AI and navigation, HUD, statescript names, lobby), with source and evidence | The 2026-11-10 survey of the projects in the original `CREDITS.md` and ow1-preservation |
| `game-rules.md` | Match, combat and progression rules from public sources | ow1-preservation's `research_notes/` |
| `ow1-preservation.md` | Every fact from ow1-preservation's notes and protocol layouts (HUD, prediction and reset bit, Recall, Blink, two clients, AI, modes) | Its `HANDOFF.md`, `CLAUDE.md`, `docs/` and `protocol/` |

**Legal files:**

- `LICENSE` stays unchanged. The MIT license requires keeping its copyright and permission notice in copies of the software, and the fork still contains upstream code. You can add your own copyright line.
- `CREDITS.md` is replaced by a short attribution list. It names the upstream projects whose code remains: the Boi-027 lobby research with Arlecchino's copyright, the LizRollin runtime branch, and the authors of the community TLS relay. Third-party code ported later (TACTLib and OWLib, both MIT) is added when it lands. The research thanks and the dependency list are dropped; `Cargo.lock` already lists the dependencies.

**Fate of every inherited document:**

| File | Fate |
| --- | --- |
| `README.md` | Rewritten: what the fork is, how to build, run and check. Phase 9 expands it. |
| `README.zh-CN.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `CONTRIBUTORS.md` | Deleted. The build and check commands move into the new README. The changelog history stays in git. |
| `CREDITS.md`, `LICENSE` | See legal files above |
| `WIP.md`, `docs/ROADMAP.md`, `docs/STATE.md` | Extracted into `behavior.md`, then deleted |
| `docs/RUST_MIGRATION.md` | Extracted into `parity.md`, `behavior.md` and `protocol.md`, then deleted |
| `docs/RESEARCH_EVIDENCE.txt` | Extracted into `protocol.md`, `data-sources.md` and `references.md`, then deleted |
| `docs/PACKET_BUDGET.txt`, `docs/RAW_PAYLOADS_174.txt`, `docs/SIMULATION_CONTRACT.txt` | Extracted into `protocol.md`, then deleted |
| `data/README.md` | Extracted into `data-sources.md`, then deleted |
| `docs/CONTROLLED_SERVER_JOURNAL_V1.txt`, `…_V2.txt`, `BODY_VM_SNAPSHOT_V1.txt`, `PHYSICS_SNAPSHOT_V1.txt`, `ENVIRONMENT_READ_TAPE_V1.txt`, `COLLISION_READ_TAPE_V1.txt`, `LOCAL_REPLAY_MANIFEST_V1.txt`, `NATIVE_BODY_REPLAY_V2.txt`, `SERVER_STORED_STATE_V1.txt` | Deleted without extraction. They specify Python-only research formats that no Rust code reads. |
| `data/replays/README.txt` | Deleted in phase 8 together with `data/replays/basic.json`, the Python replay fixture it describes |
| `.github/ISSUE_TEMPLATE/*` (4 files), `.github/PULL_REQUEST_TEMPLATE.md` | Deleted. Whether and how the fork takes issues and pull requests belongs to the later CI/CD plan. |
| `docs/REFACTOR.md` | Stays as the working plan, with its links pointed at the new notes. Phase 9 turns its lasting parts into `docs/dev/architecture.md` and deletes it. |

**Code comments are not part of this purge.** Some research knowledge exists only in Python and tool docstrings: the CASC layout in `ow174/game/casc.py`, the ENTITY chunk layout in `extract_ai_spawners.py`, and the placeable rules and client addresses in `extract_game_maps.py` and `extract_statescript_codecs.py`. Phase 6 carries it into the doc comments of the Rust code that replaces each tool, and phase 8 checks that before deleting `tools/` and `ow174/`.

**Done when:**
- the only documents left are `README.md`, `LICENSE`, `CREDITS.md`, `docs/REFACTOR.md`, `docs/notes/*` and, until phase 8, `data/replays/README.txt`;
- every link in this plan points to an existing file;
- every fact this plan cites from an inherited document is in a note;
- `git grep -n -E "docs/|docs\\\\|STATE\.md|RUST_MIGRATION|RESEARCH_EVIDENCE|CONTRIBUTING|WIP\.md|ROADMAP|data/README"` finds no reference to a deleted file anywhere in the code or scripts. Today there is one: `START_LOCAL.cmd` points to `docs\STATE.md`. The two Rust references to `CREDITS.md` (`jam_cipher.rs`, `jam_codec.rs`) stay valid because that file is kept.

### Phase 1: Module folders (moves only)

1. Use `git mv` to move `rust/src/*.rs` into folders named after the future crates (`core/`, `wire/`, `content/`, `bnet/`, `physics/`, `script/`, `sim/`, `lobby/`, `server/`), following the table in [4.2](#42-crates). Drop the name prefixes. Rewrite the `use` paths. Keep `pub use` re-exports only inside the commit that needs them, and remove them before the phase ends.
2. Move inline test modules of more than about 300 lines into sibling `tests.rs` files.
3. Split `script_init.rs`:
   - production code (lines 1–4,748, with the VM `impl` at 1,102–4,607) goes into `script/vm/` under `mod.rs`, `component.rs`, `machine.rs`, `events.rs` and `context.rs`, plus one file per group of state kinds;
   - its tests (lines 4,749–7,539) go into `script/vm/tests/`, split by topic.

   Apply the same treatment to the other files over 1,500 lines: `script_snapshot_wire.rs`, `game_replication.rs`, `game_service.rs`, `script_expr.rs`, `script_player.rs`.

**Done when:** the same 381 tests exist, the same 61 are ignored, `just check` passes, and every move was made with `git mv`.

### Phase 2: Cut the dependency cycles (done)

Phase 2 ran before phase 1: cutting the edges first made the later moves pure moves. A scan of every `crate::` path, test modules included, with the 4.2 assignment applied, found the edges below. Each was fixed until the scan reported none. The scan was a one-off script and isn't kept; from phase 3 on, Cargo enforces the rules.

| Cut | Wrong-way edge | Change |
| --- | --- | --- |
| C1 | 11 modules in `physics`, `script`, `content` and `sim` use `accounts::parse_guid` | `parse_guid` moved to the new `guid` module (`core`). All uses import it from there. |
| C2 | `script_runtime`, `script_stream`, `game_bots`, `game_projectiles`, `game_replication`, `projectile_flight` and `scene_assets` use the `game_clock` tick constants | `TICK_PERIOD`, `SEND_EVERY`, `FIRST_TICK` and `MAX_LEASE_FRAME` moved to the new `tick` module (`core`). |
| C3 | `game_clock` uses `game_connection::{HANDOFF_TIME, LEAVE_GRACE}` | After C2 only `game_service` uses `game_clock`, so it goes to `server`. Placement only. |
| C4 | `world::health` calls `vitals::Vitals::with_current_health` (wire to script) | `health()` moved to `vitals`. The `world` test that used it builds the pool component directly. |
| C5 | `script_player` uses more than 20 `game_*`, `match_*` and `mover*` modules | Placed in `sim`. Placement only. |
| C6 | `game_replication` uses `game_egress` | `game_egress` placed in `sim`. Placement only. |
| C7 | `lobby_practice` uses `game_service`, `match_scene` and `scene_assets` | Placed in `server`. Its message identity (`GAME_REQUEST`, `CREATE_GAME`, `is_request`) moved to `match_handoff` (`lobby`), which the lobby tests use. |
| C8 | `script_init` tests use `game_hitscan::HitDefinition` and `projectile_flight::ProjectileBallistics` | The authored damage and ballistics definitions (`HitDefinition`, `DamageFalloff`, `ProjectileBallistics`, `Blast`, `authored_number`) moved to the new `script_weapons` module (`script`). Hit detection and flight stay in `sim`. |
| C9 | `lobby_session` tests start the whole native server; `mover_correction` tests use `game_egress` | The native server test moved to `server`, the correction-and-egress test to `game_egress`. The client side of the lobby handshake became a shared test helper in `jam_handshake`; phase 3 moves it to `testkit`. |

`runtime::target_tags` only depends downward on `script_graph`, so it stays with `runtime`. The `retail_templates ↔ lobby_{arcade,login,passes,transport}` cycle stays inside the `lobby` crate and doesn't block anything.

**Done when:** the scan reports no wrong-way edges and `just check` passes.

### Phase 3: Workspace crates and testkit

1. Turn each folder into a crate under `crates/`. Name each package `orpheus-<name>`. Use workspace-level `[workspace.dependencies]`, lints and `rust-version`.
2. Move `bin/orpheus_server.rs` to `apps/orpheus`. `prepare_server_runtime` became `orpheus runtime prepare` in phase 4.
3. Create `testkit`:
   - one function that returns the prepared-pack path (later the game path) or panics with instructions, replacing the 54 direct environment reads;
   - shared synthetic fixtures;
   - shared builders such as `fixture_body_factory`.
4. Add `just test-runtime` support to `testkit`: the ignored tests find the prepared pack through `testkit` only.

**Done when:** every crate builds and tests on its own (`cargo test -p orpheus-<name>`), and the workspace test count and ignored count match phase 1.

### Phase 4: The `orpheus` app

1. Build the CLI from [4.5](#45-command-line): `serve`, `doctor` and `state import`, using `clap`, `tracing`, the config precedence and the state folder.
2. Add the dashboard backend in `server`:
   - serve the embedded `web/dashboard` files, moved from `ow174/dashboard/web`;
   - implement each route the native server can back with real behavior;
   - routes whose feature isn't ported yet return `501 Not Implemented` with the feature name, and are tracked in the parity list in [section 10](#10-python-retirement).

   The Python server has these routes:
   - `GET`: `/api/state`, `/api/status`, `/api/collection`;
   - `POST`: `/api/apply_to_all`, `update_profile`, `add_boxes`, `open_all_boxes`, `bot_group`, `purchase`, `grant_skin`, `select_account`, `default_account`, `set_frame`, `reconnect`, `start_game`, `matchmaking`, `mode_settings`, `set_map`, `end_matches`.
3. Replace `START_SERVER_LOCAL.cmd` with `orpheus serve`.

**Done when:**
- `orpheus serve` starts from a fresh state folder after `state import`;
- `doctor` reports a deliberately broken setup correctly (wrong port, missing pack, missing profile);
- the dashboard test passes against the embedded files.

### Phase 5: Launcher and relay in Rust

**Launcher** (`crates/launcher`, Windows only). Port `ow174/launcher/`:

- find and validate the client, from config or a file picker;
- detect the locale and pass `--tank_Locale`;
- close a running copy, then start the client;
- inject with `OpenProcess`, `VirtualAllocEx` and `CreateRemoteThread` plus `LoadLibraryW`, resolving the remote export through its owning module as `inject.py` does;
- map TCP connections to process IDs (`GetExtendedTcpTable`) so the Battle.net emulator can tell two clients on one PC apart.

The launcher loads `owwfd_relay.dll` from the same folder as `orpheus.exe`. `cargo build --workspace` builds both into the same output folder, so the launcher injects the DLL from the same build. `orpheus play` checks the install's `.build.info` version and refuses any build other than 104319; `--game` (or `ORPHEUS_GAME`, or `game` in `config.toml`) names the install, and `locale` picks the game language. The second-game helpers (`GetExtendedTcpTable` connection owners for the dashboard's `start_game`) wait for that dashboard action.

**Relay** (`crates/relay`, a `cdylib` built with `windows-sys`). Port `relay/owwfd_relay.cpp` behavior for behavior:

| C++ | Rust |
| --- | --- |
| `DllMain` starts a worker with `CreateThread` | Same, calling `CreateThread` directly. `DllMain` does nothing else. |
| `__try/__except` around reads of arbitrary heap memory (`rd64`, `rd32`, the region scan, the vtable writes) | Rust cannot catch access violations. Read with `ReadProcessMemory(GetCurrentProcess(), …)` into local buffers, region by region, after the same `VirtualQuery` filter (committed, private, read-write, not a guard page, at most 128 MiB). Write the two vtable pointers with `WriteProcessMemory` on the own process. Both return an error instead of crashing the game. |
| Thunks for `send` (slot 3), `recv` (slot 2) and the byte-stream state (slot 1) | `extern "system"` functions. Each body runs inside `catch_unwind` and falls back to the C++ result: `send` reports the full length sent, `recv` reports 0 bytes, `state` returns 1. A panic must never reach the game. |
| Constants: byte-stream vtable RVA `0x298E2F8`, offsets `+0x20`, `+0x48`, `+0x80`, `+0x88`, `+0xA8`, vtable copy of 64 slots, target `127.0.0.1:21119`, 8 ms scan loop, 30 ms `recv` wait | Same values, as named constants with the C++ comments. |
| `CRITICAL_SECTION` plus a fixed 64-entry relay table whose entries are never removed, so byte-streams after the 64th get no relay | `Mutex` around the same 64-entry table. Keep the same behavior until the live test passes, then remove entries when a stream closes, as a separate change. |
| Log at `<dll folder>\log\wfd.log` | Log path from an environment variable set by the launcher, falling back to the old location |
| Header comment mentions "the Go bnet server" and port 1119 | Corrected: the relay pipes to the native Battle.net RPC listener on 21119 |

`cargo build --release` builds `orpheus.exe` and `owwfd_relay.dll` into the same folder.

**Gate:** keep `relay/owwfd_relay.cpp`, `relay/build.bat` and the Python launcher until the Rust DLL passes a live test. The original client must log in through the Rust relay and reach the lobby, recorded in the status doc with commit, build and steps. Until then, `orpheus play --relay-dll <path>` can use the C++ build for comparison. After the gate passes, delete the C++ relay and that flag.

**Done when:** `START.cmd` → `orpheus play` gets the original client into the lobby with the Rust relay, and `START.bat` and `START_LOCAL.cmd` are deleted.

**Result (2026-11-10):** done. `ow174/launcher` is not deleted with the C++ relay as planned: `ow174/lobby/server.py`, `ow174/dashboard/service.py` and `ow174/cli.py` import it, and the Python server stays runnable as the reference until phase 8. It loads `target\release\owwfd_relay.dll` instead.

### Phase 6: Game data from CASC at run time

DataTool has already read everything the pack needs from the local install, fully offline, with online and CDN fallback disabled (`docs/notes/data-sources.md`). This phase reimplements the part of DataTool, TACTLib and the extractors that the server uses. TACTLib and OWLib are MIT-licensed on their default branches. Confirm this for the pinned revisions, and add the ports to `CREDITS.md`.

**6.0 Spike (go/no-go).** In a scratch crate, read one statescript graph (`01B`) and one entity definition (`003`) by GUID from the real install, through the 104319 manifest (TACTLib `ProCMF_104319.cs`, revision `a00459fe`). Compare the bytes with what DataTool exports for the same GUIDs. If the manifest decryption can't be ported, stop phase 6 and keep the prepared pack. Phases 0 to 5 and 7 don't depend on this phase.

**6a. `casc` crate:**
- `.build.info` and config;
- local `.idx` indices and `data.NNN` archives;
- BLTE blocks: stored and zlib, as `casc.py` already handles, plus encrypted blocks if the spike finds any needed asset uses them;
- the encoding table;
- the TRG/CMF manifest for 104319 only.

Public tests use a synthetic miniature storage built by `testkit`.

**Spike result (2026-11-10):** go. TACTLib's `ProCMF_104319` decrypts the install's manifests, and the GUID counts match DataTool's extraction (1,587 `01B`, 3,528 `003`). Encrypted blocks are needed: 8 bundles use them, so the reader handles Salsa20 with the install's keyring. One key is missing from that keyring; TACTLib's static keyring has it, and `docs/notes/data-sources.md` lists what it unlocks.

**6b. `stu` crate:** an STUv2 reader. A one-off generator (`cargo run -p orpheus-stu --bin gen-types`) reads OWLib's `overwatch/1.74` `TankLib/STU/Types` (revision `7afe5f52`) and writes a Rust type table, which is committed. The table is derived from MIT-licensed code, not from game data.

**6b result (2026-11-10):** done. The generator writes a text table (`crates/stu/data/types.txt`: 5,255 classes, 14,194 fields, 512 enums, 2,106 declared defaults) that the crate embeds and parses once, instead of Rust source; that keeps compile times down. The reader fills unstored fields with their defaults, as DataTool shows them. It decodes all 1,586 stored graphs, 3,509 definitions and 25 game modes; 152 graph instances are of classes OWLib doesn't know, which DataTool exports as null too. Against a DataTool XML export of the same install, every graph's item list matches, and all 1,167,080 numbers, nulls and strings DataTool prints on the graph roots, graph items and definition roots are equal.

**6c.0 Classify the inherited files.** No script in the repository produces `extracted_heroes.json`, `extracted_maps.json`, `extracted_items.json`, `extracted_general_unlocks.json`, `collision_keys_174.json` or `collision_props_174.json`. They most likely came from dumps taken while the live servers were still running. Their origin isn't tracked further. Other extractors read them as inputs: `extract_game_heroes.py` and `extract_ai_spawners.py` take hero names from `extracted_heroes.json`, `extract_game_maps.py` takes English map names from `extracted_maps.json`, and `extract_ai_spawners.py` and `build_collision.py` take storage keys from `collision_props_174.json` and `collision_keys_174.json`. Before porting anything in 6c, inspect each file and classify it:

- **derive:** its content can be found in the install's assets. Write a derivation with the existing file as its oracle;
- **not needed:** the manifest makes it unnecessary, which is expected for the two collision key files;
- **fixed:** it can't be reproduced from the install, so it moves to [section 7](#7-fixed-data).

Record the result in `docs/notes/data-sources.md`.

**6c. `assets` crate:** derivations that produce `content`, `script` and `physics` types directly. Each one is checked with `orpheus assets diff` against its existing pack file before the server uses it:

| Output (oracle file) | Source today | Asset inputs |
| --- | --- | --- |
| `extracted_heroes.json`, `extracted_maps.json`, `extracted_items.json`, `extracted_general_unlocks.json` | None in the repository (see 6c.0) | Derived: a port of DataTool's `list-heroes`, `list-maps`, `list-unlocks` and `list-general-unlocks --json` (`crates/assets/src/lists`), identical to DataTool's own output on this install. The pack's maps file has 207 blank or season-less display strings the install doesn't; the test pins them. |
| `game_heroes_174.json` | `extract_game_heroes.py` | `075` plus hero names |
| `game_graphs_174.json.gz` | `extract_game_graphs.py` | `01B`, `003`, `0C5` (mode variables), the AI spawner graphs. Oracle: the normalized SHA-256 `febfbcc8…` recorded in `docs/notes/data-sources.md`. |
| `game_maps_174.json` | `extract_game_maps.py` | `003`, `09F`, `0C5`, raw map chunks `0BC`, map entities, strings |
| `game_queues_174.json` | `extract_game_queues.py` | `0C7`, `0C0`, `0C5`, `039`, `09F`, `0D9`, `0EE`, `0CC`, strings `07C`, plus the captured fields in [section 7](#7-fixed-data) |
| `arcade_cards_174.json` | `extract_arcade_cards.py` | `0C7`, plus the server-set fields in section 7 |
| `game_movers_174.json` | `extract_game_movers.py` | `003` (`STUCharacterMoverComponent`) |
| `game_stats_174.json` | `extract_game_stats.py` | `054`, `075`, `062` |
| `ai_spawners_174.json` | `extract_ai_spawners.py` | Map ENTITY chunks (`0x0B`), `075` |
| `collision_keys_174.json`, `collision_props_174.json` | None in the repository (see 6c.0) | Keys: not needed, the manifest resolves the chunks. Props: only `modes` (which placeables each mode keeps) is read; the `collision_props` derivation writes it from the map ENTITY chunks. |
| Collision `.col` and `.rays.col` caches | `build_collision.py` (with `ow174.game.casc`, `collision`, `props`) | Map collision chunks and prop models. Write the existing `.col` format, so `physics::collision_cache` stays unchanged. |
| Raycast metadata, score events | `extract_raycast_metadata.py`, `extract_game_stats.py --score-events` | `097`, `098`, `054` |

The derivations depend on each other, and are ported and run in this order. An arrow means "is an input to".

```mermaid
flowchart LR
  L[lists: extracted_heroes, maps, items, general_unlocks] --> GH[game_heroes]
  L --> AI[ai_spawners]
  L --> GM[game_maps]
  GH --> GG[graphs: hero and bot bundles]
  AI --> GG
  GH --> MV[game_movers]
  AI --> MV
  GH --> ST[game_stats]
  RT[retail_templates, fixed] --> GQ[game_queues]
  AC[arcade_cards]
  COL[collision, Practice Range]
  CP[collision_props]
```

**6c result (2026-11-12):** done. `crates/assets` has 11 derivations, run in the order above (`DERIVATIONS`). Each is tested against its pack file (`just test-assets`); all match except where the test pins a known difference with its cause: 207 display strings of `extracted_maps.json` (the pack's copy wasn't made from this install's strings), the bytes of `0688.col` (6 more empty models, because the derivation also reads models stored in bundles), and `collision_props_174.json` (only `modes` is written). `orpheus_assets::collision_cache` builds any other map's cache.

**Caching:** results go to `<state>/cache/104319/`, stamped with the build version, build config key, locale and `DERIVATIONS_VERSION`; another stamp derives them again. Collision for a map other than the Practice Range is built when a match on that map first starts. The first launch is slower; later launches read the cache.

**Locale:** read the installed locale, which was zhCN in the recorded extraction. Server logic keys only on GUIDs. Display names are used only for logs and the dashboard.

**6d. Switch:** the server takes `game` from config, and `doctor` checks the build. The Rust server stops reading `ORPHEUS_RUNTIME_DIR`. Game-tier tests switch from the pack to `just test-game`, and `just test-runtime` is removed. The pack format, `orpheus runtime prepare` and the Python extractors stay until phase 8, because the frozen Python server still reads the pack and the pack is the oracle for 6c.

**6e (optional).** Dashboard item previews come from `export_item_previews.py` and DataTool PNG exports. They could later be decoded from CASC textures. This needs a texture decoder and is not required for cutover.

**Done when:** a fresh state folder plus a game install, with no pack, serves the Practice Range path the Rust server serves today, and every derivation matches its oracle file.

### Phase 7: Tools

| Tool | Fate |
| --- | --- |
| `fake_client.py` | Covered: the client side of the lobby handshake is the `client_handshake` fixture in `orpheus-wire` (feature `fixtures`), and the server's loopback test logs in, decodes the login messages and loads the Practice Range with it. Delete in phase 8. |
| `ssq.py` | Port as `orpheus script query` once a graph bundle is available to check it against (phase 6 or a prepared pack). It reads `statescript_names_174.json` (fixed research data) and is 1,440 lines of queries over the Python VM. Until then it stays in `tools/`, and phase 8 waits for it. |
| `extract_statescript_codecs.py` | Moved to `research/ida/` (done). It runs inside IDA. |
| `build_collision.py`, `extract_*.py`, `dump_schemas.py`, `export_casc_records.ps1`, `prepare_server_runtime.py` | Replaced by phase 6. Delete in phase 8. |
| `replay_game_commands.py`, `replay_native_body.py`, `probe_body_script.py`, `probe_practice.py`, `analyze_game_graphs.py`, `codec_coverage.py`, `export_item_previews.py` | They inspect or compare against the Python implementation. Delete in phase 8. Rewrite one on the Rust crates only if a concrete need comes up. |

### Phase 8: Remove Python

**Gate:** the cutover criteria and the feature list in `docs/notes/parity.md` (see [section 10](#10-python-retirement)), plus passed gates in phases 5 and 6.

1. Classify each of the 114 Python test files in `docs/notes/parity.md` as one of:
   - covered, naming the Rust test;
   - ported, naming the new Rust test;
   - dropped, with the reason (for example, it tests a Python-only research format).

   Port `tests/golden/messages.json` (lobby content golden data, used by `test_content_golden.py`) to a Rust lobby test.
2. Delete:
   - `ow174/`, `tests/*.py`, `tools/`, `data/` (the replay fixture and its README);
   - `requirements.txt`, `pyproject.toml`;
   - `orpheus runtime prepare`, `server::runtime` and `just test-runtime`.
3. Move the old `profiles/` layout handling entirely into `state import`.
4. Check that the research knowledge in the Python docstrings named in phase D now lives in Rust doc comments. Then delete `docs/notes/parity.md`, and record the final client acceptance results in `docs/notes/behavior.md`.

**Done when:** the repository contains no `.py` files outside `research/ida/`, and `just check` passes.

### Phase 9: New documentation

All documentation from here on is written for this fork.

| Document | Content |
| --- | --- |
| `README.md` | What the project is, a three-step quick start (build, `orpheus doctor`, `orpheus play`) and links to the docs below |
| `docs/user/` | Building from source (Rust and the MSVC Build Tools), first run and `state import`, playing, hosting and joining, `doctor` messages and troubleshooting |
| `docs/dev/architecture.md` | Crates, dependency rules, data flow and the state folder, taken from section 4 of this plan |
| `docs/dev/testing.md` | The test tiers and the `just` recipes |
| `docs/dev/data-sources.md` | Moved from `docs/notes/data-sources.md` once phase 6 is finished |
| `docs/notes/` | `protocol.md`, `behavior.md` and `references.md` remain as research reference |
| `CHANGELOG.md` | Starts with the first release, under the later CI/CD plan |

Each earlier phase updates the README when it changes how the program is built or run. Phase 9 finishes the set and deletes this plan.

## 7. Fixed data

These inputs are not game assets, so CASC cannot provide them. They are fixed for build 104319 and never change. Most likely they came from client dumps and traffic taken while the live servers were still running. The first seven are committed in `crates/fixed/data` (decision D1) and embedded in the binary; `docs/notes/data-sources.md` records their source and SHA-256.

| Item | File | Why it can't come from CASC |
| --- | --- | --- |
| Lobby message schemas | `lobby_schemas.json` | Field layouts of compiled C++ message classes, read from a running client by `dump_schemas.py` |
| Game link schemas | `game_schemas.json` | Same, with `--game-link` |
| Statescript payload codecs | `statescript_codecs.json` | What the client's reader code reads per state class, extracted with IDA from a client memory image |
| Battle.net descriptors | `bnet_descriptors.pb` | Protobuf descriptors compiled into the client |
| Lobby state-blob key | `lobby_state_blob.key` | A 64-byte constant of the build's handshake |
| Captured server templates | `retail_templates.json` | Server-side data, decoded from 1.68 reference traffic. It was never in the client. |
| Event resource keys | `resource_keys.json` | The server sends them to the client (`lobby_celebrations`). Treated as server-side data; whether the install also contains them is unverified (D2). |
| Server-set queue and Arcade card fields | Parts of `game_queues_174.json` and `arcade_cards_174.json` | `extract_game_queues.py` reads the captured templates. Arcade card duration and permanence are set by the server, not the card asset. |
| Rules from client code used by derivations | Constants in the extractors, for example the placeable type flag table in `extract_game_maps.py` | Facts about compiled code. They become Rust constants in `assets`. |
| Research names | `statescript_names_174.json` | Research-only naming catalog, used by `orpheus script query`. Not committed yet; it comes with the `ssq` port. |

The captured templates should become authored server content: TOML files a server owner can read and edit, generated once from `retail_templates.json`.

## 8. Decisions

### Decided

| Decision | Result |
| --- | --- |
| CI and CD | Out of scope. The workflow is deleted in phase 0 and comes back in a later plan. |
| Binaries | Users build from source. The launcher injects only the DLL from the same build. |
| Relay | Ported to Rust. The C++ version stays until the live login test passes. |
| Game data | Read from CASC at run time, except the fixed data in section 7 |
| Tools | Ported (`fake_client`, `ssq`), kept as research (the IDA script) or deleted |
| Dashboard | The JavaScript front end stays. The backend moves to Rust. No WebAssembly. |
| Runtime | No async runtime. `mio` and worker threads stay. |
| Target | Build 104319 only |
| D1 Fixed data | Committed to `crates/fixed/data` and embedded in the binary (decided 2026-11-10). The repository owns the files, so names and formats may change. |

### Open

| ID | Question | Options | Needed before |
| --- | --- | --- | --- |
| D2 | Can `resource_keys_174.json` be derived from the install? | Check during the 6.0 spike. If the install does contain the keys, the item moves out of section 7. | Phase 6d |

## 9. Risks

| Risk | Effect | Mitigation |
| --- | --- | --- |
| The 104319 manifest (TRG/CMF) port fails | No runtime parsing | 6.0 spike first. Phases 0–5 and 7 don't depend on it, and the prepared pack remains the fallback. |
| Needed assets are encrypted with keys not present locally | Some derivations impossible | Found by the spike. Missing items go to section 7. |
| STU type table incomplete for some types | Wrong or missing fields | Each derivation must match its oracle file before use |
| Collision builder port differs from Python | Movement and hit differences | Write the same `.col` format and compare caches byte for byte |
| Rust relay misbehaves inside the client (no SEH, game threads) | Client crash or no login | Safe reads and writes through the process memory API, `catch_unwind` in every thunk, the C++ DLL kept until the live test passes |
| First-start derivation is slow | Poor first impression | Per-map lazy collision, a persistent cache and progress in the log |
| Large moves are hard to review | Lost or hidden changes | Phases 1–3 move code with `git mv` only, one checkpoint commit per crate group. Logic changes are limited to the phase 2 cuts. |
| Python cutover gate depends on unported gameplay | Python stays longer | Phases 0–7 don't need Python removed. Phase 8 waits for parity. |
| A fact is lost in the docs purge | Rework or a wrong assumption | Phase D's per-file table and its done-when check. The originals stay in the upstream history bundle. |

## 10. Python retirement

The Python server stays the frozen reference until these work in Rust and are verified with the original client. The list comes from the inherited migration plan (`docs/notes/parity.md` after phase D) and the Python CLI:

- matchmaking queues and PvP modes beyond the solo Practice Range;
- social actions;
- the dashboard routes listed in phase 4;
- host mode (bind beyond loopback), join mode (local Battle.net emulator pointing at a remote lobby) and tournament mode;
- the relay and launcher workflow (phase 5);
- runtime pack replacement (phase 6).

Phase 8 records the final per-feature evidence in `docs/notes/parity.md` before deleting it, and keeps the client acceptance results in `docs/notes/behavior.md`.
