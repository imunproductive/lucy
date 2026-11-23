//! The path followers of server placeables (client component 98,
//! `STU_66A032BC`): models the client walks along a path of waypoint
//! entities by itself, among them the Practice Range's four platforms
//! `0516`, Volskaya's and Route 66's flying platforms and Rialto's boats
//! (`game_map_areas_174.json` `server_placeables` `path`, derived by
//! `lucy-assets` `map_areas`; refs `MapEntities.md` "MovingMaps").
//!
//! The client (static reading of the 1.74 client, addresses at
//! 0x7FF788F10000):
//! - The path (`0x7FF789AF3BC0` → `0x7FF789AF4210`, once): from the
//!   placeable's first waypoint each waypoint (component 38,
//!   `STUWaypointInstanceData`) gives one cubic `a u³ + b u² + c u + d`
//!   to the next, while the next has a next itself, and every waypoint
//!   gives its rotation's up axis (`0x7FF78A75BC70`). A segment's length
//!   table (`0x7FF789AF3FD0`) holds 30 trapezoid sums of the speed
//!   `|3a u² + 2b u + c|` at `u = k/30` (so the last is the length up to
//!   `u = 29/30`, the segment's length for the walk), and its parameter
//!   table (`0x7FF789AF3F60`, `0x7FF789AF52F0`) the `u` of 30 evenly
//!   spaced shares of that length, each found by 100 halvings of the
//!   length table read as a function of `u`. The path's length is the
//!   segments' lengths summed in order.
//! - Each frame its system (`0x7FF7896B3D40`) takes the time of the
//!   frame it is given from the world's clock (`0x7FF789D28880`: the
//!   frame's start less the clock's base and the interpolation delay, the
//!   same delay the idle animations' clock takes off) and places the
//!   model there (`0x7FF789AF3400`): the distance `time · speed`, less the
//!   pauses at the end of each lap (`pause` s, only the walk's velocity
//!   stops), plus `offset` laps, the mode's lap rule (0 and 2 loop, 3
//!   goes back and forth, 1 stops at the end: no placeable uses it), the
//!   segment the distance falls in, the segment's `u` from its parameter
//!   table and the cubic there. With the turn flag the model's forward
//!   axis follows the cubic's direction and its up axis the waypoints' up
//!   axes, blended along the segment (`0x7FF789AF5E60`, `0x7FF78A75BA40`);
//!   without it the model keeps the placeable's rotation. The position
//!   and the rotation go into the entity's transform (component 1), and
//!   its physics bodies with it.
//!
//! Every value is float32 in the client's order of operations; its
//! `rsqrtss`/`rsqrtps` estimates are taken correctly rounded (they differ
//! between CPUs), followed by the client's Newton step.

use serde::Deserialize;

type Vec3 = [f32; 3];
type Quat = [f32; 4];

/// The samples of a segment's tables.
const SAMPLES: usize = 30;
/// 1/30 (`0x7FF78B51FD50`) and 1/60 (`0x7FF78B520894`): the length
/// table's step in `u` and its trapezoid weight.
const STEP: f32 = 0.033_333_335;
const HALF_STEP: f32 = 0.016_666_668;
/// 1/29 (`0x7FF78B568280`).
const SHARE: f32 = 0.034_482_758;
/// The squared length below which a vector has none (`0x7FF78B4F1860`).
const TINY: f32 = 1.175_494_4e-36;
/// A distance this close to the path's length is the next lap's start
/// (`0x7FF78B509D70`).
const LAP_SLACK: f32 = 0.004;
/// The least length the parameter lookup divides by (`0x7FF78B506020`).
const MIN_LENGTH: f32 = 0.1;
/// The halvings of a parameter search (`0x7FF789AF52F0`: 20 rounds of 5).
const HALVINGS: usize = 100;

/// A path as `game_map_areas_174.json` keeps it.
#[derive(Deserialize)]
pub(crate) struct RawPath {
    speed: f32,
    mode: u32,
    offset: f32,
    pause: f32,
    orient: bool,
    /// Each segment's cubic coefficients `a`, `b`, `c`, `d`.
    segments: Vec<[Vec3; 4]>,
    /// Each waypoint's rotation, one more than the segments.
    rotations: Vec<Quat>,
}

