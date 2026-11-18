//! Project an initialized statescript body into the client baseline stream.
//!
//! The owner receives an H0 tree with all networked states off. An observer
//! receives the same tree and the initialized remote-synced states.
//!
//! A delta lists every state that changed since the acknowledged frame, and
//! an ended state as off: the owner's client ends a networked state only
//! when a frame lists it off, and begins one only when a frame lists it on
//! (owner-delta state reader 0x7FF78991C620: a listed plain class reads one
//! bit, 1 begins it unless it runs, 0 ends it if it runs; a listed StEC class
//! reads a present bit, then the active bit and its payload, and a present
//! bit of 0 changes nothing; a state the frame leaves out stays as it is;
//! `refs/reports/OwnerPrediction.md` §11, "Night (2026-11-17)").

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use crate::codecs::{Codec, CodecTable, PayloadError, PayloadMode, StatePayload};
use crate::expr::{Scope, equal};
use crate::graph::{Graph, GraphStore, Node, asset_guid};
use crate::stream::{BodyStream, ChangeKey, DeltaKind, ScriptChunk, StreamError};
use crate::vm::{Snapshot, StackBinding, State, StateKind, Unknowns};
use crate::wire::{
    self, Binding, Bindings, Instance, Layout, Value, ValueEncoding, Variables, WireError,
};
use orpheus_core::bits::{BitError, BitWriter};

const OWNER_HEALTH: u32 = 253;
/// How long a wheel action stays sendable after it ran (upstream
/// `WHEEL_CALLOUT_MS`, server delivery policy).
const ACTION_LIFETIME_MS: u64 = 2000;
/// The wheel's callout graph: its actions go out only while its root wait
/// (state 0) runs (upstream `callout_snapshot`).
const CALLOUT_GRAPH: u32 = 0x0686;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Owner,
    Observer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedSnapshot {
    entity: u32,
    view: View,
    entity_exclusions: BTreeSet<u32>,
    variables: ProjectedVariables,
    instances: BTreeMap<u16, ProjectedInstance>,
    /// The wheel actions still sendable, oldest first.
    actions: Vec<ProjectedAction>,
    /// The newest action this projection has passed: a full frame's
    /// baseline takes the newest one there is, a delta the one it carried.
    action_serial: u32,
}

impl ProjectedSnapshot {
    pub fn entity(&self) -> u32 {
        self.entity
    }

    pub fn view(&self) -> View {
        self.view
    }

    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// The activation counter and absolute start in milliseconds from this frame.
    pub fn volley_identity(&self, instance: u16, state: u16) -> Option<(u16, u64)> {
        self.instances
            .get(&instance)?
            .states
            .get(&state)?
            .volley
            .map(|volley| volley.identity)
    }

    /// An instance's graph and parent (instance and state), if this
    /// projection has it.
    pub fn instance(&self, id: u16) -> Option<(u32, Option<(u16, u16)>)> {
        self.instances
            .get(&id)
            .map(|instance| (instance.graph, instance.parent))
    }

    /// An instance's variable as this projection sends it.
    pub fn instance_variable(&self, instance: u16, id: u32) -> Option<&Value> {
        self.instances.get(&instance)?.variables.values.get(&id)
    }

    /// An entity variable as this projection sends it.
    pub fn entity_variable(&self, id: u32) -> Option<&Value> {
        self.variables.values.get(&id)
    }

    /// Send `value` for an entity variable instead of the script's (none:
    /// leave it out), without Stack links: what the frames carry for
    /// content the server doesn't run.
    pub fn set_entity_variable(&mut self, id: u32, value: Option<Value>) {
        self.variables.set(id, value);
    }

    /// As [`Self::set_entity_variable`], for an instance's variable;
    /// nothing without the instance.
    pub fn set_instance_variable(&mut self, instance: u16, id: u32, value: Option<Value>) {
        if let Some(instance) = self.instances.get_mut(&instance) {
            instance.variables.set(id, value);
        }
    }

    /// The instances, each id with its graph.
    pub fn instance_graphs(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.instances
            .iter()
            .map(|(&id, instance)| (id, instance.graph))
    }

    /// This projection with `other`'s instance `id` added, its variables,
    /// states and events with it (the practice nameplate on an acknowledged
    /// baseline: server.py `_flush_bot_nameplates`).
    pub fn with_instance_of(&self, other: &Self, id: u16) -> Option<Self> {
        let instance = other.instances.get(&id)?.clone();
        let mut merged = self.clone();
        merged.instances.insert(id, instance);
        Some(merged)
    }

    /// A state's projected payload in this frame, as `matching_volley` in
    /// `ow174/game/projectiles.py` reads `snapshot.states[key][2]`.
    pub fn state_payload(&self, instance: u16, state: u16) -> Option<&StatePayload> {
        Some(&self.instances.get(&instance)?.states.get(&state)?.payload)
    }

    /// A state of an instance in this frame (server.py `_flush_bot_frames`
    /// reads `snapshot.states[key]`).
    pub fn state(&self, instance: u16, state: u16) -> Option<&ProjectedState> {
        self.instances.get(&instance)?.states.get(&state)
    }

    /// Put `value` in for an instance's state, or remove the state when it
    /// is None (`states[key] = ...`, `states.pop(key)`); nothing without the
    /// instance.
    pub fn replace_state(&mut self, instance: u16, state: u16, value: Option<ProjectedState>) {
        let Some(instance) = self.instances.get_mut(&instance) else {
            return;
        };
        match value {
            Some(value) => {
                instance.states.insert(state, value);
            }
            None => {
                instance.states.remove(&state);
            }
        }
    }

    pub fn has_instance(&self, id: u16) -> bool {
        self.instances.contains_key(&id)
    }
}

