//! Initialization and supported frame events of authored statescript graphs.
//!
//! This covers Entry, typed variable writes, conditional links and static
//! SubScript instances. Diagnostics make a partial snapshot unsuitable for
//! gameplay replication. The component also advances static waits and dispatches
//! scoped message entries. Input, combat and other authored actions still need
//! their corresponding runtime.

mod abilities;
mod actions;
mod arrays;
mod auras;
mod buttons;
mod context;
mod entities;
mod filter_array;
mod flow;
mod health;
mod hero_select;
mod lifecycle;
mod links;
mod machine;
mod nodes;
mod play;
mod rays;
mod state_kinds;
mod statuses;
mod switches;
mod targets;
mod timers;
mod volleys;

use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashSet},
    error::Error,
    fmt,
    sync::Arc,
};

pub use auras::{AURA_APPLIED, AuraChange, AuraSource};
pub use health::HealthChange;
use indexmap::IndexMap;
use serde_json::Value as Json;
pub use statuses::{BodyEffects, CombatMod, DAMAGE_TAKEN, HEALING_TAKEN, INVULNERABLE, PHASED_OUT};

use crate::{
    expr::{
        self, Context, EmptyContext, EntityRelation, EvalError, Scope, StackPriority, Variable,
        equal, truthy,
    },
    graph::{Body, Graph, GraphStore, Node, asset_guid},
    timeline::{ButtonEdge, FRAME_MS, QueueFull, Timeline, button_edges, frame_start},
    vitals::PoolKind,
    wheel,
    wire::{Value, Variables},
};
use orpheus_physics::mover::modifiers::{
    Modifier, ModifierKey, Operation, VectorOperation, VelocityElement,
};
use orpheus_wire::{commands::Command, entity::MAX_MOVEMENT_AXIS};

pub const MAX_INIT_WORK: usize = 100_000;
const MAX_PLUG_DEPTH: usize = 64;
const MAX_EVENTS_PER_UPDATE: usize = 1000;
/// Drain, tick and deferred rounds of one instance in one update
/// (`MAX_INSTANCE_UPDATES` in `ow174/game/script/runtime.py`).
const MAX_INSTANCE_UPDATES: usize = 1000;
const MAX_PENDING_EVENTS: usize = 16_384;
const MAX_PENDING_SHOTS: usize = 256;
const ABILITY_RUNNING: u8 = 1;
const ABILITY_PUSHED: u8 = 2;
const ABILITY_HELD: u8 = 4;
const SWITCH_TO_TRUE: u16 = 1;
const SWITCH_TO_FALSE: u16 = 2;
const LINK_PRIORITY: StackPriority = StackPriority {
    level: 0,
    above_equals: true,
};
const ABORT_PLUG: &str = "m_F198FD3A";
const SUBGRAPH_STATES: &str = "m_FF3DAF1E";
const MOVEMENT_BOOLEAN_OUTPUTS: [&str; 12] = [
    "m_105DF1D6",
    "m_49F4B33C",
    "m_56AC7B36",
    "m_16BC4759",
    "m_F620FC2A",
    "m_6306AA29",
    "m_103A8581",
    "m_958FF5C0",
    "m_4C55BB03",
    "m_2EB48682",
    "m_45EB431F",
    "m_BF152E27",
];

#[derive(Clone, Debug)]
pub struct Root {
    pub id: u16,
    pub graph: u32,
    pub variables: Variables,
}

impl Root {
    pub fn new(id: u16, graph: u32) -> Self {
        Self {
            id,
            graph,
            variables: Variables::new(),
        }
    }
}

/// Initial body bindings use their asset order. Entries run after all roots exist.
pub fn body_roots(body: &Body) -> Result<Vec<Root>, InitFault> {
    let mut roots = Vec::new();
    for (offset, binding) in body.graphs.iter().enumerate() {
        let Some(graph) = binding.graph else {
            continue;
        };
        let id = u16::try_from(offset + 1).map_err(|_| InitFault::InstanceIdExhausted)?;
        let mut root = Root::new(id, graph);
        bind_values(&mut root, binding.sync.iter().flatten())?;
        roots.push(root);
    }
    Ok(roots)
}

/// Write a binding's schema entries (`STUStatescriptSchemaEntry`: the
/// variable `m_0D09D2D9`, the config `m_value`) into the root, a later
/// entry of a variable replacing an earlier one.
pub fn bind_values<'a>(
    root: &mut Root,
    entries: impl IntoIterator<Item = &'a Json>,
) -> Result<(), InitFault> {
    for field in entries {
        let key = field
            .get("m_0D09D2D9")
            .and_then(asset_guid)
            .map_or(0, |guid| guid as u16);
        let value = expr::evaluate(
            field.get("m_value").unwrap_or(&Json::Null),
            &mut EmptyContext,
        )
        .map_err(|error| InitFault::RootBinding {
            id: root.id,
            graph: root.graph,
            error,
        })?;
        root.variables.insert(u32::from(key), value);
    }
    Ok(())
}

/// Initial body bindings, then the manager and equipped weapon slots in asset order.
pub fn player_roots(body: &Body, graphs: &GraphStore) -> Result<Vec<Root>, InitFault> {
    let mut roots = body_roots(body)?;
    let mut id = roots.last().map_or(0, |root| root.id);
    let mut append = |graph, slot: Option<usize>| {
        id = id.checked_add(1).ok_or(InitFault::InstanceIdExhausted)?;
        let mut root = Root::new(id, graph);
        if let Some(slot) = slot {
            root.variables.insert(28, Value::Integer(slot as i64));
        }
        roots.push(root);
        Ok::<_, InitFault>(())
    };
    if let Some(manager) = body.manager {
        append(manager, None)?;
    }
    for (slot, graph) in body.weapons.iter().enumerate() {
        if let Some(graph) = graph {
            append(*graph, Some(slot))?;
        }
    }
    if let Some(root) = roots.iter().find(|root| graphs.graph(root.graph).is_none()) {
        return Err(InitFault::MissingPlayerGraph(root.graph));
    }
    Ok(roots)
}

const OWNER_HEALTH: u32 = 253;
const HUD_HEALTH: u32 = 1732;
const HUD_MAX_HEALTH: u32 = 1737;

pub fn player_entity_variables(body: &Body, health: f64, maximum_health: f64) -> Variables {
    let slots = body
        .weapons
        .iter()
        .map(|weapon| Value::Bool(weapon.is_some()))
        .collect();
    let first = body.weapons.iter().position(Option::is_some).unwrap_or(0);
    Variables::from([
        (29, Value::Array(slots)),
        (16_515, Value::Integer(first as i64)),
        (OWNER_HEALTH, Value::Float(health)),
        (HUD_HEALTH, Value::Float(health)),
        (HUD_MAX_HEALTH, Value::Float(maximum_health)),
        (342, Value::Float(0.0)),
    ])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Site {
    pub entity: u32,
    pub instance: u16,
    pub graph: u32,
    pub node: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetTagEffect {
    pub entity: u32,
    pub tag: u64,
    pub present: bool,
}

/// A body a target search (`TrackTargets`) can find, as the match places it
/// this tick: players' bodies and training bots alike.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetCandidate {
    pub entity: u32,
    /// Its team; none in a free-for-all, where a body is only its own ally.
    pub team: Option<u8>,
    pub dead: bool,
    /// The entity that controls it: a player's controller, a training
    /// bot's stand-in controller; none when the match has none.
    pub controller: Option<u32>,
    /// Whether that controller is an AI (has component 92, which
    /// `STU_D3EB8D87` tests): a training bot's, whose body its AI
    /// definition possesses. Players' controllers are not, AI heroes'
    /// included: the server gives them the player's damage times a share
    /// (`orpheus_sim::ai::scale_damage`).
    pub ai: bool,
    /// Its capsule: from the feet up `height`, `radius` round (the match's
    /// `TargetPose`).
    pub feet: [f64; 3],
    pub height: f64,
    pub radius: f64,
    /// Where it faces, in radians: forward is `[sin, 0, cos]` (the match's
    /// `TargetPose::yaw`).
    pub yaw: f64,
    /// Its body definition's target tags (`STUTargetTagComponent`), each
    /// once; None: unknown. Assumption: a listed body carries them and no
    /// tag its own graphs add or suppress (only the player's own body
    /// knows those).
    pub tags: Option<Arc<BTreeSet<u64>>>,
}

/// One turn in the yaw and pitch units of a movement record.
const TURN_UNITS: f64 = 65_536.0;

impl TargetCandidate {
    /// Its yaw in a movement record's units (65536 a turn).
    pub fn yaw_units(&self) -> i32 {
        (self.yaw * TURN_UNITS / std::f64::consts::TAU).round() as i64 as i32
    }

    /// Its filter word as the target searches test it (`vm/targets.rs`
    /// module notes: team bit, body kinds, dead).
    pub fn filter_word(&self) -> u64 {
        targets::filter_word(self)
    }

    /// How far `point` is from the capsule's surface (0 inside).
    pub fn distance(&self, point: [f64; 3]) -> f64 {
        let [x, y, z] = self.feet;
        let low = y + self.radius;
        let high = y + self.radius.max(self.height - self.radius);
        let nearest = [x, point[1].clamp(low, high), z];
        let offset: f64 = (0..3)
            .map(|axis| (point[axis] - nearest[axis]).powi(2))
            .sum();
        (offset.sqrt() - self.radius).max(0.0)
    }

    /// Where a ray from `origin` along unit `direction` first enters the
    /// capsule grown by `inflate` (a cast's radius), within `reach`: its
    /// distance and the surface normal there. None when it misses, or
    /// starts inside.
    pub fn ray_hit(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
        inflate: f64,
    ) -> Option<(f64, [f64; 3])> {
        let radius = self.radius + inflate;
        if self.distance(origin) <= inflate {
            return None;
        }
        let [x, y, z] = self.feet;
        let low = y + self.radius;
        let high = y + self.radius.max(self.height - self.radius);
        let mut best: Option<(f64, [f64; 3])> = None;
        let mut keep = |t: f64, normal: [f64; 3]| {
            if (0.0..=reach).contains(&t) && best.is_none_or(|(best, _)| t < best) {
                best = Some((t, normal));
            }
        };
        // The side: the vertical cylinder between the caps' centres.
        let (ox, oz) = (origin[0] - x, origin[2] - z);
        let (dx, dz) = (direction[0], direction[2]);
        let a = dx * dx + dz * dz;
        if a > 1e-12 {
            let b = 2.0 * (ox * dx + oz * dz);
            let c = ox * ox + oz * oz - radius * radius;
            let disc = b * b - 4.0 * a * c;
            if disc >= 0.0 {
                let t = (-b - disc.sqrt()) / (2.0 * a);
                let height = origin[1] + t * direction[1];
                if (low..=high).contains(&height) {
                    let (px, pz) = (ox + t * dx, oz + t * dz);
                    keep(t, [px / radius, 0.0, pz / radius]);
                }
            }
        }
        // The caps: spheres at both ends.
        for centre in [[x, low, z], [x, high, z]] {
            let m: [f64; 3] = std::array::from_fn(|axis| origin[axis] - centre[axis]);
            let b: f64 = (0..3).map(|axis| m[axis] * direction[axis]).sum();
            let c: f64 = (0..3).map(|axis| m[axis] * m[axis]).sum::<f64>() - radius * radius;
            let disc = b * b - c;
            if disc >= 0.0 {
                let t = -b - disc.sqrt();
                keep(
                    t,
                    std::array::from_fn(|axis| (m[axis] + t * direction[axis]) / radius),
                );
            }
        }
        best
    }
}

/// Where a ray meets the map's collision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapHit {
    pub distance: f64,
    pub point: [f64; 3],
    pub normal: [f64; 3],
}

