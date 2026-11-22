//! Ports of `tests/test_queue.py` and `tests/test_leaving.py`: the queue
//! handlers (44100-44107) and the leave handlers (52902, 21801) on a lobby
//! built from the derived game data. What each client receives is read from
//! its session's mailbox; the matches are opened on the game service on its
//! virtual clock (`game_service/lockstep.rs`), and the tests that need a
//! connected game dial it on its in-memory network. The lobby's clock is
//! virtual too and stands: these tests call the sweep themselves.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    net::Ipv4Addr,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use lucy_content::{catalog::ItemDb, maps::GameMap};
use lucy_core::bits::BitReader;
use lucy_lobby::{
    accounts::{Account, Accounts},
    arcade::Arcade,
    celebrations::Celebrations,
    collection::Collection,
    handoff::Handoff,
    login::Identity,
    presence::Presence,
    ranked::Ranked,
    session::PreparedLogin,
    social::{PartyId, Social},
    templates::RetailTemplates,
    transport::LobbyMessage,
};
use lucy_sim::scene_assets::SceneAssets;
use lucy_wire::{
    jam::codec::{
        Fields, Schemas,
        Value::{self, Array, Bool, Signed, Struct, Unsigned},
    },
    link::{ACK, FIN, KIND_FRAME},
    messages::read_channel,
};

use super::{Matchmaker, Room, Seat};
use crate::{
    game_service::{
        GameService, MatchPlayer,
        lockstep::{Client as GameClient, Lockstep, Network, ticks},
    },
    lobby::{
        Lobby,
        clock::VirtualClock,
        hub::{Hub, Mailbox},
        match_id,
        services::Services,
        social::{self, Turn},
    },
    platform::Platform,
};

/// How long [`Fixture::wait_for`] runs the game service at most: the 10 s it
/// waited in real time, in ticks.
const WAIT: Duration = Duration::from_secs(10);

/// The game's 20304: back to the menu (`ow174/game/server.py` LEAVE_GAME).
const LEAVE_GAME: u32 = 20304;

/// The scene assets and the collection, loaded once for every test. The
/// graph bundles load only when a test opens a world (one of them).
static SHARED: LazyLock<(Arc<SceneAssets>, Arc<Collection>)> = LazyLock::new(|| {
    let runtime = lucy_testkit::runtime_dir();
    let data = runtime.join("data");
    let items = Arc::new(ItemDb::load(&data).unwrap());
    let collection = Collection::new(&RetailTemplates::captured(), items).unwrap();
    (
        Arc::new(SceneAssets::load_deferred(&runtime).unwrap()),
        Arc::new(collection),
    )
});

/// A logged-in client: its session's mailbox and everything it received so
/// far, oldest first.
struct Client {
    mailbox: Arc<Mailbox>,
    received: Vec<LobbyMessage>,
}

