//! The match flow of the PvP modes on the server: phases, timers,
//! objectives, scoring and the match's end (docs/notes/game-rules.md §1-8,
//! 11, 16).
//!
//! In retail the mode's own graph drives this on the game mode entity
//! (`0F4A` for Assault, Escort and Hybrid, `0EC2` for Control): its phase
//! variable v59 (`003B`), its clock v1303 (`0517`) and its phase messages.
//! Assault runs on `0F4A` here too (`crate::mode_script`): the flow only
//! follows it ([`MatchFlow::follow`]) and plays no Assault of its own. The
//! VM can't run the other modes' graphs whole yet, so for them this
//! machine drives the same phases and publishes what the HUD and the
//! bodies read: the clock, the phase banner text, the objective's name,
//! the attacking team, and the phase messages. Its phase values are v59's
//! (ow1-preservation `Assault flow on Anubis`): 0 waiting, 1 assemble,
//! 2 setup, 3 round, 4 round end, 5 match end.
//!
//! Where the rules leave a value open, the machine takes an assumption,
//! named where it is used.
//!
//! Spawn rooms: the flow names the rooms each team uses
//! ([`MatchFlow::spawn_choice`]): in Escort and Hybrid the current
//! objective's attackers' or defenders' rooms (they move forward as the
//! objectives fall, and swap with the sides), in Control the stage's, in
//! Assault the mode graph's objective's.
//! Spawn doors: the doors are server placeables that run `0D32` (map
//! entities stage A); the mode graph holds them open with `0702`. Where
//! the flow drives, no `0702` goes, and the server's mover has no door
//! collision either way (docs/dev/loose-ends.md, refs `ModeGraphGaps.md`
//! "ModeGraph4"): nothing holds a human attacker in during setup.

use std::{collections::VecDeque, sync::Arc};

use lucy_content::maps::{GameMap, MapObjective, ObjectiveKind, SpawnChoice};
use lucy_script::wire::{Value, Variables};

