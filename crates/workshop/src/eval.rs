//! Evaluating values: the natively evaluated half of the Workshop (no
//! value has a graph, Workshop.md §5). Semantics follow each value's
//! description in the catalog (`054/0162`); where the description is
//! silent, the choice is marked as an assumption.

use std::collections::BTreeSet;

use crate::{
    catalog::{Catalog, Id},
    host::Variables,
    host::{Activity, Context, World},
    logic::{Arg, Value as Tree, WorkshopSetting},
    value::{Color, EntityId, Text, Value, display},
};

/// The value identifiers the evaluator knows (`054/0162`).
#[allow(missing_docs)]
pub mod ids {
    use crate::catalog::Id;

    pub const NUMBER: Id = 0x4876;
    pub const TRUE: Id = 0x4878;
    pub const FALSE: Id = 0x4877;
    pub const NULL: Id = 0x4E63;
    pub const TEAM: Id = 0x49C7;
    pub const HERO: Id = 0x4891;
    pub const BUTTON: Id = 0x74D2;
    pub const COLOR: Id = 0x7F8D;
    pub const CUSTOM_COLOR: Id = 0x7FAB;
    pub const MAP: Id = 0x6931;
    pub const GAME_MODE: Id = 0x6C35;
    pub const VECTOR: Id = 0x49C0;
    pub const UP: Id = 0x49EB;
    pub const DOWN: Id = 0x49EC;
    pub const LEFT: Id = 0x49E9;
    pub const RIGHT: Id = 0x49EA;
    pub const FORWARD: Id = 0x49ED;
    pub const BACKWARD: Id = 0x49EE;
    pub const EMPTY_ARRAY: Id = 0x53CA;
    pub const ARRAY: Id = 0x4FDA;
    pub const CUSTOM_STRING: Id = 0x5C14;
    pub const STRING: Id = 0x519B;
    pub const INPUT_BINDING_STRING: Id = 0x7A66;
    pub const ABILITY_ICON_STRING: Id = 0x74DF;
    pub const ICON_STRING: Id = 0x5B3D;
    pub const HERO_ICON_STRING: Id = 0x54D0;
    pub const GLOBAL_VARIABLE: Id = 0x49CB;
    pub const PLAYER_VARIABLE: Id = 0x49CF;
    pub const EVENT_PLAYER: Id = 0x4C0F;
    pub const ATTACKER: Id = 0x4C0D;
    pub const VICTIM: Id = 0x4C0E;
    pub const HEALER: Id = 0x5AED;
    pub const HEALEE: Id = 0x5AEE;
    pub const EVENT_DAMAGE: Id = 0x56CC;
    pub const EVENT_HEALING: Id = 0x5AF4;
    pub const EVENT_WAS_CRITICAL_HIT: Id = 0x56CD;
    pub const EVENT_WAS_HEALTH_PACK: Id = 0x6ED3;
    pub const EVENT_WAS_ENVIRONMENT: Id = 0x79EB;
    pub const EVENT_ABILITY: Id = 0x747F;
    pub const EVENT_DIRECTION: Id = 0x739D;
    pub const LOCAL_PLAYER: Id = 0x866E;
    pub const HOST_PLAYER: Id = 0x5AEF;
    pub const CURRENT_ARRAY_ELEMENT: Id = 0x4E8D;
    pub const CURRENT_ARRAY_INDEX: Id = 0x7A65;
    pub const EVALUATE_ONCE: Id = 0x804E;
    pub const UPDATE_EVERY_FRAME: Id = 0x80B5;
    pub const ADD: Id = 0x55B1;
    pub const SUBTRACT: Id = 0x55B2;
    pub const MULTIPLY: Id = 0x55B4;
    pub const DIVIDE: Id = 0x55B6;
    pub const MODULO: Id = 0x55B7;
    pub const RAISE_TO_POWER: Id = 0x55BA;
    pub const MIN: Id = 0x55BB;
    pub const MAX: Id = 0x55BC;
    pub const AND: Id = 0x4B74;
    pub const OR: Id = 0x4B75;
    pub const NOT: Id = 0x4B76;
    pub const COMPARE: Id = 0x4B77;
    pub const IF_THEN_ELSE: Id = 0x7555;
    pub const ABSOLUTE_VALUE: Id = 0x5565;
    pub const SQUARE_ROOT: Id = 0x5564;
    pub const ROUND_TO_INTEGER: Id = 0x5563;
    pub const SINE_FROM_DEGREES: Id = 0x555A;
    pub const SINE_FROM_RADIANS: Id = 0x555C;
    pub const COSINE_FROM_DEGREES: Id = 0x555B;
    pub const COSINE_FROM_RADIANS: Id = 0x555D;
    pub const TANGENT_FROM_DEGREES: Id = 0x588A;
    pub const TANGENT_FROM_RADIANS: Id = 0x588B;
    pub const ARCSINE_IN_DEGREES: Id = 0x588E;
    pub const ARCSINE_IN_RADIANS: Id = 0x588F;
    pub const ARCCOSINE_IN_DEGREES: Id = 0x588C;
    pub const ARCCOSINE_IN_RADIANS: Id = 0x588D;
    pub const ARCTANGENT_IN_DEGREES: Id = 0x5890;
    pub const ARCTANGENT_IN_RADIANS: Id = 0x5891;
    pub const RANDOM_INTEGER: Id = 0x4E6B;
    pub const RANDOM_REAL: Id = 0x4E6C;
    pub const RANDOM_VALUE_IN_ARRAY: Id = 0x4E6D;
    pub const RANDOMIZED_ARRAY: Id = 0x4E6E;
    pub const X_COMPONENT_OF: Id = 0x4B6E;
    pub const Y_COMPONENT_OF: Id = 0x4B71;
    pub const Z_COMPONENT_OF: Id = 0x4B72;
    pub const DISTANCE_BETWEEN: Id = 0x4AC0;
    pub const DIRECTION_TOWARDS: Id = 0x4AC1;
    pub const NORMALIZE: Id = 0x555E;
    pub const MAGNITUDE_OF: Id = 0x7FE5;
    pub const DOT_PRODUCT: Id = 0x5566;
    pub const CROSS_PRODUCT: Id = 0x5567;
    pub const DIRECTION_FROM_ANGLES: Id = 0x5211;
    pub const HORIZONTAL_ANGLE_FROM_DIRECTION: Id = 0x5212;
    pub const VERTICAL_ANGLE_FROM_DIRECTION: Id = 0x5213;
    pub const ANGLE_BETWEEN_VECTORS: Id = 0x5894;
    pub const ANGLE_DIFFERENCE: Id = 0x4B84;
    pub const VECTOR_TOWARDS: Id = 0x4AC2;
    pub const WORLD_VECTOR_OF: Id = 0x4C11;
    pub const LOCAL_VECTOR_OF: Id = 0x4C15;
    pub const HORIZONTAL_ANGLE_TOWARDS: Id = 0x4B7F;
    pub const VERTICAL_ANGLE_TOWARDS: Id = 0x4B80;
    pub const NEAREST_WALKABLE_POSITION: Id = 0x5555;
    pub const VALUE_IN_ARRAY: Id = 0x4DCC;
    pub const COUNT_OF: Id = 0x4B6D;
    pub const APPEND_TO_ARRAY: Id = 0x55BD;
    pub const REMOVE_FROM_ARRAY: Id = 0x55BE;
    pub const ARRAY_CONTAINS: Id = 0x5559;
    pub const ARRAY_SLICE: Id = 0x4E6F;
    pub const INDEX_OF_ARRAY_VALUE: Id = 0x5558;
    pub const FIRST_OF: Id = 0x4E91;
    pub const LAST_OF: Id = 0x4E92;
    pub const FILTERED_ARRAY: Id = 0x4E8C;
    pub const SORTED_ARRAY: Id = 0x4E93;
    pub const MAPPED_ARRAY: Id = 0x7A7E;
    pub const IS_TRUE_FOR_ANY: Id = 0x4E8B;
    pub const IS_TRUE_FOR_ALL: Id = 0x4E8A;
    pub const ALL_PLAYERS: Id = 0x4B5C;
    pub const ALL_LIVING_PLAYERS: Id = 0x4B5F;
    pub const ALL_DEAD_PLAYERS: Id = 0x4B60;
    pub const PLAYERS_IN_SLOT: Id = 0x49C6;
    pub const PLAYERS_ON_HERO: Id = 0x4C10;
    pub const PLAYERS_WITHIN_RADIUS: Id = 0x4ABA;
    pub const CLOSEST_PLAYER_TO: Id = 0x4AB8;
    pub const FARTHEST_PLAYER_FROM: Id = 0x4AB9;
    pub const NUMBER_OF_PLAYERS: Id = 0x4BA5;
    pub const NUMBER_OF_LIVING_PLAYERS: Id = 0x4BA7;
    pub const NUMBER_OF_DEAD_PLAYERS: Id = 0x4BA8;
    pub const TEAM_OF: Id = 0x4B78;
    pub const SLOT_OF: Id = 0x528B;
    pub const HERO_OF: Id = 0x4890;
    pub const POSITION_OF: Id = 0x49EF;
    pub const EYE_POSITION: Id = 0x5659;
    pub const FACING_DIRECTION_OF: Id = 0x4B83;
    pub const HORIZONTAL_FACING_ANGLE_OF: Id = 0x4B81;
    pub const VERTICAL_FACING_ANGLE_OF: Id = 0x4B82;
    pub const VELOCITY_OF: Id = 0x4B57;
    pub const SPEED_OF: Id = 0x4B58;
    pub const HORIZONTAL_SPEED_OF: Id = 0x4B59;
    pub const VERTICAL_SPEED_OF: Id = 0x4B5A;
    pub const SPEED_OF_IN_DIRECTION: Id = 0x4B5B;
    pub const THROTTLE_OF: Id = 0x4BCA;
    pub const ALTITUDE_OF: Id = 0x49F0;
    pub const IS_BUTTON_HELD: Id = 0x4BC9;
    pub const IS_ON_GROUND: Id = 0x5410;
    pub const IS_IN_AIR: Id = 0x4B91;
    pub const IS_ALIVE: Id = 0x4B79;
    pub const IS_DEAD: Id = 0x4B7A;
    pub const HAS_SPAWNED: Id = 0x545D;
    pub const IS_DUMMY_BOT: Id = 0x5C8A;
    pub const IS_FIRING_PRIMARY: Id = 0x55A6;
    pub const IS_FIRING_SECONDARY: Id = 0x55A7;
    pub const IS_USING_ABILITY_1: Id = 0x55A8;
    pub const IS_USING_ABILITY_2: Id = 0x55A9;
    pub const IS_USING_ULTIMATE: Id = 0x5122;
    pub const IS_MELEEING: Id = 0x72DF;
    pub const IS_RELOADING: Id = 0x79DA;
    pub const IS_CROUCHING: Id = 0x4B8F;
    pub const IS_JUMPING: Id = 0x72E1;
    pub const IS_MOVING: Id = 0x4B8E;
    pub const IS_ON_WALL: Id = 0x5201;
    pub const IS_IN_SPAWN_ROOM: Id = 0x4C80;
    pub const HEALTH: Id = 0x374A;
    pub const MAX_HEALTH: Id = 0x374C;
    pub const NORMALIZED_HEALTH: Id = 0x374B;
    pub const ULTIMATE_CHARGE_PERCENT: Id = 0x374D;
    pub const ABILITY_COOLDOWN: Id = 0x7468;
    pub const HAS_STATUS: Id = 0x4C32;
    pub const IS_COMMUNICATING: Id = 0x4B66;
    pub const IS_COMMUNICATING_ANY: Id = 0x5134;
    pub const IS_COMMUNICATING_ANY_EMOTE: Id = 0x5136;
    pub const IS_COMMUNICATING_ANY_SPRAY: Id = 0x8063;
    pub const ALLOWED_HEROES: Id = 0x52C2;
    pub const ALL_HEROES: Id = 0x53C9;
    pub const IS_HERO_BEING_PLAYED: Id = 0x4B9E;
    pub const OPPOSITE_TEAM_OF: Id = 0x5200;
    pub const ENTITY_EXISTS: Id = 0x4EE8;
    pub const LAST_CREATED_ENTITY: Id = 0x4C31;
    pub const LAST_TEXT_ID: Id = 0x51FE;
    pub const ENTITY_COUNT: Id = 0x865B;
    pub const TEXT_COUNT: Id = 0x8657;
    pub const MATCH_TIME: Id = 0x48A4;
    pub const TOTAL_TIME_ELAPSED: Id = 0x4C30;
    pub const CURRENT_MAP: Id = 0x6937;
    pub const CURRENT_GAME_MODE: Id = 0x6C36;
    pub const IS_GAME_IN_PROGRESS: Id = 0x4C2C;
    pub const IS_IN_SETUP: Id = 0x4C2B;
    pub const IS_WAITING_FOR_PLAYERS: Id = 0x4C29;
    pub const IS_ASSEMBLING_HEROES: Id = 0x4C2A;
    pub const SERVER_LOAD: Id = 0x59AC;
    pub const SERVER_LOAD_AVERAGE: Id = 0x59C2;
    pub const SERVER_LOAD_PEAK: Id = 0x59C1;
    pub const STRING_CONTAINS: Id = 0x800E;
    pub const STRING_LENGTH: Id = 0x8147;
    pub const CHAR_IN_STRING: Id = 0x870F;
    pub const STRING_SLICE: Id = 0x8148;
    pub const STRING_REPLACE: Id = 0x8712;
    pub const STRING_SPLIT: Id = 0x8711;
    pub const RAY_CAST_HIT_POSITION: Id = 0x565B;
    pub const RAY_CAST_HIT_NORMAL: Id = 0x56A2;
    pub const RAY_CAST_HIT_PLAYER: Id = 0x569A;
    pub const WORKSHOP_SETTING_TOGGLE: Id = 0x7B01;
    pub const WORKSHOP_SETTING_INTEGER: Id = 0x7B03;
    pub const WORKSHOP_SETTING_REAL: Id = 0x7B05;
    pub const WORKSHOP_SETTING_COMBO: Id = 0x7F4E;
    pub const WORKSHOP_SETTING_HERO: Id = 0x7F4B;
    pub const IS_IN_LINE_OF_SIGHT: Id = 0x4AC3;
    pub const IS_IN_VIEW_ANGLE: Id = 0x543F;

