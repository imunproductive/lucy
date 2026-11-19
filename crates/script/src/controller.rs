//! The player controller's own statescript: the team's controller graph
//! (`0C90` in the PvP modes and the Practice Range `005A`, the mode's
//! `m_teams[].m_controllerScript`) run on the server against a
//! [`ControllerWorld`], as retail runs it on the player's controller entity.
//! `0C90` starts hero select (st119 → `288A` → `288B` and the team list
//! `288D`) and, for a human owner, the multikill graph `0215` (st116 →
//! st117).
//!
//! The match gives the world the controller's facts (its team, the heroes
//! it may play) and routes the messages the graphs take and send. The
//! server routes `0093.025` today: an elimination, which `0215` counts into
//! multikills ([`ControllerScript::eliminated`]), and [`HERO_SWITCHED`] on
//! each hero switch (`0C90` rid220 keeps #4513/#4514/#4515 for its
//! client-only Watch, the chat line; `288B` rid228 takes the new hero as
//! the pick: [`ControllerScript::hero_switched`]). The graphs also take
//! the client's hero select requests (21616 to `288B`: [`PICK_HERO`],
//! [`OPEN_OR_CLOSE`]) and, where the mode graph runs (the Practice Range's
//! `13C0`), [`RESPAWN_HERE`] from its `0C8F`, its answer to a `0685`: the
//! controller's own after a pick with no possessed body (st122), or the
//! body's `0C8E` at its respawn countdown's end.
//!
//! What the graphs send other entities (`06FF` and `0685` to the mode, ...)
//! waits in [`ControllerScript::take_messages`]; messages to the controller
//! itself run at once (the VM's own routing).
//!
//! Not run (IDA, enabler 4): the body the controller spawns or revives.
//! `0C90` st0 StateHero runs as a plain state, and node 4 `STU_621AF478`
//! (revive the possessed body at `0687`'s spawn point) as an assumed no-op
//! ([`NO_OP_ACTIONS`]): the server creates and possesses bodies in Rust
//! (`replication.rs`), at a pick and at the respawn `0C8E`'s `0685` asks.

use std::sync::Arc;

use serde_json::Value as Json;

use crate::{
    expr::EntityRelation,
    graph::GraphStore,
    vm::{
        Component, GameMessage, InitFault, MAX_INIT_WORK, Root, ScopeChange, Site, Snapshot,
        StateKind, TrackedStat, Unknowns, VoiceRequest, World,
    },
    wire::{Value, Variables},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

/// The PvP controller graph (and the Practice Range `005A`'s).
pub const PVP_CONTROLLER: u32 = 0x0c90;
/// `288B`, the hero select screen: the instance the client's 21616 names.
pub const HERO_SELECT_SCREEN: u32 = 0x288b;
/// The multikill graph under `0C90` st117.
pub const MULTIKILLS: u32 = 0x0215;

/// `1361.025` {`005D` hero}: the client picks a hero (to `288B`).
pub const PICK_HERO: u64 = 0x0240_0000_0000_1361;
/// `1360.025` {`05A2` open}: open or close hero select (to `288B`).
pub const OPEN_OR_CLOSE: u64 = 0x0240_0000_0000_1360;
/// `037F.025` {`021E` old hero, `0220` new hero, `0221` player}: a hero
/// switch.
pub const HERO_SWITCHED: u64 = 0x0240_0000_0000_037f;
/// `0093.025` {`0015` victim, `0012` killer, ...}: an elimination by this
/// controller.
pub const ELIMINATION: u64 = 0x0240_0000_0000_0093;
/// `0687.025` {`03A1` spawn point, `03D9` camera seconds}: the mode's
/// respawn hand-off.
pub const RESPAWN_HERE: u64 = 0x0240_0000_0000_0687;

/// Target tag `04CE.06C`: a controller with it skips `0687`'s respawn
/// camera (`0C90` st73).
const RESPAWN_CAMERA_SKIP: u64 = 0x0d60_0000_0000_04ce;
/// Native classes the world answers without knowing the answer is right.
const ASSUMED: &[(&str, &str)] = &[
    ("STU_082FA44E", "the tunable's authored value"),
    // `0C90` st116 asks it of the owner (the multikill switch: a human
    // controller, retail runs `0215` for him); `0215` rid15/rid26 of the
    // victim #393. rid15 wants a hostile victim, not the killer, with
    // neither tag `06AD` nor `031D` (its none-of list): the match applies
    // that before it sends `0093` (`orpheus_sim::announcer::counted_victim`).
    ("STU_AC829876", "passes: a human owner, a counted victim"),
    // `288B` rid255/rid256 on a pick: #34707e and #32791 (`288B`'s
    // presence variable) for ForCodeSynced st120. Meaning unknown; left
    // unset, as the hand-built frames had them.
    ("STU_9AD08FCC", "unset"),
    ("STU_F80D09DB", "unset"),
    // `288B` st95 of the possessed body: `Eval_STU_3D5F853C` 0x7FF789833D80
    // is true for an entity whose scene node (component 1) has bit 0 of
    // `+0x230` clear, and that of the entity its component 46 names (none
    // read). Answered: true for a living body; the flag's meaning is not
    // read (assumption: a dead body has it).
    ("STU_3D5F853C", "true for the living possessed body"),
    // `288B` st134 on `0738` (which `0C90` st32 sends its own controller
    // while the mode's `0727` holds and the controller has no body):
    // `!STU_A89D860A() && in spawn room` opens hero select (#2438e). A
    // bool with no fields and no client registration (the base
    // `STUConfigVar` evaluate, `xor al, al`; `tools/stu.py`): the server
    // answers it. Answered false, so a player who joins the Practice Range
    // sees hero select open in the spawn room, as retail shows him
    // (`0CD5` and `0D8E`, its only other readers, are not run here).
    (
        "STU_A89D860A",
        "false: hero select opens on the mode's 0727",
    ),
    // `265F` rid22's filter (the Training Partner pick `1213` in the
    // console's scope 126) and `0C90` rid3/rid164/rid170: the client's
    // array evaluate (slot 5 `0x7FF789C01160`) lists what its host's
    // entity's vtable +624 gives, which the server can't read. Answered:
    // the heroes the controller may play, the list `STU_1AAB4B4F` gives.
    ("STU_AFC074B3", "the heroes the controller may play"),
];

/// A hero a controller may play.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayableHero {
    pub hero: u64,
    /// Its role identifier (`STU_BE1161C0`).
    pub role: Option<u64>,
    /// Its body's entity definition (`m_gameplayEntity`, `STU_94AED9DC`).
    pub body: u64,
}

