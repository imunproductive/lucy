//! The server-owned map placeables' statescript: one component per placed
//! entity, as the client starts a placed script (leafall903/ORPHEUS
//! `docs/PLACED_SCRIPT_INITIALIZATION.txt`, BE8E10): the definition's
//! initial graphs in order, then the instance data's overrides written into
//! the first of those graphs with the same index (an override without one
//! is skipped), then the instance data's extra graphs.
//!
//! The placeables' target searches find the players. A component that
//! meets content the VM doesn't implement (refused in debug builds) or
//! answers by assumption is not complete: the server doesn't send its
//! frames (see [`PlacedEntity::complete`]); one that hasn't faulted is
//! running, and the server routes its messages ([`PlacedEntity::running`]).
//!
//! The same world runs the entities the game spawns during the match
//! ([`MapEntityWorld::spawn`]: Soldier: 76's Biotic Field `03DC`, whose
//! `0258` searches for allies, puts its aura `025F` on them and destroys
//! itself). A spawned entity has an owner (its hero's controller), puts
//! auras on the bodies ([`MapEntityWorld::take_auras`]) and removes itself
//! (`STU_523EE38D`), which ends its graphs, so its ApplyAura takes its
//! auras off ([`MapEntityWorld::take_destroyed`]). Assumption: the map's
//! own placeables do neither yet (their auras and removals have no client
//! side here).
//!
//! A script can make and remove dummy bots (`STU_FC3921B1`,
//! `STU_C1561031`: the Practice Range's Training Partner console `265F`):
//! the match gives the player and body the next one gets
//! ([`MapEntityWorld::set_next_dummy`]), the graph reads the body at once,
//! and the match makes the bot ([`MapEntityWorld::take_dummy_changes`]).

use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value as Json;

use crate::{
    expr::EntityRelation,
    graph::{GraphBinding, GraphStore},
    map_tags::{MapTags, TaggedEntity},
    target_tags::TargetTags,
    vm::{
        AuraChange, Component, DummyBot, GameMessage, InitFault, MAX_INIT_WORK, MapRays,
        MessageEnd, ParentWrite, Root, ScopeChange, ScopeSource, SeenBodies, Site, Snapshot,
        TargetCandidate, TargetTagEffect, TrackedStat, Unknowns, World, bind_values,
        listed_relation,
    },
    wire::{Value, Variables},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

/// One placed entity as the map places it.
pub struct Placement<'a> {
    /// `0x80000000 | flat index`.
    pub entity: u32,
    /// The entity definition (`003`).
    pub definition: u64,
    pub position: [f64; 3],
    /// `STUStatescriptComponentInstanceData.m_6D10093E`.
    pub overrides: &'a [GraphBinding],
    /// `STUStatescriptComponentInstanceData.m_84D0983B`.
    pub extra: &'a [GraphBinding],
}

/// An entity the game spawns during the match ([`MapEntityWorld::spawn`]).
pub struct Spawn<'a> {
    pub entity: u32,
    /// Its definition's initial graphs, in order (instance ids from 1, as
    /// [`crate::vm::body_roots`] numbers them).
    pub graphs: &'a [u32],
    pub position: [f64; 3],
    /// Its owner (component 72): the hero's controller.
    pub owner: u32,
    /// The body that shot it, when it is a projectile (component 50
    /// `+0x18`, [`EntityRelation::Shooter`]); none for an entity a
    /// projectile left (the Biotic Field).
    pub shooter: Option<ProjectileShooter>,
}

/// The body that shot a spawned projectile, and its hero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectileShooter {
    pub body: u32,
    /// The hero (`075`) whose ruleset defaults the projectile's graphs
    /// read besides the mode's: a GetGameRulesetValue of its shooter's
    /// member (the Biotic Orbs' `STU_2B2E2F6D(self).#12618`/`#12619`, 1.0
    /// in Moira's schema `0A3…003B`) falls back to the key's default, as
    /// the shooter's body's own graphs read it (assumption: the client's
    /// default for an entity's member is that entity's hero's).
    pub hero: u64,
}

/// The player and first body the next dummy bot gets
/// ([`MapEntityWorld::set_next_dummy`]): the match picks them, the graph
/// that makes the bot reads the body at once (`STU_FC3921B1`'s output).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NextDummy {
    pub player: u32,
    pub body: u32,
}

/// A dummy bot change the scripts asked for, for the match to carry out
/// ([`MapEntityWorld::take_dummy_changes`]).
#[derive(Clone, Debug, PartialEq)]
pub enum DummyChange {
    /// Make `dummy` the player `next.player`, whose first body is
    /// `next.body`.
    Create { dummy: DummyBot, next: NextDummy },
    /// Remove the dummy bot in a team's slot, if any.
    Remove { team: i64, slot: i64 },
}

/// The roots a placement starts with, in the client's order (see the
/// module notes). The definition's graphs keep their list position as
/// instance id, as a body's do; the extra graphs follow.
pub fn placed_roots(
    graphs: &GraphStore,
    placement: &Placement<'_>,
) -> Result<Vec<Root>, InitFault> {
    let mut roots = match graphs.entity_body(placement.definition) {
        Some(body) => crate::vm::body_roots(body)?,
        None => Vec::new(),
    };
    for binding in placement.overrides {
        let Some(graph) = binding.graph else {
            continue;
        };
        if let Some(root) = roots.iter_mut().find(|root| root.graph == graph) {
            bind_values(root, binding.sync.iter().flatten())?;
        }
    }
    let mut id = roots.last().map_or(0, |root| root.id);
    for binding in placement.extra {
        let Some(graph) = binding.graph else {
            continue;
        };
        id = id.checked_add(1).ok_or(InitFault::InstanceIdExhausted)?;
        let mut root = Root::new(id, graph);
        bind_values(&mut root, binding.sync.iter().flatten())?;
        roots.push(root);
    }
    Ok(roots)
}

