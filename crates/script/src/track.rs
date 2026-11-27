//! A map entity's parametric track: the map's linked list of waypoints
//! from its `STUParametricMovementComponentInstanceData.m_destination`
//! (`STUWaypointInstanceData`, refs `Payloads.md` §4), as cubic Bézier
//! segments measured in their stored lengths (`m_9741DF92`, about 4%
//! shorter than the curves). Used by the lifts' `2ABA`; the held
//! modegraph5 branch also uses it for payloads.
//!
//! The graphs read and move it through server-only
//! classes the client never runs (`Payloads.md` "ModeGraph4 client
//! pass"): the length (`STU_C5B280AA`), a checkpoint's waypoint, distance
//! and pose (`STU_B252507B`), the waypoints before one (`STU_700B1181`),
//! placing an entity on it (`STU_4A4BEDAA`) and the mover itself
//! (`STU_49FE0019`, `vm/track.rs`). Strong guess (`Payloads.md` §2): a
//! speed is in stored-length metres a second, so on the curve a segment's
//! stored length stretches evenly over its arc.

use std::{collections::BTreeMap, sync::Arc};

/// How many samples of each segment map its stored length to its curve.
const SAMPLES: usize = 32;
/// How far a segment's curve may end from the next waypoint, in metres.
const CURVE_TOLERANCE: f64 = 0.02;

/// A waypoint of a track as the map places it.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackPoint {
    /// Its entity (`0x80000000 | flat index`).
    pub entity: u32,
    pub position: [f64; 3],
    /// The cubic's a, b, c to the next waypoint (d is its position); none
    /// on the last.
    pub curve: Option<[[f64; 3]; 3]>,
    /// The segment's stored length; none on the last.
    pub length: Option<f64>,
    /// `m_E18F822F`: set exactly where the previous link is empty.
    pub barrier: bool,
    /// The objective order (#10548e of its `0F4C`) of a checkpoint.
    pub objective: Option<i64>,
}

/// A waypoint of a track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waypoint {
    /// Its distance along the track.
    pub distance: f64,
    pub entity: u32,
    /// A payload rolling back stops here: barrier waypoints have no
    /// previous link (strong guess, `Payloads.md` §4).
    pub barrier: bool,
    /// The objective order of a checkpoint.
    pub objective: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
struct Segment {
    /// Distance along the track where it starts, and its stored length.
    start: f64,
    length: f64,
    /// a, b, c, d of `a t³ + b t² + c t + d`.
    curve: [[f64; 3]; 4],
    /// The arc length from its start at t = k / SAMPLES.
    arc: Vec<f64>,
}

impl Segment {
    fn at(&self, t: f64) -> [f64; 3] {
        let [a, b, c, d] = self.curve;
        std::array::from_fn(|axis| ((a[axis] * t + b[axis]) * t + c[axis]) * t + d[axis])
    }

    /// The curve's derivative at `t`.
    fn slope(&self, t: f64) -> [f64; 3] {
        let [a, b, c, _] = self.curve;
        std::array::from_fn(|axis| (3.0 * a[axis] * t + 2.0 * b[axis]) * t + c[axis])
    }

    /// The parameter a share `fraction` of its arc along.
    fn parameter(&self, fraction: f64) -> f64 {
        let total = self.arc[SAMPLES];
        let target = fraction.clamp(0.0, 1.0) * total;
        let k = self
            .arc
            .windows(2)
            .position(|pair| target <= pair[1])
            .unwrap_or(SAMPLES - 1);
        let (from, to) = (self.arc[k], self.arc[k + 1]);
        let within = if to > from {
            (target - from) / (to - from)
        } else {
            0.0
        };
        (k as f64 + within) / SAMPLES as f64
    }
}

/// A payload's track.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The first waypoint's position.
    origin: [f64; 3],
    segments: Vec<Segment>,
    waypoints: Vec<Waypoint>,
}

/// The tracks of a map's payloads, by the payload's entity.
pub type Tracks = BTreeMap<u32, Arc<Track>>;

