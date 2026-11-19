//! HitPredict's acceptance check for input timing (netcode step 1;
//! `InputQueue`). Our body VM stands in for the client: it runs each
//! command as the client makes it, frame by frame, as the client predicts.
//! The server gets the commands through a network that delays, loses and
//! repeats datagrams, each one carrying the client's redundant window, and
//! runs them a tick at a time from its input buffer. The server's state
//! after each command frame, which its owner frame (CmFD) and its movement
//! record name, equals the client's at that frame: the client has nothing
//! to correct. That holds when the client's datagrams stop for a while
//! too: the ticks that find the buffer dry run nothing, and a press in the
//! frames that came late runs at its own frame. A client that stays
//! silent past the wait gets his last input repeated a few frames, then
//! neutral frames.

use super::*;
use orpheus_script::snapshot::{ProjectedSnapshot, View};
use orpheus_wire::{
    commands::{Click, Command, FIRE, JUMP, SUBFRAME},
    entity::Movement,
};

/// The client's redundant window: the commands since the last the server
/// acknowledged, at most this many.
const WINDOW: usize = 8;

/// A Soldier: 76 picked at the Practice Range spawn; `buffered` is the
/// server's input, else the client's VM, which runs each command at once.
fn soldier(scene: &PreparedScene, card: &PlayerCard, buffered: bool) -> WorldDelivery {
    picked(scene, card, SOLDIER, buffered)
}

fn picked(scene: &PreparedScene, card: &PlayerCard, hero: u64, buffered: bool) -> WorldDelivery {
    let mut world = WorldDelivery::new(OWNER);
    if !buffered {
        world = world.unbuffered();
    }
    world.started = Some(FIRST_TICK);
    let hero = scene.heroes().get(hero).unwrap();
    world
        .switch(
            scene,
            &viewer(card),
            FIRST_TICK,
            RETAIL_QUANTA,
            OWNER | 1,
            (hero, SkinTheme::default()),
            true,
        )
        .unwrap();
    assert!(world.body_script.is_some(), "the hero's body script");
    world
}

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

fn run(world: &mut WorldDelivery, scene: &PreparedScene, card: &PlayerCard) {
    while world.has_input_work() {
        world
            .advance_input(scene, &viewer(card), RETAIL_QUANTA, 64, None)
            .unwrap();
    }
}

/// What the client compares with its prediction at a frame: the owner's
/// statescript as his owner frame projects it, and his body's movement.
fn state(world: &WorldDelivery) -> (u32, ProjectedSnapshot, Movement) {
    let script = world.body_script.as_ref().unwrap();
    let snapshot = script.runtime.snapshot().unwrap();
    let projected = script
        .runtime
        .encoder()
        .project(snapshot, View::Owner)
        .unwrap();
    (
        script.frame,
        projected,
        world.mover.as_ref().unwrap().pose(),
    )
}

/// The rifle's ammunition (`0B94`... instance 11, variable 53, as the
/// recovery tests read it).
fn ammo(world: &WorldDelivery) -> ScriptValue {
    let snapshot = world
        .body_script
        .as_ref()
        .unwrap()
        .runtime
        .snapshot()
        .unwrap();
    snapshot
        .resolved_bindings
        .get(&(11, 53))
        .unwrap_or(&snapshot.instances[&11].variables[&53])
        .clone()
}

/// The client's input at its `index`th frame: it stands, walks, holds fire,
/// taps five times (each press with its sub-frame click), then walks
/// firing through the magazine and its reload, and stands again.
fn input(index: u32, frame: u32) -> Command {
    let tap = (150..200).contains(&index) && index % 10 == 0;
    let press = index == 90 || index == 200 || tap;
    let fire = (90..130).contains(&index) || tap || (200..520).contains(&index);
    let walk = (30..90).contains(&index) || (200..520).contains(&index);
    let yaw = if index < 300 { 0 } else { 4096 };
    Command {
        frame,
        forward: if walk { 127 } else { 0 },
        yaw,
        buttons: if fire { FIRE } else { 0 } | if press { SUBFRAME } else { 0 },
        click: press.then_some(Click {
            subframe: 100,
            yaw: yaw as i16,
            pitch: 0,
        }),
        ..Command::default()
    }
}

/// When the client's `index`th datagram reaches the server, in ticks after
/// it was sent (none: lost, the next window carries its commands), and
/// whether it arrives twice.
fn delivery(index: u32) -> (Option<u32>, bool) {
    match index {
        // Late by a tick, alone and twice in a row: the waiting command
        // runs on time.
        45 | 95 | 151 | 152 | 300 => (Some(1), false),
        // Lost, while fire is held and at a tap.
        110 | 170 => (None, false),
        // Late by a tick after a duplicate.
        60 => (Some(0), true),
        61 => (Some(1), false),
        // A client stall of five frames (80 ms) over the press at 200:
        // everything comes with 203's datagram.
        198..=202 => (Some(203 - index), false),
        _ => (Some(0), false),
    }
}