/// A lobby as the server builds it, with the sessions the test logged in.
pub(super) struct Fixture {
    pub(super) lobby: Arc<Lobby>,
    /// The lobby's clock.
    pub(super) clock: VirtualClock,
    /// The game service's network and clock, when it is on.
    network: Option<Network>,
    clients: RefCell<HashMap<u32, Client>>,
    _state: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::build(true)
    }

    /// The lobby with the game server off (`--game-port 0`: Python's
    /// `LobbyServer.game` stays `None`).
    pub(super) fn without_game() -> Self {
        Self::build(false)
    }

    fn build(with_game: bool) -> Self {
        let runtime = lucy_testkit::runtime_dir();
        let data = runtime.join("data");
        let state = tempfile::tempdir().unwrap();
        let templates = RetailTemplates::captured();
        let (assets, collection) = SHARED.clone();
        let accounts = Accounts::new(
            state.path().join("profiles"),
            state.path().join("profile.json"),
        )
        .unwrap();
        let selected = accounts.get("Alpha").unwrap();
        let (game, network) = if with_game {
            let (service, network) = Lockstep::new(Arc::new(Schemas::game()))
                .unwrap()
                .into_parts();
            (Some(Arc::new(service)), Some(network))
        } else {
            (None, None)
        };
        let clock = VirtualClock::default();
        let economy = Arc::new(lucy_lobby::economy::Economy::load(&data).unwrap());
        let lobby = Arc::new(Lobby {
            signing_key: *lucy_fixed::LOBBY_STATE_BLOB_KEY,
            schemas: Schemas::lobby(),
            presence: Presence::new(&templates).unwrap(),
            ranked: Ranked::new(&templates).unwrap(),
            arcade: Arcade::load(data.join("arcade_cards_174.json"), &templates).unwrap(),
            celebrations: Celebrations::load(&data, &templates, Arc::clone(&economy)).unwrap(),
            xp: lucy_lobby::xp_table::XpTable::load(&data).unwrap(),
            templates,
            services: Arc::new(Services::new(&collection, economy)),
            collection,
            extra_preload: Vec::new(),
            dev_console: false,
            assets,
            first_scene: Mutex::new(None),
            accounts: Arc::new(accounts),
            selected: Arc::new(Mutex::new(selected)),
            active: Arc::default(),
            game,
            hub: Hub::default(),
            platform: Arc::new(Platform::new(Ipv4Addr::LOCALHOST, String::new(), None)),
            social: Social::default(),
            matchmaker: Matchmaker::new(&data, state.path(), 0, None).unwrap(),
            clock: clock.clock(),
        });
        Self {
            lobby,
            clock,
            network,
            clients: RefCell::default(),
            _state: state,
        }
    }

    /// A lobby session of the account, as its login starts one: online in
    /// the hub, the social state and the matchmaker.
    fn log_in(&self, name: &str) -> Arc<Account> {
        let account = self.lobby.accounts.get(name).unwrap();
        let mailbox = self.lobby.hub.join(account.account_lo);
        social::session_started(&self.lobby, &account).unwrap();
        let sequence = u32::try_from(self.clients.borrow().len()).unwrap() + 1;
        self.lobby.matchmaker.login(&PreparedLogin {
            account: Arc::clone(&account),
            identity: Identity::create(account.account_lo, sequence).unwrap(),
            tournament: false,
        });
        self.clients.borrow_mut().insert(
            account.account_lo,
            Client {
                mailbox,
                received: Vec::new(),
            },
        );
        account
    }

    /// Everything the account's client received so far, oldest first.
    fn received(&self, account: &Account) -> Vec<LobbyMessage> {
        let mut clients = self.clients.borrow_mut();
        let client = clients.get_mut(&account.account_lo).expect("logged in");
        client.received.extend(client.mailbox.take());
        client.received.clone()
    }

    fn messages(&self, account: &Account, group: u32, id: u32) -> Vec<Fields> {
        self.received(account)
            .into_iter()
            .filter(|&(crc, number, _)| (crc, number) == (group, id))
            .map(|(_, _, fields)| fields)
            .collect()
    }

    /// A message as the client decodes it.
    fn decoded(&self, group: u32, id: u32, fields: &Fields) -> Fields {
        let body = self.lobby.schemas.encode(group, id, fields).unwrap();
        self.lobby.schemas.decode(group, id, &body, true).unwrap()
    }

    /// The first account's party with the others joined, as `Social.join`
    /// does it (nobody is told).
    fn party(&self, accounts: &[&Arc<Account>]) -> PartyId {
        let mut state = self.lobby.social.lock();
        let id = state.party_of(accounts[0]).unwrap();
        for member in &accounts[1..] {
            state.join(member, id);
        }
        id
    }

    fn members(&self, party: PartyId) -> Vec<String> {
        self.lobby
            .social
            .lock()
            .party(party)
            .map_or_else(Vec::new, |party| {
                party
                    .members
                    .iter()
                    .map(|member| member.name.clone())
                    .collect()
            })
    }

    fn leader(&self, party: PartyId) -> String {
        self.lobby
            .social
            .lock()
            .party(party)
            .expect("the party")
            .leader
            .name
            .clone()
    }

    fn party_id(&self, account: &Account) -> Option<PartyId> {
        self.lobby.social.lock().party_id_of(account.account_lo)
    }

    /// The account's party state (20700 +0x78) as its client decodes it.
    fn party_state(&self, account: &Account) -> Fields {
        let (group, id, fields) = {
            let turn = Turn::new(&self.lobby).unwrap();
            let party = turn
                .state
                .party_id_of(account.account_lo)
                .and_then(|id| turn.state.party(id))
                .expect("in a party");
            turn.content.party_state(party).unwrap()
        };
        structure(&self.decoded(group, id, &fields)[&0x78]).clone()
    }

    /// The party state's queue entries (+0x18).
    fn entries(&self, account: &Account) -> Vec<Value> {
        array(&self.party_state(account)[&0x18]).to_vec()
    }

    fn entry(&self, account: &Account) -> Fields {
        let entries = self.entries(account);
        assert_eq!(entries.len(), 1, "{entries:?}");
        structure(&entries[0]).clone()
    }

    /// The choice (+0xC8) of the party state's member at `index`.
    fn choice(&self, account: &Account, index: usize) -> Fields {
        let state = self.party_state(account);
        let choices = array(&structure(&array(&state[&0x0])[index])[&0xc8]).to_vec();
        assert_eq!(choices.len(), 1, "{choices:?}");
        structure(&choices[0]).clone()
    }

    /// A match of these players opened on the game service, which the
    /// matchmaker knows (Python `GameServer.create_match`).
    fn open_match(&self, players: &[&Arc<Account>]) -> ([u64; 2], Vec<Handoff>) {
        let id = match_id().unwrap();
        let mut map = GameMap::practice_range();
        map.team_sizes = vec![u8::try_from(players.len()).unwrap(), 0];
        let handoffs = self
            .service()
            .open_match(
                id,
                map.clone(),
                players
                    .iter()
                    .map(|player| MatchPlayer {
                        name: player.name.clone(),
                        team: 0,
                        tournament: false,
                        ranked: false,
                    })
                    .collect(),
            )
            .unwrap();
        let seats = players
            .iter()
            .zip(&handoffs)
            .map(|(account, handoff)| Seat {
                account: Arc::clone(account),
                party: self.party_id(account).unwrap_or_default(),
                team: 0,
                role: 0,
                record: Fields::new(),
                handoff: handoff.clone(),
                joined: false,
                joined_at: None,
                gone: false,
                going_home: false,
                excused: false,
                released: false,
            })
            .collect();
        self.lobby.matchmaker.lock().rooms.push(Room {
            id,
            card: 0,
            map,
            ranked: false,
            ai: false,
            seats,
        });
        (id, handoffs)
    }

    /// The players the server took out of their match, in seat order
    /// (Python `FakeGame.taken_out`, without the reasons).
    fn taken_out(&self) -> Vec<String> {
        self.lobby
            .matchmaker
            .lock()
            .rooms
            .iter()
            .flat_map(|room| &room.seats)
            .filter(|seat| seat.going_home)
            .map(|seat| seat.account.name.clone())
            .collect()
    }

    /// Something about the account's seat in a match it has not left.
    fn seat<T>(&self, account: &Account, read: impl FnOnce(&Seat) -> T) -> Option<T> {
        let state = self.lobby.matchmaker.lock();
        state
            .player_of(account.account_lo)
            .map(|(room, seat)| read(&state.rooms[room].seats[seat]))
    }

    fn match_lobbies(&self) -> HashMap<u32, [u64; 2]> {
        self.lobby.matchmaker.lock().match_lobbies.clone()
    }

    /// 52902 {+0x78 lobby id, +0x88 as group, +0x89 waiting request}.
    fn leave(&self, account: &Arc<Account>, as_group: bool, lobby_id: [u64; 2], request: bool) {
        self.lobby
            .matchmaker
            .leave_match_lobby(&self.lobby, account, as_group, lobby_id, request)
            .unwrap();
    }

    /// 21801 {}.
    fn leaving_game(&self, account: &Arc<Account>) {
        self.lobby
            .matchmaker
            .leaving_game(&self.lobby, account)
            .unwrap();
    }

    /// The games that connected to or left their match (the lobby tick's
    /// sweep: Python `LobbyServer.game_joined`, `game_left`).
    fn sweep(&self) {
        self.lobby.matchmaker.with(&self.lobby, |ctx| ctx.sweep());
    }

    /// Sweep after each tick of the game service until the condition holds
    /// (Python `wait_for`), at most [`WAIT`]; the ticks it took.
    fn wait_for(&self, what: &str, done: impl Fn(&Self) -> bool) -> u32 {
        for ran in 0..=ticks(WAIT) {
            self.sweep();
            if done(self) {
                return ran;
            }
            self.network().step();
        }
        panic!("not in {WAIT:?}: {what}");
    }

    fn game(&self, handoff: &Handoff) -> Game {
        Game::connect(self.network(), handoff)
    }

    fn service(&self) -> &GameService {
        self.lobby.game.as_deref().expect("the game server is on")
    }

    fn network(&self) -> &Network {
        self.network.as_ref().expect("the game server is on")
    }
}

