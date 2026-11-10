//! Event greetings and login gifts (38901), challenge rewards (24901), team-skin pairs, and the account features (55500) sent again after login.

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
