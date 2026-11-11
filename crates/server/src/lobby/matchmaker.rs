//! The running matchmaker: parties that search a queue are put into matches,
//! and their games go to the game service. Ports `ow174/lobby/matchmaker.py`
//! (`Matchmaker`), the role queue state Python keeps on its `Party`
//! (`ow174/services/social.py`), and the queue handlers of
//! `ow174/lobby/handlers/matchmaking.py` and `handlers/leaving.py`.
//!
//! What the client needs:
//! - 44202 {queue key, reason} drops the client's queue entry and stops the
//!   search timer. Every reason does that; reason 1 (QueuePopped) only differs
//!   in telemetry (0x7FF7896190D0). It goes first, or the menu keeps showing
//!   the search.
//! - "Game Found!" is the client's play state 4 (0x7FF789A9D590, text 046B.07C
//!   via 0x7FF78985BC70). A queue match gets it before the game: 53000 puts the
//!   client in the match lobby of its popped queue (type 4, it answers 52903),
//!   and 53003 with reason 4 takes it out again and sets play state 4 for 5 s
//!   (0x7FF789693120). The client waits for nothing: 20600 follows after
//!   `FOUND_DELAY`, inside those 5 s. It cannot cancel then either: without a
//!   queue entry it sends no 44102 (0x7FF789618950).
//! - A game that is still in a match when it gets 20600 (the Practice Range
//!   while searching) closes the old link and dials the new match through the
//!   old world's link; the old link's close then deletes that world, the new
//!   link with it. So such a game goes to the menu first (20304, at the pop)
//!   and gets its 20600 after "Game Found!", like a game that searched from the
//!   menu.
//!
//! Each session delivers its own 20600 from its tick, so the seat it holds is
//! the one its logout releases. Messages to other sessions go through the hub;
//! party work goes through [`Parties`] once the state lock is released, since
//! the social state reads the queue fields back from here.

mod game;
pub(crate) mod parties;
#[cfg(test)]
mod ported_tests;
mod results;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use orpheus_content::maps::{GameMap, MapCatalog, MapListing};
use orpheus_lobby::{
    accounts::Account,
    handoff::{
        self, FOUND_DELAY, Handoff, LobbySeat, general_channel, match_channel, queue_popped,
        team_colours,
    },
    lineup,
    login::Identity,
    matchmaker::{self as rules, ModeSettings, OpenMatch, QUICK_PLAY, QueueRules, Seekers},
    passes::{self, Pool},
    queue::{self, PICKING, QueueFields, SEARCHING},
    ranked::{LeaderboardPlaces, Queue},
    session::PreparedLogin,
    transport::LobbyMessage,
};
use orpheus_wire::jam::codec::{Fields, Value};

use super::{Lobby, TICK, match_id, peer::Peer};
use crate::game_service::{GameService, Outcome, WorldPlayer};
use orpheus_sim::ai::Difficulty;
pub(crate) use parties::{Parties, Party};

pub(crate) struct Matchmaker {
    rules: HashMap<u64, QueueRules>,
    maps: MapCatalog,
    /// Where the dashboard's settings per card are kept (Python
    /// `paths.matchmaking`).
    modes_path: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Cards without a number of their own: 0 = full teams.
    minimum_players: usize,
    /// AI heroes of this level fill the missing players of a match that
    /// starts with fewer (`--test-ai`).
    test_ai: Option<Difficulty>,
    /// The dashboard's map for every queue; `None` = the card's pick.
    forced_map: Option<u64>,
    modes: HashMap<u64, ModeSettings>,
    hostable: HashMap<u64, bool>,
    tickets: Vec<Ticket>,
    /// The role queue of each party, by party id.
    queues: HashMap<[u64; 2], PartyQueue>,
    /// Players whose game goes to its match once the deadline passed.
    found: Vec<Found>,
    rooms: Vec<Room>,
    /// The match lobby each client has (Python `server.match_lobbies`).
    match_lobbies: HashMap<u32, [u64; 2]>,
    /// Practice Range requests the game service could not take yet, by
    /// account: the lobby session that asked.
    deferred_practice: HashMap<u32, [u64; 2]>,
    online: HashMap<u32, Online>,
    /// The game clock refused a match: retry what waits until it opens again
    /// (Python `clock_ready`).
    clock_blocked: bool,
    last_sweep: Option<Instant>,
}

struct Online {
    account: Arc<Account>,
    identity: Identity,
    tournament: bool,
}

/// What Python keeps on a party for its role queue.
#[derive(Default)]
struct PartyQueue {
    /// The key of the role queue the party is in.
    key: Option<Fields>,
    state: u64,
    /// The role numbers each member chose; they stay after the search.
    roles: HashMap<u32, Vec<u8>>,
    /// Members on the role screen.
    accepted: HashSet<u32>,
    /// Members who pressed Ready.
    ready: HashSet<u32>,
    /// The role a member spends a priority pass on.
    pass_roles: HashMap<u32, u8>,
    /// Members whose pass this search holds.
    passes_taken: HashSet<u32>,
}

struct Ticket {
    party: Party,
    /// The queue key of the party's 44100.
    key: Fields,
    card: u64,
    /// The members who play and the roles they picked, best first.
    members: Vec<(Arc<Account>, Vec<u8>)>,
}

/// A match the matchmaker opened (Python `game.server.Match`, as far as the
/// lobby reads it).
struct Room {
    id: [u64; 2],
    card: u64,
    map: GameMap,
    ranked: bool,
    /// AI heroes fill its teams (a Practice vs AI card, or `--test-ai`):
    /// nobody joins it.
    ai: bool,
    seats: Vec<Seat>,
}

struct Seat {
    account: Arc<Account>,
    party: [u64; 2],
    team: u8,
    role: u8,
    /// The player's card as the lobby shows it, for the VERSUS roster.
    record: Fields,
    handoff: Handoff,
    joined: bool,
    gone: bool,
    /// The server took the player out (Python `GameServer.going_home`).
    going_home: bool,
    /// The 20600 will not be used; the game service drops the seat.
    released: bool,
}

/// A player whose 20600 waits: after "Game Found!", or at once for the
/// Practice Range.
struct Found {
    account_lo: u32,
    session: [u64; 2],
    match_id: [u64; 2],
    handoff: Handoff,
    map_name: String,
    team: u8,
    teams: Vec<u8>,
    deadline: Instant,
}

/// A player going into a match.
struct Entry {
    account: Arc<Account>,
    session: [u64; 2],
    tournament: bool,
    party: Party,
    key: Fields,
    team: u8,
    role: u8,
}

/// Work done once the state lock is released, in order.
enum Effect {
    Post(u32, LobbyMessage),
    Notify(Party, Option<QueueFields>),
    PassesChanged(Arc<Account>),
    GameLeft(u32, [u64; 2]),
    /// A finished match into the player's profile, saved.
    Recorded(Arc<Account>, Queue, Outcome),
}

/// The card of a queue key (44100 +0x78): its +0x0 +0x0.
fn card_of(key: &Fields) -> u64 {
    match key.get(&0) {
        Some(Value::Struct(mode)) => match mode.get(&0) {
            Some(&Value::Unsigned(card)) => card,
            Some(&Value::Signed(card)) => card as u64,
            _ => 0,
        },
        _ => 0,
    }
}

fn random_index(count: usize) -> usize {
    let mut bytes = [0; 8];
    let _ = getrandom::fill(&mut bytes);
    (u64::from_le_bytes(bytes) % count as u64) as usize
}

