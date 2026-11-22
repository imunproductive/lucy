//! A fake match for the engine tests: players with positions, buttons
//! and ground state, a variable store, and an action log in place of the
//! VM's graphs.

#![allow(dead_code)]

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use lucy_workshop::{
    Catalog,
    catalog::Id,
    host::{ActionCall, ActionHandle, Actions, Activity, Param, RayHit, Variables, World},
    value::{EntityId, Value},
};

/// The catalog subset the tests use, cut from the install's
/// `workshop_catalog_174.json` (every event, condition, action and
/// predefined string; the values and enums the test scripts name).
/// `catalog_subset_is_the_installs` checks it against the install.
pub const CATALOG: &str = include_str!("../data/workshop_catalog_subset.json");

pub fn catalog() -> Arc<Catalog> {
    Arc::new(Catalog::from_reader(CATALOG.as_bytes()).unwrap())
}

/// Button enum values (`4A39`).
pub const PRIMARY_FIRE: Id = 0x4A3A;
pub const SECONDARY_FIRE: Id = 0x4A3B;
pub const ABILITY_1: Id = 0x4A3C;
pub const ABILITY_2: Id = 0x4A3D;
pub const INTERACT: Id = 0x4BAA;

#[derive(Clone, Debug)]
pub struct Player {
    pub id: EntityId,
    pub team: u8,
    pub slot: i32,
    pub position: [f64; 3],
    pub on_ground: bool,
    pub alive: bool,
    pub buttons: HashSet<Id>,
}

impl Player {
    pub fn new(id: EntityId, slot: i32) -> Self {
        Self {
            id,
            team: 1,
            slot,
            position: [0.0; 3],
            on_ground: true,
            alive: true,
            buttons: HashSet::new(),
        }
    }
}

/// One started action: its name and evaluated parameters (the server's
/// value of each, and the parameters as the engine gave them).
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    pub name: String,
    pub params: Vec<Value>,
    pub raw: Vec<Param>,
    pub event_player: Option<EntityId>,
}

pub struct Match {
    pub catalog: Arc<Catalog>,
    pub players: Vec<Player>,
    pub globals: HashMap<u8, Value>,
    pub player_variables: HashMap<(EntityId, u8), Value>,
    pub started: Vec<Started>,
    /// Ticks each started action takes before it is finished (by name);
    /// others finish at once.
    pub durations: HashMap<String, u32>,
    running: HashMap<ActionHandle, u32>,
    next_handle: ActionHandle,
}

impl Match {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Self {
            catalog,
            players: Vec::new(),
            globals: HashMap::new(),
            player_variables: HashMap::new(),
            started: Vec::new(),
            durations: HashMap::new(),
            running: HashMap::new(),
            next_handle: 0,
        }
    }

    pub fn player_mut(&mut self, id: EntityId) -> &mut Player {
        self.players
            .iter_mut()
            .find(|player| player.id == id)
            .unwrap()
    }

    fn player(&self, id: EntityId) -> Option<&Player> {
        self.players.iter().find(|player| player.id == id)
    }

    pub fn global(&self, index: u8) -> Value {
        self.globals
            .get(&index)
            .cloned()
            .unwrap_or(Value::Number(0.0))
    }

    /// The names of the started actions, in order, then cleared.
    pub fn take_started(&mut self) -> Vec<String> {
        self.started.drain(..).map(|started| started.name).collect()
    }

    /// Let running actions finish their time: call once per tick before
    /// the engine's tick.
    pub fn advance(&mut self) {
        for left in self.running.values_mut() {
            *left = left.saturating_sub(1);
        }
    }
}

