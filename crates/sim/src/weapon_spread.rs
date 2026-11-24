//! Shot spread for build 104319: the client's random tables, its circular
//! cone, the pellet rings of a volley that fires several projectiles a
//! shot, and the client's fan (`fan_directions`).
//!
//! Ported from leafall903/ORPHEUS (MIT), `src/ow174/game/weapon_spread.py`
//! (commit c239a84). The tables are the client's
//! (`lucy_script::random_tables`).
//! What is known and what is a guess:
//! - The tables (`uniform_table`, `float_table`), the seed arithmetic
//!   (`seed_base`, `seed_index`), the cone (`direction`), the ring layout
//!   (`ring_directions`) and the fan (`fan_directions`) are the client's
//!   (addresses in each item).
//! - Which server values stand for the seed's four identities (owner,
//!   instance, state, counter) is not mapped to the client's: the shot's
//!   body entity, instance id, state node and activation counter are a
//!   reconstruction (`mccree.py` `fan_direction`, `shot_seed`), so a server
//!   shot does not leave along the very direction the client drew where
//!   the layout draws from the tables (the cone, a ring's jitter and drawn
//!   roll). The fan draws nothing.

use lucy_physics::static_ray::Vec3;
use lucy_script::{
    expr::EvalError,
    graph::Node,
    random_tables::{TABLE, float_table, uniform_table},
    vm::ShotEvent,
    weapons::{PelletRings, RADIANS_PER_DEGREE, spread_cone},
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
    let axis = rotate(right_of(forward), forward, f64::from(angle));
    rotate(forward, axis, f64::from(radius)).map(|part| f64::from(part as f32))
}

/// The client's horizontal axis of a launch's layouts (`D80090`, `D808F0`):
/// `forward` × up, normalized, or +x when its squared length is below the
/// client's limit.
fn right_of(forward: Vec3) -> Vec3 {
    let right = cross(forward, [0.0, 1.0, 0.0]);
    // A camera straight up keeps its tiny horizontal axis (cos(π/2) is
    // about 6e-17): only a length below the client's limit falls back.
    let squared = right
        .iter()
        .fold(0.0, |sum, part| sum + f64::from((part * part) as f32)) as f32;
    if f64::from(squared) >= SMALLEST_SQUARED {
        let length = f64::from(squared).sqrt();
        right.map(|part| part / length)
    } else {
        [1.0, 0.0, 0.0]
    }
}

/// A fan launch's shot (`STU_1145C42E`, `lucy_script::weapons::pellet_fan`:
/// `width` and `turn` in radians): its `count` projectiles as the client
/// lays them out (`0x7FF789C908F0`, the launch's slot `+0x30`, which the
/// shot direction `0x7FF789C6EBA0` calls with each pellet's number). The
/// fan's axis is the aim's right (`right_of`) turned `turn` about the aim;
/// pellet `p` is the aim turned about that axis by (2·p/(count − 1) − 1) ·
/// width/2, a lone one by 0. No seed: every client and the server draw
/// the same fan. Float32 at the client's steps; the lift comes after
/// (`lifted`). Genji's (`0304` st11: 3, 9°, 90°) lies level: 4.5° to +x,
/// along the aim, 4.5° to −x for an aim along +z.
pub fn fan_directions(forward: Vec3, count: u8, width: f32, turn: f32) -> Vec<Vec3> {
    let forward = forward.map(|part| f64::from(part as f32));
    let axis =
        rotate(right_of(forward), forward, f64::from(turn)).map(|part| f64::from(part as f32));
    let count = count.max(1);
    (0..count)
        .map(|pellet| {
            let share = if count > 1 {
                f32::from(pellet) / (f32::from(count) - 1.0)
            } else {
                0.5
            };
            let angle = (share + share - 1.0) * (width * 0.5);
            rotate(forward, axis, f64::from(angle)).map(|part| f64::from(part as f32))
        })
        .collect()
}