/// The map's collision as a script's ray casts meet it (`vm/rays.rs`).
pub trait MapRays: Send + Sync {
    /// The first map shape a ray from `origin` along unit `direction` meets
    /// within `reach`: the shapes that stop a ray of ray profile `profile`
    /// (`097`; none: the default profile's map mask). None: the map's
    /// collision can't answer (a bad shape).
    fn cast(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
        profile: Option<u64>,
    ) -> Option<Option<MapHit>>;
}

/// External authored queries for this initialization. The caller owns mode,
/// ruleset, and entity data and can decline a query without guessing its value.
/// What the VM and the frame encoder do with graph content they don't
/// implement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unknowns {
    /// Record a diagnostic and refuse the body, so a gap can't hide. Debug
    /// builds and tests.
    Refuse,
    /// Do what the Python reference did, and record each case in
    /// [`Snapshot::assumptions`]: an unknown state is a plain state, an
    /// unknown action follows its out plug, an unknown config value or
    /// opcode is null, any other unsupported part does nothing, and an
    /// active state whose payload can't be written is left out of frames.
    /// Release builds.
    Tolerate,
}

impl Unknowns {
    pub const fn for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::Refuse
        } else {
            Self::Tolerate
        }
    }
}

pub trait World {
    /// How this world's bodies treat graph content the VM doesn't implement.
    fn unknowns(&self) -> Unknowns {
        Unknowns::for_build()
    }

    /// Whether this world's answer for a native config class is an assumption
    /// (see [`Assumption`]), and what it answers.
    fn assumption(&self, _class: &str) -> Option<&'static str> {
        None
    }

    fn ruleset(&mut self, _id: u32) -> Option<Value> {
        None
    }

    /// The match's mode entity value `STU_B182C5C9` reads (component 114
    /// field 9, [`expr::Context::mode_random`]); None: unknown here.
    fn mode_random(&self) -> Option<f64> {
        None
    }

    /// A team's size in the game struct ([`expr::Context::team_size`]);
    /// None: unknown here.
    fn team_size(&self, _team: i64) -> Option<i64> {
        None
    }

    fn entity_variable(&mut self, _entity: u32, _id: u32) -> Value {
        Value::Null
    }

    fn position(&mut self, _entity: u32) -> [f64; 3] {
        [0.0; 3]
    }

    /// The implicit owner's movement sample, if the world knows all its bits.
    fn movement_sample(&self, _site: Site) -> Option<u16> {
        None
    }

    fn native(&mut self, _site: Site, _config: &Json) -> Option<Value> {
        None
    }

    fn body_scale(&mut self, _site: Site, _target: u32, _override_value: &Value) -> Option<f64> {
        None
    }

    /// A missing kind reads the total across all pools.
    fn body_health(&self, _site: Site, _kind: Option<PoolKind>, _maximum: bool) -> Option<f64> {
        None
    }

    fn add_health_pool(
        &mut self,
        _site: Site,
        _kind: PoolKind,
        _maximum: f64,
        _current: f64,
    ) -> Option<u32> {
        None
    }

    fn health_pool_current(&self, _site: Site, _pool: u32) -> Option<f64> {
        None
    }

    fn remove_health_pool(&mut self, _site: Site, _pool: u32) -> Option<f64> {
        None
    }

    /// Whether `controller` may play `hero` (`STU_9AE5FC4C`) by the heroes
    /// it may play at all; None: this world can't tell. The controller's own
    /// HeroSelect limits apply on top ([`Context::hero_available`]).
    fn hero_available(&self, _site: Site, _controller: u32, _hero: u64) -> Option<bool> {
        None
    }

    /// The role identifier (`084E.01C` damage, `0850.01C` tank, `0851.01C`
    /// support; the hero's `+0x58`) of `hero`; None: unknown.
    fn hero_role(&self, _site: Site, _hero: u64) -> Option<u64> {
        None
    }

    fn has_target_tag(&self, _site: Site, _entity: u32, _tag: u64) -> Option<bool> {
        None
    }

    /// The entities that carry target tag `tag`, each once, in this world's
    /// order (`STU_EB216A0B`); None: this world can't list them.
    fn entities_with_tag(&self, _site: Site, _tag: u64) -> Option<Vec<u32>> {
        None
    }

    fn set_target_tag_effect(
        &mut self,
        _site: Site,
        _effect: TargetTagEffect,
        _active: bool,
    ) -> bool {
        false
    }

    fn entity_filter_flags(&self, _site: Site, _entity: u32) -> Option<u64> {
        None
    }

    /// The bodies a target search on `site` can find, each once in a fixed
    /// order, the searching body among them. None: this world can't list
    /// them, and a search finds nothing.
    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        None
    }

    /// The first map shape a ray cast on `site` meets (see
    /// [`MapRays::cast`]): Some(None) when it meets none. None: this world
    /// has no map, and the cast is a gap.
    fn map_ray(
        &self,
        _site: Site,
        _origin: [f64; 3],
        _direction: [f64; 3],
        _reach: f64,
        _profile: Option<u64>,
    ) -> Option<Option<MapHit>> {
        None
    }

    /// Whether `entity` exists; None: unknown. By default the bodies the
    /// world lists exist.
    fn entity_exists(&self, site: Site, entity: u32) -> Option<bool> {
        self.target_candidates(site)?
            .iter()
            .any(|body| body.entity == entity)
            .then_some(true)
    }

    /// Where `entity` stands; by default a listed body's feet.
    fn entity_position(&self, site: Site, entity: u32) -> Option<[f64; 3]> {
        self.target_candidates(site)?
            .iter()
            .find(|body| body.entity == entity)
            .map(|body| body.feet)
    }

    /// A body's yaw and pitch, as [`expr::Context::entity_facing`].
    fn entity_facing(&self, _site: Site, _entity: u32) -> Option<[i32; 2]> {
        None
    }

    /// A body's yaw alone (its facing along the ground): the facing's, else
    /// a listed body's.
    fn entity_yaw(&self, site: Site, entity: u32) -> Option<i32> {
        if let Some([yaw, _]) = self.entity_facing(site, entity) {
            return Some(yaw);
        }
        self.target_candidates(site)?
            .iter()
            .find(|body| body.entity == entity)
            .map(TargetCandidate::yaw_units)
    }

    /// The move-speed scale `entity`'s mover keeps (component 31 `+0x4D8`,
    /// the result of mover mod 0 its last move used; `STU_ED807E07`): 1.0
    /// for an entity without a mover. None: unknown here.
    fn speed_scale(&self, _site: Site, _entity: u32) -> Option<f64> {
        None
    }

    /// Whether the match's map lists game mode `mode` among its supported
    /// modes (`STU_02514B6A.m_supportedGamemodes`, `STU_16753B11`). None:
    /// unknown here.
    fn map_supports_mode(&self, _site: Site, _mode: u64) -> Option<bool> {
        None
    }

    /// `entity`'s team index (0 to 4); by default a listed body's team. A
    /// free-for-all body lists no team, so it is unknown.
    fn entity_team(&self, site: Site, entity: u32) -> Option<i64> {
        self.target_candidates(site)?
            .iter()
            .find(|body| body.entity == entity)?
            .team
            .map(i64::from)
    }

    /// Whether `position` is inside `entity`'s volume (`STU_73E1C5F0`: the
    /// area a spawn room `0D62` names); None: unknown here.
    fn in_entity_volume(&self, _site: Site, _entity: u32, _position: [f64; 3]) -> Option<bool> {
        None
    }

    /// Give `entity` team index `team` (-1 none, 0 to 4; `STU_0F647E55`).
    /// False: this world keeps no entity teams.
    fn set_entity_team(&mut self, _site: Site, _entity: u32, _team: i64) -> bool {
        false
    }

    /// Remove `entity` from the match (`STU_523EE38D`). False: this world
    /// can't.
    fn destroy_entity(&mut self, _site: Site, _entity: u32) -> bool {
        false
    }

    fn body_on_fire(&mut self, _site: Site, _entity: u32) -> Option<bool> {
        None
    }

    /// None means this relationship is unsupported for the supplied entity.
    /// By default a listed body's controller (none when the match gives
    /// none).
    fn related_entity(&mut self, site: Site, relation: EntityRelation, entity: u32) -> Option<u32> {
        match relation {
            EntityRelation::Controller => {
                self.target_candidates(site)?
                    .iter()
                    .find(|body| body.entity == entity)?
                    .controller
            }
            _ => None,
        }
    }

    /// Whether `entity` is an AI controller (component 92, `STU_D3EB8D87`);
    /// None: unknown. By default a listed body's controller is
    /// ([`TargetCandidate::ai`]).
    fn controller_is_ai(&self, site: Site, entity: u32) -> Option<bool> {
        self.target_candidates(site)?
            .iter()
            .find(|body| body.controller == Some(entity))
            .map(|body| body.ai)
    }

    /// None leaves an encountered AI candidate explicitly unsupported.
    fn selects_ai_behavior(&mut self, _site: Site) -> Option<bool> {
        None
    }

    /// The logical button an AI weapon slot holds down while an
    /// `AIFireWeapon` state runs (`ai_weapon_button` in
    /// `ow174/game/script/runtime.py`). None: the slot has no button.
    fn ai_weapon_button(&mut self, _slot: i64) -> Option<u8> {
        None
    }

    /// The length in seconds of the animation `alias` plays on this
    /// world's body with animation category `category` set (0: none), from
    /// the body's compiled animation data; none when the data can't tell
    /// (`World.animation_duration` in leafall903/ORPHEUS `c239a84`,
    /// `src/ow174/game/script/runtime.py`).
    fn animation_length(&self, _site: Site, _alias: u64, _category: u64) -> Option<f64> {
        None
    }

    /// The scalar an `Anim` state playing `alias` writes to its output
    /// (`World.animation_scalar`, the same source); none when the data
    /// can't tell.
    fn animation_scalar(&self, _site: Site, _alias: u64, _category: u64) -> Option<f64> {
        None
    }

    /// The asset a body has equipped in a communication wheel slot (zero for
    /// an empty slot); None: unknown body.
    fn equipped_cosmetic(
        &self,
        _site: Site,
        _entity: u32,
        _kind: crate::wheel::Cosmetic,
        _slot: i64,
    ) -> Option<u64> {
        None
    }

    /// Deliver a game message to another entity (`World.send_message` in
    /// `ow174/game/script/runtime.py`). The world owns delivery outside the
    /// current body; the receiving body takes it with
    /// [`Component::receive_message`]. False: this world can't deliver it.
    fn send_message(
        &mut self,
        _source: Site,
        _target: u32,
        _message: u64,
        _parameters: &Variables,
    ) -> bool {
        false
    }

    /// Deliver a game message to every entity of the world but the sender
    /// (a `STU_38EE1100` target `STU_1D719351` with `m_6A8DB059` set; the
    /// sender's own instances get it from the VM). False: this world can't
    /// deliver it.
    fn broadcast_message(&mut self, _source: Site, _message: u64, _parameters: &Variables) -> bool {
        false
    }

    /// A persistent sender (a SendGameMessage state) no longer names
    /// `target`, or ended: the target's receiving states that hold its
    /// message go back to idle ([`Component::end_message`]). False: this
    /// world delivers no ends, and the receiver stays as it is.
    fn end_message(&mut self, _source: Site, _target: u32, _message: u64) -> bool {
        false
    }

    /// A `STUStatescriptActionCriteria` raised a criteria on an entity
    /// ([`RaisedCriteria::target`]: its `m_targets`, the body itself or
    /// another entity it names, such as its controller): the event the
    /// target's voice set answers with a voice stimulus whose criteria names
    /// it (the Training Bot's server-only `0C17` nodes raise `0608` / `0609`
    /// after a hit, which its voice set `011D.05F` answers with its pain
    /// lines; a player's `0686` raises the wheel choice's line, Hello
    /// `0AEB`; the hero's `01CA` raises its respawn line `05D8` on its
    /// controller). The server plays the line (ClientInVoice 27500). By
    /// default nothing listens.
    fn criteria(&mut self, _site: Site, _raised: RaisedCriteria) {}

    /// A server-only `VoiceStimulus` state (`STU_40E05F6B`) began: the
    /// server's voice system says the stimulus it names for its target
    /// (`01CA` st46, the hero's elimination line `0008.078`; `0215` st28,
    /// his line after a third kill `00C9.078`). The client never runs such
    /// a state; it hears the line as ClientInVoice 27500. False: this world
    /// plays none.
    fn voice_stimulus(&mut self, _site: Site, _request: VoiceRequest) -> bool {
        false
    }

    /// A ModifyHealth state damaged or healed a body (`vm/health.rs`): the
    /// match applies it through its own health code. False: this world
    /// applies none, and the state has no effect.
    fn modify_health(&mut self, _change: HealthChange) -> bool {
        false
    }

    /// A status state (`STU_9028E035`, `vm/statuses.rs`) began with its
    /// `m_type` (Some) or ended (None). False: this world keeps none.
    fn set_status(&mut self, _site: Site, _status: Option<i64>) -> bool {
        false
    }

    /// A `CombatModFilter` state began with its modifier (Some) or ended
    /// (None). False: this world keeps none.
    fn combat_mod(&mut self, _site: Site, _modifier: Option<CombatMod>) -> bool {
        false
    }

    /// An ApplyAura state (`STU_431F3FCC`, `vm/auras.rs`) put an aura on a
    /// body or took it off: the match hands it to the target's component
    /// ([`Component::change_aura`]). False: this world applies none, and
    /// the state has no effect.
    fn change_aura(&mut self, _site: Site, _change: AuraChange) -> bool {
        false
    }
}

