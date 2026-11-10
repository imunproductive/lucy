//! Loopback-only native server for the verified retail Practice Range path.

mod dashboard;

use std::{
    collections::HashMap,
    fs, io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    game_service::GameService,
    practice::{self, SoloPracticeHandoff},
    runtime::missing_runtime_files,
    server::dashboard::{Dashboard, DashboardParts},
};
use orpheus_bnet::{
    protocol::Protocol,
    rpc::Player,
    service::{self, BnetService},
};
use orpheus_content::{catalog::ItemDb, maps::GameMap};
use orpheus_lobby::{
    accounts::{Account, Accounts, load_or_create_profile},
    arcade::Arcade,
    celebrations::Celebrations,
    collection::Collection,
    gallery::EquipRequest,
    handoff::{self, LobbySeat, general_channel, join_chat_channel},
    login::SIGNED_IN_ELSEWHERE,
    menu::{MenuHero, extra_preload_themes},
    presence::{PRO_ACCOUNT_BITS, Presence},
    ranked::{LeaderboardPlaces, Ranked},
    session::{LobbySession, LoginContent, LoginRequest, SoloLoginParty},
    templates::RetailTemplates,
};
use orpheus_sim::scene_assets::{PreparedScene, SceneAssets};
use orpheus_wire::{
    jam::codec::Schemas,
    tcp::{PeerStream, PeerWorkers, accept},
};

const MAX_LOBBY_PEERS: usize = 32;
const ACCEPT_POLL: Duration = Duration::from_millis(10);

pub struct ServerConfig {
    pub runtime: PathBuf,
    pub data: PathBuf,
    pub account: Option<String>,
    pub lobby_port: u16,
    pub game_port: u16,
    pub stall_port: u16,
    pub rpc_port: u16,
    /// The dashboard's TCP port; `None` leaves it off.
    pub dashboard_port: Option<u16>,
}

impl ServerConfig {
    pub fn new(runtime: PathBuf, data: PathBuf) -> Self {
        Self {
            runtime,
            data,
            account: None,
            lobby_port: 3724,
            game_port: 3730,
            stall_port: service::STALL_PORT,
            rpc_port: service::RPC_PORT,
            dashboard_port: Some(3725),
        }
    }
}

pub struct Server {
    lobby_addr: SocketAddrV4,
    game_addr: SocketAddr,
    stop_lobby: mpsc::Sender<()>,
    lobby: Option<JoinHandle<io::Result<()>>>,
    bnet: BnetService,
    game: Option<Arc<GameService>>,
    dashboard: Option<Dashboard>,
}

