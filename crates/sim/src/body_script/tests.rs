use super::*;
use crate::instance::TOURNAMENT_QUANTA;
use orpheus_physics::mover::sample_movement_flags;
use orpheus_physics::{
    collision::index::TriangleIndex, contact::world::ContactWorld, mover::settings::MoverSettings,
};
use orpheus_script::{
    target_tags::TargetTagLists,
    vm::{Site, TargetTagEffect, World},
    wire::Value,
};
use orpheus_wire::entity::ComponentValue::U32Array;

#[test]
fn player_natives_follow_the_verified_owner_and_override_shapes() {
    use orpheus_script::expr::EvalError;
    use serde_json::json;

    let spawn = fixture_body_factory().1;
    let mut runtime = fixture_hitscan_factory(20.0).start(spawn).unwrap();
    let evaluate = |body: &mut PlayerBodyRuntime, config: &Json| {
        body.component
            .evaluate_state_config(1, 0, config, &mut body.world)
    };
    let owner = json!({"$":"STU_B5A0CAF0","m_EE729DCB":0});
    let size = json!({"$":"STU_FDBB0B67","m_entity":owner});
    assert_eq!(evaluate(&mut runtime, &size), Ok(Value::Float(1.0)));
    for (value, supported) in [
        (Value::Integer(1), true),
        (Value::Float(1.0), true),
        (Value::Bool(true), false),
        (Value::Float(1.25), false),
        (Value::Null, true),
    ] {
        runtime
            .component
            .set_entity_variable(31_210, value, &mut runtime.world)
            .unwrap();
        assert_eq!(
            evaluate(&mut runtime, &size),
            if supported {
                Ok(Value::Float(1.0))
            } else {
                Err(EvalError::UnsupportedConfig("STU_FDBB0B67".into()))
            }
        );
    }
    let dynamic = json!({"$":"STUConfigVarDynamic","m_identifier":400,"m_60DB8F99":true});
    let dynamic_size = json!({"$":"STU_FDBB0B67","m_entity":dynamic});
    for (entity, supported) in [(spawn.entity, true), (spawn.entity + 1, false), (0, false)] {
        runtime
            .component
            .set_entity_variable(400, Value::Entity(entity), &mut runtime.world)
            .unwrap();
        assert_eq!(
            evaluate(&mut runtime, &dynamic_size).ok(),
            supported.then_some(Value::Float(1.0))
        );
    }

    // The body's controller (`STU_DB742151`) is the player's, which is no AI
    // (no component 92): the volleys deal the player's damage.
    let controller = json!({"$":"STU_DB742151","m_entity":owner,"m_EE729DCB":0});
    assert_eq!(
        evaluate(&mut runtime, &controller),
        Ok(Value::Entity(spawn.controller))
    );
    let path = json!({"$":"STU_D3EB8D87","m_00903303":controller,"m_EE729DCB":0});
    assert_eq!(evaluate(&mut runtime, &path), Ok(Value::Bool(false)));
    // Variable 400 holds no entity now: it has no controller, so no AI.
    let other_path =
        json!({"$":"STU_D3EB8D87","m_00903303":{"$":"STU_DB742151","m_entity":dynamic}});
    assert_eq!(evaluate(&mut runtime, &other_path), Ok(Value::Bool(false)));
    // IsTutorial (CE8580): whether the mode is the Tutorial
    // (leafall903/ORPHEUS, commit c239a84; Python answered null).
    let branch = json!({"$":"STU_A77AC2AC","m_EE729DCB":0});
    assert_eq!(evaluate(&mut runtime, &branch), Ok(Value::Bool(false)));
    runtime.world.mode = orpheus_content::maps::TUTORIAL_MODE;
    assert_eq!(evaluate(&mut runtime, &branch), Ok(Value::Bool(true)));
}

#[test]
fn target_tag_effects_restore_authored_tags_and_remaining_grants() {
    let (mut factory, spawn) = fixture_body_factory();
    let authored = 0x0d60_0000_0000_0042;
    let tag = 0x0d60_0000_0000_0422;
    factory.target_tags = Some(BTreeSet::from([authored]));
    let world = &mut factory.start(spawn).unwrap().world;
    let site = Site {
        entity: spawn.entity,
        instance: 1,
        graph: 42,
        node: Some(1),
    };
    let read = |world: &PlayerWorld, tag| world.has_target_tag(site, spawn.entity, tag);
    let grant = TargetTagEffect {
        entity: spawn.entity,
        tag,
        present: true,
    };
    let suppress = TargetTagEffect {
        present: false,
        ..grant
    };
    let mask_authored = TargetTagEffect {
        tag: authored,
        ..suppress
    };

    // What component 76 carries after each change: the tags added, then
    // the tags suppressed (the client drops a suppressed tag); none while
    // no tag came or went.
    let lists = |added: &[u64], suppressed: &[u64]| {
        Some(TargetTagLists {
            added: added.to_vec(),
            suppressed: suppressed.to_vec(),
        })
    };

    assert_eq!(read(world, authored), Some(true));
    assert_eq!(world.take_target_tag_change(), None);
    assert!(world.set_target_tag_effect(site, mask_authored, true));
    assert_eq!(read(world, authored), Some(false));
    assert_eq!(world.take_target_tag_change(), lists(&[], &[authored]));
    assert!(world.set_target_tag_effect(site, mask_authored, false));
    assert_eq!(read(world, authored), Some(true));
    assert_eq!(world.take_target_tag_change(), lists(&[], &[]));

    assert_eq!(read(world, tag), Some(false));
    for _ in 0..2 {
        assert!(world.set_target_tag_effect(site, grant, true));
    }
    assert_eq!(read(world, tag), Some(true));
    assert_eq!(world.take_target_tag_change(), lists(&[tag], &[]));
    for _ in 0..2 {
        assert!(world.set_target_tag_effect(site, suppress, true));
    }
    assert_eq!(read(world, tag), Some(false));
    let both = world.take_target_tag_change();
    assert_eq!(both, lists(&[tag], &[tag]));
    assert_eq!(
        target_tags_component(&both.unwrap()).fields,
        [Some(U32Array(vec![0x422])), Some(U32Array(vec![0x422]))]
    );
    assert!(!world.set_target_tag_effect(site, grant, true));
    assert!(world.set_target_tag_effect(site, grant, false));
    assert!(world.set_target_tag_effect(site, suppress, false));
    assert_eq!(read(world, tag), Some(false));
    assert_eq!(world.take_target_tag_change(), None);
    assert!(world.set_target_tag_effect(site, suppress, false));
    assert_eq!(read(world, tag), Some(true));
    assert_eq!(world.take_target_tag_change(), lists(&[tag], &[]));
    assert!(world.set_target_tag_effect(site, grant, false));
    assert_eq!(read(world, tag), Some(false));
    assert!(!world.set_target_tag_effect(site, grant, false));
    assert_eq!(world.take_target_tag_change(), lists(&[], &[]));
}

#[test]
fn target_tag_effects_keep_unknown_tags_and_replacement_independent() {
    let (factory, spawn) = fixture_body_factory();
    let tag = 0x0d60_0000_0000_0422;
    let site = Site {
        entity: spawn.entity,
        instance: 1,
        graph: 42,
        node: Some(1),
    };
    let read = |world: &PlayerWorld| world.has_target_tag(site, spawn.entity, tag);
    let effect = TargetTagEffect {
        entity: spawn.entity,
        tag,
        present: true,
    };
    let mut runtime = factory.start(spawn).unwrap();
    assert_eq!(read(&runtime.world), None);
    let foreign = TargetTagEffect {
        entity: spawn.controller,
        ..effect
    };
    assert!(!runtime.world.set_target_tag_effect(site, foreign, true));
    assert!(runtime.world.set_target_tag_effect(site, effect, true));
    assert_eq!(read(&runtime.world), Some(true));
    assert_eq!(
        runtime.world.has_target_tag(site, spawn.entity, tag + 1),
        None
    );

    let replacement = factory
        .start(OwnerBodySpawn {
            frame: spawn.frame + 1,
            ..spawn
        })
        .unwrap();
    assert_eq!(read(&replacement.world), None);
    assert!(runtime.world.set_target_tag_effect(site, effect, false));
    assert_eq!(read(&runtime.world), None);
}

#[test]
fn health_queries_read_the_authoritative_vitals() {
    use orpheus_script::expr::EvalError;
    use serde_json::json;

    let spawn = fixture_body_factory().1;
    let mut runtime = fixture_hitscan_factory(20.0).start(spawn).unwrap();
    let evaluate = |body: &mut PlayerBodyRuntime, config: &Json| {
        body.component
            .evaluate_state_config(1, 0, config, &mut body.world)
    };
    let owner = json!({"$": "STU_B5A0CAF0"});
    let current = json!({"$": "STU_32E6E4D8", "m_entity": owner});
    let maximum = json!({"$": "STU_7D7D2470", "m_entity": owner});
    assert_eq!(evaluate(&mut runtime, &current), Ok(Value::Float(200.0)));
    assert_eq!(evaluate(&mut runtime, &maximum), Ok(Value::Float(200.0)));
    runtime.world.vitals.damage(
        75.0,
        orpheus_script::vitals::Death {
            time: 0.0,
            frame: spawn.frame,
            killer: spawn.entity + 1,
        },
    );
    assert_eq!(evaluate(&mut runtime, &current), Ok(Value::Float(125.0)));
    assert_eq!(evaluate(&mut runtime, &maximum), Ok(Value::Float(200.0)));

    let target = json!({"$": "STUConfigVarDynamic", "m_identifier": 400, "m_60DB8F99": true});
    let current = json!({"$": "STU_32E6E4D8", "m_entity": target});
    // Another body's pools are unknown here; no entity reads 0 (D18110,
    // `SceneWorld.native` in leafall903/ORPHEUS, commit c239a84).
    for (entity, expected) in [
        (
            spawn.entity + 1,
            Err(EvalError::UnsupportedConfig("STU_32E6E4D8".into())),
        ),
        (0, Ok(Value::Float(0.0))),
    ] {
        runtime
            .component
            .set_entity_variable(400, Value::Entity(entity), &mut runtime.world)
            .unwrap();
        assert_eq!(evaluate(&mut runtime, &current), expected);
    }
}

#[test]
fn typed_health_queries_read_the_live_distribution_after_damage() {
    use orpheus_script::expr::EvalError;
    use serde_json::json;

    let (_, spawn) = fixture_body_factory();
    let mut factory = fixture_hitscan_factory(20.0);
    let mixed = HealthPools {
        health: 200.0,
        armour: 50.0,
        shields: 25.0,
    };
    let evaluate = |body: &mut PlayerBodyRuntime, config: &Json| {
        body.component
            .evaluate_state_config(1, 0, config, &mut body.world)
    };
    let query = |class, kind| {
        json!({
            "$":class, "m_entity":{"$":"STU_B5A0CAF0"}, "m_7BE02F89":null,
            "m_7E8B666C":{"$":"STU_297E1A30","m_7E8B666C":kind,"m_EE729DCB":0}
        })
    };
    let current = "STU_783B4CF0";
    let maximum = "STU_68883EAC";
    let unsupported = |class: &str| Err(EvalError::UnsupportedConfig(class.into()));

    let mut unknown = factory.start(spawn).unwrap();
    assert_eq!(
        evaluate(&mut unknown, &query(current, 0)),
        unsupported(current)
    );
    factory.pools = Some(FALLBACK_POOLS);
    let mut single = factory.start(spawn).unwrap();
    let hit = orpheus_script::vitals::Death {
        time: 0.0,
        frame: spawn.frame,
        killer: spawn.entity + 1,
    };
    single.world.vitals.damage(75.0, hit);
    for (kind, amount) in [(0, 125.0), (1, 0.0), (2, 0.0)] {
        assert_eq!(
            evaluate(&mut single, &query(current, kind)),
            Ok(Value::Float(amount))
        );
    }

    factory.pools = Some(mixed);
    for id in [253, 1732, 1737] {
        factory.variables.insert(id, Value::Float(mixed.total()));
    }
    let mut body = factory.start(spawn).unwrap();
    for (kind, amount) in [(0, 200.0), (1, 50.0), (2, 25.0)] {
        // `STU_F1C45C59`/`STU_005D99E6` read the same pools.
        for class in [current, maximum, "STU_F1C45C59", "STU_005D99E6"] {
            assert_eq!(
                evaluate(&mut body, &query(class, kind)),
                Ok(Value::Float(amount))
            );
        }
    }
    assert!(body.take_health_update().is_none());
    assert_eq!(body.world.vitals.damage(30.0, hit).dealt, 27.5);
    for (kind, amount) in [(0, 200.0), (1, 47.5), (2, 0.0)] {
        assert_eq!(
            evaluate(&mut body, &query(current, kind)),
            Ok(Value::Float(amount))
        );
    }
    assert_eq!(
        evaluate(&mut body, &query(maximum, 2)),
        Ok(Value::Float(25.0))
    );
    assert_eq!(
        body.take_health_update(),
        Some(body.world.vitals.component())
    );
    assert!(body.take_health_update().is_none());
    assert!(body.world.vitals.damage(1000.0, hit).killed);
    for kind in [0, 1, 2] {
        assert_eq!(
            evaluate(&mut body, &query(current, kind)),
            Ok(Value::Float(0.0))
        );
    }
    let mut foreign = query(current, 0);
    foreign["m_entity"] = json!({"$":"STU_F76C486C","m_D63CBEC2":{"$":"STU_B5A0CAF0"}});
    for config in [query(current, 3), foreign] {
        assert_eq!(evaluate(&mut single, &config), unsupported(current));
    }
}

