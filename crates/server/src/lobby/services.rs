//! Loot boxes, the shop, login rewards, the account features sent again after
//! login, the Top 500 places, and the dashboard's live updates of a connected
//! game.
//!
//! The dashboard runs on its own thread; an edit queues a push for the
//! account's session, whose `tick` sends it (at most one `TICK` later), as
//! Python's `push_profile` and `push_settings` (`ow174/lobby/server.py`) sent
//! it from the dashboard's thread.

use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use orpheus_lobby::{
    accounts::{Account, Accounts},
    celebrations::Celebrations,
    clock::server_time,
    collection::Collection,
    economy::Economy,
    leaderboard::{self, Player},
    login::Identity,
    lootbox::LootBoxes,
    player_messages::{self as player, PERMISSIONS},
    ranked::LeaderboardPlaces,
    session::{LoginContent, PreparedLogin},
    shop::Shop,
    templates::LOBBY,
    transport::LobbyMessage,
};

use super::{Lobby, peer::Peer, social};

/// The account features (55500) go again this long after login, each delay
/// counted from the one before: the client drops them on the first login
/// after the game starts, and they gate chat. `MENU_READY_DELAYS`,
/// `ow174/lobby/handlers/login.py` (it sleeps 3 s, then 7 s more).
const MENU_READY_DELAYS: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(7)];

/// What the dashboard asked to send to a connected game.
enum Push {
    /// The refreshed state, with the items the edit gave.
    State(Vec<u64>),
    /// The saved settings (20802) again.
    Settings,
}

/// One logged-in session.
struct Session {
    /// The login's party id, unique to the session: a replaced session must
    /// not take the newer one's pushes.
    key: [u64; 2],
    /// The lobby's clock at login.
    logged_in: Duration,
    features_sent: usize,
    pushes: Vec<Push>,
}

pub(crate) struct Services {
    pub loot: LootBoxes,
    pub shop: Shop,
    sessions: Mutex<HashMap<u32, Session>>,
}

impl Services {
    pub(crate) fn new(collection: &Collection, economy: Arc<Economy>) -> Self {
        Self {
            loot: LootBoxes::new(collection, economy),
            shop: Shop::new(collection),
            sessions: Mutex::default(),
        }
    }

    /// Send an edited account's state to its game, if it is connected;
    /// `granted` are the items the edit gave (`push_profile`).
    pub(crate) fn push_profile(&self, account_lo: u32, granted: Vec<u64>) {
        self.push(account_lo, Push::State(granted));
    }

    /// Send an account's saved settings (20802) to its game again, if it is
    /// connected (`push_settings`).
    pub(crate) fn push_settings(&self, account_lo: u32) {
        self.push(account_lo, Push::Settings);
    }

