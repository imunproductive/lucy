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
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    sync::{Arc, Mutex, OnceLock, PoisonError, Weak},
};

use crate::codecs::{Codec, CodecTable, PayloadError, PayloadMode, StatePayload};
use crate::expr::{Scope, equal};
use crate::graph::{Graph, GraphStore};
use crate::stream::{BodyStream, ChangeKey, DeltaKind, ScriptChunk, StreamError};
use crate::vm::{Revision, Snapshot, StackBinding, State, StateKind, Unknowns};
use crate::wire::{
    self, Binding, Bindings, Instance, Layout, Value, ValueEncoding, Variables, WireError,
};
use lucy_core::bits::{BitError, BitWriter};

const OWNER_HEALTH: u32 = 253;
/// How long a wheel action stays sendable after it ran (upstream
/// `WHEEL_CALLOUT_MS`, server delivery policy).
pub const ACTION_LIFETIME_MS: u64 = 2000;
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
    /// Shared with its copies (the chunks queued from it, a client's base).
    variables: Arc<EntityVariables>,
    /// Each shared with the projections it didn't change in
    /// ([`SnapshotEncoder::project_cached`]).
    instances: BTreeMap<u16, Arc<ProjectedInstance>>,
    /// The wheel actions still sendable, oldest first.
    actions: Vec<ProjectedAction>,
    /// The newest action this projection has passed: a full frame's
    /// baseline takes the newest one there is, a delta the one it carried.
    action_serial: u32,
    /// The observer deltas made to this projection ([`SharedDeltas`]).
    shared: SharedDeltas,
}

/// A projection and what it was made from
/// ([`SnapshotEncoder::project_cached`]): the snapshot's revision and the
/// clock readings it holds for, from the snapshot's clock then to the
/// first time a pulse goes stale or a wheel action runs out; and the same
/// for each instance.
#[derive(Debug)]
pub struct ProjectionCache {
    revision: Revision,
    made_ms: u64,
    fresh_until_ms: u64,
    projected: ProjectedSnapshot,
    instances: BTreeMap<u16, InstanceStamp>,
}

/// What one instance's projection was made from: the instance's revision
/// ([`crate::vm::Instance::revision`]), the revisions of the instances
/// whose states its links name, and the clock readings it holds for.
#[derive(Debug)]
struct InstanceStamp {
    revision: u64,
    sources: Vec<(u16, u64)>,
    made_ms: u64,
    fresh_until_ms: u64,
}

impl ProjectionCache {
    /// Instance `id`'s kept projection and stamp, if they still hold for
    /// the instance at `revision` in `snapshot`.
    fn instance(
        &self,
        id: u16,
        revision: u64,
        snapshot: &Snapshot,
    ) -> Option<(&Arc<ProjectedInstance>, &InstanceStamp)> {
        let stamp = self.instances.get(&id)?;
        let holds = stamp.revision == revision
            && stamp.made_ms <= snapshot.now_ms
            && snapshot.now_ms <= stamp.fresh_until_ms
            && stamp.sources.iter().all(|&(source, revision)| {
                snapshot
                    .instances
                    .get(&source)
                    .map(|source| source.revision())
                    == Some(revision)
            });
        holds.then_some((self.projected.instances.get(&id)?, stamp))
    }
}

/// The instances whose states an instance's projected links name
/// ([`SnapshotEncoder::project_bindings`]), with their revisions.
fn binding_sources(
    snapshot: &Snapshot,
    id: u16,
    instance: &crate::vm::Instance,
    excluded: &BTreeSet<u32>,
    view: View,
) -> Vec<(u16, u64)> {
    if view != View::Owner {
        return Vec::new();
    }
    instance
        .variables
        .keys()
        .filter(|variable| !excluded.contains(variable))
        .filter_map(|&variable| snapshot.bindings.get(&(id, variable)))
        .flatten()
        .filter_map(|binding| {
            let source = snapshot.instances.get(&binding.instance)?;
            Some((binding.instance, source.revision()))
        })
        .collect()
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
        self.variables.variables.values.get(&id)
    }

    /// Send `value` for an entity variable instead of the script's (none:
    /// leave it out), without Stack links: what the frames carry for
    /// content the server doesn't run.
    pub fn set_entity_variable(&mut self, id: u32, value: Option<Value>) {
        self.set_entity_variables([(id, value)]);
    }

    /// [`Self::set_entity_variable`] for each of `values`: one new
    /// allocation for them all, none when the projection holds each
    /// already, and the same one as last time for the same edit of the
    /// same variables.
    pub fn set_entity_variables(&mut self, values: impl IntoIterator<Item = (u32, Option<Value>)>) {
        let edits: Vec<_> = values.into_iter().collect();
        if let Some(made) = self.variables.edited.made_by(&edits) {
            self.variables = made;
            return;
        }
        let mut edited: Option<ProjectedVariables> = None;
        for (id, value) in &edits {
            let current = edited.as_ref().unwrap_or(&self.variables.variables);
            if current.holds(*id, value.as_ref()) {
                continue;
            }
            edited
                .get_or_insert_with(|| self.variables.variables.clone())
                .set(*id, value.clone());
        }
        // A new allocation: comparisons kept for the old one stay its own.
        if let Some(variables) = edited {
            let made = EntityVariables::new(variables);
            self.variables.edited.keep(edits, Arc::clone(&made));
            self.variables = made;
        }
    }

    /// Tests: the entity variables to change, in a new allocation.
    #[cfg(test)]
    fn variables_mut(&mut self) -> &mut ProjectedVariables {
        self.variables = EntityVariables::new(self.variables.variables.clone());
        &mut Arc::get_mut(&mut self.variables)
            .expect("a new allocation")
            .variables
    }

    /// As [`Self::set_entity_variable`], for an instance's variable;
    /// nothing without the instance.
    pub fn set_instance_variable(&mut self, instance: u16, id: u32, value: Option<Value>) {
        if let Some(instance) = self.instances.get_mut(&instance)
            && !instance.variables.holds(id, value.as_ref())
        {
            edit_instance(instance).variables.set(id, value);
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
        let instance = edit_instance(instance);
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

    /// Whether `other` is made of the same shared parts (its entity
    /// variables and instances, by address) and the same actions: then
    /// it holds the same values while both are kept ([`SharedDeltas`]).
    fn same_parts(&self, other: &Self) -> bool {
        self.entity == other.entity
            && self.view == other.view
            && self.action_serial == other.action_serial
            && Arc::ptr_eq(&self.variables, &other.variables)
            && self.actions == other.actions
            && self.instances.len() == other.instances.len()
            && self.instances.iter().zip(&other.instances).all(
                |((id, part), (other_id, other_part))| {
                    id == other_id && Arc::ptr_eq(part, other_part)
                },
            )
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
                        chase_steps: false,
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
            let states = &mut edit_instance(instance).states;
            match value {
                Some(value) => states.insert(raw, value),
                None => states.remove(&raw),
            };
        }
        self
    }
}

#[derive(Clone, Debug)]
struct ProjectedInstance {
    graph: u32,
    /// The entity it was created with when that isn't the snapshot's own
    /// (the descriptor's creator, `wire::write_descriptor`).
    creator: Option<u32>,
    parent: Option<(u16, u16)>,
    /// The scope state that opened it on another entity (a scope
    /// instance, `vm/scopes.rs`).
    scope: Option<wire::ScopeDescriptor>,
    variables: ProjectedVariables,
    states: BTreeMap<u16, ProjectedState>,
    events: Vec<ProjectedEvent>,
    self_same: SelfSame,
    /// Whether it is `==` to itself (no NaN), worked out once.
    reflexive: SelfSame,
    compared: Compared<ProjectedInstance, InstanceDifferences>,
}

/// Field by field; a share compared with itself is equal at once when it
/// is equal to itself (shared parts are most of a projection's).
impl PartialEq for ProjectedInstance {
    fn eq(&self, other: &Self) -> bool {
        if std::ptr::eq(self, other) {
            return *self.reflexive.0.get_or_init(|| self.same_fields(self));
        }
        self.same_fields(other)
    }
}

