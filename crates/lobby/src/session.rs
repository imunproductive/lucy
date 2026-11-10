//! Accepted lobby connections and the login preparation shared with the session.
//!
//! This module assembles the currently ported initial messages for verification.
//! The complete login and post-login handlers still have to be integrated.

use std::{
    collections::BTreeMap,
    io,
    net::{Ipv4Addr, SocketAddr, TcpStream},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    accounts::{Account, Accounts, account_id_for, battle_tag},
    arcade::Arcade,
    battle_tag_query,
    celebrations::Celebrations,
    clock::server_time,
    collection::{Collection, HERO_CATALOG, PROGRESSION},
    handoff::game_host,
    login::{self, INCOMPATIBLE_CLIENT, Identity, LOGIN_MESSAGE, Login, OUT_CONNECT},
    mode_rules::{self, MODE_RULES, RULES},
    passes, player_messages as player,
    presence::Presence,
    ranked::{LeaderboardPlaces, Ranked},
    templates::{CONFIG, IN_CONNECT, LOBBY, RetailTemplates},
    transport::{Event, LobbyMessage, LobbyTransport},
};
use orpheus_wire::{
    jam::codec::{Fields, Schemas, Value},
    jam::handshake::handshake_io,
    tcp::PeerStream,
};

const HELLO_TIMEOUT: Duration = Duration::from_secs(30);
const DISCONNECT: u32 = 20503;
const PING: u32 = 0xdda5_83ef;

pub struct LobbySession<'a> {
    transport: LobbyTransport,
    schemas: &'a Schemas,
    /// The game's address, and the lobby address it reached.
    peer: Ipv4Addr,
    reached: Ipv4Addr,
    local: bool,
    authenticated: bool,
}

pub enum LoginRequest {
    Prepared(PreparedLogin),
    Rejected { reason: u64 },
    Disconnected,
}

pub struct PreparedLogin {
    pub account: Arc<Account>,
    pub identity: Identity,
    pub tournament: bool,
}

#[derive(Clone, Copy)]
pub struct LoginContent<'a> {
    pub menu_hero: Option<u64>,
    pub extra_preload: &'a [u64],
    pub wall_time: u64,
    pub arcade: &'a Arcade,
    pub celebrations: Option<&'a Celebrations>,
    pub leaderboard_places: &'a LeaderboardPlaces,
    /// The session's party state and party chat join
    /// (`ow174/lobby/session.py` party_messages), sent before the settings.
    pub party: &'a [LobbyMessage],
}

impl<'a> LobbySession<'a> {
    /// Handle a launcher BattleTag query, or open the encrypted game connection.
    pub fn accept(
        stream: TcpStream,
        connection_id: u32,
        signing_key: &[u8; 64],
        accounts: &Accounts,
        schemas: &'a Schemas,
    ) -> io::Result<Option<Self>> {
        Self::accept_peer(
            PeerStream::new(stream, Arc::default())?,
            connection_id,
            signing_key,
            accounts,
            schemas,
        )
    }

