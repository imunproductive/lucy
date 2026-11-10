//! The lobby's feature modules. Each declares the client messages it answers
//! and its session hooks; [`MODULES`] lists them all.

mod chat;
mod friends;
mod gallery;
mod groups;
mod leaving;
mod lookups;
mod matchmaking;
mod party;
mod rewards;
mod settings;
mod shop;

use super::router::Module;

pub(crate) const MODULES: &[&Module] = &[
    &gallery::MODULE,
    &chat::MODULE,
    &friends::MODULE,
    &party::MODULE,
    &groups::MODULE,
    &matchmaking::MODULE,
    &leaving::MODULE,
    &settings::MODULE,
    &lookups::MODULE,
    &shop::MODULE,
    &rewards::MODULE,
];
