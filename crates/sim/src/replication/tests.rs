use super::*;
use crate::{instance::RETAIL_QUANTA, scene::PlayerCard, scene_assets::SceneAssets};
use orpheus_content::{
    heroes::SOLDIER,
    maps::GameMap,
    parameters::{CLIENT_GAME_MESSAGE, PICK_HERO},
};
use orpheus_core::{bits::BitReader, tick::FIRST_TICK};
use orpheus_physics::{
    collision::index::TriangleIndex, contact::world::ContactWorld, mover::settings::MoverSettings,
};
use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    timeline::FRAME_MS,
    vm::{Component, EmptyWorld, MAX_INIT_WORK, Root},
};
use orpheus_wire::{
    commands::{FIRE, SUBFRAME},
    entity::{ComponentValue, MAX_ENTITY_RECORDS, Movement},
    jam::codec::{Fields, Value},
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

fn forward(frame: u32) -> Command {
    Command {
        frame,
        forward: 127,
        ..Command::default()
    }
}

#[test]
fn bot_health_replaces_stale_packets_and_respawn_keeps_creation_order() {
    use crate::{bots::BOT_ENTITY, egress::PacketDelivery};
    use orpheus_script::vitals::health;
    use orpheus_wire::entity::Create;
    let old = BOT_ENTITY;
    let new = old + 1;
    let create = |entity| EntityUpdate {
        entity,
        operation: Some(Operation::Create(Box::new(Create::new(1, vec![])))),
        ..EntityUpdate::default()
    };
    let health_update = |now| EntityUpdate {
        entity: old,
        operation: Some(Operation::Update(vec![health(now, 200.0, 0.0, 0.0)])),
        ..EntityUpdate::default()
    };
    let send = |world: &mut WorldDelivery, index: usize| {
        let ready = &world.ready()[index];
        let record = ready.tracked();
        let queue_index = ready.index;
        world.sent(&[queue_index]);
        record
    };
    let resolve = |world: &mut WorldDelivery, record, arrived| {
        world.acknowledge(
            DeliveryReport {
                requeued_reliable: 0,
                packets: vec![PacketDelivery {
                    sequence: 0,
                    arrived,
                    records: vec![record],
                }],
            },
            Duration::ZERO,
        );
    };
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(FIRST_TICK);
    world.bots.insert(old, BotLifetime::Live);
    world.push(Record {
        update: create(old),
        delivery: Delivery::BotCreate { after: None },
    });
    world.queue_health(health_update(160.0));
    assert_eq!(world.ready().len(), 1);
    assert!(!world.identity_ready(old));
    let spawn = send(&mut world, 0);
    assert!(world.ready().is_empty());
    resolve(&mut world, spawn, true);
    assert!(world.identity_ready(old));

    let earlier_health = send(&mut world, 0);
    world.queue_health(health_update(0.0));
    resolve(&mut world, earlier_health, false);
    let ready = world.ready();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].record.update, health_update(0.0));
    let terminal = send(&mut world, 0);
    resolve(&mut world, Arc::clone(&terminal), false);
    assert!(world.health_pending(old));
    let terminal_retry = send(&mut world, 0);
    assert!(Arc::ptr_eq(&terminal, &terminal_retry));
    resolve(&mut world, terminal_retry, true);
    assert!(!world.health_pending(old));

    world.replace_bot(BotReplacement {
        retired: old,
        created: create(new),
        link: None,
    });
    assert!(!world.identity_ready(old));
    let ready = world.ready();
    assert_eq!(
        ready
            .iter()
            .map(|item| item.record.update.entity)
            .collect::<Vec<_>>(),
        [old, new]
    );
    let destroy = send(&mut world, 0); // the datagram only had room for the destroy
    assert!(world.ready().is_empty());
    resolve(&mut world, destroy, false);
    let retry = send(&mut world, 0);
    resolve(&mut world, retry, true);
    let replacement = send(&mut world, 0);
    assert_eq!(replacement.update.entity, new);
    resolve(&mut world, replacement, true);
    resolve(&mut world, terminal, false);
    world.queue_health(health_update(120.0));
    assert!(world.ready().is_empty());
    assert!(!world.identity_ready(old));
    assert!(world.identity_ready(new));
}

/// A bot can be struck by a client's command only once the datagram with
/// its first movement record arrived (the client draws it from its movement
/// records) and the command came in a later datagram; a lost first record
/// waits for the next one. Its create's arrival alone isn't enough.
#[test]
fn a_bot_is_struck_only_by_commands_made_after_its_first_movement_arrived() {
    use crate::bots::BOT_ENTITY;
    let [bot, other] = [BOT_ENTITY, BOT_ENTITY + 1];
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(100);
    for entity in [bot, other] {
        world.bots.insert(entity, BotLifetime::Live);
        world.created.insert(entity);
    }
    let moving = |entity| EntityUpdate {
        entity,
        movement: Some(Movement::default()),
        ..EntityUpdate::default()
    };
    let frames = |range: std::ops::RangeInclusive<u32>| range.map(forward).collect::<Vec<_>>();
    let mut egress = GameEgress::default();
    world.queue_commands(100, &frames(100..=103));
    assert!(world.identity_ready(bot));
    assert!(!world.bot_shown(bot, 104));

    // Datagram 5 carries both bots' first records; 6 the next ones.
    for sequence in [5, 6] {
        let sent = world
            .send_frame(
                100 + sequence,
                vec![moving(bot), moving(other)],
                false,
                &mut egress,
                |_| Ok::<_, ()>(sequence),
            )
            .unwrap();
        assert_eq!(
            sent.records,
            [RecordSource::Movement(0), RecordSource::Movement(1)]
        );
    }
    // The client has seen up to 4: nothing about 5 yet.
    world.acknowledge_movement(5, 0b1);
    world.queue_commands(104, &frames(104..=105));
    assert!(!world.bot_shown(bot, 106));
    // 5 arrived: commands of this datagram (106, 107) were made before
    // the client could draw the bot; those of the next one after it.
    world.acknowledge_movement(6, 0b1);
    world.queue_commands(106, &frames(106..=107));
    assert!(!world.bot_shown(bot, 107));
    assert!(world.bot_shown(bot, 108));
    assert!(world.bot_shown(other, 108));

    // A replacement starts again. Its first record (datagram 7) is lost;
    // 8 went while 7 was pending, so the next one counted is 9's.
    let new = BOT_ENTITY + 15;
    world.replace_bot(BotReplacement {
        retired: bot,
        created: EntityUpdate {
            entity: new,
            ..EntityUpdate::default()
        },
        link: None,
    });
    assert!(!world.bot_shown(bot, 108));
    world.created.insert(new);
    for sequence in [7, 8] {
        world
            .send_frame(
                100 + sequence,
                vec![moving(new)],
                false,
                &mut egress,
                |_| Ok::<_, ()>(sequence),
            )
            .unwrap();
    }
    world.acknowledge_movement(9, 0b01);
    world.queue_commands(108, &frames(108..=109));
    assert!(!world.bot_shown(new, 110));
    world
        .send_frame(110, vec![moving(new)], false, &mut egress, |_| {
            Ok::<_, ()>(9)
        })
        .unwrap();
    world.acknowledge_movement(10, 0b1);
    world.queue_commands(110, &frames(110..=110));
    assert!(!world.bot_shown(new, 110));
    assert!(world.bot_shown(new, 111));
}

/// A respawned bot's stand-in controller names the new body (component 44)
/// only with or after the body's create, and again if lost; the link of a
/// body that a newer respawn retired goes no more.
#[test]
fn a_respawned_bots_controller_link_follows_its_new_body() {
    use crate::{
        bots::{BOT_CONTROLLER_ENTITY, BOT_ENTITY},
        egress::PacketDelivery,
    };
    use orpheus_wire::entity::{Component, ComponentValue, Create};
    let controller = BOT_CONTROLLER_ENTITY;
    let [old, new, newer] = [BOT_ENTITY, BOT_ENTITY + 15, BOT_ENTITY + 16];
    let create = |entity| EntityUpdate {
        entity,
        operation: Some(Operation::Create(Box::new(Create::new(1, vec![])))),
        ..EntityUpdate::default()
    };
    let link = |body| EntityUpdate {
        entity: controller,
        operation: Some(Operation::Update(vec![Component {
            index: 44,
            fields: vec![Some(ComponentValue::Entity(body)); 3],
        }])),
        ..EntityUpdate::default()
    };
    let ready = |world: &WorldDelivery| -> Vec<EntityUpdate> {
        world
            .ready()
            .iter()
            .map(|item| item.record.update.clone())
            .collect()
    };
    let send = |world: &mut WorldDelivery| {
        let ready = &world.ready()[0];
        let record = ready.tracked();
        let index = ready.index;
        world.sent(&[index]);
        record
    };
    let resolve = |world: &mut WorldDelivery, record, arrived| {
        world.acknowledge(
            DeliveryReport {
                requeued_reliable: 0,
                packets: vec![PacketDelivery {
                    sequence: 0,
                    arrived,
                    records: vec![record],
                }],
            },
            Duration::ZERO,
        );
    };
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(FIRST_TICK);
    world.bots.insert(old, BotLifetime::Live);
    world.created.insert(old);
    world.replace_bot(BotReplacement {
        retired: old,
        created: create(new),
        link: Some(link(new)),
    });
    // All three can go in one datagram, the link after the create.
    assert_eq!(ready(&world).len(), 3);
    assert_eq!(ready(&world)[2], link(new));
    // Only the destroy went: the create waits for it, and the link for the
    // create.
    let destroy = send(&mut world);
    assert!(ready(&world).is_empty());
    resolve(&mut world, destroy, true);
    assert_eq!(ready(&world), [create(new), link(new)]);
    // The create went alone: the link waits until it arrived.
    let created = send(&mut world);
    assert!(ready(&world).is_empty());
    resolve(&mut world, created, true);
    assert_eq!(ready(&world), [link(new)]);
    let first = send(&mut world);
    resolve(&mut world, first, false);
    assert_eq!(ready(&world), [link(new)]);

    // The bot died again and came back before the link arrived: only the
    // newest body's link goes.
    world.replace_bot(BotReplacement {
        retired: new,
        created: create(newer),
        link: Some(link(newer)),
    });
    let updates = ready(&world);
    assert!(!updates.contains(&link(new)));
    assert_eq!(updates.last(), Some(&link(newer)));
}