/// Actions of the controller's graphs whose work the server does in Rust.
/// `STU_621AF478` (`0C90` node 4): revive the possessed body at `0687`'s
/// spawn point #8370 with its heading, on the same hero (a hero change
/// takes StateHero st0). Server-only (no registration of its own:
/// `STUStatescriptAction`'s execute, `RespawnFacing.md`). The server makes
/// a new body at each pick and respawn instead (`replication.rs`
/// `respawn_due`), so the action does nothing here, as in a release build.
const NO_OP_ACTIONS: &[(&str, &str)] = &[(
    "STU_621AF478",
    "no-op: the server makes the respawned body itself",
)];

/// What the controller graph asks outside its own entity.
pub struct ControllerWorld {
    pub graphs: Arc<GraphStore>,
    pub mode: u64,
    /// The controller entity the graph runs on.
    pub entity: u32,
    /// The controller's team index (0 to 4).
    pub team: Option<i64>,
    /// The heroes this controller may play (`STU_1AAB4B4F`, sent to the
    /// mode in `06FF`; `STU_9AE5FC4C` tests a hero against it, then the
    /// HeroSelect limits).
    pub heroes: Vec<PlayableHero>,
    /// The body the controller possesses; none before the first spawn.
    pub body: Option<u32>,
    /// The hero in the controller's own component 75 (the last
    /// possession's: `scene::Possession`); none before the first.
    /// `STU_93DCDDFD` of the controller reads it, never the body's.
    pub hero: Option<u64>,
    /// Whether the possessed body is dead: its dead flag #478e, which
    /// `288B` st95 reads (no graph writes it; `orpheus_script::bot::DEAD_FLAG`).
    pub body_dead: bool,
    /// Whether the player may pick a hero now (`STU_E4C6AAB4`): the
    /// client reads only component 74 field 6 (ControllerWorld.md, "Client
    /// tools pass (2026-11-16)"), which the server writes from
    /// `replication.rs` `may_change_hero`; that rule feeds this.
    pub may_pick: bool,
    /// The other players' controller entities in the match, each with
    /// its team: the senders of the broadcasts this controller takes
    /// (`0C90` st4/st8/st22 test the sender's team against the owner's).
    pub players: Vec<(u32, i64)>,
    pub unknowns: Unknowns,
    outbox: Vec<GameMessage>,
    /// The voice stimuli the graphs asked the controller to say.
    voices: Vec<VoiceRequest>,
    /// The stats the graphs counted (`0215` Multikills, `0C90` Lives and
    /// Time Alive).
    tracked_stats: Vec<TrackedStat>,
}

impl ControllerWorld {
    pub fn new(graphs: Arc<GraphStore>, mode: u64, entity: u32, unknowns: Unknowns) -> Self {
        Self {
            graphs,
            mode,
            entity,
            team: None,
            heroes: Vec::new(),
            body: None,
            hero: None,
            body_dead: false,
            may_pick: true,
            players: Vec::new(),
            unknowns,
            outbox: Vec::new(),
            voices: Vec::new(),
            tracked_stats: Vec::new(),
        }
    }

    /// The heroes the controller may play, as an array of hero assets.
    fn hero_list(&self) -> Value {
        Value::Array(
            self.heroes
                .iter()
                .map(|listed| Value::Asset(listed.hero))
                .collect(),
        )
    }

    fn playable(&self, hero: u64) -> Option<&PlayableHero> {
        self.heroes.iter().find(|listed| listed.hero == hero)
    }

    fn owner(&self, config: &Json, operand: &str) -> bool {
        config
            .get(operand)
            .and_then(|value| value.get("$"))
            .and_then(Json::as_str)
            == Some("STU_B5A0CAF0")
    }

    /// `operand` names the owner's possessed body (`STU_0AC15F78` of the
    /// owner).
    fn possessed_body(&self, config: &Json, operand: &str) -> bool {
        config.get(operand).is_some_and(|body| {
            body.get("$").and_then(Json::as_str) == Some("STU_0AC15F78")
                && self.owner(body, "m_entity")
        })
    }
}

