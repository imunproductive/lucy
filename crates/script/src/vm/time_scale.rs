//! ScaleTime `STU_2F07848F` (StEC): a change of the game's speed from a
//! frame to a frame (the match end's slow motion, `181E` st0, played by
//! `180A`'s first card; `0CBB`, `0D0B`, `0EC2`).
//!
//! Client (1.74, state record #1691; `python tools/stu.py STU_2F07848F`):
//!
//! - Begin (slot 23 `0x7FF789B97310`): the frame it starts (`+0x1C8`) is
//!   the delay `m_17D12F6B` (an integer of frames, none below 0) after the
//!   world's frame (`0x7FF78AA81860`: `world+464`); the host's game puts
//!   the scale `m_E75DF5AC` on its list from that frame, reaching it
//!   `m_D8989B37` frames later (`181E`: 30 frames).
//! - End (slot 24 `0x7FF789B974C0`): the frame it stops (`+0x1CC`) is the
//!   delay after the world's frame then; the scale goes back over
//!   `m_89C63AB3` frames.
//! - The frames carry the active flag and the frame of that side
//!   (`Write_STU_2F07848F` 0x7FF789B97600).
//!
//! The server keeps both frames for the frames; its own simulation keeps
//! its speed (docs/dev/loose-ends.md).

use super::*;

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// The frame a ScaleTime's change starts or stops: its delay after now.
    pub(super) fn scale_time_frame(&mut self, key: StateKey, node: &Node) -> u32 {
        let delay = match field(&node.fields, "m_17D12F6B") {
            Json::Null => 0,
            config => self
                .evaluate(key.instance, node, config, Some(key))
                .map_or(0, |value| expr::to_int(&value, 0).max(0)),
        };
        let frame = self.machine.now_ms / FRAME_MS;
        u32::try_from(frame + delay as u64).unwrap_or(u32::MAX)
    }

    pub(super) fn begin_scale_time(&mut self, key: StateKey, node: &Node) {
        let on = self.scale_time_frame(key, node);
        if let Some(state) = self.state_mut(key)
            && let StateKind::ScaleTime { off, .. } = state.kind
        {
            state.kind = StateKind::ScaleTime { on, off };
        }
    }

    pub(super) fn end_scale_time(&mut self, key: StateKey, node: &Node) {
        let off = self.scale_time_frame(key, node);
        if let Some(state) = self.state_mut(key)
            && let StateKind::ScaleTime { on, .. } = state.kind
        {
            state.kind = StateKind::ScaleTime { on, off };
        }
    }
}
