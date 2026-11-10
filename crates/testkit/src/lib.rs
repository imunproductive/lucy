//! Test support shared by the workspace crates.
//!
//! Tests that need authorized build 104319 data are marked `#[ignore]` and find
//! that data only through this crate. Run them with `just test-runtime <pack>`,
//! or `just test-game <install>` for the ones that read the game install.

pub mod casc;

use std::{env, path::PathBuf};

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

/// The prepared runtime pack named by `ORPHEUS_RUNTIME_DIR`.
pub fn runtime_dir() -> PathBuf {
    env::var_os("ORPHEUS_RUNTIME_DIR")
        .map(PathBuf::from)
        .expect("set ORPHEUS_RUNTIME_DIR to a prepared runtime pack (just test-runtime <pack>)")
}

/// The pack's bot graph bundle: the hero bundle plus the map AI spawner
/// graphs, the six Practice Range bot bodies with their AI definitions, and
/// the mode variables. The server can't run bot graphs yet, so it uses the
/// hero bundle; VM tests of bot graphs use this one.
pub fn bot_graphs() -> PathBuf {
    runtime_dir().join("data/game_graphs_bots_174.json.gz")
}
