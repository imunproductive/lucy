//! Movement on a ready collision world (ported from
//! tests/test_game_collision_ready.py).

use std::sync::Arc;

use orpheus_physics::{
    collision::{
        cache::{Mesh, Triangle},
        index::{SceneTriangle, TriangleIndex},
    },
    contact::{
        mesh::ContactMesh,
        world::{ContactWorld, StaticGeometry, StaticShape},
    },
    mover::{Mover, settings::MoverSettings},
};
use orpheus_wire::commands::Command;

/// The team proxy categories a movement triangle stops (collision.py
/// `MOVER`); team 0 moves with the first.
const MOVER: u16 = 0x18;
const TEAM0: u16 = 0x08;

/// The Python test's `wall_world`: a floor, and a two-sided wall across
/// the way at z = 2.
const WALL_WORLD: [[f32; 9]; 6] = [
    [-20.0, 0.0, -20.0, -20.0, 0.0, 20.0, 20.0, 0.0, 20.0],
    [-20.0, 0.0, -20.0, 20.0, 0.0, 20.0, 20.0, 0.0, -20.0],
    [-10.0, 0.0, 2.0, 10.0, 0.0, 2.0, 10.0, 5.0, 2.0],
    [-10.0, 0.0, 2.0, 10.0, 5.0, 2.0, -10.0, 5.0, 2.0],
    [-10.0, 0.0, 2.0, 10.0, 5.0, 2.0, 10.0, 0.0, 2.0],
    [-10.0, 0.0, 2.0, -10.0, 5.0, 2.0, 10.0, 5.0, 2.0],
];

/// Loose triangles as one mesh (collision.py `soup_mesh`): shared corners
/// merged, each edge's wing the third corner of the other triangle on it.
fn soup_mesh(triangles: &[[f32; 9]]) -> Mesh {
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let corners: Vec<[i32; 3]> = triangles
        .iter()
        .map(|triangle| {
            std::array::from_fn(|k| {
                let point = [triangle[3 * k], triangle[3 * k + 1], triangle[3 * k + 2]];
                let number = vertices
                    .iter()
                    .position(|&known| known == point)
                    .unwrap_or_else(|| {
                        vertices.push(point);
                        vertices.len() - 1
                    });
                number as i32
            })
        })
        .collect();
    let edge = |u: i32, w: i32| (u.min(w), u.max(w));
    let sides = |[a, b, c]: [i32; 3]| [(a, b), (b, c), (c, a)];
    let triangles = corners
        .iter()
        .enumerate()
        .map(|(number, &named)| Triangle {
            corners: named,
            wings: sides(named).map(|(u, w)| {
                corners
                    .iter()
                    .enumerate()
                    .find(|&(other, &corners)| {
                        other != number
                            && sides(corners)
                                .iter()
                                .any(|&(p, q)| edge(p, q) == edge(u, w))
                    })
                    .and_then(|(_, corners)| corners.iter().copied().find(|x| ![u, w].contains(x)))
                    .unwrap_or(-1)
            }),
            mask: MOVER,
            surface: 0,
        })
        .collect();
    Mesh {
        vertices,
        triangles,
    }
}

/// tests/test_game_collision_ready.py
/// `test_ready_movement_cannot_cross_the_wall`: a second of running
/// forward from z = 0 ends against the wall at z = 2, not through it.
#[test]
fn ready_movement_cannot_cross_the_wall() {
    let mesh = ContactMesh::new(soup_mesh(&WALL_WORLD), [1.0; 3], u16::MAX).unwrap();
    let shape = StaticShape::new(StaticGeometry::Mesh(Arc::new(mesh)), None).unwrap();
    let world = Arc::new(ContactWorld::new(vec![shape]).unwrap());
    let rays = TriangleIndex::new(WALL_WORLD.iter().map(|triangle| SceneTriangle {
        vertices: std::array::from_fn(|k| {
            std::array::from_fn(|axis| f64::from(triangle[3 * k + axis]))
        }),
        mask: MOVER,
        surface: 0,
    }))
    .unwrap();
    let mut mover = Mover::new(MoverSettings::default(), world, [0.0; 3], 0, TEAM0).unwrap();
    for frame in 1001..1065 {
        let command = Command {
            frame,
            forward: 127,
            ..Command::default()
        };
        mover.step(&rays, command, 0.016, |_, _| {}).unwrap();
    }
    let z = mover.pose().position[2];
    assert!(1.0 < z && z < 2.0, "z = {z}");
}
