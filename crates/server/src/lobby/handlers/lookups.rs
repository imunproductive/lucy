//! Lookups: career profiles (22206), the leaderboard (39100), player names (58202) and the OWL poll (42600).

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
