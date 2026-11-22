//! The Hero Gallery: equipping an item (24500), also during a match.

use std::io;

use lucy_lobby::gallery::{EQUIP, EquipRequest, GALLERY_OUT};
use lucy_wire::jam::codec::Fields;

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
};

pub(crate) const MODULE: Module = Module {
    routes: &[Route {
        group: GALLERY_OUT,
        message: EQUIP,
        handle: equip,
    }],
    hooks: Hooks::NONE,
};

fn equip(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let Some(equip) = EquipRequest::read(&(GALLERY_OUT, EQUIP, fields)) else {
        return Ok(());
    };
    let Some(equipped) = equip.apply(peer.login, &peer.lobby.collection, peer.wall_time)? else {
        return Ok(());
    };
    if let (Some(seat), Some(skin), Some(game)) = (
        peer.seat.as_ref(),
        equipped.skin,
        peer.lobby.game.as_deref(),
    ) {
        game.update_skin(&seat.handoff, equip.hero, skin)?;
    }
    peer.send_all(&equipped.messages)?;
    if equipped.card_changed {
        crate::lobby::social::card_changed(peer)?;
    }
    Ok(())
}
