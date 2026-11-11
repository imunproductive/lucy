//! The match flow of the PvP modes on the server: phases, timers,
//! objectives, scoring and the match's end (docs/notes/game-rules.md §1-8,
//! 11, 16).
//!
//! In retail the mode's own graph drives this on the game mode entity
//! (`0F4A` for Assault, Escort and Hybrid, `0EC2` for Control): its phase
//! variable v59 (`003B`), its clock v1303 (`0517`) and its phase messages.
//! The VM can't run those graphs whole yet (see `mode_script.rs`), so this
//! machine drives the same phases and publishes what the HUD and the bodies
//! read: the clock, the phase banner text, the objective's name, the
//! attacking team, and the phase messages. Its phase values are v59's
//! (ow1-preservation `Assault flow on Anubis`): 0 waiting, 1 assemble,
//! 2 setup, 3 round, 4 round end, 5 match end.
//!
//! Where the rules leave a value open, the machine takes an assumption,
//! named where it is used.
//!
//! Spawn rooms: the flow names the rooms each team uses
//! ([`MatchFlow::spawn_choice`]): in Assault, Escort and Hybrid the current
//! objective's attackers' or defenders' rooms (they move forward as the
//! objectives fall, and swap with the sides), in Control the stage's.
//! Spawn doors: the map data names no door entities (the doors run graph
//! 0D32, related-projects.md, but the `game_maps` derivation doesn't list
//! them), so the server opens none. Assumption: the doors are map entities
//! the client drives itself; nothing holds the attackers in during setup.

use std::collections::VecDeque;

use orpheus_content::maps::{GameMap, ObjectiveKind, SpawnChoice};
use orpheus_script::wire::{Value, Variables};

use crate::{
    mode_rules_data::ModeRules,
    objectives::{
        ASSAULT_FIRST, ASSAULT_LAST, Area, CapturePoint, ControlPoint, HYBRID_POINT, POINT_ABOVE,
        POINT_BELOW, POINT_RADIUS, Participant, Payload, Presence, payload_speeds,
    },
};

/// The game modes (`0C5`) whose flow the server runs.
pub const ELIMINATION: u64 = 0x0230_0000_0000_0009;
pub const SKIRMISH: u64 = 0x0230_0000_0000_0010;
pub const ASSAULT: u64 = 0x0230_0000_0000_0014;
pub const ESCORT: u64 = 0x0230_0000_0000_0015;
pub const HYBRID: u64 = 0x0230_0000_0000_0016;
pub const CONTROL: u64 = 0x0230_0000_0000_0017;
pub const TEAM_DEATHMATCH: u64 = 0x0230_0000_0000_001d;
pub const DEATHMATCH: u64 = 0x0230_0000_0000_001e;
pub const BOUNTY_HUNTER: u64 = 0x0230_0000_0000_0070;
pub const ASSAULT_TEST: u64 = 0x0230_0000_0000_0067;

/// The mode entity's clock, which the HUD reads as `A3DEF368.v0517`
/// (0F4A's ChaseVar at node 15 counts it down).
pub const CLOCK: u32 = 1303;
/// The phase banner text 0F4A sets on the mode entity (v1923 = `0783`,
/// nodes 13, 18 and 27: a `UXDisplayText` per phase).
pub const BANNER: u32 = 1923;
/// The objective's name (v7643 = `1DDB`), which the objective HUD (1FEB)
/// shows.
pub const OBJECTIVE_NAME: u32 = 7643;
/// The attacking and the defending team (v10795/v10796 = `2A2B`/`2A2C`):
/// 1FEB's ATTACK/DEFEND header compares the player's team with `2A2B`.
pub const ATTACKERS: u32 = 10795;
pub const DEFENDERS: u32 = 10796;

/// The phase messages 0F4A's SendMessage states send (`025` GUIDs; the
/// parameter keys are `0A4` indexes). Which phase sends which is read
/// from the order of 0F4A's nodes (an inference, not traced): 0700 in
/// waiting, 06DF at each phase change, 0692 with the clock (017E) in setup,
/// 0701 (085A = true) when the round starts, 0694 at a round's end and 0695
/// with the winner (03A6) at the match's end. The body graph 01CA takes
/// 0694 and 0695 (its v7525 is the winner).
pub const WAITING_MESSAGE: u64 = 0x0240_0000_0000_0700;
pub const PHASE_MESSAGE: u64 = 0x0240_0000_0000_06df;
pub const SETUP_MESSAGE: u64 = 0x0240_0000_0000_0692;
pub const ROUND_MESSAGE: u64 = 0x0240_0000_0000_0701;
pub const ROUND_END_MESSAGE: u64 = 0x0240_0000_0000_0694;
pub const MATCH_END_MESSAGE: u64 = 0x0240_0000_0000_0695;
const SETUP_CLOCK: u32 = 0x017e;
const ROUND_STARTED: u32 = 0x085a;
const WINNER: u32 = 0x03a6;

/// The banner texts of 0F4A's phases 1, 2 and 3 (`0DE` GUIDs).
const BANNERS: [(Phase, u64); 3] = [
    (Phase::Assemble, 0x0de0_0000_0000_6114),
    (Phase::Setup, 0x0de0_0000_0000_6115),
    (Phase::Round, 0x0de0_0000_0000_6116),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Assault,
    Escort,
    Hybrid,
    Control,
    Skirmish,
    Elimination,
    TeamDeathmatch,
    Deathmatch,
}

