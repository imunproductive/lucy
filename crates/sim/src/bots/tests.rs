use super::*;
use orpheus_content::maps::PRACTICE_RANGE_MAP;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// Path work without a limit: these bots have no navigation.
static PATHS: PathBudget = PathBudget::unlimited();

const TRAINING: u64 = 0x02e0_0000_0000_016b;
const TRAINING_BODY: u64 = 0x0400_0000_0000_1403;
const FAR_RUNNER: u64 = 0x02e0_0000_0000_016e;

/// A walk from a mover with these run, acceleration and braking values.
fn walk(run_forward: f64, accel_ground: f64, decel_ground_base: f64) -> BotWalk {
    BotWalk::from_mover(&MoverSettings {
        run_forward,
        accel_ground,
        decel_ground_base,
        ..MoverSettings::default()
    })
}

fn walker(target: [f64; 3]) -> Bot {
    Bot::new(
        BOT_ENTITY,
        Arc::new(BotSpawn {
            hero: TRAINING,
            name: "Training Bot".into(),
            body: TRAINING_BODY,
            team: 1,
            health: 200.0,
            position: [0.0; 3],
            yaw: 0,
            firing: false,
            route: vec![[0.0; 3], target],
            initial_index: 0,
        }),
        walk(4.5, 1.0, 15.0),
    )
}

/// The spring steps of a Training Bot body at rest (1403, 14ED and the
/// fixture mover share the stance): its range is [-max(2.0 + 0.5, 1.25 +
/// 0.25), 2 × 0.5] = [-2.5, 1.0], and offset 0 is 2.5 / 3.5 of 32768 steps
/// (23405.7, rounded half up).
const RESTING_SPRING: [i32; 2] = [23406; 2];

/// Step `bot` through `ticks` with its script asking for `goal`, with no
/// floor data; the ticks its walk ended in. Every record has its spring at
/// rest.
fn walk_to(
    bot: &mut Bot,
    goal: Option<[f64; 3]>,
    ticks: impl IntoIterator<Item = u32>,
) -> Vec<u32> {
    ticks
        .into_iter()
        .filter_map(|tick| {
            let ended = bot.step(tick, goal, |_, _, _, _| None, &PATHS);
            assert_eq!(bot.movement(tick).spring, RESTING_SPRING, "tick {tick}");
            ended
        })
        .collect()
}

#[test]
fn catalogs_only_unconditional_spawners_and_preserves_the_hint_sequence() {
    let catalog = BotCatalog::from_json(r#"{
            "heroes": {
                "0x02E000000000016B": {"name":"Fixture bot","body":"0x0400000000001403","health":200}
            },
            "maps": {"0x0800000000000688": {
                "hints": {"a": [1,2,3], "b": [4,5,6]},
                "spawners": [
                    {"hero":"0x02E000000000016B","team":0,"identifier":"0xF",
                     "position":[0,0,0],"rotation":[0,0,0,1],"graphs":[]},
                    {"hero":"0x02E0000000000000","team":0,
                     "position":[0,0,0],"rotation":[0,0,0,1],"graphs":[]},
                    {"hero":"0x02E000000000016B","team":1,"identifier":null,
                     "position":[2,3,4],"rotation":[0,0.70710678118,0,0.70710678118],
                     "graphs":["0x0580000000000B8E","0x0580000000000B92"],
                     "vars":{"0x0D80000000001B23":["b","missing","a","b"],"0x0D80000000000027":2}},
                    {"hero":"0x02E000000000016B","team":0,
                     "position":[0,0,0],"rotation":[0,0,0,1],"graphs":[],
                     "vars":{"0x0D80000000001B23":["a"]}}
                ]
            }}
        }"#).unwrap();
    let bots = catalog.make(PRACTICE_RANGE_MAP, |body| {
        assert_eq!(body, TRAINING_BODY);
        walk(4.5, 1.0, 15.0)
    });
    assert_eq!(bots.len(), 2);
    assert_eq!(bots[0].entity, BOT_ENTITY);
    assert_eq!(bots[1].entity, BOT_ENTITY + 1);
    assert_eq!(bots[0].spawn.hero, TRAINING);
    assert_eq!(bots[0].spawn.name, "Fixture bot");
    assert_eq!(bots[0].spawn.team, 1);
    assert_eq!(bots[0].spawn.health, 200.0);
    assert_eq!(bots[0].position, [2.0, 3.0, 4.0]);
    assert_eq!(bots[0].yaw, 16384.0);
    assert!(bots[0].spawn.firing);
    assert_eq!(
        bots[0].spawn.route,
        [[4.0, 5.0, 6.0], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]
    );
    let patrol = bots[0].patrol().unwrap();
    assert_eq!(patrol.index, 2);
    assert_eq!(patrol.route, bots[0].spawn.route.as_slice());
    assert!(bots[1].patrol().is_none());
    assert!(!bots[1].spawn.firing);
    assert!(bots[1].spawn.route.is_empty());
    assert!(catalog.make(0, |_| unreachable!()).is_empty());
}

