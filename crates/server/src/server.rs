//! The native server for the verified retail Practice Range path: the lobby,
//! the game server, the local Battle.net and the dashboard.

mod dashboard;

use std::{
    fs, io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    thread::{self, JoinHandle},
};

use crate::{
    game_data,
    game_service::{GameService, collision_build},
    lobby::{Lobby, matchmaker::Matchmaker, serve, services::Services},
    platform::{LocalGames, Platform},
    server::dashboard::{Dashboard, DashboardParts},
};
use lucy_bnet::{
    protocol::Protocol,
    rpc::Player,
    service::{self, BnetService},
};
use lucy_content::catalog::ItemDb;
use lucy_lobby::{
    accounts::{Account, Accounts, load_or_create_profile},
    arcade::Arcade,
    celebrations::Celebrations,
    clock::Clock,
    collection::Collection,
    economy::Economy,
    menu::extra_preload_themes,
    presence::{PRO_ACCOUNT_BITS, Presence},
    ranked::Ranked,
    templates::RetailTemplates,
    xp_table::XpTable,
};
use lucy_sim::scene_assets::SceneAssets;
use lucy_wire::jam::codec::Schemas;

pub struct ServerConfig {
    pub runtime: PathBuf,
    pub data: PathBuf,
    pub account: Option<String>,
    /// Where the lobby and the game server listen: 127.0.0.1 for this PC
    /// only, 0.0.0.0 for other PCs too. Battle.net and the dashboard always
    /// stay on this PC.
    pub host: Ipv4Addr,
    /// The public address sent in the handoff to games from outside the
    /// host's network (`--game-host`); empty for none.
    pub game_host: String,
    pub lobby_port: u16,
    /// The game server's UDP port; `None` leaves it off, and the lobby runs
    /// without matches (`--game-port 0`, `ow174/lobby/settings.py`).
    pub game_port: Option<u16>,
    /// Run the local Battle.net (the retail modes); a lobby only for other
    /// PCs and tournament games has none.
    pub battle_net: bool,
    pub stall_port: u16,
    pub rpc_port: u16,
    /// The dashboard's TCP port; `None` leaves it off.
    pub dashboard_port: Option<u16>,
    /// The retail games this PC starts, so Battle.net logs each in as its own
    /// account and the dashboard can start a second one.
    pub games: Option<Arc<dyn LocalGames>>,
    /// The game install the data was derived from: the collision of a map
    /// with no cache yet is built from it when a match first opens there.
    pub install: Option<PathBuf>,
    /// Start a match once this many players search a card without a number
    /// of its own; 0 = full teams only (`--test-players`,
    /// `ow174/lobby/settings.py`).
    pub test_players: usize,
    /// AI heroes of this level fill the missing players of a match that
    /// `test_players` starts (`--test-ai`; Practice vs AI, `lucy_sim::ai`).
    pub test_ai: Option<lucy_sim::ai::Difficulty>,
    /// The template profile for new accounts (`--save`); `profile.json` in
    /// the state folder when unset.
    pub template: Option<PathBuf>,
    /// Turn on the client's own engine console (Alt+backtick) and developer
    /// features in the login config (`--dev-console`).
    pub dev_console: bool,
    /// A Workshop script every player goes into from the menu: the custom
    /// match its settings describe (`--workshop`).
    pub workshop: Option<PathBuf>,
}

impl ServerConfig {
    pub fn new(runtime: PathBuf, data: PathBuf) -> Self {
        Self {
            runtime,
            data,
            account: None,
            host: Ipv4Addr::LOCALHOST,
            game_host: String::new(),
            lobby_port: 3724,
            game_port: Some(3730),
            battle_net: true,
            stall_port: service::STALL_PORT,
            rpc_port: service::RPC_PORT,
            dashboard_port: Some(3725),
            games: None,
            install: None,
            test_players: 0,
            test_ai: None,
            template: None,
            dev_console: false,
            workshop: None,
        }
    }
}

pub struct Server {
    lobby_addr: SocketAddrV4,
    game_addr: Option<SocketAddr>,
    stop_lobby: mpsc::Sender<()>,
    lobby: Option<JoinHandle<io::Result<()>>>,
    bnet: Option<BnetService>,
    game: Option<Arc<GameService>>,
    dashboard: Option<Dashboard>,
}

impl Server {
    /// Validate the data before starting the listeners. This entry point
    /// serves the base graph bundle.
    pub fn start(config: ServerConfig) -> io::Result<Self> {
        Self::start_on(config, Clock::wall(), GameService::bind)
    }