/// A criteria a graph raised ([`World::criteria`]).
#[derive(Clone, Debug, PartialEq)]
pub struct RaisedCriteria {
    pub identifier: u64,
    /// The entity the action names (`m_targets`): the owner itself for most
    /// (an owner target is never evaluated), its controller for some of the
    /// hero's `01CA` lines (`STU_F7262B36(owner())`,
    /// `GetPrimaryPossessorEntity(owner())`).
    pub target: u32,
    /// Who hears the line the voice set answers with: the entities the
    /// action's `m_297E0B63` lists when it lists some, in order; none when
    /// it names no list or all players (`STU_BD2D717B`, the wheel's `0686`).
    /// The pain criteria list the hurt body and its attacker (`0C17` rid 21
    /// to 23, `01CA` rid 89 to 91 and `14B1` rid 30 to 32 fill #1839 with
    /// them after the `STU_ED45C5E8` query), the kill criteria `071B` the
    /// victim and its killer. Assumption: the list is the line's audience;
    /// no other field of the action or the stimulus limits who hears it.
    pub audience: Option<Vec<u32>>,
}

/// What a server-only `VoiceStimulus` state asks the voice system to say
/// ([`World::voice_stimulus`]).
#[derive(Clone, Debug, PartialEq)]
pub struct VoiceRequest {
    /// `m_stimulus`: the voice stimulus (`078`).
    pub stimulus: u64,
    /// `m_targets`: whose voice set says it (the owner itself unevaluated).
    pub targets: Vec<u32>,
    /// `m_B0263D2D`: how long after the state begins the line starts, in
    /// seconds (the client's own run of the state waits this long:
    /// `0x7FF789C53F70` keeps it at `+472`, the update at `0x7FF789C54090`
    /// returns until it passed).
    pub delay: f64,
    /// `m_297E0B63`, read as for a criteria ([`RaisedCriteria::audience`]).
    pub audience: Option<Vec<u32>>,
}

/// A game message one body sends another, for the world to route
/// (`Match.queue_body_message` in `ow174/game/match.py`).
#[derive(Clone, Debug, PartialEq)]
pub struct GameMessage {
    pub sender: u32,
    pub target: u32,
    pub message: u64,
    pub parameters: Variables,
}

/// A persistent sender let its message go for a target
/// ([`World::end_message`]), for the world to route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageEnd {
    pub sender: u32,
    pub target: u32,
    pub message: u64,
}

/// A `MoveTo` state's fixed world target and speed in metres per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveRequest {
    pub instance: u16,
    pub raw_state: u16,
    pub target: [f64; 3],
    pub speed: f64,
}

/// A movement of the body's own that the graphs start and the mover carries out
/// ([`Component::take_motions`]). No Python reference modelled these; the
/// meaning of the authored values is an assumption, recorded at each node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    /// Tracer's Blink (`STU_A807207A`): `distance` metres along the movement
    /// input over `seconds`.
    Dash { distance: f64, seconds: f64 },
    /// Recall's rewind (`STU_41B3FC2B`) began: back along the last `seconds`
    /// of the path over `duration` seconds.
    Rewind { seconds: f64, duration: f64 },
    /// The rewind state ended: finished (the path is done) or aborted.
    EndRewind { finished: bool },
    /// An element of a velocity action (`STU_FF3471E9`) for the next pose.
    Velocity(VelocityElement),
}

#[derive(Default)]
pub struct EmptyWorld;

