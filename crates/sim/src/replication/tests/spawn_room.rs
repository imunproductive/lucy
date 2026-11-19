//! The Practice Range's spawn room and its fall out of the map
//! (`crate::map_areas`): invulnerable and healed inside by the mode body
//! graph `0C8E` on the room's `0723`, hero change only inside and where the
//! hero stands, the client's spawn room flag, and the environment's kill;
//! Watchpoint: Gibraltar's kill volumes; and the server placeables of the
//! Range, Rialto and Volskaya.

use super::*;
use crate::combat::{
    Combat, CombatPlayer, ENVIRONMENT_DAMAGE, Shooter, ShotContext, apply_player_hits,
};
use crate::map_areas::room_message;
use crate::messages::{HeldReach, Players, deliver_held};
use crate::mode_script::ModeRunner;
use crate::scene::{MOVING_KINDS, filter_component};
use crate::scene_assets::{FellOut, install_collision_build};
use orpheus_content::{maps::SpawnChoice, stats::PlayerStats};
use orpheus_script::vm::{GameMessage, MessageEnd};
use orpheus_wire::combat_messages::CombatNotice;

const OWNER: u32 = 0xa000_0100;
/// The Range's spawn room, the placeable `0D62` runs on.
const ROOM: u32 = 0x8000_007e;
/// The room's game message to the heroes inside (`0723.025`).
const IN_ROOM: u64 = 0x0240_0000_0000_0723;

fn scene() -> PreparedScene {
    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    assets.set_player_unknowns(orpheus_script::vm::Unknowns::Tolerate);
    assets.prepare(GameMap::practice_range()).unwrap()
}

fn viewer(card: &PlayerCard) -> PlayerAppearance<'_> {
    PlayerAppearance {
        entity: OWNER,
        slot: 0,
        team: 0,
        spawn: 0,
        room: SpawnChoice::First,
        card,
    }
}

/// A started world whose player has a Soldier: 76 at his spawn point, with
/// what the Range's `13C0` holds to every entity given to his controller
/// and body once, as the room does (`crate::messages::deliver_held`): among
/// them `06FD` (3 respawn seconds), without which `0C8E` counts no respawn.
fn spawned(scene: &PreparedScene, viewer: &PlayerAppearance<'_>) -> WorldDelivery {
    let mut world = WorldDelivery::new(OWNER);
    world.start(scene, viewer, FIRST_TICK).unwrap();
    world.created.insert(OWNER);
    world.present(scene, scene.mode_ui(), FIRST_TICK).unwrap();
    let hero = scene.heroes().get(SOLDIER).unwrap();
    world
        .switch(
            scene,
            viewer,
            FIRST_TICK,
            RETAIL_QUANTA,
            OWNER | 1,
            (hero, SkinTheme::default()),
            true,
        )
        .unwrap();
    let mode = ModeRunner::start(scene, FIRST_TICK, Vec::new());
    assert!(mode.graph_driven(), "{:?}", mode.gaps());
    let held = mode.held().to_vec();
    let mut worlds = [&mut world];
    let mut players = Players::new(&mut worlds, FIRST_TICK);
    deliver_held(&held, &mut HeldReach::default(), &mut [&mut players]);
    world
}

/// Puts the body's feet at `position`, facing `yaw`.
fn place(world: &mut WorldDelivery, scene: &PreparedScene, position: [f64; 3], yaw: i32) {
    let body = scene.heroes().get(world.hero().unwrap()).unwrap().body;
    world.mover = Some(scene.player_mover_at(body, 0, position, yaw).unwrap());
}

/// Out on the range, where the first bot stands.
fn outside(scene: &PreparedScene) -> [f64; 3] {
    scene.bots()[0].position
}

