use super::*;
use orpheus_content::maps::PRACTICE_RANGE_MAP;
use orpheus_core::tick::SEND_EVERY;
use orpheus_wire::entity::movement_record;
use sha2::{Digest, Sha256};

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

fn route_vector(bots: &mut [Bot], ticks: impl IntoIterator<Item = u32>) -> String {
    let mut digest = Sha256::new();
    for tick in ticks {
        for bot in &mut *bots {
            bot.step(tick, |_, _, _, _| None);
            let bits = movement_record(&bot.movement(tick), Some(tick)).unwrap();
            digest.update(tick.to_le_bytes());
            digest.update(u32::try_from(bits.bit_len()).unwrap().to_le_bytes());
            digest.update([u8::from(bot.record_due(tick, SEND_EVERY))]);
            digest.update(bits.as_bytes());
        }
    }
    format!("{:x}", digest.finalize())
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
    assert_eq!(bots[0].index, 2);
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

#[test]
fn backward_walk_brake_and_next_leg_match_python_wire_vectors() {
    // Python tests/test_bots.py::behind(), captured at every 16 ms tick.
    let mut bot = walker([0.0, 0.0, -20.0]);
    assert_eq!(
        route_vector(std::slice::from_mut(&mut bot), 1..=750),
        "80053f3ad91f5ca2e6e9fb638a2cb5fd1ac83851602e4079a6385c9ce39a7ee7"
    );
}

#[test]
fn skipped_ticks_catch_up_without_replaying_a_duplicate_tick() {
    let mut contiguous = walker([0.0, 0.0, 10.0]);
    for tick in 1..=251 {
        contiguous.step(tick, |_, _, _, _| None);
    }
    let mut skipped = walker([0.0, 0.0, 10.0]);
    for tick in [1, 201, 251, 251, 100] {
        skipped.step(tick, |_, _, _, _| None);
    }
    assert_eq!(contiguous.movement(251), skipped.movement(251));

    let mut late = walker([0.0, 0.0, 10.0]);
    let mut bounded = walker([0.0, 0.0, 10.0]);
    late.step(1, |_, _, _, _| None);
    late.step(10_000, |_, _, _, _| None);
    bounded.step(10_000 - CATCH_UP, |_, _, _, _| None);
    bounded.step(10_000, |_, _, _, _| None);
    assert_eq!(late.movement(10_000), bounded.movement(10_000));
}

#[test]
fn terrain_probe_uses_ramp_speed_and_falls_back_to_hint_height() {
    let mut ramp = walker([0.0, 10.0, 20.0]);
    let mut fallback = walker([0.0, 10.0, 20.0]);
    for tick in 1..=344 {
        ramp.step(tick, |_, z, top, bottom| {
            let height = 0.5 * z;
            (bottom..=top).contains(&height).then_some(height)
        });
        fallback.step(tick, |_, _, _, _| None);
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
    blocked.attach_collision(&settings, Arc::clone(&world), 0x08, |_, _, _, _| Some(0.0));
    allowed.attach_collision(&settings, world, 0x10, |_, _, _, _| Some(0.0));
    for tick in 1..=295 {
        for bot in [&mut blocked, &mut allowed] {
            bot.step(tick, |_, _, _, _| Some(0.0));
        }
    }
    assert!(blocked.position[2] < 2.0);
    assert!(allowed.position[2] > 3.0);
    assert!(blocked.velocity[2].abs() < 0.05);
}

#[test]
fn death_stops_the_bot_and_respawn_restarts_the_route_after_create() {
    let mut bot = walker([0.0, 0.0, 10.0]);
    bot.step(1, |_, _, _, _| None);
    bot.step(250, |_, _, _, _| None);
    assert!(bot.walking());
    let death_position = bot.position;
    bot.died(250);
    bot.step(400, |_, _, _, _| unreachable!());
    assert!(!bot.walking());
    assert_eq!(bot.position, death_position);
    let state = bot.movement(250);
    assert_eq!(state.flags, DEAD_MOVEMENT_FLAG);
    assert_eq!(state.velocity, [0.0; 3]);
    assert_eq!(state.throttles, [0; 2]);
    assert_eq!(state.input_frame, None);
    assert_eq!(bot.movement(251).input_frame, Some(250));
    assert!(bot.record_due(250 + SETTLE_TICKS - 1, SEND_EVERY));

    let replacement = BOT_ENTITY + 15;
    let tick = 250
        + BOT_RESPAWN_TIME
            .as_millis()
            .div_ceil(TICK_PERIOD.as_millis()) as u32;
    let records_from = tick + BOT_RECORD_DELAY;
    let mut fresh = bot.respawned(replacement, records_from);
    assert!(Arc::ptr_eq(&fresh.spawn, &bot.spawn));
    assert_eq!(fresh.entity, replacement);
    assert!(!fresh.dead);
    assert_eq!(fresh.position, [0.0; 3]);
    assert_eq!(fresh.movement(tick).input_frame, Some(NO_INPUT));
    let turn = |at: &u32| at % SETTLE_TICKS == fresh.entity % SETTLE_TICKS;
    let before = (records_from - SETTLE_TICKS..records_from)
        .find(turn)
        .unwrap();
    let after = (records_from..records_from + SETTLE_TICKS)
        .find(turn)
        .unwrap();
    assert!(!fresh.record_due(before, 1));
    assert!(fresh.record_due(after, 1));
    fresh.step(tick, |_, _, _, _| None);
    fresh.step(tick + 186, |_, _, _, _| None);
    assert!(!fresh.walking());
    fresh.step(tick + 187, |_, _, _, _| None);
    assert!(fresh.walking());
}

#[test]
#[ignore = "requires the derived 104319 AI spawner and mover tables"]
fn fifteen_practice_bots_match_python_roster_and_patrol_frames() {
    let root = orpheus_testkit::runtime_dir();
    let data = root.join("data");
    let catalog = BotCatalog::load(data.join("ai_spawners_174.json")).unwrap();
    // mover.py `SOLDIER_BODY`, the fallback body.
    let movers = orpheus_physics::mover::settings::MoverCatalog::load(
        data.join("game_movers_174.json"),
        0x0400_0000_0000_03cf,
    )
    .unwrap();
    let mut bots = catalog.make(PRACTICE_RANGE_MAP, |body| {
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

    let mut roster = Sha256::new();
    for (index, bot) in bots.iter().enumerate() {
        let spawn = &bot.spawn;
        assert_eq!(bot.entity, BOT_ENTITY + index as u32);
        roster.update(bot.entity.to_le_bytes());
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
    // Generated by Python bots.make and world.movement_record with no floor data.
    assert_eq!(
        format!("{:x}", roster.finalize()),
        "04745e4f99dbbfa08bdef514d77ccc0b8e023e769efa4f5fb58e6f794f50ac4b"
    );
    assert_eq!(
        route_vector(&mut bots, (1..=1300).chain([1325, 1955, 1956, 1960])),
        "b149457ffd1e86f976def1ffb6d53c7a955582cced2fbcc03fd424c1a0a893e9"
    );
}