impl World for EmptyWorld {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateKind {
    Plain,
    HealthPool {
        pool: Option<u32>,
    },
    TargetTag {
        effect: Option<TargetTagEffect>,
    },
    BooleanSwitch(bool),
    DelayedSwitch {
        current: bool,
        undecided: bool,
    },
    Switch {
        case: Option<usize>,
    },
    ActiveQuery,
    GamePadVibration,
    SubScript {
        child: Option<u16>,
    },
    MessageIdle,
    Wait,
    WaitFrames {
        target: u32,
    },
    AiCandidate {
        selected: bool,
    },
    LogicalButton {
        down: bool,
    },
    Stack {
        top: bool,
        under: bool,
    },
    Ability {
        flags: u8,
    },
    MovementMod,
    MovementSample(u16),
    Anim,
    Effect,
    SendMessage,
    CosmeticEntity,
    ChaseVar,
    Watch,
    /// FilterArray (`STU_0A6BE10E`, `vm/filter_array.rs`): it keeps its
    /// output the filtered copy of its source while it runs.
    FilterArray,
    /// HeroSelect (`STU_0E913A0B`, `vm/hero_select.rs`): it keeps the
    /// controller's hero limits while it runs.
    HeroSelect,
    /// A target search (`vm/targets.rs`): the bodies it finds while active
    /// are [`State::tracked`]. Forms the server can't run find nothing
    /// (an assumption, as in the Python reference).
    TrackTargets,
    /// A ray cast kept up to date (RayCast, TrackRay; `vm/rays.rs`): it
    /// casts at its begin and each tick and writes its outputs. Forms the
    /// server can't run write nothing.
    Ray,
    /// A health change (`vm/health.rs`): a rate keeps when it last applied
    /// its share.
    ModifyHealth {
        last_ms: u64,
    },
    /// A status on the body while active (`STU_9028E035`, `vm/statuses.rs`).
    Status,
    /// A combat modifier on the body while active (`CombatModFilter`).
    CombatMod,
    /// Auras on the bodies its targets list while active (ApplyAura
    /// `STU_431F3FCC`, `vm/auras.rs`); the bodies are [`State::tracked`].
    ApplyAura,
    /// Melee's hit list (`STU_77E13026`). The hits melee's ModifyHealth
    /// makes go to the match, not to this list, so it records none.
    /// Unverified.
    HitList,
    WeaponVolley {
        volleys: u8,
        offset_ms: u64,
        ammo0: i64,
        ammo_var: Option<Variable>,
    },
    /// Runs its out plug and subgraph in the instance's deferred phase, after
    /// the ticks (`Defer` in `ow174/game/script/nodes.py`).
    Defer,
    /// A client-only pulse counter (`ClientOnlyPulser`): pulses so far (mod
    /// 256) and when the last one after the instance's first frame came.
    Pulser {
        pulses: u8,
        pulsed_ms: Option<u64>,
    },
    /// Blocks the entity's logical buttons with this id while active
    /// (`DisableLogicalButton`).
    DisableButton {
        button: i64,
    },
    /// Holds an AI weapon slot's button down while active (`AIFireWeapon`).
    AiFire {
        button: Option<u8>,
    },
    /// Finishes after its count of server frames (`WaitForServerFrames`).
    ServerFrames,
    /// A fixed world target and speed for bot movement (`MoveTo`); the
    /// request is in [`Component::move_requests`].
    MoveTo,
    /// Recall's rewind (`STU_41B3FC2B`): finishes after its duration; its
    /// begin and end are [`Motion`]s for the mover.
    Rewind,
}

#[derive(Clone, Copy, Debug)]
struct BufferedPress {
    time_ms: u64,
    activation: u16,
}

#[derive(Clone, Debug)]
pub struct State {
    pub active: bool,
    pub activation: u16,
    pub start_ms: u64,
    pub kind: StateKind,
    pub chase: Option<ChasePayload>,
    pub ability_cooldown: Option<AbilityCooldown>,
    /// A target search's bodies, in the order they came in, while it runs
    /// (`vm/targets.rs`).
    pub tracked: Vec<TrackedTarget>,
    watch_seen: Option<Value>,
    ending: bool,
    pending_change: bool,
    pending_timer: bool,
    /// A completion check (`Event::Check`) is queued.
    pending_check: bool,
    delayed_timers: [bool; 2],
    buffered_press: Option<BufferedPress>,
    linked_variable: Option<VarKey>,
    ability_stack: Option<VarKey>,
}

impl State {
    fn dormant(kind: StateKind) -> Self {
        Self {
            active: false,
            activation: 0,
            start_ms: 0,
            kind,
            chase: None,
            ability_cooldown: None,
            tracked: Vec::new(),
            watch_seen: None,
            ending: false,
            pending_change: false,
            pending_timer: false,
            pending_check: false,
            delayed_timers: [false; 2],
            buffered_press: None,
            linked_variable: None,
            ability_stack: None,
        }
    }
}

/// A body a target search found, and when it came in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackedTarget {
    pub entity: u32,
    pub since_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbilityCooldown {
    pub current: f64,
    pub rate: f64,
    pub last_ms: u64,
}

impl AbilityCooldown {
    fn remaining(self, now_ms: u64) -> f64 {
        let elapsed = (now_ms.saturating_sub(self.last_ms) as f64 * -f64::from(0.001_f32)) as f32;
        let consumed = (f64::from(elapsed) * self.rate) as f32;
        f64::from(((f64::from(consumed) + self.current) as f32).max(0.0))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChasePayload {
    pub current: Value,
    pub reached: bool,
    pub remaining_ms: Option<i64>,
    pub last_ms: u64,
}

/// The instance variable the engine sets to the entity an instance was
/// created with, its creator (the client's instance creation
/// `sub_7FF789887F20`/`sub_7FF78988AA20` → setter `sub_7FF78AAA4B10`;
/// HealthAuras.md, "Client tools pass 2 (2026-11-16)").
pub(crate) const CREATOR: u32 = 27;

#[derive(Clone, Debug)]
pub struct Instance {
    pub graph: u32,
    /// Parent instance and raw state index, as carried on the wire.
    pub parent: Option<(u16, u16)>,
    /// The entity it was created with ([`CREATOR`]); none when that was no
    /// entity.
    pub source: Option<u32>,
    /// When the instance was created.
    pub start_ms: u64,
    pub variables: Variables,
    pub states: IndexMap<u16, State>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Issue {
    MissingGraph(u32),
    MissingNode(u16),
    MissingState(u16),
    UnsupportedAction(String),
    UnsupportedState(String),
    UnsupportedField(&'static str),
    InvalidSubScript,
    Expression(EvalError),
}

#[derive(Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub site: Site,
    pub issue: Issue,
}

/// An answer the server gives without knowing it is right: what the Python
/// reference did where the meaning is still open. Unlike a diagnostic it
/// doesn't stop the body; it is recorded so tests and notes can list them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assumption {
    pub site: Site,
    /// The node or config class the answer is for.
    pub class: String,
    pub answer: &'static str,
}

#[derive(Debug)]
pub struct Snapshot {
    pub entity: u32,
    pub variables: Variables,
    pub instances: IndexMap<u16, Instance>,
    /// Pending state timers and finishes in their per-instance execution order.
    pub pending_timers: IndexMap<u16, Vec<PendingTimer>>,
    /// Stack links by (instance or zero for the entity, variable).
    pub bindings: IndexMap<(u16, u32), Vec<StackBinding>>,
    /// Effective linked values, separate from their stored bases.
    pub resolved_bindings: IndexMap<(u16, u32), Value>,
    pub diagnostics: Vec<Diagnostic>,
    /// Unverified answers given so far, each site and class once.
    pub assumptions: Vec<Assumption>,
    /// The communication wheel's networked actions that ran, oldest first,
    /// the last [`MAX_FRAME_ACTIONS`].
    pub frame_actions: Vec<FrameAction>,
    /// The component's clock when this snapshot was taken.
    pub now_ms: u64,
}

/// How many recent wheel actions a body keeps (upstream
/// `MAX_WHEEL_CALLOUTS`, server delivery policy).
pub const MAX_FRAME_ACTIONS: usize = 32;

/// A networked action that ran on the server and goes to the clients in
/// the remote message list of the body's frames (104319 A0CCA0; upstream
/// `WheelCallout`): a callout (`STU_7106C356`) or an effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameAction {
    /// Counts up from 1 on this body.
    pub serial: u32,
    pub instance: u16,
    /// The instance's creation time, so a reused id doesn't match.
    pub instance_start_ms: u64,
    /// The node's position in its graph.
    pub node: u16,
    pub time_ms: u64,
}

impl Snapshot {
    /// The reached initialization path was supported. This does not check
    /// dormant paths, later input, timers, messages, or gameplay.
    pub fn initialization_complete(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum InitFault {
    InvalidBudget,
    InvalidRoot(u16),
    InstanceIdExhausted,
    MissingPlayerGraph(u32),
    RootBinding {
        id: u16,
        graph: u32,
        error: EvalError,
    },
    WorkBudget {
        limit: usize,
        operation: &'static str,
    },
    EventBudget {
        instance: u16,
    },
    PendingEvents {
        limit: usize,
    },
    PendingShots,
    FrameOrder {
        frame: u32,
        now_ms: u64,
    },
    Halted,
    PlugDepth,
    UpdateBudget {
        instance: u16,
    },
}

impl fmt::Display for InitFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBudget => {
                f.write_str("statescript initialization budget must be positive")
            }
            Self::InvalidRoot(id) => write!(f, "invalid or duplicate statescript root {id}"),
            Self::InstanceIdExhausted => {
                f.write_str("statescript initialization instance IDs exhausted")
            }
            Self::MissingPlayerGraph(graph) => {
                write!(f, "player statescript graph {graph:04X} is missing")
            }
            Self::RootBinding { id, graph, error } => {
                write!(
                    f,
                    "statescript root {id} graph {graph:04X} binding failed: {error}"
                )
            }
            Self::WorkBudget { limit, operation } => {
                write!(
                    f,
                    "statescript initialization work budget {limit} exhausted during {operation}"
                )
            }
            Self::EventBudget { instance } => {
                write!(
                    f,
                    "statescript initialization event budget exhausted in instance {instance}"
                )
            }
            Self::PendingEvents { limit } => {
                write!(f, "statescript pending event limit {limit} exceeded")
            }
            Self::PendingShots => f.write_str("statescript shot queue limit exceeded"),
            Self::FrameOrder { frame, now_ms } => {
                write!(f, "statescript frame {frame} starts before {now_ms} ms")
            }
            Self::Halted => f.write_str("statescript component halted after a failed operation"),
            Self::PlugDepth => f.write_str("statescript initialization plug depth exceeded"),
            Self::UpdateBudget { instance } => {
                write!(
                    f,
                    "statescript instance {instance} kept deferring work past its update budget"
                )
            }
        }
    }
}

impl Error for InitFault {}

impl From<QueueFull> for InitFault {
    fn from(error: QueueFull) -> Self {
        Self::PendingEvents { limit: error.limit }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingTimer {
    pub due_ms: u64,
    pub raw_state: u16,
    pub finish: bool,
    pub parameter: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackBinding {
    pub instance: u16,
    pub raw_state: u16,
    pub priority: StackPriority,
    pub slot: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotEvent {
    pub entity: u32,
    pub instance: u16,
    pub graph: u32,
    pub raw_state: u16,
    pub frame: u32,
    pub time_ms: u64,
    pub activation: u16,
    pub activation_ms: u64,
    pub ordinal: u32,
    /// The volley's normalized spread when the shot left (0 to 1), for a
    /// launch `STU_239B571E` with a spread ramp; else None.
    pub spread: Option<f64>,
}

/// A single body using the same authored graph engine as one-shot initialization.
pub struct Component {
    graphs: Arc<GraphStore>,
    machine: Machine,
    halted: bool,
}

impl Component {
    pub fn start(
        graphs: Arc<GraphStore>,
        entity: u32,
        entity_variables: Variables,
        roots: Vec<Root>,
        frame: u32,
        world: &mut impl World,
        max_work: usize,
    ) -> Result<Self, InitFault> {
        let mut machine = Machine::new(entity, entity_variables, frame, max_work)?;
        Vm::new(&graphs, world, &mut machine).start(roots)?;
        machine.capture_outputs();
        Ok(Self {
            graphs,
            machine,
            halted: false,
        })
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        (!self.halted).then_some(&self.machine.snapshot)
    }

    pub fn now_ms(&self) -> u64 {
        self.machine.now_ms
    }

    /// Consume reports only after the body graph path is ready for authority.
    pub fn take_shots(&mut self) -> Option<Vec<ShotEvent>> {
        let shots = std::mem::take(&mut self.machine.shots);
        self.snapshot()?.initialization_complete().then_some(shots)
    }

    pub fn shot_node(&self, shot: &ShotEvent) -> Option<&Node> {
        let snapshot = self.snapshot()?;
        let instance = snapshot.instances.get(&shot.instance)?;
        if snapshot.entity != shot.entity || instance.graph != shot.graph {
            return None;
        }
        self.graphs
            .graph(shot.graph)?
            .state(usize::from(shot.raw_state))
            .filter(|node| node.class == "STUStatescriptStateWeaponVolley")
    }

    pub fn evaluate_shot_config(
        &self,
        shot: &ShotEvent,
        config: &Json,
        world: &mut impl World,
    ) -> Result<Value, EvalError> {
        self.shot_node(shot)
            .ok_or(EvalError::MalformedConfig("shot source"))?;
        self.evaluate_state_config(shot.instance, shot.raw_state, config, world)
    }

    pub fn evaluate_state_config(
        &self,
        instance: u16,
        raw_state: u16,
        config: &Json,
        world: &mut impl World,
    ) -> Result<Value, EvalError> {
        let snapshot = self
            .snapshot()
            .ok_or(EvalError::MalformedConfig("state source"))?;
        let graph = snapshot
            .instances
            .get(&instance)
            .ok_or(EvalError::MalformedConfig("state source"))?
            .graph;
        let node = self
            .graphs
            .graph(graph)
            .and_then(|graph| graph.state(usize::from(raw_state)))
            .ok_or(EvalError::MalformedConfig("state source"))?;
        let mut context = InstanceContext {
            machine: &self.machine,
            world,
            site: Site {
                entity: snapshot.entity,
                instance,
                graph,
                node: Some(node.pos),
            },
            reads: Vec::new(),
            parameters: None,
            candidate: None,
            watcher: None,
            read_native: false,
            // A read outside the VM's run: nothing is recorded.
            assumed: Vec::new(),
        };
        expr::evaluate(config, &mut context)
    }

    pub fn resolved_variable(&self, instance: u16, variable: Variable) -> Option<Value> {
        (!self.halted).then(|| self.machine.value(source_key(instance, variable), None))
    }

    pub fn set_entity_variable(
        &mut self,
        id: u32,
        value: Value,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        self.run(world, |vm| {
            let key = VarKey::Entity(id);
            let old = vm.machine.value(key, None);
            vm.set_base(key, value);
            vm.notify_value(key, &old, None)
        })
    }

    /// Set an instance's variable as the server's own input; nothing when
    /// the instance isn't there.
    pub fn set_instance_variable(
        &mut self,
        instance: u16,
        id: u32,
        value: Value,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        if !self.machine.snapshot.instances.contains_key(&instance) {
            return Ok(());
        }
        self.run(world, |vm| {
            let key = VarKey::Instance(instance, id);
            let old = vm.machine.value(key, None);
            vm.set_base(key, value);
            vm.notify_value(key, &old, None)
        })
    }

    /// Authoritative modifiers require a fully supported runtime path.
    pub fn movement_modifiers(
        &mut self,
        world: &mut impl World,
    ) -> Result<Option<BTreeMap<ModifierKey, Modifier>>, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        if !self.machine.snapshot.initialization_complete() {
            return Ok(None);
        }
        let mut modifiers = BTreeMap::new();
        self.run(world, |vm| {
            modifiers = vm.collect_movement_modifiers()?;
            Ok(())
        })?;
        Ok(self
            .machine
            .snapshot
            .initialization_complete()
            .then_some(modifiers))
    }

    /// Queue the authored entries of one instance at the current clock.
    pub fn queue_message(
        &mut self,
        instance: u16,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        self.machine
            .queue_entries(&self.graphs, Some(instance), 0, message, parameters)
    }

    pub fn run_now(&mut self, world: &mut impl World) -> Result<(), InitFault> {
        self.run(world, |vm| vm.run())
    }

    pub fn advance_frame(&mut self, frame: u32, world: &mut impl World) -> Result<(), InitFault> {
        self.advance(frame, None, world)
    }

    pub fn advance_command(
        &mut self,
        command: Command,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        self.advance(command.frame, Some(command), world)
    }

    pub fn set_owner_dead(&mut self, dead: bool) {
        self.machine.owner_dead = dead;
    }

    pub fn owner_dead(&self) -> bool {
        self.machine.owner_dead
    }

    /// A game message from another entity (`sender`; none: 0): every
    /// instance's entries for it queue at the current clock and run now
    /// (`send_message` in `ow174/game/script/runtime.py`, as
    /// `Match.deliver_body_messages` calls it). Returns how many entries
    /// took it.
    pub fn receive_message(
        &mut self,
        sender: u32,
        message: u64,
        parameters: Variables,
        world: &mut impl World,
    ) -> Result<usize, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        let entries =
            self.machine
                .queue_entries(&self.graphs, None, sender, message, parameters)?;
        if entries != 0 {
            self.run_now(world)?;
        }
        Ok(entries)
    }

    /// A persistent sender (`sender`) let its message go for this entity
    /// ([`World::end_message`]): the receiving states it held go back to
    /// idle now, and what that begins runs.
    pub fn end_message(
        &mut self,
        sender: u32,
        message: u64,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        self.run(world, |vm| {
            vm.release_message(sender, message)?;
            vm.run()
        })
    }

    /// A new root instance of `graph` with the next free id. Its entries
    /// queue at the current clock and run at the next update
    /// (`Component.create` in `ow174/game/script/runtime.py`).
    pub fn add_instance(
        &mut self,
        graph: u32,
        world: &mut impl World,
    ) -> Result<Option<u16>, InitFault> {
        let id = u16::try_from(self.machine.next_id).map_err(|_| InitFault::InstanceIdExhausted)?;
        let mut created = false;
        self.run(world, |vm| {
            created = vm.create(
                id,
                graph,
                None,
                Variables::new(),
                vm.machine.snapshot.entity,
            )?;
            Ok(())
        })?;
        Ok(created.then_some(id))
    }

    /// End an instance and its children now, without their end plugs
    /// (`Component.destroy`).
    pub fn destroy_instance(&mut self, id: u16, world: &mut impl World) -> Result<(), InitFault> {
        if !self.machine.snapshot.instances.contains_key(&id) {
            return Ok(());
        }
        self.run(world, |vm| vm.destroy(id))
    }

    /// Let go of every held logical button without edges, as a dead bot
    /// does (`BotScript.die` clears `Component.held`).
    pub fn release_buttons(&mut self) {
        self.machine.held_buttons.clear();
    }

    /// Drop the shots not taken yet.
    pub fn discard_shots(&mut self) {
        self.machine.shots.clear();
    }

    /// When the next queued event is due.
    pub fn next_due_ms(&self) -> Option<u64> {
        self.machine.queues.next_due()
    }

    /// The active `MoveTo` states' targets, for the mover
    /// (`MoveTo.move_request` as `ow174/game/match.py` collects them).
    pub fn move_requests(&self) -> Vec<MoveRequest> {
        self.machine
            .movements
            .iter()
            .filter(|(key, _)| {
                self.machine
                    .snapshot
                    .instances
                    .get(&key.instance)
                    .and_then(|instance| instance.states.get(&key.raw))
                    .is_some_and(|state| state.active)
            })
            .map(|(key, &(target, speed))| MoveRequest {
                instance: key.instance,
                raw_state: key.raw,
                target,
                speed,
            })
            .collect()
    }

    /// The dashes and rewinds begun or ended since the last call, in order,
    /// for the mover.
    pub fn take_motions(&mut self) -> Vec<Motion> {
        std::mem::take(&mut self.machine.motions)
    }

    /// Begin a state from outside the graph (`State.begin` in
    /// `ow174/game/script/runtime.py`, as `VoiceLines.spawned` in
    /// `ow174/game/voicelines.py` calls it). Its begin and the plugs it
    /// follows run now; the events they queue at the current clock run at the
    /// next advance, as in Python. False when the instance or the state is
    /// missing or the state is already on.
    pub fn begin_state(
        &mut self,
        instance: u16,
        raw_state: u16,
        world: &mut impl World,
    ) -> Result<bool, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        let key = StateKey {
            instance,
            raw: raw_state,
        };
        if !self.has_state(key) || self.state_active(key) {
            return Ok(false);
        }
        self.run(world, |vm| vm.begin(key, 0))?;
        Ok(true)
    }

    /// End an active state from outside the graph, finished or aborted
    /// (`State.end` in `ow174/game/script/runtime.py`, as `VoiceLines.expire`
    /// in `ow174/game/voicelines.py` calls `state.end(True)`). Its end plugs
    /// run now; the events they queue run at the next advance. False when the
    /// state is not on.
    pub fn end_state(
        &mut self,
        instance: u16,
        raw_state: u16,
        finished: bool,
        world: &mut impl World,
    ) -> Result<bool, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        let key = StateKey {
            instance,
            raw: raw_state,
        };
        if !self.state_active(key) {
            return Ok(false);
        }
        self.run(world, |vm| vm.end(key, finished, 0))?;
        Ok(true)
    }

