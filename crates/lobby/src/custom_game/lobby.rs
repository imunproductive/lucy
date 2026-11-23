//! A custom game lobby: its players and bots in their places, its owner,
//! its name and team names, its settings, and the messages that show it
//! (`refs/reports/CustomGames.md` §11.2, agent CustomGames 2026-11-23).
//!
//! What the client reads:
//! - The descriptor (23304 `+0x78`, 23310 `+0x88`): the owner's card
//!   (`+0x0`), the owner's id (`+0x68`: the client offers the owner's
//!   menus only when it is its own, `0x7FF789A960F0`), a non-zero `+0x78`
//!   (else the client thinks it left the lobby), the name (`+0x88`).
//! - The columns of the lobby screen (`0x7FF789A94BB0`) are the match
//!   lobby's roster (53002): a record's team (`+0xA8`: 0 Team 1, 1 Team 2,
//!   2 free for all, 3 spectators) and kind (`+0xAC`: 0 seated, 1 queue,
//!   2 spectator) pick its column, `+0xAF` its seat. The number of seats
//!   comes from 53001: the teams of its 312-byte block (`+0x0` `+0xC0`) and
//!   the spectators (`+0x268`). At most 24 players (`0x7FF789348800`).
//! - 53003 makes the client leave the custom lobby (system 38 slot 7): it
//!   goes only to a player who is out (left, kicked, the lobby closed).
//! - 23310 replaces the descriptor and the team list: names at `+0x20` of
//!   the entry of team `+0x18`, a colour set at `+0x10` (0: none).

use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Signed, Struct, Unsigned},
    id16,
};

use super::{
    CustomSettings, DETAILS, ENTER_COLOURS, ENTER_DESCRIPTOR, ENTER_LOBBY, browser,
    links::normalise, unsigned,
};
use crate::{lineup::MATCH_LOBBY, templates::CUSTOM_GAMES, transport::LobbyMessage};

/// Client → server requests of the lobby (protocol `0xA6E53896`).
pub const INVITE: u32 = 24001;
pub const REMOVE: u32 = 24002;
pub const ADD_BOT: u32 = 24003;
pub const EDIT_BOT: u32 = 24004;
pub const SWAP: u32 = 24005;
pub const ACCEPT_INVITE: u32 = 24006;
pub const DECLINE_INVITE: u32 = 24007;
pub const PLAYER_RIGHTS: u32 = 24014;
pub const MOVE: u32 = 24017;
pub const LOBBY_COMMAND: u32 = 24018;
pub const RENAME: u32 = 24019;
pub const DETAILS_REQUEST: u32 = 24020;

const INVITED: u32 = 23302;
const INVITE_GONE: u32 = 23303;
const UPDATED: u32 = 23310;
const NOTICE: u32 = 23319;

/// The most players and bots a lobby holds.
pub const MOST_MEMBERS: usize = 24;

/// 24014's value of "Make Owner" (`0x7FF789939C90`).
pub const MAKE_OWNER: u64 = 9;

/// 23319's texts (`07C`): "Custom game match restarted by moderator.",
/// "Custom game settings updated, will take effect next match.", "Custom
/// game sent back to lobby by a moderator." (upstream's live-tested ones).
pub const RESTARTED: u64 = 0x0de0_0000_0000_0e3b;
pub const SETTINGS_NEXT_MATCH: u64 = 0x0de0_0000_0000_0e3c;
pub const BACK_TO_LOBBY: u64 = 0x0de0_0000_0000_2b08;

/// 24018 `{u8}`: the lobby screen's buttons (`01B:0BCE`) and the in-match
/// host key (`01B:0E01`); the client checks the settings before 3 only
/// (`0x7FF7895E2080`). 1 is Start (upstream saw it from Start); the
/// others are named after their buttons' places and upstream's tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Start,
    /// Button `61C3`; its label is not known.
    Unknown2,
    Restart,
    /// "Return To Lobby", also the "ENTERING GAME" popup's cancel.
    ReturnToLobby,
    StartGameMode,
}

impl Command {
    pub fn of(fields: &Fields) -> Option<Self> {
        Some(match fields.get(&0x78).and_then(unsigned)? {
            1 => Self::Start,
            2 => Self::Unknown2,
            3 => Self::Restart,
            4 => Self::ReturnToLobby,
            5 => Self::StartGameMode,
            _ => return None,
        })
    }
}

