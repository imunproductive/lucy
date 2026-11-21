//! The game mode entity's own statescript: the mode graph (`0F4A` for
//! Assault, Escort and Hybrid, `0EC2` for Control, `0D76`, `0D0B`, `0F60`,
//! `0F73` for the Arcade modes; `0C5` → the script of entity `16B2`) run on
//! the server against a [`ModeWorld`], as retail runs it on the entity the
//! server creates (ow1-preservation `Assault flow on Anubis`).
//!
//! The server's match flow feeds it the mode entity's variables (the clock
//! v1303, the attacking team `2A2B`, ...) and routes the messages it sends.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value as Json;

use crate::{
    expr::EntityRelation,
    graph::GraphStore,
    map_tags::MapTags,
    placed::{Gaps, MapEntityWorld, PlacedEntity},
    vm::{
        ACTION_PARAMETERS, ActionOutcome, Component, EntityCreation, GameMessage, HealthChange,
        InitFault, MAX_INIT_WORK, MapRays, MessageEnd, PlayRequest, Root, ScopeChange, ScopeSource,
        Site, Snapshot, Unknowns, World,
    },
    wire::{Value, Variables},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

/// The target the worlds give a message a graph sent to every entity
/// ([`World::broadcast_message`]); entity 0 is never one. The match hands
/// it to every entity that runs graphs on the server but the sender, whose
/// own instances took it when it was sent (`orpheus_sim::messages`).
pub const EVERY_ENTITY: u32 = 0;

/// The ids of the entities the mode entity's graphs make
/// ([`ModeScript::created`]): `CREATED_FIRST + n % CREATED_SPAN`, past the
/// Biotic Fields' and the seen projectiles' spans
/// (`orpheus_sim::projectiles`, `0xA008_0000` and `0xA00C_0000`). Retail's
/// ids are the server's own choice.
pub const CREATED_FIRST: u32 = 0xa010_0000;
pub const CREATED_SPAN: u32 = 0x0004_0000;

/// An entity the mode entity's graphs made (`STUStatescriptActionCreateEntity`:
/// the Workshop's Create Effect `1CE8` and Create Icon `1AB1`), as the
/// clients get it: the server creates it for every client and sends its
/// frames ([`ModeScript::created`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatedEntity {
    pub entity: u32,
    /// Its entity definition (`003`): `304A` for an effect, `304B` for an
    /// icon.
    pub definition: u64,
    pub position: [f64; 3],
}

/// How many rounds [`ModeScript::settle`] hands things on in one run
/// between the mode entity and the entities it made, at most: an effect's
/// begin and its first messages take two.
const SETTLE_ROUNDS: usize = 16;

/// The game struct's team byte that stands for any team (`STU_A5B9EAAD`'s
/// fallback entry).
const ANY_TEAM: i64 = 5;

/// A team's size in the game struct's team entries (team, size): the
/// entry of `team`, else the "any team" entry 5 (0x7FF789CADB40); the
/// client's last fallback, a table of its own, is not known, so no entry
/// leaves it unanswered.
pub fn team_size(entries: &[(u8, u8)], team: i64) -> Option<i64> {
    let entry = |wanted: i64| {
        entries
            .iter()
            .find(|&&(entry, _)| i64::from(entry) == wanted)
            .map(|&(_, size)| i64::from(size))
    };
    entry(team).or_else(|| entry(ANY_TEAM))
}

/// A player of the match as the mode graph knows him: his controller, his
/// current body and its hero (none while he has none) and his team as the
/// client reads it from the entity's filter bits (a free-for-all player is
/// on team 4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModePlayer {
    pub controller: u32,
    pub body: Option<u32>,
    pub hero: Option<u64>,
    pub team: i64,
}

/// The native configs whose answers come from the match's players and its
/// start condition ([`ModeScript::set_players`]).
const PLAYER_NATIVES: [&str; 3] = ["STU_A5B9EAAD", "STU_A90795E2", "STU_81A9AB10"];

/// What the mode graph asks outside its own entity. The mode entity's
/// variables are its own (the server writes them into the component);
/// the bundle's mode variables (`0C5.m_3CE93B76`, with the game ruleset's
/// over them) answer the rest.
pub struct ModeWorld {
    pub graphs: Arc<GraphStore>,
    pub mode: u64,
    /// The game's ruleset ([`GraphStore::mode_variables`]); 0: none.
    pub ruleset: u64,
    pub unknowns: Unknowns,
    /// The match's players (AI heroes too): the client counts a team's
    /// entities but those of definition `003:25CB` (`STU_A5B9EAAD`).
    pub players: Vec<ModePlayer>,
    /// Each team's size as the server sends it in the 20300/20301 game
    /// struct (`#0.#0.#0.#4`: byte `+0x60` the team, 5 any team, `+0x61`
    /// its size; `orpheus_sim::instance`).
    pub team_sizes: Vec<(u8, u8)>,
    /// The server's answer to `STU_81A9AB10`, which lets the match leave
    /// its waiting phase.
    pub ready: bool,
    /// The match's value of the mode entity's component 114 field 9, as
    /// the server sends it (`STU_B182C5C9`); none: unknown.
    pub mode_random: Option<f32>,
    /// The match's random table index (`STU_CBF6FF14`, in `[0, 16384)`),
    /// drawn by the server; none: unknown.
    pub match_seed: Option<u16>,
    /// The game modes the match's map supports (its header's
    /// `m_supportedGamemodes`), which `STU_16753B11` tests; none: unknown.
    pub map_modes: Option<Vec<u64>>,
    /// The map's tagged placeables the scene tag queries find; none:
    /// unknown.
    pub map_tags: Option<Arc<MapTags>>,
    /// The map's collision the graph's ray casts meet; none: no map.
    pub map_rays: Option<Arc<dyn MapRays>>,
    /// The heroes of the game, in the catalog's order, as All Heroes
    /// (`STU_AFC074B3`) lists them, with their bodies' entity definitions
    /// (`m_gameplayEntity`, `STU_94AED9DC`); none: unknown.
    pub heroes: Option<Vec<(u64, u64)>>,
    /// Where the map's server placeables stand (the objectives `0F4A`
    /// places its `0C49` over, `STU_71A87E92(#10551)`).
    pub placeables: BTreeMap<u32, [f64; 3]>,
    /// Mode entity variables that hold before the root graph starts, over
    /// the mode's: what the map's placeables reported before it (the
    /// objectives' `0813`, `orpheus_sim::mode_script`).
    pub start_variables: Variables,
    /// Other entities' variables as the mode entity's graphs know them:
    /// the server's ([`ModeScript::set_foreign_variable`]) and the graphs'
    /// own writes (a Workshop action's player variables, `STU_0BDBC7B2`).
    foreign: BTreeMap<(u32, u32), Value>,
    /// The graphs' writes of other entities' variables since the server
    /// took them ([`ModeScript::take_variable_writes`]).
    variable_writes: Vec<(u32, u32, Value)>,
    /// The scope instances the graphs open or close, until
    /// [`ModeScript`] hands them on: its own entity's to its component,
    /// the others' to the match ([`ModeScript::take_scope_changes`]).
    scope_changes: Vec<ScopeChange>,
    /// The graphs the graphs played on other entities, until the match
    /// takes them ([`ModeScript::take_play_requests`]).
    plays: Vec<PlayRequest>,
    /// The identifiers 2 whose map placeables the graphs removed, until
    /// the match takes them ([`ModeScript::take_placeable_removals`]).
    placeable_removals: Vec<u64>,
    /// The bodies the graphs damaged or healed (the Workshop's Kill
    /// `1482` st0), until the match takes them
    /// ([`ModeScript::take_health_changes`]).
    health_changes: Vec<HealthChange>,
    outbox: Vec<GameMessage>,
    /// The messages the graphs sent the mode entity itself, until the match
    /// takes them ([`ModeScript::take_own_messages`]).
    own: Vec<GameMessage>,
    held: Vec<GameMessage>,
    /// The ends of the graphs' persistent sends to other entities, until
    /// [`ModeScript`] hands them on.
    ends: Vec<MessageEnd>,
    /// The entities the graphs made that [`ModeScript`] hasn't started
    /// yet, and those it runs.
    made: Vec<CreatedEntity>,
    created: Vec<CreatedEntity>,
    /// The running ones the graphs removed, until [`ModeScript`] ends
    /// them ([`World::destroy_entity`]).
    destroyed: Vec<u32>,
    /// The next id to try ([`CREATED_FIRST`]).
    next_created: u32,
}

