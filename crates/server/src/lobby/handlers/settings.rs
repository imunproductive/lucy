//! Saved client settings (22200-22204, 22207) and the season intro (36200).
//! Ports `ow174/lobby/handlers/settings.py`; the rules are in
//! `lucy_lobby::saved_settings`.

use std::io;

use lucy_lobby::saved_settings::{
    self, RANKED_OUT, SAVE_UX_STATE, SAVE_VALUES, SEASON_INTRO_SEEN, SOCIAL_OUT,
};
use lucy_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    peer::Peer,
    router::{Hooks, Module, Route},
    social,
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        Route {
            group: SOCIAL_OUT,
            message: 22200,
            handle: |peer, fields| save_block(peer, &fields, 22200),
        },
        Route {
            group: SOCIAL_OUT,
            message: 22201,
            handle: |peer, fields| save_block(peer, &fields, 22201),
        },
        Route {
            group: SOCIAL_OUT,
            message: 22202,
            handle: |peer, fields| save_block(peer, &fields, 22202),
        },
        Route {
            group: SOCIAL_OUT,
            message: 22203,
            handle: |peer, fields| save_block(peer, &fields, 22203),
        },
        Route {
            group: SOCIAL_OUT,
            message: SAVE_VALUES,
            handle: save_values,
        },
        Route {
            group: SOCIAL_OUT,
            message: SAVE_UX_STATE,
            handle: save_ux_state,
        },
        Route {
            group: RANKED_OUT,
            message: SEASON_INTRO_SEEN,
            handle: season_intro_seen,
        },
    ],
    hooks: Hooks::NONE,
};

/// 22200-22203: save one block of 20802 (+0x78).
fn save_block(peer: &mut Peer<'_, '_>, fields: &Fields, message: u32) -> io::Result<()> {
    let Some(field) = saved_settings::block_field(message) else {
        return Ok(());
    };
    peer.login.account.edit_profile(|profile| {
        saved_settings::save_block(profile, field, fields.get(&0x78));
        Ok(Some(()))
    })?;
    tracing::info!("[<<<] Settings {field} saved");
    // "Members can invite" lives in this block, and the members' party state
    // shows it (+0x94).
    if field == saved_settings::SOCIAL_SETTINGS {
        social::social_settings_saved(peer.lobby, &peer.login.account)?;
    }
    Ok(())
}

/// 22204: merge the changed key/value settings (+0x78) into the saved list.
fn save_values(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let changed = match fields.get(&0x78) {
        Some(Value::Array(changed)) => changed.as_slice(),
        _ => &[],
    };
    peer.login.account.edit_profile(|profile| {
        saved_settings::save_values(profile, changed);
        Ok(Some(()))
    })?;
    tracing::info!("[<<<] {} setting values saved", changed.len());
    Ok(())
}

/// 22207: one interface state {index +0x78, state +0x7C}, sent back in 20812
/// at login. An unchanged state is not saved again.
fn save_ux_state(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let zero = Value::Unsigned(0);
    let index = fields.get(&0x78).unwrap_or(&zero);
    let state = fields.get(&0x7c).unwrap_or(&zero);
    if peer
        .login
        .account
        .edit_profile(|profile| {
            Ok(saved_settings::save_ux_state(profile, index, state).then_some(()))
        })?
        .is_some()
    {
        tracing::info!("[<<<] Interface state {index:?} = {state:?} saved");
    }
    Ok(())
}

/// 36200: the player closed a competitive season's intro (+0x78, the card);
/// the ranked state says so from now on (card +0x2C, set by the client itself
/// when it sends this, 0x7FF789728230).
fn season_intro_seen(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let card = match fields.get(&0x78) {
        Some(Value::Unsigned(card)) => *card,
        Some(Value::Signed(card)) => *card as u64,
        _ => 0,
    };
    if peer
        .login
        .account
        .edit_profile(|profile| Ok(saved_settings::season_intro_seen(profile, card).then_some(())))?
        .is_some()
    {
        tracing::info!("[<<<] Season intro 0x{card:016X} seen");
    }
    Ok(())
}
