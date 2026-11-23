//! The host's actions on the lobby's side (the dashboard's match page and
//! global message, `refs/reports/Dashboard.md` §1-3, §5): system lines in a
//! match's chat or a team's, a message to everyone online, and taking
//! players out of a match.
//!
//! A system line is a 20400 whose sender card is empty: the client draws
//! the text alone, in the channel type's colour, with no "[Channel] Name:"
//! prefix, and doesn't ask whether it joined that channel
//! (`Msg20400_Handler`, Dashboard.md §1.3). A player in a match gets it on
//! his match's channel (or his team's), one in the menu on General.

use std::io;

use lucy_lobby::{
    handoff::{general_channel, match_channel, team_channel},
    social::{PartyId, messages},
    transport::LobbyMessage,
};
use lucy_wire::jam::codec::{Fields, Value};

use super::{
    Lobby,
    chat_log::{LineChannel, LineKind},
    matchmaker::Home,
    social::Turn,
};

/// The longest line, in characters. A chat message longer than the
/// channel's slice is streamed without its control bytes, and the client's
/// lobby chat channel dies from it (seen live upstream, `212d85d0`); the
/// client cuts what it shows at 255 bytes anyway.
pub(crate) const LINE_LIMIT: usize = 200;

/// The host's text as one line: line breaks become spaces (the client cuts
/// a line at the first one), cut to [`LINE_LIMIT`] characters; none when
/// nothing is left.
pub(crate) fn one_line(text: &str) -> Option<String> {
    let line: String = text
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let line = match line.char_indices().nth(LINE_LIMIT) {
        Some((end, _)) => line[..end].trim_end().to_owned(),
        None => line,
    };
    (!line.is_empty()).then_some(line)
}

/// A system line on `channel`: a 20400 with an empty sender card.
pub(crate) fn system_message(channel: Fields, text: &str) -> LobbyMessage {
    messages::chat_message(Value::Struct(channel), Fields::new(), text, 0)
}

/// Who in a match gets a system line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Audience {
    Match,
    /// Team 0 or 1, on its team channel.
    Team(u8),
}

/// A system line to the players in match `id`'s chat (or one team's),
/// kept in its log, except for the account `except`. How many got it.
pub(crate) fn say(
    lobby: &Lobby,
    id: PartyId,
    audience: Audience,
    text: &str,
    except: Option<u32>,
) -> io::Result<usize> {
    let mut turn = Turn::new(lobby)?;
    let players: Vec<(u32, u8)> = turn.state.match_players(id).collect();
    let mut told = 0;
    for (account, team) in players {
        let channel = match audience {
            Audience::Match => match_channel(id),
            Audience::Team(wanted) if wanted == team => team_channel(id, team),
            Audience::Team(_) => continue,
        };
        if Some(account) == except || !turn.has_session(account) {
            continue;
        }
        turn.to(account, system_message(channel, text));
        told += 1;
    }
    let channel = match audience {
        Audience::Match => LineChannel::Match,
        Audience::Team(team) => LineChannel::Team(team),
    };
    lobby
        .chat_log
        .add_match(id, LineKind::System, channel, None, text);
    turn.finish_posted();
    tracing::info!("[host] to match {:04X}: {text}", id[0] & 0xFFFF);
    Ok(told)
}

/// Who a global message is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    All,
    /// The players in the menu.
    Lobby,
    /// The players in a match.
    Matches,
}

/// A system line to everyone online in `scope`: a player in a match on his
/// match's channel, one in the menu on General; kept in each match's log
/// and General's. How many got it.
pub(crate) fn broadcast(lobby: &Lobby, text: &str, scope: Scope) -> io::Result<usize> {
    let mut turn = Turn::new(lobby)?;
    let sessions: Vec<u32> = turn.state.sessions().collect();
    let mut matches = Vec::new();
    let mut lobby_told = false;
    let mut told = 0;
    for account in sessions {
        let channel = match turn.state.match_chat(account) {
            Some(chat) if scope != Scope::Lobby => {
                if !matches.contains(&chat.id) {
                    matches.push(chat.id);
                }
                match_channel(chat.id)
            }
            None if scope != Scope::Matches => {
                lobby_told = true;
                general_channel()
            }
            _ => continue,
        };
        turn.to(account, system_message(channel, text));
        told += 1;
    }
    for id in matches {
        lobby
            .chat_log
            .add_match(id, LineKind::System, LineChannel::Match, None, text);
    }
    if lobby_told || scope != Scope::Matches {
        lobby.chat_log.add_general(LineKind::System, None, text);
    }
    turn.finish_posted();
    tracing::info!("[host] to everyone ({scope:?}, {told}): {text}");
    Ok(told)
}

/// The host takes the player of `account` out of match `id`: his game goes
/// home (20304) through the matchmaker's seat, or the game service's for a
/// seat it doesn't hold (the solo Practice Range); the match's chat gets a
/// system line, and he gets the reason on General once his game is in the
/// menu. False when he isn't in the match.
pub(crate) fn kick(
    lobby: &Lobby,
    id: PartyId,
    account: u32,
    name: &str,
    reason: Option<&str>,
) -> io::Result<bool> {
    let Some(game) = lobby.game.as_deref() else {
        return Ok(false);
    };
    let taken = lobby.matchmaker.send_home(lobby, Home::Player(id, account)) > 0
        || game.kick(id, account)?;
    if !taken {
        return Ok(false);
    }
    let note = match reason {
        Some(reason) => format!("The host removed you from the match: {reason}"),
        None => "The host removed you from the match.".to_owned(),
    };
    lobby
        .chat_log
        .leave_note(account, &one_line(&note).unwrap_or(note));
    say(
        lobby,
        id,
        Audience::Match,
        &format!("The host removed {name} from the match."),
        Some(account),
    )?;
    Ok(true)
}

/// The host stops match `id` without a result: every player's game goes
/// home now (20304), through the matchmaker's seats and the game
/// service's. How many players the match had.
pub(crate) fn stop(lobby: &Lobby, id: PartyId) -> io::Result<usize> {
    let Some(game) = lobby.game.as_deref() else {
        return Ok(0);
    };
    let taken = lobby.matchmaker.send_home(lobby, Home::Match(id));
    let left = game.stop_match(id)?;
    tracing::info!("[host] stopped match {:04X}", id[0] & 0xFFFF);
    Ok(taken.max(left))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hosts_text_is_one_line_within_the_limit() {
        assert_eq!(one_line("  hello\r\n  there  "), Some("hello there".into()));
        assert_eq!(one_line(" \n "), None);
        let long = "é".repeat(LINE_LIMIT + 50);
        let cut = one_line(&long).unwrap();
        assert_eq!(cut.chars().count(), LINE_LIMIT);
    }
}