impl ModeWorld {
    pub fn new(graphs: Arc<GraphStore>, mode: u64, unknowns: Unknowns) -> Self {
        Self {
            graphs,
            mode,
            ruleset: 0,
            unknowns,
            players: Vec::new(),
            team_sizes: Vec::new(),
            ready: false,
            mode_random: None,
            match_seed: None,
            map_modes: None,
            map_tags: None,
            map_rays: None,
            heroes: None,
            placeables: BTreeMap::new(),
            start_variables: Variables::new(),
            foreign: BTreeMap::new(),
            variable_writes: Vec::new(),
            scope_changes: Vec::new(),
            plays: Vec::new(),
            placeable_removals: Vec::new(),
            health_changes: Vec::new(),
            outbox: Vec::new(),
            own: Vec::new(),
            held: Vec::new(),
            ends: Vec::new(),
            made: Vec::new(),
            created: Vec::new(),
            destroyed: Vec::new(),
            next_created: 0,
        }
    }

    /// Another entity's entity variables as the graphs find them when they
    /// start (a map placeable's; the match gives them after with
    /// [`ModeScript::set_foreign_variables`]).
    pub fn set_foreign_variables(&mut self, entity: u32, variables: &Variables) {
        for (&id, value) in variables {
            self.foreign.insert((entity, id), value.clone());
        }
    }

    /// The players on `team`.
    fn team_players(&self, team: i64) -> i64 {
        self.players
            .iter()
            .filter(|player| player.team == team)
            .count() as i64
    }

    /// The player whose controller or body `entity` is.
    fn player_of(&self, entity: u32) -> Option<&ModePlayer> {
        self.players
            .iter()
            .find(|player| player.controller == entity || player.body == Some(entity))
    }

    /// An entity the graphs made, started or not.
    fn made_entity(&self, entity: u32) -> Option<&CreatedEntity> {
        self.created
            .iter()
            .chain(&self.made)
            .find(|created| created.entity == entity)
    }

    /// The game messages the graph sent other entities, for the match to
    /// route (like `PlayerWorld::take_messages`); [`EVERY_ENTITY`] targets
    /// every entity but the mode entity.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }
}