#[test]
fn sparse_vm_work_retains_the_reference_release_and_timer_tail() {
    use BodyStep::{UnposedBoundary, UnposedTimer};
    assert_eq!(ScriptGap::before(100, 101).count(), 0);
    assert_eq!(
        ScriptGap::before(100, 102).collect::<Vec<_>>(),
        [(101, UnposedBoundary)]
    );
    assert_eq!(
        ScriptGap::before(100, 104).collect::<Vec<_>>(),
        [
            (101, UnposedBoundary),
            (102, UnposedTimer),
            (103, UnposedBoundary),
        ]
    );
    let frames = ScriptGap::before(100, 500).collect::<Vec<_>>();
    assert_eq!(frames.len(), 258);
    assert_eq!(frames[0], (101, UnposedBoundary));
    assert!(
        frames[1..257]
            .iter()
            .copied()
            .eq((243..499).map(|frame| (frame, UnposedTimer)))
    );
    assert_eq!(frames[257], (499, UnposedBoundary));
}

#[test]
fn native_command_window_keeps_duplicates_and_future_input_unacknowledged() {
    let (fresh, boundary) = command_prefix(
        Some(1000),
        1000,
        &[
            forward(1000),
            forward(1001),
            forward(1096),
            forward(1097),
            forward(20_000),
        ],
    );
    assert_eq!(fresh, [forward(1001), forward(1096)]);
    assert_eq!(boundary, None);

    let (fresh, boundary) = command_prefix(None, 1000, &[forward(MAX_LEASE_FRAME + 1)]);
    assert!(fresh.is_empty());
    assert_eq!(boundary, Some(MAX_LEASE_FRAME + 1));
    let (fresh, boundary) = command_prefix(
        None,
        MAX_LEASE_FRAME - 2,
        &[forward(MAX_LEASE_FRAME - 1), forward(MAX_LEASE_FRAME + 1)],
    );
    assert_eq!(fresh, [forward(MAX_LEASE_FRAME - 1)]);
    assert_eq!(boundary, Some(MAX_LEASE_FRAME + 1));
}

#[test]
fn overlapping_bursts_enqueue_each_frame_once_and_do_not_ack_pending_work() {
    let mut world = WorldDelivery::new(0xa000_0100);
    world.started = Some(1000);
    let first: Vec<_> = (980..1012).map(forward).collect();
    let next: Vec<_> = (1000..1032).map(forward).collect();
    world.queue_commands(1000, &first);
    world.queue_commands(1000, &next);
    world.queue_commands(1000, &first);
    let frames: Vec<_> = world
        .input
        .buffer
        .iter()
        .map(|work| {
            let InputWork::Command {
                command,
                pending: None,
            } = work
            else {
                panic!("fresh command");
            };
            command.frame
        })
        .collect();
    assert_eq!(frames, (980..1032).collect::<Vec<_>>());
    assert_eq!(world.command_ack(1000), 979);
    world.clear();
    assert!(!world.has_input_work());
    assert_eq!(world.command_ack(1001), 979);
}

#[test]
fn pending_queue_closes_after_the_prefix_at_a_boundary_or_capacity() {
    let mut input = InputQueue::default();
    input.commands(
        MAX_LEASE_FRAME - 2,
        &[forward(MAX_LEASE_FRAME - 1), forward(MAX_LEASE_FRAME + 1)],
    );
    assert!(input.closing);
    assert_eq!(input.admitted, Some(MAX_LEASE_FRAME - 1));
    assert!(matches!(
        input.work.pop_front(),
        Some(InputWork::Command { .. })
    ));
    assert!(matches!(input.work.pop_front(), Some(InputWork::Leave)));

    let mut input = InputQueue::default();
    for frame in 1..=MAX_PENDING_INPUT as u32 + 2 {
        input.commands(frame, &[forward(frame)]);
    }
    assert_eq!(input.work.len(), MAX_PENDING_INPUT + 1);
    assert_eq!(input.admitted, Some(MAX_PENDING_INPUT as u32));
    assert!(matches!(input.work.back(), Some(InputWork::Leave)));
    assert_eq!(input.processed, None);
}

/// The frames and buttons the last ticks released (`InputQueue::release`).
fn released(input: &mut InputQueue) -> Vec<(u32, u32)> {
    input
        .work
        .drain(..)
        .filter_map(|work| match work {
            InputWork::Command { command, .. } => Some((command.frame, command.buttons)),
            _ => None,
        })
        .collect()
}

fn released_frames(input: &mut InputQueue) -> Vec<u32> {
    released(input)
        .into_iter()
        .map(|(frame, _)| frame)
        .collect()
}

/// One command a tick, in frame order, with one more in hand
/// (`BUFFER_TARGET`): a datagram a tick late still runs on time. A burst
/// runs all but `MAX_BUFFERED` at once; more than the target waiting for
/// `DRAIN_TICKS` ticks runs one more. The frames after each tick ask the
/// client's clock to go faster with fewer than two in hand, slower with
/// more, four times as much beyond `MAX_BUFFERED`.
#[test]
fn the_input_buffer_runs_a_command_a_tick_with_the_next_in_hand() {
    let mut input = InputQueue::default();
    // The first command waits a tick, so the next is in hand when it runs.
    input.commands(100, &[forward(100)]);
    input.release(1, true);
    assert!(released_frames(&mut input).is_empty());
    assert_eq!(input.clock, CLOCK_FASTER);
    // Each datagram resends the window: the oldest runs, one waits.
    for frame in 101..110 {
        input.commands(frame, &[forward(frame - 1), forward(frame)]);
        input.release(1, true);
        assert_eq!(released_frames(&mut input), [frame - 1]);
        assert_eq!(input.clock, 0, "frame {frame}");
    }
    // 110 comes a tick late: 109, in hand, runs on time; then 110 runs
    // and 111 waits.
    input.release(1, true);
    assert_eq!(released_frames(&mut input), [109]);
    assert_eq!(input.clock, CLOCK_FASTER);
    input.commands(111, &[forward(109), forward(110), forward(111)]);
    input.release(1, true);
    assert_eq!(released_frames(&mut input), [110]);
    // A burst to 121: 11 in hand, all but 4 run.
    input.commands(112, &(111..=121).map(forward).collect::<Vec<_>>());
    input.release(1, true);
    assert_eq!(released_frames(&mut input), (111..=117).collect::<Vec<_>>());
    assert_eq!(input.clock, CLOCK_SLOWER | CLOCK_STRONGER);
    // Four wait; the burst's tick was the first of 20 with more than one
    // waiting, the 20th (frame 140 comes) runs two.
    for frame in 122..=140 {
        input.commands(frame, &[forward(frame)]);
        input.release(1, true);
        let expected: Vec<_> = if frame == 140 {
            vec![frame - 4, frame - 3]
        } else {
            vec![frame - 4]
        };
        assert_eq!(released_frames(&mut input), expected, "frame {frame}");
    }
    // Ticks the clock skipped run a command each.
    input.commands(141, &[forward(141)]);
    input.release(2, true);
    assert_eq!(released_frames(&mut input), [138, 139]);
    assert_eq!(input.clock, CLOCK_SLOWER);
}

/// A dry tick runs nothing: the body waits for the client's next frame,
/// the ack stays at the last frame run, and the client's clock goes four
/// times as fast. The frames that come late run in order, each with its
/// own buttons and click, as the client predicted them.
#[test]
fn a_dry_tick_waits_and_the_late_commands_run_as_the_client_made_them() {
    let fire = |frame| Command {
        buttons: FIRE,
        ..forward(frame)
    };
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(100);
    world.queue_commands(100, &[forward(100)]);
    world.release_input(1);
    world.queue_commands(101, &[forward(100), forward(101)]);
    world.release_input(1);
    assert_eq!(released(&mut world.input), [(100, 0)]);
    world.input.processed = Some(100);
    // Nothing comes for three ticks: 101, in hand, runs; then none. No
    // body: the clock is left alone; with one, it goes four times as fast.
    world.release_input(1);
    assert_eq!(released(&mut world.input), [(101, 0)]);
    world.input.processed = Some(101);
    world.release_input(1);
    assert!(released(&mut world.input).is_empty());
    assert_eq!((world.input.clock, world.input.dry), (0, 0));
    world.input.release(1, true);
    assert!(released(&mut world.input).is_empty());
    assert_eq!(
        (world.input.clock, world.input.dry),
        (CLOCK_FASTER | CLOCK_STRONGER, 1)
    );
    assert_eq!(world.command_ack(103), 101);
    // 102 was a one-frame tap with its click: it comes with 103 and 104,
    // and runs as it was made, at its own frame.
    let tap = Command {
        buttons: FIRE | SUBFRAME,
        click: Some(orpheus_wire::commands::Click {
            subframe: 7,
            yaw: 1,
            pitch: 2,
        }),
        ..forward(102)
    };
    world.queue_commands(104, &[forward(101), tap, forward(103), forward(104)]);
    assert_eq!(world.command_ack(104), 101);
    world.input.release(1, true);
    let [InputWork::Command { command, .. }] = world.input.work.make_contiguous() else {
        panic!("one command");
    };
    assert_eq!(*command, tap);
    assert_eq!(world.input.dry, 0);
    world.input.work.clear();
    world.input.release(1, true);
    assert_eq!(released(&mut world.input), [(103, 0)]);

    // Fire held, then nothing for two ticks: the body waits with it held.
    // The late 107 released it and 108 presses it again: each runs so.
    world.queue_commands(105, &[fire(105)]);
    world.input.release(1, true);
    world.queue_commands(106, &[fire(105), fire(106)]);
    world.input.release(1, true);
    world.input.release(1, true);
    world.input.release(1, true);
    world.input.release(1, true);
    assert_eq!(
        released(&mut world.input),
        [(104, 0), (105, FIRE), (106, FIRE)]
    );
    assert_eq!(world.input.dry, 2);
    world.queue_commands(109, &[fire(106), forward(107), fire(108), fire(109)]);
    world.input.release(1, true);
    world.input.release(1, true);
    assert_eq!(released(&mut world.input), [(107, 0), (108, FIRE)]);
}

