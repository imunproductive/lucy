//! A pick of the same hero after the body's script failed (ported from
//! tests/test_game_body_recovery.py; match.py `body_script_failed`,
//! `game_message`).

use super::*;
use orpheus_script::wire::Value as ScriptValue;
use orpheus_wire::commands::FIRE;

/// The body's runtime, whose commands fail while `fault` is set, as the
/// Python test's one-unit `MAX_SCRIPT_WORK` makes them.
struct Faulting {
    inner: Box<dyn OwnerBodyRuntime>,
    fault: Arc<AtomicBool>,
}

impl OwnerBodyRuntime for Faulting {
    fn snapshot(&self) -> Option<&Snapshot> {
        self.inner.snapshot()
    }

    fn encoder(&self) -> SnapshotEncoder<'_> {
        self.inner.encoder()
    }

    fn advance(&mut self, command: Command, mover: &Mover, step: BodyStep) -> io::Result<()> {
        if self.fault.load(Ordering::Relaxed) {
            return Err(io::Error::other("statescript work budget exhausted"));
        }
        self.inner.advance(command, mover, step)
    }

    fn frame_ms(&self) -> u32 {
        self.inner.frame_ms()
    }

    fn vitals(&self) -> Option<&Vitals> {
        self.inner.vitals()
    }

    fn take_health_update(&mut self) -> Option<WorldComponent> {
        self.inner.take_health_update()
    }

    fn projectile_leases(&self) -> Option<&ProjectileLeases> {
        self.inner.projectile_leases()
    }

    fn movement_modifiers(&self) -> Option<&BTreeMap<ModifierKey, Modifier>> {
        self.inner.movement_modifiers()
    }

    fn take_shots(&mut self, command: Option<Command>, mover: &Mover) -> Vec<PosedShot> {
        self.inner.take_shots(command, mover)
    }

    fn observer_activity(&self) -> Option<ObserverActivity> {
        self.inner.observer_activity()
    }

    fn vitals_mut(&mut self) -> Option<&mut Vitals> {
        self.inner.vitals_mut()
    }

    fn health_changed(&mut self) -> io::Result<()> {
        self.inner.health_changed()
    }

    fn receive_message(
        &mut self,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> io::Result<usize> {
        self.inner.receive_message(sender, message, parameters)
    }

    fn take_messages(&mut self) -> Vec<GameMessage> {
        self.inner.take_messages()
    }

    fn move_requests(&self) -> Vec<MoveRequest> {
        self.inner.move_requests()
    }

    fn finish_move(&mut self, request: MoveRequest, arrived: bool, frame: u32) -> io::Result<()> {
        self.inner.finish_move(request, arrived, frame)
    }

    fn take_motions(&mut self) -> Vec<Motion> {
        self.inner.take_motions()
    }

    fn begin_spawn_bark(&mut self, now: Duration) -> io::Result<()> {
        self.inner.begin_spawn_bark(now)
    }

    fn expire_bark(&mut self, now: Duration) -> io::Result<()> {
        self.inner.expire_bark(now)
    }
}

fn practice_range() -> PreparedScene {
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.prepare(GameMap::practice_range()).unwrap()
}

/// The scene's owner bodies, whose commands fail while `fault` is set.
fn faulting(scene: &mut PreparedScene, fault: &Arc<AtomicBool>) {
    let stock = scene.owner_body_factory.take().unwrap();
    let fault = Arc::clone(fault);
    scene.owner_body_factory = Some(Arc::new(move |hero, spawn| {
        Ok(stock(hero, spawn)?.map(|inner| {
            Box::new(Faulting {
                inner,
                fault: Arc::clone(&fault),
            }) as Box<dyn OwnerBodyRuntime>
        }))
    }));
}

/// The client's 21616 pick of `hero` (match.py `game_message`).
fn pick(hero: u64) -> Message {
    Message {
        id: CLIENT_GAME_MESSAGE,
        fields: Fields::from([
            (
                0x78,
                Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(OWNER)))])),
            ),
            (0x80, Value::Blob(hero.to_le_bytes().to_vec())),
            (0xa8, Value::Unsigned(PICK_HERO)),
            (0xb0, Value::Signed(0)),
        ]),
    }
}

fn fire(frame: u32) -> Command {
    Command {
        frame,
        buttons: FIRE,
        ..Command::default()
    }
}

