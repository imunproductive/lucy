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
        // combat.py `EYE`: Soldier: 76's standing eye.
        origin: [x - 8.0, y + 1.55, z],
        command: Command {
            frame: context.tick,
            yaw: 16384,
            ..Command::default()
        },
        shot_frame: 999,
        reach: 180.0,
        hit: HitDefinition {
            amount: 20.0,
            loadout: None,
            critical: 2.0,
            falloff: None,
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
        assert_eq!(scene.bots()[slot].dead, terminal);
    }
    assert!(
        combat
            .hitscan(&mut scene, shot, SHOOTER, context, |_| true)
            .unwrap()
            .is_none()
    );
    let due = context.now + BOT_RESPAWN_TIME;
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
    let target = |body, team, dead| PlayerSlot {
        target: PlayerTarget {
            body,
            team,
            pose: TargetPose {
                feet: [0.0; 3],
                height: 1.8,
                radius: 0.4,
                eye: 1.55,
            },
            dead,
        },
        history: TargetHistory::new(None),
    };
    let combat = Combat {
        targets: Vec::new(),
        players: vec![
            target(0xa000_0101, 0, false),
            target(0xa000_0201, 0, false),
            target(0xa000_0301, 1, false),
            target(0xa000_0401, 1, true),
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
    let bodies = |free_for_all| -> Vec<u32> {
        combat
            .player_candidates(SHOOTER, free_for_all, |slot| Some(slot.target.pose))
            .map(|(body, _)| body)
            .collect()
    };
    // Not himself, not an ally, not the dead.
    assert_eq!(bodies(false), [0xa000_0301]);
    assert_eq!(bodies(true), [0xa000_0201, 0xa000_0301]);
}