/// Past `MAX_WAIT` dry ticks the client's last input runs again for the
/// next frame each tick, `MAX_REPEATS` frames since his last datagram, so
/// his body goes on through a short gap (walking with fire held here);
/// later dry ticks run neutral frames, his aim with nothing held, so a
/// silent body neither walks nor fires. The clock asks for four times as
/// fast all along. A client that sends again, though only frames already
/// run, is not silent: the next dry tick repeats his newest input. When
/// his commands catch up, those of frames run without them are late: a
/// release and a press they had go with the next command run.
#[test]
fn past_the_wait_a_dry_tick_repeats_the_last_input_then_runs_neutral_frames() {
    let fire = |frame| Command {
        buttons: FIRE,
        yaw: 1234,
        pitch: -56,
        ..forward(frame)
    };
    let drain = |input: &mut InputQueue| -> Vec<Command> {
        input
            .work
            .drain(..)
            .filter_map(|work| match work {
                InputWork::Command { command, .. } => Some(command),
                _ => None,
            })
            .collect()
    };
    let mut input = InputQueue::default();
    input.commands(100, &[fire(100)]);
    input.release(1, true);
    input.commands(101, &[fire(100), fire(101)]);
    input.release(1, true);
    input.release(1, true);
    assert_eq!(released(&mut input), [(100, FIRE), (101, FIRE)]);
    // The client goes silent: 4 ticks wait, 4 repeat 101's input for 102
    // to 105, the next run neutral frames.
    for tick in 1..=10_u32 {
        input.release(1, true);
        assert_eq!(
            input.clock,
            CLOCK_FASTER | CLOCK_STRONGER,
            "dry tick {tick}"
        );
        let frame = 101 + tick.saturating_sub(MAX_WAIT);
        let expected: Vec<_> = if tick <= MAX_WAIT {
            Vec::new()
        } else if tick <= MAX_WAIT + MAX_REPEATS {
            vec![fire(frame)]
        } else {
            vec![Command {
                frame,
                yaw: 1234,
                pitch: -56,
                ..Command::default()
            }]
        };
        assert_eq!(drain(&mut input), expected, "dry tick {tick}");
    }
    // Ticks the clock skipped run a neutral frame each.
    input.release(2, true);
    assert_eq!(released(&mut input), [(108, 0), (109, 0)]);
    // The client is back, behind: his datagram has 102 to 109, all run
    // (104 released fire, 105 pressed it again). The next dry tick repeats
    // his newest, 109's, for 110.
    let mut commands: Vec<_> = (102..=109).map(fire).collect();
    commands[2] = forward(104);
    input.commands(110, &commands);
    input.release(1, true);
    assert_eq!(drain(&mut input), [fire(110)]);
    // He catches up: 111 and 112 run as he made them (fire held).
    input.commands(111, &(110..=112).map(fire).collect::<Vec<_>>());
    input.release(1, true);
    input.release(1, true);
    assert_eq!(released(&mut input), [(111, FIRE), (112, FIRE)]);
    assert_eq!(input.dry, 0);
    // Silent again for 6 ticks: 4 wait, 2 repeat 112's fire for 113 and
    // 114. His commands then come: 113 released fire and 114 pressed it
    // again; 115 runs released (the repeats held fire), 116 presses it.
    for _ in 0..6 {
        input.release(1, true);
    }
    assert_eq!(released(&mut input), [(113, FIRE), (114, FIRE)]);
    let mut commands: Vec<_> = (113..=116).map(fire).collect();
    commands[0] = forward(113);
    input.commands(117, &commands);
    input.release(1, true);
    input.release(1, true);
    assert_eq!(released(&mut input), [(115, 0), (116, FIRE)]);
    assert_eq!(input.dry, 0);
}

#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn native_world_resumes_a_gap_before_switching_the_body_and_acknowledging_input() {
    let runtime = orpheus_testkit::runtime_dir();
    let assets = SceneAssets::load(runtime).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let owner = 0xa000_0100;
    let viewer = PlayerAppearance {
        entity: owner,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card: &PlayerCard::default(),
    };
    let mut world = WorldDelivery::new(owner).unbuffered();
    let first = FIRST_TICK + 1;
    world.queue_commands(FIRST_TICK, &[forward(first)]);
    assert!(!world.has_input_work());
    assert_eq!(world.command_ack(FIRST_TICK), FIRST_TICK);
    world.start(&scene, &viewer, FIRST_TICK).unwrap();
    world.queue_commands(FIRST_TICK, &[forward(first)]);
    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
            .unwrap(),
        (1, false)
    );
    assert_eq!(world.command_ack(FIRST_TICK + 2), first);
    world.created.insert(owner);
    world
        .present(
            &scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
    let hero = scene.heroes().get(SOLDIER).unwrap();
    world
        .switch(
            &scene,
            &viewer,
            FIRST_TICK,
            RETAIL_QUANTA,
            owner | 1,
            (hero, SkinTheme::default()),
            true,
        )
        .unwrap();
    // The stock Soldier: 76 starts his authored owner script, so the gap
    // below is resumed through his VM before the input is acknowledged.
    assert!(world.body_script.is_some());
    let initial = world.mover.as_ref().unwrap().pose();
    let actual = Command {
        yaw: initial.yaw,
        ..forward(first + 1)
    };
    let gap = Command {
        frame: actual.frame + 64,
        ..actual
    };
    let later = Command {
        frame: gap.frame + 1,
        ..actual
    };
    world.queue_commands(FIRST_TICK + 2, &[actual]);
    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
            .unwrap(),
        (1, false)
    );
    world.queue_commands(FIRST_TICK + 2, &[actual, gap]);

    let tracer = scene.heroes().find_hero("Tracer").unwrap();
    let switch = Message {
        id: CLIENT_GAME_MESSAGE,
        fields: Fields::from([
            (
                0x78,
                Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(owner)))])),
            ),
            (0x80, Value::Blob(tracer.guid.to_le_bytes().to_vec())),
            (0xa8, Value::Unsigned(PICK_HERO)),
            (0xb0, Value::Signed(3)),
        ]),
    };
    world.queue_game_message(FIRST_TICK + 3, Duration::ZERO, switch);
    world.queue_commands(FIRST_TICK + 3, &[gap, later]);

    for turn in 1..=4 {
        assert_eq!(
            world
                .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
                .unwrap(),
            (16, false)
        );
        assert_eq!(world.body.unwrap().id.entity, owner | 1);
        assert_eq!(
            world.mover.as_ref().unwrap().pose().frame,
            Some(actual.frame + turn * 16)
        );
        assert_eq!(
            world.command_ack(FIRST_TICK + 90),
            if turn == 4 { gap.frame } else { actual.frame }
        );
    }
    let mut synchronous = scene
        .player_spawn_mover(hero.body, viewer.team, viewer.spawn, viewer.room)
        .unwrap();
    for command in [actual, gap] {
        synchronous
            .step(scene.movement_index(), command, 0.016, |_, _| {})
            .unwrap();
    }
    let switched_from = world.mover.as_ref().unwrap().pose();
    assert_eq!(switched_from, synchronous.pose());
    assert_ne!(synchronous.pose().position, initial.position);

    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
            .unwrap(),
        (2, false)
    );
    assert_eq!(world.body.unwrap().id.entity, owner | 2);
    assert_eq!(world.command_ack(FIRST_TICK + 90), later.frame);
    // The living body's pick changes the hero where he stands, facing the
    // same way (`switch`): Tracer's mover starts at Soldier's last pose.
    let mut synchronous = scene
        .player_mover_at(
            tracer.body,
            viewer.team,
            switched_from.position,
            switched_from.yaw,
        )
        .unwrap();
    synchronous
        .step(scene.movement_index(), later, 0.016, |_, _| {})
        .unwrap();
    assert_eq!(world.mover.as_ref().unwrap().pose(), synchronous.pose());

    world.queue_commands(FIRST_TICK + 3, &[forward(later.frame + 1)]);
    world.clear();
    assert!(!world.has_input_work());
    assert_eq!(world.command_ack(FIRST_TICK + 90), later.frame);
}
use crate::egress::PacketDelivery;

const OWNER: u32 = 0xa000_0100;

fn avatar(suffix: u32, generation: u64) -> BodyId {
    BodyId {
        entity: OWNER | suffix,
        generation,
    }
}

fn body(id: BodyId) -> Body {
    Body {
        id,
        hero: 7,
        skin: SkinTheme::default(),
        team_bit: 0x80_0000,
    }
}

fn record(entity: u32, delivery: Delivery) -> Record {
    let operation = match delivery {
        Delivery::CreateBody(_) => {
            Operation::Create(Box::new(orpheus_wire::entity::Create::new(1, Vec::new())))
        }
        Delivery::DestroyBody(_) => Operation::Destroy,
        _ => Operation::Update(Vec::new()),
    };
    Record {
        update: EntityUpdate {
            entity,
            operation: Some(operation),
            ..EntityUpdate::default()
        },
        delivery,
    }
}

fn outcome(record: Arc<Record>, arrived: bool) -> DeliveryReport<Arc<Record>> {
    DeliveryReport {
        requeued_reliable: 0,
        packets: vec![PacketDelivery {
            sequence: 0,
            arrived,
            records: vec![record],
        }],
    }
}

fn eligible(world: &WorldDelivery) -> Vec<usize> {
    world.ready().iter().map(|ready| ready.index).collect()
}

struct SyntheticOwner {
    component: Component,
    graphs: Arc<GraphStore>,
    codecs: CodecTable,
    modifiers: BTreeMap<ModifierKey, Modifier>,
    failed: Arc<AtomicBool>,
    health_update: Arc<Mutex<Option<WorldComponent>>>,
}

impl OwnerBodyRuntime for SyntheticOwner {
    fn snapshot(&self) -> Option<&Snapshot> {
        (!self.failed.load(Ordering::Relaxed))
            .then(|| self.component.snapshot())
            .flatten()
    }

