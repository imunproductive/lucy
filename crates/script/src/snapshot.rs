//! Project an initialized statescript body into the client baseline stream.
//!
//! The owner receives an H0 tree with all networked states off. An observer
//! receives the same tree and the initialized remote-synced states.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use crate::codecs::{Codec, CodecTable, PayloadError, PayloadMode, StatePayload};
use crate::expr::{Scope, Variable, equal};
use crate::graph::{Graph, GraphStore, Node, asset_guid};
use crate::stream::{BodyStream, ChangeKey, DeltaKind, ScriptChunk, StreamError};
use crate::vm::{Snapshot, StackBinding, State, StateKind, Unknowns};
use crate::wire::{
    self, Binding, Bindings, Instance, Layout, Value, ValueEncoding, Variables, WireError,
};
use orpheus_core::bits::{BitError, BitWriter};

const OWNER_HEALTH: u32 = 253;

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
    parent: Option<(u16, u16)>,
    variables: ProjectedVariables,
    states: BTreeMap<u16, ProjectedState>,
    events: Vec<ProjectedEvent>,
}

/// One projected state: whether it runs, its payload and volley.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedState {
    active: bool,
    payload: StatePayload,
    volley: Option<VolleyActivation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VolleyActivation {
    identity: (u16, u64), // activation counter and start time
    ammo: Option<(Variable, i64)>,
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

    fn seed_ammo(&mut self, variable: u32, ammo: i64) {
        self.values.insert(variable, Value::Integer(ammo));
        self.bindings.remove(&variable);
    }
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
                        payload,
                        volley: match state.kind {
                            StateKind::WeaponVolley {
                                ammo_var, ammo0, ..
                            } => Some(VolleyActivation {
                                identity: (state.activation, state.start_ms),
                                ammo: ammo_var.map(|variable| (variable, ammo0)),
                            }),
                            _ => None,
                        },
                    },
                );
            }
            instances.insert(
                id,
                ProjectedInstance {
                    graph: instance.graph,
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
        Ok(ProjectedSnapshot {
            entity: snapshot.entity,
            view,
            variables,
            entity_exclusions,
            instances,
        })
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
            let value = match snapshot.resolved_bindings.get(&key) {
                Some(resolved) => resolved,
                None if bindings.is_some() => {
                    return Err(SnapshotError::MissingBoundValue { owner, variable });
                }
                None => base,
            };
            if let Some(bindings) = bindings.filter(|_| view == View::Owner) {
                let links = self.project_bindings(snapshot, owner, variable, bindings)?;
                if !links.is_empty() {
                    projected.bindings.insert(variable, links);
                }
            }
            // A map has no known wire form: the client's tagged values have a
            // keyed array that nobody has decoded. The client runs the same
            // graphs and keeps its own map; the Python reference never sent one
            // (it didn't evaluate map literals).
            if matches!(value, Value::Map(_)) && !projected.bindings.contains_key(&variable) {
                continue;
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
                        (StateKind::Stack { .. }, 0) | (StateKind::Ability { .. }, 1)
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
        if view == View::Observer && delta.keys.is_empty() {
            return Ok(None);
        }
        let mut delivered = snapshot.clone();
        delta.seed_owner_ammo(&mut delivered);
        let frame = self.delta_frame(snapshot, &delta, kind, frame_ms)?;
        Ok(Some(stream.delta(&frame, delivered, delta.keys, kind)?))
    }

    fn differences<'s>(
        &self,
        base: &ProjectedSnapshot,
        current: &'s ProjectedSnapshot,
        touched: &BTreeSet<ChangeKey>,
        known: &BTreeSet<u16>,
    ) -> Result<SnapshotDelta<'s>, SnapshotError> {
        let mut delta = SnapshotDelta::default();
        delta.variables = delta_variables(
            0,
            Some(&base.variables),
            &current.variables,
            touched,
            &mut delta.keys,
        );
        for (&id, next) in &current.instances {
            let before = base.instances.get(&id);
            if before
                .is_some_and(|before| before.graph != next.graph || before.parent != next.parent)
            {
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
                if now.is_none() && self.codecs.codec(&node.class).is_some_and(Codec::stec) {
                    continue;
                }
                states.insert(raw, now);
                delta.keys.insert(key);
                let Some((now, volley)) = now
                    .filter(|_| current.view == View::Owner)
                    .and_then(|state| state.volley.map(|volley| (state, volley)))
                else {
                    continue;
                };
                let previous = was.and_then(|state| state.volley);
                let introduced = previous.is_none_or(|was| was.identity != volley.identity);
                if introduced {
                    delta.keys.insert(ChangeKey::VolleyIntroduction(id, raw));
                }
                if let Some((variable, ammo)) = volley.ammo.filter(|_| {
                    now.active && (introduced || !was.is_some_and(|state| state.active))
                }) {
                    let owner = match variable.scope {
                        Scope::Entity => 0,
                        Scope::Instance => id,
                    };
                    delta.ammo_seeds.insert((owner, variable.id), ammo);
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
                    wire::write_descriptor(&mut body, graph, current.parent)?;
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
        frame.bits(0, 2)?;
        Ok(frame)
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
            let mut events = instance.events.iter().collect::<Vec<_>>();
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
        }
        out.bit(false);
        out.w_u16(0);
        Ok(())
    }

    fn delta_states(
        &self,
        out: &mut BitWriter,
        id: u16,
        graph: &Graph,
        changed: &BTreeMap<u16, Option<&ProjectedState>>,
        mode: PayloadMode,
    ) -> Result<(), SnapshotError> {
        let owner = matches!(mode, PayloadMode::Owner { .. });
        let mut slots = BTreeMap::new();
        if owner {
            for (&raw, &state) in changed {
                let site = StateSite {
                    instance: id,
                    graph: graph.index,
                    state: raw,
                };
                let node = graph
                    .state(usize::from(raw))
                    .filter(|node| node.is_networked())
                    .ok_or(SnapshotError::MissingState(site))?;
                slots.insert(usize::from(raw), (site, node, state));
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
                if let Some(&state) = changed.get(&raw) {
                    let site = StateSite {
                        instance: id,
                        graph: graph.index,
                        state: raw,
                    };
                    slots.insert(index, (site, node, state));
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
    ammo_seeds: BTreeMap<(u16, u32), i64>,
}

impl SnapshotDelta<'_> {
    fn seed_owner_ammo(&mut self, delivered: &mut ProjectedSnapshot) {
        // StartFiring reads the unbound, pre-volley count once after applying
        // the frame. The acknowledged snapshot must match that wire value.
        for (&(owner, variable), &ammo) in &self.ammo_seeds {
            let (wire, baseline) = if owner == 0 {
                (&mut self.variables, &mut delivered.variables)
            } else {
                (
                    &mut self
                        .instances
                        .get_mut(&owner)
                        .expect("introduced volley")
                        .variables,
                    &mut delivered
                        .instances
                        .get_mut(&owner)
                        .expect("introduced volley")
                        .variables,
                )
            };
            wire.seed_ammo(variable, ammo);
            baseline.seed_ammo(variable, ammo);
            self.keys.insert(ChangeKey::Variable(owner, variable));
        }
    }
}

#[derive(Default)]
struct InstanceDelta<'a> {
    current: Option<&'a ProjectedInstance>,
    descriptor: bool,
    variables: ProjectedVariables,
    states: BTreeMap<u16, Option<&'a ProjectedState>>,
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
        if touched.contains(&key) || !equal(now, was) || bindings != previous_bindings {
            result.values.insert(id, now.clone());
            if let Some(bindings) = bindings {
                result.bindings.insert(id, bindings.clone());
            }
            changes.insert(key);
        }
    }
    result
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
            | StateKind::Switch { .. }
            | StateKind::ActiveQuery
            | StateKind::GamePadVibration
            | StateKind::Defer
            | StateKind::AiFire { .. }
            | StateKind::ServerFrames
            | StateKind::MoveTo,
            Some("none"),
        ) if codec.empty() => StatePayload::new(),
        // The block's link field is the reference's absent link.
        (StateKind::DisableButton { .. }, Some("link")) => StatePayload::new(),
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
        // Each target's age is the milliseconds since it came in (assumed,
        // `vm/targets.rs`).
        (StateKind::TrackTargets, Some("targets")) => StatePayload::from([(
            "targets".into(),
            Value::Array(
                state
                    .tracked
                    .iter()
                    .map(|target| {
                        let age = now_ms
                            .saturating_sub(target.since_ms)
                            .min(u64::from(u32::MAX));
                        Value::Array(vec![
                            Value::Entity(target.entity),
                            Value::Integer(age as i64),
                        ])
                    })
                    .collect(),
            ),
        )]),
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
