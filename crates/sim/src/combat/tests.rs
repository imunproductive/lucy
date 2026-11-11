use super::*;
use crate::{instance::RETAIL_QUANTA, scene_assets::SceneAssets};
use orpheus_content::maps::{GameMap, PRACTICE_RANGE_MODE};

const SHOOTER: Shooter = Shooter {
    source: 0xa000_0101,
    team: 0,
    bot: false,
};

#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn repeated_rifle_hits_retire_and_respawn_only_the_hit_bot() {
    let runtime = orpheus_testkit::runtime_dir();
    let mut scene = SceneAssets::load(runtime)
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let slot = scene
        .bots()
        .iter()
        .position(|bot| bot.spawn.team == 1)
        .unwrap();
    let bot = &scene.bots()[slot];
    let entity = bot.entity;
    let [x, y, z] = bot.position;
    let count = scene.bots().len();
    let mut combat = Combat::new(&scene);
    let mut context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick: 1000,
        now: Duration::from_secs(4),
    };
    combat.record_poses(&scene, &[], context.tick);
    let mut shot = PosedHitscan {
        source: SHOOTER.source,
        // Level with the Training Bot's head: its idle pose centres the
        // head sphere 1.98 m up (from Soldier: 76's standing eye, combat.py
        // `EYE` 1.55 m, a level shot meets its chest).
        origin: [x - 8.0, y + 2.0, z],
        command: Command {
            frame: context.tick,
            yaw: 16384,
            ..Command::default()
        },
        // Along +x, as yaw 16384 aims.
        direction: [1.0, 0.0, 0.0],
        shot_frame: 999,
        reach: 180.0,
        hit: HitDefinition {
            amount: 20.0,
            loadout: None,
            critical: 2.0,
            falloff: None,
            tags: [0x0d80_0000_0000_09a2].into_iter().collect(),
            allies: AllyHits::None,
        },
    };
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| false)
            .unwrap()
            .is_none()
    );
    assert!(
        combat
            .hitscan(
                &mut scene,
                shot,
                Shooter { team: 1, ..SHOOTER },
                context,
                |_| { true }
            )
            .unwrap()
            .is_none()
    );
    for index in 0..5 {
        let result = combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .unwrap();
        let terminal = index == 4;
        assert_eq!(result.health.entity, entity);
        assert_eq!(
            result.notices[0],
            Some(CombatNotice::Hit {
                target: entity,
                dealt: 40.0,
                critical: true,
                killed: terminal,
                shot_frame: shot.shot_frame,
            })
        );
        assert_eq!(
            result.notices[1],
            terminal.then_some(CombatNotice::Elimination {
                killer: shot.source,
                victim: entity,
            })
        );
        assert_eq!(
            combat.targets[slot].vitals.total(),
            160.0 - f64::from(index) * 40.0
        );
        assert_eq!(result.remaining, 160.0 - f64::from(index) * 40.0);
        assert_eq!(
            result.death,
            terminal.then_some(BotDeath {
                bot: entity,
                killer: SHOOTER.source,
                critical: true,
                loadout: None,
                tick: context.tick,
            })
        );
        // A hit it survives carries its damage for the bot's script.
        assert_eq!(
            result.damage,
            (!terminal).then_some(BotDamage {
                source: SHOOTER.source,
                dealt: 40.0,
                tags: shot.hit.tags,
                tick: context.tick,
            })
        );
        assert_eq!(scene.bots()[slot].dead, terminal);
    }
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .is_none()
    );
    let due = context.now + scene.respawn_time();
    assert!(
        combat
            .respawn_due(&mut scene, 1187, due - Duration::from_millis(1))
            .unwrap()
            .is_empty()
    );
    let [replacement] = combat
        .respawn_due(&mut scene, 1188, due)
        .unwrap()
        .try_into()
        .ok()
        .unwrap();
    assert_eq!(replacement.retired, entity);
    let next = BOT_ENTITY + count as u32;
    assert_eq!(replacement.created.entity, next);
    assert!(matches!(
        replacement.created.operation,
        Some(Operation::Create(_))
    ));
    assert!(!scene.bots()[slot].dead);
    assert_eq!(combat.targets[slot].vitals.total(), 200.0);
    context.tick = 1188;
    context.now = due;
    combat.record_poses(&scene, &[], context.tick);
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .is_none()
    );
    shot.command.frame = context.tick;
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |visible| visible
                == next)
            .unwrap()
            .is_some()
    );
    assert_eq!(combat.targets[slot].vitals.total(), 160.0);
}

