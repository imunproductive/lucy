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
//! - With the reuse flag (`m_DED58406`, a bool config, false when unset)
//!   slot 57 first looks among the target's instances for one of the
//!   state's graph and scope id (`+488`, `+8` = state `+944`) that isn't
//!   ending (`+792 & 0x104` clear), the last in order. Found, it adds the
//!   state to that instance's parent references and posts it a state
//!   event (event type 1, `0x7FF7898881F0`) of kind 2 for the state's copy
//!   instead of a begin. The event dispatcher (`0x7FF78AAA1690` case 1)
//!   calls an active copy's event slot 18 (`0x7FF789C56080`: kind 2 follows
//!   the scope plug `m_C41A9BE2` again, kind 1 `m_0647CAD7`) and an
//!   inactive copy's slot 19 (`ret 0` here). A copy whose begin aborts it
//!   (every Workshop action's: Set Status `1C4B`, the HUD switches `298C`,
//!   `29C3`) is inactive, so a reuse leaves the running instance as it is:
//!   a second Set Status of a status the player has neither restarts nor
//!   lengthens it.
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
//!
//! `STU_77E13026` (state record #203, runtime vtable 0x7FF78B582478) opens
//! its scope instances for values instead of targets: slot 56
//! (`0x7FF789C522B0`) lists the items of `m_814F8EC3` (an array's items,
//! else the value itself; nulls left out), and for each new one slot 57
//! (`0x7FF789C52400`) creates an instance of the state's own graph on the
//! state's own entity in the scope, writes the item into its
//! `m_3DC6FAAA` and its index into its `m_339A63DB`, and begins the
//! state's copy there. Items are told apart by value
//! (`0x7FF789C60F70`: an item stays while the list still holds as many
//! equal values). `180A` st17 so opens a scope 822 instance for each graph
//! of the shown card (`#13151.id(16748)`) with the graph in `#9694`;
//! there st16 plays it.
//!
//! `STU_CB4905E9` (state record #208, runtime vtable 0x7FF78B583E00) plays
//! another graph on its targets the same way: its after-begin
//! (`0x7FF789C5C080`) evaluates the graph of `m_69F4B52C` (a graph with
//! overrides, `STU_9DB46B98`; empty in every use) or else `m_script`
//! (`0x7FF789C15510`: a graph asset or a variable holding one; `180A`
//! st16 plays `#9694`), lists the valid entities of `m_targets` and hands
//! the list to `0x7FF789C61200`/`0x7FF789C60870`, which play the graph on
//! each new target and stop it on those that left; its end
//! (`0x7FF789C5CA80`) stops them all. So it runs as a scope state whose
//! targets get a root instance of that graph with `m_params` as a
//! PlayScript's ([`ScopeChange::Open`] `played`), created with the playing
//! entity (`26DB` reads the payload's distance as `#27.id(479)`). Not
//! modelled: its event slot (`0x7FF789C5CCC0`, event 1, which slot 60
//! `0x7FF789C5CC40` posts once no played instance is left) finishes it,
//! or follows `m_9DD3E3F1` when `m_8C797956` holds (false at `180A` st16;
//! `1225`'s `1246` and `26DB` never end).

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
#[derive(Clone, Debug, PartialEq)]
pub struct ScopeHit {
    pub target: u32,
    pub instance: Option<u16>,
    /// The state keeps the instance when the target leaves (`m_4B7DF406`).
    pub keep: bool,
    /// A value scope's item (`STU_77E13026`); its instance is on the
    /// state's own entity, the `target`.
    pub value: Option<Value>,
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
        /// Take the target's last live instance of this graph and scope
        /// instead of opening another (`m_DED58406`).
        reuse: bool,
        parent: Variables,
        /// Not a scope instance but `graph` played on the target
        /// (`STU_CB4905E9`): a root instance whose start entries take these
        /// parameters (`m_params`, as a PlayScript's).
        played: Option<Vec<(u64, Value)>>,
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

/// The state that plays a graph on its targets as a scope state opens
/// scopes (module notes).
const PLAYED_GRAPHS: &str = "STU_CB4905E9";

/// The state that opens a scope instance on its own entity for each value
/// its list holds (module notes).
const VALUE_SCOPES: &str = "STU_77E13026";

/// Whether `STU_CB4905E9` takes its graph from `m_script`: `m_69F4B52C`
/// names none (an empty `STU_9DB46B98`).
fn plays_script(node: &Node) -> bool {
    let overrides = field(&node.fields, "m_69F4B52C");
    overrides.is_null()
        || (overrides.get("$").and_then(Json::as_str) == Some("STU_9DB46B98")
            && field(overrides, "m_6525E98B").is_null())
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
    /// scope plug; returns the instance. A reusing open takes the last live
    /// instance of the graph and scope instead, if there is one, and posts
    /// its copy of the state the reuse's event ([`Event::Rescope`]). A
    /// close takes the instance away unless its state keeps it or another
    /// scope state still has it. None: no instance opened (a missing
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
                reuse,
                parent,
                played,
                ..
            } => {
                if let Some(&(id, _)) = self.machine.scopes.get(source) {
                    return Ok(Some(id));
                }
                if let Some(parameters) = played {
                    let id = u16::try_from(self.machine.next_id)
                        .map_err(|_| InitFault::InstanceIdExhausted)?;
                    let mut created = false;
                    self.run(erase(world), |vm| {
                        created = vm.create(id, *graph, None, Variables::new(), *creator)?;
                        if created {
                            vm.start_entries(id, parameters, *creator)?;
                        }
                        vm.run()
                    })?;
                    if !created {
                        return Ok(None);
                    }
                    self.machine.scopes.insert(*source, (id, *keep));
                    return Ok(Some(id));
                }
                let reused = self
                    .machine
                    .snapshot
                    .instances
                    .iter()
                    .rev()
                    .find(|(_, instance)| {
                        *reuse
                            && instance.graph == *graph
                            && instance
                                .scope
                                .as_ref()
                                .is_some_and(|link| link.id == *scope)
                    })
                    .map(|(&id, _)| id);
                if let Some(id) = reused {
                    self.run(erase(world), |vm| {
                        vm.post(id, Event::Rescope(source.state))?;
                        vm.run()
                    })?;
                    self.machine.scopes.insert(*source, (id, *keep));
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
                self.run(erase(world), |vm| {
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
                let shared = self.machine.scopes.values().any(|&(other, _)| other == id);
                if keep || shared || !self.machine.snapshot.instances.contains_key(&id) {
                    return Ok(None);
                }
                self.run(erase(world), |vm| {
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

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
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
    /// each that left (`0x7FF789C60F70`, `0x7FF789C60530`). A target whose
    /// instance the match hasn't opened yet is asked for again each tick:
    /// the match opens it once the target's player runs that entity, which
    /// a dummy bot made this tick doesn't yet (the console's scope 197 on
    /// a re-picked Training Partner); the client makes it at once.
    pub(super) fn refresh_scopes(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let played = node.class == PLAYED_GRAPHS;
        if !played && self.instance_scope(id) == scope_id(node) {
            return Ok(());
        }
        if node.class == VALUE_SCOPES {
            return self.refresh_value_scopes(key, node);
        }
        let played = if played {
            match self.played_graph(key, node) {
                Some(Some(graph)) => Some(graph),
                // No graph (an unset `m_script`): nothing plays, as on the
                // client.
                Some(None) => return Ok(()),
                None => {
                    self.report(
                        self.site(id, node),
                        Issue::UnsupportedField("STU_CB4905E9 graph"),
                    );
                    return Ok(());
                }
            }
        } else {
            None
        };
        let listed = self.listed_entities(key, node, "m_targets");
        let source = self.scope_source(key);
        let state = self.state(key).expect("active scope state");
        let left: Vec<u32> = state
            .scopes
            .iter()
            .map(|hit| hit.target)
            .filter(|target| !listed.contains(target))
            .collect();
        let unopened: Vec<u32> = state
            .scopes
            .iter()
            .filter(|hit| hit.instance.is_none() && listed.contains(&hit.target))
            .map(|hit| hit.target)
            .collect();
        let came: Vec<u32> = listed
            .into_iter()
            .filter(|&target| !state.scopes.iter().any(|hit| hit.target == target))
            .collect();
        if left.is_empty() && came.is_empty() && unopened.is_empty() {
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
        if !came.is_empty() || !unopened.is_empty() {
            let keep = match field(&node.fields, "m_4B7DF406") {
                Json::Null => false,
                config => self
                    .evaluate(id, node, config, Some(key))
                    .is_some_and(|value| truthy(&value)),
            };
            // The reuse flag (`m_DED58406`, `+0x110`): slot 57
            // `0x7FF789C512E0` evaluates it as a bool, false when unset
            // (`sub_7FF7898060F0(state, config, 0)`); the target takes it
            // ([`Component::change_scope`]). The index outputs `m_42BA09E3`
            // (`+0x118`, written with the reused instance's count + 1) and
            // `m_339A63DB` (`+0x100`) are unset in every use.
            let reuse = match field(&node.fields, "m_DED58406") {
                Json::Null => false,
                config => self
                    .evaluate(id, node, config, Some(key))
                    .is_some_and(|value| truthy(&value)),
            };
            if !field(&node.fields, "m_42BA09E3").is_null()
                || !field(&node.fields, "m_339A63DB").is_null()
            {
                self.assume(site, "STU_550D0C5C index outputs", "not written");
            }
            let parent = self.instance_values(id);
            let (graph, creator, parameters) = match played {
                // The played instance's creator (#27) is the entity whose
                // state plays it: `26DB` reads the payload's distance as
                // `#27.id(479)`.
                Some(graph) => {
                    let Some(parameters) = self.play_parameters(id, node) else {
                        return Ok(());
                    };
                    (graph, source.entity, Some(parameters))
                }
                None => (
                    self.machine.snapshot.instances[&id].graph,
                    self.source_of(id),
                    None,
                ),
            };
            for &target in came.iter().chain(&unopened) {
                taken &= self.world.change_scope(
                    site,
                    ScopeChange::Open {
                        source,
                        target,
                        graph,
                        scope: scope_id(node),
                        creator,
                        keep,
                        reuse,
                        parent: parent.clone(),
                        played: parameters.clone(),
                    },
                );
            }
            opened.extend(came.iter().map(|&target| ScopeHit {
                target,
                instance: None,
                keep,
                value: None,
            }));
        }
        if !taken {
            self.assume(site, &node.class, "no scopes (the world opens none)");
        }
        let state = self.state_mut(key).expect("active scope state");
        state.scopes.retain(|hit| !left.contains(&hit.target));
        state.scopes.extend(opened);
        Ok(())
    }

    /// The graph `STU_CB4905E9` plays now: `m_script` (a graph asset, or a
    /// variable holding one) when `m_69F4B52C` names none. `Some(None)`: no
    /// graph; None: another form, or a value that is no graph.
    fn played_graph(&mut self, key: StateKey, node: &Node) -> Option<Option<u32>> {
        if !plays_script(node) {
            return None;
        }
        let graph = |guid: u64| (guid >> 48 == 0x0580).then_some(guid as u32);
        if let Some(guid) = node.field("m_script.m_graph").and_then(asset_guid) {
            return graph(guid).map(Some);
        }
        let script = field(&node.fields, "m_script");
        match self.evaluate(key.instance, node, script, Some(key))? {
            Value::Null => Some(None),
            Value::Asset(guid) => graph(guid).map(Some),
            _ => None,
        }
    }

    /// A value scope's tick (`STU_77E13026`, module notes): the items of
    /// `m_814F8EC3` that came get a scope instance here, those that left
    /// lose theirs.
    fn refresh_value_scopes(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let listed = match field(&node.fields, "m_814F8EC3") {
            Json::Null => Vec::new(),
            config => match self.evaluate_value(id, node, config, Some(key)) {
                None => return Ok(()),
                Some(Value::Array(items)) => items
                    .into_iter()
                    .filter(|item| *item != Value::Null)
                    .collect(),
                Some(Value::Null) => Vec::new(),
                Some(item) => vec![item],
            },
        };
        let state = self.state(key).expect("active scope state");
        // An item stays while the list holds as many equal values.
        let mut unmatched = listed.clone();
        let mut left = Vec::new();
        for (index, hit) in state.scopes.iter().enumerate() {
            match unmatched
                .iter()
                .position(|item| Some(item) == hit.value.as_ref())
            {
                Some(found) => {
                    unmatched.remove(found);
                }
                None => left.push(index),
            }
        }
        let mut kept: Vec<&Value> = state
            .scopes
            .iter()
            .filter_map(|hit| hit.value.as_ref())
            .collect();
        let mut came = Vec::new();
        for (index, item) in listed.iter().enumerate() {
            match kept.iter().position(|value| *value == item) {
                Some(found) => {
                    kept.remove(found);
                }
                None => came.push((index, item.clone())),
            }
        }
        if left.is_empty() && came.is_empty() {
            return Ok(());
        }
        let gone: Vec<ScopeHit> = left
            .iter()
            .rev()
            .map(|&index| {
                self.state_mut(key)
                    .expect("active scope state")
                    .scopes
                    .remove(index)
            })
            .collect();
        for hit in gone {
            self.close_value_scope(&hit)?;
        }
        if came.is_empty() {
            return Ok(());
        }
        let site = self.site(id, node);
        let keep = match field(&node.fields, "m_4B7DF406") {
            Json::Null => false,
            config => self
                .evaluate(id, node, config, Some(key))
                .is_some_and(|value| truthy(&value)),
        };
        let lvalue = |name: &str| match expr::lvalue(field(&node.fields, name)) {
            Ok(variable) => Ok(variable),
            Err(error) => Err(Issue::Expression(error)),
        };
        let (item_var, index_var) = match (lvalue("m_3DC6FAAA"), lvalue("m_339A63DB")) {
            (Ok(item), Ok(index)) => (item, index),
            (Err(issue), _) | (_, Err(issue)) => {
                self.report(site, issue);
                return Ok(());
            }
        };
        let link = ScopeLink {
            id: scope_id(node),
            source: self.scope_source(key),
            parent: self.instance_values(id),
        };
        let graph = self.machine.snapshot.instances[&id].graph;
        let creator = self.source_of(id);
        let entity = self.machine.snapshot.entity;
        for (index, item) in came {
            let scope =
                u16::try_from(self.machine.next_id).map_err(|_| InitFault::InstanceIdExhausted)?;
            if !self.create_scoped(scope, graph, Some(link.clone()), creator)? {
                return Ok(());
            }
            if let Some(variable) = item_var {
                self.write(source_key(scope, variable), item.clone())?;
            }
            if let Some(variable) = index_var {
                self.write(source_key(scope, variable), Value::Integer(index as i64))?;
            }
            self.post(scope, Event::Begin(key.raw))?;
            self.state_mut(key)
                .expect("active scope state")
                .scopes
                .push(ScopeHit {
                    target: entity,
                    instance: Some(scope),
                    keep,
                    value: Some(item),
                });
        }
        Ok(())
    }

    /// A value scope's item left, or its state ended: its instance goes
    /// unless the state keeps it.
    fn close_value_scope(&mut self, hit: &ScopeHit) -> Result<(), InitFault> {
        match hit.instance {
            Some(instance)
                if !hit.keep && self.machine.snapshot.instances.contains_key(&instance) =>
            {
                self.post(instance, Event::ExitInstance)
            }
            _ => Ok(()),
        }
    }

    /// The scope state ended, or its instance went: each target loses its
    /// scope (kept instances stay; `0x7FF789C55C70`).
    pub(super) fn end_scopes(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        let hits = std::mem::take(&mut state.scopes);
        if hits.is_empty() {
            return Ok(());
        }
        let source = self.scope_source(key);
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("scope state");
        let site = self.site(key.instance, node);
        for hit in hits {
            if hit.value.is_some() {
                self.close_value_scope(&hit)?;
            } else {
                self.world.change_scope(
                    site,
                    ScopeChange::Close {
                        source,
                        target: hit.target,
                    },
                );
            }
        }
        Ok(())
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