impl World for ModeWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        match class {
            // As `PlayerWorld`: the tunable's authored value, not scaled
            // for the entity.
            "STU_082FA44E" => Some("the tunable's authored value"),
            "STU_AFC074B3" => Some("the game's heroes"),
            "STU_A90795E2" => Some("the team's players (server-only)"),
            "STU_BD2D717B" => Some("the players' controllers (1) or bodies (0)"),
            _ => None,
        }
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.ruleset_value(id, 0, self.mode, self.ruleset)
    }

    /// The players' controllers and bodies relate as a player's world
    /// answers for his own (`ControllerWorld`): a controller possesses
    /// his current body (entity 0 while he has none, `STU_0AC15F78`) and
    /// is his own controller once he has one; a body's controller and
    /// current possessor is its player's controller. The Workshop's
    /// Respawn `1383` sends `06B6` to `STU_0AC15F78` of each player, a dead
    /// body included. Others: unknown.
    fn related_entity(
        &mut self,
        _site: Site,
        relation: EntityRelation,
        entity: u32,
    ) -> Option<u32> {
        let player = self.player_of(entity)?;
        let possessing = player.body.map(|_| player.controller);
        if entity == player.controller {
            return match relation {
                EntityRelation::PossessedBody => Some(player.body.unwrap_or(0)),
                EntityRelation::Controller => Some(possessing.unwrap_or(0)),
                _ => None,
            };
        }
        match relation {
            EntityRelation::Controller | EntityRelation::CurrentPossessor => possessing,
            _ => None,
        }
    }

    fn mode_random(&self) -> Option<f64> {
        self.mode_random.map(f64::from)
    }

    fn match_seed(&self) -> Option<i64> {
        self.match_seed.map(i64::from)
    }

    fn team_size(&self, team: i64) -> Option<i64> {
        team_size(&self.team_sizes, team)
    }

    fn map_supports_mode(&self, _site: Site, mode: u64) -> Option<bool> {
        Some(self.map_modes.as_ref()?.contains(&mode))
    }

    /// The mode entity has no team: the server gives it no component 25
    /// (`orpheus_sim::scene::mode_create`), so its filter word is empty.
    /// A player's controller and body are on his team.
    fn entity_team(&self, _site: Site, entity: u32) -> Option<i64> {
        if entity == GAME_MODE_ENTITY {
            return Some(-1);
        }
        Some(self.player_of(entity)?.team)
    }

    fn entity_filter_flags(&self, _site: Site, entity: u32) -> Option<u64> {
        (entity == GAME_MODE_ENTITY).then_some(0)
    }

    fn map_rays(&self) -> Option<&dyn MapRays> {
        self.map_rays.as_deref()
    }

    /// The map's placeables that carry the tag ([`MapTags::with_tag`]).
    fn entities_with_tag(&self, _site: Site, tag: u64) -> Option<Vec<u32>> {
        self.map_tags.as_ref()?.with_tag(tag)
    }

    /// The map's tagged placeables, then the players' bodies (every hero
    /// body's definition has a target tag component, the bundle's
    /// `bodies`). Assumption (as `crate::map_tags`): no other entity
    /// carries a tag.
    fn tagged_entities(&self, _site: Site) -> Option<Vec<u32>> {
        let mut entities: Vec<u32> = self.map_tags.as_ref()?.entities().collect();
        entities.extend(self.players.iter().filter_map(|player| player.body));
        Some(entities)
    }

    /// A map placeable's tags; a player's body's are its definition's
    /// (the bundle's `bodies`), as the victim's tags at a kill
    /// (`orpheus_sim::replication`). Assumption: its states add or remove
    /// none the mode graph asks about.
    fn has_target_tag(&self, site: Site, entity: u32, tag: u64) -> Option<bool> {
        if let Some(player) = self.player_of(entity)
            && player.body == Some(entity)
        {
            let definition = self.hero_body(site, player.hero?)?;
            let tags = self
                .graphs
                .bodies()
                .get(&definition)?
                .target_tags
                .as_ref()?;
            return Some(tags.contains(&tag));
        }
        let placed = self.map_tags.as_ref()?.get(entity)?;
        Some(placed.tags.contains(&tag))
    }

    /// The mode entity, the players' controllers and bodies, the map's
    /// server placeables and tagged placeables and the entities the graphs
    /// made exist.
    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        let known = entity == GAME_MODE_ENTITY
            || self.player_of(entity).is_some()
            || self.made_entity(entity).is_some()
            || self.placeables.contains_key(&entity)
            || self
                .map_tags
                .as_ref()
                .is_some_and(|tags| tags.get(entity).is_some());
        known.then_some(true)
    }

    fn entity_position(&self, _site: Site, entity: u32) -> Option<[f64; 3]> {
        if let Some(made) = self.made_entity(entity) {
            return Some(made.position);
        }
        if let Some(&position) = self.placeables.get(&entity) {
            return Some(position);
        }
        Some(self.map_tags.as_ref()?.get(entity)?.position)
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return self
                .foreign
                .get(&(entity, id))
                .cloned()
                .unwrap_or(Value::Null);
        }
        self.graphs
            .mode_variables(self.mode, self.ruleset)
            .and_then(|variables| variables.get(&id).copied())
            .map_or(Value::Null, Value::from)
    }

    /// A Workshop action set a player's variable: kept for the graphs'
    /// next reads and for the server to set in the player's script.
    fn set_entity_variable(&mut self, _site: Site, entity: u32, id: u32, value: Value) -> bool {
        self.foreign.insert((entity, id), value.clone());
        self.variable_writes.push((entity, id, value));
        true
    }

    /// Opened on the mode entity itself (Create HUD Text's scope `31`
    /// targets it) or on a player (Teleport's on each target), by
    /// [`ModeScript`] after the run.
    fn change_scope(&mut self, _site: Site, change: ScopeChange) -> bool {
        self.scope_changes.push(change);
        true
    }

    /// Played on a player (Chase Variable's `1CCF`), by the match.
    fn play_script(&mut self, _site: Site, request: PlayRequest) -> bool {
        self.plays.push(request);
        true
    }

    /// `0CBB` st34 removing the health packs, by the match.
    fn remove_placeables(&mut self, _site: Site, identifier: u64) -> bool {
        self.placeable_removals.push(identifier);
        true
    }

    /// A body the Workshop's Kill (`1482` st0, 1e9 damage) or another
    /// action graph's ModifyHealth names, by the match's health code. An
    /// instigator that names no entity (Kill's Killer Null) is the mode
    /// entity's own.
    fn modify_health(&mut self, mut change: HealthChange) -> bool {
        if !crate::expr::valid_entity(change.instigator) {
            change.instigator = GAME_MODE_ENTITY;
        }
        self.health_changes.push(change);
        true
    }

    /// The next free id of [`CREATED_FIRST`]'s span; [`ModeScript`] starts
    /// the entity after the run. With no position it stands where its
    /// maker does: the mode entity, which the server creates at the origin
    /// (`orpheus_sim::scene::mode_create`) [assumption: the client's
    /// `sub_7FF789809ED0` with no position was not traced]. None when the
    /// span is full.
    fn create_entity(&mut self, _site: Site, creation: EntityCreation) -> Option<u32> {
        let entity = (0..CREATED_SPAN)
            .map(|step| CREATED_FIRST + self.next_created.wrapping_add(step) % CREATED_SPAN)
            .find(|&entity| self.made_entity(entity).is_none())?;
        self.next_created = (entity - CREATED_FIRST + 1) % CREATED_SPAN;
        self.made.push(CreatedEntity {
            entity,
            definition: creation.definition,
            position: creation.position.unwrap_or_default(),
        });
        Some(entity)
    }

    /// The mode entity removes an entity it made (`STU_523EE38D`: Create
    /// Icon `1AB1` rid1 on its `#38` when Destroy Icon's `0DFA` names it):
    /// one not started yet never starts; [`ModeScript`] ends a running
    /// one's graphs after the run, and the match removes it from the
    /// clients with the rest of [`ModeScript::created`]. False for an
    /// entity it didn't make.
    fn destroy_entity(&mut self, _site: Site, entity: u32) -> bool {
        if let Some(index) = self.made.iter().position(|made| made.entity == entity) {
            self.made.remove(index);
            return true;
        }
        if !self.created.iter().any(|created| created.entity == entity) {
            return false;
        }
        if !self.destroyed.contains(&entity) {
            self.destroyed.push(entity);
        }
        true
    }

    /// `STU_A3DEF368` is the first entity with component 114, the mode
    /// entity itself (ow1-preservation: getter 0xcdd340). `STU_082FA44E`
    /// reads a tunable's authored value, as `PlayerWorld` does.
    ///
    /// `STU_A5B9EAAD` (`m_team`) is "the team is full", a bool: the count
    /// of the team's entities but those of definition `003:25CB`, at least
    /// the team's size ([`World::team_size`]). `Eval_STU_A5B9EAAD`
    /// 0x7FF789BF9E10, size `sub_7FF789CADB40` (ModeGraphGaps.md, "Client
    /// tools pass (2026-11-16)"). Its uses ask both teams 0 and 1 before
    /// the match leaves its waiting phase (`0F4A` nodes 14 and 419, `0EC2`
    /// 15 and 380, `0D0B` 237, `0F60` 27), or the free-for-all team 4
    /// (`0F73` 16), so the start rule `25CA` waits for full teams: a game
    /// with fewer players than its team sizes waits there (the start rule
    /// `25CB` waits for message `0789` instead). The evaluator makes the
    /// asking state depend on the game's player list (channel 2 of the
    /// game's dependency table, `sub_7FF789AEBA60`), so a player who joins
    /// re-checks the waiting state's transition ([`ModeScript::set_players`]).
    ///
    /// `STU_81A9AB10` (a bool with no fields) gates the same start in the
    /// default start rule (ruleset `#9672` = `25CA`; `0F4A` node 7: true
    /// waits for both teams, false waits for message `0789`), and Tutorial
    /// graph `091B` waits on it to send `01DB` to the mode entity. It is
    /// server-only (no client code: the base config's evaluate, `xor al,
    /// al`); the server answers [`ModeWorld::ready`], its own rule.
    ///
    /// `STU_21A5BB52` (a bool with no fields) picks, outside Competitive
    /// play, whether the payload's random speed factor #29093 comes from
    /// message `11BA` (`0F4A` uid 1299); `0F60` and `0F73` read it in
    /// `STU_F7216D4F`. It is server-only (no client code); the server
    /// answers false, its own rule: it sends no `11BA` (`0F4A` rid504
    /// would keep its `08CF` in #29093), so it has no carried value to
    /// pick, and in `0F4A` both answers give `STU_B182C5C9` while #29093
    /// is unset (rid503: `valueOr(#29093, STU_B182C5C9())`).
    ///
    /// Neither is an assumption: no client code evaluates them, so they are
    /// the server's rules, as the retail server's were
    /// ([`World::assumption`] leaves them out).
    ///
    /// `STU_A90795E2` (`m_6CE7A900`, a team) is server-only too (the base
    /// config's evaluate). Its uses read it as the team's players: `1061`
    /// (Team Deathmatch and Deathmatch, on `06A6`) creates the team's size
    /// less it in AI heroes, `0D44` holds a count to it, and Elimination
    /// (`0D0B` rid181) gives the round to team 0 when it has players and
    /// team 1 none. The server answers [`ModeWorld::players`]' count.
    ///
    /// `STU_BD2D717B` (`m_B257A3A4`, a byte) is every player: its
    /// evaluate (slot 5 0x7FF789BF2E40) walks the game's participant list
    /// (`sub_7FF7895BC390`, the list `STU_A5B9EAAD` counts, on the same
    /// dependency channel 2) and gives one entity each, from one of two
    /// lookups by the byte (set: 0x7FF789CB7460, the entity at `+8` of what
    /// it finds; clear: 0x7FF789CB5E40). Which is the controller and which
    /// the body was not read: the voice audiences (`0686`, `0F4A`, `0EC2`
    /// Criteria and VoiceStimulus, all 0) speak to bodies, and the
    /// controller's `0215` filters with 1, so 1 = the players'
    /// controllers, 0 = their bodies [inference]. `0F4A` rid76 keeps the
    /// players in #7300 at a round's start.
    ///
    /// `STU_AFC074B3` is the Workshop's All Heroes (`1B4A` `53C9`; Set
    /// Player Allowed Heroes `1D57` and Preload Hero `1D3B` read it): the
    /// client's array evaluate (slot 5 `0x7FF789C01160`) copies the hero
    /// list its host's game object gives (vtable +624), sorted
    /// (`sub_7FF789C1CA20`), which the server can't read. Answered: the
    /// game's heroes ([`ModeWorld::heroes`]), as the controller's graphs
    /// get the heroes the player may play (`controller.rs`).
    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        let literal_team = |field: &str| {
            let team = config.get(field)?;
            if team.get("$")?.as_str()? != "STU_BD6CC023" {
                return None;
            }
            team.get("m_6CE7A900")?.as_i64()
        };
        match config.get("$")?.as_str()? {
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            "STU_A5B9EAAD" => {
                let team = literal_team("m_team")?;
                Some(Value::Bool(
                    self.team_players(team) >= self.team_size(team)?,
                ))
            }
            "STU_A90795E2" => Some(Value::Integer(
                self.team_players(literal_team("m_6CE7A900")?),
            )),
            "STU_81A9AB10" => Some(Value::Bool(self.ready)),
            "STU_21A5BB52" => Some(Value::Bool(false)),
            "STU_AFC074B3" => Some(Value::Array(
                self.heroes
                    .as_ref()?
                    .iter()
                    .map(|&(hero, _)| Value::Asset(hero))
                    .collect(),
            )),
            "STU_BD2D717B" => {
                let controllers = match config.get("m_B257A3A4")?.as_u64()? {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                Some(Value::Array(
                    self.players
                        .iter()
                        .filter_map(|player| {
                            if controllers {
                                Some(player.controller)
                            } else {
                                player.body
                            }
                        })
                        .map(Value::Entity)
                        .collect(),
                ))
            }
            _ => None,
        }
    }

    fn hero_body(&self, _site: Site, hero: u64) -> Option<u64> {
        let heroes = self.heroes.as_ref()?;
        heroes
            .iter()
            .find(|&&(listed, _)| listed == hero)
            .map(|&(_, body)| body)
    }

    fn send_message(
        &mut self,
        _source: Site,
        target: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        self.outbox.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// Kept for the match flow, which takes `0F4A`'s messages in its
    /// place (Set Match Time's `08D8`).
    fn observe_own_message(&mut self, _source: Site, message: u64, parameters: &Variables) {
        self.own.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target: GAME_MODE_ENTITY,
            message,
            parameters: parameters.clone(),
        });
    }

    fn broadcast_message(&mut self, _source: Site, message: u64, parameters: &Variables) -> bool {
        self.outbox.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target: EVERY_ENTITY,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// Held until the state's end ([`World::end_message`] to
    /// [`EVERY_ENTITY`]); the match delivers it ([`ModeScript::held`]).
    fn hold_broadcast(&mut self, _source: Site, message: u64, parameters: &Variables) -> bool {
        self.held.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target: EVERY_ENTITY,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// A held broadcast's end drops it ([`ModeScript::held`]); a
    /// persistent send's to an entity goes to the match (the capture
    /// point's rates `079E`, which `0F4A` holds to the current objective in
    /// setup and again in the round: the point's entry st0 takes the
    /// round's only once the setup's ended), or to an entity the graphs
    /// made ([`ModeScript::take_message_ends`]).
    fn end_message(&mut self, _source: Site, target: u32, message: u64) -> bool {
        if target != EVERY_ENTITY {
            self.ends.push(MessageEnd {
                sender: GAME_MODE_ENTITY,
                target,
                message,
            });
            return true;
        }
        if let Some(index) = self.held.iter().position(|held| held.message == message) {
            self.held.remove(index);
        }
        true
    }
}

/// The world of the entities the mode entity's graphs make: the match's
/// mode, ruleset, map and collision, as the mode entity's own.
fn created_world(world: &ModeWorld, frame: u32) -> MapEntityWorld {
    MapEntityWorld::start(
        Arc::clone(&world.graphs),
        world.mode,
        world.ruleset,
        world.mode_random.unwrap_or_default(),
        world.unknowns,
        [],
        world.map_tags.clone(),
        world.map_rays.clone(),
        None,
        frame,
    )
}

/// The mode graph running on the mode entity.
pub struct ModeScript {
    component: Component,
    world: ModeWorld,
    frame: u32,
    /// The Workshop actions started here that may still need retiring
    /// ([`Self::settle`]).
    actions: Vec<WorkshopAction>,
    /// The scripts of the entities the graphs made ([`CreatedEntity`]).
    entities: MapEntityWorld,
    /// The messages [`Self::settle`] handed on to the match
    /// ([`Self::take_messages`]).
    routed: Vec<GameMessage>,
    /// The ends of the graphs' persistent sends that [`Self::settle`]
    /// handed on to the match ([`Self::take_message_ends`]).
    routed_ends: Vec<MessageEnd>,
}

/// A Workshop action graph started on the mode entity
/// ([`ModeScript::start_workshop_action`]): its instance and when it was
/// created, so a later instance with the same id isn't taken for it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkshopAction {
    pub instance: u16,
    pub start_ms: u64,
}

