//! Expression context for one instance.

use super::*;

impl<W: World> InstanceContext<'_, W> {
    pub(super) fn read(&mut self, key: VarKey) -> Value {
        if !self.reads.contains(&key) {
            self.reads.push(key);
        }
        let below = self.watcher.filter(|watcher| {
            self.machine
                .snapshot
                .instances
                .get(&watcher.instance)
                .and_then(|instance| instance.states.get(&watcher.raw))
                .is_some_and(|state| {
                    matches!(state.kind, StateKind::Stack { .. })
                        && state.linked_variable == Some(key)
                })
        });
        self.machine.value(key, below)
    }
}

impl<W: World> Context for InstanceContext<'_, W> {
    fn variable(&mut self, variable: Variable) -> Value {
        self.read(source_key(self.site.instance, variable))
    }

    fn parent_variable(&mut self, levels: u8, variable: Variable) -> Value {
        let mut id = self.site.instance;
        for _ in 0..levels {
            let Some((parent, _)) = self.machine.snapshot.instances[&id].parent else {
                return Value::Null;
            };
            id = parent;
        }
        self.read(source_key(id, variable))
    }

    fn parent_entity(&mut self, _levels: u8) -> Value {
        self.owner()
    }

    fn parameter(&mut self, id: u32) -> Value {
        self.parameters
            .and_then(|parameters| parameters.get(&id))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn owner(&mut self) -> Value {
        Value::Entity(self.site.entity)
    }

    fn entity_member(&mut self, entity: u32, id: u32) -> Value {
        if entity == self.site.entity {
            self.read(VarKey::Entity(id))
        } else {
            self.world.entity_variable(entity, id)
        }
    }

    fn ruleset(&mut self, id: u32) -> Value {
        match self.read(VarKey::Entity(id)) {
            Value::Null => self.world.ruleset(id).unwrap_or(Value::Integer(0)),
            value => value,
        }
    }

    fn position(&mut self, entity: u32) -> [f64; 3] {
        self.world.position(entity)
    }

    fn random_u64(&mut self) -> Option<u64> {
        // SplitMix64 gives each component a reproducible stream without OS calls in graph execution.
        let state = self
            .machine
            .random_state
            .get()
            .wrapping_add(0x9e37_79b9_7f4a_7c15);
        self.machine.random_state.set(state);
        let mixed = (state ^ (state >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Some(mixed ^ (mixed >> 31))
    }

    fn body_scale(&mut self, entity: u32) -> Option<f64> {
        if entity != self.site.entity {
            return None;
        }
        self.read_native = true;
        let scale_override = self.read(VarKey::Entity(31_210));
        self.world.body_scale(self.site, entity, &scale_override)
    }

    fn body_health(&mut self, entity: u32, kind: Option<PoolKind>, maximum: bool) -> Option<f64> {
        if entity != self.site.entity {
            return None;
        }
        self.read_native = true;
        self.world.body_health(self.site, kind, maximum)
    }

    fn native(&mut self, config: &Json) -> Option<Value> {
        self.read_native = true;
        self.world.native(self.site, config)
    }

    fn has_target_tag(&mut self, entity: u32, tag: u64) -> Option<bool> {
        self.read_native = true;
        self.world.has_target_tag(self.site, entity, tag)
    }

    fn entity_filter_flags(&mut self, entity: u32) -> Option<u64> {
        self.read_native = true;
        self.world.entity_filter_flags(self.site, entity)
    }

    fn body_on_fire(&mut self, entity: u32) -> Option<bool> {
        self.read_native = true;
        self.world.body_on_fire(self.site, entity)
    }

    fn related_entity(&mut self, relation: EntityRelation, entity: u32) -> Option<u32> {
        self.read_native = true;
        self.world.related_entity(self.site, relation, entity)
    }

    fn would_stack_to_top(&mut self, variable: Variable, priority: StackPriority) -> Option<bool> {
        let key = source_key(self.site.instance, variable);
        self.read(key);
        Some(self.machine.allows_stack(key, priority, None))
    }
}