/// Where a member is: a seat in Team 1 (0), Team 2 (1) or the free for
/// all (2); the queue for a team (3: any); a spectator's slot, watching
/// Team 1 (0), Team 2 (1) or everyone (2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Seat { team: u8, slot: u8 },
    Queue { team: u8 },
    Spectator { team: u8, slot: u8 },
}

impl Place {
    /// The roster record's team, kind and seat.
    fn record(self) -> (u8, u8, u8) {
        match self {
            Self::Seat { team, slot } => (team, 0, slot),
            Self::Queue { team } => (team, 1, 0),
            Self::Spectator { team, slot } => (team, 2, slot),
        }
    }
}

/// A bot of the lobby: its hero (0: any) and difficulty (1-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bot {
    pub hero: u64,
    pub difficulty: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// The player's account; None for a bot.
    pub account: Option<u32>,
    /// The id the client names him by: his card's `+0x0`, or a bot's.
    pub id: [u64; 2],
    /// His card (`player_messages::record`).
    pub card: Fields,
    /// His party.
    pub group: [u64; 2],
    pub place: Place,
    pub bot: Option<Bot>,
}

impl Member {
    pub fn player(account: u32, id: [u64; 2], card: Fields, group: [u64; 2]) -> Self {
        Self {
            account: Some(account),
            id,
            card,
            group,
            place: Place::Queue { team: ANY_TEAM },
            bot: None,
        }
    }

    pub fn seated(&self) -> Option<u8> {
        match self.place {
            Place::Seat { team, .. } => Some(team),
            _ => None,
        }
    }
}

/// The queue's "any team".
pub const ANY_TEAM: u8 = 3;
const FREE_FOR_ALL: u8 = 2;
/// The high half of a bot's id.
const BOT_ID: u64 = 0xb07 << 52;

/// The seats a lobby has: Team 1, Team 2, the free for all, spectators.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub seats: [u8; 3],
    pub spectators: u8,
}

/// What a join asks for (52900 `+0x99`, 24001 `+0xB0`/`+0xB4`/`+0xB8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wish {
    pub spectator: bool,
    /// 0, 1, 2 free for all; else any.
    pub team: u8,
    /// 255: any.
    pub slot: u8,
}

impl Wish {
    pub const ANY: Self = Self {
        spectator: false,
        team: ANY_TEAM,
        slot: u8::MAX,
    };
}

#[derive(Clone, Debug)]
pub struct Lobby {
    pub id: [u64; 2],
    pub name: String,
    /// The owner's id ([`Member::id`]).
    pub owner: [u64; 2],
    pub members: Vec<Member>,
    pub team_names: [String; 2],
    /// `0B3` colour sets of Team 1 and Team 2 (24019 kinds 4 and 5).
    pub team_colours: [u64; 2],
    pub settings: CustomSettings,
    /// The game its settings play that the server hosts: map and mode.
    pub game: Option<(u64, u64)>,
    /// Counts the settings and seat changes: 53001's settings id must
    /// change for the client to take a new one.
    revision: u64,
    /// 53001 `+0x2A8`: 2 shows the "ENTERING GAME" popup.
    pub state: u8,
    next_bot: u64,
}

impl Lobby {
    /// A lobby with its owner in the first free seat.
    pub fn new(id: [u64; 2], name: String, owner: Member, settings: CustomSettings) -> Self {
        let mut lobby = Self {
            id,
            name,
            owner: owner.id,
            members: Vec::new(),
            team_names: Default::default(),
            team_colours: [0; 2],
            game: settings.games().first().copied(),
            settings,
            revision: 0,
            state: 0,
            next_bot: 1,
        };
        lobby.join(owner, Wish::ANY);
        lobby
    }

    pub fn member(&self, id: [u64; 2]) -> Option<&Member> {
        self.members.iter().find(|member| member.id == id)
    }

    pub fn member_of(&self, account: u32) -> Option<&Member> {
        self.members
            .iter()
            .find(|member| member.account == Some(account))
    }

    pub fn is_owner(&self, account: u32) -> bool {
        self.member_of(account)
            .is_some_and(|member| member.id == self.owner)
    }

