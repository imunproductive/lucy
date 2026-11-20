//! Variable storage, entry queues and stack links shared by every instance.
//!
//! Client research (1.74): a variable is a base value plus links, the states
//! that override it while active, in order; the last active one wins
//! (resolver 0x7FF78AAB70B0). A Stack's entries are its links. Every variable
//! a state reads while it evaluates registers the state as a watcher; a change
//! sends it an event. A config var the world answers is no variable, so a
//! state that reads it re-checks every update (the client's poll list,
//! 0x7FF78AAA5C20). A plain assignment keeps the value's own type
//! (0x7FF78AAB5650). The runtime state of an m_states index is made on first
//! use (0x7FF78AAA1460).

use super::*;

impl Machine {
    pub(super) fn new(
        entity: u32,
        variables: Variables,
        frame: u32,
        max_work: usize,
    ) -> Result<Self, InitFault> {
        if max_work == 0 {
            return Err(InitFault::InvalidBudget);
        }
        Ok(Self {
            snapshot: TrackedSnapshot::new(Snapshot {
                entity,
                variables,
                instances: Instances::default(),
                pending_timers: IndexMap::new(),
                bindings: IndexMap::new(),
                resolved_bindings: IndexMap::new(),
                diagnostics: Vec::new(),
                assumptions: Vec::new(),
                frame_actions: Vec::new(),
                now_ms: frame_start(frame),
                revision: Revision::first(),
            }),
            queues: Timeline::new(MAX_PENDING_EVENTS),
            watchers: BTreeMap::new(),
            foreign_watchers: BTreeMap::new(),
            native_watchers: BTreeMap::new(),
            ticking: BTreeMap::new(),
            links: BTreeMap::new(),
            volleys: BTreeMap::new(),
            shots: Vec::new(),
            ability_presses: BTreeMap::new(),
            joins: BTreeMap::new(),
            next_id: 1,
            work_left: max_work,
            max_work,
            frame,
            now_ms: frame_start(frame),
            random_state: Cell::new(u64::from(entity) << 32 | u64::from(frame)),
            command_buttons: 0,
            held_buttons: Vec::new(),
            owner_dead: false,
            deferred: BTreeMap::new(),
            disabled: BTreeMap::new(),
            movements: BTreeMap::new(),
            grapples: BTreeMap::new(),
            ai_moves: BTreeMap::new(),
            hero_selects: BTreeMap::new(),
            motions: Vec::new(),
            applied_auras: BTreeMap::new(),
            scopes: BTreeMap::new(),
        })
    }

    /// Take the clock, pending timers and links into the snapshot. Only
    /// what changed is written, so a snapshot nothing else changed keeps
    /// its revision ([`Snapshot::revision`]).
    pub(super) fn capture_outputs(&mut self) {
        self.snapshot.set_now_ms(self.now_ms);
        let mut pending_timers = IndexMap::new();
        for &id in self.snapshot.instances.keys() {
            let timers: Vec<_> = self
                .queues
                .iter(id)
                .filter_map(|(due_ms, event)| {
                    let (raw_state, finish, parameter) = match event {
                        Event::Timer(raw, param) => (*raw, false, *param),
                        Event::Finish(raw) => (*raw, true, 0),
                        _ => return None,
                    };
                    Some(PendingTimer {
                        due_ms,
                        raw_state,
                        finish,
                        parameter,
                    })
                })
                .collect();
            if !timers.is_empty() {
                pending_timers.insert(id, timers);
            }
        }
        let mut bindings = IndexMap::new();
        let mut resolved_bindings = IndexMap::new();
        for (&variable, entries) in &self.links {
            let key = match variable {
                VarKey::Entity(id) => (0, id),
                VarKey::Instance(instance, id) => (instance, id),
            };
            let Some(top) = entries.last() else { continue };
            // A chase's link is a binding of its variable on the client too
            // (its begin binds the variable, `0x7FF78AAAF430`), as are a
            // state's bound outputs and an Ability's cooldown (slot 0, its
            // restart `0x7FF789BBB110`; the client's output
            // `0x7FF789B94D00` works the remaining time out from the
            // state's timing block); a frame that lists the variable
            // without it unbinds the state there (`0x7FF78AAB78A0` →
            // `0x7FF78AAB7420` takes the frame's list).
            let stacked: Vec<_> = entries
                .iter()
                .filter(|entry| {
                    entry.stack
                        || matches!(
                            entry.output,
                            LinkValue::Chase | LinkValue::Cooldown | LinkValue::Output { .. }
                        )
                })
                .map(|entry| StackBinding {
                    instance: entry.state.instance,
                    raw_state: entry.state.raw,
                    priority: entry.priority,
                    slot: match entry.output {
                        LinkValue::AbilityStack => 1,
                        LinkValue::Output { slot, .. } => slot,
                        _ => 0,
                    },
                })
                .collect();
            if !stacked.is_empty() {
                bindings.insert(key, stacked);
            }
            resolved_bindings.insert(key, self.link_value(top));
        }
        // A changed output moves its instance's revision on too (owner 0,
        // the entity, has none).
        let mut changed = Vec::new();
        if !same_entries(
            &self.snapshot.pending_timers,
            &pending_timers,
            PartialEq::eq,
        ) {
            changed_owners(
                &self.snapshot.pending_timers,
                &pending_timers,
                |&id| id,
                PartialEq::eq,
                &mut changed,
            );
            self.snapshot.pending_timers = pending_timers;
        }
        if !same_entries(&self.snapshot.bindings, &bindings, PartialEq::eq) {
            changed_owners(
                &self.snapshot.bindings,
                &bindings,
                |&(owner, _)| owner,
                PartialEq::eq,
                &mut changed,
            );
            self.snapshot.bindings = bindings;
        }
        if !same_entries(
            &self.snapshot.resolved_bindings,
            &resolved_bindings,
            Value::identical,
        ) {
            changed_owners(
                &self.snapshot.resolved_bindings,
                &resolved_bindings,
                |&(owner, _)| owner,
                Value::identical,
                &mut changed,
            );
            self.snapshot.resolved_bindings = resolved_bindings;
        }
        for owner in changed {
            if owner != 0 {
                self.snapshot.instances.touch(owner);
            }
        }
    }

