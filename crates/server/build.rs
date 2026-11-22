//! `cfg(unwinds)`: this build catches a panic. The dev profile builds this
//! crate with Cranelift (the workspace's `Cargo.toml`), which can't unwind
//! on Windows: a panic aborts the process. The release profile and those
//! that inherit it (`gametest`) use LLVM and unwind, so a room that panics
//! ends alone (`game_service/room.rs`). The test of that runs only there.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(unwinds)");
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("PROFILE").as_deref() == Ok("release") {
        println!("cargo::rustc-cfg=unwinds");
    }
}
