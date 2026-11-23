//! The game mode entity's frames: from the mode's own graph when the VM
//! runs it whole, else the hand-built mode UI frame carrying the match
//! flow's variables.
//!
//! In a PvP match the room starts the mode graph once at the match's start
//! ([`ModeRunner::start`]); the Practice Range, which has no match flow
//! ([`without_flow`]), starts `13C0` with the room, after the placed
//! scripts' first tick (their reports to the mode entity are in when the
//! graph starts: [`reported_objectives`]). If it runs with no refused
//! content and no assumption, it stays: the flow's variables (none in the
//! Range) and the
//! match's players (`ModeScript::set_players`) are written into it each
//! tick, it advances with the server frame, its observer frames are the
//! mode entity's (each client gets them as deltas, [`crate::mode_view`]),
//! and its messages go with every other entity's
//! ([`crate::messages`]), which routes it those sent to the mode entity or
//! to every entity and gives every entity the broadcasts it holds
//! ([`ModeRunner::held`]; in the Range `0724` for the spawn rooms, `06FD`
//! for the bodies). Otherwise (0EC2 today, see `lucy_script::mode`) the
//! gaps are logged once and the mode entity gets graph 0CB6's frame with
//! the flow's variables, as the countdown did before (match.py
//! `_send_countdown`).

use std::{collections::BTreeMap, sync::Arc};

use lucy_content::maps::{PRACTICE_RANGE_MODE, PYTHON_PRACTICE_RANGE_MODE};
use lucy_script::{
    codecs::CodecTable,
    graph::GraphStore,
    mode::{CreatedEntity, EVERY_ENTITY, ModePlayer, ModeScript, ModeWorld},
    snapshot::{ProjectedSnapshot, ProjectionCache, SnapshotEncoder, View},
    vm::{GameMessage, InitFault, MessageEnd, Unknowns},
    wire::{Value, Variables},
};
use lucy_wire::entity::GAME_MODE_ENTITY;

use crate::{
    match_flow::{self, MatchState},
    placed_script::PlacedFrame,
    scene_assets::PreparedScene,
};

/// The modes with no match flow, the Practice Range's: the room starts
/// their graph with itself, not at a match's start.
pub fn without_flow(mode: u64) -> bool {
    matches!(mode, PRACTICE_RANGE_MODE | PYTHON_PRACTICE_RANGE_MODE)
}

/// The mode entity's script (`m_controllerScript` of entity 16B2) per mode
/// (`statescript_names.json` "modes").
pub fn mode_graph(mode: u64) -> Option<u32> {
    use match_flow::*;
    Some(match mode {
        ASSAULT | ESCORT | HYBRID => 0x0f4a,
        ASSAULT_TEST => 0x29a6,
        CONTROL => 0x0ec2,
        SKIRMISH => SKIRMISH_GRAPH,
        ELIMINATION => 0x0d0b,
        TEAM_DEATHMATCH => 0x0f60,
        DEATHMATCH | BOUNTY_HUNTER => 0x0f73,
        PRACTICE_RANGE_MODE | PYTHON_PRACTICE_RANGE_MODE => 0x13c0,
        _ => return None,
    })
}

/// Whether `mode`'s graph takes the Workshop's Set Match Time (`08D8`, the
/// action `1ABC`'s message to the mode entity): the entries of `0F4A`
/// (rid187), `0D76` (rid87), `0EC2`, `0D0B`, `0F60` and `0F73`. Where the
/// flow drives in such a graph's place it takes it as the graph would
/// (`match_flow::MatchFlow::receive`); Lúcioball's `0F9D`, a brawl the
/// flow plays as Skirmish, has no such entry, so its clock ignores it.
pub fn takes_set_match_time(mode: u64) -> bool {
    matches!(
        mode_graph(mode),
        Some(0x0f4a | SKIRMISH_GRAPH | 0x0ec2 | 0x0d0b | 0x0f60 | 0x0f73)
    )
}

/// The modes with a payload, which the match flow drives outside a
/// Workshop match: `0F4A` would drive them too, but the payload's `1225`
/// stops at its start (`STU_C5B280AA` on Gibraltar, more classes behind
/// it), so the payload could never move; the flow's payload stands in
/// (docs/dev/loose-ends.md).
pub const PAYLOAD_ON_FLOW: [u64; 2] = [match_flow::ESCORT, match_flow::HYBRID];

/// Classes or faults by graph, with how often they were met.
pub type Gaps = BTreeMap<(u32, String), usize>;

pub enum ModeRunner {
    /// A graph drives the mode entity's frames.
    Graph {
        script: Box<ModeScript>,
        graphs: Arc<GraphStore>,
        codecs: Arc<CodecTable>,
        /// Its last projection failed (said once, [`Self::project`]).
        unprojected: bool,
        /// The last projection, whose unchanged instances the next shares
        /// ([`SnapshotEncoder::project_cached`]): each client's delta then
        /// compares only what changed (a Workshop match's mode entity has
        /// hundreds of action instances, each projected again every tick).
        kept: Option<ProjectionCache>,
        /// The mode's own graph runs (not only a Workshop host,
        /// [`Self::workshop`]): it holds the spawn rooms' `0724` itself.
        mode_graph: bool,
        /// The entities the graph made and their latest frames
        /// ([`Self::created`]).
        created: BTreeMap<u32, Created>,
    },
    /// The hand-built frame, and why the graph doesn't drive it.
    HandBuilt { gaps: Gaps },
}

