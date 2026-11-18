//! Shot spread for build 104319: the client's random tables, its circular
//! cone, and the pellet rings of a volley that fires several projectiles a
//! shot.
//!
//! Ported from leafall903/ORPHEUS (MIT), `src/ow174/game/weapon_spread.py`
//! (commit c239a84) and `src/ow174/game/projectiles.py`
//! `launch_directions` (commit 7cb4018). The tables are the client's
//! (`orpheus_script::random_tables`).
//! Upstream's notes on what is known and what is a guess:
//! - The tables (`uniform_table`, `float_table`), the seed arithmetic
//!   (`seed_base`, `seed_index`) and the cone (`direction`) are the
//!   client's (addresses in each item).
//! - Which server values stand for the seed's four identities (owner,
//!   instance, state, counter) is not mapped to the client's: the shot's
//!   body entity, instance id, state node and activation counter are a
//!   reconstruction (`mccree.py` `fan_direction`), so a server shot does
//!   not leave along the very direction the client drew.
//! - The ring layout (`ring_directions`) is upstream's reconstruction, not
//!   the client's: rings `m_6425FFB9` × `m_spread` degrees out, pellets
//!   evenly spaced around each from the right of the aim, no per-shot
//!   rotation.

use std::f64::consts::TAU;

use orpheus_physics::static_ray::Vec3;
use orpheus_script::{
    expr::EvalError,
    graph::Node,
    random_tables::{TABLE, float_table, uniform_table},
    vm::ShotEvent,
    weapons::spread_cone,
    wire::Value,
};
use serde_json::Value as Json;

/// Both 14-bit masks of the seed index.
const INDEX_MASK: usize = TABLE - 1;
/// The client's smallest squared length of a usable axis (`D8025A`).
const SMALLEST_SQUARED: f64 = 1.175_494_350_822_287_5e-36;

/// The seed of a volley's shots (`D5EB70`): owner + instance × 23 + state
/// × 7 + the six-bit counter, each and the sum 16 bits.
pub fn seed_base(owner: u32, instance: u32, state: u32, counter: u32) -> u16 {
    ((owner & 0xffff) + (instance & 0xffff) * 23 + (state & 0xffff) * 7 + (counter & 63)) as u16
}

/// The table index of a shot's pellet (`D5F8D0`/`184B250`, both 14-bit
/// masks).
pub fn seed_index(base: u16, shot: u32, pellet: u32) -> usize {
    let at = (usize::from(base) * 131)
        .wrapping_add(shot as usize * 23)
        .wrapping_add(pellet as usize * 3)
        & INDEX_MASK;
    uniform_table()[at] as usize & INDEX_MASK
}

/// The client's circular spread (`D80090`/`884090`, aspect 1, without the
/// input and muzzle quantization): `forward` turned by an angle of `u ·
/// 2π · 2⁻³²` around it and out by `u · cone · 2⁻³²` radians, or with
/// `gaussian` by the float table's sample · cone / 3. Rounded to float32 at
/// the client's steps.
pub fn direction(
    forward: Vec3,
    cone: f32,
    base: u16,
    shot: u32,
    pellet: u32,
    gaussian: bool,
) -> Vec3 {
    let seed = seed_index(base, shot, pellet);
    let next = (seed + 1) & INDEX_MASK;
    let values = uniform_table();
    let angle = values[seed] as f32 * std::f32::consts::TAU * 2f32.powi(-32);
    let radius = if gaussian {
        float_table()[next] * cone * (1.0f32 / 3.0)
    } else {
        values[next] as f32 * cone * 2f32.powi(-32)
    };
    let forward = forward.map(|part| f64::from(part as f32));
    let right = cross(forward, [0.0, 1.0, 0.0]);
    // A camera straight up keeps its tiny horizontal axis (cos(π/2) is
    // about 6e-17): only a length below the client's limit falls back.
    let squared = right
        .iter()
        .fold(0.0, |sum, part| sum + f64::from((part * part) as f32)) as f32;
    let right = if f64::from(squared) >= SMALLEST_SQUARED {
        let length = f64::from(squared).sqrt();
        right.map(|part| part / length)
    } else {
        [1.0, 0.0, 0.0]
    };
    let axis = rotate(right, forward, f64::from(angle));
    rotate(forward, axis, f64::from(radius)).map(|part| f64::from(part as f32))
}

