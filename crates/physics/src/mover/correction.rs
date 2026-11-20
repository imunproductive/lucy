//! Frame-aligned owner movement corrections and the command replay window.
//!
//! Follows `ow174/game/correction.py`. A correction describes the state after
//! its command frame. The packet can carry it once that frame has passed and
//! the owner has acknowledged this body's create.
//!
//! Client research: the owner's client keeps the state it predicted for each
//! of 64 command frames (mover +0x1580). A ch3 movement record on the body it
//! possesses goes to its player's component 59 (0x7FF7896DA7E7 ->
//! 0x7FF7896DA8AC): when comp59 +0x3E0 <= the record's frame, the state is
//! copied to comp59 +0x70 and +0x3E0 = frame + 1. On its next tick
//! (0x7FF78970F660) the client takes its predicted state of that frame
//! (history vt+0x60 / vt+0x58, frame checked at 0x7FF78970FA0D) and compares
//! with 0x7FF789B73E00 in mode 2: every field exactly, all but +384. Equal:
//! only bookkeeping (0x7FF789ACE870 drops mods that ended before it), +0x3E0 =
//! 0. Different or no prediction: comp59 +0x60 = 1; if the body's +12
//! discontinuity counter equals the server's, it takes the server's state and
//! replays the commands after F from its send window (comp59 +0xAF0, at most
//! 32 frames), else it takes the state without replay (0x7FF789710390,
//! 0x7FF78970FD80, 0x7FF789B7DF20). The record's frame is the packet frame
//! minus an unsigned count (0x7FF789B76DFE). The client jumps its command
//! frame to a passing packet frame plus a lead of at most 16
//! (0x7FF7896DAF7A..0x7FF7896DB296). It keeps only commands from ack - 1 on
//! (0x7FF7896DAF4F -> 0x7FF789B3D6E0); a frame it no longer has runs with its
//! oldest command (0x7FF789B3D586). The spring offsets (+44, +48) are not
//! sent: the decoded state has them at 0 (0x7FF789B75EC0) and the body takes
//! them so (0x7FF789B4AC10). Retail sent the local body a ch3 record every
//! frame (OW2 traffic).

use std::collections::VecDeque;

use orpheus_wire::entity::{EntityUpdate, Movement};

