//! Queues: entering, cancelling, accepting, roles and priority passes (44100-44107), the matchmaker and Game Found.

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