    pub fn accept_peer(
        mut stream: PeerStream,
        connection_id: u32,
        signing_key: &[u8; 64],
        accounts: &Accounts,
        schemas: &'a Schemas,
    ) -> io::Result<Option<Self>> {
        let (SocketAddr::V4(peer), SocketAddr::V4(address)) =
            (stream.peer_addr()?, stream.local_addr()?)
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the lobby handshake requires IPv4",
            ));
        };
        let local = peer.ip().is_loopback();
        stream.set_deadline(Some(Instant::now() + HELLO_TIMEOUT));
        if battle_tag_query::answer_peer(&mut stream, |name| resolve_battle_tag(accounts, name))? {
            return Ok(None);
        }
        let channel = handshake_io(&mut stream, address, connection_id, signing_key)?;
        stream.set_deadline(Some(Instant::now() + HELLO_TIMEOUT));
        let transport = LobbyTransport::from_peer(stream, channel);
        Ok(Some(Self {
            transport,
            schemas,
            peer: *peer.ip(),
            reached: *address.ip(),
            local,
            authenticated: false,
        }))
    }

    /// The game runs on this PC (`Session.local` in `ow174/lobby/session.py`).
    pub fn is_local(&self) -> bool {
        self.local
    }

    /// Where this game finds the game server (`game_address` in
    /// `ow174/lobby/matchmaker.py`): the address it reached the lobby at, or
    /// the public address for a game from outside the host's network.
    pub fn game_host(&self, public: &str) -> String {
        game_host(public, self.reached, self.peer)
    }

    /// Wait for a valid login in the negotiated group. The account and session
    /// identity are available to the caller before any online state is changed.
    pub fn prepare_login(
        &mut self,
        accounts: &Accounts,
        local_default: impl FnOnce() -> io::Result<Arc<Account>>,
    ) -> io::Result<LoginRequest> {
        self.schemas
            .fields(OUT_CONNECT, LOGIN_MESSAGE)
            .map_err(invalid)?;
        let Some((_, _, fields)) =
            self.receive_matching(|crc, id| (crc, id) == (OUT_CONNECT, LOGIN_MESSAGE), || true)?
        else {
            return Ok(LoginRequest::Disconnected);
        };
        let (account, tournament) = match login::identify(&fields, self.local)? {
            Login::Named { name, tournament } => (accounts.get(&name)?, tournament),
            Login::LocalDefault => (local_default()?, false),
            Login::IncompatibleRemote => {
                return Ok(LoginRequest::Rejected {
                    reason: INCOMPATIBLE_CLIENT,
                });
            }
        };
        let identity = Identity::create(account.account_lo, self.transport.sequence)?;
        self.transport.set_deadline(None);
        self.authenticated = true;
        Ok(LoginRequest::Prepared(PreparedLogin {
            account,
            identity,
            tournament,
        }))
    }

    /// Receive the next valid client message after accepting the session login.
    /// Client messages use the single-byte group offset.
    pub fn receive_message(&mut self) -> io::Result<Option<LobbyMessage>> {
        self.receive_message_until(|| true)
    }

    /// Receive the next client message while `keep_receiving` holds; `None`
    /// when it stopped holding or the client closed the connection
    /// ([`closed`](Self::closed) tells which).
    pub fn receive_message_until(
        &mut self,
        keep_receiving: impl Fn() -> bool,
    ) -> io::Result<Option<LobbyMessage>> {
        if !self.authenticated {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "lobby session has not accepted a login",
            ));
        }
        self.receive_matching(|_, _| true, keep_receiving)
    }

    fn receive_matching(
        &mut self,
        accept: impl Fn(u32, u32) -> bool,
        keep_receiving: impl Fn() -> bool,
    ) -> io::Result<Option<LobbyMessage>> {
        while let Some(event) = self.transport.receive_until(&keep_receiving)? {
            let Event::Message { crc, offset, body } = event else {
                continue;
            };
            let Ok(base) = self.schemas.base(crc) else {
                tracing::info!(
                    "[<<<] {crc:08X} offset={offset} len={} (unknown protocol)",
                    body.len()
                );
                continue;
            };
            let Some(id) = base.checked_add(u32::from(offset)) else {
                continue;
            };
            if !accept(crc, id) {
                continue;
            }
            let fields = match self.schemas.decode(crc, id, &body, true) {
                Ok(fields) => fields,
                Err(error) => {
                    tracing::info!(
                        "[<<<] {crc:08X}/{id} len={} undecodable: {error}",
                        body.len()
                    );
                    continue;
                }
            };
            return Ok(Some((crc, id, fields)));
        }
        Ok(None)
    }

    /// Whether the client closed the connection.
    pub fn closed(&self) -> bool {
        self.transport.closed()
    }

    /// Send an outbound JAM message with the wire index announced by this client.
    /// Returns false if the client did not announce its group.
    pub fn send(&mut self, (crc, id, fields): &LobbyMessage) -> io::Result<bool> {
        let Some(wire) = self.transport.wire_for(*crc) else {
            return Ok(false);
        };
        let mut frame = self.schemas.header(*crc, wire, *id).map_err(invalid)?;
        frame.extend(self.schemas.encode(*crc, *id, fields).map_err(invalid)?);
        self.transport.send(&frame)?;
        Ok(true)
    }

    /// Send the disconnect reason. The caller manages the grace period and socket shutdown.
    pub fn send_disconnect(&mut self, reason: u64) -> io::Result<bool> {
        self.send(&(
            IN_CONNECT,
            DISCONNECT,
            Fields::from([(0x78, Value::Unsigned(reason)), (0x80, Value::Bool(false))]),
        ))
    }
}