fn drain(world: &mut WorldDelivery, scene: &PreparedScene, viewer: &PlayerAppearance<'_>) {
    while world.has_input_work() {
        world
            .advance_input(scene, viewer, RETAIL_QUANTA, 16)
            .unwrap();
    }
}

/// A player in the world who picked Soldier: 76.
fn soldier(scene: &PreparedScene, viewer: &PlayerAppearance<'_>) -> WorldDelivery {
    let mut world = WorldDelivery::new(OWNER).unbuffered();
    world.start(scene, viewer, FIRST_TICK).unwrap();
    world.created.insert(OWNER);
    world.present(scene, FIRST_TICK).unwrap();
    world.queue_game_message(FIRST_TICK, Duration::ZERO, pick(SOLDIER));
    drain(&mut world, scene, viewer);
    assert_eq!(world.body.unwrap().hero, SOLDIER);
    world
}

/// Soldier: 76's ammunition (instance 11, variable 53) as the VM has it.
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

/// The queued frames of this body's script: whether each is a delta.
fn script_frames(world: &WorldDelivery, body: BodyId) -> Vec<bool> {
    world
        .queued
        .iter()
        .filter_map(|record| match record.delivery {
            Delivery::BodyScript { id, delta, .. } if id == body => Some(delta),
            _ => None,
        })
        .collect()
}

fn lifetimes(world: &WorldDelivery) -> usize {
    world
        .queued
        .iter()
        .filter(|record| {
            matches!(
                record.delivery,
                Delivery::CreateBody(_) | Delivery::DestroyBody(_)
            )
        })
        .count()
}

/// Send frames, unacknowledged, until `sent` holds; the sequence of the
/// frame that made it hold.
fn send_until(
    world: &mut WorldDelivery,
    egress: &mut GameEgress<Arc<Record>>,
    tick: u32,
    sequence: &mut u32,
    sent: impl Fn(&WorldDelivery) -> bool,
) -> u32 {
    for _ in 0..64 {
        let this = *sequence;
        *sequence += 1;
        transmit(world, egress, tick, this);
        if sent(world) {
            return this;
        }
    }
    panic!("the record never went out");
}

