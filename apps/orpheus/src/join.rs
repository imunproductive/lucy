//! Playing on someone else's server: `join` with the full menu and
//! `join-tournament` (`join`, `join_tournament` in `ow174/cli.py`).

use std::{
    io,
    net::{SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs},
    path::Path,
    time::Duration,
};

use orpheus_lobby::battle_tag_query;

use crate::{ask, settings::ModeArgs};

/// How long a join waits for the server to answer.
const CHECK_TIME: Duration = Duration::from_secs(5);

/// The server to join: `--server`, else asked (and remembered).
fn server_address(options: &ModeArgs, state: &Path) -> io::Result<String> {
    match &options.remote {
        Some(address) => Ok(address.clone()),
        None => ask::server_address(
            &mut io::stdin().lock(),
            &mut io::stdout(),
            &state.join(ask::SERVER_ADDRESS_FILE),
        ),
    }
}

/// The player's name on that server: `--name`, else asked (and remembered).
fn player_name(options: &ModeArgs, state: &Path) -> io::Result<String> {
    match &options.name {
        Some(name) => Ok(name.clone()),
        None => ask::player_name(
            &mut io::stdin().lock(),
            &mut io::stdout(),
            &state.join(ask::PLAYER_NAME_FILE),
        ),
    }
}

/// `host:port` with the host as an IPv4 address: the Battle.net referral
/// takes no names.
fn ipv4_address(address: &str) -> io::Result<SocketAddrV4> {
    let host = address.rsplit_once(':').map_or(address, |(host, _)| host);
    let not_found = |reason: String| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Can't find the server {host} ({reason})."),
        )
    };
    address
        .to_socket_addrs()
        .map_err(|error| not_found(error.to_string()))?
        .find_map(|found| match found {
            SocketAddr::V4(found) => Some(found),
            SocketAddr::V6(_) => None,
        })
        .ok_or_else(|| not_found("no IPv4 address".into()))
}

/// The BattleTag the server shows for the name (its nickname can differ).
/// Asking also checks that the server is up and new enough for a join with
/// the full menu.
fn battle_tag_on(lobby: SocketAddrV4, address: &str, name: &str) -> io::Result<String> {
    let tag = battle_tag_query::ask(lobby, name).map_err(|error| unreachable(address, &error))?;
    if tag.is_empty() {
        return Err(io::Error::other(format!(
            "The server {address} runs an older version. Ask the host to update it, or choose 6."
        )));
    }
    Ok(tag)
}

/// Check that the server takes connections.
fn reachable(address: &str) -> io::Result<()> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for found in address
        .to_socket_addrs()
        .map_err(|error| unreachable(address, &error))?
    {
        match TcpStream::connect_timeout(&found, CHECK_TIME) {
            Ok(_) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(unreachable(address, &last))
}

fn unreachable(address: &str, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "Can't reach the server {address} ({error}). Check the address, and that the server runs."
        ),
    )
}

#[cfg(windows)]
pub use client::{run, run_tournament};

#[cfg(windows)]
mod client {
    use std::{io, net::SocketAddrV4, sync::Arc};

    use orpheus_bnet::{protocol::Protocol, rpc::Player, service::BnetService};
    use orpheus_launcher::retail::{GameSetup, RetailGames};
    use orpheus_lobby::{accounts::account_id_for, presence::PRO_ACCOUNT_BITS};
    use std::net::Ipv4Addr;
    use tracing::info;

    use super::{battle_tag_on, ipv4_address, player_name, reachable, server_address};
    use crate::{
        play::{self, close_running_copy, start_tournament_game, wait_for},
        settings::{ModeArgs, Settings},
    };

