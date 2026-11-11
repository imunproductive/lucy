//! The match flow's timers and thresholds as the install has them
//! (docs/dev/hardcoded-values.md §1), in place of the copies the Python
//! server kept (`ow174/game/match.py`):
//! - the mode graphs' values: hero select and setup (`0F4A`), the time
//!   banks of the extra rounds (`0F4A`), the Control point's lock, score
//!   rate and overtime hold (`0EC2`), and the payload's rollback (`1225`);
//! - the capture area's progress marks (`game_objectives_174.json`);
//! - the mode's ruleset defaults (`game_rulesets_174.json`): score to win,
//!   time limits, Elimination's hero select and tiebreaker, and the speed
//!   settings.
//!
//! A graph value is found by its graph and node uid; the node's class and
//! the variable it writes are checked, and its expression is evaluated with
//! the inputs named here. An expression that reads anything else, or a node
//! that is gone or changed, fails the load: nothing falls back to a copy.

use std::{fs, io, path::Path};

use orpheus_script::{
    expr::{Context, Variable, evaluate, to_float},
    graph::{GraphStore, Node},
    rulesets::Rulesets,
    wire::Value,
};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::match_flow::{
    ASSAULT, ASSAULT_TEST, BOUNTY_HUNTER, CLOCK, CONTROL, DEATHMATCH, ELIMINATION, ESCORT, HYBRID,
    Mode, Phase, SKIRMISH, TEAM_DEATHMATCH,
};

/// The derived file of the objectives' component data, in the runtime
/// `data` folder (the `orpheus-assets` derivation `game_objectives`).
pub const FILE: &str = "game_objectives_174.json";

/// The modes the server runs a flow for; their rules are checked at load.
const FLOW_MODES: [u64; 10] = [
    ASSAULT,
    ASSAULT_TEST,
    ESCORT,
    HYBRID,
    CONTROL,
    SKIRMISH,
    ELIMINATION,
    TEAM_DEATHMATCH,
    DEATHMATCH,
    BOUNTY_HUNTER,
];

/// The mode graphs of Assault, Escort and Hybrid, and of Control; the
/// payload's graph.
const SIDED_GRAPH: u32 = 0x0F4A;
const CONTROL_GRAPH: u32 = 0x0EC2;
const PAYLOAD_GRAPH: u32 = 0x1225;

const SET_VAR: &str = "STUStatescriptActionSetVar";
const STACK: &str = "STUStatescriptStateStack";
const BRANCH: &str = "STU_3387AB5D";
/// The payload's move along its path (`m_speed`).
const PATH_MOVE: &str = "STU_49FE0019";
const RULESET_VALUE: &str = "STU_3832D36C";

/// 0F4A: the round count (uid 529 adds 1 to it) and a flag uids 977/978
/// set, both read by the hero select time (uid 896).
const ROUNDS: u32 = 7901;
const ROUND_FLAG: u32 = 13205;
/// 0F4A: the two teams' time banks, the shift that cuts them to the cap
/// (uid 1297), the teams raised to the floor and the raise (uid 673), the
/// teams under the floor (uid 698).
const BANKS: u32 = 10806;
const BANK_CUT: u32 = 28932;
const RAISED: u32 = 12168;
const BANK_RAISE: u32 = 12143;
const UNDER_FLOOR: u32 = 12242;
/// The speed factor the mode graphs set (uid 525 of 0F4A and 0EC2), which
/// 0EC2's score rate and 1225's rollback multiply by.
const SPEED_FACTOR: u32 = 10354;
/// 0EC2: the owner's score rate in percent a second (uid 511), the
/// percentage overtime holds (uid 879), the phase (v59) it reads.
const SCORE_RATE: u32 = 2316;
const HOLD: u32 = 10367;
const PHASE: u32 = 59;
/// 1225: the payload's rollback speed setting, `-0.3 * v9671` (uid 44);
/// uid 376 sets v9671 to the payload speed setting times v10354.
const ROLLBACK_FACTOR: u32 = 9671;

