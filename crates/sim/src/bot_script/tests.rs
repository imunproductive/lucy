use std::{borrow::Borrow, time::Duration};

use super::*;
use crate::{
    bots::BOT_ENTITY,
    combat::BotReplacement,
    egress::{DeliveryReport, PacketDelivery, RecordSource},
    replication::WorldDelivery,
    scene::{PlayerAppearance, PlayerCard, bot_create},
    scene_assets::{PreparedScene, SceneAssets},
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_wire::entity::EntityUpdate;

const OWNER: u32 = 0xa000_0100;

/// Send everything the client may get now and let all of it arrive.
fn deliver(world: &mut WorldDelivery) -> Vec<EntityUpdate> {
    let ready = world.ready();
    let updates: Vec<EntityUpdate> = ready
        .iter()
        .map(|item| Borrow::<EntityUpdate>::borrow(item).clone())
        .collect();
    let records: Vec<_> = ready.iter().map(|item| item.tracked()).collect();
    let sources: Vec<_> = (0..ready.len()).map(RecordSource::Queued).collect();
    let indexes = WorldDelivery::sent_indices(&ready, &sources);
    drop(ready);
    world.sent(&indexes);
    world.acknowledge(
        DeliveryReport {
            requeued_reliable: 0,
            packets: vec![PacketDelivery {
                sequence: 0,
                arrived: true,
                records,
            }],
        },
        Duration::ZERO,
    );
    updates
}

fn script_chunks(updates: &[EntityUpdate]) -> Vec<u32> {
    updates
        .iter()
        .filter(|update| update.chunk.is_some() && update.entity & 0xf000_0000 == 0xc000_0000)
        .map(|update| update.entity)
        .collect()
}

fn practice_scene() -> PreparedScene {
    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    // The stock bot graphs hold content the VM doesn't know yet; the Python
    // reference ran them anyway.
    assets.set_bot_unknowns(Unknowns::Tolerate);
    assets.prepare(GameMap::practice_range()).unwrap()
}

/// match.py: the bots' scripts start one per update; the world waits for
/// them. Every bot runs its body script, the three firing bots their
/// firing behavior, which shoots.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn practice_bots_start_one_script_per_update_and_the_firing_bots_shoot() {
    let mut scene = practice_scene();
    let count = scene.bots().len();
    assert_eq!(scene.bot_scripts().pending(), count);
    let mut tick = FIRST_TICK;
    for prepared in 1..=count {
        assert!(!scene.bot_scripts_ready());
        scene.step_bots(tick);
        assert_eq!(scene.bot_scripts().pending(), count - prepared);
        tick += 1;
    }
    assert!(scene.bot_scripts_ready());
    assert_eq!(scene.bot_scripts().running(), count);
    let scripts = scene.bot_scripts();
    for bot in scene.bots() {
        let latest = scripts.latest(bot.entity).unwrap();
        assert!(
            latest.plain.is_some(),
            "{:08X} has no practice nameplate",
            bot.entity
        );
    }

    let firing: BTreeSet<u32> = scene
        .bots()
        .iter()
        .filter(|bot| bot.spawn.firing)
        .map(|bot| bot.entity)
        .collect();
    assert_eq!(firing.len(), 3);
    let mut shooters = BTreeSet::new();
    let mut live = BTreeSet::new();
    for _ in 0..600 {
        scene.step_bots(tick);
        for shot in scene.bot_weapons(tick) {
            assert_eq!(shot.tick, tick);
            let BotShotKind::Projectile(projectile) = shot.kind else {
                panic!("the Practice Range bots fire projectiles");
            };
            assert_eq!(projectile.ballistics.speed, 35.0);
            assert!(
                projectile
                    .ballistics
                    .direct
                    .is_some_and(|hit| hit.amount > 0.0)
            );
            shooters.insert(shot.source);
        }
        for event in scene.take_bot_events() {
            let BotEvent::Live { entity, .. } = event else {
                panic!("no bot died");
            };
            live.insert(entity);
        }
        tick += 1;
    }
    assert_eq!(shooters, firing);
    assert!(live.is_subset(&firing) && !live.is_empty());
}

