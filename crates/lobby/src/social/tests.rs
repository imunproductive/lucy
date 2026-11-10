use super::*;

use std::time::Duration;

use orpheus_wire::jam::codec::{
    Schemas,
    Value::{Array, Bool, Struct, Unsigned},
};
use serde_json::json;

use crate::{
    collection::Collection,
    presence::{FRIENDS, Presence, STATUS_OFFLINE, STATUS_ONLINE},
    ranked::{COMPETITIVE_CTF, LUCIO_CUP, LeaderboardPlaces, Ranked},
    social::{
        content::SocialContent,
        messages::{GROUPS, group_slot},
    },
    templates::{LOBBY, RetailTemplates},
    transport::LobbyMessage,
};

struct World {
    _directory: tempfile::TempDir,
    accounts: Accounts,
    collection: Collection,
    ranked: Ranked,
    presence: Presence,
    places: LeaderboardPlaces,
    social: Social,
    schemas: Schemas,
}

impl World {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let accounts = Accounts::new(
            directory.path().join("profiles"),
            directory.path().join("template.json"),
        )
        .unwrap();
        let small = RetailTemplates::from_reader(
            br#"{"format":1,"server":[
                ["70E5A247",24900,{"+0x80":[],"+0x98":[]}],
                ["779A8581",24300,{"+0x78":{"+0x30":[]},"+0xF8":{"+0x0":[]}}]
            ]}"#
            .as_slice(),
        )
        .unwrap();
        let captured = RetailTemplates::captured();
        Self {
            _directory: directory,
            collection: Collection::new(&small, Arc::new(ItemDb::default())).unwrap(),
            ranked: Ranked::new(&captured).unwrap(),
            presence: Presence::new(&captured).unwrap(),
            places: LeaderboardPlaces::new(),
            social: Social::default(),
            schemas: Schemas::lobby(),
            accounts,
        }
    }

    /// A saved account without a party card hero (the test catalog has none).
    fn account(&self, name: &str) -> Arc<Account> {
        let account = self.accounts.get(name).unwrap();
        account.profile().lobby_hero = "none".into();
        account.save().unwrap();
        account
    }

    fn bot(&self) -> Arc<Account> {
        let bot = Arc::clone(&self.accounts.bot);
        bot.profile().lobby_hero = "none".into();
        bot
    }

    fn content(&self, now: Instant) -> SocialContent<'_> {
        SocialContent {
            social: &self.social,
            collection: &self.collection,
            ranked: &self.ranked,
            presence: &self.presence,
            places: &self.places,
            wall_time: 1_700_000_000,
            now,
        }
    }

    /// Encode and decode, as the client would read it.
    fn wire(&self, (crc, id, fields): &LobbyMessage) -> Fields {
        let body = self.schemas.encode(*crc, *id, fields).unwrap();
        self.schemas.decode(*crc, *id, &body, true).unwrap()
    }
}

fn listing(name: &str, slots: &[u64]) -> Fields {
    Fields::from([
        (0xa0, Value::String(name.into())),
        (0x80, Array(slots.iter().copied().map(Unsigned).collect())),
    ])
}

fn list(state: &mut SocialState, leader: &Arc<Account>, listing: Fields) -> PartyId {
    let id = state.party_of(leader).unwrap();
    let party = state.party_mut(id).unwrap();
    party.listing = Some(listing);
    party.searching = true;
    id
}