#[test]
fn heroes_respawn_after_the_mode_time() {
    let times = RespawnTimes::from_json(r#"{"0x0230000000000018": 3}"#).unwrap();
    assert_eq!(
        times.for_mode(0x0230_0000_0000_0018),
        Duration::from_secs(3)
    );
    assert_eq!(
        times.for_mode(0x0230_0000_0000_0001),
        Duration::from_secs(10)
    );
    assert_eq!(RESPAWN_RETRY, Duration::from_millis(250));
}

/// combat.py `MELEE = {0x0043: (4, 1.5, 1.0)}`, read from graph 0043.
#[test]
#[ignore = "requires the derived 104319 statescript catalog"]
fn quick_melee_reads_the_python_sphere_from_its_graph() {
    let graphs = orpheus_script::graph::GraphStore::load(
        orpheus_testkit::runtime_dir().join("data/game_graphs_174.json.gz"),
    )
    .unwrap();
    assert_eq!(
        melee_states(&graphs).unwrap(),
        [MeleeState {
            state: 4,
            reach: 1.5,
            radius: 1.0
        }]
    );
}

/// combat.py `RESPAWN` and `PRACTICE_RESPAWN` from the derived mode times.
#[test]
#[ignore = "requires the derived 104319 mode respawn times"]
fn derived_respawn_times_match_the_python_reference() {
    let times =
        RespawnTimes::load(orpheus_testkit::runtime_dir().join("data/game_respawns_174.json"))
            .unwrap();
    assert_eq!(times.for_mode(PRACTICE_RANGE_MODE), Duration::from_secs(3));
    assert_eq!(
        times.for_mode(0x0230_0000_0000_0014),
        Duration::from_secs(10)
    );
}

#[test]
fn the_died_message_names_the_killer_the_crit_and_the_loadout() {
    let (message, parameters) = death_message(0xa000_0201, true, Some(0x0b90_0000_0000_008d));
    assert_eq!(message, DIED);
    assert_eq!(parameters[&KILLER], Value::Entity(0xa000_0201));
    assert_eq!(parameters[&CRIT], Value::Bool(true));
    assert_eq!(parameters[&LOADOUT], Value::Asset(0x0b90_0000_0000_008d));
    assert_eq!(parameters[&ASSISTS], Value::Array(Vec::new()));
    assert_eq!(death_message(1, false, None).1[&LOADOUT], Value::Null);
}

#[test]
fn shooters_hit_other_teams_and_everyone_in_a_free_for_all() {
    let target = |body, team, dead, hurt| PlayerSlot {
        target: PlayerTarget {
            body,
            team,
            pose: TargetPose {
                feet: [0.0; 3],
                height: 1.8,
                radius: 0.4,
                eye: 1.55,
                yaw: 0.0,
                shapes: None,
            },
            dead,
            hurt,
        },
        history: TargetHistory::new(None),
    };
    let combat = Combat {
        targets: Vec::new(),
        players: vec![
            target(0xa000_0101, 0, false, true),
            target(0xa000_0201, 0, false, false),
            target(0xa000_0301, 1, false, false),
            target(0xa000_0401, 1, true, true),
            target(0xa000_0501, 0, false, true),
            target(0xa000_0601, 0, true, true),
        ],
        rockets: Vec::new(),
        player_hits: Vec::new(),
        next_bot: BOT_ENTITY,
        tick: 0,
        seen_ids: SeenIds::new(),
        field_ids: FieldIds::default(),
        next_rocket: 0,
        landed: Vec::new(),
    };
    let bodies = |free_for_all, allies| -> Vec<u32> {
        combat
            .player_candidates(SHOOTER, free_for_all, allies, |slot| {
                Some(slot.target.pose.clone())
            })
            .map(|(body, _)| body)
            .collect()
    };
    // Not himself, not an ally, not the dead.
    assert_eq!(bodies(false, AllyHits::None), [0xa000_0301]);
    assert_eq!(
        bodies(true, AllyHits::None),
        [0xa000_0201, 0xa000_0301, 0xa000_0501]
    );
    // A hit that strikes hurt allies (the Biotic Rifle's) passes through
    // the ally at full health, never strikes the shooter or the dead, and
    // changes nothing in a free-for-all.
    assert_eq!(bodies(false, AllyHits::Hurt), [0xa000_0301, 0xa000_0501]);
    assert_eq!(
        bodies(false, AllyHits::All),
        [0xa000_0201, 0xa000_0301, 0xa000_0501]
    );
    assert_eq!(
        bodies(true, AllyHits::Hurt),
        [0xa000_0201, 0xa000_0301, 0xa000_0501]
    );
}

/// The bodies the statescript's target searches find (`TrackTargets`):
/// the players' bodies as recorded this tick, then every bot, with their
/// teams, capsules and deaths.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn target_searches_see_the_players_then_the_bots() {
    let scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let mut combat = Combat::new(&scene);
    let pose = TargetPose {
        feet: [1.0, 2.0, 3.0],
        height: 1.8,
        radius: 0.4,
        eye: 1.55,
        yaw: 0.0,
        shapes: None,
    };
    let player = PlayerTarget {
        body: SHOOTER.source,
        team: 1,
        pose: pose.clone(),
        dead: true,
        hurt: false,
    };
    combat.record_poses(&scene, &[player], 1000);
    let candidates = combat.target_candidates(&scene);
    assert_eq!(candidates.len(), 1 + scene.bots().len());
    assert_eq!(
        candidates[0],
        TargetCandidate {
            entity: SHOOTER.source,
            team: Some(1),
            dead: true,
            feet: pose.feet,
            height: pose.height,
            radius: pose.radius,
        }
    );
    for (candidate, bot) in candidates[1..].iter().zip(scene.bots()) {
        let target = scene.bot_target(bot);
        assert_eq!(
            (candidate.entity, candidate.team, candidate.dead),
            (bot.entity, Some(bot.spawn.team), bot.dead)
        );
        assert_eq!(
            (candidate.feet, candidate.height, candidate.radius),
            (target.feet, target.height, target.radius)
        );
    }
}