fn pass_count(pool: Option<Pool>, account: &Account) -> i64 {
    pool.and_then(|pool| pool.count(&account.profile()).ok())
        .unwrap_or(0)
}

impl Matchmaker {
    /// The queue rules and maps from the runtime `data` folder, the settings
    /// saved in the `state` folder, the players who start a match of a card
    /// without a number of its own (`--test-players`; 0 = full teams), and
    /// the level of the AI heroes that fill such a match (`--test-ai`).
    pub(crate) fn new(
        data: &Path,
        state: &Path,
        minimum_players: usize,
        test_ai: Option<Difficulty>,
    ) -> io::Result<Self> {
        let modes_path = state.join("matchmaking.json");
        let modes = rules::load_modes(&modes_path);
        Ok(Self {
            rules: rules::load_rules(&data.join("game_queues_174.json"))?,
            maps: MapCatalog::load(data)?,
            modes_path,
            state: Mutex::new(State {
                minimum_players,
                test_ai,
                modes,
                ..State::default()
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("matchmaker state")
    }

    fn with<T>(&self, lobby: &Lobby, run: impl FnOnce(&mut Ctx<'_>) -> T) -> T {
        let mut ctx = Ctx {
            lobby,
            mm: self,
            state: self.lock(),
            effects: Vec::new(),
        };
        let result = run(&mut ctx);
        let Ctx { state, effects, .. } = ctx;
        drop(state);
        perform(lobby, effects);
        result
    }

    /// The card's rules; a card the data does not know plays like Quick
    /// Play.
    fn rules_of(&self, card: u64) -> &QueueRules {
        static DEFAULT: std::sync::LazyLock<QueueRules> =
            std::sync::LazyLock::new(QueueRules::default);
        self.rules
            .get(&card)
            .or_else(|| self.rules.get(&QUICK_PLAY))
            .unwrap_or(&DEFAULT)
    }

    /// The party's role queue as its party state shows it, while it is in
    /// one (Python `social.party_state`).
    pub(crate) fn queue_fields(&self, lobby: &Lobby, party: &Party) -> Option<QueueFields> {
        self.lock().queue_fields(lobby, party)
    }

    // --- sessions -----------------------------------------------------------

    pub(crate) fn login(&self, login: &PreparedLogin) {
        self.lock().online.insert(
            login.account.account_lo,
            Online {
                account: Arc::clone(&login.account),
                identity: login.identity,
                tournament: login.tournament,
            },
        );
    }

    /// The session ended: its party leaves the queue, and a seat waiting for
    /// it is freed (Python `session._cleanup`, `Matchmaker.cancel`).
    pub(crate) fn logout(&self, lobby: &Lobby, account: &Arc<Account>) {
        self.with(lobby, |ctx| {
            ctx.state.online.remove(&account.account_lo);
            let party = ctx
                .state
                .tickets
                .iter()
                .find(|ticket| {
                    ticket
                        .party
                        .members
                        .iter()
                        .any(|member| member.account_lo == account.account_lo)
                })
                .map(|ticket| ticket.party.clone());
            match party {
                Some(party) => ctx.cancel(&party),
                None => ctx.cancel(&Party {
                    id: [0; 2],
                    leader: account.account_lo,
                    members: vec![Arc::clone(account)],
                }),
            }
        });
    }

    /// Each wake of a session: its 20600 when it is due, and the matches that
    /// ended and the games that connected to or left their match (at most
    /// once per lobby tick for all sessions).
    pub(crate) fn tick(&self, peer: &mut Peer<'_, '_>) -> io::Result<()> {
        let lobby = peer.lobby;
        let account_lo = peer.login.account.account_lo;
        let session = peer.login.identity.session;
        let now = Instant::now();
        let (going, retry) = self.with(lobby, |ctx| {
            let mut due = Vec::new();
            let mut index = 0;
            while index < ctx.state.found.len() {
                let found = &ctx.state.found[index];
                if found.account_lo == account_lo && found.deadline <= now {
                    due.push(ctx.state.found.remove(index));
                } else {
                    index += 1;
                }
            }
            let mut going = Vec::new();
            for found in due {
                // One whose lobby connection was replaced or whose reservation
                // expired gets no 20600.
                let pending = lobby
                    .game
                    .as_deref()
                    .map(|game| game.handoff_pending(found.handoff.conn));
                if found.session == session && matches!(pending, Some(Ok(true))) {
                    going.push(found);
                } else {
                    ctx.free_seat(&found.handoff);
                }
            }
            let sweep = ctx
                .state
                .last_sweep
                .is_none_or(|last| now.duration_since(last) >= TICK);
            let mut retry = None;
            if sweep {
                ctx.state.last_sweep = Some(now);
                ctx.record_results();
                ctx.sweep();
                if std::mem::take(&mut ctx.state.clock_blocked) {
                    ctx.retry(None);
                    retry = Some(
                        ctx.state
                            .deferred_practice
                            .iter()
                            .map(|(&account, &session)| (account, session))
                            .collect::<Vec<_>>(),
                    );
                }
            }
            (going, retry)
        });
        for found in going {
            self.enter_game(peer, found)?;
        }
        for (account_lo, session) in retry.into_iter().flatten() {
            let account = self
                .lock()
                .online
                .get(&account_lo)
                .map(|online| Arc::clone(&online.account));
            let Some(account) = account else {
                self.lock().deferred_practice.remove(&account_lo);
                continue;
            };
            self.practice(lobby, &account, session)?;
        }
        Ok(())
    }

    /// Out of General (a match has the team, match and group chats only),
    /// into the match's chat channel, the team colours, then 20600 (Python
    /// `_enter_game`).
    fn enter_game(&self, peer: &mut Peer<'_, '_>, found: Found) -> io::Result<()> {
        let lobby = peer.lobby;
        let Some(game) = lobby.game.as_deref() else {
            // No match starts with the game server off (`start`).
            return Ok(());
        };
        lobby.open_match_chat(found.match_id, &[found.account_lo]);
        let port = game.local_addr().port();
        // The address the game reached the lobby at, or the public one for
        // a player outside the network (`game_address`).
        let host = peer.session.game_host(&lobby.platform.game_host);
        let sent = handoff::enter_game(
            general_channel(),
            match_channel(found.match_id),
            team_colours(found.team, &found.teams),
            &found.handoff,
            (&host, port),
        )
        .and_then(|[leave, join, colours, ticket]| {
            peer.send(&leave)?;
            peer.send(&join)?;
            peer.send(&colours)?;
            peer.send(&ticket)
        });
        match sent {
            Ok(true) => {
                tracing::info!(
                    "[MM] Match found: {}, game server {host}:{port}, connection {}",
                    found.map_name,
                    found.handoff.conn
                );
                *peer.seat = Some(LobbySeat {
                    account_lo: found.account_lo,
                    lobby_session: found.session[0],
                    handoff: found.handoff,
                });
                Ok(())
            }
            Ok(false) => {
                tracing::warn!("[!] No 20600: the client did not announce the handoff protocol");
                self.lock().free_seat(Some(game), &found.handoff);
                Ok(())
            }
            Err(error) => {
                self.lock().free_seat(Some(game), &found.handoff);
                Err(error)
            }
        }
    }

    // --- the queue (handlers/matchmaking.py) --------------------------------

    /// 44100: the whole party joins the queue. A role queue first waits until
    /// every member pressed Ready on the role screen; the member who picked
    /// the queue is on that screen already.
    pub(crate) fn enter_queue(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        key: Fields,
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        let card = card_of(&key);
        self.with(lobby, |ctx| {
            ctx.post_members(&party, &queue::queue_joined(&key), None);
            if !lobby.arcade.has_roles(card) {
                ctx.search(&party, key, card);
                return;
            }
            let party_queue = ctx.state.queues.entry(party.id).or_default();
            party_queue.key = Some(key);
            party_queue.state = PICKING;
            // The role badge on each portrait shows these roles
            // (0x7FF7898FC8D0); they are new each time.
            party_queue.roles.clear();
            party_queue.accepted = HashSet::from([account.account_lo]);
            party_queue.ready.clear();
            party_queue.pass_roles.clear();
            party_queue.passes_taken.clear();
            ctx.post_members(&party, &queue::wait_times(), None);
            ctx.notify(&party);
            tracing::info!("[MM] Role check started");
        });
        Ok(())
    }

    /// 44102: the player left the queue (the client drops its own entry
    /// first).
    pub(crate) fn cancel_queue(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        key: Fields,
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        self.with(lobby, |ctx| {
            ctx.leave_queue(&party, &key, Some(account.account_lo))
        });
        tracing::info!("[MM] Cancel queue (44102)");
        Ok(())
    }

    /// 44104: Accept on the group banner "pick a role now?".
    pub(crate) fn accept(&self, lobby: &Lobby, account: &Arc<Account>) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        self.with(lobby, |ctx| {
            let Some(party_queue) = ctx.in_role_queue(&party) else {
                tracing::info!("[MM] Accept (44104) without a role check");
                return;
            };
            party_queue.accepted.insert(account.account_lo);
            ctx.notify(&party);
            tracing::info!("[MM] Accept (44104): on the role screen");
        });
        Ok(())
    }

    /// 44105: Decline on that banner; the party leaves the queue.
    pub(crate) fn decline(&self, lobby: &Lobby, account: &Arc<Account>) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        self.with(lobby, |ctx| {
            let Some(key) = ctx
                .in_role_queue(&party)
                .and_then(|queue| queue.key.clone())
            else {
                tracing::info!("[MM] Decline (44105) without a role check");
                return;
            };
            ctx.leave_queue(&party, &key, None);
            tracing::info!("[MM] Decline (44105): the party left the queue");
        });
        Ok(())
    }

    /// 44103: Ready with these roles. An empty list (sent after Change Role
    /// in a group) takes them back.
    pub(crate) fn set_roles(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        numbers: &[u64],
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        let me = account.account_lo;
        let mut roles = Vec::new();
        for &number in numbers {
            if let Ok(number) = u8::try_from(number) {
                if queue::role_guid(number).is_some() && !roles.contains(&number) {
                    roles.push(number);
                }
            }
        }
        tracing::info!("[MM] Roles (44103): {roles:?}");
        self.with(lobby, |ctx| {
            let party_queue = ctx.state.queues.entry(party.id).or_default();
            party_queue.roles.insert(me, roles.clone());
            // 44106 follows with the role for a priority pass, if any.
            party_queue.pass_roles.remove(&me);
            if roles.is_empty() {
                party_queue.ready.remove(&me);
            } else {
                party_queue.ready.insert(me);
                party_queue.accepted.insert(me);
            }
            if party_queue.key.is_some() {
                ctx.update_queue(&party);
            }
        });
        Ok(())
    }

    /// 44106: the role to spend a priority pass on (empty for none), sent
    /// right after 44103. The client only offers it for one chosen role that
    /// takes passes, while the player has passes for this queue.
    pub(crate) fn pass_role(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        numbers: &[u64],
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        let me = account.account_lo;
        let wanted: Vec<u8> = numbers
            .iter()
            .filter_map(|&number| u8::try_from(number).ok())
            .filter(|&number| queue::takes_passes(number))
            .collect();
        self.with(lobby, |ctx| {
            let Some(party_queue) = ctx.in_role_queue(&party) else {
                tracing::info!("[MM] Priority pass (44106) without a role check: {wanted:?}");
                return;
            };
            let pool = party_queue
                .key
                .as_ref()
                .and_then(|key| lobby.arcade.pass_pool(card_of(key)));
            let chosen = party_queue.roles.get(&me).cloned().unwrap_or_default();
            match wanted.first() {
                Some(&role) if chosen.contains(&role) && pass_count(pool, account) > 0 => {
                    party_queue.pass_roles.insert(me, role);
                    tracing::info!("[MM] Priority pass (44106) for role {role}");
                }
                _ => {
                    party_queue.pass_roles.remove(&me);
                }
            }
            if party_queue.state == SEARCHING {
                // The search already started with 44103.
                ctx.take_passes(&party);
            }
            ctx.notify(&party);
        });
        Ok(())
    }

    /// 44107: Change Role on the role screen; the member is not ready any
    /// more and the party waits for them again.
    pub(crate) fn change_roles(&self, lobby: &Lobby, account: &Arc<Account>) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        let me = account.account_lo;
        self.with(lobby, |ctx| {
            let Some(party_queue) = ctx.in_role_queue(&party) else {
                tracing::info!("[MM] Change role (44107) without a role check");
                return;
            };
            party_queue.ready.remove(&me);
            party_queue.pass_roles.remove(&me);
            party_queue.state = PICKING;
            ctx.give_passes_back(&party);
            ctx.notify(&party);
            tracing::info!("[MM] Change role (44107)");
        });
        Ok(())
    }

    /// Training > Practice Range: the player's group goes in together, up to
    /// six on one team as in retail; alone, the player has it to himself.
    /// Members already in a match stay where they are.
    pub(crate) fn practice(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        session: [u64; 2],
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        self.with(lobby, |ctx| {
            if !ctx.state.online(account.account_lo, session) {
                return;
            }
            if ctx.practice(account, &party) || lobby.game.is_none() {
                ctx.state.deferred_practice.remove(&account.account_lo);
            } else {
                ctx.state
                    .deferred_practice
                    .insert(account.account_lo, session);
            }
        });
        Ok(())
    }

    // --- leaving a match (handlers/leaving.py) ------------------------------

    /// 52902: "Leave" or "Leave as group" in the leave dialog, with the match
    /// lobby the client has. The leader leaving as a group takes his party
    /// members in the same match out with him (the party stays); a player who
    /// leaves alone also leaves the party.
    pub(crate) fn leave_match_lobby(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        as_group: bool,
        lobby_id: [u64; 2],
        waiting_request: bool,
    ) -> io::Result<()> {
        let party = lobby.party_of(account)?;
        let me = account.account_lo;
        let alone = self.with(lobby, |ctx| {
            if ctx.state.match_lobbies.get(&me) == Some(&lobby_id) {
                // The client resets its lobby itself.
                ctx.state.match_lobbies.remove(&me);
            }
            if waiting_request {
                // Only STU_3618B2A9 sets it, while a match lobby request
                // (52900) waits (0x7FF78999E3A0): a cancel in the menu, not a
                // leave.
                tracing::info!(
                    "[<<<] Leave match lobby (52902) for a waiting request: nothing to leave"
                );
                return false;
            }
            let Some((room, seat)) = ctx.state.player_of(me) else {
                tracing::info!(
                    "[<<<] Leave match lobby (52902, as group {as_group}): not in a match"
                );
                return false;
            };
            let match_id = ctx.state.rooms[room].id;
            if match_id != lobby_id {
                tracing::info!(
                    "[<<<] Leave match lobby (52902): lobby {lobby_id:X?} is not {match_id:X?}"
                );
            }
            if as_group && party.leader == me && party.members.len() > 1 {
                for member in &party.members {
                    if let Some((other_room, other)) = ctx.state.player_of(member.account_lo) {
                        if ctx.state.rooms[other_room].id == match_id {
                            ctx.take_out(other_room, other, "left with the group");
                        }
                    }
                }
                return false;
            }
            ctx.take_out(room, seat, "left");
            true
        });
        if alone {
            leave_party(lobby, account, "left the match alone")?;
        }
        Ok(())
    }

    /// 21801: the client leaves its game by itself 3 s later (a leave without
    /// a match lobby, the loot box dialog, the Esc menu's direct leave).
    /// Leaving alone takes the player out of the party, but not after the
    /// server sent the game home, as the client may send 21801 on its way out.
    pub(crate) fn leaving_game(&self, lobby: &Lobby, account: &Arc<Account>) -> io::Result<()> {
        let leaving = {
            let state = self.lock();
            state
                .player_of(account.account_lo)
                .is_some_and(|(room, seat)| !state.rooms[room].seats[seat].going_home)
        };
        tracing::info!("[<<<] Leaving the game (21801)");
        if leaving {
            leave_party(lobby, account, "left the match alone")?;
        }
        Ok(())
    }

    // --- the dashboard ------------------------------------------------------

    pub(crate) fn minimum_players(&self) -> usize {
        self.lock().minimum_players
    }

    /// How many searching players start a match of a card without a number of
    /// its own; waiting players are placed again at once.
    pub(crate) fn set_minimum_players(&self, lobby: &Lobby, players: usize) {
        self.with(lobby, |ctx| {
            ctx.state.minimum_players = players;
            ctx.retry(None);
        });
    }

    pub(crate) fn modes(&self) -> HashMap<u64, ModeSettings> {
        self.lock().modes.clone()
    }

    pub(crate) fn rules(&self, card: u64) -> Option<&QueueRules> {
        self.rules.get(&card)
    }

    /// Whether the server can host a map of the card in one of its modes.
    pub(crate) fn can_host(&self, card: u64) -> bool {
        let mut state = self.lock();
        *state.hostable.entry(card).or_insert_with(|| {
            self.rules.get(&card).is_some_and(|rules| {
                !rules::map_choices(&self.maps, &rules.maps, &rules.rulesets).is_empty()
            })
        })
    }

    /// The dashboard's settings for a card, saved in the state folder.
    /// Waiting players are placed again at once.
    pub(crate) fn set_mode(
        &self,
        lobby: &Lobby,
        card: u64,
        players_to_start: usize,
        fill_running: bool,
    ) -> io::Result<()> {
        self.with(lobby, |ctx| {
            if players_to_start != 0 || fill_running {
                ctx.state.modes.insert(
                    card,
                    ModeSettings {
                        players_to_start,
                        fill_running,
                    },
                );
            } else {
                ctx.state.modes.remove(&card);
            }
            let saved = rules::save_modes(&self.modes_path, &ctx.state.modes);
            ctx.retry(None);
            saved
        })
    }

    pub(crate) fn forced_map(&self) -> Option<u64> {
        self.lock().forced_map
    }

    pub(crate) fn set_forced_map(&self, map: Option<u64>) {
        self.lock().forced_map = map;
    }

    /// Every map the data knows, plus the Practice Range, sorted by name.
    pub(crate) fn map_catalog(&self) -> Vec<MapListing> {
        self.maps.map_catalog()
    }

    pub(crate) fn map_name(&self, guid: u64) -> Option<&str> {
        self.maps.map_name(guid)
    }

    /// Send the account's game, or every game for `None`, that is connected
    /// to a match back to the menu (20304 with 0 = leave for good) and take
    /// the player out of it. How many games were told (Python
    /// `GameServer.send_home`, for the lobby's `.leave` and the dashboard's
    /// "End matches"). A game counts as connected as soon as its handoff is
    /// used, before the next sweep marks the seat joined.
    pub(crate) fn send_home(&self, lobby: &Lobby, account_lo: Option<u32>) -> usize {
        let Some(game) = lobby.game.as_deref() else {
            return 0;
        };
        self.with(lobby, |ctx| {
            let mut told = 0;
            for room in 0..ctx.state.rooms.len() {
                for seat in 0..ctx.state.rooms[room].seats.len() {
                    let player = &ctx.state.rooms[room].seats[seat];
                    if player.gone
                        || player.released
                        || account_lo.is_some_and(|account| player.account.account_lo != account)
                    {
                        continue;
                    }
                    let connected = player.joined
                        || (matches!(game.handoff_pending(player.handoff.conn), Ok(false))
                            && matches!(game.seat_exists(&player.handoff), Ok(true)));
                    if connected {
                        let reason = if account_lo.is_some() {
                            "sent home"
                        } else {
                            "the dashboard ended the matches"
                        };
                        ctx.take_out(room, seat, reason);
                        told += 1;
                    }
                }
            }
            told
        })
    }
}

/// Out of the party, as 22107 does; nothing for a party of one.
fn leave_party(lobby: &Lobby, account: &Arc<Account>, why: &str) -> io::Result<()> {
    if lobby.leave_party(account)? {
        tracing::info!("[<<<] Left the party ({why})");
    }
    Ok(())
}

fn perform(lobby: &Lobby, effects: Vec<Effect>) {
    for effect in effects {
        let done = match effect {
            Effect::Post(account_lo, message) => {
                lobby.hub.post(account_lo, message);
                Ok(())
            }
            Effect::Notify(party, queue) => lobby.notify_party(&party, queue.as_ref()),
            Effect::PassesChanged(account) => account.save().and_then(|()| {
                let counts = passes::counts(&account.profile())?;
                lobby.hub.post(account.account_lo, counts);
                Ok(())
            }),
            Effect::GameLeft(account_lo, match_id) => lobby.game_left(account_lo, match_id),
            Effect::Recorded(account, queue, outcome) => results::save(&account, queue, outcome),
        };
        if let Err(error) = done {
            tracing::error!("[MM] {error}");
        }
    }
}

impl State {
    /// Whether this is still the account's logged-in lobby session.
    fn online(&self, account_lo: u32, session: [u64; 2]) -> bool {
        self.online
            .get(&account_lo)
            .is_some_and(|online| online.identity.session == session)
    }

    /// The account's seat in a match it has not left, whether its game
    /// connected or not (Python `GameServer.player_of`).
    fn player_of(&self, account_lo: u32) -> Option<(usize, usize)> {
        self.rooms.iter().enumerate().find_map(|(room, entry)| {
            entry
                .seats
                .iter()
                .position(|seat| seat.account.account_lo == account_lo && !seat.gone)
                .map(|seat| (room, seat))
        })
    }

    fn queue_fields(&self, lobby: &Lobby, party: &Party) -> Option<QueueFields> {
        let party_queue = self.queues.get(&party.id)?;
        let key = party_queue.key.as_ref()?;
        let pool = lobby.arcade.pass_pool(card_of(key));
        let choices = party
            .members
            .iter()
            .map(|member| {
                let lo = member.account_lo;
                let choice = queue::role_choice(
                    party_queue.roles.get(&lo).map_or(&[][..], Vec::as_slice),
                    party_queue.accepted.contains(&lo),
                    party_queue.ready.contains(&lo),
                    pass_count(pool, member),
                    party_queue.pass_roles.get(&lo).copied().unwrap_or(0),
                );
                (lo, choice)
            })
            .collect();
        Some(QueueFields {
            entry: queue::queue_entry(key, party_queue.state),
            choices,
        })
    }

    /// Free the place the game service keeps for a 20600 that will not be
    /// used; its next tick drops the seat.
    fn free_seat(&mut self, game: Option<&GameService>, handoff: &Handoff) {
        if let Some(Err(error)) = game.map(|game| game.cancel_handoff(handoff.conn)) {
            tracing::warn!("[MM] Could not free seat {}: {error}", handoff.conn);
        }
        for room in &mut self.rooms {
            for seat in &mut room.seats {
                if seat.handoff.conn == handoff.conn && room.id == handoff.match_id {
                    seat.released = true;
                }
            }
        }
    }
}

impl Room {
    /// Places left per team: the mode's team sizes less the players still in
    /// the match (one entry for free for all).
    fn free(&self) -> Vec<usize> {
        let playing = || self.seats.iter().filter(|seat| !seat.gone);
        if self.map.free_for_all {
            let size = usize::from(self.map.team_sizes.first().copied().unwrap_or(0));
            return vec![size.saturating_sub(playing().count())];
        }
        self.map
            .team_sizes
            .iter()
            .enumerate()
            .map(|(team, &size)| {
                let taken = playing()
                    .filter(|seat| usize::from(seat.team) == team)
                    .count();
                usize::from(size).saturating_sub(taken)
            })
            .collect()
    }

    /// As the matchmaker sees it; `waiting` from the game service: hero
    /// select's assemble countdown has not started (Python
    /// `Match.waiting_for_players`).
    fn open(&self, waiting: bool) -> OpenMatch {
        let mut roles = vec![BTreeMap::new(); self.map.team_sizes.len()];
        for seat in self
            .seats
            .iter()
            .filter(|seat| !seat.gone && seat.role != 0)
        {
            if let Some(team) = roles.get_mut(usize::from(seat.team)) {
                *team.entry(seat.role).or_insert(0) += 1;
            }
        }
        OpenMatch {
            waiting,
            free: self.free(),
            roles,
        }
    }

    fn teams(&self) -> Vec<u8> {
        (0..self.map.team_sizes.len().max(2))
            .map(|team| u8::try_from(team).unwrap_or(u8::MAX))
            .collect()
    }
}

struct Ctx<'a> {
    lobby: &'a Lobby,
    mm: &'a Matchmaker,
    state: MutexGuard<'a, State>,
    effects: Vec<Effect>,
}

impl Ctx<'_> {
    fn post(&mut self, account_lo: u32, message: LobbyMessage) {
        self.effects.push(Effect::Post(account_lo, message));
    }