impl Server {
    /// Validate the data before starting the loopback listeners. This entry
    /// point serves the base graph bundle and contains no client launcher.
    pub fn start(config: ServerConfig) -> io::Result<Self> {
        let missing = missing_runtime_files(&config.runtime, true);
        if !missing.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("missing runtime files: {}", missing.join(", ")),
            ));
        }
        let data = config.runtime.join("data");
        let signing_key = *orpheus_fixed::LOBBY_STATE_BLOB_KEY;
        let schemas = Schemas::lobby();
        let game_schemas = Arc::new(Schemas::game());
        let templates = RetailTemplates::captured();
        let items = Arc::new(ItemDb::load(&data)?);
        let collection = Collection::new(&templates, Arc::clone(&items))?;
        let presence = Presence::new(&templates)?;
        let ranked = Ranked::new(&templates)?;
        let arcade = Arcade::load(data.join("arcade_cards_174.json"), &templates)?;
        let celebrations = Celebrations::load(&data, &templates)?;
        let assets = SceneAssets::load(&config.runtime)?;
        // Skin themes newer than the capture, and the gun models.
        let extra_preload = extra_preload_themes(&items)
            .chain(assets.graphs().weapon_cosmetics())
            .collect();
        let scene = assets.prepare(GameMap::practice_range())?;
        if !scene.movement_ready() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the native Practice Range requires the base graph bundle",
            ));
        }
        let protocol = Arc::new(Protocol::client());
        let template = config.data.join("profile.json");
        let accounts = Accounts::new(config.data.join("profiles"), &template)?;
        let local = local_account(
            &accounts,
            &config.data,
            &template,
            config.account.as_deref(),
        )?;
        let listener = TcpListener::bind(addr(config.lobby_port))?;
        listener.set_nonblocking(true)?;
        let SocketAddr::V4(lobby_addr) = listener.local_addr()? else {
            unreachable!("IPv4 lobby bind")
        };
        let game = Arc::new(GameService::bind(
            addr(config.game_port).into(),
            game_schemas,
        )?);
        let game_addr = game.local_addr();
        let selected = Arc::new(Mutex::new(local));
        let bnet_selected = Arc::clone(&selected);
        let bnet = BnetService::start(
            addr(config.stall_port),
            addr(config.rpc_port),
            lobby_addr,
            protocol,
            move |_| {
                let bnet_account = Arc::clone(&bnet_selected.lock().expect("selected account"));
                let account = u64::from(bnet_account.account_lo);
                Player {
                    account,
                    game_account: account ^ PRO_ACCOUNT_BITS,
                    battle_tag: bnet_account.battle_tag(),
                    name: bnet_account.name.clone(),
                }
            },
        )?;
        let shared = Arc::new(Lobby {
            signing_key,
            schemas,
            templates,
            collection: Arc::new(collection),
            presence,
            ranked,
            arcade,
            celebrations,
            extra_preload,
            assets,
            first_scene: Mutex::new(Some(scene)),
            accounts: Arc::new(accounts),
            selected,
            active: Arc::default(),
            game: Arc::clone(&game),
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
                };
                Dashboard::start(port, parts)
            })
            .transpose()?;
        let (stop_lobby, shutdown) = mpsc::channel();
        let lobby = thread::Builder::new()
            .name("orpheus-lobby".into())
            .spawn(move || serve(listener, shared, shutdown))?;
        Ok(Self {
            lobby_addr,
            game_addr,
            stop_lobby,
            lobby: Some(lobby),
            bnet,
            game: Some(game),
            dashboard,
        })
    }

    pub fn lobby_addr(&self) -> SocketAddrV4 {
        self.lobby_addr
    }
    pub fn game_addr(&self) -> SocketAddr {
        self.game_addr
    }
    pub fn stall_addr(&self) -> SocketAddr {
        self.bnet.stall_addr()
    }
    pub fn rpc_addr(&self) -> SocketAddr {
        self.bnet.rpc_addr()
    }
    pub fn dashboard_addr(&self) -> Option<SocketAddr> {
        self.dashboard.as_ref().map(Dashboard::addr)
    }

    pub fn stop(&mut self) -> io::Result<()> {
        if let Some(dashboard) = &mut self.dashboard {
            dashboard.stop();
        }
        let bnet = self.bnet.stop();
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

struct Lobby {
    signing_key: [u8; 64],
    schemas: Schemas,
    templates: RetailTemplates,
    collection: Arc<Collection>,
    presence: Presence,
    ranked: Ranked,
    arcade: Arcade,
    celebrations: Celebrations,
    extra_preload: Vec<u64>,
    assets: SceneAssets,
    first_scene: Mutex<Option<PreparedScene>>,
    accounts: Arc<Accounts>,
    /// The account a freshly started game logs in as; the dashboard can change it.
    selected: Arc<Mutex<Arc<Account>>>,
    active: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    game: Arc<GameService>,
}

struct LoginLease<'a> {
    lobby: &'a Lobby,
    account: Arc<Account>,
    replaced: Arc<AtomicBool>,
    seat: Option<LobbySeat>,
}

impl<'a> LoginLease<'a> {
    fn new(lobby: &'a Lobby, account: Arc<Account>) -> Self {
        let replaced = Arc::new(AtomicBool::new(false));
        let mut active = lobby.active.lock().expect("lobby accounts");
        if let Some(previous) = active.insert(account.account_lo, Arc::clone(&replaced)) {
            previous.store(true, Ordering::Release);
        }
        account.set_status(1);
        Self {
            lobby,
            account,
            replaced,
            seat: None,
        }
    }

    fn current(&self) -> bool {
        !self.replaced.load(Ordering::Acquire)
    }
}

impl Drop for LoginLease<'_> {
    fn drop(&mut self) {
        if let Some(seat) = self.seat.take() {
            let _ = self.lobby.game.release_seat(&seat.handoff);
        }
        let mut active = self.lobby.active.lock().expect("lobby accounts");
        if active
            .get(&self.account.account_lo)
            .is_some_and(|current| Arc::ptr_eq(current, &self.replaced))
        {
            active.remove(&self.account.account_lo);
            self.account.set_status(0);
        }
    }
}

impl Lobby {
    fn selected_account(&self) -> Arc<Account> {
        Arc::clone(&self.selected.lock().expect("selected account"))
    }

