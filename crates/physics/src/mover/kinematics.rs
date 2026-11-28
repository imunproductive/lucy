//! Player command recovery and velocity before contact resolution.
//!
//! The PC path in `ow174/game/mover.py` runs friction, steering, the direction
//! cap and gravity in this order after its ground and stance transitions.
//!
//! Client functions: throttles (0x7FF789B49FA0, 0x7FF789B4DA30): the state
//! keeps `trunc(t * 127)` in float32, which for 16 values is one less in size
//! (9 -> 8, 13 -> 12, ... 104 -> 103). Wish direction 0x7FF789B44340;
//! direction top speed (strafe speed blended toward forward or backward by the
//! cosine to the facing) 0x7FF789ACEF40; friction 0x7FF789B43090 (the base
//! part goes with the tick's run speed, the per-speed part with the body's
//! own: the PC path, cvar at 0x7FF78BD3EE20); air drag 0x7FF789B416A0.

use std::f32::consts::{PI, TAU as TAU_F32};
use std::f64::consts::TAU;

use crate::mover::{
    modifiers::{ActiveModifiers, CAP_TINY},
    settings::MoverSettings,
};
use lucy_wire::commands::{Command, SUBFRAME, frame_distance};

const EPSILON: f64 = 1.1754943508222875e-36;
const TINY: f64 = 1.1920929e-7;
pub const LAND_SPEED: f64 = 0.004;
pub const WORLD_GRAVITY: f64 = 10.0;
const TURN: f64 = TAU / 65536.0;
const PITCH_RADIANS: f32 = 0.000_095_873_8;
const PITCH_LIMIT: f32 = 1.553_343;
const PITCH_UNITS: f32 = 10_430.378;
const THROTTLE_UNIT: f32 = 1.0 / 127.0;
/// A held input repeats this many frames past its command; later frames
/// keep only the aim (`ow174/game/mover.py`, the reconstructed poses).
pub const HOLD_INPUT_FRAMES: u32 = 32;

/// The input of `frame` when its command is missing: `previous` held, as a
/// reconstructed pose has it. One-shot actions and the sub-frame click never
/// repeat; past [`HOLD_INPUT_FRAMES`] only the aim does ([`neutral`]).
pub fn repeated(previous: Command, frame: u32) -> Command {
    if frame_distance(frame, previous.frame) > HOLD_INPUT_FRAMES {
        return neutral(previous, frame);
    }
    held(previous, frame)
}

/// `previous`'s input for `frame`, however far it is: the movement keys,
/// the aim and the held buttons, without its one-shot action or sub-frame
/// click.
pub fn held(previous: Command, frame: u32) -> Command {
    Command {
        frame,
        buttons: previous.buttons & !SUBFRAME,
        action: 0,
        click: None,
        ..previous
    }
}

/// The input of `frame` with nothing held: no button, no movement key,
/// `previous`'s aim.
pub fn neutral(previous: Command, frame: u32) -> Command {
    Command {
        frame,
        yaw: previous.yaw,
        pitch: previous.pitch,
        ..Command::default()
    }
}

/// Reconstruct short gaps before the newest command. Long gaps start at
/// the newest command. One-shot actions and subframe aim never repeat.
pub fn poses(previous: Option<Command>, newest: Command) -> Poses {
    let (previous, distance) = previous
        .map(|command| (command, frame_distance(newest.frame, command.frame)))
        .filter(|&(_, distance)| (2..=2 * HOLD_INPUT_FRAMES).contains(&distance))
        .unwrap_or((newest, 1));
    Poses {
        previous,
        newest,
        offsets: 1..distance + 1,
    }
}

/// Resumable sequence of reconstructed poses followed by the actual command.
pub struct Poses {
    previous: Command,
    newest: Command,
    offsets: std::ops::Range<u32>,
}

impl Poses {
    pub(crate) fn newest(&self) -> Command {
        self.newest
    }

    pub fn next_frame(&self) -> Option<u32> {
        let behind = self.offsets.len().checked_sub(1)? as u32;
        Some(self.newest.frame.wrapping_sub(behind))
    }
}

