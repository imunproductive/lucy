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

/// A Workshop script with a rule on each player event, its variables the
/// events' counts (`a_workshop_rule_runs_on_each_player_event`).
const PLAYER_EVENTS: &str = r#"
variables
{
	player:
		0: Dealt
		1: Took
		2: Died
		3: Elims
		4: FinalBlows
		5: HealDealt
		6: HealReceived
		7: DealtKnock
		8: ReceivedKnock
		9: KnockAbility
		10: PunchDealt
		11: PunchTaken
}

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

rule("Team 1 stands on point A")
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
		Teleport(Event Player, Vector(-4.265, 1.156, 0.578));
	}
}

rule("Team 2 stands 2.5 m ahead of him")
{
	event
	{
		Ongoing - Each Player;
		Team 2;
		All;
	}

	conditions
	{
		Has Spawned(Event Player) == True;
		Event Player.Died == 0;
	}

	actions
	{
		Teleport(Event Player, Vector(-4.265, 1.156, 3.078));
	}
}

rule("Dealt knockback")
{
	event
	{
		Player Dealt Knockback;
		All;
		All;
	}

	conditions
	{
		Victim != Event Player;
	}

	actions
	{
		Event Player.DealtKnock += 1;
		Event Player.KnockAbility = Event Ability == Button(Secondary Fire);
	}
}

rule("Received knockback")
{
	event
	{
		Player Received Knockback;
		All;
		All;
	}

	conditions
	{
		Attacker != Event Player;
	}

	actions
	{
		Event Player.ReceivedKnock += 1;
	}
}

rule("After his punch, Team 1 hurts and kills Team 2")
{
	event
	{
		Ongoing - Each Player;
		Team 1;
		All;
	}

	conditions
	{
		Event Player.DealtKnock >= 1;
	}

	actions
	{
		Wait(0.500, Ignore Condition);
		Damage(Players In Slot(0, Team 2), Event Player, 50);
		Wait(0.500, Ignore Condition);
		Damage(Players In Slot(0, Team 2), Event Player, 1000);
	}
}

rule("Team 2 heals himself after the first Workshop damage")
{
	event
	{
		Player Took Damage;
		Team 2;
		All;
	}

	conditions
	{
		Event Ability == Null;
		Event Damage < 100;
	}

	actions
	{
		Heal(Event Player, Event Player, 20);
	}
}

rule("Dealt damage")
{
	event
	{
		Player Dealt Damage;
		All;
		All;
	}

	conditions
	{
		Victim != Event Player;
	}

	actions
	{
		If(Event Ability == Button(Secondary Fire));
			Event Player.PunchDealt += Event Damage;
		Else If(Event Ability == Null);
			Event Player.Dealt += Event Damage;
		End;
	}
}

rule("Took damage")
{
	event
	{
		Player Took Damage;
		All;
		All;
	}

	conditions
	{
		Attacker != Event Player;
	}

	actions
	{
		If(Event Ability == Button(Secondary Fire));
			Event Player.PunchTaken += Event Damage;
		Else If(Event Ability == Null);
			Event Player.Took += Event Damage;
		End;
	}
}

rule("Died")
{
	event
	{
		Player Died;
		All;
		All;
	}

	conditions
	{
		Attacker != Event Player;
	}

	actions
	{
		Event Player.Died += 1;
	}
}

rule("Earned elimination")
{
	event
	{
		Player Earned Elimination;
		All;
		All;
	}

	actions
	{
		Event Player.Elims += 1;
	}
}

rule("Dealt final blow")
{
	event
	{
		Player Dealt Final Blow;
		All;
		All;
	}

	actions
	{
		Event Player.FinalBlows += 1;
	}
}

rule("Dealt healing")
{
	event
	{
		Player Dealt Healing;
		All;
		All;
	}

	actions
	{
		Event Player.HealDealt += Event Healing;
	}
}

rule("Received healing")
{
	event
	{
		Player Received Healing;
		All;
		All;
	}

	actions
	{
		Event Player.HealReceived += Event Healing;
	}
}
"#;

