//! A match that runs a Workshop script: the rule engine
//! ([`orpheus_workshop::Engine`], the retail server's native part) over
//! the match, and the actions it starts as their graphs on the mode entity
//! (`refs/reports/Workshop.md` §2, §5, "WorkshopRuntime").
//!
//! - **Host.** The actions run in the mode entity's script
//!   ([`ModeRunner::workshop`]: the mode graph, or `0CBB` alone, which
//!   plays `1B4A`, the table the clients evaluate the parameters with).
//!   Assumption from Workshop.md §5: retail's host is the mode entity
//!   (nothing in the graphs needs another; Create HUD Text's scope targets
//!   it, `STU_A3DEF368`).
//! - **Parameters.** An action graph reads `#18739[k]` through
//!   `STU_77B0987E`, which takes only a Workshop value tree (a keyed array:
//!   `229A` the value's `01C` GUID, `4932` its parameters), on the server
//!   and on each client alike. The engine evaluates each parameter, and
//!   [`parameter`] sends it as a tree the clients evaluate to the same
//!   value: Null as Null (`4E63`), a constant inside Number (`4876`, whose
//!   `1B4A` config is its parameter 0), a string as Custom String (`5C14`)
//!   with its values, the
//!   parts each client renders itself as their own values (Input Binding
//!   String, the icon strings), a custom colour as Custom Color. A
//!   parameter that names Local Player, which each client evaluates to
//!   himself, goes as an If-Then-Else chain over the players (`7555`,
//!   Compare `4B77` with `==` `333A`, Local Player `866E`). Variable and
//!   enum parameters stay plain values (Workshop.md "WorkshopRuntime").
//! - **Reevaluation.** A running action whose Reevaluation enum names
//!   parameters (Create HUD Text's "Visible To and String"…) gets them
//!   again each tick when they changed
//!   ([`ModeScript::set_action_parameters`]).
//! - **Variables.** Global variables are the mode entity's Workshop
//!   variables, player variables each player's controller's, where the
//!   chase helper `1CCF` moves them. The engine's own values are kept with
//!   what was written, so a variable reads back as the engine wrote it
//!   (a string, a team) until a graph changes it.
//! - **The world.** Each player's position, aim, buttons, ground and life
//!   come from his body ([`WorkshopView`]); his body's death fires Player
//!   Died, and the match's hits, heals and pushes between the players the
//!   other player events ([`WorkshopRun::player_events`]). The scope
//!   instances the actions open on the players
//!   (Teleport), the messages they send them (Stop Chasing's `0DCF`, in
//!   the step that sent them) and the graphs they play there (Chase
//!   Player Variable's `1CCF`) go to their controllers.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use orpheus_core::tick::TICK_PERIOD;
use orpheus_script::{
    expr::{workshop_tree, workshop_variable},
    mode::{ActionStatus, ModeScript, WorkshopAction},
    vm::ScopeChange,
    wire::Value as Wire,
};
use orpheus_workshop::{
    Catalog, Engine, Logic,
    catalog::{Id, ParamKind},
    eval::ids,
    host::{
        ActionCall, ActionHandle, Actions, Activity, Context, GameEvent, Param, RayHit, Variables,
        World,
    },
    logic::events,
    value::{Color, EntityId, Text, Value},
};

use crate::{
    combat::PlayerEvent, mode_script::ModeRunner, replication::WorldDelivery,
    scene_assets::PreparedScene,
};

/// The `01C` GUID of Workshop identifier `id` (a value, an enum value).
fn identifier(id: Id) -> u64 {
    0x0D80_0000_0000_0000 | u64::from(id)
}

/// Compare's `==` operator (`1B4A`'s Compare config tests parameter 1
/// against `333A`…`333E`, then `==`, `!=`, `<`, `<=`, `>`, else `>=`).
const EQUAL: Id = 0x333A;

/// The team identifiers the clients' `STU_77B0987E` resolves a team item
/// to: All Teams, Team 1, Team 2 (`sub_7FF789C14870`).
const TEAMS: [Id; 3] = [0x4D4E, 0x4D4F, 0x4D50];

/// The mode entity's variables the Create actions write their last id
/// into: Create HUD Text's text id (`1D8A` rid39) and the others' entity
/// (`1CE8`, `1AB1` `STU_0BDBC7B2`).
const LAST_TEXT_ID: u32 = 20765;
const LAST_CREATED_ENTITY: u32 = 20554;

