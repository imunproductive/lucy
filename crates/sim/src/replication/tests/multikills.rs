//! The announcer's multikill lines (`crate::announcer`): what the killer's
//! client receives when he kills two enemies within 2 s.

use super::*;
use crate::{
    announcer::MULTIKILL_WINDOW_TICKS,
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
    spawned_as(scene, card, entity, slot, team, SOLDIER)
}

/// A started world whose player plays `hero` for `team`.
fn spawned_as(
    scene: &PreparedScene,
    card: &PlayerCard,
    entity: u32,
    slot: i8,
    team: u8,
    hero: u64,
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
    world
        .present(
            scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
    let hero = scene.heroes().get(hero).unwrap();
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
            environment: false,
            from: None,
            source: None,
            health_pack: false,
        };
        let [first_stats, second_stats, third_stats] = &mut stats;
        let [first_victim, second_victim] = &mut victims;
        let (notices, _) = apply_player_hits(
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
        shooter.eliminated(
            &graphs,
            tick,
            crate::bots::BOT_ENTITY,
            training_bot,
            &DamageTags::NONE,
            true,
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

/// Three Training Bot kills, each within 2 s of the last: `0215` rid85
/// waits 1.6 s (100 ticks of 16 ms) after the third, then its server-only
/// st28 names `00C9.078` on the controller, which the player's body says
/// (ClientInVoice 27500). Tracer's voice set has the line (Soldier: 76's
/// doesn't: 24 of the 33 sets do). The body sends no commands here, so
/// its own elimination line (`01CA`, on its frames) doesn't come.
#[test]
#[ignore = "requires a derived 104319 pack"]
fn a_third_kill_in_the_window_makes_the_hero_say_his_multikill_line() {
    const TRACER: u64 = 0x02e0_0000_0000_0003;
    // Tracer's graphs meet content debug builds refuse; the release
    // server tolerates it (as the spawn room tests do).
    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let card = PlayerCard::default();
    let mut shooter = spawned_as(&scene, &card, SHOOTER, 0, 0, TRACER);
    assert!(shooter.body_script.is_some(), "Tracer's body script runs");
    let graphs = scene.shared_graphs();
    let training_bot = scene.bots().first().map(|bot| bot.spawn.body);
    let first = FIRST_TICK + 100;
    let kills = [first, first + 60, first + 120];
    let mut said = Vec::new();
    for tick in first..first + 300 {
        if kills.contains(&tick) {
            shooter.eliminated(
                &graphs,
                tick,
                crate::bots::BOT_ENTITY,
                training_bot,
                &DamageTags::NONE,
                true,
            );
        }
        said.extend(
            shooter
                .take_voice_lines(&scene, tick)
                .into_iter()
                .map(|line| (tick, line.speaker, line.stimulus)),
        );
    }
    assert_eq!(
        said,
        [(
            kills[2] + 100,
            shooter.current_body().unwrap(),
            0x0ee0_0000_0000_00c9
        )]
    );
}

/// The controller script's `0215` pulsed st21 once for "Double kill", and
/// the last controller frame queued for `shooter` is its projection then
/// (`crate::controller`): it carries st21 and #10122.
fn assert_double_kill_sent(scene: &PreparedScene, shooter: &WorldDelivery, tick: u32) {
    let script = shooter.controller_script.as_ref().unwrap();
    let multikill = script.multikill().unwrap();
    assert_eq!(multikill.pulses, 1);
    assert_eq!(multikill.stimulus, Some(0x0ee0_0000_0000_058a));
    assert_eq!(multikill.pulsed_ms / 16, u64::from(tick));
    let (_, input) = shooter.select_input(scene.controller().practice()).unwrap();
    let expected = scene
        .controller()
        .controller_frame(script, shooter.controller.command_frame(), input)
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