/// The ruleset entries (`01C` indexes) the flow reads, with the custom game
/// setting each one is (their defaults match the settings' documented
/// defaults; game-rules.md).
/// "Score to win": rounds in Control and Elimination, kills in the
/// Deathmatches.
const SCORE_TO_WIN: u32 = 8853;
/// "Time limit" in minutes: Team Deathmatch, Deathmatch.
const TEAM_DEATHMATCH_TIME: u32 = 11298;
const DEATHMATCH_TIME: u32 = 11299;
/// Elimination: "Hero selection time", "Draw after" (seconds without the
/// tiebreaker) and "Capture objective tiebreaker" (the point opens after).
const HERO_SELECT: u32 = 8990;
const DRAW_AFTER: u32 = 10585;
const TIEBREAK_AFTER: u32 = 10586;
/// Control's "Score speed modifier" (0EC2 uid 511 reads it) and the
/// payload's "Payload speed modifier" (1225 uid 376).
const SCORE_SPEED: u32 = 9519;
const PAYLOAD_SPEED: u32 = 9520;

/// The tolerance a capture's progress reaches a mark with: the marks are
/// `f32`s (1/3 is 0.33333334), progress is added up in `f64`.
pub const MARK_TOLERANCE: f64 = 1e-6;

/// The Control point's scoring (rules 57 and 27).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlScoring {
    /// The owner's percentage a second (0EC2 uid 511: 100 / (2 × 60), 1%
    /// every 1.2 s, times the score speed setting and v10354).
    pub percent_per_second: f64,
    /// The percentage overtime holds the owner at while the other team
    /// contests (0EC2 uid 879: 99 in the round, phase 3).
    pub hold_at: f64,
}

/// The rules of one mode's match flow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeRules {
    /// Hero select before the first round (0F4A uid 896, 30 s).
    pub assemble: f64,
    /// Hero select before a later round of Assault, Escort and Hybrid (0F4A
    /// uid 896, 25 s).
    pub assemble_later: f64,
    /// Setup (0F4A uid 895 sets the clock v1303 to 45).
    pub setup: f64,
    /// The extra rounds' time banks are cut to this (0F4A uid 1297, checked
    /// against uid 1298: 120 s) ...
    pub bank_cap: f64,
    /// ... or raised to this (0F4A uid 673, checked against uid 698: 60 s).
    pub bank_floor: f64,
    /// The Control point unlocks this far into the round (0EC2 uids 310,
    /// 830 and 831 set the clock to 30).
    pub control_unlock: f64,
    /// Control's scoring; a rate of 0 in the other modes.
    pub control: ControlScoring,
    /// The marks a capture point's progress never falls below once reached
    /// (`STUCaptureAreaComponent.m_segments`: 1/3 and 2/3).
    pub capture_marks: [f64; 2],
    /// The payload's rollback speed in m/s (1225 uid 44: 0.3 times the
    /// payload speed setting and v10354); 0 in modes without a payload.
    pub rollback_speed: f64,
    /// The mode's "Score to win": rounds (Control, Elimination) or kills
    /// (Deathmatch, Team Deathmatch); 0 in the other modes.
    pub score_to_win: i64,
    /// The round's clock in seconds: the Deathmatches' time limit, or
    /// Elimination's draw; 0 in the other modes.
    pub round_time: f64,
    /// Elimination's hero select before a later round; 0 elsewhere.
    pub hero_select: f64,
    /// When Elimination's tiebreaker point unlocks; 0 elsewhere.
    pub tiebreak: f64,
}

/// The values every mode shares, read once when the scene assets load:
/// the graphs' and the capture area's. [`ModeRulesData::rules`] adds a
/// mode's ruleset defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeRulesData {
    assemble: f64,
    assemble_later: f64,
    setup: f64,
    bank_cap: f64,
    bank_floor: f64,
    control_unlock: f64,
    /// 0EC2 uid 511 at a score speed setting of 1.
    control_rate: f64,
    hold_at: f64,
    /// 0.3 times 0F4A's v10354: the rollback at a payload speed of 1.
    rollback_speed: f64,
    capture_marks: [f64; 2],
}

#[derive(Deserialize)]
struct Objectives {
    capture_area: CaptureArea,
}

#[derive(Deserialize)]
struct CaptureArea {
    segments: Vec<f64>,
}

