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
            snapshot: Snapshot {
                entity,
                variables,
                instances: IndexMap::new(),
                pending_timers: IndexMap::new(),
                bindings: IndexMap::new(),
                resolved_bindings: IndexMap::new(),
                diagnostics: Vec::new(),
                assumptions: Vec::new(),
                frame_actions: Vec::new(),
                now_ms: frame_start(frame),
            },
            queues: Timeline::new(MAX_PENDING_EVENTS),
            watchers: BTreeMap::new(),
            foreign_watchers: BTreeMap::new(),
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
            hero_selects: BTreeMap::new(),
            motions: Vec::new(),
            applied_auras: BTreeMap::new(),
        })
    }

    pub(super) fn capture_outputs(&mut self) {
        self.snapshot.now_ms = self.now_ms;
        self.snapshot.pending_timers.clear();
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
                self.snapshot.pending_timers.insert(id, timers);
            }
        }
        self.snapshot.bindings.clear();
        self.snapshot.resolved_bindings.clear();
        for (&variable, entries) in &self.links {
            let key = match variable {
                VarKey::Entity(id) => (0, id),
                VarKey::Instance(instance, id) => (instance, id),
            };
            let Some(top) = entries.last() else { continue };
            // A chase's link is a binding of its variable on the client too
            // (its begin binds the variable, `0x7FF78AAAF430`); a frame that
            // lists the variable without it unbinds the chase there
            // (`0x7FF78AAB78A0` → `0x7FF78AAB7420` takes the frame's list).
            let bindings: Vec<_> = entries
                .iter()
                .filter(|entry| entry.stack || matches!(entry.output, LinkValue::Chase))
                .map(|entry| StackBinding {
                    instance: entry.state.instance,
                    raw_state: entry.state.raw,
                    priority: entry.priority,
                    slot: u8::from(matches!(entry.output, LinkValue::AbilityStack)),
                })
                .collect();
            if !bindings.is_empty() {
                self.snapshot.bindings.insert(key, bindings);
            }
            let resolved = self.link_value(top);
            self.snapshot.resolved_bindings.insert(key, resolved);
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
            entries.extend(
                graph
                    .nodes
                    .iter()
                    .filter(|node| {
                        !node.client_only
                            && node
                                .field("m_gameMessage.m_gameMessage")
                                .and_then(asset_guid)
                                == Some(message)
                            && (is_message_entry(node)
                                || (node.is_state()
                                    && node.state.is_some_and(|raw| {
                                        item.states.get(&raw).is_some_and(|state| state.active)
                                    })))
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
