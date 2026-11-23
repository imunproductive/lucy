use super::*;

/// A straight cubic from `from` along `along` (a line: `c` only).
fn line(from: Vec3, along: Vec3) -> [Vec3; 4] {
    [[0.0; 3], [0.0; 3], along, from]
}

fn path(mode: u32, orient: bool, segments: Vec<[Vec3; 4]>) -> PathFollower {
    let rotations = vec![[0.0, 0.0, 0.0, 1.0]; segments.len() + 1];
    RawPath {
        speed: 1.0,
        mode,
        offset: 0.0,
        pause: 0.0,
        orient,
        segments,
        rotations,
    }
    .path()
    .unwrap()
}

fn near(got: Vec3, expected: Vec3) {
    for axis in 0..3 {
        assert!((got[axis] - expected[axis]).abs() < 1e-4, "{got:?} {expected:?}");
    }
}

const STILL: Quat = [0.0, 0.0, 0.0, 1.0];

/// A segment's length for the walk ends at `u = 29/30` (the length
/// table's last sample), so a 10 m line is 29/3 m long to the walk and the
/// model crosses all 10 m in that distance.
#[test]
fn a_segment_walks_its_whole_cubic_in_the_length_up_to_29_30() {
    let follower = path(2, false, vec![line([1.0, 2.0, 3.0], [10.0, 0.0, 0.0])]);
    assert!((follower.length - 29.0 / 3.0).abs() < 1e-4, "{}", follower.length);
    let (position, rotation) = follower.place(f64::from(follower.length / 2.0), STILL).unwrap();
    near(position, [6.0, 2.0, 3.0]);
    assert_eq!(rotation, STILL);
    // A lap later it is there again, and the start offset counts in laps.
    let lap = f64::from(follower.length);
    let (again, _) = follower.place(lap + lap / 2.0, STILL).unwrap();
    near(again, [6.0, 2.0, 3.0]);
}

/// Mode 3 runs every other lap backwards: segment by segment from the
/// last, each from its own start, facing back.
#[test]
fn back_and_forth_runs_odd_laps_from_the_last_segment() {
    let follower = path(
        3,
        true,
        vec![
            line([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]),
            line([10.0, 0.0, 0.0], [0.0, 0.0, 10.0]),
        ],
    );
    let lap = f64::from(follower.length);
    let (forward, turned) = follower.place(1.0, STILL).unwrap();
    near(forward, [10.0 / (29.0 / 3.0), 0.0, 0.0]);
    // Facing +x: a quarter turn about y.
    let half = std::f32::consts::FRAC_1_SQRT_2;
    near([turned[1], turned[3], turned[0]], [half, half, 0.0]);
    let (back, facing) = follower.place(lap + 1.0, STILL).unwrap();
    near(back, [10.0, 0.0, 10.0 / (29.0 / 3.0)]);
    // Facing -z: half a turn about y.
    near([facing[1].abs(), facing[3], facing[0]], [1.0, 0.0, 0.0]);
}

/// A pause holds the distance at the lap's end for `speed · pause` before
/// the next lap starts.
#[test]
fn a_pause_holds_the_lap_end() {
    let mut raw_segments = vec![line([0.0; 3], [10.0, 0.0, 0.0])];
    let follower = RawPath {
        speed: 1.0,
        mode: 2,
        offset: 0.0,
        pause: 2.0,
        orient: false,
        segments: std::mem::take(&mut raw_segments),
        rotations: vec![STILL; 2],
    }
    .path()
    .unwrap();
    let length = f64::from(follower.length);
    // In the first 2 s of each lap (its pause) the model stands at the
    // lap's start, then walks.
    let (paused, _) = follower.place(1.0, STILL).unwrap();
    near(paused, [0.0; 3]);
    let (walking, _) = follower.place(3.0, STILL).unwrap();
    near(walking, [10.0 / length as f32, 0.0, 0.0]);
}

/// The waypoints' up axes come from their rotations.
#[test]
fn a_rotation_gives_its_up_axis() {
    near(up_axis(STILL), [0.0, 1.0, 0.0]);
    let half = std::f32::consts::FRAC_1_SQRT_2;
    // A quarter turn about x tips y onto z.
    near(up_axis([half, 0.0, 0.0, half]), [0.0, 0.0, 1.0]);
}
