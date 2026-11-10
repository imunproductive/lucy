//! Test support shared by the workspace crates.
//!
//! Tests that need build 104319 data are marked `#[ignore]` and find that data
//! only through this crate. Run them with `just test-game <install>`; the
//! derivation checks against a prepared pack run with `just test-assets
//! <install> <pack>`.

pub mod casc;

use std::{
    env,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The Overwatch 1.74 install named by `ORPHEUS_GAME_DIR`.
pub fn game_dir() -> PathBuf {
    env::var_os("ORPHEUS_GAME_DIR")
        .map(PathBuf::from)
        .expect("set ORPHEUS_GAME_DIR to an Overwatch 1.74 install (just test-game <install>)")
}

/// A DataTool XML export of the same install (`extract-stu-type <dump> <type>
/// --xml`), named by `ORPHEUS_DATATOOL_DUMP`; the oracle for the asset reader.
pub fn datatool_dump() -> PathBuf {
    env::var_os("ORPHEUS_DATATOOL_DUMP")
        .map(PathBuf::from)
        .expect("set ORPHEUS_DATATOOL_DUMP to a DataTool XML export (just test-datatool <install> <dump>)")
}

/// The game data the server reads, derived from the install named by
/// `ORPHEUS_GAME_DIR` once and kept in `target/orpheus-test-data` between runs.
pub fn runtime_dir() -> PathBuf {
    static DERIVED: OnceLock<PathBuf> = OnceLock::new();
    DERIVED
        .get_or_init(|| {
            let cache =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/orpheus-test-data");
            std::fs::create_dir_all(&cache).expect("the test data folder can be created");
            orpheus_assets::cache::ensure(&game_dir(), "enUS", &cache)
                .unwrap_or_else(|error| panic!("cannot derive the game data: {error}"))
        })
        .clone()
}

/// The prepared runtime pack named by `ORPHEUS_RUNTIME_DIR`: the files the
/// Python extractors made, the oracle the derivations are compared with.
pub fn oracle_pack() -> PathBuf {
    env::var_os("ORPHEUS_RUNTIME_DIR")
        .map(PathBuf::from)
        .expect("set ORPHEUS_RUNTIME_DIR to a prepared runtime pack (just test-assets <install> <pack>)")
}

/// The pack's bot graph bundle: the hero bundle plus the map AI spawner
/// graphs, the six Practice Range bot bodies with their AI definitions, and
/// the mode variables. The server can't run bot graphs yet, so it uses the
/// hero bundle; VM tests of bot graphs use this one.
pub fn bot_graphs() -> PathBuf {
    runtime_dir().join("data/game_graphs_bots_174.json.gz")
}
