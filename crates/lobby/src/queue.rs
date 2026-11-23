//! Queue entries and choosing roles for a role queue (Quick Play or
//! Competitive with roles). Ports `ow174/content/queue.py` and the queue entry
//! builders of `ow174/lobby/handlers/matchmaking.py`.
//!
//! What the client does (1.74, from its code and statescript graphs):
//!
//! - Picking a role queue opens the role screen (graph 01B/1FD3) and sends
//!   44100. The whole party is then in the queue (44201), and the party state
//!   (20700) carries the queue entry (+0x18) and each member's choice (+0xC8).
//!   Without the entry the role cards are grey (0x7FF789AB7350).
//! - The entry's state (+0x54) drives the screens (table 0x7FF78B5668E0, type
//!   3D.114): 1 = "wait until group members pick roles", 2 = "match search
//!   starts in N..." (+0x50 seconds), 3 = searching: the role screen closes,
//!   the "while you wait" screen opens (01B/2240) and the menu shows the search
//!   timer (01B/02DA). 0 means no role check, like no entry at all.
//! - A member whose choice has +0x68 off gets the group banner "pick a role
//!   now?" (01B/20AE): Accept sends 44104, Decline 44105. The member who started
//!   the queue is already on the role screen.
//! - Ready on the role screen sends 44103 with the member's whole role list,
//!   then 44106 with the role to spend a priority pass on. Once +0x69 is on, the
//!   button reads Change Role and sends 44107; in a group it follows with an
//!   empty 44103.
//! - A role is a number: 1 damage, 2 tank, 3 support (0x7FF78B5668D0,
//!   0x7FF78934DCB0).
//! - The entry's roles with +0x39 on are the ones a priority pass can be spent
//!   on (0x7FF789349580). A role card offers the pass only for them, and a
//!   member can spend one when every chosen role is among them
//!   (0x7FF78934A540).
//! - The role cards show estimated waits from 56200 (protocol A1498A6A), which
//!   the client keeps in its system 0x7C (0x7FF789617DC0) and redraws the cards
//!   on. A card reads the record of its role (0x7FF7899CCA20): +0x18 the wait
//!   and +0x20 the wait with a pass, in seconds; 0 hides the time. The flex
//!   card reads the record of role 0. The card rounds up to whole minutes,
//!   "< N min" up to 20 minutes (table 0x7FF78BD4FBE0).

use crate::{
    ranked::{DAMAGE, SUPPORT, TANK},
    transport::LobbyMessage,
};
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Float32, Struct, Unsigned},
};

/// What the client sends: 44100 enter, 44102 cancel, 44103 roles, 44104
/// accept, 44105 decline, 44106 priority pass role, 44107 change roles.
pub const MATCHMAKE: u32 = 0x1c6e_c712;
/// 44201 queue joined, 44202 queue left.
pub const QUEUE: u32 = 0xb4f8_bc62;
/// 56200 estimated waits of the role cards.
pub const QUEUE_WAITS: u32 = 0xa149_8a6a;
pub const ENTER_QUEUE: u32 = 44100;
pub const CANCEL_QUEUE: u32 = 44102;
pub const SET_ROLES: u32 = 44103;
pub const ACCEPT: u32 = 44104;
pub const DECLINE: u32 = 44105;
pub const PASS_ROLE: u32 = 44106;
pub const CHANGE_ROLES: u32 = 44107;
const QUEUE_JOINED: u32 = 44201;
const QUEUE_LEFT: u32 = 44202;
const WAIT_TIMES: u32 = 56200;
/// 44202 reason "UserRequest" (the client's reason names are at
/// 0x7FF789618000); the client only logs it.
const USER_REQUEST: u64 = 0;

pub const DAMAGE_ROLE: u8 = 1;
pub const TANK_ROLE: u8 = 2;
pub const SUPPORT_ROLE: u8 = 3;
/// Role number and role GUID, in the order the queue entry lists them.
pub const ROLE_NUMBERS: [(u8, u64); 3] = [
    (DAMAGE_ROLE, DAMAGE),
    (TANK_ROLE, TANK),
    (SUPPORT_ROLE, SUPPORT),
];

/// Queue entry states (+0x54).
pub const PICKING: u64 = 1;
pub const STARTING: u64 = 2;
pub const SEARCHING: u64 = 3;

/// Retail picked the roles for priority passes, and the waits, from the queues
/// of the moment. With no real queues these are fixed: damage has the long wait
/// and takes passes.
const PASS_ROLES: [u8; 1] = [DAMAGE_ROLE];
/// The role of the flex card's record.
const FLEX: u8 = 0;
const WAIT_SECONDS: [(u8, u64); 4] = [
    (FLEX, 45),
    (TANK_ROLE, 90),
    (SUPPORT_ROLE, 120),
    (DAMAGE_ROLE, 420),
];
const PASS_WAIT_SECONDS: [(u8, u64); 1] = [(DAMAGE_ROLE, 90)];

/// The role GUID of a role number.
pub fn role_guid(number: u8) -> Option<u64> {
    ROLE_NUMBERS
        .iter()
        .find(|(role, _)| *role == number)
        .map(|&(_, guid)| guid)
}

