//! Match entry and roster messages shared by team modes and free-for-all.
//!
//! Client research (1.74): 20300 is "load this map with this mode". Its
//! +0x500 must equal the client's command frame length in microseconds
//! (0x7FF7896E94B0), else error 262445 (tournament mode uses its own
//! tank_TournamentModeTickRate); +0x519 must match a byte that is 0 (error
//! 262455). +0x78 is the instance type (world system 0x38 +0x18): with 9
//! (ranked, on a competitive card) the team list shows each player's rank
//! (component 29) in place of level and frame (0x7FF789A15CC9,
//! 0x7FF789A1D8E0). The 1064-byte game struct of 20300 and 20301 holds the
//! mode, the players and their teams; a team's +0x60 is its number and +0x61
//! its size, from which the hero select team list takes its empty slots
//! (0x7FF789CADB40). ClientInGame 20308 `{4 flags, card}` is a player's card
//! for the client's player list, keyed by the id in his entity's component 29
//! (0x7FF7896E88B0); with the first and fourth flag on it also posts "NAME
//! joined the game." (or left, or started spectating) in the chat. The client
//! keeps a card unless it has one with a newer +0x30 for the same id
//! (0x7FF789529C90). A card is about 90 bytes.
//!
//! The training bots' stand-in controllers are participants (see
//! `scene.rs`). leafall903/ORPHEUS (MIT) `kjs` 2f85cee `match.py`
//! `game_struct`, the one setup seen naming a bot in the kill feed, also
//! put each bot in the roster: its name as the record at its roster slot
//! (its component 74's) and its controller in its team's list; its Tab
//! board then had a tile per bot. Retail's Practice Range shows no bots
//! there (the user, 2026-11-13), and fifteen bot records push the Range's
//! 20300 past a datagram, so the roster names the players alone unless
//! [`BOTS_IN_ROSTER`] is on. Our stand-in names the bot in its own
//! component 29, which kjs's had not; whether the kill feed needs the
//! roster too is not known (the IDA list, kill feed bots). The Python
//! reference had no bots in the roster at all.

use orpheus_content::maps::GameMap;
use orpheus_wire::jam::codec::{
    Fields,
    Value::{Array, Bool, String, Struct, Unsigned},
};
use orpheus_wire::messages::Message;

pub const RETAIL_QUANTA: u32 = 16_000;
pub const TOURNAMENT_QUANTA: u32 = 7_000;
pub(crate) const FREE_FOR_ALL_TEAM: u8 = 4;
/// A debugging switch: `ORPHEUS_DEBUG_BOTS_IN_ROSTER=1` names the training
/// bots in the 20300/20301 roster as kjs did (see the module notes), for an
/// in-game check of whether the kill feed needs it. Off by default:
/// retail's Tab board shows no bots. With it on, a roster too big for one
/// datagram refuses the match.
pub static BOTS_IN_ROSTER: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    let on = std::env::var("ORPHEUS_DEBUG_BOTS_IN_ROSTER").is_ok_and(|text| text.trim() == "1");
    if on {
        tracing::warn!("debugging: the roster names the training bots");
    }
    on
});

pub struct Member<'a> {
    pub entity: u32,
    pub name: &'a str,
    pub team: u8,
    pub gone: bool,
    pub ranked: bool,
}

/// A training bot in the roster: its stand-in controller, its name and
/// team, and its roster slot (`scene::bot_roster_slot`).
pub struct RosterBot<'a> {
    pub controller: u32,
    pub name: &'a str,
    pub team: u8,
    pub slot: usize,
}

pub struct Instance<'a> {
    pub map: &'a GameMap,
    pub members: &'a [Member<'a>],
    /// The training bots, by roster slot.
    pub bots: &'a [RosterBot<'a>],
    /// Whether the roster names the bots ([`BOTS_IN_ROSTER`]).
    pub list_bots: bool,
}

