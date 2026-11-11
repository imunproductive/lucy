use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use orpheus_lobby::{
    handoff::{HANDOFF, HANDOFF_MESSAGE},
    lineup::MATCH_LOBBY,
    login::{LOGIN_MESSAGE, OUT_CONNECT},
    matchmaker::QUICK_PLAY,
    party::PARTY,
    queue::{ENTER_QUEUE, MATCHMAKE, QUEUE, QUEUE_WAITS, SET_ROLES, TANK_ROLE},
    templates::IN_CONNECT,
};
use orpheus_wire::jam::{
    codec::{Fields, Schemas, Value},
    framing::{self, FrameReader},
    handshake::client_handshake,
};

use crate::server::{Server, ServerConfig};

const CHAT_IN: u32 = 0x5f91_3f6b;

fn post(dashboard: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(dashboard).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.0\r\nHost: {dashboard}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let status = reply[9..12].parse().unwrap();
    (status, reply.split_once("\r\n\r\n").unwrap().1.to_owned())
}

fn structure(value: &Value) -> &Fields {
    let Value::Struct(fields) = value else {
        panic!("expected a struct, got {value:?}")
    };
    fields
}

/// One player searches Quick Play with one player to start and the
/// Practice Range forced: the queue pops, "Game Found!" shows, and the game
/// gets its 20600 after it.
#[test]
#[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
fn one_player_to_start_pops_the_queue_and_hands_the_game_off() {
    let runtime = orpheus_testkit::runtime_dir();
    let state = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::new(runtime, state.path().into());
    config.lobby_port = 0;
    config.game_port = Some(0);
    config.stall_port = 0;
    config.rpc_port = 0;
    config.dashboard_port = Some(0);
    let mut server = Server::start(config).unwrap();
    let dashboard = server.dashboard_addr().unwrap();
    let card = format!("0x{QUICK_PLAY:016X}");
    assert_eq!(
        post(
            dashboard,
            "/api/mode_settings",
            &format!(r#"{{"card":"{card}","players_to_start":1}}"#)
        ),
        (
            200,
            r#"{"message":"Quick Play: matches start with 1 player(s)."}"#.into()
        )
    );
    let saved = std::fs::read_to_string(state.path().join("matchmaking.json")).unwrap();
    assert!(saved.contains("\"players_to_start\": 1"), "{saved}");
    assert_eq!(
        post(dashboard, "/api/set_map", r#"{"map":"0x0800000000000688"}"#),
        (
            200,
            r#"{"message":"Matches load Practice Range when their queue's modes allow it."}"#
                .into()
        )
    );

    let schemas = Schemas::lobby();
    let mut tcp = TcpStream::connect(server.lobby_addr()).unwrap();
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let (mut tx, rx) = client_handshake(&mut tcp);
    let groups = [
        IN_CONNECT,
        OUT_CONNECT,
        MATCHMAKE,
        QUEUE,
        QUEUE_WAITS,
        PARTY,
        MATCH_LOBBY,
        HANDOFF,
        CHAT_IN,
    ];
    let wire = |group: u32| groups.iter().position(|&known| known == group).unwrap() as u8 + 1;
    let mut announcement = vec![0, 0];
    announcement.extend_from_slice(&(groups.len() as u32).to_le_bytes());
    for group in groups {
        announcement.extend_from_slice(&group.to_le_bytes());
    }
    framing::send_frame(&mut tcp, &mut tx, &announcement).unwrap();
    let key = Fields::from([(
        0,
        Value::Struct(Fields::from([(0, Value::Unsigned(QUICK_PLAY))])),
    )]);
    let mut send = |tcp: &mut TcpStream, group: u32, id: u32, fields: Fields| {
        let mut frame = schemas.header(group, wire(group), id).unwrap();
        frame.extend(schemas.encode(group, id, &fields).unwrap());
        framing::send_frame(tcp, &mut tx, &frame).unwrap();
    };
    send(&mut tcp, OUT_CONNECT, LOGIN_MESSAGE, Fields::new());
    send(
        &mut tcp,
        MATCHMAKE,
        ENTER_QUEUE,
        Fields::from([(0x78, Value::Struct(key.clone()))]),
    );

    let wanted = [
        (QUEUE, 44201),
        (QUEUE, 44202),
        (QUEUE_WAITS, 56200),
        (PARTY, 20700),
        (MATCH_LOBBY, 53000),
        (MATCH_LOBBY, 53003),
        (CHAT_IN, 20402),
        (CHAT_IN, 20404),
        (HANDOFF, HANDOFF_MESSAGE),
    ];
    let mut received: Vec<(u32, Fields, Instant)> = Vec::new();
    let mut reader = FrameReader::new(rx);
    let mut buf = [0; 16384];
    let limit = Instant::now() + Duration::from_secs(15);
    let mut roles_sent = false;
    while !received.iter().any(|(id, ..)| *id == HANDOFF_MESSAGE) {
        assert!(
            Instant::now() < limit,
            "no handoff: {:?}",
            received.iter().map(|(id, ..)| id).collect::<Vec<_>>()
        );
        let count = match tcp.read(&mut buf) {
            Ok(0) => panic!("the lobby closed the connection"),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => panic!("{error}"),
        };
        for frame in reader.feed(&buf[..count]) {
            for (group, id) in wanted {
                if let Some(body) =
                    frame.strip_prefix(schemas.header(group, wire(group), id).unwrap().as_slice())
                {
                    received.push((
                        id,
                        schemas.decode(group, id, body, true).unwrap(),
                        Instant::now(),
                    ));
                }
            }
        }
        // The role screen: Ready as a tank once the role check started.
        if !roles_sent && received.iter().any(|(id, ..)| *id == 56200) {
            roles_sent = true;
            send(
                &mut tcp,
                MATCHMAKE,
                SET_ROLES,
                Fields::from([(
                    0x78,
                    Value::Array(vec![Value::Unsigned(u64::from(TANK_ROLE))]),
                )]),
            );
        }
    }
    let order: Vec<u32> = received
        .iter()
        .map(|(id, ..)| *id)
        .filter(|id| *id != 20700)
        .collect();
    // 20402 General at login; then the queue, the role check, the search, the
    // pop and the game.
    assert_eq!(
        order,
        [
            20402,
            44201,
            56200,
            44201,
            44202,
            53000,
            53003,
            20404,
            20402,
            HANDOFF_MESSAGE
        ]
    );
    let find = |wanted: u32, nth: usize| {
        received
            .iter()
            .filter(|(id, ..)| *id == wanted)
            .nth(nth)
            .unwrap()
    };
    // The role check shows the queue entry in the party state.
    let party_states: Vec<&Fields> = received
        .iter()
        .filter(|(id, ..)| *id == 20700)
        .map(|(_, fields, _)| fields)
        .collect();
    let queue_state = |fields: &Fields| match &structure(&fields[&0x78])[&0x18] {
        Value::Array(entries) => entries.first().map(|entry| structure(entry)[&0x54].clone()),
        other => panic!("{other:?}"),
    };
    let states: Vec<_> = party_states
        .iter()
        .map(|fields| queue_state(fields))
        .collect();
    // Login, the role check, the search, the pop.
    assert_eq!(
        states,
        [None, Some(Value::Signed(1)), Some(Value::Signed(3)), None]
    );

    let (_, popped, _) = find(44202, 0);
    assert_eq!(popped[&0xa8], Value::Unsigned(1));
    let (_, found, found_at) = find(53003, 0);
    assert_eq!(found[&0xa0], Value::Signed(4));
    let (_, handoff, handoff_at) = find(HANDOFF_MESSAGE, 0);
    assert!(handoff_at.duration_since(*found_at) >= Duration::from_millis(3500));
    let ticket = structure(&handoff[&0x80]);
    assert_eq!(
        ticket[&0x2c],
        Value::Unsigned(u64::from(server.game_addr().unwrap().port()))
    );
    let (_, lobby, _) = find(53000, 0);
    let match_id = &structure(&lobby[&0x78])[&0x30];
    assert_eq!(
        structure(match_id)[&0],
        Value::Array(match &structure(&ticket[&0])[&0] {
            Value::Unsigned(low) => vec![Value::Unsigned(*low), structure(&ticket[&0])[&8].clone()],
            other => panic!("{other:?}"),
        })
    );
    server.stop().unwrap();
}

/// Matches that ended by their rules, into the profiles of the room's seated
/// players (`results.rs`), and on to their games.
mod results {
    use std::{
        io::Read,
        net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
        sync::{Arc, mpsc},
        thread,
        time::{Duration, Instant},
    };

    use orpheus_content::maps::GameMap;
    use orpheus_lobby::{
        accounts::{Account, Accounts, Profile, load_or_create_profile},
        collection::PROGRESSION,
        handoff::Handoff,
        login::{Identity, LOGIN_MESSAGE, OUT_CONNECT},
        matchmaker::QUICK_PLAY,
        progression::{self, Earned, MatchPlay},
        queue::{DAMAGE_ROLE, SUPPORT_ROLE, TANK_ROLE},
        ranked::{
            CURRENT_SEASON, OPEN_QUEUE, Queue, RANKED, ROLE_QUEUE, Season, TANK, draws_of,
            matches_of, rating_of, tier_of, wins_of,
        },
        templates::{IN_CONNECT, LOBBY},
    };
    use orpheus_wire::jam::{
        codec::{Fields, Schemas, Value},
        framing::{self, FrameReader},
        handshake::client_handshake,
    };
    use serde_json::json;

    use super::super::{
        Room, Seat,
        ported_tests::Fixture,
        results::{recorded, save},
    };
    use crate::{
        game_service::{MatchResult, Outcome, PlayerResult},
        lobby::serve,
    };

    const ID: [u64; 2] = [1, 2];
    /// The seats' games connected this long before the match ended.
    const PLAYED: Duration = Duration::from_secs(600);
    /// Seconds since 1970 when the results are saved.
    const WALL_TIME: u64 = 1_700_000_000;
    /// The Practice vs AI card "Medium".
    const VERSUS_AI_MEDIUM: u64 = 0x0630_0000_0000_0071;

    fn seat(account: &Arc<Account>, conn: u32, team: u8, role: u8, joined: Instant) -> Seat {
        Seat {
            account: Arc::clone(account),
            party: [u64::from(conn); 2],
            team,
            role,
            record: Fields::new(),
            handoff: Handoff::new(conn, ID).unwrap(),
            joined: true,
            joined_at: Some(joined),
            gone: false,
            going_home: false,
            excused: false,
            released: false,
        }
    }

    fn room(card: u64, ranked: bool, seats: Vec<Seat>) -> Room {
        Room {
            id: ID,
            card,
            map: GameMap::practice_range(),
            ranked,
            seats,
        }
    }

    fn player(slot: usize, account: &Account, team: u8, outcome: Outcome) -> PlayerResult {
        PlayerResult {
            slot,
            name: account.name.clone(),
            account: Identity::for_account(account.account_lo).account_id(),
            team,
            outcome,
            finished: true,
        }
    }

    fn left(player: PlayerResult) -> PlayerResult {
        PlayerResult {
            finished: false,
            ..player
        }
    }

    fn result(players: Vec<PlayerResult>) -> MatchResult {
        MatchResult {
            id: ID,
            map: 0,
            mode: 0,
            players,
        }
    }

    fn season() -> Season {
        Season {
            number: CURRENT_SEASON,
            card: ROLE_QUEUE,
            open_card: Some(OPEN_QUEUE),
        }
    }

    /// What the room's result gives, by player name, as the matchmaker's
    /// tick sees it when the match ends.
    fn plays(room: &Room, players: Vec<PlayerResult>, joined: Instant) -> Vec<(String, MatchPlay)> {
        recorded(room, &result(players), |_| season(), joined + PLAYED)
            .into_iter()
            .map(|(account, play)| (account.name.clone(), play))
            .collect()
    }

    /// The room's result, recorded and saved as the matchmaker does.
    fn record(room: &Room, players: Vec<PlayerResult>, joined: Instant) -> Vec<Earned> {
        recorded(room, &result(players), |_| season(), joined + PLAYED)
            .into_iter()
            .map(|(account, play)| save(&account, &play, WALL_TIME).unwrap())
            .collect()
    }

    fn registry(state: &tempfile::TempDir) -> Accounts {
        Accounts::new(
            state.path().join("profiles"),
            state.path().join("profile.json"),
        )
        .unwrap()
    }

    /// Matches, wins and draws of the queue, as the lobby reads them.
    fn counts(profile: &Profile, queue: Queue) -> [u16; 3] {
        [
            matches_of(profile, queue).unwrap(),
            wins_of(profile, queue).unwrap(),
            draws_of(profile, queue).unwrap(),
        ]
    }

    /// Each seated player still in the match at its end gets one more match
    /// in the queue he played, and one more win or draw; a loss is a match
    /// only. A leaver gets the match as a loss and a leave (rules 87, 88); a
    /// slot without a seat gets nothing. The counts are saved: a new
    /// registry reads them from the profiles.
    #[test]
    fn a_finished_match_counts_for_the_seated_players_and_the_leavers() {
        let state = tempfile::tempdir().unwrap();
        let accounts = registry(&state);
        let [alpha, beta, gamma, delta] =
            ["Alpha", "Beta", "Gamma", "Delta"].map(|name| accounts.get(name).unwrap());
        let fresh = |account: &Account| account.profile().clone();
        let (alpha_before, beta_before, gamma_before, delta_before) =
            (fresh(&alpha), fresh(&beta), fresh(&gamma), fresh(&delta));
        let joined = Instant::now();

        // Quick Play: no competitive queue, so the role played, or open.
        let quick_play = room(
            QUICK_PLAY,
            false,
            vec![
                seat(&alpha, 1, 0, TANK_ROLE, joined),
                seat(&beta, 2, 1, DAMAGE_ROLE, joined),
                seat(&gamma, 3, 0, 0, joined),
                seat(&delta, 4, 1, SUPPORT_ROLE, joined),
            ],
        );
        record(
            &quick_play,
            vec![
                player(0, &alpha, 0, Outcome::Win),
                player(1, &beta, 1, Outcome::Loss),
                player(2, &gamma, 0, Outcome::Win),
                left(player(3, &delta, 1, Outcome::Win)),
                player(4, &delta, 1, Outcome::Loss),
            ],
            joined,
        );
        // The role queue's competitive queues, a draw.
        let role_queue = room(
            ROLE_QUEUE,
            true,
            vec![
                seat(&alpha, 5, 0, TANK_ROLE, joined),
                seat(&beta, 6, 1, SUPPORT_ROLE, joined),
            ],
        );
        record(
            &role_queue,
            vec![
                player(0, &alpha, 0, Outcome::Draw),
                player(1, &beta, 1, Outcome::Draw),
            ],
            joined,
        );

        let saved = registry(&state);
        let profile = |name: &str| saved.get(name).unwrap().profile().clone();
        let [matches, wins, draws] = counts(&alpha_before, Queue::Tank);
        assert_eq!(
            counts(&profile("Alpha"), Queue::Tank),
            [matches + 2, wins + 1, draws + 1]
        );
        let [matches, wins, draws] = counts(&beta_before, Queue::Damage);
        assert_eq!(
            counts(&profile("Beta"), Queue::Damage),
            [matches + 1, wins, draws]
        );
        let [matches, wins, draws] = counts(&beta_before, Queue::Support);
        assert_eq!(
            counts(&profile("Beta"), Queue::Support),
            [matches + 1, wins, draws + 1]
        );
        let [matches, wins, draws] = counts(&gamma_before, Queue::Open);
        assert_eq!(
            counts(&profile("Gamma"), Queue::Open),
            [matches + 1, wins + 1, draws]
        );
        let delta_after = profile("Delta");
        let [matches, wins, draws] = counts(&delta_before, Queue::Support);
        assert_eq!(
            counts(&delta_after, Queue::Support),
            [matches + 1, wins, draws]
        );
        assert_eq!(delta_after.leave_mask, 1);
        assert_eq!(
            (delta_after.level, delta_after.experience),
            (delta_before.level, delta_before.experience)
        );
        for name in ["Alpha", "Beta", "Gamma"] {
            assert_eq!(profile(name).leave_mask, 0);
        }
    }

    /// A play carries the queue, the time since the seat's game connected,
    /// whether the player came with his group (another seat of his party),
    /// and the card's kind: competitive (the room is ranked) or Practice vs
    /// AI.
    #[test]
    fn each_play_carries_queue_time_group_and_card() {
        let state = tempfile::tempdir().unwrap();
        let accounts = registry(&state);
        let [alpha, beta, gamma] =
            ["Alpha", "Beta", "Gamma"].map(|name| accounts.get(name).unwrap());
        let joined = Instant::now();
        let mut friend = seat(&beta, 2, 0, DAMAGE_ROLE, joined);
        friend.party = [1; 2];
        let quick_play = room(
            QUICK_PLAY,
            false,
            vec![
                seat(&alpha, 1, 0, TANK_ROLE, joined),
                friend,
                seat(&gamma, 3, 1, 0, joined),
            ],
        );
        let played = plays(
            &quick_play,
            vec![
                player(0, &alpha, 0, Outcome::Win),
                player(1, &beta, 0, Outcome::Win),
                player(2, &gamma, 1, Outcome::Loss),
            ],
            joined,
        );
        let base = MatchPlay {
            queue: Queue::Tank,
            outcome: progression::Outcome::Win,
            finished: true,
            competitive: false,
            versus_ai: false,
            group: true,
            seconds: PLAYED.as_secs_f64(),
        };
        assert_eq!(
            played,
            [
                (String::from("Alpha"), base),
                (
                    String::from("Beta"),
                    MatchPlay {
                        queue: Queue::Damage,
                        ..base
                    }
                ),
                (
                    String::from("Gamma"),
                    MatchPlay {
                        queue: Queue::Open,
                        outcome: progression::Outcome::Loss,
                        group: false,
                        ..base
                    }
                ),
            ]
        );

        let versus_ai = room(VERSUS_AI_MEDIUM, false, vec![seat(&alpha, 1, 0, 0, joined)]);
        let [(_, play)] = &plays(
            &versus_ai,
            vec![player(0, &alpha, 0, Outcome::Draw)],
            joined,
        )[..] else {
            panic!("one play");
        };
        assert!(play.versus_ai && !play.competitive && !play.group);

        let competitive = room(
            ROLE_QUEUE,
            true,
            vec![seat(&alpha, 1, 0, TANK_ROLE, joined)],
        );
        let [(_, play)] = &plays(
            &competitive,
            vec![player(0, &alpha, 0, Outcome::Win)],
            joined,
        )[..] else {
            panic!("one play");
        };
        assert!(play.competitive && !play.versus_ai);
        assert_eq!(play.queue, Queue::Tank);
    }

    /// Leaving is a leave only when the player's game had connected, the
    /// server did not take him out (the dashboard ending the matches, a
    /// queue match found), and he did not finish the match from another
    /// seat.
    #[test]
    fn server_take_outs_and_unconnected_seats_are_not_leaves() {
        let state = tempfile::tempdir().unwrap();
        let accounts = registry(&state);
        let [alpha, beta, gamma, delta] =
            ["Alpha", "Beta", "Gamma", "Delta"].map(|name| accounts.get(name).unwrap());
        let joined = Instant::now();
        let mut excused = seat(&alpha, 1, 0, 0, joined);
        excused.excused = true;
        let mut unconnected = seat(&beta, 2, 0, 0, joined);
        (unconnected.joined, unconnected.joined_at) = (false, None);
        let quick_play = room(
            QUICK_PLAY,
            false,
            vec![
                excused,
                unconnected,
                seat(&gamma, 3, 0, 0, joined),
                seat(&gamma, 4, 0, 0, joined),
                seat(&delta, 5, 1, 0, joined),
            ],
        );
        let played = plays(
            &quick_play,
            vec![
                left(player(0, &alpha, 0, Outcome::Win)),
                left(player(1, &beta, 0, Outcome::Win)),
                left(player(2, &gamma, 0, Outcome::Win)),
                player(3, &gamma, 0, Outcome::Win),
                left(player(4, &delta, 1, Outcome::Loss)),
            ],
            joined,
        );
        let who: Vec<_> = played
            .iter()
            .map(|(name, play)| (name.as_str(), play.finished))
            .collect();
        assert_eq!(who, [("Gamma", true), ("Delta", false)]);
    }

    /// A saved competitive win: experience for the time, the finish, the
    /// win and the day's first win, a level with its box, the rating and the
    /// tier, the points. The profile on disk has them.
    #[test]
    fn a_competitive_win_saves_experience_levels_rating_and_points() {
        let state = tempfile::tempdir().unwrap();
        let accounts = registry(&state);
        let alpha = accounts.get("Alpha").unwrap();
        prepare(&alpha);
        let boxes = alpha.profile().loot_boxes.len();
        let joined = Instant::now();
        let competitive = room(
            ROLE_QUEUE,
            true,
            vec![seat(&alpha, 1, 0, TANK_ROLE, joined)],
        );
        let earned = record(
            &competitive,
            vec![player(0, &alpha, 0, Outcome::Win)],
            joined,
        );
        assert_eq!(
            earned,
            [Earned {
                experience: 2406 + 250 + 500 + 1500,
                levels: 1,
                rating: 25,
                comp_points: 15,
                left: false,
            }]
        );
        let saved = load_or_create_profile(alpha.path.as_ref().unwrap()).unwrap();
        assert_won(&saved, boxes);
    }

    /// Alpha at level 5 with 7000 experience into it, no first win today,
    /// 2490 tank rating (Gold) after 30 matches, no competitive points.
    fn prepare(alpha: &Account) {
        alpha
            .edit_profile(|profile| {
                profile.level = 5;
                profile.experience = 7000;
                profile.first_win_day = 0;
                profile.comp_points = 0;
                profile.ratings.insert("tank".into(), json!(2490));
                profile.matches.insert("tank".into(), json!(30));
                Ok(Some(()))
            })
            .unwrap();
    }

    /// `prepare`d Alpha after a 10-minute competitive tank win: 7000 + 4656
    /// experience is level 6 (7500 for level 5) with 4156 into it and one
    /// box more; 2515 is Platinum; 15 points.
    fn assert_won(profile: &Profile, boxes_before: usize) {
        assert_eq!((profile.level, profile.experience), (6, 4156));
        assert_eq!(profile.loot_boxes.len(), boxes_before + 1);
        assert_eq!(rating_of(profile, Queue::Tank).unwrap(), 2515);
        assert_eq!(tier_of(profile, Queue::Tank).unwrap(), 3);
        assert_eq!(profile.comp_points, 15);
        assert_eq!(profile.leave_mask, 0);
    }

    /// The lobby side of a game client: the login, then what it receives.
    struct Client<'a> {
        schemas: &'a Schemas,
        tcp: TcpStream,
        reader: FrameReader,
        received: Vec<(u32, u32, Fields)>,
    }

    const ANNOUNCED: [u32; 5] = [IN_CONNECT, OUT_CONNECT, LOBBY, PROGRESSION, RANKED];

    impl<'a> Client<'a> {
        fn login(schemas: &'a Schemas, lobby: SocketAddr, name: &str) -> Self {
            let mut tcp = TcpStream::connect(lobby).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let (mut tx, rx) = client_handshake(&mut tcp);
            tcp.set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let mut announcement = vec![0, 0];
            announcement.extend_from_slice(&(ANNOUNCED.len() as u32).to_le_bytes());
            for group in ANNOUNCED {
                announcement.extend_from_slice(&group.to_le_bytes());
            }
            framing::send_frame(&mut tcp, &mut tx, &announcement).unwrap();
            let wire = ANNOUNCED
                .iter()
                .position(|&group| group == OUT_CONNECT)
                .unwrap() as u8
                + 1;
            let mut frame = schemas.header(OUT_CONNECT, wire, LOGIN_MESSAGE).unwrap();
            let login = Fields::from([(0x78, Value::String(name.into()))]);
            frame.extend(schemas.encode(OUT_CONNECT, LOGIN_MESSAGE, &login).unwrap());
            framing::send_frame(&mut tcp, &mut tx, &frame).unwrap();
            Self {
                schemas,
                tcp,
                reader: FrameReader::new(rx),
                received: Vec::new(),
            }
        }

        fn read(&mut self) {
            let mut buffer = [0; 65_536];
            match self.tcp.read(&mut buffer) {
                Ok(0) => panic!("the lobby closed the connection"),
                Ok(count) => {
                    for frame in self.reader.feed(&buffer[..count]) {
                        let Some(&wire) = frame.first().filter(|&&wire| wire != 0) else {
                            continue;
                        };
                        let crc = ANNOUNCED[usize::from(wire) - 1];
                        let (id, body) = self.schemas.split(crc, &frame).unwrap();
                        let fields = self.schemas.decode(crc, id, body, true).unwrap();
                        self.received.push((crc, id, fields));
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("{error}"),
            }
        }

        /// The first message of this kind received so far or within a
        /// while; it and everything before it are consumed.
        fn expect(&mut self, crc: u32, id: u32) -> Fields {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(index) = self
                    .received
                    .iter()
                    .position(|&(group, message, _)| (group, message) == (crc, id))
                {
                    return self.received.drain(..=index).next_back().unwrap().2;
                }
                assert!(Instant::now() < deadline, "no {crc:08X}/{id}");
                self.read();
            }
        }

        /// Drop everything that arrives within a short while.
        fn settle(&mut self) {
            let until = Instant::now() + Duration::from_millis(700);
            while Instant::now() < until {
                self.read();
            }
            self.received.clear();
        }
    }

    fn integer(value: &Value) -> i128 {
        match value {
            Value::Signed(value) => i128::from(*value),
            Value::Unsigned(value) => i128::from(*value),
            other => panic!("expected an integer, got {other:?}"),
        }
    }

    use super::structure;

    fn array(value: &Value) -> &[Value] {
        let Value::Array(values) = value else {
            panic!("expected an array, got {value:?}")
        };
        values
    }

    /// Loopback: a game logged in to the lobby is in a competitive match
    /// that ends by its rules. The matchmaker records the result (its tick's
    /// `record`), the profile on disk has it, and the game gets it through
    /// the dashboard's live push: its boxes (24302) with the level's box,
    /// its competitive points (24308), its level and experience (24313),
    /// the leave policy (20813) and its rating and tier (36302).
    #[test]
    #[ignore = "requires an authorized 104319 runtime in ORPHEUS_RUNTIME_DIR"]
    fn a_match_result_reaches_the_profile_and_the_players_game() {
        let fixture = Fixture::without_game();
        let lobby = &fixture.lobby;
        let alpha = lobby.accounts.get("Alpha").unwrap();
        prepare(&alpha);
        let boxes = alpha.profile().loot_boxes.len();

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, shutdown) = mpsc::channel();
        let server = {
            let lobby = Arc::clone(lobby);
            thread::spawn(move || serve(listener, lobby, shutdown))
        };
        let mut client = Client::login(&lobby.schemas, address, "Alpha");
        // The login ends with the settings (20802).
        client.expect(LOBBY, 20802);
        client.settle();

        let joined = Instant::now();
        lobby.matchmaker.lock().rooms.push(room(
            ROLE_QUEUE,
            true,
            vec![seat(&alpha, 1, 0, TANK_ROLE, joined)],
        ));
        let ended = result(vec![player(0, &alpha, 0, Outcome::Win)]);
        lobby
            .matchmaker
            .with(lobby, |ctx| ctx.record(&ended, joined + PLAYED));
        assert_won(
            &load_or_create_profile(alpha.path.as_ref().unwrap()).unwrap(),
            boxes,
        );

        let boxes_sent = client.expect(PROGRESSION, 24302);
        assert_eq!(array(&boxes_sent[&0x78]).len(), boxes + 1);
        assert_eq!(integer(&client.expect(PROGRESSION, 24308)[&0x78]), 15);
        let level = client.expect(PROGRESSION, 24313);
        assert_eq!((integer(&level[&0x78]), integer(&level[&0x80])), (6, 4156));
        let policy = client.expect(LOBBY, 20813);
        assert_eq!((integer(&policy[&0x78]), integer(&policy[&0x80])), (0, 20));
        let tank = loop {
            let card = client.expect(RANKED, 36302);
            let card = structure(&card[&0x78]);
            if integer(&card[&0x18]) != i128::from(ROLE_QUEUE) {
                continue;
            }
            break array(&card[&0])
                .iter()
                .map(structure)
                .find(|rating| integer(&rating[&0]) == i128::from(TANK))
                .expect("a tank rating")
                .clone();
        };
        assert_eq!((integer(&tank[&0x18]), integer(&tank[&0x22])), (2515, 3));

        drop(client);
        stop.send(()).unwrap();
        server.join().unwrap().unwrap();
    }
}
