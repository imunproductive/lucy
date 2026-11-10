//! `orpheus play`: the server, then the client with the relay loaded into it.

use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use orpheus_launcher::game::{GAME_EXE, TARGET_VERSION, build_info, choose_locale, resolve_game};
use tracing::info;

use crate::{serve, settings::Settings};

pub const RELAY_DLL: &str = "owwfd_relay.dll";
const INJECT_TIMEOUT: Duration = Duration::from_secs(30);
const CLOSE_PATIENCE: Duration = Duration::from_secs(6);

/// The game the settings point at, checked to be build 104319.
pub fn game(settings: &Settings) -> io::Result<PathBuf> {
    let path = settings.game.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no game configured: pass --game, set ORPHEUS_GAME, or add game = '<path>' to config.toml",
        )
    })?;
    let game = resolve_game(path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "cannot find {GAME_EXE} at {}; give the game, its installation folder or the official launcher",
                path.display()
            ),
        )
    })?;
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

/// The relay built next to this executable.
pub fn relay_dll() -> io::Result<PathBuf> {
    let path = std::env::current_exe()?.with_file_name(RELAY_DLL);
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
    orpheus_launcher::process::system_locale()
}

#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    None
}

#[cfg(not(windows))]
pub fn run(_: &Settings) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the Overwatch 1.74 client runs on Windows only",
    ))
}

#[cfg(windows)]
pub fn run(settings: &Settings) -> io::Result<()> {
    use orpheus_launcher::process;

    let game = game(settings)?;
    let relay = relay_dll()?;
    let mut server = serve::start(settings)?;
    info!(lobby = %server.lobby_addr(), "server running");
    if let Some(dashboard) = server.dashboard_addr() {
        info!("dashboard: http://{dashboard}");
    }
    let others = process::running_copies(&game)?.1;
    if !others.is_empty() {
        info!(pids = ?others, "another Overwatch is running; leaving it alone");
    }
    process::close_running_copy(&game, CLOSE_PATIENCE)?;

    let mut arguments = vec![format!("--BNetServer=127.0.0.1:{}", settings.ports.stall)];
    if let Some(locale) = locale(settings, &game) {
        info!(locale, "game language");
        arguments.push(format!("--tank_Locale={locale}"));
    }
    let logs = settings.logs();
    let relay_log = logs.join("relay.log");
    let mut child = process::start_game(
        &game,
        &arguments,
        &logs.join("game.log"),
        process::RelaySetup {
            log: &relay_log,
            rpc_port: settings.ports.rpc,
        },
    )?;
    match process::inject_when_ready(&mut child, &relay, INJECT_TIMEOUT) {
        Ok(base) => info!(
            pid = child.id(),
            base = format_args!("{base:#x}"),
            "relay loaded"
        ),
        Err(error) => {
            // Without the relay the game waits at its login screen forever.
            let _ = child.kill();
            let _ = server.stop();
            return Err(error);
        }
    }
    info!("press Ctrl+C to stop the server, or close the game");

    let (stop, stopped) = mpsc::channel();
    let ctrl_c = stop.clone();
    ctrlc::set_handler(move || {
        let _ = ctrl_c.send(());
    })
    .map_err(io::Error::other)?;
    thread::spawn(move || {
        let _ = child.wait();
        let _ = stop.send(());
    });
    let _ = stopped.recv();
    info!("game closed or Ctrl+C; stopping");
    server.stop()
}
