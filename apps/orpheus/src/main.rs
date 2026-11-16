//! The ORPHEUS server for Overwatch 1.74.0.0.104319.

#![forbid(unsafe_code)]

mod ask;
mod assets;
mod banner;
mod doctor;
mod import;
mod join;
mod logging;
mod play;
mod script_query;
mod serve;
mod settings;

use std::{
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{
    CommandFactory, FromArgMatches, Parser, Subcommand, error::ErrorKind, parser::ValueSource,
};
use tracing::error;

use serve::Hosting;
use settings::{Mode, ModeArgs, ServerArgs, Settings, default_state_dir};

/// Without a command, `orpheus` asks which mode to start (Enter is retail);
/// with no console to answer, it starts retail.
#[derive(Parser)]
#[command(version, about = "The ORPHEUS server for Overwatch 1.74.0.0.104319")]
struct Cli {
    /// Folder for profiles, logs, config.toml and remembered answers [default: %APPDATA%\orpheus].
    #[arg(long, global = true, env = "ORPHEUS_STATE_DIR", value_name = "DIR")]
    state: Option<PathBuf>,
    /// What to start without a command; without it you are asked.
    #[arg(long, value_enum)]
    mode: Option<Mode>,
    #[command(flatten)]
    options: ModeArgs,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server, start the game and load the relay into it (retail mode).
    #[command(alias = "retail")]
    Play(ModeArgs),
    /// Host for other PCs and play here: retail with the lobby open to other PCs.
    Host(ModeArgs),
    /// Run the server and the game in tournament mode, with no relay or Battle.net.
    Tournament(ModeArgs),
    /// Run the Battle.net, lobby and game services until Ctrl+C, without starting a game.
    Serve(ModeArgs),
    /// Host for other PCs without playing here: the lobby and game server only.
    Server(ModeArgs),
    /// Play on someone else's server with the full menu (--server, --name).
    Join(ModeArgs),
    /// Play on someone else's server in tournament mode (--server).
    JoinTournament(ModeArgs),
    /// Check the setup the modes need and say how to fix each problem.
    Doctor(ServerArgs),
    /// Manage saved player state.
    #[command(subcommand)]
    State(StateCommand),
    /// Game data derived from the install.
    #[command(subcommand)]
    Assets(AssetsCommand),
    /// Statescript research tools.
    #[command(subcommand)]
    Script(ScriptCommand),
}

#[derive(Subcommand)]
enum ScriptCommand {
    /// Query the statescript graphs of the derived game data (the port of
    /// tools/ssq.py).
    #[command(long_about = script_query::ABOUT)]
    Query(script_query::QueryArgs),
}

#[derive(Subcommand)]
enum AssetsCommand {
    /// Derive the game data now and print where it is.
    Build(ServerArgs),
    /// Derive the game data into a scratch folder and compare every file
    /// with a prepared runtime pack.
    Diff {
        /// The prepared pack to compare with.
        #[arg(long, value_name = "DIR")]
        pack: PathBuf,
        #[command(flatten)]
        server: ServerArgs,
    },
}

#[derive(Subcommand)]
enum StateCommand {
    /// Copy profiles and default_account.txt from an older checkout.
    Import {
        /// The older checkout's root folder.
        checkout: PathBuf,
        /// Replace files that already exist in the state folder.
        #[arg(long)]
        overwrite: bool,
    },
}

/// What runs: a mode, or `serve` (Python's `retail --no-launch`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Launch {
    Mode(Mode),
    Serve,
}

fn main() -> ExitCode {
    let cli = parse_cli(std::env::args_os()).unwrap_or_else(|error| error.exit());
    match run(cli) {
        Ok(code) => code,
        Err(problem) => {
            error!("{problem}");
            ExitCode::FAILURE
        }
    }
}

/// The command line. The mode options before a command would be ignored, so
/// they are refused there; `--state` goes anywhere.
fn parse_cli(
    arguments: impl IntoIterator<Item = impl Into<std::ffi::OsString> + Clone>,
) -> Result<Cli, clap::Error> {
    let mut command = Cli::command();
    let matches = command.try_get_matches_from_mut(arguments)?;
    if matches.subcommand().is_some() {
        let misplaced = matches.ids().find(|id| {
            id.as_str() != "state"
                && matches.value_source(id.as_str()) == Some(ValueSource::CommandLine)
        });
        if let Some(id) = misplaced {
            return Err(command.error(
                ErrorKind::ArgumentConflict,
                format!("the option {id} goes after the command, or use it without one"),
            ));
        }
    }
    Cli::from_arg_matches(&matches)
}

