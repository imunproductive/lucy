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
use orpheus_script::bot::{DEAD_FLAG, NAMEPLATE_TEXT};
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
/// nameplate alone, with the bot's name as its text; a surviving hit's
/// pools without a frame, its flinch's live frame; a death's frames with
/// the dead flag; and the respawned bot's new script.
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
    world
        .present(
            &scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            tick,
        )
        .unwrap();
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

    // A hit an enemy bot survives changes its pools alone: no live frame
    // (the client shows the plate itself, behavior.md "When a bot's plate
    // shows").
    let hit = scene.bots()[1..]
        .iter()
        .find(|bot| !bot.spawn.firing && bot.spawn.team != viewer.team)
        .unwrap()
        .entity;
    tick += 1;
    scene.bot_health(hit, 150.0);
    assert!(scene.take_bot_events().is_empty());

    // A death: the script's death message, then its timers. The victim, an
    // enemy bot that doesn't fire, survives a rifle hit first (its flinch's
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
    let rifle: orpheus_script::weapons::DamageTags = [0x03bf, 0x09a2, 0x1e1e, 0x2d2f, 0x4d29]
        .map(|tag| 0x0d80_0000_0000_0000 | tag)
        .into_iter()
        .collect();
    scene.bot_hurt(
        victim,
        &crate::combat::BotDamage {
            source: OWNER | 1,
            dealt: 50.0,
            tags: rifle,
            tick,
        },
    );
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
        critical: true,
        loadout: None,
        tags: orpheus_script::weapons::DamageTags::NONE,
        dealt: 100.0,
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
    // Its kill feed graph `0C18` keeps the death's crit (`0095` → #1821,
    // synced, rid10 sends it as the line's headshot marker `03EF`).
    let crit = |world: &WorldDelivery, entity: u32| {
        let base = &world
            .bot_stream(entity)
            .and_then(|stream| stream.base())
            .unwrap()
            .1;
        let feed = base
            .instance_graphs()
            .find(|&(_, graph)| graph == 0x0c18)
            .map(|(instance, _)| instance)
            .expect("the bot runs 0C18");
        base.instance_variable(feed, 1821).cloned()
    };
    assert_eq!(crit(&world, victim), Some(Value::Bool(true)));
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
        dealt: 200.0,
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
    // A death without a critical says so too: the client's line takes its
    // marker from the variable, not from the last line's.
    assert_eq!(crit(&world, source), Some(Value::Bool(false)));
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

/// A bot's killing hit reaches its script as a player's does
/// ([`BotScripts::died`], `combat::hit_messages`): `0011.025` with the
/// kill's fields first, then `003E.025`. Only the damage message reaches
/// `0E8C` (the rifle's `09A2` flinch strength #1024, 0.2) and `0C17`'s sum
/// of the last second's damage (#337); a message after the death would
/// reach neither (a dead body takes none). The death goes out as one frame.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_bots_killing_hit_reaches_its_script_before_its_death() {
    const FLINCH: u32 = 0x0e8c;
    const PAIN: u32 = 0x0c17;
    let mut scene = practice_scene();
    let mut tick = FIRST_TICK;
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    scene.take_bot_events();
    let slot = scene
        .bots()
        .iter()
        .position(|bot| !bot.spawn.firing)
        .unwrap();
    let victim = scene.bots()[slot].entity;
    let rifle: orpheus_script::weapons::DamageTags = [0x03bf, 0x09a2, 0x1e1e, 0x2d2f, 0x4d29]
        .map(|tag| 0x0d80_0000_0000_0000 | tag)
        .into_iter()
        .collect();
    tick += 1;
    scene.bots_mut()[slot].died();
    scene.bot_died(&BotDeath {
        bot: victim,
        killer: OWNER | 1,
        critical: false,
        loadout: None,
        tags: rifle,
        dealt: 200.0,
        tick,
    });
    assert!(matches!(
        scene.take_bot_events().as_slice(),
        [BotEvent::Death { entity, frame: Some(_) }] if *entity == victim
    ));
    let scripts = scene.bot_scripts();
    let body = &scripts.slots[scripts.slot(victim).unwrap()]
        .as_ref()
        .unwrap()
        .body;
    let snapshot = body.snapshot().unwrap();
    let variable = |graph: u32, id: u32| {
        snapshot
            .instances
            .values()
            .find(|instance| instance.graph == graph)
            .and_then(|instance| instance.variables.get(&id))
    };
    assert_eq!(
        variable(FLINCH, 1024),
        Some(&Value::Float(f64::from(0.2_f32)))
    );
    assert_eq!(variable(PAIN, 337), Some(&Value::Float(200.0)));
    assert_eq!(body.entity_variable(DEAD_FLAG), Some(&Value::Bool(true)));
}

