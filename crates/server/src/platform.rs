//! Hosting for other PCs, and the games this PC starts.
//!
//! Ports the hosting parts of `ow174/lobby/server.py` (`games`,
//! `game_account`, `reconnect_own_game`) and `game_host` from
//! `ow174/lobby/settings.py`.

use std::{
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// The retail games this PC started (`RetailGames` in
/// `ow174/launcher/retail.py`). Only modes that start a game with the relay
/// have them.
pub trait LocalGames: Send + Sync {
    /// Start a second game on this PC that plays `account`, in the
    /// background. The implementation reports a failure itself.
    fn start(&self, account: &str);
    /// The account of the second game that owns the TCP connection from
    /// `peer_port` to `server_port`, while that game runs.
    fn account_of(&self, peer_port: u16, server_port: u16) -> Option<String>;
    /// Accounts a second game is playing.
    fn second_accounts(&self) -> HashSet<String>;
}

/// What the lobby, Battle.net and the dashboard share about hosting.
pub(crate) struct Platform {
    /// Where the lobby and the game server listen.
    pub host: Ipv4Addr,
    /// The public address games outside the host's network are sent to for a
    /// match (`--game-host`); empty for none.
    pub game_host: String,
    pub games: Option<Arc<dyn LocalGames>>,
    /// Logged-in games on this PC by account: name and drop request.
    local: Mutex<HashMap<u32, (String, Arc<AtomicBool>)>>,
}

impl Platform {
    pub fn new(host: Ipv4Addr, game_host: String, games: Option<Arc<dyn LocalGames>>) -> Self {
        Self {
            host,
            game_host,
            games,
            local: Mutex::default(),
        }
    }

    /// The name of the account a retail game on this PC plays, found by its
    /// connection: a second game plays the account it was started for. `None`
    /// is the first game, which plays the dashboard's account.
    pub fn game_account_name(&self, peer_port: u16, server_port: u16) -> Option<String> {
        self.games.as_ref()?.account_of(peer_port, server_port)
    }

    /// Accounts a second game on this PC is playing.
    pub fn second_accounts(&self) -> HashSet<String> {
        self.games
            .as_ref()
            .map(|games| games.second_accounts())
            .unwrap_or_default()
    }

    /// Note a game on this PC that logged in, until the returned guard drops.
    pub fn track_local(&self, account_lo: u32, name: &str) -> LocalSession<'_> {
        let dropped = Arc::new(AtomicBool::new(false));
        self.local
            .lock()
            .expect("local sessions")
            .insert(account_lo, (name.to_owned(), Arc::clone(&dropped)));
        LocalSession {
            platform: self,
            account_lo,
            dropped,
        }
    }

    /// Drop the game played on this PC so it logs in again, as the
    /// dashboard's account now (the menu scene only changes at login). Second
    /// games and other players' games stay. Returns how many were dropped.
    pub fn reconnect_own_game(&self) -> usize {
        let second = self.second_accounts();
        let local = self.local.lock().expect("local sessions");
        local
            .values()
            .filter(|(name, _)| !second.contains(name))
            .map(|(_, dropped)| dropped.store(true, Ordering::Release))
            .count()
    }
}

/// A logged-in game on this PC.
pub(crate) struct LocalSession<'a> {
    platform: &'a Platform,
    account_lo: u32,
    dropped: Arc<AtomicBool>,
}

impl LocalSession<'_> {
    /// The dashboard asked to drop this game's connection.
    pub fn dropped(&self) -> bool {
        self.dropped.load(Ordering::Acquire)
    }
}

impl Drop for LocalSession<'_> {
    fn drop(&mut self) {
        let mut local = self.platform.local.lock().expect("local sessions");
        if local
            .get(&self.account_lo)
            .is_some_and(|(_, dropped)| Arc::ptr_eq(dropped, &self.dropped))
        {
            local.remove(&self.account_lo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SecondGame;

    impl LocalGames for SecondGame {
        fn start(&self, _: &str) {}
        fn account_of(&self, peer_port: u16, server_port: u16) -> Option<String> {
            ((peer_port, server_port) == (50_000, 21119)).then(|| "Second".into())
        }
        fn second_accounts(&self) -> HashSet<String> {
            HashSet::from(["Second".into()])
        }
    }

    fn with_games(games: Option<Arc<dyn LocalGames>>) -> Platform {
        Platform::new(Ipv4Addr::LOCALHOST, String::new(), games)
    }

    #[test]
    fn a_second_game_plays_its_own_account_and_the_first_the_dashboards() {
        let platform = with_games(Some(Arc::new(SecondGame)));
        assert_eq!(
            platform.game_account_name(50_000, 21119).as_deref(),
            Some("Second")
        );
        assert_eq!(platform.game_account_name(50_001, 21119), None);
        assert_eq!(with_games(None).game_account_name(50_000, 21119), None);
    }

    #[test]
    fn reconnect_drops_only_the_first_game_on_this_pc() {
        let platform = with_games(Some(Arc::new(SecondGame)));
        let first = platform.track_local(1, "First");
        let second = platform.track_local(2, "Second");
        assert_eq!(platform.reconnect_own_game(), 1);
        assert!(first.dropped());
        assert!(!second.dropped());

        // A newer login of the same account keeps its entry when the old one ends.
        let newer = platform.track_local(1, "First");
        drop(first);
        assert_eq!(platform.reconnect_own_game(), 1);
        assert!(newer.dropped());
        drop(newer);
        assert_eq!(platform.reconnect_own_game(), 0);
    }
}