impl ModeRulesData {
    /// Read the graphs' values from `graphs` and the capture area's from the
    /// runtime `data` folder, and check that every flow mode's ruleset
    /// defaults are there.
    pub fn load(graphs: &GraphStore, data: &Path) -> io::Result<Self> {
        let path = data.join(FILE);
        let objectives = fs::read(&path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        let rules = Self::read(graphs, &objectives)?;
        for mode in FLOW_MODES {
            rules.rules(graphs.rulesets(), mode)?;
        }
        Ok(rules)
    }

    /// Read the graphs' values and the capture area's marks (the
    /// `game_objectives_174.json` bytes).
    pub fn read(graphs: &GraphStore, objectives: &[u8]) -> io::Result<Self> {
        let objectives: Objectives = serde_json::from_slice(objectives).map_err(invalid)?;
        let capture_marks: [f64; 2] =
            objectives
                .capture_area
                .segments
                .try_into()
                .map_err(|segments: Vec<f64>| {
                    invalid(format!(
                        "{FILE}: the capture area has {} marks, not 2",
                        segments.len()
                    ))
                })?;
        if !(0.0 < capture_marks[0]
            && capture_marks[0] < capture_marks[1]
            && capture_marks[1] < 1.0)
        {
            return Err(invalid(format!(
                "{FILE}: the capture marks {capture_marks:?} don't rise within (0, 1)"
            )));
        }
        let reader = Reader(graphs);
        let (assemble, assemble_later) = reader.assemble()?;
        let (bank_cap, bank_floor) = reader.banks()?;
        let (control_rate, hold_at) = reader.control_scoring()?;
        Ok(Self {
            assemble,
            assemble_later,
            setup: reader.clock(SIDED_GRAPH, 895)?,
            bank_cap,
            bank_floor,
            control_unlock: reader.control_unlock()?,
            control_rate,
            hold_at,
            rollback_speed: reader.rollback()?,
            capture_marks,
        })
    }

    /// The rules of a mode's flow: these values and the mode's ruleset
    /// defaults. Fails when the mode's schemas lack an entry its flow plays
    /// by.
    pub fn rules(&self, rulesets: &Rulesets, mode: u64) -> io::Result<ModeRules> {
        let number = |key: u32| -> io::Result<f64> {
            match rulesets.default(key, 0, mode) {
                Some(Value::Integer(value)) => Ok(value as f64),
                Some(Value::Float(value)) => Ok(value),
                other => Err(invalid(format!(
                    "mode {mode:016X}: ruleset entry {key} is {other:?}, not a number"
                ))),
            }
        };
        let whole = |key: u32| -> io::Result<i64> {
            match rulesets.default(key, 0, mode) {
                Some(Value::Integer(value)) => Ok(value),
                other => Err(invalid(format!(
                    "mode {mode:016X}: ruleset entry {key} is {other:?}, not an integer"
                ))),
            }
        };
        let mut rules = ModeRules {
            assemble: self.assemble,
            assemble_later: self.assemble_later,
            setup: self.setup,
            bank_cap: self.bank_cap,
            bank_floor: self.bank_floor,
            control_unlock: self.control_unlock,
            control: ControlScoring {
                percent_per_second: 0.0,
                hold_at: self.hold_at,
            },
            capture_marks: self.capture_marks,
            rollback_speed: 0.0,
            score_to_win: 0,
            round_time: 0.0,
            hero_select: 0.0,
            tiebreak: 0.0,
        };
        match Mode::of(mode) {
            Some(Mode::Control) => {
                rules.score_to_win = whole(SCORE_TO_WIN)?;
                rules.control.percent_per_second = self.control_rate * number(SCORE_SPEED)?;
            }
            Some(Mode::Elimination) => {
                rules.score_to_win = whole(SCORE_TO_WIN)?;
                rules.round_time = number(DRAW_AFTER)?;
                rules.hero_select = number(HERO_SELECT)?;
                rules.tiebreak = number(TIEBREAK_AFTER)?;
            }
            Some(Mode::Deathmatch) => {
                rules.score_to_win = whole(SCORE_TO_WIN)?;
                rules.round_time = number(DEATHMATCH_TIME)? * 60.0;
            }
            Some(Mode::TeamDeathmatch) => {
                rules.score_to_win = whole(SCORE_TO_WIN)?;
                rules.round_time = number(TEAM_DEATHMATCH_TIME)? * 60.0;
            }
            Some(Mode::Escort | Mode::Hybrid) => {
                rules.rollback_speed = self.rollback_speed * number(PAYLOAD_SPEED)?;
            }
            Some(Mode::Assault | Mode::Skirmish) | None => {}
        }
        Ok(rules)
    }
}

/// Reads the values from the graphs.
struct Reader<'a>(&'a GraphStore);