fn member_names(state: &SocialState, id: PartyId) -> Vec<String> {
    state
        .party(id)
        .map(|party| {
            party
                .members
                .iter()
                .map(|member| member.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn field<'a>(fields: &'a Fields, path: &[u32]) -> &'a Value {
    let (last, parents) = path.split_last().unwrap();
    let mut fields = fields;
    for offset in parents {
        fields = structure(fields.get(offset)).unwrap();
    }
    &fields[last]
}

// --- parties -----------------------------------------------------------------

#[test]
fn a_party_has_one_leader_who_hands_over_on_leaving() {
    let world = World::new();
    let [alpha, beta, gamma] = ["Alpha", "Beta", "Gamma"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let id = state.party_of(&alpha).unwrap();
    assert_eq!(state.party_of(&alpha).unwrap(), id);
    let party = state.party(id).unwrap();
    assert_eq!(
        party.entity[0] & !u64::from(u32::MAX),
        (0x1d << 40) | (1 << 56)
    );
    state.join(&beta, id);
    state.join(&gamma, id);
    assert_eq!(member_names(&state, id), ["Alpha", "Beta", "Gamma"]);

    // Only the leader hands the party over; the new leader moves to the front.
    assert!(!state.make_leader(id, beta.account_lo, gamma.account_lo));
    assert!(!state.make_leader(id, alpha.account_lo, alpha.account_lo));
    assert!(state.make_leader(id, alpha.account_lo, gamma.account_lo));
    assert_eq!(member_names(&state, id), ["Gamma", "Alpha", "Beta"]);
    assert!(state.party(id).unwrap().is_leader(gamma.account_lo));

    // The first member leads when the leader leaves; an empty party is gone.
    assert_eq!(state.leave(gamma.account_lo), Some(id));
    assert!(state.party(id).unwrap().is_leader(alpha.account_lo));
    state.leave(alpha.account_lo);
    state.leave(beta.account_lo);
    assert!(state.party(id).is_none());
    assert_eq!(state.leave(beta.account_lo), None);
    // The next use makes a new party of one.
    assert_ne!(state.party_of(&beta).unwrap(), id);
}

#[test]
fn joining_another_party_leaves_the_old_one_and_drops_the_invite() {
    let world = World::new();
    let [alpha, beta, gamma] = ["Alpha", "Beta", "Gamma"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let now = Instant::now();
    let theirs = state.party_of(&gamma).unwrap();
    state.join(&beta, theirs);
    let ours = state.invite(&alpha, &beta, now).unwrap();
    assert_eq!(state.party(ours).unwrap().invites.len(), 1);
    state.join(&beta, ours);
    assert!(state.party(ours).unwrap().invites.is_empty());
    assert_eq!(member_names(&state, ours), ["Alpha", "Beta"]);
    assert_eq!(member_names(&state, theirs), ["Gamma"]);
}

#[test]
fn an_unanswered_invite_times_out_and_a_new_one_restarts_the_timer() {
    let world = World::new();
    let [alpha, beta, gamma] = ["Alpha", "Beta", "Gamma"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let start = Instant::now();
    let id = state.invite(&alpha, &beta, start).unwrap();
    state.invite(&alpha, &gamma, start).unwrap();
    let later = start + Duration::from_secs(20);
    // A new invite replaces the old one in its place.
    state.invite(&alpha, &beta, later).unwrap();
    let invites = &state.party(id).unwrap().invites;
    assert_eq!(invites.len(), 2);
    assert_eq!(invites[0].invitee.name, "Beta");
    assert_eq!(invites[0].remaining_ms(later), 25_000);
    assert_eq!(invites[1].remaining_ms(later), 5_000);

    assert!(
        state
            .expire_invites(start + Duration::from_secs(24))
            .is_empty()
    );
    assert_eq!(state.expire_invites(start + INVITE_TIME), [id]);
    let invites = &state.party(id).unwrap().invites;
    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0].invitee.name, "Beta");
    assert!(state.cancel_invite(id, beta.account_lo).is_some());
    assert!(state.cancel_invite(id, beta.account_lo).is_none());
}

#[test]
fn the_party_chat_channel_follows_group_membership() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let now = Instant::now();
    state.connect(&alpha, now);
    let id = state.party_of(&alpha).unwrap();
    assert_eq!(
        state.party_channel_change(alpha.account_lo, id),
        ChannelChange::None
    );
    state.join(&beta, id);
    let channel = state.party(id).unwrap().chat_channel();
    assert_eq!(
        state.party_channel_change(alpha.account_lo, id),
        ChannelChange::Join(channel.clone())
    );
    assert_eq!(
        state.party_channel_change(alpha.account_lo, id),
        ChannelChange::None
    );
    state.leave(beta.account_lo);
    assert_eq!(
        state.party_channel_change(alpha.account_lo, id),
        ChannelChange::Leave(channel)
    );
    // A new session starts without a party channel.
    state.join(&beta, id);
    state.connect(&alpha, now);
    assert!(matches!(
        state.party_channel_change(alpha.account_lo, id),
        ChannelChange::Join(_)
    ));
}

// --- chat and presence -------------------------------------------------------

#[test]
fn channel_members_are_general_the_party_or_the_match() {
    let world = World::new();
    let [alpha, beta, gamma] = ["Alpha", "Beta", "Gamma"].map(|name| world.account(name));
    let bot = world.bot();
    let mut state = SocialState::default();
    let now = Instant::now();
    for account in [&alpha, &beta, &gamma] {
        state.connect(account, now);
    }
    let names = |members: Vec<Arc<Account>>| {
        members
            .iter()
            .map(|member| member.name.clone())
            .collect::<Vec<_>>()
    };
    let general = crate::handoff::general_channel();
    assert_eq!(
        names(state.channel_members(&general, &bot)),
        ["Alpha", "Beta", "Gamma", "Bot"]
    );

    let id = state.invite(&alpha, &bot, now).unwrap();
    state.join(&bot, id);
    let party = state.party(id).unwrap().chat_channel();
    assert_eq!(names(state.channel_members(&party, &bot)), ["Alpha", "Bot"]);

    let first = [1, 2];
    let second = [3, 4];
    state.open_match_chat(first, alpha.account_lo);
    state.open_match_chat(first, gamma.account_lo);
    let matched = channel(first, CHANNEL_MATCH);
    assert_eq!(
        names(state.channel_members(&matched, &bot)),
        ["Alpha", "Gamma"]
    );

    // Leaving an old match keeps the chat of a newer one.
    state.open_match_chat(second, alpha.account_lo);
    assert_eq!(state.leave_match_chat(alpha.account_lo, Some(first)), None);
    assert_eq!(
        state.leave_match_chat(alpha.account_lo, Some(second)),
        Some(channel(second, CHANNEL_MATCH))
    );
    assert_eq!(state.match_chat_of(alpha.account_lo), None);
    assert_eq!(state.leave_match_chat(alpha.account_lo, Some(second)), None);

    state.disconnect(beta.account_lo);
    assert_eq!(
        names(state.channel_members(&general, &bot)),
        ["Alpha", "Gamma", "Bot"]
    );
}

#[test]
fn a_seat_is_seen_once() {
    let world = World::new();
    let alpha = world.account("Alpha");
    let mut state = SocialState::default();
    state.connect(&alpha, Instant::now());
    assert!(state.seat_seen(alpha.account_lo, [1, 2]));
    assert!(!state.seat_seen(alpha.account_lo, [1, 2]));
    assert!(state.seat_seen(alpha.account_lo, [3, 4]));
}

#[test]
fn offline_presence_goes_again_at_3_s_then_7_s_then_every_minute() {
    let world = World::new();
    let alpha = world.account("Alpha");
    let mut state = SocialState::default();
    let login = Instant::now();
    state.connect(&alpha, login);
    let lo = alpha.account_lo;
    let at = |seconds| login + Duration::from_secs(seconds);
    assert!(!state.presence_due(lo, at(2)));
    assert!(state.presence_due(lo, at(3)));
    assert!(!state.presence_due(lo, at(9)));
    assert!(state.presence_due(lo, at(10)));
    assert!(!state.presence_due(lo, at(69)));
    assert!(state.presence_due(lo, at(70)));
    assert!(state.presence_due(lo, at(130)));
    state.disconnect(lo);
    assert!(!state.presence_due(lo, at(1000)));
}

#[test]
fn friends_see_the_chosen_status_and_the_bot_always_online() {
    let world = World::new();
    let alpha = world.account("Alpha");
    let mut state = SocialState::default();
    assert_eq!(state.effective_status(&alpha), STATUS_OFFLINE);
    assert_eq!(state.effective_status(&world.bot()), STATUS_ONLINE);
    state.connect(&alpha, Instant::now());
    alpha.set_status(crate::presence::STATUS_BUSY);
    assert_eq!(state.effective_status(&alpha), crate::presence::STATUS_BUSY);
    assert!(state.is_online(&world.bot()));
}

// --- friends -----------------------------------------------------------------

#[test]
fn a_request_waits_then_a_request_back_means_yes_and_is_saved() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    let accounts = &world.accounts;
    let names = |account: &Account| -> Vec<String> {
        friends_of(accounts, account)
            .unwrap()
            .iter()
            .map(|friend| friend.name.clone())
            .collect()
    };
    assert_eq!(names(&alpha), ["Bot"]);

    let tag = beta.battle_tag().to_uppercase();
    let (target, outcome) = request_friend(accounts, &alpha, &tag).unwrap();
    assert_eq!(outcome, FriendRequest::Sent);
    assert_eq!(target.unwrap().account_lo, beta.account_lo);
    assert_eq!(beta.profile().friend_requests, ["Alpha"]);
    let (_, outcome) = request_friend(accounts, &alpha, &beta.battle_tag()).unwrap();
    assert_eq!(outcome, FriendRequest::Pending);

    let (_, outcome) = request_friend(accounts, &beta, &alpha.battle_tag()).unwrap();
    assert_eq!(outcome, FriendRequest::Added);
    assert_eq!(names(&alpha), ["Bot", "Beta"]);
    assert_eq!(names(&beta), ["Bot", "Alpha"]);
    assert!(beta.profile().friend_requests.is_empty());
    let saved = crate::accounts::load_or_create_profile(beta.path.as_deref().unwrap()).unwrap();
    assert_eq!(saved.friends, ["Alpha"]);

    for (tag, expected) in [
        (beta.battle_tag(), FriendRequest::Already),
        (world.bot().battle_tag(), FriendRequest::Already),
        (alpha.battle_tag(), FriendRequest::ToSelf),
        ("Nobody#1234".into(), FriendRequest::Unknown),
        ("Beta".into(), FriendRequest::Unknown),
    ] {
        assert_eq!(request_friend(accounts, &alpha, &tag).unwrap().1, expected);
    }

    remove_friend(&alpha, &beta).unwrap();
    assert_eq!(names(&alpha), ["Bot"]);
    assert_eq!(names(&beta), ["Bot"]);
}

#[test]
fn a_declined_request_only_leaves_the_list() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    request_friend(&world.accounts, &alpha, &beta.battle_tag()).unwrap();
    decline_friend(&beta, &alpha).unwrap();
    assert!(beta.profile().friend_requests.is_empty());
    assert!(!is_friend(&world.accounts, &beta, &alpha).unwrap());
}