/// An entity the mode entity's graphs made (the Workshop's effects and
/// icons), with its latest frame: the projection it was made from and the
/// frame (none while it can't be made).
pub struct Created {
    pub entity: CreatedEntity,
    frame: Option<(ProjectedSnapshot, PlacedFrame)>,
}

impl Created {
    pub fn frame(&self) -> Option<&PlacedFrame> {
        self.frame.as_ref().map(|(_, frame)| frame)
    }
}

/// The PvP modes' shared subgraph (`0CBB`, ModeGraphGaps.md), which plays
/// the Workshop's value table `1B4A` from its entry
/// (`refs/reports/Workshop.md` "WorkshopRuntime").
pub const SHARED_MODE_GRAPH: u32 = 0x0cbb;

/// The match's phase on the mode graph's root instance (`0F4A` v59,
/// `match_flow::Phase` values).
const PHASE: u32 = 59;
/// The winning team on the same instance (`0F4A` #7525).
const WINNER: u32 = 7525;
/// Skirmish's mode graph, which has no phases ([`ModeRunner::match_state`]).
const SKIRMISH_GRAPH: u32 = 0x0d76;
/// `0D76`'s declared winner (#13155, rid64 from `09A3`): its match's end.
const SKIRMISH_DECIDED: u32 = 13155;
/// The server's start of a match whose teams aren't full
/// ([`ModeRunner::start_match`]).
const START_MATCH: u64 = 0x0240_0000_0000_0789;
/// The mode entity's attacking team (`0F4A` #10795e: rid92 at the start,
/// rid153 at a side swap), the current objective's order (#10548e, which
/// rid121 counts up as objectives fall) and overtime (#3392e, rid406 st131
/// while the attackers hold an objective with the clock out).
const ATTACKERS: u32 = match_flow::ATTACKERS;
const OBJECTIVE: u32 = 10548;
const OVERTIME: u32 = 3392;
/// The mode graph's pause (`#7898` on its root, [`ModeRunner::paused`]).
const PAUSED: u32 = 7898;
/// The mode entity's "built-in completion off" (`#18600e`,
/// [`ModeRunner::completion_off`]).
const COMPLETION_OFF: u32 = 18600;

/// `0727.025` (no parameters), which the mode graphs hold to every entity
/// from their start (`0F4A` st22 beside its `0CBB` SubScript st12, `13C0`
/// st1). A controller's `0C90` st28 takes it: while he possesses no body,
/// st29 allows hero select (#8419) and st32 holds `0738` to his own
/// controller, whose `288B` st126 → st134 stacks hero select open
/// (#2438e) until he has a body. So a player with no hero has hero select
/// in every phase, and one who joins a running match gets it.
pub const ALLOW_HERO_SELECT: u64 = 0x0240_0000_0000_0727;

/// The messages held to every entity now: those of the mode graph while
/// it runs whole ([`ModeRunner::holds_rooms`]); else (`0CBB` alone on a
/// Workshop's mode entity, the hand-built frame, or before the match's
/// mode starts) those of what runs, and [`ALLOW_HERO_SELECT`] held by
/// the match flow in the mode graph's place.
pub fn held(mode: Option<&ModeRunner>) -> Vec<GameMessage> {
    let mut held = mode.map_or_else(Vec::new, |mode| mode.held().to_vec());
    if !mode.is_some_and(ModeRunner::holds_rooms) {
        held.push(GameMessage {
            sender: GAME_MODE_ENTITY,
            target: EVERY_ENTITY,
            message: ALLOW_HERO_SELECT,
            parameters: Variables::new(),
        });
    }
    held
}

impl ModeRunner {
    /// Start the mode graph with the match's `players` now. The start
    /// condition `STU_81A9AB10` (server-only) is the server's own rule:
    /// true once the match's players are in the world, when the room
    /// starts the mode. Escort and Hybrid stay on the match flow
    /// ([`PAYLOAD_ON_FLOW`]).
    pub fn start(scene: &PreparedScene, frame: u32, players: Vec<ModePlayer>) -> Self {
        if PAYLOAD_ON_FLOW.contains(&scene.map().mode_guid) {
            return Self::HandBuilt { gaps: Gaps::new() };
        }
        Self::start_graph(scene, frame, players)
    }

    /// The mode graph whole when it runs so, else the hand-built frame.
    fn start_graph(scene: &PreparedScene, frame: u32, players: Vec<ModePlayer>) -> Self {
        let mode = scene.map().mode_guid;
        let Some(graph) = mode_graph(mode) else {
            return Self::HandBuilt { gaps: Gaps::new() };
        };
        match ModeScript::start(mode_world(scene, players), graph, frame) {
            Ok(script) if script.complete() => Self::graph(scene, script, true),
            Ok(script) => {
                let gaps = script.gaps();
                eprintln!(
                    "mode graph {graph:04X} has {} gaps; the match flow drives the mode entity: {gaps:?}",
                    gaps.len()
                );
                Self::HandBuilt { gaps }
            }
            Err(error) => {
                eprintln!(
                    "mode graph {graph:04X} does not start ({error}); the match flow drives the mode entity"
                );
                Self::HandBuilt {
                    gaps: Gaps::from([((graph, error.to_string()), 1)]),
                }
            }
        }
    }

