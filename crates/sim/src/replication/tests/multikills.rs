//! The announcer's multikill lines (`crate::announcer`): what the killer's
//! client receives when he kills two enemies within 2 s.

use super::*;
use crate::{
    announcer::{MULTIKILL_WINDOW_TICKS, MultikillPulse},
    combat::{CombatPlayer, ENVIRONMENT_DAMAGE, Shooter, apply_player_hits},
    scene_assets::install_collision_build,
};
use orpheus_content::maps::SpawnChoice;
use orpheus_content::stats::PlayerStats;
use orpheus_core::bits::BitWriter;
use orpheus_wire::combat_messages::CombatNotice;

const SHOOTER: u32 = 0xa000_0100;
const VICTIMS: [u32; 2] = [0xa000_0200, 0xa000_0300];

/// A started world whose player plays Soldier: 76 for `team`.
fn spawned(
    scene: &PreparedScene,
    card: &PlayerCard,
    entity: u32,
    slot: i8,
    team: u8,
) -> WorldDelivery {
    let viewer = PlayerAppearance {
        entity,
        slot,
        team,
        spawn: 0,
        room: SpawnChoice::First,
        card,
    };
    let mut world = WorldDelivery::new(entity);
    world.start(scene, &viewer, FIRST_TICK).unwrap();
    world.created.insert(entity);
    world.present(scene, FIRST_TICK).unwrap();
    let hero = scene.heroes().get(SOLDIER).unwrap();
    world
        .switch(
            scene,
            &viewer,
            FIRST_TICK,
            RETAIL_QUANTA,
            entity | 1,
            (hero, SkinTheme::default()),
            true,
        )
        .unwrap();
    world
}

/// The controller frames queued for the player so far.
fn controller_frames(world: &WorldDelivery) -> Vec<BitWriter> {
    world
        .queued
        .iter()
        .filter(|record| record.update.entity == world.owner)
        .filter_map(|record| record.update.chunk.clone())
        .collect()
}

/// In Escort on Watchpoint: Gibraltar (PvP controller 0C90), a player kills
/// one enemy, then a second 124 ticks later (less than `0215`'s 2 s): the
/// second kill sends him at once a controller frame whose `0215` instance
/// has its pulser st21 pulsed once, fresh, and #10122 = `058A.078`, the
/// announcer's "Double kill". The first kill sends none.
#[test]
#[ignore = "requires a derived 104319 pack and the install (Gibraltar's collision)"]
fn a_second_kill_within_two_seconds_pulses_the_announcers_double_kill() {
    const GIBRALTAR: u64 = 0x0800_0000_0000_0184;
    let runtime = orpheus_testkit::runtime_dir();
    let mut assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(install_collision_build(orpheus_testkit::game_dir()));
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let map = orpheus_content::maps::MapCatalog::load(runtime.join("data"))
        .unwrap()
        .game_map(
            GIBRALTAR,
            crate::match_flow::ESCORT,
            "Escort",
            false,
            &[6, 6],
        )
        .unwrap();
    let scene = assets.prepare(map).unwrap();

    let card = PlayerCard::default();
    let mut shooter = spawned(&scene, &card, SHOOTER, 0, 0);
    let [first_victim, second_victim] = VICTIMS;
    let mut victims = [
        spawned(&scene, &card, first_victim, 1, 1),
        spawned(&scene, &card, second_victim, 2, 1),
    ];
    let mut stats = [(); 3].map(|_| PlayerStats::default());
    // A new controller frame replaces the earlier one still waiting.
    let before = controller_frames(&shooter).last().cloned();
    let first = FIRST_TICK + 100;
    let ticks = [first, first + MULTIKILL_WINDOW_TICKS - 1];
    for (index, tick) in ticks.into_iter().enumerate() {
        let hit = PlayerHit {
            body: VICTIMS[index] | 1,
            shooter: Shooter {
                source: SHOOTER | 1,
                team: 0,
                bot: false,
            },
            amount: ENVIRONMENT_DAMAGE,
            critical: false,
            loadout: None,
            frame: tick,
            world_tick: Some(tick),
            heal: false,
            tags: orpheus_script::weapons::DamageTags::NONE,
            direction: None,
        };
        let [first_stats, second_stats, third_stats] = &mut stats;
        let [first_victim, second_victim] = &mut victims;
        let notices = apply_player_hits(
            &scene,
            vec![hit],
            &mut [
                CombatPlayer {
                    entity: SHOOTER,
                    world: Some(&mut shooter),
                    stats: first_stats,
                },
                CombatPlayer {
                    entity: VICTIMS[0],
                    world: Some(first_victim),
                    stats: second_stats,
                },
                CombatPlayer {
                    entity: VICTIMS[1],
                    world: Some(second_victim),
                    stats: third_stats,
                },
            ],
            tick,
            Duration::from_millis(u64::from(tick) * 16),
        );
        assert!(
            notices[0]
                .iter()
                .any(|notice| matches!(notice, CombatNotice::Elimination { .. })),
            "kill {index}"
        );
        shooter.send_multikills(&scene, tick).unwrap();
        let last = controller_frames(&shooter).last().cloned();
        assert_eq!(
            last == before,
            index == 0,
            "a frame for the second kill only"
        );
    }
    assert_double_kill_sent(&scene, &shooter, ticks[1]);
}

