//! Weapon volleys and their shot events.
//!
//! Client research (PC 1.74): shot k leaves the gun at volley start + `R((k /
//! rate) * 1000)` ms in float32 (0x7FF789C702E0); n = min(ammo / per shot,
//! max shots); a finish timer (param 2) at the last shot's time and a
//! minimum-shots timer (param 1). While active m_out_Ammo reads
//! `max(0, ammo0 - perShot * (shots + 1))`; at the end the variable keeps the
//! value one ms later (the +1 ms rule, 0x7FF78A07F310). The outputs are
//! volatile bindings (StartFiring 0x7FF789C6E160 binds all three with the
//! flag `+0x29`): a switch that reads one re-checks each update instead of
//! waiting for a change notice (`InstanceContext::read`).

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn volley_config(
        &mut self,
        id: u16,
        node: &Node,
        name: &str,
        default: Value,
    ) -> Option<Value> {
        let config = field(&node.fields, name);
        if config.is_null() {
            Some(default)
        } else {
            self.evaluate(id, node, config, None)
        }
    }

    pub(super) fn begin_volley(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let names = [
            "m_A17BE89B",
            "m_A55BC904",
            "m_5F40D9D3",
            "m_numShotsPerSecond",
        ];
        let mut config = [
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(1),
            Value::Float(1.0),
        ];
        for (value, name) in config.iter_mut().zip(names) {
            let Some(found) = self.volley_config(id, node, name, value.clone()) else {
                return Ok(());
            };
            *value = found;
        }
        let [max, min, per, rate] = config;
        let (Ok(maximum), Ok(minimum)) = (
            u32::try_from(expr::to_int(&max, 0).max(0)),
            u32::try_from(expr::to_int(&min, 0).max(0)),
        ) else {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("weapon volley shot count"),
            );
            return Ok(());
        };
        let per_shot = expr::to_int(&per, 0).max(0);
        let rate = if rate == Value::Null {
            1.0
        } else {
            expr::to_float(&rate).max(1e-5)
        };
        if !rate.is_finite() {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("weapon volley rate"),
            );
            return Ok(());
        }
        let mut outputs = [None; 3];
        for (output, name) in outputs
            .iter_mut()
            .zip(["m_24E351A4", "m_17770E7E", "m_3FFC9EB9"])
        {
            let cfg = field(&node.fields, name);
            if cfg.is_null() {
                continue;
            }
            *output = match expr::lvalue(cfg) {
                Ok(Some(var)) => Some(var),
                Ok(None) => {
                    self.report(
                        self.site(id, node),
                        Issue::UnsupportedField("weapon volley output"),
                    );
                    return Ok(());
                }
                Err(error) => {
                    self.report(self.site(id, node), Issue::Expression(error));
                    return Ok(());
                }
            };
        }
        let [ammo_var, _, _] = outputs;
        let outputs = outputs.map(|output| output.map(|var| source_key(id, var)));
        for (index, output) in outputs.iter().enumerate() {
            if output.is_some() && outputs[..index].contains(output) {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedField("duplicate weapon volley output"),
                );
                return Ok(());
            }
        }
        let ammo0 = outputs[0].map_or_else(
            || i64::from(maximum).saturating_mul(per_shot),
            |var| expr::to_int(&self.machine.value(var, None), 0).max(0),
        );
        let spread0 = outputs[2].map_or(0.0, |var| {
            expr::to_float(&self.machine.value(var, None)).clamp(0.0, 1.0)
        });
        if !spread0.is_finite() {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("weapon volley spread"),
            );
            return Ok(());
        }
        // The spread ramp of launch `STU_239B571E` without `m_23C72C36`,
        // baked when firing starts (leafall903/ORPHEUS (MIT) `nodes.py`
        // `WeaponVolley.params`, commit e5463e3).
        let launch = field(&node.fields, "m_5EC1EB38");
        let ramp = if field(launch, "$").as_str() == Some("STU_239B571E")
            && field(launch, "m_23C72C36").is_null()
        {
            let config = field(launch, "m_AC20B1C1");
            if config.is_null() {
                Some(0.0)
            } else {
                let Some(ramp) = self.evaluate(id, node, config, None) else {
                    return Ok(());
                };
                Some(expr::to_float(&ramp) as f32)
            }
        } else {
            None
        };
        let planned = if per_shot == 0 {
            maximum
        } else {
            (ammo0 / per_shot).min(i64::from(maximum)) as u32
        };
        let start_ms = self.machine.now_ms;
        self.state_mut(key).expect("active volley").kind = StateKind::WeaponVolley {
            volleys: 1,
            offset_ms: 0,
            ammo0,
            ammo_var,
        };
        let mut volley = Volley {
            start_ms,
            ammo0,
            per_shot,
            rate,
            spread0,
            ramp,
            spread: 0.0,
            planned,
            reported: 0,
            outputs,
        };
        volley.spread = volley.spread_at(start_ms);
        // The spread output is re-evaluated each tick until it is full, so
        // what watches it (the owner's reticle) follows the shots.
        if outputs[2].is_some() && ramp.is_some() && volley.spread < 1.0 {
            self.register_tick(key, start_ms);
        }
        self.machine.volleys.insert(key, volley);
        for (slot, (target, base)) in outputs
            .into_iter()
            .zip([
                Value::Integer(ammo0),
                Value::Float(rate),
                Value::Float(spread0),
            ])
            .enumerate()
        {
            let Some(target) = target else { continue };
            let before = self.machine.value(target, None);
            self.set_base(target, base);
            self.insert_link(
                target,
                VariableLink {
                    state: key,
                    priority: LINK_PRIORITY,
                    stack: false,
                    output: LinkValue::Volley(slot as u8),
                },
                depth,
            )?;
            self.notify_value(target, &before, None)?;
        }
        if per_shot == 0 || i64::from(minimum) < ammo0 / per_shot {
            self.post_after(
                id,
                shot_delay(minimum.saturating_sub(1), rate),
                Event::Timer(key.raw, 1),
            )?;
        }
        self.post_after(
            id,
            shot_delay(planned.saturating_sub(1), rate),
            Event::Timer(key.raw, 2),
        )
    }

    pub(super) fn report_active_volleys(&mut self) -> Result<(), InitFault> {
        let active: Vec<_> = self
            .machine
            .snapshot
            .instances
            .iter()
            .flat_map(|(&id, instance)| {
                instance.states.iter().filter_map(move |(&raw, state)| {
                    (state.active && matches!(state.kind, StateKind::WeaponVolley { .. }))
                        .then_some(StateKey { instance: id, raw })
                })
            })
            .collect();
        for key in active {
            self.report_volley(key)?
        }
        Ok(())
    }

    pub(super) fn report_volley(&mut self, key: StateKey) -> Result<(), InitFault> {
        if self.machine.owner_dead {
            return Ok(());
        }
        let Some(activation) = self.state(key).map(|state| state.activation) else {
            return Ok(());
        };
        let instance = &self.machine.snapshot.instances[&key.instance];
        let Some(volley) = self.machine.volleys.get_mut(&key) else {
            return Ok(());
        };
        while volley.reported < volley.planned {
            let time_ms = volley
                .start_ms
                .saturating_add(shot_delay(volley.reported, volley.rate));
            if time_ms > self.machine.now_ms {
                break;
            }
            if self.machine.shots.len() == MAX_PENDING_SHOTS {
                return Err(InitFault::PendingShots);
            }
            self.machine.shots.push(ShotEvent {
                entity: self.machine.snapshot.entity,
                instance: key.instance,
                graph: instance.graph,
                raw_state: key.raw,
                frame: (time_ms / FRAME_MS) as u32,
                time_ms,
                activation,
                activation_ms: volley.start_ms,
                ordinal: volley.reported,
                spread: volley.ramp.map(|_| volley.spread_at(time_ms)),
            });
            volley.reported += 1;
        }
        Ok(())
    }

    pub(super) fn finish_volley(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        self.report_volley(key)?;
        let Some(volley) = self.machine.volleys.get(&key) else {
            return Ok(());
        };
        let outputs = volley.outputs;
        let at = self.machine.now_ms.saturating_add(1);
        let values = [
            volley.output(0, at),
            volley.output(1, at),
            Value::Float(volley.spread_at(at)),
        ];
        for (target, value) in outputs.into_iter().zip(values) {
            if let Some(target) = target {
                self.remove_link(key, target, Some(value), depth)?
            }
        }
        self.machine.volleys.remove(&key);
        Ok(())
    }

    /// A ticking volley re-evaluates its spread; a change is published on
    /// the linked variable as an ordinary change, until the spread is full
    /// (leafall903/ORPHEUS (MIT) `nodes.py` `WeaponVolley.tick`, commit
    /// e5463e3: Wrecking Ball's reticle reads 0.4° at 4 shots, 2° at 20).
    pub(super) fn tick_volley(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(volley) = self.machine.volleys.get(&key) else {
            return Ok(());
        };
        let spread = volley.spread_at(self.machine.now_ms);
        if spread == volley.spread {
            return Ok(());
        }
        if let Some(target) = volley.outputs[2] {
            let before = self.machine.value(target, None);
            self.machine
                .volleys
                .get_mut(&key)
                .expect("ticking volley")
                .spread = spread;
            self.notify_value(target, &before, None)?;
        }
        if spread >= 1.0 {
            self.clear_tick(key);
        }
        Ok(())
    }
}