    /// The mode entity of a match that runs a Workshop script
    /// ([`crate::workshop`]): the Workshop's action graphs run on it, so a
    /// graph must drive it. The mode graph when it runs whole
    /// ([`Self::start`]); else `0CBB` alone, the subgraph every mode graph
    /// plays, whose entry plays `1B4A`, the table the clients evaluate the
    /// actions' parameters with (`STU_77B0987E`). The match flow's
    /// variables go into it as into the mode graph; the spawn rooms get
    /// the mode's `0724` from the flow ([`Self::holds_rooms`]). None when
    /// neither starts.
    pub fn workshop(scene: &PreparedScene, frame: u32, players: Vec<ModePlayer>) -> Option<Self> {
        let started = Self::start_graph(scene, frame, players.clone());
        if started.graph_driven() {
            return Some(started);
        }
        match ModeScript::start(mode_world(scene, players), SHARED_MODE_GRAPH, frame) {
            Ok(script) => {
                if !script.complete() {
                    eprintln!(
                        "the Workshop's mode entity runs {SHARED_MODE_GRAPH:04X} with gaps: {:?}",
                        script.gaps()
                    );
                }
                Some(Self::graph(scene, script, false))
            }
            Err(error) => {
                eprintln!("{SHARED_MODE_GRAPH:04X} does not start ({error}); no Workshop host");
                None
            }
        }
    }

    fn graph(scene: &PreparedScene, script: ModeScript, mode_graph: bool) -> Self {
        Self::Graph {
            script: Box::new(script),
            graphs: scene.shared_graphs(),
            codecs: Arc::new(CodecTable::client()),
            unprojected: false,
            kept: None,
            mode_graph,
            created: BTreeMap::new(),
        }
    }

    pub fn graph_driven(&self) -> bool {
        matches!(self, Self::Graph { .. })
    }

    /// Whether the mode's own graph runs and holds the spawn rooms' `0724`
    /// and the controllers' `0727` itself; else the match flow's spawns
    /// give the first and the flow holds the other ([`held`]).
    pub fn holds_rooms(&self) -> bool {
        matches!(
            self,
            Self::Graph {
                mode_graph: true,
                ..
            }
        )
    }

    /// The match as the mode's own graph keeps it, while it drives the
    /// match ([`Self::holds_rooms`]): its root's #59 (`0F4A`: 0 waiting, 1
    /// assemble, 2 setup, 3 round, 4 round end, 5 match end), the mode
    /// entity's clock #1303 (`match_flow::CLOCK`), the winning team (#7525,
    /// which `0F4A` sends in `0695` as `03A6`), the attacking team, the
    /// current objective, overtime and whether a graph ended the game
    /// (`180A` after the match end's cards). None before the graph set its
    /// phase.
    ///
    /// Skirmish's `0D76` has no phases (it never writes #59): it plays
    /// from its start, its clock (#1303e, rid91: 1800 s) counting down
    /// from the first frame, and ends only once a winner is declared
    /// (`09A3` → rid64 #13155 = #7525 → st62's true branch, rid75's
    /// `0695`). So its match is a round until then, and its end after.
    pub fn match_state(&self) -> Option<MatchState> {
        let Self::Graph {
            script,
            mode_graph: true,
            ..
        } = self
        else {
            return None;
        };
        let root = script.snapshot()?.instances.get(&1)?;
        let phase = match root.variables.get(&PHASE) {
            Some(phase) => match lucy_script::expr::to_int(phase, -1) {
                0 => match_flow::Phase::Waiting,
                1 => match_flow::Phase::Assemble,
                2 => match_flow::Phase::Setup,
                3 => match_flow::Phase::Round,
                4 => match_flow::Phase::RoundEnd,
                5 => match_flow::Phase::MatchEnd,
                _ => return None,
            },
            None if root.graph == SKIRMISH_GRAPH => match root.variables.get(&SKIRMISH_DECIDED) {
                Some(value) if *value != lucy_script::wire::Value::Null => {
                    match_flow::Phase::MatchEnd
                }
                _ => match_flow::Phase::Round,
            },
            None => return None,
        };
        let team = |value: &lucy_script::wire::Value| {
            u8::try_from(lucy_script::expr::to_int(value, -1))
                .ok()
                .filter(|&team| team < 2)
        };
        let winner = root.variables.get(&WINNER).and_then(team);
        Some(MatchState {
            phase,
            clock: lucy_script::expr::to_float(&script.entity_value(match_flow::CLOCK)),
            winner,
            attackers: team(&script.entity_value(ATTACKERS)),
            objective: u32::try_from(lucy_script::expr::to_int(
                &script.entity_value(OBJECTIVE),
                0,
            ))
            .unwrap_or(0),
            overtime: lucy_script::expr::truthy(&script.entity_value(OVERTIME)),
            over: script.game_over(),
        })
    }

    /// The mode's own graph, while it drives the match
    /// ([`Self::holds_rooms`]): its root instance's graph (`0F4A`, `0D76`).
    pub fn mode_graph(&self) -> Option<u32> {
        let Self::Graph {
            script,
            mode_graph: true,
            ..
        } = self
        else {
            return None;
        };
        Some(script.snapshot()?.instances.get(&1)?.graph)
    }

    /// Whether the mode's own graph, while it drives the match, has an
    /// entry for game message `message` (an `EntryGameMessage` the server
    /// runs): what the Workshop's actions that send it to the mode entity
    /// can do in this mode (`0F4A` takes Set Match Time `08D8`, pause
    /// `06D5` and a declared winner `09A3`, but no round winner `0EAB`).
    pub fn takes(&self, message: u64) -> bool {
        let (Self::Graph { graphs, .. }, Some(graph)) = (self, self.mode_graph()) else {
            return false;
        };
        graphs.graph(graph).is_some_and(|graph| {
            graph
                .message_nodes(message)
                .any(|node| node.class == "STUStatescriptEntryGameMessage")
        })
    }

