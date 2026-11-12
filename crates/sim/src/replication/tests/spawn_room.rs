//! The Practice Range's spawn room and its fall out of the map
//! (`crate::map_areas`): invulnerable and healed inside, hero change only
//! inside and where the hero stands, the client's spawn room flag, and the
//! environment's kill; and Watchpoint: Gibraltar's kill volumes.

use super::*;
use crate::combat::{Combat, CombatPlayer, ENVIRONMENT_DAMAGE, Shooter, apply_player_hits};
use crate::scene_assets::install_collision_build;
use orpheus_content::{maps::SpawnChoice, stats::PlayerStats};
use orpheus_wire::combat_messages::CombatNotice;

const OWNER: u32 = 0xa000_0100;

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

/// A started world whose player has a Soldier: 76 at his spawn point.
fn spawned(scene: &PreparedScene, viewer: &PlayerAppearance<'_>) -> WorldDelivery {
    let mut world = WorldDelivery::new(OWNER);
    world.start(scene, viewer, FIRST_TICK).unwrap();
    world.created.insert(OWNER);
    world.present(scene, FIRST_TICK).unwrap();
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

/// At the spawn point the body is in its room: invulnerable, healed 400 a
/// second, free to change heroes. Out on the range it takes damage, the
/// client's flag goes off (once), and a pick is refused.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn the_spawn_room_shields_heals_and_gates_hero_change() {
    let scene = scene();
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let body = world.current_body().unwrap();
    world.area_step(&scene, &viewer);
    assert_eq!(world.in_spawn_room(), Some(true));
    assert!(world.may_change_hero());
    assert!(flags(&world).is_empty(), "the create sent the flag set");
    assert_eq!(
        world.damage_body(&scene, &hit(body, 50.0), FIRST_TICK, Duration::ZERO),
        None
    );

    place(&mut world, &scene, outside(&scene), 0);
    world.area_step(&scene, &viewer);
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

    // Back in the room: the flag comes back on and one tick heals 6.4.
    let (spawn, _) = scene
        .map()
        .spawn_point_in(0, 0, SpawnChoice::First)
        .unwrap();
    place(&mut world, &scene, spawn, 0);
    world.area_step(&scene, &viewer);
    // The newer flag supersedes the unsent one: the client gets it alone.
    assert_eq!(flags(&world), [true]);
    let healed = world.health_share().unwrap();
    assert!(healed > hurt);
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

/// A fall onto the Range's pit floor (-11 m) kills: the body is its own
/// killer ("YOU DIED", a death on the board) and it comes back at the spawn
/// point after the Range's 3 s, not a moment before.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn a_fall_onto_the_catch_floor_kills_and_respawns_after_the_delay() {
    let scene = scene();
    assert_eq!(scene.kill_height(), Some(-11.0));
    assert!(!scene.fell_out([95.0, -5.25, 10.0]));
    let card = PlayerCard::default();
    let viewer = viewer(&card);
    let mut world = spawned(&scene, &viewer);
    let body = world.current_body().unwrap();
    // Feet on the pit floor, where a fall out of the range ends.
    place(&mut world, &scene, [95.0, -11.0, 10.0], 0);
    let position = world.body_position().unwrap();
    assert!(scene.fell_out(position));

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
    let due = now + scene.respawn_time();
    assert_eq!(scene.respawn_time(), Duration::from_secs(3));
    let tick = FIRST_TICK + 30;
    assert!(
        !world
            .respawn_due(
                &scene,
                &viewer,
                tick,
                due - Duration::from_millis(1),
                RETAIL_QUANTA
            )
            .unwrap()
    );
    assert!(
        world
            .respawn_due(&scene, &viewer, tick, due, RETAIL_QUANTA)
            .unwrap()
    );
    let (spawn, _) = scene
        .map()
        .spawn_point_in(0, 0, SpawnChoice::First)
        .unwrap();
    let back = world.body_position().unwrap();
    assert!((back[0] - spawn[0]).abs() < 0.01 && (back[2] - spawn[2]).abs() < 0.01);
}

/// Watchpoint: Gibraltar has no damage areas and no catch floor: a body that
/// falls off it dies in one of its kill volumes, 4 m slabs a few metres below
/// its lowest floor, not 50 m lower under the map's lowest triangle.
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
    // The lowest floor the AI walks on (-10.84 m) is safe.
    assert!(!scene.fell_out([33.375, -10.85, -104.625]));
    // The slab under the drop by the first checkpoint: top -11.72 m.
    assert!(!scene.fell_out([100.7, -11.6, 58.4]));
    assert!(scene.fell_out([100.7, -11.8, 58.4]));
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
    assert!(
        world
            .respawn_due(
                &scene,
                &viewer,
                FIRST_TICK + 300,
                scene.respawn_time(),
                RETAIL_QUANTA
            )
            .unwrap()
    );
    assert_eq!(world.hero(), Some(tracer));
}

/// The spawn room's three doorways get their barriers: the six
/// server-owned `00F6` placeables (two in each doorway: the frame model and
/// the glowing effect) are created from the map after the player and the
/// mode, before the bots.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack"]
fn the_spawn_doorways_get_their_barriers() {
    let scene = scene();
    let card = PlayerCard::default();
    let creates: Vec<u32> = scene
        .initial_world(&viewer(&card))
        .unwrap()
        .iter()
        .filter(|update| matches!(update.operation, Some(Operation::Placeable(_))))
        .map(|update| update.entity & 0x7FFF_FFFF)
        .collect();
    assert_eq!(
        creates,
        [0x88, 0x22e, 0x234, 0x289, 204, 340, 398, 400, 424, 548]
    );
}