#[test]
fn missing_spawner_file_leaves_an_empty_catalog() {
    let root = tempfile::tempdir().unwrap();
    let catalog = BotCatalog::load(root.path().join("ai_spawners_174.json")).unwrap();
    assert!(
        catalog
            .make(PRACTICE_RANGE_MAP, |_| unreachable!())
            .is_empty()
    );
}

/// The walker sets off in the tick its script asks for a point and holds
/// its keys until ground friction will stop it there. The fixture walks at
/// 4.5 m/s, speeds up at 1.0 × 4.5 = 4.5 m/s² (0.072 m/s a 16 ms tick, so
/// full speed from tick 63, 2.249856 m covered in ticks 1-62) and brakes at
/// 15 × 4.5 = 67.5 m/s² (1.08 m/s a tick: 3.42, 2.34, 1.26, 0.18, 0.1152 m
/// of slide from full speed). 20 m behind it: 17.750144 m are left after
/// tick 62, 0.072 m a tick at full speed, and it lets go in the first tick
/// whose stride would leave no more than the slide: 62 + 245 = tick 307
/// (17.750144 − 245 × 0.072 = 0.110144 ≤ 0.1152), stopping on the point
/// four ticks later, in tick 311, the walk's end. It walks backwards and
/// turns about at 60° a second (180° in 3 s), so it faces the way it walked
/// by the end. Asked for the same point again it doesn't walk; a new point
/// sets it off in the tick it is asked for.
#[test]
fn a_walk_sets_off_when_asked_and_ends_still_on_its_point() {
    let origin = [0.0; 3];
    let behind = [0.0, 0.0, -20.0];
    let mut bot = walker(behind);
    assert!(walk_to(&mut bot, None, 1..=10).is_empty());
    assert!(!bot.walking());
    assert!(walk_to(&mut bot, Some(behind), [11]).is_empty());
    assert!(bot.walking());
    assert_ne!(bot.movement(11).throttles, [0; 2]);
    assert_eq!(walk_to(&mut bot, Some(behind), 12..=400), [321]);
    // Keys last held in tick 317 (10 + 307), still from 321 (10 + 311).
    assert_eq!(bot.movement(400).input_frame, Some(317));
    assert!(!bot.walking());
    assert_eq!(bot.position, behind);
    assert_eq!(bot.velocity, [0.0; 3]);
    assert_eq!(bot.yaw, -32768.0);
    assert!(walk_to(&mut bot, None, [401]).is_empty());
    assert!(walk_to(&mut bot, Some(origin), [402]).is_empty());
    assert!(bot.walking());
    assert_eq!(bot.target, Some(origin));
}

