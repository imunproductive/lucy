# Rules for agents

ORPHEUS is a Rust server for the Overwatch 1.74 client (build 104319). Read [README.md](README.md) and [docs/dev](docs/dev) first. [docs/dev/testing.md](docs/dev/testing.md) has the test tiers and the `just` recipes.

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

## Tests

- Before you add a test, think. Add it only if you are sure it is useful.
- First look for an existing test of the same code path. If there is one, extend it instead of adding a new test.
- Test the logic at the lowest level that can show the bug. A fast unit test is best.
- Add a loopback or game-install test only for behaviour that you cannot check below the network: what the client receives, and in what order.
- Never add a test that another test already covers, or could cover with a small change.
- Test behaviour, not wiring. Delete tests that only check wording or wiring (that a function is called, that a string reads a certain way).
- Never weaken a test to make it pass. Do not loosen an assertion, change an expected value to match the output, or add `#[ignore]` to hide a failure. Fix the code, or explain why the expected value was wrong.
- A game-install test that runs for more than about 60 s goes in the slow group: its name starts with `slow_` and its `#[ignore]` reason starts with `slow:`. See [testing.md](docs/dev/testing.md#slow-tests).
