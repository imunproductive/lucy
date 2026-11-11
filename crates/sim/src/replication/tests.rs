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
    entity::{MAX_ENTITY_RECORDS, Movement},
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
        .work
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
    let mut world = WorldDelivery::new(owner);
    let first = FIRST_TICK + 1;
    world.queue_commands(FIRST_TICK, &[forward(first)]);
    assert!(!world.has_input_work());
    assert_eq!(world.command_ack(FIRST_TICK), FIRST_TICK);
    world.start(&scene, &viewer, FIRST_TICK).unwrap();
    world.queue_commands(FIRST_TICK, &[forward(first)]);
    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
            .unwrap(),
        (1, false)
    );
    assert_eq!(world.command_ack(FIRST_TICK + 2), first);
    world.created.insert(owner);
    world.present(&scene, FIRST_TICK).unwrap();
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
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
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
                .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
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
    assert_eq!(world.mover.as_ref().unwrap().pose(), synchronous.pose());
    assert_ne!(synchronous.pose().position, initial.position);

    assert_eq!(
        world
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
            .unwrap(),
        (2, false)
    );
    assert_eq!(world.body.unwrap().id.entity, owner | 2);
    assert_eq!(world.command_ack(FIRST_TICK + 90), later.frame);
    let mut synchronous = scene
        .player_spawn_mover(tracer.body, viewer.team, viewer.spawn, viewer.room)
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
            },
            DEFINITION,
            125,
        )
        .unwrap();
    let confirmation = projectiles.advance(133, 133);
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
    let command = forward(101);
    mover
        .step(&rays, command, 0.016, |mover, command| {
            world
                .body_script
                .as_mut()
                .unwrap()
                .advance(mover, command, BodyStep::Posed, false)
                .unwrap();
        })
        .unwrap();
    assert!(
        transmit(&mut world, &mut egress, 101, 5)
            .1
            .records
            .is_empty()
    );
    let stream = &world.body_script.as_ref().unwrap().stream;
    assert_eq!(
        stream.pending().map(|(last, ..)| last).collect::<Vec<_>>(),
        [3]
    );
    assert_eq!(stream.command_frame(), 101);

    world.push(record(OWNER, Delivery::Possession(id)));
    assert_eq!(
        transmit(&mut world, &mut egress, 101, 6).1.records,
        [RecordSource::Queued(0)]
    );
    transport_outcome(&mut world, &mut egress, 6, true);
    let (c0, sent) = transmit(&mut world, &mut egress, 102, 7);
    assert_eq!(sent.records, [RecordSource::Queued(0)]);
    let command = forward(117);
    mover
        .step(&rays, command, 0.016, |mover, command| {
            world
                .body_script
                .as_mut()
                .unwrap()
                .advance(mover, command, BodyStep::Posed, false)
                .unwrap();
        })
        .unwrap();
    assert!(
        transmit(&mut world, &mut egress, 102, 8)
            .1
            .records
            .is_empty()
    );
    transport_outcome(&mut world, &mut egress, 7, false);
    assert_eq!(transmit(&mut world, &mut egress, 102, 9).0, c0);
    transport_outcome(&mut world, &mut egress, 9, true);
    assert_eq!(
        transmit(&mut world, &mut egress, 117, 10).1.records,
        [RecordSource::Queued(0)]
    );
    transport_outcome(&mut world, &mut egress, 10, false);
    assert!(
        transmit(&mut world, &mut egress, 118, 11)
            .1
            .records
            .is_empty()
    );

    mover
        .step(&rays, forward(118), 0.016, |mover, command| {
            world
                .body_script
                .as_mut()
                .unwrap()
                .advance(mover, command, BodyStep::Posed, false)
                .unwrap();
        })
        .unwrap();
    assert_eq!(
        transmit(&mut world, &mut egress, 118, 12).1.records,
        [RecordSource::Queued(0)]
    );
    world.cancel_body_script();
    let reused = avatar(1, 257);
    activate_script(&mut world, reused, 7, Arc::default());
    transport_outcome(&mut world, &mut egress, 12, true);
    assert_eq!(world.body_script.as_ref().unwrap().stream.last(), 0);
    assert!(world.queued.is_empty());
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
    scene.owner_body_factory = Some(Arc::new(move |_, spawn| {
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
    let mut world = WorldDelivery::new(OWNER);
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
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
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
                .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
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
            .advance_input(&scene, &viewer, RETAIL_QUANTA, 16)
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
    use crate::{
        body_script::{fixture_body_factory, fixture_graph_factory},
        combat::deliver_body_messages,
    };
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
    deliver_body_messages(&mut [&mut a, &mut b]);
    assert_eq!(taken(&b), Some(Value::Integer(9)));
    assert_eq!(taken(&a), None);
    assert!(a.take_body_messages().is_empty(), "it went once");
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

    fn receive_message(&mut self, message: u64, _parameters: Variables) -> io::Result<usize> {
        self.inbox.lock().unwrap().push(message);
        Ok(1)
    }

    fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }
}

/// match.py `queue_body_message`: a message to an entity no player's body
/// is goes nowhere and takes no place in the match's queue, which holds
/// MAX_BODY_MESSAGES a tick; the first ones go, in order.
#[test]
fn body_messages_to_unknown_entities_go_nowhere_and_the_rest_are_capped() {
    use crate::combat::{MAX_BODY_MESSAGES, deliver_body_messages};

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
    let mut outbox = Vec::new();
    for message in 0..300 {
        outbox.push(mail(NOBODY, 1000 + message));
        outbox.push(mail(receiving.entity, message));
    }
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
    deliver_body_messages(&mut [&mut a, &mut b]);
    assert_eq!(
        *received.lock().unwrap(),
        (0..MAX_BODY_MESSAGES as u64).collect::<Vec<_>>()
    );
    assert!(sent.lock().unwrap().is_empty());
    deliver_body_messages(&mut [&mut a, &mut b]);
    assert_eq!(received.lock().unwrap().len(), MAX_BODY_MESSAGES);
}

mod hero_moves;
mod recovery;
