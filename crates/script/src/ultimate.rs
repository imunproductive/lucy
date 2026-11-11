//! The ultimate's charge: entity variable #342 is server data that no graph
//! adds to (the ultimate's graph sets it to 0 when it starts), and its cost
//! #338 comes from the entries. The server adds to it as the retail server
//! did: [`PASSIVE_POINTS_PER_SECOND`] by itself, and a point for each point
//! of damage or healing the hero deals, each times the game ruleset's
//! ultimate generation settings. Neither is added while the ultimate runs.

use serde_json::Value as Json;

use crate::{
    expr::{Scope, Variable, to_float},
    graph::GraphStore,
    rulesets::Rulesets,
    timeline::FRAME_MS,
    vm::{Component, InitFault, StateKind, World},
    wire::Value,
};

pub const ULT_CHARGE: u32 = 342;
pub const ULT_COST: u32 = 338;
/// Points a hero's ultimate gains a second by itself. Assumption: the
/// install has no base rate (the retail server added the charge), so this
/// is the community's figure (docs/notes/game-rules.md, rule 153).
pub const PASSIVE_POINTS_PER_SECOND: f64 = 5.0;
/// The hero ruleset entries (`01C` indexes, schema `0A30…0007`, each 100% by
/// default): "Ultimate Generation", "Ultimate Generation - Combat" and
/// "Ultimate Generation - Passive".
const ULTIMATE_GENERATION: u32 = 0x253C;
const COMBAT_GENERATION: u32 = 0x32DE;
const PASSIVE_GENERATION: u32 = 0x3276;
const ABILITY_RUNNING: u8 = 1;

/// How fast an ultimate charges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargeRates {
    /// Points a second, by itself.
    pub passive: f64,
    /// Points for each point of damage or healing dealt.
    pub combat: f64,
}

impl ChargeRates {
    /// The rates the hero's and mode's ruleset defaults give: the passive
    /// base times "Ultimate Generation" and "Passive", and one point a point
    /// times "Ultimate Generation" and "Combat". A missing entry counts 100%.
    pub fn of(rulesets: &Rulesets, hero: u64, mode: u64) -> Self {
        let share = |key| {
            rulesets
                .default(key, hero, mode)
                .map_or(1.0, |value| to_float(&value))
        };
        let generation = share(ULTIMATE_GENERATION);
        Self {
            passive: PASSIVE_POINTS_PER_SECOND * generation * share(PASSIVE_GENERATION),
            combat: generation * share(COMBAT_GENERATION),
        }
    }
}

/// The abilities a body's ultimate charge waits for.
pub struct UltimateCharge {
    rates: ChargeRates,
    /// Instance and raw state of each ability that starts on the charge.
    abilities: Vec<(u16, u16)>,
}

impl UltimateCharge {
    /// The abilities of the body's instances at spawn whose start condition
    /// reads the charge (`BodyScript._spawn`).
    pub fn new(component: &Component, graphs: &GraphStore, rates: ChargeRates) -> Self {
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
        Self { rates, abilities }
    }

    /// One frame's passive charge (`BodyScript._charge_ult`): call it after
    /// every command frame the body runs.
    pub fn charge(
        &self,
        component: &mut Component,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        self.add(
            self.rates.passive * FRAME_MS as f64 / 1000.0,
            component,
            world,
        )
    }

    /// The charge for `points` of damage or healing the hero dealt.
    pub fn gain(
        &self,
        points: f64,
        component: &mut Component,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        self.add(self.rates.combat * points, component, world)
    }

    /// Add `points` up to the cost, in f32 steps, unless the ultimate runs.
    fn add(
        &self,
        points: f64,
        component: &mut Component,
        world: &mut impl World,
    ) -> Result<(), InitFault> {
        if points.is_nan() || points <= 0.0 || self.running(component) {
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
            let next = f64::from((charge + f64::from(points as f32)) as f32);
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

/// Set the ultimate's charge (`BodyScript.set_ult_charge`).
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