impl ProjectedSnapshot {
    /// Gate a new kill feed pulse (`BodyStream.kill_feed_snapshot` in
    /// driver.py) of each instance whose graph has a kill feed pulser
    /// ([`Graph::kill_feed_pulser`]): until the world allows it the client
    /// keeps what it has; once allowed the pulse goes out as a fresh first
    /// one; and a pulse already sent keeps its wire value until it is
    /// acknowledged.
    pub fn gate_kill_feed(
        mut self,
        graphs: &GraphStore,
        stream: &BodyStream<ProjectedSnapshot>,
        allowed: bool,
    ) -> Self {
        let base = stream.base().map(|(_, base)| base);
        let pending = stream.latest_snapshot();
        let count = |state: Option<&ProjectedState>| {
            state
                .filter(|state| state.active)
                .and_then(|state| state.payload.get("count").cloned())
        };
        for (&owner, instance) in &mut self.instances {
            let Some(raw) = graphs
                .graph(instance.graph)
                .and_then(Graph::kill_feed_pulser)
            else {
                continue;
            };
            let state = |snapshot: Option<&ProjectedSnapshot>| {
                snapshot?.instances.get(&owner)?.states.get(&raw).cloned()
            };
            let before = state(base);
            let sent = state(pending);
            let current = instance.states.get(&raw);
            let acknowledged = count(before.as_ref());
            let current_count = count(current);
            let value = if let Some(sent) =
                sent.filter(|sent| sent.active && count(Some(sent)) != acknowledged)
            {
                Some(sent)
            } else if let Some(pulses) =
                current_count.filter(|pulses| Some(pulses) != acknowledged.as_ref())
            {
                if allowed {
                    Some(ProjectedState {
                        active: true,
                        activation: current.map_or(0, |state| state.activation),
                        payload: StatePayload::from([
                            ("count".into(), pulses),
                            ("fresh".into(), Value::Bool(true)),
                        ]),
                        volley: None,
                    })
                } else {
                    before
                }
            } else {
                continue;
            };
            if value.as_ref() == current {
                continue;
            }
            match value {
                Some(value) => instance.states.insert(raw, value),
                None => instance.states.remove(&raw),
            };
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ProjectedInstance {
    graph: u32,
    /// The entity it was created with when that isn't the snapshot's own
    /// (the descriptor's creator, `wire::write_descriptor`).
    creator: Option<u32>,
    parent: Option<(u16, u16)>,
    variables: ProjectedVariables,
    states: BTreeMap<u16, ProjectedState>,
    events: Vec<ProjectedEvent>,
}

/// One projected state: whether it runs, its payload and volley, and its
/// activation counter. The counter makes a state that ended and began again
/// between two frames a change, so a delta lists it: a client that ended its
/// copy (its timer ran out first) begins it again, one whose copy runs keeps
/// it (the plain reader begins only a state that is off).
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedState {
    active: bool,
    activation: u16,
    payload: StatePayload,
    volley: Option<VolleyActivation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VolleyActivation {
    identity: (u16, u64), // activation counter and start time
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ProjectedVariables {
    values: Variables,
    bindings: Bindings,
}

impl ProjectedVariables {
    fn write(&self, out: &mut BitWriter, owner: u16) -> Result<(), WireError> {
        wire::bound_variable_list(
            out,
            &self.values,
            ValueEncoding::Compact,
            owner,
            &self.bindings,
        )
    }

    fn set(&mut self, variable: u32, value: Option<Value>) {
        self.bindings.remove(&variable);
        match value {
            Some(value) => {
                self.values.insert(variable, value);
            }
            None => {
                self.values.shift_remove(&variable);
            }
        }
    }
}

/// A wheel action in the remote message list of a frame (104319 A0CCA0):
/// its instance and node, raw for the owner, remote for an observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectedAction {
    serial: u32,
    instance: u16,
    graph: u32,
    raw: u16,
    remote: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProjectedEvent {
    due_ms: u64,
    raw_state: u16,
    finish: bool,
    parameter: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateSite {
    pub instance: u16,
    pub graph: u32,
    pub state: u16,
}

#[derive(Debug)]
pub enum SnapshotError {
    Incomplete {
        entity: u32,
        diagnostics: usize,
        first: String,
    },
    MissingBoundValue {
        owner: u16,
        variable: u32,
    },
    InvalidBinding {
        owner: u16,
        variable: u32,
        binding: StackBinding,
    },
    MissingGraph {
        instance: u16,
        graph: u32,
    },
    MissingState(StateSite),
    MissingStateIndex {
        instance: u16,
        graph: u32,
        node: u16,
    },
    UnsupportedState {
        site: StateSite,
        class: String,
        kind: StateKind,
    },
    UnsupportedTimer {
        site: StateSite,
        class: String,
    },
    Payload {
        site: StateSite,
        source: PayloadError,
    },
    DifferentStream,
    ReboundInstance(u16),
    FrameTime,
    Wire(WireError),
    Stream(StreamError),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete {
                entity,
                diagnostics,
                first,
            } => {
                write!(
                    f,
                    "body {entity:08X} has {diagnostics} initialization diagnostics, first {first}"
                )
            }
            Self::MissingBoundValue { owner, variable } => {
                write!(
                    f,
                    "missing resolved value for bound variable {owner}:{variable}"
                )
            }
            Self::InvalidBinding {
                owner,
                variable,
                binding,
            } => {
                write!(
                    f,
                    "invalid stack binding {binding:?} for variable {owner}:{variable}"
                )
            }
            Self::MissingGraph { instance, graph } => {
                write!(f, "instance {instance} has no graph {graph:04X}")
            }
            Self::MissingState(site) => write!(f, "missing statescript state {site:?}"),
            Self::MissingStateIndex {
                instance,
                graph,
                node,
            } => {
                write!(
                    f,
                    "instance {instance} graph {graph:04X} remote node {node} has no state index"
                )
            }
            Self::UnsupportedState { site, class, kind } => {
                write!(f, "unsupported active state {class} ({kind:?}) at {site:?}")
            }
            Self::UnsupportedTimer { site, class } => {
                write!(f, "unsupported owner timer for {class} at {site:?}")
            }
            Self::Payload { site, source } => write!(f, "statescript state {site:?}: {source}"),
            Self::DifferentStream => f.write_str(
                "statescript baseline belongs to a different body, view or variable scope",
            ),
            Self::ReboundInstance(id) => {
                write!(f, "statescript instance {id} changed its definition")
            }
            Self::FrameTime => f.write_str("statescript frame time is outside the supported range"),
            Self::Wire(error) => error.fmt(f),
            Self::Stream(error) => error.fmt(f),
        }
    }
}

impl Error for SnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Payload { source, .. } => Some(source),
            Self::Wire(error) => Some(error),
            Self::Stream(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WireError> for SnapshotError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

impl From<BitError> for SnapshotError {
    fn from(error: BitError) -> Self {
        Self::Wire(error.into())
    }
}

impl From<StreamError> for SnapshotError {
    fn from(error: StreamError) -> Self {
        Self::Stream(error)
    }
}

pub struct SnapshotEncoder<'a> {
    graphs: &'a GraphStore,
    codecs: &'a CodecTable,
    unknowns: Unknowns,
    hidden: &'a [u16],
}

impl<'a> SnapshotEncoder<'a> {
    pub fn new(graphs: &'a GraphStore, codecs: &'a CodecTable) -> Self {
        Self {
            graphs,
            codecs,
            unknowns: Unknowns::for_build(),
            hidden: &[],
        }
    }

    /// The graphs it projects with.
    pub fn graphs(&self) -> &'a GraphStore {
        self.graphs
    }

    /// Whether a state or timer whose payload can't be written fails the
    /// frame or is left out of it, as the Python reference did.
    pub fn with_unknowns(self, unknowns: Unknowns) -> Self {
        Self { unknowns, ..self }
    }

    /// Leave these server-only instances out of projections, as a bot's
    /// behavior instance is (`BotScript.replicated_instances` in
    /// `ow174/game/script/bot.py`).
    pub fn hiding(self, hidden: &'a [u16]) -> Self {
        Self { hidden, ..self }
    }

    pub fn project(
        &self,
        snapshot: &Snapshot,
        view: View,
    ) -> Result<ProjectedSnapshot, SnapshotError> {
        self.project_with_exclusions(snapshot, view, None)
    }

    /// A later projection keeps the initial graph tree's entity exclusions.
    /// Per-instance exclusions still follow each instance's own definition.
    pub fn project_next(
        &self,
        snapshot: &Snapshot,
        baseline: &ProjectedSnapshot,
    ) -> Result<ProjectedSnapshot, SnapshotError> {
        if snapshot.entity != baseline.entity {
            return Err(SnapshotError::DifferentStream);
        }
        self.project_with_exclusions(snapshot, baseline.view, Some(&baseline.entity_exclusions))
    }

    fn project_with_exclusions(
        &self,
        snapshot: &Snapshot,
        view: View,
        frozen: Option<&BTreeSet<u32>>,
    ) -> Result<ProjectedSnapshot, SnapshotError> {
        if !snapshot.initialization_complete() {
            return Err(SnapshotError::Incomplete {
                entity: snapshot.entity,
                diagnostics: snapshot.diagnostics.len(),
                first: format!("{:?}", snapshot.diagnostics.first()),
            });
        }
        let mut entity_exclusions = frozen
            .cloned()
            .unwrap_or_else(|| BTreeSet::from([OWNER_HEALTH]));
        let mut instances = BTreeMap::new();
        for (&id, instance) in &snapshot.instances {
            if self.hidden.contains(&id) {
                continue;
            }
            let graph = self.graph(id, instance.graph)?;
            let exclusions = server_only_variables(graph);
            let variables =
                self.project_variables(snapshot, id, &instance.variables, &exclusions, view)?;
            if frozen.is_none() {
                entity_exclusions.extend(exclusions);
            }
            let mut states = BTreeMap::new();
            for (&raw, state) in &instance.states {
                let site = StateSite {
                    instance: id,
                    graph: graph.index,
                    state: raw,
                };
                let node = graph
                    .state(usize::from(raw))
                    .ok_or(SnapshotError::MissingState(site))?;
                if !node.is_networked()
                    || (view == View::Observer
                        && (presenter(node) || !graph.remote_positions().contains(&node.pos)))
                {
                    continue;
                }
                let codec = self.codecs.codec(&node.class);
                if !state.active && !(state.activation != 0 && codec.is_some_and(Codec::stec)) {
                    continue;
                }
                let Some(payload) = codec
                    .filter(|codec| codec.projected())
                    .and_then(|codec| state_payload(state, codec, snapshot.now_ms))
                else {
                    if self.unknowns == Unknowns::Tolerate {
                        continue;
                    }
                    return Err(SnapshotError::UnsupportedState {
                        site,
                        class: node.class.clone(),
                        kind: state.kind,
                    });
                };
                states.insert(
                    raw,
                    ProjectedState {
                        active: state.active,
                        activation: state.activation,
                        payload,
                        volley: matches!(state.kind, StateKind::WeaponVolley { .. }).then_some(
                            VolleyActivation {
                                identity: (state.activation, state.start_ms),
                            },
                        ),
                    },
                );
            }
            instances.insert(
                id,
                ProjectedInstance {
                    graph: instance.graph,
                    creator: (instance.source != snapshot.entity).then_some(instance.source),
                    parent: instance.parent,
                    variables,
                    states,
                    events: if view == View::Owner {
                        self.project_timers(snapshot, id, graph)?
                    } else {
                        Vec::new()
                    },
                },
            );
        }
        let variables =
            self.project_variables(snapshot, 0, &snapshot.variables, &entity_exclusions, view)?;
        let actions = self.project_actions(snapshot, &instances)?;
        Ok(ProjectedSnapshot {
            entity: snapshot.entity,
            view,
            variables,
            entity_exclusions,
            instances,
            actions,
            action_serial: snapshot
                .frame_actions
                .last()
                .map_or(0, |action| action.serial),
        })
    }

    /// The wheel actions the frames may still carry: of a live instance
    /// (the same one, not a reused id), at most [`ACTION_LIFETIME_MS`] old;
    /// `0686`'s only while its root wait runs (upstream `callout_snapshot`).
    fn project_actions(
        &self,
        snapshot: &Snapshot,
        instances: &BTreeMap<u16, ProjectedInstance>,
    ) -> Result<Vec<ProjectedAction>, SnapshotError> {
        let mut actions = Vec::new();
        for action in &snapshot.frame_actions {
            let Some(instance) = snapshot
                .instances
                .get(&action.instance)
                .filter(|instance| instance.start_ms == action.instance_start_ms)
                .filter(|_| instances.contains_key(&action.instance))
            else {
                continue;
            };
            if snapshot.now_ms.saturating_sub(action.time_ms) > ACTION_LIFETIME_MS
                || (instance.graph == CALLOUT_GRAPH
                    && !instance.states.get(&0).is_some_and(|state| state.active))
            {
                continue;
            }
            let graph = self.graph(action.instance, instance.graph)?;
            let Some(node) = graph.node(action.node) else {
                continue;
            };
            let Ok(remote) = u16::try_from(node.remote) else {
                continue;
            };
            actions.push(ProjectedAction {
                serial: action.serial,
                instance: action.instance,
                graph: instance.graph,
                raw: node.rid,
                remote,
            });
        }
        Ok(actions)
    }

    fn project_variables(
        &self,
        snapshot: &Snapshot,
        owner: u16,
        values: &Variables,
        excluded: &BTreeSet<u32>,
        view: View,
    ) -> Result<ProjectedVariables, SnapshotError> {
        let mut projected = ProjectedVariables::default();
        for (&variable, base) in values {
            if excluded.contains(&variable) {
                continue;
            }
            let key = (owner, variable);
            let bindings = snapshot.bindings.get(&key);
            let mut value = match snapshot.resolved_bindings.get(&key) {
                Some(resolved) => resolved,
                None if bindings.is_some() => {
                    return Err(SnapshotError::MissingBoundValue { owner, variable });
                }
                None => base,
            };
            if let Some(bindings) = bindings.filter(|_| view == View::Owner) {
                let links = self.project_bindings(snapshot, owner, variable, bindings)?;
                // The owner's client keeps a variable as its own value plus
                // its links and works the links out itself (a Stack's entry
                // reads the value below it: client 0x7FF78AAA5FF0,
                // `vm/links.rs`); the value it reads here is that own value
                // (0x7FF78991D2C0, leafall903/ORPHEUS (MIT) `decode.py`
                // `read_var`; inferred, not traced). So with every binding a
                // Stack it has, it gets the variable's own value: the stacked
                // one counts a stack that adds to its own variable twice.
                // Ana's `0B94` st77 `#23184e + 0.65` comes to 1.3, and `01C7`
                // st49 takes her speed scalar below 0: the suspected cause of
                // her scoped walk stopping once she had owner corrections.
                // Python sent the stacked value (driver.py `_add_var`:
                // `var.value()`); it still goes where a binding stays on the
                // server or is an Ability's stack (slot 1).
                if links.len() == bindings.len() && links.iter().all(|link| link.slot == 0) {
                    value = base;
                }
                if !links.is_empty() {
                    projected.bindings.insert(variable, links);
                }
            }
            if !matches!(value, Value::Null) || projected.bindings.contains_key(&variable) {
                projected.values.insert(variable, value.clone());
            }
        }
        Ok(projected)
    }

    fn project_bindings(
        &self,
        snapshot: &Snapshot,
        owner: u16,
        variable: u32,
        bindings: &[StackBinding],
    ) -> Result<Vec<Binding>, SnapshotError> {
        let mut links = Vec::with_capacity(bindings.len());
        for &binding in bindings {
            let invalid = || SnapshotError::InvalidBinding {
                owner,
                variable,
                binding,
            };
            let source = snapshot
                .instances
                .get(&binding.instance)
                .ok_or_else(invalid)?;
            if !source.states.get(&binding.raw_state).is_some_and(|state| {
                state.active
                    && matches!(
                        (state.kind, binding.slot),
                        (StateKind::Stack { .. } | StateKind::ChaseVar, 0)
                            | (StateKind::Ability { .. }, 1)
                    )
            }) {
                return Err(invalid());
            }
            let graph = self.graph(binding.instance, source.graph)?;
            let node = graph
                .state(usize::from(binding.raw_state))
                .ok_or_else(invalid)?;
            if node.is_networked() {
                links.push(Binding {
                    instance: binding.instance,
                    state: binding.raw_state,
                    state_bits: graph.header.states_bits,
                    slot: binding.slot,
                    priority: binding.priority.level,
                    above: binding.priority.above_equals,
                });
            }
        }
        Ok(links)
    }

    /// The owner's first tree and the observer's first frame share the H0
    /// descriptor, variable, and remote slot writer.
    pub fn full(&self, snapshot: &ProjectedSnapshot) -> Result<BitWriter, SnapshotError> {
        let mut instances = Vec::with_capacity(snapshot.instances.len());
        for (&id, instance) in &snapshot.instances {
            let graph = self.graph(id, instance.graph)?;
            let mut wire = Instance::new(id, Layout::try_from(graph)?, instance.parent);
            wire.creator = instance.creator;
            wire.variables = instance.variables.values.clone();
            for (slot, (_, var)) in graph.presence_vars().enumerate() {
                if let Some(value) = var.0.and_then(|var| wire.variables.swap_remove(&var)) {
                    wire.presence.insert(slot, value);
                }
            }
            if snapshot.view == View::Observer {
                self.remote_states(id, graph, instance, &mut wire)?;
            }
            instances.push(wire);
        }
        Ok(wire::remote_full(&instances, &snapshot.variables.values)?)
    }

    /// An owner's full frame of the projection (owner view) at command
    /// frame `command_frame`, laid out as `wire::owner_full` writes one: the
    /// tree, the entity's variables and, per instance, its presence values
    /// and variables (with their Stack links), each owner state slot (1 and
    /// the owner payload for a running state, 0 for one that isn't) and its
    /// pending timer events. Values are tagged as in [`Self::full`] and the
    /// owner deltas (compact): `wire::owner_full`'s explicit tags have no
    /// array, and the client reads a value by its tag (assumption: the same
    /// reader in an owner full frame). A StEC class's owner full form is not
    /// known: such a running state fails the frame (or is left out,
    /// tolerating).
    pub fn owner_full(
        &self,
        snapshot: &ProjectedSnapshot,
        command_frame: u32,
        frame_ms: u32,
    ) -> Result<BitWriter, SnapshotError> {
        if snapshot.view != View::Owner {
            return Err(SnapshotError::DifferentStream);
        }
        if command_frame == 0 {
            return Err(WireError::ZeroCommandFrame.into());
        }
        let frame_time = u64::from(command_frame)
            .checked_mul(u64::from(frame_ms))
            .ok_or(SnapshotError::FrameTime)?;
        let mode = PayloadMode::Owner {
            full: true,
            frame_time: i64::try_from(frame_time).map_err(|_| SnapshotError::FrameTime)?,
            frame_ms,
        };
        let mut out = BitWriter::new();
        out.bit(false);
        out.bit(true); // an owner frame
        out.bit(false); // not a correction
        out.w_var(command_frame);
        let mut last = 0;
        for (&id, instance) in &snapshot.instances {
            out.w_u16(id - last);
            last = id;
            let graph =
                u16::try_from(instance.graph).map_err(|_| WireError::GraphIndex(instance.graph))?;
            wire::write_descriptor(&mut out, graph, instance.creator, instance.parent)?;
        }
        out.w_u16(0);
        let variables = &snapshot.variables;
        wire::bound_variable_list(
            &mut out,
            &variables.values,
            ValueEncoding::Compact,
            0,
            &variables.bindings,
        )?;
        for (&id, instance) in &snapshot.instances {
            let graph = self.graph(id, instance.graph)?;
            let mut values = instance.variables.values.clone();
            for (_, var) in graph.presence_vars() {
                let value = var.0.and_then(|var| Some((var, values.swap_remove(&var)?)));
                out.bit(value.is_some());
                if let Some((var, value)) = value {
                    let links = instance.variables.bindings.get(&var);
                    wire::bound_variable(
                        &mut out,
                        &value,
                        ValueEncoding::Compact,
                        id,
                        links.map_or(&[][..], Vec::as_slice),
                    )?;
                }
            }
            wire::bound_variable_list(
                &mut out,
                &values,
                ValueEncoding::Compact,
                id,
                &instance.variables.bindings,
            )?;
            let mut slots = BTreeMap::new();
            for (&raw, state) in instance.states.iter().filter(|(_, state)| state.active) {
                let site = StateSite {
                    instance: id,
                    graph: graph.index,
                    state: raw,
                };
                let node = graph
                    .state(usize::from(raw))
                    .ok_or(SnapshotError::MissingState(site))?;
                let slot = graph
                    .owner_slot(usize::from(raw))
                    .ok_or(SnapshotError::MissingState(site))?;
                let codec = match self.codecs.codec(&node.class) {
                    Some(codec) if !codec.stec() => codec,
                    _ if self.unknowns == Unknowns::Tolerate => continue,
                    _ => {
                        return Err(SnapshotError::Payload {
                            site,
                            source: PayloadError::Unsupported(node.class.clone()),
                        });
                    }
                };
                let payload = codec
                    .payload(&state.payload, mode)
                    .map_err(|source| SnapshotError::Payload { site, source })?;
                slots.insert(slot, payload);
            }
            for slot in 0..graph.owner_positions().len() {
                out.bit(slots.contains_key(&slot));
                if let Some(payload) = slots.get(&slot) {
                    out.append(payload);
                }
            }
            self.write_events(&mut out, graph, &instance.events, frame_time)?;
        }
        out.bits(0, 2)?; // no frame events
        Ok(out)
    }

    /// Prepare the baseline for this body's delivery stream. The body create
    /// acknowledgement still controls when the resulting chunk may be sent.
    pub fn queue_full(
        &self,
        stream: &mut BodyStream<ProjectedSnapshot>,
        snapshot: &ProjectedSnapshot,
    ) -> Result<ScriptChunk, SnapshotError> {
        let payload = self.full(snapshot)?;
        let mut baseline = snapshot.clone();
        if baseline.view == View::Owner {
            baseline.variables.bindings.clear();
            for instance in baseline.instances.values_mut() {
                instance.variables.bindings.clear();
                instance.states.clear();
                instance.events.clear();
            }
        }
        Ok(stream.full(&payload, baseline)?)
    }

    /// Encode a cumulative projection from the acknowledged baseline. The
    /// caller supplies the owner phase selected by the body's stream.
    pub fn queue_delta(
        &self,
        stream: &mut BodyStream<ProjectedSnapshot>,
        snapshot: &ProjectedSnapshot,
        kind: DeltaKind,
        frame_ms: u32,
    ) -> Result<Option<ScriptChunk>, SnapshotError> {
        let (_, base) = stream.base().ok_or(StreamError::BaselinePending)?;
        let view = match kind {
            DeltaKind::Observer { .. } => View::Observer,
            DeltaKind::InitialOwner(_) | DeltaKind::Correction(_) => View::Owner,
        };
        if base.entity != snapshot.entity
            || base.view != view
            || snapshot.view != view
            || base.entity_exclusions != snapshot.entity_exclusions
        {
            return Err(SnapshotError::DifferentStream);
        }
        let touched = stream.touched();
        let known: BTreeSet<_> = if view == View::Observer {
            base.instances
                .keys()
                .copied()
                .chain(
                    stream
                        .pending()
                        .flat_map(|(_, pending, _)| pending.instances.keys().copied()),
                )
                .collect()
        } else {
            BTreeSet::new()
        };
        let mut delta = self.differences(base, snapshot, &touched, &known)?;
        // The oldest action the client hasn't acknowledged, under the stamp
        // of the chunk that first carried it (upstream `frame_callout`).
        let action = snapshot
            .actions
            .iter()
            .find(|action| action.serial > base.action_serial)
            .map(|action| {
                let stamp = touched
                    .iter()
                    .filter_map(|key| match *key {
                        ChangeKey::Action { serial, stamp } if serial == action.serial => {
                            Some(stamp)
                        }
                        _ => None,
                    })
                    .min()
                    .unwrap_or(stream.last().wrapping_add(1));
                (stamp, *action)
            });
        if let Some((stamp, action)) = action {
            delta.keys.insert(ChangeKey::Action {
                serial: action.serial,
                stamp,
            });
        }
        if view == View::Observer && delta.keys.is_empty() {
            return Ok(None);
        }
        let mut delivered = snapshot.clone();
        delivered.action_serial = action.map_or(base.action_serial, |(_, action)| action.serial);
        let frame = self.delta_frame(snapshot, &delta, kind, frame_ms, action)?;
        Ok(Some(stream.delta(&frame, delivered, delta.keys, kind)?))
    }

    /// The owner's chunks list each weapon volley's ammunition variable
    /// (`m_24E351A4`) at its own value, unbound, while the volley is off.
    /// The client binds the variable to the volley's output when it
    /// predicts a press (StartFiring, `0x7FF789C6E160`) and takes the next
    /// volley's ammunition from it as resolved (`0x7FF78AAB2E80`). A chunk
    /// from before the press rolls the volley back, and before the first
    /// volley since the spawn there is no record to restore it from, so the
    /// volley is cleared (`0x7FF789C71710`: `+0x284` = 1) while the variable
    /// stays bound to it: the replayed press then read 0 and drew 0
    /// (refs/reports/OwnerPrediction.md, 2026-11-18). Listed, the variable
    /// is unbound by the chunk before the replay, and the replay reads the
    /// count.
    fn idle_volley_ammo(
        &self,
        current: &ProjectedSnapshot,
    ) -> Result<BTreeSet<ChangeKey>, SnapshotError> {
        let mut keys = BTreeSet::new();
        if current.view != View::Owner {
            return Ok(keys);
        }
        for (&id, instance) in &current.instances {
            let graph = self.graph(id, instance.graph)?;
            for node in &graph.nodes {
                let Some(raw) = node
                    .state
                    .filter(|_| node.class == "STUStatescriptStateWeaponVolley")
                else {
                    continue;
                };
                if instance.states.get(&raw).is_some_and(|state| state.active) {
                    continue;
                }
                let Some(Ok(Some(variable))) = node.field("m_24E351A4").map(crate::expr::lvalue)
                else {
                    continue;
                };
                let (owner, values) = match variable.scope {
                    Scope::Entity => (0, &current.variables.values),
                    Scope::Instance => (id, &instance.variables.values),
                };
                if values.contains_key(&variable.id) {
                    keys.insert(ChangeKey::Variable(owner, variable.id));
                }
            }
        }
        Ok(keys)
    }

    fn differences<'s>(
        &self,
        base: &ProjectedSnapshot,
        current: &'s ProjectedSnapshot,
        touched: &BTreeSet<ChangeKey>,
        known: &BTreeSet<u16>,
    ) -> Result<SnapshotDelta<'s>, SnapshotError> {
        let mut delta = SnapshotDelta::default();
        let idle = self.idle_volley_ammo(current)?;
        let touched = &if idle.is_empty() {
            Cow::Borrowed(touched)
        } else {
            Cow::Owned(touched.union(&idle).copied().collect())
        };
        delta.variables = delta_variables(
            0,
            Some(&base.variables),
            &current.variables,
            touched,
            &mut delta.keys,
        );
        for (&id, next) in &current.instances {
            let before = base.instances.get(&id);
            if before.is_some_and(|before| {
                before.graph != next.graph
                    || before.creator != next.creator
                    || before.parent != next.parent
            }) {
                return Err(SnapshotError::ReboundInstance(id));
            }
            let descriptor = before.is_none();
            if descriptor {
                delta.keys.insert(ChangeKey::Instance(id));
            }
            let variables = delta_variables(
                id,
                before.map(|before| &before.variables),
                &next.variables,
                touched,
                &mut delta.keys,
            );
            let graph = self.graph(id, next.graph)?;
            let raw_slots: BTreeSet<_> = next
                .states
                .keys()
                .copied()
                .chain(
                    before
                        .into_iter()
                        .flat_map(|before| before.states.keys().copied()),
                )
                .chain(
                    touched
                        .range(ChangeKey::State(id, 0)..=ChangeKey::State(id, u16::MAX))
                        .filter_map(|key| match key {
                            ChangeKey::State(_, state) => Some(*state),
                            _ => None,
                        }),
                )
                .collect();
            let mut states = BTreeMap::new();
            for raw in raw_slots {
                let now = next.states.get(&raw);
                let was = before.and_then(|before| before.states.get(&raw));
                let key = ChangeKey::State(id, raw);
                if now == was && !touched.contains(&key) {
                    continue;
                }
                let site = StateSite {
                    instance: id,
                    graph: graph.index,
                    state: raw,
                };
                let node = graph
                    .state(usize::from(raw))
                    .ok_or(SnapshotError::MissingState(site))?;
                let entry = match now {
                    Some(state) => Some(Cow::Borrowed(state)),
                    // An StEC entry can't say "off" without a payload (its
                    // present bit of 0 changes nothing). One the client has
                    // running goes out off with the payload it had.
                    None if self.codecs.codec(&node.class).is_some_and(Codec::stec) => {
                        let Some(was) = was.filter(|was| was.active) else {
                            continue;
                        };
                        Some(Cow::Owned(ProjectedState {
                            active: false,
                            ..was.clone()
                        }))
                    }
                    None => None,
                };
                states.insert(raw, entry);
                delta.keys.insert(key);
                let Some(volley) = now
                    .filter(|_| current.view == View::Owner)
                    .and_then(|state| state.volley)
                else {
                    continue;
                };
                let previous = was.and_then(|state| state.volley);
                if previous.is_none_or(|was| was.identity != volley.identity) {
                    delta.keys.insert(ChangeKey::VolleyIntroduction(id, raw));
                }
            }
            if descriptor || !variables.values.is_empty() || !states.is_empty() {
                delta.instances.insert(
                    id,
                    InstanceDelta {
                        current: Some(next),
                        descriptor,
                        variables,
                        states,
                    },
                );
            }
        }
        let previous_ids: BTreeSet<_> = base
            .instances
            .keys()
            .copied()
            .chain(
                touched
                    .range(ChangeKey::Instance(0)..=ChangeKey::Instance(u16::MAX))
                    .filter_map(|key| match key {
                        ChangeKey::Instance(id) => Some(*id),
                        _ => None,
                    }),
            )
            .collect();
        for id in previous_ids {
            if !current.instances.contains_key(&id)
                && (current.view == View::Owner || known.contains(&id))
            {
                delta.instances.insert(id, InstanceDelta::default());
                delta.keys.insert(ChangeKey::Instance(id));
            }
        }
        Ok(delta)
    }

    fn delta_frame(
        &self,
        current: &ProjectedSnapshot,
        delta: &SnapshotDelta<'_>,
        kind: DeltaKind,
        frame_ms: u32,
        action: Option<(u32, ProjectedAction)>,
    ) -> Result<BitWriter, SnapshotError> {
        let (cmfd, correction, mode) = match kind {
            DeltaKind::Observer { .. } => (None, false, PayloadMode::Remote { full: false }),
            DeltaKind::InitialOwner(frame) | DeltaKind::Correction(frame) => {
                if frame == 0 {
                    return Err(WireError::ZeroCommandFrame.into());
                }
                let frame_time = i64::from(frame)
                    .checked_mul(i64::from(frame_ms))
                    .ok_or(SnapshotError::FrameTime)?;
                (
                    Some(frame),
                    matches!(kind, DeltaKind::Correction(_)),
                    PayloadMode::Owner {
                        full: false,
                        frame_time,
                        frame_ms,
                    },
                )
            }
        };
        let mut body = BitWriter::new();
        let mut last = 0;
        for (&id, instance) in &delta.instances {
            body.w_u16(id - last);
            last = id;
            body.bit(instance.current.is_none());
            if let Some(current) = instance.current {
                body.bit(instance.descriptor);
                if instance.descriptor {
                    let graph = u16::try_from(current.graph)
                        .map_err(|_| WireError::GraphIndex(current.graph))?;
                    wire::write_descriptor(&mut body, graph, current.creator, current.parent)?;
                }
            }
        }
        body.w_u16(0);
        delta.variables.write(&mut body, 0)?;
        for (&id, instance) in &delta.instances {
            let Some(current) = instance.current else {
                continue;
            };
            let graph = self.graph(id, current.graph)?;
            write_delta_variables(&mut body, id, graph, &instance.variables)?;
            self.delta_states(&mut body, id, graph, &instance.states, mode)?;
        }
        if let PayloadMode::Owner { frame_time, .. } = mode {
            let frame_time = u64::try_from(frame_time).map_err(|_| SnapshotError::FrameTime)?;
            self.owner_events(&mut body, current, frame_time)?;
        }
        let mut frame = BitWriter::new();
        frame.bit(false);
        frame.bit(cmfd.is_some());
        if let Some(cmfd) = cmfd {
            frame.bit(correction);
            frame.w_var(cmfd);
            frame.w_var(u32::try_from(body.bit_len()).map_err(|_| WireError::ChunkRange)?);
        }
        frame.append(&body);
        if cmfd.is_some() {
            frame.bit(false);
        }
        self.frame_action(&mut frame, action, cmfd.is_some())?;
        Ok(frame)
    }

    /// The frame's remote messages (104319 A0CCA0; upstream
    /// `_write_frame_action`): a group present bit, then for the action its
    /// stamp (RMFr), the group size (RMSz, without the group's terminator)
    /// and one short entry: target present, the instance (TrID) and the node,
    /// raw for the owner (H = 1), its remote index for an observer; then no
    /// more entries, no more groups and no ungrouped entries.
    fn frame_action(
        &self,
        frame: &mut BitWriter,
        action: Option<(u32, ProjectedAction)>,
        owner: bool,
    ) -> Result<(), SnapshotError> {
        frame.bit(action.is_some());
        if let Some((stamp, action)) = action {
            let graph = self.graph(action.instance, action.graph)?;
            let mut entry = BitWriter::new();
            entry.bit(true);
            entry.w_u16(action.instance);
            if owner {
                entry.bits(u64::from(action.raw), usize::from(graph.header.nodes_bits))?;
            } else {
                entry.bits(
                    u64::from(action.remote),
                    usize::from(graph.header.remote_bits),
                )?;
            }
            let length = 8 + entry.bit_len();
            frame.w_u32(stamp);
            frame.w_var(u32::try_from(2 + length).map_err(|_| WireError::ChunkRange)?);
            frame.bit(true); // an entry
            frame.bit(false); // short: an 8-bit length, no node payload
            frame.bits(length as u64, 8)?;
            frame.append(&entry);
            frame.bit(false); // no more entries
            frame.bit(false); // no more groups
        }
        frame.bit(false); // no ungrouped entries
        Ok(())
    }

    fn owner_events(
        &self,
        out: &mut BitWriter,
        current: &ProjectedSnapshot,
        frame_time: u64,
    ) -> Result<(), SnapshotError> {
        // Even an empty event list replaces that instance's former queue.
        out.bit(!current.instances.is_empty());
        if current.instances.is_empty() {
            return Ok(());
        }
        let mut last_id = 0;
        for (&id, instance) in &current.instances {
            out.bit(false);
            out.w_u16(id - last_id);
            last_id = id;
            let graph = self.graph(id, instance.graph)?;
            self.write_events(out, graph, &instance.events, frame_time)?;
        }
        out.bit(false);
        out.w_u16(0);
        Ok(())
    }

    /// One instance's timer events, by due time from `frame_time`, and the
    /// list's end (3).
    fn write_events(
        &self,
        out: &mut BitWriter,
        graph: &Graph,
        events: &[ProjectedEvent],
        frame_time: u64,
    ) -> Result<(), SnapshotError> {
        let mut events = events.iter().collect::<Vec<_>>();
        events.sort_by_key(|event| event.due_ms);
        let mut last_time = frame_time;
        for event in events {
            if event.finish {
                out.bits(2, 2)?;
            } else if event.parameter == 1 {
                out.bits(0, 2)?;
            } else {
                out.bits(1, 2)?;
                out.bit(event.parameter == 0);
                if event.parameter != 0 {
                    out.w_u16(event.parameter);
                }
            }
            out.bits(
                u64::from(event.raw_state),
                usize::from(graph.header.states_bits),
            )?;
            out.bit(event.due_ms == last_time);
            if event.due_ms != last_time {
                out.bit(event.due_ms > last_time);
                let elapsed = u32::try_from(event.due_ms.abs_diff(last_time))
                    .map_err(|_| SnapshotError::FrameTime)?;
                out.w_var(elapsed);
            }
            last_time = event.due_ms;
        }
        out.bits(3, 2)?;
        Ok(())
    }

    fn delta_states(
        &self,
        out: &mut BitWriter,
        id: u16,
        graph: &Graph,
        changed: &BTreeMap<u16, Option<Cow<'_, ProjectedState>>>,
        mode: PayloadMode,
    ) -> Result<(), SnapshotError> {
        let owner = matches!(mode, PayloadMode::Owner { .. });
        let mut slots = BTreeMap::new();
        if owner {
            for (&raw, state) in changed {
                let site = StateSite {
                    instance: id,
                    graph: graph.index,
                    state: raw,
                };
                let node = graph
                    .state(usize::from(raw))
                    .filter(|node| node.is_networked())
                    .ok_or(SnapshotError::MissingState(site))?;
                slots.insert(usize::from(raw), (site, node, state.as_deref()));
            }
        } else {
            for index in 0..graph.remote_nodes.len() {
                if graph.remote_slot(index).is_none() {
                    continue;
                }
                let node = graph
                    .remote_node(index)
                    .expect("compiled remote slot references node");
                let raw = node.state.ok_or(SnapshotError::MissingStateIndex {
                    instance: id,
                    graph: graph.index,
                    node: node.pos,
                })?;
                if let Some(state) = changed.get(&raw) {
                    let site = StateSite {
                        instance: id,
                        graph: graph.index,
                        state: raw,
                    };
                    slots.insert(index, (site, node, state.as_deref()));
                }
            }
        }
        let width = if owner {
            graph.states.len()
        } else {
            graph.remote_nodes.len()
        };
        write_mask(out, width, &slots.keys().copied().collect());
        for (site, node, state) in slots.into_values() {
            let Some(state) = state else {
                out.bit(false);
                continue;
            };
            let codec = self
                .codecs
                .codec(&node.class)
                .ok_or_else(|| SnapshotError::Payload {
                    site,
                    source: PayloadError::Unverified(node.class.clone()),
                })?;
            if codec.stec() {
                out.bit(true);
            }
            out.bit(state.active);
            if state.active || codec.stec() {
                let payload = codec
                    .payload(&state.payload, mode)
                    .map_err(|source| SnapshotError::Payload { site, source })?;
                out.append(&payload);
            }
        }
        Ok(())
    }

    fn graph(&self, instance: u16, graph: u32) -> Result<&Graph, SnapshotError> {
        self.graphs
            .graph(graph)
            .ok_or(SnapshotError::MissingGraph { instance, graph })
    }

    fn project_timers(
        &self,
        snapshot: &Snapshot,
        id: u16,
        graph: &Graph,
    ) -> Result<Vec<ProjectedEvent>, SnapshotError> {
        let Some(timers) = snapshot.pending_timers.get(&id) else {
            return Ok(Vec::new());
        };
        let mut events = Vec::with_capacity(timers.len());
        for timer in timers {
            let site = StateSite {
                instance: id,
                graph: graph.index,
                state: timer.raw_state,
            };
            let node = graph
                .state(usize::from(timer.raw_state))
                .ok_or(SnapshotError::MissingState(site))?;
            if !node.is_networked() {
                continue;
            }
            if !self.codecs.sendable(&node.class, true) {
                if self.unknowns == Unknowns::Tolerate {
                    continue;
                }
                return Err(SnapshotError::UnsupportedTimer {
                    site,
                    class: node.class.clone(),
                });
            }
            events.push(ProjectedEvent {
                due_ms: timer.due_ms,
                raw_state: timer.raw_state,
                finish: timer.finish,
                parameter: timer.parameter,
            });
        }
        Ok(events)
    }

    fn remote_states(
        &self,
        id: u16,
        graph: &Graph,
        instance: &ProjectedInstance,
        wire: &mut Instance,
    ) -> Result<(), SnapshotError> {
        for (slot, &position) in graph.remote_positions().iter().enumerate() {
            let node = graph
                .node(position)
                .expect("compiled remote positions reference nodes");
            let raw = node.state.ok_or(SnapshotError::MissingStateIndex {
                instance: id,
                graph: graph.index,
                node: position,
            })?;
            let Some(state) = instance.states.get(&raw) else {
                continue;
            };
            let site = StateSite {
                instance: id,
                graph: graph.index,
                state: raw,
            };
            let codec = self
                .codecs
                .codec(&node.class)
                .ok_or_else(|| SnapshotError::Payload {
                    site,
                    source: PayloadError::Unverified(node.class.clone()),
                })?;
            let mut bits = BitWriter::new();
            bits.bit(true); // X: present in this full frame
            if codec.stec() {
                bits.bit(true); // Y: this StEC entry has a payload
            }
            bits.bit(state.active);
            let payload = codec
                .payload(&state.payload, PayloadMode::Remote { full: true })
                .map_err(|source| SnapshotError::Payload { site, source })?;
            bits.append(&payload);
            wire.states.insert(slot, bits);
        }
        Ok(())
    }
}

#[derive(Default)]
struct SnapshotDelta<'a> {
    variables: ProjectedVariables,
    instances: BTreeMap<u16, InstanceDelta<'a>>,
    keys: BTreeSet<ChangeKey>,
}

