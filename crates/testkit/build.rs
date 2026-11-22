//! What `runtime_dir` needs to know at build time:
//!
//! - `LUCY_DERIVATIONS_KEY`, a hash of the code the derived game data
//!   depends on, so the tests derive again whenever that code changes. The
//!   derivations' version alone would not do: two branches can bump it to the
//!   same number with different code, and a derivation under work changes
//!   before anyone bumps it.
//! - `LUCY_TEST_DATA_DEFAULT`, the folder the derived data goes to unless
//!   `LUCY_TEST_DATA` names another: the main checkout's
//!   `target/lucy-test-data`, which every worktree of the clone shares.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
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
    let crates = manifest.parent().expect("testkit is in crates/");
    let workspace = crates.parent().expect("crates/ is in the workspace");

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

    let checkout = main_checkout(workspace).unwrap_or_else(|| workspace.to_path_buf());
    println!(
        "cargo::rustc-env=LUCY_TEST_DATA_DEFAULT={}",
        checkout.join("target").join("lucy-test-data").display()
    );
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

/// The main checkout of the clone the workspace is in: the workspace itself,
/// or, in a linked worktree (whose `.git` is a file naming its folder under
/// the main checkout's `.git/worktrees`), the checkout that holds the
/// common `.git` folder.
fn main_checkout(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    if dot_git.is_dir() {
        return Some(workspace.to_path_buf());
    }
    println!("cargo::rerun-if-changed={}", dot_git.display());
    let text = fs::read_to_string(&dot_git).ok()?;
    let git_dir = workspace.join(text.trim().strip_prefix("gitdir:")?.trim());
    let common = fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common = normalize(&git_dir.join(common.trim()));
    Some(common.parent()?.to_path_buf())
}

/// `path` without `.` and `..` components.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}
