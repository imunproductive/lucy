//! Per-frame ticks, waits, timers and child instances.
//!
//! Client research (1.74): Wait OnEnter 0x7FF78AAA7570: `ms = R(timeout *
//! 1000)`; 0 or less finishes now, else a timer (param 1) at now + ms;
//! m_0DBAFD7F (+256) re-arms on a re-entry, m_BF1A93B0 (+257) re-evaluates
//! the timeout every tick. WaitFrames (game class vtable 0x7FF78B5836C8):
//! OnEnter 0x7FF789C54760 sets the finish frame to the component frame plus
//! m_3016B9A1 (node +0xE8, none = 0) and checks it at once; the tick
//! 0x7FF789C54BC0 finishes once that frame is reached (unsigned); a re-entry
//! restarts it only with m_0DBAFD7F (node +0x100); its payload is that frame
//! (a u32 of its property block). SubScript (OnBegin 0x7FF78AAB1F30) runs a
//! child instance of another graph while active, its variables from
//! m_89402D74, destroyed at the end; the child gets the next free instance id.

use super::*;

/// An `Anim` length below this (seconds) is the model's (leafall903/ORPHEUS
/// `c239a84`, `nodes.py` `Anim.on_enter`: 1000 × `f32::EPSILON`).
const MODEL_LENGTH_BELOW: f64 = 0.000_119_209_289_550_781_25;

/// What an `Anim` state plays, for the body's animation data.
#[derive(Clone, Copy)]
enum AnimAlias {
    /// A zero alias: no model query, length and scalar 0.
    Zero,
    /// The alias and its animation category (0 for none).
    Played(u64, u64),
    /// An alias or category that isn't an asset.
    Unknown,
}

/// The kind of an `Anim` state's handle (`m_7CCEC4F0`): the client's play
/// `0x7FF789BD5810` writes 3 in the handle's first word.
const ANIMATION_HANDLE: u8 = 3;