#[derive(Default)]
struct InstanceDelta<'a> {
    current: Option<&'a ProjectedInstance>,
    descriptor: bool,
    variables: ProjectedVariables,
    states: BTreeMap<u16, Option<Cow<'a, ProjectedState>>>,
}

fn delta_variables(
    instance: u16,
    before: Option<&ProjectedVariables>,
    current: &ProjectedVariables,
    touched: &BTreeSet<ChangeKey>,
    changes: &mut BTreeSet<ChangeKey>,
) -> ProjectedVariables {
    let ids: BTreeSet<_> = current
        .values
        .keys()
        .copied()
        .chain(
            before
                .into_iter()
                .flat_map(|vars| vars.values.keys().copied()),
        )
        .chain(
            touched
                .range(ChangeKey::Variable(instance, 0)..=ChangeKey::Variable(instance, u32::MAX))
                .filter_map(|key| match key {
                    ChangeKey::Variable(_, id) => Some(*id),
                    _ => None,
                }),
        )
        .collect();
    let mut result = ProjectedVariables::default();
    for id in ids {
        let now = current.values.get(&id).unwrap_or(&Value::Null);
        let was = before
            .and_then(|vars| vars.values.get(&id))
            .unwrap_or(&Value::Null);
        let bindings = current.bindings.get(&id);
        let previous_bindings = before.and_then(|vars| vars.bindings.get(&id));
        let key = ChangeKey::Variable(instance, id);
        if touched.contains(&key) || !same_on_wire(now, was) || bindings != previous_bindings {
            result.values.insert(id, now.clone());
            if let Some(bindings) = bindings {
                result.bindings.insert(id, bindings.clone());
            }
            changes.insert(key);
        }
    }
    result
}

