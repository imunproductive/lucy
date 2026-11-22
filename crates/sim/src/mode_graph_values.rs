//! Match values read from the mode graphs of the install: the wait after a
//! round, the overtime fuse and the capture points' rates.
//!
//! The VM can't run the mode graphs whole yet (`mode_script.rs`), so the
//! match flow (`match_flow.rs`) plays their phases itself. These values are
//! read from the graphs the server loads (`game_graphs_174.json.gz`). Each
//! one is found by its graph and node uid, and the node's class and the
//! variable it writes are checked. Data that no longer matches is an error
//! when the scene assets load, not a silent default.

use std::{collections::HashMap, io};

use lucy_script::{
    expr::{Context, Value, Variable, dynamic, evaluate, to_float},
    graph::{Graph, GraphStore, Node, asset_guid},
};
use serde_json::Value as Json;

use crate::match_flow::{ASSAULT, CONTROL, ELIMINATION, Mode};

/// The mode graph of Assault, Escort and Hybrid.
const SIDED_GRAPH: u32 = 0x0f4a;
/// The mode graph of Control.
const CONTROL_GRAPH: u32 = 0x0ec2;
/// The mode graph of Elimination.
const ELIMINATION_GRAPH: u32 = 0x0d0b;

/// The mode graphs' phase variable (`match_flow::Phase`).
const PHASE: u32 = 59;

const SET_VAR: &str = "STUStatescriptActionSetVar";
const WAIT: &str = "STUStatescriptStateWait";
const CHASE_VAR: &str = "STUStatescriptStateChaseVar";
const STACK: &str = "STUStatescriptStateStack";
/// A state that sends a game message with parameters.
const SEND_MESSAGE: &str = "STU_38EE1100";
const OUTPUT_PLUG: &str = "STUStatescriptOutputPlug";

/// How many nodes after a round's wait the next phase may be set.
const PHASE_SEARCH: usize = 8;

/// The wait after a round: the wait node, the node that sets the next
/// phase after it, and the node that sets the variable the wait reads, if
/// it reads one.
struct RoundEnd {
    graph: u32,
    wait: u32,
    next_phase: u32,
    timeout_from: Option<(u32, u32)>,
}

/// 0F4A: wait 359 (11.6 s), then v59 = 1 (uid 535).
const SIDED_ROUND_END: RoundEnd = RoundEnd {
    graph: SIDED_GRAPH,
    wait: 359,
    next_phase: 535,
    timeout_from: None,
};
/// 0EC2: wait 359 (10.9 s), then v59 = 2 (uid 535).
const CONTROL_ROUND_END: RoundEnd = RoundEnd {
    graph: CONTROL_GRAPH,
    wait: 359,
    next_phase: 535,
    timeout_from: None,
};
/// 0D0B: wait 454 for v8882, which uid 621 sets to 2.3 s when the graph
/// starts, then v59 = 1 (uid 508).
const ELIMINATION_ROUND_END: RoundEnd = RoundEnd {
    graph: ELIMINATION_GRAPH,
    wait: 454,
    next_phase: 508,
    timeout_from: Some((621, 8882)),
};

/// 0F4A's overtime fuse: the nodes that set its numbers (uid, variable).
const FUSE_FULL: (u32, u32) = (1093, 6976);
const FUSE_STEADY: (u32, u32) = (1090, 7712);
const FUSE_SHORTENING: (u32, u32) = (1091, 7714);
const FUSE_FLOOR: (u32, u32) = (1092, 7713);
/// The fuse's fall per second (v3393), set once the steady part is over.
const FUSE_FALL: (u32, u32) = (1115, 3393);
/// The state that moves the full fuse (v7717) down to its floor.
const FUSE_CHASE: (u32, u32) = (1117, 7717);