#[test]
fn skipped_ticks_catch_up_without_replaying_a_duplicate_tick() {
    let goal = Some([0.0, 0.0, 10.0]);
    let mut contiguous = walker([0.0, 0.0, 10.0]);
    for tick in 1..=120 {
        contiguous.step(tick, goal, |_, _, _, _| None, &PATHS);
    }
    let mut skipped = walker([0.0, 0.0, 10.0]);
    for tick in [1, 80, 120, 120, 50] {
        skipped.step(tick, goal, |_, _, _, _| None, &PATHS);
    }
    assert!(skipped.walking());
    assert_eq!(contiguous.movement(120), skipped.movement(120));

    let mut late = walker([0.0, 0.0, 10.0]);
    let mut bounded = walker([0.0, 0.0, 10.0]);
    late.step(1, goal, |_, _, _, _| None, &PATHS);
    late.step(10_000, goal, |_, _, _, _| None, &PATHS);
    bounded.step(10_000 - CATCH_UP, goal, |_, _, _, _| None, &PATHS);
    bounded.step(10_000, goal, |_, _, _, _| None, &PATHS);
    assert_eq!(late.movement(10_000), bounded.movement(10_000));
}

#[test]
fn terrain_probe_uses_ramp_speed_and_falls_back_to_hint_height() {
    let goal = Some([0.0, 10.0, 20.0]);
    let mut ramp = walker([0.0, 10.0, 20.0]);
    let mut fallback = walker([0.0, 10.0, 20.0]);
    // 157 ticks in: at full speed (from 63 ticks on), not near the point.
    for tick in 1..=157 {
        ramp.step(
            tick,
            goal,
            |_, z, top, bottom| {
                let height = 0.5 * z;
                (bottom..=top).contains(&height).then_some(height)
            },
            &PATHS,
        );
        fallback.step(tick, goal, |_, _, _, _| None, &PATHS);
    }
    let [_, y, z] = ramp.position;
    assert!((y - 0.5 * z).abs() < 1.0e-12);
    let [_, vy, vz] = ramp.velocity;
    assert!((vy.hypot(vz) - 4.5).abs() < 1.0e-8);
    assert!((vz - 4.5 / 1.0_f64.hypot(0.5)).abs() < 1.0e-8);
    assert!((fallback.position[1] - 0.5 * fallback.position[2]).abs() < 1.0e-12);
}

#[test]
fn patrol_uses_the_shared_capsule_and_team_mask() {
    use orpheus_physics::{
        collision::cache::{Mesh, Triangle},
        contact::mesh::ContactMesh,
        contact::world::{StaticGeometry, StaticShape},
    };

    let wall = Mesh {
        vertices: vec![
            [-10.0, 0.0, 2.0],
            [-10.0, 4.0, 2.0],
            [10.0, 0.0, 2.0],
            [10.0, 4.0, 2.0],
        ],
        triangles: vec![
            Triangle {
                corners: [0, 1, 2],
                wings: [-1, 3, -1],
                mask: 0x08,
                surface: 0,
            },
            Triangle {
                corners: [3, 2, 1],
                wings: [-1, 0, -1],
                mask: 0x08,
                surface: 0,
            },
        ],
    };
    let mesh = ContactMesh::new(wall, [1.0; 3], u16::MAX).unwrap();
    let wall = StaticShape::new(StaticGeometry::Mesh(Arc::new(mesh)), None).unwrap();
    let world = Arc::new(ContactWorld::new(vec![wall]).unwrap());
    let settings = MoverSettings::default();
    let mut blocked = walker([0.0, 0.0, 4.0]);
    let mut allowed = walker([0.0, 0.0, 4.0]);
    blocked.attach_collision(&settings, Arc::clone(&world), 0x08);
    allowed.attach_collision(&settings, world, 0x10);
    for tick in 1..=108 {
        for bot in [&mut blocked, &mut allowed] {
            bot.step(tick, Some([0.0, 0.0, 4.0]), |_, _, _, _| Some(0.0), &PATHS);
        }
    }
    assert!(blocked.position[2] < 2.0);
    assert!(allowed.position[2] > 3.0);
    assert!(blocked.velocity[2].abs() < 0.05);
}