// --- group finder ------------------------------------------------------------

#[test]
fn a_listed_group_is_found_until_it_is_full() {
    let world = World::new();
    let [alpha, beta, gamma] = ["Alpha", "Beta", "Gamma"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let solo = state.party_of(&gamma).unwrap();
    let id = list(
        &mut state,
        &alpha,
        listing("Tanks wanted", &[ANY_SLOT, ANY_SLOT]),
    );
    state.join(&beta, id);
    assert_eq!(state.listed_groups(), [id]);
    assert_ne!(solo, id);

    let party = state.party_mut(id).unwrap();
    party.take_free_slot(alpha.account_lo);
    assert_eq!(party.free_slot_types(), [ANY_SLOT]);
    assert!(party.searching);
    party.take_free_slot(beta.account_lo);
    assert!(party.free_slot_types().is_empty());
    // A full group stops looking for players.
    assert!(!party.searching);
    assert!(state.listed_groups().is_empty());
}

#[test]
fn search_filters_pick_the_groups() {
    let world = World::new();
    let alpha = world.account("Alpha");
    let mut state = SocialState::default();
    let mut entry = listing("Tanks wanted", &[ANY_SLOT, ANY_SLOT]);
    entry.extend([
        (0x76, Unsigned(1)),
        (0x77, Bool(false)),
        (0x70, Unsigned(2)),
        (
            0,
            Struct(Fields::from([(0x40, Value::String("Alpha#1234".into()))])),
        ),
    ]);
    let id = list(&mut state, &alpha, entry);
    let found = |state: &SocialState, filters: &[(u32, Value)]| {
        let wanted: Fields = filters.iter().cloned().collect();
        matches_filters(state.party(id).unwrap(), &wanted)
    };
    let text = |text: &str| {
        Array(vec![Struct(Fields::from([(
            0,
            Value::String(text.into()),
        )]))])
    };
    assert!(found(&state, &[]));
    assert!(found(&state, &[(0x8, Unsigned(1)), (0xc, Unsigned(1))]));
    assert!(!found(&state, &[(0x8, Unsigned(2)), (0xc, Unsigned(2))]));
    state
        .party_mut(id)
        .unwrap()
        .listing
        .as_mut()
        .unwrap()
        .insert(0x76, Unsigned(4));
    assert!(found(&state, &[(0x8, Unsigned(3)), (0xc, Unsigned(4))]));
    state
        .party_mut(id)
        .unwrap()
        .listing
        .as_mut()
        .unwrap()
        .insert(0x76, Unsigned(1));
    assert!(found(&state, &[(0xd, Unsigned(1))]));
    assert!(!found(&state, &[(0xd, Unsigned(2))]));
    assert!(found(&state, &[(0xf, Unsigned(2))]));
    assert!(!found(&state, &[(0xf, Unsigned(3))]));
    assert!(!found(&state, &[(0x10, Unsigned(2))]));
    assert!(found(&state, &[(0x12, Unsigned(2))]));
    assert!(!found(&state, &[(0x12, Unsigned(3))]));
    assert!(found(&state, &[(0x15, Unsigned(2))]));
    assert!(found(&state, &[(0x18, Array(vec![Unsigned(DAMAGE_SLOT)]))]));
    assert!(found(&state, &[(0x30, text("TANKS"))]));
    assert!(found(&state, &[(0x30, text("alpha"))]));
    assert!(!found(&state, &[(0x30, text("healers"))]));
}

#[test]
fn group_limits_hide_and_refuse_players_outside_them() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let mut entry = listing("Tanks wanted", &[ANY_SLOT, ANY_SLOT]);
    entry.insert(0x70, Unsigned(3));
    let id = list(&mut state, &alpha, entry);
    // The minimum endorsement level.
    assert!(!state.party(id).unwrap().can_join(&beta).unwrap());
    assert!(state.party(id).unwrap().can_join(&world.bot()).unwrap());
    beta.profile().endorsement_level = 3;
    assert!(state.party(id).unwrap().can_join(&beta).unwrap());

    // A competitive group's spread compares the best role ratings.
    let set = |account: &Account, ratings: serde_json::Value| {
        account.profile().ratings = serde_json::from_value(ratings).unwrap();
    };
    let listing = state.party_mut(id).unwrap().listing.as_mut().unwrap();
    listing.extend([(0x76, Unsigned(2)), (0x74, Unsigned(150))]);
    set(
        &alpha,
        json!({"tank": 2500, "damage": 2100, "support": 2000}),
    );
    set(
        &beta,
        json!({"tank": 1800, "damage": 2600, "support": 2400}),
    );
    assert!(state.party(id).unwrap().can_join(&beta).unwrap());
    set(
        &beta,
        json!({"tank": 1800, "damage": 2700, "support": 2400}),
    );
    assert!(!state.party(id).unwrap().can_join(&beta).unwrap());

    // A competitive Arcade card compares its own rating.
    let listing = state.party_mut(id).unwrap().listing.as_mut().unwrap();
    listing.extend([(0x76, Unsigned(3)), (0x68, Unsigned(LUCIO_CUP))]);
    set(&alpha, json!({"lucio": 2500}));
    set(&beta, json!({"lucio": 2600, "tank": 4000}));
    let party = state.party(id).unwrap();
    assert!(party.can_join(&beta).unwrap());
    let card = |card| Fields::from([(0xc, Unsigned(3)), (0, Unsigned(card))]);
    assert!(matches_filters(party, &card(LUCIO_CUP)));
    assert!(!matches_filters(party, &card(COMPETITIVE_CTF)));
    set(&beta, json!({"lucio": 2700}));
    assert!(!party.can_join(&beta).unwrap());
}

