//! Initialization and supported frame events of authored statescript graphs.
//!
//! This covers Entry, typed variable writes, conditional links and static
//! SubScript instances. Diagnostics make a partial snapshot unsuitable for
//! gameplay replication. The component also advances static waits and dispatches
//! scoped message entries. Input, combat and other authored actions still need
//! their corresponding runtime.

mod abilities;
mod actions;
mod buttons;
mod context;
mod lifecycle;
mod links;
mod machine;
mod nodes;
mod state_kinds;
mod switches;
mod timers;
mod volleys;

use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt,
    sync::Arc,
};

use indexmap::IndexMap;
use serde_json::Value as Json;

use crate::{
    expr::{
        self, Context, EmptyContext, EntityRelation, EvalError, Scope, StackPriority, Variable,
        equal, truthy,
    },
    graph::{Body, Graph, GraphStore, Node, asset_guid},
    timeline::{ButtonEdge, FRAME_MS, QueueFull, Timeline, button_edges, frame_start},
    vitals::PoolKind,
    wire::{Value, Variables},
};
use orpheus_physics::mover::modifiers::{Modifier, ModifierKey, Operation};
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
        for field in binding.sync.iter().flatten() {
            let key = field
                .get("m_0D09D2D9")
                .and_then(asset_guid)
                .map_or(0, |guid| guid as u16);
            let value = expr::evaluate(
                field.get("m_value").unwrap_or(&Json::Null),
                &mut EmptyContext,
            )
            .map_err(|error| InitFault::RootBinding { id, graph, error })?;
            root.variables.insert(u32::from(key), value);
        }
        roots.push(root);
    }
    Ok(roots)
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

    fn has_target_tag(&self, _site: Site, _entity: u32, _tag: u64) -> Option<bool> {
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

    fn body_on_fire(&mut self, _site: Site, _entity: u32) -> Option<bool> {
        None
    }

    /// None means this relationship is unsupported for the supplied entity.
    fn related_entity(
        &mut self,
        _site: Site,
        _relation: EntityRelation,
        _entity: u32,
    ) -> Option<u32> {
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

/// A `MoveTo` state's fixed world target and speed in metres per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveRequest {
    pub instance: u16,
    pub raw_state: u16,
    pub target: [f64; 3],
    pub speed: f64,
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
    /// A target search. The server can't run one yet, so it finds nothing,
    /// as in the Python reference. Unverified.
    TrackTargets,
    /// Melee's hit list (`STU_77E13026`). The server doesn't run melee yet, so
    /// it records no hits. Unverified.
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
    watch_seen: Option<Value>,
    ending: bool,
    pending_change: bool,
    pending_timer: bool,
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
            watch_seen: None,
            ending: false,
            pending_change: false,
            pending_timer: false,
            delayed_timers: [false; 2],
            buffered_press: None,
            linked_variable: None,
            ability_stack: None,
        }
    }
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

#[derive(Clone, Debug)]
pub struct Instance {
    pub graph: u32,
    /// Parent instance and raw state index, as carried on the wire.
    pub parent: Option<(u16, u16)>,
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
    /// The component's clock when this snapshot was taken.
    pub now_ms: u64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
            .queue_entries(&self.graphs, Some(instance), message, parameters)
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