fn run(cli: Cli) -> io::Result<ExitCode> {
    let state = match cli.state {
        Some(state) => state,
        None => default_state_dir()?,
    };
    let (launch, options) = match cli.command {
        Some(Command::Play(options)) => (Launch::Mode(Mode::Retail), options),
        Some(Command::Host(options)) => (Launch::Mode(Mode::Host), options),
        Some(Command::Tournament(options)) => (Launch::Mode(Mode::Tournament), options),
        Some(Command::Serve(options)) => (Launch::Serve, options),
        Some(Command::Server(options)) => (Launch::Mode(Mode::Server), options),
        Some(Command::Join(options)) => (Launch::Mode(Mode::Join), options),
        Some(Command::JoinTournament(options)) => (Launch::Mode(Mode::JoinTournament), options),
        Some(Command::Doctor(args)) => {
            logging::init(None)?;
            let healthy = doctor::run(&state, &args);
            return Ok(if healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        Some(Command::State(command)) => return state_command(command, &state),
        Some(Command::Assets(command)) => return assets_command(command, &state),
        Some(Command::Script(command)) => return script_command(command, &state),
        None => {
            let mut options = cli.options;
            let mode = match cli.mode {
                Some(mode) => mode,
                None => ask_mode(&mut options, &state)?,
            };
            (Launch::Mode(mode), options)
        }
    };
    let mut settings = Settings::resolve(&state, &options.server)?;
    settings.game_args = options.game_args.clone();
    logging::init(Some(&settings.logs()))?;
    launch_mode(launch, &settings, &options)?;
    Ok(ExitCode::SUCCESS)
}

/// The mode from the menu when someone can answer, else retail. A host is
/// asked for the public address too (`main` in `ow174/cli.py`).
fn ask_mode(options: &mut ModeArgs, state: &Path) -> io::Result<Mode> {
    if !io::stdin().is_terminal() {
        return Ok(Mode::Retail);
    }
    let mut input = io::stdin().lock();
    let mut output = io::stdout();
    let mode = ask::mode(&mut input, &mut output)?;
    if mode.hosting() && options.game_host.as_deref().unwrap_or("").is_empty() {
        options.game_host = Some(ask::public_address(
            &mut input,
            &mut output,
            &state.join(ask::PUBLIC_ADDRESS_FILE),
        )?);
    }
    Ok(mode)
}

fn launch_mode(launch: Launch, settings: &Settings, options: &ModeArgs) -> io::Result<()> {
    let hosting = |hosting: bool, battle_net: bool| Hosting {
        host: options.listen_host(hosting),
        game_host: options.game_host(),
        battle_net,
        games: None,
    };
    match launch {
        Launch::Serve => serve::run(settings, hosting(false, true)),
        Launch::Mode(Mode::Server) => serve::run(settings, hosting(true, false)),
        Launch::Mode(mode) => launch_game(mode, settings, options, hosting(mode.hosting(), true)),
    }
}

#[cfg(windows)]
fn launch_game(
    mode: Mode,
    settings: &Settings,
    options: &ModeArgs,
    hosting: Hosting,
) -> io::Result<()> {
    match mode {
        Mode::Retail | Mode::Host => play::retail(settings, hosting, options.timeout),
        Mode::Tournament => play::tournament(
            settings,
            Hosting {
                battle_net: false,
                ..hosting
            },
        ),
        Mode::Join => join::run(settings, options),
        Mode::JoinTournament => join::run_tournament(settings, options),
        Mode::Server => unreachable!("server mode starts no game"),
    }
}

#[cfg(not(windows))]
fn launch_game(_: Mode, _: &Settings, _: &ModeArgs, _: Hosting) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the Overwatch 1.74 client runs on Windows only",
    ))
}

