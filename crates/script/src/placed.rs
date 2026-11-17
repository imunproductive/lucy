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
    map_tags::{MapTags, TaggedEntity},
    vm::{
        Component, GameMessage, InitFault, MAX_INIT_WORK, Root, Site, Snapshot, TargetCandidate,
        Unknowns, World, bind_values,
    },
    wire::{Value, Variables},
};
use orpheus_wire::entity::GAME_MODE_ENTITY;

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
    /// The team index its script gave it (`STU_0F647E55`), if any.
    team: Option<i64>,
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

    /// The team index its script gave it (`STU_0F647E55`; -1 none, 0 to
    /// 4); None: its script gave none.
    pub fn team(&self) -> Option<i64> {
        self.team
    }
}

/// What the match lends the placed scripts each frame.
struct Shared<'a> {
    graphs: &'a GraphStore,
    mode: u64,
    mode_random: f32,
    unknowns: Unknowns,
    players: &'a [TargetCandidate],
    map_tags: Option<&'a MapTags>,
    outbox: &'a mut Vec<GameMessage>,
}

impl Shared<'_> {
    /// A tagged placeable of the map.
    fn tagged(&self, entity: u32) -> Option<&TaggedEntity> {
        self.map_tags?.get(entity)
    }
}

/// The world one placed entity's script sees: itself where the map put it,
/// and the players.
struct EntityWorld<'a, 'b> {
    shared: &'b mut Shared<'a>,
    entity: u32,
    position: [f64; 3],
    team: &'b mut Option<i64>,
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

    /// The mode entity's variables are the mode's (as [`crate::mode`]).
    fn entity_variable(&mut self, entity: u32, id: u32) -> Value {
        if entity != GAME_MODE_ENTITY {
            return Value::Null;
        }
        self.shared
            .graphs
            .mode_variables(self.shared.mode)
            .and_then(|variables| variables.get(&id))
            .copied()
            .map_or(Value::Null, Value::from)
    }

    /// `STU_A3DEF368` is the mode entity (the first entity with component
    /// 114, as [`crate::mode`] answers it): the Practice Range pad `26F8`
    /// and console `265F` read ruleset 29450 through it.
    fn native(&mut self, _site: Site, config: &Json) -> Option<Value> {
        match config.get("$")?.as_str()? {
            "STU_082FA44E" => self
                .shared
                .graphs
                .tunables()
                .value(config)
                .map(Value::Float),
            "STU_A3DEF368" => Some(Value::Entity(GAME_MODE_ENTITY)),
            _ => None,
        }
    }

    /// Its own team is the one its script gave it; a player's, the
    /// match's.
    fn entity_team(&self, _site: Site, entity: u32) -> Option<i64> {
        if entity == self.entity {
            return *self.team;
        }
        self.shared
            .players
            .iter()
            .find(|player| player.entity == entity)?
            .team
            .map(i64::from)
    }

    /// A placed entity keeps the team its script gives itself (the
    /// Practice Range's spawn markers `01CE`); it can't give one to
    /// another entity.
    fn set_entity_team(&mut self, _site: Site, entity: u32, team: i64) -> bool {
        if entity != self.entity {
            return false;
        }
        *self.team = Some(team);
        true
    }

    fn target_candidates(&self, _site: Site) -> Option<&[TargetCandidate]> {
        Some(self.shared.players)
    }

    /// The map's placeables that carry the tag ([`MapTags::with_tag`]).
    fn entities_with_tag(&self, _site: Site, tag: u64) -> Option<Vec<u32>> {
        self.shared.map_tags?.with_tag(tag)
    }

    fn has_target_tag(&self, _site: Site, entity: u32, tag: u64) -> Option<bool> {
        Some(self.shared.tagged(entity)?.tags.contains(&tag))
    }