/// Where a Workshop action is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActionStatus {
    /// It hasn't reported its result yet (a Wait's timer runs).
    Running,
    /// It reported its result; the instance may still run (a Create
    /// action's scope keeps its text up).
    Finished(ActionOutcome),
    /// Its instance ended without a result.
    Ended,
}

impl ModeScript {
    /// Start `graph` as the mode entity's instance 1 at `frame`, against
    /// `world`.
    /// The mode entity's own variables start as its mode's
    /// (`0C5.m_3CE93B76`, the bundle's `mode_variables`) with the game
    /// ruleset's over them: the Practice Range `005A`'s `#27534` true makes
    /// `13C0`'s #27536 false, the board poster st6 and the open doors st18,
    /// where `0018`'s false takes the training branch (st11, `0BD6`); its
    /// ruleset `0C0:34` sets the hero limit #9548 to `254D`.
    pub fn start(mut world: ModeWorld, graph: u32, frame: u32) -> Result<Self, InitFault> {
        let graphs = Arc::clone(&world.graphs);
        let mut variables: Variables = graphs
            .mode_variables(world.mode, world.ruleset)
            .iter()
            .flat_map(|variables| variables.iter())
            .map(|(&id, &value)| (id, Value::from(value)))
            .collect();
        variables.extend(std::mem::take(&mut world.start_variables));
        let mut component = Component::start(
            graphs,
            GAME_MODE_ENTITY,
            variables,
            vec![Root::new(1, graph)],
            frame,
            &mut world,
            MAX_INIT_WORK,
        )?;
        component.advance_frame(frame, &mut world)?;
        let mut script = Self {
            entities: created_world(&world, frame),
            component,
            world,
            frame,
            actions: Vec::new(),
            routed: Vec::new(),
            routed_ends: Vec::new(),
        };
        script.settle()?;
        Ok(script)
    }