/// The client's side of the game link, as far as these tests need it
/// (`tests/test_leaving.py` `PretendGame`).
struct Game {
    client: GameClient,
    schemas: Schemas,
}

impl Game {
    fn connect(network: &Network, handoff: &Handoff) -> Self {
        let mut client = GameClient::on(network, handoff);
        client.handshake();
        Self {
            client,
            schemas: Schemas::game(),
        }
    }

    fn send(&mut self, flags: u8) {
        self.client.send(flags, Vec::new());
    }

    /// The ids of the reliable messages received until `wanted` came, within
    /// 5 s of game time; every new datagram is acknowledged.
    fn reliable_until(&mut self, wanted: u32) -> Vec<u32> {
        let mut seen = Vec::new();
        let deadline = self.client.now() + Duration::from_secs(5);
        loop {
            assert!(
                self.client.now() < deadline,
                "{wanted} not seen in time: {seen:?}"
            );
            let packet = self.client.receive();
            if !self.client.window.accept(packet.seq) {
                continue;
            }
            self.send(ACK);
            if packet.kind != KIND_FRAME {
                continue;
            }
            let mut reader = BitReader::new(&packet.payload);
            reader.var_a().unwrap();
            let channel = read_channel(&mut reader, &self.schemas).unwrap();
            seen.extend(channel.reliable.iter().map(|message| message.message.id));
            if seen.contains(&wanted) {
                return seen;
            }
        }
    }
}

fn structure(value: &Value) -> &Fields {
    let Struct(fields) = value else {
        panic!("expected a struct, got {value:?}")
    };
    fields
}

fn array(value: &Value) -> &[Value] {
    let Array(values) = value else {
        panic!("expected an array, got {value:?}")
    };
    values
}

fn int(value: &Value) -> i128 {
    match *value {
        Unsigned(number) => i128::from(number),
        Signed(number) => i128::from(number),
        _ => panic!("expected a number, got {value:?}"),
    }
}

fn ints(value: &Value) -> Vec<i128> {
    array(value).iter().map(int).collect()
}

fn flag(value: &Value) -> bool {
    match *value {
        Bool(flag) => flag,
        _ => int(value) != 0,
    }
}

/// A value with its integers as one kind: Python has no signedness, and the
/// schemas decode some fields as signed.
fn plain(value: &Value) -> Value {
    match value {
        &Signed(number) if number >= 0 => Unsigned(number as u64),
        Struct(fields) => Struct(
            fields
                .iter()
                .map(|(&offset, value)| (offset, plain(value)))
                .collect(),
        ),
        Array(values) => Array(values.iter().map(plain).collect()),
        other => other.clone(),
    }
}

mod queue {
    use lucy_lobby::{
        passes::{PASSES, Pool},
        queue::{PICKING, QUEUE, QUEUE_WAITS, SEARCHING},
        ranked::{DAMAGE, SUPPORT, TANK},
    };

    use super::*;

    /// The key of a 44100 for the card (`tests/test_queue.py` `request`:
    /// competitive role queue is card 1B3, its open queue 1B4).
    fn key(card: u64) -> Fields {
        let mode = Fields::from([
            (0x0, Unsigned(0x0630_0000_0000_0000 | card)),
            (0x8, Unsigned(0)),
            (0x10, Unsigned(0)),
            (0x18, Unsigned(0)),
        ]);
        Fields::from([
            (0x0, Struct(mode)),
            (0x20, Unsigned(0)),
            (0x24, Unsigned(15_959_616)),
            (0x28, Unsigned(16_025_156)),
        ])
    }

    fn role_queue() -> Fields {
        key(0x1b3)
    }

    fn open_queue() -> Fields {
        key(0x1b4)
    }

    fn enter_queue(fixture: &Fixture, account: &Arc<Account>, key: Fields) {
        fixture
            .lobby
            .matchmaker
            .enter_queue(&fixture.lobby, account, key)
            .unwrap();
    }

    fn set_roles(fixture: &Fixture, account: &Arc<Account>, roles: &[u64]) {
        fixture
            .lobby
            .matchmaker
            .set_roles(&fixture.lobby, account, roles)
            .unwrap();
    }

