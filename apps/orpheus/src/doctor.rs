//! `orpheus doctor`: check everything `serve` needs and say how to fix each problem.

use std::{
    fs,
    net::{Ipv4Addr, TcpListener, UdpSocket},
    path::Path,
};

use orpheus_content::maps::GameMap;
use orpheus_lobby::accounts::saved_names;
use orpheus_server::{runtime::missing_runtime_files, server::local_account_name};
use orpheus_sim::scene_assets::SceneAssets;

use orpheus_launcher::game::TARGET_VERSION;

use crate::{
    play,
    settings::{CONFIG_FILE, ServerArgs, Settings},
};

/// One check's result: a passing detail, or a problem with its fix.
enum Outcome {
    Ok(String),
    Problem { what: String, fix: String },
}

fn problem(what: impl Into<String>, fix: impl Into<String>) -> Outcome {
    Outcome::Problem {
        what: what.into(),
        fix: fix.into(),
    }
}

/// Print every check and return whether all of them passed. A broken
/// config.toml stops the checks, since every other check depends on it.
pub fn run(state_dir: &Path, args: &ServerArgs) -> bool {
    let settings = match Settings::resolve(state_dir, args) {
        Ok(settings) => settings,
        Err(error) => {
            report(
                "config",
                &problem(
                    error.to_string(),
                    "correct config.toml; its keys are runtime, account, game, locale and [ports]",
                ),
            );
            return false;
        }
    };
    let checks = [
        ("config", config(&settings.state)),
        ("state", state(&settings.state)),
        ("runtime", runtime(&settings)),
        ("account", account(&settings)),
        ("ports", ports(&settings)),
        ("client", client(&settings)),
        ("relay", relay()),
    ];
    checks.iter().fold(true, |healthy, (name, outcome)| {
        report(name, outcome) && healthy
    })
}

fn report(name: &str, outcome: &Outcome) -> bool {
    match outcome {
        Outcome::Ok(detail) => {
            println!("ok    {name:<8} {detail}");
            true
        }
        Outcome::Problem { what, fix } => {
            println!("FAIL  {name:<8} {what}");
            println!("      {:<8} fix: {fix}", "");
            false
        }
    }
}

fn config(state: &Path) -> Outcome {
    let path = state.join(CONFIG_FILE);
    if path.is_file() {
        Outcome::Ok(path.display().to_string())
    } else {
        Outcome::Ok(format!("{} not present, using defaults", path.display()))
    }
}

fn state(state: &Path) -> Outcome {
    let probe = state.join(".doctor");
    match fs::create_dir_all(state)
        .and_then(|()| fs::write(&probe, b""))
        .and_then(|()| fs::remove_file(&probe))
    {
        Ok(()) => Outcome::Ok(state.display().to_string()),
        Err(error) => problem(
            format!("{} is not writable: {error}", state.display()),
            "choose another folder with --state or fix its permissions",
        ),
    }
}

fn runtime(settings: &Settings) -> Outcome {
    let Some(root) = &settings.runtime else {
        return problem(
            "no runtime pack configured",
            "pass --runtime <dir>, set ORPHEUS_RUNTIME_DIR, or add runtime = '<dir>' to config.toml",
        );
    };
    if !root.is_dir() {
        return problem(
            format!("{} does not exist", root.display()),
            "point runtime at a prepared pack (orpheus runtime prepare)",
        );
    }
    let missing = missing_runtime_files(root, true);
    if !missing.is_empty() {
        return problem(
            format!("{} is missing {}", root.display(), missing.join(", ")),
            "prepare the pack again with orpheus runtime prepare",
        );
    }
    let ready = SceneAssets::load(root)
        .and_then(|assets| assets.prepare(GameMap::practice_range()))
        .map(|scene| scene.movement_ready());
    match ready {
        Ok(true) => Outcome::Ok(format!("{} (Practice Range ready)", root.display())),
        Ok(false) => problem(
            format!("{} has no Practice Range movement data", root.display()),
            "include the base graph bundle and the 0688 collision cache when preparing the pack",
        ),
        Err(error) => problem(
            format!("{} cannot load the Practice Range: {error}", root.display()),
            "prepare the pack again with orpheus runtime prepare",
        ),
    }
}

fn account(settings: &Settings) -> Outcome {
    let result = saved_names(&settings.state.join("profiles")).and_then(|saved| {
        let name = local_account_name(&saved, &settings.state, settings.account.as_deref())?;
        let known = name
            .as_ref()
            .is_some_and(|name| saved.iter().any(|saved| saved.eq_ignore_ascii_case(name)));
        Ok((name, known))
    });
    match result {
        Ok((Some(name), true)) => Outcome::Ok(format!("{name} (saved profile)")),
        Ok((Some(name), false)) => {
            Outcome::Ok(format!("{name} (created from the template on first login)"))
        }
        Ok((None, _)) => Outcome::Ok("a new profile from the template on first login".into()),
        Err(error) => problem(
            format!("cannot read the saved profiles: {error}"),
            "repair or remove the broken file in the profiles folder, or import them with orpheus state import",
        ),
    }
}

fn ports(settings: &Settings) -> Outcome {
    let ports = settings.ports;
    let tcp = [
        ("lobby", ports.lobby),
        ("stall", ports.stall),
        ("rpc", ports.rpc),
        ("dashboard", ports.dashboard),
    ];
    let mut busy: Vec<String> = tcp
        .iter()
        .filter(|(_, port)| tcp_busy(*port))
        .map(|(name, port)| format!("{name} TCP {port}"))
        .collect();
    if UdpSocket::bind((Ipv4Addr::LOCALHOST, ports.game)).is_err() {
        busy.push(format!("game UDP {}", ports.game));
    }
    if busy.is_empty() {
        Outcome::Ok(format!(
            "lobby {}, stall {}, rpc {}, dashboard {} (TCP), game {} (UDP) are free",
            ports.lobby, ports.stall, ports.rpc, ports.dashboard, ports.game
        ))
    } else {
        problem(
            format!("in use: {}", busy.join(", ")),
            "stop the other server, or choose free ports with the --*-port flags or [ports] in config.toml",
        )
    }
}

fn client(settings: &Settings) -> Outcome {
    if settings.game.is_none() {
        return Outcome::Ok("not configured (only orpheus play needs it)".into());
    }
    match play::game(settings) {
        Ok(game) => {
            let locale = play::locale(settings, &game).unwrap_or_else(|| "the game's own".into());
            Outcome::Ok(format!(
                "{} ({TARGET_VERSION}, language {locale})",
                game.display()
            ))
        }
        Err(error) => problem(
            error.to_string(),
            "point game at the 1.74.0.0.104319 install with --game or game = '<path>' in config.toml",
        ),
    }
}

fn relay() -> Outcome {
    match play::relay_dll() {
        Ok(path) => Outcome::Ok(path.display().to_string()),
        Err(error) => problem(
            error.to_string(),
            "run cargo build --workspace (orpheus play loads it)",
        ),
    }
}

fn tcp_busy(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_err()
}