impl World for ControllerWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        ASSUMED
            .iter()
            .find(|(assumed, _)| *assumed == class)
            .map(|&(_, answer)| answer)
    }

    fn no_op_action(&self, class: &str) -> Option<&'static str> {
        NO_OP_ACTIONS
            .iter()
            .find(|(action, _)| *action == class)
            .map(|&(_, answer)| answer)
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.rulesets().default(id, 0, self.mode)
    }

    /// The mode entity's variables come from the bundle (`0C5`
    /// `m_3CE93B76`), as in `ModeWorld`; of the possessed body the world
    /// knows its dead flag #478e.
    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if self.body == Some(entity) && id == BODY_DEAD {
            return Value::Bool(self.body_dead);
        }
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.graphs
            .mode_variables(self.mode)
            .and_then(|variables| variables.get(&id))
            .copied()
            .map_or(Value::Null, Value::from)
    }

    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        let class = config.get("$")?.as_str()?;
        match class {
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            // GetGameMode.
            "STU_37F20D24" => Some(Value::Asset(self.mode)),
            "STU_1AAB4B4F" if self.owner(config, "m_controller") => Some(self.hero_list()),
            "STU_AFC074B3" => Some(self.hero_list()),
            "STU_E4C6AAB4" if self.owner(config, "m_F3E612FD") => Some(Value::Bool(self.may_pick)),
            "STU_A89D860A" => Some(Value::Bool(false)),
            // AIIsPathFollowing: a human controller follows no AI path.
            "STU_D3EB8D87" if self.owner(config, "m_00903303") => Some(Value::Bool(false)),
            "STU_AC829876" => Some(Value::Bool(true)),
            "STU_9AD08FCC" | "STU_F80D09DB" => Some(Value::Null),
            // The hero of the given entity's own component 75, else of its
            // component 123, else none: no step through the possessed body
            // (`Eval_STU_93DCDDFD` 0x7FF789BEF220; ControllerWorld.md,
            // "Client tools pass (2026-11-16)").
            "STU_93DCDDFD" if self.owner(config, "m_entity") => {
                Some(self.hero.map_or(Value::Null, Value::Asset))
            }
            "STU_3D5F853C" if self.possessed_body(config, "m_entity") => {
                Some(Value::Bool(self.body.is_some() && !self.body_dead))
            }
            _ => None,
        }
    }

    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        let known = entity == self.entity
            || entity == GAME_MODE_ENTITY
            || self.body == Some(entity)
            || self.players.iter().any(|&(player, _)| player == entity);
        known.then_some(true)
    }

    fn entity_team(&self, _site: Site, entity: u32) -> Option<i64> {
        if entity == self.entity || self.body == Some(entity) {
            return self.team;
        }
        self.players
            .iter()
            .find(|&&(player, _)| player == entity)
            .map(|&(_, team)| team)
    }

    fn hero_available(&self, _site: Site, controller: u32, hero: u64) -> Option<bool> {
        (controller == self.entity).then(|| self.heroes.iter().any(|listed| listed.hero == hero))
    }

    fn hero_role(&self, _site: Site, hero: u64) -> Option<u64> {
        self.playable(hero)?.role
    }

    /// A hero this controller may play has his body; the server knows no
    /// other's.
    fn hero_body(&self, _site: Site, hero: u64) -> Option<u64> {
        Some(self.playable(hero)?.body)
    }

    /// The one tag the server knows the controller lacks: `04CE`, which
    /// `0C90` st73 tests on `0687` (no graph gives it). Others are unknown,
    /// so a filter on them falls to [`World::native`] (`0C90` st116 asks for
    /// `0069` on the owner: the multikill switch).
    fn has_target_tag(&self, _site: Site, entity: u32, tag: u64) -> Option<bool> {
        (entity == self.entity && tag == RESPAWN_CAMERA_SKIP).then_some(false)
    }

    /// The controller possesses its body, or nothing (entity 0) before the
    /// first spawn. `STU_DB742151` is the controller itself once it
    /// possesses a body (component 44), else none; its body's is the
    /// controller. A controller has no owner (component 72); a human
    /// controller has no AI control entity.
    fn related_entity(
        &mut self,
        _site: Site,
        relation: EntityRelation,
        entity: u32,
    ) -> Option<u32> {
        let possessing = self.body.map(|_| self.entity);
        match relation {
            EntityRelation::PossessedBody if entity == self.entity => Some(self.body.unwrap_or(0)),
            EntityRelation::Controller if entity == self.entity => Some(possessing.unwrap_or(0)),
            EntityRelation::Controller | EntityRelation::CurrentPossessor
                if self.body == Some(entity) =>
            {
                possessing
            }
            EntityRelation::Owner if entity == self.entity => Some(0),
            EntityRelation::AiControlEntity if entity == self.entity => Some(0),
            _ => None,
        }
    }

    fn send_message(
        &mut self,
        _source: Site,
        target: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        self.outbox.push(GameMessage {
            sender: self.entity,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// A message to every entity (`STU_1D719351`, `0C90` st5/st21/st70):
    /// queued with the target [`crate::mode::EVERY_ENTITY`], as the mode
    /// graph's.
    fn broadcast_message(&mut self, _source: Site, message: u64, parameters: &Variables) -> bool {
        self.outbox.push(GameMessage {
            sender: self.entity,
            target: crate::mode::EVERY_ENTITY,
            message,
            parameters: parameters.clone(),
        });
        true
    }

    /// A server-only voice stimulus aimed at the controller (`0215` st28,
    /// the hero's own line 1.6 s after a third kill, `00C9.078`): the
    /// controller has no voice set, so its possessed body says it.
    fn voice_stimulus(&mut self, _site: Site, request: VoiceRequest) -> bool {
        if !request.targets.contains(&self.entity) {
            return false;
        }
        self.voices.push(request);
        true
    }

    /// The controller's stats wait for the match.
    fn track_stat(&mut self, site: Site, stat: TrackedStat) -> bool {
        if site.entity != self.entity {
            return false;
        }
        self.tracked_stats.push(stat);
        true
    }
}

/// The controller graph running on the player's controller entity.
pub struct ControllerScript {
    component: Component,
    world: ControllerWorld,
    frame: u32,
}

impl ControllerScript {
    /// Start `graph` as the controller entity's instance 1 at `frame`.
    pub fn start(mut world: ControllerWorld, graph: u32, frame: u32) -> Result<Self, InitFault> {
        let mut component = Component::start(
            Arc::clone(&world.graphs),
            world.entity,
            Variables::new(),
            vec![Root::new(1, graph)],
            frame,
            &mut world,
            MAX_INIT_WORK,
        )?;
        component.advance_frame(frame, &mut world)?;
        Ok(Self {
            component,
            world,
            frame,
        })
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.snapshot()
    }

    /// Advance to a later server frame; false for an old one.
    pub fn advance(&mut self, frame: u32) -> Result<bool, InitFault> {
        if frame <= self.frame {
            return Ok(false);
        }
        self.frame = frame;
        self.component.advance_frame(frame, &mut self.world)?;
        Ok(true)
    }

    /// A game message to the controller entity: every instance's entries
    /// for it take it now. Returns how many did.
    pub fn receive(&mut self, message: u64, parameters: Variables) -> Result<usize, InitFault> {
        self.component
            .receive_message(0, message, parameters, &mut self.world)
    }

    /// A game message the client sent one of the controller's instances
    /// at `frame` (21616 for the controller with that instance: the
    /// action `STU_79A131B4`, 0x7FF789814C20, names its host's entity and
    /// its own instance): the instance's entries for it take it now.
    /// Returns how many did.
    pub fn receive_in_instance(
        &mut self,
        frame: u32,
        instance: u16,
        message: u64,
        parameters: Variables,
    ) -> Result<usize, InitFault> {
        self.advance(frame)?;
        let entries = self
            .component
            .queue_message(instance, message, parameters)?;
        if entries != 0 {
            self.component.run_now(&mut self.world)?;
        }
        Ok(entries)
    }

    /// Open or close a scope instance on the controller at `frame`
    /// (`vm/scopes.rs`; the Training Partner console's scope 126).
    pub fn change_scope(
        &mut self,
        frame: u32,
        change: &ScopeChange,
    ) -> Result<Option<u16>, InitFault> {
        self.advance(frame)?;
        self.component.change_scope(change, &mut self.world)
    }

    /// The client's hero select request (21616) at `frame`, to `288B`'s
    /// instance as the client names it: a pick (`1361.025` {`005D` hero})
    /// or an open or close (`1360.025` {`05A2` open}). rid24 takes a pick
    /// the player may make now of a hero he may play; rid22 an open while
    /// he may pick, any close. Returns the hero select state after it, none
    /// without a `288B` instance.
    pub fn hero_select(
        &mut self,
        frame: u32,
        request: HeroSelectRequest,
    ) -> Result<Option<HeroSelectState>, InitFault> {
        self.advance(frame)?;
        let Some(screen) = self.screen() else {
            return Ok(None);
        };
        let (message, parameters) = match request {
            HeroSelectRequest::Pick(hero) => {
                (PICK_HERO, Variables::from([(0x5d, Value::Asset(hero))]))
            }
            HeroSelectRequest::OpenOrClose(open) => {
                (OPEN_OR_CLOSE, Variables::from([(0x5a2, Value::Bool(open))]))
            }
        };
        if self.component.queue_message(screen, message, parameters)? != 0 {
            self.component.run_now(&mut self.world)?;
        }
        Ok(self.hero_select_state())
    }

    /// `288B`'s instance id.
    fn screen(&self) -> Option<u16> {
        self.snapshot()?
            .instances
            .iter()
            .find(|(_, instance)| instance.graph == HERO_SELECT_SCREEN)
            .map(|(&id, _)| id)
    }

    /// What `288B` keeps on the controller entity: #2438e (hero select
    /// open), #2114e (a hero picked) and #13161e (the pick).
    pub fn hero_select_state(&self) -> Option<HeroSelectState> {
        let snapshot = self.snapshot()?;
        self.screen()?;
        let value = |id| {
            snapshot
                .resolved_bindings
                .get(&(0, id))
                .or_else(|| snapshot.variables.get(&id))
        };
        let flag = |id| matches!(value(id), Some(&Value::Bool(true)));
        let pick = match value(PICKED_HERO) {
            Some(&Value::Asset(hero)) if hero != 0 && flag(PICKED) => Some(hero),
            _ => None,
        };
        Some(HeroSelectState {
            open: flag(SELECT_OPEN),
            pick,
        })
    }

    /// The controller's player eliminated `victim` with `killer` (his body)
    /// at `frame`: `0093.025` to the controller, which `0215` counts
    /// (rid14). Parameters: `0015` the victim (#393), `0012` the killer
    /// (#392), `0091` (#1798): `final_blow`, on for the killing blow, off
    /// for an elimination the server credits for damage within the window
    /// before the death (`01CA` rid221 says the elimination line only on
    /// it, so it is the final blow; `0215` counts both, as no mode sets
    /// its #12421 that would ask for the final blow).
    pub fn eliminated(
        &mut self,
        frame: u32,
        victim: u32,
        killer: u32,
        final_blow: bool,
    ) -> Result<(), InitFault> {
        self.advance(frame)?;
        self.receive(
            ELIMINATION,
            Variables::from([
                (0x15, Value::Entity(victim)),
                (0x12, Value::Entity(killer)),
                (0x91, Value::Bool(final_blow)),
            ]),
        )?;
        Ok(())
    }

    /// The controller's player switched from hero `old` to `new` at
    /// `frame`: `037F.025` {`021E` old, `0220` new, `0221` the player's
    /// controller} to the controller. `0C90` rid220 keeps them in
    /// #4513/#4514/#4515 for its client-only Watch (the chat line);
    /// `288B` rid228 takes the new hero as the pick. Returns what `0C90`
    /// keeps, none if it did not take the message.
    pub fn hero_switched(
        &mut self,
        frame: u32,
        old: u64,
        new: u64,
        player: u32,
    ) -> Result<Option<HeroSwitch>, InitFault> {
        self.advance(frame)?;
        self.receive(
            HERO_SWITCHED,
            Variables::from([
                (0x21e, Value::Asset(old)),
                (0x220, Value::Asset(new)),
                (0x221, Value::Entity(player)),
            ]),
        )?;
        Ok(self.hero_switch())
    }

    /// `0C90`'s #4513 (old hero), #4514 (new hero) and #4515 (player).
    pub fn hero_switch(&self) -> Option<HeroSwitch> {
        let instance = self
            .snapshot()?
            .instances
            .values()
            .find(|instance| instance.graph == PVP_CONTROLLER && instance.parent.is_none())?;
        let asset = |id| match instance.variables.get(&id) {
            Some(&Value::Asset(guid)) if guid != 0 => Some(guid),
            _ => None,
        };
        let Some(&Value::Entity(player)) = instance.variables.get(&SWITCH_PLAYER) else {
            return None;
        };
        Some(HeroSwitch {
            old: asset(SWITCH_OLD_HERO)?,
            new: asset(SWITCH_NEW_HERO)?,
            player,
        })
    }

    /// `0215`'s ClientOnlyPulser st21 and #10122 once st21 has pulsed.
    pub fn multikill(&self) -> Option<Multikill> {
        let instance = self
            .snapshot()?
            .instances
            .values()
            .find(|instance| instance.graph == MULTIKILLS)?;
        let StateKind::Pulser {
            pulses,
            pulsed_ms: Some(pulsed_ms),
        } = instance.states.get(&MULTIKILL_PULSER)?.kind
        else {
            return None;
        };
        // rid72 clears #10122 to 0 on every kill before the count picks
        // a stimulus: a kill without a line leaves it so.
        let stimulus = match instance.variables.get(&MULTIKILL_STIMULUS) {
            Some(&Value::Asset(stimulus)) if stimulus != 0 => Some(stimulus),
            _ => None,
        };
        Some(Multikill {
            pulses,
            pulsed_ms,
            stimulus,
        })
    }

    /// The game messages the graphs sent other entities, for the match to
    /// route.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.world.outbox)
    }

    /// The voice stimuli the graphs asked the controller to say since the
    /// last call; its possessed body says them.
    pub fn take_voices(&mut self) -> Vec<VoiceRequest> {
        std::mem::take(&mut self.world.voices)
    }

    /// The stats the graphs counted since the last call, in order.
    pub fn take_tracked_stats(&mut self) -> Vec<TrackedStat> {
        std::mem::take(&mut self.world.tracked_stats)
    }

    /// The controller now possesses `body` playing `hero`: its components
    /// 44 and 75 (`scene::Possession`).
    pub fn possessed(&mut self, body: u32, hero: u64) {
        self.world.body = Some(body);
        self.world.hero = Some(hero);
        self.world.body_dead = false;
    }

    /// The possessed body died (or, false, lives again): its dead flag
    /// #478e, which `288B` st95 reads.
    pub fn set_body_dead(&mut self, dead: bool) {
        self.world.body_dead = dead;
    }

    /// The graphs the script runs (and its frames are projected with).
    pub fn graphs(&self) -> &GraphStore {
        &self.world.graphs
    }

    /// Component 74 field 6, which the server writes from its own rule:
    /// what `STU_E4C6AAB4` answers.
    pub fn set_may_pick(&mut self, may_pick: bool) {
        self.world.may_pick = may_pick;
    }

    /// The controller entity the graph runs on.
    pub fn entity(&self) -> u32 {
        self.world.entity
    }

    /// The controller's team.
    pub fn team(&self) -> Option<i64> {
        self.world.team
    }

    /// The other players' controllers in the match, with their teams.
    pub fn set_players(&mut self, players: Vec<(u32, i64)>) {
        self.world.players = players;
    }

    /// A message another entity sent the controller or every entity, at
    /// `frame`, from its sender. Another player's controller's `0C90` st5
    /// `068E`, st70 `08BE` and st21 `070B` (`STU_1D719351`): `0C90` st4/st8
    /// keep a teammate's pick (`08BE`) for the hero limit, st22 his role
    /// (`070B`) for the role limit. Returns how many entries took it.
    pub fn receive_from(&mut self, frame: u32, message: &GameMessage) -> Result<usize, InitFault> {
        self.advance(frame)?;
        self.component.receive_message(
            message.sender,
            message.message,
            message.parameters.clone(),
            &mut self.world,
        )
    }

    /// A persistent sender (`sender`) let its message to the controller go
    /// ([`Component::end_message`]).
    pub fn end_message(&mut self, sender: u32, message: u64) -> Result<(), InitFault> {
        self.component.end_message(sender, message, &mut self.world)
    }
}

/// `0215` st21, the ClientOnlyPulser whose client-only plug says #10122.
const MULTIKILL_PULSER: u16 = 21;
/// `0215`'s #10122: the announcer's voice stimulus (`078`).
const MULTIKILL_STIMULUS: u32 = 10122;
/// `0C90`'s hero switch variables (rid220 from `037F.025`).
const SWITCH_OLD_HERO: u32 = 4513;
const SWITCH_NEW_HERO: u32 = 4514;
const SWITCH_PLAYER: u32 = 4515;
/// `288B`'s entity variables: hero select open (#2438e), a hero picked
/// (#2114e) and the pick (#13161e).
const SELECT_OPEN: u32 = 2438;
const PICKED: u32 = 2114;
const PICKED_HERO: u32 = 13161;
/// The possessed body's dead flag #478e (`crate::bot::DEAD_FLAG`).
const BODY_DEAD: u32 = crate::bot::DEAD_FLAG;

/// The client's hero select request (21616 to `288B`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeroSelectRequest {
    /// `1361.025` {`005D` hero}.
    Pick(u64),
    /// `1360.025` {`05A2` open}.
    OpenOrClose(bool),
}

