//! The derived pack of one install, kept between runs.
//!
//! The server reads its game data from `<state>/cache/104319`, a folder in
//! the runtime pack's layout. It is derived from the install on the first
//! start and again whenever the install's build or the derivations change.

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use orpheus_casc::BuildInfo;

use crate::{Install, Result, asset_error, build};

/// The build the derivations read.
pub const BUILD: &str = "104319";
/// Bump when a derivation's output changes, so caches are derived again.
pub const DERIVATIONS_VERSION: u32 = 53;
const STAMP: &str = "derived.txt";

/// The derived pack for `install` under `cache_root`, derived first if it is
/// missing or stale. Returns the pack folder.
pub fn ensure(install: &Path, locale: &str, cache_root: &Path) -> Result<PathBuf> {
    let pack = cache_root.join(BUILD);
    ensure_pack(install, locale, &pack)?;
    Ok(pack)
}

/// Derive the pack for `install` into the folder `pack` if it is missing or
/// stale. A current pack costs one read of `.build.info` and of the stamp;
/// only a derivation opens the install's storage.
///
/// Several processes may ask at once (a server and a `script query`, test
/// processes of several checkouts that share a folder): they take turns on
/// a lock file beside the pack, `<pack>.lock`, so one derives and the others
/// then find its pack current. The deriving process builds in a folder of its
/// own and renames it to `pack` once it is stamped, so a reader without the
/// lock sees either no pack or a whole one.
pub fn ensure_pack(install: &Path, locale: &str, pack: &Path) -> Result<()> {
    let stamp = stamp(install, locale)?;
    let current =
        |pack: &Path| fs::read_to_string(pack.join(STAMP)).is_ok_and(|found| found == stamp);
    if current(pack) {
        return Ok(());
    }
    let (Some(parent), Some(name)) = (pack.parent(), pack.file_name()) else {
        return Err(asset_error(format!(
            "{} can't hold a derived pack",
            pack.display()
        )));
    };
    let name = name.to_string_lossy();
    fs::create_dir_all(parent)?;
    let _lock = lock(&parent.join(format!("{name}.lock")))?;
    if current(pack) {
        return Ok(());
    }
    let opened = Install::open(install, locale)?;
    // Any other process's folder is what a killed derivation left: the
    // lock holder is the only one deriving.
    let prefix = format!("{name}.new.");
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            remove_folder(&entry.path())?;
        }
    }
    let fresh = parent.join(format!("{prefix}{}", std::process::id()));
    fs::create_dir_all(&fresh)?;
    build(&opened, &fresh)?;
    fs::write(fresh.join(STAMP), &stamp)?;
    remove_folder(pack)?;
    fs::rename(&fresh, pack)?;
    Ok(())
}

/// Hold the lock file `path` (created if missing) until the returned file is
/// dropped, waiting while another process holds it. The system lets it go
/// when its process ends, however it ends.
// The workspace's `rust-version` predates `File::lock` (1.89), but the
// pinned toolchain (rust-toolchain.toml) has it.
#[allow(clippy::incompatible_msrv)]
fn lock(path: &Path) -> io::Result<File> {
    let file = File::create(path)?;
    file.lock()?;
    Ok(file)
}

fn remove_folder(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// What the derived files depend on: the install's build and locale and the
/// derivations themselves.
fn stamp(install: &Path, locale: &str) -> Result<String> {
    let info = BuildInfo::read(install)
        .map_err(|error| asset_error(format!("{}: {error}", install.display())))?;
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
        "build {version}\nbuild config {key}\nlocale {locale}\nderivations {DERIVATIONS_VERSION}\n"
    ))
}