fn pick(
    world: &mut WorldDelivery,
    scene: &PreparedScene,
    viewer: &PlayerAppearance<'_>,
    hero: u64,
) {
    let message = Message {
        id: CLIENT_GAME_MESSAGE,
        fields: Fields::from([
            (
                0x78,
                Value::Struct(Fields::from([(0, Value::Unsigned(u64::from(OWNER)))])),
            ),
            (0x80, Value::Blob(hero.to_le_bytes().to_vec())),
            (0xa8, Value::Unsigned(PICK_HERO)),
            (0xb0, Value::Signed(3)),
        ]),
    };
    world
        .game_message(
            scene,
            viewer,
            FIRST_TICK + 10,
            Duration::ZERO,
            RETAIL_QUANTA,
            &message,
        )
        .unwrap();
}

fn hit(body: u32, amount: f64) -> PlayerHit {
    PlayerHit {
        body,
        shooter: Shooter {
            source: 0xa000_0201,
            team: 1,
            bot: false,
        },
        amount,
        critical: false,
        loadout: None,
        frame: FIRST_TICK,
        world_tick: None,
        heal: false,
        tags: orpheus_script::weapons::DamageTags::NONE,
        direction: None,
        from: None,
    }
}

/// The spawn room flag records queued so far: component 74 with field 6
/// alone.
fn flags(world: &WorldDelivery) -> Vec<bool> {
    world
        .queued
        .iter()
        .filter(|record| matches!(record.delivery, Delivery::SpawnFlag { .. }))
        .map(|record| {
            let Some(Operation::Update(components)) = &record.update.operation else {
                panic!("an update");
            };
            assert_eq!(record.update.entity, OWNER);
            let [component] = &components[..] else {
                panic!("one component");
            };
            assert_eq!(component.index, 74);
            assert!(component.fields[..6].iter().all(Option::is_none));
            component.fields[6] == Some(orpheus_wire::entity::ComponentValue::U8(1))
        })
        .collect()
}

/// The body's filter word records queued so far: whether each carries the
/// status bit (`crate::scene::SHIELDED`).
fn shields(world: &WorldDelivery) -> Vec<bool> {
    let body = world.current_body().unwrap();
    world
        .queued
        .iter()
        .filter(|record| matches!(record.delivery, Delivery::Filter { .. }))
        .map(|record| {
            let Some(Operation::Update(components)) = &record.update.operation else {
                panic!("an update");
            };
            assert_eq!(record.update.entity, body);
            assert_eq!(components.len(), 1);
            let Some(orpheus_wire::entity::ComponentValue::Id(bits)) = &components[0].fields[0]
            else {
                panic!("component 26's word");
            };
            assert_eq!(components[0].index, 26);
            bits & crate::scene::SHIELDED != 0
        })
        .collect()
}

/// The room's `0723` with its three flags reaches the body, as the room's
/// `0D62` sends it to a hero of its team whose feet are inside
/// (`crate::placed_script`).
fn enter(world: &mut WorldDelivery) {
    let body = world.current_body().unwrap();
    assert!(world.receive_body_message(&GameMessage {
        sender: ROOM,
        target: body,
        message: IN_ROOM,
        parameters: room_message(None),
    }));
}

/// The room lets its `0723` go for the body that walked out.
fn leave(world: &mut WorldDelivery) {
    let body = world.current_body().unwrap();
    assert!(world.end_body_message(&MessageEnd {
        sender: ROOM,
        target: body,
        message: IN_ROOM,
    }));
}

/// Idle commands for `ticks` ticks; the health changes the body's graphs
/// made meanwhile go through the match's combat as in `game_service`.
fn idle(
    world: &mut WorldDelivery,
    scene: &mut PreparedScene,
    viewer: &PlayerAppearance<'_>,
    tick: &mut u32,
    ticks: u32,
) {
    let mut combat = Combat::new(scene);
    for _ in 0..ticks {
        *tick += 1;
        world.queue_commands(
            *tick,
            &[Command {
                frame: *tick,
                ..Command::default()
            }],
        );
        world.release_input(1);
        while world.has_input_work() {
            world
                .advance_input(scene, viewer, RETAIL_QUANTA, 64, None)
                .unwrap();
        }
        let target = world.combat_target(scene, 0).unwrap();
        combat.record_poses(scene, &[target], *tick);
        let context = ShotContext {
            quanta: RETAIL_QUANTA,
            tick: *tick,
            now: Duration::ZERO,
        };
        for shot in world.take_shots() {
            if let PosedShot::Health(change) = shot {
                assert_eq!(change.instigator, ROOM);
                combat.modify_health(scene, &change, context, |_| true);
            }
        }
        let mut stats = PlayerStats::default();
        apply_player_hits(
            scene,
            combat.take_player_hits(),
            &mut [CombatPlayer {
                entity: OWNER,
                world: Some(world),
                stats: &mut stats,
            }],
            *tick,
            Duration::ZERO,
        );
    }
}

