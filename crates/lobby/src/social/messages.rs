//! The social messages: chat, friends, parties and the group finder
//! (`ow174/lobby/handlers/{chat,friends,party}.py`, the group finder part of
//! `matchmaking.py`, `ow174/lobby/server.py` notify_*).

use orpheus_wire::jam::codec::{
    Fields,
    Value::{self, Array, Bool, Struct, Unsigned},
    id16,
};

use super::PartyId;
use crate::{party::PARTY, presence::FRIENDS, templates::LOBBY, transport::LobbyMessage};

/// 20400 message, 20401 channel members, 20402 joined, 20404 left.
pub const CHAT_IN: u32 = 0x5f91_3f6b;
/// 21700 send {channel, text, flags}, 21701 who.
pub const CHAT_OUT: u32 = 0x28a2_a1cd;
pub const FRIENDS_OUT: u32 = 0xa287_df29;
pub const PARTY_OUT: u32 = 0xb2ff_5a5e;
/// The group finder's requests, 52200-52205.
pub const GROUP_FINDER: u32 = 0x9529_f0ed;
/// The group finder's answers, 52300-52302 (a u32 in the frame header).
pub const GROUPS: u32 = 0xbddb_f58a;

pub const CHAT: u32 = 21700;
pub const WHO: u32 = 21701;
const CHAT_MESSAGE: u32 = 20400;
const CHANNEL_MEMBERS: u32 = 20401;
const JOINED: u32 = 20402;
const LEFT: u32 = 20404;

/// {token, 1, BattleTag, message}; answered by 27110.
pub const SEND_REQUEST: u32 = 27000;
/// {token, inviter id, 0 = accept}; answered by 27111.
pub const ANSWER_REQUEST: u32 = 27001;
/// {token, friend id}; answered by 27112.
pub const REMOVE_FRIEND: u32 = 27002;
/// A Battle.net whisper {token, target, sender, text}.
pub const WHISPER: u32 = 27004;
/// {status}: the status dropdown, 1 online, 2 away, 3 busy, 4 appear offline.
pub const SET_STATUS: u32 = 27011;
const FRIENDS_LIST: u32 = 27100;
/// Changes to the friends list go one at a time. A whole list (27100) makes
/// the client take every online friend as just come online and announce each
/// again.
/// {friend entry} (0x7FF789611AA0).
const FRIEND_ADDED: u32 = 27105;
/// {friend id} (0x7FF789614C40).
const FRIEND_REMOVED: u32 = 27106;
/// Unlike the list, it shows the "friend request" banner.
const REQUEST_RECEIVED: u32 = 27107;
/// {inviter id}: takes a request off the list (0x7FF789614EB0).
const REQUEST_REMOVED: u32 = 27108;
const REQUEST_RESULT: u32 = 27110;
const ANSWERED: u32 = 27111;
const REMOVED: u32 = 27112;
const PRESENCE: u32 = 27113;
const WHISPER_SENT: u32 = 27116;
const WHISPER_RECEIVED: u32 = 27117;
/// Replaces the client's list of friends' cards.
const FRIEND_CARDS: u32 = 20809;
/// 20803 {+0x78 code} shows the client's party error of that code (table
/// 0x7FF78B529290).
const PARTY_ERROR: u32 = 20803;
/// "Unable to suggest the player for the group."
pub const CANNOT_SUGGEST: u64 = 14;
pub const OK: u64 = 0;
pub const FAILED: u64 = 1;
/// Texts (07C) the client shows for a friend request: "Invitation sent.",
/// "This player is already your friend.", "Invalid name ... use the full
/// BattleTag."
pub const SENT_TEXT: u64 = 0x0de0_0000_0000_0365;
pub const ALREADY_FRIENDS_TEXT: u64 = 0x0de0_0000_0000_17b6;
pub const BAD_NAME_TEXT: u64 = 0x0de0_0000_0000_0360;

pub const INVITE_PLAYER: u32 = 22102;
/// Join in the group finder: the group's party id in +0x88.
pub const JOIN_GROUP: u32 = 22103;
pub const INVITE_GROUP: u32 = 22104;
pub const KICK: u32 = 22105;
pub const MAKE_LEADER: u32 = 22106;
pub const LEAVE: u32 = 22107;
pub const ACCEPT_INVITE: u32 = 22108;
pub const DECLINE_INVITE: u32 = 22109;
pub const MERGE_ACCEPT: u32 = 22110;
pub const MERGE_DECLINE: u32 = 22111;
/// 20701 {+0x78 the member's card, +0xE0 the player's card} shows the leader
/// "X suggests Y for the group" with Invite (01B/20AE, 0x7FF7899E1B90).
const SUGGESTION: u32 = 20701;
/// "X wants to merge groups" (05A/066F): {+0x78 the asking leader's card,
/// +0xE0 their group} puts a request on the client's list for 20 seconds
/// (0x7FF789762200).
const MERGE_REQUEST: u32 = 20703;
/// {inviter id, inviter card} shows "Join X's group?".
const INVITE: u32 = 20704;
/// {+0x78 inviter id} takes that invite off the invitee's list again
/// (0x7FF789D2F380).
const INVITE_CANCELLED: u32 = 20705;