/// The Workshop buttons (`4A39`) and the command bits the client sends
/// for their logical buttons (`orpheus_script::timeline::button_edges`:
/// the Workshop's button table `0x7FF78B585348` gives the same logical
/// numbers, Workshop.md "WorkshopModel").
const BUTTONS: [(Id, u32); 10] = [
    (0x4A3A, 0x001),
    (0x4A3B, 0x002),
    (0x4A3C, 0x008),
    (0x4A3D, 0x010),
    (0x4A3E, 0x020),
    (0x4BAA, 0x004),
    (0x4A3F, 0x040),
    (0x4A40, 0x080),
    (0x6ED8, 0x800),
    (0x6ED7, 0x400),
];

/// A player's body as the Workshop's values see it
/// ([`WorldDelivery::workshop_view`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkshopView {
    pub feet: [f64; 3],
    /// The eye's height above the feet.
    pub eye: f64,
    /// The unit aim direction.
    pub facing: [f64; 3],
    pub velocity: [f64; 3],
    /// The movement input: X left, Z forward.
    pub throttle: [f64; 3],
    /// The command buttons the body runs with.
    pub buttons: u32,
    pub on_ground: bool,
    pub alive: bool,
    pub hero: u64,
    pub health: f64,
    pub max_health: f64,
    /// The ultimate's charge in percent of its cost (#342e of #338e).
    pub ultimate_charge: f64,
}

/// A player of the match as the room gives him each tick.
pub struct WorkshopPlayer<'w> {
    /// His controller entity: the Workshop's player.
    pub controller: u32,
    pub slot: usize,
    /// The room's team: 0 Team 1, 1 Team 2.
    pub team: u8,
    pub world: &'w mut WorldDelivery,
}

/// A started action graph.
struct Running {
    action: WorkshopAction,
    /// The action as written and its context, to evaluate it again.
    call: ActionCall,
    /// The parameters its Reevaluation names, by index.
    reevaluated: Vec<bool>,
    /// The parameters it has now.
    sent: Vec<Wire>,
}

/// A variable as the engine wrote it and as it went to the entity.
struct Stored {
    value: Value,
    wire: Wire,
}

/// The engine of one match and the actions it started.
pub struct WorkshopRun {
    engine: Engine,
    catalog: Arc<Catalog>,
    actions: BTreeMap<ActionHandle, Running>,
    next_handle: ActionHandle,
    globals: HashMap<u8, Stored>,
    players: HashMap<(EntityId, u8), Stored>,
    /// Whether each player's body was alive at the last step.
    alive: HashMap<EntityId, bool>,
    /// The context of each player's death the combat reported since the
    /// last step ([`Self::player_events`]), for his Player Died.
    kills: HashMap<EntityId, Context>,
}

impl WorkshopRun {
    /// The rules of `logic`, ticking with the server.
    pub fn new(logic: &Logic, catalog: Arc<Catalog>, seed: u64) -> Self {
        Self {
            engine: Engine::new(logic, Arc::clone(&catalog), TICK_PERIOD.as_secs_f64(), seed),
            catalog,
            actions: BTreeMap::new(),
            next_handle: 0,
            globals: HashMap::new(),
            players: HashMap::new(),
            alive: HashMap::new(),
            kills: HashMap::new(),
        }
    }