    /// Round To Integer's rounding (enum `555F`).
    pub const ROUND_UP: Id = 0x5562;
    pub const ROUND_DOWN: Id = 0x5561;
    /// World/Local Vector Of's transformation (enum `4C12`).
    pub const ROTATION_AND_TRANSLATION: Id = 0x4C14;
    /// The communication enum (`5123`).
    pub const COMMUNICATION: Id = 0x5123;
}

/// A small deterministic random source (xorshift64*), seeded per engine.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// A number in [0, 1).
    pub fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        bits as f64 / (1u64 << 53) as f64
    }
}

/// Everything a value reads besides the world and the variables.
pub struct Scope<'a> {
    pub catalog: &'a Catalog,
    pub context: &'a Context,
    pub settings: &'a [WorkshopSetting],
    /// Total Time Elapsed, in seconds.
    pub elapsed: f64,
    pub rng: &'a mut Rng,
    /// Values the evaluator has no rule for, by identifier.
    pub unsupported: &'a mut BTreeSet<Id>,
    /// Current Array Element and Index of the innermost per-element value.
    elements: Vec<(Value, usize)>,
}

impl<'a> Scope<'a> {
    pub fn new(
        catalog: &'a Catalog,
        context: &'a Context,
        settings: &'a [WorkshopSetting],
        elapsed: f64,
        rng: &'a mut Rng,
        unsupported: &'a mut BTreeSet<Id>,
    ) -> Self {
        Self {
            catalog,
            context,
            settings,
            elapsed,
            rng,
            unsupported,
            elements: Vec::new(),
        }
    }
}

