//! The lobby sessions that are online, and the mail other sessions post to
//! them.
//!
//! Each session is a thread that blocks on its own socket. Another session
//! that must tell it something (a whisper, an invite, a friend's status)
//! posts the message to its mailbox; the session's thread sends it at its
//! next wake, at most one socket poll later.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use orpheus_lobby::transport::LobbyMessage;

/// Messages waiting for one session's thread.
#[derive(Default)]
pub(crate) struct Mailbox {
    queue: Mutex<VecDeque<LobbyMessage>>,
    has_mail: AtomicBool,
}

impl Mailbox {
    pub(crate) fn post(&self, message: LobbyMessage) {
        self.queue.lock().expect("mailbox").push_back(message);
        self.has_mail.store(true, Ordering::Release);
    }

    pub(crate) fn has_mail(&self) -> bool {
        self.has_mail.load(Ordering::Acquire)
    }

    /// Everything posted so far, oldest first.
    pub(crate) fn take(&self) -> Vec<LobbyMessage> {
        let mut queue = self.queue.lock().expect("mailbox");
        self.has_mail.store(false, Ordering::Release);
        queue.drain(..).collect()
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
    /// offline.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the chat, friends and party modules post to other sessions"
        )
    )]
    pub(crate) fn post(&self, account_lo: u32, message: LobbyMessage) -> bool {
        let mailbox = self
            .online
            .lock()
            .expect("lobby hub")
            .get(&account_lo)
            .cloned();
        match mailbox {
            Some(mailbox) => {
                mailbox.post(message);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_wire::jam::codec::Fields;

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
}