    /// A game message from another body: every instance's entries for it
    /// queue at the current clock and run now (`send_message` in
    /// `ow174/game/script/runtime.py`, as `Match.deliver_body_messages` calls
    /// it). Returns how many entries took it.
    pub fn receive_message(
        &mut self,
        message: u64,
        parameters: Variables,
        world: &mut impl World,
    ) -> Result<usize, InitFault> {
        if self.halted {
            return Err(InitFault::Halted);
        }
        let entries = self
            .machine
            .queue_entries(&self.graphs, None, message, parameters)?;
        if entries != 0 {
            self.run_now(world)?;
        }
        Ok(entries)
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
            created = vm.create(id, graph, None, Variables::new())?;
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
    spread: f64,
    planned: u32,
    reported: u32,
    outputs: [Option<VarKey>; 3],
}

impl Volley {
    fn ammo_at(&self, now_ms: u64) -> i64 {
        let Some(elapsed) = now_ms.checked_sub(self.start_ms) else {
            return self.ammo0;
        };
        let seconds = f64::from(elapsed as f32 * 0.001_f32);
        let shots = ((seconds * self.rate) as f32).floor() as i64;
        self.ammo0
            .saturating_sub(self.per_shot.saturating_mul(shots.saturating_add(1)))
            .max(0)
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
    ChaseReached(u16),
    WatchChanged(u16),
    Message {
        position: u16,
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
            | Self::ChaseReached(state)
            | Self::WatchChanged(state)
            | Self::Timer(state, _) => Some(state),
        }
    }
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
        for root in roots {
            self.create(root.id, root.graph, None, root.variables)?;
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

    fn create(
        &mut self,
        id: u16,
        graph: u32,
        parent: Option<(u16, u16)>,
        variables: Variables,
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
        self.machine.next_id = self.machine.next_id.max(u32::from(id) + 1);
        self.machine.snapshot.instances.insert(
            id,
            Instance {
                graph,
                parent,
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
                Event::ChaseReached(raw) => {
                    if self.state(key(raw)).is_some_and(|state| state.active) {
                        let node = self.graph(id).state(usize::from(raw)).expect("chase state");
                        self.try_complete(key(raw), node, 0)?;
                    }
                }
                Event::WatchChanged(raw) => self.deliver_watch(key(raw))?,
                Event::Message {
                    position,
                    parameters,
                } => {
                    self.deliver(id, position, parameters)?;
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
        parameters: Arc<Variables>,
    ) -> Result<(), InitFault> {
        let Some(node) = self.graph(id).node(position) else {
            return Ok(());
        };
        self.parameters = Some(parameters);
        let result = self.deliver_entry(id, node);
        self.parameters = None;
        result
    }

    fn deliver_entry(&mut self, id: u16, node: &Node) -> Result<(), InitFault> {
        let path = if node.is_state() {
            let Some(raw) = node.state else { return Ok(()) };
            let key = StateKey { instance: id, raw };
            let Some(state) = self.state(key).filter(|state| state.active) else {
                return Ok(());
            };
            if state.kind != StateKind::MessageIdle {
                self.report(
                    self.site(id, node),
                    Issue::UnsupportedField("state game message delivery"),
                );
                return Ok(());
            }
            self.exit_subgraph(id, node, "m_33DA5163", 1)?;
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
        self.follow(id, node, path.0, path.1, 0)?;
        Ok(())
    }

    fn send_game_message(&mut self, id: u16, node: &Node) -> Result<bool, InitFault> {
        let Some(message) = node
            .field("m_gameMessage.m_gameMessage")
            .and_then(asset_guid)
            .filter(|&id| id != 0)
        else {
            return Ok(true);
        };
        let site = self.site(id, node);
        let target = node
            .fields
            .get("m_targets")
            .or_else(|| node.fields.get("m_target"))
            .unwrap_or(&Json::Null);
        let value = if target.is_null() {
            Value::Entity(site.entity)
        } else {
            let Some(value) = self.evaluate(id, node, target, None) else {
                return Ok(false);
            };
            value
        };
        let entities: Vec<_> = match value {
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
        if entities.is_empty() {
            // A message to nobody: its parameters are never read.
            return Ok(true);
        }
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
        for entity in entities {
            self.spend("game message routing")?;
            if entity == site.entity {
                self.machine
                    .queue_entries(self.graphs, None, message, parameters.clone())?;
            } else if !self.world.send_message(site, entity, message, &parameters) {
                self.report(
                    site,
                    Issue::UnsupportedField("external game message delivery"),
                );
                return Ok(false);
            }
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
        let result = expr::evaluate(config, &mut context);
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

fn is_owner_movement_sampler(node: &Node) -> bool {
    node.is_networked()
        && node.remote == -1
        && [
            "m_entity",
            "m_A10CC74B",
            "m_0B1AA8CA",
            "m_4F8F2F3F",
            "m_8BF03679",
            "m_BF5B22B7",
        ]
        .iter()
        .all(|name| field(&node.fields, name).is_null())
        && ["m_E37BC6C2", "m_2102C541"]
            .iter()
            .all(|path| node.links(path).next().is_none())
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
mod membership_tests;
#[cfg(test)]
mod tests;
