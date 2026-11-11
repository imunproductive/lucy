//! A player's body moving with its hero's values and its statescript's
//! movement mods, ported from `tests/test_game_mover.py` (`SprintTests`,
//! `MatchMoverTests`; `ow174/game/mover.py`, `ow174/game/match.py`).

use orpheus_content::heroes::SOLDIER;
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
use orpheus_wire::commands::{CROUCH, Command};

use super::*;
use crate::{instance::RETAIL_QUANTA, replication::BodyStep, scene::team_bit};

/// Ability 1: Soldier: 76's Sprint.
const SHIFT: u32 = 0x08;
const TRACER: u64 = 0x02e0_0000_0000_0003;
const TICK: f64 = 0.016;

/// An endless floor at y = 1: the stand-in world of a mover without a map
/// (`flat_world` in `ow174/game/collision.py`), as the probe's triangles and
/// the engine's mesh.
fn flat_floor() -> (TriangleIndex, Arc<ContactWorld>) {
    let (low, high, y) = (-5000.0, 5000.0, 1.0);
    let corners = [
        [low, y, low],
        [low, y, high],
        [high, y, high],
        [high, y, low],
    ];
    let faces = [[0, 1, 2], [0, 2, 3]];
    let rays = TriangleIndex::new(faces.map(|face| SceneTriangle {
        vertices: face.map(|corner| corners[corner]),
        mask: MOVER_CATEGORY,
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
                mask: MOVER_CATEGORY,
                surface: 0,
            },
            Triangle {
                corners: [0, 2, 3],
                wings: [1, -1, -1],
                mask: MOVER_CATEGORY,
                surface: 0,
            },
        ],
    };
    let mesh = ContactMesh::new(mesh, [1.0; 3], u16::MAX).unwrap();
    let shape = StaticShape::new(StaticGeometry::Mesh(Arc::new(mesh)), None).unwrap();
    (rays, Arc::new(ContactWorld::new(vec![shape]).unwrap()))
}

/// Soldier: 76 on a flat floor with the stock owner script
/// (`SprintTests.setUp`: `BodyScript(SOLDIER, 0xA0000101, 999)` and 30 still
/// frames).
struct Sprinter {
    mover: Mover,
    rays: TriangleIndex,
    runtime: Box<dyn OwnerBodyRuntime>,
}

impl Sprinter {
    fn new() -> Self {
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let hero = scene.heroes().get(SOLDIER).unwrap();
        let (rays, contacts) = flat_floor();
        let settings = *scene.movers.for_body(Some(hero.body));
        let mover = Mover::new(settings, contacts, [0.0, 1.0, 0.0], 0, MOVER_CATEGORY).unwrap();
        let spawn = OwnerBodySpawn {
            slot: 0,
            controller: 0xa000_0100,
            entity: 0xa000_0101,
            body_filter: team_bit(0).unwrap(),
            frame: 999,
            pose: mover.pose(),
            quanta: RETAIL_QUANTA,
        };
        let runtime = scene.owner_body_runtime(hero, spawn).unwrap().unwrap();
        let mut sprinter = Self {
            mover,
            rays,
            runtime,
        };
        for frame in 1000..1030 {
            sprinter.frame(frame, 0, 0);
        }
        sprinter
    }

    /// One command frame as the server runs it: the mover, then the
    /// statescript, whose movement mods the mover follows from the next
    /// frame; the horizontal speed.
    fn frame(&mut self, frame: u32, buttons: u32, forward: i8) -> f64 {
        let command = Command {
            frame,
            forward,
            buttons,
            ..Command::default()
        };
        self.mover
            .step(&self.rays, command, TICK, |_, _| {})
            .unwrap();
        self.runtime
            .advance(command, &self.mover, BodyStep::Posed)
            .unwrap();
        let mods = self.runtime.movement_modifiers().unwrap();
        self.mover.track_modifiers(frame, mods);
        let [vx, _, vz] = self.mover.pose().velocity;
        vx.hypot(vz)
    }
}

