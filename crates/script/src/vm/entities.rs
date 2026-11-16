//! Server-only actions on other entities: a team (`STU_0F647E55`) and a
//! removal (`STU_523EE38D`); the scene tag queries of a state
//! (`STU_160F2578`) and an action (`STU_42578342`); the spawn picker
//! (`STU_5E805B5A`).
//!
//! Client research (1.74, `python tools/stu.py`): `STU_0F647E55` and
//! `STU_523EE38D` register no action of their own. The client looks up the
//! base action (record #165, `execute` 0x7FF789C44A80 = `mov al, 1; ret`),
//! so it never runs them: they are the server's rules, read from their
//! fields and uses. The others are the client's code (addresses below).

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    /// `STU_0F647E55`: give each entity of `m_targets` the team index
    /// `m_67465E75` (-1 none, 0 to 4; leafall903/ORPHEUS (MIT)
    /// `assign_entity_team` in `nodes.py`). The Practice Range's spawn
    /// markers `01CE` set their own team from #1016 (the team `0827` and
    /// `0724` name); Symmetra's teleporter (`0622`) and `05FE` set their
    /// entities' teams. A world that keeps no entity teams leaves it, as an
    /// assumption.
    pub(super) fn assign_team(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(team) = self.evaluate(id, node, field(&node.fields, "m_67465E75"), None) else {
            return Ok(false);
        };
        let team = expr::to_int(&team, i64::MIN);
        if !(-1..=4).contains(&team) {
            self.report(
                self.site(id, node),
                Issue::UnsupportedField("entity team index"),
            );
            return Ok(false);
        }
        let Some(targets) = self.evaluate(id, node, field(&node.fields, "m_targets"), None) else {
            return Ok(false);
        };
        let site = self.site(id, node);
        for entity in entities(&targets) {
            if !self.world.set_entity_team(site, entity, team) {
                self.assume(site, &node.class, "the world keeps no entity team");
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// `STU_523EE38D`: remove the entity `m_entity` from the match (the
    /// Practice Range pad `26F8` removes itself when ruleset 29450 is
    /// false; `01C7` removes the weapon's temporary entity #669, which may
    /// not exist yet; leafall903/ORPHEUS (MIT) `retire_junkenstein_corpse`
    /// in `nodes.py`). No entity: nothing to remove. A world that can't
    /// remove it leaves it, as an assumption.
    pub(super) fn destroy_entity(
        &mut self,
        id: u16,
        node: &Node,
        depth: usize,
    ) -> Result<bool, InitFault> {
        let Some(target) = self.evaluate(id, node, field(&node.fields, "m_entity"), None) else {
            return Ok(false);
        };
        let site = self.site(id, node);
        for entity in entities(&target) {
            if !self.world.destroy_entity(site, entity) {
                self.assume(site, &node.class, "the world removes no entity");
            }
        }
        self.follow(id, node, "m_outPlug", None, depth)?;
        Ok(true)
    }

    /// The entities of the scene tag query that a state (`STU_160F2578`,
    /// once after its begin, 0x7FF789C4FE60) or an action
    /// (`STU_42578342`, 0x7FF789CB8EC0; both through 0x7FF789CC3850) makes
    /// of its fields, into `m_4AEEE6A1` ([`expr`]'s `tag_query`). `0C8F`
    /// lists each team's spawn points so (#8365, #8366: tag `#8364e[team]`,
    /// all of #11016). False: the query can't run here.
    pub(super) fn write_tag_query(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let target = match expr::lvalue(field(&node.fields, "m_4AEEE6A1")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(false);
            }
        };
        let mut query = serde_json::Map::new();
        query.insert("$".into(), Json::String(node.class.clone()));
        for name in ["m_9C7BD8B8", "m_B486603E", "m_8B2A4DF3", "m_filter"] {
            query.insert(name.into(), field(&node.fields, name).clone());
        }
        let Some(entities) = self.evaluate(id, node, &Json::Object(query), None) else {
            return Ok(false);
        };
        if let Some(target) = target {
            self.write(target, entities)?;
        }
        Ok(true)
    }

    /// An input into a spawn picker (`STU_5E805B5A`, `m_F4DD996D`; the
    /// client's 0x7FF789C53010 consumes it, and runs 0x7FF789C5F130 when
    /// the state is active): pick one of the candidates `m_814F8EC3` that
    /// none of the last `m_82317B5F` seconds' picks took (none: null),
    /// write it to `m_C51AF683` and follow `m_1405CDCB`. The picks so far
    /// live in the state's [`State::tracked`] (the candidate's slot and
    /// when), which a begin clears (0x7FF789C53000). `0C8F` picks a team's
    /// spawn point so on `0685` and sends it in `0687`.
    pub(super) fn pick_spawn(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let target = match expr::lvalue(field(&node.fields, "m_C51AF683")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                return Ok(());
            }
        };
        let lifetime = field(&node.fields, "m_82317B5F");
        let lifetime = if lifetime.is_null() {
            0.0
        } else {
            let Some(lifetime) = self.evaluate(id, node, lifetime, None) else {
                return Ok(());
            };
            expr::to_float(&lifetime)
        };
        // The seconds as an f32, in milliseconds rounded half away from 0.
        let lifetime_ms = (f64::from(lifetime as f32) * 1000.0).round() as i64;
        let Some(candidates) = self.evaluate(id, node, field(&node.fields, "m_814F8EC3"), None)
        else {
            return Ok(());
        };
        let candidates = match candidates {
            Value::Array(items) => items,
            Value::Null => Vec::new(),
            value => vec![value],
        };
        let now = self.machine.now_ms;
        let age = |since: u64| i64::try_from(now.saturating_sub(since)).unwrap_or(i64::MAX);
        let mut history = std::mem::take(&mut self.state_mut(key).expect("active picker").tracked);
        // From the newest: drop the picks older than the lifetime (each
        // swapped with the last), and find the oldest kept one.
        let mut oldest = 0;
        let mut oldest_age = 0;
        for index in (0..history.len()).rev() {
            let entry_age = age(history[index].since_ms);
            if entry_age > lifetime_ms {
                history.swap_remove(index);
            } else if entry_age >= oldest_age {
                oldest = index;
                oldest_age = entry_age;
            }
        }
        let picked = if candidates.is_empty() {
            Value::Null
        } else {
            let count = candidates.len();
            if history.len() >= count && oldest < history.len() {
                history.swap_remove(oldest);
            }
            let first = self.random_u32() as usize % count;
            let mut slot = first;
            while history.iter().any(|entry| entry.entity as usize == slot) {
                slot = (slot + 1) % count;
                if slot == first {
                    break;
                }
            }
            if lifetime_ms > 0 {
                match history
                    .iter_mut()
                    .find(|entry| entry.entity as usize == slot)
                {
                    Some(entry) => entry.since_ms = now,
                    None => history.push(TrackedTarget {
                        entity: slot as u32,
                        since_ms: now,
                    }),
                }
            }
            candidates[slot].clone()
        };
        self.state_mut(key).expect("active picker").tracked = history;
        if let Some(target) = target {
            self.write(target, picked)?;
        }
        self.follow(id, node, "m_1405CDCB", Some(key), depth)?;
        Ok(())
    }

    /// A random number from the component's stream, as the client's
    /// thread's random numbers (the spawn picker's first candidate).
    fn random_u32(&mut self) -> u32 {
        (super::context::next_random(&self.machine.random_state) >> 32) as u32
    }
}

/// The valid entities of a value (one entity or an array of them), each
/// once, in order.
pub(super) fn entities(value: &Value) -> Vec<u32> {
    let mut found = Vec::new();
    let mut add = |value: &Value| {
        if let Value::Entity(entity) = value {
            if expr::valid_entity(*entity) && !found.contains(entity) {
                found.push(*entity);
            }
        }
    };
    match value {
        Value::Array(items) => items.iter().for_each(&mut add),
        value => add(value),
    }
    found
}