    /// Whether the mode's own graph has paused its clock: its root's
    /// `#7898`, which its `06D5` entry toggles (`0F4A` rid69, `0D76`
    /// rid89; the Workshop's Pause `1D53` and Unpause `1D54` send it alike,
    /// so the parameter doesn't matter) and its chases test.
    pub fn paused(&self) -> bool {
        let Self::Graph {
            script,
            mode_graph: true,
            ..
        } = self
        else {
            return false;
        };
        script
            .snapshot()
            .and_then(|snapshot| snapshot.instances.get(&1))
            .and_then(|root| root.variables.get(&PAUSED))
            .is_some_and(lucy_script::expr::truthy)
    }

    /// Whether the mode entity's built-in completion is off: `#18600e`,
    /// which the Workshop's Disable Built-In Game Mode Completion `1ABA`
    /// holds true until Enable `1AB9` sends `0D68`; every mode graph tests
    /// it before it ends the match at clock 0.
    pub fn completion_off(&self) -> bool {
        let Self::Graph { script, .. } = self else {
            return false;
        };
        lucy_script::expr::truthy(&script.entity_value(COMPLETION_OFF))
    }

    /// The graph on the mode entity, while one drives it.
    pub fn script_mut(&mut self) -> Option<&mut ModeScript> {
        match self {
            Self::Graph { script, .. } => Some(script),
            Self::HandBuilt { .. } => None,
        }
    }

    /// The mode entity's variables as the mode's own graph keeps them,
    /// while it drives the match ([`Self::holds_rooms`]), for the placed
    /// scripts that read them (`0FA8`'s attacking team #10795e).
    pub fn entity_variables(&self) -> Option<&Variables> {
        match self {
            Self::Graph {
                script,
                mode_graph: true,
                ..
            } => script.snapshot().map(|snapshot| &snapshot.variables),
            _ => None,
        }
    }

    /// Why the graph doesn't drive the mode entity; empty when it does.
    pub fn gaps(&self) -> Option<&Gaps> {
        match self {
            Self::HandBuilt { gaps } => Some(gaps),
            Self::Graph { .. } => None,
        }
    }

    /// The messages the graph holds to every entity now
    /// ([`ModeScript::held`]); none while it doesn't drive the mode.
    pub fn held(&self) -> &[GameMessage] {
        match self {
            Self::Graph { script, .. } => script.held(),
            Self::HandBuilt { .. } => &[],
        }
    }

    /// One server frame with the flow's variables and the match's players
    /// now; the messages the graph sent.
    pub fn advance(
        &mut self,
        frame: u32,
        variables: &Variables,
        players: Vec<ModePlayer>,
    ) -> Vec<GameMessage> {
        let Self::Graph { script, .. } = self else {
            return Vec::new();
        };
        let result = script
            .set_variables(variables)
            .and_then(|()| script.set_players(players, true))
            .and_then(|()| script.advance(frame).map(|_| ()));
        match result {
            Ok(()) => script.take_messages(),
            Err(error) => {
                self.stop(&error);
                Vec::new()
            }
        }
    }

    /// Whether the graph runs entity `entity`: the mode entity's, while it
    /// drives it.
    pub fn runs(&self, entity: u32) -> bool {
        entity == GAME_MODE_ENTITY && self.graph_driven()
    }

    /// A game message to the mode entity, or to every entity but from the
    /// mode ([`crate::messages`]): the graph's entries run now; what they
    /// send goes with the next frame's messages. A failure stops the graph
    /// as in [`Self::advance`].
    pub fn receive_message(&mut self, message: &GameMessage) {
        let Self::Graph { script, .. } = self else {
            return;
        };
        if message.sender == GAME_MODE_ENTITY {
            return;
        }
        if let Err(error) =
            script.receive_message(message.sender, message.message, message.parameters.clone())
        {
            self.stop(&error);
        }
    }

    /// The server starts the match: `0789.025` to the mode entity, which
    /// the mode graphs' entries take (`0F4A` rid81, `0EC2` rid76, `0D0B`,
    /// `0F60`, `0F73`) into #9677, the start their waiting state takes when
    /// the teams aren't full (`0F4A` st6: rid7 `STU_81A9AB10` false or
    /// rid14 a team short → rid80 `#9677` → rid12 assemble). No graph and
    /// no client code sends it (`script query find 0789.025`;
    /// ModeGraphGaps.md "ModeGraph3"): it is the server's. The server's
    /// rule, as match.py `_spawn_steps`: the match starts once its first
    /// player is in the world, whether or not the lobby filled the teams.
    pub fn start_match(&mut self) {
        self.receive_message(&GameMessage {
            sender: 0,
            target: GAME_MODE_ENTITY,
            message: START_MATCH,
            parameters: Variables::new(),
        });
    }

    /// The map placeables' entity variables now
    /// (`PlacedScripts::entity_variables`), which the mode graph reads of
    /// its objectives (`0F4A`: `#10551.id(10552)` the time an objective
    /// adds, `#9901.id(9866)` a point's progress, `#9901.id(99)` its owner).
    /// A failure stops the graph as in [`Self::advance`].
    pub fn set_map_variables(&mut self, entities: &[(u32, Variables)]) {
        let Self::Graph { script, .. } = self else {
            return;
        };
        let result = script.set_foreign_variables(entities);
        if let Err(error) = result {
            self.stop(&error);
        }
    }

    /// The ends of the graph's persistent sends to other entities since
    /// the last call, for the match to route with its messages
    /// ([`crate::messages::deliver`]).
    pub fn take_message_ends(&mut self) -> Vec<MessageEnd> {
        match self {
            Self::Graph { script, .. } => script.take_message_ends(),
            Self::HandBuilt { .. } => Vec::new(),
        }
    }