/// Idle commands a tick at a time, as `game_service` does: each tick the
/// body's messages are taken (its `0C8E`'s `0685` among them) and then its
/// respawn checked. The last body frame run when it respawned (within
/// `limit` ticks).
fn respawn_frame(
    world: &mut WorldDelivery,
    scene: &PreparedScene,
    viewer: &PlayerAppearance<'_>,
    tick: &mut u32,
    limit: u32,
) -> Option<u32> {
    for _ in 0..limit {
        *tick += 1;
        world.queue_commands(
            *tick,
            &[Command {
                frame: *tick,
                ..Command::default()
            }],
        );
        world.release_input(1);
        while world.has_input_work() {
            world
                .advance_input(scene, viewer, RETAIL_QUANTA, 64, None)
                .unwrap();
        }
        world.take_body_messages();
        let frame = world.input.processed;
        // A late clock: only the body's `0685` may make it due.
        if world
            .respawn_due(scene, viewer, *tick, Duration::ZERO, RETAIL_QUANTA)
            .unwrap()
        {
            return frame;
        }
    }
    None
}

/// At the spawn point the body is in its room and free to change heroes;
/// with the room's `0723` it is invulnerable (`0C8E` st11) and its filter
/// word carries the status bit, so its client predicts no push. Out on the
/// range, with the message let go, the bit goes, it takes damage, the
/// client's flag goes off (once), and a pick is refused. Back in the room
/// the flag comes back on and `0C8E` st13 heals it 400 a second, the room
/// its instigator.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn the_spawn_room_shields_heals_and_gates_hero_change() {
    let mut scene = scene();
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let body = world.current_body().unwrap();
    world.area_step(&scene, &viewer);
    assert_eq!(world.in_spawn_room(), Some(true));
    assert!(world.may_change_hero());
    assert!(flags(&world).is_empty(), "the create sent the flag set");
    assert!(
        shields(&world).is_empty(),
        "the create's word has no status"
    );
    enter(&mut world);
    world.area_step(&scene, &viewer);
    assert_eq!(shields(&world), [true]);
    assert_eq!(
        world.damage_body(&scene, &hit(body, 50.0), FIRST_TICK, Duration::ZERO),
        None
    );

    place(&mut world, &scene, outside(&scene), 0);
    leave(&mut world);
    world.area_step(&scene, &viewer);
    // The newer word supersedes the unsent one.
    assert_eq!(shields(&world), [false]);
    assert_eq!(world.in_spawn_room(), Some(false));
    assert!(!world.may_change_hero());
    world.area_step(&scene, &viewer);
    assert_eq!(flags(&world), [false]);
    let damage = world
        .damage_body(&scene, &hit(body, 50.0), FIRST_TICK, Duration::ZERO)
        .unwrap();
    assert_eq!(damage.dealt, 50.0);
    let hurt = world.health_share().unwrap();
    assert!(hurt < 1.0);

    let tracer = scene.heroes().find_hero("Tracer").unwrap().guid;
    pick(&mut world, &scene, &viewer, tracer);
    assert_eq!(world.hero(), Some(SOLDIER), "a pick outside is refused");
    assert_eq!(world.current_body(), Some(body));

    // Outside nothing heals him.
    let mut tick = FIRST_TICK;
    idle(&mut world, &mut scene, &viewer, &mut tick, 4);
    assert_eq!(world.health_share(), Some(hurt));

    // Back in the room.
    let (spawn, _) = scene
        .map()
        .spawn_point_in(0, 0, SpawnChoice::First)
        .unwrap();
    place(&mut world, &scene, spawn, 0);
    world.area_step(&scene, &viewer);
    // The newer flag supersedes the unsent one: the client gets it alone.
    assert_eq!(flags(&world), [true]);
    enter(&mut world);
    idle(&mut world, &mut scene, &viewer, &mut tick, 4);
    let healed = world.health_share().unwrap();
    assert!(healed > hurt, "{healed} after {hurt}");
}