    /// The engine, for its faults and what it doesn't evaluate.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// What the tick's hits and pushes did between the players
    /// ([`PlayerEvent`]), before the tick's [`Self::step`]: Player Dealt
    /// and Took Damage, Player Dealt Final Blow and Earned Elimination,
    /// Player Dealt and Received Healing, Player Dealt and Received
    /// Knockback, each with its context. A killing blow's context waits
    /// for the victim's Player Died ([`Self::fire_deaths`]).
    ///
    /// - Attacker is the player [`PlayerEvent::Damage`] names: the victim
    ///   for his own blow and for a fall nobody knocked him into, as the
    ///   death camera takes it (`combat::DeathCamera`); DPK scripts test
    ///   `Attacker != Event Player` in Player Died for "killed by
    ///   another" (`DoomfistRolloutsKingsRow_1_0_0`). A bot's or a
    ///   placeable's blow fires no Player Dealt Damage, and its Attacker
    ///   is Null.
    /// - Assumption: a health pack's heal is the healee's own (Healer =
    ///   Healee), since Event Was Health Pack belongs to Player Dealt
    ///   Healing as well as Received; another placeable's or a bot's heal
    ///   fires only Player Received Healing, with no Healer.
    /// - Not known on the server: Event Ability (the hit doesn't say which
    ///   ability dealt it; Null), and knockback that isn't an explosion's
    ///   push between players (a graph's knockback rings).
    pub fn player_events(&mut self, happened: &[PlayerEvent]) {
        for event in happened {
            match *event {
                PlayerEvent::Damage {
                    attacker,
                    victim,
                    dealt,
                    critical,
                    environment,
                    direction,
                    killed,
                } => {
                    let context = Context {
                        attacker,
                        victim: Some(victim),
                        event_damage: dealt,
                        event_was_critical_hit: critical,
                        event_was_environment: environment,
                        event_direction: direction.unwrap_or_default(),
                        ..Context::default()
                    };
                    if let Some(attacker) = attacker {
                        self.fire(events::PLAYER_DEALT_DAMAGE, attacker, &context);
                    }
                    self.fire(events::PLAYER_TOOK_DAMAGE, victim, &context);
                    if killed {
                        self.kills.insert(victim, context);
                    }
                }
                PlayerEvent::Elimination {
                    player,
                    victim,
                    final_blow,
                } => {
                    let mut context = self.kills.get(&victim).cloned().unwrap_or_default();
                    context.attacker = Some(player);
                    context.victim = Some(victim);
                    if final_blow {
                        self.fire(events::PLAYER_DEALT_FINAL_BLOW, player, &context);
                    }
                    self.fire(events::PLAYER_EARNED_ELIMINATION, player, &context);
                }
                PlayerEvent::Heal {
                    healer,
                    healee,
                    healed,
                    health_pack,
                } => {
                    let healer = if health_pack { Some(healee) } else { healer };
                    let context = Context {
                        healer,
                        healee: Some(healee),
                        event_healing: healed,
                        event_was_health_pack: health_pack,
                        ..Context::default()
                    };
                    if let Some(healer) = healer {
                        self.fire(events::PLAYER_DEALT_HEALING, healer, &context);
                    }
                    self.fire(events::PLAYER_RECEIVED_HEALING, healee, &context);
                }
                PlayerEvent::Knockback {
                    attacker,
                    victim,
                    push,
                } => {
                    let length = push.iter().map(|axis| axis * axis).sum::<f64>().sqrt();
                    let context = Context {
                        attacker: Some(attacker),
                        victim: Some(victim),
                        event_direction: if length > 0.0 {
                            push.map(|axis| axis / length)
                        } else {
                            [0.0; 3]
                        },
                        ..Context::default()
                    };
                    self.fire(events::PLAYER_DEALT_KNOCKBACK, attacker, &context);
                    self.fire(events::PLAYER_RECEIVED_KNOCKBACK, victim, &context);
                }
            }
        }
    }

    /// Event `kind` for `player`, in `context`.
    fn fire(&mut self, kind: u8, player: EntityId, context: &Context) {
        self.engine.fire(GameEvent {
            kind,
            context: Context {
                event_player: Some(player),
                ..context.clone()
            },
        });
    }

