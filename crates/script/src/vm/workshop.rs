//! The server-only nodes of the Workshop's action graphs
//! (`refs/reports/Workshop.md` §2): each Workshop action is a graph the
//! server starts with its parameters in instance variable `#18739`
//! ([`ACTION_PARAMETERS`]), and these nodes, which have no client code
//! (no registration of their own; the base action's `mov al, 1; ret`),
//! hand the action's result to the server's rule engine or do what only
//! the server does.
//!
//! - `STU_1E93FE1F` (no fields): the action is done; the rule goes on.
//! - `STU_DC325C52` (`m_C337E584`, a count): Skip / Skip If.
//! - `STU_ADAFD7FF` / `STU_99ED4486` (u8 `m_F3733689`, u8 `m_79252765`):
//!   Abort / Loop; the first flag is set by the "If Condition Is True"
//!   forms (`1DA2`, `1378`), the second by "If Condition Is False" (`1DA3`,
//!   `1DA0`), neither by the plain and "If" forms (`1DA6`, `1DA1`, `1377`,
//!   `1D9F`, which test their own condition first).
//! - `STU_0BDBC7B2` (`m_entity`, `m_out_Var`, `m_value`; `m_index` and
//!   `m_key` unused by the 211 graphs): set `m_out_Var` (a variable, or an
//!   opcode 72 reference to a Workshop variable) on the entity `m_entity`.
//! - `STU_D137ECA0` (`m_D10618D1` a count, `m_out_Var`, `m_value`, the
//!   plug `m_08FBE734`): for each index below the count, with parameter
//!   `0012` the index, set `m_out_Var` to `m_value` and run the plug; then
//!   the out plug (Set Player Variable `136B`: `#38 = #7566[param(12)]`
//!   then `STU_0BDBC7B2` on `#38` for each player in `#7566`).
//!
//! An action ends with its first result: an Abort If whose condition is
//! false runs on to `STU_1E93FE1F`, one whose condition is true stops at
//! `STU_ADAFD7FF`. The instance can run on after it (a Create action's
//! scope state keeps its HUD text up until a Destroy's message ends it).

use super::*;

/// The instance variable an action graph reads its parameters from: an
/// array in the catalog's parameter order (`4933.01C`).
pub const ACTION_PARAMETERS: u32 = 18739;

/// The loop index parameter of `STU_D137ECA0`'s `m_value` (`13 1` with
/// dynamic `#12` in all 25 uses).
const LOOP_INDEX: u32 = 12;

/// Whether an Abort or Loop applies always or by the rule's conditions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleTest {
    Always,
    /// When the rule's conditions are true now (`m_F3733689`).
    IfConditionsTrue,
    /// When they are false now (`m_79252765`).
    IfConditionsFalse,
}

/// How a Workshop action graph handed its rule back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActionOutcome {
    /// `STU_1E93FE1F`: run the next action.
    Done,
    /// `STU_DC325C52`: skip this many actions.
    Skip(i64),
    /// `STU_ADAFD7FF`: stop the rule.
    Abort(RuleTest),
    /// `STU_99ED4486`: run the rule's actions again from the first.
    Loop(RuleTest),
}

fn rule_test(node: &Node) -> RuleTest {
    let flag = |name| {
        field(&node.fields, name)
            .as_u64()
            .is_some_and(|flag| flag != 0)
    };
    if flag("m_F3733689") {
        RuleTest::IfConditionsTrue
    } else if flag("m_79252765") {
        RuleTest::IfConditionsFalse
    } else {
        RuleTest::Always
    }
}

impl Component {
    /// The result the action instance `instance` created at `start_ms`
    /// reported; none while it runs (or for another instance of that id).
    pub fn action_outcome(&self, instance: u16, start_ms: u64) -> Option<ActionOutcome> {
        self.machine
            .outcomes
            .get(&instance)
            .filter(|(start, _)| *start == start_ms)
            .map(|&(_, outcome)| outcome)
    }

    /// Whether the instance created at `start_ms` is still there.
    pub fn instance_alive(&self, instance: u16, start_ms: u64) -> bool {
        self.machine
            .snapshot
            .instances
            .get(&instance)
            .is_some_and(|item| item.start_ms == start_ms)
    }

    /// Whether the instance has an active state (a Wait, a Create action's
    /// scope).
    pub fn instance_busy(&self, instance: u16) -> bool {
        self.machine
            .snapshot
            .instances
            .get(&instance)
            .is_some_and(|item| item.states.values().any(|state| state.active))
    }

    /// When the instance last ran a networked action the frames carry
    /// ([`FrameAction`]).
    pub fn last_frame_action(&self, instance: u16, start_ms: u64) -> Option<u64> {
        self.machine
            .snapshot
            .frame_actions
            .iter()
            .filter(|action| action.instance == instance && action.instance_start_ms == start_ms)
            .map(|action| action.time_ms)
            .max()
    }