/// How a path follower runs its laps (`STU_C12A012E` `m_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// 0: lap after lap (`0x7FF789AF59D0`).
    Loop,
    /// 2: lap after lap (`0x7FF789AF5620`; turns toward the next
    /// waypoint, the first after the last).
    Cycle,
    /// 3: every other lap backwards, segment by segment from the last
    /// (`0x7FF789AF57B0`).
    BackAndForth,
}

/// One segment: its cubic and its tables (see the module notes).
#[derive(Debug)]
struct Segment {
    cubic: [Vec3; 4],
    lengths: [f32; SAMPLES],
    params: [f32; SAMPLES],
}

/// A placeable's path (see the module notes).
#[derive(Debug)]
pub struct PathFollower {
    speed: f32,
    mode: Mode,
    offset: f32,
    pause: f32,
    orient: bool,
    segments: Vec<Segment>,
    /// Each waypoint's up axis.
    ups: Vec<Vec3>,
    /// The path's length.
    length: f32,
}

impl RawPath {
    pub(crate) fn path(self) -> Result<PathFollower, String> {
        let mode = match self.mode {
            0 => Mode::Loop,
            2 => Mode::Cycle,
            3 => Mode::BackAndForth,
            other => return Err(format!("a path of mode {other}")),
        };
        if self.rotations.len() != self.segments.len() + 1 {
            return Err(format!(
                "a path of {} segments and {} waypoint rotations",
                self.segments.len(),
                self.rotations.len()
            ));
        }
        let segments: Vec<Segment> = self.segments.into_iter().map(Segment::new).collect();
        let length = segments
            .iter()
            .fold(0.0, |length, segment| segment.length() + length);
        Ok(PathFollower {
            speed: self.speed,
            mode,
            offset: self.offset,
            pause: self.pause,
            orient: self.orient,
            segments,
            ups: self.rotations.into_iter().map(up_axis).collect(),
            length,
        })
    }
}

impl PathFollower {
    /// Where the client puts the model at the clock's time `time`: its
    /// position and rotation (`rotation`, the placeable's, unless the
    /// path turns it); `None` for a path of no length, which leaves the
    /// model where the map puts it.
    pub fn place(&self, time: f64, rotation: Quat) -> Option<(Vec3, Quat)> {
        let length = self.length;
        if !(length > 0.0) {
            return None;
        }
        let mut distance = time as f32 * self.speed;
        if !distance.is_finite() {
            return None;
        }
        if self.pause > 0.0 {
            let paused = self.speed * self.pause;
            let lap = length + paused;
            let laps = distance / lap;
            let whole = laps as i32;
            let floor = if laps < whole as f32 { whole - 1 } else { whole };
            let rest = distance - whole as f32 * lap;
            let start = floor as f32 * length;
            distance = if rest < paused {
                start
            } else {
                (start + rest) - paused
            };
        }
        distance = distance + length * self.offset;
        let mut laps = 0u32;
        while !((distance - length).abs() > LAP_SLACK && distance <= length) {
            distance = distance - length;
            laps += 1;
        }
        let backwards = self.mode == Mode::BackAndForth && laps % 2 == 1;
        let last = self.segments.len() - 1;
        let mut index = if backwards { last } else { 0 };
        while self.segments[index].length() < distance {
            let next = if backwards {
                index.checked_sub(1)
            } else {
                Some(index + 1).filter(|&next| next <= last)
            };
            let Some(next) = next else {
                break;
            };
            distance = distance - self.segments[index].length();
            index = next;
        }
        let segment = &self.segments[index];
        let (position, direction) = segment.at(distance);
        let Some(mut forward) = unit(direction) else {
            return Some((position, rotation));
        };
        if backwards {
            forward = forward.map(|value| -value);
        }
        if !self.orient {
            return Some((position, rotation));
        }
        let mut share = distance / segment.length();
        let (from, to) = match self.mode {
            Mode::Loop => (
                index,
                if index + 1 < self.ups.len() {
                    index + 1
                } else {
                    index
                },
            ),
            Mode::Cycle => (index, (index + 1) % self.segments.len()),
            Mode::BackAndForth if backwards => {
                share = 1.0 - share;
                (index + 1, index)
            }
            Mode::BackAndForth => (index, index + 1),
        };
        let turned = blended_up(self.ups[from], self.ups[to], share, forward)
            .map_or(rotation, |up| from_axes(forward, up));
        Some((position, turned))
    }
}

