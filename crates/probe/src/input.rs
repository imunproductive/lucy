//! Scripted input, frame by frame: what the probe writes into each input
//! sample of the client (D:/Work/Coding/refs/reports/ClientInput.md).
//!
//! A script is a list of steps. `run` holds buttons and throttles for a
//! number of input samples (one a rendered frame) or milliseconds; `press` and
//! `release` change the buttons held across steps; `action` puts an action
//! code into one sample; `look`, `turn` and `free` set the view. Steps without
//! a duration take no frame: `look` then `run` turn and move in the same frame.

use std::collections::VecDeque;

use serde_json::Value;

/// Command bits of the sample's button word (`+0x1050`), by name.
pub const BUTTONS: [(&str, u32); 10] = [
    ("fire", 0x1),
    ("fire2", 0x2),
    ("interact", 0x4),
    ("ability1", 0x8),
    ("ability2", 0x10),
    ("ultimate", 0x20),
    ("jump", 0x40),
    ("crouch", 0x80),
    ("reload", 0x400),
    ("melee", 0x800),
];

/// View angle units: a full turn is 65536.
pub const UNITS_PER_RADIAN: f64 = 65536.0 / std::f64::consts::TAU;

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Buttons and throttles for `frames` samples, or for `ms` milliseconds
    /// (at least one sample either way).
    Run {
        frames: u32,
        ms: u32,
        buttons: u32,
        forward: i8,
        right: i8,
    },
    Press(u32),
    Release(u32),
    Action(u8),
    /// Absolute view, 16-bit angles (yaw: forward = (sin, 0, cos); pitch +down).
    Look(i16, i16),
    /// Relative to the view when the step starts.
    Turn(i16, i16),
    /// Stop holding the view.
    Free,
}

impl Step {
    /// `{"op": "run", "frames": 60, "buttons": 64, "forward": 127}` and so on.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let op = value["op"].as_str().ok_or("a step needs an op")?;
        let int = |key: &str| value.get(key).and_then(Value::as_i64).unwrap_or(0);
        let angle = |key: &str| int(key).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        let throttle = |key: &str| int(key).clamp(-127, 127) as i8;
        Ok(match op {
            "run" => Self::Run {
                frames: int("frames").clamp(0, i64::from(u32::MAX)) as u32,
                ms: int("ms").clamp(0, i64::from(u32::MAX)) as u32,
                buttons: int("buttons") as u32,
                forward: throttle("forward"),
                right: throttle("right"),
            },
            "press" => Self::Press(int("buttons") as u32),
            "release" => Self::Release(int("buttons") as u32),
            "action" => Self::Action(
                u8::try_from(int("code"))
                    .ok()
                    .filter(|&code| code > 0)
                    .ok_or("action code 1-255")?,
            ),
            "look" => Self::Look(angle("yaw"), angle("pitch")),
            "turn" => Self::Turn(angle("yaw"), angle("pitch")),
            "free" => Self::Free,
            other => return Err(format!("unknown step op {other:?}")),
        })
    }
}

/// What one input sample gets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub buttons: u32,
    pub forward: i8,
    pub right: i8,
    pub action: u8,
    /// The view to turn to with the client's set-view (once, when it changes).
    pub turn_to: Option<(i16, i16)>,
    /// The view written into every sample while it is held.
    pub hold_view: Option<(i16, i16)>,
}

#[derive(Clone, Debug)]
struct Running {
    left_frames: u32,
    until_ms: Option<u64>,
    buttons: u32,
    forward: i8,
    right: i8,
}

/// The script and the input it holds.
#[derive(Clone, Debug, Default)]
pub struct Script {
    steps: VecDeque<Step>,
    running: Option<Running>,
    held: u32,
    view: Option<(i16, i16)>,
    turn_pending: bool,
    /// Samples the script has written since it was loaded.
    pub samples: u64,
}

impl Script {
    pub fn push(&mut self, steps: impl IntoIterator<Item = Step>) {
        self.steps.extend(steps);
    }