    /// Queue a `MoveTo` state's end once the mover reports its movement over:
    /// a finish when it arrived, else an abort, `delay_ms` after the current
    /// clock. It runs at the next advance, not now (`body_command` in
    /// `ow174/game/match.py` calls `state.request(FINISH if arrived else
    /// ABORT, max(0, delay))`, `State.request` in
    /// `ow174/game/script/runtime.py`).
    pub fn finish_move(
        &mut self,
        instance: u16,
        raw_state: u16,
        arrived: bool,
        delay_ms: u64,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        if !self.machine.snapshot.instances.contains_key(&instance) {
            return Ok(());
        }
        let event = if arrived {
            Event::Finish(raw_state)
        } else {
            Event::Abort(raw_state)
        };
        self.run(world, |vm| vm.post_after(instance, delay_ms, event))
    }

    fn has_state(&self, key: StateKey) -> bool {
        self.machine
            .snapshot
            .instances
            .get(&key.instance)
            .and_then(|instance| self.graphs.graph(instance.graph))
            .is_some_and(|graph| graph.state(usize::from(key.raw)).is_some())
    }

    fn state_active(&self, key: StateKey) -> bool {
        self.machine
            .snapshot
            .instances
            .get(&key.instance)
            .and_then(|instance| instance.states.get(&key.raw))
            .is_some_and(|state| state.active)
    }

    fn advance(
        &mut self,
        frame: u32,
        command: Option<Command>,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        if frame_start(frame) < self.machine.now_ms {
            return Err(InitFault::FrameOrder {
                frame,
                now_ms: self.machine.now_ms,
            });
        }
        self.run(world, |vm| vm.advance_frame(frame, command))
    }

    fn run<W: World>(
        &mut self,
        world: &mut W,
        operation: impl FnOnce(&mut Vm<'_, '_, '_, W>) -> Result<(), InitFault>,
    ) -> Result<(), InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        self.machine.work_left = self.machine.max_work;
        let result = operation(&mut Vm::new(&self.graphs, world, &mut self.machine));
        self.halted = result.is_err();
        if result.is_ok() {
            self.machine.capture_outputs();
        }
        result
    }
}

pub fn initialize(
    graphs: &GraphStore,
    entity: u32,
    entity_variables: Variables,
    roots: Vec<Root>,
    world: &mut impl World,
    max_work: usize,
) -> Result<Snapshot, InitFault> {
    let mut machine = Machine::new(entity, entity_variables, 0, max_work)?;
    Vm::new(graphs, world, &mut machine).start(roots)?;
    machine.capture_outputs();
    Ok(machine.snapshot)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum VarKey {
    Entity(u32),
    Instance(u16, u32),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct StateKey {
    instance: u16,
    raw: u16,
}

enum LinkValue {
    Fixed(Value),
    Volley(u8),
    Chase,
    Cooldown,
    AbilityStack,
}

struct VariableLink {
    state: StateKey,
    priority: StackPriority,
    output: LinkValue,
    stack: bool,
}

struct Volley {
    start_ms: u64,
    ammo0: i64,
    per_shot: i64,
    rate: f64,
    /// The spread variable's value when the volley began, 0 to 1.
    spread0: f64,
    /// The shots that take the spread from `spread0` to 1: launch
    /// `STU_239B571E`'s `m_AC20B1C1` as float32 (none: the spread stays).
    ramp: Option<f32>,
    /// The spread output as last published (each tick re-evaluates it).
    spread: f64,
    planned: u32,
    reported: u32,
    outputs: [Option<VarKey>; 3],
}

impl Volley {
    /// The float-based shot index the outputs use; None before the start
    /// (`WeaponVolley.shots_at` in `ow174/game/script/nodes.py`).
    fn shots_at(&self, now_ms: u64) -> Option<i64> {
        let elapsed = now_ms.checked_sub(self.start_ms)?;
        let seconds = f64::from(elapsed as f32 * 0.001_f32);
        Some(((seconds * self.rate) as f32).floor() as i64)
    }

    fn ammo_at(&self, now_ms: u64) -> i64 {
        let Some(shots) = self.shots_at(now_ms) else {
            return self.ammo0;
        };
        self.ammo0
            .saturating_sub(self.per_shot.saturating_mul(shots.saturating_add(1)))
            .max(0)
    }

    /// The normalized spread (0 to 1, not an angle) of launch
    /// `STU_239B571E`: the completed shots plus one over the ramp, added to
    /// the spread the volley began with, at most 1; a ramp below 1e-7 is
    /// full spread at once. Client PC104319 `D80A50`-`D80A99`, the shots
    /// from `D5F979`-`D5F996`, as leafall903/ORPHEUS (MIT) has it in
    /// `nodes.py` `WeaponVolley.spread_at` (commit e5463e3).
    fn spread_at(&self, now_ms: u64) -> f64 {
        let Some(ramp) = self.ramp else {
            return self.spread0;
        };
        if ramp < 1e-7 {
            return 1.0;
        }
        let shots = self.shots_at(now_ms).map_or(0, |shots| (shots + 1).max(0));
        let increase = (shots as f64 / f64::from(ramp)) as f32;
        f64::from((increase + self.spread0 as f32).min(1.0))
    }

    fn output(&self, slot: u8, now_ms: u64) -> Value {
        match slot {
            0 => Value::Integer(self.ammo_at(now_ms)),
            1 => Value::Float(self.rate),
            2 => Value::Float(self.spread),
            _ => unreachable!("weapon volley output slot"),
        }
    }
}

#[derive(Clone, Copy)]
struct AbilityPress {
    button: u8,
    time_ms: u64,
    activation: u16,
}

/// One distinct input that reached a state-input join (`STU_1113EBD5`) of an
/// instance: the join, the node whose plug led there and that plug's path.
struct JoinArrival {
    join: u16,
    producer: u16,
    plug: Box<str>,
}

enum Event {
    Entry(u16),
    ExitInstance,
    Begin(u16),
    Finish(u16),
    Abort(u16),
    Changed(u16),
    Reevaluate(u16),
    Timer(u16, u16),
    Button(ButtonEdge),
    AbilityCheck(u16),
    /// A completion check of the state (TryComplete) a join asked for.
    Check(u16),
    ChaseReached(u16),
    WatchChanged(u16),
    Message {
        position: u16,
        /// The sending entity; 0 when unknown.
        sender: u32,
        parameters: Arc<Variables>,
    },
}

impl Event {
    fn state(&self) -> Option<u16> {
        match *self {
            Self::Entry(_) | Self::ExitInstance | Self::Message { .. } | Self::Button(_) => None,
            Self::Begin(state)
            | Self::Finish(state)
            | Self::Abort(state)
            | Self::Changed(state)
            | Self::Reevaluate(state)
            | Self::AbilityCheck(state)
            | Self::Check(state)
            | Self::ChaseReached(state)
            | Self::WatchChanged(state)
            | Self::Timer(state, _) => Some(state),
        }
    }
}

/// Whom a SendGameMessage names: its message (0: none), the entities, and
/// whether it goes to every entity.
struct Recipients {
    message: u64,
    entities: Vec<u32>,
    broadcast: bool,
}

/// The game message a node sends or waits for; None for none.
fn message_of(node: &Node) -> Option<u64> {
    node.field("m_gameMessage.m_gameMessage")
        .and_then(asset_guid)
        .filter(|&id| id != 0)
}

struct Machine {
    snapshot: Snapshot,
    queues: Timeline<Event>,
    watchers: BTreeMap<VarKey, Vec<StateKey>>,
    ticking: BTreeMap<u16, IndexMap<u16, u64>>,
    links: BTreeMap<VarKey, Vec<VariableLink>>,
    volleys: BTreeMap<StateKey, Volley>,
    shots: Vec<ShotEvent>,
    ability_presses: BTreeMap<StateKey, Vec<AbilityPress>>,
    /// Each instance's pending join inputs, in arrival order.
    joins: BTreeMap<u16, Vec<JoinArrival>>,
    next_id: u32,
    work_left: usize,
    max_work: usize,
    frame: u32,
    now_ms: u64,
    random_state: Cell<u64>,
    command_buttons: u32,
    held_buttons: Vec<u8>,
    owner_dead: bool,
    /// Each instance's `Defer` states due in its deferred phase: priority and
    /// raw state, sorted by priority.
    deferred: BTreeMap<u16, Vec<(f64, u16)>>,
    /// The active `DisableLogicalButton` states by the button they block.
    disabled: BTreeMap<i64, Vec<StateKey>>,
    /// `MoveTo` targets and speeds of the activations that resolved one.
    movements: BTreeMap<StateKey, ([f64; 3], f64)>,
    /// The running HeroSelect states' hero limits (`vm/hero_select.rs`).
    hero_selects: BTreeMap<StateKey, hero_select::HeroLimits>,
    /// Motions started or ended since the mover took them.
    motions: Vec<Motion>,
    /// The auras ApplyAura states elsewhere put on this body, by who put
    /// them there: their instances (`vm/auras.rs`).
    applied_auras: BTreeMap<AuraSource, u16>,
}

struct Vm<'graph, 'world, 'machine, W> {
    graphs: &'graph GraphStore,
    world: &'world mut W,
    machine: &'machine mut Machine,
    parameters: Option<Arc<Variables>>,
    /// The value every parameter reads while `CycleIndex` tests a candidate.
    candidate: Option<Value>,
    output_write_depth: usize,
}

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    fn new(
        graphs: &'graph GraphStore,
        world: &'world mut W,
        machine: &'machine mut Machine,
    ) -> Self {
        Self {
            graphs,
            world,
            machine,
            parameters: None,
            candidate: None,
            output_write_depth: 0,
        }
    }

    fn start(&mut self, roots: Vec<Root>) -> Result<(), InitFault> {
        let mut seen = HashSet::new();
        for root in &roots {
            if root.id == 0 || !seen.insert(root.id) {
                return Err(InitFault::InvalidRoot(root.id));
            }
        }
        let entity = self.machine.snapshot.entity;
        for root in roots {
            self.create(root.id, root.graph, None, root.variables, entity)?;
        }
        self.run()
    }

    fn spend(&mut self, operation: &'static str) -> Result<(), InitFault> {
        self.machine.work_left =
            self.machine
                .work_left
                .checked_sub(1)
                .ok_or(InitFault::WorkBudget {
                    limit: self.machine.max_work,
                    operation,
                })?;
        Ok(())
    }

    /// A new instance of `graph`, created with `source` (its [`CREATOR`],
    /// set when it is an entity): the component's own entity for its roots,
    /// the applier for an aura, the creating instance's own source for a
    /// child or a played script (assumption: the client's PlayScript and
    /// SubScript pass it on; that makes `0C71`'s children `0B99`/`0C72`
    /// see the grenade's thrower, as their AI damage test expects).
    fn create(
        &mut self,
        id: u16,
        graph: u32,
        parent: Option<(u16, u16)>,
        variables: Variables,
        source: u32,
    ) -> Result<bool, InitFault> {
        self.spend("instance creation")?;
        let Some(definition) = self.graphs.graph(graph) else {
            self.report(
                Site {
                    entity: self.machine.snapshot.entity,
                    instance: id,
                    graph,
                    node: None,
                },
                Issue::MissingGraph(graph),
            );
            return Ok(false);
        };
        let source = expr::valid_entity(source).then_some(source);
        self.machine.next_id = self.machine.next_id.max(u32::from(id) + 1);
        self.machine.snapshot.instances.insert(
            id,
            Instance {
                graph,
                parent,
                source,
                start_ms: self.machine.now_ms,
                variables,
                states: IndexMap::new(),
            },
        );
        for position in definition.entries.iter().flatten() {
            if definition
                .node(*position)
                .is_some_and(|node| !node.client_only && is_entry(node))
            {
                self.post(id, Event::Entry(*position))?;
            }
        }
        Ok(true)
    }

    /// The entity an instance was created with; 0 for none.
    fn source_of(&self, id: u16) -> u32 {
        self.machine
            .snapshot
            .instances
            .get(&id)
            .and_then(|instance| instance.source)
            .unwrap_or(0)
    }

    fn run(&mut self) -> Result<(), InitFault> {
        loop {
            self.update()?;
            if self
                .machine
                .queues
                .next_due()
                .is_none_or(|due| due > self.machine.now_ms)
            {
                return Ok(());
            }
        }
    }

    fn advance_frame(&mut self, frame: u32, command: Option<Command>) -> Result<(), InitFault> {
        self.machine.frame = frame;
        self.machine.now_ms = frame_start(frame);
        if let Some(command) = command {
            self.queue_buttons(command)?;
        }
        let end = self.machine.now_ms + FRAME_MS - 1;
        loop {
            let due = self
                .machine
                .queues
                .next_due()
                .filter(|&due| due <= end)
                .unwrap_or(end);
            self.machine.now_ms = self.machine.now_ms.max(due);
            self.update()?;
            if self.machine.now_ms == end
                && self.machine.queues.next_due().is_none_or(|due| due > end)
            {
                return self.report_active_volleys();
            }
        }
    }

    /// One update of every instance at the clock: each drains its due
    /// events, ticks its ticking states and runs its deferred states, again
    /// while deferred work ran (`Instance.update` in
    /// `ow174/game/script/runtime.py`).
    fn update(&mut self) -> Result<(), InitFault> {
        self.spend("component update")?;
        let instances: Vec<_> = self.machine.snapshot.instances.keys().copied().collect();
        for id in instances {
            let mut updates = 0;
            while self.machine.snapshot.instances.contains_key(&id) {
                if updates == MAX_INSTANCE_UPDATES {
                    return Err(InitFault::UpdateBudget { instance: id });
                }
                updates += 1;
                self.drain(id)?;
                self.tick_states(id)?;
                if !self.run_deferred(id)? {
                    break;
                }
            }
        }
        Ok(())
    }

    fn drain(&mut self, id: u16) -> Result<(), InitFault> {
        let mut events = 0;
        while let Some(event) = self.machine.queues.pop_due(id, self.machine.now_ms) {
            if events == MAX_EVENTS_PER_UPDATE {
                return Err(InitFault::EventBudget { instance: id });
            }
            events += 1;
            self.spend("event dispatch")?;
            let key = |raw| StateKey { instance: id, raw };
            match event {
                Event::Entry(position) => {
                    if let Some(node) = self.graph(id).node(position) {
                        self.run_node(id, node, None, 0)?;
                    }
                }
                Event::ExitInstance => self.exit_instance(id)?,
                Event::Begin(raw) => self.begin(key(raw), 0)?,
                Event::Finish(raw) => self.end(key(raw), true, 0)?,
                Event::Abort(raw) => self.end(key(raw), false, 0)?,
                Event::Changed(raw) => self.changed(key(raw))?,
                Event::Reevaluate(raw) => self.reevaluate(key(raw), 0)?,
                Event::Timer(raw, parameter) => self.timer(key(raw), parameter)?,
                Event::Button(edge) => self.button_edge(id, edge)?,
                Event::AbilityCheck(raw) => self.timer(key(raw), 1)?,
                Event::Check(raw) => self.check(key(raw))?,
                Event::ChaseReached(raw) => {
                    if self.state(key(raw)).is_some_and(|state| state.active) {
                        let node = self.graph(id).state(usize::from(raw)).expect("chase state");
                        self.try_complete(key(raw), node, 0)?;
                    }
                }
                Event::WatchChanged(raw) => self.deliver_watch(key(raw))?,
                Event::Message {
                    position,
                    sender,
                    parameters,
                } => {
                    self.deliver(id, position, sender, parameters)?;
                }
            }
        }
        Ok(())
    }

    fn graph(&self, id: u16) -> &'graph Graph {
        let graph = self.machine.snapshot.instances[&id].graph;
        self.graphs.graph(graph).expect("registered graph")
    }