impl Track {
    pub fn new(points: &[TrackPoint]) -> Result<Self, String> {
        let (last, inner) = points.split_last().ok_or("a track without waypoints")?;
        if last.curve.is_some() {
            return Err(format!("the last waypoint {:08X} has a curve", last.entity));
        }
        let mut segments = Vec::new();
        let mut start = 0.0;
        for (point, next) in inner.iter().zip(&points[1..]) {
            let (Some([a, b, c]), Some(length)) = (point.curve, point.length) else {
                return Err(format!("waypoint {:08X} has no curve", point.entity));
            };
            if !(length.is_finite() && length > 0.0) {
                return Err(format!(
                    "waypoint {:08X}'s segment is {length} m",
                    point.entity
                ));
            }
            let mut segment = Segment {
                start,
                length,
                curve: [a, b, c, point.position],
                arc: vec![0.0],
            };
            let mut previous = segment.at(0.0);
            for k in 1..=SAMPLES {
                let here = segment.at(k as f64 / SAMPLES as f64);
                segment
                    .arc
                    .push(segment.arc[k - 1] + distance(previous, here));
                previous = here;
            }
            if distance(previous, next.position) > CURVE_TOLERANCE {
                return Err(format!(
                    "waypoint {:08X}'s curve ends at {previous:?}, not at the next waypoint",
                    point.entity
                ));
            }
            segments.push(segment);
            start += length;
        }
        let mut along = 0.0;
        let waypoints = points
            .iter()
            .map(|point| {
                let at = along;
                along += point.length.unwrap_or_default();
                Waypoint {
                    distance: at,
                    entity: point.entity,
                    barrier: point.barrier,
                    objective: point.objective,
                }
            })
            .collect();
        Ok(Self {
            origin: points[0].position,
            segments,
            waypoints,
        })
    }

    /// Its stored length.
    pub fn length(&self) -> f64 {
        self.segments
            .last()
            .map_or(0.0, |segment| segment.start + segment.length)
    }

    pub fn waypoints(&self) -> &[Waypoint] {
        &self.waypoints
    }

    /// The waypoint of the checkpoint of objective order `objective`.
    pub fn checkpoint(&self, objective: i64) -> Option<&Waypoint> {
        self.waypoints
            .iter()
            .find(|waypoint| waypoint.objective == Some(objective))
    }

    /// The waypoint of entity `entity`.
    pub fn waypoint(&self, entity: u32) -> Option<&Waypoint> {
        self.waypoints
            .iter()
            .find(|waypoint| waypoint.entity == entity)
    }

    /// How far back from `distance` a payload rolls: the last barrier at
    /// or before it, else the start.
    pub fn floor(&self, distance: f64) -> f64 {
        self.waypoints
            .iter()
            .filter(|waypoint| waypoint.barrier && waypoint.distance <= distance)
            .map(|waypoint| waypoint.distance)
            .fold(0.0, f64::max)
    }

    /// The segment and its curve parameter at `distance`.
    fn locate(&self, distance: f64) -> Option<(&Segment, f64)> {
        let first = self.segments.first()?;
        let segment = self
            .segments
            .iter()
            .rev()
            .find(|segment| segment.start <= distance)
            .unwrap_or(first);
        Some((
            segment,
            segment.parameter((distance - segment.start) / segment.length),
        ))
    }

    /// Where the payload stands at `distance` along it.
    pub fn position(&self, distance: f64) -> [f64; 3] {
        match self.locate(distance) {
            Some((segment, t)) => segment.at(t),
            None => self.origin,
        }
    }

    /// The unit direction of the track at `distance` (the way forward);
    /// +z where the curve has none.
    pub fn direction(&self, distance: f64) -> [f64; 3] {
        let slope = self
            .locate(distance)
            .map_or([0.0; 3], |(segment, t)| segment.slope(t));
        let length = slope.iter().map(|part| part * part).sum::<f64>().sqrt();
        if length > 1e-9 {
            slope.map(|part| part / length)
        } else {
            [0.0, 0.0, 1.0]
        }
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3)
        .map(|axis| (a[axis] - b[axis]).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests;