#[test]
fn entity_relations_resolve_only_the_active_player_pair() {
    use orpheus_script::expr::EvalError;
    use serde_json::json;

    let (_, mut spawn) = fixture_body_factory();
    let factory = fixture_hitscan_factory(20.0);
    spawn.controller = 0xa012_3400;
    let replacement = OwnerBodySpawn {
        entity: 0xa012_7856,
        ..spawn
    };
    let mut first = factory.start(spawn).unwrap();
    let mut second = factory.start(replacement).unwrap();
    let target = json!({"$":"STUConfigVarDynamic","m_identifier":400,"m_60DB8F99":true});
    // The client's `STU_F76C486C` is component 72's owner (a projectile's),
    // none for a body or a controller; `STU_DB742151` is the entity itself
    // if it has component 44 (the controller), else its possessor
    // (KillFeed.md, "Client tools pass (2026-11-16)").
    let owner = json!({"$":"STU_F76C486C","m_D63CBEC2":target,"m_EE729DCB":0});
    let controller = json!({"$":"STU_DB742151","m_entity":target,"m_EE729DCB":0});
    let possessor = json!({"$":"STU_F7262B36","m_801B298F":target});
    let body = json!({"$":"STU_0AC15F78","m_entity":target});
    let invalid_owner = json!({"$":"STU_F76C486C","m_D63CBEC2":target,"m_EE729DCB":1});
    let evaluate = |runtime: &mut PlayerBodyRuntime, config: &Json, target: Value| {
        runtime
            .component
            .set_entity_variable(400, target, &mut runtime.world)
            .unwrap();
        runtime
            .component
            .evaluate_state_config(1, 0, config, &mut runtime.world)
    };
    let unsupported_owner = || Err(EvalError::UnsupportedConfig("STU_F76C486C".into()));
    let unsupported_controller = || Err(EvalError::UnsupportedConfig("STU_DB742151".into()));
    let unsupported_body = || Err(EvalError::UnsupportedConfig("STU_0AC15F78".into()));
    assert_eq!(
        evaluate(&mut first, &controller, Value::Entity(spawn.entity)),
        Ok(Value::Entity(spawn.controller))
    );
    assert_eq!(
        evaluate(&mut first, &possessor, Value::Entity(spawn.entity)),
        Ok(Value::Entity(spawn.controller))
    );
    assert_eq!(
        evaluate(&mut first, &body, Value::Entity(spawn.controller)),
        Ok(Value::Entity(spawn.entity))
    );
    assert_eq!(
        evaluate(&mut second, &body, Value::Entity(spawn.controller)),
        Ok(Value::Entity(replacement.entity))
    );
    for entity in [replacement.entity, spawn.controller] {
        assert_eq!(
            evaluate(&mut second, &controller, Value::Entity(entity)),
            Ok(Value::Entity(spawn.controller))
        );
        assert_eq!(
            evaluate(&mut second, &owner, Value::Entity(entity)),
            Ok(Value::Entity(0))
        );
    }
    assert_eq!(
        evaluate(&mut second, &possessor, Value::Entity(replacement.entity)),
        Ok(Value::Entity(spawn.controller))
    );
    // An entity this world doesn't know is refused; no entity relates to
    // none.
    assert_eq!(
        evaluate(&mut second, &owner, Value::Entity(spawn.entity)),
        unsupported_owner()
    );
    assert_eq!(
        evaluate(&mut second, &controller, Value::Entity(spawn.entity)),
        unsupported_controller()
    );
    for config in [&owner, &controller] {
        assert_eq!(
            evaluate(&mut second, config, Value::Entity(0)),
            Ok(Value::Entity(0))
        );
    }
    assert_eq!(
        evaluate(&mut second, &body, Value::Entity(replacement.entity)),
        unsupported_body()
    );
    assert_eq!(
        evaluate(&mut second, &possessor, Value::Entity(spawn.entity)),
        Err(EvalError::UnsupportedConfig("STU_F7262B36".into()))
    );
    assert_eq!(
        evaluate(
            &mut second,
            &body,
            Value::Integer(i64::from(spawn.controller))
        ),
        unsupported_body()
    );
    assert_eq!(
        evaluate(
            &mut second,
            &invalid_owner,
            Value::Entity(replacement.entity)
        ),
        unsupported_owner()
    );
}

#[test]
fn verified_body_starts_fresh_instances_and_advances_only_its_own_retail_clock() {
    let (factory, spawn) = fixture_body_factory();
    assert!(
        factory
            .start(OwnerBodySpawn {
                quanta: TOURNAMENT_QUANTA,
                ..spawn
            })
            .is_err()
    );
    let mut first = factory.start(spawn).unwrap();
    let second = factory
        .start(OwnerBodySpawn {
            entity: spawn.entity + 1,
            ..spawn
        })
        .unwrap();
    let snapshot = first.snapshot().unwrap();
    assert_eq!(
        snapshot.instances.keys().copied().collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(snapshot.instances[&3].variables[&28], Value::Integer(1));
    assert_eq!(snapshot.variables[&1732], Value::Float(200.0));
    assert_eq!(first.movement_modifiers().map(BTreeMap::len), Some(0));
    assert_eq!(first.world.position(spawn.entity), spawn.pose.position);
    assert_eq!(first.world.movement, spawn.pose);
    assert_eq!(second.snapshot().unwrap().entity, spawn.entity + 1);

    let site = Site {
        entity: spawn.entity,
        instance: 1,
        graph: 42,
        node: None,
    };
    assert_eq!(first.world.movement_sample(site), Some(2));
    assert_eq!(second.world.movement_sample(site), None);
    let mut mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let command = Command {
        frame: 101,
        right: -63,
        forward: 64,
        ..Command::default()
    };
    mover
        .step(
            &TriangleIndex::new([]).unwrap(),
            command,
            0.016,
            |mover, command| first.advance(command, mover, BodyStep::Posed).unwrap(),
        )
        .unwrap();
    assert_eq!(first.world.position(spawn.entity), mover.pose().position);
    let posed = mover.pose();
    let sample = sample_movement_flags(&posed).expect("a regular posed movement record");
    assert_eq!(first.world.movement, posed);
    assert_eq!(first.world.movement_sample(site), Some(sample));
    assert_eq!(first.component.now_ms(), 102 * FRAME_MS - 1);
    assert_eq!(second.component.now_ms(), 101 * FRAME_MS - 1);
    let gap = Command {
        frame: 104,
        ..command
    };
    assert!(first.advance(gap, &mover, BodyStep::Posed).is_err());
    assert_eq!(first.component.now_ms(), 102 * FRAME_MS - 1);
    let unposed_mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        [4.0, 5.0, 6.0],
        0,
        0x18,
    )
    .unwrap();
    first
        .advance(
            Command {
                frame: 102,
                ..Command::default()
            },
            &unposed_mover,
            BodyStep::UnposedBoundary,
        )
        .unwrap();
    assert_eq!(first.world.movement, posed);
    first
        .advance(
            Command {
                frame: 200,
                ..Command::default()
            },
            &unposed_mover,
            BodyStep::UnposedTimer,
        )
        .unwrap();
    assert_eq!(first.world.movement, posed);
    first
        .advance(
            Command {
                frame: 201,
                ..Command::default()
            },
            &mover,
            BodyStep::UnposedBoundary,
        )
        .unwrap();
    assert_eq!(first.world.throttles, [0, 0]);
    assert_eq!(first.component.now_ms(), 202 * FRAME_MS - 1);
}

#[test]
fn posed_volley_keeps_the_authored_shot_frame_and_drains_unposed_reports() {
    let factory = fixture_hitscan_factory(20.0);
    let (_, spawn) = fixture_body_factory();
    let mut runtime = factory.start(spawn).unwrap();
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let fire = Command {
        frame: 101,
        buttons: orpheus_wire::commands::FIRE,
        ..Command::default()
    };
    runtime.advance(fire, &mover, BodyStep::Posed).unwrap();
    let command = Command {
        frame: 102,
        yaw: 12,
        pitch: 30,
        ..Command::default()
    };
    let shots = runtime.take_shots(Some(command), &mover);
    let [PosedShot::Hitscan(shot)] = shots.as_slice() else {
        panic!("expected the opening shot")
    };
    assert_eq!(shot.source, spawn.entity);
    assert_eq!(shot.origin, [1.0, 3.55, 3.0]);
    assert_eq!(shot.shot_frame, 101);
    assert_eq!(shot.command, command);
    // No spread ramp: the shot leaves along the aim.
    assert_eq!(shot.direction, aim_direction(&command));
    assert_eq!(
        (shot.reach, shot.hit.amount, shot.hit.critical),
        (180.0, 20.0, 2.0)
    );
    assert!(runtime.take_shots(Some(command), &mover).is_empty());

    runtime
        .advance(command, &mover, BodyStep::UnposedBoundary)
        .unwrap();
    assert_eq!(
        runtime.snapshot().unwrap().instances[&1].states[&0].kind,
        orpheus_script::vm::StateKind::LogicalButton { down: false }
    );
    let timer = Command {
        frame: 107,
        ..Command::default()
    };
    runtime
        .advance(timer, &mover, BodyStep::UnposedTimer)
        .unwrap();
    assert!(runtime.take_shots(None, &mover).is_empty());
    assert!(runtime.take_shots(Some(timer), &mover).is_empty());
}

#[test]
fn secondary_volley_uses_authored_projectile_and_owner_lease() {
    let factory = fixture_projectile_factory(40.0);
    let (_, mut spawn) = fixture_body_factory();
    spawn.slot = 2;
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let fire = Command {
        frame: spawn.frame + 1,
        buttons: orpheus_wire::commands::SECONDARY,
        yaw: 16384,
        ..Command::default()
    };
    let mut runtime = factory.start(spawn).unwrap();
    assert_eq!(
        runtime
            .projectile_leases()
            .unwrap()
            .entity(FIXTURE_PROJECTILE_DEFINITION, fire.frame, 0),
        Some(0xa210_0065)
    );
    runtime.advance(fire, &mover, BodyStep::Posed).unwrap();
    let shots = runtime.take_shots(Some(fire), &mover);
    let [PosedShot::Projectile(rocket)] = shots.as_slice() else {
        panic!("expected a predicted rocket");
    };
    assert_eq!(
        (rocket.shot.entity, rocket.shot.frame),
        (spawn.entity, fire.frame)
    );
    assert_eq!((rocket.shot.instance, rocket.shot.raw_state), (1, 1));
    assert_eq!(rocket.origin, [1.0, 3.55, 3.0]);
    assert_eq!(rocket.direction, aim_direction(&fire));
    assert_eq!(
        rocket.ballistics.definition,
        Some(FIXTURE_PROJECTILE_DEFINITION)
    );
    assert_eq!(
        (rocket.ballistics.speed, rocket.ballistics.lifetime),
        (50.0, 2.0)
    );
    assert_eq!(rocket.ballistics.direct.unwrap().amount, 40.0);
    assert_eq!(rocket.ballistics.blast.as_ref().unwrap().hit.amount, 80.0);
    assert!(runtime.take_shots(Some(fire), &mover).is_empty());

    let encoder = runtime.encoder();
    let owner = encoder
        .project(runtime.snapshot().unwrap(), View::Owner)
        .unwrap();
    assert_eq!(
        owner.volley_identity(1, 1),
        Some((rocket.shot.activation, rocket.shot.activation_ms))
    );
}

