//! Hero select from the controller graph (`crate::controller`): the
//! client's 21616 goes to the player's `288B`, and the controller frames
//! show what the script then has.

use super::*;
use lucy_content::{heroes::ANA, maps::SpawnChoice, parameters::OPEN_OR_CLOSE};

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
/// open is refused (rid22) and a pick too (rid24): no body changes. No
/// open shows the skin selector (`288B`'s #25377): `13C0` holds no `0692`,
/// so st35 never sets it, and each time it turns on the client's st38 says
/// Athena's hero select line again.
#[test]
#[ignore = "requires a derived 104319 pack"]
fn h_opens_a_pick_switches_and_288b_closes() {
    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
    let mut world = joined(&scene, &viewer);
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
    let skins = |world: &WorldDelivery| world.select_shown.is_some_and(|shown| shown.skins);
    assert!(!skins(&world), "no skin selector in the Range");
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
    assert!(!skins(&world), "a reopen shows no skin selector either");
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

/// The player's world in the Practice Range as he joins: started, the
/// mode graph's held messages (`13C0`'s `0727`) delivered, his controller's
/// first frame queued (hero select open).
fn joined(scene: &PreparedScene, viewer: &PlayerAppearance<'_>) -> WorldDelivery {
    let mut world = WorldDelivery::new(PLAYER);
    world.start(scene, viewer, FIRST_TICK).unwrap();
    let mode = crate::mode_script::ModeRunner::start(scene, FIRST_TICK, Vec::new());
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
            scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
    world
}

/// While the player has no body (hero select open from the join), his
/// controller still gets an owner chunk with the newest command frame
/// every 100 ms once its first frame has arrived. Without them the
/// client's controller host keeps every prediction it journals for 40
/// frames, then rolls back and replays by itself (`0x7FF78991B200`); the
/// rollback undid the begin of the client-made hero preview `2906`'s
/// st2 (`#699`), which then never ended the old preview model: the hero
/// hovered at the join stayed next to the next one (user report
/// 2026-11-20, `refs/reports/ControllerWorld.md`).
#[test]
#[ignore = "requires a derived 104319 pack"]
fn the_controller_gets_its_command_frame_before_the_first_body() {
    let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
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
    let mut world = joined(&scene, &viewer);
    let controller_chunks = |world: &WorldDelivery| -> Vec<u32> {
        world
            .queued
            .iter()
            .filter(|record| record.update.entity == PLAYER)
            .filter_map(|record| match record.delivery {
                Delivery::Script { last, .. } => Some(last),
                _ => None,
            })
            .collect()
    };
    let first = controller_chunks(&world);
    assert_eq!(first.len(), 1, "the first controller frame");
    // Nothing while the first frame is in flight; the next try is due
    // 100 ms later.
    world.send_controller_ack(FIRST_TICK + 1, Duration::ZERO);
    assert_eq!(controller_chunks(&world), first);
    assert!(world.controller.arrived(first[0]));

    // 16 ms ticks from 0 to 1008 ms: an ack at the first tick at or past
    // each due time, 112, 224, 336, 448, 560, 672, 784 and 896 ms (the next
    // is due at 996 and would go at 1008): 8.
    for step in 0..63u32 {
        let tick = FIRST_TICK + 2 + step;
        world.send_controller_ack(tick, Duration::from_millis(u64::from(step) * 16));
    }
    assert!(world.body.is_none());
    let acks = controller_chunks(&world).len() - 1;
    assert_eq!(acks, 8, "an owner chunk every 100 ms");
}