impl Instance<'_> {
    fn roster(&self, viewer: u32) -> Fields {
        let playing = || self.members.iter().filter(|member| !member.gone);
        let record = |name: &str| Struct(Fields::from([(0x50, String(name.into()))]));
        let mut records: Vec<_> = playing().map(|member| record(member.name)).collect();
        let listed: &[RosterBot<'_>] = if self.list_bots { self.bots } else { &[] };
        // A slot a player holds keeps his record; a slot nobody has is
        // empty.
        for bot in listed {
            if bot.slot >= records.len() {
                records.resize(bot.slot, Struct(Fields::new()));
                records.push(record(bot.name));
            }
        }
        let team = |number, size| {
            let in_team = |team| self.map.free_for_all || team == number;
            let slots = playing()
                .filter(|member| in_team(member.team))
                .map(|member| entity(member.entity))
                .chain(
                    listed
                        .iter()
                        .filter(|bot| in_team(bot.team))
                        .map(|bot| entity(bot.controller)),
                )
                .collect();
            Struct(Fields::from([
                (0x18, Array(slots)),
                (0x60, Unsigned(u64::from(number))),
                (0x61, Unsigned(u64::from(size))),
            ]))
        };
        let capacity = |team| self.map.team_sizes.get(team).copied().unwrap_or(0);
        let teams = if self.map.free_for_all {
            vec![team(FREE_FOR_ALL_TEAM, capacity(0))]
        } else {
            vec![team(0, capacity(0)), team(1, capacity(1))]
        };
        let roster = Fields::from([
            (0, Struct(Fields::from([(0, Array(records))]))),
            (0xb0, Unsigned(self.map.mode_guid)),
            (0xc0, Array(teams)),
            (0xd8, Array(vec![entity(viewer)])),
        ]);
        Fields::from([(
            0,
            Struct(Fields::from([(0, Struct(roster)), (0x274, Unsigned(16))])),
        )])
    }

    pub fn load(&self, viewer: u32, quanta: u32) -> Message {
        // A competitive roster keeps its instance type after a ranked player leaves.
        let ranked = self.members.iter().any(|member| member.ranked);
        Message {
            id: 20300,
            fields: Fields::from([
                (0x78, Unsigned(if ranked { 9 } else { 1 })),
                (0x80, Struct(self.roster(viewer))),
                (0x4a8, Unsigned(self.map.map_guid)),
                (0x500, Unsigned(u64::from(quanta))),
                (
                    0x508,
                    Struct(Fields::from([(0, Unsigned(1)), (8, Unsigned(1))])),
                ),
                // Use the session's team colour table (lobby 23310) for this
                // world; without it the HUD stays transparent.
                (0x518, Bool(true)),
            ]),
        }
    }

    pub fn update(&self, viewer: u32) -> Message {
        Message {
            id: 20301,
            fields: Fields::from([(0x78, Struct(self.roster(viewer)))]),
        }
    }
}

pub fn ownership(viewer: u32) -> Message {
    Message {
        id: 20302,
        fields: Fields::from([
            (0x78, entity(viewer)),
            (0x7c, orpheus_wire::jam::codec::Value::Bool(false)),
            (0x80, Unsigned(0)),
        ]),
    }
}