/// The role number of a role GUID.
pub fn role_number(guid: u64) -> Option<u8> {
    ROLE_NUMBERS
        .iter()
        .find(|(_, role)| *role == guid)
        .map(|&(number, _)| number)
}

/// Whether a priority pass can be spent on the role.
pub fn takes_passes(role: u8) -> bool {
    PASS_ROLES.contains(&role)
}

/// A role record of the queue entry or of 56200: `role` is the role GUID, or 0
/// for the flex card (`ow174/content/queue.py` `_role`).
fn role(role: u64, open_for_passes: bool, wait: u64, pass_wait: u64) -> Value {
    Struct(Fields::from([
        (0x0, Unsigned(role)),
        (0x8, Unsigned(0)),
        (0x10, Unsigned(0)),
        (0x18, Unsigned(wait)),
        (0x20, Unsigned(pass_wait)),
        (0x28, Unsigned(0)),
        (0x30, Unsigned(0)),
        (0x38, Bool(false)),
        (0x39, Bool(open_for_passes)),
    ]))
}

/// The party's entry for the queue with this key (the key of the 44100
/// request).
pub fn queue_entry(key: &Fields, state: u64) -> Fields {
    let roles = ROLE_NUMBERS
        .iter()
        .map(|&(number, guid)| role(guid, takes_passes(number), 0, 0))
        .collect();
    let roles = Fields::from([(0x0, Array(roles))]);
    Fields::from([
        (0x0, Struct(Fields::from([(0x0, Struct(roles))]))),
        (0x18, Struct(key.clone())),
        (0x48, Unsigned(0)),
        (0x50, Float32(0.0)),
        (0x54, Unsigned(state)),
    ])
}

/// 56200: the estimated waits the role cards show.
pub fn wait_times() -> LobbyMessage {
    let roles = WAIT_SECONDS
        .iter()
        .map(|&(number, wait)| {
            let pass_wait = PASS_WAIT_SECONDS
                .iter()
                .find(|(role, _)| *role == number)
                .map_or(0, |&(_, wait)| wait);
            role(
                role_guid(number).unwrap_or(0),
                takes_passes(number),
                wait,
                pass_wait,
            )
        })
        .collect();
    let roles = Fields::from([(0x0, Array(roles))]);
    (
        QUEUE_WAITS,
        WAIT_TIMES,
        Fields::from([(0x78, Struct(Fields::from([(0x0, Struct(roles))])))]),
    )
}

/// A party member's choice (20700 member +0xC8).
///
/// +0x0 is how many priority passes the member has for this queue, +0x8 the
/// chosen roles. The role a pass is used for is in +0x38; +0x20 and +0x50 take
/// roles out of +0x8 and +0x38 (0x7FF78934A540), and a member with a pass role
/// left makes "your team has priority search" (0x7FF789349978). +0x68 is on
/// once the member is on the role screen (no banner), +0x69 once they pressed
/// Ready.
pub fn role_choice(
    roles: &[u8],
    accepted: bool,
    ready: bool,
    passes: i64,
    pass_role: u8,
) -> Fields {
    let numbers = |roles: &[u8]| {
        Array(
            roles
                .iter()
                .map(|&role| Unsigned(u64::from(role)))
                .collect(),
        )
    };
    let pass_roles: &[u8] = if pass_role == 0 { &[] } else { &[pass_role] };
    Fields::from([
        (0x0, Value::Signed(passes)),
        (0x8, numbers(roles)),
        (0x20, Array(Vec::new())),
        (0x38, numbers(pass_roles)),
        (0x50, Array(Vec::new())),
        (0x68, Bool(accepted)),
        (0x69, Bool(ready)),
    ])
}

/// 44201: the server's entry for a queue the client asked to join. The client
/// keeps a list of these (its queue system, 0x7FF789512330); 44100 alone only
/// marks the join as pending, so without the entry the client never counts as
/// queued. The entry starts with the key of the request.
pub fn queue_joined(key: &Fields) -> LobbyMessage {
    let ticket = Fields::from([(0x0, Unsigned(0)), (0x8, Unsigned(0)), (0x10, Unsigned(0))]);
    let entry = Fields::from([
        (0x0, Struct(key.clone())),
        (0x30, Struct(ticket)),
        (0x48, Unsigned(0)),
        (0x50, Float32(0.0)),
        (0x54, Unsigned(0)),
        (0x55, Bool(false)),
        (0x56, Bool(false)),
    ]);
    (QUEUE, QUEUE_JOINED, Fields::from([(0x78, Struct(entry))]))
}

/// 44202: the client drops its entry for the queue and goes back.
pub fn queue_left(key: &Fields) -> LobbyMessage {
    (
        QUEUE,
        QUEUE_LEFT,
        Fields::from([(0x78, Struct(key.clone())), (0xa8, Unsigned(USER_REQUEST))]),
    )
}

/// The party's role queue as the party state (20700) shows it: the entry and
/// each member's choice, by account.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueFields {
    pub entry: Fields,
    pub choices: Vec<(u32, Fields)>,
}