impl Mode {
    pub fn of(mode: u64) -> Option<Self> {
        Some(match mode {
            ASSAULT | ASSAULT_TEST => Self::Assault,
            ESCORT => Self::Escort,
            HYBRID => Self::Hybrid,
            CONTROL => Self::Control,
            SKIRMISH => Self::Skirmish,
            ELIMINATION => Self::Elimination,
            TEAM_DEATHMATCH => Self::TeamDeathmatch,
            DEATHMATCH | BOUNTY_HUNTER => Self::Deathmatch,
            _ => return None,
        })
    }

    /// One team attacks and the other defends.
    fn sided(self) -> bool {
        matches!(self, Self::Assault | Self::Escort | Self::Hybrid)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Phase {
    Waiting,
    Assemble,
    Setup,
    Round,
    RoundEnd,
    MatchEnd,
}

impl Phase {
    /// The value of 0F4A's v59.
    pub fn value(self) -> i64 {
        self as i64
    }
}

/// How a match ended for a team (or, in a free-for-all, a player).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Standing {
    Won,
    Lost,
    Drew,
}

/// The flow's timers in seconds, before the time scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timers {
    /// Every duration is multiplied by it and every speed divided: tests
    /// play a whole match in seconds (1 in matches).
    pub scale: f64,
}

impl Default for Timers {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

/// Rule 31: 4:00 for Assault point A, 3:00 more for B.
pub const ASSAULT_TIME: f64 = 240.0;
pub const ASSAULT_BONUS: f64 = 180.0;
/// Rule 43: Escort starts with 4:00; rule 53: Hybrid's point A too.
pub const ESCORT_TIME: f64 = 240.0;
/// Assumption: time added at the payload's checkpoints. The rules know no
/// Overwatch 1 values (rule 45, ⚠️OW2: +2:30 and +1:30, used here).
pub const CHECKPOINT_BONUS: [f64; 2] = [150.0, 90.0];
/// Assumption: time added when Hybrid's point A falls, as Assault's B.
pub const HYBRID_BONUS: f64 = 180.0;
/// Phase 4 of 0F4A, 11.6 s (ow1-preservation `Assault flow on Anubis`).
pub const ROUND_END: f64 = 11.6;
/// Assumption: how long the match's end shows before the players go home.
/// Neither POTG nor the cards are played; their length is unknown (rule 167).
pub const MATCH_END: f64 = 20.0;
/// Rule 95: the top 4 of a free-for-all get win credit.
pub const DEATHMATCH_WINNERS: usize = 4;
/// Rule 26 (mechanics probably Overwatch 1): the overtime fuse burns 5 s,
/// 3 s after 20 s of overtime, 1.5 s after 30 s.
pub const FUSE: [(f64, f64); 3] = [(0.0, 5.0), (20.0, 3.0), (30.0, 1.5)];
/// Rule 11: overtime adds 3 s to a respawn (the notes favour the wiki's
/// 3 s over the Jul 2016 patch's 2 s).
pub const OVERTIME_RESPAWN: f64 = 3.0;
/// Rule 69: in Assault's extra rounds point A adds 30 s.
pub const EXTRA_ASSAULT_BONUS: f64 = 30.0;

/// Something the room acts on.
#[derive(Clone, Debug, PartialEq)]
pub enum FlowEvent {
    /// A phase began.
    Phase(Phase),
    /// A phase message for the players' bodies.
    Message {
        message: u64,
        parameters: Variables,
    },
    /// An objective fell (its order), or the payload reached a checkpoint.
    Objective(u32),
    Overtime(bool),
    /// A round was decided: its winning team, or a draw.
    RoundOver(Option<u8>),
    /// The match was decided; it ends once [`MATCH_END`] is over.
    MatchOver,
    /// The end screen is over: the players go home.
    Finished,
}

/// What the attackers of a sided round play for.
#[derive(Clone, Debug)]
enum Course {
    Points(Vec<CapturePoint>),
    Hybrid {
        point: CapturePoint,
        payload: Option<Payload>,
    },
    Payload(Payload),
}

impl Course {
    /// Progress for the tiebreak (rules 63-65): the objectives taken and
    /// the share of the next one that counts, its last capture mark.
    fn progress(&self) -> (u32, f64) {
        match self {
            Self::Points(points) => {
                let taken = points.iter().filter(|point| point.captured()).count() as u32;
                let next = points
                    .iter()
                    .find(|point| !point.captured())
                    .map_or(0.0, CapturePoint::mark);
                (taken, next)
            }
            Self::Hybrid { point, payload } => match payload {
                Some(payload) => (1 + payload.reached() as u32, payload.best()),
                None => (0, point.mark()),
            },
            Self::Payload(payload) => (payload.reached() as u32, payload.best()),
        }
    }

    /// The objective (its order, 0x2934) the attackers play for now, whose
    /// rooms the teams spawn in: the next point, or the checkpoint the
    /// payload heads for (a Hybrid's point is objective 0, its
    /// checkpoints 1 and 2). The derived orders run 0, 1, 2 in the map.
    fn objective(&self) -> u32 {
        match self {
            Self::Points(points) => {
                let taken = points.iter().filter(|point| point.captured()).count();
                taken.min(points.len().saturating_sub(1)) as u32
            }
            Self::Hybrid { payload: None, .. } => 0,
            Self::Hybrid {
                payload: Some(payload),
                ..
            } => (1 + payload.reached()).min(payload.checkpoints()) as u32,
            Self::Payload(payload) => payload
                .reached()
                .min(payload.checkpoints().saturating_sub(1))
                as u32,
        }
    }

