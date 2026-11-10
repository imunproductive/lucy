//! Lobby services for retail build 104319: accounts, collection, login content, parties and match handoff.

#![forbid(unsafe_code)]

pub mod accounts;
pub mod arcade;
pub mod battle_tag_query;
pub mod celebrations;
pub mod clock;
pub mod collection;
pub mod economy;
pub mod gallery;
pub mod handoff;
pub mod leaderboard;
pub mod lineup;
pub mod login;
pub mod lookups;
pub mod lootbox;
pub mod matchmaker;
pub mod menu;
pub mod mode_rules;
pub mod party;
pub mod passes;
pub mod player_messages;
pub mod presence;
pub mod queue;
pub mod ranked;
pub mod saved_settings;
pub mod session;
pub mod shop;
pub mod social;
pub mod templates;
#[cfg(test)]
mod test_catalog;
pub mod transport;
