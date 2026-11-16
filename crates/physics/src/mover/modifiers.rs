//! Frame-scoped movement modifiers from build 104319 statescript.
//!
//! The scalar slots and operation order follow the Python reference.
//!
//! Client research: the mover keeps its mods in a list at mover +0x6E0
//! (0x38-byte entries), sorted by priority, then start (0x7FF789B3FAA0); one
//! acts on ticks start..end - 1 (0x7FF789B42FFB). The op on a value is
//! 0x7FF789B42ED0 (kinds 0 add, 1 multiply, 2 set, 3 min, 4 max, 5 clamp
//! `max(min(v, max(lo, hi)), lo)`), the indexes this mover applies are in
//! 0x7FF789B4CBA0 (jump table at RVA 0xC3D920), and the intents (jump,
//! crouch, landing limit) read through 0x7FF789B42FB0. The statescript's
//! MovementMod state is STU_316CFEF2; its mod classes (STU_6810506D:
//! m_A9561CA0 index, m_9B7A63EA value) map to ops by type object at
//! 0x7FF789C4C1E0: STU_4CB0950F add, STU_D8CC6B10 multiply, STU_56DEE1BB set,
//! STU_C61711C8 min, STU_F197CFBF max; STU_0A019882 is the clamp
//! (m_8AD739CE lo, m_8213EB22 hi). The client gives the
//! mover each mod while its state is on (begin 0x7FF789C49590, tick
//! 0x7FF789C4C1E0, end 0x7FF789C499C0); a mod starts at the statescript's
//! frame + 1 (0x7FF789C4C510..0x7FF789C4C52A) and ends at the frame + 1 it
//! went off in (0x7FF789C49CE1, 0x7FF789B456C0). Soldier: 76's Sprint adds
//! 0.5 to the speed scalar and sets the crouch intent to 0.
//!
//! Mod ids and what they change (the mover profile sits at mover +0x418 +
//! the STU offset, `orpheus_assets::heroes::movers`):
//!
//! - 0 every move speed (a scale), 1 accel_ground, 2 accel_air, 3
//!   accel_air_touching, 7 decel_air_per_speed, 9 decel_air_base_speed (with
//!   0), 10 jump_speed, 11 gravity_scale, 13/14 terminal_velocity / _up, 15/16
//!   ground_gravity_accel / _decel, 22 character_collision, 38 step_down
//!   ([`ActiveModifiers::settings`], 0x7FF789B4CBA0).
//! - 28/29 the yaw's and 30/31 the pitch's lower/upper limits in degrees,
//!   +-10000 unlimited (stored as radians at +0x4E4..+0x4F0 by
//!   0x7FF789B4CBA0, used by the aim at 0x7FF789B4C6A0; see
//!   [`ActiveModifiers::aim_limits`]).
//! - 17/18/19 the forward, backward and sideways throttle magnitudes
//!   (0x7FF789B43960; [`ActiveModifiers::throttles`]).
//! - 32 the velocity's length, 33 the horizontal speed, 34 the vertical
//!   velocity, 35 the vertical velocity in jump_speed units, after gravity
//!   and before the move (0x7FF789B41870;
//!   [`ActiveModifiers::limit_velocity`]).
//! - 20 jump, 21 crouch, 36 landing limit: intents.
//! - Not traced to a field, applied as the Python reference has them or not
//!   at all: 4, 5, 6 and 8 (the decel_ground_* and decel_air_base scales
//!   below), 12 (a runtime value at +0x4D4), 23 (runtime bytes +0x4D0 and
//!   +0x4D1), 24, 25 (byte +0x14DB), 26 (+0x4DC), 27 and 37.

use std::{cmp::Ordering, collections::BTreeMap};

use crate::mover::settings::MoverSettings;
use orpheus_wire::commands::{FRAME_HALF, frame_distance};

pub const JUMP_INTENT: u8 = 20;
pub const CROUCH_INTENT: u8 = 21;
pub const LAND_LIMIT: u8 = 36;
pub(crate) const CAP_TINY: f64 = 1.1920929e-5;

