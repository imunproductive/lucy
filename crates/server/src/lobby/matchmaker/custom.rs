//! Custom games: the lobbies players create (24000 kind 1) and find (the
//! Game Browser), join (52900, invites) and leave (52902); their seats,
//! settings and the owner's commands; their matches and the way back to
//! the lobby. The protocol is in `lucy_lobby::custom_game`; the research
//! in `refs/reports/CustomGames.md` (§11).
//!
//! A lobby lives here while it has a player. Its owner's settings uploads
//! (24008-24013) are its settings; every member gets them (23312) with
//! the match lobby's seats (53001, 53002). Start puts every seated player
//! into one match of the settings' first game the server hosts, with the
//! settings' values, teams, hero lists and Workshop; spectators and the
//! queue stay in the lobby. During the match the players keep the lobby
//! (53000 type 1 with the lobby's id, never 53003), and a game that leaves
//! the match gets the lobby again.

use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use lucy_content::maps::GameMap;
use lucy_lobby::{
    accounts::{Account, battle_tag},
    custom_game::{
        BodyRules, CustomSettings, RulesetValue, UPLOAD, Upload,
        browser::{self, Filter},
        lobby::{
            self as room, ACCEPT_INVITE, ADD_BOT, Bot, Command, DECLINE_INVITE, DETAILS_REQUEST,
            EDIT_BOT, INVITE, MAKE_OWNER, MOVE, Member, PLAYER_RIGHTS, REMOVE, RENAME, SWAP, Wish,
            byte_at, id_at,
        },
    },
    transport::LobbyMessage,
};
use lucy_script::graph::{BodyValues, ModeValue};
use lucy_sim::ai::Difficulty;
use lucy_wire::jam::codec::{Fields, Value};

use super::{Ctx, Entry, Lobby, Matchmaker, Parties, Party, Ticket, game, rules};
use crate::game_service::FlowAction;

/// The ruleset ids of custom games' matches: `0C0` GUIDs no asset has
/// (the shipped `0C0` indexes are below 0x100), one per match.
const MATCH_RULESETS: u64 = 0x0fd0_8000_0000_0000;
static NEXT_RULESET: AtomicU64 = AtomicU64::new(1);

pub(super) fn mode_value(value: RulesetValue) -> ModeValue {
    match value {
        RulesetValue::Bool(value) => ModeValue::Bool(value),
        RulesetValue::Integer(value) => ModeValue::Integer(value),
        RulesetValue::Float(value) => ModeValue::Float(f64::from(value)),
        RulesetValue::Asset(value) => ModeValue::Asset(value),
    }
}

fn body_values(body: BodyRules) -> BodyValues {
    BodyValues {
        team: body.team,
        hero: body.hero,
        values: body
            .values
            .into_iter()
            .map(|(key, value)| (key, mode_value(value)))
            .collect(),
    }
}

/// A custom game lobby and its match.
pub(super) struct CustomGame {
    pub(super) lobby: room::Lobby,
    /// The match its players are in.
    pub(super) room: Option<[u64; 2]>,
    /// Start again once the match's last game left it (Restart).
    restart: bool,
}

/// A create (24000) waiting for the creator's settings.
pub(super) struct Creating {
    name: String,
    owner: Member,
}

/// Who asks: the player's account, his card and his party.
pub(crate) struct Asker<'a> {
    pub account: &'a Arc<Account>,
    pub card: Fields,
    pub group: [u64; 2],
}

impl Asker<'_> {
    fn member(&self) -> Member {
        let id = match self.card.get(&0) {
            Some(Value::Struct(id)) => [
                id.get(&0).and_then(number).unwrap_or(0),
                id.get(&8).and_then(number).unwrap_or(0),
            ],
            _ => [u64::from(self.account.account_lo), 0],
        };
        Member::player(self.account.account_lo, id, self.card.clone(), self.group)
    }
}