    fn push(&self, account_lo: u32, push: Push) {
        if let Some(session) = self.sessions().get_mut(&account_lo) {
            session.pushes.push(push);
        }
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Session>> {
        self.sessions.lock().expect("lobby service sessions")
    }

    /// A session finished its login messages, at `now` on the lobby's clock.
    pub(crate) fn logged_in(&self, login: &PreparedLogin, now: Duration) {
        self.sessions().insert(
            login.account.account_lo,
            Session {
                key: login.identity.party_id,
                logged_in: now,
                features_sent: 0,
                pushes: Vec::new(),
            },
        );
    }

    /// The account's last session ended.
    pub(crate) fn logged_out(&self, account: &Account) {
        self.sessions().remove(&account.account_lo);
    }

    /// Whether the account features are due again at `now`, and the
    /// dashboard's pushes.
    fn due(&self, login: &PreparedLogin, now: Duration) -> (bool, Vec<Push>) {
        let mut sessions = self.sessions();
        let Some(session) = sessions
            .get_mut(&login.account.account_lo)
            .filter(|session| session.key == login.identity.party_id)
        else {
            return (false, Vec::new());
        };
        let next: Duration = MENU_READY_DELAYS
            .iter()
            .take(session.features_sent + 1)
            .sum();
        let features = session.features_sent < MENU_READY_DELAYS.len()
            && now.saturating_sub(session.logged_in) >= next;
        if features {
            session.features_sent += 1;
        }
        (features, std::mem::take(&mut session.pushes))
    }
}

/// Every module's timed work for a session: the account features after
/// login, then the dashboard's pushes.
pub(crate) fn tick(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let (features, pushes) = peer.lobby.services.due(peer.login, peer.lobby.clock.now());
    if features {
        peer.send(&(PERMISSIONS, 55500, player::features()))?;
    }
    for push in pushes {
        match push {
            Push::State(granted) => push_state(peer, &granted)?,
            Push::Settings => {
                let settings =
                    player::settings(&peer.lobby.schemas, &peer.login.account.profile())?;
                peer.send(&(LOBBY, 20802, settings))?;
            }
        }
    }
    Ok(())
}

/// Refresh the client after a dashboard edit: card, party, currencies,
/// collection, events. Challenge rewards earned and events switched on are
/// granted at once. The collection screen counts an item only when it
/// arrives as an unlock (24901), so `granted` go out as unlocks too.
/// `push_state`, `ow174/lobby/session.py`.
fn push_state(peer: &mut Peer<'_, '_>, granted: &[u64]) -> io::Result<()> {
    let lobby = peer.lobby;
    let login = peer.login;
    let wall_time = wall_now()?;
    let rewards = login
        .account
        .edit_profile(|profile| {
            let earned = Celebrations::claim_rewards(profile, &lobby.collection);
            let now = server_time(&profile.server_date, wall_time)?;
            // An event switched on in the dashboard greets the player at once.
            // Its box goes out with the others (24302).
            let greeting = lobby.celebrations.greet(profile, &lobby.collection, now)?;
            Ok((!earned.is_empty() || !greeting.messages.is_empty()).then_some((earned, greeting)))
        })?
        .unwrap_or_default();
    let (earned, greeting) = rewards;
    let places = top500_places(lobby, &login.account)?;
    let menu_hero = {
        let profile = login.account.profile();
        lobby
            .social
            .menu_hero(
                login.account.account_lo,
                &profile,
                lobby.collection.heroes(),
                lobby.collection.items(),
            )?
            .menu_guid
    };
    let party = social::party_messages(lobby, &login.account)?;
    let mut messages = login.live_messages(
        &lobby.templates,
        &lobby.collection,
        &lobby.ranked,
        LoginContent {
            menu_hero,
            dev_console: lobby.dev_console,
            extra_preload: &lobby.extra_preload,
            wall_time,
            arcade: &lobby.arcade,
            celebrations: Some(&lobby.celebrations),
            leaderboard_places: &places,
            party: &party,
        },
    )?;
    messages.extend(
        granted
            .iter()
            .chain(&earned)
            .chain(&greeting.given)
            .map(|&guid| lobby.collection.unlock_granted(guid)),
    );
    let greetings = greeting.messages.len();
    messages.extend(greeting.messages);
    let mut sent = 0;
    for message in &messages {
        sent += usize::from(peer.send(message)?);
    }
    let mut summary = format!("[>>>] Live update: {sent} messages");
    if !earned.is_empty() {
        summary += &format!(", {} challenge rewards", earned.len());
    }
    if greetings != 0 {
        summary += &format!(", {greetings} event greetings");
    }
    tracing::info!("{summary}");
    Ok(())
}

/// Rewards granted at login, before the login messages are built: challenge
/// rewards, event greetings with their boxes and gifts, and the partners of
/// team skins bought before pairs came together. Returns what goes after the
/// login messages, in retail's order at the first login of an event: the
/// unlocks, the boxes (24302), then 38901. `login`,
/// `ow174/lobby/handlers/login.py`.
pub(crate) fn claim_at_login(
    lobby: &Lobby,
    account: &Account,
    wall_time: u64,
) -> io::Result<Vec<LobbyMessage>> {
    let collection = &lobby.collection;
    let Some((earned, greeting, paired)) = account.edit_profile(|profile| {
        let earned = Celebrations::claim_rewards(profile, collection);
        let now = server_time(&profile.server_date, wall_time)?;
        let greeting = lobby.celebrations.greet(profile, collection, now)?;
        let paired = Shop::add_missing_pairs(profile, collection);
        let changed = !earned.is_empty() || !greeting.messages.is_empty() || !paired.is_empty();
        Ok(changed.then_some((earned, greeting, paired)))
    })?
    else {
        return Ok(Vec::new());
    };
    let mut messages: Vec<LobbyMessage> = earned
        .iter()
        .chain(&greeting.given)
        .chain(&paired)
        .map(|&guid| collection.unlock_granted(guid))
        .collect();
    if !greeting.boxes.is_empty() {
        messages.push(Collection::boxes_update(&greeting.boxes));
    }
    let items = collection.items();
    for &guid in &earned {
        tracing::info!("[>>>] Challenge reward: {}", items.describe(guid));
    }
    if !greeting.messages.is_empty() {
        tracing::info!(
            "[>>>] Event greeting for {} new event(s)",
            greeting.messages.len()
        );
    }
    for &guid in &greeting.given {
        tracing::info!("[>>>] Event login reward: {}", items.describe(guid));
    }
    for &guid in &paired {
        tracing::info!(
            "[>>>] Partner of a bought team skin: {}",
            items.describe(guid)
        );
    }
    messages.extend(greeting.messages);
    Ok(messages)
}

/// Every saved account, as the leaderboard ranks them
/// (`leaderboard_players`, `ow174/lobby/server.py`).
pub(crate) fn leaderboard_players(accounts: &Accounts) -> io::Result<Vec<Player>> {
    accounts
        .all_saved()?
        .into_iter()
        .map(|name| {
            let account = accounts.get(&name)?;
            let profile = account.profile().clone();
            Ok(Player {
                name: account.name.clone(),
                profile,
                identity: Identity::for_account(account.account_lo),
            })
        })
        .collect()
}

/// The account's Top 500 places among all saved accounts (`_top500_places`).
pub(crate) fn top500_places(lobby: &Lobby, account: &Account) -> io::Result<LeaderboardPlaces> {
    let players = leaderboard_players(&lobby.accounts)?;
    let profile = account.profile().clone();
    leaderboard::places(account.account_lo, &profile, &players)
}

/// A game on this PC that logs in makes the dashboard edit its account,
/// unless it is a second game started on this PC (`server.selected =
/// account`, `ow174/lobby/handlers/login.py`).
pub(crate) fn follow_local_login(lobby: &Lobby, local: bool, account: &Arc<Account>) {
    let second_game = lobby.platform.second_accounts().contains(&account.name);
    if local && !second_game {
        *lobby.selected.lock().expect("selected account") = Arc::clone(account);
    }
}

/// Seconds since 1970.
pub(crate) fn wall_now() -> io::Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use orpheus_content::catalog::ItemDb;
    use orpheus_lobby::templates::RetailTemplates;