    /// The players' accounts.
    pub fn accounts(&self) -> impl Iterator<Item = u32> + '_ {
        self.members.iter().filter_map(|member| member.account)
    }

    /// The seats the settings give the game it plays.
    pub fn layout(&self) -> Layout {
        let spectators = self.settings.max_spectators();
        let Some((map, mode)) = self.game else {
            return Layout {
                seats: [0; 3],
                spectators,
            };
        };
        let rules = self.settings.rules(map, mode);
        let sizes = rules.team_sizes.unwrap_or_default();
        let size = |team: usize| sizes.get(team).copied().unwrap_or(0);
        Layout {
            seats: if rules.free_for_all {
                [0, 0, size(0)]
            } else {
                [size(0), size(1), 0]
            },
            spectators,
        }
    }

    fn taken(&self, place: Place, but: Option<[u64; 2]>) -> bool {
        self.members
            .iter()
            .any(|member| member.place == place && Some(member.id) != but)
    }

    /// The first free seat of `team` (or of either team for any), or the
    /// seat asked for when it is free.
    fn free_seat(&self, team: u8, slot: u8, but: Option<[u64; 2]>) -> Option<Place> {
        let layout = self.layout();
        let teams: &[u8] = match team {
            0 | 1 | FREE_FOR_ALL => &[team],
            _ => &[0, 1, FREE_FOR_ALL],
        };
        // Any team: the emptier first.
        let mut teams = teams.to_vec();
        teams.sort_by_key(|&team| self.seated_in(team));
        for team in teams {
            let size = layout.seats[usize::from(team)];
            let slots: Vec<u8> = if slot < size {
                vec![slot]
            } else if slot == u8::MAX {
                (0..size).collect()
            } else {
                Vec::new()
            };
            if let Some(place) = slots
                .into_iter()
                .map(|slot| Place::Seat { team, slot })
                .find(|&place| !self.taken(place, but))
            {
                return Some(place);
            }
        }
        None
    }

    fn free_spectator(&self, team: u8, slot: u8, but: Option<[u64; 2]>) -> Option<Place> {
        let size = self.layout().spectators;
        let team = if team < 2 { team } else { FREE_FOR_ALL };
        let occupied = |slot| {
            self.members.iter().any(|member| {
                Some(member.id) != but
                    && matches!(member.place, Place::Spectator { slot: taken, .. } if taken == slot)
            })
        };
        let slots: Vec<u8> = if slot < size {
            vec![slot]
        } else if slot == u8::MAX {
            (0..size).collect()
        } else {
            Vec::new()
        };
        slots
            .into_iter()
            .find(|&slot| !occupied(slot))
            .map(|slot| Place::Spectator { team, slot })
    }

    fn seated_in(&self, team: u8) -> usize {
        self.members
            .iter()
            .filter(|member| member.seated() == Some(team))
            .count()
    }

    /// A member comes in: to the place he wishes for, else a free seat,
    /// else the queue. False when the lobby is full.
    pub fn join(&mut self, mut member: Member, wish: Wish) -> bool {
        if self.members.len() >= MOST_MEMBERS || self.member(member.id).is_some() {
            return false;
        }
        let team = if wish.team <= FREE_FOR_ALL {
            wish.team
        } else {
            ANY_TEAM
        };
        member.place = if wish.spectator {
            self.free_spectator(ANY_TEAM, wish.slot, None)
        } else {
            self.free_seat(team, wish.slot, None)
                .or_else(|| self.free_seat(ANY_TEAM, u8::MAX, None))
        }
        .unwrap_or(Place::Queue { team });
        self.members.push(member);
        self.revision += 1;
        true
    }

    /// A member goes; the owner's place goes to the first other player.
    /// None when nobody of that id is in it.
    pub fn leave(&mut self, id: [u64; 2]) -> Option<Member> {
        let index = self.members.iter().position(|member| member.id == id)?;
        let member = self.members.remove(index);
        if member.id == self.owner
            && let Some(next) = self.members.iter().find(|member| member.account.is_some())
        {
            self.owner = next.id;
        }
        self.settle();
        self.revision += 1;
        Some(member)
    }

    /// No player is left.
    pub fn is_empty(&self) -> bool {
        self.accounts().next().is_none()
    }

    /// 24017 `{id, kind, team, slot}`: kind 0 a seat of team 0, 1 or 2
    /// (free for all), 1 the queue (team 3 or 4: any), 2 a spectator's
    /// slot (team 3: watching everyone); slot 255 any. False when the
    /// place is taken or the lobby has none.
    pub fn move_to(&mut self, id: [u64; 2], kind: u8, team: u8, slot: u8) -> bool {
        let Some(member) = self.member(id) else {
            return false;
        };
        let place = match kind {
            0 if team <= FREE_FOR_ALL => self.free_seat(team, slot, Some(id)),
            1 => Some(Place::Queue {
                team: team.min(ANY_TEAM),
            }),
            2 if member.bot.is_none() => {
                // Else watching another team from the same slot.
                let same_slot = match member.place {
                    Place::Spectator { slot, .. } => Some(Place::Spectator {
                        team: if team < 2 { team } else { FREE_FOR_ALL },
                        slot,
                    }),
                    _ => None,
                };
                self.free_spectator(team, slot, Some(id)).or(same_slot)
            }
            _ => None,
        };
        let Some(place) = place else {
            return false;
        };
        let index = self.members.iter().position(|member| member.id == id);
        self.members[index.expect("the member")].place = place;
        self.revision += 1;
        true
    }

    /// 24005: two members trade places.
    pub fn swap(&mut self, a: [u64; 2], b: [u64; 2]) -> bool {
        let (Some(first), Some(second)) = (
            self.members.iter().position(|member| member.id == a),
            self.members.iter().position(|member| member.id == b),
        ) else {
            return false;
        };
        if first == second {
            return false;
        }
        let place = self.members[first].place;
        self.members[first].place = self.members[second].place;
        self.members[second].place = place;
        self.revision += 1;
        true
    }

    /// 24003: a bot in a free seat of `team` (0, 1, 2; else any).
    pub fn add_bot(&mut self, bot: Bot, team: u8) -> bool {
        if self.members.len() >= MOST_MEMBERS {
            return false;
        }
        let Some(place) = self.free_seat(team.min(ANY_TEAM), u8::MAX, None) else {
            return false;
        };
        let id = [self.next_bot, BOT_ID];
        self.next_bot += 1;
        self.members.push(Member {
            account: None,
            id,
            card: Fields::from([(0x0, Struct(id16(id[0], id[1])))]),
            group: [0; 2],
            place,
            bot: Some(bot),
        });
        self.revision += 1;
        true
    }

    /// 24004: a bot's new hero and difficulty.
    pub fn edit_bot(&mut self, id: [u64; 2], bot: Bot) -> bool {
        let Some(member) = self
            .members
            .iter_mut()
            .find(|member| member.id == id && member.bot.is_some())
        else {
            return false;
        };
        member.bot = Some(bot);
        self.revision += 1;
        true
    }

    pub fn set_owner(&mut self, id: [u64; 2]) -> bool {
        if !self
            .members
            .iter()
            .any(|member| member.id == id && member.account.is_some())
        {
            return false;
        }
        self.owner = id;
        self.revision += 1;
        true
    }

    /// New settings, and the game the server plays from them.
    pub fn set_settings(&mut self, settings: CustomSettings, game: Option<(u64, u64)>) {
        self.settings = settings;
        self.game = game;
        self.settle();
        self.revision += 1;
    }

    /// Everyone in a place the layout has: a seat beyond its team's size
    /// goes to the queue, then the queue fills the free seats in order.
    pub fn settle(&mut self) {
        let layout = self.layout();
        for index in 0..self.members.len() {
            let fits = match self.members[index].place {
                Place::Seat { team, slot } => slot < layout.seats[usize::from(team)],
                Place::Spectator { slot, .. } => slot < layout.spectators,
                Place::Queue { .. } => true,
            };
            if !fits {
                let team = self.members[index].seated().unwrap_or(ANY_TEAM);
                self.members[index].place = Place::Queue { team };
            }
        }
        for index in 0..self.members.len() {
            if let Place::Queue { team } = self.members[index].place {
                let id = self.members[index].id;
                if let Some(place) = self.free_seat(team, u8::MAX, Some(id)) {
                    self.members[index].place = place;
                }
            }
        }
    }

    /// The members who play when the game starts: the seated ones, each
    /// with his team in the match (free for all: team 0).
    pub fn players(&self) -> impl Iterator<Item = (&Member, u8)> {
        self.members.iter().filter_map(|member| {
            member
                .seated()
                .map(|team| (member, if team == FREE_FOR_ALL { 0 } else { team }))
        })
    }

    fn owner_card(&self) -> Fields {
        self.member(self.owner)
            .map(|member| member.card.clone())
            .unwrap_or_default()
    }

    /// The descriptor (176 bytes).
    fn descriptor(&self) -> Fields {
        Fields::from([
            (0x0, Struct(self.owner_card())),
            (0x68, Struct(id16(self.owner[0], self.owner[1]))),
            (0x78, Unsigned(self.id[0].max(1))),
            (0x88, Value::String(self.name.clone())),
        ])
    }

    /// The team list (72-byte entries): the teams with a name or colour.
    fn teams(&self) -> Value {
        Array(
            (0..2)
                .filter(|&team| !self.team_names[team].is_empty() || self.team_colours[team] != 0)
                .map(|team| {
                    Struct(Fields::from([
                        (0x10, Unsigned(self.team_colours[team])),
                        (0x18, Unsigned(team as u64)),
                        (0x20, Value::String(self.team_names[team].clone())),
                    ]))
                })
                .collect(),
        )
    }

    fn lobby_id(&self) -> Value {
        Struct(Fields::from([(
            0,
            Array(vec![Unsigned(self.id[0]), Unsigned(self.id[1])]),
        )]))
    }

    /// 53000 `+0x78`: the match lobby of type 1 (CUSTOM_GAME).
    fn state_fields(&self) -> Fields {
        Fields::from([(0x30, self.lobby_id()), (0x60, Unsigned(1))])
    }

    /// 53000.
    pub fn match_lobby(&self) -> LobbyMessage {
        (
            MATCH_LOBBY,
            53000,
            Fields::from([
                (0x78, Struct(self.state_fields())),
                (0xe0, Value::Bool(false)),
                (0xe8, Unsigned(0)),
            ]),
        )
    }

    /// 53001: the block of the game it plays (its teams' sizes), the
    /// spectators, a settings id that changes with each revision (24018
    /// needs one), the map's header (the preview), the state.
    pub fn lobby_settings(&self) -> LobbyMessage {
        let block = self
            .game
            .and_then(|(map, mode)| self.settings.block(map, mode))
            .unwrap_or_default();
        let mut options = self.settings.lobby_options();
        options.insert(0x8, Unsigned(u64::from(self.layout().spectators)));
        let game = Fields::from([(0x0, Struct(block)), (0x260, Struct(options))]);
        let map = self.game.map_or(0, |(map, _)| map);
        (
            MATCH_LOBBY,
            53001,
            Fields::from([(
                0x78,
                Struct(Fields::from([
                    (0x0, Struct(game)),
                    (
                        0x278,
                        Struct(id16(self.id[0], self.id[1].wrapping_add(self.revision))),
                    ),
                    (0x288, Struct(id16(browser::map_header(map), 0))),
                    (0x2a8, Unsigned(u64::from(self.state))),
                ])),
            )]),
        )
    }

    fn records(&self) -> Vec<Value> {
        self.members
            .iter()
            .map(|member| {
                let (team, kind, slot) = member.place.record();
                // A spectator watching everyone is team 2 with kind 2;
                // team 3 with kind 2 shows the same.
                Struct(Fields::from([
                    (0x0, Struct(member.card.clone())),
                    (
                        0x68,
                        Struct(Fields::from([
                            (0x0, Array(Vec::new())),
                            (0x18, Unsigned(0)),
                            (0x20, Unsigned(0)),
                        ])),
                    ),
                    (0x90, Struct(id16(member.group[0], member.group[1]))),
                    (0xa0, Unsigned(member.bot.map_or(0, |bot| bot.hero.max(1)))),
                    (0xa8, Unsigned(u64::from(team))),
                    (0xac, Signed(i64::from(kind))),
                    (
                        0xad,
                        Signed(i64::from(member.bot.map_or(0, |bot| bot.difficulty))),
                    ),
                    (0xaf, Unsigned(u64::from(slot))),
                    (0xb1, Unsigned(0)),
                ]))
            })
            .collect()
    }

    /// 53002: everyone in his place.
    pub fn roster(&self) -> LobbyMessage {
        (
            MATCH_LOBBY,
            53002,
            Fields::from([(0x78, Array(self.records()))]),
        )
    }

    /// What puts a player in the lobby, after a create or a join:
    /// 23304-23309 (descriptor, teams, settings), then the match lobby
    /// (53000 type 1, 53001, 53002).
    pub fn enter(&self) -> Vec<LobbyMessage> {
        let header = Fields::from([
            (ENTER_DESCRIPTOR, Struct(self.descriptor())),
            (ENTER_COLOURS, self.teams()),
        ]);
        let mut messages = self.settings.transfer(ENTER_LOBBY, header);
        messages.extend([self.match_lobby(), self.lobby_settings(), self.roster()]);
        messages
    }

    /// What every player gets after the owner changed the settings: the
    /// settings (23312-23317), the match lobby's settings and roster.
    pub fn settings_changed(&self) -> Vec<LobbyMessage> {
        let mut messages = self.settings.update();
        messages.extend([self.lobby_settings(), self.roster()]);
        messages
    }

    /// What every player gets after a member came, went or moved.
    pub fn seats_changed(&self) -> Vec<LobbyMessage> {
        vec![self.updated(), self.lobby_settings(), self.roster()]
    }

    /// 23310: the descriptor (owner, name) and the team list.
    pub fn updated(&self) -> LobbyMessage {
        (
            CUSTOM_GAMES,
            UPDATED,
            Fields::from([
                (0x78, self.lobby_id()),
                (0x88, Struct(self.descriptor())),
                (0x138, self.teams()),
            ]),
        )
    }

    /// 53003 for a player who is out of the lobby: his client leaves it.
    pub fn left(&self) -> LobbyMessage {
        crate::lineup::lobby_left(self.id)
    }

    /// 23302 to an invited player: the inviter's card and the lobby.
    pub fn invite(&self, inviter: &Fields) -> LobbyMessage {
        (
            CUSTOM_GAMES,
            INVITED,
            Fields::from([(0x78, Struct(inviter.clone())), (0xe0, self.lobby_id())]),
        )
    }

    /// The browser's row of the lobby.
    pub fn listing(&self) -> browser::Listing {
        let layout = self.layout();
        let (map, mode) = self.game.unwrap_or_default();
        let heroes = self.game.and_then(|(map, mode)| {
            let rules = self.settings.rules(map, mode);
            rules
                .hero_lists
                .get(&None)
                .filter(|list| list.whitelist)
                .map(|list| list.heroes.clone())
        });
        let link = match self.settings.settings.get(&super::LINK) {
            Some(Struct(link)) => link.clone(),
            _ => Fields::new(),
        };
        browser::Listing {
            lobby: self.id,
            link,
            map,
            mode,
            heroes,
            players: self.players().count() as u8,
            spectators: self
                .members
                .iter()
                .filter(|member| matches!(member.place, Place::Spectator { .. }))
                .count() as u8,
            max_spectators: layout.spectators,
            team_max: [layout.seats[0], layout.seats[1]],
            free_for_all_max: layout.seats[2],
            name: self.name.clone(),
            host: self.owner_card(),
            code: normalise(self.settings.code()),
        }
    }

    /// 23320-23325: the details of the lobby for a player who selected it
    /// in the browser (24020): the client acts only when its state names
    /// the lobby asked for (`0x7FF7895E1DA0`).
    pub fn details(&self) -> Vec<LobbyMessage> {
        let header = Fields::from([
            (0x78, Signed(0)),
            (0x80, Struct(self.state_fields())),
            (0xe8, Struct(self.descriptor())),
            (0x4f0, Array(self.records())),
            (0x508, self.listing().entry()),
        ]);
        self.settings.transfer(DETAILS, header)
    }
}