/// What a mode graph tells its capture points (graph 0E8E, whose uid 101
/// receives it): game message 0x79E with the rates by player count and
/// the decay's delay (refs `CaptureAreas.md` §4-5).
const CAPTURE_MESSAGE: u64 = 0x0240_0000_0000_079e;
/// The message's parameters (`0A4` indexes): 0x420 the capture rates (0E8E
/// v9868, by attackers), 0x421 the decay rates (v9870, by defenders), 0x422
/// the decay's delay in seconds (v9871).
const CAPTURE_RATES: u64 = 0x0420;
const DECAY_RATES: u64 = 0x0421;
const DECAY_DELAY: u64 = 0x0422;
/// A list of config vars.
const CONFIG_LIST: &str = "STU_06610EBC";
/// A native factor in 0F4A's capture speed (uid 856) that no data names.
/// Assumption: 1; with it the rates give the capture times of the rules
/// (game-rules.md 33, 49).
const CAPTURE_SPEED_NATIVE: &str = "STU_856F363B";

/// The node that sends a mode's capture rates, and the nodes it reads that
/// set its inputs first, in order: (uid, class, variable).
struct CaptureSender {
    graph: u32,
    uid: u32,
    /// The mode whose ruleset defaults the rates read.
    mode: u64,
    inputs: &'static [(u32, &'static str, u32)],
}

/// 0F4A uid 614 (Assault and Hybrid points): [1, 1.5, 2] times v12304, which
/// uid 856 sets to v10354 (uid 525: 1) times the ruleset's capture speed
/// (key 9518) and [`CAPTURE_SPEED_NATIVE`]; a 5 s delay.
const SIDED_CAPTURE: CaptureSender = CaptureSender {
    graph: SIDED_GRAPH,
    uid: 614,
    mode: ASSAULT,
    inputs: &[(525, SET_VAR, 10354), (856, STACK, 12304)],
};
/// 0EC2 uid 657 (Control): [0.125, 0.1875, 0.25] times v12304, which uid
/// 658 sets to v10354 (uid 525: 1) times the ruleset's capture speed; 5 s.
const CONTROL_CAPTURE: CaptureSender = CaptureSender {
    graph: CONTROL_GRAPH,
    uid: 657,
    mode: CONTROL,
    inputs: &[(525, SET_VAR, 10354), (658, STACK, 12304)],
};
/// 0D0B uid 839 (Elimination's tiebreaker point): one rate, 1 over the
/// ruleset's capture time (key 10595, 3 s); a 2 s delay.
const ELIMINATION_CAPTURE: CaptureSender = CaptureSender {
    graph: ELIMINATION_GRAPH,
    uid: 839,
    mode: ELIMINATION,
    inputs: &[],
};

/// The values of the mode graphs the match flow uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeGraphValues {
    /// Seconds from a round's end to the next phase in Assault, Escort and
    /// Hybrid (0F4A uid 359).
    pub sided_round_end: f64,
    /// The same in Control (0EC2 uid 359).
    pub control_round_end: f64,
    /// The same in Elimination (0D0B uid 454).
    pub elimination_round_end: f64,
    /// The overtime fuse of Assault, Escort and Hybrid (0F4A).
    pub fuse: Fuse,
    /// The capture points' rates in Assault and Hybrid (0F4A uid 614).
    pub sided_capture: CaptureRates,
    /// In Control (0EC2 uid 657).
    pub control_capture: CaptureRates,
    /// Elimination's tiebreaker point (0D0B uid 839).
    pub elimination_capture: CaptureRates,
}

/// The rates a mode graph gives its capture points (0E8E). A point's
/// progress (0 to 1) rises by `capture[attackers]` over the point's base
/// capture seconds (v12395) a second; once no attacker has been on it for
/// `decay_delay` seconds it falls by `decay[defenders]` over its base decay
/// seconds (v12396). One player or none uses the first rate, three or more
/// the last (0E8E uids 154 and 175 clamp the count).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureRates {
    pub capture: [f64; 3],
    pub decay: [f64; 3],
    pub decay_delay: f64,
}

impl CaptureRates {
    /// The capture rate with `attackers` on the point (at least one).
    pub fn capture(&self, attackers: usize) -> f64 {
        self.capture[attackers.clamp(1, 3) - 1]
    }

    /// The decay rate with `defenders` on the point.
    pub fn decay(&self, defenders: usize) -> f64 {
        self.decay[defenders.clamp(1, 3) - 1]
    }
}