    /// The new parameters of the graph's persistent sends that other
    /// entities still hold, since the last call, for the match to route
    /// after its messages ([`crate::messages::deliver_updates`]).
    pub fn take_message_updates(&mut self) -> Vec<GameMessage> {
        match self {
            Self::Graph { script, .. } => script.take_message_updates(),
            Self::HandBuilt { .. } => Vec::new(),
        }
    }

    /// A persistent sender let its message to the mode entity go.
    pub fn end_message(&mut self, end: &MessageEnd) {
        let Self::Graph { script, .. } = self else {
            return;
        };
        if let Err(error) = script.end_message(end.sender, end.message) {
            self.stop(&error);
        }
    }

    fn stop(&mut self, error: &InitFault) {
        eprintln!("the mode graph stopped ({error}); the match flow drives the mode entity");
        *self = Self::HandBuilt {
            gaps: Gaps::from([((0, error.to_string()), 1)]),
        };
    }

    /// The mode entity's observer frame now, while the graph drives it, in
    /// `previous`'s stream (`SnapshotEncoder::project_next`),
    /// so each client gets the match's frames as deltas of his first one
    /// (`crate::mode_view`). None while the graph doesn't drive it, or when
    /// the frame can't be made (said once until it can again). The
    /// entities the graph made get their frames now too ([`Self::created`]).
    pub fn project(&mut self, previous: Option<&ProjectedSnapshot>) -> Option<ProjectedSnapshot> {
        let Self::Graph {
            script,
            graphs,
            codecs,
            unprojected,
            kept,
            created,
            ..
        } = self
        else {
            return None;
        };
        let encoder = SnapshotEncoder::new(graphs, codecs)
            .with_unknowns(script.unknowns())
            .with_presenters();
        project_created(script, &encoder, created);
        let snapshot = script.snapshot()?;
        let projected = encoder
            .project_cached(snapshot, View::Observer, previous, kept)
            .cloned();
        match projected {
            Ok(projected) => {
                *unprojected = false;
                Some(projected)
            }
            Err(error) => {
                if !*unprojected {
                    eprintln!("the mode graph's frame can't be made ({error:?})");
                }
                *unprojected = true;
                None
            }
        }
    }

    /// The entities the mode entity's graphs made that are still there,
    /// with their frames as of the last [`Self::project`]: each client
    /// creates them and gets their frames on their own streams
    /// ([`crate::replication::WorldDelivery::update_mode_entities`]).
    pub fn created(&self) -> impl Iterator<Item = &Created> {
        let created = match self {
            Self::Graph { created, .. } => Some(created.values()),
            Self::HandBuilt { .. } => None,
        };
        created.into_iter().flatten()
    }

    /// The encoder of the graph's frames, while it drives the mode entity.
    pub fn encoder(&self) -> Option<SnapshotEncoder<'_>> {
        let Self::Graph {
            script,
            graphs,
            codecs,
            ..
        } = self
        else {
            return None;
        };
        Some(
            SnapshotEncoder::new(graphs, codecs)
                .with_unknowns(script.unknowns())
                .with_presenters(),
        )
    }
}

/// The made entities' frames, as the server placeables' are made
/// (`placed_script.rs`): the observer projection of each, a new full frame
/// when it changed; an entity that went is dropped. A script with gaps or
/// a fault sends no frame (its gaps are the mode entity's,
/// [`ModeScript::gaps`]).
fn project_created(
    script: &ModeScript,
    encoder: &SnapshotEncoder<'_>,
    created: &mut BTreeMap<u32, Created>,
) {
    created.retain(|entity, _| script.created().iter().any(|made| made.entity == *entity));
    for made in script.created() {
        let item = created.entry(made.entity).or_insert(Created {
            entity: *made,
            frame: None,
        });
        let snapshot = match script.created_script(made.entity) {
            Some(placed) if placed.complete() => placed.snapshot(),
            _ => None,
        };
        let Some(projected) = snapshot.and_then(|snapshot| {
            encoder
                .project(snapshot, View::Observer)
                .inspect_err(|error| {
                    eprintln!(
                        "the made entity {:08X}'s frame does not project ({error:?})",
                        made.entity
                    );
                })
                .ok()
        }) else {
            item.frame = None;
            continue;
        };
        if item
            .frame
            .as_ref()
            .is_some_and(|(known, _)| *known == projected)
        {
            continue;
        }
        item.frame = match encoder.full(&projected) {
            Ok(bits) => Some((
                projected,
                PlacedFrame {
                    serial: item.frame.as_ref().map_or(1, |(_, frame)| frame.serial + 1),
                    bits: Arc::new(bits),
                },
            )),
            Err(error) => {
                eprintln!(
                    "the made entity {:08X}'s frame does not encode ({error:?})",
                    made.entity
                );
                None
            }
        };
    }
}