    /// To every member who is online, but `skip`.
    fn post_members(&mut self, party: &Party, message: &LobbyMessage, skip: Option<u32>) {
        for member in &party.members {
            if Some(member.account_lo) != skip && self.state.online.contains_key(&member.account_lo)
            {
                self.post(member.account_lo, message.clone());
            }
        }
    }

    fn notify(&mut self, party: &Party) {
        let queue = self.state.queue_fields(self.lobby, party);
        self.effects.push(Effect::Notify(party.clone(), queue));
    }

    fn free_seat(&mut self, handoff: &Handoff) {
        self.state.free_seat(self.lobby.game.as_deref(), handoff);
    }

    /// The party's queue state while it is in a role queue.
    fn in_role_queue(&mut self, party: &Party) -> Option<&mut PartyQueue> {
        self.state
            .queues
            .get_mut(&party.id)
            .filter(|party_queue| party_queue.key.is_some())
    }

    fn pass_pool(&self, party: &Party) -> Option<Pool> {
        let key = self.state.queues.get(&party.id)?.key.as_ref()?;
        self.lobby.arcade.pass_pool(card_of(key))
    }

    /// Starts the search once every member is ready, and sends the party
    /// state. The bot has no client to press Ready, so it counts as ready.
    fn update_queue(&mut self, party: &Party) {
        let Some(party_queue) = self.state.queues.get_mut(&party.id) else {
            return;
        };
        let everyone = party
            .members
            .iter()
            .all(|member| member.is_virtual() || party_queue.ready.contains(&member.account_lo));
        if !everyone || party_queue.state == SEARCHING {
            self.notify(party);
            return;
        }
        party_queue.state = SEARCHING;
        let key = party_queue.key.clone().expect("role queue key");
        // The same key updates the client's queue entry, so its search timer
        // starts at 0.
        self.post_members(party, &queue::queue_joined(&key), None);
        self.take_passes(party);
        tracing::info!("[MM] Everyone is ready: searching");
        self.notify(party);
        let card = card_of(&key);
        self.search(party, key, card);
    }

