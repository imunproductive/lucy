//! Post-login dispatch of the solo Practice Range create-game request.

use std::io;

use crate::game_service::{GameService, MatchPlayer};
use lucy_content::maps::GameMap;
use lucy_lobby::{
    accounts::battle_tag,
    collection::Collection,
    handoff::{
        Handoff, LobbySeat, enter_game, general_channel, is_request, match_channel, team_channel,
    },
    player_messages,
    session::PreparedLogin,
    transport::LobbyMessage,
};
use lucy_sim::{
    scene::PlayerCard,
    scene_assets::{PreparedScene, SceneAssets},
};
use lucy_wire::jam::codec::Fields;

/// Chat and address data from the authenticated session's owner, and the
/// Practice Range's map (`MapCatalog::practice_range`). The caller supplies
/// this only after confirming the account is playing alone.
pub struct SoloPracticeHandoff<'a> {
    pub match_id: [u64; 2],
    pub host: &'a str,
    pub general: Fields,
    pub match_channel: Fields,
    /// The player's team chat (team [`PLAYER_TEAM`]).
    pub team_channel: Fields,
    pub map: GameMap,
}

impl<'a> SoloPracticeHandoff<'a> {
    pub fn new(match_id: [u64; 2], host: &'a str, map: GameMap) -> Self {
        Self {
            match_id,
            host,
            map,
            general: general_channel(),
            match_channel: match_channel(match_id),
            team_channel: team_channel(match_id, PLAYER_TEAM),
        }
    }
}

/// The player is team 0; the training bots are team 1.
const PLAYER_TEAM: u8 = 0;

/// Reserve a seat for a decoded post-login request and send its transfer in
/// order. The listener supplies the live login and an opener for this worker,
/// which may capture prepared world data in addition to the Practice Range map.
pub fn dispatch(
    request: &LobbyMessage,
    login: &PreparedLogin,
    game: &GameService,
    input: SoloPracticeHandoff<'_>,
    open_match: impl FnOnce(
        &GameService,
        [u64; 2],
        GameMap,
        Vec<MatchPlayer>,
    ) -> io::Result<Vec<Handoff>>,
    mut send: impl FnMut(&LobbyMessage) -> io::Result<bool>,
) -> io::Result<Option<LobbySeat>> {
    if !is_request(request) {
        return Ok(None);
    }
    let players = vec![MatchPlayer {
        name: login.account.name.clone(),
        team: PLAYER_TEAM,
        tournament: login.tournament,
        ranked: false,
    }];
    // A solo Range: its blue team is the one player (the mode takes six).
    let mut map = input.map;
    map.team_sizes = vec![1, 0];
    let tickets = open_match(game, input.match_id, map, players)?;
    let [handoff] = match <[Handoff; 1]>::try_from(tickets) {
        Ok(tickets) => tickets,
        Err(tickets) => {
            let mut error =
                io::Error::other("the solo Practice Range requires exactly one UDP seat");
            for ticket in tickets {
                error = release(game, ticket.conn, error);
            }
            return Err(error);
        }
    };
    let result = (|| {
        if handoff.match_id != input.match_id {
            return Err(io::Error::other(
                "the reserved UDP seat is for another match",
            ));
        }
        let [leave, join, team, ticket] = enter_game(
            input.general,
            input.match_channel,
            input.team_channel,
            &handoff,
            (input.host, game.local_addr().port()),
        )?;
        send(&leave)?;
        send(&join)?;
        send(&team)?;
        if !send(&ticket)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the client did not announce the game handoff protocol",
            ));
        }
        Ok(())
    })();
    if let Err(error) = result {
        return Err(release(game, handoff.conn, error));
    }
    Ok(Some(LobbySeat {
        account_lo: login.account.account_lo,
        lobby_session: login.identity.session[0],
        handoff,
        team: PLAYER_TEAM,
    }))
}