/// combat.py `_shoot`: a projectile volley the owner's client doesn't shoot
/// under a lease (not predicted) flies as a server projectile, with its
/// damage or without (beyond Python: Moira's healing orb, whose entity's
/// graph does its work); an arc, an aim id and a sticky collision config fly
/// leased with the volley's gravity and the fuse; a motion the flight
/// doesn't model flies as if it weren't there; a volley of several
/// projectiles a shot flies one along the aim, leased with as many ids a
/// frame when predicted (up to 7 a shot), else for its damage alone.
#[test]
fn projectile_volleys_fly_leased_for_damage_alone_or_not_at_all() {
    use orpheus_script::weapons::STICK_FUSE;
    use serde_json::json;

    let (_, spawn) = fixture_body_factory();
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let fire = Command {
        frame: spawn.frame + 1,
        buttons: orpheus_wire::commands::SECONDARY,
        ..Command::default()
    };
    let shoot = |fields: Json| {
        let mut runtime = fixture_volley_factory(29, true, fields)
            .start(spawn)
            .unwrap();
        runtime.advance(fire, &mover, BodyStep::Posed).unwrap();
        let mut shots = runtime.take_shots(Some(fire), &mover);
        assert!(shots.len() <= 1);
        shots.pop().map(|shot| match shot {
            PosedShot::Projectile(projectile) => projectile,
            _ => panic!("a projectile volley became another shot"),
        })
    };

    let mut fields = fixture_projectile_fields(40.0);
    fields["m_45271E8C"] = json!(false);
    let unpredicted = shoot(fields.clone()).expect("the damage flies");
    assert!(!unpredicted.leased && unpredicted.server_shown);
    fields["m_D2D11CE9"]["m_modifyHealth"]["m_amount"] = json!(0.0);
    fields["m_6396149F"]["m_modifyHealth"]["m_amount"] = json!(0.0);
    let harmless = shoot(fields).expect("a server projectile flies without damage");
    assert!(harmless.server_shown && !harmless.ballistics.damaging());

    let mut fields = fixture_projectile_fields(5.0);
    fields["m_projectileMotions"] =
        json!([{"$":"STU_E1A5B579","m_599AB8D7":{"$":"STUConfigVarFloat","m_value":30.0}}]);
    fields["m_aimID"] = json!({"$":"STU_91EFD5B1","m_value":null,
        "m_default":{"$":"STUConfigVarInt","m_value":1}});
    fields["m_55E8761C"] = json!({"$":"STU_3A858EF7",
        "m_47999E82":{"$":"STUConfigVarBool","m_value":1}});
    let bomb = shoot(fields).expect("the arc flies");
    assert!(bomb.leased);
    assert_eq!(
        (bomb.ballistics.gravity, bomb.ballistics.fuse),
        (30.0, Some(STICK_FUSE))
    );
    assert_eq!(bomb.ballistics.bound(), 2.0 + STICK_FUSE);

    let mut fields = fixture_projectile_fields(40.0);
    fields["m_projectileMotions"] = json!([{"$":"STU_7A5DFFF8"}]);
    let wobble = shoot(fields).expect("an unmodelled motion flies");
    assert!(wobble.leased);
    assert_eq!(
        (wobble.ballistics.gravity, wobble.ballistics.fuse),
        (0.0, None)
    );
    assert_eq!(wobble.ballistics.unmodelled, ["STU_7A5DFFF8"]);

    let mut fields = fixture_projectile_fields(40.0);
    fields["m_numProjectilesPerShot"] = json!(3);
    let runtime = fixture_volley_factory(29, true, fields.clone())
        .start(spawn)
        .unwrap();
    let leases = runtime.projectile_leases().unwrap();
    assert!(
        leases
            .entity(FIXTURE_PROJECTILE_DEFINITION, fire.frame, 2)
            .is_some()
    );
    assert_eq!(
        leases.entity(FIXTURE_PROJECTILE_DEFINITION, fire.frame, 3),
        None
    );
    let trio = shoot(fields.clone()).expect("a predicted volley of three flies");
    assert!(trio.leased);
    assert_eq!(trio.projectiles, 3);
    assert_eq!(trio.direction, aim_direction(&fire));
    assert_eq!(trio.pellets, [aim_direction(&fire)]);
    // Echo's Tri-Shot layout (one ring of 3 at 0.6 of the spread, here 5°):
    // the leased shot still leaves along the aim, and its three pellets
    // fly 3° around it for the damage (upstream 7cb4018).
    fields["m_5EC1EB38"] = json!({"$":"STU_613BDDB0","m_spread":5,"m_0D30E40C":[
        {"$":"STU_B101467F","m_6425FFB9":0.6,"m_B19BD35D":3}]});
    let ringed = shoot(fields.clone()).expect("a ringed volley of three flies");
    assert!(ringed.leased);
    assert_eq!(ringed.direction, aim_direction(&fire));
    let angle = |pellet: &[f64; 3]| {
        let aim = aim_direction(&fire);
        let dot: f64 = aim.iter().zip(pellet).map(|(a, b)| a * b).sum();
        dot.clamp(-1.0, 1.0).acos().to_degrees()
    };
    assert_eq!(ringed.pellets.len(), 3);
    assert!(
        ringed
            .pellets
            .iter()
            .all(|pellet| (angle(pellet) - 3.0).abs() < 1e-9)
    );
    // More than 7 a shot resolve to shots only (projectiles.py `_sync`);
    // Roadhog's rings lay out 25 pellets around the aim.
    fields["m_numProjectilesPerShot"] = json!(25);
    fields["m_5EC1EB38"]["m_0D30E40C"] = json!([
        {"$":"STU_B101467F","m_6425FFB9":0,"m_B19BD35D":1},
        {"$":"STU_B101467F","m_6425FFB9":0.3,"m_B19BD35D":5},
        {"$":"STU_B101467F","m_6425FFB9":0.6,"m_B19BD35D":10},
        {"$":"STU_B101467F","m_6425FFB9":1,"m_B19BD35D":9}]);
    let pellets = shoot(fields.clone()).expect("the pellets' damage flies");
    assert!(!pellets.leased);
    assert_eq!(pellets.projectiles, 25);
    assert_eq!(pellets.ballistics.direct.unwrap().amount, 40.0);
    let mut angles: Vec<_> = pellets
        .pellets
        .iter()
        .map(|pellet| (angle(pellet) * 1e6).round() / 1e6)
        .collect();
    angles.dedup();
    assert_eq!(angles, [0.0, 1.5, 3.0, 5.0]);
    // Rings that don't add up to the count fly one along the aim.
    fields["m_numProjectilesPerShot"] = json!(24);
    let uneven = shoot(fields).expect("the damage flies");
    assert_eq!(uneven.pellets, [aim_direction(&fire)]);
}

/// leafall903/ORPHEUS (MIT) c239a84 `weapon_spread.py` and e5463e3: a
/// volley whose launch spread grows (`STU_239B571E`, ramp 4, 1° to 3°)
/// turns each hitscan shot by the client's circular spread at the shot's
/// normalized spread (the first shot: 1/4, so 1.5°), seeded from the body,
/// instance, state index and activation counter.
#[test]
fn a_growing_spread_turns_each_shot_inside_its_cone() {
    use crate::weapon_spread::{direction, seed_base};
    use serde_json::json;

    let factory = fixture_volley_factory(
        20,
        false,
        json!({
            "m_A17BE89B":2,"m_numShotsPerSecond":9,"m_FEC435C6":180,
            "m_6396149F":{"m_modifyHealth":{"m_amount":20.0}},
            "m_5EC1EB38":{"$":"STU_239B571E","m_AC20B1C1":4,"m_23C72C36":null,
                "m_7F0FED02":1,"m_429A6C85":3}
        }),
    );
    let (_, spawn) = fixture_body_factory();
    let mut runtime = factory.start(spawn).unwrap();
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let fire = Command {
        frame: 101,
        buttons: orpheus_wire::commands::FIRE,
        yaw: 12,
        pitch: 30,
        ..Command::default()
    };
    runtime.advance(fire, &mover, BodyStep::Posed).unwrap();
    let shots = runtime.take_shots(Some(fire), &mover);
    let [PosedShot::Hitscan(shot)] = shots.as_slice() else {
        panic!("expected the opening shot")
    };
    let aim = aim_direction(&fire);
    let cone = 1.5f64.to_radians() as f32;
    // The volley is node 2, state 1: the seed takes the state index
    // (upstream main `9ed63eed`).
    assert_eq!(
        shot.direction,
        direction(aim, cone, seed_base(spawn.entity, 1, 1, 1), 0, 0, false)
    );
    assert_ne!(shot.direction, aim);
    let dot: f64 = aim.iter().zip(shot.direction).map(|(a, b)| a * b).sum();
    assert!(dot >= f64::from(cone).cos() - 1e-7);
}

/// projectiles.py `fired`: a volley whose projectile leaves an entity names
/// it as the shot's spawn, and the Biotic Field canister flies its arc. The
/// shot carries the volley state's count of volleys for the other clients'
/// shot record.
#[test]
fn a_spawning_canister_flies_its_arc_and_names_its_field() {
    use crate::projectiles::FIELD_ENTITY;
    use serde_json::json;

    let (_, spawn) = fixture_body_factory();
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let fire = Command {
        frame: spawn.frame + 1,
        buttons: orpheus_wire::commands::SECONDARY,
        ..Command::default()
    };
    let mut fields = fixture_projectile_fields(0.0);
    fields["m_projectileMotions"] = json!([{"$":"STU_E1A5B579"}]);
    fields["m_49CBA3BC"] = json!({"$":"STU_78BF9026","m_entityDefinition":{
        "$":"STU_8556841E","m_resourceKey":null,
        "m_entityDef":format!("0x{FIELD_ENTITY:016X}")}});
    let mut runtime = fixture_volley_factory(29, true, fields)
        .start(spawn)
        .unwrap();
    runtime.advance(fire, &mover, BodyStep::Posed).unwrap();
    let shots = runtime.take_shots(Some(fire), &mover);
    let [PosedShot::Projectile(canister)] = shots.as_slice() else {
        panic!("expected the canister");
    };
    assert_eq!(canister.spawn, Some(FIELD_ENTITY));
    assert!(canister.leased);
    assert_eq!(canister.feet, spawn.pose.position);
    assert_eq!(canister.volleys, 1);
}