impl ProjectedInstance {
    fn same_fields(&self, other: &Self) -> bool {
        self.graph == other.graph
            && self.creator == other.creator
            && self.parent == other.parent
            && self.scope == other.scope
            && self.variables == other.variables
            && self.states == other.states
            && self.events == other.events
    }

    /// The same projection bit for bit (unlike `==`, 0.0 and -0.0
    /// differ), each map in the same order: whatever is written from one
    /// is written the same from the other.
    fn identical(&self, other: &Self) -> bool {
        fn values<'a>(
            a: impl ExactSizeIterator<Item = (&'a String, &'a Value)>,
            b: impl ExactSizeIterator<Item = (&'a String, &'a Value)>,
        ) -> bool {
            a.len() == b.len()
                && a.zip(b)
                    .all(|((a_key, a), (b_key, b))| a_key == b_key && a.identical(b))
        }
        self.graph == other.graph
            && self.creator == other.creator
            && self.parent == other.parent
            && self.scope == other.scope
            && self.variables.values.len() == other.variables.values.len()
            && self
                .variables
                .values
                .iter()
                .zip(&other.variables.values)
                .all(|((a_id, a), (b_id, b))| a_id == b_id && a.identical(b))
            && self.variables.bindings == other.variables.bindings
            && self.states.len() == other.states.len()
            && self
                .states
                .iter()
                .zip(&other.states)
                .all(|((a_raw, a), (b_raw, b))| {
                    a_raw == b_raw
                        && a.active == b.active
                        && a.activation == b.activation
                        && a.volley == b.volley
                        && a.chase_steps == b.chase_steps
                        && values(a.payload.iter(), b.payload.iter())
                })
            && self.events == other.events
    }
}

/// [`ProjectedInstance::self_same`], worked out once. It takes no part
/// in comparisons, and a clone (made to be edited) works it out again.
#[derive(Debug, Default)]
struct SelfSame(std::sync::OnceLock<bool>);

impl Clone for SelfSame {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for SelfSame {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl ProjectedInstance {
    /// Whether a delta ([`SnapshotEncoder::differences`]) finds each of
    /// its variables, links and states the same as itself (not so for a
    /// NaN), so that a delta between two shares of it lists only what was
    /// touched.
    fn self_same(&self) -> bool {
        *self.self_same.0.get_or_init(|| {
            self.variables
                .values
                .values()
                .all(|value| same_on_wire(value, value))
                && self
                    .variables
                    .bindings
                    .values()
                    .all(|links| PartialEq::eq(links, links))
                && self
                    .states
                    .values()
                    .all(|state| PartialEq::eq(state, state))
        })
    }
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
    /// A chase that steps each frame (no [`crate::vm::TimedChase`]): its
    /// client steps it each frame too, from the same payload.
    chase_steps: bool,
}

impl ProjectedState {
    /// Whether this is `was` moved on by the chase's own steps alone: the
    /// same activation of a chase that steps each frame, running in both,
    /// arrived in both or in neither. The owner's client steps it each
    /// frame from the payload it holds (`0x7FF78AAB1050`), so these values
    /// are no correction for it: a chunk for each step rolled Doomfist's
    /// client back every frame of a slam (`0B27` st17 toward `#10471e`;
    /// `refs/reports/Doomfist.md` §15).
    fn chase_stepped_from(&self, was: &Self) -> bool {
        self.chase_steps
            && was.chase_steps
            && self.active
            && was.active
            && self.activation == was.activation
            && self.payload.get("reached") == was.payload.get("reached")
            && self.payload.contains_key("remaining") == was.payload.contains_key("remaining")
    }
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
    /// The ids of the variables whose value or links on the wire differ
    /// from `was`'s.
    fn differing_from(&self, was: &Self) -> BTreeSet<u32> {
        let links_differ = |id: &u32| self.bindings.get(id) != was.bindings.get(id);
        // Usually both list the same ids in the same order (the same
        // variables projected again): entry by entry, without lookups.
        if self.values.keys().eq(was.values.keys()) {
            return self
                .values
                .iter()
                .zip(&was.values)
                .filter(|((id, now), (_, before))| !same_on_wire(now, before) || links_differ(id))
                .map(|((&id, _), _)| id)
                .collect();
        }
        self.values
            .keys()
            .chain(was.values.keys())
            .copied()
            .filter(|id| {
                let now = self.values.get(id).unwrap_or(&Value::Null);
                let before = was.values.get(id).unwrap_or(&Value::Null);
                !same_on_wire(now, before) || links_differ(id)
            })
            .collect()
    }

    /// Whether variable `id`'s value or links on the wire differ from
    /// `was`'s ([`Self::differing_from`] for one id).
    fn differs_at(&self, was: &Self, id: u32) -> bool {
        let now = self.values.get(&id).unwrap_or(&Value::Null);
        let before = was.values.get(&id).unwrap_or(&Value::Null);
        !same_on_wire(now, before) || self.bindings.get(&id) != was.bindings.get(&id)
    }

    /// The ids of the variables whose value differs from `was`'s bit for
    /// bit (an absent one counts as null), or whose links differ. The
    /// others compare with any base as `was`'s do.
    fn changed_from(&self, was: &Self) -> BTreeSet<u32> {
        let links_differ = |id: &u32| self.bindings.get(id) != was.bindings.get(id);
        if self.values.keys().eq(was.values.keys()) {
            return self
                .values
                .iter()
                .zip(&was.values)
                .filter(|((id, now), (_, before))| !now.identical(before) || links_differ(id))
                .map(|((&id, _), _)| id)
                .collect();
        }
        self.values
            .keys()
            .chain(was.values.keys())
            .copied()
            .filter(|id| {
                let now = self.values.get(id).unwrap_or(&Value::Null);
                let before = was.values.get(id).unwrap_or(&Value::Null);
                !now.identical(before) || links_differ(id)
            })
            .collect()
    }
}

/// What a shared projection part (a projection's [`EntityVariables`], a
/// [`ProjectedInstance`]) was found to differ in from each base it was
/// compared with. The chunks queued from a projection share its parts, so
/// every client whose base is the same gets the same comparison, made
/// once: a Workshop match's mode entity has hundreds of action instances,
/// each with its parameters, which every client's delta compared again
/// each tick (most of a 12-player C32S2 match's tick). A weak reference
/// names each base: while it is listed, no other part can take the
/// base's address. It takes no part in comparisons, and a clone (made to
/// be edited) starts empty; an edit in place empties it
/// ([`edit_instance`]).
#[derive(Debug)]
struct Compared<T, D>(Mutex<Vec<(Weak<T>, Arc<D>)>>);

impl<T, D> Default for Compared<T, D> {
    fn default() -> Self {
        Self(Mutex::default())
    }
}

impl<T, D> Clone for Compared<T, D> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl<T, D> PartialEq for Compared<T, D> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T, D> Compared<T, D> {
    /// The differences from `base`, made by `compare` the first time.
    fn with(&self, base: &Arc<T>, compare: impl FnOnce() -> D) -> Arc<D> {
        let mut found = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, differences)) = found
            .iter()
            .find(|(kept, _)| std::ptr::eq(kept.as_ptr(), Arc::as_ptr(base)))
        {
            return Arc::clone(differences);
        }
        let differences = Arc::new(compare());
        found.retain(|(kept, _)| kept.strong_count() != 0);
        found.push((Arc::downgrade(base), Arc::clone(&differences)));
        differences
    }

    /// The differences from `base`, if they were made.
    fn found(&self, base: &Arc<T>) -> Option<Arc<D>> {
        let found = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        found
            .iter()
            .find(|(kept, _)| std::ptr::eq(kept.as_ptr(), Arc::as_ptr(base)))
            .map(|(_, differences)| Arc::clone(differences))
    }