pub const FIND_GROUPS: u32 = 52200;
pub const LIST_GROUP: u32 = 52201;
pub const CLOSE_GROUP: u32 = 52202;
pub const GROUP_ROLES: u32 = 52203;
pub const WATCH_GROUPS: u32 = 52204;
pub const UNWATCH_GROUPS: u32 = 52205;
const GROUPS_FOUND: u32 = 52300;
const GROUP_CHANGED: u32 = 52301;
const GROUP_CLOSED: u32 = 52302;

fn one(group: u32, id: u32, value: Value) -> LobbyMessage {
    (group, id, Fields::from([(0x78, value)]))
}

// --- chat --------------------------------------------------------------------

pub fn joined(channel: Fields) -> LobbyMessage {
    one(CHAT_IN, JOINED, Struct(channel))
}

pub fn left(channel: Fields) -> LobbyMessage {
    one(CHAT_IN, LEFT, Struct(channel))
}

/// A channel member: the player card and an unknown 0.
pub fn member(record: Fields) -> Value {
    Struct(Fields::from([(0, Struct(record)), (0x68, Unsigned(0))]))
}

/// 20400 to every member of the channel.
pub fn chat_message(channel: Value, sender: Fields, text: &str, flags: u64) -> LobbyMessage {
    (
        CHAT_IN,
        CHAT_MESSAGE,
        Fields::from([
            (0x78, channel),
            (0x90, member(sender)),
            (0x100, Value::String(text.into())),
            (0x128, Unsigned(flags)),
        ]),
    )
}

/// 20401: the channel's member list, the answer to the client's 21701.
pub fn who(channel: Value, members: Vec<Fields>) -> LobbyMessage {
    (
        CHAT_IN,
        CHANNEL_MEMBERS,
        Fields::from([
            (0x78, channel),
            (0x90, Array(members.into_iter().map(member).collect())),
        ]),
    )
}

/// 27116 completes the whisper request.
pub fn whisper_sent(token: u64) -> LobbyMessage {
    (
        FRIENDS,
        WHISPER_SENT,
        Fields::from([(0x78, Unsigned(token)), (0x80, Unsigned(0))]),
    )
}

/// 27117 {sender, text} to the target.
pub fn whisper(sender: Fields, text: &str) -> LobbyMessage {
    (
        FRIENDS,
        WHISPER_RECEIVED,
        Fields::from([(0x78, Struct(sender)), (0x88, Value::String(text.into()))]),
    )
}

// --- friends -----------------------------------------------------------------

/// One friend of the list (27100 +0x78, 27105). +0x10 is not a time: with
/// the current time in it every friend showed as a favorite.
pub fn friend_entry(friend: Fields) -> Value {
    Struct(Fields::from([
        (0, Struct(friend)),
        (0x10, Unsigned(0)),
        (0x18, Unsigned(0)),
    ]))
}

/// One friend request to `me` (27100 +0x90, 27107): ids, the request id (one
/// per inviter) and both BattleTags.
pub fn request_record(
    (inviter, inviter_lo, inviter_tag): (Fields, u32, String),
    (me, me_tag): (Fields, String),
) -> Value {
    Struct(Fields::from([
        (0, Struct(inviter)),
        (0x10, Struct(me)),
        (0x20, Unsigned(u64::from(inviter_lo))),
        (0x28, Unsigned(0)),
        (0x30, Value::String(inviter_tag)),
        (0x58, Value::String(me_tag)),
    ]))
}

/// 27100: friends, incoming friend requests, and presence. Offline friends
/// need presence too, marked offline: without it the client leaves them out.
pub fn friends_list(
    friends: Vec<Value>,
    requests: Vec<Value>,
    presence: Vec<Value>,
) -> LobbyMessage {
    (
        FRIENDS,
        FRIENDS_LIST,
        Fields::from([
            (0x78, Array(friends)),
            (0x90, Array(requests)),
            (0xa8, Array(presence)),
            (0xc0, Array(Vec::new())),
        ]),
    )
}

pub fn presence(records: Vec<Value>) -> LobbyMessage {
    one(FRIENDS, PRESENCE, Array(records))
}

pub fn friend_cards(cards: Vec<Fields>) -> LobbyMessage {
    one(
        LOBBY,
        FRIEND_CARDS,
        Array(cards.into_iter().map(Struct).collect()),
    )
}

pub fn friend_added(entry: Value) -> LobbyMessage {
    one(FRIENDS, FRIEND_ADDED, entry)
}