#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn prepared_soldier_starts_the_reference_body_and_weapon_tree() {
    use orpheus_content::maps::GameMap;
    use orpheus_core::tick::FIRST_TICK;
    use orpheus_script::{codecs::PayloadMode, vm::StateKind};

    let root = orpheus_testkit::runtime_dir();
    let graphs =
        Arc::new(GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap());
    let pools = crate::pools::PoolCatalog::from_graphs(&graphs)
        .unwrap()
        .hero_pools(SOLDIER)
        .unwrap();
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let roots = orpheus_script::vm::player_roots(body, &graphs).unwrap();
    let health = pools.total();
    let variables = orpheus_script::vm::player_entity_variables(body, health, health);
    let mut world = {
        let mut world = PlayerWorld::new(
            Arc::clone(&graphs),
            0xa000_0100,
            0xa000_0101,
            crate::scene::team_bit(0).unwrap(),
            PRACTICE_RANGE_MODE,
            SOLDIER,
        );
        world.position = GameMap::practice_range().spawn_point(0, 0).unwrap().0;
        world.movement = Movement::default();
        world.throttles = [0; 2];
        world.vitals = Vitals::empty();
        world.pool_types_known = true;
        world.target_tags = body.target_tags.clone();
        world.controller_on_fire = false;
        world
    };
    let mut component = Component::start(
        graphs,
        world.entity,
        variables,
        roots,
        FIRST_TICK,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    component.advance_frame(FIRST_TICK, &mut world).unwrap();
    let snapshot = component.snapshot().unwrap();
    // The server refuses a body whose startup has any gap.
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
    // The frozen BodyScript replay allocates the same IDs with the team
    // mask, plus 01CB (18) under 004B st2: 004B st14 holds for a body
    // without tag 0069 (the filter's none-of list, client 0x7FF789CC2FD0;
    // the Python replay read it as required). The exit retires 08B6 (9)
    // and its unstarted child 0BF8 (26).
    let expected = [
        (1, 0x0033, None),
        (2, 0x004b, None),
        (3, 0x0043, None),
        (4, 0x0251, None),
        (5, 0x0255, None),
        (6, 0x0257, None),
        (7, 0x0259, None),
        (8, 0x091b, None),
        (10, 0x0015, None),
        (11, 0x0254, None),
        (12, 0x01c7, Some((2, 0))),
        (13, 0x01cf, Some((2, 3))),
        (14, 0x02c5, Some((2, 5))),
        (15, 0x0b8b, Some((2, 10))),
        (16, 0x0c7d, Some((2, 18))),
        (17, 0x01ca, Some((2, 1))),
        (18, 0x01cb, Some((2, 2))),
        (19, 0x0ba4, Some((2, 11))),
        (20, 0x05ac, Some((2, 8))),
        (21, 0x02cf, Some((2, 6))),
        (22, 0x0b3b, Some((2, 9))),
        (23, 0x01d8, Some((2, 4))),
        (24, 0x0f82, Some((2, 21))),
        (25, 0x05ad, Some((2, 7))),
        (27, 0x0e8f, Some((13, 32))),
    ];
    let instances: Vec<_> = snapshot
        .instances
        .iter()
        .map(|(&id, instance)| (id, instance.graph, instance.parent))
        .collect();
    assert_eq!(instances, expected);
    let movement_graph = &snapshot.instances[&12];
    let movement = &movement_graph.states[&4];
    assert!(movement.active);
    assert_eq!(movement.kind, StateKind::MovementSample(2));
    assert_eq!(snapshot.variables[&545], Value::Bool(true));
    let body_pools = &snapshot.instances[&1];
    assert!(body_pools.states[&0].active);
    assert_eq!(
        body_pools.states[&0].kind,
        StateKind::HealthPool { pool: Some(1) }
    );
    assert_eq!(body_pools.variables[&520], Value::Float(200.0));
    assert_eq!(world.vitals.component(), pools.vitals().component());
    let target_tag = &snapshot.instances[&12].states[&31];
    let effect = TargetTagEffect {
        entity: world.entity,
        tag: 0x0d60_0000_0000_0422,
        present: true,
    };
    assert!(target_tag.active);
    assert_eq!(
        target_tag.kind,
        StateKind::TargetTag {
            effect: Some(effect)
        }
    );
    let site = Site {
        entity: world.entity,
        instance: 12,
        graph: 0x01c7,
        node: Some(96),
    };
    assert_eq!(
        world.has_target_tag(site, world.entity, effect.tag),
        Some(true)
    );
    assert!(
        !snapshot
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.site == site)
    );
    let Value::Pairs(messages) = &snapshot.instances[&25].variables[&29859] else {
        panic!("the authored message table must be typed pairs");
    };
    assert_eq!(messages.len(), 20);
    assert_eq!(
        messages[0],
        (
            Value::Asset(0x0a90_0000_0000_0171),
            Value::Asset(0x0240_0000_0000_05ad)
        )
    );
    assert_eq!(
        snapshot.instances[&1].variables[&9737],
        Value::Entity(world.entity)
    );
    assert!(
        !snapshot
            .diagnostics
            .iter()
            .any(|item| item.site.graph == 0x0033 && item.site.node == Some(51))
    );
    let watch = &snapshot.instances[&13].states[&21];
    assert!(watch.active);
    assert_eq!(watch.kind, orpheus_script::vm::StateKind::Watch);
    assert!(
        !snapshot
            .diagnostics
            .iter()
            .any(|item| item.site.graph == 0x01cf && item.site.node == Some(41))
    );
    assert_eq!(snapshot.instances[&11].variables[&28], Value::Integer(1));
    for (instance, state) in [(10, 42), (10, 43), (11, 83), (13, 138), (13, 139), (14, 30)] {
        let controller = &snapshot.instances[&instance].states[&state];
        assert!(controller.active);
        assert_eq!(controller.kind, orpheus_script::vm::StateKind::Plain);
    }
    let tips = &snapshot.instances[&4].states[&0];
    assert!(tips.active);
    assert_eq!(tips.kind, StateKind::Plain);
    let codecs = CodecTable::client();
    for mode in [
        PayloadMode::Remote { full: true },
        PayloadMode::Owner {
            full: true,
            frame_time: i64::from(FIRST_TICK) * FRAME_MS as i64,
            frame_ms: FRAME_MS as u32,
        },
    ] {
        for class in [
            "STU_CD46AF93",
            "STU_F5A99B2A",
            "STU_D9843704",
            "STU_E56BA8D8",
        ] {
            let codec = codecs.codec(class).unwrap();
            assert_eq!(codec.payload(&BTreeMap::new(), mode).unwrap().bit_len(), 0);
        }
    }
    for (instance, raw, position) in [(1, 13, 39), (2, 22, 32), (2, 23, 33), (10, 39, 73)] {
        let instance = &snapshot.instances[&instance];
        let node = world
            .graphs
            .graph(instance.graph)
            .unwrap()
            .state(usize::from(raw))
            .unwrap();
        assert_eq!((node.pos, node.class.as_str()), (position, "STU_CD46AF93"));
        let state = &instance.states[&raw];
        assert!(state.active);
        assert_eq!(state.kind, StateKind::Plain);
    }
    for (instance, raw) in [(1, 4), (10, 41)] {
        let child = &snapshot.instances[&instance].states[&raw];
        assert!(child.active);
        assert_eq!(child.kind, StateKind::BooleanSwitch(false));
    }
    let selection = &snapshot.instances[&19].states[&0];
    assert!(selection.active);
    assert_eq!(selection.kind, StateKind::BooleanSwitch(false));
    assert!(!snapshot.instances[&19].states.contains_key(&1));
    let on_fire = &snapshot.instances[&13].states[&17];
    assert!(on_fire.active);
    assert_eq!(on_fire.kind, StateKind::BooleanSwitch(false));
    assert!(
        !snapshot.diagnostics.iter().any(|diagnostic| {
            diagnostic.site.graph == 0x01cf && diagnostic.site.node == Some(31)
        })
    );
    for (graph, node) in [
        (0x0033, 1),
        (0x0033, 66),
        (0x004b, 19),
        (0x004b, 20),
        (0x08b6, 3),
        (0x08b6, 11),
        (0x0ba4, 10),
    ] {
        assert!(!snapshot.diagnostics.iter().any(|diagnostic| {
            diagnostic.site.graph == graph && diagnostic.site.node == Some(node)
        }));
    }
    let filter_diagnostic = snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.site.graph == 0x004b && diagnostic.site.node == Some(21));
    assert_eq!(filter_diagnostic, world.target_tags.is_none());
    for raw in [12, 13] {
        let filter = &snapshot.instances[&2].states[&raw];
        assert!(filter.active);
        assert_eq!(filter.kind, StateKind::BooleanSwitch(true));
    }
    if world.target_tags.is_some() {
        // st14 starts 01CB (the death layer) on a body without tag 0069.
        let filter = &snapshot.instances[&2].states[&14];
        assert!(filter.active);
        assert_eq!(filter.kind, StateKind::BooleanSwitch(true));
    }

    let authored_graphs = Arc::clone(&world.graphs);
    let authored_field = |graph, position, field: &str| {
        authored_graphs
            .graph(graph)
            .unwrap()
            .node(position)
            .unwrap()
            .field(field)
            .unwrap()
            .clone()
    };
    let sprint_condition = authored_field(0x0255, 6, "m_condition");
    let watch_possessor = authored_field(0x01cf, 41, "m_B7A3CF78");
    let shield_refill = authored_field(0x0033, 66, "m_BD83C8E6");
    let health_present = authored_field(0x08b6, 2, "m_condition");
    let filter = authored_field(0x004b, 20, "m_condition");
    let mask = world.body_filter;
    for (flags, passes) in [(mask, true), (mask | 2, false)] {
        world.body_filter = flags;
        assert_eq!(
            component.evaluate_state_config(2, 13, &filter, &mut world),
            Ok(Value::Bool(passes))
        );
    }
    world.body_filter = mask;
    let on_fire = authored_field(0x01cf, 31, "m_condition");
    for value in [false, true] {
        world.controller_on_fire = value;
        assert_eq!(
            component.evaluate_state_config(13, 17, &on_fire, &mut world),
            Ok(Value::Bool(value))
        );
    }
    world.controller_on_fire = false;
    let entries = authored_field(0x05ad, 240, "m_value");
    let mut message = authored_field(0x05ad, 183, "m_gameMessage");
    // Supply one authored key as a literal rather than simulating its UI input.
    message["m_configVars"] = serde_json::json!([entries["m_configVars"][0]["m_key"].clone()]);
    message["m_expression"]["m_opcodes"] = serde_json::json!([8, 0, 14, 0, 17, 0]);
    assert_eq!(
        component.evaluate_state_config(25, 0, &message, &mut world),
        Ok(Value::Asset(0x0240_0000_0000_05ad))
    );
    if let Some(original_tags) = world.target_tags.clone() {
        let filter = authored_field(0x004b, 21, "m_condition");
        let tag_query = authored_field(0x0043, 45, "m_condition");
        let required = orpheus_script::graph::target_tag_guid(&tag_query["m_88B0050A"]).unwrap();
        assert!(!original_tags.contains(&required));
        for present in [false, true] {
            if present {
                world.target_tags.as_mut().unwrap().insert(required);
            }
            // 004B st14 lists tag 0069 under none-of (m_8B2A4DF3): it holds
            // only while the body lacks the tag (client 0x7FF789CC2FD0).
            assert_eq!(
                component.evaluate_state_config(2, 14, &filter, &mut world),
                Ok(Value::Bool(!present))
            );
            assert_eq!(
                component.evaluate_state_config(3, 0, &tag_query, &mut world),
                Ok(Value::Bool(present))
            );
        }
        world.target_tags = Some(original_tags);
    }
    let rifle_conditions = [81, 83].map(|position| authored_field(0x0254, position, "m_condition"));
    // 01C7 st48 reads the same current and maximum health as the
    // health-ratio branches in 01CF st7/st9.
    let full_health = authored_field(0x01c7, 145, "m_condition");
    assert_eq!(
        component.evaluate_state_config(13, 21, &watch_possessor, &mut world),
        Ok(Value::Entity(world.controller))
    );
    assert_eq!(
        component.evaluate_state_config(1, 27, &shield_refill, &mut world),
        Ok(Value::Bool(false))
    );
    assert_eq!(
        component.evaluate_state_config(12, 48, &health_present, &mut world),
        Ok(Value::Bool(true))
    );
    assert_eq!(
        component.evaluate_state_config(12, 48, &full_health, &mut world),
        Ok(Value::Bool(true))
    );
    // 0255 st2 follows the throttle comparison for a human player.
    for (forward, enabled) in [(63, false), (64, true)] {
        world.throttles = [0, forward];
        assert_eq!(
            component.evaluate_state_config(5, 2, &sprint_condition, &mut world),
            Ok(Value::Bool(enabled))
        );
    }
    // Both 0254 rifle branches ask IsTutorial (`STU_A77AC2AC`), false outside
    // the Tutorial (leafall903/ORPHEUS, commit c239a84; Python answered null).
    for condition in rifle_conditions {
        assert_eq!(
            component.evaluate_state_config(11, 10, &condition, &mut world),
            Ok(Value::Bool(false))
        );
    }
    world.vitals.damage(
        75.0,
        orpheus_script::vitals::Death {
            time: 0.0,
            frame: FIRST_TICK + 1,
            killer: world.entity + 1,
        },
    );
    component.advance_frame(FIRST_TICK + 1, &mut world).unwrap();
    let damaged = component.snapshot().unwrap();
    assert_eq!(damaged.variables[&253], Value::Float(125.0));
    assert_eq!(damaged.variables[&1732], Value::Float(125.0));
    assert_eq!(damaged.variables[&1737], Value::Float(200.0));
    assert_eq!(damaged.instances[&1].variables[&520], Value::Float(125.0));
    assert_eq!(
        component.evaluate_state_config(12, 48, &full_health, &mut world),
        Ok(Value::Bool(false))
    );
}

#[test]
#[ignore = "requires the derived 104319 hero and graph catalogs"]
fn authored_armour_and_shield_roots_reproduce_the_reference_body_pools() {
    use orpheus_core::tick::FIRST_TICK;

    let root = orpheus_testkit::runtime_dir();
    let heroes =
        orpheus_content::heroes::HeroCatalog::load(root.join("data/game_heroes_174.json")).unwrap();
    let graphs =
        Arc::new(GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap());
    let catalog = crate::pools::PoolCatalog::from_graphs(&graphs).unwrap();
    for (hero, secondary_state) in [(0x02e0_0000_0000_0007, 3), (0x02e0_0000_0000_0068, 1)] {
        let body = &graphs.bodies()[&heroes.get(hero).unwrap().body];
        let pools = catalog.hero_pools(hero).unwrap();
        let roots = orpheus_script::vm::player_roots(body, &graphs)
            .unwrap()
            .into_iter()
            .filter(|root| root.graph == BODY_GRAPH)
            .collect();
        let health = pools.total();
        let mut world = PlayerWorld::new(
            Arc::clone(&graphs),
            0xa000_0100,
            0xa000_0101,
            crate::scene::team_bit(0).unwrap(),
            PRACTICE_RANGE_MODE,
            hero,
        );
        world.pool_types_known = true;
        world.target_tags = body.target_tags.clone();
        let component = Component::start(
            Arc::clone(&graphs),
            world.entity,
            orpheus_script::vm::player_entity_variables(body, health, health),
            roots,
            FIRST_TICK,
            &mut world,
            MAX_INIT_WORK,
        )
        .unwrap();
        let snapshot = component.snapshot().unwrap();
        assert!(!snapshot.diagnostics.iter().any(|item| {
                matches!(item.issue, orpheus_script::vm::Issue::UnsupportedField(field) if field.starts_with("health pool"))
            }));
        let states = &snapshot.instances[&1].states;
        assert_eq!(
            states[&secondary_state].kind,
            orpheus_script::vm::StateKind::HealthPool { pool: Some(1) }
        );
        assert_eq!(
            states[&0].kind,
            orpheus_script::vm::StateKind::HealthPool { pool: Some(2) }
        );
        assert_eq!(world.vitals.component(), pools.vitals().component());
    }
}

/// A body whose pools run out raises its dead flag (#478), which the
/// graphs that show it to others read; while it lives the flag isn't set.
#[test]
fn a_body_that_dies_raises_its_dead_flag() {
    use orpheus_script::vitals::Death;

    let spawn = fixture_body_factory().1;
    let mut runtime = fixture_hitscan_factory(20.0).start(spawn).unwrap();
    let flag = |runtime: &PlayerBodyRuntime| {
        runtime
            .snapshot()
            .unwrap()
            .variables
            .get(&DEAD_FLAG)
            .cloned()
    };
    let source = Death {
        time: 1.0,
        frame: spawn.frame,
        killer: 0xa000_0201,
    };
    let total = runtime.vitals().unwrap().total();
    runtime.vitals_mut().unwrap().damage(total / 2.0, source);
    runtime.health_changed().unwrap();
    assert_eq!(flag(&runtime), None);
    assert!(!runtime.component.owner_dead());
    runtime.vitals_mut().unwrap().damage(total, source);
    runtime.health_changed().unwrap();
    assert_eq!(flag(&runtime), Some(Value::Bool(true)));
    assert!(runtime.component.owner_dead());
}