#[test]
fn death_stops_the_bot_and_respawn_restarts_the_route_after_create() {
    let goal = Some([0.0, 0.0, 10.0]);
    let mut bot = walker([0.0, 0.0, 10.0]);
    bot.step(1, goal, |_, _, _, _| None, &PATHS);
    bot.step(100, goal, |_, _, _, _| None, &PATHS);
    assert!(bot.walking());
    let death_position = bot.position;
    // A living bot blocks other bodies with its standing capsule; a dead one
    // blocks nobody.
    let settings = MoverSettings::default();
    let standing = orpheus_physics::mover::ground::standing_capsule(&settings, bot.position);
    assert_eq!(bot.capsule(&settings), Some(standing));
    bot.died();
    assert_eq!(bot.capsule(&settings), None);
    assert_eq!(
        bot.step(400, goal, |_, _, _, _| unreachable!(), &PATHS),
        None
    );
    assert!(!bot.walking());
    assert_eq!(bot.position, death_position);
    let state = bot.movement(100);
    assert_eq!(state.flags, DEAD_MOVEMENT_FLAG);
    assert_eq!(state.velocity, [0.0; 3]);
    assert_eq!(state.throttles, [0; 2]);
    assert_eq!(state.input_frame, None);
    assert_eq!(bot.movement(101).input_frame, Some(100));
    // A dead bot's record goes in every frame, as a living one's.
    assert!((101..400).all(|tick| bot.record_due(tick)));

    let replacement = BOT_ENTITY + 15;
    // Respawned after the practice modes' 3 s (`game_respawns`).
    let tick = 100
        + Duration::from_secs(3)
            .as_millis()
            .div_ceil(TICK_PERIOD.as_millis()) as u32;
    let records_from = tick + BOT_RECORD_DELAY;
    let mut fresh = bot.respawned(replacement, records_from);
    assert!(Arc::ptr_eq(&fresh.spawn, &bot.spawn));
    assert_eq!(fresh.entity, replacement);
    assert!(!fresh.dead);
    assert_eq!(fresh.position, [0.0; 3]);
    assert_eq!(fresh.movement(tick).input_frame, Some(NO_INPUT));
    // Its records start once its create had time to arrive, then go in
    // every frame, resting or not (retail; Python's came about once a
    // second, so the client couldn't place the bot until its turn).
    assert!(!fresh.record_due(records_from - 1));
    assert!((records_from..records_from + 200).all(|at| fresh.record_due(at)));
    // A new body stands until its script asks for a point (its patrol's
    // first walk, 3 s on in `0B8E`), then sets off in that tick.
    let goal = Some([0.0, 0.0, 10.0]);
    fresh.step(tick, None, |_, _, _, _| None, &PATHS);
    assert!(!fresh.walking());
    fresh.step(tick + 1, goal, |_, _, _, _| None, &PATHS);
    assert!(fresh.walking());
}

/// An endless floor at y = 0 for bodies of categories 0x08 and 0x10, as the
/// probe's triangles and the engine's mesh.
fn flat_floor() -> (TriangleIndex, Arc<ContactWorld>) {
    use orpheus_physics::{
        collision::{
            cache::{Mesh, Triangle},
            index::SceneTriangle,
        },
        contact::{
            mesh::ContactMesh,
            world::{StaticGeometry, StaticShape},
        },
    };
    const MASK: u16 = 0x18;
    let (low, high) = (-5000.0, 5000.0);
    let corners = [
        [low, 0.0, low],
        [low, 0.0, high],
        [high, 0.0, high],
        [high, 0.0, low],
    ];
    let faces = [[0, 1, 2], [0, 2, 3]];
    let rays = TriangleIndex::new(faces.map(|face| SceneTriangle {
        vertices: face.map(|corner| corners[corner]),
        mask: MASK,
        surface: 0,
    }))
    .unwrap();
    let mesh = Mesh {
        vertices: corners
            .map(|corner| corner.map(|axis| axis as f32))
            .to_vec(),
        triangles: vec![
            Triangle {
                corners: [0, 1, 2],
                wings: [-1, -1, 3],
                mask: MASK,
                surface: 0,
            },
            Triangle {
                corners: [0, 2, 3],
                wings: [1, -1, -1],
                mask: MASK,
                surface: 0,
            },
        ],
    };
    let mesh = ContactMesh::new(mesh, [1.0; 3], u16::MAX).unwrap();
    let shape = StaticShape::new(StaticGeometry::Mesh(Arc::new(mesh)), None).unwrap();
    (rays, Arc::new(ContactWorld::new(vec![shape]).unwrap()))
}