impl Reader<'_> {
    /// A node, checked to be of `class`.
    fn node(&self, graph: u32, uid: u32, class: &str) -> io::Result<&Node> {
        let node = self
            .0
            .graph(graph)
            .ok_or_else(|| invalid(format!("graph {graph:04X} is missing")))?
            .nodes
            .iter()
            .find(|node| node.uid == uid)
            .ok_or_else(|| invalid(format!("graph {graph:04X} has no node uid {uid}")))?;
        if node.class != class {
            return Err(invalid(format!(
                "{graph:04X} uid {uid} is a {}, not a {class}",
                node.class
            )));
        }
        Ok(node)
    }

    /// A node of `class` that writes `variable` (its `m_out_Var`).
    fn writer(&self, graph: u32, uid: u32, class: &str, variable: u32) -> io::Result<&Node> {
        let node = self.node(graph, uid, class)?;
        let writes = node.field("m_out_Var.m_identifier").and_then(Json::as_u64);
        if writes != Some(u64::from(variable)) {
            return Err(invalid(format!(
                "{graph:04X} uid {uid} writes {writes:?}, not v{variable}"
            )));
        }
        Ok(node)
    }

    /// The seconds a SetVar puts on the mode's clock (v1303).
    fn clock(&self, graph: u32, uid: u32) -> io::Result<f64> {
        let node = self.writer(graph, uid, SET_VAR, CLOCK)?;
        Ok(to_float(&value(graph, node, "m_value", &[], &[])?))
    }

    /// Hero select before the first round and before a later one (0F4A
    /// uid 896: 30 while v7901 is at most 1 or v13205 is set, else 25).
    fn assemble(&self) -> io::Result<(f64, f64)> {
        let node = self.writer(SIDED_GRAPH, 896, SET_VAR, CLOCK)?;
        let at = |round: i64| {
            let inputs = [
                (ROUNDS, Value::Integer(round)),
                (ROUND_FLAG, Value::Bool(false)),
            ];
            value(SIDED_GRAPH, node, "m_value", &inputs, &[]).map(|value| to_float(&value))
        };
        Ok((at(1)?, at(2)?))
    }

    /// The time bank's cap and floor. 0F4A uid 1297 cuts both banks by
    /// `min(bank) - cap`, which is `-cap` at empty banks; uid 1298 cuts
    /// when both are over the cap. Uid 673 raises them by `floor - bank`
    /// of a team to raise; uid 698 names the teams under the floor.
    fn banks(&self) -> io::Result<(f64, f64)> {
        let banks = |a: f64, b: f64| (BANKS, Value::Array(vec![Value::Float(a), Value::Float(b)]));
        let cut = self.writer(SIDED_GRAPH, 1297, SET_VAR, BANK_CUT)?;
        let cap = -to_float(&value(
            SIDED_GRAPH,
            cut,
            "m_value",
            &[banks(0.0, 0.0)],
            &[],
        )?);
        let over = self.node(SIDED_GRAPH, 1298, BRANCH)?;
        let over_at = |a, b| value(SIDED_GRAPH, over, "m_condition", &[banks(a, b)], &[]);
        if over_at(cap + 1.0, cap + 1.0)? != Value::Bool(true)
            || over_at(cap, cap + 1.0)? != Value::Bool(false)
        {
            return Err(invalid(format!(
                "{SIDED_GRAPH:04X} uid 1298 doesn't cut the banks over {cap} s"
            )));
        }
        let raise = self.writer(SIDED_GRAPH, 673, SET_VAR, BANK_RAISE)?;
        let raised = (
            RAISED,
            Value::Array(vec![Value::Bool(true), Value::Bool(false)]),
        );
        let floor = to_float(&value(
            SIDED_GRAPH,
            raise,
            "m_value",
            &[raised, banks(0.0, 0.0)],
            &[],
        )?);
        let under = self.writer(SIDED_GRAPH, 698, SET_VAR, UNDER_FLOOR)?;
        let teams = under
            .field("m_value.m_configVars")
            .and_then(Json::as_array)
            .filter(|teams| teams.len() == 2)
            .ok_or_else(|| invalid(format!("{SIDED_GRAPH:04X} uid 698 lists no two teams")))?;
        for (team, config) in teams.iter().enumerate() {
            let under_at = |bank: f64| {
                let at = || format!("{SIDED_GRAPH:04X} uid 698 team {team}");
                evaluate_with(at, config, &[banks(bank, bank)], &[])
            };
            if under_at(floor - 1.0)? != Value::Bool(true) || under_at(floor)? != Value::Bool(false)
            {
                return Err(invalid(format!(
                    "{SIDED_GRAPH:04X} uid 698 doesn't find team {team} under {floor} s"
                )));
            }
        }
        Ok((cap, floor))
    }

    /// The Control point's lock: 0EC2's three SetVars of the clock before
    /// a round (uids 310, 830 and 831), which must agree.
    fn control_unlock(&self) -> io::Result<f64> {
        let unlock = self.clock(CONTROL_GRAPH, 310)?;
        for uid in [830, 831] {
            let other = self.clock(CONTROL_GRAPH, uid)?;
            if other != unlock {
                return Err(invalid(format!(
                    "{CONTROL_GRAPH:04X} uid {uid} locks the point for {other} s, uid 310 for {unlock} s"
                )));
            }
        }
        Ok(unlock)
    }

    /// The owner's score rate at a score speed setting of 1 (0EC2 uid 511,
    /// with v10354 as uid 525 sets it) and the overtime hold in the round
    /// (uid 879 at phase 3).
    fn control_scoring(&self) -> io::Result<(f64, f64)> {
        let factor = self.speed_factor(CONTROL_GRAPH)?;
        let rate = self.writer(CONTROL_GRAPH, 511, STACK, SCORE_RATE)?;
        let rate = to_float(&value(
            CONTROL_GRAPH,
            rate,
            "m_value",
            &[(SPEED_FACTOR, factor)],
            &[(SCORE_SPEED, Value::Float(1.0))],
        )?);
        let hold = self.writer(CONTROL_GRAPH, 879, STACK, HOLD)?;
        let round = Value::Integer(Phase::Round.value());
        let hold_at = to_float(&value(
            CONTROL_GRAPH,
            hold,
            "m_value",
            &[(PHASE, round)],
            &[],
        )?);
        if rate <= 0.0 {
            return Err(invalid(format!(
                "{CONTROL_GRAPH:04X} uid 511 scores {rate}% a second"
            )));
        }
        Ok((rate, hold_at))
    }

    /// The rollback speed at a payload speed setting of 1: 1225 uid 44
    /// moves the payload at `-0.3 * v9671`; uid 376 sets v9671 to the
    /// payload speed setting (ruleset entry 9520) times v10354, which 0F4A
    /// (the mode graph of the payload modes) sets with uid 525.
    fn rollback(&self) -> io::Result<f64> {
        let factor = self.speed_factor(SIDED_GRAPH)?;
        let rollback = self.node(PAYLOAD_GRAPH, 44, PATH_MOVE)?;
        let speed = -to_float(&value(
            PAYLOAD_GRAPH,
            rollback,
            "m_speed",
            &[(ROLLBACK_FACTOR, Value::Float(1.0))],
            &[],
        )?);
        let setter = self.writer(PAYLOAD_GRAPH, 376, STACK, ROLLBACK_FACTOR)?;
        let reads = |config: &Json| reads_ruleset(config, PAYLOAD_SPEED);
        if !setter.field("m_value").is_some_and(reads) {
            return Err(invalid(format!(
                "{PAYLOAD_GRAPH:04X} uid 376 doesn't read ruleset entry {PAYLOAD_SPEED}"
            )));
        }
        if speed <= 0.0 {
            return Err(invalid(format!(
                "{PAYLOAD_GRAPH:04X} uid 44 rolls back at {speed} m/s"
            )));
        }
        Ok(speed * to_float(&factor))
    }

    /// v10354 as a mode graph's uid 525 sets it.
    fn speed_factor(&self, graph: u32) -> io::Result<Value> {
        let node = self.writer(graph, 525, SET_VAR, SPEED_FACTOR)?;
        value(graph, node, "m_value", &[], &[])
    }
}

