//! The world the VM sees from a human player's own body: what the server
//! answers when the body's graphs ask about the game.
//!
//! Each answer is either computed from the server's state or, for the few
//! questions whose meaning is still open, the answer the Python reference gives
//! for a human player's body (`TRUSTED_NATIVES` in `ow174/game/script/driver.py`).
//! GetThrottle (STU_310D6529, 0x7FF789BDA130) is the command's throttle bytes
//! times f32(1/127); the evaluator of STU_4A101787 (0x7FF789BFF730) always
//! answers false.
//! VM tests of hero graphs use this world, so they test what the server does.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde_json::Value as Json;

use crate::{
    expr::{EntityRelation, native_shape, valid_entity},
    graph::GraphStore,
    vitals::{PoolKind, Vitals},
    vm::{GameMessage, Site, TargetTagEffect, Unknowns, World},
    wire::{Value, Variables},
};
use orpheus_physics::mover::sample_movement_flags;
use orpheus_wire::entity::{GAME_MODE_ENTITY, Movement};

pub struct PlayerWorld {
    pub graphs: Arc<GraphStore>,
    pub controller: u32,
    pub entity: u32,
    pub body_filter: u64,
    pub position: [f64; 3],
    pub movement: Movement,
    pub throttles: [i8; 2],
    pub mode: u64,
    pub hero: u64,
    pub vitals: Vitals,
    pub pool_types_known: bool,
    pub target_tags: Option<BTreeSet<u64>>,
    target_tag_effects: BTreeMap<u64, TargetTagCounts>,
    pub controller_on_fire: bool,
    /// How the body treats graph content the VM doesn't implement; by
    /// default as every script does ([`Unknowns::for_build`]).
    pub unknowns: Unknowns,
    /// Game messages to other entities, for the match to route.
    outbox: Vec<GameMessage>,
}

#[derive(Default)]
struct TargetTagCounts {
    grants: u32,
    suppressions: u32,
}

