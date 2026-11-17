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

    /// The movement sampler (`STU_96F1DBCF`): its outputs follow the owner's
    /// 13 movement bits. On a tick (`edges`) a change also follows its two
    /// plugs, which only Doomfist (graph `0D06`) and Baptiste (`1983`) link:
    /// `m_2102C541` when the body becomes grounded (bit 1; Doomfist's plug
    /// sets his copy of the grounded variable `0221`) and `m_E37BC6C2` when it
    /// becomes airborne (bit 0; Baptiste's checks the jump button for the Exo
    /// Boots). Which edge each plug takes is an assumption from those uses.
    pub(super) fn update_movement_sample(
        &mut self,
        key: StateKey,
        node: &Node,
        previous: u16,
        edges: bool,
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
        if edges {
            for (mask, plug) in [(2, "m_2102C541"), (1, "m_E37BC6C2")] {
                if changed & sample & mask != 0 {
                    self.follow(key.instance, node, plug, Some(key), 0)?;
                }
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
        let Some(remaining_ms) = self.chase_duration(id, node) else {
            return Ok(());
        };
        let Some(timed) = self.timed_chase(id, node) else {
            return Ok(());
        };
        let now = self.machine.now_ms;
        let state = self.state_mut(key).expect("active chase");
        state.chase = Some(ChasePayload {
            current: current.clone(),
            reached: false,
            remaining_ms,
            last_ms: now,
            timed,
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
        if !self.arm_chase_arrival(key)? {
            self.register_tick(key, now);
        }
        Ok(())
    }

    /// The chase's duration in ms (`m_5BEFF040`), if it has one; None: the
    /// config failed to evaluate (reported).
    fn chase_duration(&mut self, id: u16, node: &Node) -> Option<Option<i64>> {
        if !configured(node, "m_5BEFF040") {
            return Some(None);
        }
        let value = self.evaluate(id, node, field(&node.fields, "m_5BEFF040"), None)?;
        Some(Some((expr::to_float(&value) * 1000.0) as i64))
    }

    /// `0x7FF78AAB0400`'s test for a [`TimedChase`]: no destination bone or
    /// object, the destination and the speed absent or a constant
    /// (`STUConfigVarInt` or `STUConfigVarFloat`, `0x7FF78A3D4380`,
    /// `0x7FF78A3D4350`), and the speed above 1.2e-5; else the chase steps
    /// each frame. None: a config that failed to evaluate (reported).
    fn timed_chase(&mut self, id: u16, node: &Node) -> Option<Option<TimedChase>> {
        let constant = |name| match field(&node.fields, name) {
            Json::Null | Json::Number(_) => true,
            config => matches!(
                config.get("$").and_then(Json::as_str),
                Some("STUConfigVarInt" | "STUConfigVarFloat")
            ),
        };
        let present = |name| !field(&node.fields, name).is_null();
        if present("m_destinationBone")
            || present("m_destinationObject")
            || !constant("m_destination")
            || !constant("m_9B0385B4")
        {
            return Some(None);
        }
        let rate = if present("m_9B0385B4") {
            let value = self.evaluate(id, node, field(&node.fields, "m_9B0385B4"), None)?;
            expr::to_float(&value) as f32
        } else {
            1.0
        };
        if f64::from(rate) <= expr::EPSILON {
            return Some(None);
        }
        let destination = self.evaluate(id, node, field(&node.fields, "m_destination"), None)?;
        Some(Some(TimedChase {
            destination: chase_value(&destination),
            rate: f64::from(rate),
        }))
    }

    /// A timed chase's one timer, from where it is now (`0x7FF78AAB0400`
    /// posts it at the begin and when a write restarts the chase); false for
    /// a chase that steps each frame.
    fn arm_chase_arrival(&mut self, key: StateKey) -> Result<bool, InitFault> {
        let Some(delay) = self
            .state(key)
            .and_then(|state| state.chase.as_ref())
            .and_then(ChasePayload::arrival_ms)
        else {
            return Ok(false);
        };
        self.machine.queues.retain(
            key.instance,
            |_, event| !matches!(event, Event::Timer(raw, CHASE_ARRIVAL) if *raw == key.raw),
        );
        self.post_after(key.instance, delay, Event::Timer(key.raw, CHASE_ARRIVAL))?;
        Ok(true)
    }

    /// A timed chase's timer: it is at its destination now (event 0 of
    /// `0x7FF78AAA7F50`, `0x7FF78AAB0960`), and finishes unless it keeps
    /// running (`m_CB671004`).
    pub(super) fn chase_arrived(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let now = self.machine.now_ms;
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        let target = state.linked_variable;
        let Some(chase) = state.chase.as_mut().filter(|chase| !chase.reached) else {
            return Ok(());
        };
        let Some(timed) = chase.timed.clone() else {
            return Ok(());
        };
        // What the variable's watchers last heard: a timed chase tells
        // them nothing on the way.
        let before = std::mem::replace(&mut chase.current, timed.destination);
        chase.reached = true;
        chase.last_ms = now;
        if chase.remaining_ms.is_some() {
            chase.remaining_ms = Some(0);
        }
        if let Some(target) = target {
            self.notify_value(target, &before, None)?;
        }
        self.post(key.instance, Event::ChaseReached(key.raw))?;
        if !configured(node, "m_CB671004") {
            self.post(key.instance, Event::Finish(key.raw))?;
        }
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
        let now = self.machine.now_ms;
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        let Some(target) = state.linked_variable.take() else {
            return Ok(());
        };
        let current = state.chase.as_ref().map(|chase| chase.value_at(now));
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

    /// A config number, or `None` (reported by the evaluator) when it isn't one.
    fn number(&mut self, id: u16, node: &Node, name: &str) -> Option<f64> {
        match self.evaluate(id, node, field(&node.fields, name), None)? {
            Value::Integer(value) => Some(value as f64),
            Value::Float(value) => Some(value),
            _ => {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedField("movement number"),
                );
                None
            }
        }
    }

    /// Tracer's Blink (`STU_A807207A`, graph `0024` node 3): a dash for the
    /// mover. Of its eleven unnamed configs the server reads two: the
    /// distance `m_6A79BD98` (7.5, the public 7.5 m; Shockwave names its
    /// range the same) and the time `m_162A28FA` (0.1). Assumed, unconfirmed
    /// by the client: the rest (0.7, 1, 1.5, 1, 1, 1, 1 and two conditions on
    /// entity variables `7270`/`7271`) don't change the move. False when a
    /// value isn't a number.
    pub(super) fn dash(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let (Some(distance), Some(seconds)) = (
            self.number(id, node, "m_6A79BD98"),
            self.number(id, node, "m_162A28FA"),
        ) else {
            return Ok(false);
        };
        self.assume(
            self.site(id, node),
            &node.class,
            "dash m_6A79BD98 m along the input over m_162A28FA s",
        );
        self.machine
            .motions
            .push(Motion::Dash { distance, seconds });
        Ok(true)
    }

    /// A velocity action (`STU_FF3471E9`, run 0x7FF789C3B350): each element
    /// of `m_BC5E91CF`, in order, for the body's next pose
    /// ([`Motion::Velocity`], `orpheus_physics::mover::Mover::act_on_velocity`).
    /// A scalar element (`STU_D71F17E2`: add `7104FC1F`, multiply
    /// `1308AA6A`, set `9AF0FA80`, min `748FC65A`, max `EC4841E7`; the clamp
    /// `20E18047` with `m_8AD739CE` lower, default the smallest normal
    /// float, and `m_8213EB22` upper, default the largest) is movement mod
    /// 32 + its channel `m_A9561CA0`; a channel above 3 does nothing
    /// (0x7FF789B42860). A vector element (`STU_9C74F0E2`: add `4635C5E2`,
    /// multiply `D93B3F5E` (unset value 1, 1, 1), set `95A10547`) acts on
    /// channel `m_CFEC5F76` 1..=4; channel 0 starts a mover behaviour that
    /// isn't modelled (no data uses it): refused. Unset values and
    /// priorities (`m_priority`) are 0. False when a value can't be read.
    pub(super) fn velocity_action(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let mut elements = Vec::new();
        for config in array_field(&node.fields, "m_BC5E91CF") {
            self.spend("velocity element")?;
            let shape = config.get("$").and_then(Json::as_str).unwrap_or("");
            let Some(priority) = self.movement_number(id, node, field(config, "m_priority"), 0.0)
            else {
                return Ok(false);
            };
            let scalar = match shape {
                "STU_7104FC1F" => Some(Operation::Add),
                "STU_1308AA6A" => Some(Operation::Multiply),
                "STU_9AF0FA80" => Some(Operation::Set),
                "STU_748FC65A" => Some(Operation::Minimum),
                "STU_EC4841E7" => Some(Operation::Maximum),
                "STU_20E18047" => Some(Operation::Clamp { upper: 0.0 }),
                _ => None,
            };
            if let Some(mut operation) = scalar {
                let (name, default) = match operation {
                    Operation::Clamp { .. } => ("m_8AD739CE", f64::from(f32::MIN_POSITIVE)),
                    _ => ("m_9B7A63EA", 0.0),
                };
                let Some(value) = self.movement_number(id, node, field(config, name), default)
                else {
                    return Ok(false);
                };
                if let Operation::Clamp { upper } = &mut operation {
                    let config = field(config, "m_8213EB22");
                    let Some(bound) = self.movement_number(id, node, config, f64::from(f32::MAX))
                    else {
                        return Ok(false);
                    };
                    *upper = bound;
                }
                let channel = config.get("m_A9561CA0").and_then(Json::as_u64).unwrap_or(0);
                if channel <= 3 {
                    elements.push(VelocityElement::Modifier(Modifier {
                        index: 32 + channel as u8,
                        operation,
                        value,
                        priority,
                    }));
                }
                continue;
            }
            let (operation, default) = match shape {
                "STU_4635C5E2" => (VectorOperation::Add, [0.0; 3]),
                "STU_D93B3F5E" => (VectorOperation::Multiply, [1.0; 3]),
                "STU_95A10547" => (VectorOperation::Set, [0.0; 3]),
                _ => {
                    self.report(
                        self.site(id, node),
                        Issue::UnsupportedField("velocity element"),
                    );
                    return Ok(false);
                }
            };
            let channel = config.get("m_CFEC5F76").and_then(Json::as_u64).unwrap_or(0);
            if channel == 0 {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedField("velocity channel 0 (mover behaviour)"),
                );
                return Ok(false);
            }
            let value = field(config, "m_9B7A63EA");
            let value = if value.is_null() {
                default
            } else {
                match self.evaluate(id, node, value, None) {
                    Some(Value::Vector(value)) => value.map(|part| f64::from(part as f32)),
                    Some(_) => {
                        self.report(
                            self.site(id, node),
                            Issue::UnsupportedField("velocity vector"),
                        );
                        return Ok(false);
                    }
                    None => return Ok(false),
                }
            };
            if channel <= 4 {
                elements.push(VelocityElement::Vector {
                    channel: channel as u8,
                    operation,
                    value,
                    priority,
                });
            }
        }
        self.machine
            .motions
            .extend(elements.into_iter().map(Motion::Velocity));
        Ok(true)
    }

    /// Recall's rewind (`STU_41B3FC2B`, graph `001A` state 0): the mover goes
    /// back `m_6B5DA460` seconds of path (variable `1BD5`, 3) in
    /// `m_FAF3B2D3` seconds (variable `00D7`, 0.9), and the state finishes
    /// then, the public 3 s and 0.9 s. Its other configs (0.1, 0.1, variable
    /// `72A3`, 3) are assumed not to change the move.
    pub(super) fn begin_rewind(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let (Some(seconds), Some(duration)) = (
            self.number(id, node, "m_6B5DA460"),
            self.number(id, node, "m_FAF3B2D3"),
        ) else {
            return Ok(());
        };
        self.assume(
            self.site(id, node),
            &node.class,
            "rewind m_6B5DA460 s of path over m_FAF3B2D3 s",
        );
        self.machine
            .motions
            .push(Motion::Rewind { seconds, duration });
        let delay = (duration.max(0.0) * 1000.0).round() as u64;
        self.post_after(id, delay, Event::Finish(key.raw))
    }

    /// Recall's wind-up container (`STU_54FEB21C`, graph `001A` state 3)
    /// ends only once its variable `m_C7A42E2A` (instance variable `018B`)
    /// is true, and no graph writes it (ow1-preservation, `HANDOFF.md:397-401`):
    /// the server does, at once, so the rewind starts with the press, as
    /// Recall's public 0.9 s and the live-tested server Recall (rewind from
    /// the press, `HANDOFF.md:1426`) have it. When the client ends it is
    /// unverified.
    pub(super) fn end_wind_up(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        match expr::lvalue(field(&node.fields, "m_C7A42E2A")) {
            Ok(Some(target)) => self.write(source_key(id, target), Value::Bool(true))?,
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(());
            }
        }
        self.assume(site, &node.class, "the server ends it at once");
        self.post(id, Event::Finish(key.raw))
    }
}
