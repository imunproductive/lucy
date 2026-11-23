//! The modes that start the game here: `play` (retail), `host` and
//! `tournament` (`run` in `ow174/cli.py`).

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use lucy_launcher::game::{
    GAME_EXE, GAME_PATH_FILE, TARGET_VERSION, build_info, choose_locale, find_game,
};

use crate::settings::Settings;

pub const RELAY_DLL: &str = "owwfd_relay.dll";
/// The client probe `--probe` loads (crates/probe).
#[cfg(windows)]
pub const PROBE_DLL: &str = "owprobe.dll";

/// The game to start without asking, checked to be build 104319: the
/// configured one, the one picked last time or an unpacked copy in the
/// current folder's `game`. `Ok(None)` means the player has to pick one.
pub fn known_game(settings: &Settings) -> io::Result<Option<PathBuf>> {
    let saved = settings.state.join(GAME_PATH_FILE);
    find_game(settings.game.as_deref(), &saved, &env::current_dir()?)?
        .map(checked)
        .transpose()
}

/// The game to start: a known one, else one the player picks now. Either
/// way it is remembered for next time, so commands that only read the
/// game's data (`assets`, `script query`) find it without asking.
pub fn game(settings: &Settings) -> io::Result<PathBuf> {
    if let Some(game) = known_game(settings)? {
        if settings.game.is_some() {
            remember(settings, &game)?;
        }
        return Ok(game);
    }
    let picked = checked(pick_game()?)?;
    remember(settings, &picked)?;
    Ok(picked)
}

/// The game for a command that only reads its data: a known one, never a
/// file dialog (scripts and agents run these without anyone to answer it).
pub fn configured_game(settings: &Settings) -> io::Result<PathBuf> {
    known_game(settings)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no game configured: pass --game with the path of {GAME_EXE}, set LUCY_GAME, \
                 or run `lucy play` once to pick it"
            ),
        )
    })
}

fn remember(settings: &Settings, game: &Path) -> io::Result<()> {
    let saved = settings.state.join(GAME_PATH_FILE);
    let text = game.display().to_string();
    if fs::read_to_string(&saved).is_ok_and(|old| old.trim() == text) {
        return Ok(());
    }
    fs::create_dir_all(&settings.state)?;
    fs::write(saved, text)
}

fn checked(game: PathBuf) -> io::Result<PathBuf> {
    let version = build_info(&game).and_then(|info| info.version);
    if version.as_deref() != Some(TARGET_VERSION) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{} is build {}, not {TARGET_VERSION}",
                game.display(),
                version.as_deref().unwrap_or("unknown")
            ),
        ));
    }
    Ok(game)
}

/// Ask for the game in a file dialog until it is one.
#[cfg(windows)]
fn pick_game() -> io::Result<PathBuf> {
    use lucy_launcher::{dialog::open_game_dialog, game::resolve_game};
    use tracing::{info, warn};

    info!("Select the game's {GAME_EXE} (the game itself, not a shortcut or a launcher).");
    loop {
        let Some(chosen) = open_game_dialog()? else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("No game selected. Start again and pick {GAME_EXE}."),
            ));
        };
        if let Some(game) = resolve_game(&chosen) {
            return Ok(game);
        }
        warn!("Cannot find the game there. Select _retail_\\{GAME_EXE}.");
    }
}

#[cfg(not(windows))]
fn pick_game() -> io::Result<PathBuf> {
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no game configured: pass --game with the path of {GAME_EXE}"),
    ))
}

/// The relay built next to this executable.
pub fn relay_dll() -> io::Result<PathBuf> {
    built_dll(RELAY_DLL)
}

/// A DLL built next to this executable.
fn built_dll(name: &str) -> io::Result<PathBuf> {
    let path = env::current_exe()?.with_file_name(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is missing; build it with cargo build --workspace",
                path.display()
            ),
        ))
    }
}

/// The client probe for `--probe PORT`, built next to this executable.
#[cfg(windows)]
pub fn probe_setup(
    settings: &Settings,
    port: Option<u16>,
) -> io::Result<Option<lucy_launcher::process::ProbeSetup>> {
    port.map(|port| {
        Ok(lucy_launcher::process::ProbeSetup {
            dll: built_dll(PROBE_DLL)?,
            port,
            log: settings.logs().join("probe.log"),
        })
    })
    .transpose()
}

/// The `--tank_Locale` to pass, if any.
pub fn locale(settings: &Settings, game: &Path) -> Option<String> {
    match settings.locale.as_str() {
        "none" => None,
        "auto" => {
            let installed = build_info(game)?.locales;
            choose_locale(&installed, system_locale().as_deref())
        }
        code => Some(code.into()),
    }
}

#[cfg(windows)]
fn system_locale() -> Option<String> {
    lucy_launcher::process::system_locale()
}

#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    None
}

#[cfg(windows)]
pub use client::{
    close_running_copy, log_relay, retail, start_tournament_game, tournament, wait_for,
};

