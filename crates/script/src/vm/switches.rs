//! Watched values, switches, delayed switches and branches.
//!
//! Client research (1.74): BooleanSwitch OnBegin 0x7FF78AAA8F50 evaluates
//! m_condition (its variables become watched) and follows the true or false
//! plug and subgraph; a watched change queues one re-evaluation and the branch
//! switches only when the result changed.

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn changed(&mut self, key: StateKey) -> Result<(), InitFault> {
        if let Some(state) = self.state_mut(key) {
            state.pending_change = false;
            if state.active && matches!(state.kind, StateKind::Switch { .. }) {
                self.post(key.instance, Event::Reevaluate(key.raw))?;
            } else if state.active
                && !state.pending_timer
                && matches!(
                    state.kind,
                    StateKind::BooleanSwitch(_) | StateKind::DelayedSwitch { .. }
                )
            {
                state.pending_timer = true;
                self.post(key.instance, Event::Reevaluate(key.raw))?;
            } else if state.active && matches!(state.kind, StateKind::Stack { .. }) {
                self.update_stack(key)?;
            } else if state.active && state.kind == StateKind::Watch {
                self.check_watch(key)?;
            } else if state.active && matches!(state.kind, StateKind::HealthPool { pool: Some(_) })
            {
                let node = self
                    .graph(key.instance)
                    .state(usize::from(key.raw))
                    .expect("active health pool");
                self.report(
                    self.site(key.instance, node),
                    Issue::UnsupportedField("changing health pool capacity"),
                );
            }
        }
        Ok(())
    }

    pub(super) fn check_watch(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(seen) = self
            .state(key)
            .filter(|state| state.active)
            .and_then(|state| state.watch_seen.clone())
        else {
            return Ok(());
        };
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("Watch state");
        if self
            .evaluate(id, node, field(&node.fields, "m_B7A3CF78"), Some(key))
            .is_some_and(|value| !equal(&value, &seen))
        {
            self.post(id, Event::WatchChanged(key.raw))?;
        }
        Ok(())
    }

    pub(super) fn deliver_watch(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(seen) = self
            .state(key)
            .filter(|state| state.active)
            .and_then(|state| state.watch_seen.clone())
        else {
            return Ok(());
        };
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("Watch state");
        match expr::lvalue(field(&node.fields, "m_48A8CDEC")) {
            Ok(Some(target)) => self.write(source_key(id, target), seen)?,
            Ok(None) => (),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        }
        self.follow(id, node, "m_A70DB860", Some(key), 0)?;
        let seen = self.evaluate(id, node, field(&node.fields, "m_B7A3CF78"), Some(key));
        if let Some(state) = self.state_mut(key) {
            state.watch_seen = seen
        }
        self.try_complete(key, node, 0)
    }

    pub(super) fn reevaluate(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        let Some(state) = self.state_mut(key).filter(|state| state.active) else {
            return Ok(());
        };
        state.pending_timer = false;
        let kind = state.kind;
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("switch state");
        let evaluated = if matches!(kind, StateKind::DelayedSwitch { .. }) {
            self.recheck_delayed_switch(key, node, depth)?;
            true
        } else if let StateKind::Switch { case } = kind {
            let Some(next) = self.switch_case(key, node) else {
                return Ok(());
            };
            if next != case {
                self.exit_subgraph(key.instance, node, &case_path(case), depth + 1)?;
                self.state_mut(key).expect("active switch").kind = StateKind::Switch { case: next };
                self.follow(key.instance, node, &case_path(next), Some(key), depth)?;
            }
            true
        } else {
            self.check_switch(key, depth)?
        };
        if evaluated {
            self.try_complete(key, node, depth)?;
        }
        Ok(())
    }

    pub(super) fn check_switch(&mut self, key: StateKey, depth: usize) -> Result<bool, InitFault> {
        let Some(StateKind::BooleanSwitch(current)) = self
            .state(key)
            .filter(|state| state.active)
            .map(|state| state.kind)
        else {
            return Ok(false);
        };
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("active state node");
        if let Some(value) = self.evaluate(id, node, field(&node.fields, "m_condition"), Some(key))
        {
            let next = truthy(&value);
            if next != current {
                self.exit_subgraph(id, node, branch_path(current), depth + 1)?;
                self.state_mut(key).expect("active switch").kind = StateKind::BooleanSwitch(next);
                self.enter_branch(key, node, next, depth)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn begin_delayed_switch(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let Some(undecided) = self.evaluate(id, node, field(&node.fields, "m_308B8A80"), None)
        else {
            return Ok(());
        };
        let undecided = truthy(&undecided);
        let current = if undecided {
            false
        } else {
            let initial = field(&node.fields, "m_418D9D9D");
            let config = if initial.is_null() {
                field(&node.fields, "m_BD83C8E6")
            } else {
                initial
            };
            let Some(initial) = self.evaluate(id, node, config, Some(key)) else {
                return Ok(());
            };
            truthy(&initial)
        };
        self.state_mut(key).expect("active delayed switch").kind =
            StateKind::DelayedSwitch { current, undecided };
        self.register_tick(key, self.machine.now_ms);
        if !undecided {
            self.enter_branch(key, node, current, depth)?;
        }
        self.recheck_delayed_switch(key, node, depth)
    }

    pub(super) fn recheck_delayed_switch(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(StateKind::DelayedSwitch { current, undecided }) = self
            .state(key)
            .filter(|state| state.active)
            .map(|state| state.kind)
        else {
            return Ok(());
        };
        let id = key.instance;
        let condition = field(&node.fields, "m_BD83C8E6");
        let false_override = field(&node.fields, "m_E8FC060D");
        let requested = if current {
            let Some(holds_false) =
                self.delayed_false_condition(key, node, condition, false_override)
            else {
                return Ok(());
            };
            holds_false.then_some(false)
        } else {
            let Some(holds_true) = self.evaluate(id, node, condition, Some(key)) else {
                return Ok(());
            };
            if truthy(&holds_true) {
                Some(true)
            } else if undecided {
                let Some(holds_false) =
                    self.delayed_false_condition(key, node, condition, false_override)
                else {
                    return Ok(());
                };
                holds_false.then_some(false)
            } else {
                None
            }
        };
        let Some(branch) = requested else {
            if !undecided {
                self.cancel_delayed_timer(key, !current);
            }
            return Ok(());
        };
        let delay_field = if branch { "m_7130456B" } else { "m_ED60694A" };
        let Some(delay) = self.evaluate(id, node, field(&node.fields, delay_field), None) else {
            return Ok(());
        };
        let seconds = expr::to_float(&delay);
        let milliseconds = ((seconds as f32) * 1000.0).trunc();
        if !milliseconds.is_finite() {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("delayed switch timeout"),
            );
            return Ok(());
        }
        if seconds > 0.0 {
            self.arm_delayed_timer(key, branch, milliseconds as u64)?;
        }
        if undecided {
            self.cancel_delayed_timer(key, !branch);
        }
        if seconds <= 0.0 {
            self.set_delayed_branch(key, node, branch, depth)?;
        }
        Ok(())
    }

    pub(super) fn delayed_false_condition(
        &mut self,
        key: StateKey,
        node: &Node,
        condition: &Json,
        override_config: &Json,
    ) -> Option<bool> {
        let invert = override_config.is_null();
        let config = if invert { condition } else { override_config };
        self.evaluate(key.instance, node, config, Some(key))
            .map(|value| truthy(&value) != invert)
    }

    pub(super) fn arm_delayed_timer(
        &mut self,
        key: StateKey,
        branch: bool,
        delay_ms: u64,
    ) -> Result<(), InitFault> {
        let index = usize::from(branch);
        if !self
            .state(key)
            .expect("active delayed switch")
            .delayed_timers[index]
        {
            let parameter = if branch {
                SWITCH_TO_TRUE
            } else {
                SWITCH_TO_FALSE
            };
            self.post_after(key.instance, delay_ms, Event::Timer(key.raw, parameter))?;
            self.state_mut(key)
                .expect("active delayed switch")
                .delayed_timers[index] = true;
        }
        Ok(())
    }

    pub(super) fn cancel_delayed_timer(&mut self, key: StateKey, branch: bool) {
        let pending = &mut self
            .state_mut(key)
            .expect("active delayed switch")
            .delayed_timers[usize::from(branch)];
        if std::mem::take(pending) {
            let parameter = if branch {
                SWITCH_TO_TRUE
            } else {
                SWITCH_TO_FALSE
            };
            self.machine.queues.retain(key.instance, |_, event| {
                !matches!(event, Event::Timer(raw, param) if *raw == key.raw && *param == parameter)
            });
        }
    }

    pub(super) fn set_delayed_branch(
        &mut self,
        key: StateKey,
        node: &Node,
        next: bool,
        depth: usize,
    ) -> Result<(), InitFault> {
        if let StateKind::DelayedSwitch {
            current,
            undecided: false,
        } = self.state(key).expect("active delayed switch").kind
        {
            self.exit_subgraph(key.instance, node, branch_path(current), depth + 1)?;
        }
        self.state_mut(key).expect("active delayed switch").kind = StateKind::DelayedSwitch {
            current: next,
            undecided: false,
        };
        self.enter_branch(key, node, next, depth)
    }

    pub(super) fn switch_case(&mut self, key: StateKey, node: &Node) -> Option<Option<usize>> {
        let value = self.evaluate(
            key.instance,
            node,
            field(&node.fields, "m_262AE982"),
            Some(key),
        )?;
        for (index, case) in array_field(&node.fields, "m_05706D1C").iter().enumerate() {
            if case.is_object() {
                let candidate = self.evaluate(key.instance, node, field(case, "m_value"), None)?;
                if equal(&value, &candidate) {
                    return Some(Some(index));
                }
            }
        }
        Some(None)
    }

    pub(super) fn enter_branch(
        &mut self,
        key: StateKey,
        node: &Node,
        branch: bool,
        depth: usize,
    ) -> Result<(), InitFault> {
        let output = if branch {
            "m_onTruePlug"
        } else {
            "m_onFalsePlug"
        };
        self.follow(key.instance, node, output, Some(key), depth)?;
        if self.state(key).is_some_and(|state| state.active) {
            self.follow(key.instance, node, branch_path(branch), Some(key), depth)?;
        }
        Ok(())
    }
}
