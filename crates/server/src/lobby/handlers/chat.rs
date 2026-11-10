//! Chat: channel messages (21700, 21701), whispers (27004) and the `.hero`/`.leave` commands.

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
