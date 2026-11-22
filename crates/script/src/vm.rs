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
mod grapple;
mod health;
mod hero_select;
mod landing;
mod lifecycle;
mod links;
mod machine;
mod nodes;
mod pickups;
mod play;
mod rays;
mod scopes;
mod shockwave;
mod state_kinds;
mod stats;
mod statuses;
mod switches;
mod targets;
mod timers;
mod turn;
mod view_limits;
mod volleys;
mod walls;
mod workshop;

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
pub use pickups::Pickup;
pub use play::HEALTH_PACK;
pub use scopes::{ParentWrite, ScopeChange, ScopeHit, ScopeLink, ScopeSource};
use serde_json::Value as Json;
pub use statuses::{
    ASSIST_DEFENSIVE, ASSIST_OFFENSIVE, AssistLink, BodyEffects, CombatMod, DAMAGE_DEALT,
    DAMAGE_TAKEN, HEALING_DEALT, HEALING_TAKEN, INVULNERABLE, MODIFIED, PHASED_OUT, PartyFilter,
    REDUCTION_CAP, Scaled, combat_scale,
};
pub(crate) use targets::DEAD as DEAD_FILTER_BIT;
pub use workshop::{ACTION_PARAMETERS, ActionOutcome, RuleTest};

use crate::{
    ai_definitions::{AiControl, AiDefinitions},
    expr::{
        self, Context, EmptyContext, EntityRelation, EvalError, Scope, StackPriority, Variable,
        equal, truthy,
    },
    graph::{Body, Graph, GraphStore, Node, asset_guid, member},
    timeline::{ButtonEdge, FRAME_MS, QueueFull, Timeline, button_edges, frame_start},
    vitals::PoolKind,
    wheel,
    wire::{Value, Variables},
};
use orpheus_physics::mover::{
    Behaviour, MoveTo, PathWalk, Speed,
    modifiers::{
        MOVE_HEADING, Modifier, ModifierKey, Operation, VectorOperation, VelocityElement, heading,
    },
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
/// The timer parameter of a timed chase's arrival ([`TimedChase`]): the
/// owner's client takes its timers from the frames, and its chase handles
/// timer 0 as the arrival (`0x7FF78AAA7F50` → `0x7FF78AAB0960`); timer 1
/// unbinds the chase's variable there.
const CHASE_ARRIVAL: u16 = 0;
const LINK_PRIORITY: StackPriority = StackPriority {
    level: 0,
    above_equals: true,
};
const ABORT_PLUG: &str = "m_F198FD3A";
const SUBGRAPH_STATES: &str = "m_FF3DAF1E";
/// The states that register rows (identifier, value) for game code to read
/// ([`Context::for_code`]): ForCodeSynced and ForCode.
const FOR_CODE_SYNCED: &str = "STU_CD46AF93";
const FOR_CODE: &str = "STU_6A926B21";
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
    /// The entity that controls it (its primary possessor, `STU_DB742151`):
    /// a player's controller, an AI hero's AI entity, a training bot's
    /// stand-in controller; none when the match has none.
    pub controller: Option<u32>,
    /// The player whose body it is: his controller, whose component 44
    /// the clients see possessing it (a human's is also its `controller`;
    /// an AI hero's is not, it is his AI entity); none for a training bot.
    pub player: Option<u32>,
    /// Whether that controller is an AI (has component 92, which
    /// `STU_D3EB8D87` tests): a training bot's, whose body its AI
    /// definition possesses, and an AI hero's AI entity. Players'
    /// controllers are not.
    pub ai: bool,
    /// An AI hero's AI entity (the `controller`) with its definition and
    /// difficulty, whose profile `STU_4049CF77` reads; none for a player
    /// and a training bot (whose AI definitions have no profiles).
    pub ai_control: Option<AiControl>,
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
    /// Its pools' health and their maximum, every kind together: what
    /// `STU_32E6E4D8` and `STU_7D7D2470` read of it from another entity's
    /// graph (the healing Biotic Orb `10FF` st0 finds only hurt allies).
    /// None: unknown.
    pub health: Option<[f64; 2]>,
    /// The spawned entities the match runs whose owner is its controller
    /// (its Biotic Fields: [`crate::placed::MapEntityWorld::spawn`]), for
    /// the owner relation, and the projectiles among them that this body
    /// shot (its Biotic Orbs), for the shooter relation.
    pub spawned: Vec<OwnedEntity>,
    /// Its body definition's model hardpoints in its frame (feet at the
    /// origin, +Z forward), which `STU_FC99C77E` reads; None: unknown.
    pub hardpoints: Option<Arc<Hardpoints>>,
}

/// A body's model hardpoints: each `0DC` GUID's point in the body's frame
/// (`orpheus_assets` `hitboxes`: the bots' idle pose, the heroes' rest).
pub type Hardpoints = BTreeMap<u64, [f64; 3]>;

/// A spawned entity a listed body's controller owns
/// ([`TargetCandidate::spawned`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedEntity {
    pub entity: u32,
    /// A projectile the body shot (component 50, whose `+0x18` names the
    /// body); else an entity it left (the Biotic Field).
    pub projectile: bool,
}

/// Where a listed body was as the searcher's player saw it, rewound to his
/// view (a search with `m_1C2A9945`, `vm/targets.rs`): its capsule and
/// facing then, in [`TargetCandidate`]'s terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeenBody {
    pub entity: u32,
    pub feet: [f64; 3],
    pub height: f64,
    pub radius: f64,
    pub yaw: f64,
}

/// The bodies a rewinding search on a site tests ([`World::seen_bodies`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeenBodies<'a> {
    /// The searcher has no player (a bot, a map placeable): the bodies as
    /// listed.
    Listed,
    /// Each listed body that existed at the searcher's player's view,
    /// where he saw it; a listed body missing here did not exist then.
    Rewound(&'a [SeenBody]),
}

/// One turn in the yaw and pitch units of a movement record.
const TURN_UNITS: f64 = 65_536.0;

impl TargetCandidate {
    /// Its yaw in a movement record's units (65536 a turn).
    pub fn yaw_units(&self) -> i32 {
        (self.yaw * TURN_UNITS / std::f64::consts::TAU).round() as i64 as i32
    }

    /// Where its model hardpoint `hardpoint` is in the world, with its feet
    /// at `feet` facing `yaw` (radians): the point turned by the yaw about
    /// the feet. None: no such hardpoint known.
    pub fn hardpoint_at(&self, hardpoint: u64, feet: [f64; 3], yaw: f64) -> Option<[f64; 3]> {
        let [x, y, z] = *self.hardpoints.as_deref()?.get(&hardpoint)?;
        let (sin, cos) = yaw.sin_cos();
        let [fx, fy, fz] = feet;
        Some([fx + x * cos + z * sin, fy + y, fz - x * sin + z * cos])
    }

    /// Its filter word as the target searches test it (`vm/targets.rs`
    /// module notes: team bit, body kinds, dead).
    pub fn filter_word(&self) -> u64 {
        targets::filter_word(self)
    }

    /// `STU_4049CF77` of this body or its controller: its AI's profile
    /// value of `identifier`; none without an AI profile or such a value.
    pub fn ai_parameter(&self, definitions: &AiDefinitions, identifier: u64) -> Option<f64> {
        self.ai_control?.parameter(definitions, identifier)
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

/// How `entity` relates, as far as the listed bodies tell: a listed body's
/// controller, which also possesses it now; a listed controller or player
/// is its own (it has component 44) and possesses its body; a spawned
/// entity a body lists ([`TargetCandidate::spawned`]) is owned by that
/// body's controller, and a projectile among them was shot by that body,
/// while a listed body, controller or spawned entity that isn't a
/// projectile has no shooter (no component 50).
/// Assumption: the owner (component 72) of an entity the game spawns for
/// a hero is his controller, as the Biotic Field's aura `0B3C` heals for
/// `STU_0AC15F78(STU_F76C486C(#40))`, the body the field's owner
/// possesses. Assumption: another body's current possessor is its
/// controller, as a player's world answers for its own body (Biotic
/// Grenade's enemy aura `0C72` rid21 asks `STU_F7262B36(#40)` of the
/// thrower's body on the body it hit; the same assumption for an AI
/// hero's body: the Training Partner console's search `265F` rid1 filters
/// on `STU_F7262B36(candidate).#23689`). None: the bodies don't tell.
pub fn listed_relation(
    bodies: &[TargetCandidate],
    relation: EntityRelation,
    entity: u32,
) -> Option<u32> {
    let by_controller = || {
        bodies
            .iter()
            .find(|body| body.controller == Some(entity) || body.player == Some(entity))
    };
    let owner_of = || {
        bodies.iter().find_map(|body| {
            body.spawned
                .iter()
                .find(|owned| owned.entity == entity)
                .map(|owned| (body, owned.projectile))
        })
    };
    match relation {
        EntityRelation::Controller => bodies
            .iter()
            .find(|body| body.entity == entity)
            .map_or_else(|| by_controller().map(|_| entity), |body| body.controller),
        EntityRelation::CurrentPossessor => {
            bodies.iter().find(|body| body.entity == entity)?.controller
        }
        EntityRelation::PossessedBody => by_controller().map(|body| body.entity),
        EntityRelation::Owner => owner_of()?.0.controller,
        EntityRelation::Shooter => match owner_of() {
            Some((body, true)) => Some(body.entity),
            Some((_, false)) => Some(0),
            None if bodies.iter().any(|body| body.entity == entity)
                || by_controller().is_some() =>
            {
                Some(0)
            }
            None => None,
        },
        _ => None,
    }
}

/// Where a ray meets the map's collision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MapHit {
    pub distance: f64,
    pub point: [f64; 3],
    pub normal: [f64; 3],
}

/// The map's collision as a script's ray casts meet it (`vm/rays.rs`),
/// and what each ray profile meets.
pub trait MapRays: fmt::Debug + Send + Sync {
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

