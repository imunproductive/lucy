//! The game mode entity's frames: from the mode's own graph when the VM
//! runs it whole, else the hand-built mode UI frame carrying the match
//! flow's variables.
//!
//! In a PvP match the room starts the mode graph once at the match's start
//! ([`ModeRunner::start`]); the Practice Range, which has no match flow
//! ([`without_flow`]), starts `13C0` with the room, before the placed
//! scripts' first tick. If it runs with no refused content and no
//! assumption, it stays: the flow's variables (none in the Range) and the
//! match's players (`ModeScript::set_players`) are written into it each
//! tick, it advances with the server frame, its observer frames are the
//! mode entity's (each client gets them as deltas, [`crate::mode_view`]),
//! and its messages go with every other entity's
//! ([`crate::messages`]), which routes it those sent to the mode entity or
//! to every entity and gives every entity the broadcasts it holds
//! ([`ModeRunner::held`]; in the Range `0724` for the spawn rooms, `06FD`
//! for the bodies). Otherwise
//! (0F4A and 0EC2 today, see `orpheus_script::mode`) the gaps are logged
//! once and the mode entity gets graph 0CB6's frame with the flow's
//! variables, as the countdown did before (match.py `_send_countdown`).

use std::{collections::BTreeMap, sync::Arc};

use orpheus_content::maps::{PRACTICE_RANGE_MODE, PYTHON_PRACTICE_RANGE_MODE};
use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    mode::{ModePlayer, ModeScript, ModeWorld},
    snapshot::{ProjectedSnapshot, SnapshotEncoder, View},
    vm::{GameMessage, InitFault, MessageEnd, Unknowns},
    wire::Variables,
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

use crate::{match_flow, scene_assets::PreparedScene};

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
        SKIRMISH => 0x0d76,
        ELIMINATION => 0x0d0b,
        TEAM_DEATHMATCH => 0x0f60,
        DEATHMATCH | BOUNTY_HUNTER => 0x0f73,
        PRACTICE_RANGE_MODE | PYTHON_PRACTICE_RANGE_MODE => 0x13c0,
        _ => return None,
    })
}

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
        /// The mode's own graph runs (not only a Workshop host,
        /// [`Self::workshop`]): it holds the spawn rooms' `0724` itself.
        mode_graph: bool,
    },
    /// The hand-built frame, and why the graph doesn't drive it.
    HandBuilt { gaps: Gaps },
}

/// The PvP modes' shared subgraph (`0CBB`, ModeGraphGaps.md), which plays
/// the Workshop's value table `1B4A` from its entry
/// (`refs/reports/Workshop.md` "WorkshopRuntime").
pub const SHARED_MODE_GRAPH: u32 = 0x0cbb;

impl ModeRunner {
    /// Start the mode graph with the match's `players` now. The start
    /// condition `STU_81A9AB10` (server-only) is the server's own rule:
    /// true once the match's players are in the world, when the room
    /// starts the mode.
    pub fn start(scene: &PreparedScene, frame: u32, players: Vec<ModePlayer>) -> Self {
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
        let started = Self::start(scene, frame, players.clone());
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
            mode_graph,
        }
    }

    pub fn graph_driven(&self) -> bool {
        matches!(self, Self::Graph { .. })
    }

    /// Whether the mode's own graph runs and holds the spawn rooms' `0724`
    /// itself; else the match flow's spawns give it.
    pub fn holds_rooms(&self) -> bool {
        matches!(
            self,
            Self::Graph {
                mode_graph: true,
                ..
            }
        )
    }

    /// The graph on the mode entity, while one drives it.
    pub fn script_mut(&mut self) -> Option<&mut ModeScript> {
        match self {
            Self::Graph { script, .. } => Some(script),
            Self::HandBuilt { .. } => None,
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

    /// The mode entity's observer frame now, while the graph drives it,
    /// with `previous`'s entity exclusions (`SnapshotEncoder::project_next`),
    /// so each client gets the match's frames as deltas of his first one
    /// (`crate::mode_view`). None while the graph doesn't drive it, or when
    /// the frame can't be made (said once until it can again).
    pub fn project(&mut self, previous: Option<&ProjectedSnapshot>) -> Option<ProjectedSnapshot> {
        let Self::Graph {
            script,
            graphs,
            codecs,
            unprojected,
            ..
        } = self
        else {
            return None;
        };
        let snapshot = script.snapshot()?;
        let encoder = SnapshotEncoder::new(graphs, codecs).with_unknowns(script.unknowns());
        let projected = match previous {
            Some(previous) => encoder.project_next(snapshot, previous),
            None => encoder.project(snapshot, View::Observer),
        };
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
        Some(SnapshotEncoder::new(graphs, codecs).with_unknowns(script.unknowns()))
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
    world.match_seed = Some(crate::scene::draw_match_seed());
    world.map_modes = Some(scene.map().supported_modes.clone());
    world.map_tags = Some(scene.map_tags());
    world.map_rays = Some(scene.map_rays());
    world
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene_assets::SceneAssets;
    use orpheus_content::maps::GameMap;
    use orpheus_script::{mode::EVERY_ENTITY, wire::Value};

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
        let assets = SceneAssets::load(orpheus_testkit::runtime_dir()).unwrap();
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
        let encoder = SnapshotEncoder::new(graphs, codecs).with_unknowns(script.unknowns());
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
}
