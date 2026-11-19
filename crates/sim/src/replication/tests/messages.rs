//! The room's game messages ([`crate::messages`]) between the real graphs
//! in the Practice Range: the mode's message to every entity reaches a
//! player's body, his controller and the Training Bots; Need Healing's
//! `0307` reaches an AI hero's body and not its sender's.

use super::*;
use crate::{
    ai::Difficulty,
    messages::{Players, Recipients, deliver},
};
use orpheus_content::maps::SpawnChoice;
use orpheus_script::{mode::EVERY_ENTITY, vm::Unknowns};
use orpheus_wire::entity::GAME_MODE_ENTITY;

const PLAYER: u32 = 0xa000_0100;
const AI: u32 = 0xa000_0200;
/// `0C8E` rid121 and `0C90` rid187 take it, with its parameter `0767.026`
/// in #20491e (the body) and #20491 (the controller).
const BODY_AND_CONTROLLER: u64 = 0x0240_0000_0000_0de0;
const FLAG: u32 = 0x767;
const FLAG_VARIABLE: u32 = 20491;
/// The Training Bot's `0BA8` rid28 takes it, its sender in #40.
const BOT_MESSAGE: u64 = 0x0240_0000_0000_0686;
/// Need Healing's message (`0686` rid19, `STU_57692020`); `05AD` rid15 on
/// every hero body takes it, its sender in #40.
const NEED_HEALING: u64 = 0x0240_0000_0000_0307;
const SENDER: u32 = 40;

/// The Range with every Training Bot's script running.
fn scene() -> (PreparedScene, u32) {
    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(Unknowns::Tolerate);
    assets.set_bot_unknowns(Unknowns::Tolerate);
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    let mut tick = FIRST_TICK;
    while !scene.bot_scripts_ready() {
        scene.step_bots(tick);
        tick += 1;
    }
    (scene, tick)
}

/// A started world of team 0 whose player (an AI hero's with `ai`) plays
/// Soldier: 76.
fn spawned(scene: &PreparedScene, card: &PlayerCard, entity: u32, ai: bool) -> WorldDelivery {
    let viewer = PlayerAppearance {
        entity,
        slot: if ai { 1 } else { 0 },
        team: 0,
        spawn: 0,
        room: SpawnChoice::First,
        card,
    };
    let mut world = if ai {
        WorldDelivery::headless(entity, 0x8000_7001, Difficulty::Easy.identifier())
    } else {
        WorldDelivery::new(entity)
    };
    world.start(scene, &viewer, FIRST_TICK).unwrap();
    world.created.insert(entity);
    world
        .present(
            scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
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

fn body_snapshot(world: &WorldDelivery) -> &Snapshot {
    world
        .body_script
        .as_ref()
        .unwrap()
        .runtime
        .snapshot()
        .unwrap()
}

/// Instance variable `id` of the first instance of `graph`.
fn instance_variable(snapshot: &Snapshot, graph: u32, id: u32) -> Option<ScriptValue> {
    snapshot
        .instances
        .values()
        .find(|instance| instance.graph == graph)
        .unwrap_or_else(|| panic!("no {graph:04X} instance"))
        .variables
        .get(&id)
        .cloned()
}

fn mail(sender: u32, message: u64, parameters: Variables) -> GameMessage {
    GameMessage {
        sender,
        target: EVERY_ENTITY,
        message,
        parameters,
    }
}

/// The mode's messages to every entity (as `ModeWorld` queues them) reach
/// the player's body (`0C8E`), his controller (`0C90`) and every Training
/// Bot (`0BA8`), each through its own entries.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn a_mode_message_to_every_entity_reaches_bodies_controllers_and_bots() {
    let (mut scene, tick) = scene();
    let card = PlayerCard::default();
    let mut world = spawned(&scene, &card, PLAYER, false);
    let messages = vec![
        mail(
            GAME_MODE_ENTITY,
            BODY_AND_CONTROLLER,
            Variables::from([(FLAG, ScriptValue::Bool(true))]),
        ),
        mail(GAME_MODE_ENTITY, BOT_MESSAGE, Variables::new()),
    ];
    let mut worlds = [&mut world];
    let mut players = Players::new(&mut worlds, tick);
    let (mut bots, placed) = scene.message_recipients(tick);
    let mut recipients: [&mut dyn Recipients; 3] = [&mut players, &mut bots, placed];
    deliver(messages, Vec::new(), &mut recipients);

    assert_eq!(
        body_snapshot(&world).variables.get(&FLAG_VARIABLE),
        Some(&ScriptValue::Bool(true))
    );
    let controller = world
        .controller_script
        .as_ref()
        .unwrap()
        .snapshot()
        .unwrap();
    assert_eq!(
        instance_variable(controller, 0x0c90, FLAG_VARIABLE),
        Some(ScriptValue::Bool(true))
    );
    let mut reached = 0;
    for (slot, bot) in scene.bots().iter().enumerate() {
        let snapshot = scene.bot_scripts().snapshot(slot, bot.entity).unwrap();
        if snapshot
            .instances
            .values()
            .any(|instance| instance.graph == 0x0ba8)
        {
            assert_eq!(
                instance_variable(snapshot, 0x0ba8, SENDER),
                Some(ScriptValue::Entity(GAME_MODE_ENTITY)),
                "{:08X}",
                bot.entity
            );
            reached += 1;
        }
    }
    assert!(reached > 0, "no Training Bot runs 0BA8");
}

/// Need Healing's `0307` from a player's body reaches the AI hero's body
/// (`05AD` rid15 keeps the sender), once; the sender's body takes nothing
/// more (its own instances took it from the VM when it was sent).
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle"]
fn need_healing_reaches_an_ai_heros_body_but_not_the_senders() {
    let (scene, tick) = scene();
    let card = PlayerCard::default();
    let mut player = spawned(&scene, &card, PLAYER, false);
    let mut ai = spawned(&scene, &card, AI, true);
    let sender = player.current_body().unwrap();
    let need_healing = |world: &WorldDelivery| {
        body_snapshot(world)
            .instances
            .values()
            .find(|instance| instance.graph == 0x05ad)
            .map(|instance| instance.variables.clone())
    };
    let before = need_healing(&player);
    let mut worlds = [&mut player, &mut ai];
    let mut players = Players::new(&mut worlds, tick);
    deliver(
        vec![mail(sender, NEED_HEALING, Variables::new())],
        Vec::new(),
        &mut [&mut players],
    );
    assert_eq!(
        instance_variable(body_snapshot(&ai), 0x05ad, SENDER),
        Some(ScriptValue::Entity(sender))
    );
    assert_eq!(need_healing(&player), before);
}