/// Whether a client that holds `was` holds `now`: the same value of the
/// same wire type. The scripts' equality ([`equal`]) takes an unset value
/// for false or 0, and a delta that compared by it left out a variable
/// that went from unset to false or 0, so the client kept it unset: a
/// Training Bot's `0C18` #1821 (the death's crit, `0095`) stayed unset
/// for a kill that wasn't critical, and its kill feed line sent `03EF`
/// with no value; the user saw the marker follow earlier lines ("sometimes
/// every elimination shows as a headshot, sometimes none", 2026-11-18;
/// inference: `0DEA` rid4 keeps its #1821 when the parameter has no
/// value, so the line took the last critical kill's marker).
fn same_on_wire(now: &Value, was: &Value) -> bool {
    match (now, was) {
        (Value::Array(now), Value::Array(was)) => {
            now.len() == was.len() && now.iter().zip(was).all(|(a, b)| same_on_wire(a, b))
        }
        (Value::Map(now), Value::Map(was)) | (Value::Pairs(now), Value::Pairs(was)) => {
            now.len() == was.len()
                && now
                    .iter()
                    .zip(was)
                    .all(|((now_key, now_value), (was_key, was_value))| {
                        same_on_wire(now_key, was_key) && same_on_wire(now_value, was_value)
                    })
        }
        _ => std::mem::discriminant(now) == std::mem::discriminant(was) && equal(now, was),
    }
}