use crate::{
    mode_graph_values::ModeGraphValues,
    mode_rules_data::ModeRules,
    objective_data::MapObjectives,
    objectives::{CapturePoint, ControlPoint, Participant, Payload, Presence},
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
/// The clock parameter of 0F4A's messages (`0A4:017E`).
const SETUP_CLOCK: u32 = 0x017e;
/// Set Match Time: the Workshop action `1ABC` sends `08D8` to the mode
/// entity with the seconds (`#18739[0]`, 0 to 3599) as `017E`; 0F4A's
/// entry for it (node 187) writes them into the phase's clock `#1303`,
/// which the phase states chase to 0 (Assemble's st7 then sets `#59` = 2,
/// setup).
pub const SET_MATCH_TIME_MESSAGE: u64 = 0x0240_0000_0000_08d8;
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

    /// The mode plays for the map's objectives (points, payload).
    pub fn has_objectives(self) -> bool {
        self.sided() || matches!(self, Self::Control | Self::Elimination)
    }

    /// The mode's own graph drives its matches (`0F4A`, Assault): the flow
    /// follows it ([`MatchFlow::follow`]) and never plays it itself.
    pub fn graph_driven(self) -> bool {
        self == Self::Assault
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

/// Assumption: how long the match's end shows before the players go home
/// where no mode graph drives the match ([`MatchFlow::advance`]). Neither
/// POTG nor the cards are played; their length is unknown (rule 167). A
/// graph-driven match ends when the graph ends the game
/// ([`MatchState::over`]).
pub const MATCH_END: f64 = 20.0;
/// Rule 95: the top 4 of a free-for-all get win credit.
pub const DEATHMATCH_WINNERS: usize = 4;

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
    /// The match was decided; it ends once [`MATCH_END`] is over, or the
    /// mode graph ends the game ([`MatchState::over`]).
    MatchOver,
    /// The end screen is over: the players go home.
    Finished,
}

/// What the attackers of a sided round play for.
#[derive(Clone, Debug)]
enum Course {
    /// Hybrid: the point, then the payload (rule 51), which waits parked
    /// until the point falls.
    Hybrid {
        point: Box<CapturePoint>,
        payload: Option<Payload>,
        parked: Option<Box<Payload>>,
    },
    Payload(Payload),
}

impl Course {
    /// Progress for the tiebreak (rules 63-65): the objectives taken and
    /// the share of the next one that counts, its last capture mark.
    fn progress(&self) -> (u32, f64) {
        match self {
            Self::Hybrid { point, payload, .. } => match payload {
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
            Self::Hybrid { payload, .. } => payload.as_ref().is_some_and(Payload::delivered),
            Self::Payload(payload) => payload.delivered(),
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            Self::Hybrid {
                point,
                payload: None,
                ..
            } => point.name.as_deref(),
            _ => None,
        }
    }

    /// Where the attackers play: the next point, or the payload.
    fn position(&self) -> Option<[f64; 3]> {
        match self {
            Self::Hybrid {
                payload: Some(payload),
                ..
            }
            | Self::Payload(payload) => Some(payload.position()),
            Self::Hybrid { point, .. } => Some(point.position),
        }
    }

    /// Whether an attacker stands on the objective (it keeps overtime).
    fn presence(&self, participants: &[Participant], attackers: u8) -> Presence {
        match self {
            Self::Hybrid {
                payload: Some(payload),
                ..
            }
            | Self::Payload(payload) => payload.area().presence(participants, attackers),
            Self::Hybrid { point, .. } => point.area.presence(participants, attackers),
        }
    }
}

/// One side's round of Escort or Hybrid.
///
/// The clock: each objective adds its own time when it becomes the current
/// one (`objective_data.rs`, 0F4A uids 646-648), the first one's to the
/// team's bank at the round's start (0 in rounds 1 and 2, uid 620). With
/// Competitive rules: 4:00, +2:30, +1:30; without them: 5:00, +3:00,
/// +2:00; nothing in the extra rounds. Assumption: a match with a ranked
/// player plays the Competitive rulesets, whose "Competitive Rules"
/// (ruleset entry 9544) is on (`Payloads.md` §1); others play with it off.
#[derive(Clone, Debug)]
struct SidedRound {
    attackers: u8,
    course: Course,
    /// The clock it starts with, in unscaled seconds.
    time: f64,
    /// An extra round of the time bank (rules 67-69, 0F4A's v7901 at least
    /// 2).
    extra: bool,
    /// The round's result for the tiebreak.
    result: Option<(u32, f64)>,
    /// The time left when the attackers completed the map (unscaled).
    bank: Option<f64>,
}

#[derive(Clone, Debug)]
struct Overtime {
    elapsed: f64,
    /// Fuse seconds left.
    fuse: f64,
}

/// The match as the mode's own graph keeps it
/// (`crate::mode_script::ModeRunner::match_state`), which the flow follows
/// where the graph drives the match ([`MatchFlow::follow`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchState {
    pub phase: Phase,
    /// Seconds on the clock.
    pub clock: f64,
    pub winner: Option<u8>,
    /// The attacking team (`0F4A` #10795e).
    pub attackers: Option<u8>,
    /// The current objective's order (#10548e, 0x2934): the rooms the teams
    /// spawn in.
    pub objective: u32,
    /// Overtime (#3392e).
    pub overtime: bool,
    /// A graph ended the game (`STU_147B34C6`: `180A` after the match
    /// end's last card): the players go home.
    pub over: bool,
}

/// The match flow of a room.
#[derive(Clone, Debug)]
pub struct MatchFlow {
    mode: Mode,
    map: GameMap,
    /// The mode's timers and thresholds as the install has them.
    rules: ModeRules,
    timers: Timers,
    /// The mode graphs' values: the wait after a round, the overtime fuse
    /// and the capture points' rates.
    graph_values: ModeGraphValues,
    /// The map's objective data: the points' areas and times, the time
    /// each objective adds, the payload's track.
    objectives: Arc<MapObjectives>,
    /// Competitive: both teams attack (sided modes); otherwise one round.
    competitive: bool,
    phase: Phase,
    /// Seconds left in the phase; for a round, its clock.
    left: f64,
    round: u8,
    sided: Option<SidedRound>,
    /// Competitive: the sides still to play in this set (the second side,
    /// or the extra rounds), each its attackers and time bank.
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
    /// The mode graph's match while the flow follows it ([`Self::follow`]):
    /// its attackers, objective and overtime stand for the flow's own.
    graph: Option<MatchState>,
}

impl MatchFlow {
    /// The flow of a map's mode by the mode graphs' values, `rules` (the
    /// mode's) and the map's objective data; none for the modes the server
    /// doesn't run a flow for (Practice Range, Arcade brawls), or when the
    /// map lacks the objectives its mode plays for or their data. A mode
    /// its graph drives ([`Mode::graph_driven`]) gets a flow that only
    /// follows the graph.
    pub fn new(
        map: &GameMap,
        competitive: bool,
        graph_values: ModeGraphValues,
        rules: ModeRules,
        objectives: Arc<MapObjectives>,
        timers: Timers,
    ) -> Option<Self> {
        let mode = Mode::of(map.mode_guid)?;
        let mut flow = Self {
            mode,
            map: map.clone(),
            rules,
            timers,
            graph_values,
            objectives,
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
            graph: None,
        };
        if mode.sided() && !mode.graph_driven() {
            // game_maps.rs: team 1 attacks first; in Competitive team 0
            // attacks second, both from an empty bank.
            flow.sided = Some(flow.sided_round(1, 0.0, false)?);
            if competitive {
                flow.upcoming.push_back((0, 0.0));
            }
        }
        if mode == Mode::Control {
            let mut stages = map.objectives_of(ObjectiveKind::ControlPoint).peekable();
            stages.peek()?;
            for stage in stages {
                flow.objectives.point(stage.placeable)?;
            }
        }
        if mode == Mode::Elimination {
            flow.arena_point()?;
        }
        Some(flow)
    }

    /// A side's round: its course and its clock, the team's bank plus the
    /// first objective's time. None when an objective lacks its data.
    fn sided_round(&self, attackers: u8, bank: f64, extra: bool) -> Option<SidedRound> {
        let course = self.course()?;
        let objectives = self.map.objectives.iter().filter(|objective| {
            matches!(
                objective.kind,
                ObjectiveKind::Point | ObjectiveKind::Checkpoint
            )
        });
        for objective in objectives {
            self.objectives.times(objective.placeable)?;
        }
        Some(SidedRound {
            attackers,
            course,
            time: bank + self.objective_time(0, extra)?,
            extra,
            result: None,
            bank: None,
        })
    }

    /// The objectives of one side's round.
    fn course(&self) -> Option<Course> {
        let points: Vec<_> = self.map.objectives_of(ObjectiveKind::Point).collect();
        let capture = self.graph_values.sided_capture;
        let marks = self.rules.capture_marks;
        let point = |objective: &MapObjective| {
            let data = self.objectives.point(objective.placeable)?;
            Some(CapturePoint::new(objective, data, capture, marks))
        };
        match self.mode {
            Mode::Hybrid => Some(Course::Hybrid {
                point: Box::new(point(points.first()?)?),
                payload: None,
                parked: Some(Box::new(self.payload()?)),
            }),
            Mode::Escort => Some(Course::Payload(self.payload()?)),
            _ => None,
        }
    }

    /// The map's payload as it becomes active, heading for its checkpoints.
    fn payload(&self) -> Option<Payload> {
        let placeable = self
            .map
            .objectives_of(ObjectiveKind::Payload)
            .next()?
            .placeable;
        let checkpoints: Vec<u32> = self
            .map
            .objectives_of(ObjectiveKind::Checkpoint)
            .map(|checkpoint| checkpoint.placeable)
            .collect();
        Payload::new(
            self.objectives.payload(placeable)?,
            self.rules.payload?,
            &checkpoints,
            self.timers.scale,
        )
    }

    /// The unscaled seconds the objective of `order` adds when it becomes
    /// the current one.
    fn objective_time(&self, order: u32, extra: bool) -> Option<f64> {
        let objective = self
            .map
            .objectives
            .iter()
            .filter(|objective| {
                matches!(
                    objective.kind,
                    ObjectiveKind::Point | ObjectiveKind::Checkpoint
                )
            })
            .find(|objective| objective.order == order)?;
        let times = self.objectives.times(objective.placeable)?;
        Some(times.seconds(self.competitive, extra))
    }

    /// Elimination's tiebreaker point: the arena's one capture point (the
    /// arenas' points aren't `game_maps` objectives).
    fn arena_point(&self) -> Option<&crate::objective_data::PointData> {
        match self.objectives.points.as_slice() {
            [point] => Some(point),
            _ => None,
        }
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

    /// Overtime: the mode graph's (#3392e) while the flow follows it.
    pub fn in_overtime(&self) -> bool {
        match &self.graph {
            Some(graph) => graph.overtime,
            None => self.overtime.is_some(),
        }
    }

    /// Whether the mode's own graph drives the match ([`Self::follow`]):
    /// its phases, hero select and the bodies' overtime are its own.
    pub fn follows_graph(&self) -> bool {
        self.graph.is_some()
    }

    /// The seconds the clock shows.
    pub fn clock(&self) -> f64 {
        if self.overtime.is_some() || (self.mode == Mode::Control && self.phase == Phase::Round) {
            // Rule 58: Control has no round clock.
            return 0.0;
        }
        self.left.max(0.0)
    }

    /// The attacking team of a sided round: the mode graph's (#10795e)
    /// while the flow follows it.
    pub fn attackers(&self) -> Option<u8> {
        match &self.graph {
            Some(graph) => graph.attackers,
            None => self.sided.as_ref().map(|round| round.attackers),
        }
    }

    /// The spawn rooms `team` uses now: the current objective's attackers'
    /// or defenders' (rules 36 and 47: they move forward as objectives
    /// fall; the teams swap them with the sides), the Control stage's
    /// (rule 60), else the first round's. While the flow follows the mode
    /// graph, its objective (#10548e) and attackers (#10795e) name them.
    pub fn spawn_choice(&self, team: u8) -> SpawnChoice {
        if let Some(graph) = &self.graph
            && let Some(attackers) = graph.attackers
        {
            return SpawnChoice::Objective {
                objective: self.graph_objective(graph.objective),
                attack: team == attackers,
            };
        }
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

    /// Hero select stays open while the heroes assemble, by the flow's
    /// hand (the mode graph that drives the match holds it open itself:
    /// `0F4A` st9's `0692` → `288B` st32-st36).
    pub fn assembling(&self) -> bool {
        self.graph.is_none() && self.phase == Phase::Assemble
    }

    /// The mode graph's objective order, past the last once the attackers
    /// took it (`0F4A` rid121 counts on), as the map's last objective.
    fn graph_objective(&self, order: u32) -> u32 {
        let last = self
            .map
            .objectives
            .iter()
            .filter(|objective| {
                matches!(
                    objective.kind,
                    ObjectiveKind::Point | ObjectiveKind::Checkpoint
                )
            })
            .map(|objective| objective.order)
            .max()
            .unwrap_or(0);
        order.min(last)
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
    /// objective. While the flow follows the mode graph: its current
    /// objective where the map places it (a payload's checkpoint, not the
    /// payload).
    pub fn objective(&self) -> Option<[f64; 3]> {
        if let Some(graph) = &self.graph {
            let order = self.graph_objective(graph.objective);
            return self
                .map
                .objectives
                .iter()
                .filter(|objective| {
                    matches!(
                        objective.kind,
                        ObjectiveKind::Point | ObjectiveKind::Checkpoint
                    )
                })
                .find(|objective| objective.order == order)
                .map(|objective| objective.position);
        }
        if let Some(round) = &self.sided {
            return round.course.position();
        }
        if let Some(point) = &self.control {
            return Some(point.position);
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
    /// starts "assemble your team" at the first spawn). A mode its graph
    /// drives starts with the graph (`ModeRunner::start_match`), not here.
    /// Skirmish has no assembly: its graph `0D76` never sets a phase and
    /// counts its clock from its start, and Lúcioball's `0F9D`, the brawl
    /// the flow plays as Skirmish, goes from waiting to its setup (#59 = 2)
    /// without one; so its round begins at once.
    pub fn start(&mut self) {
        if self.phase != Phase::Waiting || self.mode.graph_driven() {
            return;
        }
        if self.mode == Mode::Skirmish {
            self.begin_round();
        } else {
            let seconds = self.rules.assemble;
            self.enter(Phase::Assemble, seconds);
        }
    }

    /// A message to the mode entity that 0F4A takes and the flow stands in
    /// for: Set Match Time ([`SET_MATCH_TIME_MESSAGE`]) gives the current
    /// phase that many seconds left (the Workshop's "Set Match Time":
    /// assembling heroes, setup or the round). Others are left to the mode
    /// entity's graph.
    pub fn receive(&mut self, message: u64, parameters: &Variables) {
        if message != SET_MATCH_TIME_MESSAGE || self.phase == Phase::Waiting {
            return;
        }
        let seconds = match parameters.get(&SETUP_CLOCK) {
            Some(Value::Float(seconds)) => *seconds,
            Some(Value::Integer(seconds)) => *seconds as f64,
            _ => return,
        };
        self.left = seconds.clamp(0.0, 3599.0);
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

    /// Follow the mode's own graph, which drives the match where it runs
    /// (`ModeRunner::match_state`): its match now, `seconds` after the last
    /// step. The flow keeps no timer and plays no objective of its own
    /// then; the graph holds its phase messages itself, so none go from
    /// here. A phase change is a [`FlowEvent::Phase`] (a round's start
    /// counts the round), a change of the graph's overtime a
    /// [`FlowEvent::Overtime`] and of its objective a
    /// [`FlowEvent::Objective`] (the one that fell); the match's end is
    /// decided by the graph's winner (`0F4A` #7525, a team, else a draw),
    /// and the players go home once the graph ends the game
    /// ([`MatchState::over`]: after the match end's cards).
    pub fn follow(&mut self, state: MatchState) -> Vec<FlowEvent> {
        let MatchState {
            phase,
            clock,
            winner,
            over,
            ..
        } = state;
        let before = self.graph.replace(state);
        if before.is_some_and(|before| before.overtime) != state.overtime {
            self.events.push(FlowEvent::Overtime(state.overtime));
        }
        if let Some(before) = before
            && state.objective > before.objective
        {
            self.events.push(FlowEvent::Objective(before.objective));
        }
        if phase != self.phase {
            self.phase = phase;
            match phase {
                Phase::Round => {
                    self.round += 1;
                }
                Phase::MatchEnd => {
                    self.standings = standings(winner);
                    self.events.push(FlowEvent::MatchOver);
                }
                _ => {}
            }
            self.events.push(FlowEvent::Phase(phase));
        }
        self.left = clock;
        if over {
            self.events.push(FlowEvent::Finished);
        }
        std::mem::take(&mut self.events)
    }

    fn scaled(&self, seconds: f64) -> f64 {
        seconds * self.timers.scale
    }

    /// A round's end, for as long as the mode's graph waits after a round.
    fn enter_round_end(&mut self) {
        self.enter(Phase::RoundEnd, self.graph_values.round_end(self.mode));
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
            Mode::Escort | Mode::Hybrid => self.sided.as_ref().map_or(0.0, |round| round.time),
            // Never started here (`Self::start`): its graph plays it.
            Mode::Assault => unreachable!("the mode graph plays Assault"),
            Mode::Deathmatch | Mode::TeamDeathmatch | Mode::Elimination => self.rules.round_time,
            // No round clock (rule 58), no score (rule 112).
            Mode::Control | Mode::Skirmish => 0.0,
        };
        if self.mode == Mode::Control {
            let stage = self.stage(self.round);
            let point = self
                .objectives
                .point(stage.placeable)
                .expect("MatchFlow::new checked the stages' data");
            self.control = Some(ControlPoint::new(
                stage,
                point,
                self.scaled(self.rules.control_unlock),
                self.graph_values.control_capture,
                self.rules.control,
            ));
        }
        if self.mode == Mode::Elimination {
            let point = self
                .arena_point()
                .expect("MatchFlow::new checked the arena's point");
            self.tiebreak = Some(ControlPoint::tiebreak(
                point,
                self.graph_values.elimination_capture,
                self.scaled(self.rules.tiebreak),
            ));
        }
        self.enter(Phase::Round, seconds);
    }

    /// Rule 59: a Control stage per round. Assumption: the stages in their
    /// order (rule 59 says random; the order keeps rounds reproducible).
    fn stage(&self, round: u8) -> &lucy_content::maps::MapObjective {
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
            Mode::Escort | Mode::Hybrid => {
                // Rule 2: the sides swap (or an extra round starts) after
                // a hero select of 25 s.
                let (attackers, bank) = self.upcoming.pop_front().expect("a side to play");
                self.sided = Some(
                    self.sided_round(attackers, bank, self.extra)
                        .expect("MatchFlow::new built the map's course"),
                );
                self.enter(Phase::Assemble, self.rules.assemble_later);
            }
            Mode::Elimination => self.enter(Phase::Assemble, self.rules.hero_select),
            Mode::Assault => unreachable!("the mode graph plays Assault"),
            // Rule 8: no source gives the time between Control rounds; the
            // point's 30 s lock is the only pause.
            _ => self.begin_round(),
        }
    }

    fn play(&mut self, seconds: f64, participants: &[Participant]) {
        match self.mode {
            Mode::Escort | Mode::Hybrid => self.play_sided(seconds, participants),
            Mode::Assault => unreachable!("the mode graph plays Assault"),
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
        let round = self.sided.as_mut().expect("a sided round");
        let attackers = round.attackers;
        let extra = round.extra;
        let current = round.course.objective();
        let mut taken = Vec::new();
        match &mut round.course {
            Course::Hybrid {
                point,
                payload,
                parked,
            } => match payload {
                None => {
                    let presence = point.area.presence(participants, attackers);
                    if point.step(seconds, scale, presence) {
                        taken.push(0);
                        // Rule 51: the payload moves once A is taken.
                        *payload = parked.take().map(|parked| *parked);
                    }
                }
                Some(payload) => {
                    let presence = payload.area().presence(participants, attackers);
                    if let Some(reached) = payload.step(seconds, scale, presence) {
                        taken.push(reached as u32);
                    }
                }
            },
            Course::Payload(payload) => {
                let presence = payload.area().presence(participants, attackers);
                if let Some(reached) = payload.step(seconds, scale, presence) {
                    taken.push((reached - 1) as u32);
                }
            }
        }
        let complete = round.course.complete();
        let next = round.course.objective();
        for objective in taken {
            self.events.push(FlowEvent::Objective(objective));
        }
        if complete {
            self.end_side();
            return;
        }
        // The next objective adds its time (rule 29: what is left, 0 in
        // overtime, plus the time).
        let bonus = if next == current {
            0.0
        } else {
            self.objective_time(next, extra)
                .expect("MatchFlow::new checked the objectives' times")
        };
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
        let fuse = self.graph_values.fuse;
        let fuse_for = |elapsed: f64| fuse.seconds(elapsed / scale) * scale;
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
            self.enter_round_end();
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
            self.enter_round_end();
            return;
        }
        self.events.push(FlowEvent::RoundOver(winner));
        self.decide(winner);
    }

    /// Rules 67-69: the extra rounds when both teams completed the map,
    /// from the time each had left (`banks` per team, unscaled).
    /// - Hybrid: a team with no time against one with at least 1:00: only
    ///   the other attacks (rule 69).
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
        self.single = self.mode == Mode::Hybrid && low <= 0.0 && high >= floor;
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
                self.enter_round_end();
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
        self.enter_round_end();
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
        self.enter_round_end();
    }

    /// The match is decided for the teams: a winner or a draw.
    fn decide(&mut self, winner: Option<u8>) {
        self.standings = standings(winner);
        self.events.push(FlowEvent::MatchOver);
        self.enter_round_end();
    }
}

/// Teams 0 and 1's standings when `winner` won (none: a draw).
fn standings(winner: Option<u8>) -> Vec<Standing> {
    (0..2u8)
        .map(|team| match winner {
            None => Standing::Drew,
            Some(winner) if winner == team => Standing::Won,
            Some(_) => Standing::Lost,
        })
        .collect()
}

#[cfg(test)]
mod tests;
