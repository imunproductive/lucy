//! Movements a body's own statescript starts: a dash along the movement
//! input (Tracer's Blink, `STU_A807207A` in graph `0024`) and a rewind along
//! the body's recent path (Recall's `STU_41B3FC2B`, graph `001A`).
//!
//! No Python reference modelled either. The facts used are from
//! `docs/notes/ow1-preservation.md` (Blink is client-predicted with collision:
//! it stops at walls and lands on the floor; Recall goes back to where the
//! hero was 3 s earlier) and the public rules (Blink 7.5 m along the movement
//! direction; Recall rewinds 3 s of movement in 0.9 s). Assumptions, which the
//! client hasn't confirmed: the dash keeps to the ground the way walking does
//! (no gravity while it lasts), ends early when a frame makes less than half
//! its progress, and gives back the velocity the body had before it; the
//! rewind follows the recorded feet positions without collision (they were
//! all reachable) at an even pace and ends at rest.

use std::collections::VecDeque;

use super::{Mover, STEP, snap};
use crate::{
    collision::index::TriangleIndex, contact::hull::HullContactError, mover::kinematics::Ground,
};

/// Recorded feet positions, one per posed frame: a little over 4 s of 16 ms
/// frames, more than Recall's 3 s.
pub(super) const HISTORY: usize = 256;

pub(super) enum Motion {
    Dash(Dash),
    Rewind(Rewind),
}

pub(super) struct Dash {
    /// Unit horizontal direction, `[x, z]`.
    direction: [f64; 2],
    remaining: f64,
    speed: f64,
    /// The velocity the body had when the dash began, given back at its end.
    resume: [f64; 3],
}

pub(super) struct Rewind {
    /// Feet positions from the start of the rewind back to its end.
    path: Vec<[f64; 3]>,
    elapsed: f64,
    duration: f64,
}

impl Mover {
    /// Start a dash of `distance` metres over `seconds` along the last pose's
    /// movement input, or straight ahead without input, horizontally. It runs
    /// in the following poses and replaces a dash already running.
    pub fn dash(&mut self, distance: f64, seconds: f64) {
        if !(distance > 0.0 && seconds > 0.0) || !distance.is_finite() || !seconds.is_finite() {
            return;
        }
        let angle = f64::from(self.pose.yaw) * std::f64::consts::TAU / 65536.0;
        let [fx, fz] = [angle.sin(), angle.cos()];
        let [right, forward] = self
            .pose
            .throttles
            .map(|throttle| f64::from(throttle) / 127.0);
        let (x, z) = (right * -fz + forward * fx, right * fx + forward * fz);
        let length = x.hypot(z);
        let direction = if length > 0.0 {
            [x / length, z / length]
        } else {
            [fx, fz]
        };
        let resume = match &self.motion {
            Some(Motion::Dash(dash)) => dash.resume,
            _ => self.pose.velocity,
        };
        self.motion = Some(Motion::Dash(Dash {
            direction,
            remaining: distance,
            speed: distance / seconds,
            resume,
        }));
    }

    /// Start going back along the recorded path to where the feet were
    /// `seconds` ago (as far as the record goes), taking `duration` seconds
    /// of `dt`-long poses.
    pub fn rewind(&mut self, seconds: f64, duration: f64, dt: f64) {
        if !(seconds > 0.0 && duration > 0.0 && dt > 0.0) || !seconds.is_finite() {
            return;
        }
        let frames = (seconds / dt).round() as usize;
        let mut path = vec![self.pose.position];
        path.extend(self.history.iter().rev().skip(1).take(frames).copied());
        self.motion = Some(Motion::Rewind(Rewind {
            path,
            elapsed: 0.0,
            duration,
        }));
    }

    /// The rewind's state ended. Finished, the body is at the end of the
    /// path even when the state's clock ran out a pose before the mover's;
    /// aborted, it stays where it got to.
    pub fn end_rewind(&mut self, finished: bool) {
        let Some(Motion::Rewind(rewind)) = self.motion.take() else {
            return;
        };
        if finished {
            let feet = *rewind.path.last().expect("a rewind path has its start");
            let lift = f64::from(self.lift());
            self.pose.position = feet;
            self.body
                .teleport([feet[0] as f32, (feet[1] + lift) as f32, feet[2] as f32]);
            self.history.clear();
            self.history.push_back(feet);
        }
        self.pose.velocity = [0.0; 3];
    }

