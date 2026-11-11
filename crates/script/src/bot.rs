//! The statescript of an NPC body, shared by all observers, as the Python
//! reference ran it (`ow174/game/script/bot.py`): the body's graphs, the
//! Practice Range firing behavior, the AI weapon buttons, the practice
//! nameplate and the bot's death.
//!
//! [`BotBody`] mirrors `BotScript`; the match owns the observer streams and
//! projects [`BotBody::snapshot`] with [`BotBody::encoder`].

use std::{error::Error, fmt, ops::Range, sync::Arc};

use serde_json::Value as Json;

use crate::{
    codecs::CodecTable,
    expr::EvalError,
    graph::{GraphStore, Node},
    snapshot::SnapshotEncoder,
    vm::{
        Component, InitFault, MAX_INIT_WORK, ShotEvent, Site, Snapshot, Unknowns, World, body_roots,
    },
    wire::{Value, Variables},
};
use orpheus_content::maps::PRACTICE_RANGE_MODE;
use orpheus_wire::entity::GAME_MODE_ENTITY;

/// `003E.025`: the body's authoritative death message.
pub const DIED: u64 = 0x0240_0000_0000_003e;
/// `0013.025`: the placement's eligibility reset.
pub const RESET: u64 = 0x0240_0000_0000_0013;
/// The Practice Range firing placement behavior (`FIRING` in
/// `ow174/game/bots.py`).
pub const FIRING_GRAPH: u32 = 0x0b92;
pub const PRACTICE_NAMEPLATE: u32 = 0x0d24;
pub const PRACTICE_NAMEPLATE_PARENT: u32 = 0x0c18;
const PRACTICE_BOT_HEROES: Range<u64> = 0x02e0_0000_0000_016b..0x02e0_0000_0000_016f;
const OWNER_HEALTH: u32 = 253;

/// The practice nameplate instance: graph `0D24` under state 3 of a `0C18`
/// instance (`practice_nameplate` in bot.py).
pub fn practice_nameplate(snapshot: &Snapshot) -> Option<u16> {
    snapshot.instances.iter().find_map(|(&id, instance)| {
        let (parent, state) = instance.parent?;
        (instance.graph == PRACTICE_NAMEPLATE
            && state == 3
            && snapshot
                .instances
                .get(&parent)
                .is_some_and(|parent| parent.graph == PRACTICE_NAMEPLATE_PARENT))
        .then_some(id)
    })
}

/// What an NPC body's graphs ask outside the body (`BotWorld` in bot.py).
/// The rest answers as the reference's base world: ruleset defaults, the
/// mode entity's variables, nothing else.
pub struct BotWorld {
    pub graphs: Arc<GraphStore>,
    pub hero: u64,
    pub mode: u64,
    /// The logical button each AI weapon slot holds (the body's
    /// `ai_weapon_buttons`).
    pub weapon_buttons: Vec<(i64, u8)>,
    /// Where the firing behavior's owner stands; none without the behavior.
    pub owner_position: Option<[f64; 3]>,
    pub unknowns: Unknowns,
}

