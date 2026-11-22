//! The start banner: where the server listens, the addresses other players
//! join with and the ports to forward (`_log_banner` and
//! `network_addresses` in `ow174/cli.py`).

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs},
    path::Path,
};

use lucy_lobby::accounts::saved_names;
use lucy_server::server::Server;
use lucy_wire::jam::codec::Schemas;
use tracing::info;

use crate::{logging::LOG_FILE, settings::Settings};

/// What the banner shows.
struct Banner<'a> {
    bind: SocketAddrV4,
    /// `None` when `--game-port 0` turns the game server off.
    game: Option<SocketAddr>,
    battle_net: Option<(SocketAddr, SocketAddr)>,
    dashboard: Option<SocketAddr>,
    /// This PC's addresses other PCs can dial.
    network: &'a [Ipv4Addr],
    game_host: &'a str,
    accounts: &'a [String],
    /// The file new accounts copy.
    template: &'a str,
    /// Lobby message layouts and protocol groups.
    schemas: (usize, usize),
    log_file: &'a Path,
}

/// Log the banner for a started server.
pub fn log(server: &Server, settings: &Settings, game_host: &str) {
    let bind = server.lobby_addr();
    let network = if bind.ip().is_unspecified() {
        network_addresses()
    } else {
        Vec::new()
    };
    let accounts = saved_names(&settings.state.join("profiles")).unwrap_or_default();
    let template = settings
        .template
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(|| "profile.json".into(), |name| name.to_string_lossy());
    let banner = Banner {
        bind,
        game: server.game_addr(),
        battle_net: server.stall_addr().zip(server.rpc_addr()),
        dashboard: server.dashboard_addr(),
        network: &network,
        game_host,
        accounts: &accounts,
        template: &template,
        schemas: Schemas::lobby().counts(),
        log_file: &settings.logs().join(LOG_FILE),
    };
    for line in lines(&banner) {
        info!("{line}");
    }
}

fn lines(banner: &Banner<'_>) -> Vec<String> {
    let port = banner.bind.port();
    let mut lines = vec![
        "OVERWATCH 1.74 LOBBY SERVER".to_owned(),
        format!(" [*] Bind:           {}", banner.bind),
    ];
    if banner.bind.ip().is_unspecified() {
        let addresses: Vec<String> = banner
            .network
            .iter()
            .map(|address| format!("{address}:{port}"))
            .collect();
        lines.push(format!(
            " [*] Your network:   {}",
            if addresses.is_empty() {
                "no address found".to_owned()
            } else {
                addresses.join(", ")
            }
        ));
        lines.push(match (banner.game_host.is_empty(), banner.game) {
            (true, _) => " [*] Internet:       off (no public address, --game-host)".to_owned(),
            (false, Some(game)) => format!(
                " [*] Internet:       {}:{port} (open TCP {port} and UDP {} in your router and firewall)",
                banner.game_host,
                game.port()
            ),
            (false, None) => format!(
                " [*] Internet:       {}:{port} (open TCP {port} in your router and firewall)",
                banner.game_host
            ),
        });
    }
    lines.push(match banner.game {
        Some(game) => format!(" [*] Game server:    {game} (UDP)"),
        None => " [*] Game server:    off".to_owned(),
    });
    lines.push(match banner.battle_net {
        Some((stall, rpc)) => format!(" [*] Battle.net:     {stall} (relay RPC {rpc})"),
        None => " [*] Battle.net:     off".to_owned(),
    });
    lines.push(match banner.dashboard {
        Some(dashboard) => format!(" [*] Dashboard:      http://{dashboard}"),
        None => " [*] Dashboard:      off".to_owned(),
    });
    lines.push(format!(
        " [*] Accounts:       {} (profiles/)",
        if banner.accounts.is_empty() {
            "none yet".to_owned()
        } else {
            banner.accounts.join(", ")
        }
    ));
    lines.push(format!(" [*] New accounts:   copy of {}", banner.template));
    let (messages, protocols) = banner.schemas;
    lines.push(format!(
        " [*] Schemas:        {messages} messages in {protocols} protocols"
    ));
    lines.push(format!(
        " [*] Log file:       {}",
        banner.log_file.display()
    ));
    lines
}

/// This PC's IPv4 addresses that players on other PCs can dial (LAN, a VPN
/// such as Radmin): what its host name resolves to.
fn network_addresses() -> Vec<Ipv4Addr> {
    let Some(name) = host_name() else {
        return Vec::new();
    };
    (name.as_str(), 0)
        .to_socket_addrs()
        .map(|found| dialable(found.map(|address| address.ip())))
        .unwrap_or_default()
}

/// IPv4 addresses without loopback and link-local ones, each once, in order.
fn dialable(found: impl IntoIterator<Item = IpAddr>) -> Vec<Ipv4Addr> {
    let mut addresses = Vec::new();
    for address in found {
        if let IpAddr::V4(address) = address {
            if !address.is_loopback() && !address.is_link_local() && !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    addresses
}

#[cfg(windows)]
fn host_name() -> Option<String> {
    lucy_launcher::network::host_name()
}

#[cfg(not(windows))]
fn host_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_pcs_dial_the_lan_addresses_once() {
        let found = [
            "127.0.0.1",
            "192.168.1.5",
            "::1",
            "169.254.3.4",
            "26.10.0.2",
            "192.168.1.5",
        ]
        .map(|address| address.parse().unwrap());
        assert_eq!(
            dialable(found),
            [Ipv4Addr::new(192, 168, 1, 5), Ipv4Addr::new(26, 10, 0, 2)]
        );
    }

    #[test]
    fn a_host_bound_to_every_address_shows_where_to_join_and_what_to_forward() {
        let network = [Ipv4Addr::new(192, 168, 1, 5)];
        let mut banner = Banner {
            bind: "0.0.0.0:3724".parse().unwrap(),
            game: Some("0.0.0.0:3730".parse().unwrap()),
            battle_net: None,
            dashboard: None,
            network: &network,
            game_host: "1.2.3.4",
            accounts: &[],
            template: "profile.json",
            schemas: (707, 87),
            log_file: Path::new("lucy.log"),
        };
        let shown = lines(&banner);
        assert!(shown.contains(&" [*] Your network:   192.168.1.5:3724".to_owned()));
        assert!(shown.contains(
            &" [*] Internet:       1.2.3.4:3724 (open TCP 3724 and UDP 3730 in your router and firewall)"
                .to_owned()
        ));
        assert!(shown.contains(&" [*] Accounts:       none yet (profiles/)".to_owned()));

        banner.game_host = "";
        banner.network = &[];
        let shown = lines(&banner);
        assert!(shown.contains(&" [*] Your network:   no address found".to_owned()));
        assert!(
            shown.contains(&" [*] Internet:       off (no public address, --game-host)".to_owned())
        );

        banner.bind = "127.0.0.1:3724".parse().unwrap();
        assert!(!lines(&banner).iter().any(|line| line.contains("Internet")));

        // `--game-port 0`: no game server, and no UDP port to forward.
        banner.bind = "0.0.0.0:3724".parse().unwrap();
        banner.game_host = "1.2.3.4";
        banner.game = None;
        let shown = lines(&banner);
        assert!(shown.contains(&" [*] Game server:    off".to_owned()));
        assert!(
            shown.contains(
                &" [*] Internet:       1.2.3.4:3724 (open TCP 3724 in your router and firewall)"
                    .to_owned()
            )
        );
    }
}