impl Iterator for Poses {
    type Item = Command;

    fn next(&mut self) -> Option<Command> {
        let offset = self.offsets.next()?;
        if self.offsets.is_empty() {
            return Some(self.newest);
        }
        Some(repeated(
            self.previous,
            self.previous.frame.wrapping_add(offset),
        ))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.offsets.size_hint()
    }
}

impl ExactSizeIterator for Poses {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionInput {
    pub yaw: i32,
    pub pitch: i16,
    /// The aim was held: the pitch to +-89 degrees, or either angle to a
    /// limit of mods 28..31.
    pub aim_held: bool,
    pub throttles: [i8; 2],
    right: f64,
    forward: f64,
}

impl From<&Command> for MotionInput {
    fn from(command: &Command) -> Self {
        Self::new(command, &ActiveModifiers::default())
    }
}

impl MotionInput {
    /// The command's input under the frame's mods: the aim held to mods
    /// 28..31's limits (0x7FF789B4C6A0: the yaw limits move by a turn to lie
    /// within half a turn of the yaw, the pitch also keeps to +-89 degrees)
    /// and the move's throttles through mods 17..19. The record's throttle
    /// bytes follow the modified direction (0x7FF789B4D9C0), not the command:
    /// zero throttles do not refresh the remembered input frame.
    /// [ASSUMPTION] a held yaw is rounded back to turn units half away from
    /// zero, as the pitch is.
    pub fn new(command: &Command, mods: &ActiveModifiers) -> Self {
        let right = throttle(command.right);
        let forward = throttle(command.forward);
        let [yaw_min, yaw_max, pitch_min, pitch_max] = mods.aim_limits().map(|limit| limit as f32);
        let yaw = command.yaw as i16 as f32 * PITCH_RADIANS;
        let middle = (yaw_min + yaw_max) * 0.5;
        let shift = if middle + PI < yaw {
            TAU_F32
        } else if middle - PI > yaw {
            -TAU_F32
        } else {
            0.0
        };
        let yaw_held = yaw.min(yaw_max + shift).max(yaw_min + shift);
        let pitch = command.pitch as f32 * PITCH_RADIANS;
        let held = pitch
            .min(pitch_max)
            .max(pitch_min)
            .clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let (right, forward) = mods.throttles(right, forward);
        let (right, forward) = mods.directed(right, forward, yaw_held);
        Self {
            yaw: if yaw_held == yaw {
                command.yaw
            } else {
                units(yaw_held)
            },
            pitch: units(held) as i16,
            aim_held: pitch != held || yaw != yaw_held,
            throttles: [right, forward]
                .map(|value| ((value as f32 * 127.0).clamp(-127.0, 127.0)) as i8),
            right,
            forward,
        }
    }

