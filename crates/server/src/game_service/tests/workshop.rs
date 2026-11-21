//! A Workshop match hosted by the game service: C32S2, the Doomfist parkour
//! of Hanamura (the DPK archive), as `--workshop` opens it
//! (`lobby::matchmaker::workshop`): its settings' map, team sizes, heroes
//! and ruleset values, its rules attached to the match
//! ([`GameService::attach_workshop`]).

use super::*;
use crate::workshop_match::{ScriptMatch, SettingNames};
use orpheus_content::{
    maps::{GameModes, MODES_FILE, MapCatalog},
    parameters::PICK_HERO,
};
use orpheus_script::wire::{DeltaInstance, read_delta_instances, read_full_head};
use orpheus_sim::scene_assets::SceneAssets;

const DOOMFIST: u64 = 0x02e0_0000_0000_012f;
const CREATE_HUD_TEXT: u16 = 0x1d8a;
/// A match ruleset of the test's own (`custom.rs` `MATCH_RULESETS`).
const RULESET: u64 = 0x0fd0_8000_0000_0001;

/// The graphs of the instances a mode entity's frame lists: a whole
/// frame's, or the new ones of a delta.
fn listed_graphs(graphs: &orpheus_script::graph::GraphStore, chunk: &ReceivedChunk) -> Vec<u16> {
    if chunk.first == 0 {
        let head = read_full_head(&chunk.data, |graph| {
            graphs
                .graph(u32::from(graph))
                .map(|graph| graph.header.states_bits)
        })
        .expect("a full frame's head");
        return head
            .instances
            .iter()
            .map(|&(_, graph, _, _)| graph)
            .collect();
    }
    read_delta_instances(&chunk.data)
        .expect("a delta's instances")
        .into_iter()
        .filter_map(|instance| match instance {
            DeltaInstance::Described((_, graph, _, _)) => Some(graph),
            _ => None,
        })
        .collect()
}

