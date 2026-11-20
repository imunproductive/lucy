//! What the rule engine needs from the match: the world it reads values
//! from, the variable store, and the action graphs it starts. The server
//! implements them on the VM side (Workshop.md §5): global variables are
//! entity variables of the mode entity, player variables those of each
//! player's controller, and each action runs its `054/0155` graph with the
//! evaluated parameters in `#18739`.

use std::sync::Arc;

use crate::{
    catalog::Id,
    logic,
    value::{EntityId, Value},
};

/// The contextual values of a rule execution (the event's players and
/// numbers), shared by the subroutines it calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    /// Event Player: none for a global rule.
    pub event_player: Option<EntityId>,
    pub attacker: Option<EntityId>,
    pub victim: Option<EntityId>,
    pub healer: Option<EntityId>,
    pub healee: Option<EntityId>,
    pub event_damage: f64,
    pub event_healing: f64,
    pub event_was_critical_hit: bool,
    pub event_was_health_pack: bool,
    pub event_was_environment: bool,
    /// Event Ability: the Button of the ability (Workshop's `Event
    /// Ability` is a Button constant).
    pub event_ability: Option<Id>,
    pub event_direction: [f64; 3],
    /// Local Player: set only when a per-player action re-evaluates its
    /// parameters for one viewer (the value is meaningless on the server
    /// otherwise; the `Local Player` description in `054/0162`).
    pub local_player: Option<EntityId>,
}

/// A player-event occurrence the server reports (Player Died, Player Dealt
/// Damage…): the event type (`logic::events`) and its context, whose
/// `event_player` is the player the event happened to.
#[derive(Clone, Debug, PartialEq)]
pub struct GameEvent {
    pub kind: u8,
    pub context: Context,
}

/// A ray cast's result (Ray Cast Hit Position, Normal, Player).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RayHit {
    pub position: [f64; 3],
    pub normal: [f64; 3],
    pub player: Option<EntityId>,
}

/// What a player is doing, for the `Is …` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    FiringPrimary,
    FiringSecondary,
    UsingAbility1,
    UsingAbility2,
    UsingUltimate,
    Meleeing,
    Reloading,
    Crouching,
    Jumping,
    OnWall,
    InSpawnRoom,
}