/// The communication wheel on Soldier: 76's own graphs
/// ([`crate::wheel`]): a choice reaches `05AD` as `02A3`, which plays
/// `0686`. Need Healing sends effect 40 (remote 8), Hello its callout 27
/// (remote 6), a configurable choice (`01EC`) callout 59 (remote 11), the
/// Voice Line slot pulses 0686's voice state 13; each a frame action or
/// state the clients get. Each choice raises its voice criteria on the
/// body, which its voice set answers with a line ([`crate::voice_lines`]):
/// Hello `0AEB` 0.3 s later, `01EC` the `0721` its map gives, both for every
/// player to hear. Need Healing's line hangs on the `0307` that `0686`'s
/// `STU_57692020` sends to every entity, which `05AD` rid15 takes with the
/// sender in #40 (the body's own here; the match gives it to every other
/// body): the sender is its own ally (rid16 `STU_AC829876`), so it raises
/// `0724` on itself for itself to hear (rid48/rid56, BotVoice.md). Nothing
/// in the path is unsupported.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn wheel_choices_run_the_callout_graph() {
    use orpheus_content::maps::GameMap;
    use orpheus_core::tick::FIRST_TICK;
    use orpheus_script::vm::StateKind;
    use orpheus_wire::{
        jam::codec::{Fields, Value as Field},
        messages::Message,
    };

    let root = orpheus_testkit::runtime_dir();
    let graphs =
        Arc::new(GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap());
    let pools = crate::pools::PoolCatalog::from_graphs(&graphs)
        .unwrap()
        .hero_pools(SOLDIER)
        .unwrap();
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let entity = 0xa000_0101;
    for (action, expected_node, criteria) in [
        (
            0x16c,
            Some(40),
            Some((0x0d80_0000_0000_0724, Some(vec![entity]))),
        ),
        (0x16b, Some(27), Some((0x0d80_0000_0000_0aeb, None))),
        (0x1ec, Some(59), Some((0x0d80_0000_0000_0721, None))),
        (0x16f, None, None),
    ] {
        let roots = orpheus_script::vm::player_roots(body, &graphs).unwrap();
        let health = pools.total();
        let variables = orpheus_script::vm::player_entity_variables(body, health, health);
        let mut world = PlayerWorld::new(
            Arc::clone(&graphs),
            0xa000_0100,
            entity,
            crate::scene::team_bit(0).unwrap(),
            PRACTICE_RANGE_MODE,
            SOLDIER,
        );
        world.position = GameMap::practice_range().spawn_point(0, 0).unwrap().0;
        world.vitals = Vitals::empty();
        world.pool_types_known = true;
        world.target_tags = body.target_tags.clone();
        let mut component = Component::start(
            Arc::clone(&graphs),
            world.entity,
            variables,
            roots,
            FIRST_TICK,
            &mut world,
            MAX_INIT_WORK,
        )
        .unwrap();
        component.advance_frame(FIRST_TICK, &mut world).unwrap();
        let mut choice = Fields::new();
        choice.insert(0x78, Field::Unsigned(0x0a90_0000_0000_0000 | action));
        choice.insert(0x80, Field::Bool(false));
        let (message, parameters) = crate::wheel::selection(&Message {
            id: crate::wheel::COMMUNICATION_ACTION,
            fields: choice,
        })
        .unwrap();
        assert_ne!(
            component
                .receive_message(0, message, parameters, &mut world)
                .unwrap(),
            0
        );
        for frame in 1..30 {
            component
                .advance_frame(FIRST_TICK + frame, &mut world)
                .unwrap();
        }
        // Every player hears the wheel's own lines (`0686` names all
        // players, `STU_BD2D717B`, as their audience); `0724` names #40.
        assert_eq!(
            world.take_criteria(),
            Vec::from_iter(criteria.map(|(identifier, audience)| RaisedCriteria {
                identifier,
                target: entity,
                audience,
            })),
            "{action:X}'s voice criteria"
        );
        let snapshot = component.snapshot().unwrap();
        if action == 0x16c {
            const NEED_HEALING: u64 = 0x0240_0000_0000_0307;
            assert!(
                world
                    .take_messages()
                    .iter()
                    .any(|message| message.message == NEED_HEALING
                        && message.target == orpheus_script::mode::EVERY_ENTITY),
                "0307 to every entity"
            );
            let wheel = snapshot
                .instances
                .values()
                .find(|instance| instance.graph == 0x05ad)
                .expect("05AD runs");
            assert_eq!(
                wheel.variables.get(&40),
                Some(&orpheus_script::wire::Value::Entity(world.entity)),
                "05AD rid15's sender"
            );
        }
        assert!(
            snapshot.diagnostics.is_empty(),
            "{:?}",
            snapshot.diagnostics
        );
        let (&callouts, instance) = snapshot
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == 0x0686)
            .expect("05AD plays 0686");
        assert_eq!(instance.parent, None);
        let ran: Vec<_> = snapshot
            .frame_actions
            .iter()
            .filter(|action| action.instance == callouts)
            .map(|action| action.node)
            .collect();
        match expected_node {
            Some(node) => assert_eq!(ran, [node], "{action:X}"),
            None => {
                assert!(ran.is_empty());
                assert!(matches!(
                    instance.states[&13].kind,
                    StateKind::Pulser { pulses: 1, .. }
                ));
            }
        }
    }
}

/// The wheel's spam limit runs on the server as `05AD` has it: each Hello
/// counts (`#4052e`), at three `#7416` holds and a fourth choice only shows
/// the refusal; one count goes every 4 s, after which a choice speaks
/// again. Choices 0.64 s apart (past `05AD` st6's 0.5 s).
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn the_wheel_refuses_a_fourth_choice_until_a_count_expires() {
    use orpheus_content::maps::GameMap;
    use orpheus_core::tick::FIRST_TICK;
    use orpheus_wire::{
        jam::codec::{Fields, Value as Field},
        messages::Message,
    };

    const HELLO: u64 = 0x0d80_0000_0000_0aeb;
    let root = orpheus_testkit::runtime_dir();
    let graphs =
        Arc::new(GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap());
    let health = crate::pools::PoolCatalog::from_graphs(&graphs)
        .unwrap()
        .hero_pools(SOLDIER)
        .unwrap()
        .total();
    let body = &graphs.bodies()[&0x0400_0000_0000_03cf];
    let mut world = PlayerWorld::new(
        Arc::clone(&graphs),
        0xa000_0100,
        0xa000_0101,
        crate::scene::team_bit(0).unwrap(),
        PRACTICE_RANGE_MODE,
        SOLDIER,
    );
    world.position = GameMap::practice_range().spawn_point(0, 0).unwrap().0;
    world.vitals = Vitals::empty();
    world.pool_types_known = true;
    world.target_tags = body.target_tags.clone();
    let mut component = Component::start(
        Arc::clone(&graphs),
        world.entity,
        orpheus_script::vm::player_entity_variables(body, health, health),
        orpheus_script::vm::player_roots(body, &graphs).unwrap(),
        FIRST_TICK,
        &mut world,
        MAX_INIT_WORK,
    )
    .unwrap();
    component.advance_frame(FIRST_TICK, &mut world).unwrap();
    let mut frame = FIRST_TICK;
    let mut hello = |component: &mut Component, world: &mut PlayerWorld, frames: u32| {
        let mut choice = Fields::new();
        choice.insert(0x78, Field::Unsigned(0x0a90_0000_0000_016b));
        choice.insert(0x80, Field::Bool(false));
        let (message, parameters) = crate::wheel::selection(&Message {
            id: crate::wheel::COMMUNICATION_ACTION,
            fields: choice,
        })
        .unwrap();
        component
            .receive_message(0, message, parameters, world)
            .unwrap();
        for _ in 0..frames {
            frame += 1;
            component.advance_frame(frame, world).unwrap();
        }
        world
            .take_criteria()
            .into_iter()
            .map(|raised| raised.identifier)
            .collect::<Vec<_>>()
    };
    for _ in 0..3 {
        assert_eq!(hello(&mut component, &mut world, 40), [HELLO]);
    }
    assert!(hello(&mut component, &mut world, 40).is_empty(), "refused");
    // The first count goes 4 s after the first choice (250 frames).
    assert!(
        hello(&mut component, &mut world, 160).is_empty(),
        "still refused"
    );
    assert_eq!(hello(&mut component, &mut world, 40), [HELLO]);
    let snapshot = component.snapshot().unwrap();
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
}

/// The mode body script `0C8E` (the Practice Range's and PvP teams'
/// `m_bodyScript`, [`ModeBody`]) on Soldier: 76: it is the body's last root
/// instance, takes the mode's `06FD` (3 s in the Range) and, once the body
/// dies, counts the respawn down in #384 (ChaseVar st1) with the HUD stacks
/// #6034 (st32) and #6036 (st33), all in the owner frames; the observers get
/// the countdown's remote state. On death it tells the mode its time (`08BB`,
/// st41) and, when the count ends, that it can respawn (`0685`). The body
/// has the game's filter bits and refuses unknown content, as debug builds
/// do: the hero's death path (`004B` st14 → the death layer `01CB`, whose
/// kill cam test reads the server-only `STU_68F68CD7`) runs to the end,
/// with the death message's camera parameters (`07A9`: the killer's
/// controller, whose body `01CB` rid224 tells of the kill).
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn a_dead_body_counts_its_respawn_down_in_the_mode_body_script() {
    use orpheus_content::{heroes::HeroCatalog, maps::GameMap};
    use orpheus_core::tick::FIRST_TICK;
    use orpheus_script::vitals::Death;
    use orpheus_wire::entity::GAME_MODE_ENTITY;

    const MODE_BODY: u32 = 0x0c8e;
    let root = orpheus_testkit::runtime_dir();
    let graphs =
        Arc::new(GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap());
    let pools = crate::pools::PoolCatalog::from_graphs(&graphs)
        .unwrap()
        .hero_pools(SOLDIER)
        .unwrap();
    let heroes = HeroCatalog::load(root.join("data/game_heroes_174.json")).unwrap();
    let spawn = OwnerBodySpawn {
        controller: 0xa000_0100,
        entity: 0xa000_0101,
        body_filter: crate::scene::body_filter(crate::scene::team_bit(0).unwrap(), false),
        slot: 0,
        frame: FIRST_TICK,
        pose: Movement {
            position: GameMap::practice_range().spawn_point(0, 0).unwrap().0,
            ..Movement::default()
        },
        quanta: RETAIL_QUANTA,
        mode_random: 0.5,
        ai: None,
        respawn_held: false,
    };
    let factory = PlayerBodyFactory::prepare(
        Arc::clone(&graphs),
        Arc::new(CodecTable::client()),
        PRACTICE_RANGE_MODE,
        orpheus_content::maps::PRACTICE_RANGE_RULESET,
        heroes.get(SOLDIER).unwrap(),
        Some(pools),
        false,
        BodyCombat {
            eyes: orpheus_content::heroes::Eyes {
                standing: 1.55,
                crouched: 1.085,
            },
        },
        Unknowns::Refuse,
        None,
        ModeBody {
            scripts: BTreeMap::from([(0, MODE_BODY)]),
            respawn_seconds: Some(3),
            team_sizes: Vec::new(),
        },
        spawn,
    )
    .unwrap();
    let mut runtime = factory.start(spawn).unwrap();
    let snapshot = runtime.snapshot().unwrap();
    let last_root = factory.roots.last().unwrap().id;
    let id = last_root + 1;
    assert_eq!(
        (
            snapshot.instances[&id].graph,
            snapshot.instances[&id].parent
        ),
        (MODE_BODY, None)
    );
    let projected = |runtime: &PlayerBodyRuntime, view| {
        let encoder = runtime.encoder();
        let projected = encoder.project(runtime.snapshot().unwrap(), view).unwrap();
        encoder.full(&projected).unwrap();
        projected
    };
    // `06FD` arrived (#2153 = 3 s): st2 runs st0, the switch on #478.
    let alive = projected(&runtime, View::Owner);
    assert_eq!(alive.instance_variable(id, 2153), Some(&Value::Integer(3)));
    assert!(alive.state(id, 2).is_some() && alive.state(id, 0).is_some());
    assert!(alive.state(id, 1).is_none());

    // The killer is an enemy body the match lists (as the Training Bots
    // are), with its controller: the death layer asks whether he still
    // exists (`01CB` rid43, `STU_E72F6348(#40)`) and tells his
    // controller's body of the kill (rid224).
    let killer = 0xa000_0201;
    let killer_controller = 0xa000_0200;
    runtime.set_target_candidates(&[TargetCandidate {
        entity: killer,
        team: Some(1),
        dead: false,
        controller: Some(killer_controller),
        player: None,
        ai: true,
        ai_control: None,
        feet: [0.0, 0.0, 6.0],
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: Some(Arc::new(BTreeSet::new())),
        health: None,
        hardpoints: None,
        spawned: Vec::new(),
    }]);
    let total = runtime.vitals().unwrap().total();
    runtime.vitals_mut().unwrap().damage(
        total,
        Death {
            time: 1.0,
            frame: FIRST_TICK,
            killer,
        },
    );
    runtime.health_changed().unwrap();
    let camera = runtime.death_camera(killer);
    assert_eq!(
        camera,
        Some(crate::combat::DeathCamera {
            killer_controller,
            position: spawn.pose.position,
        })
    );
    let (message, parameters) =
        crate::combat::death_message(killer, false, None, Default::default(), &[], camera);
    runtime.receive_message(0, message, parameters).unwrap();
    let mut to_killer = Vec::new();
    let mut to_mode = |runtime: &mut PlayerBodyRuntime, frame: u32| -> Vec<(u64, Variables)> {
        let (mode, others): (Vec<_>, Vec<_>) = runtime
            .take_messages()
            .into_iter()
            .partition(|message| message.target == GAME_MODE_ENTITY);
        to_killer.extend(
            others
                .into_iter()
                .filter(|message| message.target == killer)
                .map(|message| (frame, message.message, message.parameters)),
        );
        mode.into_iter()
            .map(|message| (message.message, message.parameters))
            .collect()
    };
    // Its time to the mode (08BB, st41 on team 0).
    assert_eq!(
        to_mode(&mut runtime, 0),
        [(
            0x0240_0000_0000_08bb,
            Variables::from([(0xb7, Value::Float(3.0))])
        )]
    );
    let dead = projected(&runtime, View::Owner);
    assert_eq!(dead.entity_variable(DEAD_FLAG), Some(&Value::Bool(true)));
    // #384 starts at the 3 s (#9575 = 06FD's seconds, no overtime, scale 1).
    assert_eq!(dead.instance_variable(id, 384), Some(&Value::Float(3.0)));
    for state in [0, 3, 1, 32, 33] {
        assert!(dead.state(id, state).is_some(), "owner st{state}");
    }
    // The countdown is remote state 0: the others see it too.
    assert!(projected(&runtime, View::Observer).state(id, 1).is_some());
    // The death layer passed its kill cam test (rid64:
    // `!valueOr(mode.id(9541), true) || STU_68F68CD7(owner) || #11485`):
    // no graph writes the mode's #9541, `STU_68F68CD7` has no value (as on
    // the client) and #11485 is unset, so it takes the kill cam switch
    // (st21), not the plain 3 s wait (st33). With the game's body filter
    // (`{any 1, all 1}`) st21 is true: it times the kill cam (#2441 =
    // min(10.5, 0.7 · 3) = 2.1 s). It shares the killer's controller (sync
    // var #21709).
    let snapshot = runtime.snapshot().unwrap();
    let (&layer_id, layer) = snapshot
        .instances
        .iter()
        .find(|(_, instance)| instance.graph == 0x01cb)
        .expect("the death layer");
    let active = |raw: u16| layer.states.get(&raw).is_some_and(|state| state.active);
    assert!(active(21) && !active(33));
    assert_eq!(
        dead.instance_variable(layer_id, 21709),
        Some(&Value::Entity(killer_controller))
    );
    assert!(
        matches!(layer.variables.get(&2441), Some(&Value::Float(seconds)) if (seconds - 2.1).abs() < 1e-6),
        "{:?}",
        layer.variables.get(&2441)
    );

    // The count runs at 1 a second from the death's frame: 3000 ms is past
    // after 188 frames of 16 ms (187 make 2992 ms), when it sends `0685`
    // (`0221` the body's controller, `STU_DB742151`: the client's possessor
    // of the body, KillFeed.md "Client tools pass (2026-11-16)"; `03D9`
    // #22241 = 0, no respawn camera).
    for frame in 1..188 {
        runtime
            .component
            .advance_frame(FIRST_TICK + frame, &mut runtime.world)
            .unwrap();
        assert!(to_mode(&mut runtime, frame).is_empty(), "frame {frame}");
    }
    let counting = projected(&runtime, View::Owner);
    assert!(counting.state(id, 1).is_some());
    runtime
        .component
        .advance_frame(FIRST_TICK + 188, &mut runtime.world)
        .unwrap();
    assert_eq!(
        to_mode(&mut runtime, 188),
        [(
            0x0240_0000_0000_0685,
            Variables::from([
                (0x221, Value::Entity(spawn.controller)),
                (0x3d9, Value::Float(0.0))
            ])
        )]
    );
    let done = projected(&runtime, View::Owner);
    assert!(done.state(id, 1).is_none() && done.state(id, 32).is_none());
    assert_eq!(done.instance_variable(id, 384), Some(&Value::Float(0.0)));
    // Two frames after st21 began (st125), rid224 tells the killer's
    // controller's body (`STU_0AC15F78(#21709)`) of the kill: `17D9` with
    // the death's tags (#1605) and the credited body (#40).
    assert_eq!(
        to_killer,
        [(
            2,
            0x0240_0000_0000_17d9,
            Variables::from([
                (0x3d, Value::Array(Vec::new())),
                (0xbd, Value::Entity(killer))
            ])
        )]
    );
    // The kill cam stays off in the Range: 0.4 s after the death the layer
    // aborts it (rid62: 2.6 s left < 5.4), so st2 never begins.
    let snapshot = runtime.snapshot().unwrap();
    let layer = &snapshot.instances[&layer_id];
    assert!(layer.states.get(&2).is_none_or(|state| !state.active));
    // The death layer ran its kill cam test and kept the body's script.
    assert!(
        snapshot.diagnostics.is_empty(),
        "{:?}",
        snapshot.diagnostics
    );
}

