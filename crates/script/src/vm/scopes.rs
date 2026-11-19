//! Per-target scopes (`STU_550D0C5C`, a state over `STU_07AD90EE`): a state
//! that, for each entity its `m_targets` lists, runs part of its own graph
//! on that entity (TrainingPartner.md "Day (2026-11-19)").
//!
//! Client research (1.74, state record #204, runtime vtable
//! 0x7FF78B581F00):
//!
//! - The scope's subgraph is the state's `m_C41A9BE2` plug
//!   (`STU_E2DB5B06`), whose `m_5AEA0A51` is the scope's id.
//! - For each new target the update (`0x7FF789C55DB0`, through
//!   `0x7FF789C60530` and slot 57 `0x7FF789C512E0`) creates an instance of
//!   the state's own graph on the target's script component (component 47
//!   of the target entity), with the creating instance's `#27` as its
//!   creator; sets the instance's scope (`+8`) to the scope's id; adds the
//!   state (its entity, instance and raw index) to the instance's parent
//!   references (`+808`, `sub_7FF789C23860` → `sub_7FF789C1F140`); and posts
//!   it a begin of the same state. That copy of the state is in a scope
//!   instance of its own scope, so its begin (`0x7FF789C55B80`) follows the
//!   scope plug and its update builds no scopes.
//! - In the scope instance `STU_B5A0CAF0` (owner) is the target (the entity
//!   of the component holding the instance, `Eval_STU_B5A0CAF0`
//!   0x7FF789BFFFE0), entity variables are the target's, a LogicalButton
//!   reads the target's input, and a parent read (opcode 10) reads the
//!   referenced instance, the scope state's.
//! - Entries run in the instances of their scope (`STUStatescriptEntry`
//!   `+0x88` `m_5AEA0A51`; 0 for an instance that isn't a scope's): the
//!   console `265F`'s `1213` entry is scope 126's, its `1214` entry scope
//!   12's.
//! - A target that leaves loses its reference; with none left the instance
//!   goes, unless the state keeps it (`m_4B7DF406`; `0x7FF789C359F0`): a
//!   kept instance ends by itself (`265F`'s scope 197 exits after 2 s).
//! - The state's frame record ("hits", `Write_STU_07AD90EE`
//!   0x7FF789C56790) lists each target: its value, the entity holding its
//!   instance and the instance's id, its index, the keep flag and a set
//!   bit; then a bit (`+0x3B4`, cleared at begin). A client runs no scopes
//!   of a server entity's state itself (the update returns early for a
//!   remote host); it gets the instances in the targets' own frames.
//!
//! The scope state asks the world to open and close the instances
//! ([`World::change_scope`]); the match hands them to the target's
//! component ([`Component::change_scope`]) and reports the instance back
//! ([`Component::scope_opened`]). A scope instance copies the scope state's
//! instance variables when it opens: its parent reads see them as they
//! were then (assumption; the client reads the parent live: `265F` reads
//! its parent only at the scope's begin).

use super::*;

/// The scope state activation that opened scope instances: its entity,
/// instance, raw state and activation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScopeSource {
    pub entity: u32,
    pub instance: u16,
    pub state: u16,
    pub activation: u16,
}

/// What a scope instance keeps of the state that opened it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeLink {
    /// The scope's id (`m_C41A9BE2.m_5AEA0A51`).
    pub id: u32,
    pub source: ScopeSource,
    /// The scope state's instance variables when it opened, for the
    /// instance's parent reads.
    pub parent: Variables,
}

/// A target of a scope state and its instance there, once the match
/// reported it ([`Component::scope_opened`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeHit {
    pub target: u32,
    pub instance: Option<u16>,
    /// The state keeps the instance when the target leaves (`m_4B7DF406`).
    pub keep: bool,
}

/// A scope instance a scope state opens on a target or closes there
/// ([`World::change_scope`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ScopeChange {
    Open {
        source: ScopeSource,
        target: u32,
        graph: u32,
        scope: u32,
        /// The creating instance's `#27`.
        creator: u32,
        keep: bool,
        parent: Variables,
    },
    Close {
        source: ScopeSource,
        target: u32,
    },
}

impl ScopeChange {
    /// The entity it is for.
    pub fn target(&self) -> u32 {
        match *self {
            Self::Open { target, .. } | Self::Close { target, .. } => target,
        }
    }
}

/// A variable a scope instance sets in the scope state's instance on
/// another entity (`STU_D49BBD03`, [`World::set_parent_variable`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ParentWrite {
    /// The scope state whose instance takes it.
    pub source: ScopeSource,
    /// The instance variable.
    pub variable: u32,
    pub value: Value,
}