/// The player picks Doomfist, the only hero C32S2 allows, and the rules
/// run against him: "Player HUD" creates his HUD texts, each a Create HUD
/// Text `1D8A` instance on the mode entity (its scope 31) that the
/// mode entity's frames carry to his game. The mode entity's root is
/// Assault's own graph `0F4A`, run whole. The ruleset's health packs are
/// off, so the map's never reach him.
#[test]
#[ignore = "requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn dpk_archive_c32s2_hosted_match_shows_a_hud_text_on_the_mode_entity() {
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let catalog = Arc::new(orpheus_workshop::Catalog::load(&data).unwrap());
    let path =
        orpheus_testkit::dpk_archive().join("workshop.codes/C32S2_OG_doom_parkur_hanamura.txt");
    let script = ScriptMatch::parse(
        &std::fs::read_to_string(path).unwrap(),
        &catalog,
        &SettingNames::load(&data).unwrap(),
    )
    .unwrap();
    let [game] = script.games.as_slice() else {
        panic!("{:?}", script.games)
    };
    let hanamura = game.maps.as_ref().unwrap()[0];
    let maps = MapCatalog::load(&data)
        .unwrap()
        .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
    let mut map = maps
        .game_map(hanamura, game.mode, "Assault", false, &[12, 0])
        .expect("Hanamura in Assault");
    map.hero_pool = script.heroes.clone().unwrap();
    map.ruleset = RULESET;
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(collision_build(orpheus_testkit::game_dir())),
    );
    assets.graphs().add_match_ruleset(
        RULESET,
        map.mode_guid,
        game.values.clone(),
        script.bodies.clone(),
    );

    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let prepare = {
        let (assets, map) = (Arc::clone(&assets), map.clone());
        Box::new(move || assets.prepare(map))
    };
    // Workshop Team 1: the room's team 0.
    let players = vec![WorldPlayer {
        player: player().remove(0),
        card: PlayerCard::default(),
        hero: 0,
    }];
    let tickets = server
        .open_world(MATCH, map, players, None, prepare)
        .unwrap();
    assert!(
        server
            .attach_workshop(MATCH, Arc::clone(&script.logic), catalog)
            .unwrap()
    );
    server.await_scenes();
    let mut client = Client::new(&server, &tickets[0]);
    client.handshake();
    let mut watcher = Watcher {
        client,
        owner: player_entity(0),
        tick: 0,
        sequences: HashSet::new(),
        datagrams: Vec::new(),
        ticks: Vec::new(),
    };
    pump_until(&mut [&mut watcher], &schemas, "the instance", |watchers| {
        watchers[0]
            .datagrams
            .iter()
            .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
    });
    let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
    let mut payload = BitWriter::new();
    write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
    watcher.client.send(ACK, payload.into_bytes());
    pump_until(&mut [&mut watcher], &schemas, "the world", |watchers| {
        let watcher = &watchers[0];
        watcher.created(GAME_MODE_ENTITY).is_some()
            && watcher
                .find(0, |record| {
                    record.entity == watcher.owner && record.script.is_some()
                })
                .is_some()
    });
    let owner = watcher.owner;
    // The owner's newest controller frame: what it shows of hero select
    // (open, the pick, the hero he plays; `controller_shows`). Besides the
    // hero select tree, the controller carries the Workshop actions' player
    // scopes (`STU_550D0C5C`) the rules opened on him.
    let select = |watcher: &Watcher| {
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .filter(|record| record.entity == watcher.owner)
            .filter_map(|record| record.script.as_ref())
            .filter_map(|chunk| controller_head(assets.graphs(), chunk))
            .map(|(shown, instances)| {
                assert_eq!(instances[..5], [0x0c90, 0x288a, 0x0215, 0x288b, 0x288d]);
                shown
            })
            .reduce(|_, newer| newer)
    };
    // Rule "Match time": Set Match Time(1) at 0.25 s, again 1.1 s later,
    // then Set Match Time(3600) after another 1.1 s: Assemble ends at
    // about 1.25 s, Setup at 2.35 s, and the Round runs from then on. The
    // player hasn't picked: hero select stays open for him, as in retail
    // for a player with no hero in any phase.
    let world = watcher.client.now();
    pump_within(
        &mut [&mut watcher],
        &schemas,
        "the round",
        Duration::from_secs(5),
        |watchers| watchers[0].client.now() >= world + Duration::from_secs(4),
    );
    assert_eq!(select(&watcher), Some((true, None, None)));
    send_player_message(
        &mut watcher.client,
        &schemas,
        owner,
        1,
        PICK_HERO,
        DOOMFIST.to_le_bytes().to_vec(),
    );
    // His body comes, and hero select closes once it lives (`288B` st95).
    pump_until(&mut [&mut watcher], &schemas, "Doomfist", |watchers| {
        watchers[0].created(owner | 1).is_some()
            && select(watchers[0]) == Some((false, Some(DOOMFIST), Some(DOOMFIST)))
    });
    // "Player HUD" waits at most 2 + 2 s before its texts. The mode
    // entity's frame (the rules' global arrays and actions, about 10 KB)
    // comes in parts the client joins (`orpheus_sim::mode_view`).
    let graphs = assets.graphs();
    let mode_chunks = |watcher: &Watcher| {
        joined_chunks(
            watcher
                .datagrams
                .iter()
                .flat_map(|(_, records)| records)
                .filter(|record| record.entity == GAME_MODE_ENTITY)
                .filter_map(|record| record.script.as_ref()),
        )
    };
    pump_within(
        &mut [&mut watcher],
        &schemas,
        "a HUD text on the mode entity",
        Duration::from_secs(15),
        |watchers| {
            mode_chunks(&*watchers[0])
                .iter()
                .any(|chunk| listed_graphs(graphs, chunk).contains(&CREATE_HUD_TEXT))
        },
    );
    assert!(
        watcher
            .datagrams
            .iter()
            .flat_map(|(_, records)| records)
            .any(|record| record.entity == GAME_MODE_ENTITY
                && record
                    .script
                    .as_ref()
                    .is_some_and(|chunk| chunk.part.is_some())),
        "no mode frame in parts"
    );
    // The root instance (1) is `0F4A`: it ran with no gap (this build's VM
    // refuses one, `Unknowns::for_build`), so the client starts its
    // client-only chain `1FEB` → `0CB6` → `106B`, which makes the HUD
    // texts' containers (`refs/reports/ModeGraphGaps.md`), rather than
    // `0CBB` alone.
    let roots: Vec<u16> = mode_chunks(&watcher)
        .iter()
        .filter(|chunk| chunk.first == 0)
        .map(|chunk| {
            let head = read_full_head(&chunk.data, |graph| {
                graphs
                    .graph(u32::from(graph))
                    .map(|graph| graph.header.states_bits)
            })
            .expect("a full frame's head");
            head.instances
                .iter()
                .find(|&&(instance, ..)| instance == 1)
                .expect("the root instance")
                .1
        })
        .collect();
    assert!(!roots.is_empty(), "no full mode frame");
    assert!(roots.iter().all(|&graph| graph == 0x0f4a), "{roots:04X?}");
    // C32S2's "Spawn Health Packs: Disabled" (#9542e): each of Hanamura's
    // 18 health packs (server placeables `0FC2`/`0FC3`, game_map_areas)
    // plays `0046` on itself (`0216`/`0217`), which removes the pack one
    // server frame after it starts (rid28-31, `STU_523EE38D`), before the
    // world reached the client; the other server placeables came (456, a
    // `20C9`).
    for pack in [
        182, 205, 243, 266, 278, 312, 391, 396, 409, 431, 604, 653, 748, 780, 810, 891, 920, 1050,
    ] {
        assert!(
            watcher.created(0x8000_0000 | pack).is_none(),
            "health pack {pack}"
        );
    }
    assert!(watcher.created(0x8000_0000 | 456).is_some());
    server.stop().unwrap();
}