/// server.py: each client gets every bot's first frame once its create
/// arrived, without the nameplate until the mode UI has arrived, then the
/// nameplate alone; a death's frames; and the respawned bot's new script.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_client_gets_the_bots_frames_nameplates_death_and_respawn() {
    let mut scene = practice_scene();
    let mut tick = FIRST_TICK;
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    let card = PlayerCard::default();
    let viewer = PlayerAppearance {
        entity: OWNER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card: &card,
    };
    let mut world = WorldDelivery::new(OWNER);
    world.start(&scene, &viewer, tick).unwrap();
    world.update_bots(&scene, &[], tick, true);
    let entities: BTreeSet<u32> = scene.bots().iter().map(|bot| bot.entity).collect();
    // The first frames wait for the creates.
    let first = deliver(&mut world);
    assert!(script_chunks(&first).is_empty());
    assert!(first.iter().any(|update| update.entity == BOT_ENTITY));
    let full = script_chunks(&deliver(&mut world));
    assert_eq!(full.iter().copied().collect::<BTreeSet<_>>(), entities);
    assert_eq!(full.len(), entities.len());
    let nameplate = |scene: &PreparedScene, world: &WorldDelivery, entity: u32| {
        let child = scene
            .bot_scripts()
            .latest(entity)
            .and_then(|latest| latest.plain.as_ref())
            .unwrap()
            .child;
        world
            .bot_stream(entity)
            .and_then(|stream| stream.base())
            .unwrap()
            .1
            .instance(child)
            .is_some()
    };
    assert!(
        entities
            .iter()
            .all(|&entity| !nameplate(&scene, &world, entity))
    );

    // The mode UI arrives: each bot's nameplate follows alone.
    tick += 1;
    world.present(&scene, tick).unwrap();
    deliver(&mut world);
    assert!(world.mode_ready());
    world.update_bots(&scene, &[], tick, false);
    assert!(script_chunks(&deliver(&mut world)).is_empty());
    world.update_bots(&scene, &[], tick, true);
    let nameplates = script_chunks(&deliver(&mut world));
    assert_eq!(nameplates.len(), entities.len());
    assert!(
        entities
            .iter()
            .all(|&entity| nameplate(&scene, &world, entity))
    );

    // A death: the script's death message, then its timers.
    let victim = scene.bots()[0].entity;
    tick += 1;
    scene.bots_mut()[0].died(tick);
    scene.bot_died(victim, OWNER | 1, false, None, tick);
    let events = scene.take_bot_events();
    assert!(matches!(
        events.as_slice(),
        [BotEvent::Death { entity, frame: Some(_) }] if *entity == victim
    ));
    world.update_bots(&scene, &events, tick, true);
    assert!(world.bot_frames_pending(victim));
    assert_eq!(script_chunks(&deliver(&mut world)), [victim]);
    assert!(!world.bot_frames_pending(victim));
    let mut updates = 0;
    // The death script runs for its lifetime, 187 frames.
    for _ in 0..200 {
        tick += 1;
        scene.advance_bot_deaths(tick);
        let events = scene.take_bot_events();
        updates += events.len();
        assert!(events.iter().all(
            |event| matches!(event, BotEvent::DeathUpdate { entity, .. } if *entity == victim)
        ));
        world.update_bots(&scene, &events, tick, true);
        assert!(
            script_chunks(&deliver(&mut world))
                .iter()
                .all(|&entity| entity == victim)
        );
    }
    assert!(updates > 0);

    // The respawn: a new script under the new entity, sent after its create.
    let fresh = scene.bots()[0].respawned(BOT_ENTITY + 15, tick + 8);
    let created = bot_create(&fresh, None).unwrap();
    scene.bots_mut()[0] = fresh;
    scene.prepare_respawned_bots(tick);
    assert!(scene.bot_scripts().latest(BOT_ENTITY + 15).is_some());
    assert!(scene.bot_scripts().latest(victim).is_none());
    world.replace_bot(BotReplacement {
        retired: victim,
        created,
    });
    assert!(world.bot_stream(victim).is_none());
    world.update_bots(&scene, &[], tick, true);
    assert!(script_chunks(&deliver(&mut world)).is_empty());
    assert_eq!(script_chunks(&deliver(&mut world)), [BOT_ENTITY + 15]);
    assert!(nameplate(&scene, &world, BOT_ENTITY + 15));
}

/// combat.py `_bot_weapons` and `_fly`: a firing bot's rockets leave its eye
/// along its yaw and hit the enemy player standing in front of it.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_firing_bots_rocket_hits_the_player_in_front_of_it() {
    use crate::{
        combat::{Combat, PlayerTarget, Shooter, ShotContext},
        hitscan::TargetPose,
        instance::RETAIL_QUANTA,
    };

    let mut scene = practice_scene();
    let mut tick = FIRST_TICK;
    let [x, y, z] = scene.map().spawn_point(0, 0).unwrap().0;
    let shooter = scene
        .bots_mut()
        .iter_mut()
        .find(|bot| bot.spawn.firing && bot.spawn.team != 0)
        .unwrap();
    // 4 m to the side of the player's spawn, facing it (a quarter turn: +x),
    // where the other bots leave the line clear.
    shooter.position = [x - 4.0, y, z];
    shooter.yaw = 16384.0;
    let (source, team) = (shooter.entity, shooter.spawn.team);
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    let body = OWNER | 1;
    let player = PlayerTarget {
        body,
        team: 0,
        pose: TargetPose {
            feet: [x, y, z],
            height: 1.8,
            radius: 0.35,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        },
        dead: false,
    };
    let mut combat = Combat::new(&scene);
    let mut hits = Vec::new();
    for _ in 0..600 {
        scene.step_bots(tick);
        combat.record_poses(&scene, std::slice::from_ref(&player), tick);
        let context = ShotContext {
            quanta: RETAIL_QUANTA,
            tick,
            now: Duration::from_millis(u64::from(tick) * 16),
        };
        combat.step_bot_projectiles(&mut scene, context).unwrap();
        for shot in scene.bot_weapons(tick) {
            let BotShotKind::Projectile(projectile) = shot.kind else {
                panic!("the Practice Range bots fire projectiles");
            };
            if shot.source == source {
                assert_eq!(projectile.origin, [x - 4.0, y + 1.55, z]);
                assert!(projectile.direction[0] > 0.999);
            }
            let shooter = Shooter {
                source: shot.source,
                team: shot.team,
                bot: true,
            };
            combat.fire_bot_projectile(shooter, projectile, shot.tick);
        }
        hits.extend(combat.take_player_hits());
        if !hits.is_empty() {
            break;
        }
        tick += 1;
    }
    let hit = hits.first().expect("a rocket reached the player");
    assert_eq!(hit.body, body);
    assert_eq!(hit.shooter.source, source);
    assert_eq!(hit.shooter.team, team);
    assert!(hit.amount > 0.0);
}