/// How an `Anim` state's timer runs.
#[derive(Clone, Copy)]
enum AnimationRun {
    /// `m_37EE887D` is true: it arms no timer.
    Untimed,
    /// Its length is unknown: it never ends by itself (an assumption).
    Endless,
    /// Timer 1 after `delay_ms` (`seconds` long), then the finish unless
    /// the client ends it (`m_154DBA91`).
    Timed {
        seconds: f32,
        delay_ms: u64,
        client_ends: bool,
    },
}

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
                        self.update_movement_sample(key, node, previous, true)?;
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
                    Some(StateKind::TrackTargets) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("search");
                        self.track_targets(key, node, false)?;
                    }
                    Some(StateKind::Ray) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("ray");
                        self.update_ray(key, node)?;
                    }
                    Some(StateKind::WallCheck) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("wall check");
                        self.update_wall_check(key, node)?;
                    }
                    Some(StateKind::AreaResidency) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("residency");
                        self.update_area_residency(key, node)?;
                    }
                    Some(StateKind::Landing) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("selector");
                        self.update_landing(key, node)?;
                    }
                    Some(StateKind::Shockwave) => self.update_shockwave(key)?,
                    Some(StateKind::FilterArray) => self.refresh_filter_array(key)?,
                    Some(StateKind::GrappleCast) => self.tick_grapple(key, 0)?,
                    Some(StateKind::ModifyHealth { last_ms }) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("health");
                        self.tick_modify_health(key, node, last_ms)?;
                    }
                    Some(StateKind::ApplyAura) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("aura");
                        self.refresh_auras(key, node)?;
                    }
                    Some(StateKind::Scope) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("scope");
                        self.refresh_scopes(key, node)?;
                    }
                    Some(StateKind::SendMessage) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("sender");
                        self.refresh_send_state(key, node)?;
                    }
                    Some(StateKind::WeaponVolley { .. }) => self.tick_volley(key)?,
                    Some(StateKind::PickupSearch { .. }) => {
                        let node = self.graph(id).state(usize::from(raw)).expect("search");
                        self.search_pickup(key, node, false)?;
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
    /// (`m_3016B9A1`; one frame without one) of the server's own frames.
    /// The client never counts them (`python tools/stu.py STU_E3CEF6B9`:
    /// begin, update and end are the base state's no-ops, codec `none`;
    /// its own slots 52/53 only name the abort and finished plugs): the
    /// server's frames end it. A server frame is the server's simulation
    /// step, the 16 ms command frame both sides share (GDC-Netcode.md §1,
    /// [Ford ~24:00]), which is this server's tick.
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

    /// The alias an `Anim` state plays and its animation category, for the
    /// body's animation data; none when an evaluation failed (reported).
    /// A zero alias skips the model query: its length and scalar are 0
    /// (leafall903/ORPHEUS `c239a84`, `driver.py` `_wheel_animation_key`).
    fn animation_alias(&mut self, id: u16, node: &Node) -> Option<AnimAlias> {
        let alias = field(&node.fields, "m_11E0A658");
        let alias = if alias.is_null() {
            Value::Null
        } else {
            self.evaluate(id, node, alias, None)?
        };
        if !truthy(&alias) {
            return Some(AnimAlias::Zero);
        }
        let category = field(&node.fields, "m_22326315");
        let category = if category.is_null() {
            Value::Null
        } else {
            self.evaluate(id, node, category, None)?
        };
        Some(match (alias, category) {
            (Value::Asset(alias), Value::Null) => AnimAlias::Played(alias, 0),
            (Value::Asset(alias), Value::Asset(category)) => AnimAlias::Played(alias, category),
            _ => AnimAlias::Unknown,
        })
    }

    /// `Anim` (leafall903/ORPHEUS `c239a84`, `src/ow174/game/script/nodes.py`
    /// `Anim`, the client's `CC19A0`/`CC1E70`). On begin, an `m_EBEFB4A7`
    /// output reads the animation's scalar (slot 2) while the state runs,
    /// when the body's data has one. A true `m_37EE887D` arms no timer.
    /// Otherwise the state runs for its authored length `m_86701C3E`; a
    /// length under 1000 × `f32::EPSILON` (none authored reads 0) is the
    /// model's: the animation's length on this body
    /// ([`World::animation_length`]) times `m_speedScalar` (1 without one).
    /// Then timer 1 follows `m_A3B86BB0` and, unless `m_154DBA91` is true,
    /// the state finishes. An alias the body's set lacks is 0 s long (the
    /// client's lookup fails), so that state ends in the frame it began, as
    /// the client's does (OwnerPrediction.md "DayFirstShot2"). An alias
    /// whose length the data can't tell, with no authored length, never
    /// ends by itself (an assumption, as the Python reference:
    /// docs/notes/behavior.md). A state that plays (it has time left, or
    /// no timer) stores its handle in `m_7CCEC4F0`: the client's play
    /// `0x7FF789BD5810` writes `{3, instance, state, 0}` (a type 7 value,
    /// `0x7FF78AAB4A10`), which `STU_6E18DEA0` reads to restart it
    /// ([`Self::restart_animation`]).
    pub(super) fn arm_animation(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        let mut alias = None;
        let target = match expr::lvalue(field(&node.fields, "m_EBEFB4A7")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                None
            }
        };
        if let Some(target) = target {
            let Some(played) = self.animation_alias(id, node) else {
                return Ok(());
            };
            alias = Some(played);
            let scalar = match played {
                AnimAlias::Zero => Some(0.0),
                AnimAlias::Played(alias, category) => {
                    self.world.animation_scalar(site, alias, category)
                }
                AnimAlias::Unknown => None,
            };
            if let Some(scalar) = scalar {
                self.state_mut(key).expect("active anim").linked_variable = Some(target);
                self.insert_link(
                    target,
                    VariableLink {
                        state: key,
                        priority: LINK_PRIORITY,
                        output: LinkValue::Fixed(Value::Float(scalar)),
                        stack: false,
                    },
                    depth,
                )?;
            }
        }
        let Some(run) = self.animation_run(id, node, alias) else {
            return Ok(());
        };
        let plays = match run {
            AnimationRun::Untimed | AnimationRun::Endless => true,
            AnimationRun::Timed { delay_ms, .. } => delay_ms > 0,
        };
        if plays {
            match expr::lvalue(field(&node.fields, "m_7CCEC4F0")) {
                Ok(Some(handle)) => self.write(
                    source_key(id, handle),
                    Value::Handle {
                        kind: ANIMATION_HANDLE,
                        instance: u32::from(id),
                        state: key.raw,
                        extra: 0,
                    },
                )?,
                Ok(None) => (),
                Err(error) => self.report(site, Issue::Expression(error)),
            }
        }
        self.arm_animation_timer(key, run)
    }

    /// Posts an `Anim` state's timer 1 and its finish.
    fn arm_animation_timer(&mut self, key: StateKey, run: AnimationRun) -> Result<(), InitFault> {
        if let AnimationRun::Timed {
            delay_ms,
            client_ends,
            ..
        } = run
        {
            self.post_after(key.instance, delay_ms, Event::Timer(key.raw, 1))?;
            if !client_ends {
                self.post_after(key.instance, delay_ms, Event::Finish(key.raw))?;
            }
        }
        Ok(())
    }

    /// How an `Anim` state's timer runs ([`Self::arm_animation`]); none
    /// when an evaluation failed (reported).
    fn animation_run(
        &mut self,
        id: u16,
        node: &Node,
        alias: Option<AnimAlias>,
    ) -> Option<AnimationRun> {
        let site = self.site(id, node);
        let no_timer = field(&node.fields, "m_37EE887D");
        if !no_timer.is_null() && truthy(&self.evaluate(id, node, no_timer, None)?) {
            return Some(AnimationRun::Untimed);
        }
        let authored = field(&node.fields, "m_86701C3E");
        let mut seconds = if authored.is_null() {
            0.0
        } else {
            expr::to_float(&self.evaluate(id, node, authored, None)?)
        };
        if seconds < MODEL_LENGTH_BELOW {
            let played = match alias {
                Some(played) => played,
                None => self.animation_alias(id, node)?,
            };
            let length = match played {
                AnimAlias::Zero => Some(0.0),
                AnimAlias::Played(alias, category) => {
                    self.world.animation_length(site, alias, category)
                }
                AnimAlias::Unknown => None,
            };
            match length {
                Some(length) => {
                    let speed = field(&node.fields, "m_speedScalar");
                    let speed = if speed.is_null() {
                        1.0
                    } else {
                        expr::to_float(&self.evaluate(id, node, speed, None)?)
                    };
                    seconds = f64::from((length * speed) as f32);
                }
                None if authored.is_null() => {
                    self.assume(site, &node.class, "never ends by itself");
                    return Some(AnimationRun::Endless);
                }
                None => (),
            }
        }
        let delay = ((seconds * 1000.0) as f32).round();
        let seconds = seconds as f32;
        if !delay.is_finite() {
            self.report(site, Issue::UnsupportedField("animation duration"));
            return None;
        }
        let removes_self = field(&node.fields, "m_154DBA91");
        let client_ends =
            !removes_self.is_null() && truthy(&self.evaluate(id, node, removes_self, None)?);
        Some(AnimationRun::Timed {
            seconds,
            delay_ms: delay.max(0.0) as u64,
            client_ends,
        })
    }

    /// `STU_6E18DEA0` (execute `0x7FF78981C830`, the client's winning
    /// registration; restart `0x7FF789BCEEE0`): restarts the `Anim` state
    /// whose handle `target` is ([`Self::arm_animation`]) when it still
    /// plays: begun over 1 ms ago and more than 1 ms before its end, which
    /// its play put its length less 1/30 s after it began playing (none
    /// without time to run), or at any time with its `m_2DEAE6A9`. The
    /// restart takes the state's length again and arms its timer 1 that
    /// far from now (with the finish, as its begin does); its begin plugs
    /// don't run again. Anything else (no handle, another state, an ended
    /// one) changes nothing.
    pub(super) fn restart_animation(&mut self, target: &Value) -> Result<(), InitFault> {
        let &Value::Handle {
            instance, state, ..
        } = target
        else {
            return Ok(());
        };
        let Ok(instance) = u16::try_from(instance) else {
            return Ok(());
        };
        let key = StateKey {
            instance,
            raw: state,
        };
        let Some(begun_ms) = self
            .state(key)
            .filter(|state| state.active && state.kind == StateKind::Anim)
            .map(|state| state.start_ms)
        else {
            return Ok(());
        };
        let Some(node) = self.graph(instance).state(usize::from(state)) else {
            return Ok(());
        };
        let looped = field(&node.fields, "m_2DEAE6A9");
        let looped = !looped.is_null()
            && self
                .evaluate(instance, node, looped, None)
                .is_some_and(|value| truthy(&value));
        let Some(run) = self.animation_run(instance, node, None) else {
            return Ok(());
        };
        let AnimationRun::Timed {
            seconds, delay_ms, ..
        } = run
        else {
            return Ok(());
        };
        let now = self.machine.now_ms;
        let timer = self
            .machine
            .queues
            .iter(instance)
            .find_map(|(due_ms, event)| {
                matches!(event, Event::Timer(raw, 1) if *raw == state).then_some(due_ms)
            });
        let end_ms = ((seconds - 1.0 / 30.0) * 1000.0).round();
        let playing = timer.is_some_and(|due_ms| {
            let end = due_ms.saturating_sub(delay_ms) as f64 + f64::from(end_ms);
            now > begun_ms + 1 && end_ms > 0.0 && (now as f64) < end - 1.0
        });
        if !(playing || looped) || delay_ms == 0 {
            return Ok(());
        }
        self.machine.queues.retain(
            instance,
            |_, event| !matches!(event, Event::Timer(raw, 1) | Event::Finish(raw) if *raw == state),
        );
        self.arm_animation_timer(key, run)
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
        if kind == StateKind::Shockwave {
            return self.update_shockwave(key);
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
        if kind == StateKind::ChaseVar && parameter == CHASE_ARRIVAL {
            self.chase_arrived(key, node)?;
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
        if self.create(child, graph, Some((id, key.raw)), vars, self.source_of(id))? {
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