    /// The mode entity with no mode graph, to run Workshop actions on
    /// ([`Self::start_workshop_action`]) where the mode graph doesn't
    /// drive the mode.
    pub fn start_host(mut world: ModeWorld, frame: u32) -> Result<Self, InitFault> {
        let graphs = Arc::clone(&world.graphs);
        let component = Component::start(
            graphs,
            GAME_MODE_ENTITY,
            Variables::new(),
            Vec::new(),
            frame,
            &mut world,
            MAX_INIT_WORK,
        )?;
        let mut script = Self {
            entities: created_world(&world, frame),
            component,
            world,
            frame,
            actions: Vec::new(),
            routed: Vec::new(),
            routed_ends: Vec::new(),
        };
        script.settle()?;
        Ok(script)
    }

    /// Start a Workshop action's graph (`054/0155` `m_graph`) on the mode
    /// entity with its evaluated `parameters` in `#18739`, in catalog
    /// order (`refs/reports/Workshop.md` §2: the retail server's host is
    /// an inference). Its synchronous part runs now: an action without a
    /// wait has its result at once ([`Self::action_status`]). None: the
    /// graph isn't in the bundle.
    pub fn start_workshop_action(
        &mut self,
        graph: u32,
        parameters: Vec<Value>,
    ) -> Result<Option<WorkshopAction>, InitFault> {
        let variables = Variables::from([(ACTION_PARAMETERS, Value::Array(parameters))]);
        let started = self
            .component
            .run_instance(graph, variables, &mut self.world)?
            .map(|(instance, start_ms)| WorkshopAction { instance, start_ms });
        self.actions.extend(started);
        self.settle()?;
        Ok(started)
    }

    pub fn action_status(&self, action: WorkshopAction) -> ActionStatus {
        match self
            .component
            .action_outcome(action.instance, action.start_ms)
        {
            Some(outcome) => ActionStatus::Finished(outcome),
            None if self
                .component
                .instance_alive(action.instance, action.start_ms) =>
            {
                ActionStatus::Running
            }
            None => ActionStatus::Ended,
        }
    }

    /// End a Workshop action's instance now (its rule stopped while it
    /// waited).
    pub fn stop_workshop_action(&mut self, action: WorkshopAction) -> Result<(), InitFault> {
        if self
            .component
            .instance_alive(action.instance, action.start_ms)
        {
            self.component
                .destroy_instance(action.instance, &mut self.world)?;
        }
        self.settle()
    }

    /// Workshop global variable `index` as the graphs see it: the mode
    /// entity's entity variable (a chase's value included).
    pub fn workshop_global(&self, index: i64) -> Value {
        self.entity_value(crate::expr::workshop_variable(index))
    }

    /// The mode entity's variable `id` now (a Create action's last created
    /// id, `#20765`, `#20554`).
    pub fn entity_value(&self, id: u32) -> Value {
        self.component.entity_value(id)
    }

    /// Whether a Workshop action's instance is still there (a Create
    /// action's scope keeps it up).
    pub fn action_alive(&self, action: WorkshopAction) -> bool {
        self.component
            .instance_alive(action.instance, action.start_ms)
    }

    /// Set another entity's variable as the mode entity's graphs see it
    /// (a player variable a chase on the player's own script moved).
    pub fn set_foreign_variable(&mut self, entity: u32, id: u32, value: Value) {
        self.world.foreign.insert((entity, id), value);
    }