/// A shot's directions, one per pellet (`pellets`, at least one): `aim`
/// turned by its volley's growing spread (a launch `STU_239B571E` with a
/// spread ramp; `ShotEvent::spread`), at a uniform radius in the cone, each
/// pellet drawn from the table at its own seed index (the client's shot
/// direction `0x7FF789C6EBA0` takes the pellet's number, which the seed
/// `0x7FF789C6F8D0` weighs × 3; the launches `0x7FF7899F43F0` and
/// `0x7FF789C89250` number a shot's pellets 0, 1, …), else `aim` for each.
/// The seed's identities are the shot's body entity, instance, the
/// volley's `m_states` index (the client's seed reads the index written at
/// `1B914DD`, `D74B68`; upstream main `9ed63eed`) and activation counter,
/// and the shot number its ordinal in the volley: upstream's
/// reconstruction (`mccree.py` `fan_direction`, which it uses for McCree's
/// Fan the Hammer alone; here for every such volley). The owner word stays
/// a guess.
pub fn shot_directions(
    shot: &ShotEvent,
    node: &Node,
    aim: Vec3,
    pellets: u8,
    evaluate: impl FnMut(&Json) -> Result<Value, EvalError>,
) -> Result<Vec<Vec3>, EvalError> {
    let pellets = u32::from(pellets.max(1));
    let cone = match (shot.spread, node.state) {
        (Some(normalized), Some(state)) => spread_cone(node, normalized, evaluate)?
            .filter(|cone| *cone > 0.0)
            .map(|cone| (cone, state)),
        _ => None,
    };
    let Some((cone, state)) = cone else {
        return Ok(vec![aim; pellets as usize]);
    };
    let base = shot_seed(shot, state);
    Ok((0..pellets)
        .map(|pellet| direction(aim, cone, base, shot.ordinal, pellet, false))
        .collect())
}

/// `direction` with its launch's lift (`lucy_script::weapons::launch_lift`)
/// added to its up part, normalized, in single precision as the client's
/// shot direction (`0x7FF789C6EBA0`) does it; unchanged without one.
pub fn lifted(direction: Vec3, lift: f64) -> Vec3 {
    if lift == 0.0 {
        return direction;
    }
    let mut raised = direction.map(|part| part as f32);
    raised[1] += lift as f32;
    let length = raised.iter().map(|part| part * part).sum::<f32>().sqrt();
    if !(length.is_finite() && length > 0.0) {
        return direction;
    }
    raised.map(|part| f64::from(part / length))
}

/// The seed of a volley's shot (`seed_base`) from the shot's body entity,
/// instance, the volley's `m_states` index and activation counter:
/// upstream's reconstruction (see `shot_directions`), the owner word a
/// guess.
pub fn shot_seed(shot: &ShotEvent, state: u16) -> u16 {
    seed_base(
        shot.entity,
        u32::from(shot.instance),
        u32::from(state),
        u32::from(shot.activation),
    )
}

/// The client's uniform draw from its table (`0x7FF78A75B1F0`): `low` +
/// the table's value at `index` (14-bit mask) · (`high` − `low`) · 2⁻³²,
/// in float32.
fn uniform(index: usize, low: f32, high: f32) -> f32 {
    uniform_table()[index & INDEX_MASK] as f32 * (high - low) * 2f32.powi(-32) + low
}

/// A ring launch's shot (`STU_613BDDB0`, `lucy_script::weapons::pellet_rings`)
/// of `count` projectiles, as the client lays each out (`0x7FF789C90310`,
/// the launch's slot `+0x30`, called by the shot direction
/// `0x7FF789C6EBA0` with the pellet's number):
/// - pellet `p` falls in the first row whose pellets it hasn't passed (the
///   last row takes any beyond), the `i`-th of its `n`;
/// - its turn around the aim is 2π · i/n plus the roll (`m_391F15AF`, or
///   when unset a draw in [0, 2π) at the table index `base` + 7 · `shot`,
///   the same for every pellet of the shot);
/// - its tilt from the aim is the row's share × max(0, spread − jitter)°,
///   plus a draw in ±jitter at `seed_index(base, shot, p)`;
/// - the aim's right (`right_of`) turned that far around the aim is the
///   axis the aim tilts about; with a jitter over 1.2e-5 rad the result
///   turns again, by a draw in ±jitter at the next index, about aim ×
///   axis (across the ring);
/// - an aspect other than 1 (`PelletRings::aspect`) shrinks the result's
///   part along the aim's up by it (below 1) or along its horizontal side
///   by its inverse (above 1); the shot direction normalizes after the
///   lift (`lifted`).
///
/// So a ring's first pellet, at roll 0, tilts up from the aim, the next
/// ones go round toward −x for an aim along +z. The seed identities are
/// upstream's reconstruction (`shot_seed`), so the jitter and a drawn
/// roll are not the client's own draws; the rest is exact. Float32 at the
/// client's steps.
pub fn ring_directions(
    forward: Vec3,
    count: u8,
    rings: &PelletRings,
    base: u16,
    shot: u32,
) -> Vec<Vec3> {
    let forward = forward.map(|part| f64::from(part as f32));
    let jitter = rings.jitter * RADIANS_PER_DEGREE;
    let roll = rings.roll.unwrap_or_else(|| {
        let index = base.wrapping_add((shot as u16).wrapping_mul(7));
        uniform(usize::from(index), 0.0, std::f32::consts::TAU)
    });
    let shrunk = ((rings.spread - rings.jitter) * RADIANS_PER_DEGREE).max(0.0);
    let right = right_of(forward);
    let aspect = rings.aspect.max(1.192_092_9e-7);
    (0..u32::from(count.max(1)))
        .map(|pellet| {
            let (turn, tilt) = ring_place(&rings.rows, pellet, shrunk);
            let seed = seed_index(base, shot, pellet);
            let toward = uniform(seed, -jitter, jitter);
            let across = uniform(seed + 1, -jitter, jitter);
            let axis =
                rotate(right, forward, f64::from(turn + roll)).map(|part| f64::from(part as f32));
            let mut out = rotate(forward, axis, f64::from(toward + tilt));
            if jitter > 1.192_092_9e-5 {
                let side = unit_or(cross(forward, axis), [0.0, 0.0, 1.0]);
                out = rotate(out, side, f64::from(across));
            }
            if aspect != 1.0 && aspect > 0.0 {
                let flat = unit_or(cross([0.0, 1.0, 0.0], forward), [1.0, 0.0, 0.0]);
                let up = cross(forward, flat);
                let (up_scale, flat_scale) = if aspect > 1.0 {
                    (1.0, 1.0 / aspect)
                } else {
                    (aspect, 1.0)
                };
                let (on_up, on_flat, on_forward) = (
                    dot(out, up) * f64::from(up_scale),
                    dot(out, flat) * f64::from(flat_scale),
                    dot(out, forward),
                );
                out = std::array::from_fn(|axis| {
                    on_up * up[axis] + on_flat * flat[axis] + on_forward * forward[axis]
                });
            }
            out.map(|part| f64::from(part as f32))
        })
        .collect()
}