#[test]
fn the_bot_group_takes_the_first_slot_and_a_merge_the_next_ones() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    let bot = world.bot();
    let mut state = SocialState::default();
    let slots = [
        TANK_SLOT,
        TANK_SLOT,
        DAMAGE_SLOT,
        DAMAGE_SLOT,
        SUPPORT_SLOT,
        SUPPORT_SLOT,
    ];
    // The bot in a player's party leaves it to lead its own group.
    let theirs = state.invite(&beta, &bot, Instant::now()).unwrap();
    state.join(&bot, theirs);
    let (group, left) = state
        .list_bot_group(&bot, listing("Bot's group", &slots))
        .unwrap();
    assert_eq!(left, Some(theirs));
    assert_eq!(member_names(&state, theirs), ["Beta"]);
    assert_eq!(
        state.party(group).unwrap().slot_types[&bot.account_lo],
        [TANK_SLOT]
    );
    // Listed again, it keeps its group.
    let (again, left) = state
        .list_bot_group(&bot, listing("Bot's group", &slots))
        .unwrap();
    assert_eq!((again, left), (group, None));

    let mine = state.party_of(&alpha).unwrap();
    state.join(&beta, mine);
    state.merge_into(mine, group);
    assert!(state.party(mine).is_none());
    assert_eq!(member_names(&state, group), ["Bot", "Alpha", "Beta"]);
    let party = state.party(group).unwrap();
    assert_eq!(party.slot_types[&alpha.account_lo], [TANK_SLOT]);
    assert_eq!(party.slot_types[&beta.account_lo], [DAMAGE_SLOT]);
    assert_eq!(
        party.free_slot_types(),
        [DAMAGE_SLOT, SUPPORT_SLOT, SUPPORT_SLOT]
    );
}

