//! HitPredict's acceptance check for input timing (netcode step 1;
//! `InputQueue`). Our body VM stands in for the client: it runs each
//! command as the client makes it, frame by frame, as the client predicts.
//! The server gets the commands through a network that delays, loses and
//! repeats datagrams, each one carrying the client's redundant window, and
//! runs them a tick at a time from its input buffer. While no tick finds
//! the buffer dry, the server's state after each command frame, which its
//! owner frame (CmFD) and its movement record name, equals the client's at
//! that frame: the client has nothing to correct. When two datagrams in a
//! row come late, a dry tick repeats the last input; the press it missed
//! still runs, so the server fires as many shots as the client.

use super::*;
use orpheus_script::snapshot::{ProjectedSnapshot, View};
use orpheus_wire::{
    commands::{Click, Command, FIRE, SUBFRAME},
    entity::Movement,
};

/// The client's redundant window: the commands since the last the server
/// acknowledged, at most this many.
const WINDOW: usize = 8;

/// A Soldier: 76 picked at the Practice Range spawn; `buffered` is the
/// server's input, else the client's VM, which runs each command at once.
fn soldier(scene: &PreparedScene, card: &PlayerCard, buffered: bool) -> WorldDelivery {
    let mut world = WorldDelivery::new(OWNER);
    if !buffered {
        world = world.unbuffered();
    }
    world.started = Some(FIRST_TICK);
    let hero = scene.heroes().get(SOLDIER).unwrap();
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
    assert!(world.body_script.is_some(), "Soldier: 76's body script");
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
            .advance_input(scene, &viewer(card), RETAIL_QUANTA, 64)
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
        assert_eq!(server.input.repeats, 0, "tick {tick}: the buffer ran dry");
        // Each frame's state as it ends (a tick may run more than one).
        while server.has_input_work() {
            let before = server.body_script.as_ref().unwrap().frame;
            server
                .advance_input(&scene, &viewer(&card), RETAIL_QUANTA, 1)
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
    // Every frame ran on the server, in order, as the client ran it.
    assert_eq!(
        server.body_script.as_ref().unwrap().frame,
        first + frames - 1
    );
    assert_eq!(compared, frames);
    assert_eq!(ammo(&server), ammo(&client));
}

/// Two datagrams late in a row: the second tick finds the buffer dry and
/// repeats the last input (no fire) for the frame of a tap; the tap's
/// press, when its command comes, runs with the next frame, so the server
/// fires the shot the client fired. The ack stays at the client's newest
/// command meanwhile, so the client keeps sending the frames the repeat
/// stood in for.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_tap_the_dry_buffer_missed_still_fires_once_its_command_comes() {
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
    // Frames 0 to 39 idle, a one-frame tap at 40, idle to 79. Datagrams 40
    // and 41 come with 42: the tick of 40 runs 39, the one in hand; the
    // tick of 41 finds the buffer dry and repeats 39's input for frame 40.
    let mut late = Vec::new();
    let mut repeated = None;
    let mut sent = Vec::new();
    // The tick after a command came runs it, the tick after the tap's
    // repeat waits for the next in hand: 79 runs at the tick of 80.
    for index in 0..=80 {
        let tick = FIRST_TICK + 1 + index;
        if index < 80 {
            let buttons = if index == 40 { FIRE | SUBFRAME } else { 0 };
            let made = command(index, buttons);
            client.queue_commands(tick, &[made]);
            run(&mut client, &scene, &card);
            sent.push(made);
            let window: Vec<_> = sent.iter().rev().take(WINDOW).rev().copied().collect();
            if index == 40 || index == 41 {
                late.push(window);
            } else {
                for window in late.drain(..) {
                    server.queue_commands(tick, &window);
                }
                server.queue_commands(tick, &window);
            }
        }
        server.release_input(1);
        if server.input.repeats != 0 && repeated.is_none() {
            // The tick that found the buffer dry ran a repeat of frame 39
            // (no fire) for the tap's frame 40; the ack stays at 39.
            repeated = Some((tick, server.input.admitted));
            assert_eq!(server.command_ack(tick), first + 39);
        }
        run(&mut server, &scene, &card);
    }
    let (tick, frame) = repeated.expect("a dry tick");
    assert_eq!(
        (tick, frame),
        (FIRST_TICK + 1 + 41, Some(first + 40)),
        "the repeat"
    );
    // One shot each: 29 rounds left.
    assert_eq!(ammo(&client), ScriptValue::Integer(29));
    assert_eq!(ammo(&server), ammo(&client));
    assert_eq!(
        server.body_script.as_ref().unwrap().frame,
        first + 79,
        "every frame ran"
    );
}
