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
//!   one), `m_DD792FF0` (1, 2 or 3), the bools `m_79758CE9`,
//!   `m_FF9E82FA`, `m_48BED081` (false in every use), `m_B12BFE8B` (quick
//!   melee's TrackTargets output `#23530`), `m_C0139FA6`, `m_724CBA57`
//!   (the owner where set), `m_E33EFEE1` and `m_10399AE7` (outputs) and the
//!   other hashed fields of `m_modifyHealth`. Each use records an
//!   assumption.
//! - `m_8B707686` (u8, state `+0x14d`) is the retail server's own flag;
//!   the client never reads it. ModifyHealth has no runtime of its own on
//!   the client (state record #160 is the base state's, vtable
//!   `0x7FF78B6BC800`: begin, end, update and event are `ret 0`), and the
//!   only client code that names the hash is the class's descriptor
//!   builder (`0x7FF78A490AC0`) and its default-value test
//!   (`0x7FF78A492D40`: `cmp byte [rcx], 1`), so it defaults to 1
//!   (`tools/who_reads.py 826613B7 m_8B707686`, 2026-11-20). In
//!   `game_graphs_174.json.gz` 94 of 159 uses keep the default and 65 author
//!   0. Neither value goes
//!   with one kind of change (by node): 0 on 16 of the 22 heals over time
//!   (the spawn room `0C8E` node 30, the payload `1225` node 11, the
//!   Biotic Field `017B` node 11) and on 33 of the 89 single blows (quick
//!   melee `0043` node 10, the 9999 kill volumes `096B`/`09BB`); 1 on 26
//!   of the 27 single heals (the health pack `0046` node 0, the respawn
//!   heal `0033` node 8) and the 1000000 suicide `0033` node 49. The
//!   server's own health code ([`World::modify_health`])
//!   applies every change alike, so the flag is handed on as is and records
//!   nothing. `docs/dev/loose-ends.md` has the entry.
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
    /// The entity that did it, when it isn't the instigator
    /// (`m_7B93233C`: the Biotic Orbs' auras `1101`/`1102` name the orb,
    /// their `#40`): it takes the healing done or damage dealt too, as its
    /// instigator does (`crate::ultimate::dealt_message`).
    pub source: Option<u32>,
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
const UNREAD: &[&str] = &["m_B12BFE8B"];
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
        let targets = self.listed_entities(key, node, "m_FA3ECC43");
        self.apply_health(key, node, &targets, share);
        Ok(())
    }

    /// A blow on each target not struck yet in this activation.
    fn strike_new_targets(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let now = self.machine.now_ms;
        let targets = self.listed_entities(key, node, "m_FA3ECC43");
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

    /// The bodies the node's field `name` names now (an entity or an
    /// array), each once.
    pub(super) fn listed_entities(&mut self, key: StateKey, node: &Node, name: &str) -> Vec<u32> {
        let config = field(&node.fields, name);
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
        // An entity, or the first of an array: an aura's graph takes its
        // instigator as a PlayScript parameter, an array (Biotic Grenade's
        // `0C72` and `0B99` st0 read `#40`, which `0C71` passes). Read as
        // anything else it fell back to the body, so the enemy aura's 60
        // came from the bot itself, which then took it as an ally's heal
        // of nothing.
        let instigator = match field(change, "m_instigator") {
            Json::Null => None,
            config => self
                .evaluate(id, node, config, None)
                .and_then(|value| expr::to_entity(&value)),
        }
        .unwrap_or(self.machine.snapshot.entity);
        let source = match field(change, "m_7B93233C") {
            Json::Null => None,
            config => self
                .evaluate(id, node, config, None)
                .and_then(|value| expr::to_entity(&value)),
        }
        .filter(|&source| expr::valid_entity(source) && source != instigator);
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
                source,
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
