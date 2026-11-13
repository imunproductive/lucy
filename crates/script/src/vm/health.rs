//! Health changes: `STUStatescriptStateModifyHealth`. While the state runs
//! it damages or heals the bodies its `m_FA3ECC43` names; the server's
//! native health code applies each change ([`World::modify_health`]), so the
//! damage message, the stats, deaths and respawns follow as for a shot.
//!
//! What the fields mean, read from the 127 uses in the hero and bot bundles
//! (`game_graphs_174.json.gz`, `game_graphs_bots_174.json.gz`):
//!
//! - `m_FA3ECC43`: the targets, an entity or an array of entities (quick
//!   melee `0043` st4 reads its TrackTargets' `#375`; `0033`'s and `0C8E`'s
//!   name the owner). Sure.
//! - `m_modifyHealth` (`STUStatescriptModifyHealth`): `m_amount` (always
//!   positive in the data), the damage tags `m_148F3152`, `m_instigator`
//!   (who dealt it; the owner when unset), `m_loadout` (the kill feed icon)
//!   and `m_277A4ECA`, an aura (`053`) the change applies to each body it
//!   reaches (Biotic Grenade's explosion names `0267`).
//! - `m_modifyHealth.m_93CDFD2F`: 1 damages, 0 heals. Strong guess: 0 on
//!   every heal that can be told by its graph (the spawn room `0C8E` st13,
//!   the payload `1225` st9, Moira's `1007` st12 and st18, D.Va's call mech
//!   `026A` st9), 1 on the damage (quick melee `0043` st4, Death Blossom
//!   `0044` st35, Moira's orb `1007` st20, the enemy spawn room `0C8E`
//!   st45).
//! - `m_7B2B3F5C`: 1 changes health over time, `m_amount` every
//!   `m_95F1FB0B` seconds (1 when unset), for as long as the state runs;
//!   0 changes it once for each body as it comes in. Strong guess: every
//!   use with `m_95F1FB0B` has `m_7B2B3F5C` 1 and the reverse; Death
//!   Blossom (170 a second), the spawn room (400 a second) and the payload
//!   heal (10 a second) are rates, quick melee is a blow. The Biotic Field's
//!   `017B` st6 spreads its heal over `#567`, the field's seconds.
//! - Not read: the knockback rings `m_knockbackRings` (quick melee has
//!   one), `m_8B707686`, `m_DD792FF0` (1, 2 or 3), the bools `m_79758CE9`,
//!   `m_FF9E82FA`, `m_48BED081` (false in every use), `m_B12BFE8B` (quick
//!   melee's TrackTargets output `#23530`), `m_C0139FA6`, `m_724CBA57`
//!   (the owner where set), `m_E33EFEE1` and `m_10399AE7` (outputs) and the
//!   other hashed fields of `m_modifyHealth`. Each use records an
//!   assumption.
//!
//! Assumed, as nothing in the data says: a body comes in once per
//! activation (a body the targets list again after it left is not struck
//! again); a rate applies each tick for the time since the last one, from
//! the state's begin; a world that applies no health change leaves the
//! state without effect, as before (recorded).

use super::{targets::is_set, *};
use crate::weapons::DamageTags;

/// A health change for the match to apply ([`World::modify_health`]).
#[derive(Clone, Debug, PartialEq)]
pub struct HealthChange {
    /// The ModifyHealth state's body, instance, graph and node.
    pub site: Site,
    /// Who dealt it (`m_instigator`): the kill feed's killer.
    pub instigator: u32,
    pub target: u32,
    /// How much; above 0.
    pub amount: f64,
    /// A heal, not damage (`m_93CDFD2F` 0).
    pub heal: bool,
    pub tags: DamageTags,
    pub loadout: Option<u64>,
    /// The aura it applies to the body it reaches (`m_277A4ECA`).
    pub aura: Option<u64>,
    /// The component's frame when it was dealt.
    pub frame: u32,
}

