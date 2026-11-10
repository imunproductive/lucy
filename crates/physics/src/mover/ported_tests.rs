//! The character mover on stand-in box worlds and on the Practice Range's
//! collision, ported from `tests/test_game_mover.py` (the frozen Python
//! specification of `ow174/game/mover.py`). Soldier: 76's values are the
//! built-in [`MoverSettings::default`] ones: the derived table has the same
//! (`soldiers_derived_mover_values_are_the_built_in_ones`).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::File,
    sync::Arc,
};

use orpheus_wire::commands::{CROUCH, Command, JUMP};

use super::{
    Mover, STEP,
    posture::{Posture, snap},
    settings::{MoverCatalog, MoverSettings},
};

/// Soldier: 76's body, the server's fallback (mover.py `SOLDIER_BODY`).
const SOLDIER_BODY: u64 = 0x0400_0000_0000_03cf;
use crate::{
    collision::{
        cache::{HalfEdge, Hull, Mesh, Triangle, read_movement_cache},
        index::{GroundHit, SceneTriangle, TriangleIndex},
        scene::CollisionScene,
    },
    contact::{
        body::CharacterBody,
        cache::CapsulePose,
        mesh::ContactMesh,
        world::{ContactWorld, StaticGeometry, StaticShape},
    },
    mover::modifiers::{
        ActiveModifiers, CROUCH_INTENT, Modifier, ModifierKey, ModifierTimeline, Operation,
    },
};

const TICK: f64 = 0.016;
/// The team proxy categories (`MOVER` in `ow174/game/collision.py`).
const MOVER: u16 = 0x18;
/// Surface bit: the probe does not stand on it (`ow174/game/collision.py`).
const NOT_GROUND: u16 = 0x1;
/// The movement state's flags word (`ow174/game/mover.py`).
const JUMP_LATCH: u32 = 0x1;
const CROUCHED: u32 = 0x2;
const JUMPED: u32 = 0x4;
const FELL: u32 = 0x8;
const REAL_JUMP: u32 = 0x400;
const TOUCHING: u32 = 0x20000;
const TURN_HELD: u32 = 0x200000;
const IN_AIR: u32 = 0x80000C;
/// The speed scalar's mod index (`SPEED` in `ow174/game/mover.py`).
const SPEED: u8 = 0;
/// A wall at z = 5: the engine keeps the capsule 0.5 + 5 mm out, on the
/// 1/1024 grid.
const REST: f64 = 4.5 - 5.0 * STEP;

type Corners = [f64; 9];

/// The 12 triangles of an axis-aligned box.
fn cuboid(x: [f64; 2], y: [f64; 2], z: [f64; 2]) -> Vec<Corners> {
    let corners: Vec<[f64; 3]> = x
        .into_iter()
        .flat_map(|x| y.into_iter().flat_map(move |y| z.map(|z| [x, y, z])))
        .collect();
    let faces = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let joined = |a: usize, b: usize, c: usize| {
        let mut out = [0.0; 9];
        for (k, corner) in [a, b, c].into_iter().enumerate() {
            out[3 * k..3 * k + 3].copy_from_slice(&corners[corner]);
        }
        out
    };
    faces
        .into_iter()
        .flat_map(|[a, b, c, d]| [joined(a, b, c), joined(a, c, d)])
        .collect()
}

/// A slope rising along +z from height `y0` at `z0`, facing up, 40 m wide.
fn ramp(z0: f64, z1: f64, y0: f64, degrees: f64) -> Vec<Corners> {
    let (x0, x1) = (-20.0, 20.0);
    let y1 = y0 + (z1 - z0) * degrees.to_radians().tan();
    vec![
        [x0, y0, z0, x1, y1, z1, x1, y0, z0],
        [x0, y0, z0, x0, y1, z1, x1, y1, z1],
    ]
}

/// A thin vertical wall from (x0, z0) to (x1, z1), 6 m high: two quads back
/// to back, so either side stops.
fn wall([x0, z0]: [f64; 2], [x1, z1]: [f64; 2]) -> Vec<Corners> {
    let (y0, y1) = (0.0, 6.0);
    let (a, b, c, d) = ([x0, y0, z0], [x1, y0, z1], [x1, y1, z1], [x0, y1, z0]);
    [[a, b, c], [a, c, d], [a, c, b], [a, d, c]]
        .into_iter()
        .map(|[p, q, r]| [p[0], p[1], p[2], q[0], q[1], q[2], r[0], r[1], r[2]])
        .collect()
}

/// A floor box with its top at y = 1.
fn floor() -> Vec<Corners> {
    cuboid([-30.0, 30.0], [0.0, 1.0], [-30.0, 30.0])
}

fn joined(parts: impl IntoIterator<Item = Vec<Corners>>) -> Vec<Corners> {
    parts.into_iter().flatten().collect()
}

/// The movement world of loose triangles (`World(triangles, surfaces=...)` in
/// `ow174/game/collision.py`): the probe's triangle index and one engine mesh
/// with shared corners merged and each edge's wing the third corner of the
/// other triangle on it (`soup_mesh`).
fn world(triangles: &[Corners], surfaces: &[u16]) -> (TriangleIndex, Arc<ContactWorld>) {
    let surface = |number: usize| surfaces.get(number).copied().unwrap_or(0);
    let rays = TriangleIndex::new(
        triangles
            .iter()
            .enumerate()
            .map(|(number, t)| SceneTriangle {
                vertices: [[t[0], t[1], t[2]], [t[3], t[4], t[5]], [t[6], t[7], t[8]]],
                mask: MOVER,
                surface: surface(number),
            }),
    )
    .unwrap();
    let mut numbers = HashMap::new();
    let mut vertices = Vec::new();
    let corners: Vec<[i32; 3]> = triangles
        .iter()
        .map(|t| {
            std::array::from_fn(|k| {
                // float32 lanes; -0.0 and 0.0 are one corner, as Python's tuple keys.
                let point = [t[3 * k] as f32, t[3 * k + 1] as f32, t[3 * k + 2] as f32]
                    .map(|axis| axis + 0.0);
                *numbers.entry(point.map(f32::to_bits)).or_insert_with(|| {
                    vertices.push(point);
                    vertices.len() as i32 - 1
                })
            })
        })
        .collect();
    let edge = |u: i32, w: i32| (u.min(w), u.max(w));
    let mut edges: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (number, &[a, b, c]) in corners.iter().enumerate() {
        for (u, w) in [(a, b), (b, c), (c, a)] {
            edges.entry(edge(u, w)).or_default().push(number);
        }
    }
    let records = corners
        .iter()
        .enumerate()
        .map(|(number, &[a, b, c])| Triangle {
            corners: [a, b, c],
            wings: [(a, b), (b, c), (c, a)].map(|(u, w)| {
                edges[&edge(u, w)]
                    .iter()
                    .find(|&&other| other != number)
                    .and_then(|&other| corners[other].into_iter().find(|&x| x != u && x != w))
                    .unwrap_or(-1)
            }),
            mask: MOVER,
            surface: surface(number),
        })
        .collect();
    let mesh = ContactMesh::new(
        Mesh {
            vertices,
            triangles: records,
        },
        [1.0; 3],
        u16::MAX,
    )
    .unwrap();
    let shapes = vec![StaticShape::new(StaticGeometry::Mesh(Arc::new(mesh)), None).unwrap()];
    (rays, Arc::new(ContactWorld::new(shapes).unwrap()))
}

