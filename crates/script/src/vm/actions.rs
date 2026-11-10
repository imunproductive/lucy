//! Conditions' branches, `CycleIndex` and `StopAbility`.

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    /// A condition's result (`branch` in `ow174/game/script/nodes.py`): a
    /// false result is taken when a false link is; a true one when a true
    /// link is, or when neither plug has links.
    pub(super) fn branch(
        &mut self,
        id: u16,
        node: &Node,
        caller: Option<StateKey>,
        result: bool,
        depth: usize,
    ) -> Result<bool, InitFault> {
        if !result {
            return self.follow(id, node, "m_falsePlug", caller, depth);
        }
        Ok(self.follow(id, node, "m_truePlug", caller, depth)?
            || (node.links("m_truePlug").next().is_none()
                && node.links("m_falsePlug").next().is_none()))
    }

    /// `CycleIndex` (`cycle_index` in nodes.py): from the current index step
    /// forward or back through `[0, count)` and keep the first candidate whose
    /// condition holds; the condition reads the candidate as every
    /// parameter. None holds: the fail index, or -1.
    pub(super) fn cycle_index(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(count) = self.evaluate(id, node, field(&node.fields, "m_D10618D1"), None) else {
            return Ok(false);
        };
        let count = expr::to_int(&count, 0);
        let target = match expr::lvalue(field(&node.fields, "m_339A63DB")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
        };
        let Some(backwards) = self.evaluate(id, node, field(&node.fields, "m_C78954CB"), None)
        else {
            return Ok(false);
        };
        let step = if truthy(&backwards) { -1 } else { 1 };
        let fail = field(&node.fields, "m_A53FE9DA");
        let mut result = if fail.is_null() {
            -1
        } else {
            let Some(fail) = self.evaluate(id, node, fail, None) else {
                return Ok(false);
            };
            expr::to_int(&fail, 0)
        };
        if let Some(target) = target.filter(|_| count > 0) {
            let current = expr::to_int(&self.machine.value(target, None), 0).clamp(0, count - 1);
            for offset in 1..=count {
                self.spend("cycle index candidate")?;
                let candidate = current.wrapping_add(step * offset).rem_euclid(count);
                let outer = self.candidate.replace(Value::Integer(candidate));
                let holds = self.evaluate(id, node, field(&node.fields, "m_condition"), None);
                self.candidate = outer;
                let Some(holds) = holds else {
                    return Ok(false);
                };
                if truthy(&holds) {
                    result = candidate;
                    break;
                }
            }
        }
        if let Some(target) = target {
            self.write(target, Value::Integer(result))?;
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `StopAbility` (`stop_ability` in nodes.py): every active ability of
    /// the instance stops running.
    pub(super) fn stop_abilities(&mut self, id: u16, depth: usize) -> Result<(), InitFault> {
        let abilities: Vec<_> = self.machine.snapshot.instances[&id]
            .states
            .iter()
            .filter(|(_, state)| state.active && matches!(state.kind, StateKind::Ability { .. }))
            .map(|(&raw, _)| raw)
            .collect();
        for raw in abilities {
            let node = self
                .graph(id)
                .state(usize::from(raw))
                .expect("ability state");
            self.stop_ability(StateKey { instance: id, raw }, node, depth + 1)?;
        }
        Ok(())
    }
}