/// Evaluate one parameter.
pub fn evaluate<H: World + Variables + ?Sized>(scope: &mut Scope, host: &H, arg: &Arg) -> Value {
    Eval { scope, host }.arg(arg)
}

struct Eval<'s, 'a, H: ?Sized> {
    scope: &'s mut Scope<'a>,
    host: &'s H,
}

type Vec3 = [f64; 3];

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: Vec3, k: f64) -> Vec3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

fn normalize(a: Vec3) -> Vec3 {
    let length = length(a);
    if length == 0.0 {
        [0.0; 3]
    } else {
        scale(a, 1.0 / length)
    }
}

/// The Workshop's direction convention (the Vector description: X left, Y
/// up, Z forward): horizontal angle 0 faces +Z and grows towards +X;
/// vertical angle grows downwards [assumption from OverPy's
/// `directionFromAngles` and `horizontalAngleOfDirection`].
fn direction_from_angles(horizontal: f64, vertical: f64) -> Vec3 {
    let (h, v) = (horizontal.to_radians(), vertical.to_radians());
    [h.sin() * v.cos(), -v.sin(), h.cos() * v.cos()]
}

fn horizontal_angle(direction: Vec3) -> f64 {
    direction[0].atan2(direction[2]).to_degrees()
}

fn vertical_angle(direction: Vec3) -> f64 {
    let n = normalize(direction);
    (-n[1]).clamp(-1.0, 1.0).asin().to_degrees()
}

/// The difference of two angles wrapped to (-180, 180].
fn angle_difference(from: f64, to: f64) -> f64 {
    let mut difference = (to - from) % 360.0;
    if difference > 180.0 {
        difference -= 360.0;
    } else if difference <= -180.0 {
        difference += 360.0;
    }
    difference
}

/// A player's local frame: left, up, forward from its horizontal facing
/// [assumption: the frame turns with the player's yaw only].
fn frame(facing: Vec3) -> (Vec3, Vec3, Vec3) {
    let forward = normalize([facing[0], 0.0, facing[2]]);
    let up = [0.0, 1.0, 0.0];
    (cross(up, forward), up, forward)
}

impl<H: World + Variables + ?Sized> Eval<'_, '_, H> {
    fn arg(&mut self, arg: &Arg) -> Value {
        match arg {
            Arg::Value(value) => self.value(value),
            Arg::Team(team) => Value::Team(*team),
            Arg::Hero(hero) => Value::Hero(*hero),
            Arg::Bool(value) => Value::Bool(*value),
            Arg::Int(value) => Value::Number(*value as f64),
            Arg::Number(value) => Value::Number(value.value()),
            Arg::Variable(index) | Arg::Subroutine(index) => Value::Number(f64::from(*index)),
            Arg::CustomString(text) => Value::Text(Text::Custom {
                template: text.clone(),
                args: Vec::new(),
            }),
            Arg::Enum(id) => Value::Enum(*id),
            Arg::PredefinedString(index) => Value::Text(Text::Predefined {
                index: *index,
                args: Vec::new(),
            }),
            Arg::Compare(_) => Value::Null,
            Arg::Array(items) => Value::Array(items.iter().map(|item| self.value(item)).collect()),
            Arg::Map(map) => Value::Map(*map),
            Arg::GameMode(mode) => Value::GameMode(*mode),
            Arg::Button(button) => Value::Button(*button),
            Arg::Flags(flags) => Value::Number(*flags as f64),
            Arg::WaitBehavior(behavior) => Value::Number(f64::from(*behavior)),
        }
    }

    /// Parameter `index` of `value`, evaluated; Null when absent.
    fn at(&mut self, value: &Tree, index: usize) -> Value {
        value
            .args
            .get(index)
            .map_or(Value::Null, |arg| self.arg(arg))
    }

    fn number(&mut self, value: &Tree, index: usize) -> f64 {
        self.at(value, index).number()
    }

    /// A parameter as a position or direction: an entity stands for its
    /// position (the Workshop converts players to positions, e.g.
    /// `Distance Between(Event Player, …)`).
    fn vector(&mut self, value: &Tree, index: usize) -> Vec3 {
        let value = self.at(value, index);
        self.as_vector(&value)
    }

    fn as_vector(&self, value: &Value) -> Vec3 {
        as_vector(self.host, value)
    }

    fn player(&mut self, value: &Tree, index: usize) -> Option<EntityId> {
        self.at(value, index).entity()
    }

    fn players(&mut self, value: &Tree, index: usize) -> Vec<EntityId> {
        self.at(value, index).entities()
    }

    /// The players of a Team value: every player for All Teams.
    fn team_players(&mut self, value: &Tree, index: usize) -> Vec<EntityId> {
        let team = self.at(value, index);
        let players = self.host.players();
        match team {
            Value::Team(0) => players,
            Value::Team(team) => players
                .into_iter()
                .filter(|&player| self.host.team_of(player) == team)
                .collect(),
            _ => players,
        }
    }

    fn per_player(&mut self, value: &Tree, read: impl Fn(&H, EntityId) -> Value) -> Value {
        match self.player(value, 0) {
            Some(player) => read(self.host, player),
            None => Value::Null,
        }
    }

    fn activity(&mut self, value: &Tree, activity: Activity) -> Value {
        self.per_player(value, |host, player| {
            Value::Bool(host.activity(player, activity))
        })
    }

    fn text(&mut self, value: &Tree, index: usize) -> String {
        let value = self.at(value, index);
        match value {
            Value::Text(text) => text.render(self.scope.catalog),
            _ => String::new(),
        }
    }

    /// Evaluate a per-element parameter for each element of an array.
    fn each(&mut self, items: &[Value], lambda: &Tree, index: usize) -> Vec<Value> {
        items
            .iter()
            .enumerate()
            .map(|(position, item)| {
                self.scope.elements.push((item.clone(), position));
                let result = self.at(lambda, index);
                self.scope.elements.pop();
                result
            })
            .collect()
    }