    /// Start the game with the full menu on someone else's server. A
    /// Battle.net emulator of our own logs it in with the player's name and
    /// sends it on to that server's lobby.
    pub fn run(settings: &Settings, options: &ModeArgs) -> io::Result<()> {
        let address = server_address(options, &settings.state)?;
        let name = player_name(options, &settings.state)?;
        let lobby = ipv4_address(&address)?;
        let tag = battle_tag_on(lobby, &address, &name)?;
        let game = play::game(settings)?;
        close_running_copy(&game)?;
        let relay = play::relay_dll()?;
        let account = u64::from(account_id_for(&name));
        let player = Player {
            account,
            game_account: account ^ PRO_ACCOUNT_BITS,
            battle_tag: tag.clone(),
            name: name.clone(),
        };
        let local = |port| SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let mut bnet = BnetService::start(
            local(settings.ports.stall),
            local(settings.ports.rpc),
            lobby,
            Arc::new(Protocol::client()),
            move |_| player.clone(),
        )
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "The Battle.net ports are taken ({error}). Is a server or another game running here?"
                ),
            )
        })?;
        let logs = settings.logs();
        let games = RetailGames::new(GameSetup {
            locale: play::locale(settings, &game),
            game,
            relay,
            stall_port: bnet.stall_addr().port(),
            rpc_port: bnet.rpc_addr().port(),
            game_log: logs.join("game.log"),
            relay_log: logs.join("relay.log"),
            timeout: options.timeout,
            extra_arguments: settings.game_args.clone(),
            probe: play::probe_setup(settings, options.probe)?,
        });
        let (child, started) = games.start()?;
        play::log_relay(&started, None);
        info!("[+] The game is starting on {address} as {name} ({tag}).");
        info!("Keep this window open while you play; closing it disconnects the game.");
        wait_for(Some(child))?;
        info!("Stopped.");
        bnet.stop()
    }

    /// Start only the game, in tournament mode, pointed at someone else's
    /// server.
    pub fn run_tournament(settings: &Settings, options: &ModeArgs) -> io::Result<()> {
        let address = server_address(options, &settings.state)?;
        reachable(&address)?;
        let game = play::game(settings)?;
        close_running_copy(&game)?;
        start_tournament_game(settings, &game, &address)?;
        info!("[+] The game is starting on {address}. You can close this window.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        net::{Ipv4Addr, TcpListener},
        thread,
    };

    #[test]
    fn a_server_name_becomes_an_ipv4_address_for_the_referral() {
        assert_eq!(
            ipv4_address("127.0.0.1:3724").unwrap(),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3724)
        );
        assert_eq!(ipv4_address("localhost:4000").unwrap().port(), 4000);
        assert!(
            ipv4_address("no-such-host.invalid:3724")
                .unwrap_err()
                .to_string()
                .starts_with("Can't find the server no-such-host.invalid")
        );
    }

    #[test]
    fn the_battle_tag_comes_from_the_server_and_an_old_server_is_named() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let SocketAddr::V4(lobby) = listener.local_addr().unwrap() else {
            unreachable!()
        };
        let server = thread::spawn(move || {
            let (mut new, _) = listener.accept().unwrap();
            battle_tag_query::answer(&mut new, |name| format!("{name}x#1234")).unwrap();
            let (mut old, _) = listener.accept().unwrap();
            // An older lobby takes the query for a game hello and closes.
            old.write_all(b"").unwrap();
        });
        let address = lobby.to_string();
        assert_eq!(battle_tag_on(lobby, &address, "Ana").unwrap(), "Anax#1234");
        let error = battle_tag_on(lobby, &address, "Ana").unwrap_err();
        assert!(
            error.to_string().contains("runs an older version"),
            "{error}"
        );
        server.join().unwrap();
    }

    #[test]
    fn an_unreachable_server_is_named() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        reachable(&address).unwrap();
        drop(listener);
        let error = reachable(&address).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with(&format!("Can't reach the server {address}"))
        );
        let error = battle_tag_on(address.parse().unwrap(), &address, "Ana").unwrap_err();
        assert!(error.to_string().starts_with("Can't reach the server"));
    }
}