#[test]
fn watchers_hear_a_group_until_it_closes() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let now = Instant::now();
    state.connect(&alpha, now);
    state.connect(&beta, now);
    let id = list(&mut state, &alpha, listing("Tanks wanted", &[ANY_SLOT]));
    assert!(state.watchers(id, false).is_empty());
    state
        .session_mut(beta.account_lo)
        .unwrap()
        .watched_groups
        .insert(id);
    assert_eq!(state.watchers(id, false), [beta.account_lo]);
    assert_eq!(state.watchers(id, true), [beta.account_lo]);
    // Closed, it is forgotten.
    assert!(state.watchers(id, true).is_empty());
}

// --- messages ----------------------------------------------------------------

#[test]
fn the_party_state_carries_the_listing_slots_and_pending_tiles() {
    let world = World::new();
    let [alpha, beta, gamma] = ["Alpha", "Beta", "Gamma"].map(|name| world.account(name));
    let mut state = SocialState::default();
    let now = Instant::now();
    let id = list(
        &mut state,
        &alpha,
        listing("Tanks wanted", &[ANY_SLOT, TANK_SLOT]),
    );
    state.join(&beta, id);
    let party = state.party_mut(id).unwrap();
    party
        .slot_types
        .insert(alpha.account_lo, vec![ANY_SLOT, TANK_SLOT]);
    state.invite(&alpha, &gamma, now).unwrap();
    alpha
        .profile()
        .settings
        .insert(SOCIAL_SETTINGS.into(), json!({"+0x0": false}));

    let content = world.content(now + Duration::from_secs(5));
    let message = content.party_state(state.party(id).unwrap()).unwrap();
    let decoded = world.wire(&message);
    let party = structure(decoded.get(&0x78)).unwrap();
    assert_eq!(party[&0x60], Struct(id16(id[0], id[1])));
    assert_eq!(party[&0x94], Bool(false));
    assert_eq!(party[&0x9a], Bool(true));
    assert_eq!(array(party.get(&0x30)).len(), 1);
    let listed = structure(Some(&array(party.get(&0x30))[0])).unwrap();
    assert_eq!(listed[&0xa0], Value::String("Tanks wanted".into()));
    let tiles = array(party.get(&0));
    assert_eq!(tiles.len(), 3);
    let tile = |index: usize| structure(Some(&tiles[index])).unwrap();
    assert_eq!(tile(0)[&0xe2], Bool(true));
    assert_eq!(
        array(tile(0).get(&0xa0)),
        [group_slot(&[ANY_SLOT, TANK_SLOT])]
    );
    assert!(array(tile(1).get(&0xa0)).is_empty());
    // The invitee comes last, with the time left.
    assert_eq!(tile(2)[&0xe0], Unsigned(1));
    assert_eq!(tile(2)[&0xb8], Unsigned(20_000));

    // The group finder shows the members only.
    let group = content.group(state.party(id).unwrap()).unwrap();
    assert_eq!(array(group.get(&0)).len(), 2);
    let found = world.wire(&messages::groups_found(vec![group.clone()]));
    assert_eq!(array(found.get(&0x78)).len(), 1);
    world.wire(&messages::group_changed(group));
    let closed = world.wire(&messages::group_closed(id));
    assert_eq!(closed[&0x78], Struct(id16(id[0], id[1])));
}