    /// The forward throttle under mods 17 and 18 (-1..1).
    pub(crate) fn forward(&self) -> f64 {
        self.forward
    }
}

/// Radians back to s16 turn units, rounded half away from zero.
fn units(radians: f32) -> i32 {
    let scaled = radians * PITCH_UNITS;
    let rounded = if scaled < 0.0 {
        scaled - 0.5
    } else {
        scaled + 0.5
    };
    rounded as i32
}

fn throttle(byte: i8) -> f64 {
    f64::from(f32::from(byte.max(-127)) * THROTTLE_UNIT)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ground {
    Standing,
    Airborne,
    AirborneTouching,
}

#[derive(Clone, Copy)]
struct Speeds {
    forward: f64,
    backward: f64,
    strafe: f64,
}

impl Speeds {
    fn new(settings: &MoverSettings, crouched: bool) -> Self {
        if crouched {
            Self {
                forward: settings.crouch_forward,
                backward: settings.crouch_backward,
                strafe: settings.crouch_strafe,
            }
        } else {
            Self {
                forward: settings.run_forward,
                backward: settings.run_backward,
                strafe: settings.run_strafe,
            }
        }
    }

    fn directional_cap(self, [dx, dz]: [f64; 2], length: f64, [fx, fz]: [f64; 2]) -> f64 {
        if length < CAP_TINY {
            return self.strafe;
        }
        let cosine = (fx * dx + fz * dz) / length;
        if cosine > 0.0 {
            self.strafe + cosine * (self.forward - self.strafe)
        } else {
            self.strafe - cosine * (self.backward - self.strafe)
        }
    }

    fn wish(self, input: MotionInput, [fx, fz]: [f64; 2]) -> (f64, f64, f64) {
        let ahead = if input.forward > 0.0 {
            self.forward
        } else {
            self.backward
        };
        let x = input.right * self.strafe * -fz + input.forward * ahead * fx;
        let z = input.right * self.strafe * fx + input.forward * ahead * fz;
        let speed = length(x, z);
        if speed <= 0.0 {
            return (0.0, 0.0, 0.0);
        }
        let (x, z) = (x / speed, z / speed);
        (x, z, speed.min(self.directional_cap([x, z], 1.0, [fx, fz])))
    }
}

pub fn top_speed(settings: &MoverSettings, crouched: bool) -> f64 {
    let speeds = Speeds::new(settings, crouched);
    speeds.forward.max(speeds.backward).max(speeds.strafe)
}

pub struct VelocityStep<'a> {
    pub base: &'a MoverSettings,
    pub settings: &'a MoverSettings,
    pub input: MotionInput,
    pub ground: Ground,
    pub crouched: bool,
    pub dt: f64,
}

impl VelocityStep<'_> {
    /// Advance the unconstrained velocity, in m/s. The caller resolves
    /// world contacts and quantizes the final motion after this step.
    pub fn advance(&self, previous: [f64; 3]) -> [f64; 3] {
        let angle = f64::from(self.input.yaw) * TURN;
        let axes = [angle.sin(), angle.cos()];
        let speeds = Speeds::new(self.settings, self.crouched);
        let velocity = self.friction(previous);
        let wish = speeds.wish(self.input, axes);
        let [mut x, mut y, mut z] = self.steer(velocity, wish);
        let squared = x * x + z * z;
        let speed = if squared > EPSILON {
            squared.sqrt()
        } else {
            0.0
        };
        let cap = speeds.directional_cap([x, z], speed, axes);
        let allowed = speed.min(cap.max(length(previous[0], previous[2])));
        if (speed - allowed).abs() > LAND_SPEED && squared >= EPSILON {
            x = x / speed * allowed;
            z = z / speed * allowed;
        }
        if self.ground != Ground::Standing {
            y -= WORLD_GRAVITY * self.settings.gravity_scale * self.dt;
        }
        [x, y, z]
    }

    fn friction(&self, [mut x, mut y, mut z]: [f64; 3]) -> [f64; 3] {
        let data = self.settings;
        let grounded = self.ground == Ground::Standing;
        let (base, base_speed, per_speed) = if grounded {
            (
                data.decel_ground_base,
                data.decel_ground_base_speed,
                data.decel_ground_per_speed,
            )
        } else {
            (
                data.decel_air_base,
                data.decel_air_base_speed,
                data.decel_air_per_speed,
            )
        };
        let speed = length(x, z);
        let rate = base * data.run_forward
            + (speed - base_speed).max(0.0) * per_speed * self.base.run_forward;
        let slow = speed.min(self.dt * rate);
        if speed * speed > EPSILON && slow > 0.0 {
            x -= x / speed * slow;
            z -= z / speed * slow;
        }
        if !grounded {
            let terminal = if y > 0.0 {
                data.terminal_velocity_up
            } else {
                data.terminal_velocity
            };
            if terminal > CAP_TINY {
                let drag = -WORLD_GRAVITY * data.gravity_scale * y.abs() / (terminal * terminal)
                    * y
                    * self.dt;
                let length = (x * x + y * y + z * z).sqrt();
                y += drag.abs().min(length).copysign(drag);
            }
        }
        [x, y, z]
    }