    fn complete(&self) -> bool {
        match self {
            Self::Points(points) => points.iter().all(CapturePoint::captured),
            Self::Hybrid { payload, .. } => payload.as_ref().is_some_and(Payload::delivered),
            Self::Payload(payload) => payload.delivered(),
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Points(points) => points
                .iter()
                .find(|point| !point.captured())
                .and_then(|point| point.name.as_deref()),
            Self::Hybrid {
                point,
                payload: None,
            } => point.name.as_deref(),
            _ => None,
        }
    }

    /// Where the attackers play: the next point, or the payload.
    fn position(&self) -> Option<[f64; 3]> {
        match self {
            Self::Points(points) => points
                .iter()
                .find(|point| !point.captured())
                .map(|point| point.area.centre),
            Self::Hybrid {
                payload: Some(payload),
                ..
            }
            | Self::Payload(payload) => Some(payload.position()),
            Self::Hybrid { point, .. } => Some(point.area.centre),
        }
    }

    /// Whether an attacker stands on the objective (it keeps overtime).
    fn presence(&self, participants: &[Participant], attackers: u8) -> Presence {
        match self {
            Self::Points(points) => points
                .iter()
                .find(|point| !point.captured())
                .map_or_else(Presence::default, |point| {
                    point.area.presence(participants, attackers)
                }),
            Self::Hybrid {
                payload: Some(payload),
                ..
            }
            | Self::Payload(payload) => payload.area().presence(participants, attackers),
            Self::Hybrid { point, .. } => point.area.presence(participants, attackers),
        }
    }
}

/// The objectives of one side's round.
fn course(map: &GameMap, mode: Mode, rules: &ModeRules) -> Option<Course> {
    let points: Vec<_> = map.objectives_of(ObjectiveKind::Point).collect();
    let checkpoints: Vec<[f64; 3]> = map
        .objectives_of(ObjectiveKind::Checkpoint)
        .map(|objective| objective.position)
        .collect();
    let start = map
        .objectives_of(ObjectiveKind::Payload)
        .next()
        .map(|payload| payload.position);
    let speeds = payload_speeds(&map.name);
    match mode {
        Mode::Assault => {
            let last = points.len().checked_sub(1)?;
            Some(Course::Points(
                points
                    .iter()
                    .enumerate()
                    .map(|(index, objective)| {
                        let times = if index == last {
                            ASSAULT_LAST
                        } else {
                            ASSAULT_FIRST
                        };
                        CapturePoint::new(objective, times, rules.capture_marks)
                    })
                    .collect(),
            ))
        }
        Mode::Hybrid => {
            let point = CapturePoint::new(points.first()?, HYBRID_POINT, rules.capture_marks);
            start?;
            (!checkpoints.is_empty()).then_some(Course::Hybrid {
                point,
                payload: None,
            })
        }
        Mode::Escort => {
            let start = start?;
            (!checkpoints.is_empty()).then(|| {
                Course::Payload(Payload::new(
                    start,
                    &checkpoints,
                    speeds,
                    rules.rollback_speed,
                ))
            })
        }
        _ => None,
    }
}

/// Where Elimination's tiebreaker point stands. Assumption: halfway
/// between the two teams' first spawn points; the arena maps are
/// symmetrical and their point entity isn't derived (`game_maps` lists no
/// objective for them).
fn arena_centre(map: &GameMap) -> Option<[f64; 3]> {
    let (blue, _) = map.spawn_point(0, 0)?;
    let (red, _) = map.spawn_point(1, 0)?;
    Some(std::array::from_fn(|axis| (blue[axis] + red[axis]) / 2.0))
}

#[derive(Clone, Debug)]
struct SidedRound {
    attackers: u8,
    course: Course,
    /// The clock it starts with, in unscaled seconds.
    time: f64,
    /// An extra round of the time bank (rules 67-69): no time added, but
    /// 30 s for Assault's A.
    extra: bool,
    /// The round's result for the tiebreak.
    result: Option<(u32, f64)>,
    /// The time left when the attackers completed the map (unscaled).
    bank: Option<f64>,
}

impl SidedRound {
    fn new(
        map: &GameMap,
        mode: Mode,
        rules: &ModeRules,
        attackers: u8,
        time: f64,
        extra: bool,
    ) -> Option<Self> {
        Some(Self {
            attackers,
            course: course(map, mode, rules)?,
            time,
            extra,
            result: None,
            bank: None,
        })
    }
}

#[derive(Clone, Debug)]
struct Overtime {
    elapsed: f64,
    /// Fuse seconds left.
    fuse: f64,
}

/// The match flow of a room.
#[derive(Clone, Debug)]
pub struct MatchFlow {
    mode: Mode,
    map: GameMap,
    /// The mode's timers and thresholds as the install has them.
    rules: ModeRules,
    timers: Timers,
    /// Competitive: both teams attack (sided modes); otherwise one round.
    competitive: bool,
    phase: Phase,
    /// Seconds left in the phase; for a round, its clock.
    left: f64,
    round: u8,
    sided: Option<SidedRound>,
    /// Competitive: the sides still to play in this set (the second side,
    /// or the extra rounds), each its attackers and clock.
    upcoming: VecDeque<(u8, f64)>,
    /// The sides played in this set.
    played: Vec<SidedRound>,
    /// The set is rule 69's single attack: its attackers win with a tick.
    single: bool,
    /// Whether the set's sides are extra rounds.
    extra: bool,
    /// Elimination: the capture tiebreaker's point (rule 100).
    tiebreak: Option<ControlPoint>,
    control: Option<ControlPoint>,
    /// Control and Elimination: rounds won per team.
    rounds_won: [u8; 2],
    /// Elimination: the teams with someone alive in this round.
    seen_alive: [bool; 2],
    /// Team Deathmatch per team; Deathmatch per slot.
    scores: Vec<i64>,
    overtime: Option<Overtime>,
    /// The match's result per team or, in a free-for-all, per slot.
    standings: Vec<Standing>,
    events: Vec<FlowEvent>,
}

impl MatchFlow {
    /// The flow of a map's mode by `rules` (the mode's); none for the modes
    /// the server doesn't run a flow for (Practice Range, Arcade brawls), or
    /// when the map lacks the objectives its mode plays for.
    pub fn new(map: &GameMap, competitive: bool, rules: ModeRules, timers: Timers) -> Option<Self> {
        let mode = Mode::of(map.mode_guid)?;
        let mut flow = Self {
            mode,
            map: map.clone(),
            rules,
            timers,
            competitive,
            phase: Phase::Waiting,
            left: 0.0,
            round: 0,
            sided: None,
            upcoming: VecDeque::new(),
            played: Vec::new(),
            single: false,
            extra: false,
            tiebreak: None,
            control: None,
            rounds_won: [0; 2],
            seen_alive: [false; 2],
            scores: Vec::new(),
            overtime: None,
            standings: Vec::new(),
            events: Vec::new(),
        };
        if mode.sided() {
            let time = match mode {
                Mode::Assault => ASSAULT_TIME,
                _ => ESCORT_TIME,
            };
            // game_maps.rs: team 1 attacks first; in Competitive team 0
            // attacks second with the same time.
            flow.sided = Some(SidedRound::new(map, mode, &rules, 1, time, false)?);
            if competitive {
                flow.upcoming.push_back((0, time));
            }
        }
        if mode == Mode::Control {
            map.objectives_of(ObjectiveKind::ControlPoint).next()?;
        }
        Some(flow)
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn round(&self) -> u8 {
        self.round
    }

    pub fn in_overtime(&self) -> bool {
        self.overtime.is_some()
    }

    /// The seconds the clock shows.
    pub fn clock(&self) -> f64 {
        if self.overtime.is_some() || (self.mode == Mode::Control && self.phase == Phase::Round) {
            // Rule 58: Control has no round clock.
            return 0.0;
        }
        self.left.max(0.0)
    }

    /// The attacking team of a sided round.
    pub fn attackers(&self) -> Option<u8> {
        self.sided.as_ref().map(|round| round.attackers)
    }

    /// The spawn rooms `team` uses now: the current objective's attackers'
    /// or defenders' (rules 36 and 47: they move forward as objectives
    /// fall; the teams swap them with the sides), the Control stage's
    /// (rule 60), else the first round's.
    pub fn spawn_choice(&self, team: u8) -> SpawnChoice {
        if let Some(round) = &self.sided {
            return SpawnChoice::Objective {
                objective: round.course.objective(),
                attack: team == round.attackers,
            };
        }
        if self.mode == Mode::Control {
            return SpawnChoice::Stage(self.stage(self.round).order);
        }
        SpawnChoice::First
    }

    /// Seconds added to a respawn now: rule 11's 3 s in overtime.
    pub fn respawn_extra(&self) -> f64 {
        if self.overtime.is_some() {
            self.scaled(OVERTIME_RESPAWN)
        } else {
            0.0
        }
    }

    /// Hero select stays open while the heroes assemble.
    pub fn assembling(&self) -> bool {
        self.phase == Phase::Assemble
    }

    /// Whether the dead come back: not within an Elimination round (rule
    /// 99), and not once the match is decided.
    pub fn respawns(&self) -> bool {
        !(self.mode == Mode::Elimination && self.phase == Phase::Round)
            && self.phase < Phase::MatchEnd
    }

    /// The match's standing of a player of `team` in `slot`, once decided.
    pub fn standing(&self, slot: usize, team: u8) -> Option<Standing> {
        let index = if self.mode == Mode::Deathmatch {
            slot
        } else {
            usize::from(team)
        };
        self.standings.get(index).copied()
    }

    pub fn team_score(&self, team: u8) -> i64 {
        match self.mode {
            Mode::Control | Mode::Elimination => {
                i64::from(self.rounds_won[usize::from(team.min(1))])
            }
            Mode::TeamDeathmatch => self.scores.get(usize::from(team)).copied().unwrap_or(0),
            _ => 0,
        }
    }

    pub fn player_score(&self, slot: usize) -> i64 {
        self.scores.get(slot).copied().unwrap_or(0)
    }

    /// The Control point's percentage for a team in this round.
    pub fn control_percent(&self, team: u8) -> Option<f64> {
        self.control.as_ref().map(|point| point.percent(team))
    }

    /// The current objective's capture progress (0 to 1), or the payload's
    /// distance in metres.
    pub fn objective_progress(&self) -> Option<f64> {
        match &self.sided.as_ref()?.course {
            Course::Points(points) => points
                .iter()
                .find(|point| !point.captured())
                .map(CapturePoint::progress),
            Course::Hybrid {
                payload: Some(payload),
                ..
            }
            | Course::Payload(payload) => Some(payload.distance()),
            Course::Hybrid { point, .. } => Some(point.progress()),
        }
    }

    /// Where the round is played, for the AI heroes (`crate::ai`): the
    /// attackers' next point or the payload, the Control point (the next
    /// round's stage between rounds). `None` in the modes without an
    /// objective.
    pub fn objective(&self) -> Option<[f64; 3]> {
        if let Some(round) = &self.sided {
            return round.course.position();
        }
        if let Some(point) = &self.control {
            return Some(point.area.centre);
        }
        let stages: Vec<_> = self
            .map
            .objectives_of(ObjectiveKind::ControlPoint)
            .collect();
        (self.mode == Mode::Control && !stages.is_empty())
            .then(|| stages[usize::from(self.round) % stages.len()].position)
    }

    /// The variables of the mode entity the HUD reads.
    pub fn variables(&self) -> Variables {
        let mut variables = Variables::from([(CLOCK, Value::Float(self.clock()))]);
        let banner = BANNERS.iter().find(|(phase, _)| *phase == self.phase);
        if let (true, Some((_, text))) = (self.mode.sided(), banner) {
            // 0F4A's texts; the other mode graphs' aren't read.
            variables.insert(BANNER, Value::Asset(*text));
        }
        if let Some(round) = &self.sided {
            variables.insert(ATTACKERS, Value::Integer(i64::from(round.attackers)));
            variables.insert(DEFENDERS, Value::Integer(i64::from(1 - round.attackers)));
            if let Some(name) = round.course.name() {
                variables.insert(OBJECTIVE_NAME, Value::String(name.into()));
            }
        }
        if let Some(name) = self
            .control
            .as_ref()
            .and_then(|point| point.name.as_deref())
        {
            variables.insert(OBJECTIVE_NAME, Value::String(name.into()));
        }
        variables
    }

    /// The players are in: hero select opens (match.py `_spawn_steps`
    /// starts "assemble your team" at the first spawn).
    pub fn start(&mut self) {
        if self.phase == Phase::Waiting {
            let seconds = self.rules.assemble;
            self.enter(Phase::Assemble, seconds);
        }
    }

    /// A kill: the killer's slot and team (none for the world or a bot) and
    /// the victim's. Deathmatch: +1 per final blow, -1 for a death by
    /// oneself or the world (rule 95). Team Deathmatch: +1 for the killer's
    /// team (rule 96).
    pub fn kill(&mut self, killer: Option<(usize, u8)>, victim: (usize, u8)) {
        if self.phase != Phase::Round {
            return;
        }
        match self.mode {
            Mode::Deathmatch => match killer {
                Some((slot, _)) if slot != victim.0 => *self.score_of(slot) += 1,
                _ => *self.score_of(victim.0) -= 1,
            },
            Mode::TeamDeathmatch => {
                if let Some((_, team)) = killer.filter(|(_, team)| *team != victim.1) {
                    *self.score_of(usize::from(team)) += 1;
                }
            }
            _ => {}
        }
    }

    fn score_of(&mut self, index: usize) -> &mut i64 {
        if self.scores.len() <= index {
            self.scores.resize(index + 1, 0);
        }
        &mut self.scores[index]
    }

    /// Advance `seconds` of real time with the players as they stand; the
    /// events of the step, in order.
    pub fn advance(&mut self, seconds: f64, participants: &[Participant]) -> Vec<FlowEvent> {
        if self.phase == Phase::Waiting {
            return std::mem::take(&mut self.events);
        }
        self.left -= seconds;
        match self.phase {
            Phase::Waiting => {}
            Phase::Assemble => {
                if self.left <= 0.0 {
                    if self.mode.sided() {
                        self.enter(Phase::Setup, self.rules.setup);
                    } else {
                        self.begin_round();
                    }
                }
            }
            Phase::Setup => {
                if self.left <= 0.0 {
                    self.begin_round();
                }
            }
            Phase::Round => self.play(seconds, participants),
            Phase::RoundEnd => {
                if self.left <= 0.0 {
                    self.next_round();
                }
            }
            Phase::MatchEnd => {
                if self.left <= 0.0 && !self.events.contains(&FlowEvent::Finished) {
                    self.left = 0.0;
                    self.events.push(FlowEvent::Finished);
                }
            }
        }
        std::mem::take(&mut self.events)
    }

    fn scaled(&self, seconds: f64) -> f64 {
        seconds * self.timers.scale
    }

    fn enter(&mut self, phase: Phase, seconds: f64) {
        self.phase = phase;
        self.left = self.scaled(seconds);
        self.events.push(FlowEvent::Phase(phase));
        let message = |message, parameters: Variables| FlowEvent::Message {
            message,
            parameters,
        };
        self.events.push(message(PHASE_MESSAGE, Variables::new()));
        match phase {
            Phase::Waiting => self.events.push(message(WAITING_MESSAGE, Variables::new())),
            Phase::Setup => self.events.push(message(
                SETUP_MESSAGE,
                Variables::from([(SETUP_CLOCK, Value::Float(self.left))]),
            )),
            Phase::Round => self.events.push(message(
                ROUND_MESSAGE,
                Variables::from([(ROUND_STARTED, Value::Bool(true))]),
            )),
            Phase::RoundEnd => self
                .events
                .push(message(ROUND_END_MESSAGE, Variables::new())),
            Phase::MatchEnd => {
                let winner = (0..2u8)
                    .find(|&team| self.standings.get(usize::from(team)) == Some(&Standing::Won))
                    .filter(|_| self.mode != Mode::Deathmatch)
                    .map_or(Value::Null, |team| Value::Integer(i64::from(team)));
                self.events.push(message(
                    MATCH_END_MESSAGE,
                    Variables::from([(WINNER, winner)]),
                ));
            }
            Phase::Assemble => {}
        }
    }

    fn begin_round(&mut self) {
        self.round += 1;
        self.overtime = None;
        self.seen_alive = [false; 2];
        let seconds = match self.mode {
            Mode::Assault | Mode::Escort | Mode::Hybrid => {
                self.sided.as_ref().map_or(ESCORT_TIME, |round| round.time)
            }
            Mode::Deathmatch | Mode::TeamDeathmatch | Mode::Elimination => self.rules.round_time,
            // No round clock (rule 58), no score (rule 112).
            Mode::Control | Mode::Skirmish => 0.0,
        };
        if self.mode == Mode::Control {
            let stage = self.stage(self.round);
            self.control = Some(ControlPoint::new(
                stage,
                self.scaled(self.rules.control_unlock),
                self.rules.control,
            ));
        }
        if self.mode == Mode::Elimination {
            self.tiebreak = arena_centre(&self.map).map(|centre| {
                ControlPoint::tiebreak(
                    Area {
                        centre,
                        radius: POINT_RADIUS,
                        below: POINT_BELOW,
                        above: POINT_ABOVE,
                    },
                    self.scaled(self.rules.tiebreak),
                )
            });
        }
        self.enter(Phase::Round, seconds);
    }

    /// Rule 59: a Control stage per round. Assumption: the stages in their
    /// order (rule 59 says random; the order keeps rounds reproducible).
    fn stage(&self, round: u8) -> &orpheus_content::maps::MapObjective {
        let stages: Vec<_> = self
            .map
            .objectives_of(ObjectiveKind::ControlPoint)
            .collect();
        stages[usize::from(round.max(1) - 1) % stages.len()]
    }

    /// The next round after a round's end, or the match's end.
    fn next_round(&mut self) {
        if !self.standings.is_empty() {
            self.enter(Phase::MatchEnd, MATCH_END);
            return;
        }
        match self.mode {
            Mode::Assault | Mode::Escort | Mode::Hybrid => {
                // Rule 2: the sides swap (or an extra round starts) after
                // a hero select of 25 s.
                let (attackers, time) = self.upcoming.pop_front().expect("a side to play");
                self.sided = Some(
                    SidedRound::new(
                        &self.map,
                        self.mode,
                        &self.rules,
                        attackers,
                        time,
                        self.extra,
                    )
                    .expect("the map's course"),
                );
                self.enter(Phase::Assemble, self.rules.assemble_later);
            }
            Mode::Elimination => self.enter(Phase::Assemble, self.rules.hero_select),
            // Rule 8: no source gives the time between Control rounds; the
            // point's 30 s lock is the only pause.
            _ => self.begin_round(),
        }
    }

    fn play(&mut self, seconds: f64, participants: &[Participant]) {
        match self.mode {
            Mode::Assault | Mode::Escort | Mode::Hybrid => self.play_sided(seconds, participants),
            Mode::Control => self.play_control(seconds, participants),
            Mode::Deathmatch => {
                let best = self.scores.iter().copied().max().unwrap_or(0);
                if best >= self.rules.score_to_win || self.left <= 0.0 {
                    self.decide_free_for_all(participants);
                }
            }
            Mode::TeamDeathmatch => {
                let [blue, red] = [self.team_score(0), self.team_score(1)];
                let goal = self.rules.score_to_win;
                if blue >= goal || red >= goal || self.left <= 0.0 {
                    let winner = match blue.cmp(&red) {
                        std::cmp::Ordering::Greater => Some(0),
                        std::cmp::Ordering::Less => Some(1),
                        std::cmp::Ordering::Equal => None,
                    };
                    self.decide(winner);
                }
            }
            Mode::Elimination => self.play_elimination(seconds, participants),
            Mode::Skirmish => {}
        }
    }

    fn play_sided(&mut self, seconds: f64, participants: &[Participant]) {
        let scale = self.timers.scale;
        let speeds = payload_speeds(&self.map.name);
        let rollback = self.rules.rollback_speed;
        let round = self.sided.as_mut().expect("a sided round");
        let attackers = round.attackers;
        let extra = round.extra;
        let mut taken = Vec::new();
        let mut bonus = 0.0;
        match &mut round.course {
            Course::Points(points) => {
                let last = points.len() - 1;
                if let Some((index, point)) = points
                    .iter_mut()
                    .enumerate()
                    .find(|(_, point)| !point.captured())
                {
                    let presence = point.area.presence(participants, attackers);
                    if point.step(seconds, scale, presence) {
                        taken.push(index as u32);
                        if index != last {
                            // Rule 29: A taken in overtime goes on with 3:00
                            // (the bonus is added to what is left, 0); rule
                            // 69: 30 s in an extra round.
                            bonus = if extra {
                                EXTRA_ASSAULT_BONUS
                            } else {
                                ASSAULT_BONUS
                            };
                        }
                    }
                }
            }
            Course::Hybrid { point, payload } => match payload {
                None => {
                    let presence = point.area.presence(participants, attackers);
                    if point.step(seconds, scale, presence) {
                        taken.push(0);
                        // Rule 69: nothing in an extra round.
                        if !extra {
                            bonus = HYBRID_BONUS;
                        }
                        let checkpoints: Vec<_> = self
                            .map
                            .objectives_of(ObjectiveKind::Checkpoint)
                            .map(|objective| objective.position)
                            .collect();
                        let start = self
                            .map
                            .objectives_of(ObjectiveKind::Payload)
                            .next()
                            .map_or(point.area.centre, |payload| payload.position);
                        // Rule 51: the payload moves once A is taken.
                        *payload = Some(Payload::new(start, &checkpoints, speeds, rollback));
                    }
                }
                Some(payload) => {
                    let presence = payload.area().presence(participants, attackers);
                    if let Some(reached) = payload.step(seconds, scale, presence) {
                        taken.push(reached as u32);
                        if !payload.delivered() && !extra {
                            bonus = CHECKPOINT_BONUS[(reached - 1).min(1)];
                        }
                    }
                }
            },
            Course::Payload(payload) => {
                let presence = payload.area().presence(participants, attackers);
                if let Some(reached) = payload.step(seconds, scale, presence) {
                    taken.push((reached - 1) as u32);
                    if !payload.delivered() && !extra {
                        bonus = CHECKPOINT_BONUS[(reached - 1).min(1)];
                    }
                }
            }
        }
        let complete = round.course.complete();
        for objective in taken {
            self.events.push(FlowEvent::Objective(objective));
        }
        if complete {
            self.end_side();
            return;
        }
        if bonus > 0.0 {
            self.left = self.left.max(0.0) + self.scaled(bonus);
            // Rule 28: a checkpoint in overtime ends it.
            if self.overtime.take().is_some() {
                self.events.push(FlowEvent::Overtime(false));
            }
        }
        if self.left > 0.0 {
            return;
        }
        // Rule 25: with time out, an attacker on the objective holds
        // overtime; the fuse burns while none is.
        let round = self.sided.as_ref().expect("a sided round");
        let presence = round.course.presence(participants, attackers);
        self.overtime_step(seconds, presence.attackers > 0);
    }

    /// Overtime with time out: `held` while the attackers (or in Control
    /// the challengers) are on the objective. True when the fuse burned out.
    fn overtime_step(&mut self, seconds: f64, held: bool) {
        let scale = self.timers.scale;
        let fuse_for = |elapsed: f64| {
            FUSE.iter()
                .rev()
                .find(|(after, _)| elapsed >= after * scale)
                .map_or(FUSE[0].1, |(_, fuse)| *fuse)
                * scale
        };
        match &mut self.overtime {
            None if held => {
                self.overtime = Some(Overtime {
                    elapsed: 0.0,
                    fuse: fuse_for(0.0),
                });
                self.events.push(FlowEvent::Overtime(true));
            }
            None => self.end_side(),
            Some(overtime) => {
                overtime.elapsed += seconds;
                let full = fuse_for(overtime.elapsed);
                if held {
                    overtime.fuse = full;
                } else {
                    overtime.fuse = overtime.fuse.min(full) - seconds;
                    if overtime.fuse <= 0.0 {
                        self.end_side();
                    }
                }
            }
        }
    }

    /// The attacking side's round is over: the match goes on with the
    /// other side attacking (Competitive) or is decided.
    fn end_side(&mut self) {
        if self.overtime.take().is_some() {
            self.events.push(FlowEvent::Overtime(false));
        }
        let mut round = self.sided.take().expect("a sided round");
        round.result = Some(round.course.progress());
        let attackers = round.attackers;
        let complete = round.course.complete();
        if complete {
            round.bank = Some(self.left.max(0.0) / self.timers.scale);
        }
        self.sided = Some(round.clone());
        self.played.push(round);
        if !self.competitive {
            // Quick Play plays one round: the attackers win by completing
            // the map, the defenders by holding it.
            let winner = if complete { attackers } else { 1 - attackers };
            self.events.push(FlowEvent::RoundOver(Some(winner)));
            self.decide(Some(winner));
            return;
        }
        if !self.upcoming.is_empty() {
            self.enter(Phase::RoundEnd, ROUND_END);
            return;
        }
        let score = |side: &SidedRound| side.result.unwrap_or((0, 0.0));
        let mut banks = None;
        let winner = match self.played.as_slice() {
            // Rule 69: the lone attackers win with a tick (33.3%) or a
            // moved payload; otherwise the match is a draw.
            [side] if self.single => {
                let (taken, next) = score(side);
                (taken > 0 || next > 0.0).then_some(side.attackers)
            }
            // Rules 63-65: more objectives, or more progress on the next
            // one, wins; progress on a point counts by ticks (33.3%).
            [.., first, second] => {
                let (a, b) = (score(first), score(second));
                match a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)) {
                    std::cmp::Ordering::Greater => Some(first.attackers),
                    std::cmp::Ordering::Less => Some(second.attackers),
                    // Rule 67: both completed the map: extra rounds on
                    // their time banks. Otherwise a tie is a draw (rule 30).
                    std::cmp::Ordering::Equal => {
                        if let (Some(one), Some(other)) = (first.bank, second.bank) {
                            let mut by_team = [0.0; 2];
                            by_team[usize::from(first.attackers)] = one;
                            by_team[usize::from(second.attackers)] = other;
                            banks = Some((by_team, first.attackers));
                        }
                        None
                    }
                }
            }
            _ => None,
        };
        if let Some((by_team, first)) = banks {
            self.extra_rounds(by_team, first);
            self.enter(Phase::RoundEnd, ROUND_END);
            return;
        }
        self.events.push(FlowEvent::RoundOver(winner));
        self.decide(winner);
    }