    /// Another entity's entity variables now, as the match gives them (a
    /// map placeable's: the capture point's progress #9866e and owner
    /// #99e, the objective's times #10552e, which `0F4A` reads through
    /// `#10551.id(...)`): each that changed is set, and the states that
    /// read it take the change ([`Component::foreign_variable_changed`]);
    /// one that went reads null.
    pub fn set_foreign_variables(
        &mut self,
        entity: u32,
        variables: &Variables,
    ) -> Result<(), InitFault> {
        let gone: Vec<u32> = self
            .world
            .foreign
            .range((entity, 0)..=(entity, u32::MAX))
            .filter(|((_, id), value)| **value != Value::Null && !variables.contains_key(id))
            .map(|((_, id), _)| *id)
            .collect();
        let changed: Vec<(u32, Value)> = variables
            .iter()
            .filter(|&(&id, value)| self.world.foreign.get(&(entity, id)) != Some(value))
            .map(|(&id, value)| (id, value.clone()))
            .chain(gone.into_iter().map(|id| (id, Value::Null)))
            .collect();
        for (id, value) in changed {
            self.world.foreign.insert((entity, id), value);
            self.component
                .foreign_variable_changed(entity, id, &mut self.world)?;
        }
        self.settle()
    }

    /// Set Workshop global variable `index`: the mode entity's entity
    /// variable ([`crate::expr::workshop_variable`]); the states that read
    /// it take the change.
    pub fn set_workshop_global(&mut self, index: i64, value: Value) -> Result<(), InitFault> {
        let id = crate::expr::workshop_variable(index);
        self.component
            .set_entity_variable(id, value, &mut self.world)?;
        self.settle()
    }

    /// The rule engine evaluated a running action's parameters again: the
    /// `changed` ones (by index) replace theirs
    /// ([`Component::set_action_parameters`]), and its instances here and
    /// the scope instances it opened on the entities the mode entity made
    /// ([`MapEntityWorld::set_scope_parameters`]: Create Effect's on its
    /// effect) copy them again.
    pub fn set_action_parameters(
        &mut self,
        action: WorkshopAction,
        changed: &[(usize, Value)],
    ) -> Result<(), InitFault> {
        let Some(parameters) = self.component.set_action_parameters(
            action.instance,
            action.start_ms,
            changed,
            &mut self.world,
        )?
        else {
            return Ok(());
        };
        self.entities
            .set_scope_parameters(GAME_MODE_ENTITY, action.instance, &parameters);
        self.settle()
    }

    /// The graphs' writes of other entities' variables (`STU_0BDBC7B2` on
    /// a player: Set Player Variable `136B`), for the server to set in
    /// their scripts.
    pub fn take_variable_writes(&mut self) -> Vec<(u32, u32, Value)> {
        std::mem::take(&mut self.world.variable_writes)
    }

    /// The scope instances the graphs open or close on other entities
    /// (Teleport's on each target player), for the match to hand to the
    /// target's script ([`Component::change_scope`]); the mode entity's
    /// own it opened itself.
    pub fn take_scope_changes(&mut self) -> Vec<ScopeChange> {
        std::mem::take(&mut self.world.scope_changes)
    }

    /// The graphs the graphs played on other entities (Chase Variable's
    /// `1CCF` on each target player), for the match to play in their
    /// scripts ([`crate::controller::ControllerScript::play_script`]).
    pub fn take_play_requests(&mut self) -> Vec<PlayRequest> {
        std::mem::take(&mut self.world.plays)
    }