    fn site(&self, instance: u16, node: &Node) -> Site {
        Site {
            entity: self.machine.snapshot.entity,
            instance,
            graph: self.machine.snapshot.instances[&instance].graph,
            node: Some(node.pos),
        }
    }

    fn assume(&mut self, site: Site, class: &str, answer: &'static str) {
        let assumptions = &mut self.machine.snapshot.assumptions;
        if !assumptions
            .iter()
            .any(|known| known.site == site && known.class == class)
        {
            assumptions.push(Assumption {
                site,
                class: class.to_owned(),
                answer,
            });
        }
    }

    fn tolerates_unknowns(&self) -> bool {
        self.world.unknowns() == Unknowns::Tolerate
    }

    fn report(&mut self, site: Site, issue: Issue) {
        if self.tolerates_unknowns() {
            let what = match &issue {
                Issue::UnsupportedAction(class) | Issue::UnsupportedState(class) => class.clone(),
                Issue::UnsupportedField(field) => (*field).to_owned(),
                Issue::Expression(error) => error.to_string(),
                issue => format!("{issue:?}"),
            };
            self.assume(site, &what, "skipped");
            return;
        }
        let diagnostic = Diagnostic { site, issue };
        if !self.machine.snapshot.diagnostics.contains(&diagnostic) {
            self.machine.snapshot.diagnostics.push(diagnostic);
        }
    }

    fn post(&mut self, id: u16, event: Event) -> Result<(), InitFault> {
        self.post_after(id, 0, event)
    }

    fn post_after(&mut self, id: u16, delay_ms: u64, event: Event) -> Result<(), InitFault> {
        self.machine
            .queues
            .push(id, self.machine.now_ms.saturating_add(delay_ms), event)?;
        Ok(())
    }

    fn deliver(
        &mut self,
        id: u16,
        position: u16,
        sender: u32,
        parameters: Arc<Variables>,
    ) -> Result<(), InitFault> {
        let Some(node) = self.graph(id).node(position) else {
            return Ok(());
        };
        self.parameters = Some(parameters);
        let result = self.deliver_entry(id, node, sender);
        self.parameters = None;
        result
    }