/// Build the player's world card from the same account and collection used at
/// login, then reserve the prepared map outside the game tick loop.
pub fn dispatch_prepared(
    request: &LobbyMessage,
    login: &PreparedLogin,
    game: &GameService,
    input: SoloPracticeHandoff<'_>,
    assets: &SceneAssets,
    collection: &Collection,
    send: impl FnMut(&LobbyMessage) -> io::Result<bool>,
) -> io::Result<Option<LobbySeat>> {
    dispatch_scene(
        request,
        login,
        game,
        input,
        (assets, collection),
        |map| assets.prepare(map),
        send,
    )
}

pub(crate) fn dispatch_scene(
    request: &LobbyMessage,
    login: &PreparedLogin,
    game: &GameService,
    input: SoloPracticeHandoff<'_>,
    (assets, collection): (&SceneAssets, &Collection),
    prepare: impl FnOnce(GameMap) -> io::Result<PreparedScene>,
    send: impl FnMut(&LobbyMessage) -> io::Result<bool>,
) -> io::Result<Option<LobbySeat>> {
    dispatch(
        request,
        login,
        game,
        input,
        |game, id, map, mut players| {
            let player = players.pop().expect("solo Practice Range player");
            let card = {
                let profile = login.account.profile();
                let portrait_frame = collection.portrait_frame(&profile);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(io::Error::other)?
                    .as_secs();
                PlayerCard {
                    account: login.identity.account_id(),
                    battle_tag: battle_tag(&profile.player_name, login.identity.account_lo),
                    level: Some(u32::try_from(profile.level).map_err(io::Error::other)?),
                    portrait_frame: Some(portrait_frame),
                    skins: assets
                        .heroes()
                        .heroes()
                        .map(|hero| {
                            collection
                                .equipped_skin(&profile, hero.guid)
                                .map(|skin| (hero.guid, skin))
                        })
                        .collect::<io::Result<_>>()?,
                    // The lobby's card, which every client of the match gets
                    // in 20308 (ow174/lobby/matchmaker.py `_card`).
                    record: player_messages::record(&profile, login.identity, portrait_frame, now),
                    ..PlayerCard::default()
                }
            };
            game.open_prepared_practice(id, prepare(map)?, player, card)
                .map(|ticket| vec![ticket])
        },
        send,
    )
}