    /// The search holds one pass of each member who used one.
    fn take_passes(&mut self, party: &Party) {
        let pool = self.pass_pool(party);
        let Some(party_queue) = self.state.queues.get_mut(&party.id) else {
            return;
        };
        for member in &party.members {
            let lo = member.account_lo;
            if !party_queue.pass_roles.contains_key(&lo)
                || party_queue.passes_taken.contains(&lo)
                || pass_count(pool, member) <= 0
            {
                continue;
            }
            if let Some(pool) = pool {
                if pool.change(&mut member.profile(), -1).is_ok() {
                    party_queue.passes_taken.insert(lo);
                    self.effects.push(Effect::PassesChanged(Arc::clone(member)));
                }
            }
        }
    }

    /// A search that ends without a match gives its passes back.
    fn give_passes_back(&mut self, party: &Party) {
        let pool = self.pass_pool(party);
        let Some(party_queue) = self.state.queues.get_mut(&party.id) else {
            return;
        };
        for member in &party.members {
            if party_queue.passes_taken.contains(&member.account_lo) {
                if let Some(pool) = pool {
                    let _ = pool.change(&mut member.profile(), 1);
                }
                self.effects.push(Effect::PassesChanged(Arc::clone(member)));
            }
        }
        party_queue.passes_taken.clear();
    }