/// 23303: the invitation to lobby `id` is gone.
pub fn invite_gone(id: [u64; 2]) -> LobbyMessage {
    (
        CUSTOM_GAMES,
        INVITE_GONE,
        Fields::from([(
            0x78,
            Struct(Fields::from([(
                0,
                Array(vec![Unsigned(id[0]), Unsigned(id[1])]),
            )])),
        )]),
    )
}

/// 23320 with a status that is not 0: the lobby asked for is gone; the
/// client drops it from the browser's list.
pub fn no_details(lobby: [u64; 2]) -> Vec<LobbyMessage> {
    let state = Fields::from([
        (
            0x30,
            Struct(Fields::from([(
                0,
                Array(vec![Unsigned(lobby[0]), Unsigned(lobby[1])]),
            )])),
        ),
        (0x60, Unsigned(1)),
    ]);
    CustomSettings::default().transfer(
        DETAILS,
        Fields::from([(0x78, Signed(1)), (0x80, Struct(state))]),
    )
}

/// 23319: a notice in the players' chat, of a `07C` text.
pub fn notice(text: u64) -> LobbyMessage {
    (
        CUSTOM_GAMES,
        NOTICE,
        Fields::from([(0x78, Unsigned(text)), (0x80, Struct(id16(0, 0)))]),
    )
}