impl ModeGraphValues {
    pub fn read(graphs: &GraphStore) -> io::Result<Self> {
        Ok(Self {
            sided_round_end: round_end(graphs, &SIDED_ROUND_END)?,
            control_round_end: round_end(graphs, &CONTROL_ROUND_END)?,
            elimination_round_end: round_end(graphs, &ELIMINATION_ROUND_END)?,
            fuse: Fuse::read(graphs)?,
            sided_capture: capture_rates(graphs, &SIDED_CAPTURE)?,
            control_capture: capture_rates(graphs, &CONTROL_CAPTURE)?,
            elimination_capture: capture_rates(graphs, &ELIMINATION_CAPTURE)?,
        })
    }

    /// The capture points' rates in `mode`: Assault's in the modes without
    /// points of their own.
    pub fn capture(&self, mode: Mode) -> CaptureRates {
        match mode {
            Mode::Control => self.control_capture,
            Mode::Elimination => self.elimination_capture,
            _ => self.sided_capture,
        }
    }

    /// Seconds from a round's end to the next phase in `mode`. Assumption:
    /// Deathmatch, Team Deathmatch and Skirmish wait as Assault does. Their
    /// graphs (0F73, 0F60, 0D76) play one round and have no wait after it,
    /// but the flow still shows a round's end before the match's end.
    pub fn round_end(&self, mode: Mode) -> f64 {
        match mode {
            Mode::Control => self.control_round_end,
            Mode::Elimination => self.elimination_round_end,
            Mode::Assault
            | Mode::Escort
            | Mode::Hybrid
            | Mode::Deathmatch
            | Mode::TeamDeathmatch
            | Mode::Skirmish => self.sided_round_end,
        }
    }
}

/// The overtime fuse (0F4A): full for the first `steady` seconds of
/// overtime, then it shortens at an even rate to `floor` times full over
/// `shortening` seconds, and stays there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fuse {
    /// v6976 (uid 1093), 5 s.
    pub full: f64,
    /// v7712 (uid 1090), 15 s.
    pub steady: f64,
    /// v7714 (uid 1091), 15 s.
    pub shortening: f64,
    /// v7713 (uid 1092), 0.3.
    pub floor: f64,
}

impl Fuse {
    /// The full fuse after `elapsed` seconds of overtime.
    pub fn seconds(&self, elapsed: f64) -> f64 {
        let fallen = ((elapsed - self.steady) / self.shortening).clamp(0.0, 1.0);
        self.full * (1.0 - (1.0 - self.floor) * fallen)
    }

    fn read(graphs: &GraphStore) -> io::Result<Self> {
        let graph = graph(graphs, SIDED_GRAPH)?;
        let mut authored = Authored::new(graphs, 0);
        let mut number = |(uid, variable)| -> io::Result<f64> {
            let value = authored.eval(graph, writer(graph, uid, SET_VAR, variable)?)?;
            authored.set(variable, value.clone());
            positive(graph, uid, to_float(&value))
        };
        let fuse = Self {
            full: number(FUSE_FULL)?,
            steady: number(FUSE_STEADY)?,
            shortening: number(FUSE_SHORTENING)?,
            floor: number(FUSE_FLOOR)?,
        };
        // The curve is the one these nodes draw: a fall of
        // full * (1 - floor) / shortening a second, down to full * floor.
        let fall = authored.eval(graph, writer(graph, FUSE_FALL.0, SET_VAR, FUSE_FALL.1)?)?;
        let chase = writer_node(graph, FUSE_CHASE.0, CHASE_VAR, FUSE_CHASE.1)?;
        let target = authored.eval(graph, field(graph, chase, "m_destination")?)?;
        let expected_fall = fuse.full * (1.0 - fuse.floor) / fuse.shortening;
        if fuse.floor >= 1.0
            || !close(to_float(&fall), expected_fall)
            || !close(to_float(&target), fuse.full * fuse.floor)
        {
            return Err(invalid(format!(
                "graph {SIDED_GRAPH:04X}: the overtime fuse no longer falls evenly from {} s to {} s",
                fuse.full,
                fuse.full * fuse.floor
            )));
        }
        Ok(fuse)
    }
}