/// A knockback (a Helix Rocket's within its 0.5 m ring: 4 m/s away, 6 m/s
/// up) throws a walking Training Bot on its body's mover: in the air and
/// rising from its next frame, it leaves its walk, its records carry the
/// flight, it comes down after 2 × 6 / (9.81 × 1.75) = 0.7 s (43 ticks;
/// ground friction stops it 3 ticks later), its
/// sample says grounded from then on, and once it stands still it walks on
/// to the point it was walking to, which its script still asks for.
#[test]
fn a_knocked_bot_flies_on_its_mover_and_walks_on_once_it_lands() {
    let (rays, world) = flat_floor();
    let settings = MoverSettings {
        run_forward: 4.5,
        run_backward: 4.5,
        run_strafe: 4.5,
        accel_ground: 1.0,
        ..MoverSettings::default()
    };
    let goal = [0.0, 0.0, 10.0];
    let mut bot = walker(goal);
    bot.attach_collision(&settings, Arc::clone(&world), 0x10);
    let run = |bot: &mut Bot, tick: u32| {
        bot.fly(tick, &rays);
        assert_eq!(
            bot.step(tick, Some(goal), |_, _, _, _| Some(0.0), &PATHS),
            None
        );
    };
    for tick in 1..=63 {
        run(&mut bot, tick);
    }
    assert!(bot.walking());
    let knock = 63;
    let (from, yaw) = (bot.position, bot.movement(knock).yaw);
    bot.knock([0.0, 6.0, 4.0], knock, || {
        Mover::new(settings, Arc::clone(&world), from, yaw, 0x10).ok()
    });
    assert!(!bot.walking());
    assert_eq!(bot.movement_sample(), BOT_MOVEMENT_SAMPLE);
    let mut top: f64 = 0.0;
    let mut down = None;
    let mut still = None;
    for tick in knock + 1..knock + 300 {
        run(&mut bot, tick);
        let sample = bot.movement_sample();
        if tick == knock + 1 {
            // In the air (bit 0), rising (bit 2), moving (bit 4).
            assert_eq!(sample & 0b10101, 0b10101, "{sample:#b}");
        }
        top = top.max(bot.position[1]);
        if down.is_none() && sample & 1 == 0 {
            assert_eq!(sample & 2, 2);
            down = Some(tick);
        }
        if down.is_none() {
            assert!(bot.record_due(tick));
            assert_ne!(bot.movement(tick).flags, 0);
        }
        if bot.flight.is_none() {
            still = Some(tick);
            break;
        }
    }
    let (down, still) = (down.unwrap(), still.unwrap());
    // At most 6² / 2g up, less up to a frame's rise for whole frames.
    let apex = 36.0 / (2.0 * 9.81 * 1.75);
    assert!(top <= apex && top > apex - 6.0 * 0.016, "{top} m up");
    assert_eq!(
        (down - knock, still - knock),
        (43, 46),
        "came down, stood still"
    );
    let landed = bot.position;
    assert_eq!(landed[1], 0.0);
    assert!(landed[2] - from[2] > 2.0, "from {from:?} to {landed:?}");
    assert_eq!(bot.movement(still).flags, 0);
    assert_eq!(bot.movement_sample(), BOT_MOVEMENT_SAMPLE);
    // In the tick it stands still it sets off again for the point its
    // script still asks for.
    assert!(bot.walking());
    assert_eq!(bot.target, Some(goal));
}

