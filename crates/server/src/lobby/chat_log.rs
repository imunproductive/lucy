//! What was said in each match's chat and in General, for the dashboard
//! (`/api/matches/<id>/chat`, `/api/lobby/chat`): the players' lines in a
//! match's channel and their teams', the host's system lines, and General.
//!
//! A match's log is kept while anyone is in its chat, and for
//! [`KEEP_AFTER`] once the last player left it ([`ChatLog::prune`], at
//! every change of the match chats and every read); each log keeps its last
//! [`LINES_KEPT`] lines. Every line has a sequence number, from 1 in each
//! log, which a reader passes back to get only the lines after it.
//!
//! Lock order: the social state, then this log; never the social state
//! while this log is locked.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lucy_lobby::social::{PartyId, SocialState};

/// How long a match's log stays once its chat emptied.
pub(crate) const KEEP_AFTER: Duration = Duration::from_secs(30 * 60);
/// The lines each log keeps.
pub(crate) const LINES_KEPT: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineKind {
    /// A player's line.
    Chat,
    /// The host's (authorless) line.
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineChannel {
    Match,
    /// Team 0 or 1's chat.
    Team(u8),
    General,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChatLine {
    pub seq: u64,
    /// Unix time in seconds.
    pub at: u64,
    pub kind: LineKind,
    pub channel: LineChannel,
    /// The speaker's account and name; none for a system line.
    pub from: Option<(u32, String)>,
    pub text: String,
}

#[derive(Default)]
struct Ring {
    lines: VecDeque<ChatLine>,
    last: u64,
}

impl Ring {
    fn add(
        &mut self,
        kind: LineKind,
        channel: LineChannel,
        from: Option<(u32, String)>,
        text: &str,
    ) {
        self.last += 1;
        if self.lines.len() == LINES_KEPT {
            self.lines.pop_front();
        }
        self.lines.push_back(ChatLine {
            seq: self.last,
            at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |time| time.as_secs()),
            kind,
            channel,
            from,
            text: text.to_owned(),
        });
    }

    /// The lines after `after`, and the sequence number to pass next.
    fn page(&self, after: u64) -> (Vec<ChatLine>, u64) {
        let lines: Vec<ChatLine> = self
            .lines
            .iter()
            .filter(|line| line.seq > after)
            .cloned()
            .collect();
        (lines, self.last)
    }
}

#[derive(Default)]
struct MatchLog {
    ring: Ring,
    /// When its chat emptied, on the lobby's clock.
    emptied: Option<Duration>,
}

#[derive(Default)]
struct Logs {
    matches: HashMap<PartyId, MatchLog>,
    general: Ring,
    /// A line each account gets once its game is back in the menu (the
    /// host's reason for a kick), on General.
    notes: HashMap<u32, String>,
}

#[derive(Default)]
pub(crate) struct ChatLog {
    logs: Mutex<Logs>,
}

impl ChatLog {
    fn lock(&self) -> std::sync::MutexGuard<'_, Logs> {
        self.logs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A line in match `id`'s log.
    pub(crate) fn add_match(
        &self,
        id: PartyId,
        kind: LineKind,
        channel: LineChannel,
        from: Option<(u32, String)>,
        text: &str,
    ) {
        let mut logs = self.lock();
        let log = logs.matches.entry(id).or_default();
        log.ring.add(kind, channel, from, text);
    }

    /// A line in General's log.
    pub(crate) fn add_general(&self, kind: LineKind, from: Option<(u32, String)>, text: &str) {
        self.lock()
            .general
            .add(kind, LineChannel::General, from, text);
    }

    /// Match `id`'s lines after `after` and the sequence number to pass
    /// next; none when it has no log (never had a line, or gone).
    pub(crate) fn match_page(&self, id: PartyId, after: u64) -> Option<(Vec<ChatLine>, u64)> {
        self.lock().matches.get(&id).map(|log| log.ring.page(after))
    }

    pub(crate) fn general_page(&self, after: u64) -> (Vec<ChatLine>, u64) {
        self.lock().general.page(after)
    }

    /// The sequence number of match `id`'s last line; 0 for none.
    pub(crate) fn last(&self, id: PartyId) -> u64 {
        self.lock().matches.get(&id).map_or(0, |log| log.ring.last)
    }

    /// Each log's match chat as `state` has it at `now`: a log whose chat
    /// emptied goes [`KEEP_AFTER`] later.
    pub(crate) fn prune(&self, state: &SocialState, now: Duration) {
        let mut logs = self.lock();
        logs.matches.retain(|&id, log| {
            if state.match_players(id).next().is_some() {
                log.emptied = None;
                return true;
            }
            let emptied = *log.emptied.get_or_insert(now);
            now.saturating_sub(emptied) < KEEP_AFTER
        });
    }

    /// A line for the account once its game is back in the menu.
    pub(crate) fn leave_note(&self, account_lo: u32, text: &str) {
        self.lock().notes.insert(account_lo, text.to_owned());
    }

    pub(crate) fn take_note(&self, account_lo: u32) -> Option<String> {
        self.lock().notes.remove(&account_lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_match_log_pages_by_sequence_and_goes_half_an_hour_after_its_chat_empties() {
        let log = ChatLog::default();
        let id = [1, 2];
        let mut state = SocialState::default();
        let folder = tempfile::tempdir().unwrap();
        let accounts = lucy_lobby::accounts::Accounts::new(
            folder.path().join("profiles"),
            folder.path().join("profile.json"),
        )
        .unwrap();
        let alpha = accounts.get("Alpha").unwrap();
        state.connect(&alpha, Duration::ZERO);
        state.open_match_chat(id, alpha.account_lo, 0);
        for text in ["one", "two", "three"] {
            log.add_match(id, LineKind::Chat, LineChannel::Match, None, text);
        }
        let (lines, next) = log.match_page(id, 1).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert_eq!(next, 3);
        assert_eq!(log.match_page(id, 3).unwrap(), (Vec::new(), 3));
        assert_eq!(log.match_page([9, 9], 0), None);

        // Kept while the chat has a player, and half an hour after.
        log.prune(&state, Duration::from_secs(3600));
        state.leave_match_chat(alpha.account_lo, None);
        let emptied = Duration::from_secs(4000);
        log.prune(&state, emptied);
        log.prune(&state, emptied + KEEP_AFTER - Duration::from_secs(1));
        assert_eq!(log.last(id), 3);
        log.prune(&state, emptied + KEEP_AFTER);
        assert_eq!(log.match_page(id, 0), None);
    }

    #[test]
    fn a_log_keeps_its_last_lines() {
        let log = ChatLog::default();
        for index in 0..LINES_KEPT + 2 {
            log.add_general(LineKind::Chat, None, &index.to_string());
        }
        let (lines, next) = log.general_page(0);
        assert_eq!(lines.len(), LINES_KEPT);
        assert_eq!(lines[0].seq, 3);
        assert_eq!(next, LINES_KEPT as u64 + 2);
    }
}