/// The match as the values see it. Players are entities (their
/// controllers). Positions are in meters, angles in degrees, as Workshop
/// values report them.
pub trait World {
    /// Every player and dummy bot in the match, Team 1 then Team 2, each
    /// in slot order.
    fn players(&self) -> Vec<EntityId>;
    /// 1 or 2; 0 in a free-for-all mode.
    fn team_of(&self, player: EntityId) -> u8;
    fn slot_of(&self, player: EntityId) -> i32;
    fn hero_of(&self, player: EntityId) -> Option<u64>;
    fn is_dummy_bot(&self, player: EntityId) -> bool;
    fn has_spawned(&self, player: EntityId) -> bool;
    fn is_alive(&self, player: EntityId) -> bool;
    fn entity_exists(&self, entity: EntityId) -> bool;
    /// An entity's position: a player's feet.
    fn position(&self, entity: EntityId) -> [f64; 3];
    fn eye_position(&self, player: EntityId) -> [f64; 3];
    /// The unit facing (aim) direction.
    fn facing(&self, player: EntityId) -> [f64; 3];
    fn velocity(&self, player: EntityId) -> [f64; 3];
    /// The movement input: X left, Z forward (Throttle Of).
    fn throttle(&self, player: EntityId) -> [f64; 3];
    /// Whether `player` holds a Button (`4A39` value).
    fn is_button_held(&self, player: EntityId, button: Id) -> bool;
    fn is_on_ground(&self, player: EntityId) -> bool;
    fn activity(&self, player: EntityId, activity: Activity) -> bool;
    /// Height above the surface below; 0 on a surface (Altitude Of).
    fn altitude(&self, player: EntityId) -> f64;
    fn health(&self, player: EntityId) -> f64;
    fn max_health(&self, player: EntityId) -> f64;
    fn ultimate_charge_percent(&self, player: EntityId) -> f64;
    /// Seconds until the ability on `button` is ready.
    fn ability_cooldown(&self, player: EntityId, button: Id) -> f64;
    /// A status of the Has Status enum (`4C33`).
    fn has_status(&self, player: EntityId, status: Id) -> bool;
    /// A communication of the Is Communicating enum (`5123`); `None` for
    /// any (and the emote and spray families by their values).
    fn is_communicating(&self, player: EntityId, kind: Option<Id>) -> bool;
    fn allowed_heroes(&self, player: EntityId) -> Vec<u64>;
    fn all_heroes(&self) -> Vec<u64>;
    fn host_player(&self) -> Option<EntityId>;
    /// Seconds left in the mode's phase (Match Time).
    fn match_time(&self) -> f64;
    fn is_game_in_progress(&self) -> bool;
    fn is_in_setup(&self) -> bool;
    fn is_waiting_for_players(&self) -> bool;
    fn is_assembling_heroes(&self) -> bool;
    fn current_map(&self) -> u32;
    fn current_game_mode(&self) -> u64;
    /// The last entity and text the action graphs created for `creator`
    /// (an event player) or globally (`None`).
    fn last_created_entity(&self, creator: Option<EntityId>) -> Value;
    fn last_text_id(&self, creator: Option<EntityId>) -> Value;
    fn entity_count(&self) -> usize;
    fn text_count(&self) -> usize;
    /// Server Load, Server Load Average, Server Load Peak (percent).
    fn server_load(&self) -> [f64; 3];
    fn nearest_walkable_position(&self, position: [f64; 3]) -> [f64; 3];
    fn ray_cast(
        &self,
        start: [f64; 3],
        end: [f64; 3],
        include: &[EntityId],
        exclude: &[EntityId],
    ) -> RayHit;
    fn is_in_line_of_sight(&self, start: [f64; 3], end: [f64; 3]) -> bool;
}

/// The variable store: global variables on the mode entity, player
/// variables on each player's controller, so the action graphs (Chase
/// Variable's helper `1CCF`) and the engine see the same values.
pub trait Variables {
    fn global(&self, index: u8) -> Value;
    fn set_global(&mut self, index: u8, value: Value);
    fn player(&self, player: EntityId, index: u8) -> Value;
    fn set_player(&mut self, player: EntityId, index: u8, value: Value);
}

pub type ActionHandle = u64;

/// An action parameter as the engine evaluated it.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    /// The same for every player.
    Value(Value),
    /// A parameter that names Local Player: its value with no local
    /// player (the server's), and each player's own (the parameter
    /// evaluated with [`Context::local_player`] that player), in
    /// [`World::players`] order.
    PerViewer {
        value: Value,
        viewers: Vec<(EntityId, Value)>,
    },
}

impl Param {
    /// The value the server sees.
    pub fn value(&self) -> &Value {
        match self {
            Self::Value(value) | Self::PerViewer { value, .. } => value,
        }
    }
}

/// An action to run as its graph.
pub struct ActionCall {
    /// The action's index (`054/0155`).
    pub action: u16,
    /// Its graph (`m_graph`); every action the engine hands over has one.
    pub graph: u64,
    /// The parameters evaluated once, in catalog order (`#18739`).
    pub params: Vec<Param>,
    /// The action as written, for the parameters an action re-evaluates
    /// (its Reevaluation enum): [`crate::Engine::evaluate_params`] with
    /// this context evaluates them again.
    pub source: Arc<logic::Action>,
    pub context: Context,
}

/// Starts action graphs. The rule waits on each started action until it
/// is finished (the graph reached `STU_1E93FE1F`, Workshop.md §2).
pub trait Actions {
    fn start(&mut self, call: ActionCall) -> ActionHandle;
    fn finished(&mut self, handle: ActionHandle) -> bool;
}

/// Everything the engine runs against.
pub trait Host: World + Variables + Actions {}

impl<T: World + Variables + Actions + ?Sized> Host for T {}