    /// Takes the party out of the queue. Members get 44202, so their client
    /// drops its queue entry and closes the role screens; the one who
    /// cancelled (`skip`) dropped it already.
    fn leave_queue(&mut self, party: &Party, key: &Fields, skip: Option<u32>) {
        self.post_members(party, &queue::queue_left(key), skip);
        self.cancel(party);
        if self.in_role_queue(party).is_none() {
            return;
        }
        self.give_passes_back(party);
        let party_queue = self.state.queues.get_mut(&party.id).expect("role queue");
        party_queue.key = None;
        party_queue.state = 0;
        party_queue.accepted.clear();
        party_queue.ready.clear();
        party_queue.pass_roles.clear();
        self.notify(party);
    }

    /// The party leaves the queue. Members in "Game Found!" whose lobby
    /// connection closed give their seat in the match back at once; the
    /// others still go (the client itself cannot cancel then).
    fn cancel(&mut self, party: &Party) {
        self.state
            .tickets
            .retain(|ticket| ticket.party.id != party.id || party.id == [0; 2]);
        let members: Vec<u32> = party
            .members
            .iter()
            .map(|member| member.account_lo)
            .collect();
        for account in &members {
            if let Some(&session) = self.state.deferred_practice.get(account) {
                if !self.state.online(*account, session) {
                    self.state.deferred_practice.remove(account);
                }
            }
        }
        let mut index = 0;
        while index < self.state.found.len() {
            let found = &self.state.found[index];
            if members.contains(&found.account_lo)
                && !self.state.online(found.account_lo, found.session)
            {
                let found = self.state.found.remove(index);
                self.free_seat(&found.handoff);
                tracing::info!("[MM] Left during \"Game Found!\": the seat in the match is free");
            } else {
                index += 1;
            }
        }
    }