#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn the_server_runs_each_frame_as_the_client_predicted_it_while_nothing_is_lost() {
    let scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let card = PlayerCard::default();
    let mut server = soldier(&scene, &card, true);
    let mut client = soldier(&scene, &card, false);
    assert_eq!(state(&server), state(&client));

    // The client's command frames start after the body's first frame.
    let first = FIRST_TICK + 1;
    let frames = 560;
    let mut predicted = BTreeMap::new();
    let mut in_flight: Vec<(u32, Vec<Command>)> = Vec::new();
    let mut sent: Vec<Command> = Vec::new();
    let mut compared = 0;
    // The server runs a frame the tick after it came, the next in hand:
    // the last, a tick after the client's last.
    let mut dry = Vec::new();
    for index in 0..=frames {
        let tick = FIRST_TICK + 1 + index;
        if index < frames {
            // The client predicts its frame and sends its window.
            let command = input(index, first + index);
            client.queue_commands(tick, &[command]);
            run(&mut client, &scene, &card);
            let (frame, projected, pose) = state(&client);
            assert_eq!(frame, command.frame);
            predicted.insert(frame, (projected, pose));
            sent.push(command);
            let acked = server.command_ack(tick);
            let window: Vec<_> = sent
                .iter()
                .rev()
                .take(WINDOW)
                .rev()
                .filter(|command| command.frame + 1 >= acked)
                .copied()
                .collect();
            let (delay, twice) = delivery(index);
            if let Some(delay) = delay {
                for _ in 0..1 + u32::from(twice) {
                    in_flight.push((tick + delay, window.clone()));
                }
            }
        }
        // The server's tick: what arrived, then its command.
        for (_, window) in in_flight.iter().filter(|(due, _)| *due == tick) {
            server.queue_commands(tick, window);
        }
        in_flight.retain(|(due, _)| *due != tick);
        server.release_input(1);
        if server.input.dry != 0 {
            dry.push(index);
        }
        // Each frame's state as it ends (a tick may run more than one).
        while server.has_input_work() {
            let before = server.body_script.as_ref().unwrap().frame;
            server
                .advance_input(&scene, &viewer(&card), RETAIL_QUANTA, 1, None)
                .unwrap();
            let (frame, projected, pose) = state(&server);
            if frame == before {
                continue;
            }
            // The ack never passes the frame the owner frame names.
            assert!(server.command_ack(tick) <= frame + 1);
            let (expected, expected_pose) = &predicted[&frame];
            assert_eq!(
                projected, *expected,
                "frame {frame}: the owner's statescript"
            );
            assert_eq!(pose, *expected_pose, "frame {frame}: the body's movement");
            compared += 1;
        }
    }
    // Only the stall dried the buffer: the tick of 198 ran 197, the one in
    // hand; those of 199 to 202 found none and waited (MAX_WAIT, 4); that
    // of 203 got 198 to 203 and ran all but the last 4 (198, 199), and 200,
    // the press, ran at the next, at its frame, as every frame compared
    // above.
    assert_eq!(dry, (199..=202).collect::<Vec<_>>());
    // Every frame ran on the server, in order, as the client ran it.
    assert_eq!(
        server.body_script.as_ref().unwrap().frame,
        first + frames - 1
    );
    assert_eq!(compared, frames);
    assert_eq!(ammo(&server), ammo(&client));
}