/// The scope id of a scope state: its scope plug's `m_5AEA0A51`.
pub(super) fn scope_id(node: &Node) -> u32 {
    node.field("m_C41A9BE2.m_5AEA0A51")
        .and_then(Json::as_u64)
        .and_then(|id| u32::try_from(id).ok())
        .unwrap_or(0)
}

/// The scope an entry runs in (`STUStatescriptEntry.m_5AEA0A51`; 0: the
/// instances that are no scope's).
pub(super) fn entry_scope(node: &Node) -> u32 {
    field(&node.fields, "m_5AEA0A51")
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .unwrap_or(0)
}

impl Component {
    /// Open or close a scope instance on this entity. An open creates an
    /// instance of the scope's graph in the scope, created with the
    /// creator, and begins the scope state's copy in it, which follows its
    /// scope plug; returns the instance. A close takes the instance away
    /// unless its state keeps it. None: no instance opened (a missing
    /// graph).
    pub fn change_scope(
        &mut self,
        change: &ScopeChange,
        world: &mut impl World,
    ) -> Result<Option<u16>, InitFault> {
        match change {
            ScopeChange::Open {
                source,
                graph,
                scope,
                creator,
                keep,
                parent,
                ..
            } => {
                if let Some(&(id, _)) = self.machine.scopes.get(source) {
                    return Ok(Some(id));
                }
                let id = u16::try_from(self.machine.next_id)
                    .map_err(|_| InitFault::InstanceIdExhausted)?;
                let link = ScopeLink {
                    id: *scope,
                    source: *source,
                    parent: parent.clone(),
                };
                let mut created = false;
                self.run(world, |vm| {
                    created = vm.create_scoped(id, *graph, Some(link), *creator)?;
                    if created {
                        vm.post(id, Event::Begin(source.state))?;
                    }
                    vm.run()
                })?;
                if !created {
                    return Ok(None);
                }
                self.machine.scopes.insert(*source, (id, *keep));
                Ok(Some(id))
            }
            ScopeChange::Close { source, .. } => {
                let Some((id, keep)) = self.machine.scopes.remove(source) else {
                    return Ok(None);
                };
                if keep || !self.machine.snapshot.instances.contains_key(&id) {
                    return Ok(None);
                }
                self.run(world, |vm| {
                    vm.post(id, Event::ExitInstance)?;
                    vm.run()
                })?;
                Ok(None)
            }
        }
    }

    /// The match opened the scope instance `instance` on `target` for the
    /// scope state `source` of this component: its frame record names it.
    pub fn scope_opened(&mut self, source: ScopeSource, target: u32, instance: u16) {
        let Some(state) = self
            .machine
            .snapshot
            .instances
            .get_mut(&source.instance)
            .and_then(|item| item.states.get_mut(&source.state))
            .filter(|state| state.active && state.activation == source.activation)
        else {
            return;
        };
        if let Some(hit) = state.scopes.iter_mut().find(|hit| hit.target == target) {
            hit.instance = Some(instance);
        }
    }
}