impl World for Match {
    fn players(&self) -> Vec<EntityId> {
        self.players.iter().map(|player| player.id).collect()
    }
    fn team_of(&self, player: EntityId) -> u8 {
        self.player(player).map_or(0, |player| player.team)
    }
    fn slot_of(&self, player: EntityId) -> i32 {
        self.player(player).map_or(-1, |player| player.slot)
    }
    fn hero_of(&self, _player: EntityId) -> Option<u64> {
        None
    }
    fn is_dummy_bot(&self, _player: EntityId) -> bool {
        false
    }
    fn has_spawned(&self, player: EntityId) -> bool {
        self.player(player).is_some()
    }
    fn is_alive(&self, player: EntityId) -> bool {
        self.player(player).is_some_and(|player| player.alive)
    }
    fn entity_exists(&self, entity: EntityId) -> bool {
        self.player(entity).is_some()
    }
    fn position(&self, entity: EntityId) -> [f64; 3] {
        self.player(entity)
            .map_or([0.0; 3], |player| player.position)
    }
    fn eye_position(&self, player: EntityId) -> [f64; 3] {
        let [x, y, z] = self.position(player);
        [x, y + 1.6, z]
    }
    fn facing(&self, _player: EntityId) -> [f64; 3] {
        [0.0, 0.0, 1.0]
    }
    fn velocity(&self, _player: EntityId) -> [f64; 3] {
        [0.0; 3]
    }
    fn throttle(&self, _player: EntityId) -> [f64; 3] {
        [0.0; 3]
    }
    fn is_button_held(&self, player: EntityId, button: Id) -> bool {
        self.player(player)
            .is_some_and(|player| player.buttons.contains(&button))
    }
    fn is_on_ground(&self, player: EntityId) -> bool {
        self.player(player).is_some_and(|player| player.on_ground)
    }
    fn activity(&self, _player: EntityId, _activity: Activity) -> bool {
        false
    }
    fn altitude(&self, player: EntityId) -> f64 {
        if self.is_on_ground(player) { 0.0 } else { 2.0 }
    }
    fn health(&self, _player: EntityId) -> f64 {
        250.0
    }
    fn max_health(&self, _player: EntityId) -> f64 {
        250.0
    }
    fn ultimate_charge_percent(&self, _player: EntityId) -> f64 {
        0.0
    }
    fn ability_cooldown(&self, _player: EntityId, _button: Id) -> f64 {
        0.0
    }
    fn has_status(&self, _player: EntityId, _status: Id) -> bool {
        false
    }
    fn is_communicating(&self, _player: EntityId, _kind: Option<Id>) -> bool {
        false
    }
    fn allowed_heroes(&self, _player: EntityId) -> Vec<u64> {
        Vec::new()
    }
    fn all_heroes(&self) -> Vec<u64> {
        Vec::new()
    }
    fn host_player(&self) -> Option<EntityId> {
        self.players.first().map(|player| player.id)
    }
    fn match_time(&self) -> f64 {
        0.0
    }
    fn is_game_in_progress(&self) -> bool {
        true
    }
    fn is_in_setup(&self) -> bool {
        false
    }
    fn is_waiting_for_players(&self) -> bool {
        false
    }
    fn is_assembling_heroes(&self) -> bool {
        false
    }
    fn current_map(&self) -> u32 {
        0x165
    }
    fn current_game_mode(&self) -> u64 {
        0
    }
    fn last_created_entity(&self, _creator: Option<EntityId>) -> Value {
        Value::Null
    }
    fn last_text_id(&self, _creator: Option<EntityId>) -> Value {
        Value::Null
    }
    fn entity_count(&self) -> usize {
        0
    }
    fn text_count(&self) -> usize {
        0
    }
    fn server_load(&self) -> [f64; 3] {
        [0.0; 3]
    }
    fn nearest_walkable_position(&self, position: [f64; 3]) -> [f64; 3] {
        position
    }
    fn ray_cast(
        &self,
        _start: [f64; 3],
        end: [f64; 3],
        _include: &[EntityId],
        _exclude: &[EntityId],
    ) -> RayHit {
        RayHit {
            position: end,
            ..RayHit::default()
        }
    }
    fn is_in_line_of_sight(&self, _start: [f64; 3], _end: [f64; 3]) -> bool {
        true
    }
}

impl Variables for Match {
    fn global(&self, index: u8) -> Value {
        self.globals
            .get(&index)
            .cloned()
            .unwrap_or(Value::Number(0.0))
    }
    fn set_global(&mut self, index: u8, value: Value) {
        self.globals.insert(index, value);
    }
    fn player(&self, player: EntityId, index: u8) -> Value {
        self.player_variables
            .get(&(player, index))
            .cloned()
            .unwrap_or(Value::Number(0.0))
    }
    fn set_player(&mut self, player: EntityId, index: u8, value: Value) {
        self.player_variables.insert((player, index), value);
    }
}

impl Actions for Match {
    fn start(&mut self, call: ActionCall) -> ActionHandle {
        let name = self.catalog.action(call.action).unwrap().name.clone();
        let duration = self.durations.get(&name).copied().unwrap_or(0);
        self.started.push(Started {
            name,
            params: call
                .params
                .iter()
                .map(|param| param.value().clone())
                .collect(),
            raw: call.params,
            event_player: call.context.event_player,
        });
        self.next_handle += 1;
        self.running.insert(self.next_handle, duration);
        self.next_handle
    }

    fn finished(&mut self, handle: ActionHandle) -> bool {
        self.running.get(&handle).is_none_or(|&left| left == 0)
    }
}