    /// Put the feet at `position` at rest, facing `yaw` when given: a
    /// graph's teleport (`STU_CFCE55DC`, the Workshop's Teleport). It ends
    /// any dash or rewind and starts the recorded path over. The
    /// discontinuity counter goes up, so the owner's client takes the next
    /// correction as it is instead of replaying its commands on it
    /// (`correction` module notes).
    pub fn teleport(&mut self, position: [f64; 3], yaw: Option<i32>) {
        let feet = position.map(snap);
        let lift = f64::from(self.lift());
        self.motion = None;
        self.pose.position = feet;
        self.body
            .teleport([feet[0] as f32, (feet[1] + lift) as f32, feet[2] as f32]);
        self.pose.velocity = [0.0; 3];
        self.posture.fall = 0.0;
        if let Some(yaw) = yaw {
            self.pose.yaw = yaw;
        }
        self.pose.discontinuity = self.pose.discontinuity.wrapping_add(1);
        self.history.clear();
        self.history.push_back(feet);
    }

    /// A dash or a rewind is running.
    pub fn authored_motion(&self) -> bool {
        self.motion.is_some()
    }

    pub(super) fn rewinding(&self) -> bool {
        matches!(self.motion, Some(Motion::Rewind(_)))
    }

    /// Keep the pose's feet for a later rewind.
    pub(super) fn record_history(&mut self) {
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(self.pose.position);
    }

    /// Run this pose's part of the motion; false when none runs.
    pub(super) fn step_motion(
        &mut self,
        rays: &TriangleIndex,
        normal: [f64; 3],
        dt: f64,
    ) -> Result<bool, HullContactError> {
        match self.motion.take() {
            None => Ok(false),
            Some(Motion::Dash(mut dash)) => {
                let wanted = (dash.speed * dt).min(dash.remaining);
                let [x, z] = dash.direction;
                let start = self.pose.position;
                let grounded = self.posture.ground() == Ground::Standing;
                self.move_body(
                    rays,
                    [x * wanted / dt, 0.0, z * wanted / dt],
                    normal,
                    grounded,
                    dt,
                )?;
                let moved =
                    (self.pose.position[0] - start[0]).hypot(self.pose.position[2] - start[2]);
                dash.remaining -= wanted;
                if moved < wanted / 2.0 || dash.remaining <= STEP {
                    self.pose.velocity = dash.resume;
                } else {
                    self.motion = Some(Motion::Dash(dash));
                }
                Ok(true)
            }
            Some(Motion::Rewind(mut rewind)) => {
                rewind.elapsed += dt;
                let fraction = (rewind.elapsed / rewind.duration).min(1.0);
                let at = fraction * (rewind.path.len() - 1) as f64;
                let (index, part) = (at.floor() as usize, at.fract());
                let from = rewind.path[index];
                let to = rewind.path[(index + 1).min(rewind.path.len() - 1)];
                let feet: [f64; 3] =
                    std::array::from_fn(|axis| snap(from[axis] + (to[axis] - from[axis]) * part));
                let lift = f64::from(self.lift());
                self.pose.position = feet;
                self.body
                    .teleport([feet[0] as f32, (feet[1] + lift) as f32, feet[2] as f32]);
                self.pose.velocity = [0.0; 3];
                self.posture.fall = 0.0;
                if fraction < 1.0 {
                    self.motion = Some(Motion::Rewind(rewind));
                } else {
                    // The path is spent: the next rewind starts from here.
                    self.history.clear();
                }
                Ok(true)
            }
        }
    }
}