/// An endless floor at that height: the stand-in world when no map is
/// given (`flat_world` in `ow174/game/collision.py`).
fn flat(height: f64) -> Vec<Corners> {
    let (low, high) = (-5000.0, 5000.0);
    vec![
        [low, height, low, low, height, high, high, height, high],
        [low, height, low, high, height, high, high, height, low],
    ]
}

/// The probe's triangles: a stand-in world's own, or a map's scene.
enum Rays {
    Own(TriangleIndex),
    Map(Arc<CollisionScene>),
}

impl Rays {
    fn index(&self) -> &TriangleIndex {
        match self {
            Self::Own(index) => index,
            Self::Map(scene) => scene.triangles(),
        }
    }
}

/// Soldier: 76's mover with the probe's triangles beside it.
struct Body {
    mover: Mover,
    rays: Rays,
}

impl Body {
    /// `Mover(position, yaw, World(triangles, surfaces=surfaces),
    /// mover_data(SOLDIER_BODY))`; without triangles the flat world at the
    /// start height.
    fn new(
        position: [f64; 3],
        triangles: Option<Vec<Corners>>,
        surfaces: &[u16],
        yaw: i32,
    ) -> Self {
        let triangles = triangles.unwrap_or_else(|| flat(snap(position[1])));
        let (rays, contacts) = world(&triangles, surfaces);
        let mover = Mover::new(MoverSettings::default(), contacts, position, yaw, MOVER).unwrap();
        Self {
            mover,
            rays: Rays::Own(rays),
        }
    }

    fn soldier(position: [f64; 3], triangles: Option<Vec<Corners>>) -> Self {
        Self::new(position, triangles, &[], 0)
    }

    fn step(&mut self, command: Command) {
        self.mover
            .step(self.rays.index(), command, TICK, |_, _| {})
            .unwrap();
    }

    /// Step `ticks` command frames from `keys.frame` on with the same keys;
    /// the feet after each.
    fn walk(&mut self, ticks: u32, keys: Command) -> Vec<[f64; 3]> {
        (keys.frame..keys.frame + ticks)
            .map(|frame| {
                self.step(Command { frame, ..keys });
                self.position()
            })
            .collect()
    }

    fn position(&self) -> [f64; 3] {
        self.mover.pose().position
    }

    fn velocity(&self) -> [f64; 3] {
        self.mover.pose().velocity
    }

    fn flags(&self) -> u32 {
        self.mover.pose().flags
    }

    fn airborne(&self) -> bool {
        self.flags() & IN_AIR != 0
    }

    fn crouched(&self) -> bool {
        self.mover.crouched()
    }
}

/// Forward at full throttle from `frame` (the `walk` helper's default keys).
fn run(frame: u32) -> Command {
    Command {
        frame,
        forward: 127,
        ..Command::default()
    }
}

/// No throttle from `frame`.
fn stand(frame: u32) -> Command {
    Command {
        frame,
        ..Command::default()
    }
}

fn steps(delta: f64) -> i64 {
    (delta / STEP).round() as i64
}

fn close(value: f64, expected: f64, delta: f64) -> bool {
    (value - expected).abs() <= delta
}

/// The distance from a point to a segment, in the plane.
fn segment_distance([px, pz]: [f64; 2], [ax, az]: [f64; 2], [bx, bz]: [f64; 2]) -> f64 {
    let (ex, ez) = (bx - ax, bz - az);
    let t = (((px - ax) * ex + (pz - az) * ez) / (ex * ex + ez * ez)).clamp(0.0, 1.0);
    (px - ax - t * ex).hypot(pz - az - t * ez)
}

/// `tests/test_game_mover.py::FlatFloorTests::test_running_is_90_steps_of_the_grid_per_tick`
#[test]
fn running_is_90_steps_of_the_grid_per_tick() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    let path = body.walk(40, run(0));
    assert_eq!(body.velocity(), [0.0, 0.0, 5.5]);
    let per_tick: Vec<_> = path[10..]
        .windows(2)
        .map(|pair| steps(pair[1][2] - pair[0][2]))
        .collect();
    assert_eq!(per_tick[..5], [90; 5]);
    assert!(path.iter().all(|feet| feet[1] == 1.0));
}

/// `tests/test_game_mover.py::FlatFloorTests::test_stopping_takes_five_ticks`
#[test]
fn stopping_takes_five_ticks() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(40, run(0));
    let z = body.position()[2];
    let path = body.walk(5, stand(40));
    assert_eq!(body.velocity(), [0.0; 3]);
    assert!(close(path[4][2] - z, 0.14, 0.005), "{}", path[4][2] - z);
}

/// `tests/test_game_mover.py::FlatFloorTests::test_the_diagonal_is_capped_and_crouching_is_slower`
#[test]
fn the_diagonal_is_capped_and_crouching_is_slower() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(
        60,
        Command {
            right: 127,
            ..run(0)
        },
    );
    let [vx, _, vz] = body.velocity();
    assert!(close(vx.hypot(vz), 5.5, 0.002), "{}", vx.hypot(vz));
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(
        60,
        Command {
            buttons: CROUCH,
            ..run(0)
        },
    );
    assert_eq!((body.flags(), body.velocity()), (CROUCHED, [0.0, 0.0, 3.0]));
}

