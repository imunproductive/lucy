//! The objectives of the PvP modes as the server runs them: capture points,
//! the payload and the Control point (docs/notes/game-rules.md §3-7).
//!
//! The retail objectives are map entities whose statescript (graphs 0F4C,
//! 0E8E, 1225, 121E) works with native components the server doesn't run.
//! The server plays their graphs' rules (refs reports `CaptureAreas.md`
//! and `Payloads.md`) on the map's derived data (`objective_data.rs`): the
//! points' areas and times, the payload's track. Each value is read from
//! the install; the guesses are named where they are used.

use orpheus_content::maps::MapObjective;
use serde::Deserialize;

use crate::{
    mode_graph_values::CaptureRates,
    mode_rules_data::{ControlScoring, MARK_TOLERANCE, PayloadRules},
    objective_data::{PayloadData, PointData, WaypointData, WaypointEvent},
};

/// Who stands on an objective.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Presence {
    pub attackers: usize,
    pub defenders: usize,
}

/// A player as the objectives see him: his team and, while he is alive in
/// the world, where his body stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Participant {
    pub slot: usize,
    pub team: u8,
    pub position: Option<[f64; 3]>,
}

impl Participant {
    pub fn alive(&self) -> bool {
        self.position.is_some()
    }
}

/// One box of a capture area (an area placeable of map chunk kind 0x0A):
/// its world centre, half extents along its own axes, and its rotation
/// `{x, y, z, w}` from those axes to the world's.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct CaptureBox {
    pub centre: [f64; 3],
    pub half: [f64; 3],
    pub rotation: [f64; 4],
}

impl CaptureBox {
    pub fn contains(&self, position: [f64; 3]) -> bool {
        let offset: [f64; 3] = std::array::from_fn(|axis| position[axis] - self.centre[axis]);
        let [x, y, z, w] = self.rotation;
        // Into the box's axes: the inverse rotation, v + 2w(u×v) + 2u×(u×v)
        // with u the conjugate's vector part.
        let local = rotate([-x, -y, -z], w, offset);
        local
            .iter()
            .zip(self.half)
            .all(|(along, half)| along.abs() <= half)
    }

    /// A box with a size and a unit rotation.
    pub(crate) fn check(&self) -> Result<(), String> {
        let norm = self.rotation.iter().map(|part| part * part).sum::<f64>();
        if !self.half.iter().all(|half| half.is_finite() && *half > 0.0) {
            return Err(format!("a box's half extents are {:?}", self.half));
        }
        if (norm - 1.0).abs() > 1e-3 {
            return Err(format!(
                "a box's rotation {:?} is not a unit",
                self.rotation
            ));
        }
        Ok(())
    }
}