/// Put a role queue into a built 20700: the entry under +0x18 and each
/// member's choice under +0xC8. `members` gives the account of each member
/// record, in the order the state lists them; pending invitees have none.
pub fn add_to_state(state: &mut LobbyMessage, queue: &QueueFields, members: &[Option<u32>]) {
    let Some(Struct(party)) = state.2.get_mut(&0x78) else {
        return;
    };
    party.insert(0x18, Array(vec![Struct(queue.entry.clone())]));
    let Some(Array(records)) = party.get_mut(&0x0) else {
        return;
    };
    for (record, account) in records.iter_mut().zip(members) {
        let (Struct(record), Some(account)) = (record, account) else {
            continue;
        };
        if let Some((_, choice)) = queue.choices.iter().find(|(member, _)| member == account) {
            record.insert(0xc8, Array(vec![Struct(choice.clone())]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Schemas;

    fn key() -> Fields {
        Fields::from([(
            0x0,
            Struct(Fields::from([(0x0, Unsigned(0x0630_0000_0000_00ed))])),
        )])
    }

    #[test]
    fn the_entry_offers_passes_only_for_damage() {
        let entry = queue_entry(&key(), PICKING);
        let Struct(outer) = &entry[&0x0] else {
            panic!()
        };
        let Struct(inner) = &outer[&0x0] else {
            panic!()
        };
        let Array(roles) = &inner[&0x0] else { panic!() };
        let open: Vec<(Value, Value)> = roles
            .iter()
            .map(|role| {
                let Struct(role) = role else { panic!() };
                (role[&0x0].clone(), role[&0x39].clone())
            })
            .collect();
        assert_eq!(
            open,
            [
                (Unsigned(DAMAGE), Bool(true)),
                (Unsigned(TANK), Bool(false)),
                (Unsigned(SUPPORT), Bool(false)),
            ]
        );
        assert_eq!(entry[&0x18], Struct(key()));
        assert_eq!(entry[&0x54], Unsigned(PICKING));
    }

    #[test]
    fn waits_give_the_flex_card_and_the_damage_pass_wait() {
        let (_, _, fields) = wait_times();
        let Struct(outer) = &fields[&0x78] else {
            panic!()
        };
        let Struct(inner) = &outer[&0x0] else {
            panic!()
        };
        let Array(roles) = &inner[&0x0] else { panic!() };
        let waits: Vec<_> = roles
            .iter()
            .map(|role| {
                let Struct(role) = role else { panic!() };
                (role[&0x0].clone(), role[&0x18].clone(), role[&0x20].clone())
            })
            .collect();
        assert_eq!(
            waits,
            [
                (Unsigned(0), Unsigned(45), Unsigned(0)),
                (Unsigned(TANK), Unsigned(90), Unsigned(0)),
                (Unsigned(SUPPORT), Unsigned(120), Unsigned(0)),
                (Unsigned(DAMAGE), Unsigned(420), Unsigned(90)),
            ]
        );
    }

    #[test]
    fn a_choice_lists_the_pass_role_only_when_one_is_used() {
        let choice = role_choice(&[1, 3], true, false, 2, 0);
        assert_eq!(choice[&0x8], Array(vec![Unsigned(1), Unsigned(3)]));
        assert_eq!(choice[&0x38], Array(Vec::new()));
        let choice = role_choice(&[1], true, true, 2, 1);
        assert_eq!(choice[&0x38], Array(vec![Unsigned(1)]));
        assert_eq!(
            (choice[&0x68].clone(), choice[&0x69].clone()),
            (Bool(true), Bool(true))
        );
    }

    #[test]
    fn the_queue_goes_into_the_party_state_by_member() {
        let member = |name| Struct(Fields::from([(0x0, Value::String(name))]));
        let mut state = (
            0,
            20700,
            Fields::from([(
                0x78,
                Struct(Fields::from([
                    (
                        0x0,
                        Array(vec![member("leader".into()), member("invitee".into())]),
                    ),
                    (0x18, Array(Vec::new())),
                ])),
            )]),
        );
        let queue = QueueFields {
            entry: queue_entry(&key(), SEARCHING),
            choices: vec![(7, role_choice(&[2], true, true, 0, 0))],
        };
        add_to_state(&mut state, &queue, &[Some(7), None]);
        let Struct(party) = &state.2[&0x78] else {
            panic!()
        };
        assert_eq!(party[&0x18], Array(vec![Struct(queue.entry.clone())]));
        let Array(records) = &party[&0x0] else {
            panic!()
        };
        let Struct(leader) = &records[0] else {
            panic!()
        };
        assert_eq!(
            leader[&0xc8],
            Array(vec![Struct(queue.choices[0].1.clone())])
        );
        let Struct(invitee) = &records[1] else {
            panic!()
        };
        assert!(!invitee.contains_key(&0xc8));
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn the_messages_encode_with_the_lobby_schemas() {
        let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
        for (crc, id, fields) in [queue_joined(&key()), queue_left(&key()), wait_times()] {
            schemas.encode(crc, id, &fields).unwrap();
        }
    }
}
