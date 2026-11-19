//! A player's hurt feedback over several hits (`WorldDelivery::damage_body`,
//! `take_voice_lines`) and the live switch that turns it off
//! (`crate::hurt_feedback`).

use super::*;
use crate::combat::Shooter;
use crate::hurt_feedback::HurtFeedback;
use orpheus_wire::{combat_messages::CombatNotice, commands::Command};

const OWNER: u32 = 0xa000_0100;
/// `02C5`'s networked knockback anim.
const KNOCKBACK: u16 = 7;
/// The Soldier: 76 voice set's pain stimuli: "PnMnr" (`0608`), "PnSvr"
/// (`0609`).
const PAIN: [u64; 2] = [0x0ee0_0000_0000_0003, 0x0ee0_0000_0000_0004];

fn viewer(card: &PlayerCard) -> PlayerAppearance<'_> {
    PlayerAppearance {
        entity: OWNER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card,
    }
}

/// What a Training Bot's rockets (30 each, the Helix explosion's tags) do
/// to a Soldier: 76 standing out on the range, hit after the commands of
/// ticks `hits` (counted from his first command): the ticks his knockback
/// anim starts and ends, and the ticks and stimuli of his voice lines.
/// The ticks the knockback anim starts (true) and ends (false), and the
/// ticks and stimuli of the voice lines.
type Reactions = (Vec<(u32, bool)>, Vec<(u32, u64)>);

fn rockets(scene: &PreparedScene, hits: &[u32], ticks: u32) -> Reactions {
    let card = PlayerCard::default();
    let mut world = WorldDelivery::new(OWNER);
    world.start(scene, &viewer(&card), FIRST_TICK).unwrap();
    world.created.insert(OWNER);
    world.present(scene, scene.mode_ui(), FIRST_TICK).unwrap();
    let hero = scene.heroes().get(SOLDIER).unwrap();
    world
        .switch(
            scene,
            &viewer(&card),
            FIRST_TICK,
            RETAIL_QUANTA,
            OWNER | 1,
            (hero, SkinTheme::default()),
            true,
        )
        .unwrap();
    let body = world.current_body().unwrap();
    let bot = &scene.bots()[0];
    world.mover = Some(
        scene
            .player_mover_at(hero.body, 0, bot.position, 0)
            .unwrap(),
    );
    world.area_step(scene, &viewer(&card));
    assert_eq!(world.in_spawn_room(), Some(false));
    let explosion: orpheus_script::weapons::DamageTags = [0x03bf, 0x04b1, 0x078d, 0x0980, 0x2d30]
        .map(|tag| 0x0d80_0000_0000_0000_u64 | tag)
        .into_iter()
        .collect();
    let mut knockback = Vec::new();
    let mut voices = Vec::new();
    let mut knocked = false;
    for index in 0..ticks {
        let tick = FIRST_TICK + 1 + index;
        world.queue_commands(
            tick,
            &[Command {
                frame: tick,
                ..Command::default()
            }],
        );
        world.release_input(1);
        while world.has_input_work() {
            world
                .advance_input(scene, &viewer(&card), RETAIL_QUANTA, 64, None)
                .unwrap();
        }
        if hits.contains(&index) {
            let hit = PlayerHit {
                body,
                shooter: Shooter {
                    source: bot.entity,
                    team: 1,
                    bot: true,
                },
                amount: 30.0,
                critical: false,
                loadout: None,
                frame: tick,
                world_tick: None,
                heal: false,
                tags: explosion,
                direction: Some([0.0, 0.0, -1.0]),
                environment: false,
                from: Some(bot.position),
            };
            let damage = world
                .damage_body(scene, &hit, tick, Duration::ZERO)
                .unwrap();
            assert_eq!((damage.dealt, damage.killed), (30.0, false));
            // His client's damage direction arc, whatever the switch: the
            // bot's place and the damage.
            assert_eq!(
                world.take_hurt_notices(),
                [CombatNotice::Hurt {
                    victim: body,
                    attacker: bot.entity,
                    from: bot.position.map(|axis| axis as f32),
                    dealt: 30.0,
                }]
            );
        }
        voices.extend(
            world
                .take_voice_lines(scene, tick)
                .into_iter()
                .map(|line| (index, line.stimulus)),
        );
        let snapshot = world
            .body_script
            .as_ref()
            .unwrap()
            .runtime
            .snapshot()
            .unwrap();
        let flinch = snapshot
            .instances
            .values()
            .find(|instance| instance.graph == 0x02c5)
            .unwrap();
        let active = flinch
            .states
            .get(&KNOCKBACK)
            .is_some_and(|state| state.active);
        if active != knocked {
            knockback.push((index, active));
            knocked = active;
        }
    }
    (knockback, voices)
}

/// Each rocket a player survives knocks him back and makes him say a pain
/// line, the third as the first. `02C5` starts the knockback anim (st7)
/// with the hit; its subgraph waits 0.25 s (16 ticks of 16 ms) and ends it
/// once he stands on the ground (#545), so it runs 16 ticks. `01CA` raises
/// the pain criteria 0.15 s after the hit, in the 10th frame. The live
/// switch takes them away: off, the body gets no damage message at all;
/// with the flinch off, the message has no tags, so no knockback but the
/// line; with the pain off, the knockback but no line.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn every_rocket_knocks_the_player_back_and_hurts_unless_switched_off() {
    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    let hits = [10, 70, 130];
    let knocked: Vec<_> = hits
        .iter()
        .flat_map(|&hit| [(hit, true), (hit + 16, false)])
        .collect();
    let pained = |voices: &[(u32, u64)]| {
        voices.iter().all(|(_, stimulus)| PAIN.contains(stimulus))
            && voices.iter().map(|(tick, _)| *tick).collect::<Vec<_>>() == hits.map(|hit| hit + 10)
    };

    let (knockback, voices) = rockets(&scene, &hits, 160);
    assert_eq!(knockback, knocked);
    assert!(pained(&voices), "{voices:?}");

    let mut switch = |change: &str| {
        let mut hurt = HurtFeedback::default();
        hurt.change(change).unwrap();
        scene.set_hurt_feedback(hurt);
        rockets(&scene, &hits, 160)
    };
    assert_eq!(switch("off"), (Vec::new(), Vec::new()));
    let (knockback, voices) = switch("flinch off");
    assert!(knockback.is_empty(), "{knockback:?}");
    assert!(pained(&voices), "{voices:?}");
    let (knockback, voices) = switch("pain off");
    assert_eq!(knockback, knocked);
    assert!(voices.is_empty(), "{voices:?}");
}