/// `tests/test_game_mover.py::SprintTests::test_sprint_runs_at_one_and_a_half_times_the_run_speed_from_the_next_frame`
#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn sprint_runs_at_one_and_a_half_times_the_run_speed_from_the_next_frame() {
    let mut body = Sprinter::new();
    let speeds: Vec<f64> = (1030..1070)
        .map(|frame| {
            let buttons = if matches!(frame, 1050 | 1060) {
                SHIFT
            } else {
                0
            };
            body.frame(frame, buttons, 127)
        })
        .collect();
    // The client's numbers: run 5.5 * 1.5 = 8.25 and acceleration 35 * 8.25
    // m/s^2 against friction 15 * 8.25 (its base speed 6.2 * 1.5); after it,
    // friction 15 * 5.5 + (8.25 - 6.2) * 2 * 5.5.
    assert_eq!(speeds[19..22], [5.5, 5.5, 8.1396484375]); // frames 1049-1051: on in 1050
    assert_eq!(speeds[22..31], [8.25; 9]);
    assert_eq!(speeds[30..33], [8.25, 6.5693359375, 5.5]); // off in 1060
    // A HUD mod adds -0.
    let mods = body.runtime.movement_modifiers().unwrap();
    assert!(
        mods.values().all(|modifier| modifier.value == 0.0),
        "{mods:?}"
    );
}

/// `tests/test_game_mover.py::SprintTests::test_crouching_waits_for_the_frame_after_sprint_ends`
#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn crouching_waits_for_the_frame_after_sprint_ends() {
    let mut body = Sprinter::new();
    for frame in 1030..1050 {
        body.frame(frame, if frame == 1040 { SHIFT } else { 0 }, 127);
    }
    // The crouch ends Sprint in this frame, but its mods act in it.
    body.frame(1050, CROUCH, 127);
    assert!(!body.mover.crouched());
    body.frame(1051, CROUCH, 127);
    assert!(body.mover.crouched());
}

/// `tests/test_game_mover.py::MatchMoverTests::test_a_players_body_walks_on_the_maps_collision_with_its_heros_values`
///
/// The player's body is the scene's spawn mover of the hero's body, as the
/// match makes it at the spawn and again for a hero switch.
#[test]
#[ignore = "requires a derived 104319 Practice Range pack without bot VM graphs"]
fn a_players_body_walks_on_the_maps_collision_with_its_heros_values() {
    let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
    let scene = assets.prepare(GameMap::practice_range()).unwrap();
    assert!(scene.movement_ready());
    let walk = |hero: u64, first: u32| {
        let body = scene.heroes().get(hero).unwrap().body;
        let mut mover = scene
            .player_spawn_mover(body, 0, 0, SpawnChoice::First)
            .unwrap();
        let (spawn, yaw) = (mover.pose().position, mover.pose().yaw);
        for frame in first..first + 60 {
            let command = Command {
                frame,
                forward: 127,
                yaw,
                ..Command::default()
            };
            mover
                .step(scene.movement_index(), command, TICK, |_, _| {})
                .unwrap();
        }
        (scene.movers.for_body(Some(body)).run_forward, spawn, mover)
    };
    let (run, spawn, soldier) = walk(SOLDIER, 7000);
    let position = soldier.pose().position;
    assert_eq!((position[1], run), (1.0, 5.5));
    let away = (0..3).map(|axis| (position[axis] - spawn[axis]).powi(2));
    assert!(away.sum::<f64>().sqrt() > 4.0, "{position:?}");
    // Tracer: a new body at the spawn.
    let (run, _, tracer) = walk(TRACER, 7060);
    assert_eq!(run, 6.0);
    let [vx, vy, vz] = tracer.pose().velocity;
    assert_eq!(vy, 0.0);
    assert!((vx.hypot(vz) - 6.0).abs() <= 0.01, "{}", vx.hypot(vz));
}