fn entity(id: u32) -> orpheus_wire::jam::codec::Value {
    Struct(Fields::from([(0, Unsigned(u64::from(id)))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> [Member<'static>; 3] {
        [
            Member {
                entity: 0x1000,
                name: "Blue",
                team: 0,
                gone: false,
                ranked: false,
            },
            Member {
                entity: 0x2000,
                name: "Retired",
                team: 1,
                gone: true,
                ranked: true,
            },
            Member {
                entity: 0x3000,
                name: "Red",
                team: 1,
                gone: false,
                ranked: false,
            },
        ]
    }

    /// The roster names the players alone by default (module notes); with
    /// the debugging switch each bot's name is the record at its roster
    /// slot (a slot a player holds keeps his record, a slot nobody has is
    /// empty) and each team's list names its bots after its players.
    #[test]
    fn the_roster_names_the_bots_at_their_slots_only_when_asked() {
        let members = members();
        let map = GameMap::practice_range();
        let bot = |controller, name, team, slot| RosterBot {
            controller,
            name,
            team,
            slot,
        };
        // "Red" (records: Blue, Red; "Retired" left) holds slot 1.
        let bots = [
            bot(0x8000_8000, "Taken", 1, 1),
            bot(0x8000_8001, "Friendly Bot", 0, 2),
            bot(0x8000_8002, "Training Bot", 1, 4),
        ];
        let roster = |list_bots| {
            let instance = Instance {
                map: &map,
                members: &members,
                bots: &bots,
                list_bots,
            };
            let Struct(outer) = &instance.update(0x1000).fields[&0x78] else {
                panic!("no roster");
            };
            let Struct(game) = &outer[&0] else {
                panic!("no game struct");
            };
            let Struct(roster) = &game[&0] else {
                panic!("no roster struct");
            };
            let Struct(names) = &roster[&0] else {
                panic!("no records");
            };
            let Array(teams) = &roster[&0xc0] else {
                panic!("no teams");
            };
            let listed: Vec<_> = teams
                .iter()
                .map(|team| {
                    let Struct(team) = team else {
                        panic!("not a team");
                    };
                    team[&0x18].clone()
                })
                .collect();
            (names[&0].clone(), listed)
        };
        let record = |name: &str| Struct(Fields::from([(0x50, String(name.into()))]));
        let records = Array(vec![
            record("Blue"),
            record("Red"),
            record("Friendly Bot"),
            Struct(Fields::new()),
            record("Training Bot"),
        ]);
        assert_eq!(
            roster(false),
            (
                Array(vec![record("Blue"), record("Red")]),
                vec![Array(vec![entity(0x1000)]), Array(vec![entity(0x3000)])]
            )
        );
        assert_eq!(
            roster(true),
            (
                records,
                vec![
                    Array(vec![entity(0x1000), entity(0x8000_8001)]),
                    Array(vec![
                        entity(0x3000),
                        entity(0x8000_8000),
                        entity(0x8000_8002)
                    ]),
                ]
            )
        );
    }

    #[test]
    fn instance_preserves_ranked_type_after_a_member_leaves() {
        let members = members();
        let map = GameMap::practice_range();
        let instance = Instance {
            map: &map,
            members: &members,
            bots: &[],
            list_bots: false,
        };
        let initial = instance.load(members[0].entity, TOURNAMENT_QUANTA);
        assert_eq!(initial.fields[&0x78], Unsigned(9));
        assert_eq!(initial.fields[&0x500], Unsigned(7000));
        assert_eq!(
            instance.update(members[0].entity).fields[&0x78],
            initial.fields[&0x80]
        );
        let active_only = Instance {
            members: &members[..1],
            ..instance
        };
        assert_eq!(
            active_only.load(members[0].entity, RETAIL_QUANTA).fields[&0x78],
            Unsigned(1)
        );
    }

    #[test]
    fn map_load_and_roster_updates_match_python_for_teams_and_free_for_all() {
        use orpheus_wire::{jam::codec::Schemas, messages::encode};
        use sha2::{Digest, Sha256};

        let schemas = Schemas::game();
        let members = members();
        // Python's Range was mode 0018 with one player; ours is 005A for a
        // party (`maps.rs` `PYTHON_PRACTICE_RANGE_MODE`), so the comparison
        // takes Python's mode and team size.
        let practice_range = GameMap {
            mode_guid: orpheus_content::maps::PYTHON_PRACTICE_RANGE_MODE,
            team_sizes: vec![1, 0],
            ..GameMap::practice_range()
        };
        let free_for_all = GameMap {
            map_guid: 12,
            mode_guid: 34,
            team_sizes: vec![8],
            free_for_all: true,
            ..GameMap::practice_range()
        };
        let cases = [
            (
                Instance {
                    map: &practice_range,
                    members: &members,
                    bots: &[],
                    list_bots: false,
                },
                [
                    (
                        513,
                        "96c8620fdd4430e82bdf363c9f1d07dd405581ad7ffd68bda00e13926a9374c3",
                    ),
                    (
                        438,
                        "8b2d19c3c9379c478ce04590af1633e8882ddde0fe26ced16112d1a5dcb62ee9",
                    ),
                ],
            ),
            (
                Instance {
                    map: &free_for_all,
                    members: &members,
                    bots: &[],
                    list_bots: false,
                },
                [
                    (
                        494,
                        "4dbcdc55773344208c6cb95ea98d4d8efda57add102a373e4314b4547babe923",
                    ),
                    (
                        419,
                        "59330c72bf73f50b987fe7fe59cb4607e486f4582add1704cec5f035df24c0bf",
                    ),
                ],
            ),
        ];
        for (instance, expected) in cases {
            let mut load = instance.load(members[0].entity, RETAIL_QUANTA);
            // The team colour switch is this server's addition to the Python
            // reference's welcome.
            assert_eq!(load.fields.remove(&0x518), Some(Bool(true)));
            let messages = [load, instance.update(members[2].entity)];
            for (message, (length, digest)) in messages.into_iter().zip(expected) {
                let bytes = encode(&schemas, message.id, &message.fields).unwrap();
                assert_eq!(bytes.len(), length);
                assert_eq!(format!("{:x}", Sha256::digest(bytes)), digest);
            }
        }
    }
}
