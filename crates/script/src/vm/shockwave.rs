//! The shockwave `STUStatescriptStateShockwave` (`STU_01D67FA0`;
//! Doomfist's slam `0B27` st44): a wave that grows to its radius and then
//! finishes the state.
//!
//! Client (image base 0x7FF788F10000): the begin `0x7FF789BA2510` registers
//! the update and, unless `m_04F3240D` holds, sends the wave once
//! (`0x7FF789BBD0D0`, its hits). The update `0x7FF789BA5270`: the wave
//! takes `m_6A79BD98 / max(m_speed, 1e-5)` seconds to reach its radius, no
//! time when `m_3418CEA4` holds; while the seconds since the begin
//! (`0x7FF78AA81D10`, ms × 0.001) are fewer it sends the wave again, and
//! after it finishes the state (`0x7FF78AA812E0`), sending the wave once
//! more when `m_3418CEA4` holds. The update also runs in the begin's frame:
//! the owner's client never shows `0B27` st44 at the end of the frame it
//! begins in (df10 s1 `cp10_slam`, frame 6761). The server registers no
//! hits (its frames carry an empty hit list, `snapshot.rs`). The form
//! `m_04F3240D` turns on (the wave timed from the world's frame clock,
//! `+668`) is run as the timed one, recorded as an assumption, and so is a
//! host with flag `0x400` at `+792`, whose update never finishes it.

use super::*;

impl<W: World> Vm<'_, '_, '_, W> {
    pub(super) fn begin_shockwave(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        if !field(&node.fields, "m_04F3240D").is_null() {
            self.assume(
                self.site(id, node),
                "STUStatescriptStateShockwave m_04F3240D",
                "the wave timed from its begin",
            );
        }
        let instant = !field(&node.fields, "m_3418CEA4").is_null()
            && self
                .evaluate(id, node, field(&node.fields, "m_3418CEA4"), None)
                .is_some_and(|value| truthy(&value));
        let seconds = if instant {
            0.0
        } else {
            let mut config = |name| {
                self.evaluate(id, node, field(&node.fields, name), None)
                    .map_or(0.0, |value| expr::to_float(&value) as f32)
            };
            let radius = config("m_6A79BD98");
            radius / config("m_speed").max(0.000_01)
        };
        if let Some(state) = self.state_mut(key) {
            state.kind = StateKind::Shockwave {
                seconds: seconds.to_bits(),
            };
        }
        self.register_tick(key, self.machine.now_ms);
        self.update_shockwave(key)
    }

    /// The wave has reached its radius: the state finishes.
    pub(super) fn update_shockwave(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(state) = self.state(key) else {
            return Ok(());
        };
        let StateKind::Shockwave { seconds } = state.kind else {
            return Ok(());
        };
        let elapsed = self.machine.now_ms.saturating_sub(state.start_ms) as f32 * 0.001;
        if elapsed >= f32::from_bits(seconds) {
            self.post(key.instance, Event::Finish(key.raw))?;
        }
        Ok(())
    }
}