    /// The identifiers 2 whose map placeables the graphs removed since the
    /// last call (`STU_A3DC6A25`), for the match to remove.
    pub fn take_placeable_removals(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.world.placeable_removals)
    }

    /// The health changes the graphs made since the last call, for the
    /// match's health code (`orpheus_sim::combat::Combat::modify_health`).
    pub fn take_health_changes(&mut self) -> Vec<HealthChange> {
        std::mem::take(&mut self.world.health_changes)
    }

    /// The match opened a scope instance of a scope state here (the mode
    /// entity's, or an entity's it made) on `target`
    /// ([`Self::take_scope_changes`]).
    pub fn scope_opened(&mut self, source: ScopeSource, target: u32, instance: u16) {
        if source.entity == GAME_MODE_ENTITY {
            self.component.scope_opened(source, target, instance);
        } else {
            self.entities.scope_opened(source, target, instance);
        }
    }

    /// The entities the graphs made that are still there, for the match
    /// to create on every client with their frames
    /// ([`Self::created_script`]).
    pub fn created(&self) -> &[CreatedEntity] {
        &self.world.created
    }

    /// The script of an entity the graphs made.
    pub fn created_script(&self, entity: u32) -> Option<&PlacedEntity> {
        self.entities.entity(entity)
    }

    /// After each run, until nothing is left to hand on (at most
    /// [`SETTLE_ROUNDS`] rounds; what is left waits for the next run):
    ///
    /// - the entities the graphs made start ([`MapEntityWorld::create`]);
    /// - the scopes the scope states here asked for open and close on the
    ///   mode entity itself and on the entities it made; those on other
    ///   entities wait for the match ([`Self::take_scope_changes`]);
    /// - the messages between the mode entity and the entities it made go
    ///   to them (Destroy Effect `1CE9` sends `0DF6` to the effect, Destroy
    ///   All Effects `1CEA` to every entity); the rest wait for the match
    ///   ([`Self::take_messages`]);
    /// - the entities that removed themselves (`STU_523EE38D`: the effect's
    ///   scope 35 on `0DF6`) or that the mode entity removed (Create Icon
    ///   `1AB1`'s scope 47 on `0DFA`, [`World::destroy_entity`]) go.
    ///
    /// Then each Workshop action that reported its result and has nothing
    /// left to do retires (no work [`Component::instance_busy`] counts, no
    /// scope instance it opened on an entity it made still up, no
    /// networked action the frames still carry,
    /// [`crate::snapshot::ACTION_LIFETIME_MS`]): the engine started it for
    /// its result, and a finished instance would stay in the mode entity's
    /// frames for good, since its ClientOnlySharedVars state never ends
    /// (assumption: retail's lifetime is not known).
    fn settle(&mut self) -> Result<(), InitFault> {
        self.hand_on()?;
        let now = self.component.now_ms();
        let mut retired = Vec::new();
        let entities = &self.entities;
        let scoped = |instance: u16| {
            entities.entities().iter().any(|placed| {
                placed.snapshot().is_some_and(|snapshot| {
                    snapshot.instances.values().any(|item| {
                        item.scope.as_ref().is_some_and(|scope| {
                            scope.source.entity == GAME_MODE_ENTITY
                                && scope.source.instance == instance
                        })
                    })
                })
            })
        };
        self.actions.retain(|&action| {
            if !self
                .component
                .instance_alive(action.instance, action.start_ms)
            {
                return false;
            }
            let done = self
                .component
                .action_outcome(action.instance, action.start_ms)
                .is_some()
                && !self.component.instance_busy(action.instance)
                && !scoped(action.instance)
                && self
                    .component
                    .last_frame_action(action.instance, action.start_ms)
                    .is_none_or(|time| {
                        now.saturating_sub(time) > crate::snapshot::ACTION_LIFETIME_MS
                    });
            if done {
                retired.push(action.instance);
            }
            !done
        });
        if retired.is_empty() {
            return Ok(());
        }
        for instance in retired {
            self.component.destroy_instance(instance, &mut self.world)?;
        }
        self.hand_on()
    }

    /// [`Self::settle`]'s first part: the made entities, scopes and
    /// messages handed on.
    fn hand_on(&mut self) -> Result<(), InitFault> {
        for _ in 0..SETTLE_ROUNDS {
            let mut moved = false;
            for made in std::mem::take(&mut self.world.made) {
                self.entities.create(
                    made.entity,
                    made.definition,
                    made.position,
                    GAME_MODE_ENTITY,
                );
                self.world.created.push(made);
                moved = true;
            }
            let mut changes = std::mem::take(&mut self.world.scope_changes);
            changes.extend(self.entities.take_scope_changes());
            for change in changes {
                let target = change.target();
                let opened = if target == GAME_MODE_ENTITY {
                    self.component.change_scope(&change, &mut self.world)?
                } else if self.world.made_entity(target).is_some() {
                    self.entities.change_scope(&change)
                } else {
                    self.world.scope_changes.push(change);
                    continue;
                };
                moved = true;
                if let (Some(instance), ScopeChange::Open { source, target, .. }) =
                    (opened, &change)
                {
                    self.scope_opened(*source, *target, instance);
                }
            }
            for message in std::mem::take(&mut self.world.outbox) {
                if self.world.made_entity(message.target).is_some() {
                    self.entities.receive_message(
                        message.target,
                        message.sender,
                        message.message,
                        &message.parameters,
                    );
                    moved = true;
                    continue;
                }
                if message.target == EVERY_ENTITY {
                    self.entities.receive_everywhere(
                        message.sender,
                        message.message,
                        &message.parameters,
                    );
                    moved = true;
                }
                self.routed.push(message);
            }
            for message in self.entities.take_messages() {
                if message.target == GAME_MODE_ENTITY || message.target == EVERY_ENTITY {
                    self.component.receive_message(
                        message.sender,
                        message.message,
                        message.parameters.clone(),
                        &mut self.world,
                    )?;
                    moved = true;
                }
                if message.target == EVERY_ENTITY {
                    self.entities.receive_everywhere(
                        message.sender,
                        message.message,
                        &message.parameters,
                    );
                } else if self.world.made_entity(message.target).is_some() {
                    self.entities.receive_message(
                        message.target,
                        message.sender,
                        message.message,
                        &message.parameters,
                    );
                    moved = true;
                    continue;
                }
                if message.target != GAME_MODE_ENTITY {
                    self.routed.push(message);
                }
            }
            for end in std::mem::take(&mut self.world.ends) {
                if self.world.made_entity(end.target).is_some() {
                    self.entities
                        .end_message(end.target, end.sender, end.message);
                    moved = true;
                } else {
                    self.routed_ends.push(end);
                }
            }
            // The made entities' persistent sends end nowhere yet.
            self.entities.take_message_ends();
            let removed = std::mem::take(&mut self.world.destroyed);
            for &entity in &removed {
                self.entities.remove(entity);
            }
            for entity in removed.into_iter().chain(self.entities.take_destroyed()) {
                self.world
                    .created
                    .retain(|created| created.entity != entity);
                moved = true;
            }
            if !moved {
                break;
            }
        }
        Ok(())
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.snapshot()
    }

    pub fn unknowns(&self) -> Unknowns {
        self.world.unknowns
    }

    /// Whether the graph ran only on what the VM implements: no gap was
    /// refused (a diagnostic) and no answer was an assumption.
    pub fn complete(&self) -> bool {
        self.snapshot().is_some_and(|snapshot| {
            snapshot.initialization_complete() && snapshot.assumptions.is_empty()
        })
    }

    /// The classes the VM refused or answered by assumption, with how
    /// often, by graph: what keeps the graph from driving the match; the
    /// entities' it made with them ([`MapEntityWorld::gaps`]).
    pub fn gaps(&self) -> Gaps {
        let mut gaps = self.entities.gaps();
        let Some(snapshot) = self.snapshot() else {
            return gaps;
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
        gaps
    }

    /// Write the mode entity's variables the server owns.
    pub fn set_variables(&mut self, variables: &Variables) -> Result<(), InitFault> {
        for (&id, value) in variables {
            self.component
                .set_entity_variable(id, value.clone(), &mut self.world)?;
        }
        Ok(())
    }

    /// The match's players and its start condition now. When either
    /// changed, the states that asked about them take the change
    /// ([`Component::natives_changed`]): a waiting state tries its
    /// transition again, as the client's does when a player joins.
    pub fn set_players(&mut self, players: Vec<ModePlayer>, ready: bool) -> Result<(), InitFault> {
        if self.world.players == players && self.world.ready == ready {
            return Ok(());
        }
        self.world.players = players;
        self.world.ready = ready;
        self.component
            .natives_changed(&PLAYER_NATIVES, &mut self.world)
    }

    /// Advance to a later server frame; false for an old one.
    pub fn advance(&mut self, frame: u32) -> Result<bool, InitFault> {
        if frame <= self.frame {
            return Ok(false);
        }
        self.frame = frame;
        self.component.advance_frame(frame, &mut self.world)?;
        self.entities.advance(frame);
        self.settle()?;
        Ok(true)
    }

    /// The messages the graphs sent the match's other entities (an entity
    /// the mode entity made gets its own in [`Self::settle`]).
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        let mut messages = std::mem::take(&mut self.routed);
        messages.append(&mut self.world.take_messages());
        messages
    }

    /// The ends of the graphs' persistent sends to the match's other
    /// entities ([`World::end_message`]), for the match to route.
    pub fn take_message_ends(&mut self) -> Vec<MessageEnd> {
        let mut ends = std::mem::take(&mut self.routed_ends);
        ends.append(&mut self.world.ends);
        ends
    }

    /// The messages of [`Self::take_messages`] to the entities `to`
    /// names; the others stay for the match. The Workshop's host gives
    /// its actions' messages to the players in the step that sent them:
    /// Stop Chasing Player Variable `1CCE` sends `0DCF` to the player's
    /// `1CCF` before the rule's next action sets the variable, and Chase
    /// `1CCC` sends it before it plays the new `1CCF`.
    pub fn take_messages_to(&mut self, to: impl Fn(u32) -> bool) -> Vec<GameMessage> {
        let (mut taken, routed) = std::mem::take(&mut self.routed)
            .into_iter()
            .partition(|message| to(message.target));
        self.routed = routed;
        let (sent, outbox): (Vec<_>, _) = std::mem::take(&mut self.world.outbox)
            .into_iter()
            .partition(|message| to(message.target));
        self.world.outbox = outbox;
        taken.extend(sent);
        taken
    }

    /// The messages the graphs sent the mode entity itself since the last
    /// call (its own entries took them too).
    pub fn take_own_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.world.own)
    }

    /// The messages the graph holds to every entity now, in the order its
    /// states began ([`World::hold_broadcast`]): the match gives each to
    /// every entity that starts while it is held (`orpheus_sim::messages`).
    pub fn held(&self) -> &[GameMessage] {
        &self.world.held
    }

    /// A game message from `sender` to the mode entity (or to every
    /// entity): the graph's entries for it run now. Returns how many took
    /// it.
    pub fn receive_message(
        &mut self,
        sender: u32,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        let taken = self
            .component
            .receive_message(sender, message, parameters, &mut self.world)?;
        self.settle()?;
        Ok(taken)
    }

    /// A persistent sender (`sender`) let its message to the mode entity
    /// go ([`Component::end_message`]).
    pub fn end_message(&mut self, sender: u32, message: u64) -> Result<(), InitFault> {
        self.component.end_message(sender, message, &mut self.world)
    }
}