/// combat.py `_bot_weapons` and `_fly`: a firing bot's rockets leave its eye
/// along its yaw and hit the enemy player standing in front of it. A rocket
/// (`0BA3` node 178) is one direct hit, no explosion: its amount is
/// `AI-possessed ? param 1961 (fallback 8) : 19`, and the bot is AI-possessed
/// with no difficulty profile, so 8 (4 m is short of its 25 m falloff).
/// Flown level from the eye it meets the player's head, but bots never
/// headshot (rule 123): 8, not 16.
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
        health: None,
        hardpoints: None,
        tags: None,
        ai: None,
        dealt: Vec::new(),
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
    assert_eq!(hits.len(), 1, "one direct hit, no splash");
    assert_eq!(hit.amount, 8.0);
    assert!(!hit.critical);
}

/// Nano Boost's aura (`053:0269`) on a Practice Range bot runs `0B9F`,
/// whose st18 (`0318` -0.5) and st19 (`0317` +0.5) the bot's world keeps:
/// a hit on it is halved, its own hits deal half again, on a bot and on a
/// player's body (`vm/statuses.rs` in `orpheus_script`).
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn nano_boost_halves_the_damage_a_bot_takes_and_boosts_what_it_deals() {
    use crate::{
        combat::{Combat, PlayerTarget, ShotContext},
        hitscan::TargetPose,
        instance::RETAIL_QUANTA,
    };
    use orpheus_script::{
        vm::{AuraChange, HealthChange, Site},
        weapons::DamageTags,
    };

    const NANO_BOOST: u64 = 0x04a0_0000_0000_0269;
    let mut scene = practice_scene();
    let mut tick = FIRST_TICK;
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    let team = |team| {
        scene
            .bots()
            .iter()
            .find(|bot| bot.spawn.team == team)
            .unwrap()
            .entity
    };
    let (boosted, enemy) = (team(1), team(0));
    scene.change_bot_aura(
        &AuraChange::Apply {
            source: None,
            target: boosted,
            aura: NANO_BOOST,
            instigator: boosted,
            creator: boosted,
            parameters: Vec::new(),
        },
        tick,
    );
    let body = OWNER | 1;
    let player = PlayerTarget {
        body,
        team: 0,
        pose: TargetPose {
            feet: [0.0; 3],
            height: 1.8,
            radius: 0.35,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        },
        dead: false,
        health: None,
        hardpoints: None,
        tags: None,
        ai: None,
        dealt: Vec::new(),
    };
    let mut combat = Combat::new(&scene);
    combat.record_poses(&scene, std::slice::from_ref(&player), tick);
    let context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick,
        now: Duration::from_millis(u64::from(tick) * 16),
    };
    let strike = |combat: &mut Combat, scene: &mut PreparedScene, instigator, target| {
        let change = HealthChange {
            site: Site {
                entity: instigator,
                instance: 1,
                graph: 0x0043,
                node: None,
            },
            instigator,
            source: None,
            target,
            amount: 40.0,
            heal: false,
            tags: DamageTags::NONE,
            loadout: None,
            aura: None,
            knockback: None,
            frame: tick,
        };
        combat
            .modify_health(scene, &change, context, |_| true)
            .and_then(|hit| hit.damage)
            .map(|damage| damage.dealt)
    };
    assert_eq!(strike(&mut combat, &mut scene, enemy, boosted), Some(20.0));
    assert_eq!(strike(&mut combat, &mut scene, boosted, enemy), Some(60.0));
    assert_eq!(strike(&mut combat, &mut scene, boosted, body), None);
    let [hit] = combat.take_player_hits().try_into().unwrap();
    assert_eq!((hit.body, hit.amount), (body, 60.0));
}