    /// Tests: the server on virtual clocks: the lobby's, and the game
    /// service's lockstep with its in-memory network
    /// (`game_service/lockstep.rs`). The configuration must turn the game
    /// server on.
    #[cfg(test)]
    pub(crate) fn start_virtual(config: ServerConfig) -> io::Result<(Self, VirtualTime)> {
        use crate::game_service::lockstep::Lockstep;

        let lobby = crate::lobby::clock::VirtualClock::default();
        let mut network = None;
        let server = Self::start_on(config, lobby.clock(), |_, schemas| {
            let (service, game) = Lockstep::new(schemas)?.into_parts();
            network = Some(game);
            Ok(service)
        })?;
        let game = network.expect("the game server is on");
        Ok((server, VirtualTime { lobby, game }))
    }

    /// The server with the lobby on `clock` and the game service that
    /// `game_service` starts at an address.
    fn start_on(
        config: ServerConfig,
        clock: Clock,
        game_service: impl FnOnce(SocketAddr, Arc<Schemas>) -> io::Result<GameService>,
    ) -> io::Result<Self> {
        let missing = game_data::missing_files(&config.runtime);
        if !missing.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing runtime files: {}", missing.join(", ")),
            ));
        }
        let data = config.runtime.join("data");
        let signing_key = *lucy_fixed::LOBBY_STATE_BLOB_KEY;
        let schemas = Schemas::lobby();
        let game_schemas = Arc::new(Schemas::game());
        let templates = RetailTemplates::captured();
        let items = Arc::new(ItemDb::load(&data)?);
        let collection = Collection::new(&templates, Arc::clone(&items))?;
        let presence = Presence::new(&templates)?;
        let ranked = Ranked::new(&templates)?;
        let arcade = Arcade::load(data.join("arcade_cards_174.json"), &templates)?;
        let economy = Arc::new(Economy::load(&data)?);
        let xp = XpTable::load(&data)?;
        let celebrations = Celebrations::load(&data, &templates, Arc::clone(&economy))?;
        let mut assets = SceneAssets::load(&config.runtime)?;
        if let Some(install) = &config.install {
            assets = assets.with_collision_build(collision_build(install.clone()));
        }
        // Skin themes newer than the capture, and the gun models.
        let extra_preload = extra_preload_themes(&items)
            .chain(assets.graphs().weapon_cosmetics())
            .collect();
        let matchmaker = Matchmaker::new(&data, &config.data, config.test_players, config.test_ai)?;
        matchmaker.set_workshop(config.workshop.as_deref())?;
        // The Practice Range's scene is prepared up front for the first match
        // (`ow174/lobby/server.py` `start_game_server`); no match starts
        // without the game server.
        let scene = if config.game_port.is_some() {
            let scene = assets.prepare(matchmaker.practice_map().clone())?;
            if !scene.movement_ready() {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "the native Practice Range requires its movement data",
                ));
            }
            Some(scene)
        } else {
            None
        };
        let protocol = Arc::new(Protocol::client());
        let template = config
            .template
            .clone()
            .unwrap_or_else(|| config.data.join("profile.json"));
        let accounts = Accounts::new(config.data.join("profiles"), &template)?;
        let local = local_account(
            &accounts,
            &config.data,
            &template,
            config.account.as_deref(),
        )?;
        let listener = TcpListener::bind(SocketAddrV4::new(config.host, config.lobby_port))
            .map_err(|error| taken(format!("Port {} is taken", config.lobby_port), error))?;
        listener.set_nonblocking(true)?;
        let SocketAddr::V4(lobby_addr) = listener.local_addr()? else {
            unreachable!("IPv4 lobby bind")
        };
        let game = config
            .game_port
            .map(|port| {
                game_service(SocketAddrV4::new(config.host, port).into(), game_schemas)
                    .map(Arc::new)
                    .map_err(|error| taken(format!("UDP port {port} is taken"), error))
            })
            .transpose()?;
        let game_addr = game.as_deref().map(GameService::local_addr);
        let selected = Arc::new(Mutex::new(local));
        let accounts = Arc::new(accounts);
        let platform = Arc::new(Platform::new(
            config.host,
            config.game_host.clone(),
            config.games.clone(),
        ));
        let bnet = if config.battle_net {
            // Known once the RPC port is bound, before any game logs in.
            let rpc_port = Arc::new(OnceLock::new());
            let player = battle_net_player(
                Arc::clone(&accounts),
                Arc::clone(&selected),
                Arc::clone(&platform),
                Arc::clone(&rpc_port),
            );
            let bnet = BnetService::start(
                addr(config.stall_port),
                addr(config.rpc_port),
                // The games are on this PC, whatever address the lobby listens on.
                addr(lobby_addr.port()),
                protocol,
                player,
            )
            .map_err(|error| taken("The Battle.net ports are taken".into(), error))?;
            let _ = rpc_port.set(bnet.rpc_addr().port());
            Some(bnet)
        } else {
            None
        };
        let services = Arc::new(Services::new(&collection, economy));
        let shared = Arc::new(Lobby {
            signing_key,
            schemas,
            templates,
            collection: Arc::new(collection),
            presence,
            ranked,
            arcade,
            celebrations,
            xp,
            extra_preload,
            dev_console: config.dev_console,
            assets: Arc::new(assets),
            first_scene: Mutex::new(scene),
            accounts,
            selected,
            active: Arc::default(),
            game: game.clone(),
            hub: Default::default(),
            platform: Arc::clone(&platform),
            services,
            social: Default::default(),
            matchmaker,
            clock: clock.clone(),
        });
        let dashboard = config
            .dashboard_port
            .map(|port| {
                let parts = DashboardParts {
                    accounts: Arc::clone(&shared.accounts),
                    selected: Arc::clone(&shared.selected),
                    online: Arc::clone(&shared.active),
                    collection: Arc::clone(&shared.collection),
                    data: config.data.clone(),
                    assets: config.data.join("assets"),
                    lobby_port: lobby_addr.port(),
                    platform,
                    services: Arc::clone(&shared.services),
                    lobby: Some(Arc::clone(&shared)),
                    clock,
                    debug_send: dashboard::debug_send_enabled(),
                };
                Dashboard::start(port, parts)
            })
            .transpose()?;
        let (stop_lobby, shutdown) = mpsc::channel();
        let lobby = thread::Builder::new()
            .name("lucy-lobby".into())
            .spawn(move || serve(listener, shared, shutdown))?;
        Ok(Self {
            lobby_addr,
            game_addr,
            stop_lobby,
            lobby: Some(lobby),
            bnet,
            game,
            dashboard,
        })
    }

    pub fn lobby_addr(&self) -> SocketAddrV4 {
        self.lobby_addr
    }
    /// The game server's address, when it runs.
    pub fn game_addr(&self) -> Option<SocketAddr> {
        self.game_addr
    }
    /// The game service, for a test to look at its matches.
    #[cfg(test)]
    pub(crate) fn game_service(&self) -> Option<&GameService> {
        self.game.as_deref()
    }
    /// The local Battle.net's port the game dials, when it runs.
    pub fn stall_addr(&self) -> Option<SocketAddr> {
        self.bnet.as_ref().map(BnetService::stall_addr)
    }
    /// The Battle.net RPC port the relay pipes to, when it runs.
    pub fn rpc_addr(&self) -> Option<SocketAddr> {
        self.bnet.as_ref().map(BnetService::rpc_addr)
    }
    pub fn dashboard_addr(&self) -> Option<SocketAddr> {
        self.dashboard.as_ref().map(Dashboard::addr)
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if let Some(dashboard) = &mut self.dashboard {
            dashboard.stop();
        }
        let bnet = self.bnet.as_mut().map_or(Ok(()), BnetService::stop);
        let lobby = if let Some(worker) = self.lobby.take() {
            let _ = self.stop_lobby.send(());
            worker
                .join()
                .map_err(|_| io::Error::other("lobby listener panicked"))
                .and_then(|result| result)
        } else {
            Ok(())
        };
        let game = self.game.take().map_or(Ok(()), |game| {
            Arc::try_unwrap(game)
                .map_err(|_| io::Error::other("game worker still in use"))?
                .stop()
        });
        bnet.and(lobby).and(game)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Tests: the clocks of a server on virtual clocks
/// ([`Server::start_virtual`]): the lobby's, which the test moves a lobby
/// tick at a time (`lobby/test_client.rs`), and the game service's network,
/// which runs its ticks.
#[cfg(test)]
pub(crate) struct VirtualTime {
    pub(crate) lobby: crate::lobby::clock::VirtualClock,
    pub(crate) game: crate::game_service::lockstep::Network,
}

/// Battle.net logs a game in as the account the lobby will use, with the ids
/// its presence shows (`_start_bnet` in `ow174/cli.py`, `game_account` in
/// `ow174/lobby/server.py`): a second game on this PC plays the account it
/// was started for, any other game the dashboard's account.
fn battle_net_player(
    accounts: Arc<Accounts>,
    selected: Arc<Mutex<Arc<Account>>>,
    platform: Arc<Platform>,
    rpc_port: Arc<OnceLock<u16>>,
) -> impl Fn(SocketAddr) -> Player + Send + Sync + 'static {
    move |peer| {
        let second = rpc_port
            .get()
            .and_then(|&server| platform.game_account_name(peer.port(), server))
            .and_then(|name| accounts.get(&name).ok());
        let account =
            second.unwrap_or_else(|| Arc::clone(&selected.lock().expect("selected account")));
        let id = u64::from(account.account_lo);
        Player {
            account: id,
            game_account: id ^ PRO_ACCOUNT_BITS,
            battle_tag: account.battle_tag(),
            name: account.name.clone(),
        }
    }
}

