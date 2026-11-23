//! The lobby sessions that are online, and the mail other sessions post to
//! them.
//!
//! Each session is a thread that blocks on its own socket. Another session
//! that must tell it something (a whisper, an invite, a friend's status)
//! posts the message to its mailbox; the session's thread sends it at its
//! next wake, at most one socket poll later.
//!
//! A mailbox holds at most [`MAILBOX_LIMIT`] posts; a post is one message,
//! or the messages of one answer that belong together (a custom game's
//! settings, which a large Workshop makes hundreds of chunks). A session
//! that falls that far behind has stopped draining it (its game no longer
//! reads, or the thread is stuck); its game cannot be kept in step by
//! dropping messages, so the mailbox shuts: it refuses all further mail,
//! as for an offline account, and the session ends its link at its next
//! wake. The game then logs in again and gets the whole state.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use lucy_lobby::transport::LobbyMessage;

/// The most posts a session's mailbox holds.
pub(crate) const MAILBOX_LIMIT: usize = 256;

/// Messages waiting for one session's thread.
#[derive(Default)]
pub(crate) struct Mailbox {
    queue: Mutex<VecDeque<Vec<LobbyMessage>>>,
    has_mail: AtomicBool,
    /// The mailbox overflowed and refuses mail; its session ends.
    shut: AtomicBool,
}

impl Mailbox {
    /// Queue messages that go together, as one post.
    pub(crate) fn post_all(&self, messages: Vec<LobbyMessage>) -> bool {
        let mut queue = self.queue.lock().expect("mailbox");
        if self.shut.load(Ordering::Acquire) {
            return false;
        }
        if queue.len() >= MAILBOX_LIMIT {
            queue.clear();
            self.shut.store(true, Ordering::Release);
            self.has_mail.store(true, Ordering::Release);
            return false;
        }
        queue.push_back(messages);
        self.has_mail.store(true, Ordering::Release);
        true
    }

    /// There is mail, or the mailbox shut: the session should wake.
    pub(crate) fn has_mail(&self) -> bool {
        self.has_mail.load(Ordering::Acquire)
    }

    /// The mailbox overflowed: the session must end.
    pub(crate) fn shut(&self) -> bool {
        self.shut.load(Ordering::Acquire)
    }

    /// Everything posted so far, oldest first.
    pub(crate) fn take(&self) -> Vec<LobbyMessage> {
        let mut queue = self.queue.lock().expect("mailbox");
        self.has_mail.store(false, Ordering::Release);
        queue.drain(..).flatten().collect()
    }
}

/// The online sessions by account.
#[derive(Default)]
pub(crate) struct Hub {
    online: Mutex<HashMap<u32, Arc<Mailbox>>>,
}

impl Hub {
    /// Register a session that just logged in; it replaces an older session
    /// of the same account.
    pub(crate) fn join(&self, account_lo: u32) -> Arc<Mailbox> {
        let mailbox = Arc::new(Mailbox::default());
        self.online
            .lock()
            .expect("lobby hub")
            .insert(account_lo, Arc::clone(&mailbox));
        mailbox
    }

    /// Forget a session that ended, unless a newer one replaced it.
    pub(crate) fn leave(&self, account_lo: u32, mailbox: &Arc<Mailbox>) {
        let mut online = self.online.lock().expect("lobby hub");
        if online
            .get(&account_lo)
            .is_some_and(|current| Arc::ptr_eq(current, mailbox))
        {
            online.remove(&account_lo);
        }
    }

    /// Post a message to an online account's session. False when it is
    /// offline or its mailbox shut.
    pub(crate) fn post(&self, account_lo: u32, message: LobbyMessage) -> bool {
        self.post_all(account_lo, vec![message])
    }

    /// Post messages that go together, as one post.
    pub(crate) fn post_all(&self, account_lo: u32, messages: Vec<LobbyMessage>) -> bool {
        let mailbox = self
            .online
            .lock()
            .expect("lobby hub")
            .get(&account_lo)
            .cloned();
        mailbox.is_some_and(|mailbox| mailbox.post_all(messages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_wire::jam::codec::Fields;

    #[test]
    fn mail_reaches_the_current_session_only() {
        let hub = Hub::default();
        let first = hub.join(7);
        assert!(hub.post(7, (1, 2, Fields::new())));
        assert!(first.has_mail());
        assert_eq!(first.take().len(), 1);
        assert!(!first.has_mail());

        let second = hub.join(7);
        hub.leave(7, &first);
        // An older session's end keeps the new one.
        assert!(hub.post(7, (1, 3, Fields::new())));
        assert!(first.take().is_empty());
        assert_eq!(second.take()[0].1, 3);

        hub.leave(7, &second);
        assert!(!hub.post(7, (1, 4, Fields::new())));
    }

    #[test]
    fn a_full_mailbox_shuts_and_refuses_mail() {
        let hub = Hub::default();
        let mailbox = hub.join(7);
        // A post of many messages (a custom game's settings) is one post.
        let settings: Vec<LobbyMessage> = (0..MAILBOX_LIMIT as u32 * 2)
            .map(|id| (1, id, Fields::new()))
            .collect();
        assert!(hub.post_all(7, settings));
        for id in 1..MAILBOX_LIMIT {
            assert!(hub.post(7, (1, id as u32, Fields::new())));
        }
        assert!(!mailbox.shut());
        // One more than it holds: the mail is dropped and the session must
        // end (`Lobby::serve_peer`); later mail is refused as for an
        // offline account.
        assert!(!hub.post(7, (1, 0, Fields::new())));
        assert!(mailbox.shut() && mailbox.has_mail());
        assert!(mailbox.take().is_empty());
        assert!(!hub.post_all(7, vec![(1, 0, Fields::new())]));
        assert!(!hub.post(7, (1, 0, Fields::new())));
    }
}