/// `288B`'s hero select state: whether the screen is open (#2438e) and
/// the hero picked (#13161e while #2114e).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeroSelectState {
    pub open: bool,
    pub pick: Option<u64>,
}

/// The last hero switch `0C90` keeps: the old and new hero and the
/// switching player's controller entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeroSwitch {
    pub old: u64,
    pub new: u64,
    pub player: u32,
}

/// What `0215` has for the announcer: st21's pulse count and the clock of
/// its last pulse, and #10122.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Multikill {
    pub pulses: u8,
    pub pulsed_ms: u64,
    /// #10122; none once a kill without a line cleared it.
    pub stimulus: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::FRAME_MS;

    const ESCORT: u64 = 0x0230_0000_0000_0015;
    const RANGE: u64 = 0x0230_0000_0000_005a;
    const SOLDIER: u64 = 0x02e0_0000_0000_006e;
    const CONTROLLER: u32 = 0x10;

    fn start(mode: u64) -> ControllerScript {
        let root = orpheus_testkit::runtime_dir();
        let graphs = Arc::new(GraphStore::load(root.join("data/game_graphs_174.json.gz")).unwrap());
        let mut world = ControllerWorld::new(graphs, mode, CONTROLLER, Unknowns::Refuse);
        world.team = Some(0);
        world.heroes = vec![playable(SOLDIER)];
        ControllerScript::start(world, PVP_CONTROLLER, 1).unwrap()
    }

    /// `hero` as the catalog lists him.
    fn playable(hero: u64) -> PlayableHero {
        let root = orpheus_testkit::runtime_dir();
        let catalog =
            orpheus_content::heroes::HeroCatalog::load(root.join("data/game_heroes_174.json"))
                .unwrap();
        let listed = catalog.get(hero).unwrap();
        PlayableHero {
            hero,
            role: listed.role_guid(),
            body: listed.body,
        }
    }

    /// The client's 21616 to `288B` in the VM (Range): an open sets #2438e
    /// (rid22); a pick sets #13161e/#2114e (rid24) and the screen stays
    /// open until the picked hero's body lives (st95, a later frame); an
    /// open clears the pick; a dead player's pick closes at once (st95's
    /// dead branch); without `may_pick` a pick is refused and st98 closes
    /// the screen, and an open is refused (rid22). Every step projects to an
    /// owner full frame.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn client_requests_drive_hero_select_in_the_vm() {
        use crate::codecs::CodecTable;
        use crate::snapshot::{SnapshotEncoder, View};
        use orpheus_content::heroes::{ANA, DAMAGE_ROLE};
        let state = |open, pick| Some(HeroSelectState { open, pick });
        let codecs = CodecTable::client();
        let projects = |script: &ControllerScript| {
            let snapshot = script.snapshot().unwrap();
            assert!(
                snapshot.initialization_complete(),
                "{:?}",
                snapshot.diagnostics
            );
            let encoder = SnapshotEncoder::new(script.graphs(), &codecs);
            let projected = encoder.project(snapshot, View::Owner).unwrap();
            assert_eq!(projected.instance_count(), 5);
            encoder.owner_full(&projected, 1234, 16).unwrap();
        };
        let mut script = start(RANGE);
        script.world.heroes.push(playable(ANA));
        script.advance(10).unwrap();
        assert_eq!(script.hero_select_state(), state(false, None));
        projects(&script);
        let open = script.hero_select(20, HeroSelectRequest::OpenOrClose(true));
        assert_eq!(open.unwrap(), state(true, None));
        let pick = script.hero_select(30, HeroSelectRequest::Pick(SOLDIER));
        assert_eq!(pick.unwrap(), state(true, Some(SOLDIER)));
        projects(&script);
        // `070B` with the hero's role (node 44, `STU_BE1161C0`) and `08BE`
        // with the pick go to every entity.
        let broadcasts: Vec<_> = script
            .take_messages()
            .into_iter()
            .filter(|message| message.target == crate::mode::EVERY_ENTITY)
            .map(|message| (message.message & 0xffff, message.parameters))
            .collect();
        let role = Variables::from([(0x3c9, Value::Asset(DAMAGE_ROLE))]);
        assert!(broadcasts.contains(&(0x070b, role)), "{broadcasts:?}");
        let picked = Variables::from([(0x5d, Value::Asset(SOLDIER))]);
        assert!(broadcasts.contains(&(0x08be, picked)), "{broadcasts:?}");
        script.possessed(0x20, SOLDIER);
        script.advance(40).unwrap();
        assert_eq!(script.hero_select_state(), state(false, Some(SOLDIER)));
        let reopen = script.hero_select(50, HeroSelectRequest::OpenOrClose(true));
        assert_eq!(reopen.unwrap(), state(true, None));
        let close = script.hero_select(60, HeroSelectRequest::OpenOrClose(false));
        assert_eq!(close.unwrap(), state(false, None));
        script.set_body_dead(true);
        script
            .hero_select(70, HeroSelectRequest::OpenOrClose(true))
            .unwrap();
        let dead = script.hero_select(80, HeroSelectRequest::Pick(ANA));
        assert_eq!(dead.unwrap(), state(false, Some(ANA)));
        script.possessed(0x21, ANA);
        script
            .hero_select(90, HeroSelectRequest::OpenOrClose(true))
            .unwrap();
        script.set_may_pick(false);
        let refused = script.hero_select(100, HeroSelectRequest::Pick(SOLDIER));
        assert_eq!(refused.unwrap(), state(false, None));
        let closed = script.hero_select(110, HeroSelectRequest::OpenOrClose(true));
        assert_eq!(closed.unwrap(), state(false, None));
        projects(&script);
    }

    /// The mode's respawn hand-off in the Practice Range: a pick with no
    /// possessed body sends `0685` {`0221` the controller} to the mode
    /// (`0C90` st122), and `13C0`'s `0C8F` answers with `0687` {`03A1` the
    /// spawn point, `03D9` null}. #940e isn't the pick yet (rid3 true), so
    /// #940e = #12511 and StateHero st0 begins (an assumed plain state). The
    /// next `0687` (a respawn's) finds the same hero (rid3 false): rid61 →
    /// node 4 revives the possessed body there (an assumed no-op), then
    /// StateHero begins again. The server makes the body itself in both
    /// cases, and the frames still project.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn the_mode_s_respawn_hand_off_runs_with_no_gap() {
        use crate::codecs::CodecTable;
        use crate::snapshot::{SnapshotEncoder, View};
        let codecs = CodecTable::client();
        let projects = |script: &ControllerScript| {
            let snapshot = script.snapshot().unwrap();
            assert!(
                snapshot.initialization_complete(),
                "{:?}",
                snapshot.diagnostics
            );
            let encoder = SnapshotEncoder::new(script.graphs(), &codecs);
            let projected = encoder.project(snapshot, View::Owner).unwrap();
            encoder.owner_full(&projected, 1234, 16).unwrap();
        };
        let assumed = |script: &ControllerScript, class: &str| {
            script
                .snapshot()
                .unwrap()
                .assumptions
                .iter()
                .any(|assumption| assumption.class == class)
        };
        let mut script = start(RANGE);
        script.advance(10).unwrap();
        script
            .hero_select(20, HeroSelectRequest::OpenOrClose(true))
            .unwrap();
        script
            .hero_select(30, HeroSelectRequest::Pick(SOLDIER))
            .unwrap();
        let request = Variables::from([(0x221, Value::Entity(CONTROLLER))]);
        assert!(script.take_messages().iter().any(|message| {
            (
                message.message & 0xffff,
                message.target,
                &message.parameters,
            ) == (0x0685, GAME_MODE_ENTITY, &request)
        }));
        script.possessed(0x20, SOLDIER);
        let spawn = Variables::from([(0x3a1, Value::Entity(0x8000_0254)), (0x3d9, Value::Null)]);
        script.receive(RESPAWN_HERE, spawn.clone()).unwrap();
        assert!(assumed(&script, "STUStatescriptStateHero"));
        assert!(!assumed(&script, "STU_621AF478"));
        projects(&script);
        script.receive(RESPAWN_HERE, spawn).unwrap();
        assert!(assumed(&script, "STU_621AF478"));
        projects(&script);
    }

    /// `0C90` starts on the controller in the Practice Range and in a PvP
    /// mode with no gap the VM refuses: hero select (`288A` under st119,
    /// `288B` and `288D` under it) and, for a human owner, `0215` under
    /// st117. Its first message is `06FF` to the mode with the heroes the
    /// player may play (`03BE`) and his team (`008D`).
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn the_controller_graph_starts_hero_select_and_the_multikills() {
        for mode in [RANGE, ESCORT] {
            let mut script = start(mode);
            script.advance(10).unwrap();
            let snapshot = script.snapshot().unwrap();
            assert!(
                snapshot.initialization_complete(),
                "{mode:x}: {:?}",
                snapshot.diagnostics
            );
            let tree: Vec<_> = snapshot
                .instances
                .values()
                .map(|instance| (instance.graph, instance.parent))
                .collect();
            let parent = |graph| {
                snapshot
                    .instances
                    .iter()
                    .find(|(_, instance)| instance.graph == graph)
                    .map(|(&id, _)| id)
                    .unwrap()
            };
            for (graph, under, state) in [
                (0x288a, PVP_CONTROLLER, 119),
                (HERO_SELECT_SCREEN, 0x288a, 0),
                (0x288d, 0x288a, 1),
                (MULTIKILLS, PVP_CONTROLLER, 117),
            ] {
                assert!(
                    tree.contains(&(graph, Some((parent(under), state)))),
                    "{mode:x}: {graph:04X} in {tree:?}"
                );
            }
            let messages = script.take_messages();
            assert_eq!(
                messages,
                [GameMessage {
                    sender: CONTROLLER,
                    target: GAME_MODE_ENTITY,
                    message: 0x0240_0000_0000_06ff,
                    parameters: Variables::from([
                        (0x3be, Value::Array(vec![Value::Asset(SOLDIER)])),
                        (0x8d, Value::Integer(0)),
                    ]),
                }]
            );
        }
    }

    /// `0215`'s rule (rid14): each elimination adds 1 to #2023 and restarts
    /// st4's wait of #2036 = 2 s; kills 2 to 6 of a streak set #10122 to
    /// `058A` (double) to `058E` and pulse st21; kill 1 and kill 7 don't.
    /// The wait's end resets the count: a kill 125 ticks (2000 / 16 ms)
    /// after the last starts over, at 124 it still counts.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn kills_within_two_seconds_of_each_other_pulse_the_announcer() {
        const WINDOW: u32 = (2000 / FRAME_MS) as u32;
        // A message after `advance(tick)` runs at the frame's last
        // millisecond, tick × 16 + 15: the pulse's clock.
        let pulsed = |tick: u32| u64::from(tick) * FRAME_MS + FRAME_MS - 1;
        let mut script = start(RANGE);
        let mut tick = 1000;
        script.eliminated(tick, 0x20, CONTROLLER | 1, true).unwrap();
        assert_eq!(script.multikill(), None);
        for (pulses, stimulus) in (1_u8..).zip(0x058a_u64..=0x058e) {
            tick += WINDOW - 1;
            script.eliminated(tick, 0x20, CONTROLLER | 1, true).unwrap();
            assert_eq!(
                script.multikill(),
                Some(Multikill {
                    pulses,
                    pulsed_ms: pulsed(tick),
                    stimulus: Some(0x0ee0_0000_0000_0000 | stimulus),
                }),
                "kill {}",
                pulses + 1
            );
        }
        // A seventh kill has no line: st21 keeps its count, #10122 is
        // cleared (rid72).
        let quiet = script.multikill().map(|pulse| Multikill {
            stimulus: None,
            ..pulse
        });
        tick += 1;
        script.eliminated(tick, 0x20, CONTROLLER | 1, true).unwrap();
        assert_eq!(script.multikill(), quiet, "the seventh kill has no line");
        assert_eq!(script.take_tracked_stats(), [], "the streak runs on");
        tick += WINDOW;
        script.eliminated(tick, 0x20, CONTROLLER | 1, true).unwrap();
        assert_eq!(script.multikill(), quiet, "a new streak's first kill");
        // The streak's wait ended at 7 kills (rid29: 3 or more): its
        // TrackStats count it, rid28 the streak's length into the two
        // "Multikill - Best" stats and rid27 one Multikill, for the owner.
        let stat = |index: u64| 0x0860_0000_0000_0000 | index;
        assert_eq!(
            script.take_tracked_stats(),
            [
                TrackedStat {
                    stats: vec![stat(0x0348), stat(0x0346)],
                    amount: 7.0,
                    targets: vec![CONTROLLER],
                },
                TrackedStat {
                    stats: vec![stat(0x0345)],
                    amount: 1.0,
                    targets: vec![CONTROLLER],
                },
            ]
        );
        tick += 1;
        script.eliminated(tick, 0x20, CONTROLLER | 1, true).unwrap();
        assert_eq!(
            script.multikill(),
            Some(Multikill {
                pulses: 6,
                pulsed_ms: pulsed(tick),
                stimulus: Some(0x0ee0_0000_0000_058a),
            })
        );
        assert!(script.snapshot().unwrap().initialization_complete());
    }

    /// `037F.025` to the controller: `0C90` rid220 keeps the old hero, the
    /// new one and the player in #4513/#4514/#4515 (the chat line's
    /// Watch), each switch replacing the last.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn a_hero_switch_lands_in_the_controller_s_variables() {
        use orpheus_content::heroes::ANA;
        for mode in [RANGE, ESCORT] {
            let mut script = start(mode);
            assert_eq!(script.hero_switch(), None);
            let first = script.hero_switched(20, SOLDIER, ANA, CONTROLLER).unwrap();
            assert_eq!(
                first,
                Some(HeroSwitch {
                    old: SOLDIER,
                    new: ANA,
                    player: CONTROLLER,
                }),
                "{mode:x}"
            );
            let back = script.hero_switched(30, ANA, SOLDIER, CONTROLLER).unwrap();
            assert_eq!(
                back,
                Some(HeroSwitch {
                    old: ANA,
                    new: SOLDIER,
                    player: CONTROLLER,
                })
            );
            // rid228 makes the new hero the pick, so `0C90` st21 broadcasts
            // `070B` with the hero's role (`STU_BE1161C0`, node 44): no gap.
            let gaps: Vec<_> = script
                .snapshot()
                .unwrap()
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.site.graph, diagnostic.site.node))
                .collect();
            assert!(gaps.is_empty(), "{mode:x}: {gaps:?}");
        }
    }
}
