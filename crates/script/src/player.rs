//! The world the VM sees from a player's own body (a human's, or an AI
//! hero's): what the server answers when the body's graphs ask about the
//! game.
//!
//! Each answer is either computed from the server's state or, for the few
//! questions whose meaning is still open, the answer the Python reference gives
//! for a human player's body (`TRUSTED_NATIVES` in `ow174/game/script/driver.py`).
//! GetThrottle (STU_310D6529, 0x7FF789BDA130) is the command's throttle bytes
//! times f32(1/127); the evaluator of STU_4A101787 (0x7FF789BFF730) always
//! answers false.
//! VM tests of hero graphs use this world, so they test what the server does.

use std::{collections::BTreeSet, sync::Arc};

use serde_json::Value as Json;

use crate::{
    ai_definitions::AiControl,
    expr::{EntityRelation, native_shape, team_from_filter, valid_entity},
    graph::GraphStore,
    map_tags::{MapTags, TaggedEntity},
    target_tags::{TargetTagLists, TargetTags},
    vitals::{PoolKind, Vitals},
    vm::{
        AssistLink, AuraChange, BodyEffects, CombatMod, GameMessage, HealthChange, MapRays,
        RaisedCriteria, SeenBodies, SeenBody, Site, TargetCandidate, TargetTagEffect, TrackedStat,
        Unknowns, VoiceRequest, World,
    },
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

/// The mode entity's overtime flag #3392e (its ForCode identifier `0D40`).
pub const MODE_OVERTIME: u32 = 0x0d40;

pub struct PlayerWorld {
    pub graphs: Arc<GraphStore>,
    /// The player's controller (his participant entity, `0C90`'s).
    pub controller: u32,
    pub entity: u32,
    /// An AI hero's AI entity, which possesses the body
    /// ([`crate::ai_definitions`]); none for a human player's body, whose
    /// primary possessor is his controller.
    pub ai: Option<AiControl>,
    pub body_filter: u64,
    pub position: [f64; 3],
    /// How far above its feet the body's eye is now (crouched or not); none
    /// when the match doesn't say.
    pub eye_height: Option<f64>,
    pub movement: Movement,
    /// The move-speed scale the body's mover used on its last move (mover
    /// `+0x4D8`, `STU_ED807E07`); none: the match doesn't say.
    pub speed_scale: Option<f64>,
    pub throttles: [i8; 2],
    pub mode: u64,
    /// The match's value of the mode entity's component 114 field 9
    /// (`STU_B182C5C9`, which the mode body script `0C8E` reads); none:
    /// unknown.
    pub mode_random: Option<f32>,
    /// Whether the match is in overtime: the mode entity's #3392e, the
    /// mode graph's ForCode value `0D40` (`0F4A`), which `0C8E` st34 adds
    /// 3 s of respawn time for. The server's match flow decides it while
    /// the mode graph doesn't run.
    pub mode_overtime: bool,
    /// The game struct's team entries (team, size), which `STU_D289FA32`
    /// reads ([`crate::mode::team_size`]); empty: unknown.
    pub team_sizes: Vec<(u8, u8)>,
    pub hero: u64,
    /// The hero's body definition (`003`), which names its animation data.
    pub body: Option<u64>,
    pub vitals: Vitals,
    pub pool_types_known: bool,
    pub target_tags: Option<BTreeSet<u64>>,
    /// The tags its own states add and suppress.
    target_tag_effects: TargetTags,
    /// The body the hero last eliminated and its authored target tags (the
    /// match gives them with `0093.025`, whose `01CA` rid210 tests them).
    pub victim_tags: Option<(u32, BTreeSet<u64>)>,
    pub controller_on_fire: bool,
    /// The bodies a target search can find, as the match placed them this
    /// tick (its own body among them); none: the world lists no bodies.
    pub targets: Option<Vec<TargetCandidate>>,
    /// Where the player saw the listed bodies at the view of the command
    /// his body's script runs now, for the searches that rewind
    /// (`World::seen_bodies`); none: the match can't tell.
    pub seen: Option<Vec<SeenBody>>,
    /// The map's collision the body's ray casts meet; none: no map.
    pub map_rays: Option<Arc<dyn MapRays>>,
    /// The map's tagged placeables the scene tag queries find; none: no
    /// map.
    pub map_tags: Option<Arc<MapTags>>,
    /// How the body treats graph content the VM doesn't implement; by
    /// default as every script does ([`Unknowns::for_build`]).
    pub unknowns: Unknowns,
    /// The communication wheel cosmetics the player has equipped for this
    /// hero (`BodyWorld.spray_unlocks` and the others in leafall903/ORPHEUS
    /// (MIT) `driver.py`).
    pub cosmetics: Cosmetics,
    /// Game messages to other entities, for the match to route.
    outbox: Vec<GameMessage>,
    /// The criteria the body's graphs raised on it or its controller, for
    /// the match to answer with the body's voice lines.
    criteria: Vec<RaisedCriteria>,
    /// The voice stimuli the body's server-only states asked to say.
    voices: Vec<VoiceRequest>,
    /// The health changes the body's graphs made, for the match to apply.
    health_changes: Vec<HealthChange>,
    /// The auras the body's ApplyAura states put on bodies or took off,
    /// for the match to apply.
    aura_changes: Vec<AuraChange>,
    /// The stats the body's graphs counted, for the match's stat table.
    tracked_stats: Vec<TrackedStat>,
    /// The statuses and combat modifiers its states hold.
    pub effects: BodyEffects,
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

    fn mode_random(&self) -> Option<f64> {
        self.mode_random.map(f64::from)
    }

    fn team_size(&self, team: i64) -> Option<i64> {
        crate::mode::team_size(&self.team_sizes, team)
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

    /// A message to every entity (`STU_57692020`, a primary channel's
    /// `STU_1D719351`): queued with the target [`crate::mode::EVERY_ENTITY`]
    /// for the match, which gives it to every other body.
    fn broadcast_message(&mut self, source: Site, message: u64, parameters: &Variables) -> bool {
        if source.entity != self.entity {
            return false;
        }
        self.outbox.push(GameMessage {
            sender: self.entity,
            target: crate::mode::EVERY_ENTITY,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// A criteria on the body itself or its controller that the body's
    /// voice set answers with a line (ClientInVoice 27500): the
    /// communication wheel's (`0686` raises the choice's, Hello `0AEB`) and
    /// the hero's reactions ([`REACTION_GRAPHS`]: the pain lines after a
    /// hit, `0608` / `0609`; the respawn line `05D8` and the revenge kill
    /// `0A22`, on the controller or the body). The hero graphs' other
    /// criteria (ability lines) are left out: nothing has checked them in
    /// the game yet.
    fn criteria(&mut self, site: Site, raised: RaisedCriteria) {
        if site.entity == self.entity
            && self.speaks_for(raised.target)
            && (crate::wheel::ACTION_GRAPHS.contains(&site.graph)
                || REACTION_GRAPHS.contains(&site.graph))
        {
            self.criteria.push(raised);
        }
    }

    /// A server-only voice stimulus of the hero's reactions
    /// ([`REACTION_GRAPHS`]: `01CA`'s elimination lines) aimed at the body or
    /// its controller: the body says it.
    fn voice_stimulus(&mut self, site: Site, request: VoiceRequest) -> bool {
        if site.entity != self.entity
            || !REACTION_GRAPHS.contains(&site.graph)
            || !request
                .targets
                .iter()
                .any(|&target| self.speaks_for(target))
        {
            return false;
        }
        self.voices.push(request);
        true
    }

    /// The body's own health changes wait for the match, which applies
    /// them as it applies a shot (`orpheus_sim` `Combat::modify_health`).
    fn modify_health(&mut self, change: HealthChange) -> bool {
        if change.site.entity != self.entity {
            return false;
        }
        self.health_changes.push(change);
        true
    }

    /// The body's ApplyAura changes wait for the match, which hands each to
    /// its target's component (the body itself included).
    fn change_aura(&mut self, site: Site, change: AuraChange) -> bool {
        if site.entity != self.entity {
            return false;
        }
        self.aura_changes.push(change);
        true
    }

    /// The body's stats wait for the match, which adds each to its
    /// targets' players' stats.
    fn track_stat(&mut self, site: Site, stat: TrackedStat) -> bool {
        if site.entity != self.entity {
            return false;
        }
        self.tracked_stats.push(stat);
        true
    }

    fn set_status(&mut self, site: Site, status: Option<i64>) -> bool {
        self.is_player_site(site) && self.effects.set_status(site, status)
    }

    fn combat_mod(&mut self, site: Site, modifier: Option<CombatMod>) -> bool {
        self.is_player_site(site) && self.effects.combat_mod(site, modifier)
    }

    /// The body's assist credits wait for a death, which the match credits
    /// from them.
    fn assist(&mut self, site: Site, link: Option<AssistLink>, now_ms: u64) -> bool {
        self.is_player_site(site) && self.effects.assist(site, link, now_ms)
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        if id == MODE_OVERTIME && self.mode_overtime {
            return Value::Bool(true);
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
        if !self.is_player_site(site) {
            return None;
        }
        if entity != self.entity {
            if let Some((victim, tags)) = &self.victim_tags
                && *victim == entity
            {
                return Some(tags.contains(&tag));
            }
            if let Some(listed) = self.listed(entity) {
                return listed.tags.as_ref().map(|tags| tags.contains(&tag));
            }
            return self
                .map_entity(entity)
                .map(|placed| placed.tags.contains(&tag));
        }
        if let Some(held) = self.target_tag_effects.has(tag) {
            return Some(held);
        }
        Some(self.target_tags.as_ref()?.contains(&tag))
    }

    /// The map's placeables that carry the tag ([`MapTags::with_tag`]); for
    /// a tag a body definition carries, then the body itself and the
    /// bodies the match lists that carry it, when the list is known and
    /// gives every body's tags (assumption: no other entity, such as a
    /// pet or a projectile, carries a body's tag).
    fn entities_with_tag(&self, site: Site, tag: u64) -> Option<Vec<u32>> {
        if !self.is_player_site(site) {
            return None;
        }
        let map = self.map_tags.as_ref()?;
        if !map.body_may_carry(tag) {
            return Some(map.placed_with_tag(tag));
        }
        let mut found = map.placed_with_tag(tag);
        if self.has_target_tag(site, self.entity, tag)? {
            found.push(self.entity);
        }
        for body in self.targets.as_deref()? {
            if body.entity != self.entity && body.tags.as_ref()?.contains(&tag) {
                found.push(body.entity);
            }
        }
        Some(found)
    }

    fn set_target_tag_effect(&mut self, site: Site, effect: TargetTagEffect, active: bool) -> bool {
        self.is_player_site(site)
            && effect.entity == self.entity
            && self.target_tag_effects.set(effect, active)
    }

    /// The body's own word, else a listed body's as the target searches
    /// take it ([`TargetCandidate::filter_word`]).
    fn entity_filter_flags(&self, site: Site, entity: u32) -> Option<u64> {
        if !self.is_player_site(site) {
            return None;
        }
        if entity == self.entity {
            return Some(self.body_filter);
        }
        self.listed(entity).map(TargetCandidate::filter_word)
    }

    /// The body, its controller, the mode entity (when the mode has one),
    /// the bodies the match lists and the map's tagged placeables exist;
    /// any other entity is unknown.
    fn entity_exists(&self, site: Site, entity: u32) -> Option<bool> {
        if !self.is_player_site(site) {
            return None;
        }
        let known = entity == self.entity
            || (valid_entity(self.controller) && entity == self.controller)
            || self.ai.is_some_and(|ai| ai.entity == entity)
            || (entity == GAME_MODE_ENTITY && self.graphs.mode_variables(self.mode).is_some())
            || self.listed(entity).is_some()
            || self.map_entity(entity).is_some();
        known.then_some(true)
    }

    /// The body stands where its mover put it; the other bodies where the
    /// match placed them this tick; a tagged placeable where the map puts
    /// it.
    fn entity_position(&self, site: Site, entity: u32) -> Option<[f64; 3]> {
        if !self.is_player_site(site) {
            return None;
        }
        if entity == self.entity {
            return Some(self.position);
        }
        self.listed(entity)
            .map(|body| body.feet)
            .or_else(|| self.map_entity(entity).map(|placed| placed.position))
    }

    /// The body looks along its last movement record's yaw and pitch
    /// (`Match.script_body_facing` in leafall903/ORPHEUS, commit c239a84).
    /// Other bodies' facing is unknown here.
    fn entity_facing(&self, site: Site, entity: u32) -> Option<[i32; 2]> {
        (self.is_player_site(site) && entity == self.entity)
            .then_some([self.movement.yaw, self.movement.pitch])
    }

    /// The body's own eye, as `STU_21A13145` gives it, and its facing.
    fn eye(&self, site: Site, entity: u32) -> Option<([f64; 3], [i32; 2])> {
        if !self.is_player_site(site) || entity != self.entity {
            return None;
        }
        let [x, y, z] = self.position;
        let eye = self.eye_height?;
        Some(([x, y + eye, z], [self.movement.yaw, self.movement.pitch]))
    }

    /// The body's own mover's, as the match gives it each move.
    fn speed_scale(&self, site: Site, entity: u32) -> Option<f64> {
        (self.is_player_site(site) && entity == self.entity)
            .then_some(self.speed_scale)
            .flatten()
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

    fn seen_bodies(&self, site: Site) -> Option<SeenBodies<'_>> {
        self.is_player_site(site)
            .then_some(self.seen.as_deref())
            .flatten()
            .map(SeenBodies::Rewound)
    }

    fn map_rays(&self) -> Option<&dyn MapRays> {
        self.map_rays.as_deref()
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
        // The client's readings (`EntityRelation`): this body, its
        // controller and its AI entity, and the other bodies the match
        // lists.
        let ai = self.ai.map(|ai| ai.entity);
        let known = entity == self.entity || entity == self.controller || Some(entity) == ai;
        match relation {
            // The controller, which the clients see possessing every
            // player's body. Assumption for an AI hero's body: whether
            // retail's current possessor was its AI entity is open.
            EntityRelation::CurrentPossessor if entity == self.entity => Some(self.controller),
            // The body's primary possessor (component 44): a human's
            // controller, an AI hero's AI entity (`crate::ai_definitions`).
            // A possessor is its own.
            EntityRelation::Controller if entity == self.entity => {
                Some(ai.unwrap_or(self.controller))
            }
            EntityRelation::Controller if known => Some(entity),
            EntityRelation::PossessedBody if known && entity != self.entity => Some(self.entity),
            // Neither a body nor a controller has component 72.
            EntityRelation::Owner if known => Some(0),
            // A human's body has no AI entity. An AI hero's runs none of
            // its graphs yet (AiDefinitions.md, plan step 3), so the
            // requests the body makes of it (`STU_066B4605`) find none and
            // finish at once, as a human's do.
            EntityRelation::AiControlEntity if entity == self.entity => Some(0),
            // Another body the match lists: the controller it gives, and no
            // owner.
            EntityRelation::Controller => self.listed(entity)?.controller,
            EntityRelation::Owner if self.listed(entity).is_some() => Some(0),
            // The others the listed bodies tell: another controller's body,
            // a spawned entity's owner.
            _ => crate::vm::listed_relation(self.targets.as_deref()?, relation, entity),
        }
    }

    /// Its own controller is a human player's; its AI entity has component
    /// 92 when its definition declares it (all 15 do); another listed
    /// body's as the match marks it ([`TargetCandidate::ai`]).
    fn controller_is_ai(&self, site: Site, entity: u32) -> Option<bool> {
        if !self.is_player_site(site) {
            return None;
        }
        if valid_entity(self.controller) && entity == self.controller {
            return Some(false);
        }
        if let Some(ai) = self.ai.filter(|ai| ai.entity == entity) {
            return Some(ai.has_component_92(self.graphs.ai_definitions()));
        }
        self.targets
            .as_deref()?
            .iter()
            .find(|body| body.controller == Some(entity))
            .map(|body| body.ai)
    }

    /// The profile of the body's own AI (none for a human's body: the
    /// graphs' fallback), or of another listed body's.
    fn ai_parameter(&mut self, site: Site, entity: u32, identifier: u64) -> Option<Option<f64>> {
        if !self.is_player_site(site) {
            return None;
        }
        let definitions = self.graphs.ai_definitions();
        let ai = self.ai.map(|ai| ai.entity);
        if entity == self.entity || entity == self.controller || Some(entity) == ai {
            return Some(self.ai.and_then(|ai| ai.parameter(definitions, identifier)));
        }
        self.targets
            .as_deref()?
            .iter()
            .find(|body| body.entity == entity || body.controller == Some(entity))
            .map(|body| body.ai_parameter(definitions, identifier))
    }

    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_1FF67CA6" => Some(Value::Entity(self.entity)),
            // Whether the body's primary possessor (`STU_DB742151` of the
            // owner) has component 92: a human player's controller has
            // not; an AI hero's AI entity has.
            "STU_D3EB8D87" if is_own_controller(config.get("m_00903303")) => {
                Some(Value::Bool(self.ai.is_some_and(|ai| {
                    ai.has_component_92(self.graphs.ai_definitions())
                })))
            }
            // Where the body's eye is (quick melee `0043` st5 measures the
            // eye's height above the feet with it, and looks from it,
            // `m_D59188D5`): its feet and the eye's height as the match
            // gives it. Fairly sure, from that use.
            "STU_21A13145" if self.is_player_site(site) && is_owner(config.get("m_entity")) => {
                let [x, y, z] = self.position;
                self.eye_height.map(|eye| Value::Vector([x, y + eye, z]))
            }
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
            ai: None,
            body_filter,
            position: [0.0; 3],
            eye_height: None,
            movement: Movement::default(),
            speed_scale: None,
            throttles: [0; 2],
            mode,
            mode_random: None,
            mode_overtime: false,
            team_sizes: Vec::new(),
            hero,
            vitals: Vitals::empty(),
            pool_types_known: false,
            target_tags: None,
            target_tag_effects: TargetTags::default(),
            map_rays: None,
            map_tags: None,
            controller_on_fire: false,
            targets: None,
            seen: None,
            victim_tags: None,
            unknowns: Unknowns::for_build(),
            cosmetics: Cosmetics::default(),
            outbox: Vec::new(),
            criteria: Vec::new(),
            voices: Vec::new(),
            health_changes: Vec::new(),
            aura_changes: Vec::new(),
            tracked_stats: Vec::new(),
            effects: BodyEffects::default(),
        }
    }

    /// The health changes the body's graphs made since the last call, in
    /// order.
    pub fn take_health_changes(&mut self) -> Vec<HealthChange> {
        std::mem::take(&mut self.health_changes)
    }

    /// The tags the body's states add and suppress now, when a tag came or
    /// went since the last call ([`TargetTags::take_change`]).
    pub fn take_target_tag_change(&mut self) -> Option<TargetTagLists> {
        self.target_tag_effects.take_change()
    }

    /// The aura changes the body's graphs made since the last call, in
    /// order.
    pub fn take_aura_changes(&mut self) -> Vec<AuraChange> {
        std::mem::take(&mut self.aura_changes)
    }

    /// The stats the body's graphs counted since the last call, in order.
    pub fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        std::mem::take(&mut self.tracked_stats)
    }

    /// The game messages the body sent other entities since the last call,
    /// in order (`Match.queue_body_message` in `ow174/game/match.py`); the
    /// recipient takes each with `Component::receive_message`.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }

    /// The criteria the body's graphs raised on it or its controller since
    /// the last call.
    pub fn take_criteria(&mut self) -> Vec<RaisedCriteria> {
        std::mem::take(&mut self.criteria)
    }

    /// The voice stimuli the body's graphs asked to say since the last call.
    pub fn take_voices(&mut self) -> Vec<VoiceRequest> {
        std::mem::take(&mut self.voices)
    }

    /// Whether a line aimed at `target` is the body's to say: the body
    /// itself, or its controller or AI entity (which have no voice set; the
    /// voice is their possessed body's).
    fn speaks_for(&self, target: u32) -> bool {
        target == self.entity
            || (valid_entity(self.controller) && target == self.controller)
            || self.ai.is_some_and(|ai| ai.entity == target)
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

    /// A tagged placeable of the map.
    fn map_entity(&self, entity: u32) -> Option<&TaggedEntity> {
        self.map_tags.as_ref()?.get(entity)
    }
}

/// `{"$": "STU_B5A0CAF0"}` (the owner), optionally with a zero
/// `m_EE729DCB`.
fn is_owner(config: Option<&Json>) -> bool {
    config.and_then(Json::as_object).is_some_and(|fields| {
        fields.get("$").and_then(Json::as_str) == Some("STU_B5A0CAF0")
            && fields.iter().all(|(key, value)| match key.as_str() {
                "$" => true,
                "m_EE729DCB" => value.as_u64() == Some(0),
                _ => false,
            })
    })
}

/// `STU_DB742151` (the controller) of the owner.
fn is_own_controller(config: Option<&Json>) -> bool {
    config.is_some_and(|config| {
        native_shape(config, "STU_DB742151", Some("m_entity")) && is_owner(config.get("m_entity"))
    })
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
            ai: false,
            ai_control: None,
            feet,
            height: 1.8,
            radius: 0.4,
            yaw: 0.0,
            spawned: Vec::new(),
            tags: None,
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
        // A listed body's controller is the one the match gives it; a body
        // has no owner (component 72).
        assert_eq!(
            world.related_entity(site, EntityRelation::Controller, 0xa000_0102),
            Some(0xa000_0100)
        );
        assert_eq!(
            world.related_entity(site, EntityRelation::Owner, 0xa000_0102),
            Some(0)
        );
        assert_eq!(
            world.related_entity(site, EntityRelation::Controller, 0xa000_0104),
            None
        );
        let other_site = Site {
            entity: 0xa000_0102,
            ..site
        };
        assert_eq!(world.entity_exists(other_site, BODY), None);
    }

    /// An AI hero's body is possessed by its AI entity, which has component
    /// 92 and answers the AI parameters from its profile; another listed
    /// AI hero's body (an aura's instigator, `#27`) answers the same from
    /// the match's list. A human's body has his controller, no AI, and
    /// takes the graphs' fallbacks.
    #[test]
    fn an_ai_hero_s_body_is_possessed_by_its_ai_entity() {
        use crate::ai_definitions::{AiControl, AiDefinitions, EASY, HARD};

        const DAMAGE: u64 = 0x0d80_0000_0000_1961;
        const DEFINITION: u64 = 0x0400_0000_0000_11f4;
        const AI: u32 = 0x8000_7001;
        const OTHER_BODY: u32 = 0xa000_0201;
        const OTHER_AI: u32 = 0x8000_7002;
        let definitions = AiDefinitions::from_json(
            br#"{"heroes": {}, "definitions": {"0x04000000000011F4": {
                    "component_92": true,
                    "difficulty_profiles": {
                        "0x0D8000000000195E": "0x0B50000000000013",
                        "0x0D80000000001960": "0x0B50000000000014"}}},
                "profiles": {
                    "0x0B50000000000013": {"values": {"0x0D80000000001961": 4.0}},
                    "0x0B50000000000014": {"values": {"0x0D80000000001961": 20.0}}}}"#,
        )
        .unwrap();
        let graphs =
            GraphStore::from_json(br#"{"version":1,"graphs":[],"bodies":{},"missing":[]}"#)
                .unwrap()
                .with_ai_definitions(Arc::new(definitions));
        let mut world = PlayerWorld::new(Arc::new(graphs), CONTROLLER, BODY, 0x0004_0001, 0, 0);
        let site = Site {
            entity: BODY,
            instance: 1,
            graph: 42,
            node: None,
        };
        let control = |entity, difficulty| AiControl {
            entity,
            definition: DEFINITION,
            difficulty,
        };
        world.targets = Some(vec![TargetCandidate {
            controller: Some(OTHER_AI),
            ai: true,
            ai_control: Some(control(OTHER_AI, HARD)),
            ..body(OTHER_BODY, Some(1), [0.0; 3])
        }]);

        // A human's body.
        let controller = EntityRelation::Controller;
        assert_eq!(
            world.related_entity(site, controller, BODY),
            Some(CONTROLLER)
        );
        assert_eq!(world.controller_is_ai(site, CONTROLLER), Some(false));
        assert_eq!(world.ai_parameter(site, BODY, DAMAGE), Some(None));

        world.ai = Some(control(AI, EASY));
        assert_eq!(world.related_entity(site, controller, BODY), Some(AI));
        assert_eq!(world.related_entity(site, controller, AI), Some(AI));
        assert_eq!(
            world.related_entity(site, EntityRelation::PossessedBody, AI),
            Some(BODY)
        );
        assert_eq!(world.entity_exists(site, AI), Some(true));
        assert_eq!(world.controller_is_ai(site, AI), Some(true));
        assert_eq!(world.controller_is_ai(site, CONTROLLER), Some(false));
        for entity in [BODY, AI] {
            assert_eq!(world.ai_parameter(site, entity, DAMAGE), Some(Some(4.0)));
        }
        // A value the profile lacks: the graphs' fallback.
        assert_eq!(world.ai_parameter(site, BODY, DAMAGE + 1), Some(None));

        // The other AI hero's body, by the match's list.
        assert_eq!(
            world.related_entity(site, controller, OTHER_BODY),
            Some(OTHER_AI)
        );
        assert_eq!(world.controller_is_ai(site, OTHER_AI), Some(true));
        for entity in [OTHER_BODY, OTHER_AI] {
            assert_eq!(world.ai_parameter(site, entity, DAMAGE), Some(Some(20.0)));
        }
        assert_eq!(world.ai_parameter(site, 0xa000_0301, DAMAGE), None);
    }

    /// Listed bodies' target tags and filter words come from the match's
    /// list; a tag a body definition carries lists the map's placeables,
    /// the body and the listed bodies with it, once every listed body's
    /// tags are known.
    #[test]
    fn listed_bodies_answer_their_tags_and_filter_words() {
        const HERO: u64 = 0x0d60_0000_0000_00f8;
        const PLACE: u64 = 0x0d60_0000_0000_0310;
        const OTHER: u32 = 0xa000_0102;
        const BOT: u32 = 0xa000_0103;
        let graphs = Arc::new(
            GraphStore::from_json(
                br#"{"version":1,"graphs":[],"missing":[],"bodies":{"0x0400000000000001":
                {"hero":null,"name":"Hero","manager":null,"weapons":[],"graphs":[],
                "target_tags":["0x0D600000000000F8"]}}}"#,
            )
            .unwrap(),
        );
        let filter = 0x0001_0041 | 0x80_0000;
        let mut world = PlayerWorld::new(Arc::clone(&graphs), CONTROLLER, BODY, filter, 0, 0);
        world.target_tags = Some(BTreeSet::from([HERO]));
        let placed = TaggedEntity {
            entity: 0x8000_0010,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            tags: BTreeSet::from([HERO, PLACE]),
        };
        world.map_tags = Some(Arc::new(MapTags::new(vec![placed], &graphs)));
        let tagged = |entity, team, tags: Option<&[u64]>| TargetCandidate {
            tags: tags.map(|tags| Arc::new(tags.iter().copied().collect())),
            ..body(entity, team, [0.0; 3])
        };
        world.targets = Some(vec![
            tagged(BODY, Some(0), Some(&[HERO])),
            tagged(OTHER, Some(1), Some(&[HERO])),
            tagged(BOT, Some(1), None),
        ]);
        let site = Site {
            entity: BODY,
            instance: 1,
            graph: 42,
            node: None,
        };
        assert_eq!(world.has_target_tag(site, OTHER, HERO), Some(true));
        assert_eq!(world.has_target_tag(site, OTHER, PLACE), Some(false));
        assert_eq!(world.has_target_tag(site, BOT, HERO), None);
        assert_eq!(
            world.entity_filter_flags(site, OTHER),
            Some(0x80_0000 << 1 | 0x0001_0041)
        );
        assert_eq!(world.entity_filter_flags(site, 0xa000_0104), None);
        assert_eq!(
            world.entities_with_tag(site, PLACE),
            Some(vec![0x8000_0010])
        );
        // The bot's tags are unknown: it might carry the hero's tag.
        assert_eq!(world.entities_with_tag(site, HERO), None);
        world.targets.as_mut().unwrap()[2] = tagged(BOT, Some(1), Some(&[]));
        assert_eq!(
            world.entities_with_tag(site, HERO),
            Some(vec![0x8000_0010, BODY, OTHER])
        );
    }

    /// A hit Soldier: 76 survives reaches his body as the damage message
    /// `0011.025`, as the match sends it. `004B` rates the hurt (#10417e),
    /// `02C5` starts the networked knockback anim st7 for a splash hit
    /// (Helix Rocket explosion tags: `078D`, not `0302`) with the hit's
    /// direction, `01CF` asks for the attacker's pet owner (#9737, `STU_F76C486C`:
    /// component 72's owner, so none for a hero body or a bot), and
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
        // Another player's body, whose controller is its id less the low
        // byte (`orpheus_sim::combat` `target_candidates`).
        const ENEMY: u32 = 0xa000_0201;
        const KNOCKBACK: u16 = 7;
        const MINOR: u64 = 0x0d80_0000_0000_0608;
        const SEVERE: u64 = 0x0d80_0000_0000_0609;
        const ABOVE_A_TENTH: u64 = 0x0d80_0000_0000_191d;
        let data = orpheus_testkit::runtime_dir().join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
        let soldier = &graphs.bodies()[&0x0400_0000_0000_03cf];
        let tags = |ids: &[u64]| -> crate::weapons::DamageTags {
            ids.iter().map(|tag| 0x0d80_0000_0000_0000 | tag).collect()
        };
        let explosion = tags(&[0x03bf, 0x04b1, 0x078d, 0x0980, 0x2d30]);
        // The Heavy Pulse Rifle's bullet (`0254` rid 23), less its one tag
        // read from a variable.
        let bullet = tags(&[0x03bf, 0x09a2, 0x1e1e, 0x2d2f, 0x4d29]);
        let hurt = |source: u32, amount: f64, tags: crate::weapons::DamageTags, direction| {
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
                body(ENEMY, Some(1), [0.0, 0.0, 10.0]),
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
                (DAMAGE_TAGS, tags.to_value()),
                (DAMAGE_SOURCE, Value::Entity(source)),
                (DAMAGE_AMOUNT, Value::Float(amount)),
                (DAMAGE_DIRECTION, Value::Vector(direction)),
            ]);
            assert_ne!(
                component
                    .receive_message(0, DAMAGED, parameters, &mut world)
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
            let knockback = flinch
                .states
                .get(&KNOCKBACK)
                .is_some_and(|state| state.active);
            let strength = flinch.variables.get(&1024).cloned();
            assert_eq!(flinch.variables[&1018], Value::Vector(direction));
            // `STU_F76C486C` is component 72's (STUPetComponent's) owner in the
            // client's evaluator (`0x7FF789BF2CF0` → `0x7FF789C2BEE0`, which
            // reads `GetComp72`); a hero body or a Training Bot has none, so
            // `01CF` keeps no assist attacker for a direct hit (see loose ends).
            assert_eq!(instance(0x01cf).variables[&9737], Value::Entity(0));
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
            (rating, criteria, knockback, strength)
        };

        let (rating, criteria, knockback, _) = hurt(BOT, 120.0, explosion, [0.0, 0.0, -1.0]);
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
        assert!(knockback, "the knockback anim");
        let (rating, criteria, knockback, _) = hurt(BODY, 40.0, explosion, [0.0, 1.0, 0.0]);
        assert!((rating - 0.296).abs() < 1e-6, "{rating}");
        assert_eq!(
            criteria,
            [(10, MINOR, Some(vec![BODY])), (10, ABOVE_A_TENTH, None)]
        );
        assert!(knockback, "the knockback anim");
        // An enemy Soldier: 76's bullet of 19: the gun tag `09A2` flinches
        // (0.4), no knockback; 0.095 + 0.3 × 19 / 125 = 0.1406. The pain's
        // audience is the hurt body and the enemy player's body.
        let (rating, criteria, knockback, strength) = hurt(ENEMY, 19.0, bullet, [0.0, 0.0, -1.0]);
        assert!((rating - 0.1406).abs() < 1e-6, "{rating}");
        assert_eq!(
            criteria,
            [
                (10, MINOR, Some(vec![BODY, ENEMY])),
                (10, ABOVE_A_TENTH, None)
            ]
        );
        assert!(!knockback);
        assert_eq!(strength, Some(Value::Float(f64::from(0.4_f32))));
        // The same bullet from an Easy AI Soldier: 76 deals his profile's 4
        // (`1961` at `195E`, `crate::ai_definitions`): 0.02 +
        // 0.3 × 4 / 125 = 0.0296, at most 0.1, so it flinches but raises no
        // pain line.
        let (rating, criteria, _, strength) = hurt(ENEMY, 4.0, bullet, [0.0, 0.0, -1.0]);
        assert!((rating - 0.0296).abs() < 1e-6, "{rating}");
        assert!(criteria.is_empty(), "{criteria:?}");
        assert_eq!(strength, Some(Value::Float(f64::from(0.4_f32))));
        // The rocket's message without its tags (`.hurt flinch off`,
        // `orpheus_sim::hurt_feedback`): `004B` rates the amount and `01CA`
        // leaves out only `191F`/`0262`, so the rating and the pain line
        // stay; `02C5` finds no gun tag and no `078D`: no flinch, no
        // knockback.
        let (rating, criteria, knockback, strength) = hurt(
            BOT,
            120.0,
            crate::weapons::DamageTags::NONE,
            [0.0, 0.0, -1.0],
        );
        assert!((rating - 0.888).abs() < 1e-6, "{rating}");
        assert_eq!(
            criteria,
            [
                (10, SEVERE, Some(vec![BODY, BOT])),
                (10, ABOVE_A_TENTH, None)
            ]
        );
        assert!(!knockback);
        assert_eq!(strength, None, "no flinch");
    }

    /// The hero's own lines from `01CA`. An elimination (`0093.025` with
    /// the final blow) of a Training Bot waits 0.7 s (the 44th 16 ms frame),
    /// then begins the server-only `VoiceStimulus` st46: his elimination
    /// line `0008.078`, said 0.5 s later; a melee kill (tag `0302` in
    /// `0355`) begins st45 instead: `0032.078`. The revive message
    /// (`0013.025`, `00C6` on) raises the respawn line `05D8` on his
    /// controller 1.5 s later (the 94th frame). Nothing in the paths is
    /// unsupported.
    #[test]
    #[ignore = "requires the derived 104319 statescript catalog"]
    fn the_hero_says_his_elimination_and_respawn_lines() {
        use crate::bot::RESET;
        use crate::controller::ELIMINATION;
        use crate::vm::{Component, MAX_INIT_WORK, RaisedCriteria, VoiceRequest};
        use orpheus_content::{heroes::SOLDIER, maps::PRACTICE_RANGE_MODE};

        const FIRST: u32 = 1000;
        const BOT: u32 = 0x8000_8800;
        const TRAINING_BOT_TAG: u64 = 0x0d60_0000_0000_02ed;
        let data = orpheus_testkit::runtime_dir().join("data");
        let graphs = Arc::new(GraphStore::load_runtime(&data, "game_graphs_174.json.gz").unwrap());
        let soldier = &graphs.bodies()[&0x0400_0000_0000_03cf];
        let tags = |ids: &[u64]| -> crate::weapons::DamageTags {
            ids.iter().map(|tag| 0x0d80_0000_0000_0000 | tag).collect()
        };
        let start = || {
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
                    controller: Some(0x8000_8000),
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
            // The game identifier the match sends every body as it starts
            // (`002D.025`, `0029`): `01C7` rid33 sets #1928e from it.
            let identifier = Variables::from([(0x29, Value::Asset(0))]);
            component
                .receive_message(0, 0x0240_0000_0000_002d, identifier, &mut world)
                .unwrap();
            (world, component)
        };
        // The frames after `message` at which the body asked for a voice
        // stimulus or raised a criteria.
        type Said = (Vec<(u32, VoiceRequest)>, Vec<(u32, RaisedCriteria)>);
        let said = |message: u64, parameters: Variables| -> Said {
            let (mut world, mut component) = start();
            world.victim_tags = Some((BOT, BTreeSet::from([TRAINING_BOT_TAG])));
            assert_ne!(
                component
                    .receive_message(0, message, parameters, &mut world)
                    .unwrap(),
                0
            );
            let (mut voices, mut criteria) = (Vec::new(), Vec::new());
            for frame in FIRST + 1..FIRST + 120 {
                component.advance_frame(frame, &mut world).unwrap();
                voices.extend(world.take_voices().into_iter().map(|v| (frame - FIRST, v)));
                criteria.extend(
                    world
                        .take_criteria()
                        .into_iter()
                        .map(|c| (frame - FIRST, c)),
                );
            }
            let snapshot = component.snapshot().unwrap();
            assert!(
                snapshot.diagnostics.is_empty(),
                "{:?}",
                snapshot.diagnostics
            );
            (voices, criteria)
        };
        let kill = |tags: crate::weapons::DamageTags| {
            Variables::from([
                (0x15, Value::Entity(BOT)),
                (0x12, Value::Entity(BODY)),
                (0x91, Value::Bool(true)),
                (0x3d, tags.to_value()),
                (0x355, tags.to_value()),
            ])
        };
        let line = |stimulus: u64| VoiceRequest {
            stimulus,
            targets: vec![BODY],
            delay: 0.5,
            audience: None,
        };
        let bullet = tags(&[0x03bf, 0x09a2, 0x1e1e, 0x2d2f, 0x4d29]);
        assert_eq!(
            said(ELIMINATION, kill(bullet)),
            (vec![(44, line(0x0ee0_0000_0000_0008))], Vec::new())
        );
        let melee = tags(&[0x0302]);
        assert_eq!(
            said(ELIMINATION, kill(melee)),
            (vec![(44, line(0x0ee0_0000_0000_0032))], Vec::new())
        );
        // A second kill asks again: the state finished after the first
        // (a state left on would ignore the begin; game check 2026-11-17).
        let (mut world, mut component) = start();
        world.victim_tags = Some((BOT, BTreeSet::from([TRAINING_BOT_TAG])));
        let mut asked = Vec::new();
        for kill_frame in [FIRST + 1, FIRST + 61] {
            component
                .receive_message(0, ELIMINATION, kill(bullet), &mut world)
                .unwrap();
            for frame in kill_frame..kill_frame + 60 {
                component.advance_frame(frame, &mut world).unwrap();
                asked.extend(world.take_voices().into_iter().map(|v| v.stimulus));
            }
        }
        assert_eq!(asked, [0x0ee0_0000_0000_0008; 2]);
        let respawn = Variables::from([(0xc6, Value::Bool(true))]);
        assert_eq!(
            said(RESET, respawn),
            (
                Vec::new(),
                vec![(
                    94,
                    RaisedCriteria {
                        identifier: 0x0d80_0000_0000_05d8,
                        target: CONTROLLER,
                        audience: None,
                    }
                )]
            )
        );
    }
}
