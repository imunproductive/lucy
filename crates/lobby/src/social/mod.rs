//! Friends, presence, parties, chat channels and the group finder that every
//! lobby session shares. Ports `ow174/services/social.py`.
//!
//! Friends are added by BattleTag and saved in both profiles. A virtual friend
//! (the bot account) is in every friends list and always online; it joins
//! parties it is invited to, so the social screens can be tried with one
//! client.
//!
//! A chat channel is `{+0x0: id, +0x10: type, +0x14: index}`. The types used
//! here are 4 for a party (the id is the party id), 5 for a team of a match
//! (the id is `handoff::team_channel_id`), 6 for the players of a match
//! (the id is the match id) and 7 for General. Whispers use their own
//! messages. 5 is the team chat and 6 the match chat: the client's
//! `STU_7106C356` posts a line with a speaker to his teammates in type 5
//! (`refs/reports/Dashboard.md` §1.1).
//!
//! The state is one [`SocialState`] behind a mutex: a caller locks it for a
//! whole client request, as the Python handlers ran under one state lock.
//! Lock order: the social state, then profiles, then the menu heroes; never
//! the social state while a profile is locked. Times (`now`) are the lobby's
//! clock ([`Clock`](crate::clock::Clock)): the time since it started.

pub mod content;
mod groups;
pub mod messages;
#[cfg(test)]
mod tests;

use std::{
    collections::{HashMap, HashSet},
    io,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use indexmap::IndexMap;
use lucy_content::catalog::ItemDb;
use lucy_wire::jam::codec::{Fields, Value, id16};

use crate::{
    accounts::{Account, Accounts, Profile},
    login::{ID_HIGH_TAG, PARTY_ENTITY_TYPE},
    menu::{MenuHero, MenuSelection},
};

pub use groups::{group_rating, matches_filters};

pub const CHANNEL_GROUP: u64 = 4;
pub const CHANNEL_TEAM: u64 = 5;
pub const CHANNEL_MATCH: u64 = 6;
pub const CHANNEL_GENERAL: u64 = 7;
/// Group finder slot types (LFG enum table 0x7FF78B566690).
pub const ANY_SLOT: u64 = 1;
pub const TANK_SLOT: u64 = 2;
pub const SUPPORT_SLOT: u64 = 3;
pub const DAMAGE_SLOT: u64 = 4;
/// The saved block of the Social options (22202).
pub const SOCIAL_SETTINGS: &str = "+0x10D";
/// A party invite waits this long for an answer, as the invitee's popup does
/// (0x7FF789761DA0).
pub const INVITE_TIME: Duration = Duration::from_secs(25);
/// A team: a merge that would make a bigger party is not asked or done.
pub const MAX_PARTY_SIZE: usize = 6;
/// Offline friends' presence goes again this long after login, then again
/// after the second delay (`ow174/lobby/handlers/login.py` MENU_READY_DELAYS):
/// shown during login they get an empty "offline ()" time until it comes
/// again.
pub const MENU_READY_DELAYS: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(7)];
/// The client writes "offline (4 minutes)" once, when a friend's presence
/// arrives (0x7FF7894EBF30 counts from its own clock), so the text only moves
/// when the presence is sent again.
pub const OFFLINE_REFRESH: Duration = Duration::from_secs(60);

/// A party or match id, `[+0x0, +0x8]`.
pub type PartyId = [u64; 2];

/// A party invite waiting for an answer.
pub struct Invite {
    pub inviter: Arc<Account>,
    pub invitee: Arc<Account>,
    /// The lobby's clock when it was sent.
    pub sent: Duration,
}

impl Invite {
    pub fn remaining_ms(&self, now: Duration) -> u64 {
        let deadline = self.sent + INVITE_TIME;
        u64::try_from(deadline.saturating_sub(now).as_millis()).unwrap_or(u64::MAX)
    }

    fn expired(&self, now: Duration) -> bool {
        now >= self.sent + INVITE_TIME
    }
}

