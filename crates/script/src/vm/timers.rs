//! Per-frame ticks, waits, timers and child instances.

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn register_tick(&mut self, key: StateKey, start_ms: u64) {
        self.machine
            .ticking
            .entry(key.instance)
            .or_default()
            .entry(key.raw)
            .or_insert(start_ms);
    }

    pub(super) fn tick_states(&mut self, id: u16) -> Result<(), InitFault> {
        let now = self.machine.now_ms;
        let due: Vec<_> = self
            .machine
            .ticking
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|(&raw, &last)| (last != now).then_some(raw))
            .collect();
        for raw in due {
            self.spend("polled state tick")?;
            if let Some(last) = self
                .machine
                .ticking
                .get_mut(&id)
                .and_then(|states| states.get_mut(&raw))
            {
                *last = now;
                let key = StateKey { instance: id, raw };
                match self
                    .state(key)
                    .filter(|state| state.active)
                    .map(|state| state.kind)
                {
                    Some(StateKind::BooleanSwitch(_)) => {
                        self.check_switch(key, 0)?;
                    }
                    Some(StateKind::DelayedSwitch { .. }) => {
                        let node = self
                            .graph(id)
                            .state(usize::from(raw))
                            .expect("delayed switch");
                        self.recheck_delayed_switch(key, node, 0)?;
                    }
                    Some(StateKind::HealthPool { pool: Some(pool) }) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("health pool");
                        if let Some(current) =
                            self.world.health_pool_current(self.site(id, node), pool)
                        {
                            self.publish_health_pool(key, node, current)?;
                        }
                    }
                    Some(StateKind::MovementSample(previous)) => {
                        let node = self
                            .graph(id)
                            .state(usize::from(raw))
                            .expect("movement sample");
                        self.update_movement_sample(key, node, previous)?;
                    }
                    Some(StateKind::Switch { .. }) => self.post(id, Event::Reevaluate(raw))?,
                    Some(StateKind::Ability { .. }) => self.activate_ability(key, 0)?,
                    Some(StateKind::ChaseVar) => self.tick_chase(key)?,
                    Some(StateKind::Watch) => self.check_watch(key)?,
                    Some(StateKind::WaitFrames { target }) if target <= self.machine.frame => {
                        self.post(id, Event::Finish(raw))?;
                    }
                    Some(StateKind::Wait) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("wait");
                        self.tick_wait(key, node)?;
                    }
                    _ => (),
                }
            }
        }
        Ok(())
    }

    pub(super) fn clear_tick(&mut self, key: StateKey) {
        if let Some(states) = self.machine.ticking.get_mut(&key.instance) {
            states.shift_remove(&key.raw);
            if states.is_empty() {
                self.machine.ticking.remove(&key.instance);
            }
        }
    }

    /// `Wait.arm` in `ow174/game/script/nodes.py`: a static timeout arms a
    /// timer (or finishes now); with `m_BF1A93B0` the timeout is evaluated
    /// again every tick.
    pub(super) fn arm_wait(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let Some(milliseconds) = self.wait_timeout(key, node) else {
            return Ok(());
        };
        if configured(node, "m_BF1A93B0") {
            self.register_tick(key, self.machine.now_ms);
            return self.tick_wait(key, node);
        }
        if milliseconds <= 0.0 {
            self.post(id, Event::Finish(key.raw))
        } else {
            self.post_after(id, milliseconds as u64, Event::Timer(key.raw, 1))
        }
    }

    /// A re-evaluated wait finishes once its current timeout has passed
    /// since it began (`Wait.tick`).
    pub(super) fn tick_wait(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let Some(milliseconds) = self.wait_timeout(key, node) else {
            return Ok(());
        };
        let start = self.state(key).expect("active wait").start_ms;
        if (self.machine.now_ms - start) as f64 >= f64::from(milliseconds) {
            self.post(key.instance, Event::Finish(key.raw))?;
        }
        Ok(())
    }

    fn wait_timeout(&mut self, key: StateKey, node: &Node) -> Option<f32> {
        let id = key.instance;
        let seconds = self.evaluate(id, node, field(&node.fields, "m_timeout"), Some(key))?;
        let milliseconds = ((expr::to_float(&seconds) * 1000.0) as f32).round();
        if !milliseconds.is_finite() {
            self.report(self.site(id, node), Issue::UnsupportedField("Wait timeout"));
            return None;
        }
        Some(milliseconds)
    }

    /// `WaitForServerFrames` finishes after the count of its integer config
    /// (one frame without one). The Python reference takes one server frame
    /// as one 16 ms command frame, which is unverified.
    pub(super) fn arm_server_frames(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let config = node.fields.as_object().and_then(|fields| {
            fields.values().find(|value| {
                matches!(
                    field(value, "$").as_str(),
                    Some("STUConfigVarInt" | "STU_919FD47C")
                )
            })
        });
        let count = match config {
            Some(config) => {
                let Some(count) = self.evaluate(id, node, config, None) else {
                    return Ok(());
                };
                expr::to_int(&count, 0)
            }
            None => 1,
        };
        self.assume(
            self.site(id, node),
            &node.class,
            "a server frame is a command frame",
        );
        let delay = u64::try_from(count.max(0))
            .unwrap_or_default()
            .saturating_mul(FRAME_MS);
        self.post_after(id, delay, Event::Finish(key.raw))
    }

    /// `Defer.on_begin`: the state joins its instance's deferred list, which
    /// stays sorted by priority (a new entry after equal ones).
    pub(super) fn begin_defer(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let Some(priority) =
            self.evaluate(key.instance, node, field(&node.fields, "m_priority"), None)
        else {
            return Ok(());
        };
        let priority = expr::to_float(&priority);
        let deferred = self.machine.deferred.entry(key.instance).or_default();
        let at = deferred.partition_point(|&(other, _)| other <= priority);
        deferred.insert(at, (priority, key.raw));
        Ok(())
    }

    /// The deferred phase of an instance's update (`Instance.run_deferred`):
    /// from the last entry down, each active `Defer` follows its out plug,
    /// then its subgraph. Returns whether anything was deferred.
    pub(super) fn run_deferred(&mut self, id: u16) -> Result<bool, InitFault> {
        let Some(deferred) = self
            .machine
            .deferred
            .remove(&id)
            .filter(|deferred| !deferred.is_empty())
        else {
            return Ok(false);
        };
        for (_, raw) in deferred.into_iter().rev() {
            let key = StateKey { instance: id, raw };
            if !self.state(key).is_some_and(|state| state.active) {
                continue;
            }
            let node = self.graph(id).state(usize::from(raw)).expect("defer state");
            self.follow(id, node, "m_FD0C0646", Some(key), 0)?;
            if self.state(key).is_some_and(|state| state.active) {
                self.follow(id, node, "m_8E484B2E", Some(key), 0)?;
            }
        }
        Ok(true)
    }

    pub(super) fn arm_animation(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let duration = field(&node.fields, "m_86701C3E");
        if duration.is_null() {
            // The length is that of the animation the alias resolves to,
            // which the server doesn't resolve (docs/notes/parity.md). Like
            // the Python reference, the state then doesn't end by itself.
            self.assume(self.site(id, node), &node.class, "never ends by itself");
            return Ok(());
        }
        let Some(seconds) = self.evaluate(id, node, duration, None) else {
            return Ok(());
        };
        let delay = ((expr::to_float(&seconds) * 1000.0) as f32).round();
        if !delay.is_finite() {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("animation duration"),
            );
            return Ok(());
        }
        let removes_self = field(&node.fields, "m_154DBA91");
        let client_ends = if removes_self.is_null() {
            false
        } else {
            let Some(value) = self.evaluate(id, node, removes_self, None) else {
                return Ok(());
            };
            truthy(&value)
        };
        let delay_ms = delay.max(0.0) as u64;
        self.post_after(id, delay_ms, Event::Timer(key.raw, 1))?;
        if !client_ends {
            self.post_after(id, delay_ms, Event::Finish(key.raw))?;
        }
        Ok(())
    }

    pub(super) fn arm_frame_wait(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let Some(value) = self.evaluate(id, node, field(&node.fields, "m_3016B9A1"), None) else {
            return Ok(());
        };
        let count = expr::to_int(&value, 0) as u32;
        let target = self.machine.frame.wrapping_add(count);
        self.state_mut(key).expect("active frame wait").kind = StateKind::WaitFrames { target };
        self.register_tick(key, self.machine.now_ms);
        if target <= self.machine.frame {
            self.post(id, Event::Finish(key.raw))
        } else {
            Ok(())
        }
    }

    pub(super) fn timer(&mut self, key: StateKey, parameter: u16) -> Result<(), InitFault> {
        let Some(state) = self.state(key).filter(|state| state.active) else {
            return Ok(());
        };
        let kind = state.kind;
        if kind == StateKind::Wait && parameter == 1 {
            self.post(key.instance, Event::Finish(key.raw))?;
        }
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("active timer state");
        if matches!(kind, StateKind::LogicalButton { .. })
            && parameter == 1
            && !self.button_held(key, node)
        {
            self.set_button_down(key, node, false, 0)?;
        }
        if kind == StateKind::Anim && parameter == 1 {
            self.follow(key.instance, node, "m_A3B86BB0", Some(key), 0)?;
        }
        if let StateKind::DelayedSwitch { current, undecided } = kind {
            let branch = match parameter {
                SWITCH_TO_TRUE => Some(true),
                SWITCH_TO_FALSE => Some(false),
                _ => None,
            };
            if let Some(branch) = branch {
                self.state_mut(key)
                    .expect("active delayed switch")
                    .delayed_timers[usize::from(branch)] = false;
                if undecided || current != branch {
                    self.set_delayed_branch(key, node, branch, 0)?;
                }
            }
        }
        if matches!(kind, StateKind::Ability { .. }) && matches!(parameter, 1 | 2) {
            self.activate_ability(key, 0)?;
        }
        if matches!(kind, StateKind::WeaponVolley { .. }) {
            match parameter {
                1 => {
                    self.follow(key.instance, node, "m_onMinShotsPlug", Some(key), 0)?;
                }
                2 => self.post(key.instance, Event::Finish(key.raw))?,
                _ => (),
            }
        }
        self.try_complete(key, node, 0)
    }

    pub(super) fn spawn_child(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<Option<u16>, InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        let Some(guid) = node
            .field("m_36F7B64C.m_36F7B64C.m_C974E369.m_graph")
            .and_then(asset_guid)
        else {
            self.report(site, Issue::InvalidSubScript);
            return Ok(None);
        };
        let graph = u32::from(guid as u16);
        if self.graphs.graph(graph).is_none() {
            self.report(site, Issue::MissingGraph(graph));
            return Ok(None);
        }
        let mut vars = Variables::new();
        let mut entity_vars = Variables::new();
        for item in array_field(&node.fields, "m_89402D74") {
            if !item.is_object() {
                continue;
            }
            let target = match expr::lvalue(field(item, "m_out_Var")) {
                Ok(Some(target)) => target,
                Ok(None) => continue,
                Err(error) => {
                    self.report(site, Issue::Expression(error));
                    return Ok(None);
                }
            };
            let Some(value) = self.evaluate(id, node, field(item, "m_value"), None) else {
                return Ok(None);
            };
            match target.scope {
                Scope::Instance => vars.insert(target.id, value),
                Scope::Entity => entity_vars.insert(target.id, value),
            };
        }
        let child =
            u16::try_from(self.machine.next_id).map_err(|_| InitFault::InstanceIdExhausted)?;
        if self.create(child, graph, Some((id, key.raw)), vars)? {
            // The reference sets child binding bases directly before Entry.
            self.machine.snapshot.variables.extend(entity_vars);
            return Ok(Some(child));
        }
        Ok(None)
    }

    pub(super) fn try_complete(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        for (index, _) in array_field(&node.fields, "m_transitionPlug")
            .iter()
            .enumerate()
        {
            if self.follow(
                key.instance,
                node,
                &format!("m_transitionPlug[{index}]"),
                Some(key),
                depth,
            )? {
                self.post(key.instance, Event::Abort(key.raw))?;
                break;
            }
        }
        Ok(())
    }
}
