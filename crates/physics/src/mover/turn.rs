//! The turn Reinhardt's Charge steers with (`STU_C168636D`, `007D` st6 and
//! st7): the client's mover behaviour of kind 7 (vtable `0x7FF78B56A878`,
//! made by the factory `0x7FF789B533F0`).
//!
//! The state's begin (`0x7FF789C407D0`) queues the behaviour from the next
//! frame with its block (slot 56 `0x7FF789C40000`, every value in degrees
//! times 0.017453292 as float32): `+24` the acceleration `m_717EAFE7`, `+28`
//! the deceleration `m_34A1CCB7`, `+32` the greatest rate `m_6716919E`; with
//! an output `m_9F17FE89` the state binds it (the charge's #85, the rate in
//! radians a second, which its search cones and the lean's data flow read).
//! Its begin (slot 4 `0x7FF789B53D80`) sets the movement state's flag
//! [`TURNING`] and the rate (`+0xD0`) to 0; its end (slot 5
//! `0x7FF789B53A10`) clears the flag.
//!
//! Each tick its slot 7 (`0x7FF789B57FF0`) runs in place of the aim (the
//! mover tick `0x7FF789B4B1D0` calls the aim `0x7FF789B4C6A0` only when no
//! behaviour's slot 7 answers true), so the body no longer turns to the
//! command's yaw and its pitch stays:
//!
//! 1. the right throttle (the command's byte, mover `+0xCF`, × 1/127)
//!    changes the rate by −dt × acceleration × throttle; with no throttle
//!    (|throttle| < 0.01) the rate falls toward 0 by dt × deceleration and
//!    stops there; then it is held to ± the greatest rate;
//! 2. the yaw (`atan2` of the rotation) grows by rate × dt, and the
//!    rotation is set from it (`0x7FF789B766B0` → `0x7FF789B763F0`, which
//!    keeps it as 16-bit turn units, rounded half away from zero).
//!
//! The velocity step then moves along the turned yaw. So the charge turns
//! with the side keys at up to 30°/s (#34534), 240°/s² up and down; the
//! view turns freely (the charge's view limits `STU_9935FBC8` st16 hold
//! only the pitch) and the first-person camera follows the body
//! (`STU_4773DA56` st2). Every value is float32 in the client's order of
//! operations; the yaw is taken from its 16-bit units (the client's
//! `atan2` of its own rotation, which was set from those units, agrees but
//! for its last bits).
//!
//! [`TURNING`]: orpheus_wire::entity::TURNING

use crate::mover::MoveKey;

/// Degrees to radians as the block takes them (slot 56).
const RADIANS: f32 = 0.017_453_292;
/// The right throttle's scale (`dword_7FF78B523B58`).
const THROTTLE: f32 = 0.007_874_016;
/// Below this the throttle is none (`dword_7FF78B5225F0`).
const NO_THROTTLE: f32 = 0.01;
/// 16-bit turn units to radians and back (`0x7FF789B4C6A0`,
/// `0x7FF789B763F0`).
const UNIT_RADIANS: f32 = 0.000_095_873_8;
const RADIAN_UNITS: f32 = 10_430.378;

/// What a `STU_C168636D` state hands the mover (the behaviour's block), in
/// radians.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Turn {
    /// `+24`: how fast a held side key changes the rate (per second²).
    pub acceleration: f32,
    /// `+28`: how fast the rate falls back to 0 without one.
    pub deceleration: f32,
    /// `+32`: the greatest rate (per second).
    pub greatest: f32,
}

impl Turn {
    /// The block from the state's fields in degrees (each as float32).
    pub fn from_degrees(acceleration: f64, deceleration: f64, greatest: f64) -> Self {
        let radians = |degrees: f64| degrees as f32 * RADIANS;
        Self {
            acceleration: radians(acceleration),
            deceleration: radians(deceleration),
            greatest: radians(greatest),
        }
    }
}

/// A running turn: its state's key, its block and the rate (`+0xD0`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Run {
    pub key: MoveKey,
    turn: Turn,
    rate: f32,
}

impl Run {
    /// Slot 4: the rate starts at 0.
    pub fn start(key: MoveKey, turn: Turn) -> Self {
        Self {
            key,
            turn,
            rate: 0.0,
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    /// Slot 7: one tick's rate from the command's right throttle, and the
    /// yaw (16-bit turn units) it turns the body to from `yaw`.
    pub fn step(&mut self, yaw: i32, right: i8, dt: f32) -> i32 {
        let throttle = f32::from(right) * THROTTLE;
        let mut rate = self.rate - dt * self.turn.acceleration * throttle;
        if throttle.abs() < NO_THROTTLE {
            let fall = dt * self.turn.deceleration;
            rate = if rate > 0.0 {
                (rate - fall).max(0.0)
            } else {
                (fall + rate).min(0.0)
            };
        }
        self.rate = rate.min(self.turn.greatest).max(-self.turn.greatest);
        let turned = self.rate * dt + yaw as i16 as f32 * UNIT_RADIANS;
        let scaled = turned * RADIAN_UNITS;
        let units = if scaled >= 0.0 {
            scaled + 0.5
        } else {
            scaled - 0.5
        };
        i32::from(units as i32 as i16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charge() -> Run {
        let key = MoveKey {
            instance: 2,
            raw_state: 6,
            activation: 1,
        };
        Run::start(key, Turn::from_degrees(240.0, 240.0, 30.0))
    }

    /// Practice Range session r0 (2026-11-22, `keys`): with the right key
    /// held his client's rate was −3.84°/s (−0.067020 rad/s, #85) after
    /// the first tick and the yaw went from 157.5° down by rate × dt each
    /// tick; the rate is held at 30°/s and falls back at 240°/s² once the
    /// key is let go.
    #[test]
    fn the_side_key_turns_the_charge_up_to_its_greatest_rate() {
        let mut run = charge();
        let mut yaw = 28672; // 157.5°
        yaw = run.step(yaw, 127, 0.016);
        assert_eq!(run.rate(), -0.067_020_65);
        // −0.0614° is 11.2 units.
        assert_eq!(yaw, 28661);
        for _ in 0..20 {
            yaw = run.step(yaw, 127, 0.016);
        }
        assert_eq!(run.rate(), -30.0 * RADIANS);
        let held = yaw;
        yaw = run.step(yaw, 0, 0.016);
        assert!(run.rate() > -30.0 * RADIANS && run.rate() < 0.0);
        assert!(yaw < held);
        for _ in 0..10 {
            yaw = run.step(yaw, 0, 0.016);
        }
        assert_eq!(run.rate(), 0.0);
        assert_eq!(run.step(yaw, 0, 0.016), yaw);
    }

    /// The crash's turn (st7) has no rate at all: the body keeps its yaw
    /// whatever the keys do.
    #[test]
    fn a_turn_without_a_rate_holds_the_yaw() {
        let mut run = Run::start(
            MoveKey {
                instance: 2,
                raw_state: 7,
                activation: 1,
            },
            Turn::from_degrees(0.0, 0.0, 0.0),
        );
        assert_eq!(run.step(-32000, -127, 0.016), -32000);
        assert_eq!(run.rate(), 0.0);
    }

    /// The yaw past half a turn wraps as the client's 16-bit units do.
    #[test]
    fn the_turned_yaw_wraps_at_half_a_turn() {
        let mut run = charge();
        for _ in 0..20 {
            run.step(0, -127, 0.016);
        }
        assert_eq!(run.rate(), 30.0 * RADIANS);
        // 179.995° + 0.48°: 32854 units, past 32767.
        assert_eq!(run.step(32767, -127, 0.016), 32854 - 65536);
    }
}