/// The height his own explosion lifts the body in 20 ticks: an exploding
/// flight whose end pushes him 8 m/s up (`advance_projectiles`), then his
/// mover runs idle commands.
fn lift(
    world: &mut WorldDelivery,
    scene: &PreparedScene,
    viewer: &PlayerAppearance<'_>,
    tick: &mut u32,
) -> f64 {
    let ballistics = orpheus_script::weapons::ProjectileBallistics {
        speed: 50.0,
        lifetime: 2.0,
        definition: None,
        direct: None,
        blast: None,
        radius: 0.0,
        gravity: 0.0,
        fuse: None,
        unmodelled: Vec::new(),
        ray_profile: None,
    };
    let flight = ProjectileFlight::new(ballistics, [0.0; 3], [0.0, 0.0, 1.0], None).unwrap();
    world
        .body_script
        .as_mut()
        .unwrap()
        .flights
        .push((None, flight));
    let floor = world.mover.as_ref().unwrap().pose().position[1];
    world
        .advance_projectiles(*tick, |_, _| {
            Ok(Some(FlightEnd {
                point: [0.0; 3],
                exploded: true,
                travelled: 0.0,
                knockback: Some([0.0, 8.0, 0.0]),
            }))
        })
        .unwrap();
    rise(world, scene, viewer, tick, floor)
}

/// How far above `floor` the body rises in 20 ticks of idle commands.
fn rise(
    world: &mut WorldDelivery,
    scene: &PreparedScene,
    viewer: &PlayerAppearance<'_>,
    tick: &mut u32,
    floor: f64,
) -> f64 {
    let mut top = floor;
    for _ in 0..20 {
        *tick += 1;
        world.queue_commands(
            *tick,
            &[orpheus_wire::commands::Command {
                frame: *tick,
                ..Default::default()
            }],
        );
        world.release_input(1);
        while world.has_input_work() {
            world
                .advance_input(scene, viewer, RETAIL_QUANTA, 64, None)
                .unwrap();
        }
        top = top.max(world.mover.as_ref().unwrap().pose().position[1]);
    }
    top - floor
}