impl World for BotWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        // As `PlayerWorld`: the tunable's authored value, not scaled for the
        // entity.
        (class == "STU_082FA44E").then_some("the tunable's authored value")
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.rulesets().default(id, self.hero, self.mode)
    }

    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.graphs
            .mode_variables(self.mode)
            .and_then(|variables| variables.get(&id))
            .copied()
            .map_or(Value::Null, Value::from)
    }

    fn ai_weapon_button(&mut self, slot: i64) -> Option<u8> {
        self.weapon_buttons
            .iter()
            .find_map(|&(item, button)| (item == slot).then_some(button))
    }

    /// The reconstruction policy for the single authored firing placement:
    /// only its graph selects, and only with an owner position.
    fn selects_ai_behavior(&mut self, site: Site) -> Option<bool> {
        Some(self.owner_position.is_some() && site.graph == FIRING_GRAPH)
    }

    fn native(&mut self, site: Site, config: &Json) -> Option<Value> {
        let class = config.get("$")?.as_str()?;
        if class == "STU_AC829876"
            && self.mode == PRACTICE_RANGE_MODE
            && PRACTICE_BOT_HEROES.contains(&self.hero)
            && site.graph == PRACTICE_NAMEPLATE_PARENT
            && is_owner(config.get("m_entity"))
            && config.get("m_filter").is_some_and(is_nameplate_filter)
        {
            // The stock 0C18 owner gate for its authored 0D24 child. What
            // filter flag 0x100 means natively is unverified.
            return Some(Value::Bool(true));
        }
        if self.selects_ai_behavior(site) == Some(true) {
            let owner = config.get("m_entity");
            let possessed = owner
                .filter(|owner| owner.get("$").and_then(Json::as_str) == Some("STU_0AC15F78"))
                .and_then(|owner| owner.get("m_entity"))
                .is_some_and(|entity| {
                    entity.get("$").and_then(Json::as_str) == Some("STU_B5A0CAF0")
                });
            if possessed {
                match class {
                    "STU_71A87E92" => return self.owner_position.map(Value::Vector),
                    // This NPC has no player possessor.
                    "STU_295D117D" => return Some(Value::Null),
                    _ => (),
                }
            }
        }
        match class {
            "STU_A3DEF368" if self.graphs.mode_variables(self.mode).is_some() => {
                Some(Value::Entity(GAME_MODE_ENTITY))
            }
            // See `assumption`.
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            _ => None,
        }
    }
}

/// `{"$": "STU_B5A0CAF0"}`, optionally with a zero `m_EE729DCB`.
fn is_owner(config: Option<&Json>) -> bool {
    config.and_then(Json::as_object).is_some_and(|fields| {
        fields.get("$").and_then(Json::as_str) == Some("STU_B5A0CAF0")
            && fields.iter().all(|(key, value)| match key.as_str() {
                "$" => true,
                "m_EE729DCB" => value.as_u64() == Some(0),
                _ => false,
            })
    })
}

/// The stock filter: its set fields (not null, not zero) are exactly the
/// candidate mask and the 0x100 exclusion.
fn is_nameplate_filter(filter: &Json) -> bool {
    let Some(fields) = filter.as_object() else {
        return false;
    };
    let set = |value: &Json| match value {
        Json::Null | Json::Bool(false) => false,
        Json::Number(number) => number.as_f64() != Some(0.0),
        _ => true,
    };
    let mut expected = [("$", false), ("m_84065FD1", false), ("m_4EA686DB", false)];
    for (key, value) in fields.iter().filter(|(_, value)| set(value)) {
        let matches = match key.as_str() {
            "$" => value.as_str() == Some("STUConfigVarFilter"),
            "m_84065FD1" => value.as_u64() == Some(0x3fff_ffff),
            "m_4EA686DB" => value.as_u64() == Some(0x100),
            _ => false,
        };
        let Some(seen) = expected.iter_mut().find(|(name, _)| name == key) else {
            return false;
        };
        if !matches {
            return false;
        }
        seen.1 = true;
    }
    expected.iter().all(|(_, seen)| *seen)
}

#[derive(Debug, Eq, PartialEq)]
pub enum BotError {
    MissingBody(u64),
    DeathOrder { frame: u32, last: u32 },
    Script(InitFault),
}

impl fmt::Display for BotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBody(body) => write!(f, "no statescript data for entity body {body:016X}"),
            Self::DeathOrder { frame, last } => {
                write!(
                    f,
                    "bot death at frame {frame} predates its script frame {last}"
                )
            }
            Self::Script(error) => error.fmt(f),
        }
    }
}

impl Error for BotError {}

impl From<InitFault> for BotError {
    fn from(error: InitFault) -> Self {
        Self::Script(error)
    }
}

/// Where and as what an NPC body starts.
#[derive(Clone, Copy, Debug)]
pub struct BotSpawn {
    /// The entity body definition (`0x0400…`).
    pub body: u64,
    pub hero: u64,
    pub entity: u32,
    pub frame: u32,
    pub mode: u64,
    pub health: f64,
    /// The firing placement's position; none for a bot without one.
    pub firing_position: Option<[f64; 3]>,
}

