//! The ORPHEUS server for Overwatch 1.74.0.0.104319.

mod assets;
mod doctor;
mod import;
mod logging;
mod play;
mod serve;
mod settings;

use std::{io, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use orpheus_server::runtime;
use tracing::error;

use settings::{ServerArgs, Settings, default_state_dir};

#[derive(Parser)]
#[command(version, about = "The ORPHEUS server for Overwatch 1.74.0.0.104319")]
struct Cli {
    /// Folder for profiles, logs and config.toml [default: %APPDATA%\orpheus].
    #[arg(long, global = true, env = "ORPHEUS_STATE_DIR", value_name = "DIR")]
    state: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the Battle.net, lobby and game services until Ctrl+C.
    Serve(ServerArgs),
    /// Run the server, start the game and load the relay into it.
    Play(ServerArgs),
    /// Check the setup `serve` and `play` need and say how to fix each problem.
    Doctor(ServerArgs),
    /// Manage saved player state.
    #[command(subcommand)]
    State(StateCommand),
    /// Game data derived from the install.
    #[command(subcommand)]
    Assets(AssetsCommand),
    /// Manage the prepared runtime pack.
    #[command(subcommand)]
    Runtime(RuntimeCommand),
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

#[derive(Subcommand)]
enum RuntimeCommand {
    /// Copy runtime inputs from a prepared local checkout into a new folder.
    Prepare {
        #[arg(long, value_name = "DIR")]
        source: PathBuf,
        #[arg(long, value_name = "DIR")]
        destination: PathBuf,
        /// Collision caches to copy, by map index; all when omitted.
        #[arg(long = "map", value_name = "INDEX")]
        maps: Vec<String>,
        /// A graph bundle to use instead of the source's.
        #[arg(long, value_name = "FILE")]
        game_graphs: Option<PathBuf>,
        /// DataTool .003 XML exports with the authored target tags.
        #[arg(long, value_name = "DIR")]
        target_tags_xml: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(problem) => {
            error!("{problem}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> io::Result<ExitCode> {
    let state = match cli.state {
        Some(state) => state,
        None => default_state_dir()?,
    };
    match cli.command {
        Command::Serve(args) => {
            let settings = Settings::resolve(&state, &args)?;
            logging::init(Some(&settings.logs()))?;
            serve::run(&settings)?;
        }
        Command::Play(args) => {
            let settings = Settings::resolve(&state, &args)?;
            logging::init(Some(&settings.logs()))?;
            play::run(&settings)?;
        }
        Command::Doctor(args) => {
            logging::init(None)?;
            if !doctor::run(&state, &args) {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::State(StateCommand::Import {
            checkout,
            overwrite,
        }) => {
            logging::init(None)?;
            let report = import::run(&checkout, &state, overwrite)?;
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
        }
        Command::Assets(AssetsCommand::Build(args)) => {
            let settings = Settings::resolve(&state, &args)?;
            logging::init(None)?;
            println!("{}", serve::game_data(&settings)?.display());
        }
        Command::Assets(AssetsCommand::Diff { pack, server }) => {
            let settings = Settings::resolve(&state, &server)?;
            logging::init(None)?;
            if !assets::diff(&settings, &pack)? {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Runtime(RuntimeCommand::Prepare {
            source,
            destination,
            maps,
            game_graphs,
            target_tags_xml,
        }) => {
            logging::init(None)?;
            let prepared = runtime::prepare_with_target_tags(
                &source,
                &destination,
                &maps,
                game_graphs.as_deref(),
                target_tags_xml.as_deref(),
            )
            .map_err(io::Error::other)?;
            println!("Prepared runtime data at {}", prepared.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}
