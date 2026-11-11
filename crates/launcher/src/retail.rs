//! The retail games one server starts, each with its own account.
//!
//! Ports `ow174/launcher/retail.py`. The first game plays the dashboard's
//! account, so "Play as" can switch it. A second game plays the account it
//! was started for. The Battle.net emulator and the lobby tell the games
//! apart by the process that owns each connection.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    network::connection_owner,
    process::{RelaySetup, inject_when_ready_with, start_game},
};

/// A second game must not take the screen from the first in exclusive
/// fullscreen. The client only masks that mode while it runs; the display
/// setting it saves stays as it was.
pub const SECOND_GAME_ARGUMENTS: [&str; 1] = ["--tank_NoFullScreen=1"];

/// What every game started here gets.
pub struct GameSetup {
    pub game: PathBuf,
    pub relay: PathBuf,
    /// `--tank_Locale`, if any.
    pub locale: Option<String>,
    /// The local Battle.net port the game dials.
    pub stall_port: u16,
    /// The Battle.net RPC port the relay pipes to.
    pub rpc_port: u16,
    pub game_log: PathBuf,
    pub relay_log: PathBuf,
    /// How long the game may take before the relay is in it.
    pub timeout: Duration,
    /// More arguments for the game, after the ones above (`orpheus play --
    /// ...`).
    pub extra_arguments: Vec<String>,
}

/// A started game with the relay loaded.
pub struct Started {
    pub pid: u32,
    /// The relay's base address in the game.
    pub base: usize,
}

type SecondGame = (Arc<Mutex<Child>>, String);

pub struct RetailGames {
    setup: GameSetup,
    /// PID → (game, account name).
    second: Mutex<HashMap<u32, SecondGame>>,
}

impl RetailGames {
    pub fn new(setup: GameSetup) -> Self {
        Self {
            setup,
            second: Mutex::default(),
        }
    }

    /// Start the game that plays the dashboard's account.
    pub fn start(&self) -> io::Result<(Child, Started)> {
        let mut child = self.spawn(false)?;
        let pid = child.id();
        match self.inject(pid, || child.try_wait()) {
            Ok(base) => Ok((child, Started { pid, base })),
            Err(error) => {
                // Without the relay the game waits at its login screen forever.
                let _ = child.kill();
                Err(error)
            }
        }
    }

    /// Start a second game that plays `account`, in a window.
    pub fn start_for(&self, account: &str) -> io::Result<Started> {
        let child = self.spawn(true)?;
        let pid = child.id();
        let child = Arc::new(Mutex::new(child));
        // Known before the relay can log it in.
        self.second
            .lock()
            .expect("second games")
            .insert(pid, (Arc::clone(&child), account.to_owned()));
        let exited = || child.lock().expect("second game").try_wait();
        match self.inject(pid, exited) {
            Ok(base) => Ok(Started { pid, base }),
            Err(error) => {
                let _ = child.lock().expect("second game").kill();
                Err(error)
            }
        }
    }

    /// The account of the second game that owns the connection from
    /// `peer_port` to `server_port`, while that game runs.
    pub fn account_of(&self, peer_port: u16, server_port: u16) -> Option<String> {
        let pid = connection_owner(peer_port, server_port)?;
        let (game, account) = self
            .second
            .lock()
            .expect("second games")
            .get(&pid)
            .cloned()?;
        running(&game).then_some(account)
    }

    /// Accounts a second game is playing.
    pub fn second_accounts(&self) -> HashSet<String> {
        self.second
            .lock()
            .expect("second games")
            .values()
            .filter(|(game, _)| running(game))
            .map(|(_, account)| account.clone())
            .collect()
    }

    fn spawn(&self, second: bool) -> io::Result<Child> {
        let setup = &self.setup;
        let mut arguments = vec![format!("--BNetServer=127.0.0.1:{}", setup.stall_port)];
        if second {
            arguments.extend(SECOND_GAME_ARGUMENTS.map(String::from));
        }
        if let Some(locale) = &setup.locale {
            arguments.push(format!("--tank_Locale={locale}"));
        }
        arguments.extend(setup.extra_arguments.iter().cloned());
        start_game(
            &setup.game,
            &arguments,
            &setup.game_log,
            Some(RelaySetup {
                log: &setup.relay_log,
                rpc_port: setup.rpc_port,
            }),
        )
    }

    fn inject(
        &self,
        pid: u32,
        exited: impl FnMut() -> io::Result<Option<std::process::ExitStatus>>,
    ) -> io::Result<usize> {
        inject_when_ready_with(pid, exited, &self.setup.relay, self.setup.timeout)
    }
}

fn running(game: &Mutex<Child>) -> bool {
    matches!(game.lock().expect("second game").try_wait(), Ok(None))
}