fn release(game: &GameService, connection: u32, error: io::Error) -> io::Error {
    match game.cancel_handoff(connection) {
        Ok(_) => error,
        Err(release) => io::Error::new(
            error.kind(),
            format!("{error} (could not release the Practice Range seat: {release})"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use lucy_content::maps::{PRACTICE_RANGE_MAP, PRACTICE_RANGE_MODE};
    use lucy_lobby::{
        accounts::Accounts,
        handoff::{CREATE_GAME, GAME_REQUEST, HANDOFF, HANDOFF_MESSAGE, Handoff},
        login::Identity,
    };
    use lucy_wire::jam::codec::{Schemas, Value};

    use crate::game_service::lockstep::{Client, Lockstep};

    const REQUEST: &[u8] = b"\x02\x00\x04\x00\x00\x00";

    fn input(match_id: [u64; 2]) -> SoloPracticeHandoff<'static> {
        SoloPracticeHandoff::new(match_id, "127.0.0.1", GameMap::practice_range())
    }

    fn require_unsigned(fields: &Fields, offset: u32) -> u64 {
        let Value::Unsigned(value) = fields[&offset] else {
            panic!("missing unsigned field {offset:#x}");
        };
        value
    }

    fn require_bytes<const N: usize>(fields: &Fields, offset: u32) -> [u8; N] {
        let Value::Array(values) = &fields[&offset] else {
            panic!("missing byte array field {offset:#x}");
        };
        values
            .iter()
            .map(|value| match value {
                Value::Unsigned(byte) => u8::try_from(*byte).unwrap(),
                _ => panic!("expected a byte"),
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    }

    #[test]
    fn practice_request_hands_off_an_authenticated_udp_seat_and_releases_failed_sends() {
        let lobby = Schemas::lobby();
        let game_schemas = Arc::new(Schemas::game());
        let game = Lockstep::new(game_schemas).unwrap();
        let fields = lobby
            .decode(GAME_REQUEST, CREATE_GAME, REQUEST, true)
            .unwrap();
        let request = (GAME_REQUEST, CREATE_GAME, fields);
        let temporary = tempfile::tempdir().unwrap();
        let accounts = Accounts::new(
            temporary.path().join("profiles"),
            temporary.path().join("seed"),
        )
        .unwrap();
        let account = accounts.get("fixture_player").unwrap();
        let login = PreparedLogin {
            identity: Identity::create(account.account_lo, 0x1234).unwrap(),
            account,
            tournament: true,
        };
        let unrelated = (GAME_REQUEST, CREATE_GAME + 1, request.2.clone());
        assert!(
            dispatch(
                &unrelated,
                &login,
                &game,
                input([4, 7]),
                |_, _, _, _| panic!("unrelated request opened a match"),
                |_| panic!("unrelated request sent a message"),
            )
            .unwrap()
            .is_none()
        );

        let match_id = [5, 8];
        let mut messages = Vec::new();
        let seat = dispatch(
            &request,
            &login,
            &game,
            input(match_id),
            |game, id, map, players| {
                assert_eq!(id, match_id);
                assert_eq!(map.map_guid, PRACTICE_RANGE_MAP);
                assert_eq!(map.mode_guid, PRACTICE_RANGE_MODE);
                assert_eq!(map.team_sizes, [1, 0]);
                assert_eq!(players.len(), 1);
                assert_eq!(players[0].name, "fixture_player");
                assert!(players[0].tournament);
                assert!(!players[0].ranked);
                game.open_match(id, map, players)
            },
            |(group, id, fields)| {
                let wire = lobby.encode(*group, *id, fields).unwrap();
                let decoded = lobby.decode(*group, *id, &wire, true).unwrap();
                messages.push((*group, *id, decoded));
                Ok(true)
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(seat.account_lo, login.account.account_lo);
        assert_eq!(seat.lobby_session, 0x1234);
        let chat = 0x5f91_3f6b;
        assert_eq!(
            messages
                .iter()
                .map(|&(crc, id, _)| (crc, id))
                .collect::<Vec<_>>(),
            [
                (chat, 20404),
                (chat, 20402),
                (chat, 20402),
                (HANDOFF, HANDOFF_MESSAGE)
            ],
        );
        let Value::Struct(ticket) = &messages[3].2[&0x80] else {
            panic!("handoff ticket missing");
        };
        let connection = u32::try_from(require_unsigned(ticket, 0x28)).unwrap();
        assert_eq!(connection, seat.handoff.conn);
        assert_eq!(
            require_unsigned(ticket, 0x2c),
            u64::from(game.local_addr().port())
        );
        let address = require_bytes::<64>(ticket, 0x2e);
        assert_eq!(&address[..10], b"127.0.0.1\0");
        assert_eq!(require_bytes::<64>(ticket, 0x6e), address);
        assert!(game.handoff_pending(connection).unwrap());
        let mut client = Client::new(
            &game,
            &Handoff {
                conn: connection,
                key_in: require_bytes(ticket, 0xae),
                key_out: require_bytes(ticket, 0xce),
                match_id,
            },
        );
        client.handshake();
        assert!(!game.handoff_pending(connection).unwrap());

        for (index, failed_id, kind) in [
            (0, 20402, io::ErrorKind::BrokenPipe),
            (1, HANDOFF_MESSAGE, io::ErrorKind::InvalidData),
        ] {
            let mut failed_seat = None;
            let result = dispatch(
                &request,
                &login,
                &game,
                input([6 + index, 8]),
                |game, id, map, players| {
                    let tickets = game.open_match(id, map, players)?;
                    failed_seat = Some(tickets[0].conn);
                    Ok(tickets)
                },
                |(_, id, _)| {
                    if *id == failed_id && kind == io::ErrorKind::BrokenPipe {
                        return Err(io::Error::new(kind, "test socket closed"));
                    }
                    Ok(*id != failed_id)
                },
            );
            let Err(error) = result else {
                panic!("failed handoff should release its seat");
            };
            assert_eq!(error.kind(), kind);
            assert!(!game.handoff_pending(failed_seat.unwrap()).unwrap());
        }
        let (mut game, _) = game.into_parts();
        game.stop().unwrap();
    }
}
