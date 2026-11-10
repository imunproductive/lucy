//! The collision chunk's format and the cache, on hand-made chunks, ported
//! from `tests/test_game_mover.py::ChunkTests` (`parse_chunk`, `pack` and
//! `World.load` in `ow174/game/collision.py`). The cache is read back with
//! the server's reader, `orpheus_physics`.

use std::collections::{BTreeSet, HashMap};

use orpheus_physics::{
    collision::{
        cache::{self, CacheError, read_movement_cache},
        scene::CollisionScene,
    },
    contact::{
        math::ContactFrame,
        world::{ContactWorld, StaticGeometry},
    },
};

use super::*;

/// The team proxy categories (`MOVER` in `ow174/game/collision.py`).
const CATEGORY: u16 = 0x18;

/// An axis-aligned box as a polytope the way the engine keeps one: planes
/// facing out, half edge twins side by side (2k, 2k + 1) (`hull_box` in
/// `tests/test_game_mover.py`).
fn hull_box(x: [f32; 2], y: [f32; 2], z: [f32; 2]) -> Hull {
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
    let mut edges = vec![[0; 4]; 2 * pairs.len()];
    let (mut faces, mut planes) = (Vec::new(), Vec::new());
    for (face, corner) in loops.into_iter().enumerate() {
        let named: [u8; 4] = std::array::from_fn(|k| half(corner[k], corner[(k + 1) % 4]));
        for (k, &number) in named.iter().enumerate() {
            let twin: i8 = if number % 2 == 0 { 1 } else { -1 };
            edges[usize::from(number)] = [twin as u8, corner[k], face as u8, named[(k + 1) % 4]];
        }
        faces.push(named[0]);
        let axis = face / 2;
        let mut plane = [0.0; 4];
        plane[axis] = if face % 2 == 1 { 1.0 } else { -1.0 };
        plane[3] = corners[usize::from(corner[0])][axis] * plane[axis];
        planes.push(plane);
    }
    Hull {
        mask: 0x1FDF,
        centroid: [0, 1, 2].map(|axis| {
            let [low, high] = [x, y, z][axis];
            (low + high) / 2.0
        }),
        points: corners,
        planes,
        faces,
        half: edges,
    }
}

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

/// A hand-made chunk 0x11: one polytope cube at x 10..11 and two meshes over
/// the same four vertices, the second holding the first's third triangle
/// again (`ChunkTests.chunk`).
fn chunk() -> Vec<u8> {
    let vertices: [[f32; 3]; 4] = [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [5.0; 3]];
    let triangles: [([u32; 3], u16, u16); 3] = [
        ([0, 1, 2], 0x1FDF, 0),
        ([0, 1, 3], 0x04C0, 0),
        ([1, 2, 3], 0x1B9F, 1),
    ];
    let cube = hull_box([10.0, 11.0], [0.0, 1.0], [0.0, 1.0]);
    let mut out = vec![0; 48];
    let polytope_at = out.len();
    out.resize(out.len() + 88, 0);
    let verts_at = out.len();
    out.extend(cube.points.iter().flatten().flat_map(|v| v.to_le_bytes()));
    let planes_at = out.len();
    out.extend(cube.planes.iter().flatten().flat_map(|v| v.to_le_bytes()));
    let faces_at = out.len();
    out.extend(&cube.faces);
    out.resize(out.len().next_multiple_of(4), 0);
    let edges_at = out.len();
    out.extend(cube.half.iter().flatten());
    for (k, offset) in [verts_at, planes_at, edges_at, faces_at]
        .into_iter()
        .enumerate()
    {
        put(
            &mut out,
            polytope_at + 8 * k,
            &(offset as u64).to_le_bytes(),
        );
    }
    for (k, value) in cube.centroid.into_iter().enumerate() {
        put(&mut out, polytope_at + 32 + 4 * k, &value.to_le_bytes());
    }
    for (k, count) in [8u32, 6, 24].into_iter().enumerate() {
        put(&mut out, polytope_at + 44 + 4 * k, &count.to_le_bytes());
    }
    put(&mut out, polytope_at + 76, &0x1FDFu16.to_le_bytes());
    let mesh_at = out.len();
    out.resize(out.len() + 2 * 56, 0);
    let points_at = out.len();
    for point in vertices {
        out.extend(point.into_iter().chain([0.0]).flat_map(f32::to_le_bytes));
    }
    let tris_at = out.len();
    for (corners, mask, surface) in triangles {
        out.extend(corners.into_iter().flat_map(u32::to_le_bytes));
        out.extend([3i32, -1, -1].into_iter().flat_map(i32::to_le_bytes));
        out.extend(
            [0u16, surface, mask, 0]
                .into_iter()
                .flat_map(u16::to_le_bytes),
        );
    }
    for (k, (tris, count)) in [(tris_at, 2u32), (tris_at + 64, 1)].into_iter().enumerate() {
        let base = mesh_at + 56 * k;
        put(&mut out, base + 8, &(points_at as u64).to_le_bytes());
        put(&mut out, base + 16, &(tris as u64).to_le_bytes());
        put(&mut out, base + 28, &4u32.to_le_bytes());
        put(&mut out, base + 32, &count.to_le_bytes());
        put(&mut out, base + 36, &1u32.to_le_bytes());
    }
    put(&mut out, 8, &1u32.to_le_bytes());
    put(&mut out, 12, &2u32.to_le_bytes());
    put(&mut out, 32, &(polytope_at as u64).to_le_bytes());
    put(&mut out, 40, &(mesh_at as u64).to_le_bytes());
    out
}