#[test]
fn the_friends_list_has_presence_for_every_friend_and_cards_for_online_ones() {
    let world = World::new();
    let [alpha, beta] = ["Alpha", "Beta"].map(|name| world.account(name));
    request_friend(&world.accounts, &alpha, &beta.battle_tag()).unwrap();
    request_friend(&world.accounts, &beta, &alpha.battle_tag()).unwrap();
    let gamma = world.account("Gamma");
    request_friend(&world.accounts, &gamma, &alpha.battle_tag()).unwrap();
    let mut state = SocialState::default();
    let now = Instant::now();
    state.connect(&alpha, now);
    let content = world.content(now);

    let list = world.wire(
        &content
            .friends_list(&state, &world.accounts, &alpha)
            .unwrap(),
    );
    let entries = array(list.get(&0x78));
    assert_eq!(entries.len(), 2);
    assert_eq!(
        *field(structure(Some(&entries[1])).unwrap(), &[0, 0]),
        Unsigned(u64::from(beta.account_lo))
    );
    let requests = array(list.get(&0x90));
    let request = structure(Some(&requests[0])).unwrap();
    assert_eq!(request[&0x20], Unsigned(u64::from(gamma.account_lo)));
    assert_eq!(request[&0x30], Value::String(gamma.battle_tag()));
    assert_eq!(request[&0x58], Value::String(alpha.battle_tag()));
    // Own, bot and Beta: three records each.
    assert_eq!(array(list.get(&0xa8)).len(), 9);
    // Beta is offline: in the presence again, not among the cards.
    assert_eq!(
        content
            .offline_presence(&state, &world.accounts, &alpha)
            .unwrap()
            .len(),
        3
    );
    let cards = world.wire(
        &content
            .friend_cards(&state, &world.accounts, &alpha)
            .unwrap(),
    );
    assert_eq!(array(cards.get(&0x78)).len(), 1);
    state.connect(&beta, now);
    let cards = content
        .friend_cards(&state, &world.accounts, &alpha)
        .unwrap();
    assert_eq!(array(cards.2.get(&0x78)).len(), 2);
    assert!(
        content
            .offline_presence(&state, &world.accounts, &alpha)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_other_social_messages_encode_as_the_client_reads_them() {
    let world = World::new();
    let alpha = world.account("Alpha");
    let content = world.content(Instant::now());
    let card = content.record(&alpha);
    let general = Struct(crate::handoff::general_channel());
    let chat = world.wire(&messages::chat_message(
        general.clone(),
        card.clone(),
        "hi",
        2,
    ));
    assert_eq!(chat[&0x100], Value::String("hi".into()));
    assert_eq!(chat[&0x128], Unsigned(2));
    let who = world.wire(&messages::who(general, vec![card.clone()]));
    assert_eq!(array(who.get(&0x90)).len(), 1);
    let id = alpha.account_id_fields();
    for message in [
        messages::whisper_sent(7),
        messages::whisper(id.clone(), "psst"),
        messages::friend_added(messages::friend_entry(id.clone())),
        messages::friend_removed(id.clone()),
        messages::request_received(content.request_record(&alpha, &alpha)),
        messages::request_removed(id.clone()),
        messages::request_result(7, messages::SENT_TEXT),
        messages::answered(7, messages::OK),
        messages::removed(7, messages::FAILED),
        messages::presence(content.own_presence(&alpha).unwrap()),
        messages::invite(card.clone()),
        messages::invite_cancelled(id.clone()),
        messages::suggestion(card.clone(), card.clone()),
        messages::merge_request(card.clone(), Fields::new()),
        messages::party_error(messages::CANNOT_SUGGEST),
        messages::joined(channel([1, 2], CHANNEL_GROUP)),
        messages::left(channel([1, 2], CHANNEL_MATCH)),
    ] {
        world.wire(&message);
    }
    let invite = world.wire(&messages::invite(card));
    assert_eq!(invite[&0x78], Struct(id));
    assert_eq!(messages::party_error(14).0, LOBBY);
    assert_eq!(messages::presence(Vec::new()).0, FRIENDS);
    assert_eq!(messages::group_closed([0, 0]).0, GROUPS);
}