    fn value(&mut self, value: &Tree) -> Value {
        use ids::*;
        let host = self.host;
        let context = self.scope.context;
        let entity = |entity: Option<EntityId>| entity.map_or(Value::Null, Value::Entity);
        match value.id {
            NUMBER => self.at(value, 0),
            TRUE => Value::Bool(true),
            FALSE => Value::Bool(false),
            NULL => Value::Null,
            TEAM | HERO | MAP | GAME_MODE | BUTTON => self.at(value, 0),
            COLOR => match self.at(value, 0) {
                Value::Enum(id) => Value::Color(Color::Named(id)),
                other => other,
            },
            CUSTOM_COLOR => Value::Color(Color::Rgba([
                self.number(value, 0),
                self.number(value, 1),
                self.number(value, 2),
                self.number(value, 3),
            ])),
            VECTOR => Value::Vector([
                self.number(value, 0),
                self.number(value, 1),
                self.number(value, 2),
            ]),
            UP => Value::Vector([0.0, 1.0, 0.0]),
            DOWN => Value::Vector([0.0, -1.0, 0.0]),
            LEFT => Value::Vector([1.0, 0.0, 0.0]),
            RIGHT => Value::Vector([-1.0, 0.0, 0.0]),
            FORWARD => Value::Vector([0.0, 0.0, 1.0]),
            BACKWARD => Value::Vector([0.0, 0.0, -1.0]),
            EMPTY_ARRAY => Value::Array(Vec::new()),
            ARRAY => self.at(value, 0),
            CUSTOM_STRING => {
                let template = match value.args.first() {
                    Some(Arg::CustomString(text)) => text.clone(),
                    _ => String::new(),
                };
                let args = (1..value.args.len()).map(|i| self.at(value, i)).collect();
                Value::Text(Text::Custom { template, args })
            }
            STRING => {
                let index = match value.args.first() {
                    Some(Arg::PredefinedString(index)) => *index,
                    _ => 0,
                };
                // Parameter 1 is hidden; {0}-{2} follow.
                let args = (2..value.args.len()).map(|i| self.at(value, i)).collect();
                Value::Text(Text::Predefined { index, args })
            }
            INPUT_BINDING_STRING => match self.at(value, 0) {
                Value::Button(button) => Value::Text(Text::InputBinding(button)),
                _ => Value::Null,
            },
            ABILITY_ICON_STRING => match (self.at(value, 0), self.at(value, 1)) {
                (Value::Hero(hero), Value::Button(button)) => {
                    Value::Text(Text::AbilityIcon { hero, button })
                }
                _ => Value::Null,
            },
            ICON_STRING => match self.at(value, 0) {
                Value::Enum(icon) => Value::Text(Text::Icon(icon)),
                _ => Value::Null,
            },
            HERO_ICON_STRING => match self.at(value, 0) {
                Value::Hero(hero) => Value::Text(Text::HeroIcon(hero)),
                _ => Value::Null,
            },
            GLOBAL_VARIABLE => match value.args.first() {
                Some(Arg::Variable(index)) => host.global(*index),
                _ => Value::Null,
            },
            PLAYER_VARIABLE => match (self.player(value, 0), value.args.get(1)) {
                (Some(player), Some(Arg::Variable(index))) => host.player(player, *index),
                _ => Value::Null,
            },
            EVENT_PLAYER => entity(context.event_player),
            ATTACKER => entity(context.attacker),
            VICTIM => entity(context.victim),
            HEALER => entity(context.healer),
            HEALEE => entity(context.healee),
            EVENT_DAMAGE => Value::Number(context.event_damage),
            EVENT_HEALING => Value::Number(context.event_healing),
            EVENT_WAS_CRITICAL_HIT => Value::Bool(context.event_was_critical_hit),
            EVENT_WAS_HEALTH_PACK => Value::Bool(context.event_was_health_pack),
            EVENT_WAS_ENVIRONMENT => Value::Bool(context.event_was_environment),
            EVENT_ABILITY => context.event_ability.map_or(Value::Null, Value::Button),
            EVENT_DIRECTION => Value::Vector(context.event_direction),
            LOCAL_PLAYER => entity(context.local_player),
            HOST_PLAYER => entity(host.host_player()),
            CURRENT_ARRAY_ELEMENT => self
                .scope
                .elements
                .last()
                .map_or(Value::Null, |(element, _)| element.clone()),
            CURRENT_ARRAY_INDEX => self
                .scope
                .elements
                .last()
                .map_or(Value::Null, |&(_, index)| Value::Number(index as f64)),
            // Evaluated once here; re-evaluating actions keep the first
            // value on the VM side.
            EVALUATE_ONCE => self.at(value, 1),
            UPDATE_EVERY_FRAME => self.at(value, 0),

            ADD | SUBTRACT | MULTIPLY | DIVIDE | MODULO | RAISE_TO_POWER | MIN | MAX => {
                let (a, b) = (self.at(value, 0), self.at(value, 1));
                arithmetic(host, value.id, &a, &b)
            }
            AND => Value::Bool(self.at(value, 0).truthy() && self.at(value, 1).truthy()),
            OR => Value::Bool(self.at(value, 0).truthy() || self.at(value, 1).truthy()),
            NOT => Value::Bool(!self.at(value, 0).truthy()),
            COMPARE => {
                let (a, b) = (self.at(value, 0), self.at(value, 2));
                let op = match value.args.get(1) {
                    Some(Arg::Compare(op)) => *op,
                    _ => crate::logic::Compare::Eq,
                };
                Value::Bool(compare(&a, op, &b))
            }
            IF_THEN_ELSE => {
                if self.at(value, 0).truthy() {
                    self.at(value, 1)
                } else {
                    self.at(value, 2)
                }
            }
            ABSOLUTE_VALUE => Value::Number(self.number(value, 0).abs()),
            SQUARE_ROOT => Value::Number(self.number(value, 0).max(0.0).sqrt()),
            ROUND_TO_INTEGER => {
                let number = self.number(value, 0);
                Value::Number(match value.args.get(1) {
                    Some(Arg::Enum(ROUND_UP)) => number.ceil(),
                    Some(Arg::Enum(ROUND_DOWN)) => number.floor(),
                    _ => number.round(),
                })
            }
            SINE_FROM_DEGREES => Value::Number(self.number(value, 0).to_radians().sin()),
            SINE_FROM_RADIANS => Value::Number(self.number(value, 0).sin()),
            COSINE_FROM_DEGREES => Value::Number(self.number(value, 0).to_radians().cos()),
            COSINE_FROM_RADIANS => Value::Number(self.number(value, 0).cos()),
            TANGENT_FROM_DEGREES => Value::Number(self.number(value, 0).to_radians().tan()),
            TANGENT_FROM_RADIANS => Value::Number(self.number(value, 0).tan()),
            ARCSINE_IN_DEGREES => {
                Value::Number(self.number(value, 0).clamp(-1.0, 1.0).asin().to_degrees())
            }
            ARCSINE_IN_RADIANS => Value::Number(self.number(value, 0).clamp(-1.0, 1.0).asin()),
            ARCCOSINE_IN_DEGREES => {
                Value::Number(self.number(value, 0).clamp(-1.0, 1.0).acos().to_degrees())
            }
            ARCCOSINE_IN_RADIANS => Value::Number(self.number(value, 0).clamp(-1.0, 1.0).acos()),
            ARCTANGENT_IN_DEGREES => Value::Number(
                self.number(value, 0)
                    .atan2(self.number(value, 1))
                    .to_degrees(),
            ),
            ARCTANGENT_IN_RADIANS => {
                Value::Number(self.number(value, 0).atan2(self.number(value, 1)))
            }
            RANDOM_INTEGER => {
                let (min, max) = (self.number(value, 0).round(), self.number(value, 1).round());
                let (low, high) = (min.min(max), min.max(max));
                let pick = low + (self.scope.rng.unit() * (high - low + 1.0)).floor();
                Value::Number(pick.min(high))
            }
            RANDOM_REAL => {
                let (min, max) = (self.number(value, 0), self.number(value, 1));
                Value::Number(min + self.scope.rng.unit() * (max - min))
            }
            RANDOM_VALUE_IN_ARRAY => {
                let items = self.at(value, 0).into_array();
                if items.is_empty() {
                    Value::Null
                } else {
                    let pick = (self.scope.rng.unit() * items.len() as f64) as usize;
                    items[pick.min(items.len() - 1)].clone()
                }
            }
            RANDOMIZED_ARRAY => {
                let mut items = self.at(value, 0).into_array();
                for i in (1..items.len()).rev() {
                    let j = (self.scope.rng.unit() * (i + 1) as f64) as usize;
                    items.swap(i, j.min(i));
                }
                Value::Array(items)
            }

            X_COMPONENT_OF => Value::Number(self.vector(value, 0)[0]),
            Y_COMPONENT_OF => Value::Number(self.vector(value, 0)[1]),
            Z_COMPONENT_OF => Value::Number(self.vector(value, 0)[2]),
            DISTANCE_BETWEEN => {
                Value::Number(length(sub(self.vector(value, 1), self.vector(value, 0))))
            }
            DIRECTION_TOWARDS => {
                Value::Vector(normalize(sub(self.vector(value, 1), self.vector(value, 0))))
            }
            VECTOR_TOWARDS => Value::Vector(sub(self.vector(value, 1), self.vector(value, 0))),
            NORMALIZE => Value::Vector(normalize(self.vector(value, 0))),
            MAGNITUDE_OF => Value::Number(length(self.vector(value, 0))),
            DOT_PRODUCT => Value::Number(dot(self.vector(value, 0), self.vector(value, 1))),
            CROSS_PRODUCT => Value::Vector(cross(self.vector(value, 0), self.vector(value, 1))),
            DIRECTION_FROM_ANGLES => Value::Vector(direction_from_angles(
                self.number(value, 0),
                self.number(value, 1),
            )),
            HORIZONTAL_ANGLE_FROM_DIRECTION => {
                Value::Number(horizontal_angle(self.vector(value, 0)))
            }
            VERTICAL_ANGLE_FROM_DIRECTION => Value::Number(vertical_angle(self.vector(value, 0))),
            ANGLE_BETWEEN_VECTORS => {
                let (a, b) = (
                    normalize(self.vector(value, 0)),
                    normalize(self.vector(value, 1)),
                );
                Value::Number(dot(a, b).clamp(-1.0, 1.0).acos().to_degrees())
            }
            ANGLE_DIFFERENCE => Value::Number(angle_difference(
                self.number(value, 0),
                self.number(value, 1),
            )),
            WORLD_VECTOR_OF | LOCAL_VECTOR_OF => {
                let vector = self.vector(value, 0);
                let Some(player) = self.player(value, 1) else {
                    return Value::Null;
                };
                let translate =
                    matches!(value.args.get(2), Some(Arg::Enum(ROTATION_AND_TRANSLATION)));
                let (left, up, forward) = frame(host.facing(player));
                let origin = host.position(player);
                Value::Vector(if value.id == WORLD_VECTOR_OF {
                    let world = add(
                        add(scale(left, vector[0]), scale(up, vector[1])),
                        scale(forward, vector[2]),
                    );
                    if translate { add(world, origin) } else { world }
                } else {
                    let local = if translate {
                        sub(vector, origin)
                    } else {
                        vector
                    };
                    [dot(local, left), dot(local, up), dot(local, forward)]
                })
            }
            HORIZONTAL_ANGLE_TOWARDS | VERTICAL_ANGLE_TOWARDS => {
                let Some(player) = self.player(value, 0) else {
                    return Value::Null;
                };
                let towards = sub(self.vector(value, 1), host.eye_position(player));
                let facing = host.facing(player);
                Value::Number(if value.id == HORIZONTAL_ANGLE_TOWARDS {
                    angle_difference(horizontal_angle(facing), horizontal_angle(towards))
                } else {
                    vertical_angle(towards) - vertical_angle(facing)
                })
            }
            NEAREST_WALKABLE_POSITION => {
                Value::Vector(host.nearest_walkable_position(self.vector(value, 0)))
            }

            VALUE_IN_ARRAY => {
                let items = self.at(value, 0).into_array();
                let index = self.number(value, 1);
                if index < 0.0 {
                    Value::Null
                } else {
                    items.into_iter().nth(index as usize).unwrap_or(Value::Null)
                }
            }
            COUNT_OF => Value::Number(self.at(value, 0).into_array().len() as f64),
            APPEND_TO_ARRAY => {
                let mut items = self.at(value, 0).into_array();
                items.extend(self.at(value, 1).into_array());
                Value::Array(items)
            }
            REMOVE_FROM_ARRAY => {
                let items = self.at(value, 0).into_array();
                let removed = self.at(value, 1).into_array();
                Value::Array(
                    items
                        .into_iter()
                        .filter(|item| !removed.iter().any(|gone| gone.equals(item)))
                        .collect(),
                )
            }
            ARRAY_CONTAINS => {
                let items = self.at(value, 0).into_array();
                let wanted = self.at(value, 1);
                Value::Bool(items.iter().any(|item| item.equals(&wanted)))
            }
            // An array that is not among the items stands for its first
            // element, as a value where one is wanted [inference: C32S2's
            // CP Clear finds the checkpoint it landed in with Index Of
            // Array Value(Local_Pos, Filtered Array(Local_Pos, …)); taken
            // whole, it is -1 and the clear sends the player to
            // checkpoint 0].
            INDEX_OF_ARRAY_VALUE => {
                let items = self.at(value, 0).into_array();
                let wanted = self.at(value, 1);
                let find = |wanted: &Value| items.iter().position(|item| item.equals(wanted));
                let found = find(&wanted).or_else(|| match &wanted {
                    Value::Array(within) => within.first().and_then(find),
                    _ => None,
                });
                Value::Number(found.map_or(-1.0, |index| index as f64))
            }
            ARRAY_SLICE => {
                let items = self.at(value, 0).into_array();
                let start = self.number(value, 1).max(0.0) as usize;
                let count = self.number(value, 2).max(0.0) as usize;
                Value::Array(items.into_iter().skip(start).take(count).collect())
            }
            FIRST_OF => self
                .at(value, 0)
                .into_array()
                .into_iter()
                .next()
                .unwrap_or(Value::Null),
            LAST_OF => self.at(value, 0).into_array().pop().unwrap_or(Value::Null),
            FILTERED_ARRAY | IS_TRUE_FOR_ANY | IS_TRUE_FOR_ALL => {
                let items = self.at(value, 0).into_array();
                let verdicts: Vec<bool> = self
                    .each(&items, value, 1)
                    .iter()
                    .map(Value::truthy)
                    .collect();
                match value.id {
                    FILTERED_ARRAY => Value::Array(
                        items
                            .into_iter()
                            .zip(&verdicts)
                            .filter_map(|(item, &keep)| keep.then_some(item))
                            .collect(),
                    ),
                    IS_TRUE_FOR_ANY => Value::Bool(verdicts.iter().any(|&verdict| verdict)),
                    _ => Value::Bool(verdicts.iter().all(|&verdict| verdict)),
                }
            }
            SORTED_ARRAY => {
                let items = self.at(value, 0).into_array();
                let ranks: Vec<f64> = self
                    .each(&items, value, 1)
                    .iter()
                    .map(Value::number)
                    .collect();
                let mut order: Vec<usize> = (0..items.len()).collect();
                order.sort_by(|&a, &b| ranks[a].total_cmp(&ranks[b]));
                Value::Array(
                    order
                        .into_iter()
                        .map(|index| items[index].clone())
                        .collect(),
                )
            }
            MAPPED_ARRAY => {
                let items = self.at(value, 0).into_array();
                Value::Array(self.each(&items, value, 1))
            }

            ALL_PLAYERS => entities(self.team_players(value, 0)),
            ALL_LIVING_PLAYERS
            | ALL_DEAD_PLAYERS
            | NUMBER_OF_LIVING_PLAYERS
            | NUMBER_OF_DEAD_PLAYERS => {
                let living = matches!(value.id, ALL_LIVING_PLAYERS | NUMBER_OF_LIVING_PLAYERS);
                let players: Vec<EntityId> = self
                    .team_players(value, 0)
                    .into_iter()
                    .filter(|&player| host.is_alive(player) == living)
                    .collect();
                if matches!(value.id, ALL_LIVING_PLAYERS | ALL_DEAD_PLAYERS) {
                    entities(players)
                } else {
                    Value::Number(players.len() as f64)
                }
            }
            NUMBER_OF_PLAYERS => Value::Number(self.team_players(value, 0).len() as f64),
            PLAYERS_IN_SLOT => {
                let slot = self.number(value, 0) as i32;
                let team = self.at(value, 1);
                let players: Vec<EntityId> = self
                    .team_players(value, 1)
                    .into_iter()
                    .filter(|&player| host.slot_of(player) == slot)
                    .collect();
                // One team: its player or Null; All Teams: an array.
                if matches!(team, Value::Team(1 | 2)) {
                    players
                        .first()
                        .map_or(Value::Null, |&player| Value::Entity(player))
                } else {
                    entities(players)
                }
            }
            PLAYERS_ON_HERO => {
                let hero = self.at(value, 0);
                let players = self
                    .team_players(value, 1)
                    .into_iter()
                    .filter(|&player| host.hero_of(player).map(Value::Hero).as_ref() == Some(&hero))
                    .collect();
                entities(players)
            }
            PLAYERS_WITHIN_RADIUS => {
                let center = self.vector(value, 0);
                let radius = self.number(value, 1);
                // The line-of-sight options other than Off check the eye
                // positions against the world.
                let line_of_sight = !matches!(value.args.get(3), Some(Arg::Enum(0x4ABC)) | None);
                let players = self
                    .team_players(value, 2)
                    .into_iter()
                    .filter(|&player| host.is_alive(player))
                    .filter(|&player| length(sub(host.position(player), center)) <= radius)
                    .filter(|&player| {
                        !line_of_sight
                            || host.is_in_line_of_sight(center, host.eye_position(player))
                    })
                    .collect();
                entities(players)
            }
            CLOSEST_PLAYER_TO | FARTHEST_PLAYER_FROM => {
                let center = self.vector(value, 0);
                let players = self.team_players(value, 1);
                let distance = |player: &EntityId| length(sub(host.position(*player), center));
                let pick = if value.id == CLOSEST_PLAYER_TO {
                    players
                        .into_iter()
                        .filter(|&p| host.is_alive(p))
                        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
                } else {
                    players
                        .into_iter()
                        .filter(|&p| host.is_alive(p))
                        .max_by(|a, b| distance(a).total_cmp(&distance(b)))
                };
                entity(pick)
            }
            TEAM_OF => self.per_player(value, |host, player| Value::Team(host.team_of(player))),
            SLOT_OF => self.per_player(value, |host, player| {
                Value::Number(f64::from(host.slot_of(player)))
            }),
            HERO_OF => self.per_player(value, |host, player| {
                host.hero_of(player).map_or(Value::Null, Value::Hero)
            }),
            POSITION_OF => match self.at(value, 0) {
                Value::Entity(entity) => Value::Vector(host.position(entity)),
                Value::Array(items) => Value::Array(
                    items
                        .iter()
                        .map(|item| {
                            item.entity()
                                .map_or(Value::Null, |e| Value::Vector(host.position(e)))
                        })
                        .collect(),
                ),
                _ => Value::Null,
            },
            EYE_POSITION => self.per_player(value, |host, player| {
                Value::Vector(host.eye_position(player))
            }),
            FACING_DIRECTION_OF => {
                self.per_player(value, |host, player| Value::Vector(host.facing(player)))
            }
            HORIZONTAL_FACING_ANGLE_OF => self.per_player(value, |host, player| {
                Value::Number(horizontal_angle(host.facing(player)))
            }),
            VERTICAL_FACING_ANGLE_OF => self.per_player(value, |host, player| {
                Value::Number(vertical_angle(host.facing(player)))
            }),
            VELOCITY_OF => {
                self.per_player(value, |host, player| Value::Vector(host.velocity(player)))
            }
            SPEED_OF => self.per_player(value, |host, player| {
                Value::Number(length(host.velocity(player)))
            }),
            HORIZONTAL_SPEED_OF => self.per_player(value, |host, player| {
                let v = host.velocity(player);
                Value::Number(v[0].hypot(v[2]))
            }),
            VERTICAL_SPEED_OF => self.per_player(value, |host, player| {
                Value::Number(host.velocity(player)[1])
            }),
            SPEED_OF_IN_DIRECTION => {
                let direction = normalize(self.vector(value, 1));
                self.per_player(value, |host, player| {
                    Value::Number(dot(host.velocity(player), direction))
                })
            }
            THROTTLE_OF => {
                self.per_player(value, |host, player| Value::Vector(host.throttle(player)))
            }
            ALTITUDE_OF => {
                self.per_player(value, |host, player| Value::Number(host.altitude(player)))
            }
            IS_BUTTON_HELD => {
                let Value::Button(button) = self.at(value, 1) else {
                    return Value::Bool(false);
                };
                self.per_player(value, |host, player| {
                    Value::Bool(host.is_button_held(player, button))
                })
            }
            IS_ON_GROUND => {
                self.per_player(value, |host, player| Value::Bool(host.is_on_ground(player)))
            }
            IS_IN_AIR => self.per_player(value, |host, player| {
                Value::Bool(host.is_alive(player) && !host.is_on_ground(player))
            }),
            IS_ALIVE => self.per_player(value, |host, player| Value::Bool(host.is_alive(player))),
            IS_DEAD => self.per_player(value, |host, player| Value::Bool(!host.is_alive(player))),
            HAS_SPAWNED => {
                self.per_player(value, |host, player| Value::Bool(host.has_spawned(player)))
            }
            IS_DUMMY_BOT => {
                self.per_player(value, |host, player| Value::Bool(host.is_dummy_bot(player)))
            }
            IS_FIRING_PRIMARY => self.activity(value, Activity::FiringPrimary),
            IS_FIRING_SECONDARY => self.activity(value, Activity::FiringSecondary),
            IS_USING_ABILITY_1 => self.activity(value, Activity::UsingAbility1),
            IS_USING_ABILITY_2 => self.activity(value, Activity::UsingAbility2),
            IS_USING_ULTIMATE => self.activity(value, Activity::UsingUltimate),
            IS_MELEEING => self.activity(value, Activity::Meleeing),
            IS_RELOADING => self.activity(value, Activity::Reloading),
            IS_CROUCHING => self.activity(value, Activity::Crouching),
            IS_JUMPING => self.activity(value, Activity::Jumping),
            IS_ON_WALL => self.activity(value, Activity::OnWall),
            IS_IN_SPAWN_ROOM => self.activity(value, Activity::InSpawnRoom),
            IS_MOVING => self.per_player(value, |host, player| {
                Value::Bool(length(host.velocity(player)) > 0.0)
            }),
            HEALTH => self.per_player(value, |host, player| Value::Number(host.health(player))),
            MAX_HEALTH => {
                self.per_player(value, |host, player| Value::Number(host.max_health(player)))
            }
            NORMALIZED_HEALTH => self.per_player(value, |host, player| {
                let max = host.max_health(player);
                Value::Number(if max > 0.0 {
                    host.health(player) / max
                } else {
                    0.0
                })
            }),
            ULTIMATE_CHARGE_PERCENT => self.per_player(value, |host, player| {
                Value::Number(host.ultimate_charge_percent(player))
            }),
            ABILITY_COOLDOWN => {
                let Value::Button(button) = self.at(value, 1) else {
                    return Value::Number(0.0);
                };
                self.per_player(value, |host, player| {
                    Value::Number(host.ability_cooldown(player, button))
                })
            }
            HAS_STATUS => {
                let Some(Arg::Enum(status)) = value.args.get(1).cloned() else {
                    return Value::Bool(false);
                };
                self.per_player(value, |host, player| {
                    Value::Bool(host.has_status(player, status))
                })
            }
            IS_COMMUNICATING => {
                let Some(Arg::Enum(kind)) = value.args.get(1).cloned() else {
                    return Value::Bool(false);
                };
                self.per_player(value, |host, player| {
                    Value::Bool(host.is_communicating(player, Some(kind)))
                })
            }
            IS_COMMUNICATING_ANY => self.per_player(value, |host, player| {
                Value::Bool(host.is_communicating(player, None))
            }),
            IS_COMMUNICATING_ANY_EMOTE | IS_COMMUNICATING_ANY_SPRAY => {
                let prefix = if value.id == IS_COMMUNICATING_ANY_EMOTE {
                    "Emote"
                } else {
                    "Spray"
                };
                let kinds: Vec<Id> = self
                    .scope
                    .catalog
                    .enum_def(COMMUNICATION)
                    .map(|def| {
                        def.values
                            .iter()
                            .filter(|(_, name)| name.starts_with(prefix))
                            .map(|&(id, _)| id)
                            .collect()
                    })
                    .unwrap_or_default();
                self.per_player(value, |host, player| {
                    Value::Bool(
                        kinds
                            .iter()
                            .any(|&kind| host.is_communicating(player, Some(kind))),
                    )
                })
            }
            ALLOWED_HEROES => self.per_player(value, |host, player| {
                Value::Array(
                    host.allowed_heroes(player)
                        .into_iter()
                        .map(Value::Hero)
                        .collect(),
                )
            }),
            ALL_HEROES => Value::Array(host.all_heroes().into_iter().map(Value::Hero).collect()),
            IS_HERO_BEING_PLAYED => {
                let hero = self.at(value, 0);
                let players = self.team_players(value, 1);
                Value::Bool(
                    players.into_iter().any(|player| {
                        host.hero_of(player).map(Value::Hero).as_ref() == Some(&hero)
                    }),
                )
            }
            OPPOSITE_TEAM_OF => match self.at(value, 0) {
                Value::Team(1) => Value::Team(2),
                Value::Team(2) => Value::Team(1),
                other => other,
            },
            ENTITY_EXISTS => match self.at(value, 0).entity() {
                Some(entity) => Value::Bool(host.entity_exists(entity)),
                None => Value::Bool(false),
            },
            LAST_CREATED_ENTITY => host.last_created_entity(context.event_player),
            LAST_TEXT_ID => host.last_text_id(context.event_player),
            ENTITY_COUNT => Value::Number(host.entity_count() as f64),
            TEXT_COUNT => Value::Number(host.text_count() as f64),
            MATCH_TIME => Value::Number(host.match_time()),
            TOTAL_TIME_ELAPSED => Value::Number(self.scope.elapsed),
            CURRENT_MAP => Value::Map(host.current_map()),
            CURRENT_GAME_MODE => Value::GameMode(host.current_game_mode()),
            IS_GAME_IN_PROGRESS => Value::Bool(host.is_game_in_progress()),
            IS_IN_SETUP => Value::Bool(host.is_in_setup()),
            IS_WAITING_FOR_PLAYERS => Value::Bool(host.is_waiting_for_players()),
            IS_ASSEMBLING_HEROES => Value::Bool(host.is_assembling_heroes()),
            SERVER_LOAD => Value::Number(host.server_load()[0]),
            SERVER_LOAD_AVERAGE => Value::Number(host.server_load()[1]),
            SERVER_LOAD_PEAK => Value::Number(host.server_load()[2]),

            STRING_CONTAINS => {
                let (text, part) = (self.text(value, 0), self.text(value, 1));
                Value::Bool(text.contains(&part))
            }
            STRING_LENGTH => Value::Number(self.text(value, 0).chars().count() as f64),
            CHAR_IN_STRING => {
                let text = self.text(value, 0);
                let index = self.number(value, 1);
                literal_text(if index < 0.0 {
                    String::new()
                } else {
                    text.chars()
                        .nth(index as usize)
                        .map(String::from)
                        .unwrap_or_default()
                })
            }
            STRING_SLICE => {
                let text = self.text(value, 0);
                let start = self.number(value, 1).max(0.0) as usize;
                let count = self.number(value, 2).max(0.0) as usize;
                literal_text(text.chars().skip(start).take(count).collect())
            }
            STRING_REPLACE => {
                let (text, pattern, replacement) = (
                    self.text(value, 0),
                    self.text(value, 1),
                    self.text(value, 2),
                );
                literal_text(if pattern.is_empty() {
                    text
                } else {
                    text.replace(&pattern, &replacement)
                })
            }
            STRING_SPLIT => {
                let (text, separator) = (self.text(value, 0), self.text(value, 1));
                let parts: Vec<Value> = if separator.is_empty() {
                    text.chars().map(|c| literal_text(c.to_string())).collect()
                } else {
                    text.split(separator.as_str())
                        .map(|part| literal_text(part.into()))
                        .collect()
                };
                Value::Array(parts)
            }
            RAY_CAST_HIT_POSITION | RAY_CAST_HIT_NORMAL | RAY_CAST_HIT_PLAYER => {
                let (start, end) = (self.vector(value, 0), self.vector(value, 1));
                let include = self.players(value, 2);
                let exclude = self.players(value, 3);
                let hit = host.ray_cast(start, end, &include, &exclude);
                match value.id {
                    RAY_CAST_HIT_POSITION => Value::Vector(hit.position),
                    RAY_CAST_HIT_NORMAL => Value::Vector(hit.normal),
                    _ => entity(hit.player),
                }
            }
            IS_IN_LINE_OF_SIGHT => {
                let (start, end) = (self.vector(value, 0), self.vector(value, 1));
                Value::Bool(host.is_in_line_of_sight(start, end))
            }
            // Within the view cone of `angle` degrees around the facing
            // direction [assumption: the angle is the cone's full width].
            IS_IN_VIEW_ANGLE => {
                let location = self.vector(value, 1);
                let angle = self.number(value, 2);
                self.per_player(value, |host, player| {
                    let towards = normalize(sub(location, host.eye_position(player)));
                    let between = dot(towards, normalize(host.facing(player)))
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees();
                    Value::Bool(between <= angle / 2.0)
                })
            }
            WORKSHOP_SETTING_TOGGLE
            | WORKSHOP_SETTING_INTEGER
            | WORKSHOP_SETTING_REAL
            | WORKSHOP_SETTING_COMBO
            | WORKSHOP_SETTING_HERO => self.workshop_setting(value),
            id => {
                if self.scope.unsupported.insert(id) {
                    let name = self
                        .scope
                        .catalog
                        .value(id)
                        .map_or("?", |def| def.name.as_str());
                    tracing::warn!(value = %format!("{id:04X}"), name, "Workshop value not evaluated");
                }
                Value::Null
            }
        }
    }

