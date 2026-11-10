//! The server without a game here: `orpheus serve` (Python's `retail
//! --no-launch`) and `orpheus server` (Python's `server` mode, the lobby for
//! other PCs).

use std::{fs, io, net::Ipv4Addr, sync::Arc, sync::mpsc};

use orpheus_server::{
    platform::LocalGames,
    server::{Server, ServerConfig},
};
use tracing::info;

use crate::{banner, settings::Settings};

/// Where and for whom the server runs.
pub struct Hosting {
    /// Where the lobby and the game server listen.
    pub host: Ipv4Addr,
    /// The public address for games outside the network; empty for none.
    pub game_host: String,
    /// The local Battle.net, for retail games on this PC.
    pub battle_net: bool,
    /// The retail games this PC starts.
    pub games: Option<Arc<dyn LocalGames>>,
}

/// Run the server until Ctrl+C. `serve` keeps Battle.net for a game started
/// by hand; `server` mode has none.
pub fn run(settings: &Settings, hosting: Hosting) -> io::Result<()> {
    let battle_net = hosting.battle_net;
    let game_host = hosting.game_host.clone();
    let mut server = start(settings, hosting)?;
    banner::log(&server, settings, &game_host);
    if battle_net {
        info!("[+] Server services ready; client launching is disabled.");
    } else {
        info!("[+] To play here too, choose 3 next time instead of 4.");
    }
    info!("Keep this window open while you play; closing it or Ctrl+C stops the server.");
    wait_for_ctrl_c()?;
    info!("Stopped.");
    server.stop()
}

pub fn start(settings: &Settings, hosting: Hosting) -> io::Result<Server> {
    let runtime = settings.runtime.clone().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no runtime pack: pass --runtime, set ORPHEUS_RUNTIME_DIR, or add runtime = '<dir>' to config.toml",
        )
    })?;
    fs::create_dir_all(&settings.state)?;
    let mut config = ServerConfig::new(runtime, settings.state.clone());
    config.account = settings.account.clone();
    config.host = hosting.host;
    config.game_host = hosting.game_host;
    config.lobby_port = settings.ports.lobby;
    config.game_port = settings.ports.game;
    config.battle_net = hosting.battle_net;
    config.stall_port = settings.ports.stall;
    config.rpc_port = settings.ports.rpc;
    config.dashboard_port = settings.dashboard_port();
    config.games = hosting.games;
    Server::start(config)
}

pub fn wait_for_ctrl_c() -> io::Result<()> {
    let (stop, stopped) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop.send(());
    })
    .map_err(io::Error::other)?;
    stopped.recv().map_err(io::Error::other)
}
