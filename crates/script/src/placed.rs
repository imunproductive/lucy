//! The server-owned map placeables' statescript: one component per placed
//! entity, as the client starts a placed script (leafall903/ORPHEUS
//! `docs/PLACED_SCRIPT_INITIALIZATION.txt`, BE8E10): the definition's
//! initial graphs in order, then the instance data's overrides written into
//! the first of those graphs with the same index (an override without one
//! is skipped), then the instance data's extra graphs.
//!
//! The placeables' target searches find the players. A component that
//! meets content the VM doesn't implement (refused in debug builds) or
//! answers by assumption is not complete: the server doesn't send its
//! frames (see [`MapEntityWorld::complete`]).

use std::{collections::BTreeMap, sync::Arc};

use serde_json::Value as Json;

use crate::{
    graph::{GraphBinding, GraphStore},
    vm::{
        Component, GameMessage, InitFault, MAX_INIT_WORK, Root, Site, Snapshot, TargetCandidate,
        Unknowns, World, bind_values,
    },
    wire::{Value, Variables},
};

/// One placed entity as the map places it.
pub struct Placement<'a> {
    /// `0x80000000 | flat index`.
    pub entity: u32,
    /// The entity definition (`003`).
    pub definition: u64,
    pub position: [f64; 3],
    /// `STUStatescriptComponentInstanceData.m_6D10093E`.
    pub overrides: &'a [GraphBinding],
    /// `STUStatescriptComponentInstanceData.m_84D0983B`.
    pub extra: &'a [GraphBinding],
}

/// The roots a placement starts with, in the client's order (see the
/// module notes). The definition's graphs keep their list position as
/// instance id, as a body's do; the extra graphs follow.
pub fn placed_roots(
    graphs: &GraphStore,
    placement: &Placement<'_>,
) -> Result<Vec<Root>, InitFault> {
    let mut roots = match graphs.entity_body(placement.definition) {
        Some(body) => crate::vm::body_roots(body)?,
        None => Vec::new(),
    };
    for binding in placement.overrides {
        let Some(graph) = binding.graph else {
            continue;
        };
        if let Some(root) = roots.iter_mut().find(|root| root.graph == graph) {
            bind_values(root, binding.sync.iter().flatten())?;
        }
    }
    let mut id = roots.last().map_or(0, |root| root.id);
    for binding in placement.extra {
        let Some(graph) = binding.graph else {
            continue;
        };
        id = id.checked_add(1).ok_or(InitFault::InstanceIdExhausted)?;
        let mut root = Root::new(id, graph);
        bind_values(&mut root, binding.sync.iter().flatten())?;
        roots.push(root);
    }
    Ok(roots)
}

/// Classes or faults by graph, with how often they were met.
pub type Gaps = BTreeMap<(u32, String), usize>;

/// A placed entity's script.
pub struct PlacedEntity {
    pub entity: u32,
    pub position: [f64; 3],
    component: Option<Component>,
    /// Why the component stopped or never started.
    fault: Option<String>,
}

impl PlacedEntity {
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.component.as_ref()?.snapshot()
    }

    /// Whether its graphs ran only on what the VM implements: no gap was
    /// refused and no answer was an assumption.
    pub fn complete(&self) -> bool {
        self.fault.is_none()
            && self.snapshot().is_some_and(|snapshot| {
                snapshot.initialization_complete() && snapshot.assumptions.is_empty()
            })
    }

    pub fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
}

/// What the match lends the placed scripts each frame.
struct Shared<'a> {
    graphs: &'a GraphStore,
    mode: u64,
    mode_random: f32,
    unknowns: Unknowns,
    players: &'a [TargetCandidate],
    outbox: &'a mut Vec<GameMessage>,
}

/// The world one placed entity's script sees: itself where the map put it,
/// and the players.
struct EntityWorld<'a, 'b> {
    shared: &'b mut Shared<'a>,
    entity: u32,
    position: [f64; 3],
}

impl World for EntityWorld<'_, '_> {
    fn unknowns(&self) -> Unknowns {
        self.shared.unknowns
    }

    fn assumption(&self, class: &str) -> Option<&'static str> {
        // As `PlayerWorld`: the tunable's authored value, not scaled for the
        // entity.
        (class == "STU_082FA44E").then_some("the tunable's authored value")
    }

    fn ruleset(&mut self, id: u32) -> Option<Value> {
        self.shared
            .graphs
            .rulesets()
            .default(id, 0, self.shared.mode)
    }

    fn mode_random(&self) -> Option<f64> {
        Some(f64::from(self.shared.mode_random))
    }

    fn position(&mut self, entity: u32) -> [f64; 3] {
        let site = Site {
            entity: self.entity,
            instance: 0,
            graph: 0,
            node: None,
        };
        self.entity_position(site, entity).unwrap_or_default()
    }

    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_082FA44E" => self
                .shared
                .graphs
                .tunables()
                .value(config)
                .map(Value::Float),
            _ => None,
        }
    }

    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        Some(self.shared.players)
    }

    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        Some(
            entity == self.entity
                || self
                    .shared
                    .players
                    .iter()
                    .any(|player| player.entity == entity),
        )
    }

    fn entity_position(&self, _site: Site, entity: u32) -> Option<[f64; 3]> {
        if entity == self.entity {
            return Some(self.position);
        }
        self.shared
            .players
            .iter()
            .find(|player| player.entity == entity)
            .map(|player| player.feet)
    }

    fn send_message(
        &mut self,
        _source: Site,
        target: u32,
        message: u64,
        parameters: &Variables,
    ) -> bool {
        self.shared.outbox.push(GameMessage {
            sender: self.entity,
            target,
            message,
            parameters: parameters.clone(),
        });
        true
    }
}