/// Classes or faults by graph, with how often they were met.
pub type Gaps = BTreeMap<(u32, String), usize>;

/// A placed entity's script.
pub struct PlacedEntity {
    pub entity: u32,
    pub position: [f64; 3],
    component: Option<Component>,
    /// Why the component stopped or never started.
    fault: Option<String>,
    /// The team index its script gave it (`STU_0F647E55`), if any.
    team: Option<i64>,
    /// The tags its own states add and suppress (`STU_E56BA8D8`).
    target_tags: TargetTags,
    /// A spawned entity's owner ([`Spawn::owner`]).
    owner: Option<u32>,
    /// A spawned projectile's shooter ([`Spawn::shooter`]).
    shooter: Option<ProjectileShooter>,
    /// A spawned projectile's speed scale (`STU_D3F71FCF`); 1 otherwise.
    speed_scale: f64,
}

impl PlacedEntity {
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.as_ref()?.snapshot()
    }

    /// Whether its graphs ran only on what the VM implements: no gap was
    /// refused and no answer was an assumption.
    pub fn complete(&self) -> bool {
        self.fault.is_none()
            && self.snapshot().is_some_and(|snapshot| {
                snapshot.initialization_complete() && snapshot.assumptions.is_empty()
            })
    }

    /// Whether its graphs are running: initialized and not faulted, though
    /// perhaps with assumptions (the server forwards such a script's
    /// messages, not its frames).
    pub fn running(&self) -> bool {
        self.fault.is_none()
            && self
                .snapshot()
                .is_some_and(|snapshot| snapshot.initialization_complete())
    }

    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// The team index its script gave it (`STU_0F647E55`; -1 none, 0 to
    /// 4); None: its script gave none.
    pub fn team(&self) -> Option<i64> {
        self.team
    }

    /// A spawned projectile's speed scale: what a speed scale state of its
    /// graphs holds (`STU_D3F71FCF`, the Biotic Orbs' 0.275 while they have
    /// targets), else 1.
    pub fn speed_scale(&self) -> f64 {
        self.speed_scale
    }
}

/// The volumes of the placed entities that have one: a spawn room's area
/// (`STU_73E1C5F0`), from the map data the match holds.
pub trait EntityVolumes: Send + Sync {
    /// Whether `point` is inside `entity`'s volume; None: the entity has
    /// none known.
    fn contains(&self, entity: u32, point: [f64; 3]) -> Option<bool>;
}

/// What the match lends the placed scripts each frame.
struct Shared<'a> {
    graphs: &'a GraphStore,
    mode: u64,
    ruleset: u64,
    mode_random: f32,
    unknowns: Unknowns,
    players: &'a [TargetCandidate],
    map_tags: Option<&'a MapTags>,
    map_rays: Option<&'a dyn MapRays>,
    volumes: Option<&'a dyn EntityVolumes>,
    outbox: &'a mut Vec<GameMessage>,
    next_dummy: &'a mut Option<NextDummy>,
    dummies: &'a mut Vec<DummyChange>,
    auras: &'a mut Vec<AuraChange>,
    destroyed: &'a mut Vec<u32>,
    ends: &'a mut Vec<MessageEnd>,
    tracked_stats: &'a mut Vec<TrackedStat>,
    scopes: &'a mut Vec<ScopeChange>,
}

impl Shared<'_> {
    /// A tagged placeable of the map.
    fn tagged(&self, entity: u32) -> Option<&TaggedEntity> {
        self.map_tags?.get(entity)
    }
}

/// The world one placed entity's script sees: itself where the map put it,
/// and the players.
struct EntityWorld<'a, 'b> {
    shared: &'b mut Shared<'a>,
    entity: u32,
    position: [f64; 3],
    team: &'b mut Option<i64>,
    target_tags: &'b mut TargetTags,
    owner: Option<u32>,
    shooter: Option<ProjectileShooter>,
    speed_scale: &'b mut f64,
}

impl World for EntityWorld<'_, '_> {
    fn unknowns(&self) -> Unknowns {
        self.shared.unknowns
    }

    fn map_rays(&self) -> Option<&dyn MapRays> {
        self.shared.map_rays
    }

    /// The mode's ruleset defaults; a spawned projectile's also its
    /// shooter's hero's ([`ProjectileShooter::hero`]).
    fn ruleset(&mut self, id: u32) -> Option<Value> {
        let hero = self.shooter.map_or(0, |shooter| shooter.hero);
        self.shared
            .graphs
            .ruleset_value(id, hero, self.shared.mode, self.shared.ruleset)
    }

    fn mode_random(&self) -> Option<f64> {
        Some(f64::from(self.shared.mode_random))
    }

    fn position(&mut self, entity: u32) -> [f64; 3] {
        let site = Site {
            entity: self.entity,
            instance: 0,
            graph: 0,
            node: None,
        };
        self.entity_position(site, entity).unwrap_or_default()
    }