/// `v` rotated by the unit quaternion `(u, w)`.
fn rotate(u: [f64; 3], w: f64, v: [f64; 3]) -> [f64; 3] {
    let t = cross(u, v).map(|part| 2.0 * part);
    let u_t = cross(u, t);
    std::array::from_fn(|axis| v[axis] + w * t[axis] + u_t[axis])
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Where a player counts as on an objective.
#[derive(Clone, Debug, PartialEq)]
pub enum Area {
    /// A capture point's area: the union of the boxes of the area placeable
    /// the point's `STUTargetTagInstanceData.m_2746D7E4` names. Strong guess
    /// (`CaptureAreas.md` §3): the boxes fit the points' floor rings, but no
    /// reader of the field was seen.
    Boxes(Vec<CaptureBox>),
    /// The payload's range: 1225's TrackTargets find players within a
    /// radius of a point (refs `Payloads.md` §3; a sphere is a strong guess
    /// from their 360° angles).
    Sphere { centre: [f64; 3], radius: f64 },
}

impl Area {
    pub fn contains(&self, position: [f64; 3]) -> bool {
        match self {
            Self::Boxes(boxes) => boxes.iter().any(|area| area.contains(position)),
            Self::Sphere { centre, radius } => {
                let squared: f64 = (0..3)
                    .map(|axis| (position[axis] - centre[axis]).powi(2))
                    .sum();
                squared <= radius * radius
            }
        }
    }

    /// The alive players of `attackers` and of the other teams inside.
    pub fn presence(&self, participants: &[Participant], attackers: u8) -> Presence {
        let mut presence = Presence::default();
        for participant in participants {
            if participant
                .position
                .is_some_and(|position| self.contains(position))
            {
                if participant.team == attackers {
                    presence.attackers += 1;
                } else {
                    presence.defenders += 1;
                }
            }
        }
        presence
    }

    /// A point's boxes where its objective entity stands: the data places
    /// them for the entity at `point.position`; a map that moves the entity
    /// (as a test may) moves its area with it.
    pub fn of_point(point: &PointData, at: [f64; 3]) -> Self {
        let shift: [f64; 3] = std::array::from_fn(|axis| at[axis] - point.position[axis]);
        Self::Boxes(
            point
                .boxes
                .iter()
                .map(|area| CaptureBox {
                    centre: std::array::from_fn(|axis| area.centre[axis] + shift[axis]),
                    ..*area
                })
                .collect(),
        )
    }
}

/// An Assault or Hybrid capture point (graph 0E8E, rules 32-35). Progress
/// runs 0 to 1; its marks (`ModeRules::capture_marks`, 1/3 and 2/3) are
/// ticks it never falls below.
///
/// Assumption: every attacker can advance it. 0E8E lets an attacker with
/// entity variable v10414 (set by some hero abilities, probably the phased
/// states) count but not advance it alone; the server sets no v10414.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturePoint {
    pub area: Area,
    pub position: [f64; 3],
    pub name: Option<String>,
    /// The base capture and decay seconds (0E8E v12395, v12396).
    capture: f64,
    decay: f64,
    rates: CaptureRates,
    marks: [f64; 2],
    progress: f64,
    /// Seconds since the last attacker left it.
    empty: f64,
    captured: bool,
}

impl CapturePoint {
    pub fn new(
        objective: &MapObjective,
        point: &PointData,
        rates: CaptureRates,
        marks: [f64; 2],
    ) -> Self {
        Self {
            area: Area::of_point(point, objective.position),
            position: objective.position,
            name: objective.name.clone(),
            capture: point.capture,
            decay: point.decay,
            rates,
            marks,
            progress: 0.0,
            empty: 0.0,
            captured: false,
        }
    }

    pub fn progress(&self) -> f64 {
        self.progress
    }

    pub fn captured(&self) -> bool {
        self.captured
    }

    /// The last mark its progress reached (rule 35): 0 before the first, 1
    /// once captured.
    pub fn mark(&self) -> f64 {
        if self.captured {
            return 1.0;
        }
        self.marks
            .iter()
            .rev()
            .copied()
            .find(|&mark| self.progress >= mark - MARK_TOLERANCE)
            .unwrap_or(0.0)
    }

    /// One step of `seconds` (already on the match's time scale) with
    /// `presence` on the point; true when it was captured in this step.
    /// 0E8E: attackers alone raise it at their count's rate over the base
    /// capture seconds (uid 154); attackers and defenders together hold it;
    /// once no attacker has been on it for the decay delay it falls at the
    /// defenders' count's rate over the base decay seconds (uids 181, 175)
    /// to its last tick (uid 302).
    pub fn step(&mut self, seconds: f64, scale: f64, presence: Presence) -> bool {
        if self.captured {
            return false;
        }
        if presence.attackers > 0 {
            self.empty = 0.0;
            if presence.defenders == 0 {
                let rate = self.rates.capture(presence.attackers) / (self.capture * scale);
                self.progress += rate * seconds;
                if self.progress >= 1.0 - 1e-9 {
                    self.progress = 1.0;
                    self.captured = true;
                    return true;
                }
            }
            return false;
        }
        self.empty += seconds;
        if self.empty >= self.rates.decay_delay * scale {
            let floor = self.mark();
            let rate = self.rates.decay(presence.defenders) / (self.decay * scale);
            self.progress = (self.progress - rate * seconds).max(floor);
        }
        false
    }
}