/// Whether a config holds a GetGameRulesetValue of `key`.
fn reads_ruleset(config: &Json, key: u32) -> bool {
    match config {
        Json::Object(fields) => {
            let named =
                fields.get("$").and_then(Json::as_str) == Some(RULESET_VALUE) && names(config, key);
            named || fields.values().any(|field| reads_ruleset(field, key))
        }
        Json::Array(items) => items.iter().any(|item| reads_ruleset(item, key)),
        _ => false,
    }
}

/// Whether a config names the dynamic identifier `key` anywhere.
fn names(config: &Json, key: u32) -> bool {
    match config {
        Json::Object(fields) => {
            fields.get("m_identifier").and_then(Json::as_u64) == Some(u64::from(key))
                || fields.values().any(|field| names(field, key))
        }
        Json::Array(items) => items.iter().any(|item| names(item, key)),
        _ => false,
    }
}

/// A node's config `field` evaluated with these variables and ruleset
/// entries; reading anything else fails.
fn value(
    graph: u32,
    node: &Node,
    field: &str,
    variables: &[(u32, Value)],
    rulesets: &[(u32, Value)],
) -> io::Result<Value> {
    let at = || format!("{graph:04X} uid {}", node.uid);
    let config = node
        .field(field)
        .ok_or_else(|| invalid(format!("{} has no {field}", at())))?;
    evaluate_with(at, config, variables, rulesets)
}