impl PreparedLogin {
    /// Build the verified subset of the initial login messages, in relative wire
    /// order. The caller must add the unported messages before serving retail.
    pub fn available_messages(
        &self,
        schemas: &Schemas,
        templates: &RetailTemplates,
        collection: &Collection,
        presence: &Presence,
        ranked: &Ranked,
        content: LoginContent<'_>,
    ) -> io::Result<Vec<LobbyMessage>> {
        let profile = self.account.profile();
        let identity = self.identity;
        let now = server_time(&profile.server_date, content.wall_time)?;
        let record = player::record(
            &profile,
            identity,
            collection.portrait_frame(&profile),
            content.wall_time,
        );
        let mut messages = vec![
            (IN_CONNECT, 20500, player::hello(&profile, identity)),
            (
                IN_CONNECT,
                20502,
                Fields::from([(0x78, Value::Struct(record))]),
            ),
        ];
        let preload = (IN_CONNECT, 20505, templates.preload(content.extra_preload)?);
        if let Some(celebrations) = content.celebrations {
            let [keys, records, progress] =
                celebrations.login_messages(&profile, collection.items(), now)?;
            messages.extend([keys, preload, records, progress]);
        } else {
            messages.push(preload);
        }
        messages.extend(player::endorsements(&profile, identity, content.wall_time));
        messages.push(presence.own(&profile, identity, content.wall_time)?);
        messages.extend([
            (PROGRESSION, 24300, collection.progression(&profile)?),
            (HERO_CATALOG, 24900, collection.hero_catalog(&profile)?),
        ]);
        let season = ranked.season(&profile);
        messages.push((
            MODE_RULES,
            RULES,
            mode_rules::mode_rules(
                templates,
                &profile,
                collection.heroes(),
                collection.items(),
                season,
            )?,
        ));
        let arcade = content.arcade.state(&profile, season, now)?;
        let mut built = BTreeMap::from([((LOBBY, 20812), player::ux_states(&profile)?)]);
        for (crc, id, fields) in arcade
            .messages
            .into_iter()
            .chain([passes::counts(&profile)?])
        {
            built.insert((crc, id), fields);
        }
        messages.extend(templates.at_login(&profile, identity, content.menu_hero, now, &built)?);
        messages.extend(ranked.login_messages(
            &profile,
            &arcade.competitive_cards,
            content.leaderboard_places,
            now,
        )?);
        messages.push((
            PING,
            35500,
            Fields::from([
                (0x78, Value::Unsigned(0)),
                (0x7c, Value::Unsigned(0)),
                (0x80, Value::Array(Vec::new())),
            ]),
        ));
        messages.extend_from_slice(content.party);
        messages.push((LOBBY, 20802, player::settings(schemas, &profile)?));
        Ok(messages)
    }

    /// The state that can be refreshed while the client is connected, after
    /// a dashboard edit: clock, menu config, card, events, Arcade,
    /// endorsements, account features, currencies, boxes, collection,
    /// competitive state and passes, then `content.party`. `live_messages`,
    /// `ow174/content/__init__.py`.
    pub fn live_messages(
        &self,
        templates: &RetailTemplates,
        collection: &Collection,
        ranked: &Ranked,
        content: LoginContent<'_>,
    ) -> io::Result<Vec<LobbyMessage>> {
        let profile = self.account.profile();
        let identity = self.identity;
        let now = server_time(&profile.server_date, content.wall_time)?;
        let record = player::record(
            &profile,
            identity,
            collection.portrait_frame(&profile),
            content.wall_time,
        );
        let mut messages = vec![
            (CONFIG, 36602, Fields::from([(0x78, Value::Unsigned(now))])),
            (
                CONFIG,
                36600,
                templates.menu_config(&profile, identity, content.menu_hero)?,
            ),
            (
                IN_CONNECT,
                20502,
                Fields::from([(0x78, Value::Struct(record))]),
            ),
        ];
        if let Some(celebrations) = content.celebrations {
            messages.extend(celebrations.login_messages(&profile, collection.items(), now)?);
        }
        let season = ranked.season(&profile);
        let arcade = content.arcade.state(&profile, season, now)?;
        messages.extend(arcade.messages);
        messages.extend(player::endorsements(&profile, identity, content.wall_time));
        messages.push((player::PERMISSIONS, 55500, player::features()));
        messages.push((PROGRESSION, 24300, collection.progression(&profile)?));
        // The client merges boxes (24302) and takes each balance and the level
        // from its own message; a 24300 sent again is not relied on for them.
        messages.push(Collection::boxes_update(&profile.loot_boxes));
        for (id, amount) in [
            (24307, profile.credits),
            (24308, profile.comp_points),
            (24309, profile.league_tokens),
        ] {
            messages.push((
                PROGRESSION,
                id,
                Fields::from([(0x78, Value::Signed(amount))]),
            ));
        }
        messages.push((
            PROGRESSION,
            24313,
            Fields::from([
                (0x78, Value::Signed(profile.level)),
                (0x80, Value::Unsigned(0)), // experience
            ]),
        ));
        messages.push((HERO_CATALOG, 24900, collection.hero_catalog(&profile)?));
        messages.extend(ranked.login_messages(
            &profile,
            &arcade.competitive_cards,
            content.leaderboard_places,
            now,
        )?);
        messages.push(passes::counts(&profile)?);
        messages.extend_from_slice(content.party);
        Ok(messages)
    }
}

fn resolve_battle_tag(accounts: &Accounts, name: &str) -> String {
    let id = account_id_for(name);
    if let Ok(Some(account)) = accounts.by_id(id) {
        return account.battle_tag();
    }
    battle_tag(name, id)
}

fn invalid(error: orpheus_wire::jam::codec::CodecError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests;
