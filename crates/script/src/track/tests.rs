use super::*;

/// A track is its Bézier segments, measured in their stored lengths: a
/// quarter circle stored as 10 m runs 10 m of track evenly along its arc,
/// facing along the curve; a payload rolls back to the last barrier.
#[test]
fn a_track_follows_its_curves_by_stored_length() {
    // A cubic close to a quarter circle of radius 10 from (10, 0, 0) to
    // (0, 0, 10): handles 0.5523 r long.
    let k = 0.5523 * 10.0;
    let [p0, p1, p2, p3] = [
        [10.0, 0.0, 0.0],
        [10.0, 0.0, k],
        [k, 0.0, 10.0],
        [0.0, 0.0, 10.0],
    ];
    let coefficient = |f: fn(f64, f64, f64, f64) -> f64| -> [f64; 3] {
        std::array::from_fn(|axis| f(p0[axis], p1[axis], p2[axis], p3[axis]))
    };
    let a = coefficient(|p0, p1, p2, p3| -p0 + 3.0 * p1 - 3.0 * p2 + p3);
    let b = coefficient(|p0, p1, p2, _| 3.0 * p0 - 6.0 * p1 + 3.0 * p2);
    let c = coefficient(|p0, p1, _, _| 3.0 * (p1 - p0));
    let points = vec![
        TrackPoint {
            entity: 0x8000_0001,
            position: p0,
            curve: Some([a, b, c]),
            length: Some(10.0),
            barrier: false,
            objective: None,
        },
        TrackPoint {
            entity: 0x8000_0002,
            position: p3,
            curve: Some([[0.0; 3], [0.0; 3], [0.0, 0.0, 5.0]]),
            length: Some(5.0),
            barrier: true,
            objective: Some(0),
        },
        TrackPoint {
            entity: 0x8000_0003,
            position: [0.0, 0.0, 15.0],
            curve: None,
            length: None,
            barrier: false,
            objective: Some(1),
        },
    ];
    let track = Track::new(&points).unwrap();
    assert_eq!(track.length(), 15.0);
    let distances: Vec<f64> = track.waypoints().iter().map(|w| w.distance).collect();
    assert_eq!(distances, [0.0, 10.0, 15.0]);
    let at = |distance: f64| track.position(distance);
    let near = |a: [f64; 3], b: [f64; 3]| super::distance(a, b) < 0.05;
    assert!(near(at(0.0), p0) && near(at(10.0), p3) && near(at(15.0), [0.0, 0.0, 15.0]));
    // Halfway along the stored length is halfway along the arc: 45°.
    let half = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
    assert!(near(at(5.0), [half, 0.0, half]), "{:?}", at(5.0));
    assert!(near(at(12.5), [0.0, 0.0, 12.5]));
    // It faces along the curve: +z at the start, -x at the quarter's end.
    assert!(near(track.direction(0.0), [0.0, 0.0, 1.0]));
    assert!(near(track.direction(9.999), [-1.0, 0.0, 0.0]));
    assert!(near(track.direction(12.5), [0.0, 0.0, 1.0]));
    // The checkpoints by objective order; a rollback's floor.
    assert_eq!(track.checkpoint(1).map(|w| w.entity), Some(0x8000_0003));
    assert_eq!(track.checkpoint(2), None);
    assert_eq!((track.floor(9.0), track.floor(14.0)), (0.0, 10.0));
    // A curve that misses the next waypoint is refused.
    let mut broken = points.clone();
    broken[1].position = [0.0, 0.0, 11.0];
    assert!(Track::new(&broken).is_err());
}
