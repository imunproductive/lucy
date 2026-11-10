//! The group finder's search filters and rating spread
//! (`ow174/services/social.py` matches_filters, _group_rating).

use std::io;

use orpheus_wire::jam::codec::Fields;

use super::{
    ANY_SLOT, DAMAGE_SLOT, Party, SUPPORT_SLOT, TANK_SLOT, array, integer, structure, text,
    unsigned,
};
use crate::{
    accounts::Profile,
    ranked::{COMPETITIVE_CTF, LUCIO_CUP, Queue, rating_of},
};

/// Listing +0x76 of a competitive group.
const COMPETITIVE_GROUP: i64 = 2;
/// A competitive Arcade card's group (the Lucio Cup), the card in +0x68.
const ARCADE_COMPETITIVE_GROUP: i64 = 3;
/// Search filter -> listing flag: roles assigned, voice chat. 0 any, 1 no,
/// 2 yes.
const YES_NO_FILTERS: [(u32, u32); 2] = [(0xd, 0x77), (0xe, 0x78)];
/// Search filter -> the free slots of that type it asks for.
const FREE_SLOT_FILTERS: [(u32, u64); 3] =
    [(0x12, TANK_SLOT), (0x13, DAMAGE_SLOT), (0x14, SUPPORT_SLOT)];

/// The rating a competitive group's spread compares: the best role rating,
/// or the rating on the group's competitive Arcade card. None for other
/// groups.
pub fn group_rating(profile: &Profile, listing: &Fields) -> io::Result<Option<u16>> {
    let game_type = integer(listing.get(&0x76));
    if game_type == COMPETITIVE_GROUP {
        let mut best = 0;
        for queue in [Queue::Tank, Queue::Damage, Queue::Support] {
            best = best.max(rating_of(profile, queue)?);
        }
        return Ok(Some(best));
    }
    let queue = match unsigned(listing.get(&0x68)) {
        COMPETITIVE_CTF => Some(Queue::Ctf),
        LUCIO_CUP => Some(Queue::Lucio),
        _ => None,
    };
    match queue {
        Some(queue) if game_type == ARCADE_COMPETITIVE_GROUP => rating_of(profile, queue).map(Some),
        _ => Ok(None),
    }
}

/// Whether a listed group passes the filters of a search (52200 +0x78): +0xC
/// game type (+0x0 its competitive Arcade card), +0xD roles assigned, +0xE
/// voice chat, +0xF minimum endorsement the group asks for, +0x10 minimum
/// players, +0x12/+0x13/+0x14 free tank/damage/support slots, +0x15 free
/// slots of any role, +0x18 slot types that must be free, +0x30 texts to find
/// in the name or the creator (filter reader 0x7FF7899B1E00). +0x11 is "only
/// players of my platform" (forced on for competitive games, hidden without
/// crossplay, 0x7FF789AA43A0); every player here is on PC, so it filters
/// nothing. The client does not filter the found groups itself. The filters
/// were found by changing one at a time on the client's filter screen
/// (05A/0668): +0x8 is the row picked in the game type list (0 any, 1 quick
/// play, 2 competitive, 3 arcade, 4 versus AI) and +0xC that type as listings
/// carry it in +0x76 (0, 1, 2, 4, 5); roles assigned and voice chat are 0 any,
/// 1 no, 2 yes; +0x0 (u64) is unknown.
pub fn matches_filters(party: &Party, wanted: &Fields) -> bool {
    let empty = Fields::new();
    let listing = party.listing.as_ref().unwrap_or(&empty);
    let game_type = integer(wanted.get(&0xc));
    if game_type != 0 && ![0, game_type].contains(&integer(listing.get(&0x76))) {
        return false;
    }
    let card = unsigned(wanted.get(&0));
    if card != 0 && ![0, card].contains(&unsigned(listing.get(&0x68))) {
        return false;
    }
    for (key, flag) in YES_NO_FILTERS {
        let filter = integer(wanted.get(&key));
        if filter != 0 && (integer(listing.get(&flag)) != 0) != (filter == 2) {
            return false;
        }
    }
    if integer(listing.get(&0x70)) < integer(wanted.get(&0xf))
        || (party.members.len() as i64) < integer(wanted.get(&0x10))
    {
        return false;
    }
    let free = party.free_slot_types();
    for (key, slot_type) in FREE_SLOT_FILTERS {
        let fitting = free
            .iter()
            .filter(|&&free| free == slot_type || free == ANY_SLOT)
            .count();
        if (fitting as i64) < integer(wanted.get(&key)) {
            return false;
        }
    }
    if (free.len() as i64) < integer(wanted.get(&0x15)) {
        return false;
    }
    let slots_free = array(wanted.get(&0x18)).iter().all(|slot| {
        let slot = unsigned(Some(slot));
        free.contains(&slot) || free.contains(&ANY_SLOT)
    });
    if !slots_free {
        return false;
    }
    let creator = structure(listing.get(&0))
        .map(|card| text(card.get(&0x40)))
        .unwrap_or_default();
    let creator = creator.split('#').next().unwrap_or_default();
    let names = format!("{} {creator}", text(listing.get(&0xa0))).to_lowercase();
    array(wanted.get(&0x30)).iter().all(|entry| {
        let wanted = structure(Some(entry)).map_or("", |entry| text(entry.get(&0)));
        names.contains(&wanted.to_lowercase())
    })
}