/// A stall longer than the wait: past `MAX_WAIT` dry ticks the server
/// repeats the last input (no fire) each tick, for the frame of a tap too;
/// the tap's press, when its command comes, runs with the next frame, so
/// the server fires the shot the client fired. The ack stays at the
/// client's newest command meanwhile, so the client keeps sending the
/// frames the repeats stood in for.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_tap_a_long_stall_repeated_still_fires() {
    let scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let card = PlayerCard::default();
    let mut server = soldier(&scene, &card, true);
    let mut client = soldier(&scene, &card, false);
    let first = FIRST_TICK + 1;
    let command = |index: u32, buttons: u32| Command {
        frame: first + index,
        buttons,
        click: (buttons & SUBFRAME != 0).then_some(Click {
            subframe: 40,
            yaw: 0,
            pitch: 0,
        }),
        ..Command::default()
    };
    // Frames 0 to 39 idle, a one-frame tap at 40, idle to 79. Datagrams
    // 37 to 45 come with 46: the tick of 37 runs 36, the one in hand; those
    // of 38 to 41 wait; those of 42 to 45 repeat 36's input (`MAX_REPEATS`,
    // 4) for frames 37 to 40, the tap's among them.
    let mut late = Vec::new();
    let mut repeats = Vec::new();
    let mut sent = Vec::new();
    for index in 0..80 {
        let tick = FIRST_TICK + 1 + index;
        let buttons = if index == 40 { FIRE | SUBFRAME } else { 0 };
        let made = command(index, buttons);
        client.queue_commands(tick, &[made]);
        run(&mut client, &scene, &card);
        sent.push(made);
        let window: Vec<_> = sent.iter().rev().take(WINDOW).rev().copied().collect();
        if (37..=45).contains(&index) {
            late.push(window);
        } else {
            for window in late.drain(..) {
                server.queue_commands(tick, &window);
            }
            server.queue_commands(tick, &window);
        }
        server.release_input(1);
        // Ticks the clock asks the client to go four times as fast.
        if server.input.dry != 0 {
            assert_eq!(server.input.clock, CLOCK_FASTER | CLOCK_STRONGER);
        }
        if server.input.dry > MAX_WAIT {
            repeats.push((index, server.input.admitted));
            assert_eq!(server.command_ack(tick), first + 36);
        }
        run(&mut server, &scene, &card);
    }
    assert_eq!(
        repeats,
        (42..=45)
            .map(|index| (index, Some(first + index - 5)))
            .collect::<Vec<_>>(),
        "the repeats"
    );
    // One shot each: 29 rounds left.
    assert_eq!(ammo(&client), ScriptValue::Integer(29));
    assert_eq!(ammo(&server), ammo(&client));
}

/// A client that stops sending while his body walks, fires and jumps:
/// past the wait the server repeats his last input for `MAX_REPEATS`
/// frames, then runs a neutral frame each tick, his aim with nothing held.
/// The body, in the air by then, goes on rising and falls to the ground
/// (gravity runs every frame), but it no longer walks, jumps or fires.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_silent_body_keeps_falling_but_holds_nothing_past_the_repeats() {
    let scene = SceneAssets::load(orpheus_testkit::runtime_dir())
        .unwrap()
        .prepare(GameMap::practice_range())
        .unwrap();
    let card = PlayerCard::default();
    let mut server = soldier(&scene, &card, true);
    let first = FIRST_TICK + 1;
    // Idle to 19, walking with fire held from 20, jump held too from 40;
    // nothing after 44. In a steady stream the tick of 45 runs 44, the one
    // in hand; those of 46 to 49 wait; those of 50 to 53 repeat 44's input
    // for frames 45 to 48; from 54 each runs a neutral frame, 49 on.
    let last = 44;
    // Away from the spawn's facing, where a ledge stands.
    let yaw = 33280;
    let command = |index: u32| Command {
        frame: first + index,
        forward: if index >= 20 { 127 } else { 0 },
        yaw,
        buttons: if index >= 20 { FIRE } else { 0 } | if index >= 40 { JUMP } else { 0 },
        ..Command::default()
    };
    let ticks = last + 100;
    let mut sent = Vec::new();
    let mut ran = Vec::new();
    let mut ground = None;
    let mut neutral = Vec::new();
    let mut repeated_ammo = None;
    for index in 0..=ticks {
        let tick = FIRST_TICK + 1 + index;
        if index <= last {
            sent.push(command(index));
            let window: Vec<_> = sent.iter().rev().take(WINDOW).rev().copied().collect();
            server.queue_commands(tick, &window);
        }
        server.release_input(1);
        ran.extend(server.input.work.iter().filter_map(|work| match work {
            InputWork::Command { command, .. } if command.frame > first + last => Some(*command),
            _ => None,
        }));
        run(&mut server, &scene, &card);
        let (frame, _, pose) = state(&server);
        if frame == first + 39 {
            ground = Some(pose.position[1]);
        }
        if frame == first + last + MAX_REPEATS {
            repeated_ammo = Some(ammo(&server));
        }
        if frame > first + last + MAX_REPEATS {
            neutral.push(pose);
        }
    }
    // One frame a tick from the tick of 50 on.
    let frames = ticks - 49;
    assert_eq!(
        server.body_script.as_ref().unwrap().frame,
        first + last + frames
    );
    assert_eq!(ran.len() as u32, frames);
    let (repeats, neutrals) = ran.split_at(MAX_REPEATS as usize);
    for repeat in repeats {
        assert_eq!(
            *repeat,
            Command {
                frame: repeat.frame,
                ..command(last)
            }
        );
    }
    for frame in neutrals {
        assert_eq!(
            *frame,
            Command {
                frame: frame.frame,
                yaw,
                ..Command::default()
            }
        );
    }
    // It fired until the repeats ended, and not after.
    let ScriptValue::Integer(rounds) = repeated_ammo.unwrap() else {
        panic!("the rifle's ammunition");
    };
    assert!(rounds < 30, "{rounds} rounds left");
    assert_eq!(ammo(&server), ScriptValue::Integer(rounds));
    // In the air at the first neutral frame; each later frame in the air
    // goes up slower, then falls faster (gravity), and it lands where it
    // jumped from and stands still: no throttle in any neutral frame.
    let ground = ground.unwrap();
    assert!(neutral[0].position[1] > ground + 0.1, "{neutral:?}");
    let landed = neutral
        .iter()
        .position(|pose| (pose.position[1] - ground).abs() < 1e-3)
        .expect("it lands");
    let air = &neutral[..landed];
    assert!(
        air.windows(2)
            .all(|pair| pair[1].velocity[1] < pair[0].velocity[1]),
        "{neutral:?}"
    );
    let falling: Vec<_> = air.iter().filter(|pose| pose.velocity[1] < 0.0).collect();
    assert!(falling.len() >= 10, "{neutral:?}");
    assert!(
        falling
            .windows(2)
            .all(|pair| pair[1].position[1] < pair[0].position[1]),
        "{neutral:?}"
    );
    assert!(neutral.iter().all(|pose| pose.throttles == [0, 0]));
    let end = neutral.last().unwrap();
    assert!((end.position[1] - ground).abs() < 1e-3);
    assert!(end.velocity[0].hypot(end.velocity[2]) < 1e-3, "{end:?}");
}