/// The header's source of a test cache (`{"map": "0x0"}`).
fn source() -> Source {
    Source {
        map: 0,
        chunk: 0,
        ckey: String::new(),
        props: 0,
    }
}

/// A parsed mesh the way the server's reader returns it.
fn read_back((vertices, records): &Mesh) -> cache::Mesh {
    cache::Mesh {
        vertices: vertices.clone(),
        triangles: records
            .iter()
            .map(|&(corners, mask, surface)| cache::Triangle {
                corners: [corners[0], corners[1], corners[2]],
                wings: [corners[3], corners[4], corners[5]],
                mask,
                surface,
            })
            .collect(),
    }
}

/// A parsed polytope the way the server's reader returns it.
fn hull_back(hull: &Hull) -> cache::Hull {
    cache::Hull {
        mask: hull.mask,
        centroid: hull.centroid,
        vertices: hull.points.clone(),
        planes: hull.planes.clone(),
        faces: hull.faces.clone(),
        half_edges: hull
            .half
            .iter()
            .map(|&[twin, origin, face, next]| cache::HalfEdge {
                twin: twin as i8,
                origin,
                face,
                next,
            })
            .collect(),
    }
}

fn kind(geometry: &StaticGeometry) -> &'static str {
    match geometry {
        StaticGeometry::Hull(_) => "hull",
        StaticGeometry::Mesh(_) => "mesh",
        StaticGeometry::Round { .. } => "round",
    }
}

/// `tests/test_game_mover.py::ChunkTests::test_only_what_stops_the_mover_is_kept_and_survives_the_cache`
#[test]
fn only_what_stops_the_mover_is_kept_and_survives_the_cache() {
    let (meshes, hulls) = parse_chunk(&chunk()).unwrap();
    // The 0x04C0 triangle stops only shots; vertex numbers are the kept ones,
    // wings too.
    assert_eq!(
        meshes[0],
        (
            vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [5.0; 3]],
            vec![([0, 1, 2, 3, -1, -1], 0x1FDF, 0)]
        )
    );
    assert_eq!(meshes[1].1, [([0, 1, 2, 2, -1, -1], 0x1B9F, 1)]);
    assert_eq!(
        (
            hulls.len(),
            hulls[0].mask,
            hulls[0].points.len(),
            hulls[0].half.len()
        ),
        (1, 0x1FDF, 8, 24)
    );
    let blob = pack("test", &meshes, &hulls, &source(), &[], &[], &[]).unwrap();
    let cache = read_movement_cache(blob.as_slice()).unwrap();
    assert_eq!(
        cache.meshes,
        meshes.iter().map(read_back).collect::<Vec<_>>()
    );
    assert_eq!(cache.hulls, hulls.iter().map(hull_back).collect::<Vec<_>>());
    assert!(cache.models.is_empty() && cache.placements.is_empty());
    assert_eq!(
        (&cache.header.source["triangles"], cache.header.hulls),
        (&serde_json::json!(2), 1)
    );
    let identifiers = BTreeSet::new();
    let scene = CollisionScene::movement_probe(&cache, &identifiers).unwrap();
    // The top of the polytope cube.
    let hit = scene
        .triangles()
        .ground(10.5, 0.5, 3.0, -1.0, CATEGORY)
        .unwrap();
    assert_eq!((hit.y, hit.normal), (1.0, [0.0, 1.0, 0.0]));
    let world = ContactWorld::from_movement_cache(cache, &identifiers).unwrap();
    let kinds: Vec<_> = world
        .shapes()
        .iter()
        .map(|shape| kind(shape.geometry()))
        .collect();
    assert_eq!(kinds, ["hull", "mesh", "mesh"]); // polytopes first
}