    fn clear(&mut self) {
        self.0
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// A projection's entity variables, shared with its copies (the chunks
/// queued from it, a client's base), with their comparisons
/// ([`Compared`]). A Workshop match's mode entity holds the rules' global
/// variables here.
#[derive(Clone, Debug, Default)]
struct EntityVariables {
    variables: ProjectedVariables,
    compared: Compared<EntityVariables, BTreeSet<u32>>,
    previous: Previous,
    /// Whether they are `==` to themselves (no NaN), worked out once.
    reflexive: SelfSame,
    edited: Edited,
}

/// By the variables; a share compared with itself is equal at once when
/// it is equal to itself.
impl PartialEq for EntityVariables {
    fn eq(&self, other: &Self) -> bool {
        if std::ptr::eq(self, other) {
            return *self
                .reflexive
                .0
                .get_or_init(|| PartialEq::eq(&self.variables, &self.variables));
        }
        self.variables == other.variables
    }
}

/// The projection an [`EntityVariables`] was made after (the kept one it
/// replaced, [`SnapshotEncoder::project_cached`]), and the ids whose value
/// or links differ from it bit for bit, found once. It takes no part in
/// comparisons, and a clone (made to be edited) has none.
#[derive(Debug, Default)]
struct Previous {
    projection: Weak<EntityVariables>,
    changed: OnceLock<BTreeSet<u32>>,
}

impl Clone for Previous {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// The last edit made to a projection's entity variables
/// ([`ProjectedSnapshot::set_entity_variables`]) and the variables it made:
/// the same edit of the same variables again (the controller's overrides,
/// each tick) gives the same share. It takes no part in comparisons, and a
/// clone has none.
#[derive(Debug, Default)]
struct Edited(Mutex<Option<EntityEdit>>);

type EntityEdit = (Vec<(u32, Option<Value>)>, Arc<EntityVariables>);

impl Clone for Edited {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl Edited {
    /// The variables `edits` made last time, if they were these.
    fn made_by(&self, edits: &[(u32, Option<Value>)]) -> Option<Arc<EntityVariables>> {
        let kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (kept_edits, made) = kept.as_ref()?;
        let same = kept_edits.len() == edits.len()
            && kept_edits
                .iter()
                .zip(edits)
                .all(|((kept_id, kept), (id, value))| {
                    kept_id == id
                        && match (kept, value) {
                            (Some(kept), Some(value)) => kept.identical(value),
                            (None, None) => true,
                            _ => false,
                        }
                });
        same.then(|| Arc::clone(made))
    }

    fn keep(&self, edits: Vec<(u32, Option<Value>)>, made: Arc<EntityVariables>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((edits, made));
    }
}

impl std::ops::Deref for EntityVariables {
    type Target = ProjectedVariables;

    fn deref(&self) -> &ProjectedVariables {
        &self.variables
    }
}

impl EntityVariables {
    fn new(variables: ProjectedVariables) -> Arc<Self> {
        Arc::new(Self {
            variables,
            ..Self::default()
        })
    }

    /// Variables projected again after `previous`.
    fn after(variables: ProjectedVariables, previous: &Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            variables,
            previous: Previous {
                projection: Arc::downgrade(previous),
                changed: OnceLock::new(),
            },
            ..Self::default()
        })
    }

    /// The ids of the variables that differ from `base`'s. Where the
    /// previous projection's differences from the same base were made
    /// (each client's base stays while the mode entity's variables move on
    /// each tick), only the variables changed since then are compared
    /// again: the others are bit for bit what was compared then.
    fn differing_from(&self, base: &Arc<Self>) -> Arc<BTreeSet<u32>> {
        self.compared.with(base, || {
            let previous = self.previous.projection.upgrade();
            let Some((previous, before)) = previous
                .as_ref()
                .and_then(|previous| Some((previous, previous.compared.found(base)?)))
            else {
                return self.variables.differing_from(&base.variables);
            };
            let changed = self
                .previous
                .changed
                .get_or_init(|| self.variables.changed_from(&previous.variables));
            before
                .iter()
                .copied()
                .filter(|id| !changed.contains(id))
                .chain(
                    changed
                        .iter()
                        .copied()
                        .filter(|&id| self.variables.differs_at(&base.variables, id)),
                )
                .collect()
        })
    }
}

/// What a [`ProjectedInstance`] differs in from a base's: its variables'
/// ids and its states' slots.
#[derive(Debug)]
struct InstanceDifferences {
    variables: BTreeSet<u32>,
    states: BTreeSet<u16>,
}

/// The observer deltas made to a projection, each with what it was made
/// from: the encoder's setup, the stream's base and its queued chunks'
/// snapshots. An observer delta with no wheel action in it depends on
/// nothing else (not on the chunks' numbers or keys), so every client
/// whose stream holds the same snapshots gets the same bytes, made once:
/// the mode entity's frame goes to every client of a match each tick,
/// another player's body to each client that sees it, and each client's
/// delta of a Workshop match's mode entity (hundreds of action instances)
/// was most of a 12-player C32S2 tick. Snapshots are told apart by their
/// shared parts ([`ProjectedSnapshot::same_parts`]); an entry holds its
/// snapshots, so no part it lists can change or be freed while listed (an
/// edit of a shared part copies it, [`edit_instance`]). It takes no part
/// in comparisons, and a clone starts empty.
#[derive(Debug, Default)]
struct SharedDeltas(Mutex<VecDeque<SharedDelta>>);

/// The entries a projection keeps: about one for each distinct stream
/// (clients whose acknowledgements keep the same pace share one).
const SHARED_DELTAS: usize = 16;

#[derive(Debug)]
struct SharedDelta {
    /// The encoder's graphs and codecs, by address (a match keeps its own
    /// for as long as its projections).
    graphs: usize,
    codecs: usize,
    unknowns: Unknowns,
    hidden: Vec<u16>,
    presenters: bool,
    frame_ms: u32,
    current: ProjectedSnapshot,
    base: ProjectedSnapshot,
    pending: Vec<ProjectedSnapshot>,
    /// The frame and its keys; none: nothing to send.
    delta: Option<(BitWriter, BTreeSet<ChangeKey>)>,
}

impl SharedDelta {
    fn made_by(&self, encoder: &SnapshotEncoder<'_>, frame_ms: u32) -> bool {
        self.graphs == std::ptr::from_ref(encoder.graphs).addr()
            && self.codecs == std::ptr::from_ref(encoder.codecs).addr()
            && self.unknowns == encoder.unknowns
            && self.hidden == encoder.hidden
            && self.presenters == encoder.presenters
            && self.frame_ms == frame_ms
    }
}

impl Clone for SharedDeltas {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PartialEq for SharedDeltas {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl SharedDeltas {
    /// The delta made for a stream with the same snapshots, if one was.
    fn find(
        &self,
        encoder: &SnapshotEncoder<'_>,
        frame_ms: u32,
        current: &ProjectedSnapshot,
        stream: &BodyStream<ProjectedSnapshot>,
    ) -> Option<Option<(BitWriter, BTreeSet<ChangeKey>)>> {
        let (_, base) = stream.base()?;
        let entries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        entries
            .iter()
            .find(|entry| {
                entry.made_by(encoder, frame_ms)
                    && entry.base.same_parts(base)
                    && entry.pending.len() == stream.pending().count()
                    && entry
                        .pending
                        .iter()
                        .zip(stream.pending())
                        .all(|(kept, (_, pending, _))| kept.same_parts(pending))
                    && entry.current.same_parts(current)
            })
            .map(|entry| entry.delta.clone())
    }

    fn insert(
        &self,
        encoder: &SnapshotEncoder<'_>,
        frame_ms: u32,
        current: &ProjectedSnapshot,
        stream: &BodyStream<ProjectedSnapshot>,
        delta: Option<(BitWriter, BTreeSet<ChangeKey>)>,
    ) {
        let Some((_, base)) = stream.base() else {
            return;
        };
        let entry = SharedDelta {
            graphs: std::ptr::from_ref(encoder.graphs).addr(),
            codecs: std::ptr::from_ref(encoder.codecs).addr(),
            unknowns: encoder.unknowns,
            hidden: encoder.hidden.to_vec(),
            presenters: encoder.presenters,
            frame_ms,
            current: current.clone(),
            base: base.clone(),
            pending: stream
                .pending()
                .map(|(_, pending, _)| pending.clone())
                .collect(),
            delta,
        };
        let mut entries = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if entries.len() == SHARED_DELTAS {
            entries.pop_front();
        }
        entries.push_back(entry);
    }
}

/// `instance` to edit, alone (a clone if shared), with no comparisons
/// kept from before the edit.
fn edit_instance(instance: &mut Arc<ProjectedInstance>) -> &mut ProjectedInstance {
    let instance = Arc::make_mut(instance);
    instance.compared.clear();
    instance.reflexive = SelfSame::default();
    instance
}

impl ProjectedVariables {
    /// Whether [`Self::set`] with these would change nothing: the variable
    /// unbound, with this value bit for bit (or absent for none).
    fn holds(&self, variable: u32, value: Option<&Value>) -> bool {
        !self.bindings.contains_key(&variable)
            && match value {
                Some(value) => self
                    .values
                    .get(&variable)
                    .is_some_and(|now| now.identical(value)),
                None => !self.values.contains_key(&variable),
            }
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
    /// Observer frames carry UX presenters ([`Self::with_presenters`]).
    presenters: bool,
}

impl<'a> SnapshotEncoder<'a> {
    pub fn new(graphs: &'a GraphStore, codecs: &'a CodecTable) -> Self {
        Self {
            graphs,
            codecs,
            unknowns: Unknowns::for_build(),
            hidden: &[],
            presenters: false,
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

    /// Observer frames carry the UX presenters too. A body's are its
    /// owner's HUD (ammo, ability icons), which no other client draws, so
    /// they are left out of others' frames (upstream `observers.py`); the
    /// mode entity's are the match's own, which every client shows from
    /// its frames: `180A`'s card presenter st19 and the cards' graphs'
    /// (`027F` st6) are remote-synced states.
    pub fn with_presenters(self) -> Self {
        Self {
            presenters: true,
            ..self
        }
    }

    pub fn project(
        &self,
        snapshot: &Snapshot,
        view: View,
    ) -> Result<ProjectedSnapshot, SnapshotError> {
        self.project_with(snapshot, view, &mut 0, None)
    }

    /// A later projection of the stream `baseline` started.
    pub fn project_next(
        &self,
        snapshot: &Snapshot,
        baseline: &ProjectedSnapshot,
    ) -> Result<ProjectedSnapshot, SnapshotError> {
        if snapshot.entity != baseline.entity {
            return Err(SnapshotError::DifferentStream);
        }
        self.project_with(snapshot, baseline.view, &mut 0, None)
    }

    /// [`Self::project`] (no `baseline`) or [`Self::project_next`], kept in
    /// `cache`: the projection `cache` holds is the answer again while the
    /// snapshot keeps its revision and the clock hasn't passed what the
    /// projection read of it ([`ProjectionCache`]). Otherwise each instance
    /// keeps its projection while it keeps its revision, so do the
    /// instances its links name, and the clock stays within what it read
    /// ([`InstanceStamp`]). The cache must stay with one encoder setup
    /// (graphs, codecs, unknowns, hidden instances).
    pub fn project_cached<'c>(
        &self,
        snapshot: &Snapshot,
        view: View,
        baseline: Option<&ProjectedSnapshot>,
        cache: &'c mut Option<ProjectionCache>,
    ) -> Result<&'c ProjectedSnapshot, SnapshotError> {
        if let Some(baseline) = baseline
            && (snapshot.entity != baseline.entity || baseline.view != view)
        {
            return Err(SnapshotError::DifferentStream);
        }
        let reusable = cache.as_ref().is_some_and(|kept| {
            kept.revision == snapshot.revision()
                && kept.made_ms <= snapshot.now_ms
                && snapshot.now_ms <= kept.fresh_until_ms
                && kept.projected.view == view
        });
        if !reusable {
            let kept = cache.take().filter(|kept| {
                kept.revision.same_component(snapshot.revision()) && kept.projected.view == view
            });
            let mut fresh_until_ms = u64::MAX;
            let mut instances = BTreeMap::new();
            let projected = self.project_with(
                snapshot,
                view,
                &mut fresh_until_ms,
                Some((kept.as_ref(), &mut instances)),
            )?;
            *cache = Some(ProjectionCache {
                revision: snapshot.revision(),
                made_ms: snapshot.now_ms,
                fresh_until_ms,
                projected,
                instances,
            });
        }
        Ok(&cache.as_ref().expect("a kept projection").projected)
    }

    /// `fresh_until_ms` comes down to the last clock reading (from
    /// `snapshot.now_ms` on) the projection holds for: the projection
    /// reads the clock only for a pulse's freshness and a wheel action's
    /// lifetime, each of which runs out once. With `stamps`, an instance of
    /// the kept projection that still holds is shared, not made again, and
    /// each instance's stamp is recorded.
    fn project_with(
        &self,
        snapshot: &Snapshot,
        view: View,
        fresh_until_ms: &mut u64,
        mut stamps: Option<(Option<&ProjectionCache>, &mut BTreeMap<u16, InstanceStamp>)>,
    ) -> Result<ProjectedSnapshot, SnapshotError> {
        if !snapshot.initialization_complete() {
            return Err(SnapshotError::Incomplete {
                entity: snapshot.entity,
                diagnostics: snapshot.diagnostics.len(),
                first: format!("{:?}", snapshot.diagnostics.first()),
            });
        }
        let mut instances = BTreeMap::new();
        for (&id, instance) in &snapshot.instances {
            if self.hidden.contains(&id) {
                continue;
            }
            let graph = self.graph(id, instance.graph)?;
            // A graph's server-only list (`m_9CEC6985`) names its own
            // instance variables: across the 104319 graphs every listed
            // identifier a node writes is written with instance scope, and
            // none as an entity variable. Leaving the entity variable of
            // the same identifier out of frames lost it on the owner's
            // client: `01D8` lists its #417, so Doomfist's client reset
            // the airborne flag #417e at every record and took the
            // grounded slam in the air (refs report `Doomfist.md` §13).
            let exclusions = graph.server_only_variables();
            if let Some((kept, stamps)) = &mut stamps {
                if let Some((projected, stamp)) =
                    kept.and_then(|kept| kept.instance(id, instance.revision(), snapshot))
                {
                    *fresh_until_ms = (*fresh_until_ms).min(stamp.fresh_until_ms);
                    instances.insert(id, Arc::clone(projected));
                    stamps.insert(
                        id,
                        InstanceStamp {
                            sources: stamp.sources.clone(),
                            ..*stamp
                        },
                    );
                    continue;
                }
            }
            let mut instance_fresh_ms = u64::MAX;
            let variables =
                self.project_variables(snapshot, id, &instance.variables, exclusions, view)?;
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
                // A state that is off and was never activated is left out
                // whatever its codec (below).
                if !state.active && state.activation == 0 {
                    continue;
                }
                if !node.is_networked()
                    || (view == View::Observer
                        && ((node.is_presenter() && !self.presenters) || !node.is_remote_synced()))
                {
                    continue;
                }
                let codec = self.codecs.node_codec(node);
                if !state.active && !(state.activation != 0 && codec.is_some_and(Codec::stec)) {
                    continue;
                }
                let Some(payload) = codec.filter(|codec| codec.projected()).and_then(|codec| {
                    state_payload(state, codec, snapshot.now_ms, &mut instance_fresh_ms)
                }) else {
                    if self.unknowns == Unknowns::Tolerate {
                        log_left_out(&node.class, codec, state.kind);
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
                        chase_steps: state
                            .chase
                            .as_ref()
                            .is_some_and(|chase| chase.timed.is_none()),
                    },
                );
            }
            let events = if view == View::Owner {
                self.project_timers(snapshot, id, graph)?
            } else {
                Vec::new()
            };
            *fresh_until_ms = (*fresh_until_ms).min(instance_fresh_ms);
            if let Some((_, stamps)) = &mut stamps {
                stamps.insert(
                    id,
                    InstanceStamp {
                        revision: instance.revision(),
                        sources: binding_sources(snapshot, id, instance, exclusions, view),
                        made_ms: snapshot.now_ms,
                        fresh_until_ms: instance_fresh_ms,
                    },
                );
            }
            let projected = ProjectedInstance {
                graph: instance.graph,
                creator: (instance.source != snapshot.entity).then_some(instance.source),
                parent: instance.parent,
                scope: instance.scope.as_ref().map(|scope| wire::ScopeDescriptor {
                    scope: scope.id,
                    entity: scope.source.entity,
                    instance: scope.source.instance,
                    state: scope.source.state,
                }),
                variables,
                states,
                events,
                self_same: SelfSame::default(),
                reflexive: SelfSame::default(),
                compared: Compared::default(),
            };
            // An instance whose revision moved without a change it projects
            // keeps its share (and the comparisons made to it).
            let kept = stamps
                .as_ref()
                .and_then(|(kept, _)| kept.as_ref()?.projected.instances.get(&id))
                .filter(|kept| kept.identical(&projected));
            instances.insert(id, kept.map_or_else(|| Arc::new(projected), Arc::clone));
        }
        // The entity's variables as the kept projection has them, shared
        // (with the comparisons made to them) while none changed.
        let kept = stamps
            .as_ref()
            .and_then(|(kept, _)| *kept)
            .map(|kept| &kept.projected.variables);
        let variables = match self.project_variables_over(
            snapshot,
            0,
            &snapshot.variables,
            &BTreeSet::from([OWNER_HEALTH]),
            view,
            kept.map(|kept| &kept.variables),
        )? {
            Some(variables) => match kept {
                Some(kept) => EntityVariables::after(variables, kept),
                None => EntityVariables::new(variables),
            },
            None => Arc::clone(kept.expect("only a kept projection is kept")),
        };
        let actions = self.project_actions(snapshot, &instances, fresh_until_ms)?;
        Ok(ProjectedSnapshot {
            entity: snapshot.entity,
            view,
            variables,
            instances,
            actions,
            action_serial: snapshot
                .frame_actions
                .last()
                .map_or(0, |action| action.serial),
            shared: SharedDeltas::default(),
        })
    }

    /// The wheel actions the frames may still carry: of a live instance
    /// (the same one, not a reused id), at most [`ACTION_LIFETIME_MS`] old;
    /// `0686`'s only while its root wait runs (upstream `callout_snapshot`).
    fn project_actions(
        &self,
        snapshot: &Snapshot,
        instances: &BTreeMap<u16, Arc<ProjectedInstance>>,
        fresh_until_ms: &mut u64,
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
            if snapshot.now_ms.saturating_sub(action.time_ms) > ACTION_LIFETIME_MS {
                continue;
            }
            let expires_ms = action.time_ms.saturating_add(ACTION_LIFETIME_MS);
            *fresh_until_ms = (*fresh_until_ms).min(expires_ms);
            if instance.graph == CALLOUT_GRAPH
                && !instance.states.get(&0).is_some_and(|state| state.active)
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
        Ok(self
            .project_variables_over(snapshot, owner, values, excluded, view, None)?
            .unwrap_or_default())
    }

    /// The variables as [`Self::project_variables`] projects them, compared
    /// as they are worked out with `kept` (the same variables' last
    /// projection): `None` when they are `kept` as it is, so nothing was
    /// copied. Without `kept`, always some.
    fn project_variables_over(
        &self,
        snapshot: &Snapshot,
        owner: u16,
        values: &Variables,
        excluded: &BTreeSet<u32>,
        view: View,
        kept: Option<&ProjectedVariables>,
    ) -> Result<Option<ProjectedVariables>, SnapshotError> {
        // `kept`'s entries matched in order so far; the new projection is
        // made from the first that differs.
        let mut matched = 0;
        let mut matched_bindings = 0;
        let mut projected = kept.is_none().then(ProjectedVariables::default);
        let prefix = |kept: &ProjectedVariables, matched: usize| {
            let mut prefix = ProjectedVariables::default();
            for (&id, value) in kept.values.iter().take(matched) {
                prefix.values.insert(id, value.clone());
                if let Some(links) = kept.bindings.get(&id) {
                    prefix.bindings.insert(id, links.clone());
                }
            }
            prefix
        };
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
            let mut links = None;
            if let Some(bindings) = bindings.filter(|_| view == View::Owner) {
                let projected_links = self.project_bindings(snapshot, owner, variable, bindings)?;
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
                // server or is an Ability's stack (slot 1). A RayCast's
                // bindings (slots 0-3) leave the own value alone too: its
                // begin binds them without writing it (`vm/rays.rs`).
                let own = |binding: &StackBinding| {
                    binding.slot == 0
                        || snapshot
                            .instances
                            .get(&binding.instance)
                            .and_then(|instance| instance.states.get(&binding.raw_state))
                            .is_some_and(|state| state.kind == StateKind::Ray)
                };
                if projected_links.len() == bindings.len() && bindings.iter().all(own) {
                    value = base;
                }
                links = (!projected_links.is_empty()).then_some(projected_links);
            }
            if matches!(value, Value::Null) && links.is_none() {
                continue;
            }
            if projected.is_none()
                && let Some(kept) = kept
            {
                if let Some((&id, was)) = kept.values.get_index(matched)
                    && id == variable
                    && was.identical(value)
                    && kept.bindings.get(&variable) == links.as_ref()
                {
                    matched += 1;
                    matched_bindings += usize::from(links.is_some());
                    continue;
                }
                projected = Some(prefix(kept, matched));
            }
            let projected = projected
                .as_mut()
                .expect("made at the start or at the first difference");
            if let Some(links) = links {
                projected.bindings.insert(variable, links);
            }
            projected.values.insert(variable, value.clone());
        }
        Ok(match (projected, kept) {
            (Some(projected), _) => Some(projected),
            (None, Some(kept))
                if matched == kept.values.len() && matched_bindings == kept.bindings.len() =>
            {
                None
            }
            (None, Some(kept)) => Some(prefix(kept, matched)),
            (None, None) => unreachable!("made at the start without a kept projection"),
        })
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
            // Volatile as the client's own link of each is: a chase's
            // (begin `0x7FF78AAAF430` → `0x7FF78AA82F90`/`0x7FF78AA83570`,
            // 1), a cooldown's (restart `0x7FF789BBB110` → `0x7FF78AA849B0`,
            // 1), a RayCast's (`0x7FF789B940E0`, 1) and a GrappleCast's
            // outputs (`0x7FF789BA1AA0`, 1); an ability's stack push
            // (`0x7FF789BA6D40`, 0). A Stack's is volatile when its value
            // read something volatile (`0x7FF78AA822B0`), which the VM
            // doesn't track: never here. A frame's binding replaces the
            // client's, and one that isn't volatile freezes the variable
            // at the value it was first read at: the HUD's cooldowns and the
            // Rocket Punch's charge stood still.
            let volatile = match source.states.get(&binding.raw_state) {
                Some(state) if state.active => match (state.kind, binding.slot) {
                    (StateKind::Stack { .. }, 0) | (StateKind::Ability { .. }, 1) => false,
                    (StateKind::ChaseVar | StateKind::Ability { .. }, 0)
                    | (StateKind::GrappleCast, 0 | 1)
                    | (StateKind::Ray, 0..=3) => true,
                    _ => return Err(invalid()),
                },
                _ => return Err(invalid()),
            };
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
                    volatile,
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
            wire.scope = instance.scope;
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
        Ok(wire::remote_full(
            &instances,
            &snapshot.variables.variables.values,
        )?)
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
            wire::write_descriptor(
                &mut out,
                graph,
                instance.creator,
                instance.parent,
                instance.scope,
            )?;
        }
        out.w_u16(0);
        let variables = &snapshot.variables.variables;
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
                let codec = match self.codecs.node_codec(node) {
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
            if !baseline.variables.variables.bindings.is_empty() {
                let mut variables = baseline.variables.variables.clone();
                variables.bindings.clear();
                baseline.variables = EntityVariables::new(variables);
            }
            for instance in baseline.instances.values_mut().map(edit_instance) {
                instance.variables.bindings.clear();
                instance.states.clear();
                instance.events.clear();
            }
        }
        Ok(stream.full(&payload, baseline)?)
    }

    /// Encode a cumulative projection from the acknowledged baseline. The
    /// caller supplies the owner phase selected by the body's stream.
    ///
    /// A correction goes only when the projection differs from what the
    /// owner's client holds once the queued chunks arrive
    /// ([`BodyStream::expected`]), or when the newest one was lost. The
    /// client rolls its statescript back for every chunk with the
    /// correction bit and compares nothing first (applier `0x7FF78991E3C0`
    /// → `0x7FF78991F5D0`), and it needs none to move on: once its oldest
    /// unconfirmed frame is 40 frames old it settles its prediction itself
    /// (`0x7FF78991B200`, each frame). Retail sent owner data only for
    /// frame ranges with changes (refs/reports/OwnerPrediction.md
    /// "DayOwnerChunks").
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
        if base.entity != snapshot.entity || base.view != view || snapshot.view != view {
            return Err(SnapshotError::DifferentStream);
        }
        if matches!(kind, DeltaKind::Correction(_))
            && !stream.resend_due()
            && !self.owner_changed(stream, snapshot)?
        {
            return Ok(None);
        }
        // An observer delta that carries no wheel action is the same for
        // every stream with the same snapshots ([`SharedDeltas`]). Action
        // keys sort last.
        let mut shared = view == View::Observer
            && !snapshot
                .actions
                .iter()
                .any(|action| action.serial > base.action_serial)
            && stream.pending().all(|(_, _, keys)| {
                !keys
                    .last()
                    .is_some_and(|key| matches!(key, ChangeKey::Action { .. }))
            });
        if shared && let Some(made) = snapshot.shared.find(self, frame_ms, snapshot, stream) {
            let Some((frame, keys)) = made else {
                return Ok(None);
            };
            let mut delivered = snapshot.clone();
            delivered.action_serial = base.action_serial;
            return Ok(Some(stream.delta(&frame, delivered, keys, kind)?));
        }
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
        let mut touched = match view {
            View::Observer => {
                let (touched, own_keys) = self.observer_touched(stream, base, &known);
                // A chunk's own keys make the delta that stream's alone.
                shared &= !own_keys;
                touched
            }
            View::Owner => stream.touched(),
        };
        // An idle volley's ammunition goes in every owner chunk, but makes
        // none go by itself.
        touched.extend(self.idle_volley_ammo(snapshot)?);
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
            if shared {
                snapshot
                    .shared
                    .insert(self, frame_ms, snapshot, stream, None);
            }
            return Ok(None);
        }
        let mut delivered = snapshot.clone();
        delivered.action_serial = action.map_or(base.action_serial, |(_, action)| action.serial);
        let frame = self.delta_frame(snapshot, &delta, kind, frame_ms, action)?;
        if shared {
            snapshot.shared.insert(
                self,
                frame_ms,
                snapshot,
                stream,
                Some((frame.clone(), delta.keys.clone())),
            );
        }
        Ok(Some(stream.delta(&frame, delivered, delta.keys, kind)?))
    }

