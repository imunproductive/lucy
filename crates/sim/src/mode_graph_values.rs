//! Match values read from the mode graphs of the install: the wait after a
//! round, the overtime fuse and Control's capture rates.
//!
//! The VM can't run the mode graphs whole yet (`mode_script.rs`), so the
//! match flow (`match_flow.rs`) plays their phases itself. These values are
//! read from the graphs the server loads (`game_graphs_174.json.gz`). Each
//! one is found by its graph and node uid, and the node's class and the
//! variable it writes are checked. Data that no longer matches is an error
//! when the scene assets load, not a silent default.

use std::{collections::HashMap, io};

use orpheus_script::{
    expr::{Context, Value, Variable, dynamic, evaluate, to_float},
    graph::{Graph, GraphStore, Node, asset_guid},
};
use serde_json::Value as Json;

use crate::{
    match_flow::{CONTROL, Mode},
    objectives::CaptureTimes,
};

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

/// 0EC2 uid 657 sends the capture rates to the Control point.
const CAPTURE_SENDER: u32 = 657;
/// The message (`025`) and its parameters (`0A4` indexes) that carry the
/// rates by player count. Both parameters hold the same rates in 104319;
/// which one is which is not known.
const CAPTURE_MESSAGE: u64 = 0x0240_0000_0000_079e;
const CAPTURE_RATES: [u64; 2] = [0x0420, 0x0421];
/// The rates are multiplied by v12304 (uid 658), which is v10354 (set to
/// 1 by uid 525) times the ruleset's capture speed (key 9518).
const CAPTURE_SPEED: (u32, u32) = (658, 12304);
const CAPTURE_SPEED_BASE: (u32, u32) = (525, 10354);

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
    /// Control's capture seconds with 1, 2 and 3 or more players on the
    /// point (0EC2 uid 657).
    pub control_capture: CaptureTimes,
}

impl ModeGraphValues {
    pub fn read(graphs: &GraphStore) -> io::Result<Self> {
        Ok(Self {
            sided_round_end: round_end(graphs, &SIDED_ROUND_END)?,
            control_round_end: round_end(graphs, &CONTROL_ROUND_END)?,
            elimination_round_end: round_end(graphs, &ELIMINATION_ROUND_END)?,
            fuse: Fuse::read(graphs)?,
            control_capture: control_capture(graphs)?,
        })
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

/// Control's capture seconds by player count: one over the rates 0EC2
/// sends to the point.
fn control_capture(graphs: &GraphStore) -> io::Result<CaptureTimes> {
    let graph = graph(graphs, CONTROL_GRAPH)?;
    let sender = node(graph, CAPTURE_SENDER, SEND_MESSAGE)?;
    if sender
        .field("m_gameMessage.m_gameMessage")
        .and_then(asset_guid)
        != Some(CAPTURE_MESSAGE)
    {
        return Err(invalid(format!(
            "graph {CONTROL_GRAPH:04X} uid {CAPTURE_SENDER} no longer sends the capture rates"
        )));
    }
    let mut authored = Authored::new(graphs, CONTROL);
    let (uid, variable) = CAPTURE_SPEED_BASE;
    let base = authored.eval(graph, writer(graph, uid, SET_VAR, variable)?)?;
    authored.set(variable, base);
    let (uid, variable) = CAPTURE_SPEED;
    let speed = authored.eval(graph, writer(graph, uid, STACK, variable)?)?;
    authored.set(variable, speed);
    let mut tables = Vec::new();
    for key in CAPTURE_RATES {
        let rates = parameter(sender, key)
            .and_then(|rates| rates.get("m_configVars")?.as_array())
            .ok_or_else(|| {
                invalid(format!(
                    "graph {CONTROL_GRAPH:04X} uid {CAPTURE_SENDER} has no capture rates {key:#06X}"
                ))
            })?;
        let mut times: CaptureTimes = [0.0; 3];
        if rates.len() != times.len() {
            return Err(invalid(format!(
                "graph {CONTROL_GRAPH:04X} uid {CAPTURE_SENDER}: {} capture rates, not {}",
                rates.len(),
                times.len()
            )));
        }
        for (time, rate) in times.iter_mut().zip(rates) {
            let rate = positive(
                graph,
                CAPTURE_SENDER,
                to_float(&authored.eval(graph, rate)?),
            )?;
            *time = 1.0 / rate;
        }
        tables.push(times);
    }
    if tables.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(invalid(format!(
            "graph {CONTROL_GRAPH:04X} uid {CAPTURE_SENDER} sends two different capture rates"
        )));
    }
    Ok(tables[0])
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
}

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests;
