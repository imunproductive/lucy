# Rules for agents

ORPHEUS is a Rust server for the Overwatch 1.74 client (build 104319). Read [README.md](README.md) and [docs/dev](docs/dev) first. [docs/dev/testing.md](docs/dev/testing.md) has the test tiers and the `just` recipes.

## CPU use

This PC has 16 logical CPUs, and cargo by default uses all 16 to build and runs 16 tests at once. Use a quarter: 4 build jobs and 4 tests at once. The `just` recipes set both themselves (`CARGO_BUILD_JOBS` and `RUST_TEST_THREADS` in the `justfile`). Before any plain `cargo` command, set them yourself:

```powershell
$env:CARGO_BUILD_JOBS = 4; $env:RUST_TEST_THREADS = 4
```

(in bash: `export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4`). Other agents and the user share the machine.

## Before you report

Run these, and nothing more:

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets --locked -- -D warnings`
3. `cargo test --workspace --locked`
4. The game-install tests of the area you touched, by name:

   ```powershell
   just test-named D:\Games\Overwatch orpheus-sim body_script
   ```

   The last argument is a name filter, as in `cargo test`. Give the narrowest filter that runs the tests of the code you changed. The recipe uses the debug build that step 3 compiled. For the long match tests of `orpheus-server`, add `gametest` at the end to run them optimised.

   If you changed only `web/`, run `just dashboard-test` instead of steps 1-4.

Do not run `just test-game` or `just test-slow` while you work on a feature. They take many minutes and check areas you did not touch. The integrator (whoever merges) runs them once after a batch of merges.

## Research knowledge

Research is expensive; never let it get lost, and never do it twice.

- **Before you research anything**, read [D:/Work/Coding/refs/reports/README.md](D:/Work/Coding/refs/reports/README.md) (the index of every report, outside the repo) and search the reports and `docs/notes/` for your subject: its graph ids (`01CB`), entity definitions, messages (`0011.025`, 25000), variables (`#478`), STU classes and fields, addresses. Start from what is already known, and say in your report which earlier findings you used.
- **Questions about the client's code**: try the client tools first ([docs/dev/client-tools.md](docs/dev/client-tools.md)): `tools/stu.py` for a statescript class, `python -m tools.owindex` for an address, `tools/who_reads.py` for a field, `python -m tools.owdbg.*` to probe, watch or diff the running game. Only what they can't answer needs a manual IDA session.
- **When you finish researching**, write down everything you found that could matter later, not only the answer to your question. Side findings count: a graph you traced that turned out to belong to another feature, how an entity is created, what a variable means, which ideas you ruled out and why. Example: a death-camera search that found `01CB` runs only for D.Va's mech and Junkrat's RIP-Tire must record how that third-person view works, for whoever does those abilities later.
  - Put it in a report in `D:/Work/Coding/refs/reports/<Topic>.md`: a new one, or the existing report on that subject. Give each fact its evidence (file, graph and node, address, test) and mark guesses as assumptions.
  - Add or update the report's line in the index `D:/Work/Coding/refs/reports/README.md`: the file, one sentence on what it covers, and the ids someone would search for.
  - What our own server does (behaviour, assumptions) still goes in `docs/notes/behavior.md`; the report holds the research behind it.
  - If your tools can't write files, put the full report in your final answer, and the integrator saves it.
  - After you write a report or the index, read the file back to check the write landed: appends from a copied shell have failed silently before.
- **Never run an `orpheus` binary with the user's state folder** (the default, `C:/Users/imunp/AppData/Roaming/orpheus`): give `--state <a temp folder of yours>` to `script query` and other commands. Two agents' queries once broke the user's game data cache there.

## Leftovers

When you finish a task, write down what you left undone so nobody has to find it again.

- **Small related work** goes in [docs/dev/loose-ends.md](docs/dev/loose-ends.md): anything your task touched but didn't finish because it was out of scope, too small to do now, or blocked on an unknown (for example "a fall death isn't credited to the last attacker"). One entry each: what is missing, where (file, graph, message), and the evidence or report. Remove an entry when you finish it. Don't write in `TODO.md`: it is the user's.
- **Questions only the client's code can answer** go in `D:/Work/Coding/refs/reports/IDA-questions.md` (outside the repo), under a heading for your task. Ask exactly: the function or address, the field, and what each answer would change.

## Tests

- Before you add a test, think. Add it only if you are sure it is useful.
- First look for an existing test of the same code path. If there is one, extend it instead of adding a new test.
- Test the logic at the lowest level that can show the bug. A fast unit test is best.
- Add a loopback or game-install test only for behaviour that you cannot check below the network: what the client receives, and in what order.
- Never add a test that another test already covers, or could cover with a small change.
- Test behaviour, not wiring. Delete tests that only check wording or wiring (that a function is called, that a string reads a certain way).
- Never weaken a test to make it pass. Do not loosen an assertion, change an expected value to match the output, or add `#[ignore]` to hide a failure. Fix the code, or explain why the expected value was wrong.
- A game-install test that runs for more than about 30 s in the `gametest` build goes in the slow group: its name starts with `slow_` and its `#[ignore]` reason starts with `slow:`. See [testing.md](docs/dev/testing.md#slow-tests).
- A game service or lobby test runs on a virtual clock: the game service on `Lockstep` (see [testing.md](docs/dev/testing.md#game-service-tests)), the whole server with `Server::start_virtual` and its lobby clients with `lobby::test_client` (see [testing.md](docs/dev/testing.md#lobby-tests)). It waits in ticks, never in real time, and its timing assertions are exact values derived from the rules, with the derivation in a comment. Only the whole-server test (`serial_native_server_logs_in_saved_profile_and_loads_practice_over_udp`) runs on the real clock and real UDP; do not add another.
- The game-install tests run in parallel. A test that opens a socket binds `127.0.0.1:0` (never a fixed port) and writes only to its own `tempfile::tempdir()`. A real-clock test whose assertion depends on real-time pacing (a window of ticks or milliseconds, or the exact contents of a datagram) goes in the serial group: its name starts with `serial_` and its `#[ignore]` reason starts with `serial:`. See [testing.md](docs/dev/testing.md#serial-tests).
