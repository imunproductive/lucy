//! The objectives of the PvP modes as the server runs them: capture points,
//! the payload and the Control point (docs/notes/game-rules.md §3-7).
//!
//! The retail objectives are map entities whose statescript (graphs 0F4C,
//! 0E8E, 1225) works with the native capture area and escort components.
//! Their volumes, the payload's waypoints and the native → graph interface
//! aren't derived yet, so these rules stand in for them, from the derived
//! objective positions (`MapObjective`) and the public rules. Every value
//! the rules don't give is an assumption, named where it is used.

use orpheus_content::maps::MapObjective;

/// Assumption: a capture area is a vertical cylinder around the objective
/// entity's position. Neither the area placeables (map chunk kind 8) nor
/// `STUECCaptureAreaInstanceData` are decoded, and the rules give no radius.
pub const POINT_RADIUS: f64 = 8.0;
/// Assumption: how far below and above the objective's position a player
/// still stands in its area.
pub const POINT_BELOW: f64 = 2.0;
pub const POINT_ABOVE: f64 = 5.0;
/// Assumption: the payload's push radius. The rules don't give it
/// (game-rules.md 48); 5 m is the payload's healing range (rule 42).
pub const PAYLOAD_RADIUS: f64 = 5.0;
/// Rule 39: the payload rolls back after 10 s without an attacker near.
pub const ROLLBACK_AFTER: f64 = 10.0;
/// Assumption: the rollback speed, the Overwatch 2 value (rule 45, ⚠️OW2);
/// no Overwatch 1 value is known.
pub const ROLLBACK_SPEED: f64 = 0.3;
/// Rule 40: speed multiplier by pusher count, for maps without a table entry.
pub const PUSH_MULTIPLIERS: [f64; 3] = [1.0, 1.165, 1.33];

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

/// A vertical cylinder (see [`POINT_RADIUS`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub centre: [f64; 3],
    pub radius: f64,
    pub below: f64,
    pub above: f64,
}

impl Area {
    pub fn point(objective: &MapObjective) -> Self {
        Self {
            centre: objective.position,
            radius: POINT_RADIUS,
            below: POINT_BELOW,
            above: POINT_ABOVE,
        }
    }

    /// The payload's push area around where it stands.
    pub fn payload(centre: [f64; 3]) -> Self {
        Self {
            centre,
            radius: PAYLOAD_RADIUS,
            below: PAYLOAD_RADIUS,
            above: PAYLOAD_RADIUS,
        }
    }

    pub fn contains(&self, position: [f64; 3]) -> bool {
        let [x, y, z] = position;
        let [cx, cy, cz] = self.centre;
        (x - cx).hypot(z - cz) <= self.radius && y >= cy - self.below && y <= cy + self.above
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
}

/// The capture seconds with 1, 2 and 3 or more attackers.
pub type CaptureTimes = [f64; 3];

/// Rule 33: Assault point A, 30/20/15 s.
pub const ASSAULT_FIRST: CaptureTimes = [30.0, 20.0, 15.0];
/// Rule 34: Assault point B, 18/12/10 s.
pub const ASSAULT_LAST: CaptureTimes = [18.0, 12.0, 10.0];
/// Rule 49: Hybrid point A, 32/21/15 s for its three ticks.
pub const HYBRID_POINT: CaptureTimes = [32.0, 21.0, 15.0];
/// Rule 56: Control, 8/5/4 s.
pub const CONTROL_CAPTURE: CaptureTimes = [8.0, 5.0, 4.0];
/// Rule 100: Elimination's tiebreaker point, 3 s (custom default).
/// Assumption: the same with more players on it.
pub const ELIMINATION_CAPTURE: CaptureTimes = [3.0; 3];

fn capture_seconds(times: &CaptureTimes, players: usize) -> f64 {
    times[players.clamp(1, 3) - 1]
}

/// An Assault or Hybrid capture point (rules 32-35). Progress runs 0 to 1;
/// its three ticks (1/3, 2/3, 1) are checkpoints it never falls below.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturePoint {
    pub area: Area,
    pub name: Option<String>,
    times: CaptureTimes,
    progress: f64,
    captured: bool,
}

