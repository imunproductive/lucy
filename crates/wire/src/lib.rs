//! Byte-exact encodings for retail build 104319: the JAM lobby protocol, the game link, entity records and input commands.

#![forbid(unsafe_code)]

pub mod combat_messages;
pub mod commands;
pub mod entity;
pub mod jam;
pub mod link;
pub mod messages;
#[cfg(test)]
mod protocol_tests;
pub mod tcp;
