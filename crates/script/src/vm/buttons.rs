//! Logical buttons and their buffered presses.
//!
//! Client research (1.74): LogicalButton edge handler 0x7FF78AAAC570, OnEnter
//! 0x7FF78AAACA80. Down: m_onGoingDownPlug then the down subgraph
//! (m_80AF45FB); up: m_onComingUpPlug then the up subgraph (m_1C9EF058). A
//! press while inactive is kept for the next activation (m_246438AD, the
//! early press). The edge handler drops a press while the owner is dead
//! (0x7FF78AAAC5FC; the client sees its owner dead through component 51,
//! 0x7FF789ADBF00). DisableLogicalButton blocks the entity's LogicalButtons
//! for its button while active (OnBegin 0x7FF78AA82530).

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn queue_buttons(&mut self, command: Command) -> Result<(), InitFault> {
        let edges: Vec<_> = button_edges(
            self.machine.command_buttons,
            command.buttons,
            command.action,
        )
        .collect();
        self.machine
            .queues
            .reserve(edges.len() * self.machine.snapshot.instances.len())?;
        for &id in self.machine.snapshot.instances.keys() {
            for &edge in &edges {
                self.machine
                    .queues
                    .push(id, self.machine.now_ms, Event::Button(edge))?;
            }
        }
        self.machine.command_buttons = command.buttons;
        Ok(())
    }

    pub(super) fn button_edge(&mut self, id: u16, edge: ButtonEdge) -> Result<(), InitFault> {
        if edge.pressed {
            if !self.machine.held_buttons.contains(&edge.button) {
                self.machine.held_buttons.push(edge.button);
            }
        } else {
            self.machine
                .held_buttons
                .retain(|&button| button != edge.button);
        }
        let graph = self.graph(id);
        for raw in raw_indices(array_field(&graph.fields, "m_680A2CB2")) {
            let Some(node) = graph.state(usize::from(raw)) else {
                continue;
            };
            if node.client_only {
                continue;
            }
            let key = StateKey { instance: id, raw };
            if node.class == "STUStatescriptStateAbility" {
                self.spend("ability button edge")?;
                self.ability_edge(key, node, edge)?;
                continue;
            }
            if node.class != "STUStatescriptStateLogicalButton" {
                continue;
            }
            self.spend("logical button edge")?;
            if self.logical_button_id(key, node) != Some(edge.button) {
                continue;
            }
            if edge.pressed
                && self.machine.owner_dead
                && !self
                    .evaluate(id, node, field(&node.fields, "m_5AE05408"), None)
                    .is_some_and(|value| truthy(&value))
            {
                continue;
            }
            if self.button_blocked(key, node) {
                continue;
            }
            if edge.pressed {
                let state = self.machine.snapshot.instances[&id]
                    .states
                    .entry(raw)
                    .or_insert_with(|| State::dormant(StateKind::LogicalButton { down: false }));
                state.buffered_press = Some(BufferedPress {
                    time_ms: self.machine.now_ms,
                    activation: state.activation.wrapping_add(u16::from(!state.active)),
                });
            }
            if self.state(key).is_some_and(|state| state.active) {
                self.set_button_down(key, node, edge.pressed, 0)?;
            }
        }
        Ok(())
    }

    pub(super) fn logical_button_id(&mut self, key: StateKey, node: &Node) -> Option<u8> {
        let value = self.evaluate(
            key.instance,
            node,
            field(&node.fields, "m_logicalButton"),
            None,
        )?;
        u8::try_from(expr::to_int(&value, 0)).ok()
    }

    pub(super) fn button_held(&mut self, key: StateKey, node: &Node) -> bool {
        self.logical_button_id(key, node)
            .is_some_and(|button| self.machine.held_buttons.contains(&button))
    }

    /// `LogicalButton.blocked` in `ow174/game/script/nodes.py`: an active
    /// `DisableLogicalButton` blocks the button unless the state ignores it
    /// (`m_53C4B6A6`).
    pub(super) fn button_blocked(&mut self, key: StateKey, node: &Node) -> bool {
        let Some(button) = self.logical_button_id(key, node) else {
            return false;
        };
        if !self.disabled(i64::from(button)) {
            return false;
        }
        let ignore = field(&node.fields, "m_53C4B6A6");
        ignore.is_null()
            || !self
                .evaluate(key.instance, node, ignore, None)
                .is_some_and(|value| truthy(&value))
    }

    pub(super) fn disabled(&self, button: i64) -> bool {
        self.machine
            .disabled
            .get(&button)
            .is_some_and(|states| !states.is_empty())
    }

    /// `DisableLogicalButton.on_begin`: block the button and bring the
    /// entity's active buttons with that id up.
    pub(super) fn begin_disable_button(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(button) = self.evaluate(
            key.instance,
            node,
            field(&node.fields, "m_logicalButton"),
            None,
        ) else {
            return Ok(());
        };
        let button = expr::to_int(&button, 0);
        self.state_mut(key).expect("active button block").kind =
            StateKind::DisableButton { button };
        self.machine.disabled.entry(button).or_default().push(key);
        self.resync_buttons(button, depth)
    }

    /// `DisableLogicalButton.on_end`.
    pub(super) fn end_disable_button(
        &mut self,
        key: StateKey,
        button: i64,
        depth: usize,
    ) -> Result<(), InitFault> {
        if let Some(states) = self.machine.disabled.get_mut(&button) {
            states.retain(|state| *state != key);
            if states.is_empty() {
                self.machine.disabled.remove(&button);
            }
        }
        self.resync_buttons(button, depth)
    }

    /// `DisableLogicalButton.resync`: every active button with this id goes
    /// down or up to match whether it is held and not blocked.
    fn resync_buttons(&mut self, button: i64, depth: usize) -> Result<(), InitFault> {
        let buttons: Vec<_> = self
            .machine
            .snapshot
            .instances
            .iter()
            .flat_map(|(&instance, item)| {
                item.states
                    .iter()
                    .filter(|(_, state)| {
                        state.active && matches!(state.kind, StateKind::LogicalButton { .. })
                    })
                    .map(move |(&raw, _)| StateKey { instance, raw })
            })
            .collect();
        for key in buttons {
            let Some(StateKind::LogicalButton { down }) = self
                .state(key)
                .filter(|state| state.active)
                .map(|state| state.kind)
            else {
                continue;
            };
            let node = self
                .graph(key.instance)
                .state(usize::from(key.raw))
                .expect("logical button");
            if self
                .logical_button_id(key, node)
                .is_none_or(|id| i64::from(id) != button)
            {
                continue;
            }
            let next = self.button_held(key, node) && !self.button_blocked(key, node);
            if next != down {
                self.set_button_down(key, node, next, depth + 1)?;
            }
        }
        Ok(())
    }

    /// `AIFireWeapon.on_enter`: a live owner's self-targeted, enabled state
    /// with an integer slot presses the world's button for that slot.
    pub(super) fn begin_ai_fire(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        if self.machine.owner_dead || !field(&node.fields, "m_234135C3").is_null() {
            return Ok(());
        }
        let mut values = Vec::with_capacity(3);
        for name in ["m_14EDDDA7", "m_20BCEE62", "m_A60150E2"] {
            let Some(value) = self.evaluate(id, node, field(&node.fields, name), None) else {
                return Ok(());
            };
            values.push(value);
        }
        let [
            Value::Entity(target),
            Value::Bool(true),
            Value::Integer(slot),
        ] = values.as_slice()
        else {
            return Ok(());
        };
        if *target != self.machine.snapshot.entity {
            return Ok(());
        }
        let Some(button) = self.world.ai_weapon_button(*slot) else {
            return Ok(());
        };
        self.state_mut(key).expect("active AI fire").kind = StateKind::AiFire {
            button: Some(button),
        };
        self.post_edge(ButtonEdge {
            button,
            pressed: true,
        })
    }

    /// A button edge at the clock on every instance (`Component.edges`).
    pub(super) fn post_edge(&mut self, edge: ButtonEdge) -> Result<(), InitFault> {
        let instances: Vec<_> = self.machine.snapshot.instances.keys().copied().collect();
        self.machine.queues.reserve(instances.len())?;
        for id in instances {
            self.post(id, Event::Button(edge))?;
        }
        Ok(())
    }

    pub(super) fn enter_button(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let blocked = self.button_blocked(key, node);
        let state = self.state(key).expect("active logical button");
        let buffer = state
            .buffered_press
            .filter(|press| press.activation == state.activation);
        let duration = field(&node.fields, "m_246438AD");
        let early = if let Some(press) = buffer.filter(|_| !duration.is_null()) {
            let waited = ((self.machine.now_ms - press.time_ms) as f64 / 1000.0) as f32;
            self.evaluate(key.instance, node, duration, None)
                .is_some_and(|value| f64::from(waited) <= expr::to_float(&value))
                && !blocked
        } else {
            false
        };
        if early {
            self.follow(key.instance, node, "m_onGoingDownPlug", Some(key), depth)?;
        }
        if !self.state(key).is_some_and(|state| state.active) {
            return Ok(());
        }
        let held = !blocked && self.button_held(key, node);
        let down = held || early;
        self.state_mut(key).expect("active logical button").kind =
            StateKind::LogicalButton { down };
        let path = if down { "m_80AF45FB" } else { "m_1C9EF058" };
        self.follow(key.instance, node, path, Some(key), depth)?;
        if early && !held {
            self.post_after(key.instance, 1, Event::Timer(key.raw, 1))?;
        }
        Ok(())
    }

    pub(super) fn set_button_down(
        &mut self,
        key: StateKey,
        node: &Node,
        down: bool,
        depth: usize,
    ) -> Result<(), InitFault> {
        self.state_mut(key).expect("active logical button").kind =
            StateKind::LogicalButton { down };
        let (previous, edge, next) = if down {
            ("m_1C9EF058", "m_onGoingDownPlug", "m_80AF45FB")
        } else {
            ("m_80AF45FB", "m_onComingUpPlug", "m_1C9EF058")
        };
        self.exit_subgraph(key.instance, node, previous, depth + 1)?;
        self.follow(key.instance, node, edge, Some(key), depth)?;
        if self.state(key).is_some_and(|state| state.active) {
            self.follow(key.instance, node, next, Some(key), depth)?;
        }
        Ok(())
    }
}