pub struct Party {
    pub id: PartyId,
    pub entity: [u64; 2],
    pub leader: Arc<Account>,
    /// The leader flag of 20700 goes to the first member.
    pub members: Vec<Arc<Account>>,
    /// Players invited and not answered yet, one per invitee, oldest first.
    pub invites: Vec<Invite>,
    /// The group finder entry the leader made (52201), None when not listed.
    pub listing: Option<Fields>,
    /// The listing is open in the group finder (party state +0x9A).
    pub searching: bool,
    /// Account -> the group slot types it took (52203).
    pub slot_types: HashMap<u32, Vec<u64>>,
    /// Leaders asked to merge their group into this one (20703).
    pub merge_invites: HashSet<u32>,
}

impl Party {
    fn new(leader: &Arc<Account>) -> io::Result<Self> {
        let sequence = random_u64()? & u64::from(u32::MAX);
        Ok(Self {
            id: [random_u64()?, random_u64()?],
            entity: [sequence | PARTY_ENTITY_TYPE | ID_HIGH_TAG, random_u64()?],
            leader: Arc::clone(leader),
            members: vec![Arc::clone(leader)],
            invites: Vec::new(),
            listing: None,
            searching: false,
            slot_types: HashMap::new(),
            merge_invites: HashSet::new(),
        })
    }

    pub fn chat_channel(&self) -> Fields {
        channel(self.id, CHANNEL_GROUP)
    }

    pub fn has_member(&self, account_lo: u32) -> bool {
        self.members
            .iter()
            .any(|member| member.account_lo == account_lo)
    }

    pub fn is_leader(&self, account_lo: u32) -> bool {
        self.leader.account_lo == account_lo
    }

    /// Whether the members may invite too: the leader's "Members can invite
    /// to group", +0x0 of the Social options block 22202 saves; on until the
    /// leader saves it. The client offers "Invite" to a member only when the
    /// party state's +0x94 is on (0x7FF7899B33FE).
    pub fn members_may_invite(&self) -> bool {
        members_may_invite(&self.leader.profile())
    }

    pub fn may_invite(&self, account_lo: u32) -> bool {
        self.is_leader(account_lo) || self.members_may_invite()
    }

    /// The listed group's slots nobody fills yet, as slot types.
    pub fn free_slot_types(&self) -> Vec<u64> {
        let mut free = self.listing.as_ref().map_or_else(Vec::new, |listing| {
            array(listing.get(&0x80))
                .iter()
                .map(|value| unsigned(Some(value)))
                .collect()
        });
        for member in &self.members {
            if let Some(&taken) = self
                .slot_types
                .get(&member.account_lo)
                .and_then(|taken| taken.first())
            {
                if let Some(index) = free.iter().position(|&slot| slot == taken) {
                    free.remove(index);
                }
            }
        }
        free
    }

    /// A full group stops looking for players; the client then offers to play
    /// (0x7FF78934B390).
    pub fn update_search(&mut self) {
        if self.listing.is_some() && self.free_slot_types().is_empty() {
            self.searching = false;
        }
    }

    /// A member joining a listed group takes its first free slot.
    pub fn take_free_slot(&mut self, account_lo: u32) {
        if let Some(&slot) = self.free_slot_types().first() {
            self.slot_types.insert(account_lo, vec![slot]);
        }
        self.update_search();
    }

    /// Whether the player meets the group's requirements: its minimum
    /// endorsement level (+0x70) and, for a competitive group, its rating
    /// spread (+0x74). The client checks neither (it only sorts the found
    /// groups by them, 0x7FF789AA1A60), so the search hides the group and a
    /// join is refused. The spread is measured between the ratings of the
    /// player and the leader.
    pub fn can_join(&self, account: &Account) -> io::Result<bool> {
        if account.is_virtual() {
            return Ok(true);
        }
        let empty = Fields::new();
        let listing = self.listing.as_ref().unwrap_or(&empty);
        let profile = account.profile().clone();
        if profile.endorsement_level < integer(listing.get(&0x70)) {
            return Ok(false);
        }
        let spread = integer(listing.get(&0x74));
        let Some(mine) = group_rating(&profile, listing)? else {
            return Ok(true);
        };
        if spread == 0 {
            return Ok(true);
        }
        let leader = self.leader.profile().clone();
        let theirs = group_rating(&leader, listing)?.unwrap_or(mine);
        Ok((i64::from(mine) - i64::from(theirs)).abs() <= spread)
    }

    pub fn is_open_group(&self) -> bool {
        self.listing.is_some() && self.searching
    }
}

