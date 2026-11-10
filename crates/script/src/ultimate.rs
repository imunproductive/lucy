//! The ultimate's charge as the Python reference ran it
//! (`ow174/game/script/driver.py`): entity variable #342 is server data that no
//! graph adds to (the ultimate's graph sets it to 0 when it starts), and its
//! cost #338 comes from the entries. The server has no damage or healing that
//! would charge it yet, so the charge fills by itself over [`ULT_SECONDS`],
//! and not while the ultimate runs.

use serde_json::Value as Json;

use crate::{
    expr::{Scope, Variable, to_float},
    graph::GraphStore,
    timeline::FRAME_MS,
    vm::{Component, InitFault, StateKind, World},
    wire::Value,
};

pub const ULT_CHARGE: u32 = 342;
pub const ULT_COST: u32 = 338;
/// Seconds an empty ultimate takes to fill (0: never).
pub const ULT_SECONDS: f64 = 60.0;
const ABILITY_RUNNING: u8 = 1;

/// The abilities a body's ultimate charge waits for.
pub struct UltimateCharge {
    seconds: f64,
    /// Instance and raw state of each ability that starts on the charge.
    abilities: Vec<(u16, u16)>,
}

impl UltimateCharge {
    /// The abilities of the body's instances at spawn whose start condition
    /// reads the charge (`BodyScript._spawn`).
    pub fn new(component: &Component, graphs: &GraphStore, seconds: f64) -> Self {
        let abilities = component
            .snapshot()
            .into_iter()
            .flat_map(|snapshot| &snapshot.instances)
            .filter_map(|(&id, instance)| Some((id, graphs.graph(instance.graph)?)))
            .flat_map(|(id, graph)| {
                graph.nodes.iter().filter_map(move |node| {
                    (node.class == "STUStatescriptStateAbility"
                        && node.field("m_150F0D92").is_some_and(reads_charge))
                    .then_some((id, node.state?))
                })
            })
            .collect();
        Self { seconds, abilities }
    }

    /// One frame's charge (`BodyScript._charge_ult`): call it after every
    /// command frame the body runs.
    pub fn charge(
        &self,
        component: &mut Component,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        if self.seconds <= 0.0 || self.running(component) {
            return Ok(());
        }
        let read = |id| {
            component
                .resolved_variable(
                    0,
                    Variable {
                        scope: Scope::Entity,
                        id,
                    },
                )
                .map_or(0.0, |value| to_float(&value))
        };
        let (cost, charge) = (read(ULT_COST), read(ULT_CHARGE));
        if cost > 0.0 && charge < cost {
            let step = (cost * FRAME_MS as f64 / (1000.0 * self.seconds)) as f32;
            let next = f64::from((charge + f64::from(step)) as f32);
            set_ult_charge(component, cost.min(next), world)?;
        }
        Ok(())
    }

    fn running(&self, component: &Component) -> bool {
        let Some(snapshot) = component.snapshot() else {
            return false;
        };
        self.abilities.iter().any(|(id, raw)| {
            snapshot
                .instances
                .get(id)
                .and_then(|instance| instance.states.get(raw))
                .is_some_and(|state| {
                    matches!(state.kind, StateKind::Ability { flags } if flags & ABILITY_RUNNING != 0)
                })
        })
    }
}

/// Set the ultimate's charge (`BodyScript.set_ult_charge`): a GM command, or
/// damage and healing later.
pub fn set_ult_charge(
    component: &mut Component,
    points: f64,
    world: &mut impl World,
) -> Result<(), InitFault> {
    component.set_entity_variable(ULT_CHARGE, Value::Float(f64::from(points as f32)), world)
}

/// Whether a config reads entity variable #342 (`_reads` in driver.py).
fn reads_charge(config: &Json) -> bool {
    match config {
        Json::Object(fields)
            if fields.get("$").and_then(Json::as_str) == Some("STUConfigVarDynamic") =>
        {
            let entity = fields.get("m_60DB8F99");
            fields.get("m_identifier").and_then(Json::as_u64) == Some(u64::from(ULT_CHARGE))
                && (entity == Some(&Json::Bool(true)) || entity.and_then(Json::as_u64) == Some(1))
        }
        Json::Object(fields) => fields.values().any(reads_charge),
        Json::Array(items) => items.iter().any(reads_charge),
        _ => false,
    }
}