/// `tests/test_game_mover.py::FlatFloorTests::test_crouching_again_waits_015_s_after_standing_up`
#[test]
fn crouching_again_waits_0_15_s_after_standing_up() {
    let crouch = |frame| Command {
        buttons: CROUCH,
        ..stand(frame)
    };
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(5, crouch(0));
    body.walk(1, stand(5)); // frame 5: stands up
    assert!(!body.crouched());
    body.walk(9, crouch(6)); // frames 6-14: less than 0.15 s later
    assert!(!body.crouched());
    body.walk(1, crouch(15)); // 10 frames, 0.16 s
    assert!(body.crouched());
}

/// `tests/test_game_mover.py::FlatFloorTests::test_missing_command_frames_run_with_the_last_command`
#[test]
fn missing_command_frames_run_with_the_last_command() {
    let mut steady = Body::soldier([0.0, 1.0, 0.0], None);
    let mut gappy = Body::soldier([0.0, 1.0, 0.0], None);
    steady.walk(40, run(0));
    gappy.walk(30, run(0));
    gappy.step(run(39)); // frames 30-38 never came
    assert_eq!(
        (gappy.position(), gappy.velocity()),
        (steady.position(), steady.velocity())
    );
    let z = gappy.position()[2];
    gappy.step(run(1000)); // a restart, not a gap: one tick only
    assert_eq!(steps(gappy.position()[2] - z), 90);
}