#[cfg(test)]
mod workshop_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codecs::CodecTable,
        snapshot::{SnapshotEncoder, View},
        timeline::FRAME_MS,
    };

    const ASSAULT: u64 = 0x0230_0000_0000_0014;
    const ESCORT: u64 = 0x0230_0000_0000_0015;
    const TEAM_DEATHMATCH: u64 = 0x0230_0000_0000_001d;
    /// The mode graph's phase (v59 on its instance 1).
    const PHASE: u32 = 59;

    fn graphs() -> Arc<GraphStore> {
        let data = orpheus_testkit::runtime_dir().join("data");
        Arc::new(GraphStore::load_runtime(data, "game_graphs_174.json.gz").unwrap())
    }

    /// One player on each of teams 0 and 1.
    fn one_a_team() -> Vec<ModePlayer> {
        (0..2)
            .map(|team| ModePlayer {
                controller: 0x100 + team as u32,
                body: Some(0x200 + team as u32),
                hero: None,
                team,
            })
            .collect()
    }

    /// Team Deathmatch's `0F60` waits in phase 0 (#59e) while its teams of
    /// one aren't full, and leaves it when the players join, without the
    /// waiting state beginning again: the default start rule `25CA` (st7's
    /// transition, rid82 → rid240 `STU_81A9AB10` → rid21 both teams'
    /// `STU_A5B9EAAD` → rid19 #59e = 1) is tried again when the players
    /// change. A change posts the check at its frame's time, so the next
    /// frame runs it: one player at frame 100 keeps phase 0 at 101 (team 1
    /// isn't full), both at 101 make phase 1 (assemble) at 102.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn a_waiting_mode_graph_starts_when_the_players_join() {
        let mut world = ModeWorld::new(graphs(), TEAM_DEATHMATCH, Unknowns::Tolerate);
        world.team_sizes = vec![(0, 1), (1, 1)];
        world.ready = true;
        let mut script = ModeScript::start(world, 0x0f60, 1).unwrap();
        let phase = |script: &ModeScript| script.snapshot().unwrap().variables.get(&PHASE).cloned();
        for frame in 2..=100 {
            script.advance(frame).unwrap();
        }
        assert_eq!(phase(&script), Some(Value::Float(0.0)));
        let mut half = one_a_team();
        half.truncate(1);
        script.set_players(half, true).unwrap();
        script.advance(101).unwrap();
        assert_eq!(phase(&script), Some(Value::Float(0.0)));
        script.set_players(one_a_team(), true).unwrap();
        script.advance(102).unwrap();
        assert_eq!(phase(&script), Some(Value::Integer(1)));
    }

    /// What a mode graph did in a match: its phase changes and the first
    /// time each gap was met, by frame.
    struct Run {
        script: ModeScript,
        phases: Vec<(u32, Value)>,
        gaps: Vec<(u32, u32, Option<u16>, String)>,
        messages: Vec<(u32, u64, u32)>,
    }

    /// Run `graph` for `mode` from frame 1 with one player a team and the
    /// start condition true, tolerating gaps, for `frames` frames.
    fn play(graphs: &Arc<GraphStore>, mode: u64, graph: u32, frames: u32) -> Run {
        let mut world = ModeWorld::new(Arc::clone(graphs), mode, Unknowns::Tolerate);
        world.players = one_a_team();
        world.team_sizes = vec![(0, 1), (1, 1)];
        world.ready = true;
        let mut run = Run {
            script: ModeScript::start(world, graph, 1).unwrap(),
            phases: Vec::new(),
            gaps: Vec::new(),
            messages: Vec::new(),
        };
        let mut seen = 0;
        for frame in 1..=frames {
            if frame > 1 {
                run.script.advance(frame).unwrap();
            }
            let snapshot = run.script.snapshot().unwrap();
            for gap in &snapshot.assumptions[seen..] {
                run.gaps
                    .push((frame, gap.site.graph, gap.site.node, gap.class.clone()));
            }
            seen = snapshot.assumptions.len();
            let phase = snapshot.instances[&1]
                .variables
                .get(&PHASE)
                .cloned()
                .unwrap_or(Value::Null);
            if run.phases.last().is_none_or(|(_, last)| *last != phase) {
                run.phases.push((frame, phase));
            }
            for message in run.script.take_messages() {
                run.messages.push((frame, message.message, message.target));
            }
        }
        run
    }

    /// How far the mode graph `0F4A` runs an Assault and an Escort match
    /// today, and the first gap it meets: the measure of what keeps it from
    /// driving the match instead of `match_flow.rs`. Debug builds refuse it.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn the_mode_graph_runs_assault_and_escort_until_its_first_gap() {
        let graphs = graphs();
        let refused = ModeScript::start(
            ModeWorld::new(Arc::clone(&graphs), ASSAULT, Unknowns::Refuse),
            0x0f4a,
            1,
        );
        assert!(refused.map_or(true, |script| !script.complete()));
        let seconds = |frame: u32| f64::from(frame) * FRAME_MS as f64 / 1000.0;
        for mode in [ASSAULT, ESCORT] {
            let run = play(&graphs, mode, 0x0f4a, 60_000);
            println!("== mode {mode:016X}");
            for (frame, phase) in &run.phases {
                println!(
                    "phase {phase:?} at frame {frame} ({:.2} s)",
                    seconds(*frame)
                );
            }
            for (frame, graph, node, class) in &run.gaps {
                println!("gap {graph:04X} node {node:?} {class} at frame {frame}");
            }
            for (frame, message, target) in run.messages.iter().take(60) {
                println!("message {message:X} to {target:X} at frame {frame}");
            }
            let snapshot = run.script.snapshot().unwrap();
            let codecs = CodecTable::client();
            let encoder = SnapshotEncoder::new(&graphs, &codecs).with_unknowns(Unknowns::Tolerate);
            let projected = encoder.project(snapshot, View::Observer).unwrap();
            assert!(encoder.full(&projected).is_ok());
        }
    }
}