    pub(super) fn queue_entries(
        &mut self,
        graphs: &GraphStore,
        recipient: Option<u16>,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        let mut entries = Vec::new();
        for (&id, item) in &self.snapshot.instances {
            if recipient.is_some_and(|recipient| recipient != id) {
                continue;
            }
            let graph = graphs.graph(item.graph).expect("registered graph");
            // An entry runs in the instances of its scope (`vm/scopes.rs`).
            let scope = item.scope.as_ref().map_or(0, |scope| scope.id);
            entries.extend(
                graph
                    .message_nodes(message)
                    .filter(|node| {
                        (is_message_entry(node) && super::scopes::entry_scope(node) == scope)
                            || (node.is_state()
                                && node.state.is_some_and(|raw| {
                                    item.states.get(&raw).is_some_and(|state| state.active)
                                }))
                    })
                    .map(|node| (id, node.pos)),
            );
        }
        self.queues.reserve(entries.len())?;
        let parameters = Arc::new(parameters);
        for &(instance, position) in &entries {
            self.queues.push(
                instance,
                self.now_ms,
                Event::Message {
                    position,
                    sender,
                    parameters: Arc::clone(&parameters),
                },
            )?;
        }
        Ok(entries.len())
    }

    pub(super) fn value(&self, key: VarKey, below: Option<StateKey>) -> Value {
        self.links
            .get(&key)
            .and_then(|entries| {
                entries
                    .iter()
                    .take_while(|entry| Some(entry.state) != below)
                    .last()
            })
            .map_or_else(
                || stored(&self.snapshot, key),
                |entry| self.link_value(entry),
            )
    }

    pub(super) fn link_value(&self, entry: &VariableLink) -> Value {
        match &entry.output {
            LinkValue::Fixed(value) => value.clone(),
            LinkValue::Volley(slot) => self.volleys[&entry.state].output(*slot, self.now_ms),
            LinkValue::Chase => self.snapshot.instances[&entry.state.instance].states
                [&entry.state.raw]
                .chase
                .as_ref()
                .expect("linked chase state")
                .value_at(self.now_ms),
            LinkValue::Cooldown => Value::Float(
                self.snapshot.instances[&entry.state.instance].states[&entry.state.raw]
                    .ability_cooldown
                    .expect("linked cooldown state")
                    .remaining(self.now_ms),
            ),
            LinkValue::AbilityStack => Value::Bool(true),
            LinkValue::Output { value, .. } => value.clone(),
        }
    }

    pub(super) fn allows_stack(
        &self,
        key: VarKey,
        priority: StackPriority,
        exclude: Option<StateKey>,
    ) -> bool {
        self.links.get(&key).is_none_or(|entries| {
            entries
                .iter()
                .filter(|entry| Some(entry.state) != exclude)
                .all(|entry| {
                    entry.priority.level < priority.level
                        || (entry.priority.level == priority.level && priority.above_equals)
                })
        })
    }
}

/// The same keys with the same values, in the same order.
fn same_entries<K: PartialEq, V>(
    a: &IndexMap<K, V>,
    b: &IndexMap<K, V>,
    same: impl Fn(&V, &V) -> bool,
) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|((ak, av), (bk, bv))| ak == bk && same(av, bv))
}

/// The owners of the entries that differ between `a` and `b` (either
/// order), into `out`.
fn changed_owners<K: Eq + std::hash::Hash, V>(
    a: &IndexMap<K, V>,
    b: &IndexMap<K, V>,
    owner: impl Fn(&K) -> u16,
    same: impl Fn(&V, &V) -> bool,
    out: &mut Vec<u16>,
) {
    for (key, value) in b {
        if a.get(key).is_none_or(|was| !same(was, value)) {
            out.push(owner(key));
        }
    }
    out.extend(a.keys().filter(|key| !b.contains_key(*key)).map(owner));
}