    fn encoder(&self) -> SnapshotEncoder<'_> {
        SnapshotEncoder::new(&self.graphs, &self.codecs)
    }

    fn advance(&mut self, command: Command, _mover: &Mover, _step: BodyStep) -> io::Result<()> {
        self.component
            .advance_command(command, &mut EmptyWorld)
            .map_err(invalid)
    }

    fn frame_ms(&self) -> u32 {
        FRAME_MS as u32
    }

    fn take_health_update(&mut self) -> Option<WorldComponent> {
        self.health_update.lock().unwrap().take()
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        Some(&self.modifiers)
    }
}

fn synthetic_owner(entity: u32, frame: u32, failed: Arc<AtomicBool>) -> SyntheticOwner {
    let graphs = Arc::new(
        GraphStore::from_json(
            br#"{"version":1,"bodies":{},"missing":[],"graphs":[{
            "index":42,"items":0,"nodes":[],"states":[],"entries":[],"remote":[],
            "sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{},
            "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
                "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4}
        }]}"#,
        )
        .unwrap(),
    );
    let component = Component::start(
        Arc::clone(&graphs),
        entity,
        Default::default(),
        vec![Root::new(1, 42)],
        frame,
        &mut EmptyWorld,
        MAX_INIT_WORK,
    )
    .unwrap();
    SyntheticOwner {
        component,
        graphs,
        codecs: CodecTable::from_json(r#"{"classes":{}}"#).unwrap(),
        modifiers: BTreeMap::new(),
        failed,
        health_update: Arc::default(),
    }
}

fn activate_script(world: &mut WorldDelivery, id: BodyId, hero: u64, failed: Arc<AtomicBool>) {
    let body = Body { hero, ..body(id) };
    world.body = Some(body);
    world.bound = None;
    world
        .bodies
        .insert(id.entity, (id.generation, Lifetime::Live));
    world.created.remove(&id.entity);
    let runtime = Box::new(synthetic_owner(id.entity, 100, failed));
    world.body_script = Some(BodyScript::new(body, 100, runtime).unwrap());
}

fn transmit(
    world: &mut WorldDelivery,
    egress: &mut GameEgress<Arc<Record>>,
    tick: u32,
    sequence: u32,
) -> (Vec<u8>, SentFrame) {
    let mut bytes = Vec::new();
    let sent = world
        .send_frame(tick, Vec::new(), false, egress, |payload| {
            bytes = payload;
            Ok::<_, ()>(sequence)
        })
        .unwrap();
    (bytes, sent)
}

fn transport_outcome(
    world: &mut WorldDelivery,
    egress: &mut GameEgress<Arc<Record>>,
    sequence: u32,
    arrived: bool,
) {
    world.acknowledge(
        egress.acknowledge(sequence + 1, u64::from(arrived)),
        Duration::ZERO,
    );
}

#[test]
fn projectile_retirement_retries_destroys_and_ignores_stale_confirmations() {
    use orpheus_script::vm::ShotEvent;

    const DEFINITION: u64 = 0x0400_0000_0000_072c;
    let id = avatar(1, 1);
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(100);
    activate_script(&mut world, id, SOLDIER, Arc::default());
    let leases = ProjectileLeases::new(0, 100, [(DEFINITION, 1)]).unwrap();
    let alias = leases.entity(DEFINITION, 101, 0).unwrap();
    let mut projectiles = OwnerProjectiles::new(id.entity, leases);
    let token = projectiles
        .admit(
            ShotEvent {
                entity: id.entity,
                instance: 1,
                graph: 42,
                raw_state: 0,
                frame: 101,
                time_ms: 101 * FRAME_MS,
                activation: 1,
                activation_ms: 101 * FRAME_MS,
                ordinal: 0,
                spread: None,
            },
            DEFINITION,
            125,
        )
        .unwrap();
    let confirmation = projectiles.advance(133);
    world.body_script.as_mut().unwrap().projectiles = Some(projectiles);
    world.queue_projectiles(id, confirmation).unwrap();
    let old = Arc::clone(&world.queued[0]);
    assert!(
        matches!(old.delivery, Delivery::ProjectileConfirm { id: source, token: key }
            if source == id && key == token)
    );
    assert!(eligible(&world).is_empty());
    world.created.insert(id.entity);
    world.bound = Some(id);
    assert_eq!(eligible(&world), [0]);
    world.sent(&[0]);

    world.cancel_body_script();
    activate_script(&mut world, avatar(2, 2), SOLDIER, Arc::default());
    world.acknowledge(outcome(old, false), Duration::ZERO);
    let destroy = Arc::clone(&world.queued[0]);
    assert_eq!(world.queued.len(), 1);
    assert_eq!(destroy.update.entity, alias);
    assert!(destroy.update.ch2.is_none());
    assert!(matches!(destroy.update.operation, Some(Operation::Destroy)));
    world.sent(&[0]);
    world.acknowledge(outcome(Arc::clone(&destroy), false), Duration::ZERO);
    assert_eq!(world.queued.len(), 1);
    world.sent(&[0]);
    world.acknowledge(outcome(destroy, true), Duration::ZERO);
    assert!(world.queued.is_empty());
    assert!(world.projectile_destroys.is_empty());
}

#[test]
fn owner_health_waits_for_creation_and_discards_superseded_lifetimes() {
    use orpheus_script::vitals::health;
    use orpheus_wire::entity::{ARMOUR_POOL, POOLS, pool, pool_component};

    let id = avatar(1, 1);
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(100);
    world.body = Some(body(id));
    world
        .bodies
        .insert(id.entity, (id.generation, Lifetime::Live));
    let runtime = synthetic_owner(id.entity, 100, Arc::default());
    let pending = Arc::clone(&runtime.health_update);
    world.body_script = Some(BodyScript::new(body(id), 100, Box::new(runtime)).unwrap());
    world.push(record(id.entity, Delivery::CreateBody(id)));

    let mut egress = GameEgress::default();
    let first = health(160.0, 200.0, 50.0, 0.0);
    *pending.lock().unwrap() = Some(first.clone());
    let sent = world
        .send_frame(100, Vec::new(), false, &mut egress, |_| Ok::<_, ()>(0))
        .unwrap();
    assert_eq!(sent.records, [RecordSource::Queued(0)]);
    assert!(world.health_pending(id.entity));
    assert!(world.ready().is_empty());
    transport_outcome(&mut world, &mut egress, 0, true);

    let old = world.ready()[0].tracked();
    assert_eq!(old.update.operation, Some(Operation::Update(vec![first])));
    world.sent(&[0]);
    let latest = pool_component([
        (0, pool(1, 0, 200.0, 120.0)),
        (ARMOUR_POOL * POOLS, [0; 20]),
    ]);
    *pending.lock().unwrap() = Some(latest.clone());
    let rejected = world.send_frame(101, Vec::new(), false, &mut egress, |_| Err::<u32, _>(()));
    assert!(matches!(rejected, Err(SendError::Transport(()))));
    world.acknowledge(outcome(Arc::clone(&old), false), Duration::ZERO);
    world.acknowledge(outcome(old, true), Duration::ZERO);
    assert!(world.health_pending(id.entity));
    let mut updates = world
        .ready()
        .into_iter()
        .filter(|item| matches!(item.record.delivery, Delivery::Health { .. }));
    let ready = updates.next().expect("latest health revision");
    assert!(updates.next().is_none());
    let latest_record = ready.tracked();
    assert_eq!(
        latest_record.update.operation,
        Some(Operation::Update(vec![latest]))
    );

    world.sent(&[ready.index]);
    world.body = Some(body(avatar(2, 2)));
    world.cancel_body_script();
    world.retire_body(id);
    world.remove_stale();
    world.queue_health(latest_record.update.clone());
    world.acknowledge(outcome(latest_record, false), Duration::ZERO);
    assert!(!world.health_pending(id.entity));
    assert!(!world.identity_ready(id.entity));
    let [destroy] = world.queued.as_slice() else {
        panic!("expected only the retired body's destroy record");
    };
    assert!(matches!(destroy.delivery, Delivery::DestroyBody(retired) if retired == id));
}

