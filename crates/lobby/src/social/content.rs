//! The social messages that carry player content: cards, presence and party
//! states (`ow174/services/social.py` with `ow174/content/player.py`).

use std::{io, sync::Arc, time::Duration};

use lucy_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Struct},
};

use super::{Party, PartyId, Social, SocialState, friends_of, messages};
use crate::{
    accounts::{Account, Accounts},
    collection::Collection,
    login::Identity,
    party::{self, Membership, PartyMember},
    player_messages::{friend_card, record},
    presence::{Presence, View},
    queue::{QueueFields, add_to_state},
    ranked::{LeaderboardPlaces, Ranked},
    transport::LobbyMessage,
};

/// What the social messages are built from, at one moment.
pub struct SocialContent<'a> {
    pub social: &'a Social,
    pub collection: &'a Collection,
    pub ranked: &'a Ranked,
    pub presence: &'a Presence,
    /// An account's Top 500 places, for the ratings its party tile shows.
    pub places: PlacesOf<'a>,
    /// Seconds since 1970.
    pub wall_time: u64,
    /// The lobby's clock.
    pub now: Duration,
    /// The role queue a party is in, from the matchmaker: its entry and each
    /// member's choice go into the party state (`content/queue.py`).
    pub queue: QueueOf<'a>,
}

/// An account's Top 500 places (`LobbyServer._top500_places`).
pub type PlacesOf<'a> = Box<dyn Fn(&Account) -> io::Result<LeaderboardPlaces> + 'a>;

/// The role queue of a party (its id and members), when it is in one.
pub type QueueOf<'a> = Box<dyn Fn(PartyId, &[Arc<Account>]) -> Option<QueueFields> + 'a>;

