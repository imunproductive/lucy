//! Party state payloads. Membership, invitations and matchmaking remain caller state.
//!
//! Client research (1.74) for the 20700 member record:
//!
//! - +0xE0 is the membership, not the away/busy status (that comes from
//!   presence). Names from the client's party debug print (0x7FF789761103):
//!   0 stranger, 1 invitee (accept), 2-4 invitee (transfer info, ticket,
//!   owner), 5 member, 6-7 leaving. The client checks 5-7 and +0xE2 for
//!   "am I the leader" (0x7FF7895443B0) and turns it into MemberStatus for
//!   the party panel (0x7FF789544370). Only the leader has +0xE2; with it on
//!   everyone, all showed as leader.
//! - An invitee that has not answered shows as a pending tile with a green
//!   bar that runs down (PendingInviteProgressBar of tile 05E/005E). +0xB8 is
//!   the time left in ms: the client adds its own clock to it when the party
//!   state arrives (0x7FF7895444D4), and the bar shows the deadline minus that
//!   clock while under 25 s (0x7FF7895442E0).
//! - The party's "members can invite" flag is read by 0x7FF789D2EE70
//!   (Social.members_may_invite).

use std::io;

use crate::{
    accounts::Profile,
    collection::Collection,
    login::Identity,
    player_messages::{endorsement, record},
    ranked::PC_PLATFORM,
    transport::LobbyMessage,
};
use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Struct, Unsigned},
    id16,
};

pub const PARTY: u32 = 0x2411_de56;
const STATE: u32 = 20700;
const INVITEE: u64 = 1;
const MEMBER: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Membership {
    Leader,
    Member,
    Invitee { remaining_ms: u64 },
}

pub struct PartyMember<'a> {
    pub profile: &'a Profile,
    pub identity: Identity,
    pub menu_hero: u64,
    pub ratings: &'a [Value],
    pub membership: Membership,
}