/// A hero body as the match starts it, with the game identifier the
/// server sends every new body (`002D.025` {`0029` 0}, which turns on
/// `01C7`'s #2426e) and the ultimate generation variables `generation`.
fn charge_body(hero: u64, generation: Option<Variables>) -> (PlayerBodyRuntime, OwnerBodySpawn) {
    hero_body(hero, generation, None)
}

/// [`charge_body`], possessed by an AI entity of the hero's AI definition
/// playing at `difficulty` when one is given.
fn hero_body(
    hero: u64,
    generation: Option<Variables>,
    difficulty: Option<u64>,
) -> (PlayerBodyRuntime, OwnerBodySpawn) {
    use orpheus_content::{heroes::HeroCatalog, maps::GameMap};
    use orpheus_core::tick::FIRST_TICK;

    let root = orpheus_testkit::runtime_dir();
    let graphs =
        Arc::new(GraphStore::load_runtime(root.join("data"), "game_graphs_174.json.gz").unwrap());
    let heroes = HeroCatalog::load(root.join("data/game_heroes_174.json")).unwrap();
    let pools = crate::pools::PoolCatalog::from_graphs(&graphs)
        .unwrap()
        .hero_pools(hero)
        .unwrap();
    let ai = difficulty.map(|difficulty| AiControl {
        entity: 0x8000_7001,
        definition: graphs.ai_definitions().definition(hero).unwrap(),
        difficulty,
    });
    let spawn = OwnerBodySpawn {
        controller: 0xa000_0100,
        entity: 0xa000_0101,
        body_filter: crate::scene::body_filter(crate::scene::team_bit(0).unwrap(), false),
        slot: 0,
        frame: FIRST_TICK,
        pose: Movement {
            position: GameMap::practice_range().spawn_point(0, 0).unwrap().0,
            ..Movement::default()
        },
        quanta: RETAIL_QUANTA,
        mode_random: 0.5,
        ai,
        respawn_held: false,
    };
    let factory = PlayerBodyFactory::prepare(
        graphs,
        Arc::new(CodecTable::client()),
        PRACTICE_RANGE_MODE,
        orpheus_content::maps::PRACTICE_RANGE_RULESET,
        heroes.get(hero).unwrap(),
        Some(pools),
        false,
        BodyCombat {
            eyes: orpheus_content::heroes::Eyes {
                standing: 1.55,
                crouched: 1.085,
            },
        },
        Unknowns::Tolerate,
        generation,
        ModeBody::default(),
        spawn,
    )
    .unwrap();
    let mut runtime = factory.start(spawn).unwrap();
    runtime
        .receive_message(
            0,
            0x0240_0000_0000_002d,
            Variables::from([(0x29, Value::Asset(0))]),
        )
        .unwrap();
    (runtime, spawn)
}

fn charge_of(runtime: &PlayerBodyRuntime) -> f64 {
    use orpheus_script::ultimate::ULT_CHARGE;
    match runtime.snapshot().unwrap().variables.get(&ULT_CHARGE) {
        Some(Value::Float(charge)) => *charge,
        other => panic!("charge {other:?}"),
    }
}

/// Run the body's first command frames after its spawn, `frames` × 16 ms.
fn run_frames(runtime: &mut PlayerBodyRuntime, spawn: OwnerBodySpawn, frames: u32) {
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    for frame in spawn.frame + 1..=spawn.frame + frames {
        runtime
            .advance(
                Command {
                    frame,
                    ..Command::default()
                },
                &mover,
                BodyStep::Posed,
            )
            .unwrap();
    }
}

/// Graph `0B8B` charges the ultimate: its server-only loop adds 5 points
/// after each 1 s Wait once `01C7` turned #2426e on (from `002D.025`); the
/// damage the body dealt (`0012.025`) to a hostile body adds its amount,
/// to an ally or to itself nothing (rid28, `STU_A58C1E63`); healing done
/// (`0044.025`) adds its amount; a direct grant (`00EB.025`, Mercy's staff
/// `006B` rid189 among eight senders) adds its amount. Nothing in Rust adds
/// to it.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn the_charge_graph_charges_soldier_s_ultimate() {
    use orpheus_script::{ultimate::dealt_message, weapons::DamageTags};

    const ENEMY: u32 = 0xa000_0301;
    const ALLY: u32 = 0xa000_0401;
    /// Training Bot (`02ED.06C`), which `0B8B` rid45 admits.
    const TRAINING_BOT_TAG: u64 = 0x06c0_0000_0000_02ed;
    let (mut runtime, spawn) = charge_body(SOLDIER, None);
    assert_eq!(
        runtime.snapshot().unwrap().variables.get(&2426),
        Some(&Value::Bool(true))
    );
    assert_eq!(charge_of(&runtime), 0.0);
    // 130 frames are 2.08 s: two of the loop's Waits.
    run_frames(&mut runtime, spawn, 130);
    assert_eq!(charge_of(&runtime), 10.0);

    let body = |entity, team| TargetCandidate {
        entity,
        team: Some(team),
        dead: false,
        controller: None,
        player: None,
        ai: true,
        ai_control: None,
        feet: [0.0; 3],
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        hardpoints: None,
        spawned: Vec::new(),
    };
    runtime.set_target_candidates(&[body(ENEMY, 1), body(ALLY, 0)]);
    let deal = |runtime: &mut PlayerBodyRuntime, target, amount, heal| {
        runtime.set_victim_tags(target, BTreeSet::from([TRAINING_BOT_TAG]));
        let (message, parameters) = dealt_message(target, amount, &DamageTags::NONE, heal);
        runtime
            .receive_message(spawn.entity, message, parameters)
            .unwrap();
    };
    deal(&mut runtime, ENEMY, 100.0, false);
    assert_eq!(charge_of(&runtime), 110.0, "damage to an enemy");
    deal(&mut runtime, ALLY, 100.0, false);
    deal(&mut runtime, spawn.entity, 100.0, false);
    assert_eq!(charge_of(&runtime), 110.0, "damage to an ally or himself");
    deal(&mut runtime, ALLY, 50.0, true);
    assert_eq!(charge_of(&runtime), 160.0, "healing done");
    runtime
        .receive_message(
            spawn.entity,
            0x0240_0000_0000_00eb,
            Variables::from([(0x2a, Value::Float(120.0))]),
        )
        .unwrap();
    assert_eq!(charge_of(&runtime), 280.0, "a direct grant");
    // The meter stops at the cost.
    deal(&mut runtime, ENEMY, 5000.0, false);
    assert_eq!(charge_of(&runtime), 2310.0);
}

