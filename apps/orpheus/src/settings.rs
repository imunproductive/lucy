//! The state folder, `config.toml` and the resolved settings.
//!
//! Precedence: command-line flag, then `ORPHEUS_*` environment variable (both
//! handled by clap), then `config.toml`, then the built-in default.

use std::{
    fs, io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Args, ValueEnum};
use serde::Deserialize;

use crate::ask;

pub const CONFIG_FILE: &str = "config.toml";
pub const DEFAULT_PORTS: Ports = Ports {
    lobby: 3724,
    stall: 1119,
    rpc: 21119,
    game: 3730,
    dashboard: 3725,
};

/// The per-user state folder: `%APPDATA%\orpheus` on Windows.
pub fn default_state_dir() -> io::Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|dirs| dirs.data_dir().join("orpheus"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ports {
    pub lobby: u16,
    pub stall: u16,
    pub rpc: u16,
    pub game: u16,
    pub dashboard: u16,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    account: Option<String>,
    game: Option<PathBuf>,
    locale: Option<String>,
    #[serde(default)]
    ports: PortsFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortsFile {
    lobby: Option<u16>,
    stall: Option<u16>,
    rpc: Option<u16>,
    game: Option<u16>,
    dashboard: Option<u16>,
}

/// Overrides shared by `serve`, `play` and `doctor`.
#[derive(Args, Clone, Debug, Default)]
pub struct ServerArgs {
    /// Saved account the game logs in as.
    #[arg(long, value_name = "NAME")]
    pub account: Option<String>,
    /// Overwatch.exe, its installation folder or the official launcher. The
    /// server reads its game data from this install.
    #[arg(long, env = "ORPHEUS_GAME", value_name = "PATH")]
    pub game: Option<PathBuf>,
    /// Game language for play: auto, none, or a code such as enUS.
    #[arg(long, value_name = "CODE")]
    pub locale: Option<String>,
    /// Lobby TCP port.
    #[arg(long, value_name = "PORT")]
    pub lobby_port: Option<u16>,
    /// Battle.net TCP port the client dials.
    #[arg(long, value_name = "PORT")]
    pub stall_port: Option<u16>,
    /// Battle.net RPC TCP port the relay pipes to.
    #[arg(long, value_name = "PORT")]
    pub rpc_port: Option<u16>,
    /// Game UDP port; 0 turns the game server off (the lobby runs without
    /// matches).
    #[arg(long, value_name = "PORT")]
    pub game_port: Option<u16>,
    /// Dashboard TCP port; 0 turns the dashboard off.
    #[arg(long, value_name = "PORT")]
    pub dashboard_port: Option<u16>,
    /// Start a match once this many players search (default: 0 = full teams
    /// only).
    #[arg(long, value_name = "PLAYERS")]
    pub test_players: Option<usize>,
    /// Template profile for new accounts [default: profile.json in the
    /// state folder].
    #[arg(long, value_name = "FILE")]
    pub save: Option<PathBuf>,
}

/// What `orpheus` starts: the modes of `ow174/cli.py`, in its menu order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Mode {
    /// The full main menu: the lobby, a local Battle.net and the game with the relay.
    Retail,
    /// The simpler menu without the lobby hero; the game dials the lobby, no relay.
    Tournament,
    /// Retail, with the lobby open to players on other PCs: you host and play.
    Host,
    /// Only the lobby and game server, open to players on other PCs.
    Server,
    /// The game with the full menu on someone else's server.
    Join,
    /// Only the game, in tournament mode, on someone else's server.
    JoinTournament,
}

impl Mode {
    pub const ALL: [Self; 6] = [
        Self::Retail,
        Self::Tournament,
        Self::Host,
        Self::Server,
        Self::Join,
        Self::JoinTournament,
    ];

    /// The lobby is there for other PCs (`HOSTING` in cli.py).
    pub fn hosting(self) -> bool {
        matches!(self, Self::Host | Self::Server)
    }
}

/// The options of every mode: the server's, and those for hosting, joining
/// and starting the game.
#[derive(Args, Clone, Debug)]
pub struct ModeArgs {
    #[command(flatten)]
    pub server: ServerArgs,
    /// Address the lobby and the game server listen on [default: 0.0.0.0
    /// for host and server, else 127.0.0.1].
    #[arg(long, value_name = "ADDRESS")]
    pub host: Option<Ipv4Addr>,
    /// Address games are sent to for a match, if not the one they reach the
    /// lobby at (behind a router's port forward: the public address).
    #[arg(long, value_name = "ADDRESS")]
    pub game_host: Option<String>,
    /// Address of the server to join, such as 1.2.3.4:3724.
    #[arg(long = "server", value_name = "HOST:PORT", value_parser = ask::parse_server_address)]
    pub remote: Option<String>,
    /// Your name on the server you join with the full menu.
    #[arg(long, value_name = "NAME", value_parser = ask::parse_player_name)]
    pub name: Option<String>,
    /// Seconds to wait for the game to be ready for the relay.
    #[arg(long, value_name = "SECONDS", default_value = "30", value_parser = parse_timeout)]
    pub timeout: Duration,
}

impl ModeArgs {
    /// Where the lobby listens: `--host`, else every address when hosting
    /// (it is there for other PCs) and only this PC in the modes that play
    /// alone (`listen_host` in cli.py).
    pub fn listen_host(&self, hosting: bool) -> Ipv4Addr {
        self.host.unwrap_or(if hosting {
            Ipv4Addr::UNSPECIFIED
        } else {
            Ipv4Addr::LOCALHOST
        })
    }

    /// The public address, or empty for none.
    pub fn game_host(&self) -> String {
        self.game_host.clone().unwrap_or_default()
    }
}

fn parse_timeout(text: &str) -> Result<Duration, String> {
    text.parse::<f64>()
        .ok()
        .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
        .filter(|timeout| !timeout.is_zero())
        .ok_or_else(|| "give the seconds as a number above 0, such as 30".into())
}

#[derive(Debug)]
pub struct Settings {
    pub state: PathBuf,
    pub account: Option<String>,
    pub game: Option<PathBuf>,
    /// `auto`, `none` or a game language code.
    pub locale: String,
    pub ports: Ports,
    /// Players who start a match (`--test-players`); 0 = full teams.
    pub test_players: usize,
    /// The template profile for new accounts (`--save`).
    pub template: Option<PathBuf>,
}

impl Settings {
    pub fn resolve(state: &Path, args: &ServerArgs) -> io::Result<Self> {
        let file = read_config(&state.join(CONFIG_FILE))?;
        let ports = &file.ports;
        Ok(Self {
            state: state.to_owned(),
            account: args.account.clone().or(file.account),
            game: args.game.clone().or(file.game),
            locale: args
                .locale
                .clone()
                .or(file.locale)
                .unwrap_or_else(|| "auto".into()),
            ports: Ports {
                lobby: args
                    .lobby_port
                    .or(ports.lobby)
                    .unwrap_or(DEFAULT_PORTS.lobby),
                stall: args
                    .stall_port
                    .or(ports.stall)
                    .unwrap_or(DEFAULT_PORTS.stall),
                rpc: args.rpc_port.or(ports.rpc).unwrap_or(DEFAULT_PORTS.rpc),
                game: args.game_port.or(ports.game).unwrap_or(DEFAULT_PORTS.game),
                dashboard: args
                    .dashboard_port
                    .or(ports.dashboard)
                    .unwrap_or(DEFAULT_PORTS.dashboard),
            },
            test_players: args.test_players.unwrap_or(0),
            template: args.save.clone(),
        })
    }

    pub fn logs(&self) -> PathBuf {
        self.state.join("logs")
    }

    /// Where game data derived from the install is kept.
    pub fn cache(&self) -> PathBuf {
        self.state.join("cache")
    }

    /// The dashboard's port, `None` when 0 turns it off.
    pub fn dashboard_port(&self) -> Option<u16> {
        (self.ports.dashboard != 0).then_some(self.ports.dashboard)
    }

    /// The game server's port, `None` when 0 turns it off (`--game-port`,
    /// `ow174/lobby/settings.py`).
    pub fn game_port(&self) -> Option<u16> {
        (self.ports.game != 0).then_some(self.ports.game)
    }
}

fn read_config(path: &Path) -> io::Result<ConfigFile> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(text: &str) -> tempfile::TempDir {
        let state = tempfile::tempdir().unwrap();
        fs::write(state.path().join(CONFIG_FILE), text).unwrap();
        state
    }

    #[test]
    fn flags_override_the_config_file_which_overrides_defaults() {
        let state = write_config(
            "game = 'D:/Games/Overwatch'\naccount = 'Saved'\n[ports]\nlobby = 4000\ngame = 4001\n",
        );
        let args = ServerArgs {
            account: Some("Flag".into()),
            game_port: Some(5001),
            ..ServerArgs::default()
        };
        let settings = Settings::resolve(state.path(), &args).unwrap();
        assert_eq!(settings.game, Some(PathBuf::from("D:/Games/Overwatch")));
        assert_eq!(settings.account.as_deref(), Some("Flag"));
        assert_eq!(
            settings.ports,
            Ports {
                lobby: 4000,
                game: 5001,
                ..DEFAULT_PORTS
            }
        );
    }

    #[test]
    fn a_missing_config_uses_defaults_and_a_misspelled_key_is_an_error() {
        let empty = tempfile::tempdir().unwrap();
        let settings = Settings::resolve(empty.path(), &ServerArgs::default()).unwrap();
        assert_eq!(settings.ports, DEFAULT_PORTS);
        assert!(settings.game.is_none());

        let typo = write_config("[ports]\nlobyb = 1\n");
        let error = Settings::resolve(typo.path(), &ServerArgs::default()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("lobyb"));
    }

    #[test]
    fn game_port_0_turns_the_game_server_off() {
        let state = write_config("[ports]\ngame = 0\n");
        let settings = Settings::resolve(state.path(), &ServerArgs::default()).unwrap();
        assert_eq!(settings.game_port(), None);
        let args = ServerArgs {
            game_port: Some(4001),
            ..ServerArgs::default()
        };
        let settings = Settings::resolve(state.path(), &args).unwrap();
        assert_eq!(settings.game_port(), Some(4001));
    }
}