fn state_command(command: StateCommand, state: &Path) -> io::Result<ExitCode> {
    let StateCommand::Import {
        checkout,
        overwrite,
    } = command;
    logging::init(None)?;
    let report = import::run(&checkout, state, overwrite)?;
    println!(
        "Imported {} file(s) into {}.",
        report.copied.len(),
        state.display()
    );
    for kept in &report.kept {
        println!(
            "Kept the existing {} (use --overwrite to replace it).",
            kept.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn assets_command(command: AssetsCommand, state: &Path) -> io::Result<ExitCode> {
    logging::init(None)?;
    match command {
        AssetsCommand::Build(args) => {
            let settings = Settings::resolve(state, &args)?;
            println!("{}", serve::game_data(&settings)?.display());
        }
        AssetsCommand::Diff { pack, server } => {
            let settings = Settings::resolve(state, &server)?;
            if !assets::diff(&settings, &pack)? {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn script_command(command: ScriptCommand, state: &Path) -> io::Result<ExitCode> {
    let ScriptCommand::Query(query) = command;
    logging::init(None)?;
    let settings = Settings::resolve(state, &query.server)?;
    script_query::run(&query, &serve::game_data(&settings)?)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv4Addr, time::Duration};

    fn parse(arguments: &[&str]) -> Cli {
        parse_cli(["orpheus"].iter().chain(arguments)).unwrap()
    }

    /// Everything after `--` goes to Overwatch.exe as it is, also options
    /// orpheus has itself; without `--` there are none.
    #[test]
    fn arguments_after_a_double_dash_go_to_the_game() {
        let cli = parse(&[
            "play",
            "--game",
            "D:\\Games\\Overwatch",
            "--",
            "--tank_TournamentModeAdmin",
            "--game",
            "x",
        ]);
        let Some(Command::Play(options)) = cli.command else {
            panic!("play")
        };
        assert_eq!(
            options.game_args,
            ["--tank_TournamentModeAdmin", "--game", "x"]
        );
        assert_eq!(
            options.server.game.as_deref(),
            Some(std::path::Path::new("D:\\Games\\Overwatch"))
        );
        let cli = parse(&["--", "--tank_Console"]);
        assert_eq!(cli.options.game_args, ["--tank_Console"]);
        let cli = parse(&["play"]);
        let Some(Command::Play(options)) = cli.command else {
            panic!("play")
        };
        assert!(options.game_args.is_empty());
    }

    #[test]
    fn every_mode_has_a_command_and_the_menu_takes_its_options() {
        let cli = parse(&["host", "--game-host", "1.2.3.4", "--dashboard-port", "0"]);
        let Some(Command::Host(options)) = cli.command else {
            panic!("host")
        };
        assert_eq!(options.listen_host(true), Ipv4Addr::UNSPECIFIED);
        assert_eq!(options.game_host(), "1.2.3.4");
        assert_eq!(options.server.dashboard_port, Some(0));

        let cli = parse(&["retail", "--host", "0.0.0.0", "--timeout", "2.5"]);
        let Some(Command::Play(options)) = cli.command else {
            panic!("retail")
        };
        assert_eq!(options.listen_host(false), Ipv4Addr::UNSPECIFIED);
        assert_eq!(options.timeout, Duration::from_millis(2500));

        let cli = parse(&["join-tournament", "--server", "games.example:3724"]);
        let Some(Command::JoinTournament(options)) = cli.command else {
            panic!("join-tournament")
        };
        assert_eq!(options.remote.as_deref(), Some("games.example:3724"));
        assert_eq!(options.timeout, Duration::from_secs(30));
        assert_eq!(options.listen_host(false), Ipv4Addr::LOCALHOST);

        let cli = parse(&[
            "--mode",
            "join",
            "--server",
            "1.2.3.4:3724",
            "--name",
            "Ana",
        ]);
        assert!(cli.command.is_none());
        assert_eq!(cli.mode, Some(Mode::Join));
        assert_eq!(cli.options.name.as_deref(), Some("Ana"));
        assert!(parse(&[]).mode.is_none());
    }

    #[test]
    fn bad_join_options_are_refused_before_anything_starts() {
        for arguments in [
            ["join", "--server", "1.2.3.4"],
            ["join", "--name", "two words"],
            ["play", "--timeout", "0"],
            ["serve", "--host", "localhost"],
            ["--host", "1.2.3.4", "play"],
        ] {
            assert!(
                parse_cli(["orpheus"].iter().chain(&arguments)).is_err(),
                "{arguments:?}"
            );
        }
    }

    #[test]
    fn the_state_folder_goes_before_or_after_a_command() {
        for arguments in [
            ["--state", "elsewhere", "doctor"],
            ["doctor", "--state", "elsewhere"],
        ] {
            let cli = parse(&arguments);
            assert!(matches!(cli.command, Some(Command::Doctor(_))));
            assert_eq!(cli.state, Some(PathBuf::from("elsewhere")));
        }
    }

    /// `show --state-index` is the graph's state; `--state` stays the state
    /// folder, before or after the command.
    #[test]
    fn script_query_show_takes_a_state_index_beside_the_state_folder() {
        for arguments in [
            &[
                "script",
                "query",
                "show",
                "0255",
                "--state-index",
                "1",
                "--depth",
                "-1",
            ][..],
            &[
                "--state",
                "elsewhere",
                "script",
                "query",
                "show",
                "0255",
                "--state-index",
                "1",
                "--depth",
                "-1",
            ],
            &[
                "script",
                "query",
                "show",
                "0255",
                "--state-index",
                "1",
                "--state",
                "elsewhere",
                "--depth",
                "-1",
                "--full",
            ],
        ] {
            let cli = parse(arguments);
            let Some(Command::Script(ScriptCommand::Query(query))) = cli.command else {
                panic!("script query: {arguments:?}")
            };
            let script_query::QueryCommand::Show {
                graph,
                state_index,
                depth,
                full,
            } = query.command
            else {
                panic!("show: {arguments:?}")
            };
            assert_eq!(
                (graph.as_str(), state_index, depth),
                ("0255", Some(1), Some(-1))
            );
            assert_eq!(full, arguments.contains(&"--full"), "{arguments:?}");
            let folder = (arguments.len() > 8).then(|| PathBuf::from("elsewhere"));
            assert_eq!(cli.state, folder, "{arguments:?}");
        }
    }
}
