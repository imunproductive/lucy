//! The communication wheel's PlayScript actions and the networked actions
//! its graphs send the clients.
//!
//! Ported from leafall903/ORPHEUS (MIT): `play.resolve_request`,
//! `play_queue.Queue` and `Instance.start_entries` (906E49..9070D0,
//! 64D2C0/64CD10, 1B95950), `BodyWorld.play_script` and
//! `BodyWorld.replicate_wheel_event` in `driver.py`. Only the wheel's own
//! self-targeted PlayScript nodes ([`wheel::PLAY_SCRIPTS`]) run; every other
//! PlayScript stays a no-op, as in the Python reference.
//!
//! A request needs a target that is the body itself. Its parameters are
//! always arrays (1BA7C60). Policy (`m_AA4DE426`) 0 keeps a running instance
//! of the graph and does nothing, 2 reuses it, 3 replaces it; otherwise a
//! new root instance is made. Then each start entry (`STU_BB4AC733`) takes
//! the parameters named in its rows and the caller in `m_B60ABDA4`, and runs.

use super::*;

impl<W: World> Vm<'_, '_, '_, W> {
    pub(super) fn play_wheel_script(
        &mut self,
        id: u16,
        node: &Node,
        graph: u32,
    ) -> Result<(), InitFault> {
        let resource = node.field("m_script.m_graph").and_then(asset_guid);
        if resource != Some(0x0580 << 48 | u64::from(graph)) {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("wheel PlayScript resource"),
            );
            return Ok(());
        }
        let entity = self.machine.snapshot.entity;
        let Some(targets) = self.evaluate(id, node, field(&node.fields, "m_targets"), None) else {
            return Ok(());
        };
        let target_self = match targets {
            Value::Entity(target) => target == entity,
            Value::Array(items) => items == [Value::Entity(entity)],
            _ => false,
        };
        if !target_self {
            return Ok(());
        }
        let mut parameters = Vec::new();
        for row in array_field(&node.fields, "m_params") {
            let Some(key) = row.get("m_B5051BCE").and_then(asset_guid) else {
                continue;
            };
            let value = match field(row, "m_value") {
                Json::Null => Value::Array(Vec::new()),
                config => match self.evaluate_value(id, node, config, None) {
                    Some(Value::Array(items)) => Value::Array(items),
                    Some(Value::Null) => Value::Array(Vec::new()),
                    Some(value) => Value::Array(vec![value]),
                    None => return Ok(()),
                },
            };
            parameters.push((key, value));
        }
        let policy = node
            .fields
            .get("m_AA4DE426")
            .and_then(Json::as_i64)
            .unwrap_or(0);
        let running: Vec<u16> = self
            .machine
            .snapshot
            .instances
            .iter()
            .filter(|(_, instance)| instance.graph == graph)
            .map(|(&running, _)| running)
            .collect();
        let target = match (policy, running.first()) {
            (0, Some(_)) => return Ok(()),
            (2, Some(&running)) => running,
            _ => {
                if policy == 3 {
                    for running in running {
                        self.destroy(running)?;
                    }
                }
                let created = u16::try_from(self.machine.next_id)
                    .map_err(|_| InitFault::InstanceIdExhausted)?;
                if !self.create(created, graph, None, Variables::new())? {
                    return Ok(());
                }
                created
            }
        };
        self.start_entries(target, &parameters, entity)
    }

    /// `Instance.start_entries`: bind each start entry's named parameters,
    /// then queue it.
    fn start_entries(
        &mut self,
        id: u16,
        parameters: &[(u64, Value)],
        caller: u32,
    ) -> Result<(), InitFault> {
        let graph = self.graph(id);
        for &position in graph.entries.iter().flatten() {
            let Some(entry) = graph.node(position) else {
                continue;
            };
            if entry.client_only || entry.class != "STU_BB4AC733" {
                continue;
            }
            let disabled = field(&entry.fields, "m_5AEA0A51");
            if disabled.as_bool() == Some(true) || disabled.as_i64().is_some_and(|flag| flag != 0) {
                continue;
            }
            for (key, value) in parameters {
                let row = array_field(&entry.fields, "m_params").iter().find(|row| {
                    row.get("m_B5051BCE").and_then(asset_guid) == Some(*key)
                        && !field(row, "m_out_Var").is_null()
                });
                if let Some(row) = row {
                    self.bind_start_value(id, entry, field(row, "m_out_Var"), value.clone())?;
                }
            }
            let source = field(&entry.fields, "m_B60ABDA4");
            if !source.is_null() && expr::valid_entity(caller) {
                self.bind_start_value(id, entry, source, Value::Entity(caller))?;
            }
            self.post(id, Event::Entry(position))?;
        }
        Ok(())
    }

    fn bind_start_value(
        &mut self,
        id: u16,
        entry: &Node,
        target: &Json,
        value: Value,
    ) -> Result<(), InitFault> {
        match expr::lvalue(target) {
            Ok(Some(target)) => self.write(source_key(id, target), value),
            Ok(None) => Ok(()),
            Err(error) => {
                self.report(self.site(id, entry), Issue::Expression(error));
                Ok(())
            }
        }
    }

    /// A networked wheel action ran: keep it for the clients' frames.
    pub(super) fn record_frame_action(&mut self, id: u16, node: &Node) {
        let instance = &self.machine.snapshot.instances[&id];
        if node.remote < 0 || !wheel::ACTION_GRAPHS.contains(&instance.graph) {
            return;
        }
        let actions = &mut self.machine.snapshot.frame_actions;
        let serial = actions.last().map_or(1, |last| last.serial.wrapping_add(1));
        actions.push(FrameAction {
            serial,
            instance: id,
            instance_start_ms: instance.start_ms,
            node: node.pos,
            time_ms: self.machine.now_ms,
        });
        if actions.len() > MAX_FRAME_ACTIONS {
            actions.remove(0);
        }
    }
}