/// What the mode entity's graphs ask outside it, from the scene, with the
/// match's `players` now.
fn mode_world(scene: &PreparedScene, players: Vec<ModePlayer>) -> ModeWorld {
    let mode = scene.map().mode_guid;
    // The team sizes are the game's, as the 20300/20301 game struct sends
    // them.
    let mut world = ModeWorld::new(scene.shared_graphs(), mode, Unknowns::for_build());
    world.ruleset = scene.map().ruleset;
    world.players = players;
    world.ready = true;
    world.team_sizes = crate::instance::team_entries(scene.map());
    world.mode_random = Some(scene.mode_random());
    world.match_seed = Some(scene.match_seed());
    world.map_modes = Some(scene.map().supported_modes.clone());
    world.map_tags = Some(scene.map_tags());
    world.map_rays = Some(scene.map_rays());
    world.placeables = scene
        .server_placeables()
        .iter()
        .map(|placed| (placed.entity(), placed.position))
        .collect();
    world.heroes = Some(
        scene
            .heroes()
            .heroes()
            .map(|hero| (hero.guid, hero.body))
            .collect(),
    );
    // What the placeables' first tick wrote and sent the mode entity: the
    // objectives' times and spawn tags (`#10551.id(10550)`), their reports.
    for (entity, variables) in scene.placed_scripts().entity_variables() {
        world.set_foreign_variables(entity, &variables);
    }
    world.start_variables = reported_objectives(scene.placed_messages());
    world
}

/// An objective's report to the mode entity (`0F4C` rid1, as its script
/// starts), with its index (`045E`; Null for the first, which `0F4C`'s
/// binding leaves unset).
const OBJECTIVE_REPORT: u64 = 0x0240_0000_0000_0813;
const REPORT_INDEX: u32 = 0x045e;
/// The mode entity's objectives by index (`#12376e`), which `0F4A`'s
/// entry for the report fills (rid115: `#12376e[index] = sender`) and
/// its current objective (`#10551` = `#12376e[#10548e]`, st52) reads.
const OBJECTIVES: u32 = 12376;