const SPEED: usize = 0;
const ACCEL_GROUND: usize = 1;
const ACCEL_AIR: usize = 2;
const ACCEL_TOUCHING: usize = 3;
const DECEL_GROUND: usize = 4;
const DECEL_GROUND_BASE: usize = 5;
const DECEL_GROUND_SPEED: usize = 6;
const DECEL_AIR: usize = 7;
const DECEL_AIR_BASE: usize = 8;
const DECEL_AIR_SPEED: usize = 9;
const JUMP_SPEED: usize = 10;
const GRAVITY: usize = 11;
const TERMINAL: usize = 13;
const TERMINAL_UP: usize = 14;
const GROUND_ACCEL: usize = 15;
const GROUND_DECEL: usize = 16;
const COLLISION: usize = 22;
const YAW_MIN: u8 = 28;
const YAW_MAX: u8 = 29;
const PITCH_MIN: u8 = 30;
const PITCH_MAX: u8 = 31;
const FORWARD_THROTTLE: u8 = 17;
const BACKWARD_THROTTLE: u8 = 18;
const SIDE_THROTTLE: u8 = 19;
const SPEED_LIMIT: u8 = 32;
const HORIZONTAL_SPEED: u8 = 33;
const VERTICAL_VELOCITY: u8 = 34;
const JUMP_VELOCITY: u8 = 35;
const STEP_DOWN: usize = 38;
/// An unlimited aim limit, in degrees.
const NO_LIMIT: f64 = 10_000.0;
const DEGREES: f64 = 0.017453292;
const EPSILON: f64 = 1.1754944e-36;