/// Doomfist's passive The Best Defense (`0DDD`, `vm/play.rs`
/// `BEST_DEFENSE`): each damage his abilities deal to an enemy hero
/// (`0012.025` with the slam's tag `245A`) plays `0DF3` on him, whose st0
/// is a temporary shields pool of 30 named `249D` (component 51 shields
/// slot, byte 16 set); grants stop at the 150 cap (`0DDD` rid8, `0DF3`
/// rid3, with the named pool read as the shields' sum). A second after the
/// last grant `0DDD` st1 drains the named pools 3 a second; a pool that runs
/// out goes, its `0DF3` instance exiting. Damage takes the temporary pool
/// first, and heals never refill it.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn doomfists_passive_grants_temporary_shields_that_drain() {
    use orpheus_script::{
        ultimate::dealt_message,
        vitals::{Death, PoolKind},
        weapons::DamageTags,
    };
    use orpheus_wire::entity::{ComponentValue, POOLS, SHIELDS_POOL};

    const DOOMFIST: u64 = 0x02e0_0000_0000_012f;
    const ENEMY: u32 = 0xa000_0301;
    /// Seismic Slam's damage tag (`0B27` st42's first).
    const SLAM: u64 = 0x0d80_0000_0000_245a;
    const BEST_DEFENSE: u32 = 0x0ddd;
    let (mut runtime, spawn) = charge_body(DOOMFIST, None);
    runtime.set_target_candidates(&[TargetCandidate {
        entity: ENEMY,
        team: Some(1),
        dead: false,
        controller: None,
        player: None,
        ai: false,
        ai_control: None,
        feet: [0.0; 3],
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        spawned: Vec::new(),
    }]);
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let mut frame = spawn.frame;
    let mut run = |runtime: &mut PlayerBodyRuntime, frames: u32| {
        for _ in 0..frames {
            frame += 1;
            runtime
                .advance(
                    Command {
                        frame,
                        ..Command::default()
                    },
                    &mover,
                    BodyStep::Posed,
                )
                .unwrap();
        }
    };
    let deal = |runtime: &mut PlayerBodyRuntime| {
        let tags: DamageTags = [SLAM].into_iter().collect();
        let (message, parameters) = dealt_message(ENEMY, 50.0, &tags, false);
        runtime
            .receive_message(spawn.entity, message, parameters)
            .unwrap();
    };
    let shields =
        |runtime: &PlayerBodyRuntime| runtime.world.vitals.amount(Some(PoolKind::Shields), false);
    let grants = |runtime: &PlayerBodyRuntime| {
        let snapshot = runtime.snapshot().unwrap();
        snapshot
            .instances
            .values()
            .filter(|instance| instance.graph == 0x0df3)
            .count()
    };
    run(&mut runtime, 10);
    assert_eq!(shields(&runtime), 0.0);
    deal(&mut runtime);
    run(&mut runtime, 1);
    assert_eq!(shields(&runtime), 30.0);
    assert_eq!(grants(&runtime), 1);
    let record = match &runtime.world.vitals.component().fields[SHIELDS_POOL * POOLS] {
        Some(ComponentValue::Blob20(record)) => *record,
        other => panic!("{other:?}"),
    };
    assert_eq!(record[8..12], 30f32.to_le_bytes());
    assert_eq!(record[16..], [1, 1, 0, 1]);
    // Five more grants reach the cap; a seventh grants nothing.
    for _ in 0..6 {
        deal(&mut runtime);
        run(&mut runtime, 1);
    }
    assert_eq!(shields(&runtime), 150.0);
    assert_eq!(grants(&runtime), 5);
    // Damage takes them first; a heal doesn't refill them.
    let hit = Death {
        time: 0.0,
        frame: spawn.frame,
        killer: ENEMY,
    };
    runtime.world.vitals.damage(40.0, hit);
    assert_eq!(shields(&runtime), 110.0);
    assert_eq!(
        runtime.world.vitals.amount(Some(PoolKind::Health), false),
        250.0
    );
    runtime.world.vitals.heal(100.0);
    assert_eq!(shields(&runtime), 110.0);
    // No drain within a second of the last grant (`0DDD` st0, 1 s).
    run(&mut runtime, 40);
    assert_eq!(shields(&runtime), 110.0);
    run(&mut runtime, 63);
    let draining = shields(&runtime);
    assert!(draining < 110.0, "{draining}");
    // Then 3 a second: 125 frames are 2 s.
    run(&mut runtime, 125);
    let left = shields(&runtime);
    assert!((draining - left - 6.0).abs() < 1e-6, "{draining} {left}");
    assert!(
        runtime
            .snapshot()
            .unwrap()
            .instances
            .values()
            .any(|instance| instance.graph == BEST_DEFENSE)
    );
    // All of it drains away, and the grants' instances exit.
    run(&mut runtime, 63 * 40);
    assert_eq!(shields(&runtime), 0.0);
    assert_eq!(grants(&runtime), 0);
}

/// D.Va's pilot (`040A`, a cost of 319.2) charges by `0B8B`'s loop as
/// every hero does: kjs saw this loop start on her bodies only, and the
/// server's own passive stood down for it; here it is the only passive.
/// The ruleset key `32E0` (#13024e, `0B8B` rid66 from the Entry rid69)
/// fills the meter at spawn.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn the_charge_graph_charges_d_va_and_fills_on_its_ruleset_key() {
    const DVA: u64 = 0x02e0_0000_0000_007a;
    let (mut runtime, spawn) = charge_body(DVA, None);
    run_frames(&mut runtime, spawn, 130);
    assert_eq!(charge_of(&runtime), 10.0);

    let full = Variables::from([(13024, Value::Bool(true))]);
    let (runtime, _) = charge_body(SOLDIER, Some(full));
    assert_eq!(charge_of(&runtime), 2310.0);
}

/// Ana's Nano Boost through her own graph `0B9E`, as the user presses it:
/// with a full meter (the ruleset key `32E0`), ABILITY_3 (command bit 0x20,
/// logical button 37) begins st0's ability; st6 searches a 10° cone
/// (`10 × #9602e`, the hero setting NANO BOOST SENSITIVITY at its default
/// 1, which her body starts with: `orpheus_content::hero_settings`) 40 m
/// along her aim from her eye for the ally nearest her crosshair; rid80
/// takes him into `#8070`, and after st11's 0.13 s st12 puts the aura
/// `0269` on him (its graph `0B9F` is
/// `nano_boost_halves_the_damage_a_bot_takes_and_boosts_what_it_deals`).
/// The cone holds two allies: the one 0.3 m beside her aim at 20 m covers
/// her crosshair and wins over the nearer one 0.6 m aside at 10 m (the
/// scoring `m_84E5DE4E`: the screen distance to the crosshair, then 1 % of
/// the distance over 40 m; the limit `m_C39691D9` 1); a third 2 m aside at
/// 10 m (11°) is out of the 5° half-angle, and an enemy straight ahead never
/// counts.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn anas_ultimate_puts_nano_boost_on_the_ally_in_her_sights() {
    use orpheus_content::heroes::ANA;
    use orpheus_script::vm::AuraChange;

    const NANO_BOOST: u64 = 0x04a0_0000_0000_0269;
    const ABILITY_3: u32 = 0x20;
    const FAR_ALLY: u32 = 0xa000_0201;
    const NEAR_ALLY: u32 = 0xa000_0301;
    const WIDE_ALLY: u32 = 0xa000_0501;
    const ENEMY: u32 = 0xa000_0401;
    let full = Variables::from([(13024, Value::Bool(true))]);
    let (mut runtime, spawn) = hero_body(ANA, Some(full), None);
    let [x, y, z] = spawn.pose.position;
    // She looks along +z (yaw 0, level).
    runtime.set_target_candidates(&[
        standing(spawn.entity, 0, spawn.controller, spawn.pose.position),
        standing(ENEMY, 1, 0xa000_0400, [x, y, z + 5.0]),
        standing(WIDE_ALLY, 0, 0xa000_0500, [x + 2.0, y, z + 10.0]),
        standing(NEAR_ALLY, 0, 0xa000_0300, [x + 0.6, y, z + 10.0]),
        standing(FAR_ALLY, 0, 0xa000_0200, [x + 0.3, y, z + 20.0]),
    ]);
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let mut auras = Vec::new();
    // One press at frame 30, then 60 frames (0.96 s) for st11's 0.13 s.
    for frame in spawn.frame + 1..=spawn.frame + 90 {
        let buttons = if frame == spawn.frame + 30 {
            ABILITY_3
        } else {
            0
        };
        runtime
            .advance(
                Command {
                    frame,
                    buttons,
                    ..Command::default()
                },
                &mover,
                BodyStep::Posed,
            )
            .unwrap();
        auras.extend(runtime.take_aura_changes());
    }
    let applied: Vec<_> = auras
        .iter()
        .filter_map(|change| match change {
            AuraChange::Apply { target, aura, .. } => Some((*target, *aura)),
            AuraChange::Remove { .. } => None,
        })
        .collect();
    assert_eq!(
        applied,
        [(FAR_ALLY, NANO_BOOST)],
        "assumptions: {:#?}",
        runtime.snapshot().unwrap().assumptions
    );
}

/// A standing body in a body's target search.
fn standing(entity: u32, team: u8, controller: u32, feet: [f64; 3]) -> TargetCandidate {
    TargetCandidate {
        entity,
        team: Some(team),
        dead: false,
        controller: Some(controller),
        player: Some(controller),
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
    }
}

/// Zenyatta's Orb of Harmony through his own graph `02F3`, as Ana's Nano
/// Boost: ABILITY_1 (command bit 0x08, logical button 35, as
/// `STU_D3685C04` is false) goes down in st1, st14 searches a 10° cone
/// (`10 × #9596e`, ORB OF HARMONY SENSITIVITY at its default 1, which his
/// body starts with) for the ally nearest his crosshair, and rid23 makes
/// him the orb's target `#1606`, which st9's chase flies the orb to. With
/// the setting null the cone was `10 × null` and found nobody, so the
/// server's orb stayed home while the client's flew.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn zenyattas_harmony_orb_goes_to_the_ally_in_his_sights() {
    const ZENYATTA: u64 = 0x02e0_0000_0000_0020;
    const ABILITY_1: u32 = 0x08;
    const ALLY: u32 = 0xa000_0201;
    const WIDE_ALLY: u32 = 0xa000_0301;
    let (mut runtime, spawn) = charge_body(ZENYATTA, None);
    let [x, y, z] = spawn.pose.position;
    // He looks along +z; the wide ally is 11° off his aim, out of the 5°
    // half-angle.
    runtime.set_target_candidates(&[
        standing(spawn.entity, 0, spawn.controller, spawn.pose.position),
        standing(WIDE_ALLY, 0, 0xa000_0300, [x + 2.0, y, z + 10.0]),
        standing(ALLY, 0, 0xa000_0200, [x + 0.3, y, z + 15.0]),
    ]);
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    for frame in spawn.frame + 1..=spawn.frame + 60 {
        let buttons = if frame == spawn.frame + 30 {
            ABILITY_1
        } else {
            0
        };
        runtime
            .advance(
                Command {
                    frame,
                    buttons,
                    ..Command::default()
                },
                &mover,
                BodyStep::Posed,
            )
            .unwrap();
    }
    let snapshot = runtime.snapshot().unwrap();
    let harmony = snapshot
        .instances
        .values()
        .find(|instance| instance.graph == 0x02f3)
        .unwrap();
    assert_eq!(
        harmony.variables.get(&1606),
        Some(&Value::Entity(ALLY)),
        "assumptions: {:#?}",
        snapshot.assumptions
    );
}

/// Mercy's TOGGLE GUARDIAN ANGEL (`#7780e`, default on) picks her Guardian
/// Angel ability as her graph `006E` starts: st23 (`#7780e ||` an AI
/// possessor) takes its true branch, st22, the ability a press of
/// ABILITY_1 toggles (`m_A5F1A77B`), as her client does at the default.
/// With the setting off (or null, as the server had it before) st23 takes
/// st19, the ability held by the button (`m_A368407D`; Soldier: 76's `0255`
/// st1 puts the sprint button there when HOLD TO SPRINT is on). A player who
/// sent it off (21604) gets it as his body starts, as `WorldDelivery` gives
/// it to a new body: st19, as his client.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn mercys_guardian_angel_toggles_or_holds_by_her_setting() {
    use orpheus_content::hero_settings::HeroSetting;

    const MERCY: u64 = 0x02e0_0000_0000_0004;
    const TOGGLE_GUARDIAN_ANGEL: u64 = 0x0d80_0000_0000_1e64;
    let guardian_angel = |runtime: &PlayerBodyRuntime| {
        let snapshot = runtime.snapshot().unwrap();
        let instance = snapshot
            .instances
            .values()
            .find(|instance| instance.graph == 0x006e)
            .unwrap();
        let active = |raw| instance.states.get(&raw).is_some_and(|state| state.active);
        let switch = instance
            .states
            .get(&23)
            .filter(|state| state.active)
            .map(|state| state.kind);
        (switch, active(22), active(19))
    };
    let (mut runtime, spawn) = charge_body(MERCY, None);
    run_frames(&mut runtime, spawn, 2);
    assert_eq!(
        guardian_angel(&runtime),
        (
            Some(orpheus_script::vm::StateKind::BooleanSwitch(true)),
            true,
            false
        )
    );

    let mut settings = HeroSettings::default();
    settings.replace(&[HeroSetting {
        hero: MERCY,
        setting: TOGGLE_GUARDIAN_ANGEL,
        value: 0,
    }]);
    let (mut runtime, spawn) = charge_body(MERCY, None);
    runtime
        .set_entity_variables(&hero_setting_variables(&settings, MERCY))
        .unwrap();
    run_frames(&mut runtime, spawn, 2);
    assert_eq!(
        guardian_angel(&runtime),
        (
            Some(orpheus_script::vm::StateKind::BooleanSwitch(false)),
            false,
            true
        )
    );
}

