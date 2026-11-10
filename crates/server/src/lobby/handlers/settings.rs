//! Saved client settings (22200-22204, 22207) and the season intro (36200).

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
