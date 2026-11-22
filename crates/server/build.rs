//! Two things for the server crate:
//!
//! - Embed the dashboard's built files (`web/dashboard/dist`, committed; `npm
//!   run build` there makes them): a table of URL path, contents and MIME type
//!   for `server/dashboard.rs`. Building the server needs no Node.
//! - `cfg(unwinds)`: this build catches a panic. The dev profile builds this
//!   crate with Cranelift (the workspace's `Cargo.toml`), which can't unwind
//!   on Windows: a panic aborts the process. The release profile and those
//!   that inherit it (`gametest`) use LLVM and unwind, so a room that panics
//!   ends alone (`game_service/room.rs`). The test of that runs only there.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    unwinds();
    dashboard_files();
}

fn unwinds() {
    println!("cargo::rustc-check-cfg=cfg(unwinds)");
    if env::var("PROFILE").as_deref() == Ok("release") {
        println!("cargo::rustc-cfg=unwinds");
    }
}

fn dashboard_files() {
    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
    let dist = manifest.join("../../web/dashboard/dist");
    println!("cargo::rerun-if-changed={}", dist.display());

    let mut files = Vec::new();
    collect(&dist, &dist, &mut files);
    assert!(
        files.iter().any(|(url, _)| url == "/index.html"),
        "{} has no index.html: run `just dashboard-build`",
        dist.display()
    );
    files.sort();

    let mut table = String::from(
        "/// The dashboard's files: URL path, contents, MIME type.\npub(super) const FILES: &[(&str, &[u8], &str)] = &[\n",
    );
    for (url, path) in &files {
        let mime = mime(path)
            .unwrap_or_else(|| panic!("{}: no MIME type for this extension", path.display()));
        table.push_str(&format!(
            "    ({url:?}, include_bytes!({:?}), {mime:?}),\n",
            path.display().to_string()
        ));
    }
    table.push_str("];\n");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    fs::write(out.join("dashboard_files.rs"), table).expect("write the dashboard file table");
}

fn collect(root: &Path, folder: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = fs::read_dir(folder).unwrap_or_else(|error| {
        panic!("{}: {error}: run `just dashboard-build`", folder.display())
    });
    for entry in entries {
        let path = entry.expect("a dashboard file").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path.strip_prefix(root).expect("inside dist");
            let url = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            files.push((format!("/{url}"), path));
        }
    }
}

fn mime(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "html" => "text/html",
        "js" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "json" => "application/json",
        "txt" => "text/plain",
        _ => return None,
    })
}
