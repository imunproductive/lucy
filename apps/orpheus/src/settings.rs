//! The state folder, `config.toml` and the resolved settings.
//!
//! Precedence: command-line flag, then `ORPHEUS_*` environment variable (both
//! handled by clap), then `config.toml`, then the built-in default.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use clap::Args;
use serde::Deserialize;

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
    /// Game UDP port.
    #[arg(long, value_name = "PORT")]
    pub game_port: Option<u16>,
    /// Dashboard TCP port.
    #[arg(long, value_name = "PORT")]
    pub dashboard_port: Option<u16>,
}

#[derive(Debug)]
pub struct Settings {
    pub state: PathBuf,
    pub account: Option<String>,
    pub game: Option<PathBuf>,
    /// `auto`, `none` or a game language code.
    pub locale: String,
    pub ports: Ports,
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
        })
    }

    pub fn logs(&self) -> PathBuf {
        self.state.join("logs")
    }

    /// Where game data derived from the install is kept.
    pub fn cache(&self) -> PathBuf {
        self.state.join("cache")
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
}
