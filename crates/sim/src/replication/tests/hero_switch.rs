//! The hero switch chat line (`crate::notices`): a switch goes to the
//! player's controller script as `037F.025`, and the notice frame carries
//! what `0C90` rid220 kept.

use super::*;
use lucy_content::{heroes::ANA, maps::SpawnChoice};
use lucy_script::controller::HeroSwitch;

const PLAYER: u32 = 0xa000_0100;

/// In the Practice Range (`005A`, `0C90` for both teams), Soldier: 76
/// switches to Ana and back: each switch waits for its notice frame with
/// the heroes and the player as `0C90` keeps them (#4513/#4514/#4515, the
/// root's presence bits); the first pick is no switch. Only the notice
/// frame carries the three: the graph keeps them, the client would post the
/// line on every frame that sets them.
#[test]
#[ignore = "requires a derived 104319 pack"]
fn a_hero_switch_takes_its_notice_from_the_controller_graph() {
    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    assert!(scene.switch_notices());
    let card = PlayerCard::default();
    let viewer = PlayerAppearance {
        entity: PLAYER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: SpawnChoice::First,
        card: &card,
    };
    let mut world = WorldDelivery::new(PLAYER);
    world.start(&scene, &viewer, FIRST_TICK).unwrap();
    world.created.insert(PLAYER);
    world
        .present(
            &scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
    let pick = |world: &mut WorldDelivery, tick, body, hero| {
        let hero = scene.heroes().get(hero).unwrap();
        world
            .switch(
                &scene,
                &viewer,
                tick,
                RETAIL_QUANTA,
                PLAYER | body,
                (hero, SkinTheme::default()),
                true,
            )
            .unwrap();
    };
    pick(&mut world, FIRST_TICK, 1, SOLDIER);
    assert!(
        world.notices.waiting.is_empty(),
        "the first pick is no switch"
    );
    assert_eq!(world.switch, None);

    for (serial, (tick, body, old, new)) in (1..).zip([
        (FIRST_TICK + 100, 2, SOLDIER, ANA),
        (FIRST_TICK + 200, 3, ANA, SOLDIER),
    ]) {
        pick(&mut world, tick, body, new);
        let kept = HeroSwitch {
            old,
            new,
            player: PLAYER,
        };
        assert_eq!(
            world.controller_script.as_ref().unwrap().hero_switch(),
            Some(kept)
        );
        let switch = world.notices.waiting.back().copied().unwrap();
        assert_eq!((switch.kept, switch.serial), (kept, serial));
        assert_eq!(world.switch, Some(switch));
        let script = world.controller_script.as_ref().unwrap();
        let (_, input) = world.select_input(true).unwrap();
        let plain = scene
            .controller()
            .controller_frame(script, 1, input)
            .unwrap();
        let notice = ControllerFrame {
            notice: Some(kept),
            ..input
        };
        let notice = scene
            .controller()
            .controller_frame(script, 1, notice)
            .unwrap();
        // Two asset values and an entity value with their presence bits.
        assert!(notice.bit_len() > plain.bit_len() + 3 * 4 + 2 * 17 + 32);
    }
}
