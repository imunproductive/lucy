//! `/api/bot_group`: the bot in the group finder. It lists a group of its
//! own, joins the player's listed group, asks to merge the player's group
//! into its own (invite), or leaves. The player's client sees it in the next
//! search or party update (`ow174/dashboard/service.py` bot_group).

use std::sync::Arc;

use serde_json::{Map, Value as Json, json};

use super::api::{Api, ApiError, ApiResult};
use crate::lobby::{Lobby, social::Turn};
use lucy_lobby::social::{DAMAGE_SLOT, PartyId, SUPPORT_SLOT, TANK_SLOT, messages};

const BOT_GROUP_NAME: &str = "Bot's group";
const QUICK_PLAY_ROLE_QUEUE: u64 = 0x0630_0000_0000_00ed;
/// Listing +0x76 of a Quick Play group.
const QUICK_PLAY_GROUP: u64 = 1;
const BOT_GROUP_SLOTS: [u64; 6] = [
    TANK_SLOT,
    TANK_SLOT,
    DAMAGE_SLOT,
    DAMAGE_SLOT,
    SUPPORT_SLOT,
    SUPPORT_SLOT,
];

pub(super) fn bot_group(api: &Api, lobby: Option<&Lobby>, data: &Map<String, Json>) -> ApiResult {
    let lobby = lobby.ok_or_else(|| ApiError::new(503, "The lobby is not running."))?;
    let account = || api.account(data.get("account").and_then(Json::as_str));
    let mut turn = Turn::new(lobby)?;
    let bot = Arc::clone(&lobby.accounts.bot);
    let message = match data.get("action").and_then(Json::as_str) {
        Some("list") => {
            list_bot_group(&mut turn)?;
            "The bot's group is in the group finder."
        }
        Some("join") => {
            let account = account()?;
            let id = turn.state.party_of(&account)?;
            let party = turn.state.party(id).expect("the player's party");
            if party.listing.is_none() {
                return Err(ApiError::new(400, "List a group in the game first"));
            }
            if party.free_slot_types().is_empty() {
                return Err(ApiError::new(400, "The group is full"));
            }
            turn.state.join(&bot, id);
            if let Some(party) = turn.state.party_mut(id) {
                party.take_free_slot(bot.account_lo);
            }
            turn.notify_party(id)?;
            "The bot joined your group."
        }
        Some("remove") => {
            if let Some(id) = turn.state.leave(bot.account_lo) {
                turn.notify_party(id)?;
            }
            "The bot left the group finder."
        }
        Some("invite") => {
            let account = account()?;
            if !turn.has_session(account.account_lo) {
                return Err(ApiError::new(400, "The player is not in the game"));
            }
            let listed = turn.state.party_id_of(bot.account_lo).filter(|&id| {
                turn.state
                    .party(id)
                    .is_some_and(|party| party.is_leader(bot.account_lo) && party.listing.is_some())
            });
            let id = match listed {
                Some(id) => id,
                None => list_bot_group(&mut turn)?,
            };
            let group = turn.state.party_mut(id).expect("the bot's group");
            group.merge_invites.insert(account.account_lo);
            let group = turn.state.party(id).expect("the bot's group");
            let request = messages::merge_request(turn.record(&bot), turn.content.group(group)?);
            turn.to(account.account_lo, request);
            "The bot asked to merge groups."
        }
        _ => return Err(ApiError::new(400, "Unknown bot action")),
    };
    turn.finish_posted();
    Ok(json!({"status": "ok", "message": message}))
}

/// List the bot's own group; a group the bot was in without leading it gets
/// the news.
fn list_bot_group(turn: &mut Turn<'_>) -> Result<PartyId, ApiError> {
    let bot = Arc::clone(turn.bot());
    let listing = messages::bot_listing(
        turn.record(&bot),
        BOT_GROUP_NAME,
        QUICK_PLAY_ROLE_QUEUE,
        QUICK_PLAY_GROUP,
        &BOT_GROUP_SLOTS,
    );
    let (group, left) = turn.state.list_bot_group(&bot, listing)?;
    if let Some(left) = left {
        turn.notify_party(left)?;
    }
    Ok(group)
}