    /// The members who can play: online, not the bot.
    fn players(&self, party: &Party) -> Vec<Arc<Account>> {
        party
            .members
            .iter()
            .filter(|member| {
                !member.is_virtual() && self.state.online.contains_key(&member.account_lo)
            })
            .cloned()
            .collect()
    }

    fn players_to_start(&self, card: u64) -> usize {
        match self.state.modes.get(&card) {
            Some(own) if own.players_to_start != 0 => own.players_to_start,
            _ => self.state.minimum_players,
        }
    }

    /// Whether players who search join the card's matches that are on:
    /// unranked cards, when the dashboard says so.
    fn fills_running(&self, card: u64) -> bool {
        self.state
            .modes
            .get(&card)
            .is_some_and(|own| own.fill_running)
            && !self.mm.rules_of(card).competitive
    }

    /// The party searches this queue now (every member ready).
    fn search(&mut self, party: &Party, key: Fields, card: u64) {
        let roles = self.state.queues.get(&party.id).map(|queue| &queue.roles);
        let members: Vec<_> = self
            .players(party)
            .into_iter()
            .map(|member| {
                let picked = roles
                    .and_then(|roles| roles.get(&member.account_lo))
                    .cloned()
                    .unwrap_or_default();
                (member, picked)
            })
            .collect();
        tracing::info!("[MM] A party of {} searches card 0x{card:X}", members.len());
        self.state
            .tickets
            .retain(|ticket| ticket.party.id != party.id);
        self.state.tickets.push(Ticket {
            party: party.clone(),
            key,
            card,
            members,
        });
        self.try_match(card);
    }

    /// Place the waiting parties again (of one card, or of all): a setting
    /// changed, or a player left a match and freed a place.
    fn retry(&mut self, card: Option<u64>) {
        let mut cards: Vec<u64> = self
            .state
            .tickets
            .iter()
            .map(|ticket| ticket.card)
            .filter(|&waiting| card.is_none_or(|card| card == waiting))
            .collect();
        cards.sort_unstable();
        cards.dedup();
        for card in cards {
            self.try_match(card);
        }
    }

    /// The card's waiting parties join its matches that have room, and the
    /// rest start a new one once enough of them wait.
    fn try_match(&mut self, card: u64) {
        let mm = self.mm;
        let rules = mm.rules_of(card);
        self.join_open_matches(card, rules);
        let waiting: Vec<usize> = (0..self.state.tickets.len())
            .filter(|&index| self.state.tickets[index].card == card)
            .collect();
        if waiting.is_empty() {
            return;
        }
        let Some((map, ruleset)) =
            rules::pick_map(&mm.maps, rules, self.state.forced_map, random_index)
        else {
            tracing::info!(
                "[MM] Card 0x{card:X} ({}): no map the server can host yet",
                rules.name
            );
            return;
        };
        // A Practice vs AI card puts its players on the human team and
        // starts at once with whoever searches: AI heroes of the card's
        // level fill both teams (rule 123). Elsewhere a match that starts
        // with fewer players (`--test-players`) gets AI heroes for the
        // missing ones when `--test-ai` asks for them.
        let (ruleset, minimum, ai) = if rules.versus_ai() {
            let level = rules.ai_teams.values().copied().max().unwrap_or(1);
            (
                rules.human_ruleset(ruleset),
                1,
                Difficulty::from_level(level).or(Some(Difficulty::Easy)),
            )
        } else {
            let minimum = self.players_to_start(card);
            let ai = self.state.test_ai.filter(|_| minimum != 0);
            (ruleset.clone(), minimum, ai)
        };
        let seekers: Vec<Vec<Vec<u8>>> = waiting
            .iter()
            .map(|&index| {
                self.state.tickets[index]
                    .members
                    .iter()
                    .map(|(_, roles)| roles.clone())
                    .collect()
            })
            .collect();
        let parties: Vec<&Seekers> = seekers.iter().map(Vec::as_slice).collect();
        let Some(roster) = rules::build_roster(&parties, &ruleset, &rules.roles, minimum) else {
            let count: usize = seekers.iter().map(Vec::len).sum();
            let full: usize = ruleset
                .team_sizes
                .iter()
                .map(|&size| usize::from(size))
                .sum();
            let needed = if minimum == 0 {
                full
            } else {
                minimum.min(full)
            };
            tracing::info!(
                "[MM] Card 0x{card:X} ({}): {count} of {needed} players waiting",
                rules.name
            );
            return;
        };
        let mut entries = Vec::new();
        let mut used = Vec::new();
        for (team, picks) in roster.teams.iter().enumerate() {
            for pick in picks {
                let ticket = &self.state.tickets[waiting[pick.party]];
                if !used.contains(&ticket.party.id) {
                    used.push(ticket.party.id);
                }
                let (account, _) = &ticket.members[pick.member];
                if let Some(entry) = self.entry(account, ticket, team, pick.role) {
                    entries.push(entry);
                }
            }
        }
        self.start(map, entries, true, card, &used, ai);
    }

    fn entry(
        &self,
        account: &Arc<Account>,
        ticket: &Ticket,
        team: usize,
        role: u8,
    ) -> Option<Entry> {
        let online = self.state.online.get(&account.account_lo)?;
        Some(Entry {
            account: Arc::clone(account),
            session: online.identity.session,
            tournament: online.tournament,
            party: ticket.party.clone(),
            key: ticket.key.clone(),
            team: u8::try_from(team).ok()?,
            role,
        })
    }

    /// Waiting parties, oldest first, join the card's matches that have room:
    /// a match that still waits for its players on any card, one that is on
    /// only where `fills_running` says so. A party goes whole into one team.
    /// With full teams a role queue keeps its role slots.
    fn join_open_matches(&mut self, card: u64, rules: &QueueRules) {
        if card == 0 || !self.state.tickets.iter().any(|ticket| ticket.card == card) {
            return;
        }
        let slots = if self.players_to_start(card) != 0 {
            BTreeMap::new()
        } else {
            rules.roles.clone()
        };
        // Whether each match still waits for its players (Python
        // `GameServer.open_matches`); one the service no longer has is over.
        let lobby = self.lobby;
        let Some(game) = lobby.game.as_deref() else {
            return;
        };
        let waiting: HashMap<[u64; 2], bool> = match game.snapshot() {
            Ok(matches) => matches
                .into_iter()
                .map(|found| (found.id, found.waiting))
                .collect(),
            Err(error) => {
                tracing::warn!("[MM] The game service did not list its matches: {error}");
                return;
            }
        };
        for room in 0..self.state.rooms.len() {
            let Some(&waiting) = waiting.get(&self.state.rooms[room].id) else {
                continue;
            };
            // AI heroes hold the free places of a match they fill.
            if self.state.rooms[room].card != card || self.state.rooms[room].ai {
                continue;
            }
            let mut index = 0;
            while index < self.state.tickets.len() {
                let open = self.state.rooms[room].open(waiting);
                if !open.free.iter().any(|&free| free > 0)
                    || (!open.waiting && !self.fills_running(card))
                {
                    break;
                }
                let ticket = &self.state.tickets[index];
                if ticket.card != card {
                    index += 1;
                    continue;
                }
                let seekers: Vec<Vec<u8>> = ticket
                    .members
                    .iter()
                    .map(|(_, roles)| roles.clone())
                    .collect();
                let placed = rules::place(&seekers, &open, &slots, !rules.roles.is_empty());
                match placed {
                    Some((team, given)) if self.join(room, index, team, &given) => {
                        self.state.tickets.remove(index);
                    }
                    _ => index += 1,
                }
            }
        }
    }