/// The leader's "Members can invite to group" setting; see
/// [`Party::members_may_invite`].
pub fn members_may_invite(profile: &Profile) -> bool {
    profile
        .settings
        .get(SOCIAL_SETTINGS)
        .and_then(|settings| settings.get("+0x0"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

/// What one lobby session of an account remembers between its messages.
pub struct Session {
    /// The party chat channel the client has joined.
    pub party_channel: Option<PartyId>,
    /// Party ids picked in the group finder (52204).
    pub watched_groups: HashSet<PartyId>,
    /// When the offline friends' presence goes again, and how often it went.
    presence_due: Duration,
    presence_resends: usize,
    /// The match whose seat this session saw last.
    seat_seen: Option<PartyId>,
}

impl Session {
    fn new(now: Duration) -> Self {
        Self {
            party_channel: None,
            watched_groups: HashSet::new(),
            presence_due: now + MENU_READY_DELAYS[0],
            presence_resends: 0,
            seat_seen: None,
        }
    }
}

/// How a session's party chat channel changes with a new party state.
#[derive(Debug, PartialEq)]
pub enum ChannelChange {
    None,
    Join(Fields),
    Leave(Fields),
}

#[derive(Default)]
pub struct SocialState {
    parties: HashMap<PartyId, Party>,
    /// Account -> its party, in the order the accounts got one (the order the
    /// group finder lists groups in).
    member_of: IndexMap<u32, PartyId>,
    /// The logged-in accounts, in login order.
    online: IndexMap<u32, Arc<Account>>,
    sessions: HashMap<u32, Session>,
    /// Account -> its match's chat channels.
    match_chats: HashMap<u32, MatchChat>,
}

/// The chat of the match an account plays in: the match's channel and his
/// team's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchChat {
    pub id: PartyId,
    pub team: u8,
}

impl MatchChat {
    pub fn channels(self) -> [Fields; 2] {
        [
            channel(
                crate::handoff::team_channel_id(self.id, self.team),
                CHANNEL_TEAM,
            ),
            channel(self.id, CHANNEL_MATCH),
        ]
    }
}

#[derive(Default)]
pub struct Social {
    state: Mutex<SocialState>,
    /// Account -> its party card hero; a random pick stays until the player
    /// changes the choice, as `ow174/content/menu_hero.py` keeps it.
    menu_heroes: Mutex<HashMap<u32, MenuHero>>,
}

impl Social {
    pub fn lock(&self) -> MutexGuard<'_, SocialState> {
        self.state.lock().expect("social state")
    }

    /// A login picks a new random menu hero (`ow174/content/__init__.py`
    /// login_messages rerolls it).
    pub fn reroll_menu_hero(&self, account_lo: u32) {
        self.menu_heroes
            .lock()
            .expect("menu heroes")
            .remove(&account_lo);
    }

    /// The account's menu and party card hero. The caller may hold the
    /// profile's lock.
    pub fn menu_hero(
        &self,
        account_lo: u32,
        profile: &Profile,
        heroes: &[u64],
        items: &ItemDb,
    ) -> io::Result<MenuSelection> {
        self.menu_heroes
            .lock()
            .expect("menu heroes")
            .entry(account_lo)
            .or_default()
            .select(profile, heroes, items)
    }
}

impl SocialState {
    // --- sessions and presence ---------------------------------------------

    /// A session of the account logged in; a newer one replaces the older
    /// one's per-session state.
    pub fn connect(&mut self, account: &Arc<Account>, now: Duration) {
        self.online.insert(account.account_lo, Arc::clone(account));
        self.sessions.insert(account.account_lo, Session::new(now));
        self.match_chats.remove(&account.account_lo);
    }

    /// The account's last session ended.
    pub fn disconnect(&mut self, account_lo: u32) {
        self.online.shift_remove(&account_lo);
        self.sessions.remove(&account_lo);
        self.match_chats.remove(&account_lo);
    }

    pub fn session(&self, account_lo: u32) -> Option<&Session> {
        self.sessions.get(&account_lo)
    }

    pub fn session_mut(&mut self, account_lo: u32) -> Option<&mut Session> {
        self.sessions.get_mut(&account_lo)
    }

    /// Whether a client of the account is logged in. The bot always is.
    pub fn is_online(&self, account: &Account) -> bool {
        account.is_virtual() || self.online.contains_key(&account.account_lo)
    }

    /// The logged-in accounts, then the bot.
    pub fn online(&self, bot: &Arc<Account>) -> Vec<Arc<Account>> {
        self.online
            .values()
            .cloned()
            .chain([Arc::clone(bot)])
            .collect()
    }

    /// Logged-in accounts only, without the bot.
    pub fn sessions(&self) -> impl Iterator<Item = u32> + '_ {
        self.online.keys().copied()
    }

    /// How friends see the account: offline when not connected, else its
    /// chosen status (which may be appear-offline). The bot is always online.
    pub fn effective_status(&self, account: &Account) -> u8 {
        if account.is_virtual() {
            crate::presence::STATUS_ONLINE
        } else if !self.online.contains_key(&account.account_lo) {
            crate::presence::STATUS_OFFLINE
        } else {
            account.status()
        }
    }

    /// True when the session's offline-friends presence is due now; the next
    /// one is then scheduled (+3 s, +7 s, then every 60 s).
    pub fn presence_due(&mut self, account_lo: u32, now: Duration) -> bool {
        let Some(session) = self.sessions.get_mut(&account_lo) else {
            return false;
        };
        if now < session.presence_due {
            return false;
        }
        session.presence_resends += 1;
        let delay = MENU_READY_DELAYS
            .get(session.presence_resends)
            .copied()
            .unwrap_or(OFFLINE_REFRESH);
        session.presence_due = now + delay;
        true
    }

    // --- chat ------------------------------------------------------------

    /// Who gets a line said in `channel`: a match's players, his team in
    /// it, a party's members, else everyone online and the bot.
    pub fn channel_members(&self, channel: &Fields, bot: &Arc<Account>) -> Vec<Arc<Account>> {
        let kind = unsigned(channel.get(&0x10));
        let id = id_of(channel.get(&0));
        let in_chat = |test: &dyn Fn(MatchChat) -> bool| {
            self.online
                .values()
                .filter(|account| {
                    self.match_chats
                        .get(&account.account_lo)
                        .is_some_and(|&chat| test(chat))
                })
                .cloned()
                .collect()
        };
        match (kind, id) {
            (CHANNEL_MATCH, Some(id)) => in_chat(&|chat| chat.id == id),
            (CHANNEL_TEAM, Some(id)) => {
                in_chat(&|chat| crate::handoff::team_channel_id(chat.id, chat.team) == id)
            }
            (CHANNEL_MATCH | CHANNEL_TEAM, None) => Vec::new(),
            (CHANNEL_GROUP, id) => id
                .and_then(|id| self.party(id))
                .map_or_else(Vec::new, |party| party.members.clone()),
            _ => self.online(bot),
        }
    }

    /// Put the account in a match's chat channel and his team's, which the
    /// in-match chat posts to.
    pub fn open_match_chat(&mut self, match_id: PartyId, account_lo: u32, team: u8) {
        self.match_chats
            .insert(account_lo, MatchChat { id: match_id, team });
    }

    pub fn match_chat_of(&self, account_lo: u32) -> Option<PartyId> {
        self.match_chats.get(&account_lo).map(|chat| chat.id)
    }

    pub fn match_chat(&self, account_lo: u32) -> Option<MatchChat> {
        self.match_chats.get(&account_lo).copied()
    }

    /// The accounts in match `id`'s chat, with their teams.
    pub fn match_players(&self, id: PartyId) -> impl Iterator<Item = (u32, u8)> + '_ {
        self.match_chats
            .iter()
            .filter(move |(_, chat)| chat.id == id)
            .map(|(&account, chat)| (account, chat.team))
    }

    /// Take the account out of its match's chat and return its channels
    /// (his team's, then the match's). With a match id, only when the chat
    /// is that match's: a later match's chat stays.
    pub fn leave_match_chat(
        &mut self,
        account_lo: u32,
        match_id: Option<PartyId>,
    ) -> Option<[Fields; 2]> {
        let current = *self.match_chats.get(&account_lo)?;
        if match_id.is_some_and(|id| id != current.id) {
            return None;
        }
        self.match_chats.remove(&account_lo);
        Some(current.channels())
    }

    /// Record the seat a session holds now. True when it is a seat the
    /// session did not see before.
    pub fn seat_seen(&mut self, account_lo: u32, match_id: PartyId) -> bool {
        let Some(session) = self.sessions.get_mut(&account_lo) else {
            return false;
        };
        let new = session.seat_seen != Some(match_id);
        session.seat_seen = Some(match_id);
        new
    }

    // --- parties -----------------------------------------------------------

    /// The account's party, creating a party of one on first use.
    pub fn party_of(&mut self, account: &Arc<Account>) -> io::Result<PartyId> {
        if let Some(&id) = self.member_of.get(&account.account_lo) {
            return Ok(id);
        }
        let party = Party::new(account)?;
        let id = party.id;
        self.parties.insert(id, party);
        self.member_of.insert(account.account_lo, id);
        Ok(id)
    }

    /// The account's party, without creating one.
    pub fn party_id_of(&self, account_lo: u32) -> Option<PartyId> {
        self.member_of.get(&account_lo).copied()
    }

    /// A party that has members.
    pub fn party(&self, id: PartyId) -> Option<&Party> {
        self.parties.get(&id)
    }

    pub fn party_mut(&mut self, id: PartyId) -> Option<&mut Party> {
        self.parties.get_mut(&id)
    }

    /// Parties whose listing is open in the group finder, one entry per party.
    pub fn listed_groups(&self) -> Vec<PartyId> {
        let mut seen = HashSet::new();
        self.member_of
            .values()
            .filter(|&&id| seen.insert(id))
            .filter(|&id| self.parties.get(id).is_some_and(Party::is_open_group))
            .copied()
            .collect()
    }

    pub fn invite(
        &mut self,
        inviter: &Arc<Account>,
        target: &Arc<Account>,
        now: Duration,
    ) -> io::Result<PartyId> {
        let id = self.party_of(inviter)?;
        let party = self.parties.get_mut(&id).expect("the inviter's party");
        let invite = Invite {
            inviter: Arc::clone(inviter),
            invitee: Arc::clone(target),
            sent: now,
        };
        // A new invite replaces an older one in its place.
        match party
            .invites
            .iter_mut()
            .find(|old| old.invitee.account_lo == target.account_lo)
        {
            Some(old) => *old = invite,
            None => party.invites.push(invite),
        }
        Ok(id)
    }

    /// Drop an invite that was not answered yet and return it.
    pub fn cancel_invite(&mut self, id: PartyId, invitee_lo: u32) -> Option<Invite> {
        let party = self.parties.get_mut(&id)?;
        let index = party
            .invites
            .iter()
            .position(|invite| invite.invitee.account_lo == invitee_lo)?;
        Some(party.invites.remove(index))
    }

    /// Drop the invites nobody answered in time. Returns the parties whose
    /// pending tiles changed.
    pub fn expire_invites(&mut self, now: Duration) -> Vec<PartyId> {
        let mut changed = Vec::new();
        for party in self.parties.values_mut() {
            let before = party.invites.len();
            party.invites.retain(|invite| !invite.expired(now));
            if party.invites.len() != before {
                changed.push(party.id);
            }
        }
        changed
    }

    pub fn join(&mut self, account: &Arc<Account>, id: PartyId) {
        if self
            .member_of
            .get(&account.account_lo)
            .is_some_and(|&old| old != id)
        {
            self.leave(account.account_lo);
        }
        let Some(party) = self.parties.get_mut(&id) else {
            return;
        };
        if !party.has_member(account.account_lo) {
            party.members.push(Arc::clone(account));
        }
        party
            .invites
            .retain(|invite| invite.invitee.account_lo != account.account_lo);
        self.member_of.insert(account.account_lo, id);
    }

    /// Remove the account from its party and return that party's id, or None
    /// when it had none. A party left empty is gone, and so is its group
    /// finder listing: the players who picked it hear it closed.
    pub fn leave(&mut self, account_lo: u32) -> Option<PartyId> {
        let id = self.member_of.shift_remove(&account_lo)?;
        let party = self.parties.get_mut(&id)?;
        party
            .members
            .retain(|member| member.account_lo != account_lo);
        party.slot_types.remove(&account_lo);
        if party.members.is_empty() {
            self.parties.remove(&id);
        } else if party.is_leader(account_lo) {
            party.leader = Arc::clone(&party.members[0]);
        }
        Some(id)
    }

    /// The new leader moves to the front, where the 20700 leader flag is.
    /// False when the account may not hand the party over to the target.
    pub fn make_leader(&mut self, id: PartyId, by: u32, target: u32) -> bool {
        let Some(party) = self.parties.get_mut(&id) else {
            return false;
        };
        if !party.is_leader(by) || by == target {
            return false;
        }
        let Some(index) = party.members.iter().position(|m| m.account_lo == target) else {
            return false;
        };
        let member = party.members.remove(index);
        party.members.insert(0, Arc::clone(&member));
        party.leader = member;
        true
    }

    /// Every member of a party moves into a listed group and takes a free
    /// slot.
    pub fn merge_into(&mut self, from: PartyId, into: PartyId) {
        let members = self
            .parties
            .get(&from)
            .map_or_else(Vec::new, |party| party.members.clone());
        for member in members {
            self.join(&member, into);
            if let Some(group) = self.parties.get_mut(&into) {
                group.take_free_slot(member.account_lo);
            }
        }
    }

    /// The bot leads a listed group of its own, so the group finder has
    /// someone else's group. Returns it and the group the bot left, if any.
    pub fn list_bot_group(
        &mut self,
        bot: &Arc<Account>,
        listing: Fields,
    ) -> io::Result<(PartyId, Option<PartyId>)> {
        let mut left = None;
        if let Some(id) = self.party_id_of(bot.account_lo) {
            if !self.parties[&id].is_leader(bot.account_lo) {
                left = self.leave(bot.account_lo);
            }
        }
        let id = self.party_of(bot)?;
        let party = self.parties.get_mut(&id).expect("the bot's party");
        party.listing = Some(listing);
        party.searching = true;
        party.slot_types.clear();
        party.take_free_slot(bot.account_lo);
        Ok((id, left))
    }

    /// The session's party chat channel after a party state: joined when the
    /// account is in a group, left when it is alone again
    /// (`ow174/lobby/session.py` party_messages).
    pub fn party_channel_change(&mut self, account_lo: u32, id: PartyId) -> ChannelChange {
        let in_group = self
            .parties
            .get(&id)
            .is_some_and(|party| party.members.len() > 1);
        let Some(session) = self.sessions.get_mut(&account_lo) else {
            return ChannelChange::None;
        };
        match session.party_channel {
            Some(joined) if !in_group => {
                session.party_channel = None;
                ChannelChange::Leave(channel(joined, CHANNEL_GROUP))
            }
            joined if in_group && joined != Some(id) => {
                session.party_channel = Some(id);
                ChannelChange::Join(channel(id, CHANNEL_GROUP))
            }
            _ => ChannelChange::None,
        }
    }

    /// Accounts whose session picked this group in the group finder. Those
    /// that saw it close forget it.
    pub fn watchers(&mut self, id: PartyId, closed: bool) -> Vec<u32> {
        let mut watchers = Vec::new();
        for account_lo in self.online.keys() {
            if let Some(session) = self.sessions.get_mut(account_lo) {
                let watching = if closed {
                    session.watched_groups.remove(&id)
                } else {
                    session.watched_groups.contains(&id)
                };
                if watching {
                    watchers.push(*account_lo);
                }
            }
        }
        watchers
    }
}