    /// What an observer delta must carry besides the base's differences:
    /// what the client may hold from a queued chunk that the base doesn't
    /// have. That is each queued chunk's keys whose values there differ
    /// from the base, not all its keys: a chunk made while an older one was
    /// on its way carries that one's keys too, so taking them all kept
    /// every key ever changed in every later chunk for as long as one chunk
    /// was always on its way. A Workshop match's mode frame changes each
    /// tick (the run timer), so its deltas grew to the whole start-up's
    /// changes, 24 KB in parts, and the HUD reached the client every 30
    /// ticks or later. Actions keep the stamp of the chunk that first
    /// carried them. Also whether a chunk's own keys went in.
    fn observer_touched(
        &self,
        stream: &BodyStream<ProjectedSnapshot>,
        base: &ProjectedSnapshot,
        known: &BTreeSet<u16>,
    ) -> (BTreeSet<ChangeKey>, bool) {
        let mut touched = BTreeSet::new();
        let mut own_keys = false;
        for (_, pending, keys) in stream.pending() {
            touched.extend(
                keys.iter()
                    .filter(|key| matches!(key, ChangeKey::Action { .. }))
                    .copied(),
            );
            match self.difference_keys(base, pending, known) {
                Ok(keys) => touched.extend(keys),
                // An instance id the base had for another graph: as before,
                // everything the chunk carried.
                Err(_) => {
                    touched.extend(keys.iter().copied());
                    own_keys = true;
                }
            }
        }
        (touched, own_keys)
    }

