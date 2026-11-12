//! Tests: the lobby on a virtual clock ([`orpheus_lobby::clock::Clock`]).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use orpheus_lobby::clock::Clock;

/// A virtual clock at zero that only the test moves.
#[derive(Clone, Default)]
pub(crate) struct VirtualClock(Arc<Mutex<Duration>>);

impl VirtualClock {
    /// The lobby's clock that reads this one.
    pub(crate) fn clock(&self) -> Clock {
        let virtual_clock = self.clone();
        Clock::new(move || virtual_clock.now())
    }

    pub(crate) fn now(&self) -> Duration {
        *self.0.lock().expect("virtual clock")
    }

    pub(crate) fn advance(&self, time: Duration) {
        *self.0.lock().expect("virtual clock") += time;
    }
}