// --- friends -----------------------------------------------------------------

/// What a friend request by BattleTag did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FriendRequest {
    Sent,
    /// The target had asked first: a request back means yes.
    Added,
    /// Asked before.
    Pending,
    Unknown,
    ToSelf,
    /// Already friends (the bot always is).
    Already,
}

/// The bot and the saved accounts in the player's friends list.
pub fn friends_of(accounts: &Accounts, me: &Account) -> io::Result<Vec<Arc<Account>>> {
    let saved: HashMap<String, String> = accounts
        .all_saved()?
        .into_iter()
        .map(|name| (name.to_lowercase(), name))
        .collect();
    let names = me.profile().friends.clone();
    let mut friends = vec![Arc::clone(&accounts.bot)];
    for name in names {
        if let Some(saved) = saved.get(&name.to_lowercase()) {
            friends.push(accounts.get(saved)?);
        }
    }
    Ok(friends)
}

pub fn is_friend(accounts: &Accounts, me: &Account, other: &Account) -> io::Result<bool> {
    Ok(friends_of(accounts, me)?
        .iter()
        .any(|friend| friend.account_lo == other.account_lo))
}

/// Ask the owner of a BattleTag to be friends.
pub fn request_friend(
    accounts: &Accounts,
    me: &Arc<Account>,
    tag: &str,
) -> io::Result<(Option<Arc<Account>>, FriendRequest)> {
    let Some(target) = accounts.by_battle_tag(tag)? else {
        return Ok((None, FriendRequest::Unknown));
    };
    if target.account_lo == me.account_lo {
        return Ok((Some(target), FriendRequest::ToSelf));
    }
    if is_friend(accounts, me, &target)? {
        return Ok((Some(target), FriendRequest::Already));
    }
    if target.profile().friend_requests.contains(&me.name) {
        return Ok((Some(target), FriendRequest::Pending));
    }
    if me.profile().friend_requests.contains(&target.name) {
        accept_friend(me, &target)?;
        return Ok((Some(target), FriendRequest::Added));
    }
    target.profile().friend_requests.push(me.name.clone());
    target.save()?;
    Ok((Some(target), FriendRequest::Sent))
}