/// Build 20700 with members in caller-supplied order, usually leader first
/// and pending invitees last. Ratings are prepared by the competitive service.
pub fn state(
    collection: &Collection,
    party_id: [u64; 2],
    entity: [u64; 2],
    members: &[PartyMember<'_>],
    members_may_invite: bool,
    now: u64,
) -> io::Result<LobbyMessage> {
    let members = members
        .iter()
        .map(|member| member.fields(collection, now).map(Struct))
        .collect::<io::Result<_>>()?;
    let state = Fields::from([
        (0x00, Array(members)),
        (0x18, Array(Vec::new())),
        (0x30, Array(Vec::new())),
        (0x48, Array(Vec::new())),
        (0x60, Struct(id16(party_id[0], party_id[1]))),
        (0x70, Struct(id16(entity[0], entity[1]))),
        (
            0x80,
            Struct(Fields::from([(0, Array(vec![Unsigned(0), Unsigned(0)]))])),
        ),
        (0x90, Unsigned(PC_PLATFORM)),
        (0x94, Bool(members_may_invite)),
        (0x95, Unsigned(1)),
        (0x96, Bool(true)),
        (0x98, Bool(true)),
    ]);
    Ok((PARTY, STATE, Fields::from([(0x78, Struct(state))])))
}

impl PartyMember<'_> {
    fn fields(&self, collection: &Collection, now: u64) -> io::Result<Fields> {
        let card = record(
            self.profile,
            self.identity,
            collection.portrait_frame(self.profile),
            now,
        );
        let (hero_or_timer, membership, hero) = match self.membership {
            Membership::Leader | Membership::Member => (self.menu_hero, MEMBER, self.menu_hero),
            Membership::Invitee { remaining_ms } => (remaining_ms, INVITEE, 0),
        };
        let skin = if hero == 0 {
            Unsigned(0)
        } else {
            collection
                .loadout(self.profile, hero)?
                .remove(&0x38)
                .unwrap_or(Unsigned(0))
        };
        Ok(Fields::from([
            (0x00, Struct(card)),
            (0x68, Array(self.ratings.to_vec())),
            (0x80, Struct(endorsement(self.profile.endorsement_level))),
            (0xa0, Array(Vec::new())),
            // Invite tiles reuse the hero field for the number of milliseconds left.
            (0xb8, Unsigned(hero_or_timer)),
            (0xc0, skin),
            (0xc8, Array(Vec::new())),
            (0xe0, Unsigned(membership)),
            (0xe1, Unsigned(0)),
            (0xe2, Bool(self.membership == Membership::Leader)),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use sha2::{Digest, Sha256};

    use crate::templates::RetailTemplates;
    use lucy_content::catalog::ItemDb;
    use lucy_wire::jam::codec::Schemas;

    #[test]
    fn pending_tiles_use_a_timer_and_only_the_leader_has_the_flag() {
        let templates = RetailTemplates::from_reader(
            br#"{"format":1,"server":[
                ["70E5A247",24900,{"+0x80":[],"+0x98":[]}],
                ["779A8581",24300,{"+0x78":{"+0x30":[]},"+0xF8":{"+0x0":[]}}]
            ]}"#
            .as_slice(),
        )
        .unwrap();
        let collection = Collection::new(&templates, Arc::new(ItemDb::default())).unwrap();
        let profile = Profile::default();
        let members = [
            PartyMember {
                profile: &profile,
                identity: Identity::for_account(1),
                menu_hero: 13,
                ratings: &[],
                membership: Membership::Leader,
            },
            PartyMember {
                profile: &profile,
                identity: Identity::for_account(2),
                menu_hero: 13,
                ratings: &[],
                membership: Membership::Invitee {
                    remaining_ms: 9_500,
                },
            },
        ];
        let (group, id, fields) = state(&collection, [1, 2], [3, 4], &members, false, 10).unwrap();
        assert_eq!((group, id), (PARTY, STATE));
        let Struct(state) = &fields[&0x78] else {
            panic!("expected party state");
        };
        assert_eq!(state[&0x60], Struct(id16(1, 2)));
        assert_eq!(state[&0x70], Struct(id16(3, 4)));
        assert_eq!(state[&0x94], Bool(false));
        let Array(members) = &state[&0] else {
            panic!("expected party members");
        };
        let [Struct(leader), Struct(invitee)] = members.as_slice() else {
            panic!("expected a leader and an invitee");
        };
        assert_eq!(leader[&0xb8], Unsigned(13));
        assert_eq!(leader[&0xe0], Unsigned(MEMBER));
        assert_eq!(leader[&0xe2], Bool(true));
        assert_eq!(invitee[&0xb8], Unsigned(9_500));
        assert_eq!(invitee[&0xc0], Unsigned(0));
        assert_eq!(invitee[&0xe0], Unsigned(INVITEE));
        assert_eq!(invitee[&0xe2], Bool(false));
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn solo_and_group_party_payloads_match_the_python_wire_bytes() {
        let runtime = lucy_testkit::runtime_dir().join("data");
        let schemas = Schemas::lobby();
        let templates = RetailTemplates::captured();
        let items = Arc::new(ItemDb::load(&runtime).unwrap());
        let reaper = items.hero_by_name("Reaper").unwrap();
        let collection = Collection::new(&templates, items).unwrap();
        let mut lead = Profile {
            player_name: "Lead".into(),
            level: 3000,
            unlock_all: true,
            ..Profile::default()
        };
        let second = Profile {
            player_name: "Member".into(),
            ..Profile::default()
        };
        let pending = Profile {
            player_name: "Invitee".into(),
            ..Profile::default()
        };
        assert!(
            collection
                .equip(&mut lead, reaper, 0x0250_0000_0000_4f53, 0)
                .unwrap()
        );
        let rating = [Struct(Fields::from([
            (
                0,
                Array(vec![Struct(Fields::from([
                    (0x00, Unsigned(0x0d80_0000_0000_0850)),
                    (0x08, Unsigned(PC_PLATFORM)),
                    (0x0c, Unsigned(16_025_156)),
                    (0x10, Unsigned(2400)),
                    (0x12, Unsigned(0)),
                    (0x14, Unsigned(2400)),
                    (0x16, Unsigned(2)),
                    (0x17, Bool(false)),
                    (0x18, Unsigned(2)),
                ]))]),
            ),
            (0x18, Unsigned(0x0630_0000_0000_01b3)),
            (0x20, Unsigned(0)),
        ]))];
        let members = [
            PartyMember {
                profile: &lead,
                identity: Identity::for_account(42),
                menu_hero: reaper,
                ratings: &rating,
                membership: Membership::Leader,
            },
            PartyMember {
                profile: &second,
                identity: Identity::for_account(43),
                menu_hero: 0,
                ratings: &rating,
                membership: Membership::Member,
            },
            PartyMember {
                profile: &pending,
                identity: Identity::for_account(44),
                menu_hero: 0,
                ratings: &rating,
                membership: Membership::Invitee {
                    remaining_ms: 21_234,
                },
            },
        ];
        for (members, may_invite, expected) in [
            (
                &members[..1],
                true,
                "5d8fdeb3c7b7aed448296ea9ec9dad81791a243fcb91617e91116d1c00cc5196",
            ),
            (
                members.as_slice(),
                false,
                "0d8d918eeac3b094be385e4288968c9afd0d1e3dfdfdd967452a858c172c30f1",
            ),
        ] {
            let (crc, id, fields) = state(
                &collection,
                [0x1234, 0x2345],
                [0x0100_1d00_0000_0033, 0x4444],
                members,
                may_invite,
                1_700_000_000,
            )
            .unwrap();
            let mut wire = schemas.header(crc, 1, id).unwrap();
            wire.extend(schemas.encode(crc, id, &fields).unwrap());
            assert_eq!(format!("{:x}", Sha256::digest(wire)), expected);
        }
    }
}
