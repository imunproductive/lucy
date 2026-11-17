//! The game mode entity's frames in a PvP match: from the mode's own graph
//! when the VM runs it whole, else the hand-built mode UI frame carrying the
//! match flow's variables.
//!
//! At the match's start the room starts the mode graph once
//! ([`ModeRunner::start`]). If it runs with no refused content and no
//! assumption, it stays: the flow's variables are written into it each
//! tick, it advances with the server frame, its observer frames are the
//! mode entity's, and its messages go to the players' bodies. Otherwise
//! (0F4A and 0EC2 today, see `orpheus_script::mode`) the gaps are logged
//! once and the mode entity gets graph 0CB6's frame with the flow's
//! variables, as the countdown did before (match.py `_send_countdown`).

use std::{collections::BTreeMap, io, sync::Arc};

use orpheus_core::bits::BitWriter;
use orpheus_script::{
    codecs::CodecTable,
    graph::GraphStore,
    mode::{ModeScript, ModeWorld},
    snapshot::{SnapshotEncoder, View},
    vm::{GameMessage, Unknowns},
    wire::Variables,
};

use crate::{match_flow, scene_assets::PreparedScene};

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
        _ => return None,
    })
}

/// Classes or faults by graph, with how often they were met.
pub type Gaps = BTreeMap<(u32, String), usize>;

pub enum ModeRunner {
    /// The graph drives the mode entity's frames.
    Graph {
        script: Box<ModeScript>,
        graphs: Arc<GraphStore>,
        codecs: Arc<CodecTable>,
    },
    /// The hand-built frame, and why the graph doesn't drive it.
    HandBuilt { gaps: Gaps },
}

impl ModeRunner {
    pub fn start(scene: &PreparedScene, frame: u32) -> Self {
        let mode = scene.map().mode_guid;
        let Some(graph) = mode_graph(mode) else {
            return Self::HandBuilt { gaps: Gaps::new() };
        };
        let graphs = scene.shared_graphs();
        // The team sizes are the game's, as the 20300/20301 game struct
        // sends them. The room gives no players or start condition yet:
        // with the graph's gaps the flow drives the match, so the graph only
        // lists them here.
        let mut world = ModeWorld::new(Arc::clone(&graphs), mode, Unknowns::for_build());
        world.team_sizes = crate::instance::team_entries(scene.map());
        world.mode_random = Some(scene.mode_random());
        world.map_modes = Some(scene.map().supported_modes.clone());
        match ModeScript::start(world, graph, frame) {
            Ok(script) if script.complete() => Self::Graph {
                script: Box::new(script),
                graphs,
                codecs: Arc::new(CodecTable::client()),
            },
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

    pub fn graph_driven(&self) -> bool {
        matches!(self, Self::Graph { .. })
    }

    /// Why the graph doesn't drive the mode entity; empty when it does.
    pub fn gaps(&self) -> Option<&Gaps> {
        match self {
            Self::HandBuilt { gaps } => Some(gaps),
            Self::Graph { .. } => None,
        }
    }

    /// One server frame with the flow's variables; the messages the graph
    /// sent.
    pub fn advance(&mut self, frame: u32, variables: &Variables) -> Vec<GameMessage> {
        let Self::Graph { script, .. } = self else {
            return Vec::new();
        };
        let result = script
            .set_variables(variables)
            .and_then(|()| script.advance(frame).map(|_| ()));
        match result {
            Ok(()) => script.take_messages(),
            Err(error) => {
                eprintln!(
                    "the mode graph stopped ({error}); the match flow drives the mode entity"
                );
                *self = Self::HandBuilt {
                    gaps: Gaps::from([((0, error.to_string()), 1)]),
                };
                Vec::new()
            }
        }
    }

    /// The mode entity's full frame now.
    pub fn frame(&self, scene: &PreparedScene, variables: &Variables) -> io::Result<BitWriter> {
        if let Self::Graph {
            script,
            graphs,
            codecs,
        } = self
        {
            if let Some(snapshot) = script.snapshot() {
                let encoder = SnapshotEncoder::new(graphs, codecs).with_unknowns(script.unknowns());
                return encoder
                    .project(snapshot, View::Observer)
                    .and_then(|projected| encoder.full(&projected))
                    .map_err(|error| {
                        io::Error::new(io::ErrorKind::InvalidData, format!("{error:?}"))
                    });
            }
        }
        scene.mode_frame(variables)
    }
}