const HISTORY: usize = 64;
// The client retains input from ACK - 1. To replay after F it needs F + 1.
const ACK_LEAD: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BodyLifetime {
    entity: u32,
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Correction {
    body: BodyLifetime,
    frame: u32,
    pose: Movement,
}

impl Correction {
    pub fn frame(&self) -> u32 {
        self.frame
    }

    pub fn update(&self) -> EntityUpdate {
        EntityUpdate {
            entity: self.body.entity,
            movement: Some(self.pose),
            ..EntityUpdate::default()
        }
    }
}

/// The frames whose poses go to the owner.
#[derive(Clone, Copy, Debug)]
enum Coverage {
    /// Every frame: a hero whose movement the server's mover follows.
    Every,
    /// The frames of the last window opened, `first..=through`; none
    /// before the first ([`CorrectionHistory::open`]).
    Window(Option<(u32, u32)>),
}

/// State for a body whose mover and statescript are eligible for correction.
/// Renew the generation when replacing the mover or reusing the entity ID.
pub struct CorrectionHistory {
    body: BodyLifetime,
    coverage: Coverage,
    poses: VecDeque<Movement>,
    corrected: Option<u32>,
}

impl CorrectionHistory {
    /// A history whose every pose goes to the owner.
    pub fn new(entity: u32, generation: u64) -> Self {
        Self::with(entity, generation, Coverage::Every)
    }

    /// A history whose poses go to the owner only in the windows
    /// [`open`](Self::open)ed.
    pub fn windowed(entity: u32, generation: u64) -> Self {
        Self::with(entity, generation, Coverage::Window(None))
    }

    fn with(entity: u32, generation: u64, coverage: Coverage) -> Self {
        Self {
            body: BodyLifetime { entity, generation },
            coverage,
            poses: VecDeque::with_capacity(HISTORY),
            corrected: None,
        }
    }

    /// Send the poses of frames `frame..=through` too (a windowed history;
    /// one with every pose ignores it). A window that is still open, or
    /// ends just before `frame`, grows; another one replaces it.
    pub fn open(&mut self, frame: u32, through: u32) {
        if let Coverage::Window(window) = &mut self.coverage {
            *window = Some(match *window {
                Some((first, last)) if frame <= last.saturating_add(1) => {
                    (first, last.max(through))
                }
                _ => (frame, through),
            });
        }
    }

    fn covers(&self, frame: u32) -> bool {
        match self.coverage {
            Coverage::Every => true,
            Coverage::Window(window) => {
                window.is_some_and(|(first, through)| (first..=through).contains(&frame))
            }
        }
    }

    /// Whether the owner gets no poses now: a windowed history whose
    /// window lies behind its newest pose (or that has none).
    fn closed(&self) -> bool {
        let Coverage::Window(window) = self.coverage else {
            return false;
        };
        let newest = self.poses.back().and_then(|pose| pose.frame);
        window.is_none_or(|(_, through)| newest.is_some_and(|newest| newest > through))
    }

    /// Save each simulated pose, including recovered poses in a command gap.
    /// Only increasing, explicit command frames belong to this lifetime.
    pub fn record(&mut self, pose: Movement) -> bool {
        let Some(frame) = pose.frame else {
            return false;
        };
        if self
            .poses
            .back()
            .and_then(|pose| pose.frame)
            .is_some_and(|last| frame <= last)
        {
            return false;
        }
        if self.poses.len() == HISTORY {
            self.poses.pop_front();
        }
        self.poses.push_back(pose);
        true
    }

    pub fn corrected(&self) -> Option<u32> {
        self.corrected
    }

    fn snapshot_for(&self, tick: u32) -> Option<(u32, Movement)> {
        self.poses
            .iter()
            .rev()
            .find_map(|&pose| {
                let frame = pose.frame?;
                (frame <= tick).then_some((frame, pose))
            })
            .filter(|&(frame, _)| {
                self.covers(frame) && self.corrected.is_none_or(|sent| frame > sent)
            })
    }

    pub fn candidate(&self, tick: u32, create_arrived: bool) -> Option<Correction> {
        if !create_arrived {
            return None;
        }
        let (frame, pose) = self.snapshot_for(tick)?;
        Some(Correction {
            body: self.body,
            frame,
            pose,
        })
    }

    /// Use the correction only if this exact candidate is in the packet under
    /// consideration. Until the first send, a usable pose already caps the ACK
    /// even when the body create or packet budget holds that pose back. A
    /// closed window caps nothing.
    pub fn command_ack(
        &self,
        tick: u32,
        latest_input: Option<u32>,
        selected: Option<&Correction>,
    ) -> u32 {
        let newest = latest_input.unwrap_or(tick);
        if selected.is_none() && self.closed() {
            return newest;
        }
        // A pose sent in an earlier window says nothing of this one.
        let corrected = self.corrected.filter(|&frame| self.covers(frame));
        let replay_from = selected
            .filter(|candidate| candidate.body == self.body)
            .map(Correction::frame)
            .max(corrected)
            .or_else(|| self.snapshot_for(tick).map(|(frame, _)| frame))
            .unwrap_or(tick);
        newest.min(replay_from.saturating_add(ACK_LEAD))
    }

    /// Commit after the selected datagram has been sent successfully.
    pub fn sent(&mut self, correction: &Correction) -> bool {
        if correction.body != self.body
            || self.corrected.is_some_and(|sent| correction.frame <= sent)
        {
            return false;
        }
        self.corrected = Some(correction.frame);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_wire::entity::{NO_INPUT, movement_record};
    use sha2::{Digest, Sha256};

    const ENTITY: u32 = 0xa000_0101;

    fn pose(frame: u32) -> Movement {
        Movement {
            position: [f64::from(frame) / 1024.0, 1.0, -2.0],
            frame: Some(frame),
            ..Movement::default()
        }
    }

    #[test]
    fn selects_one_newest_eligible_pose_after_create_ack() {
        let mut history = CorrectionHistory::new(ENTITY, 1);
        for frame in 7000..7010 {
            assert!(history.record(pose(frame)));
        }
        assert!(history.candidate(7005, false).is_none());
        assert!(history.candidate(6999, true).is_none());
        let first = history.candidate(7005, true).unwrap();
        assert_eq!(first.update().movement, Some(pose(7005)));
        assert!(history.sent(&first));
        assert!(!history.sent(&first));
        assert!(history.candidate(7005, true).is_none());
        let next = history.candidate(7006, true).unwrap();
        assert_eq!(next.frame(), 7006);
        assert!(history.sent(&next));
        let latest = history.candidate(7020, true).unwrap();
        assert_eq!(latest.frame(), 7009);
        assert!(history.sent(&latest));
        assert!(history.candidate(7021, true).is_none());
    }

    #[test]
    fn a_windowed_history_sends_only_the_poses_of_its_window() {
        let mut history = CorrectionHistory::windowed(ENTITY, 1);
        for frame in 100..110 {
            history.record(pose(frame));
        }
        // No window yet: no pose, and the acknowledgement runs free.
        assert!(history.candidate(109, true).is_none());
        assert_eq!(history.command_ack(109, Some(112), None), 112);
        history.open(110, 115);
        for frame in 110..113 {
            history.record(pose(frame));
        }
        let first = history.candidate(112, true).unwrap();
        assert_eq!(first.frame(), 112);
        assert_eq!(history.command_ack(112, Some(120), None), 114);
        assert!(history.sent(&first));
        for frame in 113..118 {
            history.record(pose(frame));
        }
        assert_eq!(history.candidate(115, true).unwrap().frame(), 115);
        // Past the window: nothing more, and the acknowledgement is free.
        assert!(history.candidate(117, true).is_none());
        assert_eq!(history.command_ack(117, Some(120), None), 120);
        // A later window caps it from its own poses, not the last one sent.
        history.open(200, 210);
        history.record(pose(200));
        assert_eq!(history.command_ack(200, Some(205), None), 202);
        assert_eq!(history.candidate(200, true).unwrap().frame(), 200);
    }

    #[test]
    fn preserves_the_python_correction_bytes_for_all_input_frame_encodings() {
        // `correction.movement(Snapshot(...))`, then `world.movement_record(..., 7010)`.
        for (input_frame, length, expected) in [
            (
                Some(NO_INPUT),
                448,
                "93062ff28740128476ab865a0845be78031fe954fe6643382270370eb6da1d42",
            ),
            (
                Some(7001),
                481,
                "608555899c859da761a3861c111bc78a572fc464e9432c659838b93a34220248",
            ),
            (
                None,
                429,
                "c906c01d81bc869fc93a6fbb87b64779fced1bcdb463b621050ac4aa90b2ce26",
            ),
        ] {
            let mut history = CorrectionHistory::new(ENTITY, 1);
            assert!(history.record(Movement {
                position: [3.125, 0.5, -3.75],
                velocity: [0.25, 0.75, -2.125],
                yaw: 12_000,
                pitch: -3000,
                flags: 0x0400_0002,
                air_ticks: 11,
                gravity: 0.875,
                throttles: [3, -127],
                input_frame,
                input_throttles: [3, -127],
                frame: Some(7007),
                crouch_frame: Some(6999),
                spring: [-13, 27],
                spring_speed: 0.25,
                fall: -0.125,
                discontinuity: 0,
                move_to: None,
                nav_path: None,
            }));
            let correction = history.candidate(7010, true).unwrap().update();
            assert_eq!(correction.entity, ENTITY);
            let record = movement_record(&correction.movement.unwrap(), Some(7010)).unwrap();
            assert_eq!(record.bit_len(), length);
            assert_eq!(format!("{:x}", Sha256::digest(record.as_bytes())), expected);
        }
    }

    #[test]
    fn retains_the_last_64_explicit_command_poses() {
        let mut history = CorrectionHistory::new(ENTITY, 1);
        assert!(!history.record(Movement::default()));
        for frame in 1000..1070 {
            assert!(history.record(pose(frame)));
        }
        assert!(!history.record(pose(1068)));
        assert!(!history.record(pose(1069)));
        assert!(history.candidate(1005, true).is_none());
        assert_eq!(history.candidate(1006, true).unwrap().frame(), 1006);
        assert_eq!(history.candidate(1080, true).unwrap().frame(), 1069);
    }

    #[test]
    fn caps_ack_while_waiting_for_the_first_pose_or_create() {
        let mut history = CorrectionHistory::new(ENTITY, 1);
        assert_eq!(history.command_ack(100, None, None), 100);
        history.record(pose(113));
        history.record(pose(179));
        assert!(history.candidate(140, false).is_none());
        assert_eq!(history.command_ack(107, Some(179), None), 109);
        assert_eq!(history.command_ack(140, Some(179), None), 115);
        let last = history.candidate(180, true).unwrap();
        assert_eq!(history.command_ack(180, Some(179), Some(&last)), 179);
        assert_eq!(history.corrected(), None);
    }

    #[test]
    fn stale_tokens_do_not_commit_or_change_the_ack() {
        let mut old = CorrectionHistory::new(ENTITY, 1);
        old.record(pose(125));
        let stale = old.candidate(130, true).unwrap();
        for (entity, generation) in [(ENTITY, 2), (ENTITY + 1, 1)] {
            let mut current = CorrectionHistory::new(entity, generation);
            current.record(pose(110));
            let valid = current.candidate(130, true).unwrap();
            assert!(current.sent(&valid));
            assert!(!current.sent(&stale));
            assert_eq!(current.corrected(), Some(110));
            assert_eq!(current.command_ack(130, Some(129), Some(&stale)), 112);
        }
    }
}