    fn pass_role(fixture: &Fixture, account: &Arc<Account>, roles: &[u64]) {
        fixture
            .lobby
            .matchmaker
            .pass_role(&fixture.lobby, account, roles)
            .unwrap();
    }

    fn change_roles(fixture: &Fixture, account: &Arc<Account>) {
        fixture
            .lobby
            .matchmaker
            .change_roles(&fixture.lobby, account)
            .unwrap();
    }

    fn cancel_queue(fixture: &Fixture, account: &Arc<Account>, key: Fields) {
        fixture
            .lobby
            .matchmaker
            .cancel_queue(&fixture.lobby, account, key)
            .unwrap();
    }

    fn competitive_passes(account: &Account) -> i64 {
        Pool::Competitive.count(&account.profile()).unwrap()
    }

    fn give_passes(account: &Account, count: i64) {
        account.profile().priority_passes =
            serde_json::Map::from_iter([("competitive".to_owned(), serde_json::json!(count))]);
    }

    /// `tests/test_queue.py::QueueTests::test_a_queue_without_roles_has_no_role_check`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_queue_without_roles_has_no_role_check() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        enter_queue(&fixture, &alpha, open_queue());
        assert_eq!(fixture.messages(&alpha, QUEUE, 44201).len(), 1);
        assert!(fixture.entries(&alpha).is_empty());
    }

    /// A forced Practice Range plays its own mode: one blue team, so two
    /// players who search the open queue apart both sit on team 0 (a red
    /// player would spawn in the blue spawn room and die there).
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_forced_practice_range_seats_everyone_on_the_blue_team() {
        let fixture = Fixture::new();
        let matchmaker = &fixture.lobby.matchmaker;
        matchmaker.set_forced_map(Some(GameMap::practice_range().map_guid));
        matchmaker.set_minimum_players(&fixture.lobby, 2);
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        enter_queue(&fixture, &alpha, open_queue());
        enter_queue(&fixture, &beta, open_queue());
        let state = matchmaker.lock();
        let [room] = &state.rooms[..] else {
            panic!("one match, not {}", state.rooms.len())
        };
        assert_eq!(room.map.map_guid, GameMap::practice_range().map_guid);
        let teams: Vec<u8> = room.seats.iter().map(|seat| seat.team).collect();
        assert_eq!(teams, [0, 0]);
    }

    /// `tests/test_queue.py::QueueTests::test_a_role_queue_starts_with_a_role_check`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_role_queue_starts_with_a_role_check() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let party = fixture.party(&[&alpha]);
        fixture
            .lobby
            .matchmaker
            .lock()
            .queues
            .entry(party)
            .or_default()
            .roles
            .insert(alpha.account_lo, vec![2]);
        enter_queue(&fixture, &alpha, role_queue());
        // The client counts as queued at once; the entry's state holds the
        // search back.
        let joined = fixture.messages(&alpha, QUEUE, 44201);
        assert_eq!(joined.len(), 1);
        assert_eq!(structure(&joined[0][&0x78])[&0x0], Struct(role_queue()));
        let entry = fixture.entry(&alpha);
        assert_eq!(plain(&entry[&0x18]), plain(&Struct(role_queue())));
        assert_eq!(int(&entry[&0x54]), i128::from(PICKING));
        // Priority passes only for damage.
        let roles = array(&structure(&structure(&entry[&0x0])[&0x0])[&0x0]);
        let pass_roles: Vec<i128> = roles
            .iter()
            .map(structure)
            .filter(|role| flag(&role[&0x39]))
            .map(|role| int(&role[&0x0]))
            .collect();
        assert_eq!(pass_roles, [i128::from(DAMAGE)]);
        // The role cards get their waits: the flex card reads role 0, damage
        // has one with a pass.
        let waits = fixture.messages(&alpha, QUEUE_WAITS, 56200);
        assert_eq!(waits.len(), 1);
        let decoded = fixture.decoded(QUEUE_WAITS, 56200, &waits[0]);
        let records: HashMap<i128, &Fields> =
            array(&structure(&structure(&decoded[&0x78])[&0x0])[&0x0])
                .iter()
                .map(structure)
                .map(|role| (int(&role[&0x0]), role))
                .collect();
        assert_eq!(
            records.keys().copied().collect::<HashSet<_>>(),
            HashSet::from([0, DAMAGE, TANK, SUPPORT].map(i128::from))
        );
        let damage = records[&i128::from(DAMAGE)];
        assert!(int(&damage[&0x18]) > int(&damage[&0x20]));
        assert!(int(&damage[&0x20]) > 0);
        assert_eq!(int(&records[&i128::from(TANK)][&0x20]), 0);
        // The one who picked the queue is on the role screen: no banner, not
        // ready yet, and no roles from the last queue on the portrait badge.
        let choice = fixture.choice(&alpha, 0);
        assert!(flag(&choice[&0x68]));
        assert!(!flag(&choice[&0x69]));
        assert!(ints(&choice[&0x8]).is_empty());
    }

    /// `tests/test_queue.py::QueueTests::test_ready_starts_the_search_and_change_role_stops_it`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn ready_starts_the_search_and_change_role_stops_it() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        enter_queue(&fixture, &alpha, role_queue());
        set_roles(&fixture, &alpha, &[2, 1, 2, 9]);
        assert_eq!(ints(&fixture.choice(&alpha, 0)[&0x8]), [2, 1]);
        assert!(flag(&fixture.choice(&alpha, 0)[&0x69]));
        assert_eq!(int(&fixture.entry(&alpha)[&0x54]), i128::from(SEARCHING));
        // The same key again: the client's search timer starts over.
        assert_eq!(fixture.messages(&alpha, QUEUE, 44201).len(), 2);
        change_roles(&fixture, &alpha);
        assert!(!flag(&fixture.choice(&alpha, 0)[&0x69]));
        assert_eq!(int(&fixture.entry(&alpha)[&0x54]), i128::from(PICKING));
        set_roles(&fixture, &alpha, &[3]);
        assert_eq!(int(&fixture.entry(&alpha)[&0x54]), i128::from(SEARCHING));
    }

    /// `tests/test_queue.py::QueueTests::test_a_group_waits_for_everyone`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_group_waits_for_everyone() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        fixture.party(&[&alpha, &beta]);
        enter_queue(&fixture, &alpha, role_queue());
        assert_eq!(fixture.messages(&beta, QUEUE, 44201).len(), 1);
        // Beta gets the banner.
        assert!(!flag(&fixture.choice(&alpha, 1)[&0x68]));
        set_roles(&fixture, &alpha, &[1]);
        assert_eq!(int(&fixture.entry(&alpha)[&0x54]), i128::from(PICKING));
        fixture
            .lobby
            .matchmaker
            .accept(&fixture.lobby, &beta)
            .unwrap();
        assert!(flag(&fixture.choice(&alpha, 1)[&0x68]));
        set_roles(&fixture, &beta, &[3]);
        assert_eq!(int(&fixture.entry(&alpha)[&0x54]), i128::from(SEARCHING));
        // Change Role in a group is followed by an empty list.
        change_roles(&fixture, &beta);
        set_roles(&fixture, &beta, &[]);
        assert!(ints(&fixture.choice(&alpha, 1)[&0x8]).is_empty());
        assert_eq!(int(&fixture.entry(&alpha)[&0x54]), i128::from(PICKING));
    }

    /// `tests/test_queue.py::QueueTests::test_decline_takes_the_party_out_of_the_queue`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn decline_takes_the_party_out_of_the_queue() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        fixture.party(&[&alpha, &beta]);
        enter_queue(&fixture, &alpha, role_queue());
        fixture
            .lobby
            .matchmaker
            .decline(&fixture.lobby, &beta)
            .unwrap();
        assert!(fixture.entries(&alpha).is_empty());
        for account in [&alpha, &beta] {
            let left = fixture.messages(account, QUEUE, 44202);
            assert_eq!(
                left,
                [Fields::from([
                    (0x78, Struct(role_queue())),
                    (0xa8, Unsigned(0))
                ])],
                "{}",
                account.name
            );
            fixture
                .lobby
                .schemas
                .encode(QUEUE, 44202, &left[0])
                .unwrap();
        }
    }

    /// `tests/test_queue.py::QueueTests::test_a_priority_pass_is_held_by_the_search_and_given_back`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_priority_pass_is_held_by_the_search_and_given_back() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        give_passes(&alpha, 2);
        enter_queue(&fixture, &alpha, role_queue());
        assert_eq!(int(&fixture.choice(&alpha, 0)[&0x0]), 2);
        // Ready: the roles, then the pass role. A solo search already started
        // with the roles.
        set_roles(&fixture, &alpha, &[1]);
        pass_role(&fixture, &alpha, &[1]);
        assert_eq!(ints(&fixture.choice(&alpha, 0)[&0x38]), [1]);
        assert_eq!(competitive_passes(&alpha), 1);
        let counts = fixture.messages(&alpha, PASSES, 58501);
        assert_eq!(counts.len(), 1);
        let pools = array(&structure(&counts[0][&0x78])[&0x0]);
        assert!(
            pools.contains(&Struct(Fields::from([
                (0x0, Unsigned(Pool::Competitive as u64)),
                (0x8, Signed(1)),
            ]))),
            "{pools:?}"
        );
        fixture
            .lobby
            .schemas
            .encode(PASSES, 58501, &counts[0])
            .unwrap();
        cancel_queue(&fixture, &alpha, role_queue());
        assert_eq!(competitive_passes(&alpha), 2);
    }

    /// `tests/test_queue.py::QueueTests::test_no_pass_without_passes_or_for_a_role_not_chosen`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn no_pass_without_passes_or_for_a_role_not_chosen() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        enter_queue(&fixture, &alpha, role_queue());
        set_roles(&fixture, &alpha, &[1]);
        pass_role(&fixture, &alpha, &[1]);
        assert!(ints(&fixture.choice(&alpha, 0)[&0x38]).is_empty());
        give_passes(&alpha, 3);
        change_roles(&fixture, &alpha);
        set_roles(&fixture, &alpha, &[1]);
        pass_role(&fixture, &alpha, &[2]);
        assert!(ints(&fixture.choice(&alpha, 0)[&0x38]).is_empty());
        // Tank takes no passes, even when chosen.
        change_roles(&fixture, &alpha);
        set_roles(&fixture, &alpha, &[2]);
        pass_role(&fixture, &alpha, &[2]);
        assert!(ints(&fixture.choice(&alpha, 0)[&0x38]).is_empty());
        assert_eq!(competitive_passes(&alpha), 3);
    }

    /// `tests/test_queue.py::QueueTests::test_cancel_takes_the_others_out_too`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn cancel_takes_the_others_out_too() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        fixture.party(&[&alpha, &beta]);
        enter_queue(&fixture, &alpha, role_queue());
        cancel_queue(&fixture, &alpha, role_queue());
        assert!(fixture.entries(&alpha).is_empty());
        // Its client dropped the entry itself.
        assert!(fixture.messages(&alpha, QUEUE, 44202).is_empty());
        assert_eq!(fixture.messages(&beta, QUEUE, 44202).len(), 1);
    }
}