pub fn friend_removed(friend: Fields) -> LobbyMessage {
    one(FRIENDS, FRIEND_REMOVED, Struct(friend))
}

pub fn request_received(request: Value) -> LobbyMessage {
    one(FRIENDS, REQUEST_RECEIVED, request)
}

pub fn request_removed(inviter: Fields) -> LobbyMessage {
    one(FRIENDS, REQUEST_REMOVED, Struct(inviter))
}

/// 27110 {token, text, 0}: the client shows the text (0x7FF78960D700).
pub fn request_result(token: u64, text: u64) -> LobbyMessage {
    (
        FRIENDS,
        REQUEST_RESULT,
        Fields::from([
            (0x78, Unsigned(token)),
            (0x80, Unsigned(text)),
            (0x88, Unsigned(0)),
        ]),
    )
}

/// 27111 to an answer of a friend request, 27112 to a removal.
pub fn answered(token: u64, result: u64) -> LobbyMessage {
    token_result(ANSWERED, token, result)
}

pub fn removed(token: u64, result: u64) -> LobbyMessage {
    token_result(REMOVED, token, result)
}

fn token_result(id: u32, token: u64, result: u64) -> LobbyMessage {
    (
        FRIENDS,
        id,
        Fields::from([(0x78, Unsigned(token)), (0x80, Unsigned(result))]),
    )
}

// --- parties -----------------------------------------------------------------

pub fn invite(inviter: Fields) -> LobbyMessage {
    let id = inviter.get(&0).cloned().unwrap_or(Struct(Fields::new()));
    (
        PARTY,
        INVITE,
        Fields::from([(0x78, id), (0x88, Struct(inviter))]),
    )
}

pub fn invite_cancelled(inviter: Fields) -> LobbyMessage {
    one(PARTY, INVITE_CANCELLED, Struct(inviter))
}

pub fn suggestion(member: Fields, player: Fields) -> LobbyMessage {
    (
        PARTY,
        SUGGESTION,
        Fields::from([(0x78, Struct(member)), (0xe0, Struct(player))]),
    )
}

pub fn merge_request(leader: Fields, group: Fields) -> LobbyMessage {
    (
        PARTY,
        MERGE_REQUEST,
        Fields::from([(0x78, Struct(leader)), (0xe0, Struct(group))]),
    )
}

pub fn party_error(code: u64) -> LobbyMessage {
    one(LOBBY, PARTY_ERROR, Unsigned(code))
}

/// A listed group's member (20700 member +0xA0): the slot types the member
/// takes (52203) and the one they fill (+0x8). Until a member has one, the
/// client keeps its "choose role(s)" screen open (0x7FF789AA46B0); the
/// member's slot is read by 0x7FF78934B270. Slot types are the group
/// finder's own: 1 any role, 2 tank, 3 support, 4 damage (table
/// 0x7FF78B566690).
pub fn group_slot(slot_types: &[u64]) -> Value {
    Struct(Fields::from([
        (0, Unsigned(0)),
        (8, Unsigned(slot_types.first().copied().unwrap_or(0))),
        (9, Unsigned(0)),
        (
            0x10,
            Array(slot_types.iter().copied().map(Unsigned).collect()),
        ),
    ]))
}

// --- group finder ------------------------------------------------------------

pub fn groups_found(groups: Vec<Fields>) -> LobbyMessage {
    (
        GROUPS,
        GROUPS_FOUND,
        Fields::from([
            (0x78, Array(groups.into_iter().map(Struct).collect())),
            (0x90, Array(Vec::new())),
        ]),
    )
}

/// 52301 refreshes a watched group while it looks for players.
pub fn group_changed(group: Fields) -> LobbyMessage {
    one(GROUPS, GROUP_CHANGED, Struct(group))
}

/// 52302 drops a group that stopped from the client's watched list.
pub fn group_closed(id: PartyId) -> LobbyMessage {
    one(GROUPS, GROUP_CLOSED, Struct(id16(id[0], id[1])))
}

/// A group finder listing (52201) of the bot, shaped like the ones the client
/// makes: +0x0 the leader's card, +0x68 the queue card, +0x76 the game type,
/// +0x80 the slot types.
pub fn bot_listing(
    card: Fields,
    name: &str,
    queue_card: u64,
    game_type: u64,
    slots: &[u64],
) -> Fields {
    Fields::from([
        (0, Struct(card)),
        (0x68, Unsigned(queue_card)),
        (0x70, Unsigned(1)),
        (0x74, Unsigned(150)),
        (0x76, Unsigned(game_type)),
        (0x77, Bool(true)),
        (0x78, Bool(false)),
        (0x79, Bool(true)),
        (0x80, Array(slots.iter().copied().map(Unsigned).collect())),
        (0x98, Bool(false)),
        (0xa0, Value::String(name.into())),
    ])
}