/// The seconds of a round's wait, checked to be followed by the next phase.
fn round_end(graphs: &GraphStore, end: &RoundEnd) -> io::Result<f64> {
    let graph = graph(graphs, end.graph)?;
    let wait = node(graph, end.wait, WAIT)?;
    let mut authored = Authored::new(graphs, 0);
    if let Some((uid, variable)) = end.timeout_from {
        let value = authored.eval(graph, writer(graph, uid, SET_VAR, variable)?)?;
        authored.set(variable, value);
    }
    let seconds = to_float(&authored.eval(graph, field(graph, wait, "m_timeout")?)?);
    writer_node(graph, end.next_phase, SET_VAR, PHASE)?;
    if !leads_to(graph, wait, end.next_phase) {
        return Err(invalid(format!(
            "graph {:04X}: wait uid {} no longer leads to the phase set by uid {}",
            end.graph, end.wait, end.next_phase
        )));
    }
    positive(graph, end.wait, seconds)
}

/// Whether the wait's end runs, through output plugs, the node `target`.
fn leads_to(graph: &Graph, wait: &Node, target: u32) -> bool {
    let mut pending: Vec<(u16, usize)> = wait
        .links("m_onFinishedPlug")
        .filter_map(|(position, _)| Some((position?, 1)))
        .collect();
    let mut seen = Vec::new();
    while let Some((position, depth)) = pending.pop() {
        let Some(node) = graph.node(position) else {
            continue;
        };
        if node.uid == target {
            return true;
        }
        if depth >= PHASE_SEARCH || seen.contains(&position) {
            continue;
        }
        seen.push(position);
        for (plug, value) in node.plugs() {
            if value.get("$").and_then(Json::as_str) == Some(OUTPUT_PLUG) {
                pending.extend(
                    node.links(&plug)
                        .filter_map(|(position, _)| Some((position?, depth + 1))),
                );
            }
        }
    }
    false
}

/// The rates a mode graph sends its capture points.
fn capture_rates(graphs: &GraphStore, sender: &CaptureSender) -> io::Result<CaptureRates> {
    let graph = graph(graphs, sender.graph)?;
    let at = || format!("graph {:04X} uid {}", sender.graph, sender.uid);
    let message = node(graph, sender.uid, SEND_MESSAGE)?;
    if message
        .field("m_gameMessage.m_gameMessage")
        .and_then(asset_guid)
        != Some(CAPTURE_MESSAGE)
    {
        return Err(invalid(format!(
            "{} no longer sends the capture rates",
            at()
        )));
    }
    let mut authored = Authored::new(graphs, sender.mode);
    for &(uid, class, variable) in sender.inputs {
        let value = authored.eval(graph, writer(graph, uid, class, variable)?)?;
        authored.set(variable, value);
    }
    let mut table = |key: u64| -> io::Result<[f64; 3]> {
        let value = parameter(message, key)
            .ok_or_else(|| invalid(format!("{} has no parameter {key:#06X}", at())))?;
        let configs: Vec<&Json> = if value.get("$").and_then(Json::as_str) == Some(CONFIG_LIST) {
            value
                .get("m_configVars")
                .and_then(Json::as_array)
                .map(|list| list.iter().collect())
                .unwrap_or_default()
        } else {
            vec![value]
        };
        let mut rates = Vec::new();
        for config in configs {
            rates.push(positive(
                graph,
                sender.uid,
                to_float(&authored.eval(graph, config)?),
            )?);
        }
        // 0E8E indexes the list by the player count, clamped to its end.
        match rates[..] {
            [rate] => Ok([rate; 3]),
            [one, two, three] => Ok([one, two, three]),
            _ => Err(invalid(format!(
                "{}: {} rates in {key:#06X}, not 1 or 3",
                at(),
                rates.len()
            ))),
        }
    };
    let capture = table(CAPTURE_RATES)?;
    let decay = table(DECAY_RATES)?;
    let delay = parameter(message, DECAY_DELAY)
        .ok_or_else(|| invalid(format!("{} has no decay delay", at())))?;
    let decay_delay = positive(graph, sender.uid, to_float(&authored.eval(graph, delay)?))?;
    Ok(CaptureRates {
        capture,
        decay,
        decay_delay,
    })
}

