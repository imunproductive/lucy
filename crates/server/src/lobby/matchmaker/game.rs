//! What the matchmaker asks of the game service, in the shape Python's
//! `GameServer` offers it (`ow174/game/server.py` `create_match`,
//! `join_match`).
//!
//! The Rust game service opens one prepared world per call and keeps no
//! account bookkeeping, so the matchmaker keeps its own rooms (`Room`) and
//! reaches the service through these functions.

use std::{io, sync::Arc, time::SystemTime};

use orpheus_content::maps::GameMap;
use orpheus_lobby::{
    accounts::{Account, battle_tag},
    handoff::Handoff,
    login::Identity,
    player_messages,
    ranked::LeaderboardPlaces,
};
use orpheus_sim::scene::PlayerCard;
use orpheus_wire::jam::codec::Fields;

use crate::{
    game_service::{GameService, MatchPlayer, WorldPlayer},
    lobby::Lobby,
};

/// Open one world for these players; their seats come back in player order.
/// Worlds open only on the Practice Range scene so far; the service refuses
/// other maps as unsupported.
pub(super) fn open_world(
    game: &GameService,
    id: [u64; 2],
    map: GameMap,
    lobby: &Lobby,
    players: Vec<WorldPlayer>,
) -> io::Result<Vec<Handoff>> {
    let scene = prepare_scene(lobby, map)?;
    game.open_prepared(id, scene, players)
}

/// Seats for more players in a match that is on (Python
/// `GameServer.join_match`), or `None` when the match takes no more. The game
/// service cannot add players to a running world yet, so a match takes none
/// and waiting parties start a match of their own instead.
pub(super) fn join_running(
    _game: &GameService,
    _id: [u64; 2],
    _players: &[WorldPlayer],
) -> io::Result<Option<Vec<Handoff>>> {
    Ok(None)
}

/// The scene of a map: the Practice Range prepared at start-up the first time,
/// then a fresh one each time.
fn prepare_scene(
    lobby: &Lobby,
    map: GameMap,
) -> io::Result<orpheus_sim::scene_assets::PreparedScene> {
    let first = lobby
        .first_scene
        .lock()
        .expect("prepared scene")
        .take_if(|scene| *scene.map() == map);
    first.map_or_else(|| lobby.assets.prepare(map), Ok)
}

/// What the game server takes for a player, and the player's card as the
/// lobby shows it (account id, icon, portrait frame, level, BattleTag). On a
/// competitive queue card the card carries the rank there: of the role played
/// in a role queue, else of the card's one queue. In a role queue the role
/// goes with it: hero select then offers only that role's heroes. Ports
/// `matchmaker.py` `_game_players` and `_card`.
pub(super) fn world_player(
    lobby: &Lobby,
    account: &Arc<Account>,
    tournament: bool,
    team: u8,
    card: u64,
    role: u8,
) -> io::Result<(WorldPlayer, Fields)> {
    let profile = account.profile();
    let identity = Identity::for_account(account.account_lo);
    let frame = lobby.collection.portrait_frame(&profile);
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let record = player_messages::record(&profile, identity, frame, now);
    let rank = match orpheus_lobby::queue::role_guid(role) {
        _ if card == 0 => None,
        guid => lobby
            .ranked
            .rank(&profile, card, guid.unwrap_or(0), &LeaderboardPlaces::new())?,
    };
    let skins = lobby
        .assets
        .heroes()
        .heroes()
        .map(|hero| {
            lobby
                .collection
                .equipped_skin(&profile, hero.guid)
                .map(|skin| (hero.guid, skin))
        })
        .collect::<io::Result<_>>()?;
    let world_card = PlayerCard {
        account: identity.account_id(),
        battle_tag: battle_tag(&profile.player_name, account.account_lo),
        level: Some(u32::try_from(profile.level).map_err(io::Error::other)?),
        portrait_frame: Some(frame),
        ranked: rank.map(|rank| {
            (
                i16::try_from(rank.skill_rating).unwrap_or(i16::MAX),
                i16::try_from(rank.top_500_place).unwrap_or(i16::MAX),
                i8::try_from(rank.tier).unwrap_or(i8::MAX),
            )
        }),
        role: (role != 0).then(|| i8::try_from(role).unwrap_or(0)),
        skins,
        record: record.clone(),
    };
    let player = MatchPlayer {
        name: account.name.clone(),
        team,
        tournament,
        ranked: rank.is_some(),
    };
    Ok((
        WorldPlayer {
            player,
            card: world_card,
        },
        record,
    ))
}