mod leaving {
    use lucy_lobby::{
        lineup::{LEAVE_MATCH_LOBBY, LEAVING_GAME, MATCH_LOBBY, MATCH_LOBBY_OUT},
        login::OUT_CONNECT,
        party::PARTY,
    };

    use super::*;
    use crate::lobby::ROUTER;

    /// A lobby id no match has (`tests/test_leaving.py` `fake_match(1)`).
    const NO_MATCH: [u64; 2] = [1, 101];

    /// How long after the release a game that stays after 20304 is dropped:
    /// the leave grace (`connection.rs` `LEAVE_GRACE`, 3 s) runs from the
    /// release, which the service takes at its last tick's time, and the
    /// first tick at least 3 s later drops the game. Ticks are 16 ms apart
    /// (`TICK_PERIOD`): the 187th comes at 2.992 s, the 188th at 3.008 s.
    const DROPPED_AFTER: Duration = Duration::from_millis(188 * 16);

    /// The party states (20700) each account received.
    fn party_states(fixture: &Fixture, accounts: &[&Arc<Account>]) -> Vec<usize> {
        accounts
            .iter()
            .map(|account| fixture.messages(account, PARTY, 20700).len())
            .collect()
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_the_handlers_are_registered`
    #[test]
    fn the_handlers_are_registered() {
        assert!(ROUTER.get(MATCH_LOBBY_OUT, LEAVE_MATCH_LOBBY).is_some());
        assert!(ROUTER.get(OUT_CONNECT, LEAVING_GAME).is_some());
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_the_leader_takes_the_party_that_is_in_his_match_and_it_stays_a_party`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn the_leader_takes_the_party_that_is_in_his_match_and_it_stays_a_party() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let gamma = fixture.log_in("Gamma");
        let party = fixture.party(&[&alpha, &beta, &gamma]);
        let (match_id, _) = fixture.open_match(&[&alpha, &beta]);
        // Another match: stays.
        fixture.open_match(&[&gamma]);
        fixture.leave(&alpha, true, match_id, false);
        assert_eq!(fixture.taken_out(), ["Alpha", "Beta"]);
        assert_eq!(fixture.members(party), ["Alpha", "Beta", "Gamma"]);
        assert_eq!(fixture.leader(party), "Alpha");
        assert_eq!(party_states(&fixture, &[&alpha, &beta, &gamma]), [0, 0, 0]);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_a_member_not_in_a_match_stays_in_the_menu_and_in_the_party`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_member_not_in_a_match_stays_in_the_menu_and_in_the_party() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        let (match_id, _) = fixture.open_match(&[&alpha]);
        fixture.leave(&alpha, true, match_id, false);
        assert_eq!(fixture.taken_out(), ["Alpha"]);
        assert_eq!(fixture.members(party).len(), 2);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_as_group_from_a_member_who_does_not_lead_leaves_alone`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn as_group_from_a_member_who_does_not_lead_leaves_alone() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        let (match_id, _) = fixture.open_match(&[&alpha, &beta]);
        fixture.leave(&beta, true, match_id, false);
        assert_eq!(fixture.taken_out(), ["Beta"]);
        assert_eq!(fixture.members(party), ["Alpha"]);
        // The party is told once.
        assert_eq!(party_states(&fixture, &[&alpha]), [1]);
        assert_ne!(fixture.party_id(&beta), Some(party));
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_a_member_who_leaves_alone_leaves_the_match_and_the_party`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_member_who_leaves_alone_leaves_the_match_and_the_party() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let gamma = fixture.log_in("Gamma");
        let party = fixture.party(&[&alpha, &beta, &gamma]);
        let (match_id, _) = fixture.open_match(&[&alpha, &beta, &gamma]);
        fixture.leave(&beta, false, match_id, false);
        assert_eq!(fixture.taken_out(), ["Beta"]);
        assert_eq!(fixture.members(party), ["Alpha", "Gamma"]);
        assert_eq!(party_states(&fixture, &[&alpha, &gamma]), [1, 1]);
        let received = fixture.received(&beta);
        let (crc, id, state) = received.last().expect("Beta's party state");
        assert_eq!((*crc, *id), (PARTY, 20700));
        // A party of one again.
        assert_eq!(array(&structure(&state[&0x78])[&0x0]).len(), 1);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_the_leader_who_leaves_alone_hands_the_party_on`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn the_leader_who_leaves_alone_hands_the_party_on() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        let (match_id, _) = fixture.open_match(&[&alpha, &beta]);
        fixture.leave(&alpha, false, match_id, false);
        assert_eq!(fixture.taken_out(), ["Alpha"]);
        assert_eq!(fixture.members(party), ["Beta"]);
        assert_eq!(fixture.leader(party), "Beta");
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_a_player_alone_just_leaves_the_match`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_player_alone_just_leaves_the_match() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let (match_id, _) = fixture.open_match(&[&alpha]);
        fixture.leave(&alpha, false, match_id, false);
        // What the client sends next on "Leave game and queue" when alone.
        fixture.leave(&alpha, true, match_id, false);
        assert_eq!(fixture.taken_out(), ["Alpha"]);
        assert_eq!(party_states(&fixture, &[&alpha]), [0]);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_52902_without_a_match_does_nothing`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn leave_without_a_match_does_nothing() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        fixture.leave(&alpha, false, NO_MATCH, false);
        assert!(fixture.taken_out().is_empty());
        assert_eq!(fixture.members(party).len(), 2);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_52902_that_cancels_a_waiting_request_leaves_nothing`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn leave_that_cancels_a_waiting_request_leaves_nothing() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        let (match_id, _) = fixture.open_match(&[&alpha, &beta]);
        fixture.leave(&alpha, true, match_id, true);
        assert!(fixture.taken_out().is_empty());
        assert_eq!(fixture.members(party).len(), 2);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_21801_in_a_match_leaves_the_party`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn leaving_the_game_in_a_match_leaves_the_party() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        fixture.open_match(&[&beta]);
        fixture.leaving_game(&beta);
        assert_eq!(fixture.members(party), ["Alpha"]);
        // The game leaves by itself.
        assert!(fixture.taken_out().is_empty());
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_21801_after_the_server_sent_the_game_home_keeps_the_party`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn leaving_the_game_after_the_server_sent_it_home_keeps_the_party() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        let (match_id, _) = fixture.open_match(&[&alpha, &beta]);
        fixture.leave(&alpha, true, match_id, false);
        fixture.leaving_game(&beta);
        fixture.leaving_game(&alpha);
        assert_eq!(fixture.members(party).len(), 2);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_21801_outside_a_match_keeps_the_party`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn leaving_the_game_outside_a_match_keeps_the_party() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        fixture.leaving_game(&beta);
        assert_eq!(fixture.members(party).len(), 2);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_a_game_gets_the_match_lobby_and_loses_it_when_it_leaves`,
    /// with `TakeOutTests::test_a_connected_game_is_reported_and_starts_with_20300`.
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_game_gets_the_match_lobby_and_loses_it_when_it_leaves() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let (match_id, handoffs) = fixture.open_match(&[&alpha]);
        let mut game = fixture.game(&handoffs[0]);
        assert_eq!(game.reliable_until(20300)[0], 20300);
        fixture.wait_for("the game joined", |fixture| {
            fixture.seat(&alpha, |seat| seat.joined) == Some(true)
        });
        let lobbies = fixture.messages(&alpha, MATCH_LOBBY, 53000);
        assert_eq!(lobbies.len(), 1);
        let decoded = fixture.decoded(MATCH_LOBBY, 53000, &lobbies[0]);
        let lobby = structure(&decoded[&0x78]);
        // The lobby id is the match id.
        assert_eq!(
            ints(&structure(&lobby[&0x30])[&0x0]),
            match_id.map(i128::from)
        );
        // `ow174/lobby/handlers/leaving.py` LOBBY_TYPE.
        assert_eq!(int(&lobby[&0x60]), 0);
        assert_eq!(
            fixture.match_lobbies(),
            HashMap::from([(alpha.account_lo, match_id)])
        );
        game.send(FIN);
        fixture.wait_for("the game left", |fixture| {
            fixture.seat(&alpha, |_| ()).is_none()
        });
        let left = fixture.messages(&alpha, MATCH_LOBBY, 53003);
        assert_eq!(left.len(), 1);
        let left = &left[0];
        assert_eq!(
            ints(&structure(&left[&0x78])[&0x0]),
            match_id.map(i128::from)
        );
        assert_eq!((int(&left[&0x88]), int(&left[&0xa0])), (0, 0));
        assert!(fixture.match_lobbies().is_empty());
    }

    /// Alpha and Beta in a party and in one match, both games connected and
    /// in the match lobby.
    fn connected_party(
        fixture: &Fixture,
    ) -> (Arc<Account>, Arc<Account>, PartyId, [u64; 2], Vec<Game>) {
        let alpha = fixture.log_in("Alpha");
        let beta = fixture.log_in("Beta");
        let party = fixture.party(&[&alpha, &beta]);
        let (match_id, handoffs) = fixture.open_match(&[&alpha, &beta]);
        let mut games: Vec<Game> = handoffs
            .iter()
            .map(|handoff| fixture.game(handoff))
            .collect();
        for game in &mut games {
            game.reliable_until(20300);
        }
        fixture.wait_for("both games joined", |fixture| {
            [&alpha, &beta]
                .iter()
                .all(|account| fixture.seat(account, |seat| seat.joined) == Some(true))
        });
        (alpha, beta, party, match_id, games)
    }

    /// `tests/test_leaving.py::GroupLeaveTests::test_both_games_of_the_party_go_home_and_leave_the_match`,
    /// with `TakeOutTests::test_a_game_taken_out_gets_20304_and_leaves_with_its_fin`.
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn both_games_of_the_party_go_home_and_leave_the_match() {
        let fixture = Fixture::new();
        let (alpha, beta, party, match_id, mut games) = connected_party(&fixture);
        fixture.leave(&alpha, true, match_id, false);
        for game in &mut games {
            assert!(game.reliable_until(LEAVE_GAME).contains(&LEAVE_GAME));
        }
        // In the match until its link closes.
        fixture.sweep();
        for account in [&alpha, &beta] {
            assert_eq!(fixture.seat(account, |seat| seat.going_home), Some(true));
        }
        for game in &mut games {
            game.send(FIN);
        }
        fixture.wait_for("both games left", |fixture| {
            [&alpha, &beta]
                .iter()
                .all(|account| fixture.seat(account, |_| ()).is_none())
        });
        // The match ends with its last player.
        fixture.wait_for("the match ended", |fixture| {
            fixture.lobby.matchmaker.lock().rooms.is_empty()
                && fixture.service().snapshot().unwrap().is_empty()
        });
        assert_eq!(fixture.members(party), ["Alpha", "Beta"]);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_a_client_that_sent_52902_reset_its_lobby_itself`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_client_that_sent_52902_reset_its_lobby_itself() {
        let fixture = Fixture::new();
        let (alpha, beta, party, match_id, mut games) = connected_party(&fixture);
        fixture.leave(&alpha, true, match_id, false);
        for game in &mut games {
            game.reliable_until(LEAVE_GAME);
            game.send(FIN);
        }
        fixture.wait_for("both games left", |fixture| {
            [&alpha, &beta]
                .iter()
                .all(|account| fixture.seat(account, |_| ()).is_none())
        });
        assert!(fixture.messages(&alpha, MATCH_LOBBY, 53003).is_empty());
        assert_eq!(fixture.messages(&beta, MATCH_LOBBY, 53003).len(), 1);
        assert_eq!(fixture.members(party).len(), 2);
    }

    /// `tests/test_leaving.py::LeaveHandlerTests::test_a_player_who_is_offline_when_he_leaves_only_loses_the_record`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_player_who_is_offline_when_he_leaves_only_loses_the_record() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let (_, handoffs) = fixture.open_match(&[&alpha]);
        let mut game = fixture.game(&handoffs[0]);
        game.reliable_until(20300);
        fixture.wait_for("the game joined", |fixture| {
            fixture.seat(&alpha, |seat| seat.joined) == Some(true)
        });
        assert_eq!(fixture.match_lobbies().len(), 1);
        // The lobby session ends (its logout hooks).
        fixture.lobby.matchmaker.logout(&fixture.lobby, &alpha);
        social::session_ended(&fixture.lobby, &alpha).unwrap();
        game.send(FIN);
        fixture.wait_for("the game left", |fixture| {
            fixture.seat(&alpha, |_| ()).is_none()
        });
        assert!(fixture.match_lobbies().is_empty());
    }

