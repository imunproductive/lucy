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
//! `scene.rs`) with no roster slot, and the roster names the players
//! alone, as the Python reference's did. leafall903/ORPHEUS (MIT) `kjs`
//! 2f85cee `match.py` `game_struct` also put each bot in the roster (its
//! name as the record at its controller's slot, its controller in its
//! team's list), and its Tab board then had a row per bot; retail's
//! Practice Range board has none (the user, 2026-11-13), the kill feed
//! names the bots without it (`refs/reports/KillFeed.md`), and fifteen bot
//! records pushed the Range's 20300 past a datagram.

use lucy_content::maps::GameMap;
use lucy_wire::jam::codec::{
    Fields,
    Value::{Array, String, Struct, Unsigned},
};
use lucy_wire::messages::Message;

pub const RETAIL_QUANTA: u32 = 16_000;
pub const TOURNAMENT_QUANTA: u32 = 7_000;
pub const FREE_FOR_ALL_TEAM: u8 = 4;

/// The game struct's teams (`#0.#0.#0.#4`): each team's number (`+0x60`)
/// and size (`+0x61`), the map's team sizes; a free-for-all game has one
/// team, [`FREE_FOR_ALL_TEAM`], of the first size. The mode graph's "team
/// is full" (`STU_A5B9EAAD`) reads the same sizes.
pub fn team_entries(map: &GameMap) -> Vec<(u8, u8)> {
    let capacity = |team: usize| map.team_sizes.get(team).copied().unwrap_or(0);
    if map.free_for_all {
        vec![(FREE_FOR_ALL_TEAM, capacity(0))]
    } else {
        vec![(0, capacity(0)), (1, capacity(1))]
    }
}

pub struct Member<'a> {
    pub entity: u32,
    pub name: &'a str,
    pub team: u8,
    pub gone: bool,
    pub ranked: bool,
}

pub struct Instance<'a> {
    pub map: &'a GameMap,
    pub members: &'a [Member<'a>],
}

impl Instance<'_> {
    fn roster(&self, viewer: u32) -> Fields {
        let playing = || self.members.iter().filter(|member| !member.gone);
        let record = |name: &str| Struct(Fields::from([(0x50, String(name.into()))]));
        let records: Vec<_> = playing().map(|member| record(member.name)).collect();
        let team = |number, size| {
            let in_team = |team| self.map.free_for_all || team == number;
            let slots = playing()
                .filter(|member| in_team(member.team))
                .map(|member| entity(member.entity))
                .collect();
            Struct(Fields::from([
                (0x18, Array(slots)),
                (0x60, Unsigned(u64::from(number))),
                (0x61, Unsigned(u64::from(size))),
            ]))
        };
        let teams = team_entries(self.map)
            .into_iter()
            .map(|(number, size)| team(number, size))
            .collect();
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
            (0x7c, lucy_wire::jam::codec::Value::Bool(false)),
            (0x80, Unsigned(0)),
        ]),
    }
}

fn entity(id: u32) -> lucy_wire::jam::codec::Value {
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

    #[test]
    fn instance_preserves_ranked_type_after_a_member_leaves() {
        let members = members();
        let map = GameMap::practice_range();
        let instance = Instance {
            map: &map,
            members: &members,
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
        use lucy_wire::{jam::codec::Schemas, messages::encode};
        use sha2::{Digest, Sha256};

        let schemas = Schemas::game();
        let members = members();
        // Python's Range was mode 0018 with one player; ours is 005A for a
        // party (`maps.rs` `PYTHON_PRACTICE_RANGE_MODE`), so the comparison
        // takes Python's mode and team size.
        let practice_range = GameMap {
            mode_guid: lucy_content::maps::PYTHON_PRACTICE_RANGE_MODE,
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
            let load = instance.load(members[0].entity, RETAIL_QUANTA);
            let messages = [load, instance.update(members[2].entity)];
            for (message, (length, digest)) in messages.into_iter().zip(expected) {
                let bytes = encode(&schemas, message.id, &message.fields).unwrap();
                assert_eq!(bytes.len(), length);
                assert_eq!(format!("{:x}", Sha256::digest(bytes)), digest);
            }
        }
    }
}
