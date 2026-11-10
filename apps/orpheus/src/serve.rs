//! `orpheus serve`: the Battle.net, lobby and game services until Ctrl+C.

use std::{fs, io, path::PathBuf, sync::mpsc};

use orpheus_launcher::game::install_root;
use orpheus_server::server::{Server, ServerConfig};
use tracing::info;

use crate::{play, settings::Settings};

/// The language of display strings in derived data when the settings don't
/// name one; server logic keys only on GUIDs.
const TEXT_LOCALE: &str = "enUS";

pub fn run(settings: &Settings) -> io::Result<()> {
    let mut server = start(settings)?;
    info!(
        lobby = %server.lobby_addr(),
        game = %server.game_addr(),
        battle_net = %server.stall_addr(),
        rpc = %server.rpc_addr(),
        "server running; press Ctrl+C to stop"
    );
    if let Some(dashboard) = server.dashboard_addr() {
        info!("dashboard: http://{dashboard}");
    }
    wait_for_ctrl_c()?;
    info!("stopping");
    server.stop()
}

pub fn start(settings: &Settings) -> io::Result<Server> {
    let data = game_data(settings)?;
    let mut config = ServerConfig::new(data, settings.state.clone());
    config.account = settings.account.clone();
    config.lobby_port = settings.ports.lobby;
    config.game_port = settings.ports.game;
    config.stall_port = settings.ports.stall;
    config.rpc_port = settings.ports.rpc;
    config.dashboard_port = Some(settings.ports.dashboard);
    Server::start(config)
}

/// The game data the server reads, derived from the configured install on
/// the first start and kept in the state folder's cache.
pub fn game_data(settings: &Settings) -> io::Result<PathBuf> {
    let game = play::game(settings)?;
    let install = install_root(&game).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} has no .build.info next to it", game.display()),
        )
    })?;
    let locale = play::locale(settings, &game).unwrap_or_else(|| TEXT_LOCALE.into());
    fs::create_dir_all(settings.cache())?;
    info!(install = %install.display(), "reading game data from the install");
    orpheus_assets::cache::ensure(&install, &locale, &settings.cache()).map_err(io::Error::other)
}

fn wait_for_ctrl_c() -> io::Result<()> {
    let (stop, stopped) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop.send(());
    })
    .map_err(io::Error::other)?;
    stopped.recv().map_err(io::Error::other)
}