/// Biotic Grenade's aura (`053:0267`) on an enemy bot plays `0C72`, whose
/// st4 grants the bot tag `02FB`: the bot's world keeps it as a player's
/// body does, and a client that has the bot gets it as the bot's component
/// 76 (added `0x2FB`, nothing suppressed). A client that starts later finds
/// it in the bot's create and gets no update for it.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_bots_target_tags_go_out_as_its_component_76() {
    use orpheus_script::vm::{AuraChange, TargetCandidate};
    use orpheus_wire::entity::Component;

    const BIOTIC_GRENADE: u64 = 0x04a0_0000_0000_0267;
    const ANA: u32 = OWNER | 1;
    let mut scene = practice_scene();
    let mut tick = FIRST_TICK;
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    let card = PlayerCard::default();
    let viewer = |entity| PlayerAppearance {
        entity,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card: &card,
    };
    let mut world = WorldDelivery::new(OWNER);
    world.start(&scene, &viewer(OWNER), tick).unwrap();
    world.update_bots(&scene, &[], tick);
    let tags_of = |update: &EntityUpdate| -> Option<Component> {
        let components = match update.operation.as_ref()? {
            Operation::Create(definition) => &definition.components,
            Operation::Update(components) => components,
            _ => return None,
        };
        components
            .iter()
            .find(|component| component.index == 76)
            .cloned()
    };
    assert!(
        deliver(&mut world)
            .iter()
            .all(|update| tags_of(update).is_none())
    );

    let bot = scene.bots().iter().find(|bot| bot.spawn.team == 1).unwrap();
    let target = bot.entity;
    let body = |entity, team, feet| TargetCandidate {
        entity,
        team: Some(team),
        dead: false,
        controller: Some(entity & !0xff),
        player: Some(entity & !0xff),
        ai: false,
        ai_control: None,
        feet,
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        hardpoints: None,
        spawned: Vec::new(),
    };
    let candidates = [body(target, 1, bot.position), body(ANA, 0, [0.0; 3])];
    scene.set_bot_target_candidates(&candidates);
    tick += 1;
    scene.change_bot_aura(
        &AuraChange::Apply {
            source: None,
            target,
            aura: BIOTIC_GRENADE,
            instigator: ANA,
            creator: ANA,
            parameters: Vec::new(),
        },
        tick,
    );
    scene.advance_live_bots(tick);
    let expected = Component {
        index: 76,
        fields: vec![
            Some(ComponentValue::U32Array(vec![0x2fb])),
            Some(ComponentValue::U32Array(Vec::new())),
        ],
    };
    let events = scene.take_bot_events();
    world.update_bots(&scene, &events, tick);
    let tagged: Vec<_> = deliver(&mut world)
        .iter()
        .filter_map(|update| Some((update.entity, tags_of(update)?)))
        .collect();
    assert_eq!(tagged, [(target, expected.clone())]);
    world.update_bots(&scene, &[], tick);
    assert!(
        deliver(&mut world)
            .iter()
            .all(|update| tags_of(update).is_none())
    );

    let late = OWNER + 0x100;
    let mut world = WorldDelivery::new(late);
    world.start(&scene, &viewer(late), tick).unwrap();
    world.update_bots(&scene, &[], tick);
    let tagged: Vec<_> = deliver(&mut world)
        .iter()
        .filter_map(|update| {
            matches!(update.operation, Some(Operation::Create(_)))
                .then(|| Some((update.entity, tags_of(update)?)))
                .flatten()
        })
        .collect();
    assert_eq!(tagged, [(target, expected)]);
    world.update_bots(&scene, &[], tick);
    assert!(
        deliver(&mut world)
            .iter()
            .all(|update| tags_of(update).is_none())
    );
}

