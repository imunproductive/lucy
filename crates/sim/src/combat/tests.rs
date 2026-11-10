use super::*;
use crate::{instance::RETAIL_QUANTA, scene_assets::SceneAssets};
use orpheus_content::maps::GameMap;

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
        origin: [x - 8.0, y + eye_height(SOLDIER, false), z],
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