/// An id of a request: `{u64, u64}` (`+0x0`, `+0x8`) or `{u64[2]}`.
pub fn id_at(fields: &Fields, offset: u32) -> Option<[u64; 2]> {
    let Some(Struct(id)) = fields.get(&offset) else {
        return None;
    };
    if let Some(Array(halves)) = id.get(&0) {
        return Some([
            halves.first().and_then(unsigned).unwrap_or(0),
            halves.get(1).and_then(unsigned).unwrap_or(0),
        ]);
    }
    Some([
        id.get(&0).and_then(unsigned).unwrap_or(0),
        id.get(&8).and_then(unsigned).unwrap_or(0),
    ])
}

/// A request's byte at `offset`.
pub fn byte_at(fields: &Fields, offset: u32) -> u8 {
    fields
        .get(&offset)
        .and_then(|value| match value {
            Value::Signed(value) => u8::try_from(*value).ok(),
            other => unsigned(other).and_then(|value| u8::try_from(value).ok()),
        })
        .unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_game::{Chunk, ChunkKind};
    use lucy_wire::jam::codec::Schemas;

    const HANAMURA: u64 = 0x0800_0000_0000_0165;
    const ASSAULT: u64 = 0x0230_0000_0000_0014;

    /// Hanamura Assault with teams of `sizes` and 2 spectators.
    fn settings(sizes: [u64; 2]) -> CustomSettings {
        let teams = Array(
            sizes
                .iter()
                .enumerate()
                .map(|(team, &size)| {
                    Struct(Fields::from([
                        (0x60, Unsigned(team as u64)),
                        (0x61, Unsigned(size)),
                    ]))
                })
                .collect(),
        );
        CustomSettings {
            settings: Fields::from([(0x1d0, Struct(Fields::from([(0x8, Unsigned(2))])))]),
            extra: Fields::new(),
            chunks: vec![Chunk {
                kind: ChunkKind::Map,
                fields: Fields::from([(
                    0x78,
                    Struct(Fields::from([
                        (
                            0x0,
                            Struct(Fields::from([
                                (0xb0, Unsigned(ASSAULT)),
                                (0xc0, teams),
                                (0x108, Unsigned(1)),
                            ])),
                        ),
                        (0x138, Unsigned(HANAMURA)),
                    ])),
                )]),
            }],
        }
    }

    fn player(account: u32) -> Member {
        Member::player(
            account,
            [u64::from(account), 1],
            Fields::from([(0x0, Struct(id16(u64::from(account), 1)))]),
            [0; 2],
        )
    }

    fn places(lobby: &Lobby) -> Vec<Place> {
        lobby.members.iter().map(|member| member.place).collect()
    }

    #[test]
    fn players_take_free_seats_then_spectate_or_queue() {
        let mut lobby = Lobby::new([5, 6], "Lobby".into(), player(1), settings([1, 1]));
        assert_eq!(lobby.game, Some((HANAMURA, ASSAULT)));
        assert_eq!(
            lobby.layout(),
            Layout {
                seats: [1, 1, 0],
                spectators: 2
            }
        );
        assert!(lobby.join(player(2), Wish::ANY));
        assert!(lobby.join(
            player(3),
            Wish {
                spectator: true,
                ..Wish::ANY
            }
        ));
        // Both seats taken: the queue.
        assert!(lobby.join(player(4), Wish::ANY));
        assert!(!lobby.join(player(4), Wish::ANY), "already in");
        assert_eq!(
            places(&lobby),
            [
                Place::Seat { team: 0, slot: 0 },
                Place::Seat { team: 1, slot: 0 },
                Place::Spectator { team: 2, slot: 0 },
                Place::Queue { team: ANY_TEAM },
            ]
        );
        // The owner leaves: the next player owns it and the queue takes
        // the free seat.
        assert!(lobby.leave([1, 1]).is_some());
        assert_eq!(lobby.owner, [2, 1]);
        assert_eq!(
            lobby.member([4, 1]).unwrap().place,
            Place::Seat { team: 0, slot: 0 }
        );
        // Smaller teams send the extra seat to the queue.
        lobby.set_settings(settings([1, 0]), Some((HANAMURA, ASSAULT)));
        assert_eq!(
            lobby.member([2, 1]).unwrap().place,
            Place::Queue { team: 1 }
        );
    }

    #[test]
    fn the_owner_moves_swaps_and_adds_bots() {
        let mut lobby = Lobby::new([5, 6], "Lobby".into(), player(1), settings([2, 2]));
        lobby.join(player(2), Wish::ANY);
        assert!(lobby.move_to([2, 1], 0, 0, 1));
        assert!(!lobby.move_to([2, 1], 0, 0, 0), "the owner's seat");
        assert!(lobby.move_to([2, 1], 2, 3, u8::MAX));
        assert_eq!(
            lobby.member([2, 1]).unwrap().place,
            Place::Spectator { team: 2, slot: 0 }
        );
        // Watching Team 2 only, from the same slot.
        assert!(lobby.move_to([2, 1], 2, 1, u8::MAX));
        assert_eq!(
            lobby.member([2, 1]).unwrap().place,
            Place::Spectator { team: 1, slot: 0 }
        );
        assert!(lobby.add_bot(
            Bot {
                hero: 0,
                difficulty: 2
            },
            1
        ));
        let bot = lobby.members[2].id;
        assert_eq!(lobby.members[2].place, Place::Seat { team: 1, slot: 0 });
        assert!(lobby.swap([1, 1], bot));
        assert_eq!(
            lobby.member([1, 1]).unwrap().place,
            Place::Seat { team: 1, slot: 0 }
        );
        assert!(!lobby.move_to(bot, 2, 3, u8::MAX), "a bot doesn't spectate");
        assert!(!lobby.set_owner(bot));
        assert!(lobby.set_owner([2, 1]));
        assert!(lobby.is_owner(2) && !lobby.is_owner(1));
        let players: Vec<_> = lobby
            .players()
            .map(|(member, team)| (member.id, team))
            .collect();
        assert_eq!(players, [([1, 1], 1), (bot, 0)]);
    }

    #[test]
    fn its_messages_encode_and_show_each_place() {
        let schemas = Schemas::lobby();
        let mut lobby = Lobby::new([5, 6], "Lobby".into(), player(1), settings([1, 1]));
        lobby.join(
            player(2),
            Wish {
                spectator: true,
                ..Wish::ANY
            },
        );
        lobby.team_names[1] = "Red".into();
        let first = lobby.lobby_settings();
        lobby.join(player(3), Wish::ANY);
        let second = lobby.lobby_settings();
        // Each change gives 53001 a new settings id.
        assert_ne!(first.2, second.2);
        let messages: Vec<LobbyMessage> = lobby
            .enter()
            .into_iter()
            .chain(lobby.settings_changed())
            .chain(lobby.seats_changed())
            .chain(lobby.details())
            .chain(no_details([1, 2]))
            .chain([
                lobby.left(),
                lobby.invite(&Fields::new()),
                invite_gone([5, 6]),
                notice(RESTARTED),
            ])
            .collect();
        for (crc, id, fields) in &messages {
            schemas
                .encode(*crc, *id, fields)
                .unwrap_or_else(|error| panic!("{id}: {error:?}"));
        }
        let ids: Vec<u32> = lobby.enter().iter().map(|(_, id, _)| *id).collect();
        assert_eq!(ids, [23304, 23305, 53000, 53001, 53002]);
        let Array(records) = &lobby.roster().2[&0x78] else {
            panic!()
        };
        let places: Vec<(Value, Value, Value)> = records
            .iter()
            .map(|record| {
                let Struct(record) = record else { panic!() };
                (
                    record[&0xa8].clone(),
                    record[&0xac].clone(),
                    record[&0xaf].clone(),
                )
            })
            .collect();
        assert_eq!(
            places,
            [
                (Unsigned(0), Signed(0), Unsigned(0)),
                (Unsigned(2), Signed(2), Unsigned(0)),
                (Unsigned(1), Signed(0), Unsigned(0)),
            ]
        );
        let Struct(descriptor) = &lobby.updated().2[&0x88] else {
            panic!()
        };
        assert_eq!(descriptor[&0x68], Struct(id16(1, 1)));
        let Array(teams) = &lobby.updated().2[&0x138] else {
            panic!()
        };
        assert_eq!(teams.len(), 1);
        let listing = lobby.listing();
        assert_eq!((listing.players, listing.spectators), (2, 1));
        assert_eq!(listing.team_max, [1, 1]);
    }

    #[test]
    fn requests_name_their_ids_either_way() {
        let pair = Fields::from([(0x78, Struct(id16(3, 4)))]);
        let array = Fields::from([(
            0x78,
            Struct(Fields::from([(0, Array(vec![Unsigned(3), Unsigned(4)]))])),
        )]);
        assert_eq!(id_at(&pair, 0x78), Some([3, 4]));
        assert_eq!(id_at(&array, 0x78), Some([3, 4]));
        assert_eq!(id_at(&pair, 0x80), None);
        assert_eq!(
            Command::of(&Fields::from([(0x78, Unsigned(1))])),
            Some(Command::Start)
        );
        assert_eq!(Command::of(&Fields::from([(0x78, Unsigned(9))])), None);
    }
}