    fn steer(&self, [x, y, z]: [f64; 3], (wx, wz, wish): (f64, f64, f64)) -> [f64; 3] {
        let data = self.settings;
        let rate = match self.ground {
            Ground::Standing => data.accel_ground,
            Ground::Airborne => data.accel_air,
            Ground::AirborneTouching => data.accel_air_touching,
        } * data.run_forward;
        let squared = x * x + z * z;
        let speed = if squared >= EPSILON {
            squared.sqrt()
        } else {
            -1.0
        };
        let target = if wish > 0.0 { speed.max(wish) } else { wish };
        let (cx, cz) = if speed > 0.0 { (x, z) } else { (-x, -z) };
        let (dx, dz) = (target * wx - cx, target * wz - cz);
        let delta = length(dx, dz);
        if delta * delta <= EPSILON || target <= 0.0 {
            return [x, y, z];
        }
        let k = (rate * self.dt / delta).clamp(0.0, 1.0);
        let (mut nx, mut nz) = (cx + k * dx, cz + k * dz);
        if speed > TINY && wx * cx + wz * cz > 0.0 {
            let desired = speed + (rate * self.dt).min(target - speed);
            let norm = length(nx, nz);
            if norm > 0.0 {
                nx = nx / norm * desired;
                nz = nz / norm * desired;
            }
        }
        [x + (nx - cx), y, z + (nz - cz)]
    }
}

