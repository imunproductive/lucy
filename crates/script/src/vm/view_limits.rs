//! The view limits `STU_9935FBC8` (Doomfist's slam `0B27` st33 and punch
//! `0D06` st45, 37 nodes in 26 graphs): while on, the owner's view stays
//! within its yaw and pitch ranges around a facing.
//!
//! Client (image base 0x7FF788F10000): the begin `0x7FF789B98180` links
//! the output `m_4BE66EFF` and puts its block at "no limit" (no entity,
//! 0, 0, −174532.9, −174532.9, 174532.9, 174532.9, 0), then its helper
//! `0x7FF789BBC810` (begin 0, end 1, update 2) evaluates, in degrees,
//! the facing `m_A53161C8` (yaw) and `m_AB37A892` (pitch), unset 0, and
//! the ranges `m_D6119DFB`/`m_C33684DE` (least yaw/pitch, unset
//! −9999999) and `m_83221709`/`m_8FA7FF85` (greatest, unset 9999999),
//! each × 0.017453292 into the block (`+0x1EC`..`+0x200`), and the
//! entity the body stands on and its yaw (`+0x1E8`, `+0x204`), which keep
//! the yaw relative to that entity. `m_93CDFD2F` 1 evaluates at the begin
//! only; otherwise each update does. The block goes only in the owner's
//! full frames (`codecs.rs` `project_linked_target`); the server's bodies
//! stand on no entity (no entity, yaw 0).

use super::*;

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// Evaluate the limits again (begin, or a change of what they read).
    pub(super) fn refresh_view_limits(&mut self, key: StateKey) {
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("view limits state");
        let once = self
            .evaluate_quietly(id, node, field(&node.fields, "m_93CDFD2F"))
            .is_some_and(|value| expr::to_int(&value, 0) == 1);
        let watcher = (!once).then_some(key);
        let mut radians = [0; 6];
        for (slot, (name, unset)) in radians.iter_mut().zip([
            ("m_A53161C8", 0.0),
            ("m_AB37A892", 0.0),
            ("m_D6119DFB", -9_999_999.0),
            ("m_C33684DE", -9_999_999.0),
            ("m_83221709", 9_999_999.0),
            ("m_8FA7FF85", 9_999_999.0),
        ]) {
            let config = field(&node.fields, name);
            let degrees = if config.is_null() {
                unset
            } else {
                self.evaluate(id, node, config, watcher)
                    .map_or(unset, |value| expr::to_float(&value) as f32)
            };
            *slot = (degrees * 0.017_453_292_f32).to_bits();
        }
        if let Some(state) = self.state_mut(key) {
            state.kind = StateKind::ViewLimits { radians };
        }
    }
}
