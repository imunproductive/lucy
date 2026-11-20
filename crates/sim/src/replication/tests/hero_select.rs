//! Hero select from the controller graph (`crate::controller`): the
//! client's 21616 goes to the player's `288B`, and the controller frames
//! show what the script then has.

use super::*;
use orpheus_content::{heroes::ANA, maps::SpawnChoice, parameters::OPEN_OR_CLOSE};

const PLAYER: u32 = 0xa000_0100;

/// A 21616 of the player: `guid` with `parameters`.
fn request(guid: u64, parameters: Vec<u8>) -> Message {
    Message {
        id: CLIENT_GAME_MESSAGE,
        fields: Fields::from([
            (
                0x78,
                Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(PLAYER)))])),
            ),
            (0x80, Value::Blob(parameters)),
            (0xa8, Value::Unsigned(guid)),
            (0xb0, Value::Signed(4)),
        ]),
    }
}

/// An open (true) or close as the client sends it: the bool, then the two
/// entity words.
fn open_or_close(open: bool) -> Message {
    let mut parameters = vec![1, u8::from(open)];
    parameters.extend(PLAYER.to_le_bytes());
    parameters.extend(PLAYER.to_le_bytes());
    request(OPEN_OR_CLOSE, parameters)
}

/// In the Practice Range: hero select is open from the start (`13C0`'s
/// `0727`, which the room gives the controller: `0C90` st28 → `0738` →
/// `288B` st134); a pick of Soldier: 76 gives him his body with the screen
/// still open, and the next tick `288B` closes it (st95: the picked hero's
/// body lives), each in a controller frame. H opens it again and clears the
/// pick; a pick of Ana switches; a close closes. Outside the spawn room an
/// open is refused (rid22) and a pick too (rid24): no body changes.
#[test]
#[ignore = "requires a derived 104319 pack"]
fn h_opens_a_pick_switches_and_288b_closes() {
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
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
    let mode = crate::mode_script::ModeRunner::start(&scene, FIRST_TICK, Vec::new());
    assert!(mode.graph_driven(), "{:?}", mode.gaps());
    let held = crate::mode_script::held(Some(&mode));
    {
        let mut worlds = [&mut world];
        let mut players = crate::messages::Players::new(&mut worlds, FIRST_TICK);
        crate::messages::deliver_held(
            &held,
            &mut crate::messages::HeldReach::default(),
            &mut [&mut players],
        );
    }
    world.created.insert(PLAYER);
    world
        .present(
            &scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
    let shown = |world: &WorldDelivery| {
        world
            .select_shown
            .map(|shown| (shown.open, shown.pick, shown.hero))
    };
    let frames = |world: &WorldDelivery| {
        world
            .queued
            .iter()
            .filter(|record| record.update.entity == PLAYER && record.update.chunk.is_some())
            .count()
    };
    assert_eq!(shown(&world), Some((true, None, None)));
    assert_eq!(frames(&world), 1);

    let mut tick = FIRST_TICK + 1;
    let message = |world: &mut WorldDelivery, tick: u32, message: Message| {
        world
            .game_message(
                &scene,
                &viewer,
                tick,
                Duration::ZERO,
                RETAIL_QUANTA,
                &message,
            )
            .unwrap();
        world.send_hero_select(&scene, tick).unwrap();
    };
    message(
        &mut world,
        tick,
        request(PICK_HERO, SOLDIER.to_le_bytes().to_vec()),
    );
    assert_eq!(world.hero(), Some(SOLDIER));
    assert_eq!(shown(&world), Some((true, Some(SOLDIER), Some(SOLDIER))));
    assert_eq!(frames(&world), 2);
    tick += 1;
    world.send_hero_select(&scene, tick).unwrap();
    assert_eq!(shown(&world), Some((false, Some(SOLDIER), Some(SOLDIER))));
    assert_eq!(frames(&world), 3);

    tick += 1;
    message(&mut world, tick, open_or_close(true));
    assert_eq!(shown(&world), Some((true, None, Some(SOLDIER))));
    tick += 1;
    message(
        &mut world,
        tick,
        request(PICK_HERO, ANA.to_le_bytes().to_vec()),
    );
    assert_eq!(world.hero(), Some(ANA));
    tick += 1;
    world.send_hero_select(&scene, tick).unwrap();
    assert_eq!(shown(&world), Some((false, Some(ANA), Some(ANA))));
    tick += 1;
    message(&mut world, tick, open_or_close(true));
    assert_eq!(shown(&world), Some((true, None, Some(ANA))));
    tick += 1;
    message(&mut world, tick, open_or_close(false));
    assert_eq!(shown(&world), Some((false, None, Some(ANA))));

    // Out of the spawn room: no open, no pick.
    world.spawn_room = Some(false);
    tick += 1;
    message(&mut world, tick, open_or_close(true));
    assert_eq!(shown(&world), Some((false, None, Some(ANA))));
    message(
        &mut world,
        tick + 1,
        request(PICK_HERO, SOLDIER.to_le_bytes().to_vec()),
    );
    assert_eq!(world.hero(), Some(ANA));
    let snapshot = world
        .controller_script
        .as_ref()
        .unwrap()
        .snapshot()
        .unwrap();
    assert!(
        snapshot.initialization_complete(),
        "{:?}",
        snapshot.diagnostics
    );
}
