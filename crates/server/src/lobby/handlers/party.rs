//! Parties: invites, joins, merges, leaders, kicks and leaving (22102-22111).

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