    /// A Workshop setting by its name (parameter 2), else its default
    /// (parameter 3). The text's `workshop` block writes toggles as On/Off
    /// and numbers as numbers; a combo's value is its option index
    /// [assumption: options are matched by position only].
    fn workshop_setting(&mut self, value: &Tree) -> Value {
        let name = self.text(value, 2);
        let default = self.at(value, 3);
        let Some(found) = self
            .scope
            .settings
            .iter()
            .find(|setting| setting.name == name)
        else {
            return default;
        };
        let text = found.value.trim();
        match value.id {
            ids::WORKSHOP_SETTING_TOGGLE => Value::Bool(
                matches!(
                    text.to_ascii_lowercase().as_str(),
                    "on" | "true" | "yes" | "enabled" | "1"
                ) || text.parse::<f64>().is_ok_and(|n| n != 0.0),
            ),
            ids::WORKSHOP_SETTING_HERO => {
                self.scope.catalog.hero(text).map_or(default, Value::Hero)
            }
            _ => text.parse::<f64>().map_or(default, Value::Number),
        }
    }
}

fn entities(players: Vec<EntityId>) -> Value {
    Value::Array(players.into_iter().map(Value::Entity).collect())
}

fn literal_text(text: String) -> Value {
    Value::Text(Text::Custom {
        template: text.replace('{', "{{").replace('}', "}}"),
        args: Vec::new(),
    })
}