/// The height an enemy's Helix Rocket (graph `0254` node 35's numbers)
/// lifts the body in 20 ticks, as the match runs it: it flies straight
/// down 0.8 m beside his feet and bursts on the ground, 0.4 m from his
/// capsule, within the 0.5 m knockback ring (4 m/s away from the burst, 6
/// m/s up); the room's combat deals its hits, then hands its pushes to the
/// victim's mover (`apply_player_pushes`).
fn blasted(
    world: &mut WorldDelivery,
    scene: &mut PreparedScene,
    viewer: &PlayerAppearance<'_>,
    tick: &mut u32,
) -> f64 {
    use crate::combat::apply_player_pushes;
    use orpheus_script::weapons::{
        AllyHits, Blast, DamageTags, HitDefinition, Knockback, ProjectileBallistics,
    };
    let hit = |amount| HitDefinition {
        amount,
        loadout: None,
        critical: 1.0,
        falloff: None,
        tags: DamageTags::NONE,
        allies: AllyHits::None,
    };
    let helix = ProjectileBallistics {
        speed: 50.0,
        lifetime: 2.0,
        definition: None,
        direct: Some(hit(40.0)),
        blast: Some(Blast {
            hit: hit(80.0),
            rings: vec![(1.0, 1.0), (3.0, 0.5)],
            self_damage: 0.5,
            aura: None,
            knockback: Some(Knockback {
                away: 4.0,
                upward: 6.0,
                away_rings: vec![(0.5, 1.0), (3.0, 0.75)],
                upward_rings: vec![(0.5, 1.0), (3.0, 0.25)],
            }),
        }),
        radius: 0.0,
        gravity: 0.0,
        fuse: None,
        unmodelled: Vec::new(),
        ray_profile: None,
    };
    let enemy = Shooter {
        source: 0xa000_0201,
        team: 1,
        bot: false,
    };
    let target = world.combat_target(scene, 0).unwrap();
    let [x, y, z] = target.pose.feet;
    let mut combat = Combat::new(scene);
    combat.record_poses(scene, &[target], *tick);
    let mut flight =
        ProjectileFlight::new(helix, [x + 0.8, y + 0.3, z], [0.0, -1.0, 0.0], Some(*tick)).unwrap();
    let context = ShotContext {
        quanta: RETAIL_QUANTA,
        tick: *tick + 1,
        now: Duration::ZERO,
    };
    let end = combat
        .step_projectile(scene, enemy, &mut flight, context, &mut Vec::new())
        .unwrap();
    assert!(
        end.is_some_and(|end| end.exploded),
        "it bursts on the ground"
    );
    let floor = world.mover.as_ref().unwrap().pose().position[1];
    let mut stats = PlayerStats::default();
    let mut players = [CombatPlayer {
        entity: OWNER,
        world: Some(&mut *world),
        stats: &mut stats,
    }];
    apply_player_hits(
        scene,
        combat.take_player_hits(),
        &mut players,
        context.tick,
        Duration::ZERO,
    );
    apply_player_pushes(combat.take_player_pushes(), &mut players);
    rise(world, scene, viewer, tick, floor)
}

/// An explosion pushes only a body it can hurt (the user, 2026-11-17): his
/// own Helix Rocket's knockback and an enemy's leave him standing, and
/// unhurt, in the spawn room, where the room's `0723` makes him
/// invulnerable (its status bit 0x4 also stops his client's own push), and
/// lift him out on the range, the enemy's through the room's combat as a
/// push on his mover, whose owner corrections his client follows (it
/// predicts no push from another's explosion).
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn explosions_push_him_only_outside_the_spawn_room() {
    let mut scene = scene();
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let (spawn, _) = scene
        .map()
        .spawn_point_in(0, 0, SpawnChoice::First)
        .unwrap();
    place(&mut world, &scene, spawn, 0);
    world.area_step(&scene, &viewer);
    assert_eq!(world.in_spawn_room(), Some(true));
    enter(&mut world);
    let mut tick = FIRST_TICK;
    let lifted = lift(&mut world, &scene, &viewer, &mut tick);
    assert!(lifted.abs() < 0.01, "{lifted} m up in the room");
    let lifted = blasted(&mut world, &mut scene, &viewer, &mut tick);
    assert!(lifted.abs() < 0.01, "{lifted} m up in the room");
    assert_eq!(world.health_share(), Some(1.0));

    let outside = outside(&scene);
    place(&mut world, &scene, outside, 0);
    leave(&mut world);
    world.area_step(&scene, &viewer);
    assert_eq!(world.in_spawn_room(), Some(false));
    let lifted = lift(&mut world, &scene, &viewer, &mut tick);
    assert!(lifted > 1.0, "{lifted} m up on the range");
    place(&mut world, &scene, outside, 0);
    let lifted = blasted(&mut world, &mut scene, &viewer, &mut tick);
    assert!(lifted > 1.0, "{lifted} m up on the range");
    assert!(world.health_share().unwrap() < 1.0, "the burst hurts him");
}

