//! Round props as the probe's triangles, ported from
//! `tests/test_game_mover.py` (`props.prism` and `props.place` in
//! `ow174/game/props.py`).

use super::*;

/// `tests/test_game_mover.py::ChunkTests::test_a_capsule_shape_becomes_a_prism_around_it`
#[test]
fn a_capsule_shape_becomes_a_prism_around_it() {
    let mut triangles = Vec::new();
    let capsule = RoundProp {
        a: [0.0; 3],
        b: [0.0, 1.0, 0.0],
        radius: 0.5,
        mask: 0x18,
    };
    append_prism(&mut triangles, capsule).unwrap();
    let corners: Vec<Vec3> = triangles
        .iter()
        .flat_map(|triangle| triangle.vertices)
        .collect();
    // Its sides 0.5 away.
    let sides = corners
        .iter()
        .map(|[x, _, z]| x.hypot(*z))
        .fold(f64::MIN, f64::max);
    let expected = 0.5 / (std::f64::consts::PI / PRISM_SIDES as f64).cos();
    assert!((sides - expected).abs() < 5e-7, "{sides}");
    let heights = corners.iter().map(|corner| corner[1]);
    let low = heights.clone().fold(f64::MAX, f64::min);
    let high = heights.fold(f64::MIN, f64::max);
    assert_eq!((low, high), (-0.5, 1.5));
    // A quarter turn about up: scaled, turned, moved.
    let quarter = [0.0, 0.5f64.sqrt(), 0.0, 0.5f64.sqrt()];
    let moved = Transform {
        translation: [10.0, 0.0, 0.0],
        scale: [2.0; 3],
        rotation: quarter,
    }
    .place([1.0, 0.0, 0.0]);
    let rounded = moved.map(|axis| (axis * 1e5).round() / 1e5 + 0.0);
    assert_eq!(rounded, [10.0, 0.0, -2.0]);
}