impl SocialContent<'_> {
    /// The player card: ids, icon, portrait frame, level and name.
    pub fn record(&self, account: &Account) -> Fields {
        let profile = account.profile();
        record(
            &profile,
            Identity::for_account(account.account_lo),
            self.collection.portrait_frame(&profile),
            self.wall_time,
        )
    }

    /// A friend's card for the friends list (20809): the client caches it, so
    /// the friend shows an icon, level and endorsement.
    pub fn friend_card(&self, account: &Account) -> Fields {
        let profile = account.profile();
        friend_card(
            &profile,
            Identity::for_account(account.account_lo),
            self.collection.portrait_frame(&profile),
            self.wall_time,
        )
    }

    /// The account's presence as its friends see it.
    pub fn presence(&self, state: &SocialState, account: &Account) -> io::Result<Vec<Value>> {
        let status = state.effective_status(account);
        self.records(account, status, View::Friend)
    }

    /// The account's presence as its own client sees it: the status picked in
    /// the dropdown.
    pub fn own_presence(&self, account: &Account) -> io::Result<Vec<Value>> {
        self.records(account, account.status(), View::Own)
    }

    fn records(&self, account: &Account, status: u8, view: View) -> io::Result<Vec<Value>> {
        let profile = account.profile();
        self.presence.records(
            &profile,
            account.account_lo,
            status,
            account.created,
            self.wall_time,
            view,
        )
    }

    /// 27100: friends, incoming friend requests, and presence. Offline
    /// friends get presence too, marked offline: without it the client leaves
    /// them out.
    pub fn friends_list(
        &self,
        state: &SocialState,
        accounts: &Accounts,
        me: &Account,
    ) -> io::Result<LobbyMessage> {
        let mut entries = Vec::new();
        let mut records = self.own_presence(me)?;
        for friend in friends_of(accounts, me)? {
            entries.push(messages::friend_entry(friend.account_id_fields()));
            records.extend(self.presence(state, &friend)?);
        }
        let names = me.profile().friend_requests.clone();
        let mut requests = Vec::with_capacity(names.len());
        for name in names {
            let inviter = accounts.get(&name)?;
            requests.push(self.request_record(me, &inviter));
        }
        Ok(messages::friends_list(entries, requests, records))
    }

    /// One friend request to `me`.
    pub fn request_record(&self, me: &Account, inviter: &Account) -> Value {
        messages::request_record(
            (
                inviter.account_id_fields(),
                inviter.account_lo,
                inviter.battle_tag(),
            ),
            (me.account_id_fields(), me.battle_tag()),
        )
    }

    /// 20809: the cards of the player's friends who are online.
    pub fn friend_cards(
        &self,
        state: &SocialState,
        accounts: &Accounts,
        me: &Account,
    ) -> io::Result<LobbyMessage> {
        let cards = friends_of(accounts, me)?
            .iter()
            .filter(|friend| state.is_online(friend))
            .map(|friend| self.friend_card(friend))
            .collect();
        Ok(messages::friend_cards(cards))
    }

    /// Presence records of the player's friends who are offline.
    pub fn offline_presence(
        &self,
        state: &SocialState,
        accounts: &Accounts,
        me: &Account,
    ) -> io::Result<Vec<Value>> {
        let mut records = Vec::new();
        for friend in friends_of(accounts, me)? {
            if !state.is_online(&friend) {
                records.extend(self.presence(state, &friend)?);
            }
        }
        Ok(records)
    }

    /// 20700 for the party. Players invited and not answered yet come last,
    /// as pending tiles.
    pub fn party_state(&self, party: &Party) -> io::Result<LobbyMessage> {
        let invitees: Vec<_> = party
            .invites
            .iter()
            .map(|invite| {
                (
                    Arc::clone(&invite.invitee),
                    Membership::Invitee {
                        remaining_ms: invite.remaining_ms(self.now),
                    },
                )
            })
            .collect();
        let state = self.state(party, &invitees)?;
        let mut message = (party::PARTY, 20700, Fields::from([(0x78, Struct(state))]));
        if let Some(queue) = (self.queue)(party.id, &party.members) {
            let accounts: Vec<_> = party
                .members
                .iter()
                .map(|member| Some(member.account_lo))
                .chain(invitees.iter().map(|_| None))
                .collect();
            add_to_state(&mut message, &queue, &accounts);
        }
        Ok(message)
    }

    /// A listed party as the group finder shows it (52300, 52301): its party
    /// state without the pending tiles.
    pub fn group(&self, party: &Party) -> io::Result<Fields> {
        self.state(party, &[])
    }

    fn state(&self, party: &Party, invitees: &[(Arc<Account>, Membership)]) -> io::Result<Fields> {
        let members: Vec<_> = party
            .members
            .iter()
            .enumerate()
            .map(|(index, member)| {
                let membership = if index == 0 {
                    Membership::Leader
                } else {
                    Membership::Member
                };
                (Arc::clone(member), membership)
            })
            .chain(invitees.iter().cloned())
            .collect();
        let mut tiles = Vec::with_capacity(members.len());
        for (account, membership) in &members {
            let profile = account.profile().clone();
            let menu_hero = if matches!(membership, Membership::Invitee { .. }) {
                0
            } else {
                self.social
                    .menu_hero(
                        account.account_lo,
                        &profile,
                        self.collection.heroes(),
                        self.collection.items(),
                    )?
                    .party_hero
            };
            let ratings = self
                .ranked
                .party_ratings(&profile, &(self.places)(account)?)?;
            tiles.push((profile, account.account_lo, menu_hero, ratings, *membership));
        }
        let tiles: Vec<_> = tiles
            .iter()
            .map(
                |(profile, account_lo, menu_hero, ratings, membership)| PartyMember {
                    profile,
                    identity: Identity::for_account(*account_lo),
                    menu_hero: *menu_hero,
                    ratings,
                    membership: *membership,
                },
            )
            .collect();
        let (_, _, mut message) = party::state(
            self.collection,
            party.id,
            party.entity,
            &tiles,
            party.members_may_invite(),
            self.wall_time,
        )?;
        let Some(Struct(mut state)) = message.remove(&0x78) else {
            unreachable!("party::state builds a struct");
        };
        if let Some(listing) = &party.listing {
            state.insert(0x30, Array(vec![Struct(listing.clone())]));
            // +0x9A on: the group is looking for players. The client then
            // refreshes its search (0x7FF78975F2A0), and the leader's panel
            // button reads "Done" and closes the search with 52202. Off, the
            // panel offers "Find more players".
            state.insert(0x9a, Bool(party.searching));
            if let Some(Array(records)) = state.get_mut(&0) {
                for (record, member) in records.iter_mut().zip(&party.members) {
                    let (Value::Struct(record), Some(slots)) =
                        (record, party.slot_types.get(&member.account_lo))
                    else {
                        continue;
                    };
                    if !slots.is_empty() {
                        record.insert(0xa0, Array(vec![messages::group_slot(slots)]));
                    }
                }
            }
        }
        Ok(state)
    }
}
