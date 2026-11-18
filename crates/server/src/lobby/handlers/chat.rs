//! Chat: channel messages (21700, 21701), whispers (27004), the `.hero`,
//! `.leave`, `.plate` and `.hurt` commands a player types in any chat while in a
//! match, and the match chat a player leaves when its game is back in the
//! menu. Ports `ow174/lobby/handlers/chat.py` and the chat part of
//! `server.py` game_left.

use std::{io, sync::Arc};

use orpheus_lobby::social::{
    account_lo_of,
    messages::{self, CHAT, CHAT_OUT, FRIENDS_OUT, WHISPER, WHO},
    structure, text, unsigned,
};
use orpheus_wire::jam::codec::{Fields, Value};

use crate::lobby::{
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
    if said.starts_with('.') {
        // A failing command logs one line and answers nothing (upstream
        // `212d85d0`): its text must not go out as chat, and the error must
        // not end the lobby connection.
        match match_command(peer, &said) {
            Ok(None) => {}
            Ok(Some(reply)) => {
                bot_answer(peer, channel, &said, &reply)?;
                return Ok(());
            }
            Err(error) => {
                tracing::warn!("[chat] {said} -> {error}");
                return Ok(());
            }
        }
    }
    let me = Arc::clone(&peer.login.account);
    let mut turn = Turn::new(peer.lobby)?;
    let bot = Arc::clone(turn.bot());
    let empty = Fields::new();
    let members = turn
        .state
        .channel_members(structure(Some(&channel)).unwrap_or(&empty), &bot);
    let flags = unsigned(fields.get(&0xb8));
    let message = messages::chat_message(channel.clone(), turn.record(&me), &said, flags);
    deliver(&mut turn, &members, &message);
    tracing::info!(
        "[chat #{}] {}: {said}",
        structure(Some(&channel)).map_or(0, |channel| unsigned(channel.get(&0x10))),
        me.name
    );
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
    members: &[Arc<orpheus_lobby::accounts::Account>],
    message: &orpheus_lobby::transport::LobbyMessage,
) {
    for member in members {
        if turn.has_session(member.account_lo) {
            turn.to(member.account_lo, message.clone());
        }
    }
}

/// Run a match command and give the bot's answer. None when it is not one,
/// or the game server is off (`--game-port 0`), so the text goes out as chat.
fn match_command(peer: &mut Peer<'_, '_>, said: &str) -> io::Result<Option<String>> {
    let (command, rest) = said[1..].split_once(' ').unwrap_or((&said[1..], ""));
    let command = command.to_lowercase();
    let lobby = peer.lobby;
    let Some(game) = lobby.game.as_deref() else {
        return Ok(None);
    };
    let reply = match command.as_str() {
        "hero" => {
            let wanted = rest.trim();
            match peer.lobby.assets.heroes().find_hero(wanted) {
                None => format!("No hero called '{wanted}'."),
                Some(hero) => {
                    let switched = match peer.seat.as_ref() {
                        Some(seat) => game.switch_hero(&seat.handoff, hero.guid)?,
                        None => false,
                    };
                    if switched {
                        format!("You play {} now.", hero.name)
                    } else {
                        "You are not in a match.".to_owned()
                    }
                }
            }
        }
        "leave" => {
            // Python `GameServer.send_home(account)`.
            let home = peer
                .lobby
                .matchmaker
                .send_home(peer.lobby, Some(peer.login.account.account_lo))
                != 0;
            if home {
                "Back to the menu.".to_owned()
            } else {
                "You are not in a match.".to_owned()
            }
        }
        // The live nameplate tool (`orpheus_sim::bot_plate`): `.plate`
        // shows it, `.plate <change>` changes every match's training bots.
        "plate" => {
            let rest = rest.trim();
            let change = if rest.is_empty() || rest.eq_ignore_ascii_case("show") {
                Ok(None)
            } else {
                orpheus_sim::bot_plate::PlateChange::parse(rest).map(Some)
            };
            match change
                .and_then(|change| game.bot_plate(change).map_err(|error| error.to_string())?)
            {
                Ok(view) => view.summary(),
                Err(reason) => reason,
            }
        }
        // The live hurt feedback switch (`orpheus_sim::hurt_feedback`):
        // `.hurt` shows it, `.hurt on|off`, `.hurt flinch on|off` and
        // `.hurt pain on|off` change it for every match's players.
        "hurt" => match game.hurt_feedback(rest)? {
            Ok(hurt) => hurt.summary(),
            Err(reason) => reason,
        },
        _ => return Ok(None),
    };
    Ok(Some(reply))
}

/// The longest bot answer, in characters. A chat message longer than the
/// channel's slice is streamed without its control bytes, and the client's
/// lobby chat channel dies from it (seen live upstream, `212d85d0`).
const REPLY_LIMIT: usize = 200;

/// The bot's answer to a match command, to the player only, cut to
/// [`REPLY_LIMIT`] characters.
fn bot_answer(peer: &mut Peer<'_, '_>, channel: Value, said: &str, reply: &str) -> io::Result<()> {
    tracing::info!("[chat] {said} -> {reply}");
    let reply = capped(reply);
    let bot = &peer.lobby.accounts.bot;
    let record = Turn::new(peer.lobby)?.record(bot);
    peer.send(&messages::chat_message(channel, record, reply, 0))?;
    Ok(())
}

fn capped(reply: &str) -> &str {
    reply
        .char_indices()
        .nth(REPLY_LIMIT)
        .map_or(reply, |(end, _)| &reply[..end])
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
/// player in its match's chat (the handoff already joined the channel), and a
/// seat whose game left its match takes the player back to General with the
/// party state (`server.py` game_left). The Rust game service has no leave
/// event, so the seat's end is seen here.
fn follow_seat(peer: &mut Peer<'_, '_>) -> io::Result<()> {
    let Some(seat) = peer.seat.as_ref() else {
        return Ok(());
    };
    let account_lo = peer.login.account.account_lo;
    let match_id = seat.handoff.match_id;
    {
        let mut state = peer.lobby.social.lock();
        if state.seat_seen(account_lo, match_id) {
            state.open_match_chat(match_id, account_lo);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_answer_is_cut_to_the_limit_on_a_character_boundary() {
        assert_eq!(capped("short"), "short");
        let long = "é".repeat(REPLY_LIMIT + 50);
        let cut = capped(&long);
        assert_eq!(cut.chars().count(), REPLY_LIMIT);
        assert!(long.starts_with(cut));
    }
}
