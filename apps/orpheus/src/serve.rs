//! The server without a game here: `orpheus serve` (Python's `retail
//! --no-launch`) and `orpheus server` (Python's `server` mode, the lobby for
//! other PCs).

use std::{
    fs, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
};

use orpheus_launcher::game::install_root;
use orpheus_server::{
    platform::LocalGames,
    server::{Server, ServerConfig},
};
use tracing::info;

use crate::{banner, play, settings::Settings};

/// The language of display strings in derived data when the settings don't
/// name one; server logic keys only on GUIDs.
const TEXT_LOCALE: &str = "enUS";

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
    let data = game_data(settings)?;
    let mut config = ServerConfig::new(data, settings.state.clone());
    config.account = settings.account.clone();
    config.host = hosting.host;
    config.game_host = hosting.game_host;
    config.lobby_port = settings.ports.lobby;
    config.game_port = settings.game_port();
    config.battle_net = hosting.battle_net;
    config.stall_port = settings.ports.stall;
    config.rpc_port = settings.ports.rpc;
    config.dashboard_port = settings.dashboard_port();
    config.games = hosting.games;
    config.install = Some(install(&play::game(settings)?)?);
    config.test_players = settings.test_players;
    config.test_ai = settings.test_ai;
    config.template = settings.template.clone();
    config.dev_console = settings.dev_console;
    config.workshop = settings.workshop.clone();
    Server::start(config)
}

/// The game data the server reads, derived from the configured install on
/// the first start and kept in the state folder's cache.
pub fn game_data(settings: &Settings) -> io::Result<PathBuf> {
    let game = play::configured_game(settings)?;
    let install = install(&game)?;
    let locale = play::locale(settings, &game).unwrap_or_else(|| TEXT_LOCALE.into());
    fs::create_dir_all(settings.cache())?;
    info!(install = %install.display(), "reading game data from the install");
    orpheus_assets::cache::ensure(&install, &locale, &settings.cache()).map_err(io::Error::other)
}

/// The install folder of the configured game.
fn install(game: &Path) -> io::Result<PathBuf> {
    install_root(game).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} has no .build.info next to it", game.display()),
        )
    })
}

pub fn wait_for_ctrl_c() -> io::Result<()> {
    let (stop, stopped) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop.send(());
    })
    .map_err(io::Error::other)?;
    stopped.recv().map_err(io::Error::other)
}