/// Moira's damaging Biotic Orb puts `02D5.053` (`002A` = 50) on an enemy
/// bot, the orb its instigator; the bot runs the aura's graph `1101`
/// (from its own bundle), whose ModifyHealth hurts it by `#499` = 50 a
/// second, dealt by Moira (`#42` = `STU_2B2E2F6D(#40, 0)`, the orb's
/// shooter, which the bot's world reads from her listed orb) with the orb
/// as its source (`m_7B93233C` = `#40`). The bot takes an aura of tick T
/// at the end of its frame T − 1 (`BotBody::change_aura`), and the rate
/// applies each frame its share of the time since (`vm/health.rs`); a bot
/// runs every frame while the rate does (`BotBody::due_by`). So by tick
/// T + 125 it has dealt 126 frames of 16 ms at 50 a second, 100.8, and
/// once the orb takes its aura off, nothing more.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn the_damaging_orbs_aura_hurts_a_bot_for_its_thrower() {
    use orpheus_script::{
        vm::{AuraChange, AuraSource, OwnedEntity, TargetCandidate},
        wire::Value,
    };

    const DAMAGE_ORB_AURA: u64 = 0x04a0_0000_0000_02d5;
    const MOIRA: u32 = OWNER | 1;
    const ORB: u32 = 0xa00f_0000;
    let mut scene = practice_scene();
    let mut tick = FIRST_TICK;
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    let bot = scene.bots().iter().find(|bot| bot.spawn.team == 1).unwrap();
    let target = bot.entity;
    let body = |entity, team, feet| TargetCandidate {
        entity,
        team: Some(team),
        dead: false,
        controller: Some(entity & !0xff),
        player: Some(entity & !0xff),
        ai: false,
        ai_control: None,
        feet,
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        hardpoints: None,
        spawned: Vec::new(),
    };
    let mut moira = body(MOIRA, 0, [0.0; 3]);
    moira.spawned = vec![OwnedEntity {
        entity: ORB,
        projectile: true,
    }];
    scene.set_bot_target_candidates(&[body(target, 1, bot.position), moira]);
    let source = AuraSource {
        entity: ORB,
        instance: 1,
        state: 1,
        activation: 1,
    };
    tick += 1;
    scene.change_bot_aura(
        &AuraChange::Apply {
            source: Some(source),
            target,
            aura: DAMAGE_ORB_AURA,
            instigator: ORB,
            creator: ORB,
            parameters: vec![(0x0a40_0000_0000_002a, Value::Integer(50))],
        },
        tick,
    );
    assert!(scene.take_bot_health_changes().is_empty());
    let mut dealt = 0.0;
    for _ in 0..125 {
        tick += 1;
        scene.advance_live_bots(tick);
        let changes = scene.take_bot_health_changes();
        assert!(!changes.is_empty(), "a share every tick");
        for change in changes {
            assert_eq!(
                (change.target, change.instigator, change.source, change.heal),
                (target, MOIRA, Some(ORB), false),
                "{change:?}"
            );
            dealt += change.amount;
        }
    }
    assert!((dealt - 100.8_f64).abs() < 1e-9, "{dealt}");
    scene.change_bot_aura(&AuraChange::Remove { source, target }, tick);
    for _ in 0..10 {
        tick += 1;
        scene.advance_live_bots(tick);
    }
    assert!(scene.take_bot_health_changes().is_empty());
}