    /// A root instance of `graph` with `variables`, its entries run now
    /// (a Workshop action runs within the server tick that starts it);
    /// returns the instance and its start. Ids go up to `u16::MAX`, then
    /// start again at the lowest free one: a Workshop game starts actions
    /// for as long as it runs.
    pub fn run_instance(
        &mut self,
        graph: u32,
        variables: Variables,
        world: &mut impl World,
    ) -> Result<Option<(u16, u64)>, InitFault> {
        let id = match u16::try_from(self.machine.next_id) {
            Ok(id) => id,
            Err(_) => (1..=u16::MAX)
                .find(|id| !self.machine.snapshot.instances.contains_key(id))
                .ok_or(InitFault::InstanceIdExhausted)?,
        };
        let start = self.machine.now_ms;
        // A reused id's earlier result is not this instance's.
        self.machine.outcomes.remove(&id);
        let mut created = false;
        self.run(world, |vm| {
            created = vm.create(id, graph, None, variables, vm.machine.snapshot.entity)?;
            vm.run()
        })?;
        Ok(created.then_some((id, start)))
    }
}

impl<W: World> Vm<'_, '_, '_, W> {
    /// The action's first result counts.
    fn report_outcome(&mut self, id: u16, outcome: ActionOutcome) {
        let start = self.machine.snapshot.instances[&id].start_ms;
        match self.machine.outcomes.get(&id) {
            Some((reported, _)) if *reported == start => {}
            _ => {
                self.machine.outcomes.insert(id, (start, outcome));
            }
        }
    }

    /// `STU_1E93FE1F`, `STU_DC325C52`, `STU_ADAFD7FF`, `STU_99ED4486`.
    pub(super) fn action_result(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let outcome = match node.class.as_str() {
            "STU_DC325C52" => {
                let Some(count) = self.evaluate(id, node, field(&node.fields, "m_C337E584"), None)
                else {
                    return Ok(false);
                };
                ActionOutcome::Skip(expr::to_int(&count, 0))
            }
            "STU_ADAFD7FF" => ActionOutcome::Abort(rule_test(node)),
            "STU_99ED4486" => ActionOutcome::Loop(rule_test(node)),
            _ => ActionOutcome::Done,
        };
        self.report_outcome(id, outcome);
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// Evaluate with the instance's context, outside any state.
    pub(super) fn evaluate_with<T>(
        &mut self,
        id: u16,
        node: &Node,
        evaluate: impl FnOnce(&mut InstanceContext<'_, W>) -> Result<T, EvalError>,
    ) -> Option<T> {
        let site = self.site(id, node);
        let mut context = InstanceContext {
            machine: self.machine,
            graphs: self.graphs,
            world: self.world,
            site,
            reads: Vec::new(),
            foreign_reads: Vec::new(),
            parameters: self.parameters.as_deref(),
            candidate: self.candidate.as_ref(),
            sender: self.sender,
            watcher: None,
            read_native: false,
            read_volatile: false,
            natives: Vec::new(),
            assumed: Vec::new(),
        };
        let result = evaluate(&mut context);
        for (class, answer) in context.assumed {
            self.assume(site, &class, answer);
        }
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                None
            }
        }
    }

    /// `STU_0BDBC7B2`: set a variable of the entity `m_entity`. On another
    /// entity the world sets it ([`World::set_entity_variable`]).
    pub(super) fn set_variable_of(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let site = self.site(id, node);
        if !field(&node.fields, "m_index").is_null() || !field(&node.fields, "m_key").is_null() {
            self.report(site, Issue::UnsupportedField("STU_0BDBC7B2 index or key"));
            return Ok(false);
        }
        let Some(entity) = self.evaluate(id, node, field(&node.fields, "m_entity"), None) else {
            return Ok(false);
        };
        let target = field(&node.fields, "m_out_Var");
        let variable = match expr::dynamic(target) {
            Ok(Some(variable)) => Some(variable.id),
            Ok(None) => {
                let Some(reference) = self.evaluate_with(id, node, |context| {
                    expr::workshop_reference(target, context)
                }) else {
                    return Ok(false);
                };
                reference.map(|(_, variable)| variable)
            }
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(false);
            }
        };
        let Some(variable) = variable else {
            self.report(site, Issue::UnsupportedField("STU_0BDBC7B2 variable"));
            return Ok(false);
        };
        let Some(value) = self.evaluate_value(id, node, field(&node.fields, "m_value"), None)
        else {
            return Ok(false);
        };
        match expr::to_entity(&entity).filter(|&entity| expr::valid_entity(entity)) {
            Some(entity) if entity == self.machine.snapshot.entity => {
                self.write(VarKey::Entity(variable), value)?;
            }
            Some(entity) if !self.world.set_entity_variable(site, entity, variable, value) => {
                self.assume(site, &node.class, "sets no other entity's variable");
            }
            // Set by the world, or no entity (a player list that was
            // empty): nothing more to do.
            Some(_) | None => {}
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_D137ECA0`: the loop over `m_D10618D1` indexes.
    pub(super) fn for_each_index(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(count) = self.evaluate(id, node, field(&node.fields, "m_D10618D1"), None) else {
            return Ok(false);
        };
        let count = expr::to_int(&count, 0).max(0);
        let saved = self.parameters.clone();
        for index in 0..count {
            self.spend("STU_D137ECA0 iteration")?;
            let mut parameters = saved.as_deref().cloned().unwrap_or_default();
            parameters.insert(LOOP_INDEX, Value::Integer(index));
            self.parameters = Some(Arc::new(parameters));
            let result = self
                .store(id, node, &node.fields, false)
                .and_then(|stored| {
                    if stored {
                        self.follow(id, node, "m_08FBE734", None, depth)?;
                    }
                    Ok(stored)
                });
            if !matches!(result, Ok(true)) {
                self.parameters = saved;
                return result;
            }
        }
        self.parameters = saved;
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }
}
