//! Test support shared by the workspace crates.
//!
//! Tests that need authorized build 104319 data are marked `#[ignore]` and find
//! that data only through this crate. Run them with `just test-runtime <pack>`.

use std::{env, path::PathBuf};

/// The prepared runtime pack named by `ORPHEUS_RUNTIME_DIR`.
pub fn runtime_dir() -> PathBuf {
    env::var_os("ORPHEUS_RUNTIME_DIR")
        .map(PathBuf::from)
        .expect("set ORPHEUS_RUNTIME_DIR to a prepared runtime pack (just test-runtime <pack>)")
}

/// The pack's Battle.net descriptor set, or `ORPHEUS_BNET_DESCRIPTORS` when set.
pub fn bnet_descriptors() -> PathBuf {
    env::var_os("ORPHEUS_BNET_DESCRIPTORS")
        .map_or_else(|| runtime_dir().join("bnet/descriptors.pb"), PathBuf::from)
}