    /// A game message entry takes its parameters and its sender: the
    /// entity in `m_B60ABDA4` (`#40` in `05AD` rid15, which rates the
    /// sender's health by it), as an applied aura's and a played script's
    /// entry take theirs.
    fn deliver_entry(&mut self, id: u16, node: &Node, sender: u32) -> Result<(), InitFault> {
        let path = if node.is_state() {
            let Some(raw) = node.state else { return Ok(()) };
            let key = StateKey { instance: id, raw };
            let Some(state) = self.state(key).filter(|state| state.active) else {
                return Ok(());
            };
            if state.kind == StateKind::SendMessage {
                // A persistent sender (`STU_38EE1100`) names the message it
                // sends; its own message to every entity is no entry of it.
                // Its notification (`m_67FD6A60`, a timer in the client:
                // D4CDE0 in leafall903/ORPHEUS `docs/MESSAGE_SENDER.txt`) is
                // not modelled.
                return Ok(());
            }
            if state.kind != StateKind::MessageIdle {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedField("state game message delivery"),
                );
                return Ok(());
            }
            self.exit_subgraph(id, node, "m_33DA5163", 1)?;
            // The sender now holds it ([`Self::release_message`]).
            let since_ms = self.machine.now_ms;
            let state = self.state_mut(key).expect("active receiving state");
            if !state.tracked.iter().any(|held| held.entity == sender) {
                state.tracked.push(TrackedTarget {
                    entity: sender,
                    since_ms,
                });
            }
            ("m_452862E6", Some(key))
        } else {
            ("m_outPlug", None)
        };
        let site = self.site(id, node);
        for field in array_field(&node.fields, "m_params") {
            if !field.is_object() {
                continue;
            }
            let target = match expr::lvalue(field.get("m_out_Var").unwrap_or(&Json::Null)) {
                Ok(Some(target)) => source_key(id, target),
                Ok(None) => continue,
                Err(error) => {
                    self.report(site, Issue::Expression(error));
                    return Ok(());
                }
            };
            let key = parameter_id(field);
            let value = self
                .parameters
                .as_deref()
                .and_then(|params| params.get(&key))
                .cloned()
                .unwrap_or(Value::Null);
            self.write(target, value)?;
        }
        if expr::valid_entity(sender) {
            match expr::lvalue(field(&node.fields, "m_B60ABDA4")) {
                Ok(Some(target)) => self.write(source_key(id, target), Value::Entity(sender))?,
                Ok(None) => {}
                Err(error) => {
                    self.report(site, Issue::Expression(error));
                    return Ok(());
                }
            }
        }
        self.follow(id, node, path.0, path.1, 0)?;
        Ok(())
    }

    /// A SendGameMessage action: the message goes once to the recipients
    /// its target names now.
    fn send_game_message(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let Some(recipients) = self.message_recipients(id, node) else {
            return Ok(false);
        };
        self.deliver_game_message(id, node, &recipients, &recipients.entities)
    }

    /// A SendGameMessage state (`STU_38EE1100`) is a persistent sender
    /// (leafall903/ORPHEUS (MIT) `docs/MESSAGE_SENDER.txt` and
    /// `MESSAGE_RECEIVER.txt`, from the client's message manager D4C3E0 and
    /// the receiving state D4BD40): while it runs, the entities its target
    /// names hold its message, and a receiving state (StateGameMessageEntry)
    /// that holds it leaves its idle subgraph for its receive subgraph; when
    /// the sender ends or no longer names an entity, that entity's receiver
    /// goes back to idle. So the state sends its message to the entities its
    /// target names at its begin, then each tick to those that came in since
    /// ([`State::tracked`] lists the ones holding it), and ends it for those
    /// that left and, at its own end, for all ([`World::end_message`]). A
    /// target of channels (`STU_1D719351`, every entity) is sent once at
    /// the begin, as before: who joins later is not followed.
    pub(super) fn begin_send_state(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let Some(recipients) = self.message_recipients(key.instance, node) else {
            return Ok(());
        };
        if !self.deliver_game_message(key.instance, node, &recipients, &recipients.entities)? {
            return Ok(());
        }
        if recipients.broadcast || recipients.message == 0 {
            return Ok(());
        }
        let since_ms = self.machine.now_ms;
        if let Some(state) = self.state_mut(key) {
            state.tracked = recipients
                .entities
                .iter()
                .map(|&entity| TrackedTarget { entity, since_ms })
                .collect();
        }
        self.register_tick(key, since_ms);
        Ok(())
    }

    /// A running sender's tick ([`Self::begin_send_state`]): the message
    /// goes to the entities its target names that don't hold it yet, and
    /// ends for those it no longer names.
    pub(super) fn refresh_send_state(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let Some(recipients) = self.message_recipients(key.instance, node) else {
            return Ok(());
        };
        let held: Vec<u32> = self
            .state(key)
            .map(|state| state.tracked.iter().map(|target| target.entity).collect())
            .unwrap_or_default();
        let joined: Vec<u32> = recipients
            .entities
            .iter()
            .copied()
            .filter(|entity| !held.contains(entity))
            .collect();
        let left: Vec<u32> = held
            .iter()
            .copied()
            .filter(|entity| !recipients.entities.contains(entity))
            .collect();
        if joined.is_empty() && left.is_empty() {
            return Ok(());
        }
        if !self.deliver_game_message(key.instance, node, &recipients, &joined)? {
            return Ok(());
        }
        for &entity in &left {
            self.end_game_message(key.instance, node, recipients.message, entity)?;
        }
        let since_ms = self.machine.now_ms;
        if let Some(state) = self.state_mut(key) {
            state
                .tracked
                .retain(|target| !left.contains(&target.entity));
            state.tracked.extend(
                joined
                    .iter()
                    .map(|&entity| TrackedTarget { entity, since_ms }),
            );
        }
        Ok(())
    }

    /// A sender's end: every entity that holds its message lets it go.
    pub(super) fn end_send_state(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let Some(message) = message_of(node) else {
            return Ok(());
        };
        let held: Vec<u32> = self
            .state(key)
            .map(|state| state.tracked.iter().map(|target| target.entity).collect())
            .unwrap_or_default();
        for entity in held {
            self.end_game_message(key.instance, node, message, entity)?;
        }
        Ok(())
    }

    /// `entity` no longer holds the sender's message: its own receiving
    /// states here, another entity's through the world.
    fn end_game_message(
        &mut self,
        id: u16,
        node: &Node,
        message: u64,
        entity: u32,
    ) -> Result<(), InitFault> {
        self.spend("game message end")?;
        let site = self.site(id, node);
        if entity == site.entity {
            self.release_message(site.entity, message)
        } else {
            // A world that delivers no ends leaves the receiver as it is.
            self.world.end_message(site, entity, message);
            Ok(())
        }
    }

    /// The receiving states of `message` that hold it from `sender` go back
    /// to their idle subgraph once no sender holds them
    /// ([`Self::begin_send_state`]).
    pub(super) fn release_message(&mut self, sender: u32, message: u64) -> Result<(), InitFault> {
        let held: Vec<StateKey> = self
            .machine
            .snapshot
            .instances
            .iter()
            .flat_map(|(&instance, item)| {
                item.states
                    .iter()
                    .filter(|(_, state)| {
                        state.active
                            && state.kind == StateKind::MessageIdle
                            && state.tracked.iter().any(|held| held.entity == sender)
                    })
                    .map(move |(&raw, _)| StateKey { instance, raw })
            })
            .collect();
        for key in held {
            let graphs = self.graphs;
            let graph = self.machine.snapshot.instances[&key.instance].graph;
            let node = graphs
                .graph(graph)
                .and_then(|graph| graph.state(usize::from(key.raw)))
                .expect("active receiving state");
            if message_of(node) != Some(message) {
                continue;
            }
            let state = self.state_mut(key).expect("active receiving state");
            state.tracked.retain(|held| held.entity != sender);
            if !state.tracked.is_empty() {
                continue;
            }
            self.exit_subgraph(key.instance, node, "m_452862E6", 1)?;
            self.follow(key.instance, node, "m_33DA5163", Some(key), 0)?;
        }
        Ok(())
    }

    /// Whom a SendGameMessage names now; None when its target can't be
    /// evaluated (reported).
    fn message_recipients(&mut self, id: u16, node: &Node) -> Option<Recipients> {
        let nobody = Recipients {
            message: 0,
            entities: Vec::new(),
            broadcast: false,
        };
        let Some(message) = message_of(node) else {
            return Some(nobody);
        };
        let site = self.site(id, node);
        let mut target = node
            .fields
            .get("m_targets")
            .or_else(|| node.fields.get("m_target"))
            .unwrap_or(&Json::Null);
        // A SendGameMessage state's target (`m_target`) is a target kind:
        // `STU_9B213168` is the entities of its `m_targets`, its only field
        // in all 134 uses in 104319 (all in `STU_38EE1100`; `0C8E` st41
        // sends `08BB` to the mode entity so). Inferred from its shape, not
        // read in the client.
        //
        // `STU_1D719351` names no entity: its three bytes pick the message
        // manager's global channels (leafall903/ORPHEUS (MIT),
        // `docs/MESSAGE_SENDER.txt`: `m_6A8DB059` the primary channel,
        // `m_1199BFE0` and `m_111E4A0E` two more in a mode). Read from who
        // takes its 289 messages in 104319: with `m_6A8DB059` set they reach
        // every entity, the sender's own instances among them (the mode
        // graphs' `0692`/`0694`/`0695`/`0700`/`0701` reach the controllers
        // `0C90`, the bodies' `01CA`, the payload `1225` and the mode's own
        // `0CBB`; `0702` the doors `0D32`; `06FD` the bodies' `0C8E`).
        // Messages with only the other two bytes set are all sent by client
        // graphs and taken by none of the server's, so those channels are
        // the clients' [assumption] and the server delivers nothing on them.
        let channels = target.get("$").and_then(Json::as_str) == Some("STU_1D719351");
        if channels && field(target, "m_6A8DB059").as_u64() == Some(0) {
            return Some(nobody);
        }
        // `STU_57692020` names no receivers and the client has no code for
        // it: the server picks them. Its message reaches every entity, the
        // sender's own instances among them, as the primary channel's: the
        // only listener of `0686`'s `0307` is `05AD` rid15 on every hero
        // body, the sender's own included (BotVoice.md, "Client tools pass
        // (2026-11-16)"; the routing itself is an inference from the data).
        let broadcast = channels || node.class == "STU_57692020";
        if target.get("$").and_then(Json::as_str) == Some("STU_9B213168") {
            target = field(target, "m_targets");
        }
        let value = if target.is_null() || broadcast {
            Value::Entity(site.entity)
        } else {
            self.evaluate(id, node, target, None)?
        };
        let mut entities: Vec<u32> = match value {
            Value::Entity(entity) if entity != 0 => vec![entity],
            Value::Array(items) => items
                .into_iter()
                .filter_map(|value| {
                    let Value::Entity(entity) = value else {
                        return None;
                    };
                    (entity != 0 && entity & 0x1fff_ffff != 0x1fff_ffff).then_some(entity)
                })
                .collect(),
            _ => Vec::new(),
        };
        entities.dedup();
        Some(Recipients {
            message,
            entities,
            broadcast,
        })
    }

    /// The message with its parameters, evaluated now, to `entities` (and
    /// to every entity for a broadcast); false when a parameter or the
    /// delivery failed (reported).
    fn deliver_game_message(
        &mut self,
        id: u16,
        node: &Node,
        recipients: &Recipients,
        entities: &[u32],
    ) -> Result<bool, InitFault> {
        if recipients.message == 0 || entities.is_empty() {
            // A message to nobody: its parameters are never read.
            return Ok(true);
        }
        let site = self.site(id, node);
        let message = recipients.message;
        let mut parameters = Variables::new();
        for parameter in array_field(&node.fields, "m_params")
            .iter()
            .filter(|value| value.is_object())
        {
            self.spend("game message parameter")?;
            let Some(value) = self.evaluate(id, node, field(parameter, "m_value"), None) else {
                return Ok(false);
            };
            parameters.insert(parameter_id(parameter), value);
        }
        for &entity in entities {
            self.spend("game message routing")?;
            if entity == site.entity {
                self.machine.queue_entries(
                    self.graphs,
                    None,
                    site.entity,
                    message,
                    parameters.clone(),
                )?;
            } else if !self.world.send_message(site, entity, message, &parameters) {
                self.report(
                    site,
                    Issue::UnsupportedField("external game message delivery"),
                );
                return Ok(false);
            }
        }
        if recipients.broadcast && !self.world.broadcast_message(site, message, &parameters) {
            self.report(
                site,
                Issue::UnsupportedField("broadcast game message delivery"),
            );
            return Ok(false);
        }
        Ok(true)
    }

    fn state(&self, key: StateKey) -> Option<&State> {
        self.machine
            .snapshot
            .instances
            .get(&key.instance)?
            .states
            .get(&key.raw)
    }

    fn state_mut(&mut self, key: StateKey) -> Option<&mut State> {
        self.machine
            .snapshot
            .instances
            .get_mut(&key.instance)?
            .states
            .get_mut(&key.raw)
    }

    fn evaluate(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        watcher: Option<StateKey>,
    ) -> Option<Value> {
        self.evaluate_as(id, node, config, watcher, false)
    }

    /// The value an assignment reads ([`expr::evaluate_value`]).
    fn evaluate_value(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        watcher: Option<StateKey>,
    ) -> Option<Value> {
        self.evaluate_as(id, node, config, watcher, true)
    }

    fn evaluate_as(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        watcher: Option<StateKey>,
        assignment: bool,
    ) -> Option<Value> {
        let site = self.site(id, node);
        let mut context = InstanceContext {
            machine: self.machine,
            world: self.world,
            site,
            reads: Vec::new(),
            parameters: self.parameters.as_deref(),
            candidate: self.candidate.as_ref(),
            watcher,
            read_native: false,
            assumed: Vec::new(),
        };
        let result = if assignment {
            expr::evaluate_value(config, &mut context)
        } else {
            expr::evaluate(config, &mut context)
        };
        let reads = context.reads;
        let read_native = context.read_native;
        for (class, answer) in context.assumed {
            self.assume(site, &class, answer);
        }
        if let Some(watcher) = watcher.filter(|&key| {
            self.state(key)
                .is_some_and(|state| state.active && !state.ending)
        }) {
            for key in reads {
                self.watch(key, watcher);
            }
            if let Some(start) = self
                .state(watcher)
                .filter(|state| {
                    read_native
                        && matches!(
                            state.kind,
                            StateKind::BooleanSwitch(_)
                                | StateKind::Switch { .. }
                                | StateKind::Watch
                        )
                })
                .map(|state| state.start_ms)
            {
                self.register_tick(watcher, start);
            }
        }
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                None
            }
        }
    }

    fn watch(&mut self, key: VarKey, watcher: StateKey) {
        let watchers = self.machine.watchers.entry(key).or_default();
        if !watchers.contains(&watcher) {
            watchers.push(watcher);
        }
    }

    fn write(&mut self, key: VarKey, value: Value) -> Result<(), InitFault> {
        let old = self.machine.value(key, None);
        self.set_base(key, value);
        let outputs: Vec<_> = self
            .machine
            .links
            .get(&key)
            .into_iter()
            .flatten()
            .filter_map(|link| match link.output {
                LinkValue::Chase => Some((link.state, false)),
                LinkValue::Cooldown => Some((link.state, true)),
                _ => None,
            })
            .collect();
        for (state, cooldown) in outputs {
            if !self
                .state(state)
                .is_some_and(|state| state.active && state.linked_variable == Some(key))
            {
                continue;
            }
            if self.output_write_depth == MAX_PLUG_DEPTH {
                return Err(InitFault::PlugDepth);
            }
            self.output_write_depth += 1;
            let result = if cooldown {
                let node = self
                    .graph(state.instance)
                    .state(usize::from(state.raw))
                    .expect("ability state");
                self.restart_cooldown(state, node, self.output_write_depth)
            } else {
                self.chase_write(state, key)
            };
            self.output_write_depth -= 1;
            result?;
        }
        self.notify_value(key, &old, None)
    }

    fn set_base(&mut self, key: VarKey, value: Value) {
        match key {
            VarKey::Entity(id) => self.machine.snapshot.variables.insert(id, value),
            VarKey::Instance(instance, id) => self.machine.snapshot.instances[&instance]
                .variables
                .insert(id, value),
        };
    }

    fn notify_value(
        &mut self,
        key: VarKey,
        old: &Value,
        source: Option<StateKey>,
    ) -> Result<(), InitFault> {
        let changed = !same(old, &self.machine.value(key, None));
        let watchers = self.machine.watchers.get(&key).cloned().unwrap_or_default();
        for watcher in watchers {
            let Some(state) = self
                .state(watcher)
                .filter(|state| state.active && !state.ending && !state.pending_change)
            else {
                continue;
            };
            if matches!(state.kind, StateKind::Stack { .. }) && state.linked_variable == Some(key) {
                if let Some(source) = source {
                    let above = self.machine.links.get(&key).is_some_and(|entries| {
                        entries
                            .iter()
                            .skip_while(|entry| entry.state != source)
                            .skip(1)
                            .any(|entry| entry.state == watcher)
                    });
                    if !above {
                        continue;
                    }
                }
            } else if !changed {
                continue;
            }
            self.state_mut(watcher)
                .expect("active watcher")
                .pending_change = true;
            self.post(watcher.instance, Event::Changed(watcher.raw))?;
        }
        Ok(())
    }
}