/// Widowmaker's Grappling Hook (`005A`) from the Range spawn: the press
/// (Shift, logical button 35) casts the hook (`STU_4BBC5D09`), which flies
/// 40 m/s until it meets the map, and the `MoveTo` pulls her toward it at
/// 20 m/s; the ability ends after the pull. The server, its commands
/// buffered a tick, has her where her client predicted her on every frame.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn the_server_pulls_widowmaker_along_her_grappling_hook_as_her_client_predicts() {
    const WIDOWMAKER: u64 = 0x02e0_0000_0000_000a;
    const HOOK: u32 = 0x08;
    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let card = PlayerCard::default();
    // Up a little along the spawn's facing (+z), where the Range's ledge
    // stands within the hook's 20 m.
    let (yaw, pitch) = (0, -3000);
    let input = |index: u32, frame: u32| Command {
        frame,
        yaw,
        pitch,
        buttons: if index == 30 { HOOK } else { 0 },
        ..Command::default()
    };
    let mut server = picked(&scene, &card, WIDOWMAKER, true);
    let mut client = picked(&scene, &card, WIDOWMAKER, false);
    let hook = |world: &WorldDelivery, id: u32| {
        let snapshot = world
            .body_script
            .as_ref()
            .unwrap()
            .runtime
            .snapshot()
            .unwrap();
        let (_, instance) = snapshot
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == 0x005a)
            .unwrap();
        instance.variables.get(&id).cloned()
    };
    let first = FIRST_TICK + 1;
    let frames = 200;
    let mut predicted = BTreeMap::new();
    let mut start = None;
    let mut farthest: f64 = 0.0;
    for index in 0..=frames {
        let tick = FIRST_TICK + 1 + index;
        if index < frames {
            let command = input(index, first + index);
            client.queue_commands(tick, &[command]);
            run(&mut client, &scene, &card);
            let (frame, projected, pose) = state(&client);
            predicted.insert(frame, (projected, pose));
            let at = pose.position;
            let from = *start.get_or_insert(at);
            farthest = farthest.max(
                (0..3)
                    .map(|axis| (at[axis] - from[axis]).powi(2))
                    .sum::<f64>()
                    .sqrt(),
            );
            server.queue_commands(tick, &[command]);
        }
        server.release_input(1);
        while server.has_input_work() {
            let before = server.body_script.as_ref().unwrap().frame;
            server
                .advance_input(&scene, &viewer(&card), RETAIL_QUANTA, 1, None)
                .unwrap();
            let (frame, projected, pose) = state(&server);
            if frame == before {
                continue;
            }
            let (expected, expected_pose) = &predicted[&frame];
            assert_eq!(pose, *expected_pose, "frame {frame}: the body's movement");
            assert_eq!(
                projected, *expected,
                "frame {frame}: the owner's statescript"
            );
        }
    }
    // The hook met the map (#435) and pulled her along it.
    assert_eq!(hook(&client, 435), Some(ScriptValue::Bool(true)));
    assert!(farthest > 2.0, "she moved {farthest} m");
    assert_eq!(hook(&server, 435), hook(&client, 435));
    // The pull (st1) and the cast (st0) are over.
    let snapshot = client
        .body_script
        .as_ref()
        .unwrap()
        .runtime
        .snapshot()
        .unwrap();
    let grapple = snapshot
        .instances
        .values()
        .find(|instance| instance.graph == 0x005a)
        .unwrap();
    assert!(
        [0, 1]
            .iter()
            .all(|raw| grapple.states.get(raw).is_none_or(|state| !state.active)),
        "the grapple ended"
    );
}