fn write_mask(out: &mut BitWriter, width: usize, chosen: &BTreeSet<usize>) {
    if width <= 8 {
        for slot in 0..width {
            out.bit(chosen.contains(&slot));
        }
        return;
    }
    let groups: Vec<_> = (0..width)
        .step_by(8)
        .map(|start| {
            let range = start..(start + 8).min(width);
            let present = chosen.range(range.clone()).next().is_some();
            out.bit(present);
            (range, present)
        })
        .collect();
    for (slots, present) in groups {
        if present {
            for slot in slots {
                out.bit(chosen.contains(&slot));
            }
        }
    }
}

fn write_delta_variables(
    out: &mut BitWriter,
    owner: u16,
    graph: &Graph,
    vars: &ProjectedVariables,
) -> Result<(), WireError> {
    let mut sync = BTreeMap::new();
    let mut extra = vars.values.clone();
    for (slot, var) in graph.sync_vars.iter().enumerate() {
        let Some(id) = var.0.filter(|_| var.1 == 0) else {
            continue;
        };
        if let Some(value) = extra.swap_remove(&id) {
            sync.insert(slot, (id, value));
        }
    }
    write_mask(out, graph.sync_vars.len(), &sync.keys().copied().collect());
    for (id, value) in sync.values() {
        let links = vars.bindings.get(id).map_or(&[][..], Vec::as_slice);
        wire::bound_variable(out, value, ValueEncoding::Compact, owner, links)?;
    }
    wire::bound_variable_list(out, &extra, ValueEncoding::Compact, owner, &vars.bindings)
}