/// A pellet's turn around the aim (2π · i/n, before the roll) and tilt
/// (the row's share of `shrunk`) in its ring (`0x7FF789C90310`
/// `@0x7ff789c90387`..`@0x7ff789c903eb`); none without rows.
fn ring_place(rows: &[(f32, i32)], pellet: u32, shrunk: f32) -> (f32, f32) {
    let mut index = pellet as i32;
    for (row, &(share, pellets)) in rows.iter().enumerate() {
        if index < pellets || row + 1 == rows.len() {
            let turn = index as f32 / pellets.max(1) as f32 * std::f32::consts::TAU;
            return (turn, shrunk * share);
        }
        index -= pellets;
    }
    (0.0, 0.0)
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `vector` normalized, or `fallback` when its squared length is below
/// the client's limit.
fn unit_or(vector: Vec3, fallback: Vec3) -> Vec3 {
    let squared = dot(vector, vector);
    if squared >= SMALLEST_SQUARED {
        vector.map(|part| part / squared.sqrt())
    } else {
        fallback
    }
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

    fn rings(
        spread: f32,
        jitter: f32,
        roll: Option<f32>,
        rows: &[(f32, i32)],
        aspect: f32,
    ) -> PelletRings {
        PelletRings {
            spread,
            jitter,
            roll,
            rows: rows.to_vec(),
            aspect,
        }
    }

    /// The angle between two directions, well conditioned near 0.
    fn degrees_off(forward: Vec3, direction: Vec3) -> f64 {
        let across = cross(forward, direction);
        dot(across, across)
            .sqrt()
            .atan2(dot(forward, direction))
            .to_degrees()
    }

    /// Doomfist's rings (`0D0A` st2: 1, 4 and 6 pellets at 0, 0.5 and 1 of
    /// 2.5°, roll 0), here without his 0.3° jitter: the client's layout
    /// (`0x7FF789C90310`) leaves at 0°, 1.25° and 2.5°, eleven distinct
    /// unit directions; a ring's first pellet tilts up from the aim, the
    /// next a quarter turn on, toward −x for an aim along +z.
    #[test]
    fn hand_cannon_rings_leave_at_their_angles() {
        let hand_cannon = [(0.0, 1), (0.5, 4), (1.0, 6)];
        let forward = [0.0, 0.0, 1.0];
        let directions = ring_directions(
            forward,
            11,
            &rings(2.5, 0.0, Some(0.0), &hand_cannon, 1.0),
            77,
            3,
        );
        let mut angles: Vec<f64> = directions
            .iter()
            .map(|&direction| (degrees_off(forward, direction) * 1e3).round() / 1e3)
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
        for &direction in &directions {
            assert!((dot(direction, direction) - 1.0).abs() < 1e-6);
        }
        let ring = &directions[1..5];
        assert!(ring[0][1] > 0.0 && ring[0][0].abs() < 1e-6);
        assert!(ring[1][0] < 0.0 && ring[1][1].abs() < 1e-6);
        assert!(ring[2][1] < 0.0 && ring[2][0].abs() < 1e-6);
    }

    /// With his 0.3° jitter the rings shrink to share × 2.2° and each
    /// pellet turns by two draws of at most 0.3°, the same for the same
    /// seed and shot, others for another shot.
    #[test]
    fn a_ring_jitter_shrinks_the_rings_and_moves_each_pellet() {
        let hand_cannon = rings(2.5, 0.3, Some(0.0), &[(0.0, 1), (0.5, 4), (1.0, 6)], 1.0);
        let forward = [0.48, 0.6, -0.64];
        let directions = ring_directions(forward, 11, &hand_cannon, 77, 3);
        let shares = [0.0, 0.5, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        let mut moved = false;
        for (&direction, share) in directions.iter().zip(shares) {
            let off = degrees_off(forward, direction) - share * 2.2;
            assert!(off.abs() <= 0.6, "{off}");
            moved |= off.abs() > 1e-3;
        }
        assert!(moved);
        assert_eq!(
            directions,
            ring_directions(forward, 11, &hand_cannon, 77, 3)
        );
        assert_ne!(
            directions,
            ring_directions(forward, 11, &hand_cannon, 77, 4)
        );
    }

    /// Roadhog's hitscan Scrap Gun shot (`01C6` st14: 16 pellets at 0, 0.5
    /// and 1 of 20°, jitter 2°, no roll, aspect 0.1) lies flat: its part
    /// along the aim's up is a tenth of a cone's, its sides reach out.
    #[test]
    fn an_aspect_below_one_flattens_the_rings() {
        let scrap = rings(20.0, 2.0, None, &[(0.0, 1), (0.5, 5), (1.0, 10)], 0.1);
        for forward in [[0.0, 0.0, 1.0], [0.48, 0.6, -0.64]] {
            let up = cross(
                forward,
                unit_or(cross([0.0, 1.0, 0.0], forward), [1.0, 0.0, 0.0]),
            );
            let directions = ring_directions(forward, 16, &scrap, 512, 9);
            assert_eq!(directions.len(), 16);
            let limit = 0.1 * 22f64.to_radians().sin() + 1e-6;
            for &direction in &directions {
                assert!(dot(direction, up).abs() <= limit);
            }
            let widest = directions
                .iter()
                .map(|&direction| (direction[0] * forward[2] - direction[2] * forward[0]).abs())
                .fold(0.0, f64::max);
            assert!(widest > 0.15, "{widest}");
        }
        // The roll is drawn per shot.
        let forward = [0.0, 0.0, 1.0];
        assert_ne!(
            ring_directions(forward, 16, &scrap, 512, 9),
            ring_directions(forward, 16, &scrap, 512, 10)
        );
    }

    /// Genji's Fan of Blades (`0304` st11: 3 a shot, 9° wide, turned 90°;
    /// the client's `0x7FF789C908F0`): level, 4.5° to +x, along the aim,
    /// 4.5° to −x.
    #[test]
    fn genjis_fan_lies_level_across_the_aim() {
        let (width, turn) = (9.0 * 0.017_453_292f32, 90.0 * 0.017_453_292f32);
        let fan = fan_directions([0.0, 0.0, 1.0], 3, width, turn);
        let (sin, cos) = 4.5f64.to_radians().sin_cos();
        assert_eq!(fan.len(), 3);
        close(fan[0], [sin, 0.0, cos], 1e-6);
        close(fan[1], [0.0, 0.0, 1.0], 1e-7);
        close(fan[2], [-sin, 0.0, cos], 1e-6);
        // Looking up and to the side: the side pellets 4.5° off the aim,
        // mirrored about it, the middle one along it; the fan holds the
        // aim's right (its plane has no part along the aim's up).
        let forward = [0.48, 0.6, -0.64];
        let fan = fan_directions(forward, 3, width, turn);
        let dot = |a: Vec3, b: Vec3| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
        close(fan[1], forward, 1e-6);
        for side in [fan[0], fan[2]] {
            assert!((dot(side, forward) - cos).abs() < 1e-6);
        }
        let right = right_of(forward);
        let up = cross(right, forward);
        for pellet in &fan {
            assert!(dot(*pellet, up).abs() < 1e-6);
        }
        assert!(dot(fan[0], right) < 0.0 && dot(fan[2], right) > 0.0);
        // A lone projectile leaves along the aim.
        close(fan_directions(forward, 1, width, turn)[0], forward, 1e-6);
    }
}
