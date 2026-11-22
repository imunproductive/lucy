//! Which handler answers each client message after login.
//!
//! Each feature module under `handlers` declares its own routes and hooks
//! in a [`Module`]; `handlers::MODULES` lists them all. Ports the Python
//! `ow174/lobby/router.py`.

use std::{collections::HashMap, io, sync::Arc};

use lucy_lobby::accounts::Account;
use lucy_wire::jam::codec::Fields;

use super::{Lobby, peer::Peer};

pub(crate) type Handler = fn(&mut Peer<'_, '_>, Fields) -> io::Result<()>;

/// One client message a module answers: its protocol group, its id and the
/// handler.
pub(crate) struct Route {
    pub group: u32,
    pub message: u32,
    pub handle: Handler,
}

/// What a module does around the messages it answers.
pub(crate) struct Hooks {
    /// After the login messages, before the first client message.
    pub login: Option<fn(&mut Peer<'_, '_>) -> io::Result<()>>,
    /// Each time the session wakes: after each client message, on mail, and
    /// at least every [`TICK`](super::TICK).
    pub tick: Option<fn(&mut Peer<'_, '_>) -> io::Result<()>>,
    /// After the session ended, unless a newer session of the account
    /// replaced it.
    pub logout: Option<fn(&Lobby, &Arc<Account>)>,
}

impl Hooks {
    pub const NONE: Self = Self {
        login: None,
        tick: None,
        logout: None,
    };
}

pub(crate) struct Module {
    pub routes: &'static [Route],
    pub hooks: Hooks,
}

pub(crate) struct Router {
    handlers: HashMap<(u32, u32), Handler>,
}

impl Router {
    /// # Panics
    /// When two modules answer the same message, as the Python router
    /// refused it.
    pub(crate) fn new(modules: &[&Module]) -> Self {
        let mut handlers = HashMap::new();
        for route in modules.iter().flat_map(|module| module.routes) {
            let previous = handlers.insert((route.group, route.message), route.handle);
            assert!(
                previous.is_none(),
                "message {:08X}/{} already has a handler",
                route.group,
                route.message
            );
        }
        Self { handlers }
    }

    pub(crate) fn get(&self, group: u32, message: u32) -> Option<Handler> {
        self.handlers.get(&(group, message)).copied()
    }
}
