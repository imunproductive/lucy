# Releasing

A release is a GitHub Release for a tag `v<version>`, with one zip for Windows. Pushing the tag starts the workflow [`.github/workflows/release.yml`](../../.github/workflows/release.yml), which builds, packages and publishes it. The packaging is a script, [`tools/dist/dist.py`](../../tools/dist/dist.py), so you can make the same zip on your PC with `just dist <version>`.

## What a release contains

The release has three files:

| File | Content |
| --- | --- |
| `lucy-<version>-windows-x64.zip` | Everything a host or player needs, in a folder `lucy-<version>/` (below) |
| `SHA256SUMS.txt` | The zip's SHA-256, in the `sha256sum -c` format |
| The release notes | The tag's annotation, or else the commits since the previous `v*` tag |

The zip's folder:

| File | From |
| --- | --- |
| `lucy.exe` | The release build, with the version in `lucy --version`. The dashboard is embedded (`crates/server/build.rs`). |
| `owwfd_relay.dll` | The release build of `crates/relay` |
| `lucy_relay.py` | `tools/relay/lucy_relay.py`, the relay without the DLL |
| `START.cmd`, `START_SERVER_LOCAL.cmd` | `tools/dist/`: they run `lucy.exe` and `lucy.exe serve`. The checkout's own `START.cmd` and `START_SERVER_LOCAL.cmd` build with Cargo first, which a download can't. |
| `README.md` | `tools/dist/release-readme.md`: the files, the first start, hosting |
| `docs\` | The user guide (`docs/user`). In the copies, `target\release\lucy.exe` becomes `lucy.exe`, where it is in the zip. |
| `LICENSE`, `CREDITS.md` | The repository's |
| `SHA256SUMS.txt` | The SHA-256 of each file above |

Each file in the zip is dated at the commit's time and the files are in name order, so the same commit and builds give the same zip.

## The version

The tag names the version: `v1.2.3`, or `v1.2.3-rc.1` for a pre-release (a version with a `-` part is published as a pre-release). `dist.py` refuses a tag that is not `v` and a semantic version.

The build gets the version in the environment variable `LUCY_VERSION`, which `lucy --version` prints (`apps/lucy/src/main.rs`, `option_env!`): `lucy 1.2.3`. A build without it, from source, prints `lucy 0.0.0-dev`. Cargo rebuilds `lucy` when the variable changes.

The workspace's Cargo version stays `0.0.0`. Setting it from the tag would change the version of every workspace crate in `Cargo.lock` too, and every build uses `--locked`, so a release would need a commit that edits both files. Nothing reads the Cargo version except the `-dev` fallback.

## Cut a release

1. Merge everything into `main` and check it: `just check`, then the game-install tests on your PC (`just test-game`, `just test-slow`, `just test-dpk`; see [testing](testing.md)). CI can't run those: a runner has no Overwatch install.
2. Optional: try the package on your PC with the version you will tag:

   ```powershell
   just dist 1.2.3
   ```

   This builds `lucy.exe` and `owwfd_relay.dll` (release profile, LLVM), checks that `lucy.exe --version` prints `lucy 1.2.3`, and writes to `dist/`: the zip, `SHA256SUMS.txt` and `release-notes.md`. Before the tag exists, the notes list the commits since the previous tag up to `HEAD`. `python tools/dist/dist.py 1.2.3 --no-build` packages the release build already in the target folder (it still checks its version). Or run the workflow by hand on `main` (Actions, Release, Run workflow, a version): it builds the package on a runner and keeps the files as a workflow artifact, with no release.
3. Tag the commit with an annotated tag. Its message becomes the release notes (Markdown); its first line is the notes' first line:

   ```powershell
   git tag -a v1.2.3 -m "Lucy 1.2.3" -m "- What changed for players and hosts"
   ```

   A lightweight tag, or an annotation that is only the tag's name, gets the list of commits (merges left out) since the previous `v*` tag instead.
4. Push the tag: `git push origin v1.2.3`. The workflow publishes the release, titled `Lucy 1.2.3`, when its release build finishes (Actions, Release).

To redo a release, delete the GitHub Release and the tag (`git push origin :refs/tags/v1.2.3`), fix, and tag again.

## The release workflow

`release.yml` runs on a Windows runner (`windows-2025`):

1. It checks out the whole history, so the notes can find the previous tag, then fetches the tags again with `--force`: `actions/checkout` stores the pushed tag without its annotation.
2. `rustup toolchain install` installs the nightly that `rust-toolchain.toml` pins. `Cargo.toml`'s `cargo-features = ["codegen-backend"]` needs it. The release profile uses LLVM; only debug builds use Cranelift.
3. `Swatinem/rust-cache` restores the cargo registry and the compiled dependencies (key `release`). A tag's run can read only the caches of the default branch and its own: the first tag builds everything, and a dry run on `main` saves a cache the next tag uses.
4. `python tools/dist/dist.py <tag>` builds and packages. It writes the step outputs `version`, `tag`, `zip` and `prerelease`.
5. `gh release create` publishes the zip and `SHA256SUMS.txt` with `release-notes.md` as the notes (`--verify-tag`; `--prerelease` for a version with a `-` part). The workflow's token needs `contents: write`, which the workflow asks for.

## CI

[`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) runs on pushes to `main` and on pull requests, on Windows runners: `just fmt-check`, `just lint`, `just test` and `just test-windows` (loads the relay and probe DLLs into a stand-in process) in one job, `just dashboard-test` in another. It sets `CARGO_BUILD_JOBS` and `RUST_TEST_THREADS` to 4, the runner's CPU threads: the justfile's default is a quarter of them, for a shared PC.

The game-install tests (`just test-game`, `test-slow`, `test-affected`, `test-dpk`, `test-assets`, `test-datatool`, `load`) are not in CI: they read an Overwatch 1.74 install, which can't be put on a runner. Run them on your PC before a merge and before a tag.
