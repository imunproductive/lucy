//! The host's controls of a running match (`controls.rs`), on Hanamura
//! Assault, whose own graph `0F4A` drives the match: the flow controls run
//! their Workshop actions on the mode entity, a kick sends one game home,
//! and a stop sends the others.

use super::*;
use crate::game_service::{Control, Driven, FlowAction, FlowRefusal};
use lucy_content::maps::{GameModes, MODES_FILE, MapCatalog};
use lucy_sim::{
    match_flow::{ASSAULT, Phase},
    scene_assets::SceneAssets,
};

/// Two players, one a team, in the Assault graph's assembly. Set Match
/// Time `1ABC` gives the phase 5 s on `0F4A`'s clock; Pause `1D53` toggles
/// its `#7898`, so the clock stands and only Unpause is offered; Unpause
/// lets it run. The host kicks the second player: his game goes home
/// (20304), and once it closes its link the first sees "left the game".
/// Declare Team Victory `1384` for team 0 decides the match on the graph
/// (`#7525`): the result goes to the lobby, a win for the first player.
/// Then the host stops the match without waiting for the cards: the first
/// game goes home at the next tick.
#[test]
#[ignore = "requires the derived 104319 data and Hanamura's collision (built from LUCY_GAME_DIR when missing)"]
fn the_host_sets_the_time_pauses_kicks_declares_a_winner_and_stops_an_assault_match() {
    let runtime = lucy_testkit::runtime_dir();
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(collision_build(lucy_testkit::game_dir())),
    );
    let data = runtime.join("data");
    let catalog = MapCatalog::load(&data)
        .unwrap()
        .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
    let hanamura = catalog
        .map_catalog()
        .into_iter()
        .find(|listing| listing.name == "Hanamura")
        .unwrap();
    let map = catalog
        .game_map(hanamura.guid, ASSAULT, "Assault", false, &[6, 6])
        .expect("Hanamura in Assault");
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let player = |tag: &str, team, account| WorldPlayer {
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
    };
    let prepare = {
        let (assets, map) = (Arc::clone(&assets), map.clone());
        Box::new(move || assets.prepare(map))
    };
    let tickets = server
        .open_world(
            MATCH,
            map,
            vec![player("first#1111", 0, 7), player("second#2222", 1, 8)],
            None,
            prepare,
        )
        .unwrap();
    server.await_scenes();
    let mut watchers: Vec<Watcher> = tickets
        .iter()
        .enumerate()
        .map(|(slot, ticket)| {
            let mut client = Client::new(&server, ticket);
            client.handshake();
            Watcher {
                client,
                owner: player_entity(slot),
                tick: 0,
                sequences: HashSet::new(),
                datagrams: Vec::new(),
                ticks: Vec::new(),
            }
        })
        .collect();
    let [first, second] = &mut watchers[..] else {
        unreachable!()
    };
    pump_until(
        &mut [first, second],
        &schemas,
        "the instances",
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher
                    .datagrams
                    .iter()
                    .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
            })
        },
    );
    for watcher in [&mut *first, &mut *second] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    let flow = |server: &Lockstep| server.snapshot().unwrap()[0].flow.clone();
    pump_until(&mut [first, second], &schemas, "the assembly", |_| {
        flow(&server).phase == Some(Phase::Assemble)
    });
    let state = flow(&server);
    assert_eq!(
        (state.driven, state.mode_graph),
        (Driven::Graph, Some(0x0f4a))
    );
    // `0F4A` has entries for `08D8`, `06D5`, `09A3` and `0F00`, none for a
    // round's winner (`0EAB`).
    assert_eq!(
        state.controls,
        [
            Control::SetTime,
            Control::SkipPhase,
            Control::Pause,
            Control::DeclareWinner,
            Control::Draw,
            Control::Assemble,
            Control::Completion,
        ]
    );
    assert_eq!(
        server.run_flow(MATCH, FlowAction::RoundWinner(0)).unwrap(),
        Err(FlowRefusal::NotAllowed)
    );

    // Set Match Time 5: the graph's clock has 5 s, less the tick it ran.
    server
        .run_flow(MATCH, FlowAction::SetTime(5.0))
        .unwrap()
        .unwrap();
    server.step();
    let clock = flow(&server).clock;
    assert!(clock > 4.9 && clock <= 5.0, "{clock}");

    // Pause: the clock stands for a second; Unpause lets it run.
    server.run_flow(MATCH, FlowAction::Pause).unwrap().unwrap();
    server.step();
    let paused = flow(&server);
    assert!(paused.paused);
    assert!(paused.controls.contains(&Control::Unpause));
    assert!(!paused.controls.contains(&Control::Pause));
    server.run(Duration::from_secs(1));
    assert_eq!(flow(&server).clock, paused.clock);
    server
        .run_flow(MATCH, FlowAction::Unpause)
        .unwrap()
        .unwrap();
    server.run(Duration::from_secs(1));
    let running = flow(&server);
    assert!(!running.paused);
    assert!(running.clock < paused.clock - 0.9, "{running:?}");

    // The kick: 20304 for the second game, "left the game" for the first
    // once it closes its link.
    assert!(!server.kick(MATCH, 9).unwrap());
    let kicked = second.datagrams.len();
    assert!(server.kick(MATCH, 8).unwrap());
    pump_until(&mut [first, second], &schemas, "the 20304", |watchers| {
        watchers[1].datagrams[kicked..]
            .iter()
            .any(|(messages, _)| messages.iter().any(|message| message.id == 20304))
    });
    let left = first.datagrams.len();
    second.client.send(FIN | ACK, Vec::new());
    pump_until(&mut [first], &schemas, "left the game", |watchers| {
        watchers[0].card(left, "second#2222").is_some()
    });
    assert_eq!(
        first.card(left, "second#2222").unwrap().1,
        [true, false, false, true]
    );

    // Declare Team Victory for team 0: the graph's match end, and the
    // result for the lobby.
    server
        .run_flow(MATCH, FlowAction::DeclareWinner(0))
        .unwrap()
        .unwrap();
    pump_until(&mut [first], &schemas, "the match end", |_| {
        flow(&server).phase == Some(Phase::MatchEnd)
    });
    assert_eq!(flow(&server).winner, Some(0));
    assert!(flow(&server).controls.is_empty());
    // The room's result reaches the router (`Events::result`) at its tick.
    server.step();
    let results = server.take_results().unwrap();
    let [result] = results.as_slice() else {
        panic!("{results:?}")
    };
    let outcomes: Vec<_> = result
        .players
        .iter()
        .map(|player| (player.name.as_str(), player.outcome, player.finished))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("first#1111", Outcome::Win, true),
            ("second#2222", Outcome::Loss, false)
        ]
    );

    // The stop: the first game goes home now, during the cards (56.4 s),
    // as soon as its reliable messages before it are through.
    let stopped = first.datagrams.len();
    assert_eq!(server.stop_match(MATCH).unwrap(), 1);
    pump_within(
        &mut [first],
        &schemas,
        "the stop's 20304",
        Duration::from_secs(1),
        |watchers| {
            watchers[0].datagrams[stopped..]
                .iter()
                .any(|(messages, _)| messages.iter().any(|message| message.id == 20304))
        },
    );
    server.stop().unwrap();
}