    /// Rules 67-69: the extra rounds when both teams completed the map,
    /// from the time each had left (`banks` per team, unscaled).
    /// - Assault and Hybrid: a team with no time against one with at least
    ///   1:00: only the other attacks (rule 69).
    /// - Both at 2:00 or more: both are cut by the same amount until the
    ///   lower is 2:00 (rules 67 and 68).
    /// - Either under 1:00: the lower is raised to 1:00 and the other by
    ///   the same amount (rule 67; both at 0 s get 1:00, rule 69).
    ///
    /// The team with less time attacks first (rule 67). Assumption: with
    /// equal banks the team that attacked first in the set attacks first
    /// again. Assumption: each extra side opens hero select for 25 s and
    /// has setup, as the second side does.
    fn extra_rounds(&mut self, mut banks: [f64; 2], first: u8) {
        self.played.clear();
        self.extra = true;
        let low = banks[0].min(banks[1]);
        let high = banks[0].max(banks[1]);
        let (cap, floor) = (self.rules.bank_cap, self.rules.bank_floor);
        self.single =
            matches!(self.mode, Mode::Assault | Mode::Hybrid) && low <= 0.0 && high >= floor;
        if self.single {
            let team = if banks[0] >= floor { 0 } else { 1 };
            self.upcoming.push_back((team, banks[usize::from(team)]));
            return;
        }
        let shift = if low >= cap {
            cap - low
        } else if low < floor {
            floor - low
        } else {
            0.0
        };
        for bank in &mut banks {
            *bank += shift;
        }
        let first = if banks[0] == banks[1] {
            first
        } else if banks[0] < banks[1] {
            0
        } else {
            1
        };
        let second = 1 - first;
        self.upcoming.push_back((first, banks[usize::from(first)]));
        self.upcoming
            .push_back((second, banks[usize::from(second)]));
    }