/// How many samples of each track segment map its stored length to its
/// curve.
const SAMPLES: usize = 32;
/// How far a segment's curve may end from the next waypoint, in metres.
const CURVE_TOLERANCE: f64 = 0.02;

/// A payload's track: the cubic Bézier segments between its waypoints,
/// measured in their stored lengths (`m_9741DF92`, about 4% shorter than
/// the curves). Strong guess (`Payloads.md` §2): the payload moves its
/// speed in stored-length metres a second, so on the curve a segment's
/// stored length stretches evenly over its arc.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    segments: Vec<Segment>,
    waypoints: Vec<Waypoint>,
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

    /// The point a share `fraction` of its arc along.
    fn along(&self, fraction: f64) -> [f64; 3] {
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
        self.at((k as f64 + within) / SAMPLES as f64)
    }
}

/// A waypoint of a track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waypoint {
    /// Its distance along the track.
    pub distance: f64,
    pub placeable: u32,
    /// Rollback stops here: barrier waypoints have no previous link
    /// (strong guess, `Payloads.md` §4).
    pub barrier: bool,
    /// Graph 121E's answer when the payload reaches it.
    pub event: Option<WaypointEvent>,
}

impl Track {
    pub fn new(waypoints: &[WaypointData]) -> Result<Self, String> {
        let (last, inner) = waypoints.split_last().ok_or("a track without waypoints")?;
        if last.curve.is_some() {
            return Err(format!("the last waypoint {} has a curve", last.placeable));
        }
        let mut segments = Vec::new();
        let mut start = 0.0;
        for (waypoint, next) in inner.iter().zip(&waypoints[1..]) {
            let (Some([a, b, c]), Some(length)) = (waypoint.curve, waypoint.length) else {
                return Err(format!("waypoint {} has no curve", waypoint.placeable));
            };
            if !(length.is_finite() && length > 0.0) {
                return Err(format!(
                    "waypoint {}'s segment is {length} m",
                    waypoint.placeable
                ));
            }
            let mut segment = Segment {
                start,
                length,
                curve: [a, b, c, waypoint.position],
                arc: vec![0.0],
            };
            let mut previous = segment.at(0.0);
            for k in 1..=SAMPLES {
                let point = segment.at(k as f64 / SAMPLES as f64);
                let step = distance(previous, point);
                segment.arc.push(segment.arc[k - 1] + step);
                previous = point;
            }
            if distance(previous, next.position) > CURVE_TOLERANCE {
                return Err(format!(
                    "waypoint {}'s curve ends at {previous:?}, not at the next waypoint",
                    waypoint.placeable
                ));
            }
            segments.push(segment);
            start += length;
        }
        let mut distance = 0.0;
        let waypoints = waypoints
            .iter()
            .map(|waypoint| {
                let at = distance;
                distance += waypoint.length.unwrap_or_default();
                Waypoint {
                    distance: at,
                    placeable: waypoint.placeable,
                    barrier: waypoint.barrier,
                    event: waypoint.event,
                }
            })
            .collect();
        Ok(Self {
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

    /// Where the payload stands at `distance` along it.
    pub fn position(&self, distance: f64) -> [f64; 3] {
        let Some(first) = self.segments.first() else {
            return [0.0; 3];
        };
        let segment = self
            .segments
            .iter()
            .rev()
            .find(|segment| segment.start <= distance)
            .unwrap_or(first);
        segment.along((distance - segment.start) / segment.length)
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3)
        .map(|axis| (a[axis] - b[axis]).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// The payload (graph 1225, rules 39-47) on its track.
#[derive(Clone, Debug, PartialEq)]
pub struct Payload {
    track: Track,
    rules: PayloadRules,
    /// The push speeds times the map's speed factor (v7779).
    push_speeds: [f64; 3],
    /// The checkpoints' distances along the track; the last is the
    /// delivery.
    checkpoints: Vec<f64>,
    distance: f64,
    /// The farthest it got (rule 63).
    best: f64,
    /// Checkpoints reached.
    reached: usize,
    /// Seconds without an attacker in range.
    idle: f64,
    /// Seconds the contest still lasts.
    contest: f64,
    /// Seconds it stays put: the waits once active, a waypoint's pause.
    hold: f64,
    /// On a run of its own, until the next waypoint with an event.
    auto: bool,
    /// The waypoints up to here have been reached.
    passed: usize,
}

impl Payload {
    /// The payload as it becomes active, heading through the checkpoints
    /// (their placeables, in order) on its track; none when a checkpoint is
    /// not a waypoint of the track or they don't come in order. 1225 waits
    /// v2102 and v2118 (Hybrid's unlock) and, with v1362 set, runs on its
    /// own to the first waypoint with an event (King's Row and Blizzard
    /// World's unlock run). Assumption: the run comes after the waits.
    pub fn new(
        data: &PayloadData,
        rules: PayloadRules,
        checkpoints: &[u32],
        scale: f64,
    ) -> Option<Self> {
        let waypoints = data.track.waypoints();
        let checkpoints: Vec<f64> = checkpoints
            .iter()
            .map(|placeable| {
                waypoints
                    .iter()
                    .find(|waypoint| waypoint.placeable == *placeable)
                    .map(|waypoint| waypoint.distance)
            })
            .collect::<Option<_>>()?;
        if checkpoints.is_empty() || checkpoints.windows(2).any(|pair| pair[0] >= pair[1]) {
            return None;
        }
        Some(Self {
            track: data.track.clone(),
            rules,
            push_speeds: rules.push_speeds.map(|speed| speed * data.speed),
            checkpoints,
            distance: 0.0,
            best: 0.0,
            reached: 0,
            idle: 0.0,
            contest: 0.0,
            hold: (data.unlock[0] + data.unlock[1]) * scale,
            auto: data.auto_start,
            passed: 0,
        })
    }

    pub fn distance(&self) -> f64 {
        self.distance
    }

    pub fn best(&self) -> f64 {
        self.best
    }

    pub fn length(&self) -> f64 {
        self.track.length()
    }

    /// Checkpoints reached; the last is the delivery.
    pub fn reached(&self) -> usize {
        self.reached
    }

    pub fn checkpoints(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn delivered(&self) -> bool {
        self.reached == self.checkpoints()
    }

    /// Where it stands.
    pub fn position(&self) -> [f64; 3] {
        self.track.position(self.distance)
    }

    /// Its push, contest and heal range (the same for every map).
    pub fn area(&self) -> Area {
        let position = self.position();
        Area::Sphere {
            centre: std::array::from_fn(|axis| position[axis] + self.rules.centre[axis]),
            radius: self.rules.radius,
        }
    }

    /// One step with `presence` in its range; the checkpoint reached in
    /// this step. 1225: attackers push it at their count's speed unless it
    /// is contested, which lasts v3032 after the last defender left (uids
    /// 4, 203-205); after v2312 without an attacker it rolls back (uids 52,
    /// 44) to the last barrier it passed. A waypoint with an event stops it
    /// for its pause and may send it on alone (uids 61, 73-75, 124-125).
    /// Assumption: an event fires once, the first time it is reached.
    pub fn step(&mut self, seconds: f64, scale: f64, presence: Presence) -> Option<usize> {
        if self.delivered() {
            return None;
        }
        if presence.defenders > 0 {
            self.contest = self.rules.contest_linger * scale;
        } else {
            self.contest = (self.contest - seconds).max(0.0);
        }
        let contested = presence.defenders > 0 || self.contest > 0.0;
        if presence.attackers > 0 {
            self.idle = 0.0;
        } else {
            self.idle += seconds;
        }
        if self.hold > 0.0 {
            self.hold -= seconds;
            return None;
        }
        if let Some(reached) = self.arrive(scale) {
            return Some(reached);
        }
        if self.hold > 0.0 {
            return None;
        }
        let forward = if self.auto {
            Some(self.rules.auto_speed)
        } else if presence.attackers > 0 && !contested {
            Some(self.push_speeds[presence.attackers.clamp(1, 3) - 1])
        } else {
            None
        };
        if let Some(speed) = forward {
            let stop = self.next_stop().unwrap_or_else(|| self.length());
            self.distance = (self.distance + speed / scale * seconds).min(stop);
            self.best = self.best.max(self.distance);
            return self.arrive(scale);
        }
        if presence.attackers == 0 && self.idle >= self.rules.rollback_after * scale {
            let floor = self
                .track
                .waypoints()
                .iter()
                .filter(|waypoint| waypoint.barrier && waypoint.distance <= self.distance)
                .map(|waypoint| waypoint.distance)
                .fold(0.0, f64::max);
            let floor = self.checkpoints[..self.reached]
                .iter()
                .copied()
                .fold(floor, f64::max);
            self.distance =
                (self.distance - self.rules.rollback_speed / scale * seconds).max(floor);
        }
        None
    }

    /// The distance of the next waypoint it stops at: one with an event or
    /// a checkpoint.
    fn next_stop(&self) -> Option<f64> {
        self.track.waypoints()[self.passed..]
            .iter()
            .find(|waypoint| self.stops_at(waypoint))
            .map(|waypoint| waypoint.distance)
    }

    fn stops_at(&self, waypoint: &Waypoint) -> bool {
        waypoint.event.is_some() || self.checkpoints.contains(&waypoint.distance)
    }

    /// Reach the waypoints up to where it stands: the first stop among
    /// them takes its pause and its run of its own; a checkpoint counts.
    fn arrive(&mut self, scale: f64) -> Option<usize> {
        while let Some(&waypoint) = self.track.waypoints().get(self.passed) {
            if waypoint.distance > self.distance + 1e-9 {
                return None;
            }
            self.passed += 1;
            self.distance = self.distance.max(waypoint.distance);
            if let Some(event) = waypoint.event {
                self.hold = event.pause * scale;
                self.auto = event.auto_next;
            }
            if self.checkpoints.get(self.reached) == Some(&waypoint.distance) {
                self.reached += 1;
                return Some(self.reached);
            }
            if self.stops_at(&waypoint) {
                return None;
            }
        }
        None
    }
}

/// The Control point of a round (graph 0E8E, rules 55-58): it unlocks
/// after 30 s, a team captures it in the times 0EC2 gives
/// (`mode_graph_values.rs`), and the owner's percentage rises at its
/// scoring rate (1% every 1.2 s) to 100. Also Elimination's tiebreaker
/// point (rule 100), which only needs an owner.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlPoint {
    pub area: Area,
    pub position: [f64; 3],
    pub name: Option<String>,
    locked: f64,
    capture: f64,
    decay: f64,
    rates: CaptureRates,
    /// None for Elimination's tiebreaker point, which only needs an owner.
    scoring: Option<ControlScoring>,
    owner: Option<u8>,
    /// The team taking it and its progress, 0 to 1.
    taking: Option<(u8, f64)>,
    /// Seconds since the last of the team taking it left.
    empty: f64,
    percent: [f64; 2],
}

impl ControlPoint {
    pub fn new(
        objective: &MapObjective,
        point: &PointData,
        unlock: f64,
        rates: CaptureRates,
        scoring: ControlScoring,
    ) -> Self {
        let mut control = Self::tiebreak(point, rates, unlock);
        control.area = Area::of_point(point, objective.position);
        control.position = objective.position;
        control.name = objective.name.clone();
        control.scoring = Some(scoring);
        control
    }

    /// Elimination's tiebreaker point (rule 100): it unlocks after
    /// `unlock` and the first team to own it wins the round.
    pub fn tiebreak(point: &PointData, rates: CaptureRates, unlock: f64) -> Self {
        Self {
            area: Area::Boxes(point.boxes.clone()),
            position: point.position,
            name: None,
            locked: unlock,
            capture: point.capture,
            decay: point.decay,
            rates,
            scoring: None,
            owner: None,
            taking: None,
            empty: 0.0,
            percent: [0.0; 2],
        }
    }

    pub fn owner(&self) -> Option<u8> {
        self.owner
    }

    pub fn locked(&self) -> bool {
        self.locked > 0.0
    }

    pub fn percent(&self, team: u8) -> f64 {
        self.percent[usize::from(team.min(1))]
    }

    /// Seconds until it unlocks.
    pub fn unlocks_in(&self) -> f64 {
        self.locked.max(0.0)
    }

    /// Whether the team that doesn't own it stands on it, which holds the
    /// owner at the scoring's hold (99%, overtime).
    pub fn contested(&self, participants: &[Participant]) -> bool {
        let Some(owner) = self.owner else {
            return false;
        };
        self.area.presence(participants, owner).defenders > 0
    }

    /// One step; the round's winner once a team reaches 100%. 0E8E with
    /// v9847 set (the Control points' override): the team taking it is the
    /// one with more players on a neutral point (equal numbers: a random
    /// pick, uid 136; assumption: team 0, for reproducible rounds), or the
    /// other team on an owned one. It captures as an Assault point does,
    /// and decays to 0; on a neutral point the decay starts at once when
    /// the other team stands on it (uid 298).
    pub fn step(&mut self, seconds: f64, scale: f64, participants: &[Participant]) -> Option<u8> {
        if self.locked > 0.0 {
            self.locked -= seconds;
            return None;
        }
        let blue = self.area.presence(participants, 0);
        let on = [blue.attackers, blue.defenders];
        let taker = match (self.taking, self.owner) {
            (Some((team, _)), _) => Some(team),
            (None, Some(owner)) => Some(1 - owner).filter(|other| on[usize::from(*other)] > 0),
            (None, None) if on == [0, 0] => None,
            (None, None) => Some(u8::from(on[1] > on[0])),
        };
        if let Some(team) = taker {
            let progress = self.taking.map_or(0.0, |(_, progress)| progress);
            let attackers = on[usize::from(team)];
            let defenders = on[usize::from(1 - team)];
            if attackers > 0 {
                self.empty = 0.0;
                let progress = if defenders == 0 {
                    progress + self.rates.capture(attackers) / (self.capture * scale) * seconds
                } else {
                    progress
                };
                if progress >= 1.0 - 1e-9 {
                    self.owner = Some(team);
                    self.taking = None;
                } else {
                    self.taking = Some((team, progress));
                }
            } else {
                self.empty += seconds;
                let at_once = self.owner.is_none() && defenders > 0;
                let progress = if at_once || self.empty >= self.rates.decay_delay * scale {
                    progress - self.rates.decay(defenders) / (self.decay * scale) * seconds
                } else {
                    progress
                };
                self.taking = (progress > 0.0).then_some((team, progress));
            }
        }
        let owner = self.owner?;
        let scoring = self.scoring?;
        let score = &mut self.percent[usize::from(owner)];
        *score += seconds * scoring.percent_per_second / scale;
        let held = on[usize::from(1 - owner)] > 0;
        if held && *score >= scoring.hold_at {
            *score = scoring.hold_at;
        }
        (*score >= 100.0).then(|| {
            *score = 100.0;
            owner
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