fn length(x: f64, z: f64) -> f64 {
    (x * x + z * z).sqrt()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{
        mover::modifiers::{
            CROUCH_INTENT, JUMP_INTENT, LAND_LIMIT, Modifier, ModifierKey, ModifierTimeline,
            Operation,
        },
        mover::settings::MoverCatalog,
    };
    use lucy_wire::commands::{Click, FIRE};

    #[test]
    fn small_input_gaps_repeat_held_inputs_then_neutral_and_keep_newest() {
        let last = Command {
            frame: u32::MAX - 30,
            forward: 127,
            right: -9,
            yaw: 32781,
            pitch: -15,
            buttons: FIRE | SUBFRAME,
            action: 3,
            extra: 70,
            click: Some(Click {
                subframe: 15,
                yaw: 33,
                pitch: 12,
            }),
        };
        let command = Command {
            frame: 33,
            action: 5,
            ..last
        };
        let recovered: Vec<_> = poses(Some(last), command).collect();
        assert_eq!(recovered.len(), 64);
        assert_eq!(recovered[29].frame, u32::MAX);
        assert_eq!(recovered[30].frame, 0);
        assert_eq!((recovered[31].buttons, recovered[31].extra), (FIRE, 70));
        assert_eq!((recovered[31].action, recovered[31].click), (0, None));
        assert_eq!(
            recovered[32],
            Command {
                frame: 2,
                yaw: last.yaw,
                pitch: last.pitch,
                ..Command::default()
            },
        );
        assert_eq!(recovered[63], command);
        let long_gap = Command {
            frame: 34,
            ..command
        };
        assert_eq!(poses(Some(last), long_gap).collect::<Vec<_>>(), [long_gap]);
    }

    #[test]
    fn clamps_pitch_and_stores_the_truncated_throttle() {
        let positive = MotionInput::from(&Command {
            pitch: 30000,
            right: 9,
            forward: -128,
            ..Command::default()
        });
        let negative = MotionInput::from(&Command {
            pitch: -30000,
            right: -9,
            forward: 127,
            ..Command::default()
        });
        assert_eq!((positive.pitch, negative.pitch), (16202, -16202));
        assert!(positive.aim_held && negative.aim_held);
        assert_eq!(positive.throttles, [8, -127]);
        assert_eq!(negative.throttles, [-8, 127]);
        assert_eq!(
            MotionInput::from(&Command {
                pitch: 89,
                ..Command::default()
            })
            .pitch,
            89
        );
    }

    #[test]
    fn movement_modifiers_are_reflected_in_record_throttle_bytes() {
        let mut timeline = ModifierTimeline::default();
        timeline.track(
            0,
            &[(17, 0.5), (18, 0.25), (19, 0.75)]
                .into_iter()
                .enumerate()
                .map(|(slot, (index, value))| {
                    (
                        ModifierKey {
                            instance: 1,
                            state: 1,
                            slot,
                        },
                        Modifier {
                            index,
                            operation: Operation::Multiply,
                            value,
                            priority: 1.0,
                        },
                    )
                })
                .collect(),
        );
        let mods = timeline.at(1);
        let input = |right, forward| {
            MotionInput::new(
                &Command {
                    right,
                    forward,
                    ..Command::default()
                },
                &mods,
            )
        };
        // B4D9C0 multiplies the post-mod floats by 127, clamps and truncates:
        // 127 * 0.75 -> 95, 127 * 0.5 -> 63, 127 * 0.25 -> 31.
        assert_eq!(input(-127, 127).throttles, [-95, 63]);
        assert_eq!(input(127, -127).throttles, [95, -31]);
    }

    #[test]
    fn aim_mods_hold_yaw_and_pitch_to_their_limits() {
        let key = |slot| ModifierKey {
            instance: 1,
            state: 1,
            slot,
        };
        let set = |index, value| Modifier {
            index,
            operation: Operation::Set,
            value,
            priority: 0.0,
        };
        let mut timeline = ModifierTimeline::default();
        timeline.track(
            0,
            &BTreeMap::from([
                (key(0), set(28, 170.0)),
                (key(1), set(29, 190.0)),
                (key(2), set(31, 10.0)),
            ]),
        );
        let mods = timeline.at(1);
        let input = |yaw, pitch| {
            MotionInput::new(
                &Command {
                    yaw,
                    pitch,
                    ..Command::default()
                },
                &mods,
            )
        };
        // -175.8 degrees lies within 170..190 once the limits move a turn.
        let inside = input(-32000, -3000);
        assert_eq!(
            (inside.yaw, inside.pitch, inside.aim_held),
            (-32000, -3000, false)
        );
        // Yaw 11 degrees goes to 170, pitch 16.5 to 10 degrees.
        let held = input(2000, 3000);
        assert!(held.aim_held);
        assert!((30940..=30950).contains(&held.yaw), "{}", held.yaw);
        assert_eq!(held.pitch, 1820);
    }

    #[test]
    fn movement_preserves_momentum_and_uses_contact_specific_air_control() {
        let settings = MoverSettings::default();
        let step = VelocityStep {
            base: &settings,
            settings: &settings,
            input: MotionInput::from(&Command {
                forward: 127,
                ..Command::default()
            }),
            ground: Ground::Standing,
            crouched: false,
            dt: 0.016,
        };
        assert_eq!(step.advance([0.0; 3]), [0.0, 0.0, 3.08]);
        assert!((step.advance([0.0, 0.0, 12.0])[2] - 9.6592).abs() < 1e-12);
        let air = VelocityStep {
            ground: Ground::Airborne,
            ..step
        }
        .advance([0.0; 3]);
        let touching = VelocityStep {
            ground: Ground::AirborneTouching,
            ..step
        }
        .advance([0.0; 3]);
        assert_eq!((air[2], touching[2]), (0.308, 0.044));
        assert_eq!(air[1], -0.28);
    }

    #[test]
    #[ignore = "requires the game data derived from a 1.74 install (just test-game <install>)"]
    fn movement_velocity_matches_python_across_authored_mover_settings() {
        let path = lucy_testkit::runtime_dir().join("data/game_movers_174.json");
        // mover.py `SOLDIER_BODY`, the fallback body.
        let movers = MoverCatalog::load(&path, 0x0400_0000_0000_03cf).unwrap();
        let raw: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(raw.len(), 38);
        let indexes = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 20, 21, 22, 36, 38,
        ];
        let operations = [
            Operation::Add,
            Operation::Multiply,
            Operation::Set,
            Operation::Minimum,
            Operation::Maximum,
        ];
        let dts = [0.0, 0.016, 0.032, 0.001, 0.25, 0.064];
        let mut digest = Sha256::new();
        for (i, guid) in raw.keys().enumerate() {
            let guid = lucy_core::guid::parse_guid(guid).unwrap();
            digest.update(guid.to_le_bytes());
            let base = movers.for_body(Some(guid));
            for n in 0..192 {
                let seed = n + i;
                let mut timeline = ModifierTimeline::default();
                if n % 4 != 0 {
                    let changes = [
                        (1, 0, 0, Operation::Multiply, 1.5, 2.0),
                        (1, 1, 0, Operation::Add, -0.24, -1.0),
                        (1, 2, 14, Operation::Set, (n % 15) as f64 / 2.0, 0.0),
                        (
                            1,
                            3,
                            11,
                            Operation::Multiply,
                            [-1.0, 0.0, 1.0, 1.25][(n / 4) % 4],
                            0.0,
                        ),
                        (
                            2,
                            0,
                            indexes[seed % indexes.len()],
                            operations[(seed / indexes.len()) % 5],
                            (((n * 17 + i * 5) % 33) as f64 - 9.0) / 7.0,
                            0.0,
                        ),
                    ];
                    let mods = changes
                        .into_iter()
                        .map(|(instance, slot, index, operation, value, priority)| {
                            (
                                ModifierKey {
                                    instance,
                                    state: 1,
                                    slot,
                                },
                                Modifier {
                                    index,
                                    operation,
                                    value,
                                    priority,
                                },
                            )
                        })
                        .collect();
                    timeline.track(100, &mods);
                }
                let active = timeline.at(101);
                let settings = active.settings(base);
                let input = MotionInput::from(&Command {
                    yaw: ((i * 10013 + n * 6029) % 262144) as i32 - 131072,
                    pitch: ((n * 907) % 200000) as i32 - 100000,
                    right: (((n * 29 + i * 5) % 256) as i16 - 128) as i8,
                    forward: (((n * 13 + i * 11) % 256) as i16 - 128) as i8,
                    ..Command::default()
                });
                digest.update(input.pitch.to_le_bytes());
                digest.update([
                    u8::from(input.aim_held),
                    input.throttles[0] as u8,
                    input.throttles[1] as u8,
                ]);
                for throttle in [input.right, input.forward] {
                    digest.update((throttle as f32).to_le_bytes());
                }
                for value in [
                    settings.run_forward,
                    settings.run_backward,
                    settings.run_strafe,
                    settings.crouch_forward,
                    settings.crouch_backward,
                    settings.crouch_strafe,
                    settings.accel_ground,
                    settings.accel_air,
                    settings.accel_air_touching,
                    settings.decel_ground_base,
                    settings.decel_ground_per_speed,
                    settings.decel_ground_base_speed,
                    settings.decel_air_base,
                    settings.decel_air_per_speed,
                    settings.decel_air_base_speed,
                    settings.jump_speed,
                    settings.gravity_scale,
                    settings.terminal_velocity,
                    settings.terminal_velocity_up,
                    settings.ground_gravity_accel,
                    settings.ground_gravity_decel,
                    settings.step_down,
                ] {
                    digest.update(value.to_le_bytes());
                }
                digest.update(settings.character_collision.to_le_bytes());
                for (index, value) in [
                    (JUMP_INTENT, 1.0),
                    (CROUCH_INTENT, 1.0),
                    (LAND_LIMIT, 0.004),
                ] {
                    digest.update(active.intent(index, value).to_le_bytes());
                }
                let ground = [Ground::Standing, Ground::Airborne, Ground::AirborneTouching][n % 3];
                let dt = dts[(n / 3) % dts.len()];
                let previous = match n % 17 {
                    0 => [0.0; 3],
                    1 => [1e-20, 0.125, -1e-19],
                    _ => [
                        (((n * 31 + i * 7) % 129) as f64 - 64.0) / 3.0,
                        (((n * 17 + i * 11) % 127) as f64 - 90.0) / 2.0,
                        (((n * 23 + i * 13) % 117) as f64 - 58.0) / 3.0,
                    ],
                };
                let step = VelocityStep {
                    base,
                    settings: &settings,
                    input,
                    ground,
                    crouched: seed % 2 == 0,
                    dt,
                };
                for axis in step.advance(previous) {
                    // Compare before the much coarser game grid but allow
                    // low-bit differences in the host's sin/cos.
                    digest.update(((axis * 16777216.0 + 0.5).floor() as i64).to_le_bytes());
                }
            }
        }
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "1cc8efee5023b28c77d6cfca201498c0c64e7706b0088fb4651cf6156d551347"
        );
    }
}