    fn serve_peer(&self, stream: PeerStream, id: u32) -> io::Result<()> {
        let Some(mut session) = LobbySession::accept_peer(
            stream,
            id,
            &self.signing_key,
            &self.accounts,
            &self.schemas,
        )?
        else {
            return Ok(());
        };
        let login = match session.prepare_login(&self.accounts, || Ok(self.selected_account()))? {
            LoginRequest::Prepared(login) => login,
            LoginRequest::Rejected { reason } => {
                session.send_disconnect(reason)?;
                return Ok(());
            }
            LoginRequest::Disconnected => return Ok(()),
        };
        let mut lease = LoginLease::new(self, Arc::clone(&login.account));
        let wall_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        let (menu, members_may_invite) = {
            let mut profile = login.account.profile();
            profile.last_online = wall_time as i64;
            let menu = MenuHero::default().select(
                &profile,
                self.collection.heroes(),
                self.collection.items(),
            )?;
            let members = profile
                .settings
                .get("+0x10D")
                .and_then(|settings| settings.get("+0x0"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            (menu, members)
        };
        login.account.save()?;
        let messages = login.available_messages(
            &self.schemas,
            &self.templates,
            &self.collection,
            &self.presence,
            &self.ranked,
            LoginContent {
                menu_hero: menu.menu_guid,
                extra_preload: &self.extra_preload,
                wall_time,
                arcade: &self.arcade,
                celebrations: Some(&self.celebrations),
                leaderboard_places: &LeaderboardPlaces::new(),
                solo_party: Some(SoloLoginParty {
                    id: login.identity.party_id,
                    entity: login.identity.party_entity,
                    menu_hero: menu.party_hero,
                    members_may_invite,
                }),
            },
        )?;
        for message in messages {
            if !lease.current() {
                break;
            }
            session.send(&message)?;
        }
        if lease.current() {
            session.send(&join_chat_channel(general_channel()))?;
        }
        while let Some(request) = session.receive_message_until(|| lease.current())? {
            if let Some(equip) = EquipRequest::read(&request) {
                if let Some(equipped) = equip.apply(&login, &self.collection, wall_time)? {
                    if let (Some(seat), Some(skin)) = (&lease.seat, equipped.skin) {
                        self.game.update_skin(&seat.handoff, equip.hero, skin)?;
                    }
                    for message in equipped.messages {
                        session.send(&message)?;
                    }
                }
                continue;
            }
            if !handoff::is_request(&request) {
                continue;
            }
            if let Some(old) = &lease.seat {
                if self.game.seat_exists(&old.handoff)? {
                    continue;
                }
            }
            let input = SoloPracticeHandoff::new(match_id()?, "127.0.0.1");
            lease.seat = practice::dispatch_scene(
                &request,
                &login,
                &self.game,
                input,
                (&self.assets, &self.collection),
                |map| {
                    let first = self.first_scene.lock().expect("prepared scene").take();
                    first.map_or_else(|| self.assets.prepare(map), Ok)
                },
                |message| session.send(message),
            )?;
        }
        if !lease.current() {
            session.send_disconnect(SIGNED_IN_ELSEWHERE)?;
        }
        Ok(())
    }
}

fn serve(listener: TcpListener, lobby: Arc<Lobby>, shutdown: mpsc::Receiver<()>) -> io::Result<()> {
    let mut peers = PeerWorkers::default();
    let mut id = 0u32;
    while matches!(shutdown.try_recv(), Err(mpsc::TryRecvError::Empty)) {
        peers.reap();
        if let Some((socket, _)) = accept(&listener)? {
            if peers.len() < MAX_LOBBY_PEERS {
                id = id.wrapping_add(1);
                let lobby = Arc::clone(&lobby);
                let _ = peers.spawn("orpheus-lobby-peer", socket, move |stream| {
                    let _ = lobby.serve_peer(stream, id);
                });
            }
        } else if !matches!(
            shutdown.recv_timeout(ACCEPT_POLL),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            break;
        }
    }
    Ok(())
}

fn match_id() -> io::Result<[u64; 2]> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(std::array::from_fn(|i| {
        u64::from_le_bytes(random[i * 8..i * 8 + 8].try_into().unwrap()) >> 1
    }))
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
    };

    use orpheus_lobby::{
        battle_tag_query,
        collection::{HERO_CATALOG, PROGRESSION},
        handoff::GAME_REQUEST,
        login::{LOGIN_MESSAGE, OUT_CONNECT},
        player_messages as player,
        templates::IN_CONNECT,
    };
    use orpheus_wire::{
        jam::codec::{Fields, Value},
        jam::framing::{self, FrameReader},
        jam::handshake::client_handshake,
    };

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
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn native_server_logs_in_saved_profile_and_loads_practice_over_udp() {
        use crate::server::{Server, ServerConfig};
        use orpheus_content::{heroes::SOLDIER, maps::PRACTICE_RANGE_MAP};
        use orpheus_core::bits::BitReader;
        use orpheus_lobby::{
            accounts::load_or_create_profile,
            gallery::{EQUIP, EQUIPPED, GALLERY_OUT},
            handoff::CREATE_GAME,
            handoff::{HANDOFF, HANDOFF_MESSAGE},
        };
        use orpheus_sim::scene_assets::SceneAssets;
        use orpheus_wire::{
            link::{ACK, KIND_FRAME, LinkCipher, MAX_DATAGRAM, Packet, SYN},
            messages::read_channel,
        };
        use std::{fs, net::UdpSocket, time::Instant};

        let runtime = orpheus_testkit::runtime_dir();
        let state = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::new(runtime.clone(), state.path().into());
        config.lobby_port = 0;
        config.game_port = 0;
        config.stall_port = 0;
        config.rpc_port = 0;
        if !SceneAssets::load(&runtime)
            .unwrap()
            .prepare(orpheus_content::maps::GameMap::practice_range())
            .unwrap()
            .movement_ready()
        {
            assert!(
                matches!(Server::start(config), Err(error) if error.kind() == io::ErrorKind::Unsupported)
            );
            assert!(!state.path().join("profiles").exists());
            return;
        }
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
            Value::Unsigned(u64::from(server.game_addr().port()))
        );
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
        let game = server.game_addr();
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
}
