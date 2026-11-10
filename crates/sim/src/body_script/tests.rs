use super::*;
use crate::instance::TOURNAMENT_QUANTA;
use orpheus_physics::mover::sample_movement_flags;
use orpheus_physics::{
    collision::index::TriangleIndex, contact::world::ContactWorld, mover::settings::MoverSettings,
};
use orpheus_script::{
    vm::{Site, TargetTagEffect, World},
    wire::Value,
};

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

    let ai = json!({"$":"STU_DB742151","m_entity":owner,"m_EE729DCB":0});
    assert_eq!(evaluate(&mut runtime, &ai), Ok(Value::Entity(0)));
    let path = json!({"$":"STU_D3EB8D87","m_00903303":ai,"m_EE729DCB":0});
    assert_eq!(evaluate(&mut runtime, &path), Ok(Value::Bool(false)));
    let other_path =
        json!({"$":"STU_D3EB8D87","m_00903303":{"$":"STU_DB742151","m_entity":dynamic}});
    assert_eq!(
        evaluate(&mut runtime, &other_path),
        Err(EvalError::UnsupportedConfig("STU_DB742151".into()))
    );
    let branch = json!({"$":"STU_A77AC2AC","m_EE729DCB":0});
    assert_eq!(evaluate(&mut runtime, &branch), Ok(Value::Null));
    // Null in every mode (main 184ff56, as in Python).
    runtime.world.mode = 0;
    assert_eq!(evaluate(&mut runtime, &branch), Ok(Value::Null));
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

    assert_eq!(read(world, authored), Some(true));
    assert!(world.set_target_tag_effect(site, mask_authored, true));
    assert_eq!(read(world, authored), Some(false));
    assert!(world.set_target_tag_effect(site, mask_authored, false));
    assert_eq!(read(world, authored), Some(true));

    assert_eq!(read(world, tag), Some(false));
    for _ in 0..2 {
        assert!(world.set_target_tag_effect(site, grant, true));
    }
    assert_eq!(read(world, tag), Some(true));
    for _ in 0..2 {
        assert!(world.set_target_tag_effect(site, suppress, true));
    }
    assert_eq!(read(world, tag), Some(false));
    assert!(!world.set_target_tag_effect(site, grant, true));
    assert!(world.set_target_tag_effect(site, grant, false));
    assert!(world.set_target_tag_effect(site, suppress, false));
    assert_eq!(read(world, tag), Some(false));
    assert!(world.set_target_tag_effect(site, suppress, false));
    assert_eq!(read(world, tag), Some(true));
    assert!(world.set_target_tag_effect(site, grant, false));
    assert_eq!(read(world, tag), Some(false));
    assert!(!world.set_target_tag_effect(site, grant, false));
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
    for entity in [spawn.entity + 1, 0] {
        runtime
            .component
            .set_entity_variable(400, Value::Entity(entity), &mut runtime.world)
            .unwrap();
        assert_eq!(
            evaluate(&mut runtime, &current),
            Err(EvalError::UnsupportedConfig("STU_32E6E4D8".into()))
        );
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
        for class in [current, maximum] {
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
    let mut named = query(current, 0);
    named["m_7BE02F89"] = json!({"$":"STUConfigVarIdentifier","m_identifier":"0x01"});
    let mut foreign = query(current, 0);
    foreign["m_entity"] = json!({"$":"STU_F76C486C","m_D63CBEC2":{"$":"STU_B5A0CAF0"}});
    for config in [named, query(current, 3), foreign] {
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
    let owner = json!({"$":"STU_F76C486C","m_D63CBEC2":target,"m_EE729DCB":0});
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
    let unsupported_body = || Err(EvalError::UnsupportedConfig("STU_0AC15F78".into()));
    assert_eq!(
        evaluate(&mut first, &owner, Value::Entity(spawn.entity)),
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
    assert_eq!(
        evaluate(&mut second, &owner, Value::Entity(replacement.entity)),
        Ok(Value::Entity(spawn.controller))
    );
    assert_eq!(
        evaluate(&mut second, &possessor, Value::Entity(replacement.entity)),
        Ok(Value::Entity(spawn.controller))
    );
    for entity in [spawn.entity, spawn.controller, 0] {
        assert_eq!(
            evaluate(&mut second, &owner, Value::Entity(entity)),
            unsupported_owner()
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

#[test]
fn unsupported_projectile_modes_do_not_become_hitscan_or_straight_damage() {
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
    // A leased projectile with no direct damage still flies (combat.py
    // `_shoot`: `damaging or shot is not None`), so only the modes the flight
    // can't model are refused.
    for (field, value) in [
        ("m_45271E8C", json!(false)),
        ("m_projectileMotions", json!([{"$":"STU_E1A5B579"}])),
        ("m_numProjectilesPerShot", json!(2)),
    ] {
        let mut fields = fixture_projectile_fields(40.0);
        fields[field] = value;
        let mut runtime = fixture_volley_factory(29, true, fields)
            .start(spawn)
            .unwrap();
        runtime.advance(fire, &mover, BodyStep::Posed).unwrap();
        assert!(runtime.take_shots(Some(fire), &mover).is_empty(), "{field}");
    }
}

/// projectiles.py `fired`: a volley whose projectile leaves an entity names
/// it as the shot's spawn, and the Biotic Field canister flies despite its
/// arc (combat.py `_shoot` flies it straight). The shot carries the volley
/// state's count of volleys for the other clients' shot record.
#[test]
fn a_spawning_canister_flies_despite_its_arc_and_names_its_field() {
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
    assert_eq!(canister.feet, spawn.pose.position);
    assert_eq!(canister.volleys, 1);
}

/// A body whose entries set the ultimate's cost (#338) and whose ultimate
/// ability, when `ability`, starts on the charge (#342 in its m_150F0D92).
fn ultimate_factory(ability: bool) -> PlayerBodyFactory {
    use orpheus_script::ultimate::{ULT_CHARGE, ULT_COST};
    use serde_json::json;

    let var = |id: u32| json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":true});
    let mut links = vec![json!([1, "m_inPlug"])];
    let mut nodes = vec![
        json!({"pos":0,"class":"STUStatescriptEntry","uid":0,"rid":0,"remote":-1,
            "state":null,"client_only":false,"server_only":false,"fields":{
            "m_2BBEEAB8":false,"m_ADEB6E05":false}}),
        json!({"pos":1,"class":"STUStatescriptActionSetVar","uid":1,"rid":1,"remote":-1,
            "state":null,"client_only":false,"server_only":false,"fields":{
            "m_2BBEEAB8":false,"m_ADEB6E05":true,"m_out_Var":var(ULT_COST),
            "m_value":{"$":"STUConfigVarFloat","m_value":100.0}}}),
    ];
    if ability {
        links.push(json!([2, "m_beginPlug"]));
        nodes.push(
            json!({"pos":2,"class":"STUStatescriptStateAbility","uid":2,"rid":2,"remote":-1,
            "state":0,"client_only":false,"server_only":true,"fields":{
            "m_2BBEEAB8":true,"m_ADEB6E05":false,"m_150F0D92":var(ULT_CHARGE)}}),
        );
    }
    nodes[0]["fields"]["m_outPlug"] = json!({"$":"Plug","links":links,"m_FF3DAF1E":[]});
    let states = if ability { json!([2]) } else { json!([]) };
    fixture_graph_factory(
        json!({
            "index":42,"items":nodes.len(),"nodes":nodes,"states":states,"entries":[0],
            "remote":[],"sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{},
            "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
                "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4}
        }),
        Variables::new(),
    )
}

/// driver.py `BodyScript._charge_ult`: after every command frame the body
/// runs, the ultimate's charge (#342) rises by its cost (#338, from the
/// entries) * 16 ms / ULT_SECONDS, in f32 steps; a timer frame charges
/// nothing, and neither does any frame while an ability that starts on the
/// charge (its m_150F0D92 reads #342) runs.
#[test]
fn the_ultimate_charges_on_command_frames_until_its_ability_runs() {
    use orpheus_script::ultimate::ULT_CHARGE;

    let (_, spawn) = fixture_body_factory();
    let mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        spawn.pose.position,
        0,
        0x18,
    )
    .unwrap();
    let step = (100.0 * 16.0 / (1000.0 * ULT_SECONDS)) as f32;
    let next = |charge: f64| f64::from((charge + f64::from(step)) as f32);
    let charge = |runtime: &PlayerBodyRuntime| {
        runtime
            .snapshot()
            .unwrap()
            .variables
            .get(&ULT_CHARGE)
            .cloned()
    };
    let command = |frame| Command {
        frame,
        ..Command::default()
    };

    let mut runtime = ultimate_factory(false).start(spawn).unwrap();
    assert_eq!(
        charge(&runtime),
        Some(Value::Float(0.0)),
        "no charge before a command frame"
    );
    let mut expected = 0.0;
    for frame in spawn.frame + 1..=spawn.frame + 3 {
        runtime
            .advance(command(frame), &mover, BodyStep::Posed)
            .unwrap();
        expected = next(expected);
        assert_eq!(
            charge(&runtime),
            Some(Value::Float(expected)),
            "frame {frame}"
        );
    }
    let timer = spawn.frame + 10;
    runtime
        .advance(command(timer), &mover, BodyStep::UnposedTimer)
        .unwrap();
    assert_eq!(
        charge(&runtime),
        Some(Value::Float(expected)),
        "timer frame"
    );
    runtime
        .advance(command(timer + 1), &mover, BodyStep::UnposedBoundary)
        .unwrap();
    expected = next(expected);
    assert_eq!(
        charge(&runtime),
        Some(Value::Float(expected)),
        "neutral frame"
    );

    // The first charge starts the ability at its next tick; it runs, and
    // the charge waits.
    let mut runtime = ultimate_factory(true).start(spawn).unwrap();
    let first = spawn.frame + 1;
    runtime
        .advance(command(first), &mover, BodyStep::Posed)
        .unwrap();
    let charged = Some(Value::Float(next(0.0)));
    assert_eq!(charge(&runtime), charged);
    for frame in first + 1..=first + 3 {
        runtime
            .advance(command(frame), &mover, BodyStep::Posed)
            .unwrap();
        let kind = runtime.snapshot().unwrap().instances[&1].states[&0].kind;
        // ultimate.rs `ABILITY_RUNNING`.
        assert!(
            matches!(kind, StateKind::Ability { flags } if flags & 1 != 0),
            "frame {frame}: {kind:?}"
        );
        assert_eq!(charge(&runtime), charged, "frame {frame}");
    }
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
    // mask. The exit retires 08B6 (9) and its unstarted child 0BF8 (25).
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
        (18, 0x0ba4, Some((2, 11))),
        (19, 0x05ac, Some((2, 8))),
        (20, 0x02cf, Some((2, 6))),
        (21, 0x0b3b, Some((2, 9))),
        (22, 0x01d8, Some((2, 4))),
        (23, 0x0f82, Some((2, 21))),
        (24, 0x05ad, Some((2, 7))),
        (26, 0x0e8f, Some((13, 32))),
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
    let Value::Map(messages) = &snapshot.instances[&24].variables[&29859] else {
        panic!("the authored message table must be a map");
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
    let selection = &snapshot.instances[&18].states[&0];
    assert!(selection.active);
    assert_eq!(selection.kind, StateKind::BooleanSwitch(false));
    assert!(!snapshot.instances[&18].states.contains_key(&1));
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
        let filter = &snapshot.instances[&2].states[&14];
        assert!(filter.active);
        assert_eq!(filter.kind, StateKind::BooleanSwitch(false));
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
        component.evaluate_state_config(24, 0, &message, &mut world),
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
            assert_eq!(
                component.evaluate_state_config(2, 14, &filter, &mut world),
                Ok(Value::Bool(present))
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
    // Both 0254 rifle branches have the same parameterless human condition.
    for condition in rifle_conditions {
        assert_eq!(
            component.evaluate_state_config(11, 10, &condition, &mut world),
            Ok(Value::Null)
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