impl<W: World> Vm<'_, '_, '_, W> {
    /// The scope id of an instance: 0 for one that isn't a scope's.
    pub(super) fn instance_scope(&self, id: u16) -> u32 {
        self.machine.snapshot.instances[&id]
            .scope
            .as_ref()
            .map_or(0, |scope| scope.id)
    }

    /// A scope state's begin: in a scope instance of its own scope it
    /// follows the scope plug; elsewhere it opens its targets' scopes and
    /// keeps them up to date each tick.
    pub(super) fn begin_scope(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let scope = scope_id(node);
        if scope != 0 && self.instance_scope(key.instance) == scope {
            self.follow(key.instance, node, "m_C41A9BE2", Some(key), depth)?;
            return Ok(());
        }
        self.refresh_scopes(key, node)?;
        self.register_tick(key, self.machine.now_ms);
        Ok(())
    }

    fn scope_source(&self, key: StateKey) -> ScopeSource {
        ScopeSource {
            entity: self.machine.snapshot.entity,
            instance: key.instance,
            state: key.raw,
            activation: self.state(key).map_or(0, |state| state.activation),
        }
    }

    /// The scope state's tick: a scope for each target that came, none for
    /// each that left (`0x7FF789C60F70`, `0x7FF789C60530`).
    pub(super) fn refresh_scopes(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        if self.instance_scope(id) == scope_id(node) {
            return Ok(());
        }
        let listed = self.listed_entities(key, node, "m_targets");
        let source = self.scope_source(key);
        let state = self.state(key).expect("active scope state");
        let left: Vec<u32> = state
            .scopes
            .iter()
            .map(|hit| hit.target)
            .filter(|target| !listed.contains(target))
            .collect();
        let came: Vec<u32> = listed
            .into_iter()
            .filter(|&target| !state.scopes.iter().any(|hit| hit.target == target))
            .collect();
        if left.is_empty() && came.is_empty() {
            return Ok(());
        }
        let site = self.site(id, node);
        let mut taken = true;
        for &target in &left {
            taken &= self
                .world
                .change_scope(site, ScopeChange::Close { source, target });
        }
        let mut opened = Vec::new();
        if !came.is_empty() {
            let keep = match field(&node.fields, "m_4B7DF406") {
                Json::Null => false,
                config => self
                    .evaluate(id, node, config, Some(key))
                    .is_some_and(|value| truthy(&value)),
            };
            if !field(&node.fields, "m_DED58406").is_null()
                || !field(&node.fields, "m_42BA09E3").is_null()
                || !field(&node.fields, "m_339A63DB").is_null()
            {
                self.assume(site, "STU_550D0C5C reuse and index outputs", "not read");
            }
            let parent = self.instance_values(id);
            let creator = self.source_of(id);
            let graph = self.machine.snapshot.instances[&id].graph;
            for &target in &came {
                taken &= self.world.change_scope(
                    site,
                    ScopeChange::Open {
                        source,
                        target,
                        graph,
                        scope: scope_id(node),
                        creator,
                        keep,
                        parent: parent.clone(),
                    },
                );
                opened.push(ScopeHit {
                    target,
                    instance: None,
                    keep,
                });
            }
        }
        if !taken {
            self.assume(site, &node.class, "no scopes (the world opens none)");
        }
        let state = self.state_mut(key).expect("active scope state");
        state.scopes.retain(|hit| !left.contains(&hit.target));
        state.scopes.extend(opened);
        Ok(())
    }

    /// The scope state ended, or its instance went: each target loses its
    /// scope (kept instances stay; `0x7FF789C55C70`).
    pub(super) fn end_scopes(&mut self, key: StateKey) {
        let Some(state) = self.state(key) else {
            return;
        };
        let targets: Vec<u32> = state.scopes.iter().map(|hit| hit.target).collect();
        if targets.is_empty() {
            return;
        }
        let source = self.scope_source(key);
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("scope state");
        let site = self.site(key.instance, node);
        for target in targets {
            self.world
                .change_scope(site, ScopeChange::Close { source, target });
        }
        if let Some(state) = self.state_mut(key) {
            state.scopes.clear();
        }
    }

    /// An instance's variables as its expressions read them now.
    fn instance_values(&self, id: u16) -> Variables {
        let instance = &self.machine.snapshot.instances[&id];
        instance
            .variables
            .keys()
            .map(|&variable| {
                (
                    variable,
                    self.machine.value(VarKey::Instance(id, variable), None),
                )
            })
            .collect()
    }

    /// `STU_D49BBD03` (`Action_STU_D49BBD03` 0x7FF789BC8A00): evaluate
    /// `m_value` and set it into `m_out_Var` of each instance the
    /// instance references (its parent references `+808`, a scope
    /// instance's scope state; without one its creator's, `+864`), where
    /// that instance is the authority's. The console's scope 12 sets its
    /// `#940` to the picked hero so (`265F` rid53): the picker's default
    /// (rid56 `parent1.#940`) and the console's shared `#940`. The
    /// world sets it in the scope state's instance; the scope instance's
    /// own copy of its parent's variables follows. An instance with no
    /// scope (its creator's instance), an entity variable or a world that
    /// sets none is an assumption: nothing is set.
    pub(super) fn set_parent_variable(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let site = self.site(id, node);
        let target = match expr::lvalue(field(&node.fields, "m_out_Var")) {
            Ok(target) => target,
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(false);
            }
        };
        let Some(value) = self.evaluate(id, node, field(&node.fields, "m_value"), None) else {
            return Ok(false);
        };
        let source = self.machine.snapshot.instances[&id]
            .scope
            .as_ref()
            .map(|scope| scope.source);
        let set = match (target, source) {
            (Some(variable), Some(source)) if variable.scope == expr::Scope::Instance => {
                let write = ParentWrite {
                    source,
                    variable: variable.id,
                    value: value.clone(),
                };
                let set = self.world.set_parent_variable(site, write);
                if set {
                    let scope = self
                        .machine
                        .snapshot
                        .instances
                        .get_mut(&id)
                        .and_then(|item| item.scope.as_mut());
                    if let Some(scope) = scope {
                        scope.parent.insert(variable.id, value);
                    }
                }
                set
            }
            (None, _) => true,
            _ => false,
        };
        if !set {
            self.assume(site, &node.class, "sets no parent instance's variable");
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }
}
