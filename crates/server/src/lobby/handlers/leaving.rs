//! Leaving a match (52902, 21801) and the match lobby (53000, 53003).

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