/// In the Practice Range (mode 005A, controller 0C90), a player kills one
/// Training Bot, then a second 124 ticks later: as in PvP, the second kill
/// sends him at once the controller frame with `0215` pulsed for "Double
/// kill" (retail says it there, for Training Bots too). The game service
/// counts a bot's death for the player who shot it
/// (`WorldDelivery::eliminated`, `deliver_bot_hits`).
#[test]
#[ignore = "requires a derived 104319 pack"]
fn in_the_practice_range_a_second_training_bot_kill_pulses_the_double_kill() {
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let card = PlayerCard::default();
    let mut shooter = spawned(&scene, &card, SHOOTER, 0, 0);
    let graphs = scene.shared_graphs();
    let training_bot = scene.bots().first().map(|bot| bot.spawn.body);
    let before = controller_frames(&shooter).last().cloned();
    let first = FIRST_TICK + 100;
    let ticks = [first, first + MULTIKILL_WINDOW_TICKS - 1];
    for (index, tick) in ticks.into_iter().enumerate() {
        shooter.eliminated(&graphs, tick, crate::bots::BOT_ENTITY, training_bot);
        shooter.send_multikills(&scene, tick).unwrap();
        let last = controller_frames(&shooter).last().cloned();
        assert_eq!(
            last == before,
            index == 0,
            "a frame for the second kill only"
        );
    }
    assert_double_kill_sent(&scene, &shooter, ticks[1]);
}

/// The last controller frame queued for `shooter` carries the one he would
/// get at `tick` with st21 pulsed once, fresh, for "Double kill".
fn assert_double_kill_sent(scene: &PreparedScene, shooter: &WorldDelivery, tick: u32) {
    let practice = scene.controller().practice();
    let expected = scene
        .controller()
        .controller_frame(
            shooter.command_ack(tick),
            ControllerFrame {
                select_open: shooter.select_open,
                skins: shooter.select_open && (practice || shooter.assembling),
                hide_assemble: practice,
                presence: None,
                hero: Some(SOLDIER),
                multikills: Some(MultikillPulse {
                    count: 1,
                    fresh: true,
                    stimulus: Some(0x0ee0_0000_0000_058a),
                }),
            },
        )
        .unwrap();
    let frames = controller_frames(shooter);
    let chunk = frames.last().unwrap();
    assert!(orpheus_testkit::contains_bits(
        chunk.as_bytes(),
        chunk.bit_len(),
        expected.as_bytes(),
        expected.bit_len()
    ));
}