fn number(value: &Value) -> Option<u64> {
    match value {
        Value::Unsigned(value) => Some(*value),
        Value::Signed(value) => u64::try_from(*value).ok(),
        _ => None,
    }
}

impl Matchmaker {
    /// 24000 kind 1: a new lobby with the player as its owner, made once
    /// his settings come: the client sends its default settings right
    /// after (`0x7FF7895DD340`). A lobby he was in, he leaves.
    pub(crate) fn create_custom_game(
        &self,
        lobby: &Lobby,
        asker: &Asker<'_>,
        name: String,
    ) -> io::Result<()> {
        let account = asker.account.account_lo;
        tracing::info!("[CG] {} creates custom game {name:?}", asker.account.name);
        self.with(lobby, |ctx| {
            ctx.leave_custom(account, "created another");
            ctx.state.creating.insert(
                account,
                Creating {
                    name,
                    owner: asker.member(),
                },
            );
        });
        Ok(())
    }

    /// 24008-24013: a part of a settings upload. Complete, it makes the
    /// lobby the player created, or it is his lobby's new settings when he
    /// owns it.
    pub(crate) fn custom_game_settings(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        id: u32,
        fields: Fields,
    ) -> io::Result<()> {
        self.with(lobby, |ctx| {
            let me = account.account_lo;
            let uploaded = ctx
                .state
                .uploads
                .entry(me)
                .or_insert_with(|| Upload::new(UPLOAD))
                .push(id, fields);
            let settings = match uploaded {
                Ok(Some(uploaded)) => uploaded.settings,
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!("[CG] Settings {id} from {}: {error}", account.name);
                    return;
                }
            };
            tracing::info!(
                "[CG] {} uploaded settings: games {:X?}, {} chunks, code {:?}",
                account.name,
                settings.games(),
                settings.chunks.len(),
                settings.code()
            );
            tracing::debug!("[CG] settings: {settings:?}");
            ctx.custom_settings(me, settings);
        });
        Ok(())
    }

    /// 24018: the owner's lobby command.
    pub(crate) fn custom_game_command(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        fields: &Fields,
    ) -> io::Result<()> {
        let command = Command::of(fields);
        self.with(lobby, |ctx| {
            let me = account.account_lo;
            let Some(id) = ctx.owned(me) else {
                tracing::info!(
                    "[CG] Lobby command {command:?} from {}: not an owner",
                    account.name
                );
                return;
            };
            tracing::info!("[CG] {}: lobby command {command:?}", account.name);
            match command {
                Some(Command::Start) => ctx.start_custom_game(id),
                Some(Command::Restart) => ctx.end_custom_match(id, true),
                Some(Command::ReturnToLobby) => ctx.end_custom_match(id, false),
                Some(Command::StartGameMode) => ctx.start_game_mode(id),
                Some(Command::Unknown2) | None => {}
            }
        });
        Ok(())
    }

    /// The lobby's requests: invites (24001, 24006, 24007), the owner's
    /// commands (24002-24005, 24014, 24017, 24019) and a lobby's details
    /// (24020).
    pub(crate) fn custom_game_request(
        &self,
        lobby: &Lobby,
        asker: &Asker<'_>,
        id: u32,
        fields: &Fields,
    ) -> io::Result<()> {
        self.with(lobby, |ctx| ctx.custom_request(asker, id, fields));
        Ok(())
    }

    /// 52900 of a custom game (type 1): JOIN in the browser, a friend's
    /// "Join game" or "Spectate". False: not a custom game's.
    pub(crate) fn join_custom_game(
        &self,
        lobby: &Lobby,
        asker: &Asker<'_>,
        fields: &Fields,
    ) -> io::Result<bool> {
        let Some(id) = id_at(fields, 0x78) else {
            return Ok(false);
        };
        let wish = Wish {
            spectator: byte_at(fields, 0x99) == 2,
            team: room::ANY_TEAM,
            slot: byte_at(fields, 0xa0),
        };
        Ok(self.with(lobby, |ctx| {
            if !ctx.state.custom.contains_key(&id) {
                return false;
            }
            ctx.join_custom(asker, id, wish);
            true
        }))
    }

    /// 52902 with the lobby id of the player's custom game: Leave Game,
    /// in the lobby or its match. False: not his custom game's.
    pub(crate) fn leave_custom_game(
        &self,
        lobby: &Lobby,
        account: &Arc<Account>,
        lobby_id: [u64; 2],
    ) -> bool {
        self.with(lobby, |ctx| {
            if ctx.state.custom_of.get(&account.account_lo) != Some(&lobby_id) {
                return false;
            }
            ctx.leave_custom(account.account_lo, "left");
            true
        })
    }

    /// 40703: the lobbies that pass the browser's filter.
    pub(crate) fn browse_custom_games(&self, fields: &Fields) -> LobbyMessage {
        let filter = Filter::of(fields);
        let state = self.lock();
        let listings: Vec<browser::Listing> = state
            .custom
            .values()
            .map(|game| game.lobby.listing())
            .collect();
        browser::results(&filter, &listings)
    }
}