fn local_account(
    accounts: &Accounts,
    data: &Path,
    template: &Path,
    chosen: Option<&str>,
) -> io::Result<Arc<Account>> {
    match local_account_name(&accounts.all_saved()?, data, chosen)? {
        Some(name) => accounts.get(&name),
        None => accounts.get(&load_or_create_profile(template)?.player_name),
    }
}

/// The account the local game logs in as: the explicit choice, the saved
/// `default_account.txt` entry or the first saved profile. `None` means a new
/// profile from the template. Reads only.
pub fn local_account_name(
    saved: &[String],
    data: &Path,
    chosen: Option<&str>,
) -> io::Result<Option<String>> {
    if let Some(name) = chosen.map(str::trim) {
        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty local account name",
            ));
        }
        return Ok(Some(name.to_owned()));
    }
    let default = match fs::read_to_string(data.join("default_account.txt")) {
        Ok(name) => name.trim().to_owned(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    if let Some(name) = saved
        .iter()
        .find(|name| name.eq_ignore_ascii_case(&default))
        .or(saved.first())
    {
        return Ok(Some(name.clone()));
    }
    Ok(None)
}

/// A port another program holds, with Python's hint (`ow174/cli.py`).
fn taken(what: String, error: io::Error) -> io::Error {
    if error.kind() != io::ErrorKind::AddrInUse {
        return error;
    }
    io::Error::new(
        error.kind(),
        format!("{what} ({error}). Is the server already running in another window?"),
    )
}

fn addr(port: u16) -> SocketAddrV4 {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpStream,
        time::Duration,
    };

    use lucy_lobby::{
        battle_tag_query,
        collection::{HERO_CATALOG, PROGRESSION},
        handoff::GAME_REQUEST,
        login::{LOGIN_MESSAGE, OUT_CONNECT},
        player_messages as player,
        templates::IN_CONNECT,
    };
    use lucy_wire::{
        jam::codec::{Fields, Value},
        jam::framing::{self, FrameReader},
        jam::handshake::client_handshake,
    };

    #[test]
    fn battle_net_logs_a_second_game_in_as_its_account_and_others_as_the_dashboards() {
        struct Second;
        impl LocalGames for Second {
            fn start(&self, _: &str) {}
            fn account_of(&self, peer_port: u16, server_port: u16) -> Option<String> {
                ((peer_port, server_port) == (50_000, 21119)).then(|| "second".into())
            }
            fn second_accounts(&self) -> std::collections::HashSet<String> {
                ["second".into()].into()
            }
        }
        let data = tempfile::tempdir().unwrap();
        let accounts = Arc::new(
            Accounts::new(
                data.path().join("profiles"),
                data.path().join("profile.json"),
            )
            .unwrap(),
        );
        let first = accounts.get("First").unwrap();
        let second = accounts.get("Second").unwrap();
        let platform = Platform::new(Ipv4Addr::LOCALHOST, String::new(), Some(Arc::new(Second)));
        let rpc_port = Arc::new(OnceLock::new());
        let player = battle_net_player(
            accounts,
            Arc::new(Mutex::new(Arc::clone(&first))),
            Arc::new(platform),
            Arc::clone(&rpc_port),
        );
        let peer = |port| SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        rpc_port.set(21119).unwrap();
        let logged_in = player(peer(50_000));
        assert_eq!(
            (logged_in.name.as_str(), logged_in.account),
            ("Second", u64::from(second.account_lo))
        );
        assert_eq!(logged_in.game_account, logged_in.account ^ PRO_ACCOUNT_BITS);
        assert_eq!(logged_in.battle_tag, second.battle_tag());
        assert_eq!(player(peer(50_001)).name, "First");
    }

    #[test]
    fn the_login_account_is_the_choice_then_the_default_then_the_first_saved() {
        let data = tempfile::tempdir().unwrap();
        let saved = ["Alpha".to_owned(), "beta".to_owned()];
        let name = |chosen| local_account_name(&saved, data.path(), chosen).unwrap();
        assert_eq!(name(None), Some("Alpha".into()));
        fs::write(data.path().join("default_account.txt"), "BETA\n").unwrap();
        assert_eq!(name(None), Some("beta".into()));
        assert_eq!(name(Some(" Gamma ")), Some("Gamma".into()));
        assert!(local_account_name(&saved, data.path(), Some(" ")).is_err());
        assert_eq!(local_account_name(&[], data.path(), None).unwrap(), None);
    }

    #[test]
    #[ignore = "serial: asserts the server stops within 3 s, which other tests' load stretches; requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn serial_native_server_logs_in_saved_profile_and_loads_practice_over_udp() {
        use crate::server::{Server, ServerConfig};
        use lucy_content::{heroes::SOLDIER, maps::PRACTICE_RANGE_MAP};
        use lucy_core::bits::BitReader;
        use lucy_lobby::{
            accounts::load_or_create_profile,
            gallery::{EQUIP, EQUIPPED, GALLERY_OUT},
            handoff::CREATE_GAME,
            handoff::{HANDOFF, HANDOFF_MESSAGE},
        };
        use lucy_wire::{
            link::{ACK, KIND_FRAME, LinkCipher, MAX_DATAGRAM, Packet, SYN},
            messages::read_channel,
        };
        use std::{fs, net::UdpSocket, time::Instant};

        let runtime = lucy_testkit::runtime_dir();
        let state = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::new(runtime.clone(), state.path().into());
        config.lobby_port = 0;
        config.game_port = Some(0);
        config.stall_port = 0;
        config.rpc_port = 0;
        config.dashboard_port = Some(0);
        // A game on this PC is sent to the address it reached, not the public one.
        config.game_host = "games.example".into();
        let accounts = Accounts::new(
            state.path().join("profiles"),
            state.path().join("profile.json"),
        )
        .unwrap();
        accounts.get("earlier").unwrap();
        let rust = accounts.get("rust").unwrap();
        {
            let mut profile = rust.profile();
            profile.level = 3000;
            profile.endorsement_level = 5;
            profile.unlock_all = true;
        }
        rust.save().unwrap();
        fs::write(state.path().join("default_account.txt"), "RUST\n").unwrap();
        let schemas = Schemas::lobby();
        let game_schemas = Schemas::game();
        let collection = Collection::new(
            &RetailTemplates::captured(),
            Arc::new(ItemDb::load(&runtime.join("data")).unwrap()),
        )
        .unwrap();
        let original_skin = collection.equipped_skin(&rust.profile(), SOLDIER).unwrap();
        let skin = collection
            .owned_for_hero(&rust.profile(), SOLDIER)
            .into_iter()
            .filter_map(|guid| collection.items().get(guid))
            .find(|item| {
                item.item_type == "Skin"
                    && item.skin_theme != 0
                    && item.skin_theme != original_skin.guid
            })
            .unwrap();
        let equip = Fields::from([
            (0x78, Value::Unsigned(SOLDIER)),
            (0x80, Value::Unsigned(skin.guid)),
            (0x88, Value::Signed(0)),
        ]);
        let mut server = Server::start(config).unwrap();
        for address in [server.stall_addr(), server.rpc_addr()] {
            let address = address.unwrap();
            assert!(address.ip().is_loopback());
            TcpStream::connect(address).unwrap();
        }
        let lobby = server.lobby_addr();
        let mut stalled = TcpStream::connect(lobby).unwrap();
        stalled.write_all(b"HELLO").unwrap();
        assert_eq!(
            battle_tag_query::ask(lobby, "rust").unwrap(),
            rust.battle_tag()
        );

        let mut tcp = TcpStream::connect(lobby).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let (mut tx, rx) = client_handshake(&mut tcp);
        let groups = [
            IN_CONNECT,
            OUT_CONNECT,
            GAME_REQUEST,
            player::ENDORSEMENTS,
            PROGRESSION,
            HERO_CATALOG,
            HANDOFF,
            GALLERY_OUT,
        ];
        let mut announcement = vec![0, 0];
        announcement.extend_from_slice(&(groups.len() as u32).to_le_bytes());
        for group in groups {
            announcement.extend_from_slice(&group.to_le_bytes());
        }
        framing::send_frame(&mut tcp, &mut tx, &announcement).unwrap();
        for (crc, wire, id, fields) in [
            (OUT_CONNECT, 2, LOGIN_MESSAGE, Fields::new()),
            (
                GAME_REQUEST,
                3,
                CREATE_GAME,
                Fields::from([
                    (0x78, Value::Unsigned(2)),
                    (0x80, Value::String(String::new())),
                    (0xa8, Value::Unsigned(4)),
                ]),
            ),
            (GALLERY_OUT, 8, EQUIP, equip.clone()),
        ] {
            let mut frame = schemas.header(crc, wire, id).unwrap();
            frame.extend(schemas.encode(crc, id, &fields).unwrap());
            framing::send_frame(&mut tcp, &mut tx, &frame).unwrap();
        }

        let wanted = [
            (IN_CONNECT, 1, 20500),
            (player::ENDORSEMENTS, 4, 52002),
            (PROGRESSION, 5, 24300),
            (HERO_CATALOG, 6, 24900),
            (HANDOFF, 7, HANDOFF_MESSAGE),
            (HERO_CATALOG, 6, EQUIPPED),
        ];
        let mut received = std::collections::HashMap::new();
        let mut reader = FrameReader::new(rx);
        let mut buf = [0; 8192];
        let limit = Instant::now() + Duration::from_secs(5);
        while received.len() < wanted.len() {
            let count = tcp.read(&mut buf).unwrap();
            assert!(
                count != 0 && Instant::now() < limit,
                "incomplete native lobby login or handoff"
            );
            for frame in reader.feed(&buf[..count]) {
                for (crc, wire, id) in wanted {
                    if let Some(body) =
                        frame.strip_prefix(schemas.header(crc, wire, id).unwrap().as_slice())
                    {
                        received.insert(id, schemas.decode(crc, id, body, true).unwrap());
                    }
                }
            }
        }
        fn structure(value: &Value) -> &Fields {
            let Value::Struct(fields) = value else {
                panic!("expected structured protocol fields")
            };
            fields
        }
        fn owned_ids(value: &Value) -> impl Iterator<Item = u64> + '_ {
            let Value::Array(items) = value else {
                panic!("expected collection ownership array")
            };
            items.iter().map(|item| {
                let fields = structure(item);
                let Value::Unsigned(guid) = fields[&0] else {
                    panic!("missing cosmetic ID")
                };
                assert_eq!(fields[&8], Value::Bool(true));
                guid
            })
        }
        assert_eq!(received[&20500][&0xa8], Value::String(rust.battle_tag()));
        assert_eq!(structure(&received[&52002][&0x88])[&0x18], Value::Signed(5));
        let progression = structure(&received[&24300][&0x78]);
        assert_eq!(progression[&0x68], Value::Signed(3000));
        let Value::Array(heroes) = &received[&24900][&0x80] else {
            panic!("missing hero collection")
        };
        let cosmetics: std::collections::HashSet<_> = owned_ids(&progression[&0x30])
            .chain(
                heroes
                    .iter()
                    .flat_map(|hero| owned_ids(&structure(hero)[&0])),
            )
            .collect();
        assert_eq!(cosmetics.len(), 7122);
        assert_eq!(received[&EQUIPPED], equip);
        let saved = load_or_create_profile(rust.path.as_deref().unwrap()).unwrap();
        assert_eq!(
            collection.equipped_skin(&saved, SOLDIER).unwrap().guid,
            skin.skin_theme
        );

        let ticket = structure(&received[&HANDOFF_MESSAGE][&0x80]);
        assert_eq!(
            ticket[&0x2c],
            Value::Unsigned(u64::from(server.game_addr().unwrap().port()))
        );
        let Value::Array(address) = &ticket[&0x2e] else {
            panic!("missing game address")
        };
        let address: Vec<u64> = address
            .iter()
            .take(10)
            .map(|byte| match byte {
                Value::Unsigned(byte) => *byte,
                _ => panic!("invalid address byte"),
            })
            .collect();
        assert_eq!(address, b"127.0.0.1\0".map(u64::from));
        let Value::Unsigned(connection) = ticket[&0x28] else {
            panic!("missing UDP connection")
        };
        let key = |offset| {
            let Value::Array(values) = &ticket[&offset] else {
                panic!("missing UDP key")
            };
            std::array::from_fn(|index| {
                let Value::Unsigned(byte) = values[index] else {
                    panic!("invalid UDP key byte")
                };
                u8::try_from(byte).unwrap()
            })
        };
        let cipher = LinkCipher::new(key(0xce), key(0xae));
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let game = server.game_addr().unwrap();
        udp.connect(game).unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut packet = Packet {
            conn: u32::try_from(connection).unwrap(),
            flags: SYN,
            ..Packet::default()
        };
        udp.send(&cipher.seal(&packet).unwrap()).unwrap();
        let mut buf = [0; MAX_DATAGRAM];
        let count = udp.recv(&mut buf).unwrap();
        let syn_ack = cipher.open(&buf[..count]).unwrap();
        assert_eq!((syn_ack.conn, syn_ack.flags), (packet.conn, SYN | ACK));
        packet.seq += 1;
        packet.flags = ACK;
        packet.ack = syn_ack.seq.wrapping_add(1);
        udp.send(&cipher.seal(&packet).unwrap()).unwrap();
        let count = udp.recv(&mut buf).unwrap();
        let load = cipher.open(&buf[..count]).unwrap();
        assert_eq!(load.kind, KIND_FRAME);
        let mut bits = BitReader::new(&load.payload);
        bits.var_a().unwrap();
        let channel = read_channel(&mut bits, &game_schemas).unwrap();
        assert_eq!(channel.reliable[0].message.id, 20300);
        assert_eq!(
            channel.reliable[0].message.fields[&0x4a8],
            Value::Unsigned(PRACTICE_RANGE_MAP)
        );

        let closing = Instant::now();
        server.stop().unwrap();
        assert!(closing.elapsed() < Duration::from_secs(3));
        TcpListener::bind(lobby).unwrap();
        UdpSocket::bind(game).unwrap();
    }

    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn lobby_services_answer_the_game_and_dashboard_edits_reach_it() {
        use lucy_lobby::{
            accounts::load_or_create_profile,
            leaderboard::{LEADERBOARD_OUT, PAGE_REQUEST, PROFILES},
            lookups::{
                NAME_QUERY, NAME_REPLY, NAMES, OWL_LIVE, OWL_MATCHES, OWL_POLL, STORE, STORE_PAGE,
                STORE_QUERY,
            },
            lootbox::{OPEN_BOX, PROGRESSION_OUT},
            mode_rules::PROFILE_REQUEST,
            saved_settings::SOCIAL_OUT,
            shop::{Currency, PURCHASE, Shop},
            templates::LOBBY,
        };
        use std::collections::HashMap;

        use crate::lobby::{
            TICK,
            test_client::{Client, run},
        };

        let runtime = lucy_testkit::runtime_dir();
        let state = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::new(runtime.clone(), state.path().into());
        config.lobby_port = 0;
        config.game_port = Some(0);
        config.stall_port = 0;
        config.rpc_port = 0;
        config.dashboard_port = Some(0);
        let accounts = Accounts::new(
            state.path().join("profiles"),
            state.path().join("profile.json"),
        )
        .unwrap();
        accounts.get("rust").unwrap();
        let other = accounts.get("other").unwrap();
        fs::write(state.path().join("default_account.txt"), "rust\n").unwrap();
        let collection = Collection::new(
            &RetailTemplates::captured(),
            Arc::new(ItemDb::load(&runtime.join("data")).unwrap()),
        )
        .unwrap();
        let shop = Shop::new(&collection);
        let product = collection
            .store()
            .keys()
            .copied()
            .find(|&guid| {
                shop.product(guid)
                    .is_some_and(|offer| offer.currency == Currency::Credits && offer.price <= 1000)
                    && !collection.owns(&other.profile(), guid)
            })
            .unwrap();
        let price = shop.product(product).unwrap().price;
        let credits = other.profile().credits;
        let first_box = other.profile().loot_boxes[0].id;
        let (mut server, time) = Server::start_virtual(config).unwrap();
        let dashboard = server.dashboard_addr().unwrap();

        let groups = [
            IN_CONNECT,
            OUT_CONNECT,
            PROGRESSION,
            HERO_CATALOG,
            PROGRESSION_OUT,
            SOCIAL_OUT,
            STORE_QUERY,
            STORE,
            NAME_QUERY,
            NAME_REPLY,
            OWL_POLL,
            OWL_LIVE,
            LOBBY,
            PROFILES,
            LEADERBOARD_OUT,
            player::PERMISSIONS,
        ];
        let schemas = Schemas::lobby();
        let mut client = Client::connect(&schemas, SocketAddr::from(server.lobby_addr()), &groups);
        let id16 = |lo: u64| {
            Value::Struct(Fields::from([
                (0, Value::Unsigned(lo)),
                (8, Value::Unsigned(1 << 56)),
            ]))
        };
        let lo = u64::from(other.account_lo);
        // A local game that logs in by name: the dashboard follows it.
        client.send(
            OUT_CONNECT,
            LOGIN_MESSAGE,
            Fields::from([(0x78, Value::String("other".into()))]),
        );
        client.send(
            PROGRESSION_OUT,
            OPEN_BOX,
            Fields::from([(
                0x78,
                Value::Struct(Fields::from([(
                    0,
                    Value::Array(vec![Value::Unsigned(first_box), Value::Unsigned(0)]),
                )])),
            )]),
        );
        client.send(
            PROGRESSION_OUT,
            PURCHASE,
            Fields::from([(0x78, Value::Unsigned(product))]),
        );
        client.send(
            SOCIAL_OUT,
            22202,
            Fields::from([(
                0x78,
                Value::Struct(Fields::from([
                    (0, Value::Bool(false)),
                    (5, Value::Unsigned(2)),
                ])),
            )]),
        );
        client.send(STORE_QUERY, STORE_PAGE, Fields::new());
        client.send(
            NAME_QUERY,
            NAMES,
            Fields::from([(0x78, Value::Array(vec![id16(lo)]))]),
        );
        client.send(OWL_POLL, OWL_MATCHES, Fields::new());
        client.send(
            SOCIAL_OUT,
            PROFILE_REQUEST,
            Fields::from([(0x78, id16(lo)), (0x88, id16(lo))]),
        );
        client.send(
            LEADERBOARD_OUT,
            PAGE_REQUEST,
            Fields::from([(
                0x78,
                Value::Struct(Fields::from([
                    (0, Value::Unsigned(32 | (4 << 24))),
                    (8, Value::Unsigned(1)),
                ])),
            )]),
        );

        // What the client received of these, by id; each must be there.
        let take = |client: &mut Client<'_>, wanted: &[(u32, u32)]| {
            let received: HashMap<u32, Fields> = client
                .take()
                .into_iter()
                .filter(|&(crc, id, _)| wanted.contains(&(crc, id)))
                .map(|(_, id, fields)| (id, fields))
                .collect();
            for (_, id) in wanted {
                assert!(
                    received.contains_key(id),
                    "missing {id}, got {:?}",
                    received.keys()
                );
            }
            received
        };
        let replies = take(
            &mut client,
            &[
                (PROGRESSION, 24305),
                (PROGRESSION, 24307),
                (PROGRESSION, 24306),
                (STORE, 26400),
                (NAME_REPLY, 58301),
                (OWL_LIVE, 43200),
                (LOBBY, 20807),
                (PROFILES, 39002),
                (PROFILES, 39000),
            ],
        );
        assert_eq!(replies[&24306][&0x78], Value::Unsigned(product));
        let Value::Struct(opened) = &replies[&24305][&0x78] else {
            panic!("box result")
        };
        let Value::Array(drops) = &opened[&0] else {
            panic!("drops")
        };
        assert_eq!(drops.len(), 4);
        let Value::Array(names) = &replies[&58301][&0x78] else {
            panic!("names")
        };
        let Value::Struct(name) = &names[0] else {
            panic!("a name")
        };
        assert_eq!(name[&0x18], Value::String("other".into()));
        assert_eq!(replies[&20807][&0x78], id16(lo));

        let saved = load_or_create_profile(other.path.as_deref().unwrap()).unwrap();
        // The whole decoded block, every field present as the client sent it.
        let social = &saved.settings["+0x10D"];
        assert_eq!(
            (&social["+0x0"], &social["+0x5"]),
            (&serde_json::json!(false), &serde_json::json!(2))
        );
        assert_eq!(social.as_object().unwrap().len(), 17);
        assert!(saved.unlocked_guids().contains(&product));
        // Each slot gave at most 500 credits instead of an item or for a duplicate.
        assert!((credits - price..=credits - price + 4 * 500).contains(&saved.credits));
        assert!(saved.loot_boxes.iter().all(|item| item.id != first_box));

        // The dashboard edits the account the game logged in as, and the game
        // gets the new state.
        let post = |path: &str, body: &str| {
            let mut stream = TcpStream::connect(dashboard).unwrap();
            write!(
                stream,
                "POST {path} HTTP/1.0\r\nHost: {dashboard}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let mut reply = String::new();
            stream.read_to_string(&mut reply).unwrap();
            reply
        };
        let reply = post("/api/update_profile", r#"{"level": 777}"#);
        assert!(
            reply.starts_with("HTTP/1.0 200") || reply.starts_with("HTTP/1.1 200"),
            "{reply}"
        );
        // The edit reaches the game at the session's next tick (`services.rs`).
        run(&time.lobby, &mut [&mut client], TICK);
        let pushed = take(
            &mut client,
            &[(PROGRESSION, 24313), (player::PERMISSIONS, 55500)],
        );
        assert_eq!(pushed[&24313][&0x78], Value::Signed(777));
        client.close();
        server.stop().unwrap();
    }
}