/// Rules of [`a_workshop_match_keeps_each_team_in_its_own_spawn_room`]:
/// the match's clock skips assemble and setup, and Team 1's player goes
/// to point A 2 s after he spawns.
const ROOM_TELEPORT: &str = r#"
rule("Match time")
{
	event
	{
		Ongoing - Global;
	}

	actions
	{
		Wait(0.250, Ignore Condition);
		Set Match Time(1);
		Wait(1.100, Ignore Condition);
		Set Match Time(1);
		Wait(1.100, Ignore Condition);
		Set Match Time(3600);
	}
}

rule("Team 1 goes to point A")
{
	event
	{
		Ongoing - Each Player;
		Team 1;
		All;
	}

	conditions
	{
		Has Spawned(Event Player) == True;
	}

	actions
	{
		Wait(2.000, Ignore Condition);
		Teleport(Event Player, Vector(-4.265, 1.156, 0.578));
	}
}
"#;

/// Two Doomfists, one a team, in a Workshop match on Hanamura Assault
/// ([`ROOM_TELEPORT`]). Each spawns in his own team's spawn room: its
/// `0D62` sends him `0723`, so his `0C8E` heals him (st13), and no room
/// of the other team sends him `0B7B` (st45, 400 a second). Then Team 1's
/// rule teleports him to point A, out of his room: the room's search
/// drops him and lets its `0723` go, so the heal stops.
#[test]
#[ignore = "requires an Overwatch 1.74 install (ORPHEUS_GAME_DIR)"]
fn a_workshop_match_keeps_each_team_in_its_own_spawn_room() {
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let catalog = Arc::new(orpheus_workshop::Catalog::load(&data).unwrap());
    let logic = Arc::new(
        orpheus_workshop::parse(ROOM_TELEPORT, &catalog)
            .unwrap()
            .logic,
    );
    let maps = MapCatalog::load(&data)
        .unwrap()
        .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
    let hanamura = maps
        .map_catalog()
        .into_iter()
        .find(|listing| listing.name == "Hanamura")
        .unwrap();
    let map = maps
        .game_map(
            hanamura.guid,
            orpheus_sim::match_flow::ASSAULT,
            "Assault",
            false,
            &[1, 1],
        )
        .expect("Hanamura in Assault");
    let assets = Arc::new(
        SceneAssets::load(&runtime)
            .unwrap()
            .with_collision_build(collision_build(orpheus_testkit::game_dir())),
    );
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let prepare = {
        let (assets, map) = (Arc::clone(&assets), map.clone());
        Box::new(move || assets.prepare(map))
    };
    let players = ["first#1111", "second#2222"]
        .iter()
        .enumerate()
        .map(|(team, tag)| WorldPlayer {
            player: MatchPlayer {
                name: (*tag).into(),
                team: team as u8,
                tournament: false,
                ranked: false,
            },
            card: PlayerCard::default(),
            hero: 0,
        })
        .collect();
    let tickets = server
        .open_world(MATCH, map, players, None, prepare)
        .unwrap();
    assert!(server.attach_workshop(MATCH, logic, catalog).unwrap());
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
    pump_until(&mut [first, second], &schemas, "the instance", |watchers| {
        watchers.iter().all(|watcher| {
            watcher
                .datagrams
                .iter()
                .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
        })
    });
    for watcher in [&mut *first, &mut *second] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    // "Match time" ends Assemble and Setup by 2.35 s; they pick in the
    // Round.
    let world = first.client.now();
    pump_within(
        &mut [first, second],
        &schemas,
        "the round",
        Duration::from_secs(6),
        |watchers| watchers[0].client.now() >= world + Duration::from_secs(4),
    );
    for watcher in [&mut *first, &mut *second] {
        let owner = watcher.owner;
        send_player_message(
            &mut watcher.client,
            &schemas,
            owner,
            1,
            PICK_HERO,
            DOOMFIST.to_le_bytes().to_vec(),
        );
    }
    let (mut first_input, mut second_input) = (first.tick + 1, second.tick + 1);
    let mut step = |first: &mut Watcher, second: &mut Watcher| {
        send_input(first, &mut first_input, 0, 0, 0, false);
        send_input(second, &mut second_input, 0, 0, 0, false);
        first.pump(&schemas);
        second.pump(&schemas);
    };
    let deadline = server.now() + Duration::from_secs(5);
    while server
        .bodies(MATCH)
        .unwrap()
        .iter()
        .any(|body| body.health.is_none())
    {
        assert!(server.now() < deadline, "{:?}", server.bodies(MATCH));
        step(first, second);
    }
    // A second in their rooms: each room's search (every 12 frames) has
    // found its team's player.
    let settled = server.now() + Duration::from_secs(1);
    while server.now() < settled {
        step(first, second);
        for body in server.bodies(MATCH).unwrap() {
            assert!(!body.room_hurt, "{body:?}");
            assert_eq!(body.health, Some(1.0), "{body:?}");
        }
    }
    for body in server.bodies(MATCH).unwrap() {
        assert_eq!(
            (body.spawn_room, body.room_heal),
            (Some(true), true),
            "{body:?}"
        );
    }
    // Team 1 on point A, out of his room.
    let on_point = |server: &Lockstep| {
        server.bodies(MATCH).unwrap()[0]
            .position
            .is_some_and(|feet| (feet[0] + 4.265).hypot(feet[2] - 0.578) < 0.5)
    };
    let deadline = server.now() + Duration::from_secs(3);
    while !on_point(&server) {
        assert!(server.now() < deadline, "{:?}", server.bodies(MATCH));
        step(first, second);
    }
    let left = server.now() + Duration::from_millis(500);
    while server.now() < left {
        step(first, second);
    }
    let [team1, team2] = <[_; 2]>::try_from(server.bodies(MATCH).unwrap()).unwrap();
    assert_eq!(
        (team1.spawn_room, team1.room_heal, team1.room_hurt),
        (Some(false), false, false),
        "{team1:?}"
    );
    assert_eq!(
        (
            team2.spawn_room,
            team2.room_heal,
            team2.room_hurt,
            team2.health
        ),
        (Some(true), true, false, Some(1.0)),
        "{team2:?}"
    );
    server.stop().unwrap();
}
