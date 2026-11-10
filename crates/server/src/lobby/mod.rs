//! The lobby: one thread per client connection, the shared state they read,
//! and the loop that answers each logged-in client.

mod handlers;
pub(crate) mod hub;
pub(crate) mod peer;
pub(crate) mod router;
pub(crate) mod services;

use std::{
    collections::HashMap,
    io,
    net::TcpListener,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use orpheus_lobby::{
    accounts::{Account, Accounts},
    arcade::Arcade,
    celebrations::Celebrations,
    collection::Collection,
    handoff::{LobbySeat, general_channel, join_chat_channel},
    login::SIGNED_IN_ELSEWHERE,
    menu::MenuHero,
    presence::Presence,
    ranked::Ranked,
    session::{LobbySession, LoginContent, LoginRequest, SoloLoginParty},
    templates::RetailTemplates,
};
use orpheus_sim::scene_assets::{PreparedScene, SceneAssets};
use orpheus_wire::{
    jam::codec::{Fields, Schemas},
    tcp::{PeerStream, PeerWorkers, accept},
};

use crate::{game_service::GameService, platform::Platform};
use hub::{Hub, Mailbox};
use peer::Peer;
use router::Router;

const MAX_LOBBY_PEERS: usize = 32;
const ACCEPT_POLL: Duration = Duration::from_millis(10);
/// The longest a session sleeps between ticks.
pub(crate) const TICK: Duration = Duration::from_millis(250);
/// How long a kicked client keeps the link after the disconnect reason, so
/// its login screen shows the reason, not "lost connection"
/// (`ow174/lobby/session.py` `KICK_GRACE_SECONDS`).
const KICK_GRACE: Duration = Duration::from_millis(1500);
/// Client telemetry groups, never logged (`ow174/jam/groups.py`).
const TELEMETRY: [u32; 2] = [0xC64B_397E, 0x692C_511B];
/// Characters of an unhandled message shown in the log.
const LOG_VALUE_LIMIT: usize = 200;

pub(crate) struct Lobby {
    pub signing_key: [u8; 64],
    pub schemas: Schemas,
    pub templates: RetailTemplates,
    pub collection: Arc<Collection>,
    pub presence: Presence,
    pub ranked: Ranked,
    pub arcade: Arcade,
    pub celebrations: Celebrations,
    pub extra_preload: Vec<u64>,
    pub assets: SceneAssets,
    pub first_scene: Mutex<Option<PreparedScene>>,
    pub accounts: Arc<Accounts>,
    /// The account a freshly started game logs in as; the dashboard can change it.
    pub selected: Arc<Mutex<Arc<Account>>>,
    pub active: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    pub game: Arc<GameService>,
    pub hub: Hub,
    /// Hosting for other PCs and the games this PC started.
    pub platform: Arc<Platform>,
    /// Loot boxes, the shop, login rewards and the dashboard's live updates.
    pub services: Arc<services::Services>,
}

struct LoginLease<'a> {
    lobby: &'a Lobby,
    account: Arc<Account>,
    replaced: Arc<AtomicBool>,
    mailbox: Arc<Mailbox>,
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
        let mailbox = lobby.hub.join(account.account_lo);
        Self {
            lobby,
            account,
            replaced,
            mailbox,
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
        self.lobby.hub.leave(self.account.account_lo, &self.mailbox);
        let mut active = self.lobby.active.lock().expect("lobby accounts");
        if active
            .get(&self.account.account_lo)
            .is_some_and(|current| Arc::ptr_eq(current, &self.replaced))
        {
            active.remove(&self.account.account_lo);
            drop(active);
            self.account.set_status(0);
            // `ow174/lobby/session.py` `_cleanup`.
            if let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) {
                self.account.profile().last_online = now.as_secs() as i64;
                if let Err(error) = self.account.save() {
                    tracing::error!("saving {} at logout failed: {error}", self.account.name);
                }
            }
            for module in handlers::MODULES {
                if let Some(logout) = module.hooks.logout {
                    logout(self.lobby, &self.account);
                }
            }
        }
    }
}