#[test]
fn body_script_retries_frozen_initial_frames_and_ignores_old_generation_reports() {
    let mut world = WorldDelivery::new(OWNER);
    let id = avatar(1, 1);
    world.started = Some(100);
    activate_script(&mut world, id, 7, Arc::default());
    world.push(record(id.entity, Delivery::CreateBody(id)));
    let mut egress = GameEgress::default();
    // The script record a datagram carried: its chunk (first, span), the
    // owner frame's `lead | H | C` bits (2: the initial owner delta) and its
    // CmFD.
    let owner_frame = |report: &DeliveryReport<Arc<Record>>| {
        let records: Vec<_> = report
            .packets
            .iter()
            .flat_map(|packet| &packet.records)
            .collect();
        let [record] = records.as_slice() else {
            panic!("expected only the script record");
        };
        let chunk = record.update.chunk.as_ref().unwrap();
        let mut bits = BitReader::new(chunk.as_bytes());
        let first = bits.w_u32().unwrap();
        let span = bits.w_var().unwrap();
        assert!(!bits.bit().unwrap());
        bits.w_var().unwrap();
        (first, span, bits.bits(3).unwrap(), bits.w_var().unwrap())
    };
    assert_eq!(
        transmit(&mut world, &mut egress, 100, 0).1.records,
        [RecordSource::Queued(0)]
    );
    assert!(
        transmit(&mut world, &mut egress, 100, 1)
            .1
            .records
            .is_empty()
    );
    transport_outcome(&mut world, &mut egress, 0, true);
    let (h0, sent) = transmit(&mut world, &mut egress, 101, 2);
    assert_eq!(sent.records, [RecordSource::Queued(0)]);
    transport_outcome(&mut world, &mut egress, 2, false);
    assert_eq!(transmit(&mut world, &mut egress, 101, 3).0, h0);
    transport_outcome(&mut world, &mut egress, 3, true);
    assert_eq!(
        world.body_script.as_ref().unwrap().stream.base().unwrap().0,
        1
    );

    let blocked = transmit(&mut world, &mut egress, 101, 4).1;
    assert!(blocked.records.is_empty());
    let mut mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        [0.0; 3],
        0,
        0x18,
    )
    .unwrap();
    let rays = TriangleIndex::new([]).unwrap();
    let advance = |world: &mut WorldDelivery, mover: &mut Mover, frame| {
        mover
            .step(&rays, forward(frame), 0.016, |mover, command| {
                world
                    .body_script
                    .as_mut()
                    .unwrap()
                    .advance(mover, command, BodyStep::Posed, None, false)
                    .unwrap();
            })
            .unwrap();
    };
    // The full frame came in datagrams of frame 101, which the client takes
    // as the body's command frame: the initial owner delta waits for a
    // command past it (`BodyStream::full_sent`).
    advance(&mut world, &mut mover, 101);
    assert!(
        transmit(&mut world, &mut egress, 101, 5)
            .1
            .records
            .is_empty()
    );
    let stream = &world.body_script.as_ref().unwrap().stream;
    assert_eq!(stream.pending().count(), 0);
    advance(&mut world, &mut mover, 102);
    assert!(
        transmit(&mut world, &mut egress, 102, 6)
            .1
            .records
            .is_empty()
    );
    advance(&mut world, &mut mover, 103);
    assert!(
        transmit(&mut world, &mut egress, 103, 7)
            .1
            .records
            .is_empty()
    );
    let stream = &world.body_script.as_ref().unwrap().stream;
    assert_eq!(
        stream.pending().map(|(last, ..)| last).collect::<Vec<_>>(),
        [3]
    );
    assert_eq!(stream.command_frame(), 103);

    world.push(record(OWNER, Delivery::Possession(id)));
    assert_eq!(
        transmit(&mut world, &mut egress, 103, 8).1.records,
        [RecordSource::Queued(0)]
    );
    transport_outcome(&mut world, &mut egress, 8, true);
    let (c0, sent) = transmit(&mut world, &mut egress, 104, 9);
    assert_eq!(sent.records, [RecordSource::Queued(0)]);
    advance(&mut world, &mut mover, 119);
    assert!(
        transmit(&mut world, &mut egress, 104, 10)
            .1
            .records
            .is_empty()
    );
    transport_outcome(&mut world, &mut egress, 9, false);
    assert_eq!(transmit(&mut world, &mut egress, 104, 11).0, c0);
    // Nothing more goes while the initial owner delta is retried; this
    // synthetic body never changes, so no correction would follow it either
    // (`SnapshotEncoder::queue_delta`).
    advance(&mut world, &mut mover, 120);
    assert!(
        transmit(&mut world, &mut egress, 120, 12)
            .1
            .records
            .is_empty()
    );
    world.cancel_body_script();
    let reused = avatar(1, 257);
    activate_script(&mut world, reused, 7, Arc::default());
    // c0, the initial owner delta at command 103: chunks 2 to 3, the
    // pending frame above (it replaced the unsent one at 102). It arrives
    // after the body's script was replaced: the new stream ignores it.
    let arrived = egress.acknowledge(12, 1);
    assert_eq!(owner_frame(&arrived), (2, 1, 2, 103));
    world.acknowledge(arrived, Duration::ZERO);
    assert_eq!(world.body_script.as_ref().unwrap().stream.last(), 0);
    assert!(world.queued.is_empty());
}

/// The owner delta of the client's own body goes first in its datagram: a
/// queue the datagram can't take whole (here more records than it holds)
/// leaves other records for the next tick, not the command frame his
/// prediction is checked at (`WorldDelivery::ready`).
#[test]
fn the_owner_delta_goes_ahead_of_a_crowded_queue() {
    let mut world = WorldDelivery::new(OWNER);
    let id = avatar(1, 1);
    world.started = Some(100);
    activate_script(&mut world, id, 7, Arc::default());
    world.push(record(id.entity, Delivery::CreateBody(id)));
    world.push(record(OWNER, Delivery::Possession(id)));
    let mut egress = GameEgress::default();
    assert_eq!(transmit(&mut world, &mut egress, 100, 0).1.records.len(), 2);
    transport_outcome(&mut world, &mut egress, 0, true);
    // The full frame, in a datagram of frame 101.
    assert_eq!(
        transmit(&mut world, &mut egress, 101, 1).1.records,
        [RecordSource::Queued(0)]
    );
    transport_outcome(&mut world, &mut egress, 1, true);
    let mut mover = Mover::new(
        MoverSettings::default(),
        Arc::new(ContactWorld::new(Vec::new()).unwrap()),
        [0.0; 3],
        0,
        0x18,
    )
    .unwrap();
    let rays = TriangleIndex::new([]).unwrap();
    mover
        .step(&rays, forward(102), 0.016, |mover, command| {
            world
                .body_script
                .as_mut()
                .unwrap()
                .advance(mover, command, BodyStep::Posed, None, false)
                .unwrap();
        })
        .unwrap();
    for i in 0..MAX_ENTITY_RECORDS {
        world.push(Record::baseline(EntityUpdate {
            entity: 0x8000_0100 + i as u32,
            operation: Some(Operation::Destroy),
            ..EntityUpdate::default()
        }));
    }
    // The delta after command 102, queued behind them, goes first.
    let sent = transmit(&mut world, &mut egress, 102, 2).1;
    assert_eq!(sent.records.len(), MAX_ENTITY_RECORDS);
    let carried = egress.acknowledge(3, 1);
    assert!(matches!(
        carried.packets[0].records[0].delivery,
        Delivery::BodyScript { delta: true, .. }
    ));
    let [left] = world.queued.as_slice() else {
        panic!("one record left");
    };
    assert_eq!(
        left.update.entity,
        0x8000_0100 + MAX_ENTITY_RECORDS as u32 - 1
    );
}

fn read_ack(payload: &[u8]) -> u32 {
    let mut bits = BitReader::new(payload);
    bits.var_a().unwrap();
    assert!(!bits.bit().unwrap());
    bits.var_a().unwrap()
}

#[test]
fn owner_correction_respects_create_budget_and_failed_transport() {
    let mut world = WorldDelivery::new(OWNER);
    let id = avatar(1, 1);
    world.started = Some(100);
    let failed = Arc::new(AtomicBool::new(false));
    activate_script(&mut world, id, SOLDIER, Arc::clone(&failed));
    world.input.processed = Some(125);
    let history = world
        .body_script
        .as_mut()
        .unwrap()
        .corrections
        .as_mut()
        .unwrap();
    history.record(Movement {
        frame: Some(100),
        ..Movement::default()
    });
    let mut egress = GameEgress::default();
    let (blocked, sent) = transmit(&mut world, &mut egress, 100, 0);
    assert_eq!(read_ack(&blocked), 102);
    assert!(sent.records.is_empty());
    world.acknowledge(
        outcome(Arc::new(record(id.entity, Delivery::CreateBody(id))), true),
        Duration::ZERO,
    );
    let (first, sent) = transmit(&mut world, &mut egress, 100, 1);
    assert_eq!(read_ack(&first), 102);
    assert!(sent.records.contains(&RecordSource::Movement(0)));
    transport_outcome(&mut world, &mut egress, 1, true);
    let history = world
        .body_script
        .as_mut()
        .unwrap()
        .corrections
        .as_mut()
        .unwrap();
    assert_eq!(history.corrected(), Some(100));
    history.record(Movement {
        frame: Some(106),
        ..Movement::default()
    });
    history.record(Movement {
        frame: Some(112),
        ..Movement::default()
    });
    for i in 0..MAX_ENTITY_RECORDS {
        world.push(Record::baseline(EntityUpdate {
            entity: 0x8000_0100 + i as u32,
            operation: Some(Operation::Destroy),
            ..EntityUpdate::default()
        }));
    }
    let (crowded, sent) = transmit(&mut world, &mut egress, 107, 2);
    assert_eq!(read_ack(&crowded), 102);
    assert_eq!(sent.records.len(), MAX_ENTITY_RECORDS);
    assert!(!sent.records.contains(&RecordSource::Movement(0)));
    transport_outcome(&mut world, &mut egress, 2, true);
    let rejected = world.send_frame(107, Vec::new(), false, &mut egress, |payload| {
        assert_eq!(read_ack(&payload), 108);
        Err(())
    });
    assert!(matches!(rejected, Err(SendError::Transport(()))));
    assert_eq!(
        world
            .body_script
            .as_ref()
            .unwrap()
            .corrections
            .as_ref()
            .unwrap()
            .corrected(),
        Some(100)
    );
    let (selected, sent) = transmit(&mut world, &mut egress, 107, 3);
    assert_eq!(read_ack(&selected), 108);
    assert_eq!(sent.records, [RecordSource::Movement(0)]);
    assert!(egress.acknowledge(4, 0).packets.is_empty());
    assert!(
        transmit(&mut world, &mut egress, 107, 4)
            .1
            .records
            .is_empty()
    );

    failed.store(true, Ordering::Relaxed);
    let (payload, sent) = transmit(&mut world, &mut egress, 112, 5);
    assert_eq!(read_ack(&payload), 125);
    assert!(world.body_script.is_none());
    assert!(sent.records.is_empty());
    assert!(world.queued.is_empty());
}