    /// Whether the owner's projection differs from what his client holds
    /// once the queued chunks arrive: a state, variable, binding or
    /// instance, or an action no queued chunk carries yet. A chase's own
    /// steps are not a difference ([`ProjectedState::chase_stepped_from`]).
    fn owner_changed(
        &self,
        stream: &BodyStream<ProjectedSnapshot>,
        current: &ProjectedSnapshot,
    ) -> Result<bool, SnapshotError> {
        let (Some((_, base)), Some(expected)) = (stream.base(), stream.expected()) else {
            return Ok(true);
        };
        let action = current
            .actions
            .iter()
            .find(|action| action.serial > base.action_serial);
        if action.is_some_and(|action| action.serial > expected.action_serial) {
            return Ok(true);
        }
        let keys = self.difference_keys(expected, current, &BTreeSet::new())?;
        Ok(keys.iter().any(|key| match *key {
            ChangeKey::State(id, raw) => !current
                .state(id, raw)
                .zip(expected.state(id, raw))
                .is_some_and(|(now, was)| now.chase_stepped_from(was)),
            _ => true,
        }))
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
    /// count. The server can't tell whether the client predicted a press
    /// after a chunk's frame, so every owner chunk that goes lists it; an
    /// unchanged one makes no chunk go ([`Self::queue_delta`]).
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
            for &(raw, variable) in graph.volley_ammo() {
                if instance.states.get(&raw).is_some_and(|state| state.active) {
                    continue;
                }
                let (owner, values) = match variable.scope {
                    Scope::Entity => (0, &current.variables.variables.values),
                    Scope::Instance => (id, &instance.variables.values),
                };
                if values.contains_key(&variable.id) {
                    keys.insert(ChangeKey::Variable(owner, variable.id));
                }
            }
        }
        Ok(keys)
    }

    /// The delta from `base` to `current`.
    fn differences<'s>(
        &self,
        base: &ProjectedSnapshot,
        current: &'s ProjectedSnapshot,
        touched: &BTreeSet<ChangeKey>,
        known: &BTreeSet<u16>,
    ) -> Result<SnapshotDelta<'s>, SnapshotError> {
        self.delta_from(base, current, touched, known, true)
    }

    /// The keys of [`Self::differences`], without the values they list:
    /// what a queued chunk or the owner's expected frame touched.
    fn difference_keys(
        &self,
        base: &ProjectedSnapshot,
        current: &ProjectedSnapshot,
        known: &BTreeSet<u16>,
    ) -> Result<BTreeSet<ChangeKey>, SnapshotError> {
        Ok(self
            .delta_from(base, current, &BTreeSet::new(), known, false)?
            .keys)
    }

    /// [`Self::differences`]; `values`: with the values it lists.
    fn delta_from<'s>(
        &self,
        base: &ProjectedSnapshot,
        current: &'s ProjectedSnapshot,
        touched: &BTreeSet<ChangeKey>,
        known: &BTreeSet<u16>,
        values: bool,
    ) -> Result<SnapshotDelta<'s>, SnapshotError> {
        let mut delta = SnapshotDelta::default();
        // The entity's variables that differ from the base's, found once
        // for every client with this base ([`EntityVariables`]), and the
        // touched ones.
        let differing = current.variables.differing_from(&base.variables);
        delta.variables = listed_variables(
            0,
            differing
                .iter()
                .copied()
                .chain(touched_variables(0, touched))
                .collect(),
            &current.variables.variables,
            &mut delta.keys,
            values,
        );
        for (&id, next) in &current.instances {
            let before = base.instances.get(&id);
            if before.is_some_and(|before| {
                before.graph != next.graph
                    || before.creator != next.creator
                    || before.parent != next.parent
                    || before.scope != next.scope
            }) {
                return Err(SnapshotError::ReboundInstance(id));
            }
            let descriptor = before.is_none();
            if descriptor {
                delta.keys.insert(ChangeKey::Instance(id));
            }
            // A baseline instance the projection shares lists only what
            // was touched: each entry compares equal to itself; untouched,
            // nothing (most of a Workshop mode entity's hundreds of action
            // instances, in each client's deltas). What it differs in from
            // another base is found once for every client with that base
            // ([`Compared`]).
            let shared = before.is_some_and(|before| Arc::ptr_eq(before, next) && next.self_same());
            if shared
                && touched
                    .range(ChangeKey::Variable(id, 0)..=ChangeKey::Variable(id, u32::MAX))
                    .next()
                    .is_none()
                && touched
                    .range(ChangeKey::State(id, 0)..=ChangeKey::State(id, u16::MAX))
                    .next()
                    .is_none()
            {
                continue;
            }
            let differing = before.filter(|_| !shared).map(|before| {
                next.compared.with(before, || InstanceDifferences {
                    variables: next.variables.differing_from(&before.variables),
                    states: next
                        .states
                        .keys()
                        .chain(before.states.keys())
                        .copied()
                        .filter(|raw| next.states.get(raw) != before.states.get(raw))
                        .collect(),
                })
            });
            let touched_ids = touched_variables(id, touched);
            let variables = match &differing {
                Some(differing) => listed_variables(
                    id,
                    differing
                        .variables
                        .iter()
                        .copied()
                        .chain(touched_ids)
                        .collect(),
                    &next.variables,
                    &mut delta.keys,
                    values,
                ),
                None if shared => listed_variables(
                    id,
                    touched_ids.collect(),
                    &next.variables,
                    &mut delta.keys,
                    values,
                ),
                None => new_variables(id, &next.variables, touched, &mut delta.keys, values),
            };
            let graph = self.graph(id, next.graph)?;
            let touched_states = touched
                .range(ChangeKey::State(id, 0)..=ChangeKey::State(id, u16::MAX))
                .filter_map(|key| match key {
                    ChangeKey::State(_, state) => Some(*state),
                    _ => None,
                });
            let raw_slots: BTreeSet<_> = match &differing {
                Some(differing) => differing
                    .states
                    .iter()
                    .copied()
                    .chain(touched_states)
                    .collect(),
                None if shared => touched_states.collect(),
                None => next.states.keys().copied().chain(touched_states).collect(),
            };
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
                    None if self.codecs.node_codec(node).is_some_and(Codec::stec) => {
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
                    wire::write_descriptor(
                        &mut body,
                        graph,
                        current.creator,
                        current.parent,
                        current.scope,
                    )?;
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
                .node_codec(node)
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
            if !self.codecs.node_sendable(node, true) {
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
                .node_codec(node)
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
    variables: ListedVariables<'a>,
    instances: BTreeMap<u16, InstanceDelta<'a>>,
    keys: BTreeSet<ChangeKey>,
}

#[derive(Default)]
struct InstanceDelta<'a> {
    current: Option<&'a ProjectedInstance>,
    descriptor: bool,
    variables: ListedVariables<'a>,
    states: BTreeMap<u16, Option<Cow<'a, ProjectedState>>>,
}

/// The variables a delta lists, borrowed from the projection: each id in
/// id order with its value (an unset one as null), and the projection's
/// links (each listed id's own).
#[derive(Default)]
struct ListedVariables<'a> {
    values: Vec<(u32, &'a Value)>,
    bindings: Option<&'a Bindings>,
}

impl ListedVariables<'_> {
    fn bindings(&self) -> &Bindings {
        static NONE: Bindings = Bindings::new();
        self.bindings.unwrap_or(&NONE)
    }

    fn write(&self, out: &mut BitWriter, owner: u16) -> Result<(), WireError> {
        wire::bound_variable_entries(
            out,
            &self.values,
            ValueEncoding::Compact,
            owner,
            self.bindings(),
        )
    }

    /// Tests: listed variable `id`'s value.
    #[cfg(test)]
    fn value(&self, id: u32) -> &Value {
        self.values
            .iter()
            .find_map(|&(listed, value)| (listed == id).then_some(value))
            .expect("a listed variable")
    }

    /// Tests: whether any listed variable goes with links.
    #[cfg(test)]
    fn has_links(&self) -> bool {
        self.values
            .iter()
            .any(|(id, _)| self.bindings().contains_key(id))
    }
}