fn is_entry(node: &Node) -> bool {
    matches!(node.class.as_str(), "STUStatescriptEntry" | "STU_571CC72E")
}

fn is_message_entry(node: &Node) -> bool {
    node.class == "STUStatescriptEntryGameMessage"
}

fn is_client_subgraph_controller(graph: &Graph, node: &Node) -> bool {
    // The controller is replicated, but its specialized branch starts on the client.
    const BRANCH: &str = "m_5B10D01F";
    let plug = field(&node.fields, BRANCH);
    node.is_networked()
        && field(plug, "$").as_str() == Some("STU_904BFCEC")
        && array_field(plug, "links").len() == 1
        && node
            .links(BRANCH)
            .next()
            .and_then(|(position, _)| graph.node(position?))
            .is_some_and(|target| target.client_only && !target.server_only)
}

/// The `STU_54FEB21C` form that waits on its variable alone (Recall's
/// wind-up, graph `001A` node 4): `m_C7A42E2A` names a variable and none of
/// its other configs is set. Its other forms stay without effect.
fn is_server_ended(node: &Node) -> bool {
    matches!(expr::lvalue(field(&node.fields, "m_C7A42E2A")), Ok(Some(_)))
        && ["m_ED97A97C", "m_7C8AD485", "m_F8CE386A", "m_C46403A5"]
            .iter()
            .all(|name| field(&node.fields, name).is_null())
}

/// The owner's own movement sampler. Its edge plugs (Doomfist, Baptiste)
/// and membership of a subgraph (`m_0B1AA8CA`, Bastion) don't change what it
/// samples; another entity's (`m_entity`, Mei) and the extra output
/// `m_A10CC74B` aren't supported.
fn is_owner_movement_sampler(node: &Node) -> bool {
    node.is_networked()
        && node.remote == -1
        && [
            "m_entity",
            "m_A10CC74B",
            "m_4F8F2F3F",
            "m_8BF03679",
            "m_BF5B22B7",
        ]
        .iter()
        .all(|name| field(&node.fields, name).is_null())
        && MOVEMENT_BOOLEAN_OUTPUTS
            .iter()
            .all(|name| expr::lvalue(field(&node.fields, name)).is_ok())
}

fn is_networked_for_code_synced(node: &Node) -> bool {
    node.is_networked()
        && node.remote >= 0
        && field(&node.fields, "m_DC897C87").as_u64() == Some(1)
        && field(&node.fields, "m_598E32D5").as_u64() == Some(0)
        && field(&node.fields, "m_0582B0E4").as_u64() == Some(0)
        && field(&node.fields, "m_0B1AA8CA").is_null()
        && field(&node.fields, "m_values")
            .as_array()
            .is_some_and(|values| {
                !values.is_empty()
                    && values.iter().all(|value| {
                        field(value, "$").as_str() == Some("STUStatescriptForCodeValue")
                            && field(field(value, "m_value"), "$").as_str().is_some()
                    })
            })
}

fn is_base_health_pool(node: &Node) -> bool {
    let config = |name| field(&node.fields, name);
    let is_owner = |value| expr::native_shape(value, "STU_B5A0CAF0", None);
    let flag = |name, expected| {
        let value = config(name);
        expr::native_shape(value, "STUConfigVarBool", Some("m_value"))
            && match field(value, "m_value") {
                Json::Bool(flag) => *flag == expected,
                Json::Number(flag) => flag.as_u64() == Some(u64::from(expected)),
                _ => false,
            }
    };
    let source = config("m_69A20070");
    node.is_networked()
        && [
            "m_7BE02F89",
            "m_C99FAFDD",
            "m_FF79E783",
            "m_AF1104D0",
            "m_BFA825EA",
        ]
        .into_iter()
        .all(|name| config(name).is_null())
        && pool_kind(config("m_7E8B666C")).is_some()
        && is_owner(config("m_FA3ECC43"))
        && expr::native_shape(source, "STU_FEDD683C", Some("m_entity"))
        && is_owner(field(source, "m_entity"))
        && flag("m_4895B8B6", true)
        && flag("m_64839023", false)
        && flag("m_0E8A5C8E", false)
        && matches!(expr::lvalue(config("m_CEAB19E8")), Ok(Some(_)))
}

fn pool_kind(value: &Json) -> Option<PoolKind> {
    match value.as_u64()? {
        0 => Some(PoolKind::Health),
        1 => Some(PoolKind::Armour),
        2 => Some(PoolKind::Shields),
        _ => None,
    }
}

fn pool_number(value: Value) -> Option<f64> {
    match value {
        Value::Float(amount) => Some(amount),
        Value::Integer(amount) => Some(amount as f64),
        _ => None,
    }
}

fn ability_is_stacked(node: &Node) -> bool {
    !field(&node.fields, "m_out_StackVar").is_null() && !field(&node.fields, "m_5E561E77").is_null()
}

fn configured(node: &Node, name: &str) -> bool {
    match field(&node.fields, name) {
        Json::Null | Json::Bool(false) => false,
        Json::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Json::String(value) => !value.is_empty(),
        Json::Array(value) => !value.is_empty(),
        Json::Object(value) => !value.is_empty(),
        Json::Bool(true) => true,
    }
}

fn parameter_id(config: &Json) -> u32 {
    config
        .get("m_B5051BCE")
        .and_then(asset_guid)
        .map_or(0, |guid| u32::from(guid as u16))
}

fn field<'a>(fields: &'a Json, name: &str) -> &'a Json {
    fields.get(name).unwrap_or(&Json::Null)
}

fn array_field<'a>(fields: &'a Json, name: &str) -> &'a [Json] {
    field(fields, name).as_array().map_or(&[], Vec::as_slice)
}

fn raw_indices(values: &[Json]) -> impl Iterator<Item = u16> + '_ {
    values
        .iter()
        .filter_map(|value| u16::try_from(value.as_u64()?).ok())
}

fn case_path(case: Option<usize>) -> String {
    case.map_or_else(
        || "m_A5EF385D".into(),
        |index| format!("m_05706D1C[{index}]"),
    )
}

fn branch_path(branch: bool) -> &'static str {
    if branch {
        "m_trueSubgraphPlug"
    } else {
        "m_falseSubgraphPlug"
    }
}

fn same(a: &Value, b: &Value) -> bool {
    a == b || matches!((a, b), (Value::Float(a), Value::Float(b)) if a.is_nan() && b.is_nan())
}

fn source_key(instance: u16, variable: Variable) -> VarKey {
    match variable.scope {
        Scope::Instance => VarKey::Instance(instance, variable.id),
        Scope::Entity => VarKey::Entity(variable.id),
    }
}

fn stored(snapshot: &Snapshot, key: VarKey) -> Value {
    match key {
        VarKey::Entity(id) => snapshot.variables.get(&id),
        VarKey::Instance(instance, id) => snapshot
            .instances
            .get(&instance)
            .and_then(|instance| instance.variables.get(&id)),
    }
    .cloned()
    .unwrap_or(Value::Null)
}

fn shot_delay(ordinal: u32, rate: f64) -> u64 {
    ((f64::from(ordinal) / rate) as f32 * 1000.0).round() as u64
}

fn chase_value(value: &Value) -> Value {
    match value {
        Value::Vector(_) => value.clone(),
        _ => Value::Float(expr::to_float(value)),
    }
}

fn chase_vector(value: Value) -> [f64; 3] {
    match value {
        Value::Vector(vector) => vector,
        _ => [expr::to_float(&value), 0.0, 0.0],
    }
}

fn chase_lerp(current: Value, destination: Value, share: f64) -> Value {
    let interpolate = |from: f64, to: f64| f64::from((from + (to - from) * share) as f32);
    if let (Value::Float(current), Value::Float(destination)) = (&current, &destination) {
        return Value::Float(interpolate(*current, *destination));
    }
    let current = chase_vector(current);
    let destination = chase_vector(destination);
    Value::Vector(std::array::from_fn(|axis| {
        interpolate(current[axis], destination[axis])
    }))
}

fn chase_step(current: Value, destination: Value, step: f64) -> (Value, bool) {
    if let (Value::Float(current), Value::Float(destination)) = (&current, &destination) {
        let delta = f64::from((*destination - *current) as f32);
        return if delta.abs() <= step {
            (Value::Float(*destination), true)
        } else {
            let next = *current + if delta > 0.0 { step } else { -step };
            (Value::Float(f64::from(next as f32)), false)
        };
    }
    let current = chase_vector(current);
    let destination = chase_vector(destination);
    let delta: [f64; 3] =
        std::array::from_fn(|axis| f64::from((destination[axis] - current[axis]) as f32));
    let length = f64::from((delta.iter().map(|value| value * value).sum::<f64>().sqrt()) as f32);
    if length <= step {
        (Value::Vector(destination), true)
    } else {
        let next = std::array::from_fn(|axis| {
            f64::from((current[axis] + delta[axis] * step / length) as f32)
        });
        (Value::Vector(next), false)
    }
}

struct InstanceContext<'a, W> {
    machine: &'a Machine,
    world: &'a mut W,
    site: Site,
    reads: Vec<VarKey>,
    parameters: Option<&'a Variables>,
    /// Answers every parameter read when set: a `CycleIndex` candidate.
    candidate: Option<&'a Value>,
    watcher: Option<StateKey>,
    read_native: bool,
    /// Native answers the world marked as assumptions, by config class.
    assumed: Vec<(String, &'static str)>,
}

#[cfg(test)]
mod array_tests;
#[cfg(test)]
mod membership_tests;
#[cfg(test)]
mod tests;