/// An NPC body's statescript on the server clock (`BotScript` in bot.py).
pub struct BotBody {
    component: Component,
    world: BotWorld,
    behavior: Option<u16>,
    frame: u32,
    death_frame: Option<u32>,
}

impl BotBody {
    /// Run the body's graphs through their spawn frame and, with a firing
    /// position and AI weapon buttons, start the firing behavior with its
    /// scoped reset.
    pub fn start(
        graphs: Arc<GraphStore>,
        spawn: BotSpawn,
        unknowns: Unknowns,
    ) -> Result<Self, BotError> {
        let body = graphs
            .entity_body(spawn.body)
            .ok_or(BotError::MissingBody(spawn.body))?;
        let weapon_buttons: Vec<_> = body
            .ai_weapon_buttons
            .iter()
            .filter_map(|(slot, button)| {
                Some((slot.parse().ok()?, u8::try_from(button.as_u64()?).ok()?))
            })
            .collect();
        let firing = !weapon_buttons.is_empty() && graphs.graph(FIRING_GRAPH).is_some();
        let roots = body_roots(body)?;
        let mut world = BotWorld {
            graphs: Arc::clone(&graphs),
            hero: spawn.hero,
            mode: spawn.mode,
            weapon_buttons,
            owner_position: spawn.firing_position.filter(|_| firing),
            unknowns,
        };
        let mut component = Component::start(
            graphs,
            spawn.entity,
            Variables::from([(OWNER_HEALTH, Value::Float(spawn.health))]),
            roots,
            spawn.frame,
            &mut world,
            MAX_INIT_WORK,
        )?;
        component.advance_frame(spawn.frame, &mut world)?;
        let mut behavior = None;
        if world.owner_position.is_some() {
            behavior = component.add_instance(FIRING_GRAPH, &mut world)?;
            if let Some(id) = behavior {
                // The reconstructed spawn reset goes to this instance only;
                // other 0013 receivers have their own rules.
                component.queue_message(id, RESET, Variables::new())?;
                component.run_now(&mut world)?;
            }
        }
        Ok(Self {
            component,
            world,
            behavior,
            frame: spawn.frame,
            death_frame: None,
        })
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.snapshot()
    }

