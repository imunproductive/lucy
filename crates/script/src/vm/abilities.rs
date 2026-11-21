//! Ability activation, stacking and cooldowns.
//!
//! Client research (PC 1.74): update 0x7FF789B95560, keep 0x7FF789BB87E0,
//! start check 0x7FF789BB8D20 (`try_start_ability`), start
//! 0x7FF789BA9460, stop 0x7FF789BA6BC0, push 0x7FF789BA6D40, stack hook
//! 0x7FF789B94DC0, output 0x7FF789B94D00, button edge 0x7FF789B947C0 (a
//! press of the start, hold or stop button that no DisableLogicalButton
//! blocks is kept with its time and the state's activation, 0x7FF789BAE6F0
//! takes one within the early-press time). Keep: with no hold button (or
//! m_755F9921 set) a press of the stop button stops it, latched (flag 4)
//! when it is the start button; with a hold button, letting go stops it;
//! then the continue condition and the stack's "continuing" priority. An
//! ability with a stack variable and a priority to set (m_5E561E77) pushes
//! itself (flag 2, slot 1, value true) and starts when its entry is the
//! top; it stops when it is no longer the top.
//! Running is flag 1. The cooldown variable is linked as slot 0 (client
//! restart 0x7FF789BBB110 → link 0x7FF78AA849B0) at the priority of
//! `m_5E561E77`, as the stack is, and falls by `rate` per second from the
//! value last written. AbilityCooldownDuration (evaluator 0x7FF789BE2E80) is
//! `max(base * scalar, least)`, scalar 1 by default.

use super::*;