/// Recorded positions for [`Mover::rewind`], newest last.
pub(super) type History = VecDeque<[f64; 3]>;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        contact::round::{RoundShape, Sphere},
        contact::world::{ContactWorld, StaticGeometry, StaticShape},
        mover::settings::MoverSettings,
    };
    use orpheus_wire::commands::Command;

    const DT: f64 = 0.016;

    fn mover_in(shapes: Vec<StaticShape>) -> Mover {
        Mover::new(
            MoverSettings::default(),
            Arc::new(ContactWorld::new(shapes).unwrap()),
            [0.0; 3],
            0,
            0x18,
        )
        .unwrap()
    }

    fn command(frame: u32, right: i8, forward: i8) -> Command {
        Command {
            frame,
            right,
            forward,
            ..Command::default()
        }
    }

    fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
        (a[0] - b[0]).hypot(a[2] - b[2])
    }

    #[test]
    fn a_dash_covers_its_distance_along_the_input_in_its_time() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        // Strafing right with yaw 0: forward is +z, right is -x.
        mover
            .step_pose_to(&rays, command(1, 127, 0), DT, None)
            .unwrap();
        let start = mover.pose().position;
        mover.dash(7.5, 0.1);
        let mut frames = 0;
        while mover.authored_motion() {
            frames += 1;
            mover
                .step_pose_to(&rays, command(1 + frames, 0, 0), DT, None)
                .unwrap();
        }
        // 0.1 s of 16 ms poses: six whole frames and the last 0.3 m.
        assert_eq!(frames, 7);
        let end = mover.pose().position;
        assert!((horizontal(start, end) - 7.5).abs() < 0.01, "{end:?}");
        assert!(end[0] < -7.4 && end[2].abs() < 0.01);
        // No gravity while dashing: the feet are still where they started.
        assert!((end[1] - start[1]).abs() < 0.01);
    }

    #[test]
    fn a_dash_without_input_goes_ahead_and_stops_at_a_wall() {
        let rays = TriangleIndex::new([]).unwrap();
        let wall = StaticGeometry::Round {
            shape: RoundShape::Sphere(Sphere {
                center: [0.0, 1.0, 3.0],
                radius: 1.0,
            }),
            mask: 0x18,
        };
        let mut mover = mover_in(vec![StaticShape::new(wall, None).unwrap()]);
        mover
            .step_pose_to(&rays, command(1, 0, 0), DT, None)
            .unwrap();
        mover.dash(7.5, 0.1);
        let mut frame = 1;
        while mover.authored_motion() {
            frame += 1;
            mover
                .step_pose_to(&rays, command(frame, 0, 0), DT, None)
                .unwrap();
        }
        let end = mover.pose().position;
        assert!(end[2] > 1.0 && end[2] < 2.0, "{end:?}");
    }

    #[test]
    fn a_rewind_goes_back_along_the_path_to_where_the_body_was() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        let mut positions = Vec::new();
        for frame in 1..=300 {
            mover
                .step_pose_to(&rays, command(frame, 0, 127), DT, None)
                .unwrap();
            positions.push(mover.pose().position);
        }
        let now = *positions.last().unwrap();
        // 3 s of 16 ms frames is 187.5: the record goes 188 poses back.
        let then = positions[positions.len() - 1 - 188];
        mover.rewind(3.0, 0.9, DT);
        let mut frames = 0;
        let mut last = now;
        while mover.authored_motion() {
            frames += 1;
            mover
                .step_pose_to(&rays, command(300 + frames, 0, 127), DT, None)
                .unwrap();
            let at = mover.pose().position;
            // Monotonically back along z.
            assert!(at[2] <= last[2]);
            last = at;
        }
        assert_eq!(frames, 57);
        assert_eq!(mover.pose().position, then);
        assert_eq!(mover.pose().velocity, [0.0; 3]);
    }

    #[test]
    fn an_aborted_rewind_stops_where_it_got_to_and_a_finished_one_at_its_end() {
        let rays = TriangleIndex::new([]).unwrap();
        let walk = |mover: &mut Mover| {
            let mut positions = Vec::new();
            for frame in 1..=100 {
                mover
                    .step_pose_to(&rays, command(frame, 0, 127), DT, None)
                    .unwrap();
                positions.push(mover.pose().position);
            }
            positions
        };
        let rewind_for = |mover: &mut Mover, frames: u32| {
            mover.rewind(3.0, 0.9, DT);
            for frame in 101..=100 + frames {
                mover
                    .step_pose_to(&rays, command(frame, 0, 0), DT, None)
                    .unwrap();
            }
        };
        let mut aborted = mover_in(Vec::new());
        let now = *walk(&mut aborted).last().unwrap();
        rewind_for(&mut aborted, 10);
        aborted.end_rewind(false);
        assert!(!aborted.authored_motion());
        let at = aborted.pose().position;
        assert!(at[2] < now[2] && at[2] > 0.0);

        // The record holds only 100 poses: the rewind ends at the first.
        let mut finished = mover_in(Vec::new());
        let first = walk(&mut finished)[0];
        rewind_for(&mut finished, 56);
        assert!(finished.authored_motion());
        finished.end_rewind(true);
        assert!(!finished.authored_motion());
        assert_eq!(finished.pose().position, first);
    }
}