/// The Biotic Field's aura `025F` heals the body it is on through the
/// body's own graph `0B3C`: st0 ModifyHealth changes health over time,
/// `m_amount` `#420 * #567` (35 × 5 = 175) every `m_95F1FB0B`
/// `#567 - 0.050000004` seconds (4.95), each tick its share of the time
/// since the last (`vm/health.rs`), dealt by the body the field's owner
/// possesses (`STU_0AC15F78(STU_F76C486C(#40))`). The field keeps the aura
/// on for 312 ticks (`spawned.rs` `the_field_runs_its_graph_from_aura_to_removal`):
/// 312 × 16 ms = 4.992 s, so 175 × 4.992 / 4.95 is credited.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn the_biotic_field_aura_heals_the_body_through_its_own_graph() {
    use orpheus_script::vm::{AuraChange, AuraSource};

    const FIELD: u32 = 0xa008_0000;
    let (mut runtime, spawn) = charge_body(SOLDIER, None);
    runtime.set_target_candidates(&[TargetCandidate {
        entity: spawn.entity,
        team: Some(0),
        dead: false,
        controller: Some(spawn.controller),
        player: Some(spawn.controller),
        ai: false,
        ai_control: None,
        feet: [0.0; 3],
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        hardpoints: None,
        spawned: vec![orpheus_script::vm::OwnedEntity {
            entity: FIELD,
            projectile: false,
        }],
    }]);
    let source = AuraSource {
        entity: FIELD,
        instance: 1,
        state: 7,
        activation: 1,
    };
    runtime
        .change_aura(&AuraChange::Apply {
            source: Some(source),
            target: spawn.entity,
            aura: 0x04a0_0000_0000_025f,
            instigator: FIELD,
            creator: FIELD,
            parameters: vec![
                (0x0a40_0000_0000_002a, Value::Float(35.0)),
                (0x0a40_0000_0000_002b, Value::Integer(5)),
            ],
        })
        .unwrap();
    run_frames(&mut runtime, spawn, 312);
    runtime
        .change_aura(&AuraChange::Remove {
            source,
            target: spawn.entity,
        })
        .unwrap();
    let changes = runtime.world.take_health_changes();
    assert!(!changes.is_empty());
    for change in &changes {
        assert!(change.heal && change.target == spawn.entity, "{change:?}");
        assert_eq!(change.instigator, spawn.entity, "the owner's body heals");
    }
    let healed: f64 = changes.iter().map(|change| change.amount).sum();
    // The expression's float arithmetic is f32's.
    let period = f64::from(5.0_f32 - 0.050_000_004_f32);
    let expected = 175.0 * 4.992 / period;
    assert!(
        (healed - expected).abs() < 1e-6,
        "healed {healed}, not {expected}"
    );
}

/// Moira's healing Biotic Orb puts `02A1.053` (`002A` = 65) on a hurt ally,
/// the orb its instigator; the ally's body runs the aura's graph `1102`,
/// whose ModifyHealth heals it by `#499` = 65 a second, dealt by Moira
/// (`STU_2B2E2F6D(#40, 0)`, the orb's shooter, read from her listed orb)
/// with the orb as its source (`m_7B93233C` = `#40`), which counts it
/// against its pool. 125 frames (2 s) heal 130.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn the_healing_orbs_aura_heals_an_ally_for_its_thrower() {
    use orpheus_script::vm::{AuraChange, AuraSource, OwnedEntity};

    const MOIRA: u32 = 0xa000_0201;
    const ORB: u32 = 0xa00f_0000;
    let (mut runtime, spawn) = charge_body(SOLDIER, None);
    let body = |entity: u32, controller: u32, spawned| TargetCandidate {
        entity,
        team: Some(0),
        dead: false,
        controller: Some(controller),
        player: Some(controller),
        ai: false,
        ai_control: None,
        feet: [0.0; 3],
        height: 1.8,
        radius: 0.4,
        yaw: 0.0,
        tags: None,
        health: None,
        hardpoints: None,
        spawned,
    };
    runtime.set_target_candidates(&[
        body(spawn.entity, spawn.controller, Vec::new()),
        body(
            MOIRA,
            MOIRA & !0xff,
            vec![OwnedEntity {
                entity: ORB,
                projectile: true,
            }],
        ),
    ]);
    let source = AuraSource {
        entity: ORB,
        instance: 1,
        state: 1,
        activation: 1,
    };
    runtime
        .change_aura(&AuraChange::Apply {
            source: Some(source),
            target: spawn.entity,
            aura: 0x04a0_0000_0000_02a1,
            instigator: ORB,
            creator: ORB,
            parameters: vec![(0x0a40_0000_0000_002a, Value::Integer(65))],
        })
        .unwrap();
    run_frames(&mut runtime, spawn, 125);
    runtime
        .change_aura(&AuraChange::Remove {
            source,
            target: spawn.entity,
        })
        .unwrap();
    let changes = runtime.world.take_health_changes();
    assert!(!changes.is_empty());
    for change in &changes {
        assert_eq!(
            (change.heal, change.target, change.instigator, change.source),
            (true, spawn.entity, MOIRA, Some(ORB)),
            "{change:?}"
        );
    }
    let healed: f64 = changes.iter().map(|change| change.amount).sum();
    assert!((healed - 130.0).abs() < 1e-9, "healed {healed}");
}

/// An AI hero's weapons deal his AI profile's damage, as their graphs are
/// written: Soldier: 76's volleys (`0254`) amount to
/// `STU_D3EB8D87(STU_DB742151(owner)) ? STU_4049CF77(parameter, fallback) :
/// player damage`. His AI entity (`11F4`, which declares component 92)
/// possesses an AI's body, so the parameter is read from the profile of
/// the room's difficulty (`game_ai_definitions_174.json`: `11F4`'s `195E`,
/// `195F` and `1960` profiles): the rifle (st10) `1961` 4, 10 and 20 for the
/// player's 20 (#476); the Helix Rocket (st16) `1995` 8, 20 and 40 direct
/// for the player's 40, and `1996` 16, 40 and 80 in its blast for the
/// player's 80. A human's body (no AI entity) deals the player's.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn an_ai_hero_deals_his_difficulty_profile_s_damage() {
    use orpheus_script::{
        ai_definitions::{EASY, HARD, MEDIUM},
        expr::EntityRelation,
    };

    const WEAPON: u32 = 0x0254;
    const RIFLE: u16 = 10;
    const HELIX: u16 = 16;
    let damage = |difficulty: Option<u64>| {
        let (mut runtime, spawn) = hero_body(SOLDIER, None, difficulty);
        let snapshot = runtime.snapshot().unwrap();
        assert!(
            snapshot.diagnostics.is_empty(),
            "{:?}",
            snapshot.diagnostics
        );
        let instance = snapshot
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == WEAPON)
            .map(|(&id, _)| id)
            .unwrap();
        let graphs = Arc::clone(&runtime.world.graphs);
        let state = |raw: u16, field: &str| {
            graphs
                .graph(WEAPON)
                .unwrap()
                .state(usize::from(raw))
                .unwrap()
                .field(field)
                .unwrap()
                .clone()
        };
        let mut hit = |raw: u16, config: &Json| {
            let PlayerBodyRuntime {
                component, world, ..
            } = &mut runtime;
            HitDefinition::from_authored(config, |value| {
                component.evaluate_state_config(instance, raw, value, world)
            })
            .unwrap()
            .unwrap()
            .amount
        };
        let rifle = hit(RIFLE, &state(RIFLE, "m_6396149F"));
        let direct = hit(HELIX, &state(HELIX, "m_6396149F"));
        let blast = hit(HELIX, &state(HELIX, "m_D2D11CE9"));
        // The body's primary possessor: the AI entity, else the controller.
        let site = Site {
            entity: spawn.entity,
            instance,
            graph: WEAPON,
            node: None,
        };
        assert_eq!(
            runtime
                .world
                .related_entity(site, EntityRelation::Controller, spawn.entity),
            Some(spawn.ai.map_or(spawn.controller, |ai| ai.entity))
        );
        [rifle, direct, blast]
    };
    assert_eq!(damage(None), [20.0, 40.0, 80.0]);
    assert_eq!(damage(Some(EASY)), [4.0, 8.0, 16.0]);
    assert_eq!(damage(Some(MEDIUM)), [10.0, 20.0, 40.0]);
    assert_eq!(damage(Some(HARD)), [20.0, 40.0, 80.0]);
}

/// Every hero's `0033` asks a health pack for its heal: while the body is
/// alive and hurt (st10), st11 (`STU_87621906`) finds the pickup whose
/// sphere touches it (`orpheus_script` `vm/pickups.rs`) and rid23 sends it
/// `0643.025`, again every 0.5 s (st12) while it still touches one. A body
/// at full health, or one the sphere doesn't reach, asks nothing.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn a_hurt_body_on_a_health_pack_asks_it_for_its_heal() {
    use orpheus_content::heroes::SOLDIER;
    use orpheus_script::{vitals::Death, vm::Pickup};

    const PACK: u32 = 0x8000_00b9;
    const TOUCH: u64 = 0x0240_0000_0000_0643;
    let (mut runtime, spawn) = hero_body(SOLDIER, None, None);
    let [x, y, z] = spawn.pose.position;
    runtime.set_target_candidates(&[standing(
        spawn.entity,
        0,
        spawn.controller,
        spawn.pose.position,
    )]);
    let pack = |dx: f64| Pickup {
        entity: PACK,
        kind: 0,
        centre: [x + dx, y + 0.5, z],
        radius: 0.7,
        team: -1,
    };
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let mut frame = spawn.frame;
    // The frames on which the body asked the pack.
    let mut run = |runtime: &mut PlayerBodyRuntime, frames: u32| {
        let mut asked = Vec::new();
        for _ in 0..frames {
            frame += 1;
            runtime
                .advance(
                    Command {
                        frame,
                        ..Command::default()
                    },
                    &mover,
                    BodyStep::Posed,
                )
                .unwrap();
            if runtime
                .take_messages()
                .iter()
                .any(|message| message.target == PACK && message.message == TOUCH)
            {
                asked.push(frame - spawn.frame);
            }
        }
        asked
    };
    runtime.set_pickups(&[pack(0.0)]);
    assert_eq!(run(&mut runtime, 10), [] as [u32; 0], "at full health");
    runtime.vitals_mut().unwrap().damage(
        50.0,
        Death {
            time: 1.0,
            frame: spawn.frame,
            killer: spawn.entity,
        },
    );
    runtime.health_changed().unwrap();
    runtime.set_pickups(&[pack(1.5)]);
    assert_eq!(run(&mut runtime, 10), [] as [u32; 0], "out of reach");
    runtime.set_pickups(&[pack(0.5)]);
    // The first ask at once, the next 0.5 s (32 frames: the first after
    // 500 ms) later.
    let asked = run(&mut runtime, 40);
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(asked[1] - asked[0], 32, "{asked:?}");
    let snapshot = runtime.snapshot().unwrap();
    assert!(
        !snapshot
            .assumptions
            .iter()
            .any(|item| item.class == "STU_87621906"),
        "{:?}",
        snapshot.assumptions
    );
}

/// Soldier: 76's Tactical Visor aims his rifle on the server as on his
/// client: with a full meter, ABILITY_3 begins `0259`'s ultimate, whose
/// search st7 (a 45° cone, 45 m, hostile heroes and bots) puts the enemy
/// 11° off his aim in #2337e, and the rifle's volley (`0254` st10) leaves
/// along its launch's `m_E2FD6F04`, `norm(STU_FC99C77E(#2337e[0],
/// 0DC.0001) - STU_21A13145(owner()))`: from his eye to the enemy's chest
/// hardpoint (the Training Bot's, 1.32 m up its axis). Before, the server
/// shot along his view, past the enemy, and the visor hit nothing.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn tactical_visor_shots_leave_toward_the_target_s_chest() {
    const ABILITY_3: u32 = 0x20;
    const ENEMY: u32 = 0xa000_0401;
    const TRAINING_BOT: u64 = 0x0400_0000_0000_1403;
    const CHEST: u64 = 0x0dc0_0000_0000_0001;
    let hitboxes = crate::hitboxes::HitboxCatalog::load(
        orpheus_testkit::runtime_dir().join("data/game_hitboxes_174.json"),
    )
    .unwrap();
    let hardpoints = hitboxes.hardpoints(TRAINING_BOT).unwrap();
    let full = Variables::from([(13024, Value::Bool(true))]);
    let (mut runtime, spawn) = hero_body(SOLDIER, Some(full), None);
    let [x, y, z] = spawn.pose.position;
    let feet = [x + 2.0, y, z + 10.0];
    let enemy = TargetCandidate {
        ai: true,
        player: None,
        hardpoints: Some(Arc::clone(&hardpoints)),
        ..standing(ENEMY, 1, 0xa000_0400, feet)
    };
    runtime.set_target_candidates(&[
        standing(spawn.entity, 0, spawn.controller, spawn.pose.position),
        enemy,
    ]);
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let mut directions = Vec::new();
    // The ultimate at frame 30, then fire from frame 91 (he looks along
    // +z, level).
    for frame in spawn.frame + 1..=spawn.frame + 120 {
        let buttons = if frame == spawn.frame + 30 {
            ABILITY_3
        } else if frame > spawn.frame + 90 {
            orpheus_wire::commands::FIRE
        } else {
            0
        };
        let command = Command {
            frame,
            buttons,
            ..Command::default()
        };
        runtime.advance(command, &mover, BodyStep::Posed).unwrap();
        for shot in runtime.take_shots(Some(command), &mover) {
            if let PosedShot::Hitscan(shot) = shot {
                directions.push((shot.origin, shot.direction));
            }
        }
    }
    let snapshot = runtime.snapshot().unwrap();
    assert_eq!(
        snapshot.variables.get(&2337),
        Some(&Value::Array(vec![Value::Entity(ENEMY)]))
    );
    let [cx, cy, cz] = hardpoints[&CHEST];
    assert!(!directions.is_empty());
    for (origin, direction) in directions {
        let to: [f64; 3] =
            std::array::from_fn(|k| [feet[0] + cx, feet[1] + cy, feet[2] + cz][k] - origin[k]);
        let length = to.iter().map(|part| part * part).sum::<f64>().sqrt();
        for k in 0..3 {
            assert!(
                (direction[k] - to[k] / length).abs() < 1e-5,
                "{direction:?} toward {to:?}"
            );
        }
    }
}