/// The variables of a new instance's `current` a delta lists: those the
/// client doesn't hold as unset ([`ProjectedVariables::differing_from`]
/// an empty base) and the touched ones.
fn new_variables<'a>(
    instance: u16,
    current: &'a ProjectedVariables,
    touched: &BTreeSet<ChangeKey>,
    changes: &mut BTreeSet<ChangeKey>,
    values: bool,
) -> ListedVariables<'a> {
    let listed = current
        .differing_from(&ProjectedVariables::default())
        .into_iter()
        .chain(touched_variables(instance, touched))
        .collect();
    listed_variables(instance, listed, current, changes, values)
}

/// The ids of `instance`'s variables in `touched`.
fn touched_variables(instance: u16, touched: &BTreeSet<ChangeKey>) -> impl Iterator<Item = u32> {
    touched
        .range(ChangeKey::Variable(instance, 0)..=ChangeKey::Variable(instance, u32::MAX))
        .filter_map(|key| match key {
            ChangeKey::Variable(_, id) => Some(*id),
            _ => None,
        })
}

/// The variables `ids` of `current` (unset ones as null), in id order, for
/// a delta: each a change of `instance`. Without `values`, only the
/// changes.
fn listed_variables<'a>(
    instance: u16,
    ids: BTreeSet<u32>,
    current: &'a ProjectedVariables,
    changes: &mut BTreeSet<ChangeKey>,
    values: bool,
) -> ListedVariables<'a> {
    static NULL: Value = Value::Null;
    let mut result = ListedVariables {
        values: Vec::new(),
        bindings: values.then_some(&current.bindings),
    };
    for id in ids {
        changes.insert(ChangeKey::Variable(instance, id));
        if values {
            result
                .values
                .push((id, current.values.get(&id).unwrap_or(&NULL)));
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
    vars: &ListedVariables<'_>,
) -> Result<(), WireError> {
    // The listed variables are in id order: each sync slot takes its id's
    // entry (once), the rest go as the list.
    let mut sync = BTreeMap::new();
    let mut taken = vec![false; vars.values.len()];
    for (slot, var) in graph.sync_vars.iter().enumerate() {
        let Some(id) = var.0.filter(|_| var.1 == 0) else {
            continue;
        };
        if let Ok(at) = vars.values.binary_search_by_key(&id, |&(id, _)| id)
            && !taken[at]
        {
            taken[at] = true;
            sync.insert(slot, vars.values[at]);
        }
    }
    write_mask(out, graph.sync_vars.len(), &sync.keys().copied().collect());
    let bindings = vars.bindings();
    for (id, value) in sync.values() {
        let links = bindings.get(id).map_or(&[][..], Vec::as_slice);
        wire::bound_variable(out, value, ValueEncoding::Compact, owner, links)?;
    }
    let extra: Vec<_> = vars
        .values
        .iter()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(&entry, _)| entry)
        .collect();
    wire::bound_variable_entries(out, &extra, ValueEncoding::Compact, owner, bindings)
}

