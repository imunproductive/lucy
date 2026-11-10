//! A match's collision gates its world (ported from
//! tests/test_game_collision_ready.py; match.py `_refresh_collision`,
//! `_collision_failure`).

use super::*;
use std::sync::atomic::AtomicUsize;

const OTHER: MatchId = [6, 9];

fn world_player(tag: &str, team: u8, account: u64) -> WorldPlayer {
    WorldPlayer {
        player: MatchPlayer {
            name: tag.into(),
            team,
            tournament: false,
            ranked: false,
        },
        card: PlayerCard {
            account: [account, 0x0100_0000_0000_0000],
            battle_tag: tag.into(),
            record: Fields::from([(0x40, Value::String(tag.into()))]),
            ..PlayerCard::default()
        },
        hero: 0,
    }
}

fn watch(server: &GameService, ticket: &Handoff, slot: usize) -> Watcher {
    let mut client = Client::new(server.local_addr(), ticket);
    client.handshake();
    Watcher {
        client,
        owner: player_entity(slot),
        tick: 0,
        sequences: HashSet::new(),
        datagrams: Vec::new(),
        ticks: Vec::new(),
    }
}

/// The client has the match's instance (20300 with its roster).
fn has_instance(watcher: &Watcher) -> bool {
    watcher.datagrams.iter().any(|(messages, _)| {
        messages.iter().any(|message| {
            message.id == 20300 && message.fields[&0x80] != Value::Struct(Fields::new())
        })
    })
}

/// The client's map is loaded (21602).
fn load(watcher: &mut Watcher, schemas: &Schemas) {
    let loaded = encode(schemas, 21602, &Fields::new()).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    watcher.client.send(ACK, payload.into_bytes());
}

/// The reliable messages the client got, in order, each once.
fn reliable_ids(watcher: &Watcher) -> Vec<u32> {
    watcher
        .datagrams
        .iter()
        .flat_map(|(messages, _)| messages.iter().map(|message| message.id))
        .collect()
}

/// The client got its own controller's frames: he is in the world.
fn in_world(watcher: &Watcher) -> bool {
    watcher
        .find(0, |record| {
            record.entity == watcher.owner && record.script.is_some()
        })
        .is_some()
}

fn no_world(watcher: &Watcher) -> bool {
    watcher
        .datagrams
        .iter()
        .all(|(_, records)| records.is_empty())
}

fn states(server: &GameService, id: MatchId) -> Vec<PlayerState> {
    server
        .snapshot()
        .unwrap()
        .into_iter()
        .find(|found| found.id == id)
        .map(|found| found.players.iter().map(|player| player.state).collect())
        .unwrap_or_default()
}