impl CapturePoint {
    pub fn new(objective: &MapObjective, times: CaptureTimes) -> Self {
        Self {
            area: Area::point(objective),
            name: objective.name.clone(),
            times,
            progress: 0.0,
            captured: false,
        }
    }

    pub fn progress(&self) -> f64 {
        self.progress
    }

    pub fn captured(&self) -> bool {
        self.captured
    }

    /// The ticks reached, 0 to 3 (rule 35).
    pub fn ticks(&self) -> u8 {
        if self.captured {
            3
        } else {
            ((self.progress * 3.0 + 1e-9).floor() as u8).min(3)
        }
    }

    /// One step of `seconds` (already on the match's time scale) with
    /// `presence` on the point; true when it was captured in this step.
    /// Progress rises with attackers alone on it (rule 32), holds while it
    /// is contested, and without attackers falls back to its last tick.
    /// Assumption: it falls back at the one-attacker rate (rule 35 gives no
    /// rate).
    pub fn step(&mut self, seconds: f64, scale: f64, presence: Presence) -> bool {
        if self.captured {
            return false;
        }
        match (presence.attackers, presence.defenders) {
            (0, _) => {
                let floor = f64::from(self.ticks()) / 3.0;
                let rate = 1.0 / (self.times[0] * scale);
                self.progress = (self.progress - rate * seconds).max(floor);
            }
            (attackers, 0) => {
                let rate = 1.0 / (capture_seconds(&self.times, attackers) * scale);
                self.progress += rate * seconds;
                if self.progress >= 1.0 - 1e-9 {
                    self.progress = 1.0;
                    self.captured = true;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

/// The payload's speeds in m/s with 1, 2 and 3 or more pushers, by map
/// (rules 41 and 50); other maps take the most common row.
pub fn payload_speeds(map_name: &str) -> [f64; 3] {
    const SLOW: [f64; 3] = [1.4, 1.65, 1.85];
    const MID: [f64; 3] = [1.55, 1.8, 2.05];
    const FAST: [f64; 3] = [1.7, 2.0, 2.25];
    let name = map_name.to_ascii_lowercase();
    let any = |names: &[&str]| names.iter().any(|map| name.contains(map));
    if any(&["havana", "route 66"]) {
        SLOW
    } else if any(&["gibraltar", "rialto", "blizzard world", "eichenwalde"]) {
        FAST
    } else {
        MID
    }
}

/// The payload (rules 39-47). Assumption: it moves along straight lines
/// from its start through each checkpoint in order; the waypoints
/// (`STUWaypointInstanceData`) that give the real path aren't derived.
#[derive(Clone, Debug, PartialEq)]
pub struct Payload {
    path: Vec<[f64; 3]>,
    /// Distance along the path of each path point.
    lengths: Vec<f64>,
    speeds: [f64; 3],
    distance: f64,
    /// The farthest it got (rule 63).
    best: f64,
    /// Checkpoints reached; path point `reached` is the last one.
    reached: usize,
    idle: f64,
}

impl Payload {
    /// From `start` through `checkpoints` (the last is the delivery point).
    pub fn new(start: [f64; 3], checkpoints: &[[f64; 3]], speeds: [f64; 3]) -> Self {
        let mut path = vec![start];
        path.extend_from_slice(checkpoints);
        let mut lengths = vec![0.0];
        for pair in path.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            let step =
                ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();
            lengths.push(lengths.last().copied().unwrap_or_default() + step);
        }
        Self {
            path,
            lengths,
            speeds,
            distance: 0.0,
            best: 0.0,
            reached: 0,
            idle: 0.0,
        }
    }

    pub fn distance(&self) -> f64 {
        self.distance
    }

    pub fn best(&self) -> f64 {
        self.best
    }

    pub fn length(&self) -> f64 {
        self.lengths.last().copied().unwrap_or_default()
    }

    /// Checkpoints reached; the last is the delivery.
    pub fn reached(&self) -> usize {
        self.reached
    }

    pub fn checkpoints(&self) -> usize {
        self.path.len() - 1
    }

    pub fn delivered(&self) -> bool {
        self.reached == self.checkpoints()
    }

    /// Where it stands.
    pub fn position(&self) -> [f64; 3] {
        let segment = self
            .lengths
            .windows(2)
            .position(|pair| self.distance <= pair[1])
            .unwrap_or(self.path.len().saturating_sub(2));
        let (from, to) = (self.lengths[segment], self.lengths[segment + 1]);
        let t = if to > from {
            ((self.distance - from) / (to - from)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let [a, b] = [self.path[segment], self.path[segment + 1]];
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }

    pub fn area(&self) -> Area {
        Area::payload(self.position())
    }

    /// One step: attackers alone push it (rule 39, the speed by pusher
    /// count); any defender stops it; after [`ROLLBACK_AFTER`] without
    /// attackers it rolls back toward the last checkpoint. Returns the
    /// checkpoint reached in this step.
    pub fn step(&mut self, seconds: f64, scale: f64, presence: Presence) -> Option<usize> {
        if self.delivered() {
            return None;
        }
        if presence.attackers > 0 {
            self.idle = 0.0;
        } else {
            self.idle += seconds;
        }
        if presence.attackers > 0 && presence.defenders == 0 {
            let speed = self.speeds[presence.attackers.clamp(1, 3) - 1] / scale;
            self.distance = (self.distance + speed * seconds).min(self.length());
            self.best = self.best.max(self.distance);
            let next = self.lengths[self.reached + 1];
            if self.distance >= next - 1e-9 {
                self.reached += 1;
                return Some(self.reached);
            }
        } else if presence.attackers == 0 && self.idle >= ROLLBACK_AFTER * scale {
            let floor = self.lengths[self.reached];
            self.distance = (self.distance - ROLLBACK_SPEED / scale * seconds).max(floor);
        }
        None
    }
}

/// The Control point of a round (rules 55-58): it unlocks after 30 s,
/// a team captures it (8/5/4 s, any enemy on it blocks), and the owner's
/// percentage rises 1% every 1.2 s to 100.
#[derive(Clone, Debug, PartialEq)]
pub struct ControlPoint {
    pub area: Area,
    pub name: Option<String>,
    locked: f64,
    times: CaptureTimes,
    owner: Option<u8>,
    /// The team taking it and its progress, 0 to 1.
    taking: Option<(u8, f64)>,
    percent: [f64; 2],
}

/// Rule 57: 1% every 1.2 s.
pub const CONTROL_SCORE_SECONDS: f64 = 1.2;
/// Rule 27: overtime holds the owner at 99% while the other team contests.
pub const CONTROL_OVERTIME_AT: f64 = 99.0;

impl ControlPoint {
    pub fn new(objective: &MapObjective, unlock: f64) -> Self {
        Self {
            area: Area::point(objective),
            name: objective.name.clone(),
            locked: unlock,
            times: CONTROL_CAPTURE,
            owner: None,
            taking: None,
            percent: [0.0; 2],
        }
    }

    /// Elimination's tiebreaker point (rule 100): it unlocks after
    /// `unlock` and the first team to own it wins the round.
    pub fn tiebreak(area: Area, unlock: f64) -> Self {
        Self {
            area,
            name: None,
            locked: unlock,
            times: ELIMINATION_CAPTURE,
            owner: None,
            taking: None,
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
    /// owner at 99% (overtime).
    pub fn contested(&self, participants: &[Participant]) -> bool {
        let Some(owner) = self.owner else {
            return false;
        };
        self.area.presence(participants, owner).defenders > 0
    }

    /// One step; the round's winner once a team reaches 100%. Assumption:
    /// an unfinished capture with nobody on the point falls back at the
    /// one-player rate (rule 56 gives no rate).
    pub fn step(&mut self, seconds: f64, scale: f64, participants: &[Participant]) -> Option<u8> {
        if self.locked > 0.0 {
            self.locked -= seconds;
            return None;
        }
        let blue = self.area.presence(participants, 0);
        let on = [blue.attackers, blue.defenders];
        let contested = on[0] > 0 && on[1] > 0;
        let alone = (0..2u8).find(|&team| on[usize::from(team)] > 0 && !contested);
        match (alone, self.taking) {
            (Some(team), _) if Some(team) == self.owner => self.taking = None,
            (Some(team), taking) => {
                let progress = match taking {
                    Some((taker, progress)) if taker == team => progress,
                    _ => 0.0,
                };
                let rate = 1.0 / (capture_seconds(&self.times, on[usize::from(team)]) * scale);
                let progress = progress + rate * seconds;
                if progress >= 1.0 - 1e-9 {
                    self.owner = Some(team);
                    self.taking = None;
                } else {
                    self.taking = Some((team, progress));
                }
            }
            (None, Some((team, progress))) if !contested => {
                let rate = 1.0 / (self.times[0] * scale);
                let progress = progress - rate * seconds;
                self.taking = (progress > 0.0).then_some((team, progress));
            }
            _ => {}
        }
        let owner = self.owner?;
        let score = &mut self.percent[usize::from(owner)];
        *score += seconds / (CONTROL_SCORE_SECONDS * scale);
        let held = on[usize::from(1 - owner)] > 0;
        if held && *score >= CONTROL_OVERTIME_AT {
            *score = CONTROL_OVERTIME_AT;
        }
        (*score >= 100.0).then(|| {
            *score = 100.0;
            owner
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_content::maps::ObjectiveKind;

    const STEP: f64 = 1.0 / 60.0;

    fn objective(position: [f64; 3]) -> MapObjective {
        MapObjective {
            kind: ObjectiveKind::Point,
            order: 0,
            name: Some("Point A".into()),
            placeable: 1,
            position,
        }
    }

    fn run(seconds: f64, mut step: impl FnMut() -> bool) -> Option<f64> {
        let mut elapsed = 0.0;
        while elapsed < seconds {
            elapsed += STEP;
            if step() {
                return Some(elapsed);
            }
        }
        None
    }

    fn on(attackers: usize, defenders: usize) -> Presence {
        Presence {
            attackers,
            defenders,
        }
    }

    #[test]
    fn point_capture_times_follow_the_attacker_count() {
        for (times, expected) in [
            (ASSAULT_FIRST, [30.0, 20.0, 15.0]),
            (ASSAULT_LAST, [18.0, 12.0, 10.0]),
        ] {
            for (attackers, seconds) in [
                (1, expected[0]),
                (2, expected[1]),
                (3, expected[2]),
                (6, expected[2]),
            ] {
                let mut point = CapturePoint::new(&objective([0.0; 3]), times);
                let took = run(60.0, || point.step(STEP, 1.0, on(attackers, 0))).unwrap();
                assert!((took - seconds).abs() < 2.0 * STEP, "{attackers}: {took} s");
            }
        }
        // The time scale shortens it.
        let mut point = CapturePoint::new(&objective([0.0; 3]), ASSAULT_FIRST);
        let took = run(60.0, || point.step(STEP, 0.1, on(1, 0))).unwrap();
        assert!((took - 3.0).abs() < 2.0 * STEP);
    }

    #[test]
    fn a_contested_point_holds_and_an_empty_one_falls_back_to_its_tick() {
        let mut point = CapturePoint::new(&objective([0.0; 3]), ASSAULT_FIRST);
        run(12.0, || point.step(STEP, 1.0, on(1, 0)));
        assert_eq!(point.ticks(), 1);
        let held = point.progress();
        run(20.0, || point.step(STEP, 1.0, on(2, 1)));
        assert_eq!(point.progress(), held, "contested: no change");
        run(20.0, || point.step(STEP, 1.0, on(0, 0)));
        assert!(
            (point.progress() - 1.0 / 3.0).abs() < 1e-9,
            "back to the first tick"
        );
        run(20.0, || point.step(STEP, 1.0, on(0, 2)));
        assert!((point.progress() - 1.0 / 3.0).abs() < 1e-9);
        assert!(!point.captured());
    }

    #[test]
    fn areas_count_alive_players_inside_by_side() {
        let area = Area::point(&objective([10.0, 0.0, 10.0]));
        let player = |slot, team, position| Participant {
            slot,
            team,
            position,
        };
        let players = [
            player(0, 1, Some([12.0, 1.0, 13.0])),
            player(1, 1, Some([10.0, 4.9, 10.0])),
            player(2, 1, Some([10.0, 5.1, 10.0])),
            player(3, 0, Some([17.9, 0.0, 10.0])),
            player(4, 0, None),
            player(5, 0, Some([18.1, 0.0, 10.0])),
        ];
        assert_eq!(area.presence(&players, 1), on(2, 1));
        assert_eq!(area.presence(&players, 0), on(1, 2));
    }

    #[test]
    fn the_payload_moves_by_pusher_count_stops_and_rolls_back_to_its_checkpoint() {
        let speeds = payload_speeds("Watchpoint: Gibraltar");
        assert_eq!(speeds, [1.7, 2.0, 2.25]);
        assert_eq!(payload_speeds("Route 66"), [1.4, 1.65, 1.85]);
        assert_eq!(payload_speeds("King's Row"), [1.55, 1.8, 2.05]);
        let mut payload = Payload::new([0.0; 3], &[[20.0, 0.0, 0.0], [40.0, 0.0, 0.0]], speeds);
        assert_eq!(payload.length(), 40.0);
        for (pushers, speed) in [(1, 1.7), (2, 2.0), (3, 2.25), (5, 2.25)] {
            let mut payload = payload.clone();
            run(1.0, || {
                payload.step(STEP, 1.0, on(pushers, 0));
                false
            });
            assert!(
                (payload.distance() - speed).abs() < 0.05,
                "{pushers}: {}",
                payload.distance()
            );
        }
        let reached = run(20.0, || payload.step(STEP, 1.0, on(1, 0)) == Some(1)).unwrap();
        assert!((reached - 20.0 / 1.7).abs() < 2.0 * STEP);
        run(3.0, || payload.step(STEP, 1.0, on(1, 0)).is_some());
        let pushed = payload.distance();
        run(5.0, || payload.step(STEP, 1.0, on(3, 1)).is_some());
        assert_eq!(payload.distance(), pushed, "a defender stops it");
        run(9.0, || payload.step(STEP, 1.0, on(0, 0)).is_some());
        assert_eq!(payload.distance(), pushed, "no rollback before 10 s");
        run(60.0, || payload.step(STEP, 1.0, on(0, 0)).is_some());
        assert_eq!(payload.distance(), 20.0, "rolls back to the checkpoint");
        assert!(payload.best() > 20.0);
        assert_eq!(payload.position(), [20.0, 0.0, 0.0]);
        run(60.0, || payload.step(STEP, 1.0, on(2, 0)).is_some());
        assert!(payload.delivered());
    }

    #[test]
    fn control_unlocks_captures_scores_and_holds_at_99_while_contested() {
        let point_at = [0.0; 3];
        let mut point = ControlPoint::new(&objective(point_at), 30.0);
        let blue = Participant {
            slot: 0,
            team: 0,
            position: Some(point_at),
        };
        let red = Participant {
            slot: 1,
            team: 1,
            position: Some(point_at),
        };
        run(29.0, || point.step(STEP, 1.0, &[blue]).is_some());
        assert!(point.locked());
        run(1.0 + 7.9, || point.step(STEP, 1.0, &[blue]).is_some());
        assert_eq!(point.owner(), None, "8 s with one player");
        run(0.2, || point.step(STEP, 1.0, &[blue]).is_some());
        assert_eq!(point.owner(), Some(0));
        // 118.8 s more takes blue to 99%, held there while red contests.
        let won = run(200.0, || point.step(STEP, 1.0, &[blue, red]).is_some());
        assert!(won.is_none());
        assert_eq!(point.percent(0), 99.0);
        assert!(point.contested(&[blue, red]));
        // Red alone takes it in 8 s; blue keeps its 99%.
        run(8.1, || point.step(STEP, 1.0, &[red]).is_some());
        assert_eq!(point.owner(), Some(1));
        assert_eq!(point.percent(0), 99.0);
        let won = run(121.0, || point.step(STEP, 1.0, &[red]).is_some()).unwrap();
        assert!((won - 120.0).abs() < 0.5, "{won}");
        assert_eq!(point.percent(1), 100.0);
    }
}