/// A literal as written: `f32` widened through its shortest decimal, so
/// `0.016` stays 0.016.
pub(crate) fn widen(number: f32) -> f64 {
    number.to_string().parse().unwrap_or(f64::from(number))
}

/// A value as a position or direction: an entity stands for its position.
fn as_vector<H: World + ?Sized>(host: &H, value: &Value) -> Vec3 {
    match value {
        Value::Vector(vector) => *vector,
        Value::Entity(entity) => host.position(*entity),
        Value::Array(items) => items.first().map_or([0.0; 3], |item| as_vector(host, item)),
        _ => [0.0; 3],
    }
}

/// Add through Max on evaluated operands (also the Modify Variable
/// operations): numbers, or vectors when either side is one (an entity
/// counts as its position). Division and modulo by zero give zero and a
/// negative base gives zero (the values' descriptions); the remainder
/// takes the dividend's sign [assumption].
pub(crate) fn arithmetic<H: World + ?Sized>(host: &H, id: Id, a: &Value, b: &Value) -> Value {
    use ids::*;
    let quotient = |a: f64, b: f64| if b == 0.0 { 0.0 } else { a / b };
    match (id, a, b) {
        (ADD | SUBTRACT, Value::Vector(_) | Value::Entity(_), _)
        | (ADD | SUBTRACT, _, Value::Vector(_)) => {
            let sign = if id == ADD { 1.0 } else { -1.0 };
            Value::Vector(add(as_vector(host, a), scale(as_vector(host, b), sign)))
        }
        (ADD, a, b) => Value::Number(a.number() + b.number()),
        (SUBTRACT, a, b) => Value::Number(a.number() - b.number()),
        (MULTIPLY, Value::Vector(a), Value::Vector(b)) => {
            Value::Vector([a[0] * b[0], a[1] * b[1], a[2] * b[2]])
        }
        (MULTIPLY, Value::Vector(v), k) | (MULTIPLY, k, Value::Vector(v)) => {
            Value::Vector(scale(*v, k.number()))
        }
        (MULTIPLY, a, b) => Value::Number(a.number() * b.number()),
        (DIVIDE, Value::Vector(a), Value::Vector(b)) => Value::Vector([
            quotient(a[0], b[0]),
            quotient(a[1], b[1]),
            quotient(a[2], b[2]),
        ]),
        (DIVIDE, Value::Vector(v), k) => {
            let k = k.number();
            Value::Vector(v.map(|c| quotient(c, k)))
        }
        (DIVIDE, a, b) => Value::Number(quotient(a.number(), b.number())),
        (MODULO, a, b) => {
            let (a, b) = (a.number(), b.number());
            Value::Number(if b == 0.0 { 0.0 } else { a % b })
        }
        (RAISE_TO_POWER, a, b) => {
            let (a, b) = (a.number(), b.number());
            Value::Number(if a < 0.0 { 0.0 } else { a.powf(b) })
        }
        (MIN, a, b) => Value::Number(a.number().min(b.number())),
        (MAX, a, b) => Value::Number(a.number().max(b.number())),
        _ => Value::Null,
    }
}