/// The Biotic Rifle's hit (`0B94` st44: 70, strikes hurt allies) on the
/// Practice Range, after leafall903/ORPHEUS (MIT) `test_game_ana.py`
/// (c239a84): a hurt Friendly Bot heals 70 and then the rest to full, with
/// no notice; at full health the shot passes through it to the Training
/// Bot behind, which takes 70 with the shooter's 25001; a hurt allied
/// player's body is healed by the match, an unhurt one is passed through;
/// the hip dart, a projectile, heals the same.
#[test]
#[ignore = "requires a derived 104319 Practice Range scene"]
fn the_biotic_rifle_heals_hurt_allies_and_damages_enemies() {
    use crate::projectile_flight::ProjectileFlight;

    const RIFLE: HitDefinition = HitDefinition {
        amount: 70.0,
        loadout: Some(0x0b90_0000_0000_00e3),
        critical: 1.0,
        falloff: None,
        allies: AllyHits::Hurt,
        tags: DamageTags::NONE,
    };
    let mut scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let team = |team| {
        scene
            .bots()
            .iter()
            .position(|bot| bot.spawn.team == team)
            .unwrap()
    };
    let (friendly, enemy) = (team(0), team(1));
    let [x, y, z] = scene.bots()[friendly].position;
    scene.bots_mut()[enemy].position = [x + 2.0, y, z];
    let (friendly_bot, enemy_bot) = (scene.bots()[friendly].entity, scene.bots()[enemy].entity);
    let full = scene.bots()[friendly].spawn.health;
    let mut combat = Combat::new(&scene);
    let context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick: 1000,
        now: Duration::from_secs(4),
    };
    combat.record_poses(&scene, &[], context.tick);
    let shot = PosedHitscan {
        source: SHOOTER.source,
        origin: [x - 8.0, y + 1.2, z],
        command: Command {
            frame: context.tick,
            yaw: 16384,
            ..Command::default()
        },
        direction: [1.0, 0.0, 0.0],
        shot_frame: 999,
        reach: 100.0,
        hit: RIFLE,
    };
    let fire = |combat: &mut Combat, scene: &mut PreparedScene| {
        combat
            .hitscan(scene, shot, SHOOTER, context, |_| true)
            .unwrap()
    };
    let wound = Death {
        time: 0.0,
        frame: 0,
        killer: 0,
    };
    combat.targets[friendly].vitals.damage(100.0, wound);
    for (given, total) in [(70.0, full - 30.0), (30.0, full)] {
        let hit = fire(&mut combat, &mut scene).expect("the friendly bot heals");
        assert_eq!(
            (hit.bot, hit.healed, hit.remaining, hit.notices, hit.death),
            (friendly_bot, given, total, [None, None], None)
        );
        assert_eq!(hit.health.entity, friendly_bot);
    }
    let hit = fire(&mut combat, &mut scene).expect("the enemy behind takes the dart");
    assert_eq!(
        (hit.bot, hit.healed, hit.remaining),
        (enemy_bot, 0.0, 130.0)
    );
    assert_eq!(
        hit.notices[0],
        Some(CombatNotice::Hit {
            target: enemy_bot,
            dealt: 70.0,
            critical: false,
            killed: false,
            shot_frame: shot.shot_frame,
        })
    );
    assert_eq!(combat.targets[friendly].vitals.total(), full);

    // An allied player's body in front: struck while hurt, for the match
    // to heal; passed through at full health.
    let player = |hurt| PlayerTarget {
        body: 0xa000_0201,
        team: 0,
        pose: TargetPose {
            feet: [x - 4.0, y, z],
            height: 1.8,
            radius: 0.4,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        },
        dead: false,
        hurt,
    };
    combat.record_poses(&scene, &[player(true)], context.tick);
    assert!(fire(&mut combat, &mut scene).is_none());
    let [hit] = combat.take_player_hits().try_into().unwrap();
    assert_eq!(
        (hit.body, hit.amount, hit.heal, hit.critical),
        (0xa000_0201, 70.0, true, false)
    );
    combat.record_poses(&scene, &[player(false)], context.tick);
    assert_eq!(fire(&mut combat, &mut scene).unwrap().bot, enemy_bot);
    assert!(combat.take_player_hits().is_empty());

    // The hip dart flies (125 m/s) and heals the same.
    combat.record_poses(&scene, &[], context.tick);
    combat.targets[friendly].vitals.damage(50.0, wound);
    let dart = ProjectileBallistics {
        speed: 125.0,
        lifetime: 0.8,
        definition: None,
        direct: Some(RIFLE),
        blast: None,
        gravity: 0.0,
        fuse: None,
        unmodelled: Vec::new(),
    };
    let mut flight =
        ProjectileFlight::new(dart, shot.origin, shot.direction, Some(context.tick)).unwrap();
    let mut hits = Vec::new();
    for tick in context.tick + 1..context.tick + 10 {
        let context = ShotContext { tick, ..context };
        if combat
            .step_projectile(&mut scene, SHOOTER, &mut flight, context, &mut hits)
            .unwrap()
            .is_some()
        {
            break;
        }
    }
    let [hit] = hits.try_into().ok().unwrap();
    assert_eq!(
        (hit.bot, hit.healed, hit.remaining, hit.notices),
        (friendly_bot, 50.0, full, [None, None])
    );
}