impl Segment {
    fn new(cubic: [Vec3; 4]) -> Self {
        let [a, b, c, _] = cubic;
        let mut lengths = [0.0; SAMPLES];
        let mut speed = client_length(c);
        let mut u = STEP;
        for k in 1..SAMPLES {
            let direction: Vec3 =
                std::array::from_fn(|axis| ((3.0 * a[axis]) * u) * u + (2.0 * b[axis]) * u + c[axis]);
            let next = client_length(direction);
            u += STEP;
            lengths[k] = (speed * STEP + lengths[k - 1]) + (next - speed) * HALF_STEP;
            speed = next;
        }
        let length = lengths[SAMPLES - 1];
        let params = std::array::from_fn(|k| search(&lengths, length * (k as f32 * SHARE)));
        Self {
            cubic,
            lengths,
            params,
        }
    }

    /// Its length for the walk: the length table's last sum.
    fn length(&self) -> f32 {
        self.lengths[SAMPLES - 1]
    }

    /// The cubic's point and direction `distance` along it
    /// (`0x7FF789AF3300`, `0x7FF789AF3E40`).
    fn at(&self, distance: f32) -> (Vec3, Vec3) {
        let u = lerp_table(&self.params, distance * (29.0 / MIN_LENGTH.max_sse(self.length())));
        let [a, b, c, d] = self.cubic;
        let position = std::array::from_fn(|axis| {
            ((((a[axis] * u) * u) * u + (b[axis] * u) * u) + c[axis] * u) + d[axis]
        });
        let direction =
            std::array::from_fn(|axis| ((3.0 * a[axis]) * u) * u + (2.0 * b[axis]) * u + c[axis]);
        (position, direction)
    }
}

/// `0x7FF789AF52F0`: the `u` at which the length table, read as a
/// function of `u`, reaches `target`, by halving `[0, 1]`: the last
/// midpoint tried.
fn search(lengths: &[f32; SAMPLES], target: f32) -> f32 {
    let (mut low, mut high) = (0.0f32, 1.0f32);
    let mut middle = 0.0;
    for _ in 0..HALVINGS {
        middle = (high + low) * 0.5;
        if lerp_table(lengths, middle * 29.0) > target {
            high = middle;
        } else {
            low = middle;
        }
    }
    middle
}

/// A table of 30 read at `at` (in samples) between its two nearest
/// samples, clamped to the table (the client's reads of both tables).
fn lerp_table(table: &[f32; SAMPLES], at: f32) -> f32 {
    let whole = at as i32;
    let clamp = |index: i32| index.clamp(0, SAMPLES as i32 - 1) as usize;
    let fraction = at - whole as f32;
    (1.0 - fraction) * table[clamp(whole)] + fraction * table[clamp(whole + 1)]
}

/// The SSE `maxss`: the second operand unless the first is greater.
trait MaxSse {
    fn max_sse(self, other: f32) -> f32;
}

impl MaxSse for f32 {
    fn max_sse(self, other: f32) -> f32 {
        if self > other { self } else { other }
    }
}

/// The correctly rounded `rsqrtss` estimate.
fn estimate(value: f32) -> f32 {
    (1.0 / f64::from(value).sqrt()) as f32
}

/// `value`'s inverse square root as the client refines its estimate:
/// `(3 − r² v) · (r / 2)`.
fn inverse_root(value: f32) -> f32 {
    let r = estimate(value);
    (3.0 - (r * r) * value) * (0.5 * r)
}