#[cfg(windows)]
mod client {
    use std::{
        io,
        path::Path,
        process::Child,
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    use lucy_launcher::{
        process,
        retail::{GameSetup, RetailGames, Started},
    };
    use lucy_server::platform::LocalGames;
    use tracing::{error, info};

    use super::{game, locale, probe_setup, relay_dll};
    use crate::{
        banner,
        serve::{self, Hosting},
        settings::Settings,
    };

    const CLOSE_PATIENCE: Duration = Duration::from_secs(6);

    /// The retail game with the relay, on a server for this PC (`play`) or
    /// for other PCs too (`host`). A second game can be started from the
    /// dashboard; the server stops when the first game closes. `probe`: the
    /// port of the client probe to load into the first game.
    pub fn retail(
        settings: &Settings,
        hosting: Hosting,
        timeout: Duration,
        probe: Option<u16>,
    ) -> io::Result<()> {
        let game = game(settings)?;
        let relay = relay_dll()?;
        let probe = probe_setup(settings, probe)?;
        close_running_copy(&game)?;
        let logs = settings.logs();
        let games = Arc::new(RetailGames::new(GameSetup {
            locale: logged_locale(settings, &game),
            game,
            relay,
            stall_port: settings.ports.stall,
            rpc_port: settings.ports.rpc,
            game_log: logs.join("game.log"),
            relay_log: logs.join("relay.log"),
            timeout,
            extra_arguments: settings.game_args.clone(),
            probe,
        }));
        let game_host = hosting.game_host.clone();
        let mut server = serve::start(
            settings,
            Hosting {
                games: Some(Arc::new(DashboardGames(Arc::clone(&games)))),
                ..hosting
            },
        )?;
        banner::log(&server, settings, &game_host);
        let child = match games.start() {
            Ok((child, started)) => {
                log_relay(&started, None);
                child
            }
            Err(error) => {
                let _ = server.stop();
                return Err(error);
            }
        };
        info!("Keep this window open while you play; closing it or the game stops the server.");
        wait_for(Some(child))?;
        info!("Stopped.");
        server.stop()
    }

    /// The game in tournament mode on this PC's server: it dials the lobby
    /// itself, with no relay and no Battle.net.
    pub fn tournament(settings: &Settings, hosting: Hosting) -> io::Result<()> {
        let game = game(settings)?;
        close_running_copy(&game)?;
        let game_host = hosting.game_host.clone();
        let mut server = serve::start(settings, hosting)?;
        banner::log(&server, settings, &game_host);
        let lobby = format!("127.0.0.1:{}", server.lobby_addr().port());
        if let Err(error) = start_tournament_game(settings, &game, &lobby) {
            let _ = server.stop();
            return Err(error);
        }
        info!("Keep this window open while you play; closing it or Ctrl+C stops the server.");
        wait_for(None)?;
        info!("Stopped.");
        server.stop()
    }

    /// Start the game in tournament mode on the lobby at `address`.
    pub fn start_tournament_game(
        settings: &Settings,
        game: &Path,
        address: &str,
    ) -> io::Result<()> {
        let mut arguments = vec![
            "--tank_TournamentMode".to_owned(),
            format!("--lobbyServer={address}"),
        ];
        if let Some(locale) = logged_locale(settings, game) {
            arguments.push(format!("--tank_Locale={locale}"));
        }
        arguments.extend(settings.game_args.iter().cloned());
        process::start_game(game, &arguments, &settings.logs().join("game.log"), None)?;
        Ok(())
    }

    /// Close a copy of this game left from an earlier run; other installs
    /// stay.
    pub fn close_running_copy(game: &Path) -> io::Result<()> {
        let others = process::running_copies(game)?.1;
        if !others.is_empty() {
            info!(pids = ?others, "Another Overwatch is running; leaving it alone.");
        }
        process::close_running_copy(game, CLOSE_PATIENCE)
    }

    /// Wait for Ctrl+C, or for the game to close.
    pub fn wait_for(game: Option<Child>) -> io::Result<()> {
        let (stop, stopped) = mpsc::channel();
        let ctrl_c = stop.clone();
        ctrlc::set_handler(move || {
            let _ = ctrl_c.send(());
        })
        .map_err(io::Error::other)?;
        if let Some(mut game) = game {
            thread::spawn(move || {
                let _ = game.wait();
                let _ = stop.send(());
            });
        }
        stopped.recv().map_err(io::Error::other)
    }

    fn logged_locale(settings: &Settings, game: &Path) -> Option<String> {
        let locale = locale(settings, game);
        if let Some(locale) = &locale {
            info!("Game language: {locale}");
        }
        locale
    }

    pub fn log_relay(started: &Started, account: Option<&str>) {
        let playing = account
            .map(|account| format!(" for {account}"))
            .unwrap_or_default();
        info!(
            "[+] Relay loaded into Overwatch (PID {}) at {:#X}{playing}.",
            started.pid, started.base
        );
        if let Some(probe) = started.probe {
            info!("[+] Client probe loaded at {probe:#X} (tools/owprobe).");
        }
    }

    /// The dashboard's view of the games started here.
    struct DashboardGames(Arc<RetailGames>);

    impl LocalGames for DashboardGames {
        fn start(&self, account: &str) {
            let games = Arc::clone(&self.0);
            let account = account.to_owned();
            let started = thread::Builder::new().name("game".into()).spawn(move || {
                match games.start_for(&account) {
                    Ok(started) => log_relay(&started, Some(&account)),
                    Err(problem) => {
                        error!("The second game for {account} did not start: {problem}");
                    }
                }
            });
            if let Err(problem) = started {
                error!("The second game did not start: {problem}");
            }
        }

        fn account_of(&self, peer_port: u16, server_port: u16) -> Option<String> {
            self.0.account_of(peer_port, server_port)
        }

        fn second_accounts(&self) -> std::collections::HashSet<String> {
            self.0.second_accounts()
        }
    }
}