static ROUTER: LazyLock<Router> = LazyLock::new(|| Router::new(handlers::MODULES));

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
            LoginRequest::Rejected { reason } => return kick(&mut session, reason),
            LoginRequest::Disconnected => return Ok(()),
        };
        let _span = tracing::info_span!("lobby", account = %login.account.name).entered();
        let mut lease = LoginLease::new(self, Arc::clone(&login.account));
        let local_game = session.is_local().then(|| {
            self.platform
                .track_local(login.account.account_lo, &login.account.name)
        });
        services::follow_local_login(self, session.is_local(), &login.account);
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
        let rewards = services::claim_at_login(self, &login.account, wall_time)?;
        let places = services::top500_places(self, &login.account)?;
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
                leaderboard_places: &places,
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
            for message in &rewards {
                session.send(message)?;
            }
        }
        let mailbox = Arc::clone(&lease.mailbox);
        let mut peer = Peer {
            lobby: self,
            login: &login,
            session: &mut session,
            seat: &mut lease.seat,
            wall_time,
        };
        for module in handlers::MODULES {
            if let Some(login) = module.hooks.login {
                login(&mut peer)?;
            }
        }
        let replaced = || lease.replaced.load(Ordering::Acquire);
        // The dashboard's reconnect closes this connection; the game logs in again.
        let current = || !replaced() && !local_game.as_ref().is_some_and(|game| game.dropped());
        while current() {
            for message in mailbox.take() {
                peer.send(&message)?;
            }
            for module in handlers::MODULES {
                if let Some(tick) = module.hooks.tick {
                    tick(&mut peer)?;
                }
            }
            let woke = Instant::now();
            let received = peer.session.receive_message_until(|| {
                current() && !mailbox.has_mail() && woke.elapsed() < TICK
            })?;
            if let Some((group, message, fields)) = received {
                dispatch(&mut peer, group, message, fields);
            } else if peer.session.closed() {
                break;
            }
        }
        if replaced() {
            return kick(&mut session, SIGNED_IN_ELSEWHERE);
        }
        if local_game.as_ref().is_some_and(|game| game.dropped()) {
            tracing::info!("[>>>] Disconnecting for reconnect (dashboard)");
        }
        Ok(())
    }
}

/// Run a client message's handler. As in Python (`session.py` `dispatch`), a
/// message nobody handles is logged, and a failing handler is logged without
/// ending the session; a broken link ends it at the next receive.
fn dispatch(peer: &mut Peer<'_, '_>, group: u32, message: u32, fields: Fields) {
    if TELEMETRY.contains(&group) {
        return;
    }
    let Some(handle) = ROUTER.get(group, message) else {
        let shown: String = format!("{fields:?}")
            .chars()
            .take(LOG_VALUE_LIMIT)
            .collect();
        tracing::info!("[<<<] {group:08X}/{message} {shown}");
        return;
    };
    if let Err(error) = handle(peer, fields) {
        tracing::error!("handler for {group:08X}/{message} failed: {error}");
    }
}

/// Send a disconnect reason and keep the link [`KICK_GRACE`] longer.
fn kick(session: &mut LobbySession<'_>, reason: u64) -> io::Result<()> {
    session.send_disconnect(reason)?;
    std::thread::sleep(KICK_GRACE);
    Ok(())
}

pub(crate) fn serve(
    listener: TcpListener,
    lobby: Arc<Lobby>,
    shutdown: mpsc::Receiver<()>,
) -> io::Result<()> {
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

pub(crate) fn match_id() -> io::Result<[u64; 2]> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(std::array::from_fn(|i| {
        u64::from_le_bytes(random[i * 8..i * 8 + 8].try_into().unwrap()) >> 1
    }))
}