    fn play_control(&mut self, seconds: f64, participants: &[Participant]) {
        let scale = self.timers.scale;
        let point = self.control.as_mut().expect("a Control round");
        let won = point.step(seconds, scale, participants);
        let contested = point.contested(participants);
        let at_hold = point
            .owner()
            .is_some_and(|owner| point.percent(owner) >= self.rules.control.hold_at);
        match (self.overtime.is_some(), at_hold && contested) {
            (false, true) => {
                self.overtime = Some(Overtime {
                    elapsed: 0.0,
                    fuse: 0.0,
                });
                self.events.push(FlowEvent::Overtime(true));
            }
            (true, false) => {
                self.overtime = None;
                self.events.push(FlowEvent::Overtime(false));
            }
            _ => {}
        }
        if let Some(team) = won {
            self.overtime = None;
            self.rounds_won[usize::from(team)] += 1;
            self.events.push(FlowEvent::RoundOver(Some(team)));
            if i64::from(self.rounds_won[usize::from(team)]) >= self.rules.score_to_win {
                self.decide(Some(team));
            } else {
                self.enter(Phase::RoundEnd, ROUND_END);
            }
        }
    }

    /// Rule 99: a team with no one left alive loses the round; a round
    /// still on after the rules' round time (135 s) is a draw. A team
    /// counts once one of its players was alive in the round, so the
    /// respawns at its start come first. Rule 100: from the rules'
    /// tiebreak (105 s) on, the team that captures the arena's point (3 s,
    /// any enemy on it blocks) wins the round.
    fn play_elimination(&mut self, seconds: f64, participants: &[Participant]) {
        let alive = |team: u8| {
            participants
                .iter()
                .any(|participant| participant.team == team && participant.alive())
        };
        for team in 0..2u8 {
            self.seen_alive[usize::from(team)] |= alive(team);
        }
        let scale = self.timers.scale;
        let captured = self.tiebreak.as_mut().and_then(|point| {
            point.step(seconds, scale, participants);
            point.owner()
        });
        let out = |team: u8| self.seen_alive[usize::from(team)] && !alive(team);
        let winner = match (out(0), out(1)) {
            (false, true) => Some(0),
            (true, false) => Some(1),
            (true, true) => None,
            (false, false) if captured.is_some() => captured,
            (false, false) if self.left <= 0.0 => None,
            (false, false) => return,
        };
        self.tiebreak = None;
        self.events.push(FlowEvent::RoundOver(winner));
        if let Some(team) = winner {
            self.rounds_won[usize::from(team)] += 1;
            if i64::from(self.rounds_won[usize::from(team)]) >= self.rules.score_to_win {
                self.decide(Some(team));
                return;
            }
        }
        self.enter(Phase::RoundEnd, ROUND_END);
    }

    fn decide_free_for_all(&mut self, participants: &[Participant]) {
        let mut ranked: Vec<(usize, i64)> = participants
            .iter()
            .map(|participant| (participant.slot, self.player_score(participant.slot)))
            .collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let slots = participants
            .iter()
            .map(|participant| participant.slot + 1)
            .max()
            .unwrap_or(0)
            .max(self.scores.len());
        self.standings = vec![Standing::Lost; slots];
        for (slot, _) in ranked.into_iter().take(DEATHMATCH_WINNERS) {
            self.standings[slot] = Standing::Won;
        }
        self.events.push(FlowEvent::MatchOver);
        self.enter(Phase::RoundEnd, ROUND_END);
    }

    /// The match is decided for the teams: a winner or a draw.
    fn decide(&mut self, winner: Option<u8>) {
        self.standings = (0..2u8)
            .map(|team| match winner {
                None => Standing::Drew,
                Some(winner) if winner == team => Standing::Won,
                Some(_) => Standing::Lost,
            })
            .collect();
        self.events.push(FlowEvent::MatchOver);
        self.enter(Phase::RoundEnd, ROUND_END);
    }
}

#[cfg(test)]
mod tests;