/// How long a pulse stays fresh for its first read
/// (`ClientOnlyPulser.FRESH_MS` in `ow174/game/script/nodes.py`).
const PULSE_FRESH_MS: u64 = 500;

/// A release build leaves an active state out of frames when it can't
/// write its payload ([`Unknowns::Tolerate`]): the client never runs it, or
/// ends the copy it predicted at its next rollback past the begin. So the
/// gap is logged, once per class and server run.
fn log_left_out(class: &str, codec: Option<&Codec>, kind: StateKind) {
    use std::{
        collections::HashSet,
        sync::{LazyLock, Mutex},
    };
    static LOGGED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
    let mut logged = LOGGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !logged.insert(class.to_owned()) {
        return;
    }
    let name = codec
        .and_then(|codec| codec.codec.as_deref())
        .unwrap_or("-");
    let reason = match codec {
        None => "the client's codec table has no such class".to_owned(),
        Some(codec) if !codec.verified() => format!("codec {name} has no verified layout"),
        Some(codec) if !codec.projected() => format!("codec {name} has no payload family"),
        Some(_) => format!("no payload of codec {name} for the VM's {kind:?}"),
    };
    eprintln!("statescript state {class} left out of frames: {reason}");
}

/// A pulse's freshness is the one value read from the clock: while it is
/// fresh, `fresh_until_ms` comes down to its last fresh time.
fn state_payload(
    state: &State,
    codec: &Codec,
    now_ms: u64,
    fresh_until_ms: &mut u64,
) -> Option<StatePayload> {
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
            // The Charge's turn (`STU_C168636D`, reader nullsub).
            | StateKind::Turn
            | StateKind::Rise
            | StateKind::AiMove
            | StateKind::ModifyHealth { .. }
            | StateKind::Status
            | StateKind::ProjectileSpeed
            | StateKind::CombatMod
            | StateKind::Assist
            | StateKind::HeroSelect
            // TrackStatTime: the client's reader is a nullsub
            // (`0x7FF78ABF5640`, `statescript_codecs.json`).
            | StateKind::StatTime
            // The searches the owner's client runs itself: the ray casts
            // (RayCast, TrackRay), the wall check (`STU_FBFF5DE7`). Left
            // out, its client never ran them: Doomfist's ground ray `0B27`
            // st8 and the punch's wall check `0D06` st7 and wall rays
            // st79/st80 were the server's values a round trip late
            // (`refs/reports/Doomfist.md` §15).
            | StateKind::Ray
            | StateKind::WallCheck
            // StateCreateEntity (record #1699, reader nullsub): the entity
            // it made goes to the clients as an entity of its own.
            | StateKind::CreateEntity { .. }
            // The pickup pair (server-only, the base state's reader).
            | StateKind::Pickup
            | StateKind::PickupSearch { .. },
            Some("none"),
        ) if codec.empty() => StatePayload::new(),
        // A presenter's bit (`+0x470`, `vm/presenter.rs`): fading out.
        (StateKind::Presenter, Some("bit")) => StatePayload::from([(
            "value".into(),
            Value::Bool(state.presenter.is_some_and(|presenter| presenter.bit)),
        )]),
        // ScaleTime's frames (`vm/time_scale.rs`): the frame it starts, the
        // frame it stops; the codec sends the one of its side.
        (StateKind::ScaleTime { on, off }, Some("value_slot")) => StatePayload::from([
            ("active".into(), Value::Bool(state.active)),
            ("on".into(), Value::Integer(i64::from(on))),
            ("off".into(), Value::Integer(i64::from(off))),
        ]),
        // The landing search (`0B27` st6, `vm/landing.rs`): its block's
        // byte, which its begin sets to 0 and only its input plug
        // `m_6647ED22` (no link in the landing form) sets to 1
        // (0x7FF7897F8770, 0x7FF7897F8AC0). Left out, the owner's client
        // never searched: it waited a round trip for the server's point and
        // its button state st64 cancelled a tapped airborne slam.
        (StateKind::Landing, Some("none+u8")) => {
            StatePayload::from([("value".into(), Value::Integer(0))])
        }
        // Shockwave (`STU_01D67FA0`, Doomfist's slam `0B27` st44,
        // `vm/shockwave.rs`): its block's list (8-byte records,
        // `Read_STU_01D67FA0` 0x7FF789BA11B0 replaces the client's with it)
        // is empty: a 0 bit. The hit pass writes its hits to the node's
        // array variable, not to the list; no form the VM runs fills it.
        // Left out, the owner's client ended the shockwave it predicted at
        // its next rollback.
        (StateKind::Shockwave, Some("shockwave")) => StatePayload::new(),
        // The view limits (`STU_9935FBC8`, `vm/view_limits.rs`): only the
        // owner's full frames carry the block (no entity, the six angles,
        // the entity's yaw 0); deltas the absent link. Left out, the
        // owner's client dropped the limits of Doomfist's punch and slam
        // at its next rollback.
        (StateKind::ViewLimits { radians }, Some("linked_target")) => StatePayload::from([(
            "values".into(),
            Value::Array(
                radians
                    .iter()
                    .map(|&bits| Value::Float(f32::from_bits(bits).into()))
                    .chain([Value::Float(0.0)])
                    .collect(),
            ),
        )]),
        // The Grappling Hook cast (`STU_4BBC5D09`): its begin; an owner frame
        // carries the milliseconds from it to the frame (`codecs.rs`
        // `project_start`).
        (StateKind::GrappleCast, Some("start")) => StatePayload::from([(
            "start".into(),
            Value::Integer(i64::try_from(state.start_ms).ok()?),
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
        (StateKind::Pulser { pulses, pulsed_ms }, Some("pulser")) => {
            let fresh =
                pulsed_ms.is_some_and(|pulsed| now_ms.saturating_sub(pulsed) <= PULSE_FRESH_MS);
            if let Some(pulsed) = pulsed_ms.filter(|_| fresh) {
                *fresh_until_ms = (*fresh_until_ms).min(pulsed.saturating_add(PULSE_FRESH_MS));
            }
            StatePayload::from([
                ("fresh".into(), Value::Bool(fresh)),
                ("count".into(), Value::Integer(i64::from(pulses))),
            ])
        }
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
        // Recall's rewind (`codecs.rs` `project_rewind`): its end (the
        // VM's finish time) and the frames of history it rewinds along.
        (StateKind::Rewind { end_ms, frames }, Some("time16+s64")) => StatePayload::from([
            ("end".into(), Value::Integer(i64::try_from(end_ms).ok()?)),
            ("frames".into(), Value::Integer(i64::from(frames))),
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
        // A scope state's targets (`vm/scopes.rs`, `Write_STU_07AD90EE`
        // 0x7FF789C56790): per target its value (the target entity, or a
        // value scope's listed value), the entity holding its instance and
        // the instance once opened, its index, the keep flag and the bit
        // every created hit sets (`+37`); the closing bit (`+0x3B4`) is
        // cleared at the state's begin.
        (StateKind::Scope, Some("hits")) => StatePayload::from([
            (
                "hits".into(),
                Value::Array(
                    state
                        .scopes
                        .iter()
                        .enumerate()
                        .map(|(index, hit)| {
                            Value::Array(vec![
                                hit.value
                                    .clone()
                                    .unwrap_or(Value::Entity(hit.target)),
                                hit.instance.map_or(Value::Null, |_| Value::Entity(hit.target)),
                                Value::Integer(i64::from(hit.instance.unwrap_or(0))),
                                Value::Integer(index as i64),
                                Value::Bool(hit.keep),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("flag".into(), Value::Bool(false)),
        ]),
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