/// A shot's direction: `aim` turned by its volley's growing spread (a
/// launch `STU_239B571E` with a spread ramp; `ShotEvent::spread`), at a
/// uniform radius in the cone, else `aim`. The seed's identities are the
/// shot's body entity, instance, the volley's `m_states` index (the client's
/// seed reads the index written at `1B914DD`, `D74B68`; upstream main
/// `9ed63eed`) and activation counter, and the shot number its ordinal in
/// the volley: upstream's reconstruction (`mccree.py` `fan_direction`,
/// which it uses for McCree's Fan the Hammer alone; here for every such
/// volley). The owner word stays a guess.
pub fn shot_direction(
    shot: &ShotEvent,
    node: &Node,
    aim: Vec3,
    evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<Vec3, EvalError> {
    let (Some(normalized), Some(state)) = (shot.spread, node.state) else {
        return Ok(aim);
    };
    let Some(cone) = spread_cone(node, normalized, evaluate)?.filter(|cone| *cone > 0.0) else {
        return Ok(aim);
    };
    let base = seed_base(
        shot.entity,
        u32::from(shot.instance),
        u32::from(state),
        u32::from(shot.activation),
    );
    Ok(direction(aim, cone, base, shot.ordinal, 0, false))
}

/// The pellet rings of a shot (upstream 7cb4018 `launch_directions`): for
/// each ring of (tilt in radians, pellets), the pellets evenly spaced
/// around `forward` that far out, the first on its right; a ring of tilt 0
/// fires its pellets along `forward`.
pub fn ring_directions(forward: Vec3, rings: &[(f64, u32)]) -> Vec<Vec3> {
    let [x, y, z] = forward;
    let horizontal = x.hypot(z);
    let right = if horizontal != 0.0 {
        [z / horizontal, 0.0, -x / horizontal]
    } else {
        [1.0, 0.0, 0.0]
    };
    let up = [y * right[2], horizontal, -y * right[0]];
    let mut directions = Vec::with_capacity(rings.iter().map(|&(_, count)| count as usize).sum());
    for &(tilt, pellets) in rings {
        if tilt == 0.0 {
            directions.extend(std::iter::repeat_n(forward, pellets as usize));
            continue;
        }
        let (radial, along) = tilt.sin_cos();
        for index in 0..pellets {
            let (sin, cos) = (TAU * f64::from(index) / f64::from(pellets)).sin_cos();
            let (across, above) = (radial * cos, radial * sin);
            directions.push(std::array::from_fn(|axis| {
                along * forward[axis] + across * right[axis] + above * up[axis]
            }));
        }
    }
    directions
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `vector` turned `angle` radians around the unit `axis` (Rodrigues).
fn rotate(vector: Vec3, axis: Vec3, angle: f64) -> Vec3 {
    let (sin, cos) = angle.sin_cos();
    let dot = axis[0] * vector[0] + axis[1] * vector[1] + axis[2] * vector[2];
    let perpendicular = cross(axis, vector);
    std::array::from_fn(|i| vector[i] * cos + perpendicular[i] * sin + axis[i] * dot * (1.0 - cos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: Vec3, expected: Vec3, tolerance: f64) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() <= tolerance,
                "{actual} vs {expected}"
            );
        }
    }

    /// Upstream's seed arithmetic for a sample identity.
    #[test]
    fn the_seed_masks_each_part() {
        assert_eq!(seed_base(0x12345, 7, 37, 70), 9455);
        assert_eq!(seed_index(77, 3, 1), 2333);
    }

    /// Upstream `test_ninety_degree_camera_keeps_native_tiny_horizontal_basis`:
    /// the client's own result for a camera straight up.
    #[test]
    fn a_camera_straight_up_keeps_its_tiny_horizontal_axis() {
        let vector = direction(
            [0.0, 1.0, std::f64::consts::FRAC_PI_2.cos()],
            6f64.to_radians() as f32,
            284,
            46,
            3,
            false,
        );
        close(
            vector,
            [
                -0.023_236_999_288_201_332,
                0.994_645_178_318_023_7,
                -0.100_702_303_118_343_35,
            ],
            2e-6,
        );
    }

    /// Upstream's generator on sample shots: uniform and gaussian radii.
    #[test]
    fn sample_shots_leave_where_upstream_sends_them() {
        let cone = 6f64.to_radians() as f32;
        let expected = [
            [-0.029_185_872_525, 0.043_395_645_916, 0.998_631_536_960],
            [-0.004_935_056_436, -0.006_017_203_908, 0.999_969_720_840],
            [-0.012_180_025_689, -0.014_533_030_800, 0.999_820_232_391],
            [-0.007_917_299_866, 0.004_439_408_890, 0.999_958_813_190],
        ];
        for (shot, expected) in expected.into_iter().enumerate() {
            let vector = direction([0.0, 0.0, 1.0], cone, 77, shot as u32, 0, shot % 2 == 1);
            close(vector, expected, 1e-6);
        }
        close(
            direction([0.6, 0.0, 0.8], 2f64.to_radians() as f32, 1000, 5, 0, false),
            [0.597_984_790_802, 0.034_275_449_812, 0.800_774_276_256],
            1e-6,
        );
    }

    /// Upstream `test_uniform_samples_remain_inside_authored_six_degree_cone`
    /// and `test_same_event_and_cold_cache_produce_identical_vectors`.
    #[test]
    fn uniform_shots_stay_inside_the_cone_and_repeat() {
        let cone = 6f64.to_radians();
        for forward in [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
        ] {
            for base in 0..128 {
                let vector = direction(forward, cone as f32, base, 0, 0, false);
                let length: f64 = vector.iter().map(|part| part * part).sum();
                assert!((length - 1.0).abs() <= 5e-7);
                let dot: f64 = forward.iter().zip(vector).map(|(a, b)| a * b).sum();
                assert!(dot >= cone.cos() - 1e-7);
            }
        }
        let shots = || -> Vec<Vec3> {
            (0..32)
                .map(|shot| direction([0.0, 0.0, 1.0], cone as f32, 77, shot, 0, shot % 2 == 1))
                .collect()
        };
        let first = shots();
        assert_eq!(first, shots());
        let mut distinct = first.clone();
        distinct.sort_by(|a, b| a.partial_cmp(b).unwrap());
        distinct.dedup();
        assert!(distinct.len() > 24);
    }

    /// Upstream `test_hand_cannon_sweeps_all_eleven_pellets_from_one_shot`:
    /// Doomfist's rings (1, 4 and 6 pellets at 0, 0.5 and 1 of 2.5°) leave
    /// at 0°, 1.25° and 2.5°, eleven distinct unit directions.
    #[test]
    fn hand_cannon_rings_leave_at_their_angles() {
        let spread = 2.5f64;
        let rings = [0.0, 0.5, 1.0].map(|share: f64| (share * spread).to_radians());
        let directions = ring_directions(
            [0.0, 0.0, 1.0],
            &[(rings[0], 1), (rings[1], 4), (rings[2], 6)],
        );
        let mut angles: Vec<f64> = directions
            .iter()
            .map(|[x, y, z]| (x.hypot(*y).atan2(*z).to_degrees() * 1e4).round() / 1e4)
            .collect();
        angles.sort_by(f64::total_cmp);
        let mut expected = vec![0.0];
        expected.extend([1.25; 4]);
        expected.extend([2.5; 6]);
        assert_eq!(angles, expected);
        let mut distinct = directions.clone();
        distinct.sort_by(|a, b| a.partial_cmp(b).unwrap());
        distinct.dedup();
        assert_eq!(distinct.len(), 11);
        for direction in &directions {
            let length: f64 = direction.iter().map(|part| part * part).sum();
            assert!((length - 1.0).abs() < 1e-12);
        }
        // The first pellet of a ring is on the aim's right, the next
        // above: a quarter turn apart for four.
        let ring = &directions[1..5];
        assert!(ring[0][0] > 0.0 && ring[0][1].abs() < 1e-12);
        assert!(ring[1][1] > 0.0 && ring[1][0].abs() < 1e-12);
    }

    /// A ring around an aim looking up and to the side keeps its tilt.
    #[test]
    fn rings_tilt_around_any_aim() {
        let forward = [0.48, 0.6, -0.64];
        let tilt = 5f64.to_radians();
        for direction in ring_directions(forward, &[(tilt, 6)]) {
            let dot: f64 = forward.iter().zip(direction).map(|(a, b)| a * b).sum();
            assert!((dot - tilt.cos()).abs() < 1e-12);
        }
        let straight = ring_directions([0.0, 1.0, 0.0], &[(tilt, 3)]);
        assert_eq!(straight.len(), 3);
    }
}
