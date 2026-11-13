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
    graph::GraphStore,
    vm::{Component, GameMessage, InitFault, MAX_INIT_WORK, Root, Site, Snapshot, Unknowns, World},
    wire::{Value, Variables},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

/// The target [`ModeScript::take_messages`] gives a message the graph sent
/// to every entity ([`World::broadcast_message`]); entity 0 is never one.
pub const EVERY_ENTITY: u32 = 0;

/// What the mode graph asks outside its own entity. The mode entity's
/// variables are its own (the server writes them into the component);
/// the bundle's mode variables (`0C5.m_3CE93B76`) answer the rest.
pub struct ModeWorld {
    pub graphs: Arc<GraphStore>,
    pub mode: u64,
    pub unknowns: Unknowns,
    /// The players on teams 0 and 1 (`STU_A5B9EAAD`).
    pub team_sizes: [i64; 2],
    /// The server's answer to `STU_81A9AB10`, which lets the match leave
    /// its waiting phase.
    pub ready: bool,
    outbox: Vec<GameMessage>,
}

impl ModeWorld {
    pub fn new(graphs: Arc<GraphStore>, mode: u64, unknowns: Unknowns) -> Self {
        Self {
            graphs,
            mode,
            unknowns,
            team_sizes: [0; 2],
            ready: false,
            outbox: Vec::new(),
        }
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
            "STU_81A9AB10" => Some("the server's own start condition"),
            "STU_21A5BB52" => Some("false"),
            _ => None,
        }
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.graphs.rulesets().default(id, 0, self.mode)
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

    /// `STU_A3DEF368` is the first entity with component 114, the mode
    /// entity itself (ow1-preservation: getter 0xcdd340). `STU_082FA44E`
    /// reads a tunable's authored value, as `PlayerWorld` does.
    ///
    /// `STU_A5B9EAAD` (an int with only `m_team`) is the number of players
    /// on the team: all seven uses ask that teams 0 and 1 both have one
    /// before the match leaves its waiting phase (`0F4A` nodes 14 and 419,
    /// `0EC2` 15 and 380, `0D0B` 237, `0F60` 27, `0F73` 16). Inferred from
    /// those uses, not read in the client.
    ///
    /// `STU_81A9AB10` (a bool with no fields) gates the same start in the
    /// default start rule (ruleset `#9672` = `25CA`; `0F4A` node 7: true
    /// waits for both teams, false waits for message `0789`), and Tutorial
    /// graph `091B` waits on it to send `01DB` to the mode entity. Its
    /// native meaning is open (refs/reports/IDA-questions.md); the server
    /// answers [`ModeWorld::ready`], an assumption.
    ///
    /// `STU_21A5BB52` (a bool with no fields) picks, outside Competitive
    /// play, whether the payload's random speed factor #29093 comes from
    /// message `11BA` (`0F4A` uid 1299); `0F60` and `0F73` read it in
    /// `STU_F7216D4F`. Its meaning is open; the server answers false, an
    /// assumption.
    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
            "STU_A5B9EAAD" => {
                let team = config.get("m_team")?;
                if team.get("$")?.as_str()? != "STU_BD6CC023" {
                    return None;
                }
                let team = usize::try_from(team.get("m_6CE7A900")?.as_i64()?).ok()?;
                self.team_sizes.get(team).copied().map(Value::Integer)
            }
            "STU_81A9AB10" => Some(Value::Bool(self.ready)),
            "STU_21A5BB52" => Some(Value::Bool(false)),
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
            sender: GAME_MODE_ENTITY,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
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
}

/// The mode graph running on the mode entity.
pub struct ModeScript {
    component: Component,
    world: ModeWorld,
    frame: u32,
}

impl ModeScript {
    /// Start `graph` as the mode entity's instance 1 at `frame`, against
    /// `world`.
    pub fn start(mut world: ModeWorld, graph: u32, frame: u32) -> Result<Self, InitFault> {
        let graphs = Arc::clone(&world.graphs);
        let mut component = Component::start(
            graphs,
            GAME_MODE_ENTITY,
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
    /// often, by graph: what keeps the graph from driving the match.
    pub fn gaps(&self) -> BTreeMap<(u32, String), usize> {
        let mut gaps = BTreeMap::new();
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

    /// Advance to a later server frame; false for an old one.
    pub fn advance(&mut self, frame: u32) -> Result<bool, InitFault> {
        if frame <= self.frame {
            return Ok(false);
        }
        self.frame = frame;
        self.component.advance_frame(frame, &mut self.world)?;
        Ok(true)
    }

    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        self.world.take_messages()
    }
}

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
    /// The mode graph's phase (v59 on its instance 1).
    const PHASE: u32 = 59;

    fn graphs() -> Arc<GraphStore> {
        let data = orpheus_testkit::runtime_dir().join("data");
        Arc::new(GraphStore::load_runtime(data, "game_graphs_174.json.gz").unwrap())
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
        world.team_sizes = [1, 1];
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
