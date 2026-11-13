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
    expr::{EntityRelation, native_shape, team_from_filter, valid_entity},
    graph::GraphStore,
    vitals::{PoolKind, Vitals},
    vm::{GameMessage, RaisedCriteria, Site, TargetCandidate, TargetTagEffect, Unknowns, World},
    wheel::{Cosmetic, Cosmetics},
    wire::{Value, Variables},
};
use orpheus_content::maps::TUTORIAL_MODE;
use orpheus_physics::mover::sample_movement_flags;
use orpheus_wire::entity::{GAME_MODE_ENTITY, Movement};

/// The hero body graphs that raise the body's voice reactions: `01CA` and
/// `14B1`, the two subgraphs `004B` st27 chooses between by `06A3.025`
/// (every hero body starts `004B`). Each takes the damage message
/// `0011.025` and, 0.15 s later (`01CA` st16, `14B1` st2), raises a pain
/// criteria on the body: minor `0608` while `004B`'s rating of the hurt
/// (#10417e) is above 0.1 and below 0.3, severe `0609` from 0.3; none for
/// a hit tagged `191F` or `0262`, or rated 0.1 or less. `004B` rid 58 rates
/// it as #7404 / 200 + 0.3 × (maximum − health) / (maximum − 75), #7404
/// the damage of the last moments (halved every 0.2 s). Every hero's
/// voice set answers both (stimuli "PnMnr" and "PnSvr").
pub const REACTION_GRAPHS: [u32; 2] = [0x01ca, 0x14b1];

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
    /// The hero's body definition (`003`), which names its animation data.
    pub body: Option<u64>,
    pub vitals: Vitals,
    pub pool_types_known: bool,
    pub target_tags: Option<BTreeSet<u64>>,
    target_tag_effects: BTreeMap<u64, TargetTagCounts>,
    pub controller_on_fire: bool,
    /// The bodies a target search can find, as the match placed them this
    /// tick (its own body among them); none: the world lists no bodies.
    pub targets: Option<Vec<TargetCandidate>>,
    /// How the body treats graph content the VM doesn't implement; by
    /// default as every script does ([`Unknowns::for_build`]).
    pub unknowns: Unknowns,
    /// The communication wheel cosmetics the player has equipped for this
    /// hero (`BodyWorld.spray_unlocks` and the others in leafall903/ORPHEUS
    /// (MIT) `driver.py`).
    pub cosmetics: Cosmetics,
    /// Game messages to other entities, for the match to route.
    outbox: Vec<GameMessage>,
    /// The criteria the body's graphs raised on it, for the match to answer
    /// with the body's voice lines.
    criteria: Vec<RaisedCriteria>,
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

    /// From the body's compiled animation data
    /// ([`crate::anim_lengths::AnimLengths`]).
    fn animation_length(&self, site: Site, alias: u64, category: u64) -> Option<f64> {
        let body = self.body.filter(|_| site.entity == self.entity)?;
        self.graphs.anim_lengths().length(body, alias, category)
    }

    fn animation_scalar(&self, site: Site, alias: u64, category: u64) -> Option<f64> {
        let body = self.body.filter(|_| site.entity == self.entity)?;
        self.graphs.anim_lengths().scalar(body, alias, category)
    }

    fn equipped_cosmetic(&self, site: Site, entity: u32, kind: Cosmetic, slot: i64) -> Option<u64> {
        (self.is_player_site(site) && entity == self.entity)
            .then(|| self.cosmetics.equipped(kind, slot))
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

    /// A criteria on the body itself that its voice set answers with a line
    /// (ClientInVoice 27500): the communication wheel's (`0686` raises the
    /// choice's, Hello `0AEB`) and the hero's reactions ([`REACTION_GRAPHS`]:
    /// the pain lines after a hit, `0608` / `0609`). The hero graphs' other
    /// criteria (ability lines) are left out: nothing has checked them in
    /// the game yet.
    fn criteria(&mut self, site: Site, raised: RaisedCriteria) {
        if site.entity == self.entity
            && (crate::wheel::ACTION_GRAPHS.contains(&site.graph)
                || REACTION_GRAPHS.contains(&site.graph))
        {
            self.criteria.push(raised);
        }
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

    /// The body, its controller, the mode entity (when the mode has one) and
    /// the bodies the match lists exist; any other entity is unknown.
    fn entity_exists(&self, site: Site, entity: u32) -> Option<bool> {
        if !self.is_player_site(site) {
            return None;
        }
        let known = entity == self.entity
            || (valid_entity(self.controller) && entity == self.controller)
            || (entity == GAME_MODE_ENTITY && self.graphs.mode_variables(self.mode).is_some())
            || self.listed(entity).is_some();
        known.then_some(true)
    }

    /// The body stands where its mover put it; the other bodies where the
    /// match placed them this tick.
    fn entity_position(&self, site: Site, entity: u32) -> Option<[f64; 3]> {
        if !self.is_player_site(site) {
            return None;
        }
        if entity == self.entity {
            return Some(self.position);
        }
        self.listed(entity).map(|body| body.feet)
    }

    /// The body looks along its last movement record's yaw and pitch
    /// (`Match.script_body_facing` in leafall903/ORPHEUS, commit c239a84).
    /// Other bodies' facing is unknown here.
    fn entity_facing(&self, site: Site, entity: u32) -> Option<[i32; 2]> {
        (self.is_player_site(site) && entity == self.entity)
            .then_some([self.movement.yaw, self.movement.pitch])
    }

    /// The body's team from its filter bits; the other bodies' as listed.
    fn entity_team(&self, site: Site, entity: u32) -> Option<i64> {
        if !self.is_player_site(site) {
            return None;
        }
        if entity == self.entity {
            return Some(team_from_filter(self.body_filter));
        }
        self.listed(entity)?.team.map(i64::from)
    }

    fn target_candidates(&self, site: Site) -> Option<&[TargetCandidate]> {
        self.is_player_site(site)
            .then_some(self.targets.as_deref())
            .flatten()
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
            // Another body the match lists: the controller it gives (`01CF`
            // rid 173 asks for the attacker's in the damage message).
            EntityRelation::OwningController if entity != self.entity => {
                self.listed(entity)?.controller
            }
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
            // IsTutorial (CE8580): whether the mode is the Tutorial
            // (`SceneWorld.native` in leafall903/ORPHEUS, commit c239a84).
            "STU_A77AC2AC" if native_shape(config, "STU_A77AC2AC", None) => {
                Some(Value::Bool(self.mode == TUTORIAL_MODE))
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
            body: graphs.hero_body(hero),
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
            targets: None,
            unknowns: Unknowns::for_build(),
            cosmetics: Cosmetics::default(),
            outbox: Vec::new(),
            criteria: Vec::new(),
        }
    }

    /// The game messages the body sent other entities since the last call,
    /// in order (`Match.queue_body_message` in `ow174/game/match.py`); the
    /// recipient takes each with `Component::receive_message`.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }

    /// The criteria the body's graphs raised on it since the last call.
    pub fn take_criteria(&mut self) -> Vec<RaisedCriteria> {
        std::mem::take(&mut self.criteria)
    }

    fn is_player_site(&self, site: Site) -> bool {
        site.entity == self.entity && valid_entity(self.entity)
    }

    /// A body the match lists this tick.
    fn listed(&self, entity: u32) -> Option<&TargetCandidate> {
        self.targets
            .iter()
            .flatten()
            .find(|body| body.entity == entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: u32 = 0xa000_0101;
    const CONTROLLER: u32 = 0xa000_0001;

    fn body(entity: u32, team: Option<u8>, feet: [f64; 3]) -> TargetCandidate {
        TargetCandidate {
            entity,
            team,
            dead: false,
            controller: Some(entity & !0xff),
            feet,
            height: 1.8,
            radius: 0.4,
        }
    }

    /// The body's own facts come from its world; other bodies' from the
    /// match's list; anything else stays unknown.
    #[test]
    fn entity_answers_come_from_the_body_and_the_listed_bodies() {
        let graphs =
            GraphStore::from_json(br#"{"version":1,"graphs":[],"bodies":{},"missing":[]}"#)
                .unwrap();
        // Team 1's filter bit, with the player bit.
        let filter = 0x0004_0001 | (0x80_0000 << 1);
        let mut world = PlayerWorld::new(Arc::new(graphs), CONTROLLER, BODY, filter, 0, 0);
        world.position = [1.0, 2.0, 3.0];
        world.movement.yaw = 16384;
        world.movement.pitch = -100;
        world.targets = Some(vec![
            body(BODY, Some(1), [1.0, 2.0, 3.0]),
            body(0xa000_0102, Some(0), [4.0, 0.0, 4.0]),
            body(0xa000_0103, None, [5.0, 0.0, 5.0]),
        ]);
        let site = Site {
            entity: BODY,
            instance: 1,
            graph: 42,
            node: None,
        };
        for entity in [BODY, CONTROLLER, 0xa000_0102] {
            assert_eq!(world.entity_exists(site, entity), Some(true));
        }
        assert_eq!(world.entity_exists(site, 0xa000_0104), None);
        assert_eq!(world.entity_exists(site, GAME_MODE_ENTITY), None);
        assert_eq!(world.entity_position(site, BODY), Some([1.0, 2.0, 3.0]));
        assert_eq!(
            world.entity_position(site, 0xa000_0102),
            Some([4.0, 0.0, 4.0])
        );
        assert_eq!(world.entity_position(site, CONTROLLER), None);
        assert_eq!(world.entity_facing(site, BODY), Some([16384, -100]));
        assert_eq!(world.entity_facing(site, 0xa000_0102), None);
        assert_eq!(world.entity_team(site, BODY), Some(1));
        assert_eq!(world.entity_team(site, 0xa000_0102), Some(0));
        // A free-for-all body lists no team.
        assert_eq!(world.entity_team(site, 0xa000_0103), None);
        // A listed body's controller is the one the match gives it.
        assert_eq!(
            world.related_entity(site, EntityRelation::OwningController, 0xa000_0102),
            Some(0xa000_0100)
        );
        assert_eq!(
            world.related_entity(site, EntityRelation::OwningController, 0xa000_0104),
            None
        );
        let other_site = Site {
            entity: 0xa000_0102,
            ..site
        };
        assert_eq!(world.entity_exists(other_site, BODY), None);
    }

    /// A hit Soldier: 76 survives reaches his body as the damage message
    /// `0011.025`, as the match sends it. `004B` rates the hurt (#10417e),
    /// `02C5` starts the networked knockback anim st7 for a splash hit
    /// (Helix Rocket explosion tags: `078D`, not `0302`) with the hit's
    /// direction, `01CF` asks for the attacker's controller (#9737), and
    /// 0.15 s later `01CA` raises the pain criteria on the body: severe
    /// `0609` for a Training Bot's 120 (0.6 + 0.3 × 120 / 125 = 0.888), minor
    /// `0608` for his own rocket's 40 at his feet (0.2 + 0.3 × 40 / 125 =
    /// 0.296), each with `191D` (above 0.1). The pain criteria's audience
    /// is the body and its attacker (once: his own rocket's lists him
    /// alone). The wait ends in the 10th frame after the hit (150 ms of
    /// 16 ms frames). Nothing in the path is unsupported.
    #[test]
    #[ignore = "requires the derived 104319 statescript catalog"]
    fn a_hit_the_hero_survives_raises_his_pain_line_and_knockback() {
        use crate::bot::{DAMAGE_AMOUNT, DAMAGE_DIRECTION, DAMAGE_SOURCE, DAMAGE_TAGS, DAMAGED};
        use crate::vitals::Death;
        use crate::vm::{Component, MAX_INIT_WORK};
        use orpheus_content::{heroes::SOLDIER, maps::PRACTICE_RANGE_MODE};

        const FIRST: u32 = 1000;
        const BOT: u32 = 0x8000_8800;
        const BOT_CONTROLLER: u32 = 0x8000_8000;
        const KNOCKBACK: u16 = 7;
        const MINOR: u64 = 0x0d80_0000_0000_0608;
        const SEVERE: u64 = 0x0d80_0000_0000_0609;
        const ABOVE_A_TENTH: u64 = 0x0d80_0000_0000_191d;
        let data = orpheus_testkit::runtime_dir().join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
        let soldier = &graphs.bodies()[&0x0400_0000_0000_03cf];
        let explosion: crate::weapons::DamageTags = [0x03bf, 0x04b1, 0x078d, 0x0980, 0x2d30]
            .map(|tag| 0x0d80_0000_0000_0000 | tag)
            .into_iter()
            .collect();
        let hurt = |source: u32, amount: f64, direction: [f64; 3]| {
            let filter = 0x0004_0001 | (0x80_0000 << 1);
            let mut world = PlayerWorld::new(
                Arc::clone(&graphs),
                CONTROLLER,
                BODY,
                filter,
                PRACTICE_RANGE_MODE,
                SOLDIER,
            );
            world.pool_types_known = true;
            world.target_tags = soldier.target_tags.clone();
            world.targets = Some(vec![
                body(BODY, Some(0), [0.0; 3]),
                TargetCandidate {
                    controller: Some(BOT_CONTROLLER),
                    ..body(BOT, Some(1), [0.0, 0.0, 10.0])
                },
            ]);
            let mut component = Component::start(
                Arc::clone(&graphs),
                BODY,
                crate::vm::player_entity_variables(soldier, 200.0, 200.0),
                crate::vm::player_roots(soldier, &graphs).unwrap(),
                FIRST,
                &mut world,
                MAX_INIT_WORK,
            )
            .unwrap();
            component.advance_frame(FIRST, &mut world).unwrap();
            let hit = Death {
                time: 0.0,
                frame: FIRST,
                killer: source,
            };
            assert_eq!(world.vitals.damage(amount, hit).dealt, amount);
            let health = Value::Float(world.vitals.total());
            for id in [253, 1732] {
                component
                    .set_entity_variable(id, health.clone(), &mut world)
                    .unwrap();
            }
            let parameters = Variables::from([
                (DAMAGE_TAGS, explosion.to_value()),
                (DAMAGE_SOURCE, Value::Entity(source)),
                (DAMAGE_AMOUNT, Value::Float(amount)),
                (DAMAGE_DIRECTION, Value::Vector(direction)),
            ]);
            assert_ne!(
                component
                    .receive_message(DAMAGED, parameters, &mut world)
                    .unwrap(),
                0
            );
            let snapshot = component.snapshot().unwrap();
            let instance = |graph| {
                snapshot
                    .instances
                    .values()
                    .find(|instance| instance.graph == graph)
                    .unwrap()
            };
            let flinch = instance(0x02c5);
            assert!(flinch.states[&KNOCKBACK].active, "the knockback anim");
            assert_eq!(flinch.variables[&1018], Value::Vector(direction));
            assert_eq!(
                instance(0x01cf).variables[&9737],
                Value::Entity(if source == BODY {
                    CONTROLLER
                } else {
                    BOT_CONTROLLER
                })
            );
            let Value::Float(rating) = snapshot.variables[&10417] else {
                panic!("004B rates the hurt");
            };
            assert!(world.take_criteria().is_empty(), "the pain waits 0.15 s");
            let mut criteria = Vec::new();
            for frame in FIRST + 1..FIRST + 20 {
                component.advance_frame(frame, &mut world).unwrap();
                criteria.extend(
                    world
                        .take_criteria()
                        .into_iter()
                        .map(|raised| (frame - FIRST, raised.identifier, raised.audience)),
                );
            }
            let snapshot = component.snapshot().unwrap();
            assert!(
                snapshot.diagnostics.is_empty(),
                "{:?}",
                snapshot.diagnostics
            );
            (rating, criteria)
        };

        let (rating, criteria) = hurt(BOT, 120.0, [0.0, 0.0, -1.0]);
        assert!((rating - 0.888).abs() < 1e-6, "{rating}");
        // The pain's audience (`01CA` #1839) is the hurt body and its
        // attacker, once each; `191D` names none.
        assert_eq!(
            criteria,
            [
                (10, SEVERE, Some(vec![BODY, BOT])),
                (10, ABOVE_A_TENTH, None)
            ]
        );
        let (rating, criteria) = hurt(BODY, 40.0, [0.0, 1.0, 0.0]);
        assert!((rating - 0.296).abs() < 1e-6, "{rating}");
        assert_eq!(
            criteria,
            [(10, MINOR, Some(vec![BODY])), (10, ABOVE_A_TENTH, None)]
        );
    }
}