/// The mode entity's objectives as the map's placeables reported them in
/// the messages the match hasn't routed yet: the room runs the placeables'
/// first tick before it starts the mode graph, so `0F4A` starts with
/// them, as its report entry would write them. Assumption (retail's order
/// was not read): the objectives report before the mode graph's first
/// lists. Started without them, `0F4A`'s spawn room lists (`#12738`,
/// `#12739`: the tagless query of the current objective's spawn tags,
/// every tagged placeable while the tags are Null) send every tagged
/// placeable `0724` with both teams for a tick; a room kept in one list
/// then lets the other list's go, which ends its sender's hold of it, and
/// the room holds none. The report itself still reaches the graph with
/// the other messages and writes the same.
fn reported_objectives(messages: &[GameMessage]) -> Variables {
    let mut objectives = Vec::new();
    for report in messages
        .iter()
        .filter(|message| message.target == GAME_MODE_ENTITY && message.message == OBJECTIVE_REPORT)
    {
        let index = report
            .parameters
            .get(&REPORT_INDEX)
            .map_or(0, |index| lucy_script::expr::to_int(index, 0));
        let Ok(index) = usize::try_from(index) else {
            continue;
        };
        if objectives.len() <= index {
            objectives.resize(index + 1, Value::Null);
        }
        objectives[index] = Value::Entity(report.sender);
    }
    if objectives.is_empty() {
        return Variables::new();
    }
    Variables::from([(OBJECTIVES, Value::Array(objectives))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_assets::SceneAssets;
    use lucy_content::maps::GameMap;
    use lucy_script::{mode::EVERY_ENTITY, wire::Value};

    /// The objectives' reports (`0813`) the match hasn't routed yet give
    /// `0F4A` its objectives by index as it starts: the first one's index
    /// is unset (Null, index 0); other messages and targets don't count.
    #[test]
    fn the_mode_graph_starts_with_the_objectives_reported() {
        let report = |sender, index: Value| GameMessage {
            sender,
            target: GAME_MODE_ENTITY,
            message: OBJECTIVE_REPORT,
            parameters: Variables::from([(REPORT_INDEX, index)]),
        };
        let messages = [
            report(0x8000_034b, Value::Integer(1)),
            GameMessage {
                target: 0x8000_0001,
                ..report(0x8000_0002, Value::Integer(0))
            },
            GameMessage {
                message: ALLOW_HERO_SELECT,
                ..report(0x8000_0003, Value::Integer(0))
            },
            report(0x8000_0332, Value::Null),
        ];
        assert_eq!(
            reported_objectives(&messages),
            Variables::from([(
                OBJECTIVES,
                Value::Array(vec![Value::Entity(0x8000_0332), Value::Entity(0x8000_034b)])
            )])
        );
        assert_eq!(reported_objectives(&[]), Variables::new());
    }

    /// The Practice Range's `13C0` runs whole and drives the mode entity.
    /// Mode `005A`'s variable #27534 is true, so #27536 is false and st2
    /// takes its false subgraph: the Tab board poster st6 (`06DF`,
    /// `STU_38EE1100`, remote slot 4) runs, with `0CB6` under st21, which
    /// the hand-built frame stood in for (`scene::BOARD_POSTERS` had the
    /// Range's rows). Its SendGameMessage states hold to every entity
    /// (`STU_1D719351` byte A) `0727` (st1, the controllers' `0C90` st28),
    /// `0694` (st3), `06DF` (st6), `0724` with invulnerable, heal and hero
    /// select and no team (st9, the spawn rooms' `0D62`), `06FD` with 3
    /// respawn seconds (st15, the bodies' `0C8E`) and `0702` (st18, the
    /// doors' `0D32`); and its st5 SubScript `0CBB` (the PvP modes' shared
    /// subgraph) holds from its entry rid9 `06E3` (st6, `017E` = #8736e)
    /// and `06EE` (st8), whose targets have byte A set too (their bytes B
    /// and C differ). That is what the room gives every entity
    /// ([`crate::messages::deliver_held`]).
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_range_mode_graph_drives_the_mode_entity_with_its_board_poster() {
        let assets = SceneAssets::load(lucy_testkit::runtime_dir()).unwrap();
        let scene = assets.prepare(GameMap::practice_range()).unwrap();
        let mut mode = ModeRunner::start(&scene, 100, Vec::new());
        assert!(mode.advance(101, &Variables::new(), Vec::new()).is_empty());
        let ModeRunner::Graph {
            script,
            graphs,
            codecs,
            ..
        } = &mode
        else {
            panic!("13C0 doesn't drive the Range: {:?}", mode.gaps());
        };
        let snapshot = script.snapshot().unwrap();
        let root = &snapshot.instances[&1];
        assert_eq!(root.graph, 0x13c0);
        assert!(root.states.get(&6).is_some_and(|state| state.active));
        let encoder = SnapshotEncoder::new(graphs, codecs)
            .with_unknowns(script.unknowns())
            .with_presenters();
        let projected = encoder.project(snapshot, View::Observer).unwrap();
        assert!(
            projected.state(1, 6).is_some(),
            "the poster is in the frame"
        );
        assert!(projected.instance_graphs().any(
            |(id, graph)| graph == 0x0cb6 && projected.instance(id).unwrap().1 == Some((1, 21))
        ));
        let held: Vec<(u64, &Variables)> = mode
            .held()
            .iter()
            .map(|message| {
                assert_eq!(
                    (message.sender, message.target),
                    (GAME_MODE_ENTITY, EVERY_ENTITY)
                );
                (message.message & 0xFFFF, &message.parameters)
            })
            .collect();
        let mut ids: Vec<u64> = held.iter().map(|&(id, _)| id).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            [
                0x0694, 0x06df, 0x06e3, 0x06ee, 0x06fd, 0x0702, 0x0724, 0x0727
            ]
        );
        let parameters = |id| held.iter().find(|&&(held, _)| held == id).unwrap().1;
        assert_eq!(parameters(0x0724), &crate::map_areas::room_message(None));
        assert_eq!(
            parameters(0x06fd),
            &Variables::from([(0x00b7, Value::Integer(3))])
        );
        assert_eq!(
            mode.project(None),
            Some(projected),
            "the graph's frame, not the hand-built one"
        );
    }

    /// The match's end on `0F4A` (Practice vs AI on Anubis, ruleset
    /// `00E8`; `09A3` declares team 0 the winner at 32 ms, frame 2): its
    /// child `180A` gets `0695`, waits 0.24 s (st23) and shows the five
    /// cards of `#16638` in turn, each a list of graphs (`#13151.id(16748)`)
    /// and a time (`.id(215)`): 2.9 s, 5, 17, 15 and 15 s. For each graph
    /// of the shown card st17 (`STU_77E13026`) opens a scope 822 instance
    /// of `180A` on the mode entity with the graph in `#9694`, and there
    /// st16 (`STU_CB4905E9`) plays it as a root instance; a graph the next
    /// card shows too keeps its instances. After a card's time the
    /// presenter st19 (`STU_4BD2E886`: fade in 0.3 s, hold 0.1 s, fade
    /// out 0.3 s, client arithmetic) moves on to the next card as its fade
    /// in ends (#59 + 1), at the end of that server frame; card 3 has
    /// `.id(2579)` and moves on with no presenter at its time's end. After
    /// the last card `180A` sends its clients `0BED` and ends the game
    /// (`STU_147B34C6`). Every frame of the mode entity can be made.
    #[test]
    #[ignore = "requires the derived 104319 data"]
    fn the_match_end_plays_its_cards_and_then_ends_the_game() {
        use lucy_content::maps::MapCatalog;
        const ANUBIS: u64 = 0x0800_0000_0000_005b;
        const CARDS: u16 = 7;
        let runtime = lucy_testkit::runtime_dir();
        let assets = SceneAssets::load(&runtime).unwrap();
        let catalog = MapCatalog::load(runtime.join("data")).unwrap();
        let mut map = catalog
            .game_map(
                ANUBIS,
                crate::match_flow::ASSAULT,
                "Assault",
                false,
                &[6, 6],
            )
            .unwrap();
        map.ruleset = 0x0fd0_0000_0000_00e8;
        let mut scene = assets.prepare(map).unwrap();
        scene.advance_placed(1, &[]);
        let mut mode = ModeRunner::start(&scene, 1, Vec::new());
        assert!(mode.holds_rooms(), "{:?}", mode.gaps());
        mode.start_match();
        mode.advance(2, &Variables::new(), Vec::new());
        mode.receive_message(&GameMessage {
            sender: 0,
            target: GAME_MODE_ENTITY,
            message: 0x0240_0000_0000_09a3,
            parameters: Variables::from([(0x008d, Value::Integer(0))]),
        });
        let graph = |index: u64| Value::Asset(0x0580_0000_0000_0000 | index);
        let mut projected: Option<ProjectedSnapshot> = None;
        let mut steps = Vec::new();
        let mut bits = Vec::new();
        let mut over = None;
        let mut last_step = None;
        let mut last_bit = None;
        let mut shown = std::collections::BTreeSet::new();
        for frame in 3..=3600u32 {
            mode.advance(frame, &Variables::new(), Vec::new());
            let next = mode.project(projected.as_ref());
            assert!(
                next.is_some(),
                "frame {frame} can't be made: {:?}",
                mode.gaps()
            );
            projected = next;
            let frame_now = projected.as_ref().unwrap();
            let Some(script) = mode.script_mut() else {
                panic!("the mode graph stopped: {:?}", mode.gaps());
            };
            let cards = &script.snapshot().unwrap().instances[&CARDS];
            assert_eq!(cards.graph, 0x180a);
            let step = cards.variables.get(&59).cloned();
            if step != last_step {
                steps.push((frame, step.clone()));
                last_step = step;
            }
            let presenter = cards.states.get(&19).filter(|state| state.active).map(|_| {
                frame_now.state_payload(CARDS, 19).unwrap_or_else(|| {
                    panic!(
                        "{frame}: {:?}",
                        (0..30u16)
                            .filter(|&raw| frame_now.state(CARDS, raw).is_some())
                            .collect::<Vec<_>>()
                    )
                })["value"]
                    .clone()
            });
            if presenter != last_bit {
                bits.push((frame, presenter.clone()));
                last_bit = presenter;
            }
            if over.is_none() && mode.match_state().is_some_and(|state| state.over) {
                over = Some(frame);
            }
            let listed = |frame_now: &ProjectedSnapshot| {
                frame_now
                    .instance_graphs()
                    .filter(|&(id, _)| id > 8)
                    .collect::<Vec<_>>()
            };
            shown.extend(
                listed(frame_now)
                    .into_iter()
                    .map(|(_, graph)| graph)
                    .filter(|&graph| graph != 0x180a),
            );
            if frame == 20 {
                // Card 0 (from 272 ms): `027F`, `17DF`, `181E`, `0D94`,
                // `184C`, each in a scope instance (9 to 13) and played in
                // a root instance (14 to 18).
                assert_eq!(
                    listed(frame_now),
                    [
                        (9, 0x180a),
                        (10, 0x180a),
                        (11, 0x180a),
                        (12, 0x180a),
                        (13, 0x180a),
                        (14, 0x027f),
                        (15, 0x17df),
                        (16, 0x181e),
                        (17, 0x0d94),
                        (18, 0x184c)
                    ]
                );
                for id in 9..=18 {
                    assert_eq!(frame_now.instance(id).unwrap().1, None);
                }
                // st17's frame record: per graph its value, the entity
                // holding its scope instance and that instance, its index,
                // not kept.
                let hits: Vec<Value> = [0x027f, 0x17df, 0x181e, 0x0d94, 0x184c]
                    .into_iter()
                    .zip(9..)
                    .enumerate()
                    .map(|(index, (played, instance))| {
                        Value::Array(vec![
                            graph(played),
                            Value::Entity(GAME_MODE_ENTITY),
                            Value::Integer(instance),
                            Value::Integer(index as i64),
                            Value::Bool(false),
                        ])
                    })
                    .collect();
                assert_eq!(
                    frame_now.state_payload(CARDS, 17),
                    Some(&lucy_script::codecs::StatePayload::from([
                        ("hits".into(), Value::Array(hits)),
                        ("flag".into(), Value::Bool(false)),
                    ]))
                );
            }
            if frame == 218 {
                // Card 1, in the frame after #59 moved on (st17 lists its
                // card's graphs again on its tick). Its server-only graphs
                // ran at once and exited: `1F93` (`0F58` to every body)
                // and `181D` (`0669` to every body, then it removes the
                // entities tagged `0D60…44`). `17DF` and `184C` stay;
                // `0281` and `17E2` came, the others left.
                assert_eq!(
                    listed(frame_now),
                    [
                        (10, 0x180a),
                        (13, 0x180a),
                        (15, 0x17df),
                        (18, 0x184c),
                        (19, 0x180a),
                        (20, 0x180a),
                        (21, 0x180a),
                        (22, 0x180a),
                        (24, 0x0281),
                        (25, 0x17e2)
                    ]
                );
            }
            if over.is_some() {
                break;
            }
        }
        // The cards' graphs the clients get, each for at least a frame (the
        // server-only `1F93` and `181D` exit in the frame they came).
        assert_eq!(
            shown.into_iter().collect::<Vec<_>>(),
            [
                0x027f, 0x0281, 0x0282, 0x0283, 0x0b46, 0x0d94, 0x17ca, 0x17df, 0x17e2, 0x17e3,
                0x17e4, 0x1818, 0x181e, 0x184c, 0x184d
            ]
        );
        // In ms: card 0 at 32 + 240 = 272 (frame 17, which sets #59 to 0),
        // its presenter at 272 + 2900 = 3172, which moves on at the first
        // frame end 300 ms later (3472, frame 217); card 1 from there, its
        // presenter at 8472, on at 8784 (frame 549); card 2's presenter at
        // 8784 + 17000, on at 26096 (frame 1631); card 3 (`2579`) moves on
        // at its time's end, 41096 (frame 2569, the frame ending 41104);
        // card 4's presenter at 56096, on at 56400 (frame 3525), and with
        // #59 at 5 the game ends.
        assert_eq!(
            steps,
            [
                (17, Some(Value::Float(0.0))),
                (217, Some(Value::Float(1.0))),
                (549, Some(Value::Float(2.0))),
                (1631, Some(Value::Float(3.0))),
                (2569, Some(Value::Float(4.0))),
                (3525, Some(Value::Float(5.0)))
            ]
        );
        assert_eq!(over, Some(3525));
        // Card 0's presenter: on from 3172 (frame 199, the frame ending
        // 3184) unset, set once 0.1 s passed after its fade in (3572,
        // frame 224), off once its fade out ends (3872, frame 242).
        assert_eq!(
            &bits[..3],
            [
                (199, Some(Value::Bool(false))),
                (224, Some(Value::Bool(true))),
                (242, None)
            ]
        );
    }
}
