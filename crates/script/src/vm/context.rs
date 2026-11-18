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
        let value = self.read(source_key(self.site.instance, variable));
        // `#27` unwritten is the entity the instance was created with,
        // which the engine holds (not a variable the frames carry).
        if value == Value::Null && variable.scope == Scope::Instance && variable.id == CREATOR {
            if let Some(source) = self.machine.snapshot.instances[&self.site.instance].source {
                return Value::Entity(source);
            }
        }
        value
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
        if let Some(candidate) = self.candidate {
            return candidate.clone();
        }
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
            if !self.foreign_reads.contains(&(entity, id)) {
                self.foreign_reads.push((entity, id));
            }
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

    fn mode_random(&mut self) -> Option<f64> {
        self.read_native = true;
        self.world.mode_random()
    }

    fn team_size(&mut self, team: i64) -> Option<i64> {
        self.read_native = true;
        self.world.team_size(team)
    }

    fn random_u64(&mut self) -> Option<u64> {
        Some(next_random(&self.machine.random_state))
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
        let value = self.world.native(self.site, config)?;
        let class = config.get("$").and_then(Json::as_str).unwrap_or_default();
        if let Some(answer) = self.world.assumption(class) {
            self.assumed.push((class.to_owned(), answer));
        }
        Some(value)
    }

    fn tolerate_unknown(&mut self, what: &str) -> bool {
        if self.world.unknowns() != Unknowns::Tolerate {
            return false;
        }
        self.assumed.push((what.to_owned(), "null"));
        true
    }

    fn equipped_cosmetic(
        &mut self,
        entity: u32,
        kind: crate::wheel::Cosmetic,
        slot: i64,
    ) -> Option<u64> {
        self.read_native = true;
        self.world.equipped_cosmetic(self.site, entity, kind, slot)
    }

    fn has_target_tag(&mut self, entity: u32, tag: u64) -> Option<bool> {
        self.read_native = true;
        self.world.has_target_tag(self.site, entity, tag)
    }

    fn entities_with_tag(&mut self, tag: u64) -> Option<Vec<u32>> {
        self.read_native = true;
        self.world.entities_with_tag(self.site, tag)
    }

    fn entity_filter_flags(&mut self, entity: u32) -> Option<u64> {
        self.read_native = true;
        self.world.entity_filter_flags(self.site, entity)
    }

    fn body_on_fire(&mut self, entity: u32) -> Option<bool> {
        self.read_native = true;
        self.world.body_on_fire(self.site, entity)
    }

    /// The world's answer, then, for this entity's own controller, every
    /// running HeroSelect's limits (`vm/hero_select.rs`): a refusal by
    /// any refuses.
    fn hero_available(&mut self, controller: u32, hero: u64) -> Option<bool> {
        self.read_native = true;
        let allowed = self.world.hero_available(self.site, controller, hero)?;
        if !allowed || controller != self.site.entity {
            return Some(allowed);
        }
        for limits in self.machine.hero_selects.values() {
            if limits.refuses(hero, || self.world.hero_role(self.site, hero))? {
                return Some(false);
            }
        }
        Some(true)
    }

    fn entity_exists(&mut self, entity: u32) -> Option<bool> {
        self.read_native = true;
        // The script's own entity exists.
        if entity == self.site.entity {
            return Some(true);
        }
        self.world.entity_exists(self.site, entity)
    }

    fn entity_position(&mut self, entity: u32) -> Option<[f64; 3]> {
        self.read_native = true;
        self.world.entity_position(self.site, entity)
    }

    fn entity_facing(&mut self, entity: u32) -> Option<[i32; 2]> {
        self.read_native = true;
        self.world.entity_facing(self.site, entity)
    }

    fn entity_yaw(&mut self, entity: u32) -> Option<i32> {
        self.read_native = true;
        self.world.entity_yaw(self.site, entity)
    }

    fn speed_scale(&mut self, entity: u32) -> Option<f64> {
        self.read_native = true;
        self.world.speed_scale(self.site, entity)
    }

    fn map_ray(
        &mut self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
    ) -> Option<Option<[f64; 3]>> {
        self.read_native = true;
        let hit = self
            .world
            .map_ray(self.site, origin, direction, reach, None)?;
        Some(hit.map(|hit| hit.point))
    }

    fn map_supports_mode(&mut self, mode: u64) -> Option<bool> {
        self.read_native = true;
        self.world.map_supports_mode(self.site, mode)
    }

    fn entity_team(&mut self, entity: u32) -> Option<i64> {
        self.read_native = true;
        self.world.entity_team(self.site, entity)
    }

    fn in_entity_volume(&mut self, entity: u32, position: [f64; 3]) -> Option<bool> {
        self.read_native = true;
        self.world.in_entity_volume(self.site, entity, position)
    }

    fn related_entity(&mut self, relation: EntityRelation, entity: u32) -> Option<u32> {
        self.read_native = true;
        self.world.related_entity(self.site, relation, entity)
    }

    fn controller_is_ai(&mut self, entity: u32) -> Option<bool> {
        self.read_native = true;
        self.world.controller_is_ai(self.site, entity)
    }

    fn ai_parameter(&mut self, entity: u32, identifier: u64) -> Option<Option<f64>> {
        self.read_native = true;
        self.world.ai_parameter(self.site, entity, identifier)
    }

    fn would_stack_to_top(&mut self, variable: Variable, priority: StackPriority) -> Option<bool> {
        let key = source_key(self.site.instance, variable);
        self.read(key);
        Some(self.machine.allows_stack(key, priority, None))
    }
}

/// The component's next random number. SplitMix64 gives each component a
/// reproducible stream without OS calls in graph execution.
pub(super) fn next_random(random_state: &Cell<u64>) -> u64 {
    let state = random_state.get().wrapping_add(0x9e37_79b9_7f4a_7c15);
    random_state.set(state);
    let mixed = (state ^ (state >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^ (mixed >> 31)
}
