# Testing

## Before every change

```powershell
just check
```

`just check` runs four recipes: `fmt-check`, `lint`, `test` and `dashboard-test`. A change must pass it. It needs no game, no network (except Cargo's crate download) and no prepared data.

You need [just](https://github.com/casey/just) and Node.js in addition to the build tools in [building](../user/building.md). The `justfile` uses Windows PowerShell. Each recipe is a short `cargo` or `node` command, so you can also run it by hand.

## What to run, and when

While you work on a feature, run only the tests that are directly involved:

- `just check` (or `just test` and `just lint`). These are cheap.
- For a quick loop, the game-install tests of the area you are working on, by name:

  ```powershell
  just test-named C:\Games\Overwatch orpheus-sim body_script
  ```

  The last argument is a name filter, as in `cargo test`. `test-named` uses the debug build, which `just test` has already compiled. For the long match tests of `orpheus-server`, add `gametest` to use the optimised build: `just test-named C:\Games\Overwatch orpheus-server assault gametest`. That build takes about two minutes after a change in `sim`, but the tests then run two to three times faster.

Before you report, run `just test-affected <install>`: every test, game-install tests included, of the crates your change touches and every crate that depends on them (`tools/affected.py`, changes since `main`), without `slow_` and `serial_`. A name filter misses tests in the crates that depend on yours.

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
| `just test` | `cargo nextest run --workspace --locked`, then the doc tests | The public tests |
| `just dashboard-test` | `node --test web/tests/test_dashboard_client.mjs` | The dashboard JavaScript test |
| `just build` | `cargo build --workspace --release --locked` | Release build of `orpheus.exe` and `owwfd_relay.dll` |
| `just test-game <install>` | ignored tests with `ORPHEUS_GAME_DIR`, optimised, without the slow group, side by side; then the `serial_` tests one at a time | Tests that read the install; the integrator runs them after a batch of merges |
| `just test-slow <install>` | ignored `slow_` tests, optimised | The game-install tests that play a whole match or run over a minute; the integrator runs them with `test-game` |
| `just test-named <install> <crate> <filter> [gametest]` | ignored tests of one crate whose names contain the filter; debug build unless `gametest` is given | The game-install tests of the area you work on |
| `just test-affected <install>` | every test, ignored ones included, of the crates `tools/affected.py` names, without `slow_` and `serial_` | An agent's own check before reporting |
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
- `testkit::runtime_dir()` derives the game data from that install once and keeps it in `<test data>/104319/<key>`. `<key>` is a hash of the code that makes the data (the library code of `assets`, `casc`, `fixed` and `stu`, computed by `crates/testkit/build.rs`), so a change to a derivation derives again, and a derivation's test reads the output of the code under test rather than deriving it itself: `testkit::runtime_json(path)` reads a derived file, and `testkit::derived_outputs(name)` lists the files the derivation `name` returned (`outputs.txt` in the pack, which `orpheus_assets::build` writes). The test data folder is `ORPHEUS_TEST_DATA`, by default the main checkout's `target/orpheus-test-data`: every worktree of the clone, and every branch with the same derivation code, shares one derivation. Test processes that start at once derive it once: they take turns on `<key>.lock`, and the one that derives builds in a folder of its own and renames it into place when it is whole. A process that derives deletes the data of other keys that no test used for 7 days (`<key>.used` holds the last use). The old layout, `target/orpheus-test-data/104319/data` and `collision` in each worktree, is no longer read; delete it.

`just test-game` and `just test-slow` build with the Cargo profile `gametest` (in `Cargo.toml`). It optimises every crate as the release profile does, but it keeps the debug assertions and overflow checks. So the VM still refuses statescript content it does not implement, as in `cargo test`. Each game-install test, run alone, takes about half the time of a debug build; the `sim` tests take a third. The build goes to `target/gametest`, apart from the debug build. The first `gametest` build compiles everything once (about 7 minutes); after a change in `sim` it takes about two minutes.

The `just` recipes build and run the tests with a quarter of the PC's CPU threads (`CARGO_BUILD_JOBS`, `RUST_TEST_THREADS` in the `justfile`; 4 on a 16-thread PC), since other agents and the user share the machine. The game-install tests need a cap for memory too: each holds about 0.7 GB of game data, and with one test per CPU thread a PC with 16 threads and 16 GB ran out of memory. A test that opens a socket binds `127.0.0.1:0`, so the system gives it a free port, and writes only to its own temporary folder (`tempfile::tempdir()`). So the tests don't take each other's ports or files, and they run while a server listens on the default ports. A new test must do the same. Only the derived game data and its collision caches are shared; a missing map's collision is built once, by whichever test process asks first, while the others wait on its lock file (`collision/<map>.lock`).

#### Game service tests

The game service's tests (`crates/server/src/game_service/tests.rs`) run it on a virtual clock, with an in-memory network in place of its UDP socket (`game_service/lockstep.rs`). The loop is the real one (`GameLoop::pass`); only its clock and its socket are the test's. The clock moves in lockstep with the test's clients: they send, the service takes their datagrams and runs the next tick exactly when it is due, and they read what it sent. No tick is skipped, so a test's run does not depend on the PC's speed or on the other tests.

To write one:

- Start the service with `Lockstep::new(schemas)`, not `GameService::bind`. It derefs to the `GameService`, so open matches as usual.
- Connect a `Client::new(&server, &ticket)` (or a `Watcher`). `send` queues a datagram that arrives before the next tick; `receive` runs ticks until a datagram comes, at most `RECEIVE_TICKS` (2 s).
- Wait in game time, never in real time: no `Instant`, `thread::sleep` or socket timeout. `server.now()` (or `client.now()`) is the time of the last tick, so a deadline is `server.now() + Duration::from_secs(n)`. `server.step()` runs one tick and `server.run(time)` the ticks of `time`, for example between two polls of `server.snapshot()`.
- `open_world` prepares the scene on a thread of its own, in real time: call `server.await_scenes()` after it. The clock waits until every match has its scene, so the scene comes at a known tick. A test that holds the preparation back itself (`collision_ready.rs`) calls it after it lets the scene go.
- Timing is exact. The server sends each client a datagram every tick, and a client sends right after each datagram, so its input arrives at a tick's time, after that tick. Game messages run before the next tick. Commands run one a tick from the player's input buffer, with the next one in hand (`InputQueue` in `replication.rs`): a client that sends one command after each datagram has its first command wait a tick, then each runs the tick after it arrives (the command sent after tick T runs before tick T + 2); a burst runs all but the last 4 at once. An AI player's (an AI hero's, a dummy bot's) command of tick T runs right after tick T, before `step` returns, so a request after the step sees it. What a tick queues goes in that tick's datagram. Derive the expected tick from the rule and write the derivation in a comment, as `a_player_kills_another_who_respawns_after_the_practice_range_delay` does for the respawn (187 ticks after the death notice) and `exercise_loading` for the loading steps. Never give a window for the PC's speed.

A test on the real clock and real UDP only proves the real loop and its sockets: `server::tests::serial_native_server_logs_in_saved_profile_and_loads_practice_over_udp` runs the whole server so. Do not add another for behaviour.

#### Lobby tests

Every lobby timer reads the lobby's clock (`orpheus_lobby::clock::Clock`, the time since the lobby started): a session's ticks and keepalive, the matchmaker's handoff delays, sweeps and time played, the account features after login, the offline friends' presence, party invites and the dashboard's uptime. The server reads the wall clock. A test's lobby clock is virtual (`lobby/clock.rs`, `VirtualClock`): it stands until the test moves it.

To write one:

- Start the whole server with `Server::start_virtual(config)`, not `Server::start`: the lobby on a virtual clock and the game service on its lockstep (above). It returns the clocks: `time.lobby` and the game service's network `time.game`. The matchmaker's tests that need no client build the lobby alone (`matchmaker/ported_tests.rs`, `Fixture`) and call its sweep themselves.
- Connect a game's lobby client with `lobby::test_client::Client` (loopback TCP). A session answers a ping in turn with its client's messages, once it has sent everything it had to send by then. So `send` returns once the lobby handled the message, with its replies; `expect`, `settle` and `take` read up to such a ping; `close` returns once the session ended and its logout ran. Mail one session posts to another is there when the other client reads next. A game on the game service is a `game_service::lockstep::Client` on `time.game`.
- Move the lobby's clock with `test_client::run(&time.lobby, &mut [&mut client], time)`: it moves a lobby tick (`TICK`, 250 ms) at a time, and each client's session ticks after each. The game service's clock moves only with its network (`time.game.run(time)`, or a game client that waits for a datagram). Never wait in real time: the socket's read timeout only fails a test whose lobby hangs.
- Timing is exact. Derive the expected time from the rule and write the derivation in a comment: the 20600 of a popped queue comes `FOUND_DELAY` (4 s) after "Game Found!", at the 16th lobby tick (`one_player_to_start_pops_the_queue_and_hands_the_game_off`); a game that stays after 20304 is dropped at the first game tick 3 s after its release, the 188th (`a_game_that_stays_after_20304_is_dropped`).

#### Slow tests

A game-install test that runs for more than about 30 seconds in the `gametest` build is in the slow group (today only the Practice vs AI match, whose twelve bodies take about 40 s to simulate):

- Its name starts with `slow_`. `just test-game` skips the names that contain `slow_` (`--skip slow_`); `just test-slow` runs only them. Do not use `slow_` in the name of another ignored test.
- Its `#[ignore]` reason starts with `slow:` and says why it is slow.

```powershell
just test-slow C:\Games\Overwatch
```

#### Serial tests

The load of other tests stretches real time: a server on the real clock skips the ticks it falls behind on. A game-install test on the real clock whose assertion depends on that timing (a window of ticks or milliseconds, or the exact contents of a datagram) is in the serial group. A game service or lobby test on a virtual clock never is.

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

## Test speed

The whole suite, debug build (Cranelift), 4 tests at once, before and after this work (2026-11-20; per-test times in `D:/Work/Coding/refs/test-times/2026-11-20-cranelift` and `…-integspeed`):

| Phase | Before: wall (sum of tests) | After: wall (sum of tests) |
| --- | --- | --- |
| Game-install tests (283) | 864 s (3106 s; median 8.1 s, slowest 78 s) | 255 s (1001 s; median 2.5 s, slowest 33.5 s) |
| Serial test | 21 s | 5.9 s |
| Slow test | 924 s (partly while agents built) | 542 s |

The unit tests (908) take about the same: one process per test adds about 0.03 s each.

Each test runs in its own process (nextest), so whatever a test loads before its own work, it loads every time; a `LazyLock` fixture in test code lasts one test. Measured 2026-11-20 on the 16-thread PC: debug build (Cranelift), `cargo nextest run -j 4`, `CARGO_BUILD_JOBS=4`.

What a game-install test that prepares the Practice Range spent before its own work, alone (timings in a throwaway build). "First" is before this work, "then" after the first round (shared test data, parallel loads), "now" after the second (one parse for both bundles, graphs parsed on every thread, zlib collision caches):

| Step | First | Then | Now |
| --- | --- | --- | --- |
| `runtime_dir()`: is the derived data current? | 1.5 s: it opened the install's storage for the stamp | none: it reads `.build.info` | none |
| Hero graph bundle, 32 MB of JSON (2.7 s of it `serde_json`) | 2.9 s | 2.9 s, beside the bot bundle | both bundles 1.7 s: the 628 graphs whose JSON the two share (of 661 and 660) are parsed once, every graph on a thread of the PC's (the hero bundle alone: 1.05 s) |
| Bot graph bundle, 32 MB of JSON | 2.9 s | on a thread of its own | (above) |
| Map areas, 11 MB of JSON | 0.4 s | on a thread of its own | on a thread of its own |
| Movement collision: read (0.2 s), scene, contacts | 0.9 s | 0.9 s, beside the ray scene | read 0.09 s |
| Ray collision: read (1.2 s: XZ 0.55 s, geometry 0.46 s), scene | 2.2 s | on a thread of its own | read 0.63 s: the caches are packed with zlib (format 6) |
| Everything else (catalogs, navigation, controller) | 0.5 s | 0.5 s | 0.5 s |

The derivations' own tests derived their file again in each test (`game_objectives` 35 s, `hitboxes` and `game_body_data` 9 s); they now read the file `runtime_dir()` derived with the same code (`testkit::runtime_json`), and check the list of files the derivation returned (`testkit::derived_outputs`).

| Tests | First | Then | Now |
| --- | --- | --- | --- |
| `orpheus-assets` game-install tests (24) | 89.6 s wall, 277.7 s summed | 4.9 s wall, 10.6 s summed | 3.5 s wall |
| `orpheus-sim` and `orpheus-server` game-install tests (166, without the slow and serial groups) | 680 s wall, 2693 s summed, median 16.5 s | 391 s wall, 1556 s summed, median 8.9 s | 326 s wall, 1292 s summed, median 6.7 s |
| `spawn_room::a_hero_change_in_the_room_keeps_position_and_yaw`, alone | 13.6 s | 7.1 s | 4.8 s |
| `scene_assets::tests::stock_soldier_plays_a_scripted_session`, alone | 19.3 s | 11.4 s | 10.5 s |
| `game_service::tests::a_firing_practice_bot_shoots_the_player`, alone | 19.8 s | 14.2 s | 14.2 s |
| `lobby::social::tests::match_commands_and_general_again_after_the_match`, alone | 18.0 s | 11.5 s | 9.9 s |
| A derivation for a new key, four test processes asking at once | each process derived in a folder of its own (from the code; not measured) | one derived (6 min), three waited on the lock | the same |

In the suite the game service tests (virtual clock, so bound by the CPU) vary by several seconds from run to run; the machine is shared, and each process's graph parse briefly takes every thread.

The matchmaker's ported tests (`lobby::matchmaker::ported_tests`, 28) took 0.5 s each when every test of a binary shared one process (their `LazyLock` loaded the scene assets once for all of them), 12 s each per process before this work, and 2.65 s alone after the second round, 1.7 s of it the scene assets. Only one of them opens a world, so their lobby now takes `SceneAssets::load_deferred`: the graph bundles load the first time a scene is prepared or a graph asked for, and the map areas load beside the other catalogs (in `SceneAssets::load` too, which the server uses and which still loads everything at once). Alone, such a test takes 1.3 s (the assets 0.35 s); in the suite the 28 took 42 s summed instead of 117 s, and the `sim` and `server` game-install tests 260 s wall, 945 s summed, median 5.2 s (from 326 s, 1292 s and 6.7 s).

The `script query` tests (`apps/orpheus/src/script_query/tests.rs`, 7) each load the graph bundle into the queries' own JSON values and index every node, which took about 4.3 s (bundle) and 12 s (index) on one thread (measured alone while the PC was busy; 7.4 to 7.9 s each in the suite run). `Bundle::load` now parses each graph's JSON on a thread of the PC's, and `Index::build` indexes runs of neighbouring graphs on the PC's threads and merges the runs in graph order (merging one graph at a time cost as much as indexing it): about 1 s for the bundle and 1.5 s for the index. The controller tests (`crates/script/src/controller.rs`) loaded the hero bundle at each start of the controller, four times in `a_rulesets_9548_turns_the_hero_limit_off_or_keeps_it`; they now load it once a test. Alone (`-j 1`, two runs each, other agents building): `script_query::tests::aliases_are_unique` 12.4 and 11.2 s before, 3.5 and 2.5 s after; `a_rulesets_9548_turns_the_hero_limit_off_or_keeps_it` 12.6 and 17.5 s before, 2.8 and 1.4 s after. The 13 side by side (`-j 4`): 48 s wall before, 24 s after.

Not changed, measured:

- A binary form of the graph bundles does not pay off. Most of the parse is building the `serde_json::Value` trees of the nodes' fields, not reading the text: in a scratch program with the same profile, the hero bundle parsed into a `Value` in 1.7 s from JSON and 1.9 s from MessagePack (debug), 0.61 s and 0.51 s optimised.
- `serde_json`'s `preserve_order` makes that parse 2.75 s instead of 1.69 s (debug), but the server needs it: it is on for every crate (Cargo builds one `serde_json` with the features any crate asks for, so it can't be on for the derivations' writers only), and the graphs' readers depend on the order of a node's fields. `vm/timers.rs` takes a node's first `STUConfigVarInt` field in file order; 1050 nodes of the hero bundle have more than one, and in 810 of them the first in file order is not the first by name. The derived files also keep the Python pack's key order (`Output::json`), which the catalogs read into `IndexMap`s in that order.

### The slowest game-install tests

Measured on 2026-11-20 with the debug build (Cranelift), `cargo nextest run --run-ignored only -j 4` of the tests in the table side by side, `CARGO_BUILD_JOBS=4`, on the same PC as the baseline, which other agents were also using. Expect a few seconds of noise. The times are in seconds.

| Test | Before | After | What changed |
| --- | ---: | ---: | --- |
| `slow_a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end` | 887 | 537 | Server: a body's observer frame is projected only when a client needs a full frame (below) |
| `the_mode_graph_runs_assault_and_escort_until_its_first_gap` | 59 | 35 | Server: cached server-only variables, inactive states skipped first (below) |
| `a_competitive_assault_on_anubis_swaps_sides_and_spawn_rooms` | 56 | 28 | Observer projection, scene preparation (below) |
| `a_pvp_match_on_another_map_assembles_its_team_and_takes_a_third_player` | 54 | 25 | The same; the mode frames come from `scene::mode_ui`, not from a second scene |
| `a_reinhardt_training_partner_comes_once_with_the_consoles_spawn_effect` | 51 | 30 | Observer projection, scene preparation |
| `an_assault_match_on_anubis_captures_a_and_b_and_sends_the_players_home` | 50 | 24 | The same, and the mode frames without a second scene |
| `a_biotic_field_heals_a_wounded_teammate_and_every_client_sees_it` | 48 | 26 | Observer projection, scene preparation |
| `collision_ready::a_cache_only_server_fails_a_missing_map_and_a_builder_still_builds_it` | 44 | 15 | The builder server uses the same loaded data (one load, not two); scene preparation |
| `practice_range_patrols_walk_around_what_blocks_the_straight_way` | 32 | 22 | Scene preparation |
| `healthy_and_scriptless_same_hero_picks_keep_the_body` | 32 | 16 | The data is loaded once for both cases (each still prepares its own scene) |
| `ana_and_widowmaker_walk_at_35_percent_once_fully_zoomed` | 30 | 13 | One prepared Practice Range for both heroes, not one load and scene each |
| `an_ai_hero_walks_round_a_wall_to_the_point_behind_it` | 29 | 18 | Scene preparation (Gibraltar has the largest ray cache) |
| `rialto_and_volskaya_get_their_server_placeables` | 26 | 20 | Scene preparation (two large maps) |
| `elevated_practice_patrols_reach_their_hints_without_crossing_walls` | 24 | 16 | Scene preparation |
| `an_attacker_walks_from_the_anubis_spawn_to_point_a` | 21 | 16 | Scene preparation |

The changes to the code, which also make the server faster:

- **Scene preparation** (`SceneAssets::prepare`). The collision grid (`orpheus_physics::collision::grid`) used to be a hash map with one `Vec` for each column. Now it is built once into flat arrays (`GridBuilder`): a slot for each column of the geometry's box (or a sorted list of the occupied columns when the geometry is spread thin), with no hashing on a query. The columns list their geometry in the same order as before. On Rialto the ray index (768k triangles) took 3.2 s and now takes 1.5 s. The meshes of the collision caches are read in slices, not field by field. The queries' "seen" sets use the physics crate's `FastSet`.
- **Observer frames** (`orpheus_sim::others`). Every tick, the server made a full observer projection of each observed body's script, but used it only for a client that still needed a full frame. Now the projection is made only then, at most once a tick. In the gametest profile of the Practice vs AI match, these projections took 14% of the game loop.
- **Snapshot projection** (`orpheus_script::snapshot`). A graph's server-only variables are read from its JSON once, when the graph loads (`Graph::server_only_variables`), not for each instance in each projection. A state that is off and was never activated is skipped before its codec is looked up by class name.

What is left:

- Loading takes about 6.5 s of each test that loads the data (`SceneAssets::load`). `testkit::runtime_dir` also opens the install in every test process to check the derived data. In the gametest profile, that check took 12% to 23% of a short test. Both belong to test fixture loading.
- A scene's ray cache still takes 1 to 1.5 s to decompress (xz) and its triangle index 1 to 1.5 s to build in the debug build.
- The server's tick: see below.
- A sampling profile of the debug build can't name the functions of `sim`, `script` and `physics`: Cranelift writes no CodeView debug information for them. Profile the `gametest` build instead. Its stacks unwind with the executable's `.pdata`.

### The server tick

Measured with SlowTests' sampler on the `gametest` build (frame pointers on) of `slow_a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end`, 100 samples a second, the same PC. Under the sampler the whole test took 120 s before these changes and 88 s with all of them but the short-object lookups and the last two no-op writes below; the game loop thread went from 22,000 samples to 15,800 (−28%). Run without the sampler, alternating old and new binaries, the test took 165 and 139 s before and 93 and 105 s after (82.8 s in nextest), but one earlier lone run of the old binary took 64 s: other agents were loading the PC, so the sample counts are the better measure.

| Part of the game loop (base profile) | Share of the loop | After |
| --- | ---: | --- |
| Snapshot projection (`project_with_exclusions`) | 22% | reused while the snapshot's revision holds (below); made again for most AI bodies, whose scripts change almost every tick |
| Owner and observer deltas (`queue_delta`) | 19% | unchanged |
| JSON member lookups (`serde_json` `index_into`) | 19% | the largest callers removed; short objects searched without hashing |
| `Machine::queue_entries` (a message's receivers) | 8.5% | gone: an index by message |
| `ObserverActivity::of` (once per viewer and body) | 8.3% | once per body and revision |
| `CodecTable::codec` (hash of the class name) | 5% | once per node |

The game-install sets of `orpheus-script`, `orpheus-sim` and `orpheus-server` (debug build, `-j 4`, 210 tests, `slow_` and `serial_` left out), before and after, in seconds. The match tests gain the most; the tests that mostly load and prepare a scene stay within noise:

| Test | Before | After |
| --- | ---: | ---: |
| `the_mode_graph_runs_assault_and_escort_until_its_first_gap` | 45.9 | 31.5 |
| `a_competitive_assault_on_anubis_swaps_sides_and_spawn_rooms` | 36.2 | 22.3 |
| `a_biotic_field_heals_a_wounded_teammate_and_every_client_sees_it` | 34.2 | 21.9 |
| `a_reinhardt_training_partner_comes_once_with_the_consoles_spawn_effect` | 31.9 | 25.4 |
| `a_pvp_match_on_another_map_assembles_its_team_and_takes_a_third_player` | 30.3 | 18.6 |
| `a_firing_practice_bot_shoots_the_player` | 26.7 | 18.4 |
| `an_assault_match_on_anubis_captures_a_and_b_and_sends_the_players_home` | 26.6 | 19.9 |
| `a_pulse_bomb_sticks_to_a_bot_kills_it_after_its_fuse_and_another_client_sees_it` | 24.9 | 16.6 |
| `rialto_and_volskaya_get_their_server_placeables` (loading) | 22.8 | 24.5 |
| The whole set, run time | 671 | 651 |

The changes (`orpheus_script::graph`, `snapshot`, `vm`; `orpheus_sim::others`, `replication`):

- **Node information compiled once** (`Graph::compile`). Each node keeps what the VM and the projection read on every pass: its state and action flags, whether it is a UX presenter, whether it is remote-synced, and the game message of its `m_gameMessage` (`Node::message`). A graph keeps its server-run nodes by game message (`Graph::message_nodes`; `queue_entries`, auras and `sends_message` read it) and its weapon volleys' ammunition variables (`Graph::volley_ammo`, read by every owner delta). A node remembers its class's entry in the codec table (`CodecTable::node_codec`; every client table shares one id, a fixture table looks its classes up each time).
- **Short JSON objects** (`graph::member`, behind `field_at`, the VM's `field` and the evaluator's `child`/`class`). An object of up to 12 members is searched in order: `serde_json` keeps the members' order (`preserve_order`, an `IndexMap` with SipHash), and comparing a few keys costs less than hashing one.
- **Snapshot revisions** (`Snapshot::revision`). The machine's snapshot sits behind `TrackedSnapshot`: every mutable borrow moves its revision on, so no write can be missed. `capture_outputs` writes the pending timers, bindings and resolved values only when they changed (values compared bit for bit: `Value::identical`), and the clock outside the revision. A variable set to the value it has, a link to a variable that is already there, a delayed switch's timer that isn't pending and an Ability's unchanged flags write nothing.
- **Kept projections** (`SnapshotEncoder::project_cached`, `ProjectionCache`). A body keeps its last owner projection, and one observer projection that all clients share. It is the answer again while the revision, view and entity exclusions are the same and the clock is still before the first moment a projected value depends on it: a pulser's `fresh` (500 ms from its pulse) or a wheel action's lifetime (2 s). The projection reads the clock nowhere else (a grapple's and a volley's start, and the owner timers, are absolute). The observer activity is kept per revision too. The kill feed test (`kill_feed_pulse_waits_for_the_world_then_stays_fresh_until_acknowledged`) checks a kept projection against a new one every frame while the pulse goes stale.

How often a projection is kept: in the biotic field test about 30% of the owner and observer projections, in the Anubis assault nearly all (idle bots). Of the projections made again, 61% came out the same as the one before; the writes behind them were a delayed switch's timer cancel, an Ability's flags and the resolved values of links (a chase's value at the new time), fixed above except the last, and a ModifyHealth's `last_ms` (not projected).

Then (2026-11-20, day/vmexpr), measured the same way on the `gametest` build of merged main (`D:/Temp/bench/vmexpr`: `cpu.py` gives a process's CPU seconds, `loop.py` counts the game loop thread's samples). The PC was shared with other agents' builds, so the runs vary by about a third; the binaries were run alternately:

| `slow_a_practice_vs_ai_match_plays_its_ai_heroes_to_the_end` | Before | Compiled configs | Both changes |
| --- | ---: | ---: | ---: |
| CPU seconds of the test process, two runs | 165.5, 120.8 | 162.4, 127.2 | 62.6, 65.2 |
| Game loop samples (100 a second), two profiles | 5033, 7768 | 3878, 7076 | 2787, 4218 |
| Config evaluation (`Vm::evaluate_as`), share of the loop | 13.3%, 13.7% | 10.1%, 10.8% | 18.7%, 19.0% |
| Projection (`project_cached`) | 25.8%, 23.1% | 25.1%, 25.2% | 13.0%, 14.7% |
| Deltas (`queue_delta`) | 23.2%, 23.2% | 27.1%, 25.1% | 14.7%, 13.6% |
| The mover (`Mover::step`, unchanged) | 19.9%, 19.0% | 18.4%, 18.8% | 30.9%, 27.7% |

Against the unchanged mover the loop takes about a third less (loop/mover samples 5.0 and 5.3 before, 3.2 and 3.6 after); by CPU time about half. `a_biotic_field_heals_a_wounded_teammate_and_every_client_sees_it` mostly loads in `gametest` (4 s): 5.4 and 5.4 CPU seconds before, 4.5 and 4.8 after.

- **Compiled configs** (`expr::ConfigTable`, `Node::configs`). The first time the VM evaluates a config of a node, every config in the node's fields is compiled once and kept by its address in the fields (a loaded graph's fields don't change): a config that reads no context (the literals, the asset configs of `resource_field`) as its value, a `STUConfigVarDynamic` as its variable, an expression as its bytecode decoded at every offset with its constants, identifiers and branch targets read (`expr::Program`; what can't be decoded keeps its error and raises it only when it runs, as before). The evaluator looks a config up in the node's table before reading its class; the VM's `evaluate_as`, `evaluate_quietly` and `evaluate_state_config` pass the table, the other callers compile an expression where they run it. Lazily per node, because a test loads 1300 graphs and runs few of their nodes. In the profile the evaluation's own overhead went (`Expression::read`, the class match, the JSON lookups); what is left is the work: variable reads, entity filters, rulesets, and `evaluate_as` recording the reads.
- **Per-instance revisions** (`vm::Instances`, `Instance::revision`). The snapshot's instances sit in a map whose every mutable borrow moves that instance's revision on; `capture_outputs` moves an instance on when its pending timers, links or resolved values changed.
- **Kept instances** (`ProjectionCache`'s `InstanceStamp`). A projected instance is shared (`Arc`). When the snapshot changed, each instance keeps its projection while its revision, the revisions of the instances its owner links name (`project_bindings` reads their states) and its own clock window hold. The deltas share it too: the stream's pending baseline is a clone of the projection, now a few reference counts, and a delta between a baseline instance and the same shared instance lists only the touched entries, once the instance is known to compare equal to itself (`ProjectedInstance::self_same`: no NaN). `a_kept_projection_shares_the_instances_that_did_not_change` checks that a kept projection equals a new one and shares the unchanged instance.

What is left in the tick: the mover (`Mover::step_pending_with`, now the largest part), the config evaluation's real work and `evaluate_as`'s read records (`IndexMap::entry` with SipHash), the projection and deltas of the instances that change every tick (`project_variables`, `delta_variables`), and the VM's own field reads by name (`field(&node.fields, "m_condition")`, short-object searches). Next steps: a fast hasher for `wire::Variables` and the snapshot's maps (136 places build a `Variables` with `new()`), and the entity variables' projection, still made on every miss.

## Debug and release builds

Debug builds refuse statescript content the VM does not implement; release builds tolerate it (see [architecture](architecture.md#unknowns-policy)). A test that runs a hero graph in a debug build shows each gap as a diagnostic.

## Build speed

Measured 2026-11-20 on this PC (Ryzen 7 2700, 16 GB, Windows 10), 6 build jobs, at below-normal priority; one run each, while the PC was in use, so read the numbers as ±10-20 %. Scripts and logs: `D:/Temp/bench/`.

| Step | stable 1.98, link.exe | stable + rust-lld | nightly 1.102 + rust-lld | nightly + Cranelift |
| --- | --- | --- | --- | --- |
| Full build of the test binaries | 636 s | 695 s | 479 s | 157 s |
| Rebuild after touching `crates/script` | 391 s | 427 s | 314 s | 35 s |
| Rebuild after touching `crates/server` | 23 s | 25 s | 13 s | 8.5 s |
| sim `body_script` tests, run time | 19.0 s | 18.1 s | 14.5 s | 26.4 s |
| server `practice_bots_run_their_scripts_for_a_client`, run time | 9.5 s | 10.2 s | 9.5 s | 17.0 s |

- The linker is not what is slow: rust-lld alone changed nothing measurable.
- Nightly compiles faster than stable, probably mostly through the parallel front end, on by default in nightly since 2026-11-14 (2 threads; `-Zunstable-options --jobs-frontend=N` sets more; not measured apart).
- Cranelift compiles about nine times faster after a change to `orpheus-script` and makes the tests run 1.2-1.8 times slower. So debug builds and tests use Cranelift (`Cargo.toml` `profile.dev`), and `release` and `gametest` keep LLVM.
- `rust-toolchain.toml` pins the nightly. Move the pin on purpose: install the new nightly with the `rustc-codegen-cranelift-preview` component, run `just check` and `just test-game`, then change the pin.
- Cranelift can't unwind on Windows: a debug build aborts on a panic, and each test runs in its own process (`panic-abort-tests` in `.cargo/config.toml`; nextest does the same). So a fixture a test file shares through a `LazyLock` or `OnceLock` is built again by every test: keep expensive fixtures on disk (the derived test data), not in a static.
- `tools/lowprio.ps1` keeps every build and test process at below-normal priority, so the user's programs come first.