/// Compare: `==` and `!=` by Workshop equality, the rest as numbers.
pub fn compare(a: &Value, op: crate::logic::Compare, b: &Value) -> bool {
    use crate::logic::Compare::*;
    match op {
        Eq => a.equals(b),
        Ne => !a.equals(b),
        Lt => a.number() < b.number(),
        Le => a.number() <= b.number(),
        Gt => a.number() > b.number(),
        Ge => a.number() >= b.number(),
    }
}

/// A value as text, for logs and tests.
pub fn describe(value: &Value, catalog: &Catalog) -> String {
    display(value, catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angles_follow_the_workshop_axes() {
        let forward = direction_from_angles(0.0, 0.0);
        assert!((forward[2] - 1.0).abs() < 1e-9);
        let left = direction_from_angles(90.0, 0.0);
        assert!((left[0] - 1.0).abs() < 1e-9);
        assert!((horizontal_angle(left) - 90.0).abs() < 1e-9);
        let down = direction_from_angles(0.0, 90.0);
        assert!((down[1] + 1.0).abs() < 1e-9);
        assert!((vertical_angle(down) - 90.0).abs() < 1e-9);
        assert_eq!(angle_difference(170.0, -170.0), 20.0);
        assert_eq!(angle_difference(-170.0, 170.0), -20.0);
    }

    #[test]
    fn literals_keep_their_decimal_value() {
        assert_eq!(widen(0.016), 0.016);
        assert_eq!(widen(-2.0), -2.0);
    }
}