    /// Whether a ray of ray profile `profile` (none: the default profile)
    /// meets the living bodies: only a profile whose rays test the bodies'
    /// hit shapes does (`m_0220680A` bit 0).
    fn meets_bodies(&self, profile: Option<u64>) -> bool;

    /// The map's collision categories a cast of ray profile `profile` (none:
    /// the default profile) meets.
    fn map_mask(&self, profile: Option<u64>) -> u16;
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

    /// An action class this world runs as a no-op because the server does
    /// its work itself, with the answer its assumption records; none runs
    /// the VM's own rule.
    fn no_op_action(&self, _class: &str) -> Option<&'static str> {
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

    /// The match's random table index `STU_CBF6FF14` reads
    /// ([`expr::Context::match_seed`]); None: unknown here.
    fn match_seed(&self) -> Option<i64> {
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

    /// The owner landed and left the ground again within its last move: it
    /// is airborne with its air ticks (mover state `+0x10`) at 0 on a fresh
    /// move record.
    fn movement_restarted(&self, _site: Site) -> bool {
        false
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

    /// Another body's health or its maximum, every kind together: a listed
    /// body's ([`TargetCandidate::health`]). None: unknown, or a pool kind.
    fn listed_health(
        &self,
        site: Site,
        entity: u32,
        kind: Option<PoolKind>,
        maximum: bool,
    ) -> Option<f64> {
        if kind.is_some() {
            return None;
        }
        let [current, most] = self
            .target_candidates(site)?
            .iter()
            .find(|body| body.entity == entity)?
            .health?;
        Some(if maximum { most } else { current })
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

    /// A temporary pool on the body of `site`, named `name` when given
    /// ([`crate::vitals::Vitals::add_temporary_pool`]); its id.
    fn add_temporary_pool(
        &mut self,
        _site: Site,
        _kind: PoolKind,
        _maximum: f64,
        _current: f64,
        _name: Option<u64>,
    ) -> Option<u32> {
        None
    }

    /// Takes up to `amount` from the pools named `name` of the body of
    /// `site`; what it took. None: this world has no such pools.
    fn drain_named_pool(&mut self, _site: Site, _name: u64, _amount: f64) -> Option<f64> {
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

    /// The body (`STUHero.m_gameplayEntity`) of `hero`; None: unknown.
    fn hero_body(&self, _site: Site, _hero: u64) -> Option<u64> {
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

    /// Every entity that has target tags (component 76), each once, in
    /// this world's order (a tag query with no tag, `STU_EB216A0B`); None:
    /// this world can't list them.
    fn tagged_entities(&self, _site: Site) -> Option<Vec<u32>> {
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

    /// Where the searcher on `site` saw the bodies of
    /// [`Self::target_candidates`], for a search that rewinds them
    /// (`TrackTargets` `m_1C2A9945`). None: this world can't tell, and the
    /// search tests them as listed (an assumption).
    fn seen_bodies(&self, _site: Site) -> Option<SeenBodies<'_>> {
        None
    }

    /// The map's collision a ray cast on this world meets
    /// ([`MapRays::cast`]); every world of a match gets the same map (the
    /// bodies', bots', placeables' and mode's scripts). None: this world
    /// has no map, and a cast is a gap.
    fn map_rays(&self) -> Option<&dyn MapRays> {
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

    /// Where `entity`'s model hardpoint `hardpoint` is (`STU_FC99C77E`):
    /// a listed body's hardpoint, turned by its yaw about its feet, where
    /// the searcher's player saw it when the world rewinds
    /// ([`Self::seen_bodies`]), else where it stands. None: unknown.
    fn hardpoint(&self, site: Site, entity: u32, hardpoint: u64) -> Option<[f64; 3]> {
        let body = self
            .target_candidates(site)?
            .iter()
            .find(|body| body.entity == entity)?;
        let (feet, yaw) = match self.seen_bodies(site) {
            Some(SeenBodies::Rewound(seen)) => seen
                .iter()
                .find(|seen| seen.entity == entity)
                .map_or((body.feet, body.yaw), |seen| (seen.feet, seen.yaw)),
            _ => (body.feet, body.yaw),
        };
        body.hardpoint_at(hardpoint, feet, yaw)
    }

    /// A body's yaw and pitch, as [`expr::Context::entity_facing`].
    fn entity_facing(&self, _site: Site, _entity: u32) -> Option<[i32; 2]> {
        None
    }

    /// Where a body's eye is (its feet and eye height) and its yaw and
    /// pitch: the view a Grappling Hook starts from (`vm/grapple.rs`).
    fn eye(&self, _site: Site, _entity: u32) -> Option<([f64; 3], [i32; 2])> {
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

    /// The radius of `entity`'s mover capsule in its stance now
    /// (`STU_F7FD94F9`). None: unknown here.
    fn body_radius(&self, _site: Site, _entity: u32) -> Option<f64> {
        None
    }

    /// The rate (radians a second, movement state `+0xD0`) of the turn
    /// `entity`'s mover runs for a `STU_C168636D` state
    /// ([`orpheus_physics::mover::turn`]) after its last move; None: no turn
    /// runs, or unknown here.
    fn turn_rate(&self, _site: Site, _entity: u32) -> Option<f32> {
        None
    }

    /// How far the walls turn a move of `entity`'s body along the flat
    /// unit `direction` for `distance`, as the cosine (1: free, 0: stopped)
    /// a wall check's trial in `form` finds (`STU_FBFF5DE7`,
    /// [`orpheus_physics::mover::trial`]). None: unknown here.
    fn trial_turn(
        &mut self,
        _site: Site,
        _entity: u32,
        _direction: [f64; 3],
        _distance: f64,
        _form: orpheus_physics::mover::trial::TrialForm,
    ) -> Option<f64> {
        None
    }

    /// Where a placement selector's landing search from `entity`'s eye
    /// lands, its casts meeting the map as ray profile `profile` does
    /// (`STU_4C96470A`, [`orpheus_physics::mover::landing`]): Some(None)
    /// when it finds no ground. None: unknown here.
    fn landing_point(
        &mut self,
        _site: Site,
        _entity: u32,
        _search: &orpheus_physics::mover::landing::LandingSearch,
        _profile: Option<u64>,
    ) -> Option<Option<[f64; 3]>> {
        None
    }

    /// Whether the ground walk of a shockwave on `site` reaches a body, its
    /// first and last casts meeting the map as ray profile `sphere_profile`
    /// does and its sample rays as `ray_profile` does
    /// ([`orpheus_physics::mover::ground_walk`]). None: unknown here.
    fn ground_walk(
        &self,
        _site: Site,
        _walk: &orpheus_physics::mover::ground_walk::GroundWalk,
        _sphere_profile: Option<u64>,
        _ray_profile: Option<u64>,
    ) -> Option<bool> {
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

    /// Remove every map placeable whose identifier 2 is `identifier`
    /// (`STU_A3DC6A25`, `vm/entities.rs`). False: this world removes none.
    fn remove_placeables(&mut self, _site: Site, _identifier: u64) -> bool {
        false
    }

    /// Create a dummy bot (`STU_FC3921B1`): a hero body no player plays;
    /// the entity it gets. None: this world creates none.
    fn create_dummy_bot(&mut self, _site: Site, _dummy: DummyBot) -> Option<u32> {
        None
    }

    /// Remove the dummy bot in a team's slot (`STU_C1561031`). False: this
    /// world removes none.
    fn remove_dummy_bot(&mut self, _site: Site, _team: i64, _slot: i64) -> bool {
        false
    }

    /// Leave the match's waiting phase (`STU_5E019216`, the Workshop's
    /// Start Game Mode). False: this world can't.
    fn start_game_mode(&mut self, _site: Site) -> bool {
        false
    }

    fn body_on_fire(&mut self, _site: Site, _entity: u32) -> Option<bool> {
        None
    }

    /// None means this relationship is unsupported for the supplied entity.
    /// By default what the listed bodies tell ([`listed_relation`]).
    fn related_entity(&mut self, site: Site, relation: EntityRelation, entity: u32) -> Option<u32> {
        listed_relation(self.target_candidates(site)?, relation, entity)
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

    /// An AI parameter (`STU_4049CF77`, [`expr::Context::ai_parameter`]);
    /// None: unknown here.
    fn ai_parameter(&mut self, _site: Site, _entity: u32, _identifier: u64) -> Option<Option<f64>> {
        None
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

    /// A game message the script sent its own entity, which the VM's own
    /// entries took ([`Machine::queue_entries`]); a world that stands in
    /// for other receivers on that entity (the match flow in place of the
    /// mode graph) may look at it.
    fn observe_own_message(&mut self, _source: Site, _message: u64, _parameters: &Variables) {}

    /// Deliver a game message to every entity of the world but the sender
    /// (a `STU_38EE1100` target `STU_1D719351` with `m_6A8DB059` set; the
    /// sender's own instances get it from the VM). False: this world can't
    /// deliver it.
    fn broadcast_message(&mut self, _source: Site, _message: u64, _parameters: &Variables) -> bool {
        false
    }

    /// A SendGameMessage state to every entity began: the client keeps
    /// such a sender in its live-sender registry while the state runs, and
    /// receivers look it up each frame, so an entity that starts later
    /// finds it too (`sub_7FF789CAA770`, GameMessages.md). The state's end
    /// comes as [`World::end_message`] to [`crate::mode::EVERY_ENTITY`].
    /// By default a world delivers it once ([`World::broadcast_message`]).
    fn hold_broadcast(&mut self, source: Site, message: u64, parameters: &Variables) -> bool {
        self.broadcast_message(source, message, parameters)
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

    /// A TrackStat action ran or a TrackStatTime state ended
    /// ([`TrackedStat`]): the match adds the amount to the stats of each
    /// target's player. False: this world keeps no stats.
    fn track_stat(&mut self, _site: Site, _stat: TrackedStat) -> bool {
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

    /// A projectile's speed scale state (`STU_D3F71FCF`) began with its
    /// scale (Some) or ended (None, back to 1). False: this entity is no
    /// projectile the world flies.
    fn set_projectile_speed(&mut self, _site: Site, _scale: Option<f64>) -> bool {
        false
    }

    /// A `CombatModFilter` state began with its modifier (Some) or ended
    /// (None). False: this world keeps none.
    fn combat_mod(&mut self, _site: Site, _modifier: Option<CombatMod>) -> bool {
        false
    }

    /// An assist state (`STU_0DB9E66B`, `vm/statuses.rs`) began with its
    /// credit (Some) or ended (None), at the script's `now_ms`. False: this
    /// world keeps none.
    fn assist(&mut self, _site: Site, _link: Option<AssistLink>, _now_ms: u64) -> bool {
        false
    }

    /// An ApplyAura state (`STU_431F3FCC`, `vm/auras.rs`) put an aura on a
    /// body or took it off: the match hands it to the target's component
    /// ([`Component::change_aura`]). False: this world applies none, and
    /// the state has no effect.
    fn change_aura(&mut self, _site: Site, _change: AuraChange) -> bool {
        false
    }

    /// A scope state (`STU_550D0C5C`, `vm/scopes.rs`) opened a scope
    /// instance on a target or closed it: the match hands it to the
    /// target's component ([`Component::change_scope`]). False: this world
    /// opens none, and the state has no effect.
    fn change_scope(&mut self, _site: Site, _change: ScopeChange) -> bool {
        false
    }

    /// A scope instance set a variable of its scope state's instance on
    /// another entity (`STU_D49BBD03`): the match sets it there
    /// ([`Component::set_instance_variable`]). False: this world sets
    /// none.
    fn set_parent_variable(&mut self, _site: Site, _write: ParentWrite) -> bool {
        false
    }

    /// A graph set another entity's variable (`STU_0BDBC7B2`: a Workshop
    /// action's write of a player's variable, `vm/workshop.rs`): the world
    /// sets it in that entity's script. False: this world sets none.
    fn set_entity_variable(&mut self, _site: Site, _entity: u32, _id: u32, _value: Value) -> bool {
        false
    }

    /// A graph put a body somewhere (`STU_CFCE55DC`: the Workshop's
    /// Teleport on its target's body): the match moves it and corrects its
    /// owner's client. False: this world moves none.
    fn move_body(&mut self, _site: Site, _move: BodyMove) -> bool {
        false
    }

    /// A graph named who an environment kill of a body credits
    /// (`STU_545AB3B8`, Set Environment Credit Player): the match keeps it
    /// as the body's knock credit. False: this world credits none.
    fn credit_environment(&mut self, _site: Site, _credit: EnvironmentCredit) -> bool {
        false
    }

    /// A graph set the cooldown of an ability of a body (`STU_FFF26F80`:
    /// the Workshop's Set Ability Cooldown `2735` on its player's
    /// possessed body): the match writes it in that body's script
    /// ([`Component::set_ability_cooldown`]). False: this world sets none.
    fn set_ability_cooldown(&mut self, _site: Site, _cooldown: CooldownWrite) -> bool {
        false
    }

    /// A graph revived a dead body (`STU_621AF478`: the mode body script
    /// `0C8E` on the Workshop's Resurrect): the match brings it back where
    /// it lies. False: this world revives none.
    fn revive(&mut self, _site: Site, _body: u32) -> bool {
        false
    }

    /// A `PlayScript` action played its graph on another entity (the
    /// Workshop's Chase Variable `1CCC` plays `1CCF` on each target
    /// player): the match plays it in that entity's script
    /// ([`Component::play_script`]). False: this world plays none there.
    fn play_script(&mut self, _site: Site, _request: PlayRequest) -> bool {
        false
    }

    /// `STU_AFBB59E9` (server-only) reported a knockback: the attacker's
    /// push of the victim ([`Knockback`]), for the match's own knockback
    /// credit (the Workshop's Player Dealt / Received Knockback). False:
    /// this world keeps none.
    fn knockback(&mut self, _site: Site, _knockback: Knockback) -> bool {
        false
    }

    /// `STUStatescriptActionCreateEntity` or `StateCreateEntity` made an
    /// entity (`vm/entities.rs`): the world gives it an id, starts its
    /// script and tells the clients. None: this world makes none.
    fn create_entity(&mut self, _site: Site, _creation: EntityCreation) -> Option<u32> {
        None
    }

    /// A pickup state (`STU_913C09E4`, `vm/pickups.rs`) at `site` began
    /// (Some) or ended (None). False: this world keeps none.
    fn set_pickup(&mut self, _site: Site, _pickup: Option<Pickup>) -> bool {
        false
    }

    /// The pickup of `kind` the body `entity` touches now, its entity
    /// ([`Pickup::entity`]); Some(None): none. None: this world knows no
    /// pickups.
    fn touched_pickup(&mut self, _site: Site, _entity: u32, _kind: i64) -> Option<Option<u32>> {
        None
    }
}

/// A graph played on another entity ([`World::play_script`]): its target,
/// the graph, the start entries' parameters and the action's policy for an
/// instance already running (`m_AA4DE426`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayRequest {
    pub target: u32,
    pub graph: u32,
    pub parameters: Vec<(u64, Value)>,
    pub policy: i64,
    /// The entity that played it (the start entries' caller).
    pub caller: u32,
    /// The PlayScript action that played it: its entity, instance, graph
    /// and node (a hero ability's graph names the Workshop's Event
    /// Ability of what the played graph does,
    /// [`GraphStore::ability_button`]).
    pub site: Site,
}

/// A knockback a graph reported (`STU_AFBB59E9`, [`World::knockback`]):
/// Rocket Punch's victim graph `0D07` rid63 reports the puncher (`#40`)
/// pushing its own body along the punch (`#2083`) with Rocket Punch's
/// loadout `0B9:107`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knockback {
    /// `m_C285CBB2`.
    pub attacker: u32,
    /// `m_0F99C09F`.
    pub victim: u32,
    /// `m_direction`; zero when unset.
    pub direction: [f64; 3],
    /// `m_loadout`, the ability.
    pub loadout: Option<u64>,
}

/// An entity a graph made ([`World::create_entity`]).
#[derive(Clone, Debug, PartialEq)]
pub struct EntityCreation {
    /// Its entity definition (`003`).
    pub definition: u64,
    /// Where the action put it (`m_position`); none: where it didn't say
    /// (all the Workshop's Create Effect and Create Icon).
    pub position: Option<[f64; 3]>,
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

/// An amount a graph added to stats ([`World::track_stat`]): TrackStat
/// (`STU_CEBDAF17`) as it runs, TrackStatTime (`STU_441B50E4`) as it
/// ends. Both are server-only (no client code; the client's action is the
/// base action's run): the retail server's stat table took them.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackedStat {
    /// `m_stat` then `m_stats` (`062` stats), each in order. The stats in
    /// `m_D32CB484` (12 TrackStat nodes; every one a "Best in Game"
    /// accuracy, the ratio of two counted stats) are not among them: the
    /// server derives those from the stats counted; nothing adds to them.
    pub stats: Vec<u64>,
    /// `m_amount`; for TrackStatTime the seconds the state ran.
    pub amount: f64,
    /// `m_targets`, each once: whose stats (the owner itself unevaluated;
    /// a body, a controller or a player entity).
    pub targets: Vec<u32>,
}

/// A dummy bot a graph asks for (`STU_FC3921B1`, server-only: no client
/// code; its fields are the Workshop's "Create Dummy Bot" parameters one
/// for one, `refs/reports/TrainingPartner.md`). The Practice Range's
/// Training Partner console `265F` makes one so.
#[derive(Clone, Debug, PartialEq)]
pub struct DummyBot {
    /// `m_heroGUID`: the hero (`075`).
    pub hero: u64,
    /// `m_67465E75`: the team index (0 blue, 1 red).
    pub team: i64,
    /// `m_slot`: its slot in the team.
    pub slot: i64,
    /// `m_06B5A4F1`: where it stands.
    pub position: [f64; 3],
    /// `m_9211FBC7`: where it faces.
    pub facing: [f64; 3],
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

/// A running mover behaviour state's request for the mover
/// (`orpheus_physics::mover`: a `MoveTo`'s move or a `STU_1F5ED928` path
/// walk): the state, its activation (a state begun again is a new
/// behaviour) and the block it handed over at its begin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveRequest {
    pub instance: u16,
    pub raw_state: u16,
    pub activation: u16,
    pub params: Behaviour,
}

/// An AI movement state's (`STU_ECAEE401`) point to walk to, from its move
/// goal (`STU_194D744E`), for the server's walker
/// ([`Component::ai_move_requests`]). The walker reports the walk's end with
/// [`Component::ai_move_arrived`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AiMoveRequest {
    pub instance: u16,
    pub raw_state: u16,
    pub target: [f64; 3],
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

/// Where a graph puts a body ([`World::move_body`]): its place and, when
/// the graph says it, the direction it faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyMove {
    pub body: u32,
    pub position: [f64; 3],
    pub facing: Option<[f64; 3]>,
}

/// Who an environment kill of `body` credits ([`World::credit_environment`]):
/// the entity `by` (none: nobody).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnvironmentCredit {
    pub body: u32,
    pub by: Option<u32>,
}

/// A cooldown another entity's graph sets ([`World::set_ability_cooldown`]):
/// the ability of `body` on logical button `button` gets `value`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CooldownWrite {
    pub body: u32,
    pub button: i64,
    pub value: f64,
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
    /// A wall check (`STU_FBFF5DE7`, `vm/walls.rs`): a trial move of the
    /// body each tick, its output true when a wall turns it.
    WallCheck,
    /// A placement selector's landing search (`STU_4C96470A`,
    /// `vm/landing.rs`): it searches each tick while its gate holds.
    Landing,
    /// The view limits around a facing (`STU_9935FBC8`,
    /// `vm/view_limits.rs`): the f32 bits of its yaw, pitch, least yaw,
    /// least pitch, greatest yaw and greatest pitch, in radians.
    ViewLimits {
        radians: [u32; 6],
    },
    /// A shockwave (`STUStatescriptStateShockwave`, `vm/shockwave.rs`):
    /// each update its wave grows and sends its hits; it finishes once the
    /// wave reaches its radius.
    Shockwave,
    /// A health change (`vm/health.rs`): a rate keeps when it last applied
    /// its share.
    ModifyHealth {
        last_ms: u64,
    },
    /// A status on the body while active (`STU_9028E035`, `vm/statuses.rs`).
    Status,
    /// A projectile's speed scale while active (`STU_D3F71FCF`,
    /// `vm/statuses.rs`).
    ProjectileSpeed,
    /// A combat modifier on the body while active (`CombatModFilter`).
    CombatMod,
    /// An assist credit while active, and for its linger after
    /// (`STU_0DB9E66B`, server-only; `vm/statuses.rs` [`AssistLink`]).
    Assist,
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
    /// A `MoveTo`: the move it handed the mover is in
    /// [`Component::move_requests`] while it runs.
    MoveTo,
    /// Widowmaker's Grappling Hook cast (`STU_4BBC5D09`, `vm/grapple.rs`):
    /// the hook flies each tick until it hits or reaches its range.
    GrappleCast,
    /// A mover behaviour that walks a path through the air (`STU_1F5ED928`,
    /// kind 10, `vm/grapple.rs`): its walk is in
    /// [`Component::move_requests`] while it runs.
    NavPath,
    /// Reinhardt's Charge's turn (`STU_C168636D`, kind 7, `vm/turn.rs`): its
    /// block is in [`Component::move_requests`] while it runs, and each
    /// tick it writes the mover's rate to its output.
    Turn,
    /// An AI movement state (`STU_ECAEE401`): the point its move goal names
    /// for the server's walker, in [`Component::ai_move_requests`].
    AiMove,
    /// Recall's rewind (`STU_41B3FC2B`): finishes after its duration; its
    /// begin and end are [`Motion`]s for the mover. Its frames carry when it
    /// ends (ms) and the frames of history it rewinds along.
    Rewind {
        end_ms: u64,
        frames: u16,
    },
    /// TrackStatTime (`STU_441B50E4`, server-only): the seconds it ran go
    /// to its stats when it ends ([`World::track_stat`]).
    StatTime,
    /// A per-target scope (`STU_550D0C5C`, `vm/scopes.rs`): its targets'
    /// scope instances are [`State::scopes`].
    Scope,
    /// `StateCreateEntity` (`vm/entities.rs`): the entity it made while
    /// active, which goes as it ends.
    CreateEntity {
        entity: Option<u32>,
    },
    /// A pickup while active (`STU_913C09E4`, server-only; `vm/pickups.rs`).
    Pickup,
    /// A body's search for the pickup it touches (`STU_87621906`,
    /// server-only; `vm/pickups.rs`): what it last wrote.
    PickupSearch {
        found: Option<u32>,
    },
}

impl StateKind {
    /// Whether the state keeps a value it evaluated up to date, so a change
    /// of what it read re-evaluates it ([`Vm::changed`]) rather than only
    /// re-checking its transitions.
    fn updates_on_change(self) -> bool {
        matches!(
            self,
            Self::Switch { .. }
                | Self::BooleanSwitch(_)
                | Self::DelayedSwitch { .. }
                | Self::Stack { .. }
                | Self::Watch
                | Self::FilterArray
                | Self::HeroSelect
                | Self::ViewLimits { .. }
        )
    }

    /// An ability state whose ability runs: started by its button and not
    /// stopped yet (an ability state is on for as long as the hero can use
    /// it).
    pub fn runs_ability(self) -> bool {
        matches!(self, Self::Ability { flags } if flags & (ABILITY_RUNNING | ABILITY_PUSHED) != 0)
    }
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
    /// A scope state's targets and their instances (`vm/scopes.rs`).
    pub scopes: Vec<ScopeHit>,
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
            scopes: Vec::new(),
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
    /// The value as of `last_ms`. A timed chase keeps the value it began
    /// with until it arrives, as the client's does.
    pub current: Value,
    pub reached: bool,
    pub remaining_ms: Option<i64>,
    pub last_ms: u64,
    /// The destination and speed of a chase toward a constant.
    pub timed: Option<TimedChase>,
}

/// A chase toward a constant, which the client doesn't step each frame:
/// `sub_7FF78AAB0400` marks it (`+624`), turns its update off and posts one
/// timer at its begin, its duration or `R(distance / speed * 1000)` ms on;
/// the timer (event 0, `0x7FF78AAA7F50` → `0x7FF78AAB0960`) puts it at its
/// destination and finishes it. Its value in between is worked out when read
/// (`0x7FF78AAAEC80`), and its block (the payload) keeps the value and time
/// it began with. A payload that moved each frame (Python's per-tick chase)
/// made the owner's client read the state every frame, and its own arrival
/// never fired, so Ana's, Roadhog's and Zenyatta's re-fire after a cooldown
/// chase (`0B94` st13 on `#173`) was never predicted (OwnerPrediction.md,
/// "Day (2026-11-17)").
#[derive(Clone, Debug, PartialEq)]
pub struct TimedChase {
    pub destination: Value,
    pub rate: f64,
}

impl ChasePayload {
    /// The chased value at `now_ms` (`0x7FF78AAAEC80` for a timed chase: a
    /// share of its duration, or its speed's step from where it began).
    pub(crate) fn value_at(&self, now_ms: u64) -> Value {
        let Some(timed) = self.timed.as_ref().filter(|_| !self.reached) else {
            return self.current.clone();
        };
        let elapsed = now_ms.saturating_sub(self.last_ms);
        if let Some(remaining) = self.remaining_ms {
            if remaining <= elapsed as i64 {
                return timed.destination.clone();
            }
            let share = f64::from((elapsed as f64 / remaining as f64) as f32);
            return chase_lerp(self.current.clone(), timed.destination.clone(), share);
        }
        let seconds = (f64::from(elapsed as f32) * 0.001) as f32;
        let step = f64::from(seconds * timed.rate as f32);
        if chase_distance(&self.current, &timed.destination) > step + expr::EPSILON {
            chase_step(self.current.clone(), timed.destination.clone(), step).0
        } else {
            timed.destination.clone()
        }
    }

    /// How long after `last_ms` a timed chase arrives (`0x7FF78AAB0400`:
    /// its duration, else `(int)((distance / speed) * 1000 + 0.5)` in f32).
    fn arrival_ms(&self) -> Option<u64> {
        let timed = self.timed.as_ref()?;
        if let Some(remaining) = self.remaining_ms {
            return Some(remaining.max(0) as u64);
        }
        let distance = chase_distance(&self.current, &timed.destination) as f32;
        let ms = (distance / timed.rate as f32) * 1000.0 + 0.5;
        Some((ms as i32).max(0) as u64)
    }
}

/// The instance variable the engine sets to the entity an instance was
/// created with, its creator (the client's instance creation
/// `sub_7FF789887F20`/`sub_7FF78988AA20` → setter `sub_7FF78AAA4B10`;
/// HealthAuras.md, "Client tools pass 2 (2026-11-16)"). A creation without
/// an entity sets the host's own (`ScriptHost_GetEntity` at
/// `0x7FF78988816C`), so it is always an entity. A network-made instance
/// takes it from its descriptor (`wire::write_descriptor`; InstanceCreator.md).
pub(crate) const CREATOR: u32 = 27;

/// Parameter `0009.026`: a delivered message's sender (the dispatch's
/// parameter source `sub_7FF78AAA32F0`, the message's `+152`), which
/// message filters test (`0C90` st4/st8 `param(9) != owner()`).
const MESSAGE_SENDER: u32 = 9;

#[derive(Clone, Debug)]
pub struct Instance {
    pub graph: u32,
    /// Parent instance and raw state index, as carried on the wire.
    pub parent: Option<(u16, u16)>,
    /// The entity it was created with ([`CREATOR`]).
    pub source: u32,
    /// When the instance was created.
    pub start_ms: u64,
    pub variables: Variables,
    pub states: IndexMap<u16, State>,
    /// A scope instance's link to the scope state that opened it on
    /// another entity (`vm/scopes.rs`).
    pub scope: Option<ScopeLink>,
    /// Moved on by every mutable borrow through [`Instances`].
    revision: u64,
}

impl Instance {
    /// Its content's revision in its snapshot: equal for two borrows
    /// between which nothing could change it. A projection of it holds
    /// while this and the snapshot's outputs for it (timers, links) stay
    /// ([`Instances::touch`]).
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// A snapshot's instances by id. Each mutable borrow of an instance moves
/// that instance's revision on ([`Instance::revision`]), so no write can
/// leave it behind; reads go through the map ([`std::ops::Deref`]).
#[derive(Debug, Default)]
pub struct Instances {
    map: IndexMap<u16, Instance>,
    /// The last revision given out.
    writes: u64,
}

impl Instances {
    fn next_revision(&mut self) -> u64 {
        self.writes += 1;
        self.writes
    }

    pub fn get_mut(&mut self, id: &u16) -> Option<&mut Instance> {
        let revision = self.next_revision();
        let instance = self.map.get_mut(id)?;
        instance.revision = revision;
        Some(instance)
    }

    pub fn insert(&mut self, id: u16, mut instance: Instance) -> Option<Instance> {
        instance.revision = self.next_revision();
        self.map.insert(id, instance)
    }

    pub fn shift_remove(&mut self, id: &u16) -> Option<Instance> {
        self.map.shift_remove(id)
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Move an instance's revision on for a change kept outside it (its
    /// pending timers, its variables' links).
    pub(crate) fn touch(&mut self, id: u16) {
        self.get_mut(&id);
    }
}

impl std::ops::Deref for Instances {
    type Target = IndexMap<u16, Instance>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl std::ops::Index<&u16> for Instances {
    type Output = Instance;

    fn index(&self, id: &u16) -> &Instance {
        &self.map[id]
    }
}

impl std::ops::IndexMut<&u16> for Instances {
    fn index_mut(&mut self, id: &u16) -> &mut Instance {
        self.get_mut(id).expect("IndexMap: key not found")
    }
}

impl<'a> IntoIterator for &'a Instances {
    type Item = (&'a u16, &'a Instance);
    type IntoIter = indexmap::map::Iter<'a, u16, Instance>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
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
    pub instances: Instances,
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
    /// Which content this is ([`Snapshot::revision`]).
    revision: Revision,
}

/// What a snapshot holds apart from its clock (`now_ms`): two snapshots
/// with the same revision hold the same values, in the same order. Every
/// write the VM makes to its snapshot moves it on, whether or not the
/// write changed anything ([`TrackedSnapshot`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Revision {
    /// Unique to the component.
    component: u64,
    writes: u64,
}

impl Revision {
    /// A new component's first revision.
    fn first() -> Self {
        static COMPONENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        Self {
            component: COMPONENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            writes: 0,
        }
    }

    /// Whether both are revisions of one component's snapshot, whose
    /// instances' revisions ([`Instance::revision`]) count on from each
    /// other.
    pub fn same_component(self, other: Self) -> bool {
        self.component == other.component
    }
}

/// A machine's snapshot: a mutable borrow moves its revision on, so no
/// write can leave it behind.
struct TrackedSnapshot(Snapshot);

impl TrackedSnapshot {
    fn new(snapshot: Snapshot) -> Self {
        Self(snapshot)
    }

    /// The clock, which the revision leaves out.
    fn set_now_ms(&mut self, now_ms: u64) {
        self.0.now_ms = now_ms;
    }
}

impl std::ops::Deref for TrackedSnapshot {
    type Target = Snapshot;

    fn deref(&self) -> &Snapshot {
        &self.0
    }
}

impl std::ops::DerefMut for TrackedSnapshot {
    fn deref_mut(&mut self) -> &mut Snapshot {
        self.0.revision.writes += 1;
        &mut self.0
    }
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

    /// Its content's revision: equal for two snapshots that hold the same
    /// values (their clocks aside), so what was worked out from one holds
    /// for the other.
    pub fn revision(&self) -> Revision {
        self.revision
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

/// The world as the VM's generic code takes it. That code is instantiated
/// once per world type, and the unit tests' dozens of worlds would each make
/// a copy: more sections than one COFF object holds (link.exe LNK1136 or
/// LNK1106; docs/dev/testing.md "Build speed"). A test build hands every world
/// to the VM as `dyn World`, so they share one copy; other builds keep each
/// world's own.
#[cfg(test)]
fn erase(world: &mut impl World) -> &mut dyn World {
    world
}

#[cfg(not(test))]
fn erase<W: World>(world: &mut W) -> &mut W {
    world
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
        let world = erase(world);
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
        (!self.halted).then_some(&*self.machine.snapshot)
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
        let world = erase(world);
        self.evaluate_in_state(instance, raw_state, None, world, |node, context| {
            expr::evaluate_in(config, node.configs(), context)
        })
    }

    /// Whether `candidate`, a health change's other party, passes the
    /// party filter of the combat modifier `filter` names (`m_6AA5D70F`):
    /// its masks and tags, then its condition evaluated with the candidate
    /// as every parameter, as the client's query 0x7FF789C375A0 binds it
    /// while it tests the filter (`vm/statuses.rs`). None: the state is
    /// gone or the filter can't be answered.
    pub fn passes_party_filter<W: World>(
        &self,
        filter: PartyFilter,
        candidate: u32,
        world: &mut W,
    ) -> Option<bool> {
        let world = erase(world);
        let bound = Value::Entity(candidate);
        self.evaluate_in_state(
            filter.instance,
            filter.state,
            Some(&bound),
            world,
            |node, context: &mut InstanceContext<'_, _>| {
                let filter = field(&node.fields, statuses::PARTY_FILTER);
                expr::passes_filter_as_candidate(filter, candidate, context)
            },
        )
        .ok()
    }

    /// The party filters of the modifiers of `kind` that `effects` (in
    /// `world`, which runs them) holds for its body that `other` passes
    /// ([`Self::passes_party_filter`]; one the server can't answer
    /// doesn't pass), for [`BodyEffects::scale`].
    pub fn passed_party_filters<W: World>(
        &self,
        world: &mut W,
        effects: impl Fn(&W) -> &BodyEffects,
        kind: u64,
        other: u32,
    ) -> Vec<PartyFilter> {
        let filters: Vec<PartyFilter> = effects(world).party_filters(kind).collect();
        filters
            .into_iter()
            .filter(|&filter| self.passes_party_filter(filter, other, world) == Some(true))
            .collect()
    }

    /// `evaluate` in a context of the state `raw_state` of `instance`, as a
    /// read outside the VM's run (nothing is recorded); `candidate` answers
    /// every parameter when set.
    fn evaluate_in_state<W: World + ?Sized, T>(
        &self,
        instance: u16,
        raw_state: u16,
        candidate: Option<&Value>,
        world: &mut W,
        evaluate: impl FnOnce(&Node, &mut InstanceContext<'_, W>) -> Result<T, EvalError>,
    ) -> Result<T, EvalError> {
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
            graphs: &self.graphs,
            world,
            site: Site {
                entity: snapshot.entity,
                instance,
                graph,
                node: Some(node.pos),
            },
            reads: Vec::new(),
            foreign_reads: Vec::new(),
            parameters: None,
            candidate: candidate.cloned(),
            sender: None,
            watcher: None,
            read_native: false,
            read_volatile: false,
            natives: Vec::new(),
            // A read outside the VM's run: nothing is recorded.
            assumed: Vec::new(),
        };
        evaluate(node, &mut context)
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
        let world = erase(world);
        self.run(world, |vm| {
            let key = VarKey::Entity(id);
            let old = vm.machine.value(key, None);
            vm.set_base(key, value);
            vm.notify_value(key, &old, None)
        })
    }

    /// Another entity's graph set the cooldown of this entity's ability on
    /// logical button `button` (the Workshop's Set Ability Cooldown,
    /// `STU_FFF26F80`): see `vm/abilities.rs` `set_ability_cooldown`.
    /// Nothing when no running ability has that button.
    pub fn set_ability_cooldown(
        &mut self,
        button: i64,
        value: f64,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        let world = erase(world);
        self.run(world, |vm| vm.set_ability_cooldown(button, value).map(drop))
    }

    /// The cooldown left on this entity's ability on logical button
    /// `button`, as the Workshop's Ability Cooldown reads it: the value of
    /// the `m_out_CooldownVar` of the first running Ability whose start or
    /// hold button it is (the lookup of `STU_FFF26F80`, 0x7FF789C9EED0;
    /// assumption: the value reads the same ability). The buttons are read
    /// with no world (every hero's are constants). None: no ability has
    /// that button.
    pub fn ability_cooldown(&self, button: i64) -> Option<f64> {
        let mut empty = EmptyWorld;
        let world = erase(&mut empty);
        let snapshot = self.snapshot()?;
        let abilities: Vec<(u16, u16)> = snapshot
            .instances
            .iter()
            .flat_map(|(&instance, item)| {
                item.states
                    .iter()
                    .filter(|(_, state)| {
                        state.active && matches!(state.kind, StateKind::Ability { .. })
                    })
                    .map(move |(&raw, _)| (instance, raw))
            })
            .collect();
        abilities.into_iter().find_map(|(instance, raw)| {
            let target = self
                .evaluate_in_state(instance, raw, None, world, |node, context| {
                    for name in ["m_A5F1A77B", "m_A368407D"] {
                        let value =
                            expr::evaluate_in(field(&node.fields, name), node.configs(), context)?;
                        if button != 0 && expr::to_int(&value, 0) == button {
                            return expr::lvalue(field(&node.fields, "m_out_CooldownVar"));
                        }
                    }
                    Ok(None)
                })
                .ok()
                .flatten()?;
            Some(expr::to_float(
                &self.machine.value(source_key(instance, target), None),
            ))
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
        let world = erase(world);
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

    /// The variables of `entity` that the active states' expressions read,
    /// which [`Self::foreign_variable_changed`] re-evaluates.
    pub fn foreign_reads(&self, entity: u32) -> impl Iterator<Item = u32> + '_ {
        self.machine
            .foreign_watchers
            .range((entity, 0)..=(entity, u32::MAX))
            .map(|(&(_, id), _)| id)
    }

    /// Another entity's variable `id` changed in the world (as
    /// [`World::entity_variable`] answers it): the active states whose
    /// expressions read it re-evaluate, as for the entity's own variables.
    pub fn foreign_variable_changed(
        &mut self,
        entity: u32,
        id: u32,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        let world = erase(world);
        self.run(world, |vm| {
            let watchers = vm
                .machine
                .foreign_watchers
                .get(&(entity, id))
                .cloned()
                .unwrap_or_default();
            for watcher in watchers {
                let Some(state) = vm
                    .state_mut(watcher)
                    .filter(|state| state.active && !state.ending && !state.pending_change)
                else {
                    continue;
                };
                state.pending_change = true;
                vm.post(watcher.instance, Event::Changed(watcher.raw))?;
            }
            Ok(())
        })
    }

    /// What the world answers for the native configs `classes` changed:
    /// each active state whose expressions read one of them takes it as
    /// the client takes a change of a value it depends on
    /// (`sub_7FF78AA9ED40`, ModeGraphGaps.md "Waiting state"): a state that
    /// keeps a value up to date (a switch, a stack, a watch, a filtered
    /// list) re-evaluates it, any other gets a completion check, so its
    /// transitions are tried again (event kind 2, `sub_7FF78AA81220`).
    pub fn natives_changed(
        &mut self,
        classes: &[&str],
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        let world = erase(world);
        self.run(world, |vm| {
            let mut watchers: Vec<StateKey> = Vec::new();
            for class in classes {
                for &watcher in vm.machine.native_watchers.get(*class).into_iter().flatten() {
                    if !watchers.contains(&watcher) {
                        watchers.push(watcher);
                    }
                }
            }
            for watcher in watchers {
                let Some(state) = vm
                    .state_mut(watcher)
                    .filter(|state| state.active && !state.ending)
                else {
                    continue;
                };
                if state.kind.updates_on_change() {
                    if !state.pending_change {
                        state.pending_change = true;
                        vm.post(watcher.instance, Event::Changed(watcher.raw))?;
                    }
                } else if !state.pending_check {
                    state.pending_check = true;
                    vm.post(watcher.instance, Event::Check(watcher.raw))?;
                }
            }
            Ok(())
        })
    }

    /// Authoritative modifiers require a fully supported runtime path.
    pub fn movement_modifiers(
        &mut self,
        world: &mut impl World,
    ) -> Result<Option<BTreeMap<ModifierKey, Modifier>>, InitFault> {
        let world = erase(world);
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

    /// Queue the authored entries of one instance at the current clock,
    /// for a message with no sender.
    pub fn queue_message(
        &mut self,
        instance: u16,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        self.queue_message_from(instance, 0, message, parameters)
    }

    /// Queue the authored entries of one instance at the current clock,
    /// for a message `sender` sent (its entries' filters test it:
    /// [`Vm::message_filter_admits`]).
    pub fn queue_message_from(
        &mut self,
        instance: u16,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        self.machine
            .queue_entries(&self.graphs, Some(instance), sender, message, parameters)
    }

    pub fn run_now(&mut self, world: &mut impl World) -> Result<(), InitFault> {
        self.run(erase(world), |vm| vm.run())
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
        let world = erase(world);
        self.run(world, |vm| {
            vm.release_message(sender, message)?;
            vm.run()
        })
    }

    /// A new root instance of `graph` with the next free id and its own
    /// `variables` (a spawner's overrides). Its entries queue at the current
    /// clock and run at the next update (`Component.create` in
    /// `ow174/game/script/runtime.py`).
    pub fn add_instance(
        &mut self,
        graph: u32,
        variables: Variables,
        world: &mut impl World,
    ) -> Result<Option<u16>, InitFault> {
        let world = erase(world);
        let id = u16::try_from(self.machine.next_id).map_err(|_| InitFault::InstanceIdExhausted)?;
        let mut created = false;
        self.run(world, |vm| {
            created = vm.create(id, graph, None, variables, vm.machine.snapshot.entity)?;
            Ok(())
        })?;
        Ok(created.then_some(id))
    }

    /// End an instance and its children now, without their end plugs
    /// (`Component.destroy`).
    pub fn destroy_instance(&mut self, id: u16, world: &mut impl World) -> Result<(), InitFault> {
        let world = erase(world);
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

    /// Whether a health change state runs (`STUStatescriptStateModifyHealth`,
    /// `vm/health.rs`: an aura's rate, the damaging Biotic Orb's `1101`):
    /// the client applies it at every frame.
    pub fn changes_health(&self) -> bool {
        self.machine
            .snapshot
            .instances
            .values()
            .flat_map(|instance| instance.states.values())
            .any(|state| state.active && matches!(state.kind, StateKind::ModifyHealth { .. }))
    }

    /// The active mover behaviour states' requests, for the mover.
    pub fn move_requests(&self) -> Vec<MoveRequest> {
        self.machine
            .movements
            .iter()
            .filter_map(|(key, &params)| {
                let state = self
                    .machine
                    .snapshot
                    .instances
                    .get(&key.instance)?
                    .states
                    .get(&key.raw)
                    .filter(|state| state.active)?;
                Some(MoveRequest {
                    instance: key.instance,
                    raw_state: key.raw,
                    activation: state.activation,
                    params,
                })
            })
            .collect()
    }

    /// The running AI movement states' points to walk to, for the walker
    /// ([`AiMoveRequest`]); a state's point goes once the walker reported
    /// the walk over, and comes back when its input plug asks again.
    pub fn ai_move_requests(&self) -> Vec<AiMoveRequest> {
        self.machine
            .ai_moves
            .iter()
            .map(|(key, &target)| AiMoveRequest {
                instance: key.instance,
                raw_state: key.raw,
                target,
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
        let world = erase(world);
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
        let world = erase(world);
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
        let world = erase(world);
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

    /// Queue the walker's report that an AI movement state's walk is over,
    /// `delay_ms` after the current clock; it runs at the next advance. The
    /// state lets its point go and follows `m_1B09389D`: the client's event
    /// handler of `STU_ECAEE401` (`0x7FF789B89D30`) follows that plug on
    /// event 3, the one the patrol `0B8E` and the idle wander `0B6F` wait
    /// on before their next walk. Which native event that is (arrived,
    /// gave up) is not in the client; arrived is the reading the data fits
    /// (AiDefinitions.md, §19 Q5).
    pub fn ai_move_arrived(
        &mut self,
        instance: u16,
        raw_state: u16,
        delay_ms: u64,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        let world = erase(world);
        if self.halted {
            return Err(InitFault::Halted);
        }
        if !self.machine.snapshot.instances.contains_key(&instance) {
            return Ok(());
        }
        self.run(world, |vm| {
            vm.post_after(instance, delay_ms, Event::AiMoveArrived(raw_state))
        })
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
        let world = erase(world);
        if frame_start(frame) < self.machine.now_ms {
            return Err(InitFault::FrameOrder {
                frame,
                now_ms: self.machine.now_ms,
            });
        }
        self.run(world, |vm| vm.advance_frame(frame, command))
    }

    fn run<W: World + ?Sized>(
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
    let world = erase(world);
    let mut machine = Machine::new(entity, entity_variables, 0, max_work)?;
    Vm::new(graphs, world, &mut machine).start(roots)?;
    machine.capture_outputs();
    Ok(machine.snapshot.0)
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
    /// A state's output bound to its variable (the Grappling Hook cast's
    /// `#431` and `#432`, `vm/grapple.rs`): the client keeps the value in the
    /// variable and the binding with this slot.
    Output {
        slot: u8,
        value: Value,
    },
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
    /// The walker's report that an AI movement state's walk is over
    /// ([`Component::ai_move_arrived`]).
    AiMoveArrived(u16),
    WatchChanged(u16),
    /// A reusing scope state took this scope instance
    /// ([`Component::change_scope`]): the client's state event of kind 2,
    /// which follows an active copy's scope plug again and does nothing to
    /// an inactive one (`vm/scopes.rs`).
    Rescope(u16),
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
            | Self::AiMoveArrived(state)
            | Self::WatchChanged(state)
            | Self::Rescope(state)
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
    /// A broadcast held by a running state ([`World::hold_broadcast`]).
    held: bool,
}

/// The game message a node sends or waits for; None for none.
fn message_of(node: &Node) -> Option<u64> {
    node.message().filter(|&id| id != 0)
}

struct Machine {
    snapshot: TrackedSnapshot,
    queues: Timeline<Event>,
    watchers: BTreeMap<VarKey, Vec<StateKey>>,
    /// The states whose expressions read another entity's variable
    /// (entity, id), for [`Component::foreign_variable_changed`].
    foreign_watchers: BTreeMap<(u32, u32), Vec<StateKey>>,
    /// The states whose expressions read a native config, by class, for
    /// [`Component::natives_changed`].
    native_watchers: BTreeMap<String, Vec<StateKey>>,
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
    /// The blocks the running `MoveTo` and path walk states handed the mover
    /// at their begin.
    movements: BTreeMap<StateKey, Behaviour>,
    /// The running Grappling Hook casts (`STU_4BBC5D09`, `vm/grapple.rs`).
    grapples: BTreeMap<StateKey, grapple::Cast>,
    /// The running AI movement states' points to walk to ([`AiMoveRequest`]).
    ai_moves: BTreeMap<StateKey, [f64; 3]>,
    /// The running HeroSelect states' hero limits (`vm/hero_select.rs`).
    hero_selects: BTreeMap<StateKey, hero_select::HeroLimits>,
    /// Motions started or ended since the mover took them.
    motions: Vec<Motion>,
    /// The auras ApplyAura states elsewhere put on this body, by who put
    /// them there: their instances (`vm/auras.rs`).
    applied_auras: BTreeMap<AuraSource, u16>,
    /// The scope instances scope states elsewhere opened here, by who
    /// opened them: the instance and whether its state keeps it
    /// (`vm/scopes.rs`).
    scopes: BTreeMap<ScopeSource, (u16, bool)>,
    /// The Workshop action results reported, by instance, with the
    /// instance's start (`vm/workshop.rs`).
    outcomes: BTreeMap<u16, (u64, ActionOutcome)>,
}

struct Vm<'graph, 'world, 'machine, W: ?Sized> {
    graphs: &'graph GraphStore,
    world: &'world mut W,
    machine: &'machine mut Machine,
    parameters: Option<Arc<Variables>>,
    /// The value every parameter reads while `CycleIndex` tests a candidate.
    candidate: Option<Value>,
    /// The sender of the message being delivered, when it is an entity:
    /// parameter `0009` ([`MESSAGE_SENDER`]).
    sender: Option<u32>,
    output_write_depth: usize,
}

impl<'graph, 'world, 'machine, W: World + ?Sized> Vm<'graph, 'world, 'machine, W> {
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
            sender: None,
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

    /// A new instance of `graph`, created with `source` (its [`CREATOR`];
    /// the snapshot's entity when `source` is none, as on the client): the
    /// component's own entity for its roots, the applier for an aura, the
    /// creating instance's own source for a child or a played script
    /// (assumption: the client's PlayScript and SubScript pass it on; that
    /// makes `0C71`'s children `0B99`/`0C72` see the grenade's thrower, as
    /// their AI damage test expects).
    fn create(
        &mut self,
        id: u16,
        graph: u32,
        parent: Option<(u16, u16)>,
        variables: Variables,
        source: u32,
    ) -> Result<bool, InitFault> {
        self.create_instance(id, graph, parent, variables, source, None)
    }

    /// A new scope instance ([`Self::create`] in a scope, `vm/scopes.rs`).
    fn create_scoped(
        &mut self,
        id: u16,
        graph: u32,
        scope: Option<ScopeLink>,
        source: u32,
    ) -> Result<bool, InitFault> {
        self.create_instance(id, graph, None, Variables::new(), source, scope)
    }

    /// Its entries of its scope (none: 0) queue at the current clock.
    fn create_instance(
        &mut self,
        id: u16,
        graph: u32,
        parent: Option<(u16, u16)>,
        variables: Variables,
        source: u32,
        scope: Option<ScopeLink>,
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
        let source = if expr::valid_entity(source) {
            source
        } else {
            self.machine.snapshot.entity
        };
        let scope_id = scope.as_ref().map_or(0, |scope| scope.id);
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
                scope,
                revision: 0,
            },
        );
        for position in definition.entries.iter().flatten() {
            if definition.node(*position).is_some_and(|node| {
                !node.client_only && is_entry(node) && scopes::entry_scope(node) == scope_id
            }) {
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
            .map_or(0, |instance| instance.source)
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
                Event::AiMoveArrived(raw) => self.ai_move_arrived(key(raw))?,
                Event::WatchChanged(raw) => self.deliver_watch(key(raw))?,
                Event::Rescope(raw) => {
                    if self.state(key(raw)).is_some_and(|state| state.active) {
                        let node = self.graph(id).state(usize::from(raw)).expect("scope state");
                        self.follow(id, node, "m_C41A9BE2", Some(key(raw)), 0)?;
                    }
                }
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
        self.sender = expr::valid_entity(sender).then_some(sender);
        let result = self.deliver_entry(id, node, sender);
        self.parameters = None;
        self.sender = None;
        result
    }

    /// Parameter `id` of the message being delivered: `0009` is its
    /// sender, whatever the message carries (the dispatch's parameter
    /// source `sub_7FF78AAA32F0` answers it from the message's `+152`
    /// before its own parameters), the others the message's.
    fn message_parameter(&self, id: u32) -> Value {
        match self.sender {
            Some(sender) if id == MESSAGE_SENDER => Value::Entity(sender),
            _ => self
                .parameters
                .as_deref()
                .and_then(|parameters| parameters.get(&id))
                .cloned()
                .unwrap_or(Value::Null),
        }
    }

    /// Whether a message entry's filter (`m_filter`) lets the message in.
    /// The client tests the message's sender with
    /// `ScriptHost_EntityPassesFilter` (an EntryGameMessage in the dispatch
    /// `sub_7FF78AAA1D70`; a receiving state hands its filter to the held
    /// message lookup `sub_7FF789CAAD40`); a message with no sender passes.
    /// The condition reads the message's parameters and `0009`, its
    /// sender: `0C90` st4/st8 keep only another player's `08BE` pick, so a
    /// controller's own broadcast is not a teammate's pick; the Training
    /// Partner pick's `1213` (`265F` rid22, sent by the client's
    /// `STU_79A131B4` from its host, the controller) wants a hero
    /// (`param(2278)`, `08E6`) with a body that the player may play
    /// (`STU_94AED9DC`, `STU_AFC074B3`), `0C90` rid164/rid170 such heroes
    /// too. Masks and tag lists on a message filter (`16C5` rid10 alone
    /// among the entries the server runs) are not tested: taken, as an
    /// assumption.
    fn message_filter_admits(&mut self, id: u16, node: &Node) -> bool {
        let filter = field(&node.fields, "m_filter");
        if filter.is_null() || self.sender.is_none() {
            return true;
        }
        if !filter_array::condition_only(filter) {
            self.assume(
                self.site(id, node),
                &node.class,
                "message filter masks and tags not tested: taken",
            );
        }
        let condition = field(filter, "m_condition");
        condition.is_null()
            || self
                .evaluate(id, node, condition, None)
                .is_some_and(|admitted| expr::truthy(&admitted))
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
            if !self.message_filter_admits(id, node) {
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
            if !self.message_filter_admits(id, node) {
                return Ok(());
            }
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
            let value = self.message_parameter(parameter_id(field));
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
    /// target of channels (`STU_1D719351`, every entity) is held while the
    /// state runs ([`World::hold_broadcast`]) and ended with it, as the
    /// client's live-sender registry keeps it for entities that look it up
    /// later (GameMessages.md).
    pub(super) fn begin_send_state(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let Some(mut recipients) = self.message_recipients(key.instance, node) else {
            return Ok(());
        };
        recipients.held = recipients.broadcast;
        if !self.deliver_game_message(key.instance, node, &recipients, &recipients.entities)? {
            return Ok(());
        }
        if recipients.message == 0 {
            return Ok(());
        }
        let since_ms = self.machine.now_ms;
        if recipients.broadcast {
            // The sender's own instances took it from the VM; every other
            // entity holds it through the world.
            if let Some(state) = self.state_mut(key) {
                state.tracked = recipients
                    .entities
                    .iter()
                    .copied()
                    .chain([crate::mode::EVERY_ENTITY])
                    .map(|entity| TrackedTarget { entity, since_ms })
                    .collect();
            }
            return Ok(());
        }
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
            held: false,
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
            held: false,
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
                self.world.observe_own_message(site, message, &parameters);
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
        let delivered = !recipients.broadcast
            || if recipients.held {
                self.world.hold_broadcast(site, message, &parameters)
            } else {
                self.world.broadcast_message(site, message, &parameters)
            };
        if !delivered {
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
            graphs: self.graphs,
            world: self.world,
            site,
            reads: Vec::new(),
            foreign_reads: Vec::new(),
            parameters: self.parameters.as_deref(),
            candidate: self.candidate.clone(),
            sender: self.sender,
            watcher,
            read_native: false,
            read_volatile: false,
            natives: Vec::new(),
            assumed: Vec::new(),
        };
        let result = if assignment {
            expr::evaluate_value_in(config, node.configs(), &mut context)
        } else {
            expr::evaluate_in(config, node.configs(), &mut context)
        };
        let reads = context.reads;
        let foreign_reads = context.foreign_reads;
        // A native or a volatile binding read: the client re-runs the
        // switches each update (BooleanSwitch begin `0x7FF78AA80090` sets
        // the flag `+0x1B5` that puts a volatile reader on the update list,
        // `0x7FF78AAA4C30`; its update re-checks the condition).
        let polled = context.read_native || context.read_volatile;
        let natives = context.natives;
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
            for key in foreign_reads {
                let watchers = self.machine.foreign_watchers.entry(key).or_default();
                if !watchers.contains(&watcher) {
                    watchers.push(watcher);
                }
            }
            for class in natives {
                let watchers = self.machine.native_watchers.entry(class).or_default();
                if !watchers.contains(&watcher) {
                    watchers.push(watcher);
                }
            }
            if let Some(start) = self
                .state(watcher)
                .filter(|state| {
                    polled
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
        let snapshot = &self.machine.snapshot;
        let current = match key {
            VarKey::Entity(id) => snapshot.variables.get(&id),
            VarKey::Instance(instance, id) => snapshot.instances[&instance].variables.get(&id),
        };
        // The same value again leaves the snapshot (and its revision) as
        // it is.
        if current.is_some_and(|current| current.identical(&value)) {
            return;
        }
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
            // A state that re-evaluates its own fields takes the change
            // (slot 25); any other dependent read the change in its
            // transitions and tries them again (event kind 2): the
            // client's dependents walk `0x7FF78AA9ED40` → `0x7FF78AA82630`
            // → `0x7FF78AA81220`.
            if state.kind.updates_on_change()
                || matches!(state.kind, StateKind::HealthPool { pool: Some(_) })
            {
                self.state_mut(watcher)
                    .expect("active watcher")
                    .pending_change = true;
                self.post(watcher.instance, Event::Changed(watcher.raw))?;
            } else if !state.pending_check {
                self.state_mut(watcher)
                    .expect("active watcher")
                    .pending_check = true;
                self.post(watcher.instance, Event::Check(watcher.raw))?;
            }
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
        && matches!(field(&node.fields, "m_598E32D5").as_u64(), Some(0 | 1))
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

/// A ForCodeSynced whose rows also go through its transport step: after
/// begin (0x7FF789C54D00) the client calls 0x7FF789C5F690 when the
/// component's host takes it (its vtable +8) and `m_598E32D5` (slot 57,
/// state +0xFC) or `m_0582B0E4` (slot 58) is set. What that step does on
/// the retail server was not read; `0CBB`'s phase rows (identifier `2123`:
/// `2297` waiting, `2124` setup, `2298` round, `2125` round end, `2126`
/// match end) use it.
fn for_code_transport(node: &Node) -> bool {
    node.class == FOR_CODE_SYNCED && field(&node.fields, "m_598E32D5").as_u64() == Some(1)
}

/// The HealthPool forms the VM runs, or None: the body's own pools (`0033`:
/// `m_4895B8B6`, an output `m_CEAB19E8`, no name), Some(false); and the
/// temporary ones (`m_0E8A5C8E`: Doomfist's `0DF3` st0, Wrecking Ball's
/// `1566` st6, `073B`, `12C1`, `1721`, `1D03`), named (`m_7BE02F89`, an
/// identifier) or not, with or without the outputs `m_CEAB19E8` (its
/// current amount) and `m_C99FAFDD` (its pool, which `073B`'s ModifyHealth
/// names in `m_C744A524`), Some(true).
fn health_pool_form(node: &Node) -> Option<bool> {
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
    let output = |name| {
        let value = config(name);
        value.is_null() || matches!(expr::lvalue(value), Ok(Some(_)))
    };
    let source = config("m_69A20070");
    let common = node.is_networked()
        && ["m_FF79E783", "m_AF1104D0", "m_BFA825EA"]
            .into_iter()
            .all(|name| config(name).is_null())
        && pool_kind(config("m_7E8B666C")).is_some()
        && is_owner(config("m_FA3ECC43"))
        && expr::native_shape(source, "STU_FEDD683C", Some("m_entity"))
        && is_owner(field(source, "m_entity"))
        && flag("m_64839023", false);
    if !common {
        return None;
    }
    if flag("m_0E8A5C8E", false)
        && config("m_7BE02F89").is_null()
        && config("m_C99FAFDD").is_null()
        && flag("m_4895B8B6", true)
        && matches!(expr::lvalue(config("m_CEAB19E8")), Ok(Some(_)))
    {
        return Some(false);
    }
    let name = config("m_7BE02F89");
    (flag("m_0E8A5C8E", true)
        && config("m_4895B8B6").is_null()
        && (name.is_null()
            || (field(name, "$").as_str() == Some("STUConfigVarIdentifier")
                && field(name, "m_identifier").is_string()
                && field(name, "m_resourceKey").is_null()))
        && output("m_CEAB19E8")
        && output("m_C99FAFDD"))
    .then_some(true)
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
    member(fields, name).unwrap_or(&Json::Null)
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

/// The f32 length from `current` to `destination`.
fn chase_distance(current: &Value, destination: &Value) -> f64 {
    let current = chase_vector(current.clone());
    let destination = chase_vector(destination.clone());
    let squares = (0..3)
        .map(|axis| {
            let delta = (destination[axis] - current[axis]) as f32;
            delta * delta
        })
        .sum::<f32>();
    f64::from(squares.sqrt())
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

struct InstanceContext<'a, W: ?Sized> {
    machine: &'a Machine,
    /// The graphs, for the nodes of the instances' active states (the
    /// ForCode rows [`Context::for_code`] reads).
    graphs: &'a GraphStore,
    world: &'a mut W,
    site: Site,
    reads: Vec<VarKey>,
    /// Other entities' variables read (entity, id).
    foreign_reads: Vec<(u32, u32)>,
    parameters: Option<&'a Variables>,
    /// Answers every parameter read when set: a `CycleIndex` candidate, a
    /// filter's ([`expr::Context::bind_candidate`]).
    candidate: Option<Value>,
    /// The delivered message's sender: parameter `0009`.
    sender: Option<u32>,
    watcher: Option<StateKey>,
    read_native: bool,
    /// A variable bound to a volatile output (a weapon volley's) was read.
    read_volatile: bool,
    /// The native configs' classes read ([`Context::native`]).
    natives: Vec<String>,
    /// Native answers the world marked as assumptions, by config class.
    assumed: Vec<(String, &'static str)>,
}

#[cfg(test)]
mod array_tests;
#[cfg(test)]
mod membership_tests;
#[cfg(test)]
mod tests;
