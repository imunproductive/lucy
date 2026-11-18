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
//! the pick: [`ControllerScript::hero_switched`]). The graphs also take,
//! and the server does not route yet (`docs/dev/loose-ends.md`): the
//! client's hero select requests (21616 to `288B`: [`PICK_HERO`],
//! [`OPEN_OR_CLOSE`]) and [`RESPAWN_HERE`] from the mode's `0C8F`.
//!
//! What the graphs send other entities (`06FF` and `0685` to the mode, ...)
//! waits in [`ControllerScript::take_messages`]; messages to the controller
//! itself run at once (the VM's own routing).
//!
//! Not run (IDA, enabler 4): the body the controller spawns or revives,
//! `0C90` st0 StateHero and node 4 `STU_621AF478` on `0687`; the server
//! creates and possesses bodies in Rust (`replication.rs`).

use std::sync::Arc;

use serde_json::Value as Json;

use crate::{
    expr::EntityRelation,
    graph::GraphStore,
    vm::{
        Component, GameMessage, InitFault, MAX_INIT_WORK, Root, Site, Snapshot, StateKind,
        Unknowns, VoiceRequest, World,
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
];

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
    /// HeroSelect limits), each with its role identifier.
    pub heroes: Vec<(u64, Option<u64>)>,
    /// The body the controller possesses; none before the first spawn.
    pub body: Option<u32>,
    /// The hero in the controller's own component 75 (the last
    /// possession's: `scene::Possession`); none before the first.
    /// `STU_93DCDDFD` of the controller reads it, never the body's.
    pub hero: Option<u64>,
    /// Whether the player may pick a hero now (`STU_E4C6AAB4`): the
    /// client reads only component 74 field 6 (ControllerWorld.md, "Client
    /// tools pass (2026-11-16)"), which the server writes from
    /// `replication.rs` `may_change_hero`; that rule feeds this.
    pub may_pick: bool,
    pub unknowns: Unknowns,
    outbox: Vec<GameMessage>,
    /// The voice stimuli the graphs asked the controller to say.
    voices: Vec<VoiceRequest>,
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
            may_pick: true,
            unknowns,
            outbox: Vec::new(),
            voices: Vec::new(),
        }
    }

    fn owner(&self, config: &Json, operand: &str) -> bool {
        config
            .get(operand)
            .and_then(|value| value.get("$"))
            .and_then(Json::as_str)
            == Some("STU_B5A0CAF0")
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

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.rulesets().default(id, 0, self.mode)
    }

    /// The mode entity's variables come from the bundle (`0C5`
    /// `m_3CE93B76`), as in `ModeWorld`.
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

    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        let class = config.get("$")?.as_str()?;
        match class {
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            // GetGameMode.
            "STU_37F20D24" => Some(Value::Asset(self.mode)),
            "STU_1AAB4B4F" if self.owner(config, "m_controller") => Some(Value::Array(
                self.heroes
                    .iter()
                    .map(|&(hero, _)| Value::Asset(hero))
                    .collect(),
            )),
            "STU_E4C6AAB4" if self.owner(config, "m_F3E612FD") => Some(Value::Bool(self.may_pick)),
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
            _ => None,
        }
    }

    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        let known =
            entity == self.entity || entity == GAME_MODE_ENTITY || self.body == Some(entity);
        known.then_some(true)
    }

    fn entity_team(&self, _site: Site, entity: u32) -> Option<i64> {
        (entity == self.entity || self.body == Some(entity))
            .then_some(self.team)
            .flatten()
    }

    fn hero_available(&self, _site: Site, controller: u32, hero: u64) -> Option<bool> {
        (controller == self.entity).then(|| self.heroes.iter().any(|&(listed, _)| listed == hero))
    }

    fn hero_role(&self, _site: Site, hero: u64) -> Option<u64> {
        self.heroes
            .iter()
            .find(|&&(listed, _)| listed == hero)
            .and_then(|&(_, role)| role)
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

    /// The controller's player eliminated `victim` with `killer` (his body)
    /// at `frame`: `0093.025` to the controller, which `0215` counts
    /// (rid14). Parameters: `0015` the victim (#393), `0012` the killer
    /// (#392), `0091` (#1798) on: the final blow (assumption, KillFeed.md;
    /// the server credits no assists, and no mode sets `0215`'s #12421 that
    /// would ask for it).
    pub fn eliminated(&mut self, frame: u32, victim: u32, killer: u32) -> Result<(), InitFault> {
        self.advance(frame)?;
        self.receive(
            ELIMINATION,
            Variables::from([
                (0x15, Value::Entity(victim)),
                (0x12, Value::Entity(killer)),
                (0x91, Value::Bool(true)),
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

    /// The controller now possesses `body` playing `hero`: its components
    /// 44 and 75 (`scene::Possession`).
    pub fn possessed(&mut self, body: u32, hero: u64) {
        self.world.body = Some(body);
        self.world.hero = Some(hero);
    }

    /// Component 74 field 6, which the server writes from its own rule:
    /// what `STU_E4C6AAB4` answers.
    pub fn set_may_pick(&mut self, may_pick: bool) {
        self.world.may_pick = may_pick;
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
        world.heroes = vec![(SOLDIER, Some(orpheus_content::heroes::DAMAGE_ROLE))];
        ControllerScript::start(world, PVP_CONTROLLER, 1).unwrap()
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
        script.eliminated(tick, 0x20, CONTROLLER | 1).unwrap();
        assert_eq!(script.multikill(), None);
        for (pulses, stimulus) in (1_u8..).zip(0x058a_u64..=0x058e) {
            tick += WINDOW - 1;
            script.eliminated(tick, 0x20, CONTROLLER | 1).unwrap();
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
        script.eliminated(tick, 0x20, CONTROLLER | 1).unwrap();
        assert_eq!(script.multikill(), quiet, "the seventh kill has no line");
        tick += WINDOW;
        script.eliminated(tick, 0x20, CONTROLLER | 1).unwrap();
        assert_eq!(script.multikill(), quiet, "a new streak's first kill");
        tick += 1;
        script.eliminated(tick, 0x20, CONTROLLER | 1).unwrap();
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
            // `070B` with the hero's role: `STU_BE1161C0` (node 44) is the
            // one gap (`docs/dev/loose-ends.md`, hero select limits).
            let gaps: Vec<_> = script
                .snapshot()
                .unwrap()
                .diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.site.graph, diagnostic.site.node))
                .collect();
            assert_eq!(gaps, [(PVP_CONTROLLER, Some(44))], "{mode:x}");
        }
    }
}