    /// Itself, the players and the map's tagged placeables.
    fn entity_exists(&self, _site: Site, entity: u32) -> Option<bool> {
        Some(
            entity == self.entity
                || self
                    .shared
                    .players
                    .iter()
                    .any(|player| player.entity == entity)
                || self.shared.tagged(entity).is_some(),
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
            .or_else(|| self.shared.tagged(entity).map(|placed| placed.position))
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
    /// The map's tagged placeables the scene tag queries find.
    map_tags: Option<Arc<MapTags>>,
    outbox: Vec<GameMessage>,
    entities: Vec<PlacedEntity>,
    frame: u32,
}

impl MapEntityWorld {
    /// Start every placement that has graphs at `frame`, in order. A
    /// placement whose start faults keeps the fault and no component.
    /// `map_tags`: the map's tagged placeables in this mode, if known.
    #[allow(clippy::too_many_arguments)]
    pub fn start<'p>(
        graphs: Arc<GraphStore>,
        mode: u64,
        mode_random: f32,
        unknowns: Unknowns,
        placements: impl IntoIterator<Item = Placement<'p>>,
        map_tags: Option<Arc<MapTags>>,
        frame: u32,
    ) -> Self {
        let mut world = Self {
            graphs,
            mode,
            mode_random,
            unknowns,
            players: Vec::new(),
            map_tags,
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
                        team: None,
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
                map_tags: world.map_tags.as_deref(),
                outbox: &mut world.outbox,
            };
            let mut team = None;
            let mut entity_world = EntityWorld {
                shared: &mut shared,
                entity: placement.entity,
                position: placement.position,
                team: &mut team,
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
                team,
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
            map_tags: self.map_tags.as_deref(),
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
                team: &mut placed.team,
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ENTITY: u32 = 0x8000_01CE;

    /// A spawn marker as the Practice Range's `01CE` starts: it stores the
    /// mode entity (`STU_A3DEF368`) in #1, then gives itself the team #1016
    /// holds (`STU_0F647E55`), here 1.
    #[test]
    fn a_placed_entity_keeps_the_team_its_script_gives_it_and_finds_the_mode_entity() {
        let owner = json!({"$": "STU_B5A0CAF0", "m_EE729DCB": 0});
        let plug = |to: u16| json!({"$": "Plug", "links": [[to, "m_inPlug"]], "m_FF3DAF1E": []});
        let node = |pos: u16, class: &str, fields: Json| {
            json!({
                "pos": pos, "class": class, "uid": pos, "rid": pos, "remote": -1,
                "state": null, "client_only": false, "server_only": false, "fields": fields,
            })
        };
        let var = |id: u32, entity: bool| json!({"$": "STUConfigVarDynamic", "m_identifier": id, "m_60DB8F99": entity});
        let nodes = vec![
            node(0, "STUStatescriptEntry", json!({"m_outPlug": plug(1)})),
            node(
                1,
                "STUStatescriptActionSetVar",
                json!({
                    "m_out_Var": var(1, true),
                    "m_value": {"$": "STU_A3DEF368", "m_EE729DCB": 0},
                    "m_outPlug": plug(2),
                }),
            ),
            node(
                2,
                "STU_0F647E55",
                json!({"m_targets": owner, "m_67465E75": var(1016, false)}),
            ),
        ];
        let graph = json!({
            "index": 0x01ce, "items": nodes.len(), "nodes": nodes, "states": [],
            "entries": [0], "remote": [], "sync_vars": [], "containers": {},
            "unknown_nodes": [], "fields": {},
            "header": {
                "m_nodesBitCount": 4, "m_statesBitCount": 4,
                "m_remoteSyncNodesBitCount": 4, "m_syncVarsBitCount": 4
            }
        });
        let graphs = GraphStore::from_json(
            &serde_json::to_vec(&json!({
                "version": 1, "graphs": [graph], "bodies": {}, "missing": []
            }))
            .unwrap(),
        )
        .unwrap();
        let extra = [GraphBinding {
            graph: Some(0x01ce),
            sync: Some(vec![json!({
                "$": "STUStatescriptSchemaEntry",
                "m_0D09D2D9": "0x0D800000000003F8",
                "m_value": {"$": "STUConfigVarInt", "m_value": 1, "m_EE729DCB": 0},
            })]),
            flags: 0,
        }];
        let world = MapEntityWorld::start(
            Arc::new(graphs),
            0,
            0.0,
            Unknowns::Refuse,
            [Placement {
                entity: ENTITY,
                definition: 0,
                position: [0.0; 3],
                overrides: &[],
                extra: &extra,
            }],
            None,
            1,
        );
        let placed = world.entity(ENTITY).unwrap();
        let snapshot = placed.snapshot().unwrap();
        assert!(placed.complete(), "{:?}", snapshot.diagnostics);
        assert_eq!(placed.team(), Some(1));
        assert_eq!(
            snapshot.variables.get(&1),
            Some(&Value::Entity(GAME_MODE_ENTITY))
        );
    }
}
