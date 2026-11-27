//! Variable stack links and movement modifiers.
//!
//! Client research (1.74): Stack OnBegin 0x7FF78AAA5FF0 pushes an entry
//! `{state, priority}` on m_out_Var; the last entry is the top (its state
//! follows m_topSubgraphPlug, every other m_underSubgraphPlug, only on a
//! change), and the variable reads the top's m_value. A modifier that reads
//! its own output variable uses the value below its entry (Doomfist 0D06
//! state 77 adds 0.5 to entity var 5A90). The push (0x7FF78AA82F90, the sort
//! of PS4 sub_172E790) keeps entries ascending by priority: a new entry goes
//! after every lower priority, above equal ones when its flag is set, below
//! them otherwise; pushing the same state again replaces its entry.

use super::*;

impl<'graph, 'world, 'machine, W: World + ?Sized> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn insert_link(
        &mut self,
        target: VarKey,
        link: VariableLink,
        depth: usize,
    ) -> Result<(), InitFault> {
        let old = self.machine.value(target, None);
        let snapshot = &self.machine.snapshot;
        let present = match target {
            VarKey::Entity(id) => snapshot.variables.contains_key(&id),
            VarKey::Instance(instance, id) => {
                snapshot.instances[&instance].variables.contains_key(&id)
            }
        };
        if !present {
            match target {
                VarKey::Entity(id) => self.machine.snapshot.variables.insert(id, Value::Null),
                VarKey::Instance(instance, id) => self.machine.snapshot.instances[&instance]
                    .variables
                    .insert(id, Value::Null),
            };
        }
        let entries = self.machine.links.entry(target).or_default();
        entries.retain(|entry| entry.state != link.state);
        let at = entries
            .iter()
            .position(|entry| {
                entry.priority.level > link.priority.level
                    || (entry.priority.level == link.priority.level && !link.priority.above_equals)
            })
            .unwrap_or(entries.len());
        entries.insert(at, link);
        self.notify_value(target, &old, None)?;
        self.sync_stack(target, depth)
    }

    pub(super) fn sync_stack(&mut self, variable: VarKey, depth: usize) -> Result<(), InitFault> {
        if depth >= MAX_PLUG_DEPTH {
            return Err(InitFault::PlugDepth);
        }
        let Some(entries) = self.machine.links.get(&variable) else {
            return Ok(());
        };
        let states: Vec<_> = entries
            .iter()
            .filter(|entry| entry.stack)
            .map(|entry| entry.state)
            .collect();
        for key in states {
            self.spend("stack position change")?;
            let Some(state) = self
                .state(key)
                .filter(|state| state.active && !state.ending)
            else {
                continue;
            };
            let top = self
                .machine
                .links
                .get(&variable)
                .and_then(|entries| entries.last());
            if let StateKind::Ability { flags } = state.kind {
                if flags & ABILITY_PUSHED != 0 && state.ability_stack == Some(variable) {
                    let node = self
                        .graph(key.instance)
                        .state(usize::from(key.raw))
                        .expect("ability state");
                    if top.is_some_and(|entry| {
                        entry.state == key && matches!(entry.output, LinkValue::AbilityStack)
                    }) {
                        if flags & ABILITY_RUNNING == 0 {
                            self.start_ability(key, node, depth + 1)?;
                        }
                    } else {
                        self.stop_ability(key, node, depth + 1)?;
                    }
                }
                continue;
            }
            if state.linked_variable != Some(variable) {
                continue;
            }
            let top = top.is_some_and(|entry| entry.state == key);
            let kind = StateKind::Stack { top, under: !top };
            if state.kind == kind {
                continue;
            }
            self.state_mut(key).expect("active stack").kind = kind;
            let node = self
                .graph(key.instance)
                .state(usize::from(key.raw))
                .expect("stack state");
            let (old, next) = if top {
                ("m_underSubgraphPlug", "m_topSubgraphPlug")
            } else {
                ("m_topSubgraphPlug", "m_underSubgraphPlug")
            };
            self.exit_subgraph(key.instance, node, old, depth + 1)?;
            self.follow(key.instance, node, next, Some(key), depth)?;
        }
        Ok(())
    }

    pub(super) fn unlink_single_output(
        &mut self,
        key: StateKey,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(target) = self
            .state_mut(key)
            .and_then(|state| state.linked_variable.take())
        else {
            return Ok(());
        };
        self.remove_link(key, target, None, depth)
    }

    pub(super) fn remove_link(
        &mut self,
        key: StateKey,
        target: VarKey,
        bake: Option<Value>,
        depth: usize,
    ) -> Result<(), InitFault> {
        self.unlink_output(key, target, bake)?;
        self.sync_stack(target, depth)
    }

    /// A state's end releases the outputs still bound to it
    /// ([`LinkValue::Output`]): each variable keeps its own value.
    pub(super) fn unlink_outputs(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        let targets: Vec<_> = self
            .machine
            .links
            .iter()
            .filter(|(_, entries)| {
                entries.iter().any(|entry| {
                    entry.state == key && matches!(entry.output, LinkValue::Output { .. })
                })
            })
            .map(|(&target, _)| target)
            .collect();
        for target in targets {
            self.remove_link(key, target, None, depth)?;
        }
        Ok(())
    }

    pub(super) fn unlink_output(
        &mut self,
        key: StateKey,
        target: VarKey,
        bake: Option<Value>,
    ) -> Result<(), InitFault> {
        let old = self.machine.value(target, None);
        if let Some(value) = bake {
            self.set_base(target, value);
        }
        if let Some(entries) = self.machine.links.get_mut(&target) {
            entries.retain(|entry| entry.state != key);
            if entries.is_empty() {
                self.machine.links.remove(&target);
            }
        }
        self.notify_value(target, &old, None)
    }

    pub(super) fn update_stack(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(target) = self.state(key).and_then(|state| state.linked_variable) else {
            return Ok(());
        };
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("stack state");
        let value = field(&node.fields, "m_value");
        if value.is_null() {
            return Ok(());
        }
        let Some(value) = self.evaluate(id, node, value, Some(key)) else {
            return Ok(());
        };
        let old = self.machine.value(target, None);
        if let Some(LinkValue::Fixed(current)) = self
            .machine
            .links
            .get_mut(&target)
            .and_then(|entries| entries.iter_mut().find(|entry| entry.state == key))
            .map(|entry| &mut entry.output)
        {
            if !same(current, &value) {
                *current = value;
                self.notify_value(target, &old, Some(key))?;
            }
        }
        Ok(())
    }

    pub(super) fn collect_movement_modifiers(
        &mut self,
    ) -> Result<BTreeMap<ModifierKey, Modifier>, InitFault> {
        let states: Vec<_> = self
            .machine
            .snapshot
            .instances
            .iter()
            .flat_map(|(&id, instance)| {
                instance.states.iter().filter_map(move |(&raw, state)| {
                    (state.active && state.kind == StateKind::MovementMod)
                        .then_some(StateKey { instance: id, raw })
                })
            })
            .collect();
        let mut modifiers = BTreeMap::new();
        for key in states {
            let id = key.instance;
            let node = self
                .graph(id)
                .state(usize::from(key.raw))
                .expect("movement modifier state");
            for (slot, config) in array_field(&node.fields, "m_BC5E91CF").iter().enumerate() {
                self.spend("movement modifier")?;
                let shape = config.get("$").and_then(Json::as_str);
                // The vector form of set (`STU_09F9B1FE`) on channel 2 turns
                // the direction forward throttle moves in: the mover keeps
                // it in its list at +0x7D0 as type 2 and reads it after
                // mods 17..19 (0x7FF789B43A94..0x7FF789B43D50), flat and
                // unit, so only its heading counts and it goes to the mover
                // as one ([`MOVE_HEADING`]). Rocket Punch's dash (`0D06`
                // st42, the release facing #2083) is its only use in 1.74;
                // the add and multiply forms (`STU_56F54BF7`,
                // `STU_09441550`) and other channels are not modelled.
                if shape == Some("STU_09F9B1FE")
                    && config.get("m_CFEC5F76").and_then(Json::as_u64) == Some(2)
                {
                    let vector = match self.evaluate(id, node, field(config, "m_9B7A63EA"), None) {
                        Some(Value::Vector(vector)) => vector,
                        Some(_) => {
                            self.report(
                                self.site(id, node),
                                Issue::UnsupportedField("movement direction"),
                            );
                            continue;
                        }
                        None => continue,
                    };
                    let Some(priority) =
                        self.movement_number(id, node, field(config, "m_priority"), 0.0)
                    else {
                        continue;
                    };
                    modifiers.insert(
                        ModifierKey {
                            instance: id,
                            state: key.raw,
                            slot,
                        },
                        Modifier {
                            index: MOVE_HEADING,
                            operation: Operation::Set,
                            value: heading(vector),
                            priority,
                        },
                    );
                    continue;
                }
                // The clamp (STU_0A019882, client kind 5) keeps its bounds in
                // m_8AD739CE (lower) and m_8213EB22 (upper); [ASSUMPTION] an
                // unset bound is 0.
                let (mut operation, value_field, default) = match shape {
                    Some("STU_4CB0950F") => (Operation::Add, "m_9B7A63EA", 0.0),
                    Some("STU_D8CC6B10") => (Operation::Multiply, "m_9B7A63EA", 1.0),
                    Some("STU_56DEE1BB") => (Operation::Set, "m_9B7A63EA", 0.0),
                    Some("STU_C61711C8") => (Operation::Minimum, "m_9B7A63EA", f64::from(f32::MAX)),
                    Some("STU_F197CFBF") => (
                        Operation::Maximum,
                        "m_9B7A63EA",
                        f64::from(f32::MIN_POSITIVE),
                    ),
                    Some("STU_0A019882") => (Operation::Clamp { upper: 0.0 }, "m_8AD739CE", 0.0),
                    _ => {
                        self.report(
                            self.site(id, node),
                            Issue::UnsupportedField("movement modifier operation"),
                        );
                        continue;
                    }
                };
                let Some(index) = config
                    .get("m_A9561CA0")
                    .and_then(Json::as_u64)
                    .and_then(|value| u8::try_from(value).ok())
                else {
                    self.report(
                        self.site(id, node),
                        Issue::UnsupportedField("movement modifier index"),
                    );
                    continue;
                };
                let Some(value) =
                    self.movement_number(id, node, field(config, value_field), default)
                else {
                    continue;
                };
                if let Operation::Clamp { upper } = &mut operation {
                    let Some(bound) =
                        self.movement_number(id, node, field(config, "m_8213EB22"), 0.0)
                    else {
                        continue;
                    };
                    *upper = bound;
                }
                let Some(priority) =
                    self.movement_number(id, node, field(config, "m_priority"), 0.0)
                else {
                    continue;
                };
                modifiers.insert(
                    ModifierKey {
                        instance: id,
                        state: key.raw,
                        slot,
                    },
                    Modifier {
                        index,
                        operation,
                        value,
                        priority,
                    },
                );
            }
        }
        Ok(modifiers)
    }

    /// A mover number as the client reads it (`0x7FF789806350`, which
    /// converts with `0x7FF789E74C80`): the default when the field is
    /// absent; an array gives its first element; a present but empty value
    /// (an unset variable, value type 0xFF) gives 0.0. `023A` st1 multiplies
    /// mods 17, 18, 19 and 21 by the unset `#2101` during a one-shot emote,
    /// so the emote locks the throttles (EmoteWalk, refs `Emotes.md`).
    /// [ASSUMPTION] Another value (an asset, a vector, an empty array) gives
    /// the default: the client returns whatever its `xmm1` holds then.
    pub(super) fn movement_number(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        default: f64,
    ) -> Option<f64> {
        if config.is_null() {
            return Some(default);
        }
        let value = self.evaluate(id, node, config, None)?;
        Some(match expr::first_scalar(&value) {
            Some(Value::Null) => 0.0,
            Some(scalar @ (Value::Bool(_) | Value::Integer(_) | Value::Float(_))) => {
                f64::from(expr::to_float(scalar) as f32)
            }
            _ => default,
        })
    }
}