/// A hero change in the room keeps the body where it stands and its
/// facing, not the spawn point.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_hero_change_in_the_room_keeps_position_and_yaw() {
    let scene = scene();
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let (spawn, _) = scene
        .map()
        .spawn_point_in(0, 0, SpawnChoice::First)
        .unwrap();
    place(
        &mut world,
        &scene,
        [spawn[0] + 0.5, spawn[1], spawn[2] + 0.5],
        12345,
    );
    world.area_step(&scene, &viewer);
    assert_eq!(world.in_spawn_room(), Some(true));
    let before = world.mover.as_ref().unwrap().pose();
    let tracer = scene.heroes().find_hero("Tracer").unwrap().guid;
    pick(&mut world, &scene, &viewer, tracer);
    assert_eq!(world.hero(), Some(tracer));
    let after = world.mover.as_ref().unwrap().pose();
    assert_eq!(after.position, before.position);
    assert_eq!(after.yaw, 12345);
}

/// A fall onto the Range's pit floor (-11 m) kills: the rules test the
/// capsule centre, 1.25 m above the feet of a standing Soldier: 76 (2.0 m,
/// pogo 0.5), so landing there puts it at -9.75 m. The body is its own
/// killer ("YOU DIED", a death on the board) and it comes back at the spawn
/// point after the Range's 3 s, not a moment before.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_fall_onto_the_catch_floor_kills_and_respawns_after_the_delay() {
    let scene = scene();
    assert_eq!(scene.kill_height(), Some(-11.0));
    // A centre over the lowest floor a player walks on (-5.25 m).
    assert_eq!(scene.fell_out([95.0, -4.0, 10.0], 1.25), None);
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let body = world.current_body().unwrap();
    // Feet on the pit floor, where a fall out of the range ends.
    place(&mut world, &scene, [95.0, -11.0, 10.0], 0);
    let (centre, above_feet) = world.body_centre().unwrap();
    assert!((above_feet - 1.25).abs() < 1e-6, "{above_feet}");
    assert_eq!(
        scene.fell_out(centre, above_feet),
        Some(FellOut::Environment)
    );

    let mut combat = Combat::new(&scene);
    combat.environment_kill(body, 0, FIRST_TICK + 20);
    let hits = combat.take_player_hits();
    assert_eq!(hits[0].amount, ENVIRONMENT_DAMAGE);
    let mut stats = PlayerStats::default();
    let now = Duration::from_secs(10);
    let notices = apply_player_hits(
        &scene,
        hits,
        &mut [CombatPlayer {
            entity: OWNER,
            world: Some(&mut world),
            stats: &mut stats,
        }],
        FIRST_TICK + 20,
        now,
    );
    assert_eq!(
        notices[0],
        [CombatNotice::Death {
            source: body,
            killed_by: false
        }]
    );
    assert!(world.body_dead());
    assert!(world.may_change_hero(), "the dead may change heroes");
    assert_eq!(scene.respawn_time(), Duration::from_secs(3));
    // The body's `0C8E` counts #384 down from #9575 = `06FD`'s 3 s (#2153;
    // no overtime #3392e, scale #9531e 1) a 16 ms frame at a time from the
    // death's frame, FIRST_TICK (the body ran none before): 3000 ms is past
    // on frame FIRST_TICK + 188 (187 frames make 2992 ms), when it sends
    // `0685`, and the respawn follows on the tick that ran that frame. The
    // input runs the body's frames in order, one a tick, whatever the
    // commands' frames.
    let mut tick = FIRST_TICK + 20;
    assert_eq!(
        respawn_frame(&mut world, &scene, &viewer, &mut tick, 400),
        Some(FIRST_TICK + 188)
    );
    let (spawn, _) = scene
        .map()
        .spawn_point_in(0, 0, SpawnChoice::First)
        .unwrap();
    let back = world.body_position().unwrap();
    assert!((back[0] - spawn[0]).abs() < 0.01 && (back[2] - spawn[2]).abs() < 0.01);
    // The new body takes the revive message (`0013.025`, `00C6` on): its
    // `01CA` raises the respawn line `05D8` on the controller 1.5 s later,
    // which Soldier: 76's set answers with stimulus `002C`, once.
    let mut said = Vec::new();
    for index in 1..=120 {
        let tick = tick + index;
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
                .advance_input(&scene, &viewer, RETAIL_QUANTA, 64, None)
                .unwrap();
        }
        said.extend(
            world
                .take_voice_lines(&scene, tick)
                .into_iter()
                .map(|line| (line.speaker, line.stimulus, line.audience)),
        );
    }
    assert_eq!(
        said,
        [(world.current_body().unwrap(), 0x0ee0_0000_0000_002c, None)]
    );
}