    /// `tests/test_leaving.py::TakeOutTests::test_a_game_that_stays_after_20304_is_dropped`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_game_that_stays_after_20304_is_dropped() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let (match_id, handoffs) = fixture.open_match(&[&alpha]);
        let mut game = fixture.game(&handoffs[0]);
        game.reliable_until(20300);
        fixture.wait_for("the game joined", |fixture| {
            fixture.seat(&alpha, |seat| seat.joined) == Some(true)
        });
        let released = fixture.network().now();
        fixture.leave(&alpha, false, match_id, false);
        game.reliable_until(LEAVE_GAME);
        // The game sends nothing more: the leave grace drops it.
        fixture.wait_for("the game was dropped", |fixture| {
            fixture.seat(&alpha, |_| ()).is_none()
        });
        assert_eq!(fixture.network().now() - released, DROPPED_AFTER);
        // The match ends with its last player.
        fixture.wait_for("the match ended", |fixture| {
            fixture.lobby.matchmaker.lock().rooms.is_empty()
                && fixture.service().snapshot().unwrap().is_empty()
        });
    }

    /// `tests/test_leaving.py::TakeOutTests::test_send_home_also_drops_a_game_that_stays`
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn send_home_also_drops_a_game_that_stays() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let (_, handoffs) = fixture.open_match(&[&alpha]);
        let mut game = fixture.game(&handoffs[0]);
        game.reliable_until(20300);
        fixture.wait_for("the game joined", |fixture| {
            fixture.seat(&alpha, |seat| seat.joined) == Some(true)
        });
        let released = fixture.network().now();
        assert_eq!(
            fixture
                .lobby
                .matchmaker
                .send_home(&fixture.lobby, Some(alpha.account_lo)),
            1
        );
        assert_eq!(fixture.seat(&alpha, |seat| seat.going_home), Some(true));
        game.reliable_until(LEAVE_GAME);
        fixture.wait_for("the game was dropped", |fixture| {
            fixture.seat(&alpha, |_| ()).is_none()
        });
        assert_eq!(fixture.network().now() - released, DROPPED_AFTER);
    }

    /// `tests/test_leaving.py::TakeOutTests::test_a_player_taken_out_before_his_game_connects_leaves_at_once_and_only_gets_20304`,
    /// through the leave handler and the lobby's sweep.
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn a_player_taken_out_before_his_game_connects_leaves_at_once_and_only_gets_20304() {
        let fixture = Fixture::new();
        let alpha = fixture.log_in("Alpha");
        let (match_id, handoffs) = fixture.open_match(&[&alpha]);
        fixture.leave(&alpha, false, match_id, false);
        assert_eq!(fixture.taken_out(), ["Alpha"]);
        // He leaves at once: the next sweep finds no seat, and the match is over.
        fixture.sweep();
        assert!(fixture.seat(&alpha, |_| ()).is_none());
        assert!(fixture.lobby.matchmaker.lock().rooms.is_empty());
        assert!(fixture.service().snapshot().unwrap().is_empty());
        // His game still connects, and gets only 20304.
        let mut game = fixture.game(&handoffs[0]);
        assert_eq!(game.reliable_until(LEAVE_GAME), [LEAVE_GAME]);
        fixture.sweep();
        assert!(fixture.messages(&alpha, MATCH_LOBBY, 53000).is_empty());
        assert!(fixture.match_lobbies().is_empty());
    }
}

