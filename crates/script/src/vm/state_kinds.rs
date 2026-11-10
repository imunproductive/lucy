//! Begin handlers for state kinds with native behavior: movement sampling, target tags, health pools, stacks, cosmetics and chase variables.
//!
//! Client research (1.74): Anim (OnBegin 0x7FF78981DD90): with
//! m_playOverDuration (m_86701C3E) set, a timer at start + d follows
//! m_A3B86BB0 (animation finished), and unless m_clientAnimRemovesSelf
//! (m_154DBA91) the state finishes at start + d; without it the duration is
//! the animation asset's. Effect: its payload is its activation counter & 3
//! (reader 0x7FF789894540). ClientOnlyPulser (vtable 0x7FF78B549DE8): OnEnter
//! and every begin while active (0x7FF78990D6D0) pulse once, and the client
//! follows m_B6A27F9A once per pulse (0x7FF78990D872). Its payload (reader
//! 0x7FF78990D760) is a bit and the 8-bit pulse count: a count that moved on
//! pulses the client as often (mod 256); a state read for the first time
//! pulses once only with the bit set (or the node's m_C85E2CBA, node +0xF0),
//! else it takes the count as seen. The original server's freshness policy is
//! unverified.

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn has_no_ai_control_target(&mut self, id: u16, node: &Node) -> bool {
        let site = self.site(id, node);
        node.is_networked()
            && node.remote == -1
            && expr::native_shape(field(&node.fields, "m_00D49978"), "STU_B5A0CAF0", None)
            && self
                .world
                .related_entity(site, EntityRelation::AiControlEntity, site.entity)
                == Some(0)
    }

    pub(super) fn update_movement_sample(
        &mut self,
        key: StateKey,
        node: &Node,
        previous: u16,
    ) -> Result<(), InitFault> {
        let site = self.site(key.instance, node);
        let Some(sample) = self
            .world
            .movement_sample(site)
            .filter(|&mask| mask < 1 << 13)
        else {
            self.report(site, Issue::UnsupportedField("owner movement sample"));
            return Ok(());
        };
        let changed = previous ^ sample;
        if changed == 0 {
            return Ok(());
        }
        self.state_mut(key).expect("active movement sample").kind =
            StateKind::MovementSample(sample);
        for (bit, name) in MOVEMENT_BOOLEAN_OUTPUTS.iter().enumerate() {
            let mask = 1 << bit;
            if changed & mask == 0 {
                continue;
            }
            if let Some(variable) =
                expr::lvalue(field(&node.fields, name)).expect("validated movement sample output")
            {
                self.write(
                    source_key(key.instance, variable),
                    Value::Bool(sample & mask != 0),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn begin_target_tag(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let site = self.site(key.instance, node);
        let add = field(&node.fields, "m_targetTags");
        let suppress = field(&node.fields, "m_44D1BE94");
        let (literal, present) = match (add.is_null(), suppress.is_null()) {
            (false, true) => (add, true),
            (true, false) => (suppress, false),
            _ => {
                self.report(site, Issue::UnsupportedField("target tag operands"));
                return Ok(());
            }
        };
        let Some(tag) = expr::target_tag_config(literal).filter(|&tag| tag != 0) else {
            self.report(site, Issue::UnsupportedField("target tag literal"));
            return Ok(());
        };
        let Some(Value::Entity(entity)) =
            self.evaluate(key.instance, node, field(&node.fields, "m_31714AFC"), None)
        else {
            self.report(site, Issue::UnsupportedField("target tag entity"));
            return Ok(());
        };
        let effect = TargetTagEffect {
            entity,
            tag,
            present,
        };
        if !self.world.set_target_tag_effect(site, effect, true) {
            self.report(site, Issue::UnsupportedField("target tag effect"));
            return Ok(());
        }
        self.state_mut(key).expect("active target tag").kind = StateKind::TargetTag {
            effect: Some(effect),
        };
        Ok(())
    }

    pub(super) fn begin_health_pool(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        if !matches!(
            self.evaluate(id, node, field(&node.fields, "m_FA3ECC43"), None),
            Some(Value::Entity(target)) if target == site.entity
        ) {
            self.report(site, Issue::UnsupportedField("health pool target"));
            return Ok(());
        }
        let Some(maximum) = self
            .evaluate(id, node, field(&node.fields, "m_amount"), Some(key))
            .and_then(pool_number)
        else {
            self.report(site, Issue::UnsupportedField("health pool capacity"));
            return Ok(());
        };
        let initial = field(&node.fields, "m_BCFDD2E9");
        let current = if initial.is_null() {
            maximum
        } else {
            let Some(current) = self.evaluate(id, node, initial, None).and_then(pool_number) else {
                self.report(site, Issue::UnsupportedField("health pool initial amount"));
                return Ok(());
            };
            current
        };
        let kind = pool_kind(field(&node.fields, "m_7E8B666C")).expect("validated pool kind");
        let Some(pool) = self.world.add_health_pool(site, kind, maximum, current) else {
            self.report(site, Issue::UnsupportedField("health pool registration"));
            return Ok(());
        };
        self.state_mut(key).expect("active health pool").kind =
            StateKind::HealthPool { pool: Some(pool) };
        self.register_tick(key, self.machine.now_ms);
        self.publish_health_pool(key, node, current)
    }

    pub(super) fn publish_health_pool(
        &mut self,
        key: StateKey,
        node: &Node,
        current: f64,
    ) -> Result<(), InitFault> {
        let output = expr::lvalue(field(&node.fields, "m_CEAB19E8"))
            .expect("validated health pool output")
            .expect("health pool output variable");
        let output = source_key(key.instance, output);
        let current = Value::Float(current);
        if !same(&self.machine.value(output, None), &current) {
            self.write(output, current)?;
        }
        self.publish_health_totals(self.site(key.instance, node))
    }

    pub(super) fn publish_health_totals(&mut self, site: Site) -> Result<(), InitFault> {
        for (variable, maximum) in [
            (OWNER_HEALTH, false),
            (HUD_HEALTH, false),
            (HUD_MAX_HEALTH, true),
        ] {
            if let Some(amount) = self.world.body_health(site, None, maximum) {
                let key = VarKey::Entity(variable);
                let value = Value::Float(amount);
                if !same(&self.machine.value(key, None), &value) {
                    self.write(key, value)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn begin_stack(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let Some(value) = self.evaluate(id, node, field(&node.fields, "m_value"), Some(key)) else {
            return Ok(());
        };
        let target = match expr::lvalue(field(&node.fields, "m_out_Var")) {
            Ok(Some(target)) => source_key(id, target),
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        let priority = match expr::stack_priority(field(&node.fields, "m_priority")) {
            Ok(priority) => priority,
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        self.state_mut(key).expect("active stack").linked_variable = Some(target);
        self.insert_link(
            target,
            VariableLink {
                state: key,
                priority,
                output: LinkValue::Fixed(value),
                stack: true,
            },
            depth,
        )
    }

    pub(super) fn begin_cosmetic(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let target = match expr::lvalue(field(&node.fields, "m_1DEF59FA")) {
            Ok(Some(target)) => source_key(key.instance, target),
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(self.site(key.instance, node), Issue::Expression(error));
                return Ok(());
            }
        };
        self.state_mut(key)
            .expect("active cosmetic")
            .linked_variable = Some(target);
        self.insert_link(
            target,
            VariableLink {
                state: key,
                priority: LINK_PRIORITY,
                stack: false,
                output: LinkValue::Fixed(Value::Handle {
                    kind: 1,
                    instance: u32::from(key.instance),
                    state: key.raw,
                    extra: 0,
                }),
            },
            depth,
        )
    }

    pub(super) fn begin_chase(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let target = match expr::lvalue(field(&node.fields, "m_out_Var")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        let initial = field(&node.fields, "m_EB0FC261");
        let current = if initial.is_null() {
            target.map_or(Value::Null, |target| self.machine.value(target, None))
        } else {
            let Some(value) = self.evaluate(id, node, initial, None) else {
                return Ok(());
            };
            value
        };
        let current = chase_value(&current);
        let remaining_ms = if configured(node, "m_5BEFF040") {
            let Some(value) = self.evaluate(id, node, field(&node.fields, "m_5BEFF040"), None)
            else {
                return Ok(());
            };
            Some((expr::to_float(&value) * 1000.0) as i64)
        } else {
            None
        };
        let now = self.machine.now_ms;
        let state = self.state_mut(key).expect("active chase");
        state.chase = Some(ChasePayload {
            current: current.clone(),
            reached: false,
            remaining_ms,
            last_ms: now,
        });
        state.linked_variable = target;
        if let Some(target) = target {
            let old = self.machine.value(target, None);
            self.set_base(target, current);
            self.insert_link(
                target,
                VariableLink {
                    state: key,
                    priority: LINK_PRIORITY,
                    output: LinkValue::Chase,
                    stack: false,
                },
                depth,
            )?;
            self.notify_value(target, &old, None)?;
        }
        self.register_tick(key, now);
        Ok(())
    }

    pub(super) fn tick_chase(&mut self, key: StateKey) -> Result<(), InitFault> {
        let id = key.instance;
        let now = self.machine.now_ms;
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        let target = state.linked_variable;
        let Some(chase) = state.chase.as_mut() else {
            return Ok(());
        };
        let elapsed = (now - chase.last_ms) as i64;
        chase.last_ms = now;
        if elapsed == 0 || chase.reached {
            return Ok(());
        }
        let mut next = chase.clone();
        let before = target.map(|target| self.machine.value(target, None));
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("chase state");
        let Some(destination) =
            self.evaluate(id, node, field(&node.fields, "m_destination"), Some(key))
        else {
            return Ok(());
        };
        let destination = chase_value(&destination);
        if let Some(remaining) = next.remaining_ms {
            if remaining <= elapsed {
                next.current = destination;
                next.reached = true;
            } else {
                let share = f64::from((elapsed as f64 / remaining as f64) as f32);
                next.current = chase_lerp(next.current, destination, share);
                next.remaining_ms = Some(remaining - elapsed);
            }
        } else {
            let rate = field(&node.fields, "m_9B0385B4");
            let rate = if rate.is_null() {
                1.0
            } else {
                let Some(value) = self.evaluate(id, node, rate, Some(key)) else {
                    return Ok(());
                };
                expr::to_float(&value)
            };
            if rate > 0.0 {
                let per_ms = (rate * f64::from(0.001_f32)) as f32;
                let distance = (f64::from(per_ms) * elapsed as f64) as f32;
                let step = f64::from((f64::from(distance) + expr::EPSILON) as f32);
                (next.current, next.reached) = chase_step(next.current, destination, step);
            }
        }
        let reached = next.reached;
        self.state_mut(key).expect("active chase").chase = Some(next);
        if let (Some(target), Some(before)) = (target, before) {
            self.notify_value(target, &before, None)?
        }
        if reached {
            self.post(id, Event::ChaseReached(key.raw))?;
            if !configured(node, "m_CB671004") {
                self.post(id, Event::Finish(key.raw))?
            }
        }
        Ok(())
    }

    pub(super) fn chase_write(&mut self, key: StateKey, target: VarKey) -> Result<(), InitFault> {
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("chase state");
        match field(&node.fields, "m_1E10B2B1").as_i64().unwrap_or(0) {
            0 => self.unlink_single_output(key, self.output_write_depth)?,
            1 => {
                let value = chase_value(&stored(&self.machine.snapshot, target));
                if let Some(chase) = self.state_mut(key).and_then(|state| state.chase.as_mut()) {
                    chase.current = value;
                }
            }
            _ => (),
        }
        Ok(())
    }

    pub(super) fn finish_chase(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        let Some(target) = state.linked_variable.take() else {
            return Ok(());
        };
        let current = state.chase.as_ref().map(|chase| chase.current.clone());
        self.remove_link(key, target, current, depth)
    }

    /// `ClientOnlyPulser.pulse` in `ow174/game/script/nodes.py`: one more
    /// pulse (mod 256) at its begin and at every begin while active. A pulse
    /// after the instance's first frame is fresh for its payload.
    pub(super) fn pulse(&mut self, key: StateKey) {
        let now = self.machine.now_ms;
        let fresh =
            now / FRAME_MS > self.machine.snapshot.instances[&key.instance].start_ms / FRAME_MS;
        let state = self.state_mut(key).expect("active pulser");
        if let StateKind::Pulser { pulses, pulsed_ms } = state.kind {
            state.kind = StateKind::Pulser {
                pulses: pulses.wrapping_add(1),
                pulsed_ms: if fresh { Some(now) } else { pulsed_ms },
            };
        }
    }

    /// `MoveTo.on_begin`: only a fixed world target with a speed resolves a
    /// request, and only one the movement record can carry.
    pub(super) fn begin_move_to(&mut self, key: StateKey, node: &Node) {
        self.machine.movements.remove(&key);
        let fixed = match field(&node.fields, "m_B060F688") {
            Json::Number(mode) => mode.as_f64() == Some(0.0),
            Json::Bool(mode) => !mode,
            _ => false,
        };
        let speed = field(&node.fields, "m_speed");
        if !fixed
            || speed.is_null()
            || ["m_destinationEntity", "m_destinationBone", "m_5BEFF040"]
                .iter()
                .any(|name| !field(&node.fields, name).is_null())
        {
            return;
        }
        let id = key.instance;
        let Some(destination) = self.evaluate(id, node, field(&node.fields, "m_865F3D01"), None)
        else {
            return;
        };
        let Some(speed) = self.evaluate(id, node, speed, None) else {
            return;
        };
        let speed = match speed {
            Value::Integer(speed) => speed as f64,
            Value::Float(speed) => speed,
            _ => return,
        };
        let Value::Vector(target) = destination else {
            return;
        };
        if speed > 0.0
            && speed <= MAX_MOVEMENT_AXIS
            && target.iter().all(|axis| axis.abs() <= MAX_MOVEMENT_AXIS)
        {
            self.machine.movements.insert(key, (target, speed));
        }
    }
}