/// Watchpoint: Gibraltar has no damage areas and no catch floor: a body that
/// falls off it dies when its capsule centre enters one of its kill volumes,
/// 4 m slabs a few metres below its lowest floor. The safety net, 20 m under
/// the map's lowest triangle (about -63.5 m), catches only what they miss.
#[test]
#[ignore = "requires a derived 104319 pack and the install (Gibraltar's collision)"]
fn a_fall_off_gibraltar_dies_in_its_kill_volume() {
    const GIBRALTAR: u64 = 0x0800_0000_0000_0184;
    let runtime = orpheus_testkit::runtime_dir();
    let assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(install_collision_build(orpheus_testkit::game_dir()));
    let map = orpheus_content::maps::MapCatalog::load(runtime.join("data"))
        .unwrap()
        .game_map(
            GIBRALTAR,
            crate::match_flow::ESCORT,
            "Escort",
            false,
            &[6, 6],
        )
        .unwrap();
    let scene = assets.prepare(map).unwrap();
    assert_eq!(scene.kill_height(), None);
    // A standing centre (1.25 m up) over the lowest floor the AI walks on
    // (-10.84 m) is safe.
    assert_eq!(scene.fell_out([33.375, -9.59, -104.625], 1.25), None);
    // The slab under the drop by the first checkpoint: top -11.72 m, so the
    // feet are at -12.97 m when the centre enters it.
    assert_eq!(scene.fell_out([100.7, -11.6, 58.4], 1.25), None);
    assert_eq!(
        scene.fell_out([100.7, -11.8, 58.4], 1.25),
        Some(FellOut::Environment)
    );
    // Under the slab, above the net: alive; below the net: dead.
    assert_eq!(scene.fell_out([100.7, -70.0, 58.4], 1.25), None);
    assert_eq!(
        scene.fell_out([100.7, -90.0, 58.4], 1.25),
        Some(FellOut::BelowMap)
    );
}

/// A pick while dead waits for the respawn, which plays the new hero.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_pick_while_dead_takes_the_respawn() {
    let scene = scene();
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let body = world.current_body().unwrap();
    place(&mut world, &scene, outside(&scene), 0);
    world.area_step(&scene, &viewer);
    world
        .damage_body(
            &scene,
            &hit(body, ENVIRONMENT_DAMAGE),
            FIRST_TICK,
            Duration::ZERO,
        )
        .unwrap();
    let tracer = scene.heroes().find_hero("Tracer").unwrap().guid;
    pick(&mut world, &scene, &viewer, tracer);
    assert_eq!(world.hero(), Some(SOLDIER), "still dead");
    // `0C8E`'s 3 s count (as in the fall test): 188 frames.
    let mut tick = FIRST_TICK;
    assert_eq!(
        respawn_frame(&mut world, &scene, &viewer, &mut tick, 400),
        Some(FIRST_TICK + 188)
    );
    assert_eq!(world.hero(), Some(tracer));
}

/// In overtime the mode entity's #3392e is on for the body's `0C8E`, whose
/// st34 makes the count 3 s longer, and the respawn waits for it.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn overtime_makes_0c8e_count_three_seconds_more() {
    let scene = scene();
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    world.set_overtime(true);
    let body = world.current_body().unwrap();
    place(&mut world, &scene, outside(&scene), 0);
    world.area_step(&scene, &viewer);
    world
        .damage_body(
            &scene,
            &hit(body, ENVIRONMENT_DAMAGE),
            FIRST_TICK,
            Duration::ZERO,
        )
        .unwrap();
    // #9575 = max((#2153 3 s + 3 s) × #9531e 1, …) = 6 s from the death's
    // frame FIRST_TICK: reached on frame FIRST_TICK + 375 (375 × 16 ms =
    // 6000 ms), 187 frames later than without overtime.
    let mut tick = FIRST_TICK;
    assert_eq!(
        respawn_frame(&mut world, &scene, &viewer, &mut tick, 800),
        Some(FIRST_TICK + 375)
    );
}

