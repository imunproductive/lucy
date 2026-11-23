//! Chat: channel messages (21700, 21701), whispers (27004), the dashboard's
//! log of the match, team and General lines (`chat_log.rs`), and the match
//! chat a player leaves when its game is back in the menu. Ports
//! `ow174/lobby/handlers/chat.py` and the chat part of `server.py`
//! game_left.

use std::{io, sync::Arc};

use lucy_lobby::social::{
    CHANNEL_GENERAL, CHANNEL_MATCH, CHANNEL_TEAM, account_lo_of,
    messages::{self, CHAT, CHAT_OUT, FRIENDS_OUT, WHISPER, WHO},
    structure, text, unsigned,
};
use lucy_wire::jam::codec::{Fields, Value};

use crate::lobby::{
    chat_log::{LineChannel, LineKind},
    peer::Peer,
    router::{Hooks, Module, Route},
    social::Turn,
};

pub(crate) const MODULE: Module = Module {
    routes: &[
        Route {
            group: CHAT_OUT,
            message: CHAT,
            handle: chat,
        },
        Route {
            group: CHAT_OUT,
            message: WHO,
            handle: channel_members,
        },
        Route {
            group: FRIENDS_OUT,
            message: WHISPER,
            handle: whisper,
        },
    ],
    hooks: Hooks {
        login: None,
        tick: Some(follow_seat),
        logout: None,
    },
};

/// The bot's echo when the player's profile has `bot_chat` on.
fn bot_reply(name: &str, text: &str) -> String {
    format!("{name}, I hear you: {text}")
}

fn chat(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let channel = fields
        .get(&0x78)
        .cloned()
        .unwrap_or(Value::Struct(Fields::new()));
    let said = text(fields.get(&0x90)).to_owned();
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let bot = Arc::clone(turn.bot());
    let empty = Fields::new();
    let fields_of = structure(Some(&channel)).unwrap_or(&empty);
    let members = turn.state.channel_members(fields_of, &bot);
    let flags = unsigned(fields.get(&0xb8));
    let message = messages::chat_message(channel.clone(), turn.record(&me), &said, flags);
    deliver(&mut turn, &members, &message);
    let kind = unsigned(fields_of.get(&0x10));
    tracing::info!("[chat #{kind}] {}: {said}", me.name);
    // The dashboard's log: the speaker's match and team lines, and General.
    let from = Some((me.account_lo, me.name.clone()));
    let log = &peer.lobby.chat_log;
    match (kind, turn.state.match_chat(me.account_lo)) {
        (CHANNEL_MATCH, Some(chat)) => {
            log.add_match(chat.id, LineKind::Chat, LineChannel::Match, from, &said);
        }
        (CHANNEL_TEAM, Some(chat)) => log.add_match(
            chat.id,
            LineKind::Chat,
            LineChannel::Team(chat.team),
            from,
            &said,
        ),
        (CHANNEL_GENERAL, _) => log.add_general(LineKind::Chat, from, &said),
        _ => {}
    }
    let bot_chat = me.profile().bot_chat;
    if bot_chat
        && members
            .iter()
            .any(|member| member.account_lo == bot.account_lo)
    {
        let reply = bot_reply(&me.name, &said);
        let message = messages::chat_message(channel, turn.record(&bot), &reply, 0);
        deliver(&mut turn, &members, &message);
    }
    turn.finish(peer)
}

fn deliver(
    turn: &mut Turn<'_>,
    members: &[Arc<lucy_lobby::accounts::Account>],
    message: &lucy_lobby::transport::LobbyMessage,
) {
    for member in members {
        if turn.has_session(member.account_lo) {
            turn.to(member.account_lo, message.clone());
        }
    }
}

fn channel_members(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let channel = fields
        .get(&0x78)
        .cloned()
        .unwrap_or(Value::Struct(Fields::new()));
    let turn = Turn::new(peer.lobby)?;
    let empty = Fields::new();
    let members = turn
        .state
        .channel_members(structure(Some(&channel)).unwrap_or(&empty), turn.bot());
    let records = members.iter().map(|member| turn.record(member)).collect();
    drop(turn);
    peer.send(&messages::who(channel, records))?;
    Ok(())
}

/// A Battle.net whisper. 27116 completes the request, and the target gets
/// 27117 {sender, text}. The sender's client shows its own line itself, so it
/// gets no copy; the bot answers when `bot_chat` is on.
fn whisper(peer: &mut Peer<'_, '_>, fields: Fields) -> io::Result<()> {
    let me = Arc::clone(&peer.login.account);
    let said = text(fields.get(&0xa0)).to_owned();
    peer.send(&messages::whisper_sent(unsigned(fields.get(&0x78))))?;
    // The first known player who is not the sender: either field may hold
    // the sender.
    let mut target = None;
    for key in [0x80, 0x90] {
        let Some(account_lo) = account_lo_of(fields.get(&key)) else {
            continue;
        };
        if let Some(account) = peer.lobby.accounts.by_id(account_lo)? {
            if account.account_lo != me.account_lo {
                target = Some(account);
                break;
            }
        }
    }
    let Some(target) = target else {
        tracing::info!("[<<<] Whisper to an unknown player");
        return Ok(());
    };
    tracing::info!("[whisper] {} -> {}: {said}", me.name, target.name);
    if target.is_virtual() {
        if me.profile().bot_chat {
            let reply = bot_reply(&me.name, &said);
            peer.send(&messages::whisper(target.account_id_fields(), &reply))?;
        }
        return Ok(());
    }
    let turn = Turn::new(peer.lobby)?;
    if turn.has_session(target.account_lo) {
        drop(turn);
        peer.lobby.hub.post(
            target.account_lo,
            messages::whisper(me.account_id_fields(), &said),
        );
    }
    Ok(())
}

/// The match chat follows the session's game seat: a new seat puts the
/// player in its match's chat and his team's (the handoff already joined
/// the channels), and a seat whose game left its match takes the player
/// back to General with the party state (`server.py` game_left). The Rust
/// game service has no leave event, so the seat's end is seen here.
fn follow_seat(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let Some(seat) = peer.seat.as_ref() else {
        return Ok(());
    };
    let account_lo = peer.login.account.account_lo;
    let match_id = seat.handoff.match_id;
    {
        let mut state = peer.lobby.social.lock();
        if state.seat_seen(account_lo, match_id) {
            state.open_match_chat(match_id, account_lo, seat.team);
            peer.lobby.chat_log.prune(&state, peer.lobby.clock.now());
            return Ok(());
        }
        if state.match_chat_of(account_lo) != Some(match_id) {
            return Ok(());
        }
    }
    let Some(game) = peer.lobby.game.as_deref() else {
        return Ok(());
    };
    if game.seat_exists(&seat.handoff)? {
        return Ok(());
    }
    let mut turn = Turn::new(peer.lobby)?;
    turn.game_left(account_lo, match_id)?;
    turn.finish(peer)
}