mod game_server_off {
    use lucy_lobby::lineup::MATCH_LOBBY;

    use super::*;

    /// With `--game-port 0` the lobby has no game server: Training > Practice
    /// Range starts nothing and is not kept for later (Python
    /// `Matchmaker.practice`, `_start`), and `.leave` or the dashboard's "End
    /// matches" tell no game (`GameServer.send_home`).
    #[test]
    #[ignore = "requires a derived 104319 runtime"]
    fn no_match_starts_and_no_game_is_told() {
        let fixture = Fixture::without_game();
        let alpha = fixture.log_in("Alpha");
        let session = fixture.lobby.matchmaker.lock().online[&alpha.account_lo]
            .identity
            .session;
        fixture
            .lobby
            .matchmaker
            .practice(&fixture.lobby, &alpha, session)
            .unwrap();
        {
            let state = fixture.lobby.matchmaker.lock();
            assert!(state.rooms.is_empty());
            assert!(!state.deferred_practice.contains_key(&alpha.account_lo));
        }
        assert!(fixture.messages(&alpha, MATCH_LOBBY, 53000).is_empty());
        let matchmaker = &fixture.lobby.matchmaker;
        assert_eq!(
            matchmaker.send_home(&fixture.lobby, Some(alpha.account_lo)),
            0
        );
        assert_eq!(matchmaker.send_home(&fixture.lobby, None), 0);
    }
}