    /// The mode entity's variables are the game's (as [`crate::mode`]).
    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.shared
            .graphs
            .mode_variables(self.shared.mode, self.shared.ruleset)
            .and_then(|variables| variables.get(&id).copied())
            .map_or(Value::Null, Value::from)
    }

    /// `STU_A3DEF368` is the mode entity (the first entity with component
    /// 114, as [`crate::mode`] answers it): the Practice Range pad `26F8`
    /// and console `265F` read ruleset 29450 through it. `STU_6EA8533F`
    /// is every player (the console's scope 126): the client
    /// (0x7FF789BEC4C0, its list from `0x7FF7895BC390`) walks the pool of
    /// component 74, the player component of the players' controllers,
    /// AI players' too: the controllers of the listed bodies, in their
    /// order (the room's slots; the pool's order is its entities' creation
    /// order [inference], and orders only the scope state's frame record).
    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_082FA44E" => self
                .shared
                .graphs
                .tunables()
                .value(config)
                .map(Value::Float),
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_6EA8533F" => {
                let mut players = Vec::new();
                for controller in self
                    .shared
                    .players
                    .iter()
                    .filter_map(|player| player.controller)
                {
                    if !players.contains(&Value::Entity(controller)) {
                        players.push(Value::Entity(controller));
                    }
                }
                Some(Value::Array(players))
            }
            _ => None,
        }
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        match class {
            // As `PlayerWorld`: the tunable's authored value, not scaled
            // for the entity.
            "STU_082FA44E" => Some("the tunable's authored value"),
            _ => None,
        }
    }

    /// Its own team is the one its script gave it, else a spawned
    /// entity's owner's; a player's (or his controller's), the match's.
    fn entity_team(&self, _site: Site, entity: u32) -> Option<i64> {
        let entity = if entity == self.entity {
            if self.team.is_some() {
                return *self.team;
            }
            self.owner?
        } else {
            entity
        };
        self.shared
            .players
            .iter()
            .find(|player| player.entity == entity || player.controller == Some(entity))?
            .team
            .map(i64::from)
    }

    /// A placed entity keeps the team its script gives itself (the
    /// Practice Range's spawn markers `01CE`); it can't give one to
    /// another entity.
    fn set_entity_team(&mut self, _site: Site, entity: u32, team: i64) -> bool {
        if entity != self.entity {
            return false;
        }
        *self.team = Some(team);
        true
    }

    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        Some(self.shared.players)
    }

    /// A map placeable has no player: it sees the bodies where they are. A
    /// spawned entity's owner's player saw them earlier, at a view the
    /// match doesn't give its world: unknown.
    fn seen_bodies(&self, _site: Site) -> Option<SeenBodies<'_>> {
        self.owner.is_none().then_some(SeenBodies::Listed)
    }

    /// The map's placeables that carry the tag ([`MapTags::with_tag`]).
    fn entities_with_tag(&self, _site: Site, tag: u64) -> Option<Vec<u32>> {
        self.shared.map_tags?.with_tag(tag)
    }

    /// Its own tags as its states hold them, else the map's authored ones.
    fn has_target_tag(&self, _site: Site, entity: u32, tag: u64) -> Option<bool> {
        if entity == self.entity {
            if let Some(held) = self.target_tags.has(tag) {
                return Some(held);
            }
        }
        Some(self.shared.tagged(entity)?.tags.contains(&tag))
    }

    /// A target tag state on itself (the Practice Range's spawn markers'
    /// `01CE` st1 gives each `0035` while it holds the mode's `0724`); it
    /// can't tag another entity. Only its own script sees the change: the
    /// placeables' component 76 isn't sent, and other entities' tag
    /// searches find the authored tags ([`MapTags`]).
    fn set_target_tag_effect(
        &mut self,
        _site: Site,
        effect: TargetTagEffect,
        active: bool,
    ) -> bool {
        effect.entity == self.entity && self.target_tags.set(effect, active)
    }

    /// Itself, the players and the map's tagged placeables.
    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        Some(
            entity == self.entity
                || self
                    .shared
                    .players
                    .iter()
                    .any(|player| player.entity == entity)
                || self.shared.tagged(entity).is_some(),
        )
    }

    fn entity_position(&self, _site: Site, entity: u32) -> Option<[f64; 3]> {
        if entity == self.entity {
            return Some(self.position);
        }
        self.shared
            .players
            .iter()
            .find(|player| player.entity == entity)
            .map(|player| player.feet)
            .or_else(|| self.shared.tagged(entity).map(|placed| placed.position))
    }

    /// A tagged placeable faces its placement's forward axis
    /// ([`TaggedEntity::forward`]): the client reads an entity's world
    /// transform for `STU_2200A66D`/`STU_021D01CD` (0x7FF789C28780,
    /// ScriptGap.md), a body's along its aim. The console `265F` gives its
    /// Training Partner the facing of its pad `3992` so (#634).
    fn entity_facing(&self, _site: Site, entity: u32) -> Option<[i32; 2]> {
        let [x, y, z] = self.shared.tagged(entity)?.forward();
        let units = |angle: f64| (angle * 65_536.0 / std::f64::consts::TAU).round() as i32;
        Some([units(x.atan2(z)), units((-y).clamp(-1.0, 1.0).asin())])
    }

    fn send_message(
        &mut self,
        _source: Site,
        target: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        self.shared.outbox.push(GameMessage {
            sender: self.entity,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// A message to every entity (`STU_1D719351`): queued with the target
    /// [`crate::mode::EVERY_ENTITY`], as the mode graph's.
    fn broadcast_message(&mut self, _source: Site, message: u64, parameters: &Variables) -> bool {
        self.shared.outbox.push(GameMessage {
            sender: self.entity,
            target: crate::mode::EVERY_ENTITY,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// A spawned entity's owner is its hero's controller, a spawned
    /// projectile's shooter the body that shot it (a map placeable, a
    /// spawned field: none); the others, what the listed bodies tell.
    fn related_entity(
        &mut self,
        _site: Site,
        relation: EntityRelation,
        entity: u32,
    ) -> Option<u32> {
        match relation {
            EntityRelation::Owner if entity == self.entity => self.owner,
            EntityRelation::Shooter if entity == self.entity => {
                Some(self.shooter.map_or(0, |shooter| shooter.body))
            }
            _ => listed_relation(self.shared.players, relation, entity),
        }
    }

    /// A listed body's word, as the target searches take it
    /// ([`TargetCandidate::filter_word`]).
    fn entity_filter_flags(&self, _site: Site, entity: u32) -> Option<u64> {
        self.shared
            .players
            .iter()
            .find(|player| player.entity == entity)
            .map(TargetCandidate::filter_word)
    }

    /// A spawned entity's ApplyAura puts its auras on the bodies through
    /// the match.
    fn change_aura(&mut self, _site: Site, change: AuraChange) -> bool {
        if self.owner.is_none() {
            return false;
        }
        self.shared.auras.push(change);
        true
    }

    /// A spawned projectile's speed scale state: the match flies it at
    /// that scale ([`PlacedEntity::speed_scale`]).
    fn set_projectile_speed(&mut self, site: Site, scale: Option<f64>) -> bool {
        if site.entity != self.entity || self.shooter.is_none() {
            return false;
        }
        *self.speed_scale = scale.filter(|scale| scale.is_finite()).unwrap_or(1.0);
        true
    }

    /// A spawned entity removes itself.
    fn destroy_entity(&mut self, _site: Site, entity: u32) -> bool {
        if self.owner.is_none() || entity != self.entity {
            return false;
        }
        if !self.shared.destroyed.contains(&entity) {
            self.shared.destroyed.push(entity);
        }
        true
    }

    /// A dummy bot gets the player and body the match gave
    /// ([`MapEntityWorld::set_next_dummy`]), one a run; the match makes it.
    /// The graph gets his player entity, his controller: the console's
    /// scope 197 (`265F` rid46) runs on `STU_00707E1E(#29454)`, the body
    /// a controller possesses (TrainingPartner.md, "Day (2026-11-19)").
    fn create_dummy_bot(&mut self, _site: Site, dummy: DummyBot) -> Option<u32> {
        let next = self.shared.next_dummy.take()?;
        self.shared
            .dummies
            .push(DummyChange::Create { dummy, next });
        Some(next.player)
    }

    fn remove_dummy_bot(&mut self, _site: Site, team: i64, slot: i64) -> bool {
        self.shared.dummies.push(DummyChange::Remove { team, slot });
        true
    }

    /// A spawned entity's stats go to the match, which credits each
    /// target's player (Biotic Field `0258` rid14: "Biotic Fields
    /// Deployed" for `STU_0340A42A(owner())`, its owner's player entity).
    fn track_stat(&mut self, _site: Site, stat: TrackedStat) -> bool {
        if self.owner.is_none() {
            return false;
        }
        self.shared.tracked_stats.push(stat);
        true
    }

    fn end_message(&mut self, _source: Site, target: u32, message: u64) -> bool {
        self.shared.ends.push(MessageEnd {
            sender: self.entity,
            target,
            message,
        });
        true
    }

    /// A scope state's instances on its targets go to the match, which
    /// opens them in the targets' components.
    fn change_scope(&mut self, _site: Site, change: ScopeChange) -> bool {
        self.shared.scopes.push(change);
        true
    }

    /// A spawn room's own area (the match's map data).
    fn in_entity_volume(&self, _site: Site, entity: u32, point: [f64; 3]) -> Option<bool> {
        self.shared.volumes?.contains(entity, point)
    }
}

/// Every placed entity's script in one match.
pub struct MapEntityWorld {
    graphs: Arc<GraphStore>,
    mode: u64,
    /// The game's ruleset ([`GraphStore::mode_variables`]); 0: none.
    ruleset: u64,
    /// The match's mode entity value (`World::mode_random`).
    mode_random: f32,
    unknowns: Unknowns,
    players: Vec<TargetCandidate>,
    /// The map's tagged placeables the scene tag queries find.
    map_tags: Option<Arc<MapTags>>,
    /// The map's collision the scripts' ray casts meet.
    map_rays: Option<Arc<dyn MapRays>>,
    volumes: Option<Arc<dyn EntityVolumes>>,
    outbox: Vec<GameMessage>,
    /// The auras the spawned entities put on bodies or took off.
    auras: Vec<AuraChange>,
    /// The spawned entities that removed themselves since the match last
    /// looked.
    destroyed: Vec<u32>,
    ends: Vec<MessageEnd>,
    /// The stats the spawned entities counted ([`World::track_stat`]).
    tracked_stats: Vec<TrackedStat>,
    /// The scope instances the scope states opened or closed on their
    /// targets ([`World::change_scope`]).
    scopes: Vec<ScopeChange>,
    /// The player and body the next dummy bot gets, if the match gave one.
    next_dummy: Option<NextDummy>,
    /// The dummy bots made or removed since the match last looked.
    dummies: Vec<DummyChange>,
    entities: Vec<PlacedEntity>,
    frame: u32,
}

impl MapEntityWorld {
    /// Start every placement that has graphs at `frame`, in order. A
    /// placement whose start faults keeps the fault and no component.
    /// `ruleset`: the game's ruleset, which sets the mode entity's
    /// variables with `mode`; `map_tags`: the map's tagged placeables in
    /// this mode, if known;
    /// `map_rays`: the map's collision, if known; `volumes`: the
    /// placeables' volumes, if known.
    #[allow(clippy::too_many_arguments)]
    pub fn start<'p>(
        graphs: Arc<GraphStore>,
        mode: u64,
        ruleset: u64,
        mode_random: f32,
        unknowns: Unknowns,
        placements: impl IntoIterator<Item = Placement<'p>>,
        map_tags: Option<Arc<MapTags>>,
        map_rays: Option<Arc<dyn MapRays>>,
        volumes: Option<Arc<dyn EntityVolumes>>,
        frame: u32,
    ) -> Self {
        let mut world = Self {
            graphs,
            mode,
            ruleset,
            mode_random,
            unknowns,
            players: Vec::new(),
            map_tags,
            map_rays,
            volumes,
            outbox: Vec::new(),
            auras: Vec::new(),
            destroyed: Vec::new(),
            ends: Vec::new(),
            tracked_stats: Vec::new(),
            scopes: Vec::new(),
            next_dummy: None,
            dummies: Vec::new(),
            entities: Vec::new(),
            frame,
        };
        world.place(placements);
        world
    }

    /// Start more placements at the world's frame, in order, as
    /// [`Self::start`] does.
    pub fn place<'p>(&mut self, placements: impl IntoIterator<Item = Placement<'p>>) {
        for placement in placements {
            let roots = placed_roots(&self.graphs, &placement);
            if roots.as_ref().is_ok_and(Vec::is_empty) {
                continue;
            }
            self.start_entity(placement.entity, placement.position, roots, None, None);
        }
    }

    /// An entity a graph made ([`crate::vm::World::create_entity`]), owned
    /// by `owner`, the entity whose graph made it: it starts its
    /// definition's graphs, if any, at once, and hosts the scope instances
    /// other entities open on it (the Workshop's Create Effect `1CE8`
    /// opens its scope 35 on the `304A` it made, whose definition has a
    /// statescript component with no graphs; [`Self::change_scope`]). It
    /// removes itself as a spawned entity does (`STU_523EE38D`).
    pub fn create(&mut self, entity: u32, definition: u64, position: [f64; 3], owner: u32) {
        let roots = match self.graphs.entity_body(definition) {
            Some(body) => crate::vm::body_roots(body),
            None => Ok(Vec::new()),
        };
        self.start_entity(entity, position, roots, Some(owner), None);
    }

    /// A scope instance another entity's scope state opened or closed on
    /// one of these entities: its script opens or closes it now. The
    /// instance it opened, for the scope state's frame record; none for a
    /// close or an entity not here.
    pub fn change_scope(&mut self, change: &ScopeChange) -> Option<u16> {
        let target = change.target();
        let mut opened = None;
        self.run_scripts(
            |entity| entity == target,
            |component, world| {
                opened = component.change_scope(change, world)?;
                Ok(())
            },
        );
        opened
    }

    /// Spawn an entity at the current frame: it starts its initial graphs
    /// and runs them at once. False when it has none or one is missing.
    pub fn spawn(&mut self, spawn: Spawn<'_>) -> bool {
        if spawn.graphs.is_empty()
            || spawn
                .graphs
                .iter()
                .any(|&graph| self.graphs.graph(graph).is_none())
        {
            return false;
        }
        let roots = (1..)
            .zip(spawn.graphs)
            .map(|(id, &graph)| Root::new(id, graph))
            .collect();
        self.start_entity(
            spawn.entity,
            spawn.position,
            Ok(roots),
            Some(spawn.owner),
            spawn.shooter,
        );
        true
    }

    /// Start one entity's roots at the world's frame.
    fn start_entity(
        &mut self,
        entity: u32,
        position: [f64; 3],
        roots: Result<Vec<Root>, InitFault>,
        owner: Option<u32>,
        shooter: Option<ProjectileShooter>,
    ) {
        let roots = match roots {
            Ok(roots) => roots,
            Err(error) => {
                self.entities.push(PlacedEntity {
                    entity,
                    position,
                    component: None,
                    fault: Some(error.to_string()),
                    team: None,
                    target_tags: TargetTags::default(),
                    owner,
                    shooter,
                    speed_scale: 1.0,
                });
                return;
            }
        };
        let frame = self.frame;
        let mut shared = Shared {
            graphs: &self.graphs,
            mode: self.mode,
            ruleset: self.ruleset,
            mode_random: self.mode_random,
            unknowns: self.unknowns,
            players: &self.players,
            map_tags: self.map_tags.as_deref(),
            map_rays: self.map_rays.as_deref(),
            volumes: self.volumes.as_deref(),
            outbox: &mut self.outbox,
            auras: &mut self.auras,
            destroyed: &mut self.destroyed,
            ends: &mut self.ends,
            tracked_stats: &mut self.tracked_stats,
            scopes: &mut self.scopes,
            next_dummy: &mut self.next_dummy,
            dummies: &mut self.dummies,
        };
        let mut team = None;
        let mut target_tags = TargetTags::default();
        let mut speed_scale = 1.0;
        let mut entity_world = EntityWorld {
            shared: &mut shared,
            entity,
            position,
            team: &mut team,
            target_tags: &mut target_tags,
            owner,
            shooter,
            speed_scale: &mut speed_scale,
        };
        let started = Component::start(
            Arc::clone(&self.graphs),
            entity,
            Variables::new(),
            roots,
            frame,
            &mut entity_world,
            MAX_INIT_WORK,
        )
        .and_then(|mut component| {
            component.advance_frame(frame, &mut entity_world)?;
            Ok(component)
        });
        let (component, fault) = match started {
            Ok(component) => (Some(component), None),
            Err(error) => (None, Some(error.to_string())),
        };
        self.entities.push(PlacedEntity {
            entity,
            position,
            component,
            fault,
            team,
            target_tags,
            owner,
            shooter,
            speed_scale,
        });
        self.end_destroyed();
    }

    /// A spawned entity moves to `position` (a projectile in flight): its
    /// graphs see it there from their next run. False: no such entity.
    pub fn set_position(&mut self, entity: u32, position: [f64; 3]) -> bool {
        let Some(placed) = self
            .entities
            .iter_mut()
            .find(|placed| placed.entity == entity && placed.owner.is_some())
        else {
            return false;
        };
        placed.position = position;
        true
    }

    pub fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    pub fn entities(&self) -> &[PlacedEntity] {
        &self.entities
    }

    pub fn entity(&self, entity: u32) -> Option<&PlacedEntity> {
        self.entities.iter().find(|placed| placed.entity == entity)
    }

    /// The bodies the placeables' target searches find.
    pub fn set_players(&mut self, players: Vec<TargetCandidate>) {
        self.players = players;
    }

    /// Advance every running script to a later server frame; false for an
    /// old one. A script that faults stops; the others run on.
    pub fn advance(&mut self, frame: u32) -> bool {
        if frame <= self.frame {
            return false;
        }
        self.frame = frame;
        self.run_scripts(
            |_| true,
            |component, world| component.advance_frame(frame, world),
        );
        true
    }

    /// A game message to the placed entity `target` from `sender` (the
    /// mode's `0724` to a spawn room): its entries run now. False when no
    /// running script of that entity took it.
    pub fn receive_message(
        &mut self,
        target: u32,
        sender: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        let mut taken = false;
        self.run_scripts(
            |entity| entity == target,
            |component, world| {
                taken = component.receive_message(sender, message, parameters.clone(), world)? != 0;
                Ok(())
            },
        );
        taken
    }

    /// A game message `sender` sent every entity: the entries of every
    /// running script but the sender's run now (the sender's own took it
    /// when it was sent).
    pub fn receive_everywhere(&mut self, sender: u32, message: u64, parameters: &Variables) {
        self.run_scripts(
            |entity| entity != sender,
            |component, world| {
                component.receive_message(sender, message, parameters.clone(), world)?;
                Ok(())
            },
        );
    }

    /// A persistent sender (`sender`) let its message go for the placed
    /// entity `target` ([`Component::end_message`]).
    pub fn end_message(&mut self, target: u32, sender: u32, message: u64) {
        self.run_scripts(
            |entity| entity == target,
            |component, world| component.end_message(sender, message, world),
        );
    }

    /// Run `run` on the running script of each entity `pick` takes, with
    /// its world; a script that faults stops.
    fn run_scripts(
        &mut self,
        mut pick: impl FnMut(u32) -> bool,
        mut run: impl FnMut(&mut Component, &mut EntityWorld<'_, '_>) -> Result<(), InitFault>,
    ) {
        let mut shared = Shared {
            graphs: &self.graphs,
            mode: self.mode,
            ruleset: self.ruleset,
            mode_random: self.mode_random,
            unknowns: self.unknowns,
            players: &self.players,
            map_tags: self.map_tags.as_deref(),
            map_rays: self.map_rays.as_deref(),
            volumes: self.volumes.as_deref(),
            outbox: &mut self.outbox,
            auras: &mut self.auras,
            destroyed: &mut self.destroyed,
            ends: &mut self.ends,
            tracked_stats: &mut self.tracked_stats,
            scopes: &mut self.scopes,
            next_dummy: &mut self.next_dummy,
            dummies: &mut self.dummies,
        };
        for placed in &mut self.entities {
            if !pick(placed.entity) {
                continue;
            }
            let Some(component) = placed.component.as_mut() else {
                continue;
            };
            let mut world = EntityWorld {
                shared: &mut shared,
                entity: placed.entity,
                position: placed.position,
                team: &mut placed.team,
                target_tags: &mut placed.target_tags,
                owner: placed.owner,
                shooter: placed.shooter,
                speed_scale: &mut placed.speed_scale,
            };
            if let Err(error) = run(component, &mut world) {
                placed.fault = Some(error.to_string());
                placed.component = None;
            }
        }
        self.end_destroyed();
    }

    /// Remove a spawned entity the match ends (its owner's body went): its
    /// graphs end, as when it removes itself. False: no such entity.
    pub fn remove(&mut self, entity: u32) -> bool {
        let Some(index) = self
            .entities
            .iter()
            .position(|placed| placed.entity == entity && placed.owner.is_some())
        else {
            return false;
        };
        self.end_entity(index);
        true
    }

    /// The spawned entities that removed themselves go: their graphs end
    /// (their ApplyAura states take their auras off).
    fn end_destroyed(&mut self) {
        let mut index = 0;
        while index < self.entities.len() {
            if self.destroyed.contains(&self.entities[index].entity) {
                self.end_entity(index);
            } else {
                index += 1;
            }
        }
    }

    fn end_entity(&mut self, index: usize) {
        let mut placed = self.entities.remove(index);
        let Some(mut component) = placed.component.take() else {
            return;
        };
        let instances: Vec<u16> = component
            .snapshot()
            .map(|snapshot| snapshot.instances.keys().copied().collect())
            .unwrap_or_default();
        let mut shared = Shared {
            graphs: &self.graphs,
            mode: self.mode,
            ruleset: self.ruleset,
            mode_random: self.mode_random,
            unknowns: self.unknowns,
            players: &self.players,
            map_tags: self.map_tags.as_deref(),
            map_rays: self.map_rays.as_deref(),
            volumes: self.volumes.as_deref(),
            outbox: &mut self.outbox,
            auras: &mut self.auras,
            destroyed: &mut self.destroyed,
            ends: &mut self.ends,
            tracked_stats: &mut self.tracked_stats,
            scopes: &mut self.scopes,
            next_dummy: &mut self.next_dummy,
            dummies: &mut self.dummies,
        };
        let mut world = EntityWorld {
            shared: &mut shared,
            entity: placed.entity,
            position: placed.position,
            team: &mut placed.team,
            target_tags: &mut placed.target_tags,
            owner: placed.owner,
            shooter: placed.shooter,
            speed_scale: &mut placed.speed_scale,
        };
        for id in instances {
            // An instance that faults on its end leaves the rest to end.
            let _ = component.destroy_instance(id, &mut world);
        }
    }

    /// The game messages the scripts sent, for the match to route.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }

    /// The auras the spawned entities put on bodies or took off, for the
    /// match to hand to the bodies' scripts.
    pub fn take_auras(&mut self) -> Vec<AuraChange> {
        std::mem::take(&mut self.auras)
    }

    /// The spawned entities that removed themselves since the last call.
    pub fn take_destroyed(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.destroyed)
    }

    /// The messages the scripts' senders let go since the last call
    /// ([`World::end_message`]), for the match to route.
    pub fn take_message_ends(&mut self) -> Vec<MessageEnd> {
        std::mem::take(&mut self.ends)
    }

    /// The stats the spawned entities counted since the last call, for the
    /// match to credit to their targets' players.
    pub fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        std::mem::take(&mut self.tracked_stats)
    }

    /// The scope instances the scripts' scope states opened or closed on
    /// their targets since the last call, for the match to carry out in
    /// the targets' components (`vm/scopes.rs`).
    pub fn take_scope_changes(&mut self) -> Vec<ScopeChange> {
        std::mem::take(&mut self.scopes)
    }

    /// The match opened the scope instance `instance` on `target` for the
    /// scope state `source`: that state's frame record names it.
    pub fn scope_opened(&mut self, source: ScopeSource, target: u32, instance: u16) {
        if let Some(component) = self
            .entities
            .iter_mut()
            .find(|placed| placed.entity == source.entity)
            .and_then(|placed| placed.component.as_mut())
        {
            component.scope_opened(source, target, instance);
        }
    }

    /// A scope instance on another entity set a variable of its scope
    /// state's instance here (`STU_D49BBD03`: the console's `#940` from a
    /// re-pick in its scope 12): the instance takes it now.
    pub fn set_parent_variable(&mut self, write: &ParentWrite) {
        self.run_scripts(
            |entity| entity == write.source.entity,
            |component, world| {
                component.set_instance_variable(
                    write.source.instance,
                    write.variable,
                    write.value.clone(),
                    world,
                )
            },
        );
    }

    /// The player and first body the next dummy bot the scripts make gets
    /// (`STU_FC3921B1`); None: they can make none now. A bot made takes it.
    pub fn set_next_dummy(&mut self, next: Option<NextDummy>) {
        self.next_dummy = next;
    }

    /// The dummy bots the scripts made or removed since the last call, in
    /// order, for the match to make or remove.
    pub fn take_dummy_changes(&mut self) -> Vec<DummyChange> {
        std::mem::take(&mut self.dummies)
    }

    /// The classes the VM refused or answered by assumption, and the
    /// faults, with how often, by graph (0 for a fault outside a graph).
    pub fn gaps(&self) -> Gaps {
        let mut gaps = Gaps::new();
        for placed in &self.entities {
            if let Some(fault) = &placed.fault {
                *gaps.entry((0, fault.clone())).or_default() += 1;
            }
            let Some(snapshot) = placed.snapshot() else {
                continue;
            };
            for assumption in &snapshot.assumptions {
                *gaps
                    .entry((assumption.site.graph, assumption.class.to_string()))
                    .or_default() += 1;
            }
            for diagnostic in &snapshot.diagnostics {
                *gaps
                    .entry((diagnostic.site.graph, format!("{:?}", diagnostic.issue)))
                    .or_default() += 1;
            }
        }
        gaps
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ENTITY: u32 = 0x8000_01CE;

    /// A spawn marker as the Practice Range's `01CE` starts: it stores the
    /// mode entity (`STU_A3DEF368`) in #1, then gives itself the team #1016
    /// holds (`STU_0F647E55`), here 1.
    #[test]
    fn a_placed_entity_keeps_the_team_its_script_gives_it_and_finds_the_mode_entity() {
        let owner = json!({"$": "STU_B5A0CAF0", "m_EE729DCB": 0});
        let plug = |to: u16| json!({"$": "Plug", "links": [[to, "m_inPlug"]], "m_FF3DAF1E": []});
        let node = |pos: u16, class: &str, fields: Json| {
            json!({
                "pos": pos, "class": class, "uid": pos, "rid": pos, "remote": -1,
                "state": null, "client_only": false, "server_only": false, "fields": fields,
            })
        };
        let var = |id: u32, entity: bool| json!({"$": "STUConfigVarDynamic", "m_identifier": id, "m_60DB8F99": entity});
        let nodes = vec![
            node(0, "STUStatescriptEntry", json!({"m_outPlug": plug(1)})),
            node(
                1,
                "STUStatescriptActionSetVar",
                json!({
                    "m_out_Var": var(1, true),
                    "m_value": {"$": "STU_A3DEF368", "m_EE729DCB": 0},
                    "m_outPlug": plug(2),
                }),
            ),
            node(
                2,
                "STU_0F647E55",
                json!({"m_targets": owner, "m_67465E75": var(1016, false)}),
            ),
        ];
        let graph = json!({
            "index": 0x01ce, "items": nodes.len(), "nodes": nodes, "states": [],
            "entries": [0], "remote": [], "sync_vars": [], "containers": {},
            "unknown_nodes": [], "fields": {},
            "header": {
                "m_nodesBitCount": 4, "m_statesBitCount": 4,
                "m_remoteSyncNodesBitCount": 4, "m_syncVarsBitCount": 4
            }
        });
        let graphs = GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version": 1, "graphs": [graph], "bodies": {}, "missing": []
            }))
            .unwrap(),
        )
        .unwrap();
        let extra = [GraphBinding {
            graph: Some(0x01ce),
            sync: Some(vec![json!({
                "$": "STUStatescriptSchemaEntry",
                "m_0D09D2D9": "0x0D800000000003F8",
                "m_value": {"$": "STUConfigVarInt", "m_value": 1, "m_EE729DCB": 0},
            })]),
            flags: 0,
        }];
        let world = MapEntityWorld::start(
            Arc::new(graphs),
            0,
            0,
            0.0,
            Unknowns::Refuse,
            [Placement {
                entity: ENTITY,
                definition: 0,
                position: [0.0; 3],
                overrides: &[],
                extra: &extra,
            }],
            None,
            None,
            None,
            1,
        );
        let placed = world.entity(ENTITY).unwrap();
        let snapshot = placed.snapshot().unwrap();
        assert!(placed.complete(), "{:?}", snapshot.diagnostics);
        assert_eq!(placed.team(), Some(1));
        assert_eq!(
            snapshot.variables.get(&1),
            Some(&Value::Entity(GAME_MODE_ENTITY))
        );
    }

    /// A spawn marker's `01CE` st1 (`STU_E56BA8D8`) gives the marker tag
    /// `0035` while it runs: the placed world takes the effect on the
    /// entity itself (it refused it as a gap before), so the script stays
    /// whole with the state running.
    #[test]
    fn a_placed_entity_holds_the_target_tag_its_own_state_gives_it() {
        const TAG: &str = "0x0D60000000000035";
        let owner = json!({"$": "STU_B5A0CAF0", "m_EE729DCB": 0});
        let plug =
            |to: u16, name: &str| json!({"$": "Plug", "links": [[to, name]], "m_FF3DAF1E": []});
        let node = |pos: u16, class: &str, state: Option<u16>, fields: Json| {
            json!({
                "pos": pos, "class": class, "uid": pos, "rid": pos, "remote": -1,
                "state": state, "client_only": false, "server_only": false, "fields": fields,
            })
        };
        let nodes = vec![
            node(
                0,
                "STUStatescriptEntry",
                None,
                json!({"m_outPlug": plug(1, "m_beginPlug")}),
            ),
            node(
                1,
                "STU_E56BA8D8",
                Some(0),
                json!({
                    "m_31714AFC": owner,
                    "m_targetTags": {"$": "STU_357DABBB", "m_7D7C86A1": TAG},
                    "m_2BBEEAB8": true,
                }),
            ),
        ];
        let graph = json!({
            "index": 0x01ce, "items": nodes.len(), "nodes": nodes, "states": [1],
            "entries": [0], "remote": [], "sync_vars": [], "containers": {},
            "unknown_nodes": [], "fields": {},
            "header": {
                "m_nodesBitCount": 4, "m_statesBitCount": 4,
                "m_remoteSyncNodesBitCount": 4, "m_syncVarsBitCount": 4
            }
        });
        let graphs = GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version": 1, "graphs": [graph], "bodies": {}, "missing": []
            }))
            .unwrap(),
        )
        .unwrap();
        let extra = [GraphBinding {
            graph: Some(0x01ce),
            sync: None,
            flags: 0,
        }];
        let world = MapEntityWorld::start(
            Arc::new(graphs),
            0,
            0,
            0.0,
            Unknowns::Refuse,
            [Placement {
                entity: ENTITY,
                definition: 0,
                position: [0.0; 3],
                overrides: &[],
                extra: &extra,
            }],
            None,
            None,
            None,
            1,
        );
        let placed = world.entity(ENTITY).unwrap();
        let snapshot = placed.snapshot().unwrap();
        assert!(placed.complete(), "{:?}", snapshot.diagnostics);
        assert!(
            snapshot.instances[&1]
                .states
                .values()
                .any(|state| state.active)
        );
    }
}
