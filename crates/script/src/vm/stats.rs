//! The graphs' stats: TrackStat (`STU_CEBDAF17`, 386 nodes in 164 graphs)
//! and TrackStatTime (`STU_441B50E4`, 18 states). Both are server-only: no
//! registration of their own, so the client runs the base action's run
//! (`0x7FF78AA7EBA0`) and the base state; the retail server's stat table
//! took what they name ([`World::track_stat`]).
//!
//! What they count (the stats' `062` assets, `refs/reports/RustInsteadOfGraphs.md`
//! "TrackStat"): hero and ability stats ("Helix Rocket Kills", "Melee
//! Final Blows", "Damage Blocked", "Enemies Slept", "Multikills", "Time
//! Alive"), each beside its "- Most in Game" stat. None names a stat of the
//! Tab board's player list (Eliminations `0025`, Hero Damage Done `04B8`,
//! Healing Done `01D1`, Deaths `0029`, Objective Kills `0326` and Time
//! `0327`, nor their base stats `001F`, `04B7`, `01C1`, `002A`, `031C`,
//! `031D`), Final Blows `002B` or Weapon Accuracy's shots: those were the
//! retail server's own counting.

use super::*;

impl<W: World> Vm<'_, '_, '_, W> {
    /// TrackStat: `m_amount` to each stat for each target, now. An amount
    /// or a target that can't be evaluated counts nothing; nothing here
    /// can fail.
    pub(super) fn track_stat(&mut self, id: u16, node: &Node) {
        let stats = node_stats(node);
        if stats.is_empty() {
            return;
        }
        let amount = match self.evaluate_quietly(id, node, field(&node.fields, "m_amount")) {
            Some(value @ (Value::Integer(_) | Value::Float(_) | Value::Bool(_))) => {
                expr::to_float(&value)
            }
            _ => return,
        };
        self.credit_stats(id, node, stats, amount);
    }

    /// TrackStatTime ended: the seconds since it began go to its stats.
    pub(super) fn end_stat_time(&mut self, key: StateKey, node: &Node) {
        let Some(started) = self.state(key).map(|state| state.start_ms) else {
            return;
        };
        let stats = node_stats(node);
        let elapsed = self.machine.now_ms.saturating_sub(started);
        if stats.is_empty() || elapsed == 0 {
            return;
        }
        self.credit_stats(key.instance, node, stats, elapsed as f64 / 1000.0);
    }

    fn credit_stats(&mut self, id: u16, node: &Node, stats: Vec<u64>, amount: f64) {
        if amount == 0.0 {
            return;
        }
        let targets = self.voice_targets(id, node);
        if targets.is_empty() {
            return;
        }
        let site = self.site(id, node);
        self.world.track_stat(
            site,
            TrackedStat {
                stats,
                amount,
                targets,
            },
        );
    }
}

/// `m_stat`, then `m_stats`, in order (a GUID of another type, as `0636`
/// rid43's second, left out).
fn node_stats(node: &Node) -> Vec<u64> {
    std::iter::once(field(&node.fields, "m_stat"))
        .chain(array_field(&node.fields, "m_stats"))
        .filter_map(asset_guid)
        .filter(|&stat| stat >> 48 == STAT_TYPE)
        .collect()
}

/// The GUID type bits of a stat (`062`), as GUIDs carry them.
const STAT_TYPE: u64 = 0x0860;