/// A length as the scalar code takes it (`0x7FF789AF3FD0`): none below
/// [`TINY`], else `v · ((3 − v r²) · (r / 2))` of its square `v`, summed
/// `(y² + x²) + z²`.
fn client_length([x, y, z]: Vec3) -> f32 {
    let squared = (y * y + x * x) + z * z;
    if squared > TINY {
        let r = estimate(squared);
        squared * ((3.0 - squared * (r * r)) * (r * 0.5))
    } else {
        0.0
    }
}

/// `0x7FF789503B30`: the unit vector, none below [`TINY`] (summed
/// `(x² + y²) + z²`).
fn unit(vector: Vec3) -> Option<Vec3> {
    let [x, y, z] = vector;
    let squared = (x * x + y * y) + z * z;
    (squared >= TINY).then(|| {
        let scale = inverse_root(squared);
        vector.map(|value| value * scale)
    })
}

/// A vector made unit with the vector code's sum `(y² + x²) + z²`.
fn unit_yx(vector: Vec3) -> Vec3 {
    let [x, y, z] = vector;
    let scale = inverse_root((y * y + x * x) + z * z);
    vector.map(|value| value * scale)
}

/// `a × b` as the SSE shuffles compute it.
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Whether a vector is shorter than [`TINY`] (summed `(x² + y²) + z²`).
fn tiny([x, y, z]: Vec3) -> bool {
    (x * x + y * y) + z * z < TINY
}

/// `0x7FF78A75BC70`: a rotation's up axis (its image of `y`).
fn up_axis([x, y, z, w]: Quat) -> Vec3 {
    let (x2, y2, z2) = (x + x, y + y, z + z);
    [
        x * y2 - w * z2,
        (-(x * x2) - z * z2) + 1.0,
        y * z2 - -(w * x2),
    ]
}

/// `0x7FF789AF5E60`: the up axis at `share` of a segment from up `from`
/// to up `to` (none blends to `y`), made square to `forward`; none when
/// `forward` runs along it.
fn blended_up(from: Vec3, to: Vec3, share: f32, forward: Vec3) -> Option<Vec3> {
    let rest = 1.0 - share;
    let blended: Vec3 = std::array::from_fn(|axis| rest * from[axis] + share * to[axis]);
    let up = if tiny(blended) {
        [0.0, 1.0, 0.0]
    } else {
        unit_yx(blended)
    };
    let side = cross(forward, up);
    if tiny(side) {
        return None;
    }
    let square = cross(unit_yx(side), forward);
    if tiny(square) {
        return None;
    }
    Some(unit_yx(square))
}

/// `0x7FF78A75BA40` and `0x7FF78A75BAA0`: the rotation whose rows are
/// `up × forward`, `up` and `forward`, made unit (`0x7FF789503AA0`).
fn from_axes(forward: Vec3, up: Vec3) -> Quat {
    let side = cross(up, forward);
    let (r00, r11, r22) = (side[0], up[1], forward[2]);
    let trace = ((r11 + r00) + r22) + 1.0;
    let quaternion = if trace >= 1.0 {
        [
            up[2] - forward[1],
            forward[0] - side[2],
            side[1] - up[0],
            trace,
        ]
    } else if r00 > r11 && r00 > r22 {
        [
            ((r00 - r11) - r22) + 1.0,
            up[0] + side[1],
            side[2] + forward[0],
            up[2] - forward[1],
        ]
    } else if r11 > r22 {
        [
            up[0] + side[1],
            ((r11 - r22) - r00) + 1.0,
            forward[1] + up[2],
            forward[0] - side[2],
        ]
    } else {
        [
            side[2] + forward[0],
            forward[1] + up[2],
            ((r22 - r00) - r11) + 1.0,
            side[1] - up[0],
        ]
    };
    normalized(quaternion)
}

/// `0x7FF789503AA0`: the unit quaternion (summed by two horizontal adds),
/// the identity below [`TINY`].
fn normalized(q: Quat) -> Quat {
    let [x, y, z, w] = q;
    if ((x * x + y * y) + z * z) + w * w < TINY {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let scale = inverse_root((x * x + y * y) + (z * z + w * w));
    q.map(|value| value * scale)
}

#[cfg(test)]
mod tests;