    fn services() -> Services {
        let templates = RetailTemplates::from_reader(
            br#"{"format":1,"server":[
                ["70E5A247",24900,{"+0x80":[],"+0x98":[]}],
                ["779A8581",24300,{"+0x78":{"+0x30":[]},"+0xF8":{"+0x0":[]}}]
            ]}"#
            .as_slice(),
        )
        .unwrap();
        Services::new(
            &Collection::new(&templates, Arc::new(ItemDb::default())).unwrap(),
            Arc::default(),
        )
    }

    fn login(accounts: &Accounts, name: &str, key: u64) -> PreparedLogin {
        let account = accounts.get(name).unwrap();
        PreparedLogin {
            identity: Identity {
                party_id: [key, 0],
                ..Identity::for_account(account.account_lo)
            },
            account,
            tournament: false,
        }
    }

    fn pushes(pushes: &[Push]) -> Vec<Option<Vec<u64>>> {
        pushes
            .iter()
            .map(|push| match push {
                Push::State(granted) => Some(granted.clone()),
                Push::Settings => None,
            })
            .collect()
    }

    #[test]
    fn dashboard_pushes_wait_for_the_accounts_own_session() {
        let state = tempfile::tempdir().unwrap();
        let accounts =
            Accounts::new(state.path().join("profiles"), state.path().join("t")).unwrap();
        let services = services();
        let first = login(&accounts, "Alpha", 1);
        let lo = first.account.account_lo;
        services.push_profile(lo, vec![5]);
        services.logged_in(&first, Duration::ZERO);
        assert!(
            services.due(&first, Duration::ZERO).1.is_empty(),
            "offline edits are not kept"
        );

        services.push_profile(lo, vec![7, 8]);
        services.push_settings(lo);
        let replaced = login(&accounts, "Alpha", 2);
        assert!(
            services.due(&replaced, Duration::ZERO).1.is_empty(),
            "another session's pushes"
        );
        let (features, due) = services.due(&first, Duration::ZERO);
        assert!(!features);
        assert_eq!(pushes(&due), [Some(vec![7, 8]), None]);
        assert!(services.due(&first, Duration::ZERO).1.is_empty());

        services.logged_out(&first.account);
        services.push_settings(lo);
        services.logged_in(&first, Duration::ZERO);
        assert!(services.due(&first, Duration::ZERO).1.is_empty());
    }

    /// Login at 0: the features go again at 3 s, then 7 s later (10 s),
    /// and no more (`MENU_READY_DELAYS`).
    #[test]
    fn account_features_go_again_three_then_ten_seconds_after_login() {
        let state = tempfile::tempdir().unwrap();
        let accounts =
            Accounts::new(state.path().join("profiles"), state.path().join("t")).unwrap();
        let services = services();
        let session = login(&accounts, "Alpha", 1);
        services.logged_in(&session, Duration::ZERO);
        let due = |millis| services.due(&session, Duration::from_millis(millis)).0;
        assert!(!due(0));
        assert!(!due(2_999));
        assert!(due(3_000));
        assert!(!due(3_000));
        assert!(!due(9_999));
        assert!(due(10_000));
        assert!(!due(600_000), "only twice");
    }
}