/// tests/test_game_collision_ready.py
/// `test_unavailable_resources_are_visible_and_return_players_home_once`
/// (and the failure's terminal state of
/// `test_unexpected_loader_failure_is_terminal_and_clears_pending`): when
/// the match's collision fails, each connected game goes back to the menu
/// (20304) once and gets no world, the seat nobody connected to is
/// dropped, and the match takes nobody more. Its players leave it.
#[test]
fn a_failed_collision_sends_connected_games_home_once_and_drops_the_others() {
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let (release, wait) = mpsc::channel::<io::Error>();
    let prepare: ScenePreparation = Box::new(move || Err(wait.recv().unwrap()));
    let tickets = server
        .open_world(
            MATCH,
            GameMap::practice_range(),
            vec![
                world_player("a#1111", 0, 7),
                world_player("b#2222", 0, 8),
                world_player("c#3333", 0, 9),
            ],
            prepare,
        )
        .unwrap();
    let mut a = watch(&server, &tickets[0], 0);
    let mut b = watch(&server, &tickets[1], 1);
    pump_until(
        &mut [&mut a, &mut b],
        &schemas,
        "the instances",
        |watchers| watchers.iter().all(|watcher| has_instance(watcher)),
    );
    load(&mut a, &schemas);
    load(&mut b, &schemas);
    pump_until(&mut [&mut a, &mut b], &schemas, "the games", |watchers| {
        watchers
            .iter()
            .all(|watcher| reliable_ids(watcher).contains(&20301))
    });
    assert!(server.handoff_pending(tickets[2].conn).unwrap());

    release
        .send(io::Error::other("missing collision chunk"))
        .unwrap();
    pump_until(
        &mut [&mut a, &mut b],
        &schemas,
        "the way home",
        |watchers| {
            watchers
                .iter()
                .all(|watcher| reliable_ids(watcher).contains(&20304))
        },
    );
    // Later ticks send nobody home again.
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        a.pump(&schemas);
        b.pump(&schemas);
    }
    for watcher in [&a, &b] {
        let ids = reliable_ids(watcher);
        assert_eq!(ids.iter().filter(|&&id| id == 20304).count(), 1, "{ids:?}");
        assert!(no_world(watcher));
    }

    // The seat nobody connected to is gone, and a late game gets no answer.
    assert!(!server.handoff_pending(tickets[2].conn).unwrap());
    let deadline = Instant::now() + Duration::from_secs(2);
    while server.seat_exists(&tickets[2]).unwrap() {
        assert!(Instant::now() < deadline, "waiting for the dropped seat");
        thread::sleep(Duration::from_millis(10));
    }
    let mut late = Client::new(server.local_addr(), &tickets[2]);
    late.socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    late.send(SYN, Vec::new());
    let mut buffer = [0; MAX_DATAGRAM];
    let error = late.socket.recv(&mut buffer).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ));
    assert!(
        server
            .join_match(MATCH, vec![world_player("d#4444", 0, 10)])
            .unwrap()
            .is_none()
    );

    // The players leave: the match is over.
    for watcher in [&mut a, &mut b] {
        watcher.client.send(FIN | ACK, Vec::new());
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while !server.snapshot().unwrap().is_empty() {
        assert!(
            Instant::now() < deadline,
            "waiting for the players to leave"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!server.seat_exists(&tickets[0]).unwrap());
    assert!(!server.seat_exists(&tickets[1]).unwrap());
    server.stop().unwrap();
}

/// tests/test_game_collision_ready.py
/// `test_pending_spawn_and_commands_wait_while_another_match_runs` (and
/// `test_async_request_returns_pending_until_the_worker_publishes`): the
/// seats come back while the scene is still being prepared; its loaded
/// players get their ownership and game messages (20302, 20301) and
/// nothing of the world while another match's player is in his world and
/// picks a hero. Once the scene is ready both spawn.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn seats_wait_for_their_scene_while_another_match_plays() {
    use orpheus_content::{heroes::SOLDIER, parameters::PICK_HERO};
    use orpheus_sim::scene_assets::SceneAssets;

    let assets = Arc::new(SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap());
    let schemas = Arc::new(Schemas::game());
    let mut server = GameService::bind(local(), Arc::clone(&schemas)).unwrap();
    let (release, wait) = mpsc::channel::<()>();
    let prepared = Arc::new(AtomicUsize::new(0));
    let prepare: ScenePreparation = {
        let (assets, prepared) = (Arc::clone(&assets), Arc::clone(&prepared));
        Box::new(move || {
            wait.recv().unwrap();
            prepared.fetch_add(1, Ordering::Relaxed);
            assets.prepare(GameMap::practice_range())
        })
    };
    let waiting = server
        .open_world(
            MATCH,
            GameMap::practice_range(),
            vec![world_player("a#1111", 0, 7), world_player("b#2222", 0, 8)],
            prepare,
        )
        .unwrap();
    let other = server
        .open_prepared(
            OTHER,
            assets.prepare(GameMap::practice_range()).unwrap(),
            vec![world_player("shooter#3333", 0, 9)],
        )
        .unwrap();
    let mut a = watch(&server, &waiting[0], 0);
    let mut b = watch(&server, &waiting[1], 1);
    let mut shooter = watch(&server, &other[0], 0);
    let mut all = [&mut a, &mut b, &mut shooter];
    pump_until(&mut all, &schemas, "the instances", |watchers| {
        watchers.iter().all(|watcher| has_instance(watcher))
    });
    for watcher in all.iter_mut() {
        load(watcher, &schemas);
    }
    pump_until(&mut all, &schemas, "the other match's world", |watchers| {
        in_world(watchers[2])
    });
    let owner = all[2].owner;
    send_player_message(
        &mut all[2].client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        SOLDIER.to_le_bytes().to_vec(),
    );
    pump_until(&mut all, &schemas, "the other match's body", |watchers| {
        watchers[2].created(owner | 1).is_some()
    });
    pump_until(&mut all, &schemas, "the waiting games", |watchers| {
        watchers[..2]
            .iter()
            .all(|watcher| reliable_ids(watcher).contains(&20301))
    });
    for watcher in &all[..2] {
        // 20302/20301 can go before the world; nothing else does.
        assert_eq!(reliable_ids(watcher), [20300, 20302, 20301]);
        assert!(no_world(watcher));
    }
    assert_eq!(prepared.load(Ordering::Relaxed), 0);
    assert_eq!(states(&server, MATCH), [PlayerState::Loading; 2]);
    assert_eq!(states(&server, OTHER), [PlayerState::Playing]);

    release.send(()).unwrap();
    pump_until(
        &mut all,
        &schemas,
        "the waiting players' world",
        |watchers| watchers[..2].iter().all(|watcher| in_world(watcher)),
    );
    for watcher in &all[..2] {
        assert!(watcher.created(watcher.owner).is_some());
    }
    assert_eq!(prepared.load(Ordering::Relaxed), 1);
    assert_eq!(states(&server, MATCH), [PlayerState::Playing; 2]);
    server.stop().unwrap();
}

/// tests/test_game_collision_ready.py
/// `test_cache_only_missing_map_never_consults_client_and_does_not_poison_builder`:
/// a server without the install (no builder) only reads collision caches,
/// so a map without one fails to prepare and nothing is built; a server
/// with the builder then builds that map's cache once and prepares it.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene without bot VM graphs"]
fn a_cache_only_server_fails_a_missing_map_and_a_builder_still_builds_it() {
    use orpheus_sim::scene_assets::SceneAssets;

    let runtime = orpheus_testkit::runtime_dir();
    let folder = tempfile::tempdir().unwrap();
    let cache = folder.path().join("0688.col");
    let deployed = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_folder(folder.path());
    let error = deployed.prepare(GameMap::practice_range()).err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    assert!(!cache.exists());

    let builds = Arc::new(AtomicUsize::new(0));
    let build: Arc<CollisionBuild> = {
        let (builds, built) = (Arc::clone(&builds), runtime.join("collision/0688.col"));
        Arc::new(move |map, _name| {
            assert_eq!(map, GameMap::practice_range().map_guid);
            builds.fetch_add(1, Ordering::Relaxed);
            std::fs::read(&built)
        })
    };
    let local = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_folder(folder.path())
        .with_collision_build(build);
    local.prepare(GameMap::practice_range()).unwrap();
    assert_eq!(builds.load(Ordering::Relaxed), 1);
    assert!(cache.is_file());
    local.prepare(GameMap::practice_range()).unwrap();
    assert_eq!(builds.load(Ordering::Relaxed), 1);
}
