//! State-input joins, the StateElapsed condition and the action Switch.
//!
//! Ported from leafall903/ORPHEUS (MIT), commit c239a84: `Instance.join` in
//! `src/ow174/game/script/runtime.py`, `state_elapsed` and `action_switch` in
//! `src/ow174/game/script/nodes.py`. Their research is upstream's
//! `docs/STATE_INPUT_JOIN.txt` (client functions 1B6EED0, 1B73BC0 and
//! 1B700F0 for the join, 1B75730 for StateElapsed).

use super::*;

/// The join's count of distinct inputs (`STU_1113EBD5`).
const JOIN_COUNT: &str = "m_B24D1794";
/// The states a join checks on its first input and aborts when it passes.
const JOIN_STATES: &str = "m_122DF0CA";
/// StateElapsed's age in seconds (`STU_4E2BCAAB`).
const ELAPSED_SECONDS: &str = "m_AA6E611E";
/// The action Switch's indexed form: the value is the case number.
const SWITCH_INDEXED: &str = "m_F0F0720B";

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    /// A state-input join (`STU_1113EBD5`) reached through `plug` of node
    /// `producer` (`Instance.join` in upstream's `runtime.py`). The client
    /// keeps one record per distinct input (join, producer, plug); a repeat
    /// does nothing. The first input asks every active listed state but the
    /// caller for a completion check, and is not taken, even when the count
    /// is 1. The input that brings the count to m_B24D1794 clears the join's
    /// records (tail swap), follows m_outPlug and, when a link was taken,
    /// aborts the listed states that are still active. Following the out
    /// plug at once instead ended McCree's Deadeye as it began
    /// (`docs/STATE_INPUT_JOIN.txt`).
    pub(super) fn join(
        &mut self,
        id: u16,
        node: &Node,
        producer: u16,
        plug: &str,
        caller: Option<StateKey>,
        depth: usize,
    ) -> Result<bool, InitFault> {
        self.spend("node execution")?;
        let arrivals = self.machine.joins.entry(id).or_default();
        if arrivals.iter().any(|arrival| {
            arrival.join == node.pos && arrival.producer == producer && *arrival.plug == *plug
        }) {
            return Ok(false);
        }
        let previous = arrivals
            .iter()
            .filter(|arrival| arrival.join == node.pos)
            .count();
        arrivals.push(JoinArrival {
            join: node.pos,
            producer,
            plug: plug.into(),
        });
        let states = array_field(&node.fields, JOIN_STATES);
        if previous == 0 {
            for raw in raw_indices(states) {
                self.spend("join completion check")?;
                let key = StateKey { instance: id, raw };
                if caller == Some(key) {
                    continue;
                }
                // A state not made yet is looked up, not made.
                let Some(state) = self
                    .state_mut(key)
                    .filter(|state| state.active && !state.pending_check)
                else {
                    continue;
                };
                state.pending_check = true;
                self.post(id, Event::Check(raw))?;
            }
            return Ok(false);
        }
        let count = field(&node.fields, JOIN_COUNT).as_i64().unwrap_or(0);
        if i64::try_from(previous).ok() != count.checked_sub(1) {
            return Ok(false);
        }
        let arrivals = self
            .machine
            .joins
            .get_mut(&id)
            .expect("recorded join input");
        let mut index = 0;
        while index < arrivals.len() {
            if arrivals[index].join == node.pos {
                arrivals.swap_remove(index);
            } else {
                index += 1;
            }
        }
        if !self.follow(id, node, "m_outPlug", caller, depth)? {
            return Ok(false);
        }
        // The states are looked up after the out plug ran, which may have
        // begun or ended some of them.
        for raw in raw_indices(states) {
            self.spend("join state abort")?;
            if self
                .state(StateKey { instance: id, raw })
                .is_some_and(|state| state.active)
            {
                self.post(id, Event::Abort(raw))?;
            }
        }
        Ok(true)
    }

    /// A completion check a join asked for (upstream's `CHECK` event): an
    /// active state that is not ending tries to complete. An Ability runs
    /// its update instead (its event handler 0x7FF789B946D0 takes kinds 1
    /// and 2 to the update 0x7FF789B95560 and nothing else), so a write
    /// its conditions read, such as its cooldown variable's, takes effect
    /// in the same frame.
    pub(super) fn check(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(state) = self.state_mut(key) else {
            return Ok(());
        };
        state.pending_check = false;
        if !state.active || state.ending {
            return Ok(());
        }
        if matches!(state.kind, StateKind::Ability { .. }) {
            return self.activate_ability(key, 0);
        }
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("active state node");
        self.try_complete(key, node, 0)
    }

    /// StateElapsed (`STU_4E2BCAAB`, `state_elapsed` in upstream's
    /// `nodes.py`): true when the calling state (or, without one, the
    /// instance's parent state) has run at least m_AA6E611E seconds. A
    /// number outside [0, 2147483) is false. Too early with a calling state:
    /// that state's timer 0 is set for the moment it gets old enough
    /// (replacing one already set), so its transitions are checked again then.
    pub(super) fn state_elapsed(
        &mut self,
        id: u16,
        node: &Node,
        caller: Option<StateKey>,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(seconds) = self.evaluate(id, node, field(&node.fields, ELAPSED_SECONDS), caller)
        else {
            return Ok(false);
        };
        let seconds = match seconds {
            Value::Integer(seconds) => seconds as f64,
            Value::Float(seconds) => seconds,
            _ => return self.branch(id, node, caller, false, depth),
        };
        if !seconds.is_finite() || !(0.0..2_147_483.0).contains(&seconds) {
            return self.branch(id, node, caller, false, depth);
        }
        let delay = ((seconds * 1000.0) as f32).round() as u64;
        let state = caller.or_else(|| {
            let (instance, raw) = self.machine.snapshot.instances[&id].parent?;
            Some(StateKey { instance, raw })
        });
        let start = state
            .and_then(|key| self.state(key))
            .map(|state| state.start_ms);
        let now = self.machine.now_ms;
        let ready = start.is_some_and(|start| now.saturating_sub(start) >= delay);
        if let Some(caller) = caller.filter(|_| !ready) {
            let start = self.state(caller).map_or(now, |state| state.start_ms);
            self.machine.queues.retain(
                caller.instance,
                |_, event| !matches!(event, Event::Timer(raw, 0) if *raw == caller.raw),
            );
            self.machine.queues.push(
                caller.instance,
                start.saturating_add(delay),
                Event::Timer(caller.raw, 0),
            )?;
        }
        self.branch(id, node, caller, ready, depth)
    }

    /// The action Switch (`action_switch` in upstream's `nodes.py`): follows
    /// the chosen case's plug (or m_CD38D5DB when none is), then m_outPlug.
    /// In the indexed form (m_F0F0720B = 1) an integer value in range is the
    /// case number; otherwise the first case whose m_value equals the value.
    pub(super) fn action_switch(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(value) = self.evaluate(id, node, field(&node.fields, "m_262AE982"), None) else {
            return Ok(false);
        };
        let cases = array_field(&node.fields, "m_FDD34C25");
        let indexed = field(&node.fields, SWITCH_INDEXED);
        let mut chosen = None;
        if matches!(indexed, Json::Bool(true)) || indexed.as_f64() == Some(1.0) {
            if let Value::Integer(index) = value {
                chosen = usize::try_from(index)
                    .ok()
                    .filter(|&index| index < cases.len());
            }
        } else {
            for (number, case) in cases.iter().enumerate() {
                if !case.is_object() {
                    continue;
                }
                let Some(candidate) = self.evaluate(id, node, field(case, "m_value"), None) else {
                    return Ok(false);
                };
                if equal(&value, &candidate) {
                    chosen = Some(number);
                    break;
                }
            }
        }
        let path = chosen.map_or_else(
            || "m_CD38D5DB".to_owned(),
            |number| format!("m_FDD34C25[{number}]"),
        );
        self.follow(id, node, &path, None, depth)?;
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }
}