#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn prepared_worker_resumes_unposed_vm_frames_before_advancing_physics() {
    let runtime = orpheus_testkit::runtime_dir();
    let assets = SceneAssets::load(runtime).unwrap();
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    let (factory, _) = crate::body_script::fixture_body_factory();
    scene.owner_body_factory = Some(Arc::new(move |_, spawn, _| {
        Ok(Some(Box::new(factory.start(spawn)?)))
    }));
    let viewer = PlayerAppearance {
        entity: OWNER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card: &PlayerCard::default(),
    };
    let mut world = WorldDelivery::new(OWNER).unbuffered();
    world.started = Some(FIRST_TICK);
    let hero = scene.heroes().get(SOLDIER).unwrap();
    world
        .switch(
            &scene,
            &viewer,
            FIRST_TICK,
            RETAIL_QUANTA,
            OWNER | 1,
            (hero, SkinTheme::default()),
            true,
        )
        .unwrap();
    let tick = FIRST_TICK + 1;
    world.queue_commands(tick, &[forward(tick)]);
    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
            .unwrap(),
        (1, false)
    );
    assert_eq!(world.body_script.as_ref().unwrap().frame, tick);
    let mut egress = GameEgress::default();
    assert!(
        !transmit(&mut world, &mut egress, tick, 0)
            .1
            .records
            .contains(&RecordSource::Movement(0))
    );
    transport_outcome(&mut world, &mut egress, 0, true);
    assert_eq!(
        transmit(&mut world, &mut egress, tick, 1).1.records,
        [RecordSource::Queued(0), RecordSource::Movement(0)]
    );
    transport_outcome(&mut world, &mut egress, 1, true);

    let after_unposed_gap = tick + 400;
    world.queue_commands(after_unposed_gap, &[forward(after_unposed_gap)]);
    for turn in 1..=16 {
        assert_eq!(
            world
                .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
                .unwrap(),
            (16, false)
        );
        assert_eq!(
            world.body_script.as_ref().unwrap().frame,
            tick + 141 + turn * 16
        );
        assert_eq!(world.mover.as_ref().unwrap().pose().frame, Some(tick));
        assert_eq!(world.command_ack(after_unposed_gap), tick);
        assert!(
            transmit(&mut world, &mut egress, after_unposed_gap, turn + 1)
                .1
                .records
                .is_empty()
        );
        world.queue_commands(after_unposed_gap, &[forward(after_unposed_gap)]);
    }
    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16, None)
            .unwrap(),
        (3, false)
    );
    assert!(!world.has_input_work());
    assert_eq!(world.command_ack(after_unposed_gap), after_unposed_gap);
    assert_eq!(world.body_script.as_ref().unwrap().frame, after_unposed_gap);
    assert_eq!(
        world.mover.as_ref().unwrap().pose().frame,
        Some(after_unposed_gap)
    );
    assert_eq!(
        transmit(&mut world, &mut egress, after_unposed_gap, 18)
            .1
            .records,
        [RecordSource::Queued(0), RecordSource::Movement(0)]
    );
}

#[test]
fn possession_and_retirement_wait_when_budget_splits_the_prerequisites() {
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(1001);
    let previous = avatar(1, 1);
    let current = avatar(2, 2);
    world.body = Some(body(current));
    world
        .bodies
        .insert(previous.entity, (1, Lifetime::Retiring));
    world.bodies.insert(current.entity, (2, Lifetime::Live));
    world.push(record(current.entity, Delivery::CreateBody(current)));
    world.push(record(OWNER, Delivery::Possession(current)));
    world.push(record(previous.entity, Delivery::DestroyBody(previous)));
    assert_eq!(eligible(&world), [0, 1, 2]);

    let create = world.ready()[0].tracked();
    world.sent(&[0]); // The packet budget only fit the create.
    assert!(eligible(&world).is_empty());
    world.acknowledge(outcome(Arc::clone(&create), false), Duration::ZERO);
    assert_eq!(eligible(&world), [0, 1, 2]);
    world.sent(&[0]);
    world.acknowledge(outcome(create, true), Duration::ZERO);
    assert_eq!(eligible(&world), [0, 1]);

    let possess = world.ready()[0].tracked();
    world.sent(&[0]);
    assert!(eligible(&world).is_empty());
    world.acknowledge(outcome(possess, true), Duration::ZERO);
    assert_eq!(eligible(&world), [0]);
    let destroy = world.ready()[0].tracked();
    world.sent(&[0]);
    let now = Duration::from_secs(5);
    world.acknowledge(outcome(destroy, true), now);
    assert!(matches!(
        world.bodies[&previous.entity].1,
        Lifetime::ReusableAt(at) if at == now + BODY_REUSE_DELAY
    ));
}

#[test]
fn a_late_ack_or_loss_cannot_resurrect_a_retired_or_reused_body() {
    let mut world = WorldDelivery::new(OWNER);
    world.started = Some(1001);
    let old = avatar(1, 1);
    let reused = avatar(1, 257);
    world.body = Some(body(reused));
    world
        .bodies
        .insert(reused.entity, (reused.generation, Lifetime::Live));
    let stale = Arc::new(record(old.entity, Delivery::CreateBody(old)));
    world.acknowledge(outcome(Arc::clone(&stale), true), Duration::ZERO);
    world.acknowledge(outcome(stale, false), Duration::ZERO);
    assert!(!world.created.contains(&reused.entity));
    assert!(world.queued.is_empty());
    world.acknowledge(
        outcome(Arc::new(record(OWNER, Delivery::Possession(old))), true),
        Duration::ZERO,
    );
    assert!(world.bound.is_none());
}

#[test]
fn ring_reuses_only_acknowledged_destroy_after_the_retention_window() {
    let mut world = WorldDelivery::new(OWNER);
    world.body = Some(body(avatar(255, 255)));
    for suffix in 1..=255 {
        world
            .bodies
            .insert(OWNER | suffix, (u64::from(suffix), Lifetime::Live));
    }
    let target = avatar(1, 1);
    world.bodies.insert(target.entity, (1, Lifetime::Retiring));
    assert_eq!(world.next_body(Duration::from_secs(1000)), None);
    world
        .bodies
        .insert(target.entity, (1, Lifetime::ReusableAt(BODY_REUSE_DELAY)));
    assert_eq!(
        world.next_body(BODY_REUSE_DELAY - Duration::from_millis(1)),
        None
    );
    assert_eq!(world.next_body(BODY_REUSE_DELAY), Some(target.entity));
}

/// A started client whose current body runs `runtime`.
fn scripted_world(owner: u32, id: BodyId, runtime: Box<dyn OwnerBodyRuntime>) -> WorldDelivery {
    let mut world = WorldDelivery::new(owner);
    world.started = Some(100);
    world.body = Some(body(id));
    world.body_script = Some(BodyScript::new(body(id), 100, runtime).unwrap());
    world
}

/// match.py `deliver_body_messages`: a message one body's script sends
/// another (SendGameMessage) runs the other body's entries for it, with its
/// parameters; the sender takes nothing of it, and it goes once.
#[test]
fn a_body_message_runs_the_named_bodys_entries() {
    use crate::body_script::{fixture_body_factory, fixture_graph_factory};
    use orpheus_script::wire::Value;
    use serde_json::{Value as Json, json};

    const RECEIVER: u32 = 0xa000_0200;
    let message = "0x0240000000000005";
    let parameter = "0x0D80000000000007";
    let var = |id: u32, entity: bool| json!({"$":"STUConfigVarDynamic","m_identifier":id,"m_60DB8F99":entity});
    let node = |pos: u16, class: &str, fields: Json| {
        let mut fields = fields;
        fields["m_2BBEEAB8"] = json!(false);
        fields["m_ADEB6E05"] = json!(class.contains("Action"));
        json!({"pos":pos,"class":class,"uid":pos,"rid":pos,"remote":-1,"state":null,
            "client_only":false,"server_only":false,"fields":fields})
    };
    let graph = |nodes: Vec<Json>| {
        json!({
            "index":42,"items":nodes.len(),"nodes":nodes,"states":[],"entries":[0],
            "remote":[],"sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{},
            "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
                "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4}
        })
    };
    // The sender's entry sends the message to the body in its #90.
    let sender = graph(vec![
        node(
            0,
            "STUStatescriptEntry",
            json!({"m_outPlug":{"$":"Plug","links":[[1,"m_inPlug"]],"m_FF3DAF1E":[]}}),
        ),
        node(
            1,
            "STUStatescriptActionSendGameMessage",
            json!({
                "m_gameMessage":{"m_gameMessage":message},
                "m_target":var(90, true),
                "m_params":[{"m_B5051BCE":parameter,
                    "m_value":{"$":"STUConfigVarInt","m_value":9}}],
            }),
        ),
    ]);
    // The receiver's entry for it keeps the parameter in its #100.
    let receiver = graph(vec![node(
        0,
        "STUStatescriptEntryGameMessage",
        json!({
            "m_gameMessage":{"m_gameMessage":message},
            "m_params":[{"m_B5051BCE":parameter,"m_out_Var":var(100, false)}],
        }),
    )]);
    let (_, spawn) = fixture_body_factory();
    let (sending, receiving) = (
        avatar(1, 1),
        BodyId {
            entity: RECEIVER | 1,
            generation: 1,
        },
    );
    let sender = fixture_graph_factory(
        sender,
        Variables::from([(90, Value::Entity(receiving.entity))]),
    )
    .start(spawn)
    .unwrap();
    let receiver = fixture_graph_factory(receiver, Variables::new())
        .start(OwnerBodySpawn {
            controller: RECEIVER,
            entity: receiving.entity,
            ..spawn
        })
        .unwrap();
    let mut a = scripted_world(OWNER, sending, Box::new(sender));
    let mut b = scripted_world(RECEIVER, receiving, Box::new(receiver));
    let taken = |world: &WorldDelivery| {
        world
            .body_script
            .as_ref()
            .unwrap()
            .runtime
            .snapshot()
            .unwrap()
            .instances[&1]
            .variables
            .get(&100)
            .cloned()
    };
    assert_eq!(taken(&b), None);
    route(&mut [&mut a, &mut b]);
    assert_eq!(taken(&b), Some(Value::Integer(9)));
    assert_eq!(taken(&a), None);
    assert!(a.take_messages().is_empty(), "it went once");
}

/// A body whose script only sends and takes messages.
struct Mailbox {
    owner: SyntheticOwner,
    outbox: Vec<GameMessage>,
    inbox: Arc<Mutex<Vec<u64>>>,
}

impl OwnerBodyRuntime for Mailbox {
    fn snapshot(&self) -> Option<&Snapshot> {
        self.owner.snapshot()
    }

    fn encoder(&self) -> SnapshotEncoder<'_> {
        self.owner.encoder()
    }

    fn advance(&mut self, command: Command, mover: &Mover, step: BodyStep) -> io::Result<()> {
        self.owner.advance(command, mover, step)
    }

    fn frame_ms(&self) -> u32 {
        self.owner.frame_ms()
    }

    fn receive_message(
        &mut self,
        _sender: u32,
        message: u64,
        _parameters: Variables,
    ) -> io::Result<usize> {
        self.inbox.lock().unwrap().push(message);
        Ok(1)
    }

    fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }
}

