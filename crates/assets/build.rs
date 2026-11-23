//! `LUCY_DERIVATIONS_KEY`, a hash of the code the derived game data depends
//! on (`cache::DERIVATIONS_KEY`): the cache stamp holds it, so data derived by
//! other code is derived again. The derivations' version alone would not do:
//! two branches can bump it to the same number with different code, and a
//! derivation under work changes before anyone bumps it.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use md5::{Digest, Md5};

/// The crates whose code makes the derived files: the derivations and the
/// crates they read the install and the fixed data with
/// (`lucy-assets`'s path dependencies).
const DERIVATION_CRATES: &[&str] = &["assets", "casc", "fixed", "stu"];
/// The parts of a crate that go into its library (its tests don't).
const LIBRARY_PARTS: &[&str] = &["Cargo.toml", "build.rs", "src", "data"];

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let crates = manifest.parent().expect("assets is in crates/");

    let mut files = Vec::new();
    for name in DERIVATION_CRATES {
        for part in LIBRARY_PARTS {
            let path = crates.join(name).join(part);
            // A path that doesn't exist would rerun the script on every build.
            if path.exists() {
                println!("cargo::rerun-if-changed={}", path.display());
                collect(&path, &mut files);
            }
        }
    }
    files.sort();
    let mut hash = Md5::new();
    for file in &files {
        let name = file.strip_prefix(crates).expect("collected under crates/");
        let name = name.to_string_lossy().replace('\\', "/");
        // A checkout with CRLF line ends has the same code as one with LF.
        let bytes = fs::read(file).expect("a source file can be read");
        let bytes = String::from_utf8(bytes)
            .map(|text| text.replace("\r\n", "\n").into_bytes())
            .unwrap_or_else(|error| error.into_bytes());
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    let key: String = hash.finalize()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!("cargo::rustc-env=LUCY_DERIVATIONS_KEY={key}");
}

/// Every file under `path`.
fn collect(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_dir() {
        for entry in fs::read_dir(path).expect("a source folder can be listed") {
            collect(&entry.expect("a source folder can be listed").path(), files);
        }
    } else {
        files.push(path.to_path_buf());
    }
}