impl Ctx<'_> {
    /// The lobby the account owns.
    fn owned(&self, account: u32) -> Option<[u64; 2]> {
        let id = *self.state.custom_of.get(&account)?;
        self.state.custom[&id].lobby.is_owner(account).then_some(id)
    }

    /// To every player of lobby `id` but `skip`, as one post each.
    fn post_lobby(&mut self, id: [u64; 2], messages: &[LobbyMessage], skip: Option<u32>) {
        let accounts: Vec<u32> = self.state.custom[&id]
            .lobby
            .accounts()
            .filter(|&account| Some(account) != skip)
            .collect();
        for account in accounts {
            self.post_all(account, messages.to_vec());
        }
    }

    /// The first game of `settings` that the server hosts.
    fn hosted_game(&self, settings: &CustomSettings) -> Option<(u64, u64)> {
        settings
            .games()
            .into_iter()
            .find(|&(map, mode)| self.custom_map(&[map], &[mode], false).is_some())
    }

    /// A complete settings upload of `account`.
    fn custom_settings(&mut self, account: u32, settings: CustomSettings) {
        if let Some(creating) = self.state.creating.remove(&account) {
            let id = match super::match_id() {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!("[CG] {error}");
                    return;
                }
            };
            let game = self.hosted_game(&settings);
            let mut lobby = room::Lobby::new(id, creating.name, creating.owner, settings);
            lobby.game = game.or(lobby.game);
            tracing::info!("[CG] Lobby {id:X?} made: game {:X?}", lobby.game);
            self.post_all(account, lobby.enter());
            self.state.custom_of.insert(account, id);
            self.state.custom.insert(
                id,
                CustomGame {
                    lobby,
                    room: None,
                    restart: false,
                },
            );
            return;
        }
        let Some(id) = self.owned(account) else {
            tracing::info!("[CG] Settings from a player who owns no lobby: dropped");
            return;
        };
        let game = self.hosted_game(&settings);
        let custom = self.state.custom.get_mut(&id).expect("the lobby");
        custom.lobby.set_settings(settings, game);
        let messages = custom.lobby.settings_changed();
        let playing = custom.room.is_some();
        self.post_lobby(id, &messages, None);
        if playing {
            self.post_lobby(id, &[room::notice(room::SETTINGS_NEXT_MATCH)], None);
        }
    }

    /// The player comes into lobby `id`: the lobby to him, his place to the
    /// others; into its match when it plays and he has a seat.
    fn join_custom(&mut self, asker: &Asker<'_>, id: [u64; 2], wish: Wish) {
        let me = asker.account.account_lo;
        if self.state.custom_of.get(&me) == Some(&id) {
            return;
        }
        self.leave_custom(me, "joined another");
        self.state.creating.remove(&me);
        let custom = self.state.custom.get_mut(&id).expect("the lobby");
        if !custom.lobby.join(asker.member(), wish) {
            tracing::info!("[CG] {} can't join {id:X?}: it is full", asker.account.name);
            let left = custom.lobby.left();
            self.post(me, left);
            return;
        }
        tracing::info!("[CG] {} joins {id:X?}", asker.account.name);
        let enter = custom.lobby.enter();
        let seats = custom.lobby.seats_changed();
        let team = custom
            .lobby
            .member_of(me)
            .and_then(Member::seated)
            .map(|team| if team == 2 { 0 } else { team });
        let playing = custom.room;
        self.state.custom_of.insert(me, id);
        self.state.invites.remove(&me);
        self.post_all(me, enter);
        self.post_lobby(id, &seats, Some(me));
        if let (Some(room_id), Some(team)) = (playing, team) {
            self.join_custom_match(asker.account, room_id, team);
        }
    }

    /// The player is out of his custom game: his place goes, the others
    /// see it; out of its match too. A lobby without players closes.
    pub(super) fn leave_custom(&mut self, account: u32, why: &str) {
        let Some(id) = self.state.custom_of.remove(&account) else {
            return;
        };
        let Some(custom) = self.state.custom.get_mut(&id) else {
            return;
        };
        let Some(member) = custom.lobby.member_of(account).map(|member| member.id) else {
            return;
        };
        custom.lobby.leave(member);
        tracing::info!("[CG] Account {account} out of {id:X?} ({why})");
        let playing = custom.room;
        if custom.lobby.is_empty() {
            self.state.custom.remove(&id);
            self.state.invites.retain(|_, (lobby, _)| *lobby != id);
            tracing::info!("[CG] Lobby {id:X?} closed: nobody is left");
        } else {
            let seats = custom.lobby.seats_changed();
            self.post_lobby(id, &seats, None);
        }
        if let Some(room_id) = playing
            && let Some((room, seat)) = self.state.player_of(account)
            && self.state.rooms[room].id == room_id
        {
            self.take_out(room, seat, why);
        }
    }

    fn custom_request(&mut self, asker: &Asker<'_>, request: u32, fields: &Fields) {
        let me = asker.account.account_lo;
        match request {
            DETAILS_REQUEST => {
                let Some(id) = id_at(fields, 0x78) else {
                    return;
                };
                let messages = match self.state.custom.get(&id) {
                    Some(custom) => custom.lobby.details(),
                    None => room::no_details(id),
                };
                self.post_all(me, messages);
                return;
            }
            ACCEPT_INVITE => {
                let Some(id) = id_at(fields, 0x78) else {
                    return;
                };
                let wish = match self.state.invites.remove(&me) {
                    Some((lobby, wish)) if lobby == id => wish,
                    _ => Wish::ANY,
                };
                if self.state.custom.contains_key(&id) {
                    self.join_custom(asker, id, wish);
                } else {
                    self.post(me, room::invite_gone(id));
                }
                return;
            }
            DECLINE_INVITE => {
                self.state.invites.remove(&me);
                return;
            }
            _ => {}
        }
        let Some(&id) = self.state.custom_of.get(&me) else {
            tracing::info!("[CG] {request} from {} outside a lobby", asker.account.name);
            return;
        };
        if request == INVITE {
            self.invite(asker, id, fields);
            return;
        }
        if !self.state.custom[&id].lobby.is_owner(me) {
            tracing::info!("[CG] {request} from {}: not the owner", asker.account.name);
            return;
        }
        let custom = self.state.custom.get_mut(&id).expect("the lobby");
        let lobby = &mut custom.lobby;
        let changed = match request {
            MOVE => id_at(fields, 0x78).is_some_and(|member| {
                lobby.move_to(
                    member,
                    byte_at(fields, 0x88),
                    byte_at(fields, 0x89),
                    byte_at(fields, 0x8a),
                )
            }),
            SWAP => match (id_at(fields, 0x78), id_at(fields, 0x88)) {
                (Some(a), Some(b)) => lobby.swap(a, b),
                _ => false,
            },
            ADD_BOT => {
                let bots = match fields.get(&0x78) {
                    Some(Value::Array(bots)) => bots.clone(),
                    _ => Vec::new(),
                };
                let mut added = false;
                for bot in bots {
                    let Value::Struct(bot) = bot else { continue };
                    added |= lobby.add_bot(
                        Bot {
                            hero: bot.get(&0x0).and_then(number).unwrap_or(0),
                            difficulty: byte_at(&bot, 0xc).clamp(1, 3),
                        },
                        u8::try_from(bot.get(&0x8).and_then(number).unwrap_or(3)).unwrap_or(3),
                    );
                }
                added
            }
            EDIT_BOT => match (fields.get(&0x78), id_at(fields, 0x88)) {
                (Some(Value::Struct(bot)), Some(member)) => lobby.edit_bot(
                    member,
                    Bot {
                        hero: bot.get(&0x0).and_then(number).unwrap_or(0),
                        difficulty: byte_at(bot, 0xc).clamp(1, 3),
                    },
                ),
                _ => false,
            },
            PLAYER_RIGHTS if fields.get(&0x88).and_then(number) == Some(MAKE_OWNER) => {
                id_at(fields, 0x78).is_some_and(|member| lobby.set_owner(member))
            }
            RENAME => {
                let text = match fields.get(&0x80) {
                    Some(Value::String(text)) => text.trim_end_matches('\0').to_owned(),
                    _ => String::new(),
                };
                let colour = fields.get(&0xa8).and_then(number).unwrap_or(0);
                match byte_at(fields, 0x78) {
                    1 => lobby.name = text,
                    2 => lobby.team_names[0] = text,
                    3 => lobby.team_names[1] = text,
                    4 => lobby.team_colours[0] = colour,
                    5 => lobby.team_colours[1] = colour,
                    _ => {}
                }
                true
            }
            REMOVE => {
                let Some(member) = id_at(fields, 0x78) else {
                    return;
                };
                let account = lobby.member(member).and_then(|member| member.account);
                match account {
                    Some(account) if account == me => false,
                    Some(account) => {
                        let left = lobby.left();
                        self.leave_custom(account, "removed by the owner");
                        self.post(account, left);
                        return;
                    }
                    None => lobby.leave(member).is_some(),
                }
            }
            _ => false,
        };
        if changed {
            let seats = custom.lobby.seats_changed();
            self.post_lobby(id, &seats, None);
        } else {
            tracing::info!("[CG] {request} from {} changed nothing", asker.account.name);
        }
    }

    /// 24001: 23302 to the invited player, by his account id or his
    /// BattleTag, if he is online.
    fn invite(&mut self, asker: &Asker<'_>, id: [u64; 2], fields: &Fields) {
        let by_id = id_at(fields, 0x78)
            .and_then(|invitee| u32::try_from(invitee[0]).ok())
            .filter(|&account| account != 0);
        let name = match fields.get(&0x88) {
            Some(Value::String(name)) => name.trim_end_matches('\0').to_lowercase(),
            _ => String::new(),
        };
        let invitee = by_id
            .filter(|account| self.state.online.contains_key(account))
            .or_else(|| {
                self.state.online.iter().find_map(|(&account, online)| {
                    let profile = online.account.profile();
                    (!name.is_empty()
                        && (battle_tag(&profile.player_name, account).to_lowercase() == name
                            || profile.player_name.to_lowercase() == name
                            || online.account.name.to_lowercase() == name))
                        .then_some(account)
                })
            });
        let Some(invitee) = invitee else {
            tracing::info!(
                "[CG] Invite from {}: nobody online by that id or name",
                asker.account.name
            );
            return;
        };
        let team = match fields.get(&0xb4).and_then(number) {
            Some(team @ 0..=1) => team as u8,
            _ => room::ANY_TEAM,
        };
        let wish = Wish {
            spectator: byte_at(fields, 0xb0) == 2,
            team,
            slot: byte_at(fields, 0xb8),
        };
        let invite = self.state.custom[&id].lobby.invite(&asker.card);
        self.state.invites.insert(invitee, (id, wish));
        tracing::info!("[CG] {} invites account {invitee}", asker.account.name);
        self.post(invitee, invite);
    }

    /// Start: every seated player into a match of the lobby's game.
    fn start_custom_game(&mut self, id: [u64; 2]) {
        let custom = &self.state.custom[&id];
        if custom.room.is_some() {
            tracing::info!("[CG] Start of {id:X?}: its match is on");
            return;
        }
        let lobby = custom.lobby.clone();
        let Some((map_guid, mode)) = lobby.game else {
            tracing::warn!("[CG] Start of {id:X?}: no game of its settings plays here");
            return;
        };
        let Some(mut map) = self.custom_map(&[map_guid], &[mode], false) else {
            tracing::warn!("[CG] Start of {id:X?}: {map_guid:X} in {mode:X} doesn't play here");
            return;
        };
        let rules = lobby.settings.rules(map.map_guid, map.mode_guid);
        let catalog = match self.mm.catalog() {
            Ok(catalog) => catalog,
            Err(error) => {
                tracing::warn!("[CG] The Workshop catalog: {error}");
                return;
            }
        };
        // Each team as many places as it has players and bots: the
        // bots are the AI heroes that fill the places the players leave.
        let mut sizes = vec![0u8; if rules.free_for_all { 1 } else { 2 }];
        let mut entries = Vec::new();
        let mut difficulty = 0;
        for (member, team) in lobby.players() {
            let team_index = usize::from(team).min(sizes.len() - 1);
            sizes[team_index] = sizes[team_index].saturating_add(1);
            if let Some(bot) = member.bot {
                difficulty = difficulty.max(bot.difficulty);
                continue;
            }
            let Some(online) = member
                .account
                .and_then(|account| self.state.online.get(&account))
            else {
                continue;
            };
            let account = Arc::clone(&online.account);
            let ticket = Ticket {
                party: Party {
                    id: member.group,
                    leader: account.account_lo,
                    members: vec![Arc::clone(&account)],
                },
                key: Fields::new(),
                card: 0,
                members: Vec::new(),
            };
            if let Some(entry) = self.entry(&account, &ticket, team_index, 0) {
                entries.push(entry);
            }
        }
        if entries.is_empty() {
            tracing::info!("[CG] Start of {id:X?}: no player has a seat");
            return;
        }
        map.team_sizes = sizes;
        map.hero_pools = (0..map.team_sizes.len() as u8)
            .map(|team| {
                rules
                    .hero_lists
                    .get(&Some(team))
                    .or_else(|| rules.hero_lists.get(&None))
                    .map(|list| {
                        catalog
                            .heroes()
                            .map(|(_, hero)| hero)
                            .filter(|&hero| list.allows(hero))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        let logic = match lobby.settings.workshop_text() {
            Ok(Some(text)) => lucy_workshop::parse(&text, &catalog)
                .map(|script| script.logic)
                .map_err(|error| format!("its Workshop text: {error}")),
            Ok(None) => {
                let workshop = lobby.settings.workshop();
                lucy_workshop::decode(&workshop.block, &workshop.items, &catalog)
                    .map_err(|error| format!("its Workshop: {error}"))
            }
            Err(error) => Err(format!("its Workshop text: {error}")),
        };
        let logic = match logic {
            Ok(logic) => logic,
            Err(error) => {
                tracing::warn!("[CG] Start of {id:X?}: {error}");
                return;
            }
        };
        let values = rules
            .values
            .into_iter()
            .map(|(key, value)| (key, mode_value(value)))
            .collect();
        let bodies = rules.bodies.into_iter().map(body_values).collect();
        let ai = Difficulty::from_level(difficulty);
        if let Some(room_id) = self.start_custom(entries, map, values, bodies, Arc::new(logic), ai)
            && let Some(custom) = self.state.custom.get_mut(&id)
        {
            custom.room = Some(room_id);
            custom.restart = false;
            let index = self.state.rooms.len() - 1;
            self.state.rooms[index].custom = Some(id);
        }
    }

    /// Return to Lobby (or Restart: then start again once the last game
    /// left the match): every game of the lobby's match goes back.
    fn end_custom_match(&mut self, id: [u64; 2], restart: bool) {
        let custom = self.state.custom.get_mut(&id).expect("the lobby");
        let Some(room_id) = custom.room else {
            if restart {
                self.start_custom_game(id);
            }
            return;
        };
        custom.restart = restart;
        let text = if restart {
            room::RESTARTED
        } else {
            room::BACK_TO_LOBBY
        };
        self.post_lobby(id, &[room::notice(text)], None);
        let Some(room) = self.state.rooms.iter().position(|room| room.id == room_id) else {
            return;
        };
        for seat in 0..self.state.rooms[room].seats.len() {
            let player = &self.state.rooms[room].seats[seat];
            if !player.gone && !player.released {
                self.take_out(room, seat, "back to the lobby");
                self.state.rooms[room].seats[seat].excused = true;
            }
        }
    }

    /// Start Game Mode: the match's waiting phase ends.
    fn start_game_mode(&mut self, id: [u64; 2]) {
        let Some(room_id) = self.state.custom[&id].room else {
            return;
        };
        if let Some(game) = self.lobby.game.as_deref() {
            match game.run_flow(room_id, FlowAction::SkipPhase) {
                Ok(Ok(())) => {}
                Ok(Err(refusal)) => tracing::info!("[CG] Start Game Mode: {refusal:?}"),
                Err(error) => tracing::warn!("[CG] Start Game Mode: {error}"),
            }
        }
    }

    /// A player who joined a lobby whose match is on goes into it.
    fn join_custom_match(&mut self, account: &Arc<Account>, room_id: [u64; 2], team: u8) {
        let Some(room) = self.state.rooms.iter().position(|room| room.id == room_id) else {
            return;
        };
        let ticket = Ticket {
            party: Party {
                id: [0; 2],
                leader: account.account_lo,
                members: vec![Arc::clone(account)],
            },
            key: Fields::new(),
            card: 0,
            members: Vec::new(),
        };
        let Some(entry) = self.entry(account, &ticket, usize::from(team), 0) else {
            return;
        };
        let entries = vec![entry];
        let Some((players, records)) = self.world_players(&entries, 0) else {
            return;
        };
        let Some(game) = self.lobby.game.as_deref() else {
            return;
        };
        match game::join_running(game, room_id, players) {
            Ok(Some(handoffs)) => {
                self.seat(room, &entries, &handoffs, records);
                let name = self.state.rooms[room].map.name.clone();
                self.send_to_game(entries, handoffs, &name, false);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!("[CG] Into match {room_id:X?}: {error}"),
        }
    }

    /// The custom match `room_id` has no game left: its lobby has no
    /// match; after a Restart it starts again.
    pub(super) fn custom_match_over(&mut self, lobby: [u64; 2]) {
        let Some(custom) = self.state.custom.get_mut(&lobby) else {
            return;
        };
        custom.room = None;
        if std::mem::take(&mut custom.restart) {
            self.start_custom_game(lobby);
        }
    }

    /// A player's game left the custom match of `lobby`: he is back in
    /// its lobby, which he gets again (upstream's live-tested return).
    pub(super) fn back_to_custom_lobby(&mut self, lobby: [u64; 2], account: u32) {
        if self.state.custom_of.get(&account) != Some(&lobby) {
            return;
        }
        let enter = self.state.custom[&lobby].lobby.enter();
        self.post_all(account, enter);
    }

    /// The match lobby a game of a custom match gets when it connects:
    /// its lobby's (type 1).
    pub(super) fn custom_match_lobby(&self, lobby: [u64; 2]) -> Option<LobbyMessage> {
        Some(self.state.custom.get(&lobby)?.lobby.match_lobby())
    }

    /// Open a custom match for `entries`: `map` with a ruleset of its own,
    /// whose `values` its graphs read before the schema defaults
    /// (`GraphStore::ruleset_value`) and over the mode entity's variables,
    /// with the `bodies` values its bodies get as well
    /// (`GraphStore::body_variables`), and its Workshop `logic` for the
    /// Workshop engine; with `ai`, AI heroes of that level fill the places
    /// the players leave. Its id.
    pub(super) fn start_custom(
        &mut self,
        entries: Vec<Entry>,
        mut map: GameMap,
        values: BTreeMap<u32, ModeValue>,
        bodies: Vec<BodyValues>,
        logic: Arc<lucy_workshop::Logic>,
        ai: Option<Difficulty>,
    ) -> Option<[u64; 2]> {
        for entry in &entries {
            let team = usize::from(entry.team);
            if map.team_sizes.len() <= team {
                map.team_sizes.resize(team + 1, 0);
            }
            if map.team_sizes[team] == 0 {
                map.team_sizes[team] = 1;
            }
        }
        map.ruleset = MATCH_RULESETS | NEXT_RULESET.fetch_add(1, Ordering::Relaxed);
        let graphs = self.lobby.assets.graphs();
        graphs.add_match_ruleset(map.ruleset, map.mode_guid, values.clone(), bodies.clone());
        tracing::info!(
            "[CG] {} ({}) for {} players, teams {:?}, AI {ai:?}, {} Workshop rules, ruleset values {values:?}, body values {bodies:?}",
            map.name,
            map.mode_name,
            entries.len(),
            map.team_sizes,
            logic.rules.len()
        );
        let ruleset = map.ruleset;
        if !self.start(map, entries, false, 0, &[], ai) {
            graphs.remove_match_ruleset(ruleset);
            return None;
        }
        let id = self.state.rooms.last().expect("the started room").id;
        if let Some(game) = self.lobby.game.as_deref()
            && let Err(error) = self
                .mm
                .catalog()
                .and_then(|catalog| game.attach_workshop(id, logic, catalog))
        {
            tracing::warn!("[CG] The game service did not take the Workshop: {error}");
        }
        Some(id)
    }

    /// The first of `maps` that the catalog plays in one of `modes` (in
    /// that order) that the queue rules know, or with `any_mode` in any
    /// such mode after them; no `0C0` ruleset (the mode's own values).
    pub(super) fn custom_map(
        &self,
        maps: &[u64],
        modes: &[u64],
        any_mode: bool,
    ) -> Option<GameMap> {
        let mut rulesets: Vec<_> = self
            .mm
            .rules
            .values()
            .flat_map(|rules| rules.rulesets.iter())
            .filter(|ruleset| any_mode || modes.contains(&ruleset.mode))
            .collect();
        rulesets.sort_by_key(|ruleset| {
            (
                modes
                    .iter()
                    .position(|&mode| mode == ruleset.mode)
                    .unwrap_or(usize::MAX),
                ruleset.mode,
                ruleset.ruleset,
            )
        });
        rulesets.dedup_by_key(|ruleset| ruleset.mode);
        for &map in maps {
            let maps = [(map, Vec::new())];
            for ruleset in &rulesets {
                let found =
                    rules::map_choices(&self.mm.maps, &maps, std::slice::from_ref(*ruleset));
                if let Some((mut found, _)) = found.into_iter().next() {
                    found.ruleset = 0;
                    return Some(found);
                }
            }
        }
        None
    }
}

/// The parts the handlers ask for: the account's card and party.
pub(crate) fn asker<'a>(
    lobby: &Lobby,
    account: &'a Arc<Account>,
    card: Fields,
) -> io::Result<Asker<'a>> {
    Ok(Asker {
        account,
        card,
        group: lobby.party_of(account)?.id,
    })
}