/// `tests/test_game_mover.py::FlatFloorTests::test_a_jump_reaches_098_m_and_lands_after_43_ticks`
#[test]
fn a_jump_reaches_0_98_m_and_lands_after_43_ticks() {
    let jump = |frame| Command {
        buttons: JUMP,
        ..stand(frame)
    };
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(1, jump(0));
    assert_eq!(body.flags(), JUMP_LATCH | JUMPED | REAL_JUMP);
    let (mut top, mut ticks) = (body.position()[1], 1);
    while body.airborne() && ticks < 100 {
        body.walk(1, jump(ticks));
        top = top.max(body.position()[1]);
        ticks += 1;
    }
    assert!(close(top - 1.0, 0.98, 0.005), "{}", top - 1.0);
    assert_eq!(ticks, 43);
    assert_eq!((body.position()[1], body.mover.pose().air_ticks), (1.0, 0));
    body.walk(5, jump(ticks)); // held: no hop
    assert!(!body.airborne());
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_wall_stops_the_body_5_mm_out`
#[test]
fn a_wall_stops_the_body_5_mm_out() {
    let world = joined([floor(), cuboid([-30.0, 30.0], [1.0, 6.0], [5.0, 6.0])]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    let path = body.walk(120, run(0));
    // 4.495 on the grid: the radius and the 5 mm contact skin.
    assert_eq!(path[119][2], REST);
    assert_eq!(body.velocity(), [0.0; 3]);
    assert_ne!(body.flags() & TOUCHING, 0);
    assert!(path.iter().all(|feet| feet[1] == 1.0));
    assert!(path.iter().all(|feet| feet[2] <= REST));
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_walking_into_a_wall_at_an_angle_slides_along_it`
#[test]
fn walking_into_a_wall_at_an_angle_slides_along_it() {
    let world = joined([floor(), cuboid([-30.0, 30.0], [1.0, 6.0], [5.0, 6.0])]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    // Forward and to the left (+x).
    let path = body.walk(
        120,
        Command {
            right: -127,
            ..run(0)
        },
    );
    assert_eq!(path[119][2], REST);
    assert!(path[119][0] - path[60][0] > 2.0); // still going along the wall
    assert!(body.velocity()[0] > 3.0);
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_long_slide_keeps_the_same_distance_every_tick`
#[test]
fn a_long_slide_keeps_the_same_distance_every_tick() {
    let world = joined([floor(), cuboid([-30.0, 30.0], [1.0, 6.0], [5.0, 6.0])]);
    // 45 degrees to the wall.
    let mut body = Body::new([-25.0, 1.0, 3.0], Some(world), &[], 8192);
    let path = body.walk(
        300,
        Command {
            yaw: 8192,
            ..run(0)
        },
    );
    let touching: Vec<_> = path.iter().filter(|feet| feet[2] == REST).collect();
    assert!(touching.len() > 250, "{}", touching.len());
    assert_eq!(path[299][2], REST);
    let mut along: Vec<_> = touching[20..]
        .windows(2)
        .map(|pair| steps(pair[1][0] - pair[0][0]))
        .collect();
    along.dedup();
    // The same step along the wall every tick, no stutter.
    assert_eq!(along.len(), 1, "{along:?}");
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_corner_holds_the_body_on_both_walls`
#[test]
fn a_corner_holds_the_body_on_both_walls() {
    let world = joined([
        floor(),
        cuboid([-30.0, 30.0], [1.0, 6.0], [5.0, 6.0]),
        cuboid([-3.0, -2.0], [1.0, 6.0], [-30.0, 30.0]),
    ]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    // Forward and to the right (-x).
    let path = body.walk(
        150,
        Command {
            right: 127,
            ..run(0)
        },
    );
    let last = path[149];
    assert_eq!((last[0], last[2]), (-2.0 + 0.5 + 5.0 * STEP, REST));
    assert_eq!(last, path[130]); // at rest in the corner
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_an_acute_corner_holds_the_body_still`
#[test]
fn an_acute_corner_holds_the_body_still() {
    let (apex, back) = ([0.0, 5.0], 3.0 / 30f64.to_radians().tan());
    // A 60 degree pocket opening to -z.
    let sides = [(apex, [-3.0, 5.0 - back]), (apex, [3.0, 5.0 - back])];
    let world = joined([
        floor(),
        wall(sides[0].0, sides[0].1),
        wall(sides[1].0, sides[1].1),
    ]);
    let mut body = Body::soldier([0.0, 1.0, -2.0], Some(world));
    let path = body.walk(200, run(0));
    assert_eq!(path[199], path[170]); // wedged and still, no jitter
    let [x, _, z] = path[199];
    assert!(z < apex[1] - 0.5, "{z}");
    for (a, b) in sides {
        // Out of both walls.
        assert!(segment_distance([x, z], a, b) > 0.5, "{x} {z}");
    }
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_step_against_a_wall`
#[test]
fn a_step_against_a_wall() {
    let world = joined([
        floor(),
        cuboid([-30.0, 30.0], [1.0, 6.0], [5.0, 6.0]),
        cuboid([-30.0, 30.0], [1.0, 1.2], [4.6, 5.0]), // a 0.2 m kerb at its foot
    ]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    let path = body.walk(150, run(0));
    // The capsule floats over the kerb and stops at the wall.
    assert_eq!(path[149][2], REST);
    assert!(!body.airborne());
    assert_eq!(path[149], path[140]);
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_walking_along_a_crate_edge_slides_round_it`
#[test]
fn walking_along_a_crate_edge_slides_round_it() {
    // A 1 m crate, its near face at z = 4, its corner at x = 0.5.
    let crate_box = cuboid([-0.5, 0.5], [1.0, 2.0], [4.0, 5.0]);
    let mut body = Body::soldier([0.9, 1.0, 0.0], Some(joined([floor(), crate_box])));
    let path = body.walk(150, run(0));
    let [x, _, z] = path[149];
    assert!(z > 5.0, "{z}"); // pushed off the crate's corner and past it
    assert!(x > 0.9, "{x}");
    for [fx, _, fz] in path {
        // The capsule never goes into the crate.
        let gap = (-0.5 - fx)
            .max(0.0)
            .max(fx - 0.5)
            .hypot((4.0 - fz).max(0.0).max(fz - 5.0));
        assert!(gap > 0.5, "{fx} {fz}");
    }
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_walking_off_a_ledge_falls_and_lands_below`
#[test]
fn walking_off_a_ledge_falls_and_lands_below() {
    let upper = cuboid([-30.0, 30.0], [0.0, 1.0], [-30.0, 3.0]);
    let lower = cuboid([-30.0, 30.0], [-3.0, -1.0], [-30.0, 30.0]); // its top at y = -1
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(joined([upper, lower])));
    let (mut flags, mut heights) = (Vec::new(), Vec::new());
    for tick in 0..150 {
        body.walk(1, run(tick));
        flags.push(body.flags());
        heights.push(body.position()[1]);
    }
    let fell = flags.iter().position(|word| word & FELL != 0).unwrap();
    let landed = fell
        + flags[fell..]
            .iter()
            .position(|word| word & FELL == 0)
            .unwrap();
    assert_eq!(heights[0], 1.0);
    // Past the edge the centre ray misses and the disc holds the feet on its
    // cast's point, 1 cm out of the top (as the live client's).
    assert_eq!(heights[fell - 1], 1034.0 * STEP);
    assert!(body.position()[2] > 3.0);
    // 2 m down at 17.5 m/s^2: about 0.48 s.
    assert!((20..=40).contains(&(landed - fell)), "{}", landed - fell);
    assert_eq!(body.position()[1], -1.0);
    assert!(!body.airborne());
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_steps_keep_the_feet_on_each_tread`
#[test]
fn steps_keep_the_feet_on_each_tread() {
    // 0.25 m risers, 1 m treads, from z = 3.
    let stairs = (0..6).map(|k| {
        let k = f64::from(k);
        cuboid([-30.0, 30.0], [1.0, 1.25 + 0.25 * k], [3.0 + k, 30.0])
    });
    let world = joined(std::iter::once(floor()).chain(stairs));
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    let mut path = vec![body.position()];
    for tick in 0..200 {
        body.walk(1, run(tick));
        assert!(!body.airborne(), "tick {tick}");
        path.push(body.position());
    }
    // The probe at the start of a tick snaps the feet to the tread under
    // them: the tread where the feet were a tick before.
    for pair in path.windows(2) {
        let (z, y) = (pair[0][2], pair[1][1]);
        let tread = if z < 3.0 {
            1.0
        } else {
            1.25 + 0.25 * (z - 3.0).floor().min(5.0)
        };
        assert!(close(y, tread, STEP), "{y} at z {z}");
    }
    assert_eq!(path[200][1], 2.5);
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_slope_keeps_the_feet_on_it`
#[test]
fn a_slope_keeps_the_feet_on_it() {
    let world = joined([
        floor(),
        ramp(2.0, 22.0, 1.0, 30.0),
        cuboid([-30.0, 30.0], [1.0, 40.0], [22.0, 23.0]),
    ]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    let mut before = body.position()[2];
    for tick in 0..150 {
        body.walk(1, run(tick));
        assert!(!body.airborne(), "tick {tick}");
        let [_, y, z] = body.position();
        if before > 2.0 {
            // On the slope since the last tick: the feet go along it, within
            // the spring's dead band (1/512 m, where the capsule does not
            // follow the snap) and the grid.
            let on_slope = 1.0 + (z - 2.0) * 30f64.to_radians().tan();
            assert!(close(y, on_slope, 3.0 * STEP), "tick {tick} at z {z}");
        }
        before = z;
    }
    assert!(body.position()[2] > 10.0);
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_down_a_slope_the_body_stays_on_the_ground`
#[test]
fn down_a_slope_the_body_stays_on_the_ground() {
    let bottom = 1.0 - 20.0 * 30f64.to_radians().tan();
    let world = joined([
        cuboid([-30.0, 30.0], [0.0, 1.0], [2.0, 30.0]),
        ramp(-18.0, 2.0, bottom, 30.0),
        cuboid([-30.0, 30.0], [-15.0, bottom], [-40.0, -18.0]),
    ]);
    // Facing -z.
    let mut body = Body::new([0.0, 1.0, 1.0], Some(world), &[], 32768);
    for tick in 0..120 {
        body.walk(
            1,
            Command {
                yaw: 32768,
                ..run(tick)
            },
        );
        assert!(!body.airborne(), "tick {tick}");
    }
    // The speed along the slope is the horizontal one.
    assert!(body.position()[1] < -4.0, "{}", body.position()[1]);
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_steep_slope_cannot_be_walked_up`
#[test]
fn a_steep_slope_cannot_be_walked_up() {
    let world = joined([floor(), ramp(2.0, 6.0, 1.0, 60.0)]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(world));
    body.walk(150, run(0));
    let [_, y, z] = body.position();
    assert!(z < 2.5, "{z}");
    assert!(y < 1.5, "{y}");
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_low_ceiling_keeps_the_body_crouched`
#[test]
fn a_low_ceiling_keeps_the_body_crouched() {
    // 1.6 m above the floor, from z = 3 to 8.
    let ceiling = cuboid([-30.0, 30.0], [2.6, 4.0], [3.0, 8.0]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(joined([floor(), ceiling])));
    // Crouched, it goes under.
    body.walk(
        75,
        Command {
            buttons: CROUCH,
            ..run(0)
        },
    );
    assert!(body.position()[2] > 3.5);
    assert!(body.crouched());
    body.walk(20, stand(75)); // the button is up: no room to stand
    assert!(body.crouched());
    body.walk(160, run(95)); // out the other side it stands up
    assert!(body.position()[2] > 8.6);
    assert!(!body.crouched());
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_jump_into_a_ceiling_comes_back_down`
#[test]
fn a_jump_into_a_ceiling_comes_back_down() {
    // 2.5 m above the floor.
    let ceiling = cuboid([-30.0, 30.0], [3.5, 5.0], [-30.0, 30.0]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(joined([floor(), ceiling])));
    body.walk(
        1,
        Command {
            buttons: JUMP,
            ..stand(0)
        },
    );
    let mut top: f64 = 0.0;
    for tick in 1..80 {
        body.walk(1, stand(tick));
        top = top.max(body.position()[1]);
    }
    // The capsule's 2 m top stops under it.
    assert!(top < 1.0 + 3.5 - 1.0 - 2.0 + 0.01, "{top}");
    assert_eq!(body.position()[1], 1.0);
    assert!(!body.airborne());
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_body_that_falls_out_of_the_map_stops_far_below_it`
#[test]
fn a_body_that_falls_out_of_the_map_stops_far_below_it() {
    // A 2 m platform, nothing under it.
    let platform = cuboid([-1.0, 1.0], [0.0, 1.0], [-1.0, 1.0]);
    let mut body = Body::soldier([0.0, 1.0, 0.0], Some(platform));
    body.walk(600, run(0));
    assert_eq!((body.position()[1], body.velocity()[1]), (-100.0, 0.0));
    assert!(body.airborne());
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_spawning_a_little_above_or_below_the_floor_stands_on_it`
#[test]
fn spawning_a_little_above_or_below_the_floor_stands_on_it() {
    for start in [1.3, 0.97] {
        let mut body = Body::soldier([0.0, start, 0.0], Some(floor()));
        body.walk(30, stand(0));
        assert_eq!(body.position()[1], 1.0, "from {start}");
        assert!(!body.airborne(), "from {start}");
    }
}

/// `tests/test_game_mover.py::CollisionMoveTests::test_a_surface_that_is_not_ground_is_not_stood_on`
#[test]
fn a_surface_that_is_not_ground_is_not_stood_on() {
    let slippery = cuboid([-30.0, 30.0], [0.0, 1.0], [-30.0, 30.0]);
    let below = cuboid([-30.0, 30.0], [-3.0, -1.0], [-30.0, 30.0]);
    let surfaces: Vec<u16> = [NOT_GROUND; 12].into_iter().chain([0; 12]).collect();
    let mut body = Body::new(
        [0.0, 1.0, 0.0],
        Some(joined([slippery, below])),
        &surfaces,
        0,
    );
    body.walk(1, stand(0));
    assert!(body.airborne());
}

/// `tests/test_game_mover.py::MoverStateTests::test_the_pitch_is_held_to_89_degrees_and_that_is_remembered`
#[test]
fn the_pitch_is_held_to_89_degrees_and_that_is_remembered() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    let aim = |frame, pitch| Command {
        pitch,
        ..stand(frame)
    };
    body.step(aim(1, 9000));
    assert_eq!(
        (body.mover.pose().pitch, body.flags() & TURN_HELD),
        (9000, 0)
    );
    body.step(aim(2, 17000));
    assert_eq!(
        (body.mover.pose().pitch, body.flags() & TURN_HELD),
        (16202, TURN_HELD)
    );
    body.step(aim(3, -20000));
    assert_eq!(body.mover.pose().pitch, -16202);
    body.step(stand(4));
    // 0x200000 is never cleared.
    assert_eq!((body.mover.pose().pitch, body.flags()), (0, TURN_HELD));
}

/// `tests/test_game_mover.py::MoverStateTests::test_the_state_keeps_the_throttle_bytes_the_way_the_client_rounds_them`
///
/// Python's `input_tick` (the server tick of the last throttled command) has
/// no Rust counterpart; the Rust pose keeps the command frame only, as `None`
/// meaning the record's own frame.
#[test]
fn the_state_keeps_the_throttle_bytes_the_way_the_client_rounds_them() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.step(Command {
        forward: 104,
        right: -127,
        ..stand(7)
    });
    let pose = body.mover.pose();
    assert_eq!(
        (pose.throttles, pose.frame, pose.input_frame),
        ([-127, 103], Some(7), None)
    );
    body.step(Command {
        forward: 127,
        right: 9,
        ..stand(8)
    });
    assert_eq!(body.mover.pose().input_throttles, [8, 127]);
}

/// `tests/test_game_mover.py::MoverStateTests::test_the_spring_offsets_go_as_steps_of_their_range_and_the_second_follows_the_first`
#[test]
fn the_spring_offsets_go_as_steps_of_their_range_and_the_second_follows_the_first() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(5, stand(0));
    let rest = 23406; // Soldier's range [-2.5, 1.0]: 2.5 / 3.5 of 32768
    assert_eq!(
        (body.mover.pose().spring, body.mover.posture.spring_offset()),
        ([rest, rest], 2f32.powi(-15))
    );
    let mut springs = Vec::new();
    for tick in 5..20 {
        // Crouched, the capsule stays put: 0.25 m above its new rest.
        body.walk(
            1,
            Command {
                buttons: CROUCH,
                ..stand(tick)
            },
        );
        springs.push(body.mover.pose().spring);
    }
    assert!(springs[0][0] > rest + 1500, "{springs:?}");
    let snapped = springs
        .iter()
        .position(|[first, _]| *first == rest)
        .unwrap();
    assert!(
        springs[..snapped]
            .iter()
            .all(|[first, second]| first == second),
        "{springs:?}"
    );
    // It keeps the first one's last move, then rests too.
    assert!(springs[snapped][1] > rest, "{springs:?}");
    assert_eq!(springs[14], [rest, rest]);
    let pose = body.mover.pose();
    assert_eq!((pose.spring_speed, pose.crouch_frame), (0.0, None));
    body.walk(1, stand(20));
    assert_eq!(body.mover.pose().crouch_frame, Some(20));
}

/// `tests/test_game_mover.py::MoverStateTests::test_gravitys_part_of_the_velocity_follows_the_fall`
#[test]
fn gravitys_part_of_the_velocity_follows_the_fall() {
    let mut body = Body::soldier([0.0, 1.0, 0.0], None);
    body.walk(
        1,
        Command {
            buttons: JUMP,
            ..stand(0)
        },
    );
    let mut falls = Vec::new();
    while body.airborne() {
        body.walk(1, stand(falls.len() as u32 + 1));
        let pose = body.mover.pose();
        falls.push((pose.velocity[1], f64::from(pose.fall)));
    }
    // Going up: none.
    assert!(
        falls
            .iter()
            .filter(|(speed, _)| *speed > 0.0)
            .all(|(_, fall)| *fall == 0.0)
    );
    // Falling it is the fall speed (before the grid), or what gravity added
    // if that is less.
    assert!(
        falls
            .iter()
            .filter(|(speed, _)| *speed < 0.0)
            .all(|(speed, fall)| (fall - speed).abs() < 2.0 * STEP),
        "{falls:?}"
    );
    assert!(falls[falls.len() - 2].1 < -5.0); // the last tick in the air
    assert_eq!(body.mover.pose().fall, 0.0); // the landing hands it to the body
}

fn speed_mod(operation: Operation, value: f64) -> Modifier {
    Modifier {
        index: SPEED,
        operation,
        value,
        priority: 0.0,
    }
}

/// `tests/test_game_mover.py::ModTests::test_a_mod_acts_from_the_frame_after_the_statescript_turned_it_on_up_to_the_one_it_went_off`
///
/// Whether the mod acts on a frame is read from the run speed it gives there.
#[test]
fn a_mod_acts_from_the_frame_after_the_statescript_turned_it_on_up_to_the_one_it_went_off() {
    let base = MoverSettings::default();
    let key = ModifierKey {
        instance: 5,
        state: 0,
        slot: 0,
    };
    let acts =
        |timeline: &ModifierTimeline, frame| timeline.at(frame).settings(&base).run_forward != 5.5;
    let mut timeline = ModifierTimeline::default();
    timeline.track(10, &BTreeMap::from([(key, speed_mod(Operation::Add, 0.5))]));
    // Start 11, no end.
    assert_eq!(
        [10, 11, 1000].map(|frame| acts(&timeline, frame)),
        [false, true, true]
    );
    timeline.track(20, &BTreeMap::new());
    assert_eq!(
        [10, 11, 20, 21].map(|frame| acts(&timeline, frame)),
        [false, true, true, false]
    );
    // The mover's next tick is frame 22: the mod is gone, even for frame 20.
    timeline.track(21, &BTreeMap::new());
    assert!(!acts(&timeline, 20));
}

/// `tests/test_game_mover.py::ModTests::test_the_speed_scalar_changes_the_speeds_and_the_friction_base_speed`
#[test]
fn the_speed_scalar_changes_the_speeds_and_the_friction_base_speed() {
    let data = MoverSettings::default();
    assert_eq!(ActiveModifiers::default().settings(&data), data);
    let key = |slot| ModifierKey {
        instance: 1,
        state: 0,
        slot,
    };
    let mut timeline = ModifierTimeline::default();
    timeline.track(
        0,
        &BTreeMap::from([
            (key(0), speed_mod(Operation::Add, 0.5)),
            (
                key(1),
                Modifier {
                    index: CROUCH_INTENT,
                    operation: Operation::Set,
                    value: 0.0,
                    priority: 0.0,
                },
            ),
        ]),
    );
    let sprint = timeline.at(1).settings(&data);
    let f32 = |value: f64| f64::from(value as f32);
    assert_eq!(
        (sprint.run_forward, sprint.run_strafe, sprint.crouch_forward),
        (8.25, 8.25, 4.5)
    );
    assert_eq!(sprint.run_backward, f32(4.95 * 1.5));
    assert_eq!(sprint.decel_ground_base_speed, f32(1.5 * 6.2));
    assert_eq!(
        (sprint.accel_ground, sprint.decel_ground_base),
        (35.0, 15.0)
    );
}

/// An axis-aligned box as a polytope the way the engine keeps one: planes
/// facing out, half edge twins side by side (2k, 2k + 1) (`hull_box` in
/// `tests/test_game_mover.py`).
fn hull_box(x: [f32; 2], y: [f32; 2], z: [f32; 2], mask: u16) -> Hull {
    let corners: Vec<[f32; 3]> = x
        .into_iter()
        .flat_map(|x| y.into_iter().flat_map(move |y| z.map(|z| [x, y, z])))
        .collect();
    let loops: [[u8; 4]; 6] = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let mut pairs: HashMap<(u8, u8), u8> = HashMap::new();
    for corners in loops {
        for k in 0..4 {
            let (a, b) = (corners[k], corners[(k + 1) % 4]);
            if !pairs.contains_key(&(b, a)) && !pairs.contains_key(&(a, b)) {
                pairs.insert((a, b), pairs.len() as u8);
            }
        }
    }
    let half = |a: u8, b: u8| match pairs.get(&(a, b)) {
        Some(&number) => 2 * number,
        None => 2 * pairs[&(b, a)] + 1,
    };
    let mut half_edges = vec![
        HalfEdge {
            twin: 0,
            origin: 0,
            face: 0,
            next: 0
        };
        2 * pairs.len()
    ];
    let (mut faces, mut planes) = (Vec::new(), Vec::new());
    for (face, corner) in loops.into_iter().enumerate() {
        let named: [u8; 4] = std::array::from_fn(|k| half(corner[k], corner[(k + 1) % 4]));
        for (k, &number) in named.iter().enumerate() {
            half_edges[usize::from(number)] = HalfEdge {
                twin: if number % 2 == 0 { 1 } else { -1 },
                origin: corner[k],
                face: face as u8,
                next: named[(k + 1) % 4],
            };
        }
        faces.push(named[0]);
        let axis = face / 2;
        let mut plane = [0.0; 4];
        plane[axis] = if face % 2 == 1 { 1.0 } else { -1.0 };
        plane[3] = corners[usize::from(corner[0])][axis] * plane[axis];
        planes.push(plane);
    }
    Hull {
        mask,
        centroid: [0, 1, 2].map(|axis| {
            let [low, high] = [x, y, z][axis];
            (low + high) / 2.0
        }),
        vertices: corners,
        planes,
        faces,
        half_edges,
    }
}

/// `tests/test_game_mover.py::ChunkTests::test_a_shape_with_one_teams_proxy_stops_only_that_team`
#[test]
fn a_shape_with_one_teams_proxy_stops_only_that_team() {
    const TEAM0: u16 = 0x08;
    const TEAM1: u16 = 0x10;
    // Mask 0x1FD5 has team 1's proxy bit only, as 58 props of Blizzard World do.
    let hull = hull_box([-1.0, 1.0], [-1.0, 2.0], [4.0, 6.0], 0x1FD5);
    let shape = StaticShape::new(StaticGeometry::Hull(Arc::new(hull)), None).unwrap();
    let world = Arc::new(ContactWorld::new(vec![shape]).unwrap());
    for (category, stopped) in [(TEAM1, true), (TEAM0, false), (MOVER, true)] {
        let pose = CapsulePose {
            position: [0.0; 3],
            radius: 0.5,
            top: 1.5,
        };
        let mut body = CharacterBody::new(Arc::clone(&world), pose, category);
        body.teleport([0.25, 0.0, 2.0]);
        let mut last = [0.0; 3];
        for _ in 0..30 {
            last = body.move_capsule([0.0, 0.0, 0.2], 0.016).unwrap().position;
        }
        assert_eq!(last[2] < 3.5, stopped, "category {category:#x}: {last:?}");
        // Off the box's middle: the engine's GJK misses an overlap whose
        // first two support points line up with the origin exactly (the
        // client's does too).
        body.teleport([0.25, 0.0, 5.25]);
        assert_eq!(
            body.fits(0.5, 1.5).unwrap(),
            !stopped,
            "category {category:#x}"
        );
    }
}

/// Soldier: 76's derived mover component has the built-in values the box
/// world tests run with (`mover_data(SOLDIER_BODY)` in
/// `ow174/game/mover.py`).
#[test]
#[ignore = "requires the derived 104319 mover catalog"]
fn soldiers_derived_mover_values_are_the_built_in_ones() {
    let path = orpheus_testkit::runtime_dir().join("data/game_movers_174.json");
    let catalog = MoverCatalog::load(path, SOLDIER_BODY).unwrap();
    assert_eq!(
        catalog.for_body(Some(SOLDIER_BODY)),
        &MoverSettings::default()
    );
}

/// The Practice Range's movement world for the modes without placeable
/// identifiers (`World.load(cache_path(PRACTICE_RANGE.map_guid))` in
/// `tests/test_game_mover.py`).
struct Map {
    scene: Arc<CollisionScene>,
    contacts: Arc<ContactWorld>,
}

impl Map {
    fn practice_range() -> Self {
        let path = orpheus_testkit::runtime_dir().join("collision/0688.col");
        let cache = read_movement_cache(File::open(path).unwrap()).unwrap();
        let identifiers = BTreeSet::new();
        let scene = CollisionScene::movement_probe(&cache, &identifiers).unwrap();
        let contacts = ContactWorld::from_movement_cache(cache, &identifiers).unwrap();
        Self {
            scene: Arc::new(scene),
            contacts: Arc::new(contacts),
        }
    }

    /// `Mover(position, yaw, world, mover_data(SOLDIER_BODY), category)`.
    fn soldier(&self, position: [f64; 3], yaw: i32, category: u16) -> Body {
        let contacts = Arc::clone(&self.contacts);
        Body {
            mover: Mover::new(MoverSettings::default(), contacts, position, yaw, category).unwrap(),
            rays: Rays::Map(Arc::clone(&self.scene)),
        }
    }

    fn ground(&self, x: f64, z: f64, top: f64, bottom: f64) -> Option<GroundHit> {
        self.scene.triangles().ground(x, z, top, bottom, MOVER)
    }
}

/// The Practice Range's spawn (`PRACTICE_RANGE.spawns[0]` in
/// `ow174/game/content.py`) and its yaw in command units,
/// `round(-170.3434 * 65536 / 360)`.
const SPAWN: [f64; 3] = [54.56507, 1.0, 42.14788];
const SPAWN_YAW: i32 = -31010;

/// `tests/test_game_mover.py::RealMapTests::test_the_practice_range_spawns_stand_on_the_floor`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn the_practice_range_spawns_stand_on_the_floor() {
    let map = Map::practice_range();
    let mut body = map.soldier(SPAWN, 0, MOVER);
    body.walk(20, stand(0));
    let [x, y, z] = body.position();
    let ground = map.ground(x, z, y + 1.0, y - 1.0).unwrap();
    assert!(!body.airborne());
    assert!(close(y, ground.y, STEP), "{y} over {}", ground.y);
}

/// `tests/test_game_mover.py::RealMapTests::test_walking_out_of_the_practice_range_spawn`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn walking_out_of_the_practice_range_spawn() {
    let map = Map::practice_range();
    let mut body = map.soldier(SPAWN, SPAWN_YAW, MOVER);
    // Out of the spawn room, over a 2.4 cm step.
    for tick in 0..300 {
        body.walk(
            1,
            Command {
                yaw: SPAWN_YAW,
                ..run(tick)
            },
        );
        assert!(!body.airborne(), "tick {tick}");
        let [x, y, z] = body.position();
        let ground = map.ground(x, z, y + 0.6, y - 0.6).unwrap();
        // It stands there from the next tick on.
        assert!(
            (y - ground.y).abs() < 0.1,
            "tick {tick}: {y} over {}",
            ground.y
        );
    }
    body.walk(
        10,
        Command {
            yaw: SPAWN_YAW,
            ..stand(300)
        },
    );
    let [x, y, z] = body.position();
    assert!(close(
        y,
        map.ground(x, z, y + 0.6, y - 0.6).unwrap().y,
        STEP
    ));
    let away = (0..3).map(|axis| (SPAWN[axis] - body.position()[axis]).powi(2));
    assert!(away.sum::<f64>().sqrt() > 20.0);
}

/// `tests/test_game_mover.py::PropTests::test_a_railing_keeps_the_body_on_the_balcony`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn a_railing_keeps_the_body_on_the_balcony() {
    // The balcony north of the spawn (its floor at y = 7 up to x = 47.8, then
    // a 6 m drop) has a 0.9 m railing (definition 2600) at x = 47.5; without
    // it the body walks off and falls to the floor.
    let map = Map::practice_range();
    let mut body = map.soldier([46.5, 7.0, 43.3], 16384, MOVER); // facing +x
    body.walk(
        90,
        Command {
            yaw: 16384,
            ..run(0)
        },
    );
    let [x, y, _] = body.position();
    assert!(x < 47.5, "{x}");
    assert!(close(y, 7.0, 0.05), "{y}");
    assert!(!body.airborne());
    assert_ne!(body.flags() & TOUCHING, 0);
}

/// `tests/test_game_mover.py::PropTests::test_the_office_chairs_on_wheels_are_not_solid`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn the_office_chairs_on_wheels_are_not_solid() {
    // 04AA (a chair on five wheels) is a dynamic body in the client, which
    // the mover does not meet: the one at the desks (44.15, 1.0, 33.47) is no
    // ground (its seat used to be, at 1.72 m) and no shape of the engine.
    let map = Map::practice_range();
    assert_eq!(map.ground(44.15, 33.47, 1.9, 1.2), None);
    let chair = map.contacts.shapes().iter().filter_map(|shape| {
        let low = shape.fat_bounds()?.low;
        ((low[0] - 43.658).abs() < 0.01 && (low[2] - 32.936).abs() < 0.01).then_some(low)
    });
    assert_eq!(chair.count(), 0);
}

/// `tests/test_game_mover.py::PropTests::test_a_body_on_the_desk_stops_at_the_monitor_where_the_client_does`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn a_body_on_the_desk_stops_at_the_monitor_where_the_client_does() {
    // The desk 04AB's hull hangs on its model's node 0x8B, 1.62 m up: a box
    // from the desk top to 3 m in front of the wall monitor. The live client
    // stopped at (46.01171875, 31.1826171875) walking along the wall on the
    // desk (session 070300_3A33, frame 50668).
    let map = Map::practice_range();
    let monitor = map.contacts.shapes().iter().any(|shape| {
        let Some(fat) = shape
            .fat_bounds()
            .filter(|_| matches!(shape.geometry(), StaticGeometry::Hull(_)))
        else {
            return false;
        };
        (43.5..43.7).contains(&fat.low[0])
            && (fat.low[1] - 1.805).abs() < 0.01
            && (fat.high[1] - 3.123).abs() < 0.01
    });
    assert!(monitor);
    for z in [31.18, 31.2] {
        let mut body = map.soldier([46.40, 1.9326171875, z], -16384, MOVER);
        body.walk(
            60,
            Command {
                yaw: -16384,
                ..run(0)
            },
        );
        let [x, _, z] = body.position();
        assert_eq!([x, z], [46.01171875, 31.1826171875]);
        assert_ne!(body.flags() & TOUCHING, 0);
    }
}

/// Soldier: 76 (team 0) in a state the live client predicted: on the ground,
/// the spring at rest (`client_mover` in `tests/test_game_mover.py`). The
/// rested posture is taken from a body that stood still for a tick, which
/// puts both spring offsets on step 23406 of 32768.
fn client_mover(
    map: &Map,
    position: [f64; 3],
    velocity: [f64; 3],
    [yaw, pitch]: [i32; 2],
    throttles: [i8; 2],
    frame: u32,
) -> Body {
    const TEAM0: u16 = 0x08;
    let mut rested = Body::soldier([0.0, 1.0, 0.0], None);
    rested.walk(1, stand(0));
    assert_eq!(rested.mover.pose().spring, [23406; 2]);
    let base = MoverSettings::default();
    let mut body = map.soldier(position, yaw, TEAM0);
    let mover = &mut body.mover;
    mover.posture = std::mem::replace(&mut rested.mover.posture, Posture::new(&base));
    mover.posture.flags = TURN_HELD | REAL_JUMP;
    let [x, y, z] = mover.pose.position;
    let lift = f64::from(mover.lift());
    mover.body.teleport([x as f32, (y + lift) as f32, z as f32]);
    mover.pose.velocity = velocity;
    mover.pose.pitch = pitch;
    mover.pose.throttles = throttles;
    mover.pose.input_throttles = throttles;
    mover.last_input = Some(frame);
    body
}

/// `tests/test_game_mover.py::PropTests::test_the_disc_holds_the_feet_1_cm_over_a_ledge_where_the_client_does`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn the_disc_holds_the_feet_1_cm_over_a_ledge_where_the_client_does() {
    // Stepping up onto a seat (1.6899) and the desk top (1.9307) the centre
    // ray misses the ledge and the ground disc catches it: the live client's
    // feet stand on the cast's point, 1 cm out of the mesh (session
    // 070300_3A33, its own states and commands: frames 50315..50322 and
    // 51438..51439).
    // (position, velocity, [yaw, pitch], throttles, frame) of the client's
    // state, its next commands as (forward, right, yaw, pitch), the feet.
    type State = ([f64; 3], [f64; 3], [i32; 2], [i8; 2], u32);
    type Keys = (i8, i8, i32, i32);
    let steps: [(State, &[Keys], [f64; 3]); 2] = [
        (
            (
                [45.8525390625, 1.0, 32.9755859375],
                [-3.6005859375, 0.0, -4.1572265625],
                [-25323, 5756],
                [0, 127],
                50314,
            ),
            &[
                (127, 0, -25215, 6008),
                (127, 0, -25143, 6242),
                (127, 0, -25034, 6495),
                (127, 0, -24980, 6657),
                (127, 0, -24890, 6855),
                (127, 0, -24854, 6999),
                (127, 0, -24818, 7107),
                (127, 0, -24782, 7180),
            ],
            [45.54296875, 1.7001953125, 32.548828125],
        ),
        (
            (
                [45.0029296875, 1.7529296875, 32.1494140625],
                [-4.890625, 0.0, 2.5166015625],
                [-27810, 15932],
                [-127, 0],
                51437,
            ),
            &[(0, -127, -27810, 15896), (0, -127, -27810, 15860)],
            [44.84765625, 1.9404296875, 32.2294921875],
        ),
    ];
    let map = Map::practice_range();
    for ((position, velocity, aim, throttles, frame), commands, feet) in steps {
        let mut body = client_mover(&map, position, velocity, aim, throttles, frame);
        for (frame, &(forward, right, yaw, pitch)) in (frame + 1..).zip(commands) {
            body.step(Command {
                frame,
                forward,
                right,
                yaw,
                pitch,
                ..Command::default()
            });
        }
        assert_eq!(body.position(), feet);
    }
}

/// `tests/test_game_mover.py::PropTests::test_the_things_on_the_desk_are_not_ground`
#[test]
#[ignore = "requires a derived 104319 Practice Range movement cache"]
fn the_things_on_the_desk_are_not_ground() {
    // 04A3 (capsules, a hull and a sphere on the desk) is dynamic in the
    // client; the server's body used to step onto its hull (2.1888 m) at the
    // desk's left end where the client's stood on the desk.
    let map = Map::practice_range();
    let desk = map.ground(43.0745, 32.0777, 2.5, 1.5).unwrap();
    assert!(desk.y < 1.94, "{}", desk.y); // the desk top, 1.9328
    assert!(
        !map.contacts
            .shapes()
            .iter()
            .any(|shape| matches!(shape.geometry(), StaticGeometry::Round { .. }))
    );
}
