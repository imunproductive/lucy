//! `orpheus serve`: the Battle.net, lobby and game services until Ctrl+C.

use std::{fs, io, sync::mpsc};

use orpheus_server::server::{Server, ServerConfig};
use tracing::info;

use crate::settings::Settings;

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
    let runtime = settings.runtime.clone().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no runtime pack: pass --runtime, set ORPHEUS_RUNTIME_DIR, or add runtime = '<dir>' to config.toml",
        )
    })?;
    fs::create_dir_all(&settings.state)?;
    let mut config = ServerConfig::new(runtime, settings.state.clone());
    config.account = settings.account.clone();
    config.lobby_port = settings.ports.lobby;
    config.game_port = settings.ports.game;
    config.stall_port = settings.ports.stall;
    config.rpc_port = settings.ports.rpc;
    config.dashboard_port = Some(settings.ports.dashboard);
    Server::start(config)
}

fn wait_for_ctrl_c() -> io::Result<()> {
    let (stop, stopped) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop.send(());
    })
    .map_err(io::Error::other)?;
    stopped.recv().map_err(io::Error::other)
}