impl<'graph, 'world, 'machine, W: World + ?Sized> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn ability_buttons(&mut self, key: StateKey, node: &Node) -> Option<[u8; 3]> {
        let mut buttons = [0; 3];
        for (button, field_name) in
            buttons
                .iter_mut()
                .zip(["m_A5F1A77B", "m_A368407D", "m_8CBD15DA"])
        {
            let value = self.evaluate(key.instance, node, field(&node.fields, field_name), None)?;
            *button = u8::try_from(expr::to_int(&value, 0)).ok()?;
        }
        Some(buttons)
    }

    /// `Ability.held` in `ow174/game/script/nodes.py`: held and not blocked
    /// by a `DisableLogicalButton`.
    fn ability_held(&self, button: u8) -> bool {
        self.machine.held_buttons.contains(&button) && !self.disabled(i64::from(button))
    }

    pub(super) fn ability_edge(
        &mut self,
        key: StateKey,
        node: &Node,
        edge: ButtonEdge,
    ) -> Result<(), InitFault> {
        if !edge.pressed
            || edge.button == 0
            || self.disabled(i64::from(edge.button))
            || !self
                .ability_buttons(key, node)
                .is_some_and(|buttons| buttons.contains(&edge.button))
        {
            return Ok(());
        }
        let state = self.state(key);
        let active = state.is_some_and(|state| state.active);
        let activation = state
            .map_or(0, |state| state.activation)
            .wrapping_add(u16::from(!active));
        let now = self.machine.now_ms;
        let presses = self.machine.ability_presses.entry(key).or_default();
        presses.retain(|press| now - press.time_ms < 2000);
        presses.push(AbilityPress {
            button: edge.button,
            time_ms: now,
            activation,
        });
        if active {
            self.post(key.instance, Event::AbilityCheck(key.raw))?;
        }
        Ok(())
    }

    /// A buffered press of `button` within the early-press time
    /// (m_246438AD) for this activation (`Ability.take_press` in
    /// `ow174/game/script/nodes.py`). `retain` leaves it buffered: the start
    /// check looks at the press first and takes it only when the ability
    /// starts. Ported from leafall903/ORPHEUS (MIT), commit c239a84
    /// (`take_press(retain=True)`, `docs/MCCREE_ROLL.txt`).
    pub(super) fn take_ability_press(
        &mut self,
        key: StateKey,
        node: &Node,
        button: u8,
        retain: bool,
    ) -> bool {
        let early = field(&node.fields, "m_246438AD");
        let window = if early.is_null() {
            0.0
        } else {
            let Some(value) = self.evaluate(key.instance, node, early, None) else {
                return false;
            };
            ((expr::to_float(&value) * 1000.0) as f32).round()
        };
        let now = self.machine.now_ms;
        let activation = self.state(key).expect("active ability").activation;
        let Some(presses) = self.machine.ability_presses.get_mut(&key) else {
            return false;
        };
        let Some(index) = presses.iter().position(|press| {
            press.button == button
                && (now - press.time_ms) as f32 <= window
                && press.activation <= activation
        }) else {
            return false;
        };
        if !retain {
            presses.remove(index);
        }
        true
    }

    pub(super) fn ability_flags(&self, key: StateKey) -> Option<u8> {
        let StateKind::Ability { flags } = self.state(key).filter(|state| state.active)?.kind
        else {
            return None;
        };
        Some(flags)
    }

    pub(super) fn set_ability_flags(&mut self, key: StateKey, flags: u8) {
        if self.ability_flags_set(key) != Some(flags) {
            self.state_mut(key).expect("active ability").kind = StateKind::Ability { flags };
        }
    }

    /// The flags of the Ability state `key`, active or not.
    fn ability_flags_set(&self, key: StateKey) -> Option<u8> {
        match self.state(key)?.kind {
            StateKind::Ability { flags } => Some(flags),
            _ => None,
        }
    }

    pub(super) fn ability_condition(&mut self, key: StateKey, node: &Node, name: &str) -> bool {
        let config = field(&node.fields, name);
        config.is_null()
            || self
                .evaluate(key.instance, node, config, Some(key))
                .is_some_and(|value| truthy(&value))
    }

    pub(super) fn ability_stack_allows(&mut self, key: StateKey, node: &Node, name: &str) -> bool {
        let config = field(&node.fields, name);
        if config.is_null() {
            return true;
        }
        let variable = match expr::lvalue(field(&node.fields, "m_out_StackVar")) {
            Ok(Some(variable)) => source_key(key.instance, variable),
            Ok(None) => return true,
            Err(error) => {
                self.report(self.site(key.instance, node), Issue::Expression(error));
                return false;
            }
        };
        match expr::stack_priority(config) {
            Ok(priority) => {
                self.watch(variable, key);
                self.machine.allows_stack(variable, priority, Some(key))
            }
            Err(error) => {
                self.report(self.site(key.instance, node), Issue::Expression(error));
                false
            }
        }
    }

    pub(super) fn activate_ability(
        &mut self,
        key: StateKey,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(flags) = self.ability_flags(key) else {
            return Ok(());
        };
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("ability state");
        self.settle_cooldown(key, node, depth)?;
        if self.machine.owner_dead {
            return self.stop_ability(key, node, depth);
        }
        let Some(buttons) = self.ability_buttons(key, node) else {
            return Ok(());
        };
        let active_flag = if ability_is_stacked(node) {
            ABILITY_PUSHED
        } else {
            ABILITY_RUNNING
        };
        if flags & active_flag != 0 {
            self.keep_ability(key, node, flags, buttons, depth)
        } else {
            self.try_start_ability(key, node, flags, buttons, depth)
        }
    }

    /// The update's cooldown check (client 0x7FF789B95560, before the
    /// start and keep): with the cooldown linked to this state, a value at
    /// or below 0 ends the link and leaves the variable at 0
    /// (0x7FF78AA84610 slot 0, 0.0); unlinked, a value above 0 restarts the
    /// cooldown from it (0x7FF789BBB110). So the link lasts from a restart
    /// to the end of its fall, and the owner's client, which unlinks there
    /// itself, gets the variable at 0 and unbound, as it has it. The
    /// client's other branch, taken when its entity's component 25 has
    /// bit 10 of `+88` (0x7FF789B2D860) and `m_F6BF1466` is true, clears
    /// the cooldown; the server keeps no such status.
    pub(super) fn settle_cooldown(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Ok(Some(target)) = expr::lvalue(field(&node.fields, "m_out_CooldownVar")) else {
            return Ok(());
        };
        let target = source_key(key.instance, target);
        let value = expr::to_float(&self.machine.value(target, None)) as f32;
        let linked = self.machine.links.get(&target).is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry.state == key && matches!(entry.output, LinkValue::Cooldown))
        });
        if !linked {
            return if value > 0.0 {
                self.restart_cooldown(key, node, depth)
            } else {
                Ok(())
            };
        }
        if value <= 0.0 {
            if let Some(state) = self.state_mut(key) {
                state.linked_variable = None;
            }
            self.remove_link(key, target, Some(Value::Float(0.0)), depth)?;
        }
        Ok(())
    }

    pub(super) fn keep_ability(
        &mut self,
        key: StateKey,
        node: &Node,
        mut flags: u8,
        [start, hold, stop]: [u8; 3],
        depth: usize,
    ) -> Result<(), InitFault> {
        if !self.ability_held(start) {
            flags &= !ABILITY_HELD;
            self.set_ability_flags(key, flags);
        }
        let keep_held = field(&node.fields, "m_755F9921");
        if hold == 0
            || (!keep_held.is_null()
                && self
                    .evaluate(key.instance, node, keep_held, Some(key))
                    .is_some_and(|value| truthy(&value)))
        {
            if stop != 0 && self.take_ability_press(key, node, stop, false) {
                if stop == start {
                    self.set_ability_flags(key, flags | ABILITY_HELD);
                }
                return self.stop_ability(key, node, depth);
            }
        } else if !self.machine.held_buttons.contains(&hold) {
            // The keep check tests the hold button against the held list
            // alone (0x7FF789BB8A3C), not against a DisableLogicalButton.
            return self.stop_ability(key, node, depth);
        }
        if !self.ability_condition(key, node, "m_F495731F")
            || !self.ability_stack_allows(key, node, "m_343E99EE")
        {
            self.stop_ability(key, node, depth)?;
        }
        Ok(())
    }

    /// The start check (client 0x7FF789BB8D20). With a hold button
    /// (`m_A368407D`) the ability may start while that button is held and
    /// not blocked; a buffered press of it, else of the start button, is
    /// taken. Without one, a buffered press of the start button
    /// (`m_A5F1A77B`) within the early-press time (`m_246438AD`) may start
    /// it, or the start button held while the latch (flag 4) is down: a
    /// start through the start button sets the latch, and it stays until
    /// the button comes up. Then every gate: the cooldown at 0, the start
    /// (`m_150F0D92`) and continue (`m_F495731F`) conditions true, and the
    /// stack variable taking each of the three priorities it has
    /// (`m_3C0EB4A6`, `m_343E99EE` and the push's `m_5E561E77`; would-stack
    /// 0x7FF78AAA5E10). A gate that fails puts the taken press back
    /// (0x7FF789BB93BF), so it can still start the ability within its time:
    /// a press while another ability holds the stack at the same priority
    /// starts this one when that one ends (Rocket Punch pressed during
    /// Rising Uppercut, Rising Uppercut pressed in the punch's last
    /// frames). Not modelled: the client's flag 8, the hold button's latch,
    /// which its code only reads and clears (the server's frames set it),
    /// and the stun gate (`m_C558D07B` false and the entity's
    /// 0x7FF789B2CC30).
    pub(super) fn try_start_ability(
        &mut self,
        key: StateKey,
        node: &Node,
        flags: u8,
        [start, hold, _]: [u8; 3],
        depth: usize,
    ) -> Result<(), InitFault> {
        // The press stays buffered with its own time until every gate passes:
        // putting it back with the current time on each unready frame kept a
        // released early press alive forever, so the ability started by
        // itself when its cooldown ended (leafall903/ORPHEUS (MIT), commit
        // c239a84, `docs/MCCREE_ROLL.txt`: "Retain the original press until
        // gates pass, consume only on start, and let its authored early-press
        // window expire").
        let mut taken = None;
        let mut latch = false;
        if hold != 0 {
            if !self.ability_held(hold) {
                return Ok(());
            }
            if self.take_ability_press(key, node, hold, true) {
                taken = Some(hold);
            } else if start != 0 && self.take_ability_press(key, node, start, true) {
                taken = Some(start);
                latch = true;
            }
        } else if start != 0 {
            if self.take_ability_press(key, node, start, true) {
                taken = Some(start);
            } else if !self.ability_held(start) {
                self.set_ability_flags(key, flags & !ABILITY_HELD);
                return Ok(());
            } else if flags & ABILITY_HELD != 0 {
                return Ok(());
            }
            latch = true;
        }
        if self.cooldown_remaining(key) > 0.0
            || !self.ability_condition(key, node, "m_150F0D92")
            || !self.ability_condition(key, node, "m_F495731F")
            || ["m_3C0EB4A6", "m_343E99EE", "m_5E561E77"]
                .into_iter()
                .any(|priority| !self.ability_stack_allows(key, node, priority))
        {
            return Ok(());
        }
        if let Some(button) = taken {
            self.take_ability_press(key, node, button, false);
        }
        if latch {
            self.set_ability_flags(key, flags | ABILITY_HELD);
        }
        if ability_is_stacked(node) {
            self.push_ability(key, node, depth)
        } else {
            self.start_ability(key, node, depth)
        }
    }

    pub(super) fn push_ability(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let target = match expr::lvalue(field(&node.fields, "m_out_StackVar")) {
            Ok(Some(target)) => source_key(id, target),
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        let priority = match expr::stack_priority(field(&node.fields, "m_5E561E77")) {
            Ok(priority) => priority,
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        if self.state(key).and_then(|state| state.linked_variable) == Some(target) {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("overlapping ability outputs"),
            );
            return Ok(());
        }
        let state = self.state_mut(key).expect("active ability");
        let StateKind::Ability { flags } = state.kind else {
            unreachable!("active ability")
        };
        state.kind = StateKind::Ability {
            flags: flags | ABILITY_PUSHED,
        };
        state.ability_stack = Some(target);
        self.insert_link(
            target,
            VariableLink {
                state: key,
                priority,
                output: LinkValue::AbilityStack,
                stack: true,
            },
            depth,
        )
    }

    pub(super) fn start_ability(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let flags = self.ability_flags(key).expect("active ability");
        self.set_ability_flags(key, flags | ABILITY_RUNNING);
        self.follow(id, node, "m_B969AFC9", Some(key), depth)?;
        self.follow(id, node, "m_4E22B5C4", Some(key), depth)?;
        self.ability_vars(id, node, "m_EF3C5C73")
    }

    pub(super) fn stop_ability(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        self.unlink_ability_stack(key, depth)?;
        let Some(flags) = self
            .ability_flags(key)
            .filter(|flags| flags & ABILITY_RUNNING != 0)
        else {
            return Ok(());
        };
        let id = key.instance;
        self.set_ability_flags(key, flags & !ABILITY_RUNNING);
        self.exit_subgraph(id, node, "m_B969AFC9", depth + 1)?;
        self.follow(id, node, "m_6EEEC0AA", Some(key), depth)?;
        self.ability_vars(id, node, "m_19899EAC")
    }

    pub(super) fn unlink_ability_stack(
        &mut self,
        key: StateKey,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(flags) = self
            .ability_flags(key)
            .filter(|flags| flags & ABILITY_PUSHED != 0)
        else {
            return Ok(());
        };
        self.set_ability_flags(key, flags & !ABILITY_PUSHED);
        if let Some(target) = self
            .state_mut(key)
            .and_then(|state| state.ability_stack.take())
        {
            self.remove_link(key, target, None, depth + 1)?;
        }
        Ok(())
    }

    pub(super) fn cooldown_remaining(&self, key: StateKey) -> f64 {
        self.state(key)
            .and_then(|state| state.ability_cooldown)
            .map_or(0.0, |cooldown| cooldown.remaining(self.machine.now_ms))
    }

    pub(super) fn restart_cooldown(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let target = match expr::lvalue(field(&node.fields, "m_out_CooldownVar")) {
            Ok(Some(target)) => source_key(id, target),
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        let modifier = self.machine.value(VarKey::Entity(3652), None);
        let mut rate = (expr::to_float(&modifier) + 1.0) as f32;
        let scalar = field(&node.fields, "m_F7CAF887");
        if !scalar.is_null() {
            let Some(value) = self.evaluate(id, node, scalar, None) else {
                return Ok(());
            };
            rate = (f64::from(rate) * expr::to_float(&value)) as f32;
        }
        let current = expr::to_float(&stored(&self.machine.snapshot, target)).max(0.0);
        let delay = ((current * 1000.0) as f32 / rate).round();
        if !current.is_finite() || !rate.is_finite() || (rate > 0.0 && !delay.is_finite()) {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("ability cooldown"),
            );
            return Ok(());
        }
        let priority = match expr::stack_priority(field(&node.fields, "m_5E561E77")) {
            Ok(priority) => priority,
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        // The reference removes its previous slot before relinking at the same priority.
        if let Some(entries) = self.machine.links.get_mut(&target) {
            entries.retain(|entry| entry.state != key);
        }
        let cooldown = AbilityCooldown {
            current,
            rate: f64::from(rate),
            last_ms: self.machine.now_ms,
        };
        let state = self.state_mut(key).expect("active ability");
        state.linked_variable = Some(target);
        state.ability_cooldown = Some(cooldown);
        self.insert_link(
            target,
            VariableLink {
                state: key,
                priority,
                output: LinkValue::Cooldown,
                stack: false,
            },
            depth,
        )?;
        self.machine.queues.retain(
            id,
            |_, event| !matches!(event, Event::Timer(raw, 2) if *raw == key.raw),
        );
        if current > 0.0 && rate > 0.0 {
            self.post_after(id, delay.max(1.0) as u64, Event::Timer(key.raw, 2))?;
        }
        Ok(())
    }

    pub(super) fn finish_cooldown(&mut self, key: StateKey) -> Result<(), InitFault> {
        let remaining = self.cooldown_remaining(key);
        if let Some(target) = self
            .state_mut(key)
            .and_then(|state| state.linked_variable.take())
        {
            self.unlink_output(key, target, Some(Value::Float(remaining)))?;
        }
        Ok(())
    }

    /// Another graph set the cooldown of the ability on `button`
    /// (`STU_FFF26F80`, the Workshop's Set Ability Cooldown `2735`;
    /// client `Action_STU_FFF26F80` 0x7FF789B94EC0): the first running
    /// Ability whose start (`m_A5F1A77B`) or hold (`m_A368407D`) button is
    /// `button` (0x7FF789C9EED0, over the host's abilities) gets `value`
    /// written to its `m_out_CooldownVar` (+0x198, 0x7FF78AA9D150). A
    /// cooldown that runs falls from the written value from now on, as
    /// after its own restart: so 0 ends it at the next update
    /// ([`Self::settle_cooldown`]), and the owner frames carry the
    /// restarted link (assumption: the client's write reaches the link's
    /// base the same way; DoomfistMoves12 traced the restart and the
    /// output, not the write's effect on a linked variable). False: no
    /// ability has that button.
    pub(super) fn set_ability_cooldown(
        &mut self,
        button: i64,
        value: f64,
    ) -> Result<bool, InitFault> {
        if button == 0 {
            return Ok(false);
        }
        let abilities: Vec<StateKey> = self
            .machine
            .snapshot
            .instances
            .iter()
            .flat_map(|(&instance, item)| {
                item.states
                    .iter()
                    .filter(|(_, state)| {
                        state.active && matches!(state.kind, StateKind::Ability { .. })
                    })
                    .map(move |(&raw, _)| StateKey { instance, raw })
            })
            .collect();
        let found = abilities.into_iter().find(|&key| {
            let node = self
                .graph(key.instance)
                .state(usize::from(key.raw))
                .expect("ability state");
            ["m_A5F1A77B", "m_A368407D"].iter().any(|name| {
                self.evaluate(key.instance, node, field(&node.fields, name), None)
                    .is_some_and(|value| expr::to_int(&value, 0) == button)
            })
        });
        let Some(key) = found else {
            return Ok(false);
        };
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("ability state");
        let Ok(Some(target)) = expr::lvalue(field(&node.fields, "m_out_CooldownVar")) else {
            return Ok(false);
        };
        let target = source_key(key.instance, target);
        let old = self.machine.value(target, None);
        self.set_base(target, Value::Float(value));
        if self
            .state(key)
            .is_some_and(|state| state.linked_variable == Some(target))
        {
            self.restart_cooldown(key, node, 0)?;
        }
        self.notify_value(target, &old, None)?;
        Ok(true)
    }

    pub(super) fn ability_vars(
        &mut self,
        id: u16,
        node: &Node,
        name: &str,
    ) -> Result<(), InitFault> {
        for item in array_field(&node.fields, name)
            .iter()
            .filter(|item| item.is_object())
        {
            self.store(id, node, item, false)?;
        }
        Ok(())
    }
}