    /// The observer frames' encoder: it leaves the firing behavior out.
    pub fn encoder<'a>(
        &'a self,
        graphs: &'a GraphStore,
        codecs: &'a CodecTable,
    ) -> SnapshotEncoder<'a> {
        SnapshotEncoder::new(graphs, codecs)
            .with_unknowns(self.world.unknowns)
            .hiding(self.behavior.as_slice())
    }

    pub fn component(&self) -> &Component {
        &self.component
    }

    pub fn world(&self) -> &BotWorld {
        &self.world
    }

    /// The firing behavior's instance while it runs.
    pub fn behavior(&self) -> Option<u16> {
        self.behavior
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn death_frame(&self) -> Option<u32> {
        self.death_frame
    }

    pub fn take_shots(&mut self) -> Vec<ShotEvent> {
        self.component.take_shots().unwrap_or_default()
    }

    /// Read the authored volley a shot came from, evaluating its configs in
    /// the shot's state (`state.evaluate` in combat.py `_shoot`). None: the
    /// shot's state is no longer a weapon volley of this body.
    pub fn with_shot<T>(
        &mut self,
        shot: &ShotEvent,
        read: impl FnOnce(&Node, &mut dyn FnMut(&Json) -> Result<Value, EvalError>) -> T,
    ) -> Option<T> {
        let Self {
            component, world, ..
        } = self;
        let node = component.shot_node(shot)?;
        let mut evaluate = |config: &Json| component.evaluate_shot_config(shot, config, world);
        Some(read(node, &mut evaluate))
    }

    /// The volley count of the state a shot came from, which the other
    /// clients' shot record names (projectiles.py `_show_others`:
    /// `state.volleys`); 0 when that state is gone.
    pub fn shot_volleys(&self, shot: &ShotEvent) -> u8 {
        self.component
            .snapshot()
            .and_then(|snapshot| snapshot.instances.get(&shot.instance))
            .and_then(|instance| instance.states.get(&shot.raw_state))
            .map_or(0, |state| match state.kind {
                crate::vm::StateKind::WeaponVolley { volleys, .. } => volleys,
                _ => 0,
            })
    }

    /// The owner's health input #253 (`set_health` in driver.py), kept in
    /// step with the bot's pools after each hit (combat.py
    /// `_sync_body_health`).
    pub fn set_health(&mut self, health: f64) -> Result<(), BotError> {
        self.component
            .set_entity_variable(OWNER_HEALTH, Value::Float(health), &mut self.world)?;
        Ok(())
    }

    /// Advance a live body one server frame; the firing behavior's owner
    /// moves to `position` (`BotScript.advance_live`). False: nothing ran
    /// (an old frame or a dead bot).
    pub fn advance_live(
        &mut self,
        frame: u32,
        position: Option<[f64; 3]>,
    ) -> Result<bool, BotError> {
        if frame <= self.frame || self.component.owner_dead() {
            return Ok(false);
        }
        self.frame = frame;
        if let Some(position) = position.filter(|_| self.behavior.is_some()) {
            self.world.owner_position = Some(position);
        }
        self.component.advance_frame(frame, &mut self.world)?;
        Ok(true)
    }

    /// Deliver the death message (`BotScript.die`): the behavior goes, the
    /// shots and held buttons are dropped, the frame runs, health is 0, and
    /// `003E.025` runs now. A bot that dies in its spawn frame runs the death
    /// in the next frame, after its initial frame. False: it was already dead
    /// or no entry took the message. The caller freezes the last live
    /// observer baseline before calling this.
    pub fn die(&mut self, frame: u32, parameters: Variables) -> Result<bool, BotError> {
        if self.component.owner_dead() {
            return Ok(false);
        }
        if frame < self.frame {
            return Err(BotError::DeathOrder {
                frame,
                last: self.frame,
            });
        }
        self.frame = frame.max(self.frame.saturating_add(1));
        self.component.set_owner_dead(true);
        if let Some(behavior) = self.behavior.take() {
            self.component.destroy_instance(behavior, &mut self.world)?;
        }
        self.component.discard_shots();
        self.component.advance_frame(self.frame, &mut self.world)?;
        self.component.release_buttons();
        self.component
            .set_entity_variable(OWNER_HEALTH, Value::Float(0.0), &mut self.world)?;
        if self
            .component
            .receive_message(DIED, parameters, &mut self.world)?
            == 0
        {
            return Ok(false);
        }
        self.death_frame = Some(frame);
        Ok(true)
    }

    /// Run the graph events due within the death's lifetime, frame by frame
    /// up to `frame` (`BotScript.advance_death`); `each` sees the body after
    /// every frame that ran.
    pub fn advance_death(
        &mut self,
        frame: u32,
        lifetime_frames: u32,
        mut each: impl FnMut(u32, &Snapshot),
    ) -> Result<(), BotError> {
        let Some(death) = self.death_frame else {
            return Ok(());
        };
        let limit = frame.min(death.saturating_add(lifetime_frames));
        while let Some(due) = self.component.next_due_ms() {
            let Ok(due) = u32::try_from(due / crate::timeline::FRAME_MS) else {
                break;
            };
            if due > limit {
                break;
            }
            self.frame = due;
            self.component.advance_frame(due, &mut self.world)?;
            if let Some(snapshot) = self.component.snapshot() {
                each(due, snapshot);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BODY: u64 = 0x0400_0000_0000_1403;
    const TRAINING_BOT: u64 = 0x02e0_0000_0000_016b;
    const ENTITY: u32 = 0xc000_1000;
    const WEAPON: u32 = 0xf0fe;
    const LIVE: u32 = 32;

    fn node(position: u16, class: &str, state: Option<u16>, mut fields: Json) -> Json {
        fields["m_2BBEEAB8"] = json!(state.is_some());
        fields["m_ADEB6E05"] = json!(class.contains("Action"));
        json!({
            "pos":position, "class":class, "uid":position, "rid":position, "remote":-1,
            "state":state, "client_only":false, "server_only":false, "fields":fields,
        })
    }

    fn plug(target: u16, input: &str) -> Json {
        json!({"$":"Plug","links":[[target, input]],"m_FF3DAF1E":[]})
    }

    fn on_message(position: u16, message: u64, target: u16, input: &str) -> Json {
        node(
            position,
            "STUStatescriptEntryGameMessage",
            None,
            json!({
                "m_gameMessage":{"m_gameMessage":format!("0x{message:016X}")},
                "m_outPlug":plug(target, input),
            }),
        )
    }

    fn mark(position: u16) -> Json {
        node(
            position,
            "STUStatescriptActionSetVar",
            None,
            json!({
                "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":LIVE},
                "m_value":{"$":"STUConfigVarBool","m_value":1},
            }),
        )
    }

    fn graph(index: u32, nodes: Vec<Json>, states: Json, entries: Json) -> Json {
        json!({
            "index":index, "items":nodes.len(), "nodes":nodes, "states":states,
            "entries":entries, "remote":[], "sync_vars":[], "containers":{},
            "unknown_nodes":[], "fields":{"m_680A2CB2":[0]},
            "header":{
                "m_nodesBitCount":4, "m_statesBitCount":4, "m_remoteSyncNodesBitCount":4,
                "m_syncVarsBitCount":4
            }
        })
    }

    /// A weapon whose button 20 starts a volley, and a firing behavior whose
    /// reset holds AI weapon slot 0 (button 20) down.
    fn graphs() -> Arc<GraphStore> {
        let weapon = graph(
            WEAPON,
            vec![
                node(
                    0,
                    "STUStatescriptEntry",
                    None,
                    json!({"m_outPlug":plug(1, "m_beginPlug")}),
                ),
                node(
                    1,
                    "STUStatescriptStateLogicalButton",
                    Some(0),
                    json!({
                        "m_logicalButton":{"$":"STUConfigVarInt","m_value":20},
                        "m_onGoingDownPlug":plug(2, "m_beginPlug"),
                    }),
                ),
                node(2, "STUStatescriptStateWeaponVolley", Some(1), json!({})),
                on_message(3, RESET, 5, "m_inPlug"),
                on_message(4, DIED, 5, "m_inPlug"),
                mark(5),
            ],
            json!([1, 2]),
            json!([0, 3, 4]),
        );
        let firing = graph(
            FIRING_GRAPH,
            vec![
                on_message(0, RESET, 1, "m_inPlug"),
                node(
                    1,
                    "STUStatescriptActionSetVar",
                    None,
                    json!({
                        "m_out_Var":{"$":"STUConfigVarDynamic","m_identifier":LIVE},
                        "m_value":{"$":"STUConfigVarBool","m_value":1},
                        "m_outPlug":plug(2, "m_beginPlug"),
                    }),
                ),
                node(
                    2,
                    "STU_D7F9E1C3",
                    Some(0),
                    json!({
                        "m_14EDDDA7":{"$":"STU_B5A0CAF0"},
                        "m_20BCEE62":{"$":"STUConfigVarBool","m_value":1},
                        "m_A60150E2":{"$":"STUConfigVarInt","m_value":0},
                    }),
                ),
            ],
            json!([2]),
            json!([0]),
        );
        let body = json!({
            "hero":null, "name":"Fixture", "manager":null, "weapons":[],
            "graphs":[{"graph":WEAPON, "m_1EB5A024":null, "m_75408640":0}],
            "ai_weapon_buttons":{"0":20},
        });
        Arc::new(
            GraphStore::from_json(
                &serde_json::to_vec(&json!({
                    "version":1, "graphs":[weapon, firing], "bodies":{},
                    "entities":{format!("0x{BODY:016X}"):body}, "missing":[]
                }))
                .unwrap(),
            )
            .unwrap(),
        )
    }

    fn spawn(firing_position: Option<[f64; 3]>) -> BotSpawn {
        BotSpawn {
            body: BODY,
            hero: TRAINING_BOT,
            entity: ENTITY,
            frame: 100,
            mode: PRACTICE_RANGE_MODE,
            health: 200.0,
            firing_position,
        }
    }

    #[test]
    fn the_firing_behavior_holds_the_weapon_button_until_the_bot_dies() {
        let graphs = graphs();
        let codecs = CodecTable::client();
        // Without a firing position nothing presses the button.
        let mut idle = BotBody::start(Arc::clone(&graphs), spawn(None), Unknowns::Refuse).unwrap();
        assert_eq!(idle.behavior(), None);
        assert!(idle.advance_live(101, None).unwrap());
        assert!(idle.take_shots().is_empty());

        let mut bot = BotBody::start(
            Arc::clone(&graphs),
            spawn(Some([0.0, 0.0, 3.0])),
            Unknowns::Refuse,
        )
        .unwrap();
        assert_eq!(bot.behavior(), Some(2));
        let snapshot = bot.snapshot().unwrap();
        assert!(
            snapshot.initialization_complete(),
            "{:?}",
            snapshot.diagnostics
        );
        // The reset reached the behavior only.
        assert_eq!(snapshot.instances[&2].variables[&LIVE], Value::Bool(true));
        assert!(!snapshot.instances[&1].variables.contains_key(&LIVE));
        // The held button went down and fired its one-shot volley.
        assert_eq!(
            snapshot.instances[&1].states[&0].kind,
            crate::vm::StateKind::LogicalButton { down: true }
        );
        assert_eq!(snapshot.instances[&1].states[&1].activation, 1);
        let observed = bot
            .encoder(&graphs, &codecs)
            .project(snapshot, crate::snapshot::View::Observer)
            .unwrap();
        assert_eq!(observed.instance_count(), 1);
        assert!(!bot.advance_live(100, None).unwrap());
        assert!(bot.advance_live(101, Some([1.0, 0.0, 3.0])).unwrap());
        assert_eq!(bot.world().owner_position, Some([1.0, 0.0, 3.0]));
        assert!(!bot.take_shots().is_empty());

        assert_eq!(
            bot.die(100, Variables::new()),
            Err(BotError::DeathOrder {
                frame: 100,
                last: 101
            })
        );
        assert!(bot.die(101, Variables::new()).unwrap());
        assert!(bot.component().owner_dead());
        assert_eq!(bot.behavior(), None);
        assert_eq!(bot.frame(), 102);
        assert_eq!(bot.death_frame(), Some(101));
        let snapshot = bot.snapshot().unwrap();
        assert_eq!(snapshot.instances.keys().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(snapshot.instances[&1].variables[&LIVE], Value::Bool(true));
        assert_eq!(snapshot.variables[&OWNER_HEALTH], Value::Float(0.0));
        assert_eq!(
            snapshot.instances[&1].states[&0].kind,
            crate::vm::StateKind::LogicalButton { down: false }
        );
        assert!(bot.take_shots().is_empty());
        // A dead bot neither dies again nor advances live.
        assert!(!bot.die(103, Variables::new()).unwrap());
        assert!(!bot.advance_live(103, None).unwrap());
        let mut frames = Vec::new();
        bot.advance_death(200, 60, |frame, _| frames.push(frame))
            .unwrap();
        assert!(frames.is_empty());
    }

    #[test]
    fn the_world_answers_only_the_stock_nameplate_gate_and_firing_owner() {
        let filter = json!({
            "$":"STUConfigVarFilter", "m_84065FD1":0x3fff_ffff_u32, "m_4EA686DB":0x100,
            "m_27CF71D5":0, "m_condition":null,
        });
        let gate = json!({"$":"STU_AC829876", "m_entity":{"$":"STU_B5A0CAF0"}, "m_filter":filter});
        let site = |graph| Site {
            entity: ENTITY,
            instance: 1,
            graph,
            node: Some(1),
        };
        let mut world = BotWorld {
            graphs: graphs(),
            hero: TRAINING_BOT,
            mode: PRACTICE_RANGE_MODE,
            weapon_buttons: vec![(0, 20)],
            owner_position: Some([1.0, 2.0, 3.0]),
            unknowns: Unknowns::Refuse,
        };
        assert_eq!(
            world.native(site(PRACTICE_NAMEPLATE_PARENT), &gate),
            Some(Value::Bool(true))
        );
        assert_eq!(world.native(site(0x0a5b), &gate), None);
        let mut other_filter = gate.clone();
        other_filter["m_filter"]["m_66747E7D"] = json!(2);
        assert_eq!(
            world.native(site(PRACTICE_NAMEPLATE_PARENT), &other_filter),
            None
        );
        let mut other_owner = gate.clone();
        other_owner["m_entity"] = json!({"$":"STU_1FF67CA6"});
        assert_eq!(
            world.native(site(PRACTICE_NAMEPLATE_PARENT), &other_owner),
            None
        );
        world.hero = 0x02e0_0000_0000_0170;
        assert_eq!(world.native(site(PRACTICE_NAMEPLATE_PARENT), &gate), None);

        let possessed = json!({"$":"STU_0AC15F78","m_entity":{"$":"STU_B5A0CAF0"}});
        let position = json!({"$":"STU_71A87E92","m_entity":possessed});
        let possessor = json!({"$":"STU_295D117D","m_entity":possessed});
        assert_eq!(world.selects_ai_behavior(site(FIRING_GRAPH)), Some(true));
        assert_eq!(
            world.native(site(FIRING_GRAPH), &position),
            Some(Value::Vector([1.0, 2.0, 3.0]))
        );
        assert_eq!(
            world.native(site(FIRING_GRAPH), &possessor),
            Some(Value::Null)
        );
        assert_eq!(world.native(site(WEAPON), &position), None);
        assert_eq!(world.ai_weapon_button(0), Some(20));
        assert_eq!(world.ai_weapon_button(1), None);
        world.owner_position = None;
        assert_eq!(world.selects_ai_behavior(site(FIRING_GRAPH)), Some(false));
        assert_eq!(world.native(site(FIRING_GRAPH), &position), None);
    }

    /// The pack's Training Bot in the Practice Range, unknown content read as
    /// the Python reference did: the firing behavior runs out of sight, the
    /// stock nameplate mounts under 0C18, and the death runs to its end.
    #[test]
    #[ignore = "requires the derived game data's bot graph bundle"]
    fn the_practice_training_bot_starts_its_behavior_and_dies() {
        let data = orpheus_testkit::runtime_dir().join("data");
        let graphs =
            Arc::new(GraphStore::load_runtime(&data, "game_graphs_bots_174.json.gz").unwrap());
        let codecs = CodecTable::client();
        let mut bot = BotBody::start(
            Arc::clone(&graphs),
            spawn(Some([0.0, 0.0, 3.0])),
            Unknowns::Tolerate,
        )
        .unwrap();
        let snapshot = bot.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        let behavior = bot.behavior().unwrap();
        assert_eq!(snapshot.instances[&behavior].graph, FIRING_GRAPH);
        let nameplate = practice_nameplate(snapshot).unwrap();
        let parent = snapshot.instances[&nameplate].parent.unwrap();
        assert_eq!(
            snapshot.instances[&parent.0].graph,
            PRACTICE_NAMEPLATE_PARENT
        );
        let encoder = bot.encoder(&graphs, &codecs);
        let observed = encoder
            .project(snapshot, crate::snapshot::View::Observer)
            .unwrap();
        assert_eq!(observed.instance_count(), snapshot.instances.len() - 1);
        encoder.full(&observed).unwrap();
        for frame in 101..=160 {
            assert!(bot.advance_live(frame, Some([0.0, 0.0, 3.0])).unwrap());
        }
        assert!(bot.die(161, Variables::new()).unwrap());
        assert_eq!(bot.behavior(), None);
        let mut frames = Vec::new();
        bot.advance_death(400, 120, |frame, _| frames.push(frame))
            .unwrap();
        assert!(!frames.is_empty());
        assert!(frames.iter().all(|&frame| frame <= 161 + 120));
        let snapshot = bot.snapshot().unwrap();
        assert!(snapshot.initialization_complete());
        encoder_full(&graphs, &codecs, &bot);
    }

    fn encoder_full(graphs: &GraphStore, codecs: &CodecTable, bot: &BotBody) {
        let encoder = bot.encoder(graphs, codecs);
        let projected = encoder
            .project(bot.snapshot().unwrap(), crate::snapshot::View::Observer)
            .unwrap();
        encoder.full(&projected).unwrap();
    }
}