    /// One server tick at `tick`, after the mode entity's: the rules run,
    /// the running actions get their reevaluated parameters, and what the
    /// actions' graphs did to the players reaches them. Nothing while no
    /// graph drives the mode entity.
    pub fn step(
        &mut self,
        tick: u32,
        scene: &PreparedScene,
        mode: &mut ModeRunner,
        players: &mut [WorkshopPlayer<'_>],
    ) {
        let kills = std::mem::take(&mut self.kills);
        let Some(script) = mode.script_mut() else {
            return;
        };
        players.sort_by_key(|player| (player.team, player.slot));
        for player in players.iter_mut() {
            player.world.advance_controller_to(tick);
        }
        let views = views(scene, players);
        self.fire_deaths(players, &views, kills);
        let mut host = Host {
            tick,
            scene,
            catalog: &self.catalog,
            script,
            players,
            views,
            actions: &mut self.actions,
            next_handle: &mut self.next_handle,
            globals: &mut self.globals,
            player_variables: &mut self.players,
        };
        // What the graphs did since the last step (a scope that closed
        // with time).
        host.route();
        self.engine.tick(&mut host);
        host.reevaluate(&mut self.engine);
        host.route();
        host.actions
            .retain(|_, running| host.script.action_alive(running.action));
    }

    /// Player Died for each player whose body was alive at the last step
    /// and is dead now: Event Player and Victim the player, the rest from
    /// the blow that killed him ([`Self::player_events`]). A death no
    /// blow reported has no Attacker.
    fn fire_deaths(
        &mut self,
        players: &[WorkshopPlayer<'_>],
        views: &[Option<WorkshopView>],
        mut kills: HashMap<EntityId, Context>,
    ) {
        for (player, view) in players.iter().zip(views) {
            let Some(view) = view else {
                continue;
            };
            let was = self.alive.insert(player.controller, view.alive);
            if was == Some(true) && !view.alive {
                let context = kills.remove(&player.controller).unwrap_or_default();
                self.fire(
                    events::PLAYER_DIED,
                    player.controller,
                    &Context {
                        victim: Some(player.controller),
                        ..context
                    },
                );
            }
        }
    }
}

/// Each player's body now.
fn views(scene: &PreparedScene, players: &[WorkshopPlayer<'_>]) -> Vec<Option<WorkshopView>> {
    players
        .iter()
        .map(|player| player.world.workshop_view(scene))
        .collect()
}

/// The engine's view of the match for one step.
struct Host<'a, 'w> {
    tick: u32,
    scene: &'a PreparedScene,
    catalog: &'a Catalog,
    script: &'a mut ModeScript,
    players: &'a mut [WorkshopPlayer<'w>],
    views: Vec<Option<WorkshopView>>,
    actions: &'a mut BTreeMap<ActionHandle, Running>,
    next_handle: &'a mut ActionHandle,
    globals: &'a mut HashMap<u8, Stored>,
    player_variables: &'a mut HashMap<(EntityId, u8), Stored>,
}

impl Host<'_, '_> {
    fn index(&self, player: EntityId) -> Option<usize> {
        self.players
            .iter()
            .position(|item| item.controller == player)
    }

    fn view(&self, player: EntityId) -> Option<&WorkshopView> {
        self.views[self.index(player)?].as_ref()
    }

    /// The scope instances the action graphs opened or closed on the
    /// players (Teleport's), the messages they sent them (Stop Chasing's
    /// `0DCF`), the graphs they played there (Chase Player Variable's
    /// `1CCF`) and the player variables they set, to the players'
    /// controllers; an opened scope instance goes back to its scope state.
    fn route(&mut self) {
        for change in self.script.take_scope_changes() {
            let target = change.target();
            let Some(player) = self
                .players
                .iter_mut()
                .find(|player| player.world.runs(target))
            else {
                continue;
            };
            if let (Some(instance), ScopeChange::Open { source, .. }) =
                (player.world.change_scope(self.tick, &change), &change)
            {
                self.script.scope_opened(*source, target, instance);
            }
        }
        // The messages to the players first: Chase `1CCC` stops the old
        // `1CCF` (`0DCF`) before it plays the new one.
        let teams: Vec<(u32, i64)> = self
            .players
            .iter()
            .map(|player| (player.controller, i64::from(player.team)))
            .collect();
        let players = &*self.players;
        let messages = self
            .script
            .take_messages_to(|target| players.iter().any(|player| player.world.runs(target)));
        for message in messages {
            if let Some(player) = self
                .players
                .iter_mut()
                .find(|player| player.world.runs(message.target))
            {
                player.world.receive_message(self.tick, &teams, &message);
            }
        }
        for request in self.script.take_play_requests() {
            for player in self.players.iter_mut() {
                if player.world.play_script(self.tick, &request) {
                    break;
                }
            }
        }
        for (entity, id, value) in self.script.take_variable_writes() {
            let Some(index) = workshop_index(id) else {
                continue;
            };
            if let Some(player) = self
                .players
                .iter_mut()
                .find(|player| player.controller == entity)
            {
                player.world.set_workshop_variable(index, value);
            }
        }
    }

    /// The running actions' reevaluated parameters, where they changed.
    fn reevaluate(&mut self, engine: &mut Engine) {
        let due: Vec<ActionHandle> = self
            .actions
            .iter()
            .filter(|(_, running)| running.reevaluated.iter().any(|&again| again))
            .map(|(&handle, _)| handle)
            .collect();
        for handle in due {
            let running = &self.actions[&handle];
            let (source, context) = (
                Arc::clone(&running.call.source),
                running.call.context.clone(),
            );
            let params = engine.evaluate_params(&*self, &source, &context);
            let kinds = param_kinds(self.catalog, running.call.action);
            let running = self.actions.get_mut(&handle).expect("due action");
            let mut changed = false;
            for (index, param) in params.iter().enumerate() {
                if !running.reevaluated.get(index).copied().unwrap_or(false) {
                    continue;
                }
                let value = parameter(self.catalog, kinds.get(index).copied(), param);
                if let Some(sent) = running.sent.get_mut(index)
                    && !sent.identical(&value)
                {
                    *sent = value;
                    changed = true;
                }
            }
            if changed {
                let (action, sent) = (running.action, running.sent.clone());
                if let Err(error) = self.script.set_action_parameters(action, sent) {
                    eprintln!("the Workshop's mode entity stopped ({error})");
                }
            }
        }
    }

    /// The variable `stored` as the engine wrote it, while the entity still
    /// has what was written; else the entity's value now (a chase moved
    /// it). Workshop variables start at 0.
    fn read(stored: Option<&Stored>, now: Wire) -> Value {
        match stored {
            Some(stored) if stored.wire.identical(&now) => stored.value.clone(),
            _ if now == Wire::Null => Value::Number(0.0),
            _ => from_wire(&now),
        }
    }
}

/// The Workshop variable index of entity variable `id`
/// ([`workshop_variable`]: `FF7F` to `FFFE`).
fn workshop_index(id: u32) -> Option<u8> {
    (0..=127u8).find(|&index| workshop_variable(i64::from(index)) == id)
}

/// The catalog kinds of action `action`'s parameters.
fn param_kinds(catalog: &Catalog, action: u16) -> Vec<ParamKind> {
    catalog
        .action(action)
        .map(|entry| entry.params.iter().map(|param| param.kind).collect())
        .unwrap_or_default()
}

/// Which of an action's parameters its Reevaluation enum value names
/// (`5066`, `506B`, `507D`, `51AA`, `51D3`: "Visible To, Position, and
/// Radius", "String", "Destination and Rate"…): each part names the
/// parameter of that name; "String" names the text parameters (Header,
/// Subheader, Text), "Color" every colour, "Radius" Radius / Loudness.
/// None: no Reevaluation, or "None".
fn reevaluated(catalog: &Catalog, call: &ActionCall) -> Vec<bool> {
    let Some(entry) = catalog.action(call.action) else {
        return Vec::new();
    };
    let named = entry
        .params
        .iter()
        .position(|param| param.name.as_deref() == Some("Reevaluation"))
        .and_then(|index| match call.params.get(index)?.value() {
            Value::Enum(id) => catalog.enum_value_name(*id),
            _ => None,
        })
        .unwrap_or("None");
    let parts: Vec<&str> = named
        .split(", ")
        .flat_map(|part| part.split(" and "))
        .map(|part| part.trim_start_matches("and ").trim())
        .filter(|part| !part.is_empty() && *part != "None")
        .collect();
    entry
        .params
        .iter()
        .map(|param| {
            let name = param.name.as_deref().unwrap_or("");
            param.kind == ParamKind::Value
                && parts.iter().any(|&part| match part {
                    "String" => ["Header", "Subheader", "Text"].contains(&name),
                    "Color" => name.contains("Color"),
                    "Radius" => name.starts_with("Radius"),
                    part => part == name,
                })
        })
        .collect()
}

/// An action parameter as its graph takes it in `#18739`: a value slot as
/// a value tree ([`tree`]), Local Player's per player; anything else (a
/// variable index, an enum value) as its plain value.
fn parameter(catalog: &Catalog, kind: Option<ParamKind>, param: &Param) -> Wire {
    if kind != Some(ParamKind::Value) {
        return plain(catalog, param.value());
    }
    match param {
        Param::Value(value) => tree(catalog, value),
        Param::PerViewer { value, viewers } => {
            if viewers.iter().all(|(_, own)| own == value) {
                return tree(catalog, value);
            }
            let local_player = workshop_tree(identifier(ids::LOCAL_PLAYER), Vec::new());
            viewers
                .iter()
                .rev()
                .fold(tree(catalog, value), |otherwise, (viewer, own)| {
                    let viewing = workshop_tree(
                        identifier(ids::COMPARE),
                        vec![
                            local_player.clone(),
                            Wire::Asset(identifier(EQUAL)),
                            Wire::Entity(*viewer),
                        ],
                    );
                    workshop_tree(
                        identifier(ids::IF_THEN_ELSE),
                        vec![viewing, tree(catalog, own), otherwise],
                    )
                })
        }
    }
}

/// A value as a Workshop value tree the clients evaluate to it: a string
/// or a custom colour as their own values ([`text`]), Null as Null
/// (`4E63`; inside Number a client read it as 0, and Create HUD Text's
/// Null header showed "0"), anything else inside Number.
fn tree(catalog: &Catalog, value: &Value) -> Wire {
    match value {
        Value::Null => workshop_tree(identifier(ids::NULL), Vec::new()),
        Value::Text(value) => text(catalog, value),
        Value::Color(Color::Rgba(rgba)) => custom_color(rgba),
        value => workshop_tree(identifier(ids::NUMBER), vec![plain(catalog, value)]),
    }
}

fn custom_color(rgba: &[f64; 4]) -> Wire {
    workshop_tree(
        identifier(ids::CUSTOM_COLOR),
        rgba.iter().map(|&part| Wire::Float(part)).collect(),
    )
}

/// A string as its value tree: Custom String with its template and values
/// (the clients put each value's text in, `Eval_STU_43CFA23F`), a part
/// each client renders itself as its own value. A predefined string goes
/// already put together, as text inside Number [assumption: how `String`
/// `519B`'s `STU_1869F3BD` takes its index is not traced].
fn text(catalog: &Catalog, value: &Text) -> Wire {
    let id = |id: Id| Wire::Asset(identifier(id));
    match value {
        Text::Custom { template, args } => workshop_tree(
            identifier(ids::CUSTOM_STRING),
            std::iter::once(Wire::String(template.clone()))
                .chain(args.iter().map(|arg| text_argument(catalog, arg)))
                .collect(),
        ),
        Text::Predefined { .. } => workshop_tree(
            identifier(ids::NUMBER),
            vec![Wire::String(value.render(catalog))],
        ),
        Text::InputBinding(button) => {
            workshop_tree(identifier(ids::INPUT_BINDING_STRING), vec![id(*button)])
        }
        Text::Icon(icon) => workshop_tree(identifier(ids::ICON_STRING), vec![id(*icon)]),
        Text::HeroIcon(hero) => {
            workshop_tree(identifier(ids::HERO_ICON_STRING), vec![Wire::Asset(*hero)])
        }
        Text::AbilityIcon { hero, button } => workshop_tree(
            identifier(ids::ABILITY_ICON_STRING),
            vec![Wire::Asset(*hero), id(*button)],
        ),
    }
}

/// A value inside a string: a string as its own tree (evaluated first),
/// anything else as it is.
fn text_argument(catalog: &Catalog, value: &Value) -> Wire {
    match value {
        Value::Text(value) => text(catalog, value),
        value => plain(catalog, value),
    }
}

/// A value as the graphs hold it: numbers as floats, the Workshop's
/// constants (teams, heroes, buttons, colours, enum values) as their
/// identifiers, a string as its text.
fn plain(catalog: &Catalog, value: &Value) -> Wire {
    match value {
        Value::Null => Wire::Null,
        Value::Bool(value) => Wire::Bool(*value),
        Value::Number(value) => Wire::Float(*value),
        Value::Vector(vector) => Wire::Vector(*vector),
        Value::Entity(entity) => Wire::Entity(*entity),
        Value::Team(team) => Wire::Asset(identifier(TEAMS[usize::from(*team).min(2)])),
        Value::Hero(hero) | Value::GameMode(hero) => Wire::Asset(*hero),
        Value::Button(id) | Value::Enum(id) | Value::Color(Color::Named(id)) => {
            Wire::Asset(identifier(*id))
        }
        Value::Color(Color::Rgba(rgba)) => custom_color(rgba),
        Value::Map(map) => Wire::Integer(i64::from(*map)),
        Value::Text(text) => Wire::String(text.render(catalog)),
        Value::Array(items) => Wire::Array(items.iter().map(|item| plain(catalog, item)).collect()),
    }
}

/// A graph's value as the engine reads it (a chased variable, a created
/// id).
fn from_wire(value: &Wire) -> Value {
    match value {
        Wire::Null => Value::Null,
        Wire::Bool(value) => Value::Bool(*value),
        Wire::Integer(value) => Value::Number(*value as f64),
        Wire::Float(value) => Value::Number(*value),
        Wire::Vector(vector) => Value::Vector(*vector),
        Wire::Entity(entity) => Value::Entity(*entity),
        Wire::Asset(guid) if guid >> 48 == 0x0D80 => Value::Enum((guid & 0xFFFF) as Id),
        Wire::Asset(guid) => Value::Hero(*guid),
        Wire::String(text) => Value::Text(Text::Custom {
            template: text.clone(),
            args: Vec::new(),
        }),
        Wire::Array(items) => Value::Array(items.iter().map(from_wire).collect()),
        _ => Value::Null,
    }
}

impl World for Host<'_, '_> {
    /// Team 1 then Team 2, each in slot order ([`WorkshopRun::step`]
    /// sorts them so).
    fn players(&self) -> Vec<EntityId> {
        self.players
            .iter()
            .map(|player| player.controller)
            .collect()
    }
    fn team_of(&self, player: EntityId) -> u8 {
        self.index(player)
            .map_or(0, |index| self.players[index].team + 1)
    }
    fn slot_of(&self, player: EntityId) -> i32 {
        self.index(player)
            .map_or(-1, |index| self.players[index].slot as i32)
    }
    fn hero_of(&self, player: EntityId) -> Option<u64> {
        self.view(player).map(|view| view.hero)
    }
    fn is_dummy_bot(&self, _player: EntityId) -> bool {
        false
    }
    fn has_spawned(&self, player: EntityId) -> bool {
        self.view(player).is_some()
    }
    fn is_alive(&self, player: EntityId) -> bool {
        self.view(player).is_some_and(|view| view.alive)
    }
    fn entity_exists(&self, entity: EntityId) -> bool {
        self.index(entity).is_some()
    }
    fn position(&self, entity: EntityId) -> [f64; 3] {
        self.view(entity).map_or([0.0; 3], |view| view.feet)
    }
    fn eye_position(&self, player: EntityId) -> [f64; 3] {
        self.view(player).map_or([0.0; 3], |view| {
            let [x, y, z] = view.feet;
            [x, y + view.eye, z]
        })
    }
    fn facing(&self, player: EntityId) -> [f64; 3] {
        self.view(player)
            .map_or([0.0, 0.0, 1.0], |view| view.facing)
    }
    fn velocity(&self, player: EntityId) -> [f64; 3] {
        self.view(player).map_or([0.0; 3], |view| view.velocity)
    }
    fn throttle(&self, player: EntityId) -> [f64; 3] {
        self.view(player).map_or([0.0; 3], |view| view.throttle)
    }
    fn is_button_held(&self, player: EntityId, button: Id) -> bool {
        let Some(view) = self.view(player) else {
            return false;
        };
        BUTTONS
            .iter()
            .any(|&(id, bit)| id == button && view.buttons & bit != 0)
    }
    fn is_on_ground(&self, player: EntityId) -> bool {
        self.view(player).is_some_and(|view| view.on_ground)
    }
    fn activity(&self, player: EntityId, activity: Activity) -> bool {
        let Some(view) = self.view(player) else {
            return false;
        };
        let held = |id: Id| self.is_button_held(player, id);
        match activity {
            Activity::Crouching => held(0x4A40),
            Activity::Jumping => !view.on_ground && view.velocity[1] > 0.0,
            Activity::FiringPrimary => held(0x4A3A),
            Activity::FiringSecondary => held(0x4A3B),
            // What the abilities do is the body graphs'; not read here.
            _ => false,
        }
    }
    /// The distance down to the map below the feet; 0 on the ground.
    fn altitude(&self, player: EntityId) -> f64 {
        let Some(view) = self.view(player) else {
            return 0.0;
        };
        if view.on_ground {
            return 0.0;
        }
        self.scene
            .map_rays()
            .cast(view.feet, [0.0, -1.0, 0.0], 1000.0, None)
            .flatten()
            .map_or(1000.0, |hit| hit.distance)
    }
    fn health(&self, player: EntityId) -> f64 {
        self.view(player).map_or(0.0, |view| view.health)
    }
    fn max_health(&self, player: EntityId) -> f64 {
        self.view(player).map_or(0.0, |view| view.max_health)
    }
    fn ultimate_charge_percent(&self, player: EntityId) -> f64 {
        self.view(player).map_or(0.0, |view| view.ultimate_charge)
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
        self.all_heroes()
    }
    fn all_heroes(&self) -> Vec<u64> {
        self.catalog.heroes().map(|(_, hero)| hero).collect()
    }
    fn host_player(&self) -> Option<EntityId> {
        self.players.first().map(|player| player.controller)
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
        self.scene.map().map_guid as u32
    }
    fn current_game_mode(&self) -> u64 {
        self.scene.map().mode_guid
    }
    fn last_created_entity(&self, _creator: Option<EntityId>) -> Value {
        from_wire(&self.script.entity_value(LAST_CREATED_ENTITY))
    }
    fn last_text_id(&self, _creator: Option<EntityId>) -> Value {
        from_wire(&self.script.entity_value(LAST_TEXT_ID))
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
        start: [f64; 3],
        end: [f64; 3],
        _include: &[EntityId],
        _exclude: &[EntityId],
    ) -> RayHit {
        let delta: [f64; 3] = std::array::from_fn(|axis| end[axis] - start[axis]);
        let reach = delta.iter().map(|part| part * part).sum::<f64>().sqrt();
        if reach <= f64::EPSILON {
            return RayHit {
                position: end,
                ..RayHit::default()
            };
        }
        let direction = delta.map(|part| part / reach);
        match self
            .scene
            .map_rays()
            .cast(start, direction, reach, None)
            .flatten()
        {
            Some(hit) => RayHit {
                position: hit.point,
                normal: hit.normal,
                player: None,
            },
            None => RayHit {
                position: end,
                ..RayHit::default()
            },
        }
    }
    fn is_in_line_of_sight(&self, start: [f64; 3], end: [f64; 3]) -> bool {
        self.ray_cast(start, end, &[], &[]).position == end
    }
}

impl Variables for Host<'_, '_> {
    fn global(&self, index: u8) -> Value {
        Self::read(
            self.globals.get(&index),
            self.script.workshop_global(i64::from(index)),
        )
    }
    fn set_global(&mut self, index: u8, value: Value) {
        let wire = plain(self.catalog, &value);
        if let Err(error) = self
            .script
            .set_workshop_global(i64::from(index), wire.clone())
        {
            eprintln!("the Workshop's mode entity stopped ({error})");
        }
        self.globals.insert(index, Stored { value, wire });
    }
    fn player(&self, player: EntityId, index: u8) -> Value {
        let now = self
            .index(player)
            .and_then(|at| self.players[at].world.workshop_variable(index));
        let stored = self.player_variables.get(&(player, index));
        match now {
            Some(now) => Self::read(stored, now),
            None => stored.map_or(Value::Number(0.0), |stored| stored.value.clone()),
        }
    }
    fn set_player(&mut self, player: EntityId, index: u8, value: Value) {
        let wire = plain(self.catalog, &value);
        // The mode entity's graphs read the player's variable too.
        self.script
            .set_foreign_variable(player, workshop_variable(i64::from(index)), wire.clone());
        if let Some(at) = self.index(player) {
            self.players[at]
                .world
                .set_workshop_variable(index, wire.clone());
        }
        self.player_variables
            .insert((player, index), Stored { value, wire });
    }
}

impl Actions for Host<'_, '_> {
    /// Its graph on the mode entity with its parameters as the graph takes
    /// them; what it did to the players reaches them now (a Teleport moves
    /// the body this tick).
    fn start(&mut self, call: ActionCall) -> ActionHandle {
        *self.next_handle += 1;
        let handle = *self.next_handle;
        let kinds = param_kinds(self.catalog, call.action);
        let sent: Vec<Wire> = call
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| parameter(self.catalog, kinds.get(index).copied(), param))
            .collect();
        let graph = call.graph as u32;
        match self.script.start_workshop_action(graph, sent.clone()) {
            Ok(Some(action)) => {
                let reevaluated = reevaluated(self.catalog, &call);
                self.actions.insert(
                    handle,
                    Running {
                        action,
                        call,
                        reevaluated,
                        sent,
                    },
                );
            }
            Ok(None) => eprintln!("Workshop action graph {graph:04X} is not in the bundle"),
            Err(error) => eprintln!("Workshop action graph {graph:04X} stopped ({error})"),
        }
        self.route();
        // A Teleport moved the body now: the next values see it there.
        self.views = views(self.scene, self.players);
        handle
    }

    /// Done once its graph reported its result (or ended without one).
    fn finished(&mut self, handle: ActionHandle) -> bool {
        self.actions.get(&handle).is_none_or(|running| {
            self.script.action_status(running.action) != ActionStatus::Running
        })
    }
}