/// A velocity action its script began (Pharah's Jump Jet shape,
/// `STU_FF3471E9`: channel 4 sets the velocity to 6 m/s up and 4 m/s
/// along z) puts a walking bot on its body's mover as a knockback does:
/// it rises from the frame after the tick the action began, leaves its
/// walk, comes down and walks on once it stands still.
#[test]
fn a_velocity_action_lifts_a_walking_bot_onto_its_mover() {
    use orpheus_physics::mover::modifiers::{VectorOperation, VelocityElement};

    let (rays, world) = flat_floor();
    let settings = MoverSettings {
        run_forward: 4.5,
        run_backward: 4.5,
        run_strafe: 4.5,
        accel_ground: 1.0,
        ..MoverSettings::default()
    };
    let goal = [0.0, 0.0, 10.0];
    let mut bot = walker(goal);
    bot.attach_collision(&settings, Arc::clone(&world), 0x10);
    let run = |bot: &mut Bot, tick: u32| {
        bot.fly(tick, &rays);
        bot.step(tick, Some(goal), |_, _, _, _| Some(0.0), &PATHS);
    };
    for tick in 1..=63 {
        run(&mut bot, tick);
    }
    let began = 63;
    let (from, yaw) = (bot.position, bot.movement(began).yaw);
    let jet = Motion::Velocity(VelocityElement::Vector {
        channel: 4,
        operation: VectorOperation::Set,
        value: [0.0, 6.0, 4.0],
        priority: 0.0,
    });
    bot.start_motions(vec![jet], began, || {
        Mover::new(settings, Arc::clone(&world), from, yaw, 0x10).ok()
    });
    assert!(!bot.walking());
    run(&mut bot, began + 1);
    assert!(bot.position[1] > 0.0, "{:?}", bot.position);
    assert_eq!(bot.movement_sample() & 0b101, 0b101);
    let mut top: f64 = 0.0;
    for tick in began + 2..began + 300 {
        run(&mut bot, tick);
        top = top.max(bot.position[1]);
        if bot.flight.is_none() {
            break;
        }
    }
    assert!(bot.flight.is_none());
    assert!(top > 0.9, "{top} m up");
    assert_eq!(bot.position[1], 0.0);
    assert!(
        bot.position[2] - from[2] > 2.0,
        "from {from:?} to {:?}",
        bot.position
    );
    assert!(bot.walking());
}

#[test]
#[ignore = "requires the derived 104319 AI spawner and mover tables"]
fn fifteen_practice_bots_match_python_roster() {
    let root = orpheus_testkit::runtime_dir();
    let data = root.join("data");
    let catalog = BotCatalog::load(data.join("ai_spawners_174.json")).unwrap();
    // mover.py `SOLDIER_BODY`, the fallback body.
    let movers = orpheus_physics::mover::settings::MoverCatalog::load(
        data.join("game_movers_174.json"),
        0x0400_0000_0000_03cf,
    )
    .unwrap();
    let bots = catalog.make(PRACTICE_RANGE_MAP, |body| {
        BotWalk::from_mover(movers.for_body(Some(body)))
    });
    assert_eq!(bots.len(), 15);
    assert_eq!(bots.iter().filter(|bot| bot.spawn.team == 1).count(), 11);
    assert_eq!(bots.iter().filter(|bot| bot.spawn.team == 0).count(), 4);
    assert_eq!(
        bots.iter()
            .filter(|bot| !bot.spawn.route.is_empty())
            .count(),
        6
    );
    assert_eq!(
        bots.iter()
            .filter(|bot| bot.spawn.hero == FAR_RUNNER)
            .count(),
        4
    );

    // Python's bots were `0xC0001000 + n`; ours are in id class 0 (module
    // notes), so the digest takes Python's id for each.
    const PYTHON_BOT_ENTITY: u32 = 0xc000_1000;
    let mut roster = Sha256::new();
    for (index, bot) in bots.iter().enumerate() {
        let spawn = &bot.spawn;
        assert_eq!(bot.entity, BOT_ENTITY + index as u32);
        roster.update((bot.entity - BOT_ENTITY + PYTHON_BOT_ENTITY).to_le_bytes());
        roster.update(spawn.hero.to_le_bytes());
        roster.update(spawn.body.to_le_bytes());
        roster.update([spawn.team]);
        roster.update((spawn.health as f32).to_le_bytes());
        roster.update(spawn.yaw.to_le_bytes());
        roster.update([u8::from(spawn.firing)]);
        roster.update(u32::try_from(spawn.name.len()).unwrap().to_le_bytes());
        roster.update(spawn.name.as_bytes());
        for value in
            bot.position
                .into_iter()
                .chain([bot.walk.speed, bot.walk.acceleration, bot.walk.brake])
        {
            roster.update((value as f32).to_le_bytes());
        }
        roster.update(i32::try_from(spawn.initial_index).unwrap().to_le_bytes());
        roster.update(u32::try_from(spawn.route.len()).unwrap().to_le_bytes());
        for &point in &spawn.route {
            for value in point {
                roster.update((value as f32).to_le_bytes());
            }
        }
    }
    // Generated by Python bots.make with no floor data. Python's patrol
    // frames digest went with its timings: the patrol graph `0B8E` decides
    // when the bots walk now (`bot_script` tests).
    assert_eq!(
        format!("{:x}", roster.finalize()),
        "04745e4f99dbbfa08bdef514d77ccc0b8e023e769efa4f5fb58e6f794f50ac4b"
    );
}

