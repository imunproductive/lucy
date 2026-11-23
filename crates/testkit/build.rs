//! What `runtime_dir` needs to know at build time: `LUCY_TEST_DATA_DEFAULT`,
//! the folder the derived data goes to unless `LUCY_TEST_DATA` names another:
//! the main checkout's `target/lucy-test-data`, which every worktree of the
//! clone shares.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let workspace = manifest
        .parent()
        .and_then(Path::parent)
        .expect("testkit is in crates/ in the workspace");
    let checkout = main_checkout(workspace).unwrap_or_else(|| workspace.to_path_buf());
    println!(
        "cargo::rustc-env=LUCY_TEST_DATA_DEFAULT={}",
        checkout.join("target").join("lucy-test-data").display()
    );
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