/// A config evaluated with these inputs only; `at` names it in errors.
fn evaluate_with(
    at: impl Fn() -> String,
    config: &Json,
    variables: &[(u32, Value)],
    rulesets: &[(u32, Value)],
) -> io::Result<Value> {
    let mut inputs = Inputs {
        variables,
        rulesets,
        unknown: None,
    };
    let value =
        evaluate(config, &mut inputs).map_err(|error| invalid(format!("{}: {error}", at())))?;
    match inputs.unknown {
        Some(read) => Err(invalid(format!("{} reads {read}", at()))),
        None => Ok(value),
    }
}

/// An expression's inputs; the first other thing it reads is kept.
struct Inputs<'a> {
    variables: &'a [(u32, Value)],
    rulesets: &'a [(u32, Value)],
    unknown: Option<String>,
}

impl Inputs<'_> {
    fn input(&mut self, inputs: &[(u32, Value)], id: u32, name: &str) -> Value {
        match inputs.iter().find(|(key, _)| *key == id) {
            Some((_, value)) => value.clone(),
            None => {
                self.unknown.get_or_insert_with(|| format!("{name} {id}"));
                Value::Null
            }
        }
    }
}

impl Context for Inputs<'_> {
    fn variable(&mut self, variable: Variable) -> Value {
        self.input(self.variables, variable.id, "variable")
    }

    fn ruleset(&mut self, id: u32) -> Value {
        self.input(self.rulesets, id, "ruleset entry")
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// The rules 1.74's data gives a mode, for the tests that run without the
/// data; `tests::the_install_gives_the_fixture_rules` pins them to the
/// data. Expressions compute in `f32`, and the marks are `f32`s.
#[cfg(test)]
pub(crate) fn fixture(mode: u64) -> ModeRules {
    let single = |value: f64| f64::from(value as f32);
    let mut rules = ModeRules {
        assemble: 30.0,
        assemble_later: 25.0,
        setup: 45.0,
        bank_cap: 120.0,
        bank_floor: 60.0,
        control_unlock: 30.0,
        control: ControlScoring {
            percent_per_second: 0.0,
            hold_at: 99.0,
        },
        capture_marks: [0.33333334, 0.6666667],
        rollback_speed: 0.0,
        score_to_win: 0,
        round_time: 0.0,
        hero_select: 0.0,
        tiebreak: 0.0,
    };
    match Mode::of(mode) {
        Some(Mode::Control) => {
            rules.score_to_win = 2;
            rules.control.percent_per_second = single(100.0 / 120.0);
        }
        Some(Mode::Elimination) => {
            rules.score_to_win = 3;
            rules.round_time = 135.0;
            rules.hero_select = 20.0;
            rules.tiebreak = 105.0;
        }
        Some(Mode::Deathmatch) => {
            rules.score_to_win = 20;
            rules.round_time = 600.0;
        }
        Some(Mode::TeamDeathmatch) => {
            rules.score_to_win = 30;
            rules.round_time = 600.0;
        }
        Some(Mode::Escort | Mode::Hybrid) => rules.rollback_speed = single(0.3),
        Some(Mode::Assault | Mode::Skirmish) | None => {}
    }
    rules
}

#[cfg(test)]
mod tests;