/// The message parameter whose `0A4` index is `key`.
fn parameter(node: &Node, key: u64) -> Option<&Json> {
    node.field("m_params")?
        .as_array()?
        .iter()
        .find(|parameter| {
            parameter
                .get("m_B5051BCE")
                .and_then(asset_guid)
                .is_some_and(|guid| guid & 0xffff_ffff_ffff == key)
        })?
        .get("m_value")
}

fn graph(graphs: &GraphStore, index: u32) -> io::Result<&Graph> {
    graphs
        .graph(index)
        .ok_or_else(|| invalid(format!("the mode graph {index:04X} is missing")))
}

/// The one node with `uid`, of `class`.
fn node<'a>(graph: &'a Graph, uid: u32, class: &str) -> io::Result<&'a Node> {
    let mut found = graph.nodes.iter().filter(|node| node.uid == uid);
    match (found.next(), found.next()) {
        (Some(node), None) if node.class == class => Ok(node),
        (Some(node), None) => Err(invalid(format!(
            "graph {:04X} uid {uid} is a {}, not a {class}",
            graph.index, node.class
        ))),
        _ => Err(invalid(format!(
            "graph {:04X} has no single node uid {uid}",
            graph.index
        ))),
    }
}

/// The node `uid` of `class` that writes `variable`.
fn writer_node<'a>(graph: &'a Graph, uid: u32, class: &str, variable: u32) -> io::Result<&'a Node> {
    let node = node(graph, uid, class)?;
    let written = node
        .field("m_out_Var")
        .and_then(|config| dynamic(config).ok().flatten())
        .map(|variable| variable.id);
    if written != Some(variable) {
        return Err(invalid(format!(
            "graph {:04X} uid {uid} writes {written:?}, not v{variable}",
            graph.index
        )));
    }
    Ok(node)
}

/// The value the node `uid` of `class` gives `variable`.
fn writer<'a>(graph: &'a Graph, uid: u32, class: &str, variable: u32) -> io::Result<&'a Json> {
    let node = writer_node(graph, uid, class, variable)?;
    field(graph, node, "m_value")
}

fn field<'a>(graph: &Graph, node: &'a Node, path: &str) -> io::Result<&'a Json> {
    node.field(path).ok_or_else(|| {
        invalid(format!(
            "graph {:04X} uid {} has no {path}",
            graph.index, node.uid
        ))
    })
}

fn positive(graph: &Graph, uid: u32, value: f64) -> io::Result<f64> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(invalid(format!(
            "graph {:04X} uid {uid} gives {value}, not a positive number",
            graph.index
        )))
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-5 * b.abs().max(1.0)
}

/// The graph's values as authored: the variables read are the ones the
/// checked nodes set, and the ruleset's are the mode's defaults.
struct Authored<'a> {
    graphs: &'a GraphStore,
    mode: u64,
    variables: HashMap<u32, Value>,
}

impl<'a> Authored<'a> {
    fn new(graphs: &'a GraphStore, mode: u64) -> Self {
        Self {
            graphs,
            mode,
            variables: HashMap::new(),
        }
    }

    fn set(&mut self, variable: u32, value: Value) {
        self.variables.insert(variable, value);
    }

    fn eval(&mut self, graph: &Graph, config: &Json) -> io::Result<Value> {
        evaluate(config, self)
            .map_err(|error| invalid(format!("graph {:04X}: {error}", graph.index)))
    }
}

impl Context for Authored<'_> {
    fn variable(&mut self, variable: Variable) -> Value {
        self.variables
            .get(&variable.id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn ruleset(&mut self, id: u32) -> Value {
        self.graphs
            .rulesets()
            .default(id, 0, self.mode)
            .unwrap_or(Value::Null)
    }

    fn native(&mut self, config: &Json) -> Option<Value> {
        (config.get("$").and_then(Json::as_str) == Some(CAPTURE_SPEED_NATIVE))
            .then_some(Value::Float(1.0))
    }
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