/// The map's server-owned placeables are created from the map after the
/// loading list, the player and the mode, before the bots: on the Practice
/// Range its 26 (the spawn room, the six barriers in the three doorways, the
/// Training Partner console and pad, ...), none of them with an identifier.
/// The four moving platforms (definition `0516`: 129, 194, 271, 403) are
/// created in the client's hitscan group (component 26 = 0x10000); the
/// others carry no components.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn the_server_placeables_follow_the_player_and_the_mode() {
    let scene = scene();
    let card = PlayerCard::default();
    let updates = scene.initial_world(&viewer(&card)).unwrap();
    let creates: Vec<(u32, &Vec<orpheus_wire::entity::Component>)> = updates
        .iter()
        .filter_map(|update| match &update.operation {
            Some(Operation::Placeable(_, components)) => {
                Some((update.entity & 0x7FFF_FFFF, components))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        creates
            .iter()
            .map(|(entity, _)| *entity)
            .collect::<Vec<_>>(),
        [
            0x88, 0x22e, 0x234, 0x289, // the loading list
            126, 129, 137, 150, 185, 188, 194, 199, 204, 243, 267, 271, 340, 358, 398, 400, 403,
            415, 416, 424, 479, 480, 482, 548, 586, 590,
        ]
    );
    let moving: Vec<u32> = creates
        .iter()
        .filter(|(_, components)| !components.is_empty())
        .map(|(entity, components)| {
            assert_eq!(**components, [filter_component(MOVING_KINDS)]);
            *entity
        })
        .collect();
    assert_eq!(moving, [129, 194, 271, 403]);
    // The 4 loading placeables, the player and the mode come first.
    assert_eq!(updates[6].entity, 0x8000_0000 | 126);
}

/// Each map and mode gets its own server placeables: Rialto's 224 in
/// Escort, and in Assault Volskaya's 159 less the 8 whose identifier
/// `2962` only modes `1D`, `1E`, `59` and `70` list (its four `00F6`
/// barriers, 1100 among them, and four `0D73` frames).
#[test]
#[ignore = "requires a derived 104319 pack and the install (Rialto's and Volskaya's collision)"]
fn rialto_and_volskaya_get_their_server_placeables() {
    const RIALTO: u64 = 0x0800_0000_0000_0871;
    const VOLSKAYA: u64 = 0x0800_0000_0000_01DB;
    let runtime = orpheus_testkit::runtime_dir();
    let assets = SceneAssets::load(&runtime)
        .unwrap()
        .with_collision_build(install_collision_build(orpheus_testkit::game_dir()));
    let catalog = orpheus_content::maps::MapCatalog::load(runtime.join("data")).unwrap();
    let card = PlayerCard::default();
    for (map, mode, name, server) in [
        (RIALTO, crate::match_flow::ESCORT, "Escort", 224),
        (VOLSKAYA, crate::match_flow::ASSAULT, "Assault", 151),
    ] {
        let map = catalog.game_map(map, mode, name, false, &[6, 6]).unwrap();
        let loading = map.placeables.len();
        let scene = assets.prepare(map).unwrap();
        let creates: Vec<u32> = scene
            .initial_world(&viewer(&card))
            .unwrap()
            .iter()
            .filter(|update| matches!(update.operation, Some(Operation::Placeable(..))))
            .map(|update| update.entity & 0x7FFF_FFFF)
            .collect();
        assert_eq!(creates.len(), loading + server, "{name}");
        assert!(!creates.contains(&1100), "{name}");
    }
}