    /// The party joins an open match on that team.
    fn join(&mut self, room: usize, ticket: usize, team: usize, given: &[u8]) -> bool {
        let ticket = &self.state.tickets[ticket];
        let entries: Vec<Entry> = ticket
            .members
            .iter()
            .zip(given)
            .filter_map(|((account, _), &role)| self.entry(account, ticket, team, role))
            .collect();
        if entries.is_empty() {
            return false;
        }
        let card = self.state.rooms[room].card;
        let Some((players, records)) = self.world_players(&entries, card) else {
            return false;
        };
        let id = self.state.rooms[room].id;
        let lobby = self.lobby;
        let Some(game) = lobby.game.as_deref() else {
            return false;
        };
        let handoffs = match game::join_running(game, id, players) {
            Ok(Some(handoffs)) => handoffs,
            Ok(None) => return false,
            Err(error) => {
                tracing::warn!("[MM] Joining match {id:X?} failed: {error}");
                return false;
            }
        };
        self.seat(room, &entries, &handoffs, records);
        let (name, teams) = (
            self.state.rooms[room].map.name.clone(),
            self.state.rooms[room].teams(),
        );
        self.send_to_game(entries, handoffs, &name, &teams, true);
        true
    }

    fn world_players(
        &self,
        entries: &[Entry],
        card: u64,
    ) -> Option<(Vec<WorldPlayer>, Vec<Fields>)> {
        let mut players = Vec::new();
        let mut records = Vec::new();
        for entry in entries {
            match game::world_player(
                self.lobby,
                &entry.account,
                entry.tournament,
                entry.team,
                card,
                entry.role,
            ) {
                Ok((player, record)) => {
                    players.push(player);
                    records.push(record);
                }
                Err(error) => {
                    tracing::error!("[MM] {}'s card: {error}", entry.account.name);
                    return None;
                }
            }
        }
        Some((players, records))
    }

    fn seat(&mut self, room: usize, entries: &[Entry], handoffs: &[Handoff], records: Vec<Fields>) {
        let seats = entries
            .iter()
            .zip(handoffs)
            .zip(records)
            .map(|((entry, handoff), record)| Seat {
                account: Arc::clone(&entry.account),
                party: entry.party.id,
                team: entry.team,
                role: entry.role,
                record,
                handoff: handoff.clone(),
                joined: false,
                gone: false,
                going_home: false,
                released: false,
            });
        self.state.rooms[room].seats.extend(seats);
    }

    /// Training > Practice Range for the player and the members of his party
    /// who are not in a match, on one team.
    fn practice(&mut self, account: &Arc<Account>, party: &Party) -> bool {
        let mut members = vec![Arc::clone(account)];
        for member in self.players(party) {
            if member.account_lo != account.account_lo
                && self.state.player_of(member.account_lo).is_none()
            {
                members.push(member);
            }
        }
        let ticket = Ticket {
            party: party.clone(),
            key: Fields::new(),
            card: 0,
            members: Vec::new(),
        };
        let entries: Vec<Entry> = members
            .iter()
            .filter_map(|member| self.entry(member, &ticket, 0, 0))
            .collect();
        let mut map = GameMap::practice_range();
        map.team_sizes = vec![u8::try_from(members.len()).unwrap_or(u8::MAX), 0];
        self.start(map, entries, false, 0, &[], None)
    }

    /// Open the match and send its players there; with `ai`, AI heroes of
    /// that level fill its teams.
    fn start(
        &mut self,
        map: GameMap,
        entries: Vec<Entry>,
        popped: bool,
        card: u64,
        used: &[[u64; 2]],
        ai: Option<Difficulty>,
    ) -> bool {
        let lobby = self.lobby;
        let Some(game) = lobby.game.as_deref() else {
            tracing::warn!("[MM] The game server is off (--game-port 0)");
            return false;
        };
        if entries.is_empty() {
            return false;
        }
        let Some((players, records)) = self.world_players(&entries, card) else {
            return false;
        };
        let id = match match_id() {
            Ok(id) => id,
            Err(error) => {
                tracing::error!("[MM] {error}");
                return false;
            }
        };
        let handoffs = match game::open_world(game, id, map.clone(), lobby, players, ai) {
            Ok(handoffs) if handoffs.len() == entries.len() => handoffs,
            Ok(handoffs) => {
                for handoff in &handoffs {
                    self.free_seat(handoff);
                }
                return false;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                tracing::info!("[MM] {}: the game clock is not ready: {error}", map.name);
                self.state.clock_blocked = true;
                return false;
            }
            Err(error) => {
                tracing::warn!(
                    "[MM] {} ({}): the game service did not open it: {error}",
                    map.name,
                    map.mode_name
                );
                return false;
            }
        };
        // Competitive as for the game's 20300: the lobby gives the players a
        // rank there.
        let ranked = card != 0
            && self
                .lobby
                .ranked
                .rank(
                    &entries[0].account.profile(),
                    card,
                    0,
                    &LeaderboardPlaces::new(),
                )
                .is_ok_and(|rank| rank.is_some());
        self.state.rooms.push(Room {
            id,
            card,
            map,
            ranked,
            ai: ai.is_some(),
            seats: Vec::new(),
        });
        let room = self.state.rooms.len() - 1;
        self.seat(room, &entries, &handoffs, records);
        if popped {
            self.state
                .tickets
                .retain(|ticket| !used.contains(&ticket.party.id));
        }
        let (name, teams) = (
            self.state.rooms[room].map.name.clone(),
            self.state.rooms[room].teams(),
        );
        self.send_to_game(entries, handoffs, &name, &teams, popped);
        true
    }

    /// The players' searches end and their games go to the match. A queue
    /// match (popped) shows "Game Found!" first; the Practice Range goes at
    /// once.
    fn send_to_game(
        &mut self,
        entries: Vec<Entry>,
        handoffs: Vec<Handoff>,
        map_name: &str,
        teams: &[u8],
        popped: bool,
    ) {
        let mut players: Vec<(Entry, Handoff)> = entries.into_iter().zip(handoffs).collect();
        let mut delay = Duration::ZERO;
        if popped {
            let mut ended = Vec::new();
            for (entry, _) in &players {
                if !ended.contains(&entry.party.id) {
                    ended.push(entry.party.id);
                    self.end_search(&entry.party.clone());
                }
            }
            let mut kept = Vec::new();
            for (entry, handoff) in players {
                if self.state.online(entry.account.account_lo, entry.session) {
                    self.post(entry.account.account_lo, queue_popped(entry.key.clone()));
                    kept.push((entry, handoff));
                } else {
                    tracing::warn!(
                        "[!] No game for {}: the lobby connection closed",
                        entry.account.name
                    );
                    self.free_seat(&handoff);
                }
            }
            players = kept;
            if players.is_empty() {
                return;
            }
            let away = self.leave_other_games(&players);
            delay = handoff::handoff_delay(true, FOUND_DELAY, away);
            self.game_found(&players, map_name, delay);
        }
        let deadline = Instant::now() + delay;
        for (entry, handoff) in players {
            self.state.found.push(Found {
                account_lo: entry.account.account_lo,
                session: entry.session,
                match_id: handoff.match_id,
                handoff,
                map_name: map_name.to_owned(),
                team: entry.team,
                teams: teams.to_vec(),
                deadline,
            });
        }
    }

