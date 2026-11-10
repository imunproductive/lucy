//! Friends and presence: requests (27000-27002), the status dropdown (27011), the friends list and cards at login, and friends told of logins and logouts.

use crate::lobby::router::{Hooks, Module};

pub(crate) const MODULE: Module = Module {
    routes: &[],
    hooks: Hooks::NONE,
};