/// The fields the server doesn't read; each set one is an assumption.
const UNREAD: &[&str] = &["m_8B707686", "m_B12BFE8B"];
const UNREAD_CHANGE: &[&str] = &["m_knockbackRings", "m_C0139FA6", "m_E33EFEE1", "m_10399AE7"];

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn begin_modify_health(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let site = self.site(key.instance, node);
        for name in UNREAD {
            if is_set(field(&node.fields, name)) {
                self.assume(site, &format!("{} {name}", node.class), "not read");
            }
        }
        let change = field(&node.fields, "m_modifyHealth");
        for name in UNREAD_CHANGE {
            if is_set(field(change, name)) {
                self.assume(
                    site,
                    &format!("STUStatescriptModifyHealth {name}"),
                    "not read",
                );
            }
        }
        if over_time(node) {
            // The first share comes at the next tick.
            self.state_mut(key).expect("active ModifyHealth").kind = StateKind::ModifyHealth {
                last_ms: self.machine.now_ms,
            };
        } else {
            self.strike_new_targets(key, node)?;
        }
        self.register_tick(key, self.machine.now_ms);
        Ok(())
    }

    /// Each tick: a rate's share since the last tick on every target now, or
    /// a blow on the bodies that came in.
    pub(super) fn tick_modify_health(
        &mut self,
        key: StateKey,
        node: &Node,
        last_ms: u64,
    ) -> Result<(), InitFault> {
        if !over_time(node) {
            return self.strike_new_targets(key, node);
        }
        let now = self.machine.now_ms;
        self.state_mut(key).expect("active ModifyHealth").kind =
            StateKind::ModifyHealth { last_ms: now };
        let id = key.instance;
        let period = match field(&node.fields, "m_95F1FB0B") {
            Json::Null => 1.0,
            config => self
                .evaluate(id, node, config, None)
                .map_or(0.0, |value| expr::to_float(&value)),
        };
        if !(period.is_finite() && period > 0.0) {
            return Ok(());
        }
        let share = now.saturating_sub(last_ms) as f64 / 1000.0 / period;
        let targets = self.health_targets(key, node);
        self.apply_health(key, node, &targets, share);
        Ok(())
    }

    /// A blow on each target not struck yet in this activation.
    fn strike_new_targets(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let now = self.machine.now_ms;
        let targets = self.health_targets(key, node);
        let state = self.state_mut(key).expect("active ModifyHealth");
        let new: Vec<u32> = targets
            .into_iter()
            .filter(|&entity| !state.tracked.iter().any(|seen| seen.entity == entity))
            .collect();
        state
            .tracked
            .extend(new.iter().map(|&entity| TrackedTarget {
                entity,
                since_ms: now,
            }));
        self.apply_health(key, node, &new, 1.0);
        Ok(())
    }

    /// The bodies `m_FA3ECC43` names now, each once.
    fn health_targets(&mut self, key: StateKey, node: &Node) -> Vec<u32> {
        let config = field(&node.fields, "m_FA3ECC43");
        let mut targets = Vec::new();
        let mut add = |value: &Value| {
            if let Value::Entity(entity) = *value
                && expr::valid_entity(entity)
                && !targets.contains(&entity)
            {
                targets.push(entity);
            }
        };
        match self.evaluate(key.instance, node, config, None) {
            Some(Value::Array(items)) => items.iter().for_each(&mut add),
            Some(value) => add(&value),
            None => (),
        }
        targets
    }

    /// `scale` times the change's amount to each of `targets`.
    fn apply_health(&mut self, key: StateKey, node: &Node, targets: &[u32], scale: f64) {
        if targets.is_empty() {
            return;
        }
        let id = key.instance;
        let site = self.site(id, node);
        let change = field(&node.fields, "m_modifyHealth");
        let amount = self
            .evaluate(id, node, field(change, "m_amount"), None)
            .map_or(0.0, |value| expr::to_float(&value))
            * scale;
        if !(amount.is_finite() && amount > 0.0) {
            return;
        }
        let instigator = match field(change, "m_instigator") {
            Json::Null => Some(self.machine.snapshot.entity),
            config => self
                .evaluate(id, node, config, None)
                .and_then(|value| match value {
                    Value::Entity(entity) => Some(entity),
                    _ => None,
                }),
        }
        .unwrap_or(self.machine.snapshot.entity);
        let loadout = match self.evaluate_set(id, node, field(change, "m_loadout")) {
            Some(Value::Asset(guid)) => Some(guid),
            _ => None,
        };
        let aura = match self.evaluate_set(id, node, field(change, "m_277A4ECA")) {
            Some(Value::Asset(guid)) if guid != 0 => Some(guid),
            _ => None,
        };
        let tags = DamageTags::from_authored(change, |config| {
            self.evaluate(id, node, config, None)
                .ok_or(EvalError::MalformedConfig("damage tag"))
        });
        let heal = field(change, "m_93CDFD2F").as_i64() == Some(0);
        let mut taken = true;
        for &target in targets {
            taken &= self.world.modify_health(HealthChange {
                site,
                instigator,
                target,
                amount,
                heal,
                tags,
                loadout,
                aura,
                frame: self.machine.frame,
            });
        }
        if !taken {
            self.assume(
                site,
                &node.class,
                "no effect (the world applies no health change)",
            );
        }
    }

    fn evaluate_set(&mut self, id: u16, node: &Node, config: &Json) -> Option<Value> {
        if config.is_null() {
            return None;
        }
        self.evaluate(id, node, config, None)
    }
}

/// A rate, not a blow (`m_7B2B3F5C`).
fn over_time(node: &Node) -> bool {
    field(&node.fields, "m_7B2B3F5C").as_i64().unwrap_or(0) != 0
}