/// Two Doomfists, one a team, in a Workshop match on Hanamura Assault
/// whose rules count each player event in player variables
/// ([`PLAYER_EVENTS`]): the rules put them 2.5 m apart on point A (out of
/// both spawn rooms, whose `0C8E` hurts an enemy inside), and
/// Team 1's client punches Team 2 (a tapped Rocket Punch, held 0.3 s). His
/// `0D06` plays its victim graph `0D07` on Team 2's body, whose velocity
/// action pushes it and whose `STU_AFBB59E9` reports the knockback:
/// Player Dealt Knockback for Team 1 with Event Ability Secondary Fire,
/// Player Received Knockback for Team 2, and the punch's damage under
/// Secondary Fire. Then Team 1's rule deals 50 and 1000 with the
/// Workshop's Damage (Event Ability Null): Team 2 heals himself 20 after
/// the first (Player Dealt and Received Healing, both his), and the second
/// kills him: Player Died, Player Earned Elimination and Player Dealt Final
/// Blow, each once, and the damage dealt is the damage taken.
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn a_workshop_rule_runs_on_each_player_event() {
    use orpheus_wire::commands::SECONDARY;
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let catalog = Arc::new(orpheus_workshop::Catalog::load(&data).unwrap());
    let logic = Arc::new(
        orpheus_workshop::parse(PLAYER_EVENTS, &catalog)
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
            orpheus_sim::match_flow::SKIRMISH,
            "Assault",
            false,
            &[1, 1],
        )
        .expect("Hanamura in Assault");
    // Rocket Punch's camera states run plain, as in a release build
    // (`refs/reports/Doomfist.md` §1).
    let mut assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(collision_build(orpheus_testkit::game_dir()));
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let assets = Arc::new(assets);
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let prepare = {
        let (assets, map) = (Arc::clone(&assets), map.clone());
        Box::new(move || assets.prepare(map))
    };
    let players = ["puncher#1111", "punched#2222"]
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
    let [puncher, punched] = &mut watchers[..] else {
        unreachable!()
    };
    pump_until(
        &mut [puncher, punched],
        &schemas,
        "the instance",
        |watchers| {
            watchers.iter().all(|watcher| {
                watcher
                    .datagrams
                    .iter()
                    .any(|(messages, _)| messages.iter().any(|message| message.id == 20300))
            })
        },
    );
    for watcher in [&mut *puncher, &mut *punched] {
        let loaded = encode(&schemas, 21602, &Fields::new()).unwrap();
        let mut payload = BitWriter::new();
        write_channel(&mut payload, &[(0, loaded)], &[]).unwrap();
        watcher.client.send(ACK, payload.into_bytes());
    }
    // "Match time" ends Assemble and Setup by 2.35 s; the players pick
    // Doomfist in the Round.
    let world = puncher.client.now();
    pump_within(
        &mut [puncher, punched],
        &schemas,
        "the round",
        Duration::from_secs(6),
        |watchers| watchers[0].client.now() >= world + Duration::from_secs(4),
    );
    for watcher in [&mut *puncher, &mut *punched] {
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
    let variables = |server: &Lockstep| -> Vec<Vec<f64>> {
        server
            .bodies(MATCH)
            .unwrap()
            .iter()
            .map(|body| {
                body.workshop
                    .iter()
                    .map(|value| {
                        use orpheus_script::wire::Value as ScriptValue;
                        match value {
                            ScriptValue::Float(value) => *value,
                            ScriptValue::Integer(value) => *value as f64,
                            ScriptValue::Bool(value) => f64::from(u8::from(*value)),
                            _ => 0.0,
                        }
                    })
                    .collect()
            })
            .collect()
    };
    // Both stand where the rules put them; the clients go on sending their
    // commands, which run their bodies' scripts.
    let placed = |server: &Lockstep| {
        let bodies = server.bodies(MATCH).unwrap();
        bodies.len() == 2
            && [[-4.265, 0.578], [-4.265, 3.078]]
                .iter()
                .zip(&bodies)
                .all(|(&[x, z], body)| {
                    body.position
                        .is_some_and(|feet| (feet[0] - x).hypot(feet[2] - z) < 0.5)
                })
    };
    let yaw = |server: &Lockstep| {
        let bodies = server.bodies(MATCH).unwrap();
        let [from, to] = [0, 1].map(|slot| bodies[slot].position.unwrap());
        let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
        (dx.atan2(dz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16
    };
    let (mut punching, mut idle) = (puncher.tick + 1, punched.tick + 1);
    let mut step = |server: &Lockstep, puncher: &mut Watcher, punched: &mut Watcher, buttons| {
        let aim = if placed(server) { yaw(server) } else { 0 };
        send_input(puncher, &mut punching, aim, 0, buttons, false);
        send_input(punched, &mut idle, 0, 0, 0, false);
        puncher.pump(&schemas);
        punched.pump(&schemas);
    };
    let deadline = server.now() + Duration::from_secs(15);
    while !placed(&server) {
        assert!(
            server.now() < deadline,
            "placing: {:?}",
            server.bodies(MATCH)
        );
        step(&server, puncher, punched, 0);
    }
    let settled = server.now() + Duration::from_millis(500);
    while server.now() < settled {
        step(&server, puncher, punched, 0);
    }
    let punch = server.now() + Duration::from_millis(300);
    while server.now() < punch {
        step(&server, puncher, punched, SECONDARY);
    }
    let deadline = server.now() + Duration::from_secs(8);
    while variables(&server)[1][2] < 1.0 {
        assert!(
            server.now() < deadline,
            "{:?}\n{:?}\n{:?}",
            variables(&server),
            server.workshop_report(MATCH),
            server
                .bodies(MATCH)
                .unwrap()
                .iter()
                .map(|body| (body.position, body.health, body.hero))
                .collect::<Vec<_>>()
        );
        step(&server, puncher, punched, 0);
    }
    let [team1, team2] = <[Vec<f64>; 2]>::try_from(variables(&server)).unwrap();
    // Knockback: once each way, with Rocket Punch's button.
    assert_eq!(
        (team1[7], team1[9], team2[8]),
        (1.0, 1.0, 1.0),
        "{team1:?} {team2:?}"
    );
    // The punch's damage, under Secondary Fire.
    assert!(
        team1[10] > 0.0 && team1[10] == team2[11],
        "{team1:?} {team2:?}"
    );
    // His own 20 heal.
    assert_eq!((team2[5], team2[6]), (20.0, 20.0), "{team2:?}");
    // The Workshop's 50, then what was left: 250 - punch - 50 + 20.
    let left = 250.0 - team2[11] - 50.0 + 20.0;
    assert_eq!(
        (team1[0], team2[1]),
        (50.0 + left, 50.0 + left),
        "{team1:?} {team2:?}"
    );
    assert_eq!(
        (team2[2], team1[3], team1[4]),
        (1.0, 1.0, 1.0),
        "{team1:?} {team2:?}"
    );
    server.stop().unwrap();
}

/// Create Dummy Bot `203A` makes an AI player in the room's next slot, on
/// the team its Workshop team names (`STU_382A5949`: Team 2 is the room's
/// team 1), of its hero, standing where it says; Destroy Dummy Bot `2080`
/// of that team and slot takes him out again (his body goes).
#[test]
#[ignore = "requires an Overwatch 1.74 install (just test-game <install>)"]
fn a_workshop_dummy_bot_joins_and_leaves() {
    const ANA: u64 = 0x02e0_0000_0000_013b;
    const RULES: &str = "rule(\"bot\")\n{\n\tevent\n\t{\n\t\tOngoing - Global;\n\t}\n\n\tactions\n\t{\n\t\tWait(1, Ignore Condition);\n\t\tCreate Dummy Bot(Hero(Ana), Team 2, 0, Vector(-4.265, 1.156, 3.078), Vector(0, 0, 1));\n\t\tWait(2, Ignore Condition);\n\t\tDestroy Dummy Bot(Team 2, 0);\n\t}\n}\n";
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let catalog = Arc::new(orpheus_workshop::Catalog::load(&data).unwrap());
    let logic = Arc::new(orpheus_workshop::parse(RULES, &catalog).unwrap().logic);
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
            orpheus_sim::match_flow::SKIRMISH,
            "Assault",
            false,
            &[1, 1],
        )
        .expect("Hanamura in Assault");
    let mut assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(collision_build(orpheus_testkit::game_dir()));
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let assets = Arc::new(assets);
    let schemas = Arc::new(Schemas::game());
    let mut server = Lockstep::new(Arc::clone(&schemas)).unwrap();
    let prepare = {
        let (assets, map) = (Arc::clone(&assets), map.clone());
        Box::new(move || assets.prepare(map))
    };
    let players = vec![WorldPlayer {
        player: player().remove(0),
        card: PlayerCard::default(),
        hero: 0,
    }];
    let tickets = server
        .open_world(MATCH, map, players, None, prepare)
        .unwrap();
    assert!(server.attach_workshop(MATCH, logic, catalog).unwrap());
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
    let mut next = watcher.tick + 1;
    let mut run_until =
        |server: &Lockstep,
         what: &str,
         done: &dyn Fn(&[crate::game_service::ai::BodyState]) -> bool| {
            let deadline = server.now() + Duration::from_secs(6);
            loop {
                let bodies = server.bodies(MATCH).unwrap();
                if done(&bodies) {
                    return bodies;
                }
                assert!(
                    server.now() < deadline,
                    "{what}: {bodies:?}\n{:?}",
                    server.workshop_report(MATCH)
                );
                send_input(&mut watcher, &mut next, 0, 0, 0, false);
                watcher.pump(&schemas);
            }
        };
    let joined = run_until(&server, "the bot joins", &|bodies| {
        bodies
            .get(1)
            .is_some_and(|bot| bot.ai && bot.team == 1 && bot.hero == Some(ANA))
    });
    let feet = joined[1].position.expect("the bot's body");
    assert!(
        (feet[0] + 4.265).hypot(feet[2] - 3.078) < 0.5,
        "the bot at {feet:?}"
    );
    run_until(&server, "the bot leaves", &|bodies| {
        bodies[1].hero.is_none()
    });
    server.stop().unwrap();
}

/// What one of the archive's scripts met in a hosted match
/// ([`slow_dpk_archive_scripts_run_in_a_hosted_match`]): its map, whether
/// its rules ran, and what they could not do. Err: no map of its modes
/// plays here, or its rules didn't run, its hero didn't spawn, or a value
/// was unsupported or the engine faulted.
fn survey_script(path: &std::path::Path) -> Result<String, String> {
    let runtime = orpheus_testkit::runtime_dir();
    let data = runtime.join("data");
    let catalog = Arc::new(orpheus_workshop::Catalog::load(&data).unwrap());
    let script = ScriptMatch::parse(
        &std::fs::read_to_string(path).unwrap(),
        &catalog,
        &SettingNames::load(&data).unwrap(),
    )
    .unwrap();
    let maps = MapCatalog::load(&data)
        .unwrap()
        .with_modes(GameModes::load(data.join(MODES_FILE)).unwrap());
    let listed: Vec<u64> = maps
        .map_catalog()
        .into_iter()
        .map(|listing| listing.guid)
        .collect();
    // The first map of its modes the server hosts, as `--workshop` takes
    // it (`lobby::matchmaker::workshop`).
    let found = script.games.iter().find_map(|game| {
        let candidates = game.maps.clone().unwrap_or_else(|| {
            listed
                .iter()
                .copied()
                .filter(|map| !game.without.contains(map))
                .collect()
        });
        candidates
            .into_iter()
            .find_map(|map| {
                [false, true].into_iter().find_map(|free_for_all| {
                    maps.game_map(map, game.mode, "Workshop", free_for_all, &[1, 1])
                })
            })
            .map(|map| (map, game))
    });
    let Some((mut map, game)) = found else {
        let games: Vec<String> = script
            .games
            .iter()
            .map(|game| format!("{:X} {:X?}", game.mode, game.maps))
            .collect();
        return Err(format!(
            "no map of its modes plays here (games {games:?}, skipped {:?})",
            script.skipped
        ));
    };
    let name = map.name.clone();
    map.hero_pool = script.heroes.clone().unwrap_or_default();
    map.ruleset = RULESET;
    let hero = script
        .heroes
        .as_ref()
        .and_then(|heroes| heroes.first().copied())
        .unwrap_or(DOOMFIST);
    // The heroes' gaps as in a release build: the survey is about the
    // Workshop's.
    let mut assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(collision_build(orpheus_testkit::game_dir()));
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let assets = Arc::new(assets);
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
    let mut next = watcher.tick + 1;
    let started = server.now();
    let mut picked = false;
    while server.now() < started + Duration::from_secs(20) {
        if !picked && server.now() >= started + Duration::from_secs(4) {
            let owner = watcher.owner;
            send_player_message(
                &mut watcher.client,
                &schemas,
                owner,
                1,
                PICK_HERO,
                hero.to_le_bytes().to_vec(),
            );
            picked = true;
        }
        send_input(&mut watcher, &mut next, 0, 0, 0, false);
        watcher.pump(&schemas);
    }
    let report = server.workshop_report(MATCH).unwrap().unwrap();
    let alive = server.bodies(MATCH).unwrap()[0].hero.is_some();
    server.stop().unwrap();
    let mut gaps: Vec<String> = report
        .gaps
        .iter()
        .map(|((graph, class), count)| format!("{graph:04X} {class} x{count}"))
        .collect();
    gaps.sort();
    let line = format!(
        "{name}: rules run {}, hero {alive}, unsupported {:?}, faults {:?}, gaps {gaps:?}",
        report.running, report.unsupported, report.faults
    );
    if report.running && alive && report.unsupported.is_empty() && report.faults.is_empty() {
        Ok(line)
    } else {
        Err(line)
    }
}

/// Every script of the archive the 1.74 client imports (the workshop
/// crate's `dpk_archive_scripts_parse_round_trip_and_run` lists the four it
/// refuses), each in its own hosted match on the first map of its modes
/// for 20 s with one player who picks its first allowed hero: its rules
/// run, its hero spawns, and no value is unsupported nor the engine
/// faults; the action graph gaps they meet are printed
/// (`refs/reports/Workshop.md` "WorkshopEvents", "DpkScripts"). 7E6PK and
/// A3A4G name "Hanamura Winter" and "King's Row Winter", as the importer
/// matches the maps "Hanamura (Winter)" and "King's Row (Winter)".
#[test]
#[ignore = "slow: requires an Overwatch 1.74 install and the DPK archive (just test-dpk <install> <archive>)"]
fn slow_dpk_archive_scripts_run_in_a_hosted_match() {
    let archive = orpheus_testkit::dpk_archive();
    let scripts = [
        "workshop.codes/C32S2_OG_doom_parkur_hanamura.txt",
        "workshop.codes/7E6PK_Doomfist_Parkour_map_generator_Beta_by_nanofanes_2272.txt",
        "workshop.codes/A3A4G_Doom_Park.txt",
        "workshop.codes/QF189_The_ultimate_doomfist_parkour_tutorial_outdated.txt",
        "github/WelkinU_OverwatchWorkshopScripts/DoomfistParkour/DoomParkourIllios.txt",
        "github/WelkinU_OverwatchWorkshopScripts/DoomfistParkour/DoomfistParkourBusanStadium.txt",
        "github/WelkinU_OverwatchWorkshopScripts/DoomfistParkour/DoomfistRolloutsKingsRow_1_0_0.txt",
        "github/WelkinU_OverwatchWorkshopScripts/DoomfistParkour/MultiHeroParkourRialto.txt",
        "github/WelkinU_OverwatchWorkshopScripts/utils/DestroyMapRailings.txt",
        "github/WelkinU_OverwatchWorkshopScripts/utils/FastMsDestructo.txt",
        "github/WelkinU_OverwatchWorkshopScripts/utils/SkyboxDetector.txt",
    ];
    let mut failed = Vec::new();
    for script in scripts {
        match survey_script(&archive.join(script)) {
            Ok(line) => eprintln!("{script}: {line}"),
            Err(line) => {
                eprintln!("{script}: {line}");
                failed.push(script);
            }
        }
    }
    assert_eq!(failed, [] as [&str; 0]);
}