/// The players' messages through the room's routing
/// ([`crate::messages::deliver`]), with the players alone.
fn route(worlds: &mut [&mut WorldDelivery]) {
    let mut players = crate::messages::Players::new(worlds, 100);
    let messages = players.take_messages();
    crate::messages::deliver(messages, Vec::new(), &mut [&mut players]);
}

/// match.py `queue_body_message`: a message to an entity no player's body
/// is goes nowhere; one to every entity reaches every body but the
/// sender's, whose own instances took it when it was sent.
#[test]
fn body_messages_reach_the_named_body_or_every_body_but_the_senders() {
    const RECEIVER: u32 = 0xa000_0200;
    const NOBODY: u32 = 0xa000_0901;
    let (sending, receiving) = (
        avatar(1, 1),
        BodyId {
            entity: RECEIVER | 1,
            generation: 1,
        },
    );
    let mail = |target, message| GameMessage {
        sender: sending.entity,
        target,
        message,
        parameters: Variables::new(),
    };
    let outbox = vec![
        mail(orpheus_script::mode::EVERY_ENTITY, 7777),
        mail(NOBODY, 1000),
        mail(receiving.entity, 1),
    ];
    let sent = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut a = scripted_world(
        OWNER,
        sending,
        Box::new(Mailbox {
            owner: synthetic_owner(sending.entity, 100, Arc::default()),
            outbox,
            inbox: Arc::clone(&sent),
        }),
    );
    let mut b = scripted_world(
        RECEIVER,
        receiving,
        Box::new(Mailbox {
            owner: synthetic_owner(receiving.entity, 100, Arc::default()),
            outbox: Vec::new(),
            inbox: Arc::clone(&received),
        }),
    );
    route(&mut [&mut a, &mut b]);
    assert_eq!(*received.lock().unwrap(), [7777, 1]);
    assert!(sent.lock().unwrap().is_empty());
}

/// The client's hero settings (21604) reach the current body's graphs as
/// entity variables at once: ALLIED HEALTH BARS is `#8336e`, SHOW FRIENDLY
/// OUTLINES `#27568e`; the hero's own entry wins over the general one, an
/// entry for another hero changes nothing, and each list replaces the last,
/// so a setting it leaves out (the client's DEFAULT) is cleared.
#[test]
fn hero_settings_reach_the_bodys_graphs_as_entity_variables() {
    use crate::body_script::{fixture_body_factory, fixture_graph_factory};
    use orpheus_content::hero_settings::{
        ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS, ALLIED_HEALTH_BARS_OFF, FRIENDLY_OUTLINES,
        FRIENDLY_OUTLINES_ALWAYS, HeroSetting,
    };
    use orpheus_script::wire::Value as ScriptValue;
    use serde_json::json;

    let graph = json!({
        "index":42,"items":1,"states":[],"entries":[0],
        "nodes":[{"pos":0,"class":"STUStatescriptEntry","uid":0,"rid":0,"remote":-1,
            "state":null,"client_only":false,"server_only":false,
            "fields":{"m_2BBEEAB8":false,"m_ADEB6E05":false}}],
        "remote":[],"sync_vars":[],"containers":{},"unknown_nodes":[],"fields":{},
        "header":{"m_nodesBitCount":4,"m_statesBitCount":4,
            "m_remoteSyncNodesBitCount":4,"m_syncVarsBitCount":4}
    });
    let (_, spawn) = fixture_body_factory();
    let id = BodyId {
        entity: spawn.entity,
        generation: 1,
    };
    let runtime = fixture_graph_factory(graph, Variables::new())
        .start(spawn)
        .unwrap();
    let mut world = scripted_world(spawn.controller, id, Box::new(runtime));
    let hero = world.body.unwrap().hero;
    let setting = |hero, setting, value| HeroSetting {
        hero,
        setting,
        value,
    };
    let variable = |world: &WorldDelivery, id: u64| {
        world
            .body_script
            .as_ref()
            .unwrap()
            .runtime
            .snapshot()
            .unwrap()
            .variables
            .get(&(id as u32))
            .cloned()
            .filter(|value| *value != ScriptValue::Null)
    };
    let allied = |world: &WorldDelivery| variable(world, ALLIED_HEALTH_BARS);
    let outlines = |world: &WorldDelivery| variable(world, FRIENDLY_OUTLINES);
    assert_eq!(allied(&world), None);
    let always = setting(0, ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS);
    world.update_hero_settings(&[always]);
    assert_eq!(
        allied(&world),
        Some(ScriptValue::Asset(ALLIED_HEALTH_BARS_ALWAYS))
    );
    world.update_hero_settings(&[
        always,
        setting(hero + 1, ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_OFF),
        setting(0, FRIENDLY_OUTLINES, FRIENDLY_OUTLINES_ALWAYS),
    ]);
    assert_eq!(
        allied(&world),
        Some(ScriptValue::Asset(ALLIED_HEALTH_BARS_ALWAYS))
    );
    assert_eq!(
        outlines(&world),
        Some(ScriptValue::Asset(FRIENDLY_OUTLINES_ALWAYS))
    );
    world.update_hero_settings(&[
        always,
        setting(hero, ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_OFF),
    ]);
    assert_eq!(
        allied(&world),
        Some(ScriptValue::Asset(ALLIED_HEALTH_BARS_OFF))
    );
    assert_eq!(outlines(&world), None, "left out: back at DEFAULT");
    world.update_hero_settings(&[]);
    assert_eq!(allied(&world), None);
    assert!(!world.script_failed);
}

/// A body's hero settings are the data's defaults until the client sends a
/// value, read as the client keeps it (`0x7FF789B7F0F0`): Ana's NANO BOOST
/// SENSITIVITY as the bits of an f64, Soldier: 76's HOLD TO SPRINT as 1. A
/// hero's body holds only his own settings and every hero's, and a sent
/// setting at DEFAULT is null.
#[test]
fn hero_settings_are_the_sent_values_else_the_defaults() {
    use orpheus_content::{
        hero_settings::{ALLIED_HEALTH_BARS, HeroSetting, HeroSettings},
        heroes::ANA,
    };
    use orpheus_script::wire::Value as ScriptValue;

    const NANO_BOOST_SENSITIVITY: u64 = 0x0d80_0000_0000_2582;
    const HOLD_TO_SPRINT: u64 = 0x0d80_0000_0000_1ddc;
    let defaults = HeroSettings::default();
    let ana = hero_setting_variables(&defaults, ANA);
    assert_eq!(ana.get(&0x2582), Some(&ScriptValue::Float(1.0)));
    assert_eq!(
        ana.get(&(ALLIED_HEALTH_BARS as u32)),
        Some(&ScriptValue::Null)
    );
    assert_eq!(ana.get(&0x1ddc), None, "Soldier: 76's own setting");
    assert_eq!(
        hero_setting_variables(&defaults, SOLDIER).get(&0x1ddc),
        Some(&ScriptValue::Bool(false))
    );

    let mut sent = HeroSettings::default();
    sent.replace(&[
        HeroSetting {
            hero: ANA,
            setting: NANO_BOOST_SENSITIVITY,
            value: 0.5f64.to_bits(),
        },
        HeroSetting {
            hero: SOLDIER,
            setting: HOLD_TO_SPRINT,
            value: 1,
        },
    ]);
    assert_eq!(
        hero_setting_variables(&sent, ANA).get(&0x2582),
        Some(&ScriptValue::Float(0.5))
    );
    assert_eq!(
        hero_setting_variables(&sent, SOLDIER).get(&0x1ddc),
        Some(&ScriptValue::Bool(true))
    );
}

