//! A hero ability's victim graph, played by the server on the body the
//! ability struck (`lucy_script` `vm/play.rs` `Plays::Others`): Rocket
//! Punch's `0D07` on a training bot and on an enemy player, Soundwave's
//! `0ECA` on an enemy player, each in the struck body's frames, and the
//! push the struck body takes.

use super::*;
use lucy_script::{
    vm::Unknowns,
    wire::{DeltaInstance, read_delta_instances, read_full_head},
};
use lucy_sim::scene_assets::SceneAssets;
use lucy_wire::commands::SECONDARY;

/// Rocket Punch's victim graph.
const PUNCHED: u16 = 0x0d07;
/// Soundwave's victim graph.
const SOUNDWAVED: u16 = 0x0eca;

/// Whether one of `entity`'s frames `watcher` received lists an instance of
/// `graph` (a whole frame's, or a delta's new ones).
fn frames_list(
    watcher: &Watcher,
    graphs: &lucy_script::graph::GraphStore,
    entity: u32,
    graph: u16,
) -> bool {
    watcher
        .datagrams
        .iter()
        .flat_map(|(_, records)| records)
        .filter(|record| record.entity == entity)
        .filter_map(|record| record.script.as_ref())
        .any(|chunk| {
            if chunk.first == 0 {
                read_full_head(&chunk.data, |graph| {
                    graphs
                        .graph(u32::from(graph))
                        .map(|graph| graph.header.states_bits)
                })
                .is_ok_and(|head| head.instances.iter().any(|instance| instance.1 == graph))
            } else {
                read_delta_instances(&chunk.data).is_ok_and(|instances| {
                    instances.iter().any(|instance| {
                        matches!(instance, DeltaInstance::Described((_, listed, _, _)) if *listed == graph)
                    })
                })
            }
        })
}

/// The assets with the heroes' and the bots' gaps as in a release build
/// (Rocket Punch's camera states run plain, `refs/reports/Doomfist.md` §1;
/// the training bots' `0033` meets `STU_F76C486C`, and a refused bot has
/// no script to play on).
fn assets() -> SceneAssets {
    let mut assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    assets.set_bot_unknowns(Unknowns::Tolerate);
    assets
}

/// The striker (`hero`) and an enemy Soldier: 76 in the two-team Practice
/// Range, pushed apart at its one spawn: the striker aims at the other and
/// holds `buttons` for `hold`, then both idle for 1.5 s. Returns how far
/// the struck body moved and whether its own client's frames listed
/// `victim`.
fn strike_player(hero: &str, buttons: u32, hold: Duration, victim: u16) -> (f64, bool) {
    let assets = assets();
    let (mut server, schemas, [mut striker, mut struck], _) = practice_world(
        [("striker#1111", 0, hero), ("struck#2222", 1, "Soldier: 76")],
        &assets,
        |_| {},
    );
    let positions = |server: &Lockstep| {
        let bodies = server.bodies(MATCH).unwrap();
        [0, 1].map(|slot| bodies[slot].position.unwrap())
    };
    let (mut striking, mut idle) = (striker.tick + 1, struck.tick + 1);
    let mut step = |server: &Lockstep, striker: &mut Watcher, struck: &mut Watcher, buttons| {
        let [from, to] = positions(server);
        let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
        let yaw = (dx.atan2(dz) * 32768.0 / std::f64::consts::PI).round() as i32 as i16;
        send_input(striker, &mut striking, yaw, 0, buttons, false);
        send_input(struck, &mut idle, 0, 0, 0, false);
        striker.pump(&schemas);
        struck.pump(&schemas);
    };
    let settled = server.now() + Duration::from_millis(500);
    while server.now() < settled {
        step(&server, &mut striker, &mut struck, 0);
    }
    let before = positions(&server)[1];
    let released = server.now() + hold;
    while server.now() < released {
        step(&server, &mut striker, &mut struck, buttons);
    }
    let after = server.now() + Duration::from_millis(1500);
    while server.now() < after {
        step(&server, &mut striker, &mut struck, 0);
    }
    let moved = positions(&server)[1];
    let distance = (0..3)
        .map(|axis| (moved[axis] - before[axis]).powi(2))
        .sum::<f64>()
        .sqrt();
    // His own client's frames of his body, or the striker's.
    let body = struck.owner | 1;
    let listed = [&struck, &striker]
        .into_iter()
        .any(|watcher| frames_list(watcher, assets.graphs(), body, victim));
    server.stop().unwrap();
    (distance, listed)
}

/// Doomfist's tapped Rocket Punch (Secondary Fire held 0.3 s) on the enemy
/// beside him: his `0D06` finds him (the cylinder search, `vm/targets.rs`)
/// and plays `0D07` on his body, whose velocity action throws him metres
/// away; his own client gets the instance in his body's frames.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn doomfists_punch_plays_its_victim_graph_on_the_enemy_and_throws_him() {
    let (distance, listed) =
        strike_player("Doomfist", SECONDARY, Duration::from_millis(300), PUNCHED);
    assert!(listed, "no 0D07 in the punched body's frames");
    assert!(distance > 3.0, "thrown {distance} m");
}

/// Lúcio's Soundwave (Secondary Fire) on the enemy beside him: its
/// ModifyHealth's push vector (`0776` rid28, 17 m/s along his aim) throws
/// him, and `0776` plays `0ECA` on his body.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn lucios_soundwave_plays_its_victim_graph_on_the_enemy_and_pushes_him() {
    let (distance, listed) =
        strike_player("Lúcio", SECONDARY, Duration::from_millis(50), SOUNDWAVED);
    assert!(listed, "no 0ECA in the struck body's frames");
    assert!(distance > 1.0, "pushed {distance} m");
}

/// Doomfist's tapped Rocket Punch on an enemy Training Bot 2 m ahead: `0D07`
/// plays on the bot's body script, and his client sees it in the bot's
/// frames.
#[test]
#[ignore = "requires a prepared 104319 Practice Range scene"]
fn doomfists_punch_plays_its_victim_graph_on_a_training_bot() {
    const AHEAD: f64 = 2.0;
    let assets = assets();
    let mut aim = None;
    let (mut server, schemas, [mut doomfist], _) =
        practice_world([("doomfist#1111", 0, "Doomfist")], &assets, |scene| {
            let ([x, y, z], yaw) = scene.map().spawn_point(0, 0).unwrap();
            let (sin, cos) = yaw.to_radians().sin_cos();
            let bot = scene
                .bots_mut()
                .iter_mut()
                .find(|bot| bot.spawn.team == 1 && !bot.spawn.firing && bot.spawn.route.is_empty())
                .unwrap();
            bot.position = [x + AHEAD * sin, y, z + AHEAD * cos];
            aim = Some((bot.entity, (yaw * 65536.0 / 360.0).round() as i32 as i16));
        });
    let (bot, yaw) = aim.unwrap();
    let mut next = doomfist.tick + 1;
    let mut run = |doomfist: &mut Watcher, buttons: u32, time: Duration| {
        let until = server.now() + time;
        while server.now() < until {
            send_input(doomfist, &mut next, yaw, 0, buttons, false);
            doomfist.pump(&schemas);
        }
    };
    run(&mut doomfist, 0, Duration::from_secs(1));
    run(&mut doomfist, SECONDARY, Duration::from_millis(300));
    run(&mut doomfist, 0, Duration::from_millis(1500));
    let bot_graphs = assets.bot_graphs().expect("the bot bundle");
    assert!(
        frames_list(&doomfist, bot_graphs, bot, PUNCHED),
        "no 0D07 in the bot's frames"
    );
    server.stop().unwrap();
}