/// Make two accounts friends and drop the request between them.
pub fn accept_friend(me: &Account, inviter: &Account) -> io::Result<()> {
    for (account, other) in [(me, inviter), (inviter, me)] {
        {
            let mut profile = account.profile();
            let other_name = other.name.to_lowercase();
            profile
                .friend_requests
                .retain(|name| name.to_lowercase() != other_name);
            if !profile.friends.contains(&other.name) {
                profile.friends.push(other.name.clone());
            }
        }
        account.save()?;
    }
    Ok(())
}

pub fn decline_friend(me: &Account, inviter: &Account) -> io::Result<()> {
    let other_name = inviter.name.to_lowercase();
    me.profile()
        .friend_requests
        .retain(|name| name.to_lowercase() != other_name);
    me.save()
}

pub fn remove_friend(me: &Account, friend: &Account) -> io::Result<()> {
    for (account, other) in [(me, friend), (friend, me)] {
        let other_name = other.name.to_lowercase();
        account
            .profile()
            .friends
            .retain(|name| name.to_lowercase() != other_name);
        account.save()?;
    }
    Ok(())
}

// --- values ------------------------------------------------------------------

pub fn channel(id: PartyId, kind: u64) -> Fields {
    Fields::from([
        (0, Value::Struct(id16(id[0], id[1]))),
        (0x10, Value::Unsigned(kind)),
        (0x14, Value::Unsigned(0)),
    ])
}

