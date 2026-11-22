//! Process wiring for retail build 104319: the Battle.net, lobby and game services.

// `unsafe` only where the OS must be called: the room threads' timer
// resolution on Windows (`game_service/timer.rs`), allowed there alone.
#![deny(unsafe_code)]

pub mod clock;
pub mod connection;
pub mod game_data;
pub mod game_service;
pub mod ingress;
mod lobby;
pub mod platform;
pub mod practice;
pub mod server;
pub mod workshop_match;