/// The Practice Range's patrols walk to every point of their routes, the
/// legs a straight line can't take too (Training Bot `80008809`: both legs
/// of its 26 m route, with something between; `8000880B`: its 20 m leg out;
/// `8000880C`: up to its route's raised point): their walks go around over
/// the map's navigation, each
/// line one the bot's capsule walks. As the match runs them: the patrol
/// graph `0B8E` asks for the points (3 s, then 1 s waits), the walker
/// walks, its end goes back to the script.
#[test]
#[ignore = "requires the derived 104319 data with its bot graph bundle and navigation"]
fn practice_range_patrols_walk_around_what_blocks_the_straight_way() {
    use crate::scene_assets::SceneAssets;
    use orpheus_content::maps::GameMap;
    use orpheus_core::tick::FIRST_TICK;
    use orpheus_script::vm::Unknowns;

    let mut assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    // The stock bot graphs hold content the VM doesn't know yet.
    assets.set_bot_unknowns(Unknowns::Tolerate);
    let mut scene = assets.prepare(GameMap::practice_range()).unwrap();
    // The patrols with a leg the bot's capsule can't walk straight.
    let blocked: Vec<usize> = scene
        .bots()
        .iter()
        .enumerate()
        .filter(|(_, bot)| {
            let walkable = scene.walk_test(Some(bot.spawn.body), bot.spawn.team);
            let route = &bot.spawn.route;
            (0..route.len()).any(|leg| !walkable(route[leg], route[(leg + 1) % route.len()]))
        })
        .map(|(slot, _)| slot)
        .collect();
    let entities: Vec<u32> = blocked
        .iter()
        .map(|&slot| scene.bots()[slot].entity)
        .collect();
    assert_eq!(entities, [BOT_ENTITY + 9, BOT_ENTITY + 11, BOT_ENTITY + 12]);
    let mut reached: Vec<Vec<bool>> = blocked
        .iter()
        .map(|&slot| vec![false; scene.bots()[slot].spawn.route.len()])
        .collect();
    // 8000880C's route of three: 3 s, the 10 m and 13 m legs at 6 m/s with
    // 1 s between; 40 s leaves room for the turns.
    for tick in FIRST_TICK..FIRST_TICK + 40 * 1000 / TICK_PERIOD.as_millis() as u32 {
        scene.step_bots(tick);
        scene.advance_live_bots(tick);
        for (index, &slot) in blocked.iter().enumerate() {
            let bot = &scene.bots()[slot];
            assert!(!bot.dead);
            for (point, done) in bot.spawn.route.iter().zip(&mut reached[index]) {
                if !bot.walking() && planar_distance(bot.position, *point) <= ARRIVED {
                    *done = true;
                }
            }
        }
    }
    for (index, slot) in blocked.into_iter().enumerate() {
        let bot = &scene.bots()[slot];
        assert!(
            reached[index].iter().all(|&done| done),
            "{:08X} reached {:?} of {:?}, at {:?}",
            bot.entity,
            reached[index],
            bot.spawn.route,
            bot.position
        );
    }
}
