//! Test support shared by the workspace crates.
//!
//! Tests that need build 104319 data are marked `#[ignore]` and find that data
//! only through this crate. Run them with `just test-game <install>`; the
//! derivation checks against a prepared pack run with `just test-assets
//! <install> <pack>`.

#![forbid(unsafe_code)]

pub mod casc;

use std::{
    env,
    fs::{self, File},
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

use lucy_assets::cache::{self, BUILD};

/// The hash of the derivations' code (`build.rs`): the derived data of one
/// code is kept apart from another's.
const DERIVATIONS_KEY: &str = env!("LUCY_DERIVATIONS_KEY");
/// How long the derived data of other code is kept after its last use.
const KEEP_UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The Overwatch 1.74 install named by `LUCY_GAME_DIR`.
pub fn game_dir() -> PathBuf {
    env::var_os("LUCY_GAME_DIR")
        .map(PathBuf::from)
        .expect("set LUCY_GAME_DIR to an Overwatch 1.74 install (just test-game <install>)")
}

/// A DataTool XML export of the same install (`extract-stu-type <dump> <type>
/// --xml`), named by `LUCY_DATATOOL_DUMP`; the oracle for the asset reader.
pub fn datatool_dump() -> PathBuf {
    env::var_os("LUCY_DATATOOL_DUMP").map(PathBuf::from).expect(
        "set LUCY_DATATOOL_DUMP to a DataTool XML export (just test-datatool <install> <dump>)",
    )
}

/// The Doomfist parkour script archive (`dfp-archive`: workshop.codes and
/// GitHub `.txt` Workshop scripts), named by `LUCY_DPK_ARCHIVE`; the
/// Workshop parser and engine run over it.
pub fn dpk_archive() -> PathBuf {
    env::var_os("LUCY_DPK_ARCHIVE").map(PathBuf::from).expect(
        "set LUCY_DPK_ARCHIVE to the Doomfist parkour archive (just test-dpk <install> <archive>)",
    )
}

/// The game data the server reads, derived from the install named by
/// `LUCY_GAME_DIR`, in `<test data>/104319/<key>`; `<key>` is the hash of
/// the derivations' code, so a change to it derives again, and the derived
/// data are the current code's output. Every checkout of the same code
/// shares the folder, and the processes that ask at once derive it once
/// (`lucy_assets::cache::ensure_pack`).
///
/// The test data folder is `LUCY_TEST_DATA`, by default the main
/// checkout's `target/lucy-test-data`, which its worktrees share. A
/// process that derives deletes the data of other keys unused for
/// [`KEEP_UNUSED`].
pub fn runtime_dir() -> PathBuf {
    static DERIVED: LazyLock<PathBuf> = LazyLock::new(|| {
        let builds = env::var_os("LUCY_TEST_DATA")
            .map_or_else(
                || PathBuf::from(env!("LUCY_TEST_DATA_DEFAULT")),
                PathBuf::from,
            )
            .join(BUILD);
        let pack = builds.join(DERIVATIONS_KEY);
        let missing = !pack.is_dir();
        cache::ensure_pack(&game_dir(), "enUS", &pack)
            .unwrap_or_else(|error| panic!("cannot derive the game data: {error}"));
        File::create(used(&pack)).expect("the test data folder is writable");
        if missing {
            prune(&builds, &pack);
        }
        pack
    });
    DERIVED.clone()
}

/// A derived file of [`runtime_dir`], parsed.
pub fn runtime_json(path: &str) -> serde_json::Value {
    let bytes =
        fs::read(runtime_dir().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// The paths of the files the derivation `name` returned when it derived
/// [`runtime_dir`], in its order (`lucy_assets::OUTPUTS`).
pub fn derived_outputs(name: &str) -> Vec<String> {
    let path = runtime_dir().join(lucy_assets::OUTPUTS);
    let list =
        fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    list.lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(derivation, _)| *derivation == name)
        .map(|(_, path)| path.to_owned())
        .collect()
}

/// The file whose time is the last use of a derived pack.
fn used(pack: &Path) -> PathBuf {
    pack.with_extension("used")
}

/// Delete the packs in `builds` other than `current` unused for
/// [`KEEP_UNUSED`]: their stamp first, so a pack that can't be deleted whole
/// (a process still reads it) derives again rather than being trusted.
fn prune(builds: &Path, current: &Path) {
    let Ok(entries) = fs::read_dir(builds) else {
        return;
    };
    let now = SystemTime::now();
    for pack in entries.flatten().map(|entry| entry.path()) {
        let unused = fs::metadata(used(&pack))
            .and_then(|meta| meta.modified())
            .is_ok_and(|last| now.duration_since(last).unwrap_or_default() > KEEP_UNUSED);
        if pack == current || !pack.is_dir() || !unused {
            continue;
        }
        let _ = fs::remove_file(pack.join("derived.txt"));
        let _ = fs::remove_dir_all(&pack);
        let _ = fs::remove_file(pack.with_extension("lock"));
        let _ = fs::remove_file(used(&pack));
    }
}

/// The prepared runtime pack named by `LUCY_RUNTIME_DIR`: the files the
/// Python extractors made, the oracle the derivations are compared with.
pub fn oracle_pack() -> PathBuf {
    env::var_os("LUCY_RUNTIME_DIR").map(PathBuf::from).expect(
        "set LUCY_RUNTIME_DIR to a prepared runtime pack (just test-assets <install> <pack>)",
    )
}

/// The pack's bot graph bundle: the hero bundle plus the map AI spawner
/// graphs, the six Practice Range bot bodies with their AI definitions, and
/// the mode variables. The server can't run bot graphs yet, so it uses the
/// hero bundle; VM tests of bot graphs use this one.
pub fn bot_graphs() -> PathBuf {
    runtime_dir().join("data/game_graphs_bots_174.json.gz")
}

/// Whether the first `needle_bits` bits of `needle` appear in the first
/// `haystack_bits` bits of `haystack` at any bit offset, both read
/// least-significant bit first as the wire writes them: a value written
/// alone found inside a frame.
pub fn contains_bits(
    haystack: &[u8],
    haystack_bits: usize,
    needle: &[u8],
    needle_bits: usize,
) -> bool {
    let bit = |data: &[u8], index: usize| data[index / 8] >> (index % 8) & 1;
    needle_bits <= haystack_bits
        && (0..=haystack_bits - needle_bits)
            .any(|start| (0..needle_bits).all(|i| bit(haystack, start + i) == bit(needle, i)))
}
