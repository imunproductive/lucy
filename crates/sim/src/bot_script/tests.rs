use std::{borrow::Borrow, time::Duration};

use super::*;
use crate::{
    bot_plate::{BotPlate, PlateChange},
    bots::BOT_ENTITY,
    combat::BotReplacement,
    egress::{DeliveryReport, PacketDelivery, RecordSource},
    replication::WorldDelivery,
    scene::{PlayerAppearance, PlayerCard, bot_create},
    scene_assets::{PreparedScene, SceneAssets},
};
use orpheus_core::tick::FIRST_TICK;
use orpheus_script::bot::{DEAD_FLAG, NAMEPLATE_ENABLED, NAMEPLATE_TEXT};
use orpheus_wire::entity::{ComponentValue, EntityUpdate, Operation};

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
        .filter(|update| {
            update.chunk.is_some()
                && (crate::bots::BOT_CONTROLLER_ENTITY..0xa000_0000).contains(&update.entity)
        })
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
/// nameplate alone, with the bot's name as its text, enabled for the bots
/// on the player's team; a surviving hit's enabled nameplate; a death's
/// frames with the dead flag; and the respawned bot's new script.
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
    world.update_bots(&scene, &[], tick);
    let entities: BTreeSet<u32> = scene.bots().iter().map(|bot| bot.entity).collect();
    // The first frames wait for the creates.
    let first = deliver(&mut world);
    assert!(script_chunks(&first).is_empty());
    assert!(first.iter().any(|update| update.entity == BOT_ENTITY));
    let full = script_chunks(&deliver(&mut world));
    assert_eq!(full.iter().copied().collect::<BTreeSet<_>>(), entities);
    assert_eq!(full.len(), entities.len());
    // Whether the client's acknowledged frame has the bot's nameplate; one
    // that has it shows the bot's hero name.
    let nameplate = |scene: &PreparedScene, world: &WorldDelivery, entity: u32| {
        let child = scene
            .bot_scripts()
            .latest(entity)
            .and_then(|latest| latest.plain.as_ref())
            .unwrap()
            .child;
        let base = &world
            .bot_stream(entity)
            .and_then(|stream| stream.base())
            .unwrap()
            .1;
        base.instance(child)?;
        let bot = scene.bots().iter().find(|bot| bot.entity == entity)?;
        assert!(!bot.spawn.name.is_empty());
        assert_eq!(
            base.instance_variable(child, NAMEPLATE_TEXT),
            Some(&Value::String(bot.spawn.name.clone())),
            "{entity:08X}'s nameplate text"
        );
        Some(())
    };
    assert!(
        entities
            .iter()
            .all(|&entity| nameplate(&scene, &world, entity).is_none())
    );

    // The mode UI arrives: each bot's nameplate follows alone.
    tick += 1;
    world.present(&scene, tick).unwrap();
    deliver(&mut world);
    assert!(world.mode_ready());
    world.update_bots(&scene, &[], tick);
    let nameplates = script_chunks(&deliver(&mut world));
    assert_eq!(nameplates.len(), entities.len());
    assert!(
        entities
            .iter()
            .all(|&entity| nameplate(&scene, &world, entity).is_some())
    );

    // The player's team's bots, which no player can hit, have it enabled
    // from the start; a hit an enemy bot that doesn't fire survives enables
    // its nameplate: a live frame with #8807 true, once (upstream
    // `show_damage_nameplate`).
    let enabled = |scene: &PreparedScene, world: &WorldDelivery, entity: u32| {
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
            .instance_variable(child, NAMEPLATE_ENABLED)
            == Some(&Value::Bool(true))
    };
    let allies: BTreeSet<u32> = scene
        .bots()
        .iter()
        .filter(|bot| bot.spawn.team == viewer.team)
        .map(|bot| bot.entity)
        .collect();
    assert_eq!(allies.len(), 4);
    let hit = scene.bots()[1..]
        .iter()
        .find(|bot| !bot.spawn.firing && bot.spawn.team != viewer.team)
        .unwrap()
        .entity;
    assert!(
        entities
            .iter()
            .all(|&entity| enabled(&scene, &world, entity) == allies.contains(&entity))
    );
    tick += 1;
    scene.bot_health(hit, 150.0);
    let events = scene.take_bot_events();
    assert!(matches!(
        events.as_slice(),
        [BotEvent::Live { entity, .. }] if *entity == hit
    ));
    world.update_bots(&scene, &events, tick);
    assert_eq!(script_chunks(&deliver(&mut world)), [hit]);
    assert!(enabled(&scene, &world, hit));
    assert!(nameplate(&scene, &world, hit).is_some());
    assert!(
        entities
            .iter()
            .filter(|&&entity| entity != hit)
            .all(|&entity| enabled(&scene, &world, entity) == allies.contains(&entity))
    );
    scene.bot_health(hit, 100.0);
    assert!(scene.take_bot_events().is_empty());

    // A death: the script's death message, then its timers. The victim, an
    // enemy bot that doesn't fire, survives a hit first (its nameplate's
    // live frame) and dies before the client acknowledged that frame. The
    // death's frame goes out at once all the same: with the dead flag up,
    // 0D24 hides the health bar (`#17549` reads `!#478e`).
    let slot = scene
        .bots()
        .iter()
        .position(|bot| !bot.spawn.firing && bot.spawn.team != viewer.team && bot.entity != hit)
        .unwrap();
    let victim = scene.bots()[slot].entity;
    tick += 1;
    scene.bot_health(victim, 150.0);
    let events = scene.take_bot_events();
    assert!(matches!(
        events.as_slice(),
        [BotEvent::Live { entity, .. }] if *entity == victim
    ));
    world.update_bots(&scene, &events, tick);
    tick += 1;
    scene.bots_mut()[slot].died();
    scene.bot_died(&BotDeath {
        bot: victim,
        killer: OWNER | 1,
        critical: false,
        loadout: None,
        tags: orpheus_script::weapons::DamageTags::NONE,
        tick,
    });
    let events = scene.take_bot_events();
    assert!(matches!(
        events.as_slice(),
        [BotEvent::Death { entity, frame: Some(_) }] if *entity == victim
    ));
    world.update_bots(&scene, &events, tick);
    assert!(world.bot_frames_pending(victim));
    assert_eq!(script_chunks(&deliver(&mut world)), [victim, victim]);
    assert!(!world.bot_frames_pending(victim));
    // Its acknowledged frame has the dead flag up.
    let dead = |world: &WorldDelivery| {
        world
            .bot_stream(victim)
            .and_then(|stream| stream.base())
            .unwrap()
            .1
            .entity_variable(DEAD_FLAG)
            .cloned()
    };
    assert_eq!(dead(&world), Some(Value::Bool(true)));
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
        world.update_bots(&scene, &events, tick);
        assert!(
            script_chunks(&deliver(&mut world))
                .iter()
                .all(|&entity| entity == victim)
        );
    }
    assert!(updates > 0);
    assert_eq!(dead(&world), Some(Value::Bool(true)));

    // A firing bot dies while its rocket's copy still waits for its launch
    // (the launch needs the shot's volley in the client's acknowledged
    // frame). Its death frame goes out at once all the same, with the dead
    // flag and the held volley, and the launch follows.
    let shooter = scene
        .bots()
        .iter()
        .position(|bot| bot.spawn.firing && bot.spawn.team != viewer.team)
        .unwrap();
    let source = scene.bots()[shooter].entity;
    let mut ids = crate::projectiles::SeenIds::new();
    loop {
        tick += 1;
        scene.step_bots(tick);
        let rocket = scene
            .bot_weapons(tick)
            .iter()
            .find(|shot| shot.source == source)
            .and_then(|shot| SeenRocket::of(1, shot));
        if let Some(rocket) = &rocket {
            world.show_bot_rocket(rocket, tick, || ids.next());
        }
        let events = scene.take_bot_events();
        world.update_bots(&scene, &events, tick);
        deliver(&mut world);
        if rocket.is_some() {
            break;
        }
        assert!(tick < FIRST_TICK + 2000, "the firing bot never fired");
    }
    // The launch is the shot record in the bot body's channel 2.
    let launch = |updates: &[EntityUpdate]| {
        updates
            .iter()
            .any(|update| update.entity == source && update.ch2.is_some())
    };
    tick += 1;
    scene.bots_mut()[shooter].died();
    scene.bot_died(&BotDeath {
        bot: source,
        killer: OWNER | 1,
        critical: false,
        loadout: None,
        tags: orpheus_script::weapons::DamageTags::NONE,
        tick,
    });
    let events = scene.take_bot_events();
    world.update_bots(&scene, &events, tick);
    let died = deliver(&mut world);
    assert_eq!(script_chunks(&died), [source]);
    assert!(!launch(&died));
    let dead_flag = |world: &WorldDelivery| {
        world
            .bot_stream(source)
            .and_then(|stream| stream.base())
            .unwrap()
            .1
            .entity_variable(DEAD_FLAG)
            .cloned()
    };
    assert_eq!(dead_flag(&world), Some(Value::Bool(true)));
    // The copy's launch, then the rest of the death.
    world.advance_observed(tick);
    let launched = deliver(&mut world);
    assert!(launch(&launched), "the rocket's launch after its bot died");
    for _ in 0..4 {
        tick += 1;
        world.advance_observed(tick);
        world.update_bots(&scene, &[], tick);
        deliver(&mut world);
    }
    assert_eq!(dead_flag(&world), Some(Value::Bool(true)));

    // The respawn: a new script under the new entity, sent after its create.
    let fresh = scene.bots()[slot].respawned(BOT_ENTITY + 15, tick + 8);
    let created = bot_create(&fresh, scene.bot_plate()).unwrap();
    scene.bots_mut()[slot] = fresh;
    scene.prepare_respawned_bots(tick);
    assert!(scene.bot_scripts().latest(BOT_ENTITY + 15).is_some());
    assert!(scene.bot_scripts().latest(victim).is_none());
    world.replace_bot(BotReplacement {
        retired: victim,
        created,
        link: None,
    });
    assert!(world.bot_stream(victim).is_none());
    world.update_bots(&scene, &[], tick);
    assert!(script_chunks(&deliver(&mut world)).is_empty());
    assert_eq!(script_chunks(&deliver(&mut world)), [BOT_ENTITY + 15]);
    assert!(nameplate(&scene, &world, BOT_ENTITY + 15).is_some());
    // The replacement starts alive: no dead flag.
    assert_eq!(
        world
            .bot_stream(BOT_ENTITY + 15)
            .and_then(|stream| stream.base())
            .unwrap()
            .1
            .entity_variable(DEAD_FLAG),
        None
    );

    // The live nameplate tool: a change reaches every live bot at once,
    // the pools as one component 51 update each, the variables in one live
    // frame each; nothing respawns.
    let live: BTreeSet<u32> = scene
        .bots()
        .iter()
        .filter(|bot| !bot.dead)
        .map(|bot| bot.entity)
        .collect();
    assert_eq!(live.len(), entities.len() - 1);
    let tail = |update: &EntityUpdate| match &update.operation {
        Some(Operation::Update(components)) => match components.as_slice() {
            [pools] if pools.index == 51 => match &pools.fields[0] {
                Some(ComponentValue::Blob20(part)) => {
                    Some(<[u8; 4]>::try_from(&part[16..]).unwrap())
                }
                _ => None,
            },
            _ => None,
        },
        Some(Operation::Create(create)) => match &create.components[0].fields[0] {
            Some(ComponentValue::Blob20(part)) => Some(<[u8; 4]>::try_from(&part[16..]).unwrap()),
            _ => None,
        },
        _ => None,
    };
    let plated = |scene: &mut PreparedScene, command: &str| {
        let mut plate = BotPlate::clone(scene.bot_plate());
        plate.change(PlateChange::parse(command).unwrap()).unwrap();
        scene.set_bot_plate(std::sync::Arc::new(plate), |_| None)
    };
    let variable = |world: &WorldDelivery, entity: u32, id: u32| {
        world
            .bot_stream(entity)
            .and_then(|stream| stream.base())
            .unwrap()
            .1
            .entity_variable(id)
            .cloned()
    };
    let text = |scene: &PreparedScene, world: &WorldDelivery, entity: u32| {
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
            .instance_variable(child, NAMEPLATE_TEXT)
            .cloned()
    };
    tick += 1;
    let pools = plated(&mut scene, "meter 1; name null; health.b19 0");
    assert_eq!(
        pools
            .iter()
            .map(|update| update.entity)
            .collect::<BTreeSet<_>>(),
        live
    );
    assert_eq!(pools.len(), live.len());
    assert!(
        pools
            .iter()
            .all(|update| tail(update) == Some([0, 1, 0, 0]))
    );
    let events = scene.take_bot_events();
    assert_eq!(events.len(), live.len());
    assert!(
        events
            .iter()
            .all(|event| matches!(event, BotEvent::Live { entity, .. } if live.contains(entity)))
    );
    world.update_bots(&scene, &events, tick);
    let changed = script_chunks(&deliver(&mut world));
    assert_eq!(changed.iter().copied().collect::<BTreeSet<_>>(), live);
    for &entity in &live {
        assert_eq!(
            variable(&world, entity, 6036),
            Some(Value::Integer(1)),
            "{entity:08X}"
        );
        // A projection leaves a null out; the frame's delta sent it as one,
        // since the text was there before.
        assert_eq!(text(&scene, &world, entity), None, "{entity:08X}");
    }
    // What the tool shows: the first live bot as it sends it now.
    let sample = scene.bot_plate_sample(|_| None).unwrap();
    assert!(live.contains(&sample.entity));
    assert_eq!(sample.parts[0].bytes, [0, 1, 0, 0]);
    assert!(sample.parts[0].tuned);
    assert!(sample.variables.contains(&(
        crate::bot_plate::PlateVariable::Nameplate(NAMEPLATE_TEXT),
        Some(Value::Null),
        true
    )));
    assert!(sample.variables.contains(&(
        crate::bot_plate::PlateVariable::Entity(6036),
        Some(Value::Integer(1)),
        true
    )));

    // A bot that respawns gets the changes from its create on.
    let fresh = scene.bots()[shooter].respawned(BOT_ENTITY + 16, tick + 8);
    let created = bot_create(&fresh, scene.bot_plate()).unwrap();
    assert_eq!(tail(&created), Some([0, 1, 0, 0]));
    scene.bots_mut()[shooter] = fresh;
    scene.prepare_respawned_bots(tick);
    world.replace_bot(BotReplacement {
        retired: source,
        created,
        link: None,
    });
    world.update_bots(&scene, &[], tick);
    deliver(&mut world);
    assert_eq!(script_chunks(&deliver(&mut world)), [BOT_ENTITY + 16]);
    assert_eq!(
        variable(&world, BOT_ENTITY + 16, 6036),
        Some(Value::Integer(1))
    );
    assert_eq!(text(&scene, &world, BOT_ENTITY + 16), None);

    // `reset`: the bots' own pools, meter and name again.
    tick += 1;
    let pools = plated(&mut scene, "reset");
    assert_eq!(pools.len(), live.len() + 1);
    assert!(
        pools
            .iter()
            .all(|update| tail(update) == Some([0, 1, 0, 1]))
    );
    let events = scene.take_bot_events();
    assert_eq!(events.len(), live.len() + 1);
    world.update_bots(&scene, &events, tick);
    deliver(&mut world);
    for entity in live.iter().copied().chain([BOT_ENTITY + 16]) {
        assert_eq!(variable(&world, entity, 6036), None, "{entity:08X}");
        assert!(nameplate(&scene, &world, entity).is_some());
    }
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
        hurt: false,
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