impl World for PlayerWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        match class {
            // Server-only: picks the entities message 0BD goes to (graph
            // 01CA, node 126; moving and firing reach it). Its meaning is
            // open; the Python reference answered nothing, so the message
            // went nowhere.
            "STU_ED45C5E8" => Some("no entities"),
            // An entity filter the evaluator can't compute (`native`).
            "STU_AC829876" => Some("false"),
            // A tunable value for an entity (`native`): the authored value,
            // not scaled for the entity.
            "STU_082FA44E" => Some("the tunable's authored value"),
            _ => None,
        }
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.rulesets().default(id, self.hero, self.mode)
    }

    /// The body's messages to other entities wait for the match
    /// (`BodyWorld.send_message` in `ow174/game/script/driver.py`).
    fn send_message(
        &mut self,
        source: Site,
        target: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        if source.entity != self.entity {
            return false;
        }
        self.outbox.push(GameMessage {
            sender: self.entity,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.graphs
            .mode_variables(self.mode)
            .and_then(|variables| variables.get(&id))
            .copied()
            .map_or(Value::Null, Value::from)
    }

    fn position(&mut self, entity: u32) -> [f64; 3] {
        if entity == self.entity {
            self.position
        } else {
            [0.0; 3]
        }
    }

    fn movement_sample(&self, site: Site) -> Option<u16> {
        if !self.is_player_site(site) {
            return None;
        }
        sample_movement_flags(&self.movement)
    }

    fn body_scale(&mut self, site: Site, target: u32, override_value: &Value) -> Option<f64> {
        if target != self.entity || !self.is_player_site(site) {
            return None;
        }
        match override_value {
            Value::Null | Value::Integer(1) => Some(1.0),
            Value::Float(scale) if *scale == 1.0 => Some(1.0),
            _ => None,
        }
    }

    fn body_health(&self, site: Site, kind: Option<PoolKind>, maximum: bool) -> Option<f64> {
        if !self.is_player_site(site) || (kind.is_some() && !self.pool_types_known) {
            return None;
        }
        let amount = self.vitals.amount(kind, maximum);
        (amount.is_finite() && amount >= 0.0).then_some(amount)
    }

    fn add_health_pool(
        &mut self,
        site: Site,
        kind: PoolKind,
        maximum: f64,
        current: f64,
    ) -> Option<u32> {
        if !self.is_player_site(site) || !self.pool_types_known {
            return None;
        }
        self.vitals.add_pool(kind, maximum, current)
    }

    fn health_pool_current(&self, site: Site, pool: u32) -> Option<f64> {
        if !self.is_player_site(site) {
            return None;
        }
        self.vitals.pool_current(pool)
    }

    fn remove_health_pool(&mut self, site: Site, pool: u32) -> Option<f64> {
        if !self.is_player_site(site) {
            return None;
        }
        self.vitals.remove_pool(pool)
    }

    fn has_target_tag(&self, site: Site, entity: u32, tag: u64) -> Option<bool> {
        if !self.is_player_site(site) || entity != self.entity {
            return None;
        }
        if let Some(counts) = self.target_tag_effects.get(&tag) {
            return Some(counts.suppressions == 0);
        }
        Some(self.target_tags.as_ref()?.contains(&tag))
    }

    fn set_target_tag_effect(&mut self, site: Site, effect: TargetTagEffect, active: bool) -> bool {
        if !self.is_player_site(site) || effect.entity != self.entity {
            return false;
        }
        let counts = if active {
            self.target_tag_effects.entry(effect.tag).or_default()
        } else {
            let Some(counts) = self.target_tag_effects.get_mut(&effect.tag) else {
                return false;
            };
            counts
        };
        // The authored removal can mask existing grants. A later grant's
        // precedence over an already-active suppression is still unknown.
        if active && effect.present && counts.suppressions != 0 {
            return false;
        }
        let count = if effect.present {
            &mut counts.grants
        } else {
            &mut counts.suppressions
        };
        let next = if active {
            count.checked_add(1)
        } else {
            count.checked_sub(1)
        };
        let Some(next) = next else {
            return false;
        };
        *count = next;
        if counts.grants == 0 && counts.suppressions == 0 {
            self.target_tag_effects.remove(&effect.tag);
        }
        true
    }

    fn entity_filter_flags(&self, site: Site, entity: u32) -> Option<u64> {
        (self.is_player_site(site) && entity == self.entity).then_some(self.body_filter)
    }

    fn body_on_fire(&mut self, site: Site, entity: u32) -> Option<bool> {
        self.related_entity(site, EntityRelation::CurrentPossessor, entity)
            .map(|_| self.controller_on_fire)
    }

    fn related_entity(&mut self, site: Site, relation: EntityRelation, entity: u32) -> Option<u32> {
        if !self.is_player_site(site)
            || !valid_entity(self.controller)
            || self.controller == self.entity
        {
            return None;
        }
        // Both relations point to the same controller for this human body.
        match relation {
            EntityRelation::OwningController | EntityRelation::CurrentPossessor
                if entity == self.entity =>
            {
                Some(self.controller)
            }
            EntityRelation::PossessedBody if entity == self.controller => Some(self.entity),
            // The authenticated human body has no AI control entity in this world.
            EntityRelation::AiControlEntity if entity == self.entity => Some(0),
            _ => None,
        }
    }

    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_1FF67CA6" => Some(Value::Entity(self.entity)),
            "STU_310D6529" => {
                const THROTTLE_UNIT: f32 = 1.0 / 127.0;
                let [right, forward] = self
                    .throttles
                    .map(|byte| f64::from(f32::from(byte) * THROTTLE_UNIT));
                Some(Value::Vector([right, forward, 0.0]))
            }
            "STU_37F20D24" => Some(Value::Asset(self.mode)),
            "STU_A3DEF368" if self.graphs.mode_variables(self.mode).is_some() => {
                Some(Value::Entity(GAME_MODE_ENTITY))
            }
            "STU_4A101787" => Some(Value::Bool(false)),
            // See `assumption`.
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            // See `assumption`.
            "STU_ED45C5E8" if self.is_player_site(site) => Some(Value::Array(Vec::new())),
            // Does an entity pass a filter (teams, tags): the forms the
            // evaluator can't compute answer false, as
            // `ow174/game/script/driver.py` does.
            "STU_AC829876" if self.is_player_site(site) => Some(Value::Bool(false)),
            // A human player's own body takes the falsey branch for this
            // parameterless query, in every mode (`TRUSTED_NATIVES` in
            // `ow174/game/script/driver.py`). Its native meaning is open.
            "STU_A77AC2AC"
                if self.is_player_site(site) && native_shape(config, "STU_A77AC2AC", None) =>
            {
                Some(Value::Null)
            }
            _ => None,
        }
    }
}

impl PlayerWorld {
    /// A world for the body `entity` that `controller` possesses, standing
    /// still at the origin with no health pools and no target tags.
    pub fn new(
        graphs: Arc<GraphStore>,
        controller: u32,
        entity: u32,
        body_filter: u64,
        mode: u64,
        hero: u64,
    ) -> Self {
        Self {
            graphs,
            controller,
            entity,
            body_filter,
            position: [0.0; 3],
            movement: Movement::default(),
            throttles: [0; 2],
            mode,
            hero,
            vitals: Vitals::empty(),
            pool_types_known: false,
            target_tags: None,
            target_tag_effects: BTreeMap::new(),
            controller_on_fire: false,
            unknowns: Unknowns::for_build(),
            outbox: Vec::new(),
        }
    }

    /// The game messages the body sent other entities since the last call,
    /// in order (`Match.queue_body_message` in `ow174/game/match.py`); the
    /// recipient takes each with `Component::receive_message`.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }

    fn is_player_site(&self, site: Site) -> bool {
        site.entity == self.entity && valid_entity(self.entity)
    }
}