fn server_only_variables(graph: &Graph) -> BTreeSet<u32> {
    graph
        .fields
        .get("m_9CEC6985")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(asset_guid)
        .map(|guid| u32::from(guid as u16))
        .collect()
}

fn presenter(node: &Node) -> bool {
    node.class == "STUStatescriptStateUXPresenter" || node.fields.get("m_45216F79").is_some()
}

/// How long a pulse stays fresh for its first read
/// (`ClientOnlyPulser.FRESH_MS` in `ow174/game/script/nodes.py`).
const PULSE_FRESH_MS: u64 = 500;

fn state_payload(state: &State, codec: &Codec, now_ms: u64) -> Option<StatePayload> {
    Some(match (state.kind, codec.codec.as_deref()) {
        (
            StateKind::Plain
            | StateKind::HealthPool { .. }
            | StateKind::TargetTag { .. }
            | StateKind::Wait
            | StateKind::AiCandidate { .. }
            | StateKind::MovementMod
            | StateKind::CosmeticEntity
            | StateKind::Watch
            | StateKind::FilterArray
            | StateKind::Switch { .. }
            | StateKind::ActiveQuery
            | StateKind::GamePadVibration
            | StateKind::Defer
            | StateKind::AiFire { .. }
            | StateKind::ServerFrames
            | StateKind::MoveTo
            | StateKind::NavPath
            | StateKind::AiMove
            | StateKind::ModifyHealth { .. }
            | StateKind::Status
            | StateKind::CombatMod
            | StateKind::Assist
            | StateKind::HeroSelect
            // TrackStatTime: the client's reader is a nullsub
            // (`0x7FF78ABF5640`, `statescript_codecs.json`).
            | StateKind::StatTime,
            Some("none"),
        ) if codec.empty() => StatePayload::new(),
        // The Grappling Hook cast (`STU_4BBC5D09`, writer `0x7FF789BA5C40`):
        // the milliseconds since it began; the reader takes its start from it.
        (StateKind::GrappleCast, Some("start")) => StatePayload::from([(
            "t".into(),
            Value::Integer(i64::try_from(now_ms.saturating_sub(state.start_ms)).ok()?),
        )]),
        // The block's link field is the reference's absent link: Python
        // gave a plain state (the base `State.payload`) and Disable Button
        // no link (`ow174/game/script/driver.py` `_add_state`). The field of
        // view (`STU_75D1C308`) and the aim speed (`STU_3716E1D4`) are such
        // plain states: left out, the owner's client never runs them.
        (
            StateKind::Plain
            | StateKind::DisableButton { .. }
            | StateKind::Status
            | StateKind::CombatMod
            | StateKind::Assist,
            Some("link"),
        ) => StatePayload::new(),
        (StateKind::Pulser { pulses, pulsed_ms }, Some("pulser")) => StatePayload::from([
            (
                "fresh".into(),
                Value::Bool(
                    pulsed_ms.is_some_and(|pulsed| now_ms.saturating_sub(pulsed) <= PULSE_FRESH_MS),
                ),
            ),
            ("count".into(), Value::Integer(i64::from(pulses))),
        ]),
        (StateKind::BooleanSwitch(current), Some("switch")) => {
            StatePayload::from([("current".into(), Value::Bool(current))])
        }
        (StateKind::DelayedSwitch { current, undecided }, Some("two_bits")) => {
            StatePayload::from([
                ("current".into(), Value::Bool(current)),
                ("undecided".into(), Value::Bool(undecided)),
            ])
        }
        (StateKind::MovementSample(mask), Some("flags13")) => {
            StatePayload::from([("value".into(), Value::Integer(i64::from(mask)))])
        }
        (StateKind::SubScript { child }, Some("subscript")) => StatePayload::from([(
            "child".into(),
            Value::Integer(i64::from(child.unwrap_or_default())),
        )]),
        (StateKind::MessageIdle, Some("message")) => StatePayload::new(),
        (StateKind::WaitFrames { target }, Some("frames")) => {
            StatePayload::from([("target".into(), Value::Integer(i64::from(target)))])
        }
        // Recall's rewind: a 64-bit block field and a 16-bit reading
        // ow1-preservation calls duration-like (`HANDOFF.md:634-640`); their
        // meaning is unverified. Assumed: the field is zero and the reading
        // is the milliseconds since the state began.
        (StateKind::Rewind, Some("time16+s64")) => StatePayload::from([
            ("m_0007BF47".into(), Value::Integer(0)),
            (
                "time".into(),
                Value::Integer(i64::from(
                    u16::try_from(now_ms.saturating_sub(state.start_ms)).unwrap_or(u16::MAX),
                )),
            ),
        ]),
        (StateKind::LogicalButton { .. }, Some("button")) => StatePayload::from([(
            "counter".into(),
            Value::Integer(i64::from(state.activation & 15)),
        )]),
        (StateKind::Stack { top, under }, Some("stack")) => StatePayload::from([
            ("top".into(), Value::Bool(top)),
            ("under".into(), Value::Bool(under)),
        ]),
        (StateKind::Ability { flags }, Some("ability")) => {
            let mut payload =
                StatePayload::from([("flags".into(), Value::Integer(i64::from(flags)))]);
            if let Some(cooldown) = &state.ability_cooldown {
                payload.extend([
                    ("cur".into(), Value::Float(cooldown.current)),
                    ("rate".into(), Value::Float(cooldown.rate)),
                    (
                        "last".into(),
                        Value::Integer(i64::try_from(cooldown.last_ms).ok()?),
                    ),
                ]);
            }
            payload
        }
        (StateKind::HitList, Some("hits")) => {
            StatePayload::from([("flag".into(), Value::Bool(false))])
        }
        // The frame lists the dropped targets a search still holds for its
        // `m_93AB8C13` seconds (writer `0x7FF789BA5F80`), not the found
        // ones (those go in the output variables). The server runs no
        // search with that field (`vm/targets.rs`), so the list is empty.
        (StateKind::TrackTargets, Some("targets")) => {
            StatePayload::from([("targets".into(), Value::Array(Vec::new()))])
        }
        (StateKind::Anim, Some("anim")) => StatePayload::from([(
            "counter".into(),
            Value::Integer(i64::from(state.activation & 7)),
        )]),
        (StateKind::Effect, Some("counter2")) => StatePayload::from([(
            "counter".into(),
            Value::Integer(i64::from(state.activation & 3)),
        )]),
        (StateKind::SendMessage, Some("send")) => StatePayload::new(),
        (StateKind::ChaseVar, Some("chase")) => {
            let chase = state.chase.as_ref()?;
            if !matches!(chase.current, Value::Float(_) | Value::Vector(_)) {
                return None;
            }
            let mut payload = StatePayload::from([
                ("cur".into(), chase.current.clone()),
                ("reached".into(), Value::Bool(chase.reached)),
                (
                    "last".into(),
                    Value::Integer(i64::try_from(chase.last_ms).ok()?),
                ),
            ]);
            if let Some(remaining) = chase.remaining_ms {
                payload.insert("remaining".into(), Value::Integer(remaining));
            }
            payload
        }
        (
            StateKind::WeaponVolley {
                volleys, offset_ms, ..
            },
            Some("volley"),
        ) => StatePayload::from([
            (
                "start".into(),
                Value::Integer(i64::try_from(state.start_ms).ok()?),
            ),
            (
                "offset".into(),
                Value::Integer(i64::try_from(offset_ms).ok()?),
            ),
            ("volleys".into(), Value::Integer(i64::from(volleys))),
            (
                "counter".into(),
                Value::Integer(i64::from(state.activation)),
            ),
        ]),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
