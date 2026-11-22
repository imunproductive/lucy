//! Loot boxes (24201), purchases (24203) and the store page (26500).
//! Ports `ow174/lobby/handlers/gallery.py` and the store part of
//! `ow174/lobby/handlers/lookups.py`.

use std::io;

use lucy_lobby::{
    collection::PROGRESSION,
    lookups::{self, STORE_PAGE, STORE_QUERY},
    lootbox::{self, OPEN_BOX, PROGRESSION_OUT, SystemDice},
    shop::PURCHASE,
};
use lucy_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        Route {
            group: PROGRESSION_OUT,
            message: OPEN_BOX,
            handle: open_box,
        },
        Route {
            group: PROGRESSION_OUT,
            message: PURCHASE,
            handle: purchase,
        },
        Route {
            group: STORE_QUERY,
            message: STORE_PAGE,
            handle: store,
        },
    ],
    hooks: Hooks::NONE,
};

/// Open a box: its drops (24305), then the credit balance (24307).
fn open_box(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let Some(box_id) = lootbox::box_id(&fields) else {
        return Ok(());
    };
    let lobby = peer.lobby;
    let Some((opening, credits)) = peer.login.account.edit_profile(|profile| {
        let opening =
            lobby
                .services
                .loot
                .open(box_id, profile, &lobby.collection, true, &mut SystemDice)?;
        Ok(Some((opening, profile.credits)))
    })?
    else {
        return Ok(());
    };
    peer.send_all(&[
        lootbox::opened_message([box_id, 0], &opening.drops),
        lootbox::credits_message(credits),
    ])?;
    let items = lobby.collection.items();
    let drops: Vec<String> = opening
        .drops
        .iter()
        .map(|drop| match drop.amount {
            Some(amount) => format!("{amount} credits"),
            None if drop.duplicate => {
                format!("{} (dup +{})", items.describe(drop.unlock), drop.credits)
            }
            None => items.describe(drop.unlock),
        })
        .collect();
    tracing::info!(
        "[>>>] Opened {} box #{box_id}: {}",
        opening.box_name,
        drops.join("; ")
    );
    Ok(())
}

/// Buy an item: charge the profile, save it, and send the unlock (24306),
/// the partner of a team skin (24901 / 24301) and all three balances (24300).
/// A refused purchase changes nothing and gets no answer.
fn purchase(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let guid = match fields.get(&0x78) {
        Some(Value::Unsigned(guid)) => *guid,
        _ => 0,
    };
    let lobby = peer.lobby;
    let collection = &lobby.collection;
    let items = collection.items();
    let mut refused = None;
    let bought = peer.login.account.edit_profile(|profile| {
        match lobby.services.shop.purchase(profile, guid, collection) {
            Ok(receipt) => {
                let balance = *receipt.currency.balance(profile);
                Ok(Some((receipt, balance, collection.progression(profile)?)))
            }
            Err(error) => {
                refused = Some(error);
                Ok(None)
            }
        }
    })?;
    let Some((receipt, balance, progression)) = bought else {
        if let Some(refused) = refused {
            tracing::info!(
                "[<<<] Purchase {} refused: {}",
                items.describe(guid),
                refused.message
            );
        }
        return Ok(());
    };
    let mut messages = vec![collection.unlock_bought(guid)];
    messages.extend(
        receipt
            .also
            .iter()
            .map(|&pair| collection.unlock_granted(pair)),
    );
    // 24300 carries all three balances; a credits-only update would leave the
    // league token and competitive point counters stale.
    messages.push((PROGRESSION, 24300, progression));
    peer.send_all(&messages)?;
    tracing::info!(
        "[>>>] Purchased {} for {} {}, balance {balance}",
        items.describe(guid),
        receipt.price,
        receipt.currency.name()
    );
    for &pair in &receipt.also {
        tracing::info!("[>>>] Came with it: {}", items.describe(pair));
    }
    Ok(())
}

/// The real-money store page: the recorded STORE messages, localized.
fn store(peer: &mut Peer<'_, '_>, _: Fields) -> io::Result<()> {
    let region = peer.login.account.profile().region.clone();
    peer.send_all(&lookups::store(&peer.lobby.templates, &region))
}