    /// Games still in another match (the Practice Range played while
    /// searching) go to the menu first (20304). From the menu a game makes a
    /// new world for the new match. Whether any game was sent.
    fn leave_other_games(&mut self, players: &[(Entry, Handoff)]) -> bool {
        let mut away = false;
        for (entry, handoff) in players {
            for room in 0..self.state.rooms.len() {
                if self.state.rooms[room].id == handoff.match_id {
                    continue;
                }
                let seat = self.state.rooms[room].seats.iter().position(|seat| {
                    seat.account.account_lo == entry.account.account_lo && !seat.gone
                });
                if let Some(seat) = seat {
                    self.take_out(room, seat, "its queue match was found");
                    away = true;
                }
            }
        }
        away
    }

    /// Each player's client shows "Game Found!" (53000, then 53003 with
    /// reason 4); the 20600 follows after `delay`. On a competitive card
    /// 53001 makes the match ranked and 53002 gives the teams, for the VERSUS
    /// loading screen (`lineup.rs`).
    fn game_found(&mut self, players: &[(Entry, Handoff)], map_name: &str, delay: Duration) {
        let versus = self.versus(players);
        for (entry, handoff) in players {
            for message in handoff::game_found(entry.key.clone(), handoff.match_id, versus.clone())
            {
                self.post(entry.account.account_lo, message);
            }
            // 53003 resets the client's match lobby whatever its +0x78 says
            // (0x7FF789693120), so the old match's leave sends none: with both
            // ids 0 again it would pass the client's test lobby +0x88 ==
            // post-game +0x30 and run "Leaving match lobby" (0x7FF7896931C4).
            self.state.match_lobbies.remove(&entry.account.account_lo);
            tracing::info!(
                "[MM] \"Game Found!\": {map_name}, the game in {} s",
                delay.as_secs_f32()
            );
        }
    }

    /// The match's settings and roster (53001, 53002) when it is ranked.
    fn versus(&self, players: &[(Entry, Handoff)]) -> Option<(Fields, Fields)> {
        let match_id = players.first()?.1.match_id;
        let room = self.state.rooms.iter().find(|room| room.id == match_id)?;
        if !room.ranked {
            return None;
        }
        let lineup: Vec<lineup::Player> = room
            .seats
            .iter()
            .filter(|seat| !seat.gone)
            .map(|seat| {
                let rating = self
                    .lobby
                    .ranked
                    .party_ratings(&seat.account.profile(), &LeaderboardPlaces::new())
                    .ok()
                    .and_then(|ratings| {
                        ratings.into_iter().find(|rating| {
                            matches!(rating, Value::Struct(fields)
                                if fields.get(&0x18) == Some(&Value::Unsigned(room.card)))
                        })
                    });
                lineup::Player {
                    card: seat.record.clone(),
                    team: seat.team,
                    role: seat.role,
                    rating,
                    group: seat.party,
                }
            })
            .collect();
        Some((
            lineup::settings(match_id, lineup::RANKED),
            lineup::roster(&lineup),
        ))
    }

    /// The party is out of the queue; the passes it spent stay spent (a match
    /// was found).
    fn end_search(&mut self, party: &Party) {
        let party_queue = self.state.queues.entry(party.id).or_default();
        party_queue.key = None;
        party_queue.state = 0;
        party_queue.accepted.clear();
        party_queue.ready.clear();
        party_queue.pass_roles.clear();
        party_queue.passes_taken.clear();
        self.notify(party);
    }

    /// Take a player out of its match from the server side: a connected game
    /// goes back to the menu (20304) and leaves the match when its link
    /// closes; a game that has not connected leaves the match now, and gets
    /// 20304 if it still connects (Python `GameServer.take_out`).
    fn take_out(&mut self, room: usize, seat: usize, reason: &str) {
        let player = &mut self.state.rooms[room].seats[seat];
        player.going_home = true;
        let released = self
            .lobby
            .game
            .as_deref()
            .map(|game| game.release_seat(&player.handoff));
        let done = match released {
            Some(Ok(_)) if player.joined => "sent to the menu",
            Some(Ok(_)) => "left before its game connected",
            _ => "the game service did not answer",
        };
        let name = player.account.name.clone();
        let id = self.state.rooms[room].id;
        tracing::info!("[MM] {name} out of match {id:X?} ({reason}): {done}");
    }

    /// The matches that ended by their rules since the last tick go into
    /// the profiles of their seated players who were still in them (see
    /// `results`). They come before the sweep, which drops a room once its
    /// last game left. The room itself stays: its games leave the ended
    /// match as from any other, and the sweep frees it after the last.
    fn record_results(&mut self) {
        let Some(game) = self.lobby.game.as_deref() else {
            return;
        };
        let ended = match game.take_results() {
            Ok(ended) => ended,
            Err(error) => {
                tracing::warn!("[MM] The game service did not report its results: {error}");
                return;
            }
        };
        for result in ended {
            let Some(room) = self.state.rooms.iter().find(|room| room.id == result.id) else {
                tracing::info!("[MM] Match {:X?} ended without a room", result.id);
                continue;
            };
            let ranked = &self.lobby.ranked;
            for (account, queue, outcome) in
                results::recorded(room, &result, |profile| ranked.season(profile))
            {
                tracing::info!(
                    "[MM] {} finished match {:X?}: {outcome:?} in {}",
                    account.name,
                    result.id,
                    queue.name()
                );
                self.effects.push(Effect::Recorded(account, queue, outcome));
            }
        }
    }

    /// The games that connected to their match get its match lobby; those
    /// that left it lose it, are back in General, and their place may go to
    /// a party that waits (Python `LobbyServer.game_joined`, `game_left`).
    fn sweep(&mut self) {
        let Some(game) = self.lobby.game.as_deref() else {
            return;
        };
        let mut events = Vec::new();
        for (room_index, room) in self.state.rooms.iter_mut().enumerate() {
            for seat in room.seats.iter_mut().filter(|seat| !seat.gone) {
                match game.seat_exists(&seat.handoff) {
                    Ok(false) => {
                        seat.gone = true;
                        events.push((room_index, seat.account.account_lo, false));
                    }
                    // A reserved seat whose handoff no longer waits has
                    // connected. The game service reports no connect event,
                    // so a reservation that just ran out (60 s) also reads as
                    // a connect until the service's next tick drops the seat,
                    // which the next sweep sees as a leave.
                    Ok(true) if !seat.joined && !seat.released && !seat.going_home => {
                        if let Ok(false) = game.handoff_pending(seat.handoff.conn) {
                            seat.joined = true;
                            events.push((room_index, seat.account.account_lo, true));
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut cards = Vec::new();
        for (room, account_lo, joined) in events {
            let (match_id, card, ranked) = {
                let room = &self.state.rooms[room];
                (room.id, room.card, room.ranked)
            };
            let online = self.state.online.contains_key(&account_lo);
            if joined {
                if online {
                    self.post(account_lo, lineup::lobby_state(match_id));
                    self.state.match_lobbies.insert(account_lo, match_id);
                    tracing::info!("[>>>] Match lobby of {match_id:X?} (53000)");
                }
                continue;
            }
            if self.state.match_lobbies.get(&account_lo) == Some(&match_id) {
                self.state.match_lobbies.remove(&account_lo);
                self.post(account_lo, lineup::lobby_left(match_id));
            }
            if online {
                if ranked {
                    // Its VERSUS roster goes (lineup.rs).
                    self.post(account_lo, lineup::no_roster());
                }
                self.effects.push(Effect::GameLeft(account_lo, match_id));
            }
            if card != 0 && !cards.contains(&card) {
                cards.push(card);
            }
        }
        self.state
            .rooms
            .retain(|room| room.seats.iter().any(|seat| !seat.gone));
        for card in cards {
            self.retry(Some(card));
        }
    }
}
