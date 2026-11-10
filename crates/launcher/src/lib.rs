//! Finds, starts and injects the Overwatch 1.74 client.
//!
//! This is one of the two crates allowed to use `unsafe` (the other is the
//! relay). Every `unsafe` block states why it is sound.

#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

pub mod game;
#[cfg(windows)]
pub mod process;
