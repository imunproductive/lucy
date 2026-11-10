//! The derived pack of one install, kept between runs.
//!
//! The server reads its game data from `<state>/cache/104319`, a folder in
//! the runtime pack's layout. It is derived from the install on the first
//! start and again whenever the install's build or the derivations change.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{Install, Result, asset_error, build};

/// The build the derivations read.
pub const BUILD: &str = "104319";
/// Bump when a derivation's output changes, so caches are derived again.
pub const DERIVATIONS_VERSION: u32 = 7;
const STAMP: &str = "derived.txt";

/// The derived pack for `install` under `cache_root`, derived first if it is
/// missing or stale. Returns the pack folder.
pub fn ensure(install: &Path, locale: &str, cache_root: &Path) -> Result<PathBuf> {
    let pack = cache_root.join(BUILD);
    let opened = Install::open(install, locale)?;
    let stamp = stamp(&opened)?;
    if fs::read_to_string(pack.join(STAMP)).is_ok_and(|found| found == stamp) {
        return Ok(pack);
    }
    let fresh = cache_root.join(format!("{BUILD}.new"));
    match fs::remove_dir_all(&fresh) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::create_dir_all(&fresh)?;
    build(&opened, &fresh)?;
    fs::write(fresh.join(STAMP), &stamp)?;
    match fs::remove_dir_all(&pack) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    fs::rename(&fresh, &pack)?;
    Ok(pack)
}

/// What the derived files depend on: the install's build and locale and the
/// derivations themselves.
fn stamp(install: &Install) -> Result<String> {
    let info = install.game().storage().info();
    let key = info
        .get("BuildKey")
        .ok_or_else(|| asset_error(".build.info has no build key"))?;
    let version = info.get("Version").unwrap_or_default();
    if !version.ends_with(BUILD) {
        return Err(asset_error(format!(
            "the install is build {version}; the server needs 1.74.0.0.{BUILD}"
        )));
    }
    Ok(format!(
        "build {version}\nbuild config {key}\nlocale {}\nderivations {DERIVATIONS_VERSION}\n",
        install.locale()
    ))
}