/// Every placed entity's script in one match.
pub struct MapEntityWorld {
    graphs: Arc<GraphStore>,
    mode: u64,
    /// The match's mode entity value (`World::mode_random`).
    mode_random: f32,
    unknowns: Unknowns,
    players: Vec<TargetCandidate>,
    outbox: Vec<GameMessage>,
    entities: Vec<PlacedEntity>,
    frame: u32,
}

impl MapEntityWorld {
    /// Start every placement that has graphs at `frame`, in order. A
    /// placement whose start faults keeps the fault and no component.
    pub fn start<'p>(
        graphs: Arc<GraphStore>,
        mode: u64,
        mode_random: f32,
        unknowns: Unknowns,
        placements: impl IntoIterator<Item = Placement<'p>>,
        frame: u32,
    ) -> Self {
        let mut world = Self {
            graphs,
            mode,
            mode_random,
            unknowns,
            players: Vec::new(),
            outbox: Vec::new(),
            entities: Vec::new(),
            frame,
        };
        for placement in placements {
            let roots = match placed_roots(&world.graphs, &placement) {
                Ok(roots) if roots.is_empty() => continue,
                Ok(roots) => roots,
                Err(error) => {
                    world.entities.push(PlacedEntity {
                        entity: placement.entity,
                        position: placement.position,
                        component: None,
                        fault: Some(error.to_string()),
                    });
                    continue;
                }
            };
            let mut shared = Shared {
                graphs: &world.graphs,
                mode,
                mode_random,
                unknowns,
                players: &world.players,
                outbox: &mut world.outbox,
            };
            let mut entity_world = EntityWorld {
                shared: &mut shared,
                entity: placement.entity,
                position: placement.position,
            };
            let started = Component::start(
                Arc::clone(&world.graphs),
                placement.entity,
                Variables::new(),
                roots,
                frame,
                &mut entity_world,
                MAX_INIT_WORK,
            )
            .and_then(|mut component| {
                component.advance_frame(frame, &mut entity_world)?;
                Ok(component)
            });
            let (component, fault) = match started {
                Ok(component) => (Some(component), None),
                Err(error) => (None, Some(error.to_string())),
            };
            world.entities.push(PlacedEntity {
                entity: placement.entity,
                position: placement.position,
                component,
                fault,
            });
        }
        world
    }

    pub fn unknowns(&self) -> Unknowns {
        self.unknowns
    }

    pub fn entities(&self) -> &[PlacedEntity] {
        &self.entities
    }

    pub fn entity(&self, entity: u32) -> Option<&PlacedEntity> {
        self.entities.iter().find(|placed| placed.entity == entity)
    }

    /// The bodies the placeables' target searches find.
    pub fn set_players(&mut self, players: Vec<TargetCandidate>) {
        self.players = players;
    }

    /// Advance every running script to a later server frame; false for an
    /// old one. A script that faults stops; the others run on.
    pub fn advance(&mut self, frame: u32) -> bool {
        if frame <= self.frame {
            return false;
        }
        self.frame = frame;
        let mut shared = Shared {
            graphs: &self.graphs,
            mode: self.mode,
            mode_random: self.mode_random,
            unknowns: self.unknowns,
            players: &self.players,
            outbox: &mut self.outbox,
        };
        for placed in &mut self.entities {
            let Some(component) = placed.component.as_mut() else {
                continue;
            };
            let mut world = EntityWorld {
                shared: &mut shared,
                entity: placed.entity,
                position: placed.position,
            };
            if let Err(error) = component.advance_frame(frame, &mut world) {
                placed.fault = Some(error.to_string());
                placed.component = None;
            }
        }
        true
    }

    /// The game messages the scripts sent, for the match to route.
    pub fn take_messages(&mut self) -> Vec<GameMessage> {
        std::mem::take(&mut self.outbox)
    }

    /// The classes the VM refused or answered by assumption, and the
    /// faults, with how often, by graph (0 for a fault outside a graph).
    pub fn gaps(&self) -> Gaps {
        let mut gaps = Gaps::new();
        for placed in &self.entities {
            if let Some(fault) = &placed.fault {
                *gaps.entry((0, fault.clone())).or_default() += 1;
            }
            let Some(snapshot) = placed.snapshot() else {
                continue;
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
        }
        gaps
    }
}