/// The stock body graph `01CF` turns ALLIED HEALTH BARS into target tag
/// `0042`, which the nameplate `0D24` asks of the local view for an ally's
/// health bar (`#17553`): ALWAYS gives it to Soldier: 76 (st116 → st117), on
/// his running body and on the next one; OFF takes Ana's own tag away (st115
/// → st114); DEFAULT (an empty list) does neither. The owner frames the
/// client acknowledges carry each of these states and `#8336e`.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn allied_health_bars_give_or_take_the_support_tag_of_the_body() {
    use orpheus_content::hero_settings::{
        ALLIED_HEALTH_BARS, ALLIED_HEALTH_BARS_ALWAYS, ALLIED_HEALTH_BARS_OFF, HeroSetting,
    };
    use orpheus_script::{
        snapshot::ProjectedSnapshot,
        stream::BodyStream,
        vm::{StateKind, TargetTagEffect},
        wire::Value as ScriptValue,
    };
    use std::cell::Cell;

    const SUPPORT_TAG: u64 = 0x0d60_0000_0000_0042;
    let runtime = orpheus_testkit::runtime_dir();
    let mut assets = SceneAssets::load(runtime).unwrap();
    // Ana's graphs still have gaps: tolerated, as release builds do.
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    let owner = 0xa000_0100;
    let viewer = PlayerAppearance {
        entity: owner,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card: &PlayerCard::default(),
    };
    let mut world = WorldDelivery::new(owner);
    world.start(&scene, &viewer, FIRST_TICK).unwrap();
    world.created.insert(owner);
    world
        .present(
            &scene,
            crate::mode_view::ModeFrame::Full(scene.mode_ui()),
            FIRST_TICK,
        )
        .unwrap();
    let general = |value| HeroSetting {
        hero: 0,
        setting: ALLIED_HEALTH_BARS,
        value,
    };
    // What the owner's client is sent: the body's first frame, then an
    // owner frame after each change, each acknowledged before the next.
    let open = |world: &WorldDelivery| {
        let script = world.body_script.as_ref().unwrap();
        let encoder = script.runtime.encoder();
        let first = encoder
            .project(script.runtime.snapshot().unwrap(), View::Owner)
            .unwrap();
        let mut stream = BodyStream::<ProjectedSnapshot>::new(script.id.entity);
        let chunk = encoder.queue_full(&mut stream, &first).unwrap();
        stream.create_arrived();
        stream.sent(chunk.last);
        stream.arrived(chunk.last);
        stream
    };
    let frame = Cell::new(FIRST_TICK);
    // The 01CF states 114 (OFF: drop the tag) and 117 (ALWAYS: take it):
    // each one's effect while it is active, which the next owner frame
    // carries, with the setting's variable.
    let tag_states = |world: &WorldDelivery, stream: &mut BodyStream<ProjectedSnapshot>| {
        let script = world.body_script.as_ref().unwrap();
        let snapshot = script.runtime.snapshot().unwrap();
        let encoder = script.runtime.encoder();
        frame.set(frame.get() + 32);
        let kind = stream
            .owner_kind(frame.get(), false, false)
            .unwrap()
            .unwrap();
        let next = encoder
            .project_next(snapshot, stream.base().unwrap().1)
            .unwrap();
        let chunk = encoder
            .queue_delta(stream, &next, kind, FRAME_MS as u32)
            .unwrap()
            .unwrap();
        stream.sent(chunk.last);
        stream.arrived(chunk.last);
        let delivered = stream.base().unwrap().1;
        assert_eq!(
            delivered
                .entity_variable(ALLIED_HEALTH_BARS as u32)
                .filter(|value| **value != ScriptValue::Null),
            snapshot
                .variables
                .get(&(ALLIED_HEALTH_BARS as u32))
                .filter(|value| **value != ScriptValue::Null),
            "#8336e in the owner frame"
        );
        let (&id, instance) = snapshot
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == 0x01cf)
            .expect("the body runs 01CF");
        [114, 117].map(|raw| {
            let effect = match instance.states.get(&raw) {
                Some(state) if state.active => match state.kind {
                    StateKind::TargetTag { effect } => effect,
                    _ => None,
                },
                _ => None,
            };
            assert_eq!(
                delivered.state(id, raw).is_some(),
                effect.is_some(),
                "st{raw} in the owner frame"
            );
            effect
        })
    };
    let tag = |entity, present| {
        Some(TargetTagEffect {
            entity,
            tag: SUPPORT_TAG,
            present,
        })
    };
    let spawn = |world: &mut WorldDelivery, entity, hero: &Hero| {
        world
            .switch(
                &scene,
                &viewer,
                FIRST_TICK,
                RETAIL_QUANTA,
                entity,
                (hero, SkinTheme::default()),
                true,
            )
            .unwrap();
        assert!(world.body_script.is_some(), "{} has no script", hero.name);
        open(world)
    };
    // The new body's create carries the tags its states took as it started
    // (component 76: added, suppressed), as the update would.
    let created_tags = |world: &WorldDelivery, entity: u32| {
        world.queued.iter().find_map(|record| {
            let Some(Operation::Create(definition)) = &record.update.operation else {
                return None;
            };
            let component = definition
                .components
                .iter()
                .find(|component| record.update.entity == entity && component.index == 76)?;
            match component.fields.as_slice() {
                [
                    Some(ComponentValue::U32Array(added)),
                    Some(ComponentValue::U32Array(suppressed)),
                ] => Some((added.clone(), suppressed.clone())),
                _ => None,
            }
        })
    };

    let soldier = scene.heroes().get(SOLDIER).unwrap();
    let mut stream = spawn(&mut world, owner | 1, soldier);
    assert_eq!(tag_states(&world, &mut stream), [None, None], "DEFAULT");
    world.update_hero_settings(&[general(ALLIED_HEALTH_BARS_ALWAYS)]);
    assert_eq!(
        tag_states(&world, &mut stream),
        [None, tag(owner | 1, true)]
    );
    world.update_hero_settings(&[]);
    assert_eq!(
        tag_states(&world, &mut stream),
        [None, None],
        "back to DEFAULT"
    );
    world.update_hero_settings(&[general(ALLIED_HEALTH_BARS_ALWAYS)]);
    let mut stream = spawn(&mut world, owner | 2, soldier);
    assert_eq!(
        tag_states(&world, &mut stream),
        [None, tag(owner | 2, true)],
        "respawn"
    );
    let (added, suppressed) = created_tags(&world, owner | 2).expect("tags in the create");
    assert!(added.contains(&0x42) && suppressed.is_empty(), "{added:?}");

    let ana = scene.heroes().find_hero("Ana").unwrap();
    let mut stream = spawn(&mut world, owner | 3, ana);
    assert_eq!(
        tag_states(&world, &mut stream),
        [None, None],
        "ALWAYS: a support has it"
    );
    world.update_hero_settings(&[general(ALLIED_HEALTH_BARS_OFF)]);
    assert_eq!(
        tag_states(&world, &mut stream),
        [tag(owner | 3, false), None]
    );
    let mut stream = spawn(&mut world, owner | 4, ana);
    assert_eq!(
        tag_states(&world, &mut stream),
        [tag(owner | 4, false), None],
        "respawn"
    );
    assert_eq!(
        created_tags(&world, owner | 4).map(|(_, suppressed)| suppressed),
        Some(vec![0x42])
    );
    world.update_hero_settings(&[]);
    assert_eq!(
        tag_states(&world, &mut stream),
        [None, None],
        "back to DEFAULT"
    );
    assert!(!world.script_failed);
}

/// The hero settings a body starts with (`hero_setting_defaults`, from
/// `orpheus_content::hero_settings::SCRIPT_SETTINGS`) are every setting of
/// the data (`054/0126`) that a node the server runs (not client-only)
/// reads, with the data's hero and default. A setting a networked node reads
/// that the table lacked would be null on the server, and the node could
/// take another branch than the client's. A read is a variable
/// (`STUConfigVarDynamic`) in a node's fields, without the lists that name
/// variables to sync rather than read them: ClientOnlySharedVars
/// (`STU_9D7BF987`) and the remote sync lists (`m_BF5B22B7`, `m_8BF03679`).
#[test]
#[ignore = "requires the 1.74 install and the derived 104319 statescript catalog"]
fn the_script_settings_are_the_hero_settings_server_nodes_read() {
    use std::collections::BTreeSet;

    use orpheus_content::hero_settings::{SCRIPT_SETTINGS, ScriptSetting, SettingDefault};
    use orpheus_stu::{Asset, Object, Value as Stu};
    use serde_json::Value as Json;

    const INDEX: u64 = 0xffff_ffff_ffff;
    const HERO: u64 = 0x02e0_0000_0000_0000;
    const IDENTIFIER: u64 = 0x0d80_0000_0000_0000;
    fn reads(value: &Json, found: &mut BTreeSet<u64>) {
        match value {
            Json::Object(fields) => {
                if fields.get("$").and_then(Json::as_str) == Some("STUConfigVarDynamic")
                    && let Some(id) = fields.get("m_identifier").and_then(Json::as_u64)
                {
                    found.insert(id);
                }
                fields.values().for_each(|value| reads(value, found));
            }
            Json::Array(values) => values.iter().for_each(|value| reads(value, found)),
            _ => {}
        }
    }
    fn objects<'a>(asset: &'a Asset, object: &'a Object, name: &str) -> Vec<&'a Object> {
        match object.get(name) {
            Some(Stu::Array(values)) => values
                .iter()
                .map(|value| asset.resolve(value).unwrap())
                .collect(),
            other => panic!("{name}: {other:?}"),
        }
    }

    let install = orpheus_assets::Install::open(&orpheus_testkit::game_dir(), "enUS").unwrap();
    let guid = install
        .guids(0x054)
        .into_iter()
        .find(|guid| guid & INDEX == 0x126)
        .unwrap();
    let asset = install.stu(guid).unwrap();
    let reference = |object: &Object, name: &str| match object.get(name) {
        Some(Stu::Guid(guid)) => guid & INDEX,
        _ => 0,
    };
    let setting = |hero: u64, object: &Object| {
        let default = object.get("m_default");
        let default = match (object.class_name(), default) {
            (Some("STUHeroSettingBool"), Some(Stu::Int(value))) => {
                SettingDefault::Bool(*value != 0)
            }
            (Some("STUHeroSettingBool"), Some(Stu::UInt(value))) => {
                SettingDefault::Bool(*value != 0)
            }
            (Some("STUHeroSettingFloat"), Some(Stu::Float(value))) => {
                SettingDefault::Float(*value as f32)
            }
            (Some("STUHeroSettingIdentifier"), _) => SettingDefault::Identifier(
                Some(reference(object, "m_default"))
                    .filter(|&index| index != 0)
                    .map(|index| IDENTIFIER | index),
            ),
            (class, default) => panic!("{class:?} with the default {default:?}"),
        };
        ScriptSetting {
            hero: if hero == 0 { 0 } else { HERO | hero },
            setting: IDENTIFIER | reference(object, "m_identifier"),
            default,
        }
    };
    let root = asset.root().unwrap();
    let mut data: Vec<_> = objects(&asset, root, "m_142A3CA9")
        .into_iter()
        .map(|object| setting(0, object))
        .collect();
    for hero in objects(&asset, root, "m_heroSpecificSettings") {
        let index = reference(hero, "m_hero");
        data.extend(
            objects(&asset, hero, "m_settings")
                .into_iter()
                .map(|object| setting(index, object)),
        );
    }
    assert_eq!(data.len(), 91, "the settings of 054/0126");

    let graphs =
        GraphStore::load(orpheus_testkit::runtime_dir().join("data/game_graphs_174.json.gz"))
            .unwrap();
    let mut read = BTreeSet::new();
    for index in graphs.graph_indexes() {
        let nodes = graphs.graph(index).unwrap().nodes.iter();
        for node in nodes.filter(|node| !node.client_only && node.class != "STU_9D7BF987") {
            let Json::Object(fields) = &node.fields else {
                continue;
            };
            for (name, value) in fields {
                if name != "m_BF5B22B7" && name != "m_8BF03679" {
                    reads(value, &mut read);
                }
            }
        }
    }
    let key = |setting: &ScriptSetting| (setting.hero, setting.setting);
    let mut expected: Vec<_> = data
        .into_iter()
        .filter(|setting| read.contains(&(setting.setting & INDEX)))
        .collect();
    expected.sort_by_key(key);
    let mut table = SCRIPT_SETTINGS.to_vec();
    table.sort_by_key(key);
    assert_eq!(table, expected);
}

mod hero_moves;
mod hero_select;
mod hero_switch;
mod hurt;
mod input_timing;
mod messages;
mod multikills;
mod recovery;

mod spawn_room;
mod workshop;