/// tests/test_game_body_recovery.py
/// `test_same_hero_pick_after_fault_replaces_body_and_waits_for_fresh_acks`:
/// a command that faults the body's script drops it and its shots; a pick
/// of the same hero then gives a new body and mover with a new script from
/// the last command on, and the old body goes. A late ACK of the old
/// body's full frame does not start the new script's frames: they wait for
/// the new create's ACK, and its owner deltas for its full frame's ACK.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn a_same_hero_pick_after_a_fault_replaces_the_body_and_waits_for_fresh_acks() {
    let fault = Arc::new(AtomicBool::new(false));
    let mut scene = practice_range();
    faulting(&mut scene, &fault);
    let card = PlayerCard::default();
    let viewer = PlayerAppearance {
        entity: OWNER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: orpheus_content::maps::SpawnChoice::First,
        card: &card,
    };
    let mut world = soldier(&scene, &viewer);
    let old = world.body.unwrap().id;
    let mut egress = GameEgress::default();

    // The old body's create arrived and its full frame went out.
    let create = world
        .queued
        .iter()
        .find(|record| matches!(record.delivery, Delivery::CreateBody(id) if id == old))
        .cloned()
        .unwrap();
    world.acknowledge(outcome(create, true), Duration::ZERO);
    world.prepare_body_updates(FIRST_TICK);
    assert_eq!(script_frames(&world, old), [false]);
    let mut sequence = 0;
    let old_full = send_until(
        &mut world,
        &mut egress,
        FIRST_TICK,
        &mut sequence,
        |world| script_frames(world, old).is_empty(),
    );

    fault.store(true, Ordering::Relaxed);
    let first = fire(FIRST_TICK + 1);
    world.queue_commands(FIRST_TICK + 1, &[first]);
    drain(&mut world, &scene, &viewer);
    fault.store(false, Ordering::Relaxed);
    assert!(world.body_script.is_none());
    assert!(world.script_failed);
    assert!(world.take_shots().is_empty());
    assert_eq!(world.body.unwrap().id, old);
    let stood = world.mover.as_ref().unwrap().pose();

    world.queue_game_message(FIRST_TICK + 2, Duration::ZERO, pick(SOLDIER));
    drain(&mut world, &scene, &viewer);
    let new = world.body.unwrap().id;
    assert_ne!(new.entity, old.entity);
    assert_eq!(world.body.unwrap().hero, SOLDIER);
    let script = world.body_script.as_ref().unwrap();
    assert_eq!(script.id, new);
    assert_eq!(script.frame, first.frame);
    assert_eq!(world.input.processed, Some(first.frame));
    assert!(!world.script_failed);
    // A new mover where the old body stood, facing the same way: a living
    // body's pick never sends him back to the spawn point (`switch`), even
    // after a script fault.
    let hero = scene.heroes().get(SOLDIER).unwrap();
    let kept = scene
        .player_mover_at(hero.body, 0, stood.position, stood.yaw)
        .unwrap()
        .pose();
    assert_eq!(world.mover.as_ref().unwrap().pose(), kept);
    assert!(
        world
            .queued
            .iter()
            .any(|record| matches!(record.delivery, Delivery::DestroyBody(id) if id == old))
    );

    // A delayed positive ACK for the old full frame.
    transport_outcome(&mut world, &mut egress, old_full, true);
    world.prepare_body_updates(FIRST_TICK + 2);
    assert!(!world.created.contains(&new.entity));
    assert!(script_frames(&world, new).is_empty());

    // The new create goes out and arrives: the new script's full frame.
    let create = send_until(
        &mut world,
        &mut egress,
        FIRST_TICK + 2,
        &mut sequence,
        |world| {
            !world
                .queued
                .iter()
                .any(|record| matches!(record.delivery, Delivery::CreateBody(id) if id == new))
        },
    );
    transport_outcome(&mut world, &mut egress, create, true);
    assert!(world.created.contains(&new.entity));
    world.prepare_body_updates(FIRST_TICK + 2);
    assert_eq!(script_frames(&world, new), [false]);
    let full = send_until(
        &mut world,
        &mut egress,
        FIRST_TICK + 2,
        &mut sequence,
        |world| script_frames(world, new).is_empty(),
    );

    let second = fire(FIRST_TICK + 2);
    world.queue_commands(FIRST_TICK + 3, &[second]);
    drain(&mut world, &scene, &viewer);
    world.prepare_body_updates(FIRST_TICK + 3);
    // Still waiting for this body's full frame ACK.
    assert!(script_frames(&world, new).is_empty());
    transport_outcome(&mut world, &mut egress, full, true);
    world.prepare_body_updates(FIRST_TICK + 3);
    assert_eq!(script_frames(&world, new), [true]);
    let script = world.body_script.as_ref().unwrap();
    assert_eq!(script.stream.command_frame(), second.frame);
    assert_eq!(ammo(&world), ScriptValue::Integer(29));
}

/// tests/test_game_body_recovery.py
/// `test_healthy_and_script_disabled_same_hero_picks_keep_the_body`: a
/// pick of the same hero keeps the body, its mover and its script when the
/// script runs, and when the hero has no script at all.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn healthy_and_scriptless_same_hero_picks_keep_the_body() {
    for scripted in [true, false] {
        let mut scene = practice_range();
        if !scripted {
            scene.owner_body_factory = None;
        }
        let card = PlayerCard::default();
        let viewer = PlayerAppearance {
            entity: OWNER,
            slot: 0,
            team: 0,
            spawn: 0,
            room: orpheus_content::maps::SpawnChoice::First,
            card: &card,
        };
        let mut world = soldier(&scene, &viewer);
        let body = world.body.unwrap().id;
        assert_eq!(world.body_script.is_some(), scripted);
        let first = fire(FIRST_TICK + 1);
        world.queue_commands(FIRST_TICK + 1, &[first]);
        drain(&mut world, &scene, &viewer);
        if scripted {
            assert_eq!(ammo(&world), ScriptValue::Integer(29));
        }
        let pose = world.mover.as_ref().unwrap().pose();
        let lifetimes_before = lifetimes(&world);

        world.queue_game_message(FIRST_TICK + 2, Duration::ZERO, pick(SOLDIER));
        drain(&mut world, &scene, &viewer);
        assert_eq!(world.body.unwrap().id, body, "scripted: {scripted}");
        assert_eq!(world.mover.as_ref().unwrap().pose(), pose);
        assert_eq!(
            world
                .body_script
                .as_ref()
                .map(|script| (script.id, script.frame)),
            scripted.then_some((body, first.frame))
        );
        assert!(!world.script_failed);
        assert_eq!(lifetimes(&world), lifetimes_before);
        if scripted {
            assert_eq!(ammo(&world), ScriptValue::Integer(29));
        }
    }
}
