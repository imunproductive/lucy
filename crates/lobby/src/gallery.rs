//! Retail gallery equip requests are sent on the lobby connection, even during a match.

use std::io;

use crate::{
    collection::{Collection, HERO_CATALOG},
    player_messages::record,
    session::PreparedLogin,
    templates::IN_CONNECT,
    transport::LobbyMessage,
};
use lucy_wire::{
    entity::SkinTheme,
    jam::codec::{Fields, Value},
};

pub const GALLERY_OUT: u32 = 0xb688_70b8;
pub const EQUIP: u32 = 24500;
pub const EQUIPPED: u32 = 24902;

pub struct EquipRequest {
    pub hero: u64,
    item: u64,
    slot: i32,
}

impl EquipRequest {
    pub fn read((crc, id, fields): &LobbyMessage) -> Option<Self> {
        if (*crc, *id) != (GALLERY_OUT, EQUIP) {
            return None;
        }
        let (Value::Unsigned(hero), Value::Unsigned(item), Value::Signed(slot)) =
            (fields.get(&0x78)?, fields.get(&0x80)?, fields.get(&0x88)?)
        else {
            return None;
        };
        Some(Self {
            hero: *hero,
            item: *item,
            slot: i32::try_from(*slot).ok()?,
        })
    }

    pub fn apply(
        &self,
        login: &PreparedLogin,
        collection: &Collection,
        wall_time: u64,
    ) -> io::Result<Option<Equipped>> {
        let Some(item) = collection.items().get(self.item) else {
            return Ok(None);
        };
        login.account.edit_profile(|profile| {
            if !collection.equip(profile, self.hero, self.item, self.slot)? {
                return Ok(None);
            }
            let confirmation = Fields::from([
                (0x78, Value::Unsigned(self.hero)),
                (0x80, Value::Unsigned(self.item)),
                (0x88, Value::Signed(i64::from(self.slot))),
            ]);
            let mut messages = vec![(HERO_CATALOG, EQUIPPED, confirmation)];
            // An account item or an icon changes the player card, which the
            // party and friends see too (`ow174/lobby/handlers/gallery.py`).
            let card_changed = self.hero == 0 || item.item_type == "Icon";
            if card_changed {
                let card = record(
                    profile,
                    login.identity,
                    collection.portrait_frame(profile),
                    wall_time,
                );
                messages.push((
                    IN_CONNECT,
                    20502,
                    Fields::from([(0x78, Value::Struct(card))]),
                ));
            }
            let skin = match item.item_type.as_str() {
                "Skin" | "WeaponSkin" => Some(collection.equipped_skin(profile, self.hero)?),
                _ => None,
            };
            Ok(Some(Equipped {
                messages,
                skin,
                card_changed,
            }))
        })
    }
}

pub struct Equipped {
    pub messages: Vec<LobbyMessage>,
    pub skin: Option<SkinTheme>,
    /// The player card changed (20502 is among the messages): the party and
    /// friends must be told.
    pub card_changed: bool,
}