/// The 12 triangles of an axis-aligned box, as a mesh from loose triangles:
/// shared corners merged, each edge's wing the third corner of the other
/// triangle on it (`soup_mesh(box(...))` in `ow174/game/collision.py`).
fn box_mesh(x: [f32; 2], y: [f32; 2], z: [f32; 2], mask: u16) -> Mesh {
    let vertices: Vec<[f32; 3]> = x
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
    let corners: Vec<[i32; 3]> = faces
        .into_iter()
        .flat_map(|[a, b, c, d]| [[a, b, c], [a, c, d]])
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
        .map(|(number, &[a, b, c])| {
            let wings = [(a, b), (b, c), (c, a)].map(|(u, w)| {
                edges[&edge(u, w)]
                    .iter()
                    .find(|&&other| other != number)
                    .and_then(|&other| corners[other].into_iter().find(|&x| x != u && x != w))
                    .unwrap_or(-1)
            });
            ([a, b, c, wings[0], wings[1], wings[2]], mask, 0)
        })
        .collect();
    (vertices, records)
}

/// `tests/test_game_mover.py::ChunkTests::test_a_prop_that_needs_an_identifier_is_kept_only_in_the_modes_that_list_it`
#[test]
fn a_prop_that_needs_an_identifier_is_kept_only_in_the_modes_that_list_it() {
    let floor = box_mesh([-5.0, 5.0], [0.0, 1.0], [-5.0, 5.0], 0x1FDF);
    let crate_model = vec![Shape::Hull(hull_box([0.0, 1.0], [1.0, 2.0], [0.0, 1.0]))];
    // Needs 0x1234.
    let placements: [Placed; 1] = [(0, 0x1234, [0.0; 3], [1.0; 3], [0.0, 0.0, 0.0, 1.0])];
    let blob = pack(
        "test",
        &[floor],
        &[],
        &source(),
        &[crate_model],
        &placements,
        &[0x1234],
    )
    .unwrap();
    assert_eq!(&blob[8..12], &VERSION.to_le_bytes());
    let load = |identifiers: &BTreeSet<u64>| {
        let cache = read_movement_cache(blob.as_slice()).unwrap();
        let scene = CollisionScene::movement_probe(&cache, identifiers).unwrap();
        let world = ContactWorld::from_movement_cache(cache, identifiers).unwrap();
        (scene, world)
    };
    let triangles = |scene: &CollisionScene| {
        scene
            .triangles()
            .nearby([-100.0; 3], [100.0; 3], u16::MAX)
            .len()
    };
    let (scene, world) = load(&BTreeSet::new());
    assert_eq!((triangles(&scene), world.shapes().len()), (12, 1));
    let (scene, world) = load(&BTreeSet::from([0x1234]));
    assert_eq!((triangles(&scene), world.shapes().len()), (24, 2));
    let top = scene.triangles().ground(0.5, 0.5, 3.0, 0.0, CATEGORY);
    assert_eq!(top.map(|hit| hit.y), Some(2.0));
    let prop = &world.shapes()[1];
    assert_eq!(
        (kind(prop.geometry()), prop.frame()),
        (
            "hull",
            Some(ContactFrame {
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
            })
        )
    );
    // A cache of pass 3 is not read; it has to be built again.
    let mut old = blob.clone();
    old[8..12].copy_from_slice(&3u32.to_le_bytes());
    assert!(matches!(
        read_movement_cache(old.as_slice()),
        Err(CacheError::Version(3))
    ));
}