pub(crate) fn narrow(value: f64) -> f64 {
    f64::from(value as f32)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Operation {
    Add,
    Multiply,
    Set,
    Minimum,
    Maximum,
    /// Between the modifier's value and this upper bound; a bound below the
    /// value gives the value.
    Clamp {
        upper: f64,
    },
}

impl Operation {
    fn apply(self, value: f64, operand: f64) -> f64 {
        match self {
            Self::Add => narrow(value + operand),
            Self::Multiply => narrow(value * operand),
            Self::Set => operand,
            Self::Minimum if operand < value => operand,
            Self::Maximum if operand > value => operand,
            Self::Minimum | Self::Maximum => value,
            Self::Clamp { upper } => value.min(operand.max(upper)).max(operand),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Modifier {
    pub index: u8,
    pub operation: Operation,
    pub value: f64,
    pub priority: f64,
}

impl Modifier {
    fn apply(self, value: f64) -> f64 {
        self.operation.apply(value, self.value)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ModifierKey {
    pub instance: u16,
    pub state: u16,
    pub slot: usize,
}

struct Interval {
    modifier: Modifier,
    start: u32,
    end: Option<u32>,
}

#[derive(Default)]
pub struct ModifierTimeline {
    intervals: BTreeMap<ModifierKey, Interval>,
}

impl ModifierTimeline {
    /// Refresh the active states after a simulated command. Newly added or
    /// retired states take effect at the next frame. Retained states can
    /// refresh their value and priority without changing their start order.
    pub fn track(&mut self, frame: u32, active: &BTreeMap<ModifierKey, Modifier>) {
        let next = frame.wrapping_add(1);
        self.intervals.retain(|key, interval| {
            if interval.end.is_some_and(|end| reached(frame, end)) {
                return false;
            }
            if !active.contains_key(key) {
                interval.end.get_or_insert(next);
            }
            true
        });
        for (&key, &modifier) in active {
            let interval = self.intervals.entry(key).or_insert(Interval {
                modifier,
                start: next,
                end: None,
            });
            if interval.end.take().is_some() {
                interval.start = next;
                interval.modifier = modifier;
            } else {
                interval.modifier.value = modifier.value;
                interval.modifier.priority = modifier.priority;
            }
        }
    }

    pub fn at(&self, frame: u32) -> ActiveModifiers {
        let mut active: Vec<_> = self
            .intervals
            .values()
            .filter_map(|interval| {
                let age = frame_distance(frame, interval.start);
                (age < FRAME_HALF && interval.end.is_none_or(|end| !reached(frame, end)))
                    .then_some((age, interval.modifier))
            })
            .collect();
        // Stable sort keeps the iteration order of equal start and priority
        // values. The timeline already iterates by instance, state and slot.
        active.sort_by(|(left_age, left), (right_age, right)| {
            let priority = if left.priority == right.priority {
                Ordering::Equal
            } else {
                left.priority.total_cmp(&right.priority)
            };
            priority.then_with(|| right_age.cmp(left_age))
        });
        ActiveModifiers(active)
    }
}

fn reached(frame: u32, boundary: u32) -> bool {
    frame_distance(frame, boundary) < FRAME_HALF
}

#[derive(Default)]
pub struct ActiveModifiers(Vec<(u32, Modifier)>);

/// Aim limits in radians: `[yaw_min, yaw_max, pitch_min, pitch_max]`.
pub type AimLimits = [f64; 4];

impl ActiveModifiers {
    fn applies(&self, index: u8) -> bool {
        self.0.iter().any(|(_, modifier)| modifier.index == index)
    }

    /// Mods 28..31 on +-10000 degrees, in radians (0x7FF789B4CBA0).
    pub fn aim_limits(&self) -> AimLimits {
        [
            (YAW_MIN, -NO_LIMIT),
            (YAW_MAX, NO_LIMIT),
            (PITCH_MIN, -NO_LIMIT),
            (PITCH_MAX, NO_LIMIT),
        ]
        .map(|(index, base)| narrow(self.intent(index, base) * DEGREES))
    }

    /// The move's throttles (0x7FF789B43960): mod 17 on the forward part, 18
    /// on the backward part's size, 19 on the sideways size, each back in
    /// -1..1. The pose keeps the command's throttles.
    pub fn throttles(&self, right: f64, forward: f64) -> (f64, f64) {
        if self.0.is_empty() {
            return (right, forward);
        }
        let ahead = self.intent(FORWARD_THROTTLE, forward.max(0.0));
        let back = self.intent(BACKWARD_THROTTLE, -forward.min(0.0));
        let side = self.intent(SIDE_THROTTLE, right.abs());
        let side = if right < 0.0 { -side } else { side };
        (side.clamp(-1.0, 1.0), narrow(ahead - back).clamp(-1.0, 1.0))
    }

    /// The velocity after gravity (0x7FF789B41870): mod 32 on its length,
    /// 33 on its horizontal length, 34 on its vertical part and 35 on the
    /// vertical part over `jump_speed` (the tick's, after mod 10).
    pub fn limit_velocity(&self, velocity: [f64; 3], jump_speed: f64) -> [f64; 3] {
        if self.0.is_empty() {
            return velocity;
        }
        let scale = |velocity: [f64; 3], index: u8, axes: [bool; 3]| {
            let squared = narrow(
                (0..3)
                    .filter(|&axis| axes[axis])
                    .map(|axis| velocity[axis] * velocity[axis])
                    .sum(),
            );
            if squared <= EPSILON {
                return velocity;
            }
            let length = narrow(squared.sqrt());
            if length <= CAP_TINY {
                return velocity;
            }
            let ratio = narrow(self.intent(index, length) / length);
            std::array::from_fn(|axis| {
                if axes[axis] {
                    narrow(velocity[axis] * ratio)
                } else {
                    velocity[axis]
                }
            })
        };
        let velocity = scale(velocity, SPEED_LIMIT, [true; 3]);
        let [x, y, z] = scale(velocity, HORIZONTAL_SPEED, [true, false, true]);
        let mut y = self.intent(VERTICAL_VELOCITY, y);
        if jump_speed > CAP_TINY && self.applies(JUMP_VELOCITY) {
            y = narrow(self.intent(JUMP_VELOCITY, narrow(y / jump_speed)) * jump_speed);
        }
        [x, y, z]
    }

    pub fn intent(&self, index: u8, base: f64) -> f64 {
        self.0
            .iter()
            .map(|(_, modifier)| *modifier)
            .filter(|modifier| modifier.index == index)
            .fold(base, |value, modifier| modifier.apply(value))
    }

    pub fn settings(&self, base: &MoverSettings) -> MoverSettings {
        if self.0.is_empty() {
            return *base;
        }
        let mut slots = [None; STEP_DOWN + 1];
        slots[..10].fill(Some(1.0));
        for (index, value) in [
            (JUMP_SPEED, base.jump_speed),
            (GRAVITY, 1.0),
            (TERMINAL, base.terminal_velocity),
            (TERMINAL_UP, base.terminal_velocity_up),
            (GROUND_ACCEL, base.ground_gravity_accel),
            (GROUND_DECEL, base.ground_gravity_decel),
            (COLLISION, f64::from(base.character_collision)),
            (STEP_DOWN, 1.0),
        ] {
            slots[index] = Some(value);
        }
        for &(_, modifier) in &self.0 {
            if let Some(Some(value)) = slots.get_mut(usize::from(modifier.index)) {
                *value = modifier.apply(*value);
            }
        }
        let slot = |index: usize| slots[index].expect("initialized movement scalar");
        let nonnegative = |index| slot(index).max(0.0);
        let speed = nonnegative(SPEED);
        let ground_decel = nonnegative(DECEL_GROUND);
        let air_decel = nonnegative(DECEL_AIR);
        MoverSettings {
            run_forward: narrow(base.run_forward * speed),
            run_backward: narrow(base.run_backward * speed),
            run_strafe: narrow(base.run_strafe * speed),
            crouch_forward: narrow(base.crouch_forward * speed),
            crouch_backward: narrow(base.crouch_backward * speed),
            crouch_strafe: narrow(base.crouch_strafe * speed),
            accel_ground: narrow(base.accel_ground * nonnegative(ACCEL_GROUND)),
            accel_air: narrow(base.accel_air * nonnegative(ACCEL_AIR)),
            accel_air_touching: narrow(base.accel_air_touching * nonnegative(ACCEL_TOUCHING)),
            decel_ground_base: narrow(
                narrow(nonnegative(DECEL_GROUND_BASE) * ground_decel) * base.decel_ground_base,
            ),
            decel_ground_per_speed: narrow(ground_decel * base.decel_ground_per_speed),
            decel_ground_base_speed: narrow(
                narrow(speed * slot(DECEL_GROUND_SPEED)) * base.decel_ground_base_speed,
            ),
            decel_air_base: narrow(
                narrow(nonnegative(DECEL_AIR_BASE) * air_decel) * base.decel_air_base,
            ),
            decel_air_per_speed: narrow(air_decel * base.decel_air_per_speed),
            decel_air_base_speed: narrow(
                narrow(speed * slot(DECEL_AIR_SPEED)) * base.decel_air_base_speed,
            ),
            jump_speed: nonnegative(JUMP_SPEED),
            gravity_scale: narrow(slot(GRAVITY) * base.gravity_scale),
            terminal_velocity: nonnegative(TERMINAL),
            terminal_velocity_up: nonnegative(TERMINAL_UP),
            ground_gravity_accel: slot(GROUND_ACCEL),
            ground_gravity_decel: slot(GROUND_DECEL),
            character_collision: u32::from(slot(COLLISION) > CAP_TINY),
            step_down: narrow(base.step_down * nonnegative(STEP_DOWN)),
            ..*base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifier(operation: Operation, value: f64, priority: f64) -> Modifier {
        Modifier {
            index: SPEED as u8,
            operation,
            value,
            priority,
        }
    }

    #[test]
    fn intervals_switch_on_the_next_frame_across_wrap() {
        let key = ModifierKey {
            instance: 1,
            state: 22,
            slot: 0,
        };
        let base = MoverSettings::default();
        let mut timeline = ModifierTimeline::default();
        let sprint = BTreeMap::from([(key, modifier(Operation::Multiply, 1.5, 0.0))]);
        timeline.track(u32::MAX, &sprint);
        assert_eq!(timeline.at(u32::MAX).settings(&base), base);
        assert_eq!(timeline.at(0).settings(&base).run_forward, 8.25);
        timeline.track(0, &BTreeMap::new());
        assert_eq!(timeline.at(0).settings(&base).run_forward, 8.25);
        assert_eq!(timeline.at(1).settings(&base), base);
        timeline.track(1, &BTreeMap::new());
        assert!(timeline.intervals.is_empty());
        timeline.track(2, &sprint);
        assert_eq!(timeline.at(2).settings(&base), base);
        assert_eq!(timeline.at(3).settings(&base).run_forward, 8.25);
    }

    #[test]
    fn application_orders_priority_then_first_frame_then_state_slot() {
        let key = |state| ModifierKey {
            instance: 1,
            state,
            slot: 0,
        };
        let mut timeline = ModifierTimeline::default();
        let mut active = BTreeMap::from([(key(9), modifier(Operation::Set, 3.0, 0.0))]);
        timeline.track(1, &active);
        active.insert(key(3), modifier(Operation::Multiply, 3.0, 0.0));
        active.insert(key(2), modifier(Operation::Add, 1.0, 0.0));
        active.insert(key(1), modifier(Operation::Minimum, 1.0, 1.0));
        timeline.track(2, &active);
        assert_eq!(timeline.at(2).intent(SPEED as u8, 1.0), 3.0);
        assert_eq!(timeline.at(3).intent(SPEED as u8, 1.0), 1.0);
        active.remove(&key(1));
        active.get_mut(&key(9)).unwrap().value = 2.0;
        timeline.track(3, &active);
        assert_eq!(timeline.at(4).intent(SPEED as u8, 1.0), 9.0);
    }

    fn on(index: u8, operation: Operation, value: f64) -> ActiveModifiers {
        ActiveModifiers(vec![(
            0,
            Modifier {
                index,
                operation,
                value,
                priority: 0.0,
            },
        )])
    }

    #[test]
    fn every_kind_changes_accel_ground_the_way_the_client_does() {
        let base = MoverSettings::default();
        let accel = |operation, value| {
            on(ACCEL_GROUND as u8, operation, value)
                .settings(&base)
                .accel_ground
        };
        let scaled = |factor: f64| narrow(base.accel_ground * factor);
        assert_eq!(accel(Operation::Add, 0.5), scaled(1.5));
        assert_eq!(accel(Operation::Multiply, 2.0), scaled(2.0));
        assert_eq!(accel(Operation::Set, 3.0), scaled(3.0));
        assert_eq!(accel(Operation::Minimum, 0.5), scaled(0.5));
        assert_eq!(accel(Operation::Maximum, 2.0), scaled(2.0));
        assert_eq!(accel(Operation::Clamp { upper: 2.0 }, 1.5), scaled(1.5));
        assert_eq!(accel(Operation::Clamp { upper: 0.5 }, 0.25), scaled(0.5));
        // An upper bound below the lower one gives the lower one.
        assert_eq!(accel(Operation::Clamp { upper: 0.25 }, 3.0), scaled(3.0));
    }

    #[test]
    fn mapped_ids_set_their_profile_fields() {
        let base = MoverSettings::default();
        let set = |index, value| on(index, Operation::Set, value).settings(&base);
        let narrow_times = |a: f64, b: f64| narrow(a * b);
        let speed = set(0, 2.0);
        assert_eq!(speed.run_forward, narrow_times(base.run_forward, 2.0));
        assert_eq!(speed.crouch_strafe, narrow_times(base.crouch_strafe, 2.0));
        assert_eq!(
            set(1, 2.0).accel_ground,
            narrow_times(base.accel_ground, 2.0)
        );
        assert_eq!(set(2, 2.0).accel_air, narrow_times(base.accel_air, 2.0));
        assert_eq!(
            set(3, 2.0).accel_air_touching,
            narrow_times(base.accel_air_touching, 2.0)
        );
        assert_eq!(
            set(7, 2.0).decel_air_per_speed,
            narrow_times(base.decel_air_per_speed, 2.0)
        );
        assert_eq!(
            set(9, 2.0).decel_air_base_speed,
            narrow_times(base.decel_air_base_speed, 2.0)
        );
        assert_eq!(set(10, 9.0).jump_speed, 9.0);
        assert_eq!(
            set(11, 2.0).gravity_scale,
            narrow_times(base.gravity_scale, 2.0)
        );
        assert_eq!(set(13, 7.0).terminal_velocity, 7.0);
        assert_eq!(set(14, 7.0).terminal_velocity_up, 7.0);
        assert_eq!(set(15, 7.0).ground_gravity_accel, 7.0);
        assert_eq!(set(16, 7.0).ground_gravity_decel, 7.0);
        assert_eq!(set(22, 0.0).character_collision, 0);
        assert_eq!(set(38, 2.0).step_down, narrow_times(base.step_down, 2.0));
    }

    #[test]
    fn throttle_mods_scale_each_direction_and_keep_to_one() {
        let none = ActiveModifiers::default();
        assert_eq!(none.throttles(-0.5, -0.5), (-0.5, -0.5));
        let mods = on(FORWARD_THROTTLE, Operation::Multiply, 0.5);
        assert_eq!(mods.throttles(0.0, 1.0), (0.0, 0.5));
        assert_eq!(mods.throttles(0.0, -1.0), (0.0, -1.0));
        let mods = on(BACKWARD_THROTTLE, Operation::Set, 0.0);
        assert_eq!(mods.throttles(0.0, -1.0), (0.0, 0.0));
        let mods = on(SIDE_THROTTLE, Operation::Multiply, 3.0);
        assert_eq!(mods.throttles(-0.5, 0.25), (-1.0, 0.25));
    }

    #[test]
    fn aim_limits_are_degrees_in_radians() {
        assert_eq!(
            ActiveModifiers::default().aim_limits(),
            [-NO_LIMIT, NO_LIMIT, -NO_LIMIT, NO_LIMIT].map(|limit| narrow(limit * DEGREES))
        );
        let limits = on(PITCH_MAX, Operation::Set, 10.0).aim_limits();
        assert_eq!(limits[3], narrow(10.0 * DEGREES));
        assert_eq!(limits[0], narrow(-NO_LIMIT * DEGREES));
    }

    #[test]
    fn velocity_mods_cap_length_horizontal_and_vertical_speeds() {
        let velocity = [3.0, 4.0, 0.0];
        let capped = on(SPEED_LIMIT, Operation::Minimum, 2.5).limit_velocity(velocity, 6.0);
        assert_eq!(capped, [1.5, 2.0, 0.0]);
        let flat = on(HORIZONTAL_SPEED, Operation::Multiply, 2.0).limit_velocity(velocity, 6.0);
        assert_eq!(flat, [6.0, 4.0, 0.0]);
        let vertical = on(VERTICAL_VELOCITY, Operation::Set, -60.0).limit_velocity(velocity, 6.0);
        assert_eq!(vertical, [3.0, -60.0, 0.0]);
        let jump = on(JUMP_VELOCITY, Operation::Maximum, 2.0).limit_velocity(velocity, 6.0);
        assert_eq!(jump, [3.0, 12.0, 0.0]);
        // Mod 35 does nothing without a jump speed.
        let jump = on(JUMP_VELOCITY, Operation::Maximum, 2.0).limit_velocity(velocity, 0.0);
        assert_eq!(jump, velocity);
    }
}