/// A two-word id struct `{+0x0, +0x8}`.
pub fn id_of(value: Option<&Value>) -> Option<PartyId> {
    let Some(Value::Struct(id)) = value else {
        return None;
    };
    Some([unsigned(id.get(&0)), unsigned(id.get(&8))])
}

/// The account at `+0x0` of an account id struct.
pub fn account_lo_of(value: Option<&Value>) -> Option<u32> {
    let Some(Value::Struct(id)) = value else {
        return None;
    };
    u32::try_from(unsigned(id.get(&0))).ok()
}

pub fn integer(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Unsigned(value)) => i64::try_from(*value).unwrap_or(i64::MAX),
        Some(Value::Signed(value)) => *value,
        Some(Value::Bool(value)) => i64::from(*value),
        _ => 0,
    }
}

pub fn unsigned(value: Option<&Value>) -> u64 {
    match value {
        Some(Value::Unsigned(value)) => *value,
        Some(Value::Signed(value)) => u64::try_from(*value).unwrap_or(0),
        Some(Value::Bool(value)) => u64::from(*value),
        _ => 0,
    }
}

pub fn text(value: Option<&Value>) -> &str {
    match value {
        Some(Value::String(text)) => text,
        _ => "",
    }
}

pub fn structure(value: Option<&Value>) -> Option<&Fields> {
    match value {
        Some(Value::Struct(fields)) => Some(fields),
        _ => None,
    }
}

pub fn array(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(values)) => values,
        _ => &[],
    }
}

fn random_u64() -> io::Result<u64> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(u64::from_le_bytes(bytes))
}