    /// Drop the steps and everything held.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn pending(&self) -> usize {
        self.steps.len() + usize::from(self.running.is_some())
    }

    pub fn active(&self) -> bool {
        self.pending() > 0 || self.held != 0 || self.view.is_some()
    }

    /// The input of the next sample at `now_ms`, with the client's current view.
    pub fn next(&mut self, now_ms: u64, current_view: (i16, i16)) -> Sample {
        let mut sample = Sample::default();
        while self.running.is_none() {
            let Some(step) = self.steps.pop_front() else {
                break;
            };
            match step {
                Step::Run {
                    frames,
                    ms,
                    buttons,
                    forward,
                    right,
                } => {
                    self.running = Some(Running {
                        left_frames: if ms > 0 { u32::MAX } else { frames.max(1) },
                        until_ms: (ms > 0).then(|| now_ms + u64::from(ms)),
                        buttons,
                        forward,
                        right,
                    });
                }
                Step::Press(bits) => self.held |= bits,
                Step::Release(bits) => self.held &= !bits,
                Step::Action(code) => sample.action = code,
                Step::Look(yaw, pitch) => self.turn((yaw, pitch)),
                Step::Turn(yaw, pitch) => {
                    let (from_yaw, from_pitch) = self.view.unwrap_or(current_view);
                    self.turn((from_yaw.wrapping_add(yaw), from_pitch.saturating_add(pitch)));
                }
                Step::Free => {
                    self.view = None;
                    self.turn_pending = false;
                }
            }
        }
        sample.buttons = self.held;
        if let Some(running) = &mut self.running {
            sample.buttons |= running.buttons;
            sample.forward = running.forward;
            sample.right = running.right;
            running.left_frames = running.left_frames.saturating_sub(1);
            let over = match running.until_ms {
                Some(until) => now_ms >= until,
                None => running.left_frames == 0,
            };
            if over {
                self.running = None;
            }
        }
        if std::mem::take(&mut self.turn_pending) {
            sample.turn_to = self.view;
        }
        sample.hold_view = self.view;
        self.samples += 1;
        sample
    }

    fn turn(&mut self, (yaw, pitch): (i16, i16)) {
        // The client's pitch limit is about ±89 degrees.
        self.view = Some((yaw, pitch.clamp(-16_000, 16_000)));
        self.turn_pending = true;
    }
}

/// The client's set-view takes a quaternion `(x, y, z, w)`: yaw about +y,
/// then pitch about the rotated +x (ClientInput.md "View").
pub fn view_quaternion(yaw: i16, pitch: i16) -> [f32; 4] {
    let yaw = f64::from(yaw) / UNITS_PER_RADIAN;
    let pitch = f64::from(pitch) / UNITS_PER_RADIAN;
    let (sy, cy) = (yaw / 2.0).sin_cos();
    let (sx, cx) = (pitch / 2.0).sin_cos();
    [
        (cy * sx) as f32,
        (cx * sy) as f32,
        (-sy * sx) as f32,
        (cy * cx) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(frames: u32, buttons: u32, forward: i8) -> Step {
        Step::Run {
            frames,
            ms: 0,
            buttons,
            forward,
            right: 0,
        }
    }

    #[test]
    fn steps_take_their_frames_and_instant_steps_take_none() {
        let mut script = Script::default();
        script.push([
            run(2, 0, 127),
            Step::Look(100, 0),
            Step::Press(0x40),
            run(1, 0x1, 127),
            Step::Release(0x40),
        ]);
        let samples: Vec<Sample> = (0..4).map(|_| script.next(0, (0, 0))).collect();
        assert_eq!((samples[0].forward, samples[0].buttons), (127, 0));
        assert_eq!((samples[1].forward, samples[1].turn_to), (127, None));
        // The look and the press start with the third frame's step.
        assert_eq!(samples[2].turn_to, Some((100, 0)));
        assert_eq!(samples[2].buttons, 0x41);
        // The release ran at the fourth sample; the view stays held.
        assert_eq!(
            (samples[3].buttons, samples[3].forward, samples[3].turn_to),
            (0, 0, None)
        );
        assert_eq!(samples[3].hold_view, Some((100, 0)));
        assert_eq!(script.pending(), 0);
    }

    #[test]
    fn timed_steps_last_until_their_time_and_turns_are_relative() {
        let mut script = Script::default();
        script.push([
            Step::Turn(-200, 50),
            Step::Run {
                frames: 0,
                ms: 100,
                buttons: 0x40,
                forward: 0,
                right: -127,
            },
        ]);
        let first = script.next(1000, (32_700, 0));
        assert_eq!(first.turn_to, Some((32_500, 50)));
        assert_eq!((first.buttons, first.right), (0x40, -127));
        assert_eq!(script.next(1099, (0, 0)).buttons, 0x40);
        assert_eq!(script.next(1100, (0, 0)).buttons, 0x40);
        assert_eq!(script.next(1101, (0, 0)).buttons, 0);
    }

    #[test]
    fn the_quaternion_matches_the_calibrated_view() {
        // owdbg's calibration: yaw 45 degrees, pitch 10 degrees.
        let q = view_quaternion(8192, 1820);
        let expected = [0.080_5, 0.381_2, -0.033_4, 0.920_4];
        for (got, want) in q.iter().zip(expected) {
            assert!((got - want).abs() < 1e-3, "{q:?}");
        }
    }
}
