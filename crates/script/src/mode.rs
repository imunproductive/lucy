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

/// What the mode graph asks outside its own entity. The mode entity's
/// variables are its own (the server writes them into the component);
/// the bundle's mode variables (`0C5.m_3CE93B76`) answer the rest.
pub struct ModeWorld {
    pub graphs: Arc<GraphStore>,
    pub mode: u64,
    pub unknowns: Unknowns,
    outbox: Vec<GameMessage>,
}

impl ModeWorld {
    pub fn new(graphs: Arc<GraphStore>, mode: u64, unknowns: Unknowns) -> Self {
        Self {
            graphs,
            mode,
            unknowns,
            outbox: Vec::new(),
        }
    }

    /// The game messages the graph sent other entities, for the match to
    /// route (like `PlayerWorld::take_messages`).
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }
}

impl World for ModeWorld {
    fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        // As `PlayerWorld`: the tunable's authored value, not scaled for the
        // entity.
        (class == "STU_082FA44E").then_some("the tunable's authored value")
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
    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            "STU_082FA44E" => self.graphs.tunables().value(config).map(Value::Float),
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
}

/// The mode graph running on the mode entity.
pub struct ModeScript {
    component: Component,
    world: ModeWorld,
    frame: u32,
}

impl ModeScript {
    /// Start `graph` as the mode entity's instance 1 at `frame`.
    pub fn start(
        graphs: Arc<GraphStore>,
        mode: u64,
        graph: u32,
        unknowns: Unknowns,
        frame: u32,
    ) -> Result<Self, InitFault> {
        let mut world = ModeWorld::new(Arc::clone(&graphs), mode, unknowns);
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
    };

    const ASSAULT: u64 = 0x0230_0000_0000_0014;

    fn graphs() -> Arc<GraphStore> {
        let root = orpheus_testkit::runtime_dir();
        Arc::new(GraphStore::load(root.join("data/game_graphs_174.json.gz")).unwrap())
    }

    /// What runs of 0F4A today: it starts and runs when unknown content is
    /// tolerated, and its own nodes set the phase banner text (v1923 =
    /// `0DE…6114`, the assemble phase) on the mode entity, but it reaches
    /// classes the VM doesn't know, so its frames can't drive the match yet.
    /// Debug builds refuse it.
    #[test]
    #[ignore = "requires the derived 104319 graph bundle"]
    fn the_assault_mode_graph_runs_only_with_assumptions() {
        let graphs = graphs();
        let refused = ModeScript::start(Arc::clone(&graphs), ASSAULT, 0x0f4a, Unknowns::Refuse, 1);
        assert!(refused.map_or(true, |script| !script.complete()));
        let mut script =
            ModeScript::start(Arc::clone(&graphs), ASSAULT, 0x0f4a, Unknowns::Tolerate, 1).unwrap();
        script
            .set_variables(&Variables::from([(1303, Value::Float(30.0))]))
            .unwrap();
        for frame in 2..200 {
            script.advance(frame).unwrap();
        }
        assert!(!script.complete());
        let gaps = script.gaps();
        for class in ["STU_6A926B21", "STU_1D719351", "STU_9B213168"] {
            assert!(
                gaps.keys()
                    .any(|(graph, gap)| *graph == 0x0f4a && gap == class),
                "{class} in {gaps:?}"
            );
        }
        let snapshot = script.snapshot().unwrap();
        assert_eq!(
            snapshot.variables.get(&1923),
            Some(&Value::Asset(0x0de0_0000_0000_6114))
        );
        let codecs = CodecTable::client();
        let encoder = SnapshotEncoder::new(&graphs, &codecs).with_unknowns(Unknowns::Tolerate);
        let projected = encoder.project(snapshot, View::Observer).unwrap();
        assert!(encoder.full(&projected).is_ok());
    }
}
