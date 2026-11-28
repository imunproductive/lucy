//! Character stance, take-off, landing and pogo spring state for 104319.
//!
//! Client research: on the ground the feet snap to the ground
//! (0x7FF789AD0150); the capsule springs to pogo above the feet (implicit
//! damped spring, 0x7FF789B4F110; offsets within 1/512 m rest). A stance
//! change does not move the capsule: the spring offset takes the difference
//! of the pogo heights and a second offset starts from there
//! (0x7FF789B4C5DF); it wears off by as much as the first moves
//! (0x7FF789B4F31D). The tick start's feet and pogo vector are read at
//! 0x7FF789ACF060. The spring offset's range is mover +0x4F8 / +0x4FC (set at
//! 0x7FF789AD0BD1).

use crate::{
    contact::math::TINY as TINY_SQUARED,
    mover::ground::{ProbeResult, capsule, pogo},
    mover::kinematics::{Ground, LAND_SPEED, WORLD_GRAVITY},
    mover::modifiers::{ActiveModifiers, CROUCH_INTENT, JUMP_INTENT, LAND_LIMIT},
    mover::settings::MoverSettings,
};
use lucy_simd::reciprocal_sqrt_estimate;
use lucy_wire::{
    commands::{CROUCH, Command, JUMP, frame_distance},
    entity::POSITION_UNITS,
};

const JUMP_LATCH: u32 = 0x1;
pub(crate) const CROUCHED: u32 = 0x2;
pub(crate) const JUMPED: u32 = 0x4;
pub(crate) const FELL: u32 = 0x8;
const REAL_JUMP: u32 = 0x400;
pub(crate) const TOUCHING: u32 = 0x20000;
pub(crate) const NOT_GROUND_UNDER: u32 = 0x80000;
const TURN_HELD: u32 = 0x200000;
pub(crate) const IN_AIR: u32 = 0x80000c;

const SPRING_REST: f32 = 1.0 / 512.0;
const SPRING_STEPS: f32 = 32768.0;
const TINY: f32 = f32::EPSILON;
const CROUCH_AGAIN: f64 = 0.15;

pub fn snap(value: f64) -> f64 {
    (value * POSITION_UNITS + 0.5).floor() / POSITION_UNITS
}

pub struct Posture {
    pub flags: u32,
    pub air_ticks: u32,
    pub crouch_end: Option<u32>,
    landed: bool,
    /// The last transition jumped (the jump from the ground).
    jumped: bool,
    pub fall: f32,
    spring: Spring,
    #[cfg(test)]
    pub(super) fall_projection: Option<FallProjection>,
    #[cfg(test)]
    fall_added: Option<f32>,
}

/// Inputs before grid quantization, for the corpus's independent fall models.
/// None of this observation state exists in production builds.
#[cfg(test)]
pub(super) struct FallProjection {
    pub accumulated: f32,
    pub added: Option<f32>,
    pub gravity_scale: f64,
    pub vertical: f64,
    pub landed: bool,
    pub airborne: bool,
}

struct Spring {
    low: f32,
    high: f32,
    primary: f32,
    secondary: f32,
    encoded: [u16; 2],
    speed: f32,
}

pub struct PostureStep<'a> {
    pub command: &'a Command,
    pub base: &'a MoverSettings,
    pub values: &'a MoverSettings,
    pub mods: &'a ActiveModifiers,
    pub probe: ProbeResult,
    pub dt: f64,
    pub aim_held: bool,
    /// A dash or rewind drives this pose (`move_to` in `Mover.step_pose`,
    /// `ow174/game/mover.py`): no jump, and the feet keep their height
    /// instead of standing on the probed ground before the spring settles.
    pub driven: bool,
}

impl Posture {
    pub fn new(base: &MoverSettings) -> Self {
        let mut spring = Spring {
            low: 0.0,
            high: 0.0,
            primary: 0.0,
            secondary: 0.0,
            encoded: [0; 2],
            speed: 0.0,
        };
        spring.set_limits(base);
        // At rest the offsets are 0, which is not step 0 (the bottom of the
        // range): see `spring_steps`.
        spring.encoded = [spring.step(0.0).unwrap_or(0); 2];
        Self {
            flags: 0,
            air_ticks: 0,
            crouch_end: None,
            landed: false,
            jumped: false,
            fall: 0.0,
            spring,
            #[cfg(test)]
            fall_projection: None,
            #[cfg(test)]
            fall_added: None,
        }
    }

    pub fn ground(&self) -> Ground {
        match self.flags {
            flags if flags & IN_AIR == 0 => Ground::Standing,
            flags if flags & TOUCHING != 0 => Ground::AirborneTouching,
            _ => Ground::Airborne,
        }
    }

    pub fn crouched(&self) -> bool {
        self.flags & CROUCHED != 0
    }

    /// Whether the last transition landed the body (from a jump or a fall).
    pub fn landed(&self) -> bool {
        self.landed
    }

    /// Whether the last transition jumped it off the ground.
    pub fn jumped(&self) -> bool {
        self.jumped
    }

    pub fn spring_offset(&self) -> f32 {
        self.spring.primary
    }

    pub fn spring_speed(&self) -> f32 {
        self.spring.speed
    }

    /// Both spring offsets as steps of their range (state +52, +56). At rest
    /// (offset 0) they are not 0: step 0 is the bottom of the range, the
    /// capsule a whole stance and pogo below the feet. Another client places
    /// a body's collision capsule at its feet plus the pogo plus the first
    /// offset decoded from them (0x7FF789ACF010, 0x7FF789ACF060): with step 0
    /// a remote body's capsule was below the floor, measured in the original
    /// client (leafall903/ORPHEUS (MIT) `docs/MCCREE_ROLL.txt`, "Spring
    /// fields in the movement record").
    pub fn spring_steps(&self) -> [u16; 2] {
        self.spring.encoded
    }

    /// Run after the ground probe and before resolving the requested motion.
    /// The resize callback commits the new capsule only if it fits.
    pub fn advance<E>(
        &mut self,
        step: PostureStep<'_>,
        feet: &mut [f64; 3],
        velocity: &mut [f64; 3],
        resize: impl FnOnce(f32, f32) -> Result<bool, E>,
    ) -> Result<(), E> {
        let was_crouched = self.crouched();
        let start = SpringAnchor {
            feet: feet[1],
            pogo: pogo(step.base, was_crouched),
            spring: self.spring.primary,
        };
        self.transition(&step, velocity);

        let frame = step.command.frame;
        let requested = step.command.buttons & CROUCH != 0
            && (was_crouched
                || self.crouch_end.is_none_or(|stood| {
                    f64::from(frame_distance(frame, stood)) * step.dt >= CROUCH_AGAIN
                }))
            && self.flags & IN_AIR == 0;
        let crouch = step
            .mods
            .intent(CROUCH_INTENT, f64::from(u8::from(requested)))
            > 0.5;
        if crouch != was_crouched {
            self.change_stance(crouch, frame, step.base, resize)?;
        }
        if self.flags & IN_AIR == 0 && !step.driven {
            if let Some(ground) = step.probe.ground {
                feet[1] = ground.y;
            }
        }
        self.spring.settle(
            start,
            feet[1],
            pogo(step.base, self.crouched()),
            step.values,
            step.dt as f32,
        );
        Ok(())
    }

    /// A frame a `MoveTo` move drives (mover behaviour kind 1, phases 1 and
    /// 3): its slot 8 (0x7FF789B58F10) takes the place of the ground probe
    /// and the transitions, so nothing lands, falls or counts air ticks;
    /// it clears flag 0x80000 and stands the body up if it can
    /// (0x7FF789B4C3D0 with 0). Its slot 10 (0x7FF789B58410) skips the
    /// spring. The aim still sets the turn flag.
    pub fn hold<E>(
        &mut self,
        frame: u32,
        base: &MoverSettings,
        aim_held: bool,
        resize: impl FnOnce(f32, f32) -> Result<bool, E>,
    ) -> Result<(), E> {
        self.landed = false;
        self.flags = (self.flags & !NOT_GROUND_UNDER) | (u32::from(aim_held) * TURN_HELD);
        if self.crouched() {
            self.change_stance(false, frame, base, resize)?;
        }
        Ok(())
    }

    /// 0x7FF789B4C3D0: the stance changes when its capsule fits; the
    /// spring takes the difference of the pogo heights (0x7FF789B4C5DF).
    fn change_stance<E>(
        &mut self,
        crouch: bool,
        frame: u32,
        base: &MoverSettings,
        resize: impl FnOnce(f32, f32) -> Result<bool, E>,
    ) -> Result<(), E> {
        let (radius, segment) = capsule(base, crouch);
        if resize(radius, segment + radius)? {
            self.flags = (self.flags & !CROUCHED) | (u32::from(crouch) * CROUCHED);
            if !crouch {
                self.crouch_end = Some(frame);
            }
            let difference = (base.crouch_pogo - base.stand_pogo) as f32;
            if difference < 0.0 {
                let shift = if crouch { -difference } else { difference };
                let minimum = -pogo(base, crouch);
                self.spring.primary = minimum.max(f64::from(self.spring.primary + shift)) as f32;
                self.spring.secondary = self.spring.primary;
            }
        }
        Ok(())
    }

    fn transition(&mut self, step: &PostureStep<'_>, velocity: &mut [f64; 3]) {
        let mut flags = self.flags;
        self.landed = false;
        self.jumped = false;
        if flags & IN_AIR != 0 {
            let [x, y, z] = *velocity;
            let limit = step.mods.intent(LAND_LIMIT, LAND_SPEED);
            let landing = step.probe.ground.is_some_and(|ground| {
                let [nx, ny, nz] = ground.normal;
                (x * nx + y * ny) + z * nz < limit
            });
            if landing {
                if flags & (JUMPED | FELL) != 0 {
                    flags &= !(JUMPED | FELL);
                    self.air_ticks = 0;
                    self.landed = true;
                }
            } else {
                self.air_ticks = self.air_ticks.saturating_add(1);
            }
        } else if step.probe.ground.is_none() {
            self.air_ticks = 0;
            flags = (flags & !JUMPED) | FELL;
        }
        flags = (flags & !NOT_GROUND_UNDER)
            | (u32::from(step.probe.non_ground_under) * NOT_GROUND_UNDER)
            | (u32::from(step.aim_held) * TURN_HELD);
        let button = step.command.buttons & JUMP != 0;
        let jump = !step.driven && step.mods.intent(JUMP_INTENT, f64::from(u8::from(button))) > 0.5;
        if jump {
            if flags & (IN_AIR | JUMP_LATCH) == 0 {
                velocity[1] = step.values.jump_speed;
                self.air_ticks = 0;
                flags = (flags & !(FELL | REAL_JUMP)) | JUMPED | REAL_JUMP | JUMP_LATCH;
                self.jumped = true;
            }
        } else if flags & IN_AIR == 0 {
            flags &= !JUMP_LATCH;
        }
        self.flags = flags;
    }

    /// The velocity's vertical speed rose over the pose's velocity changes
    /// (0x7FF789AD0370): the body falls, its air ticks from 0 unless it was
    /// already in the air.
    pub fn take_off(&mut self) {
        if self.flags & (JUMPED | FELL) == 0 {
            self.air_ticks = 0;
        }
        self.flags = (self.flags & !(JUMPED | FELL)) | FELL;
    }

    /// A frame Meteor Strike's flight moves (mover behaviour kind 13): its
    /// slot 8 (`0x7FF789B58E70`) takes the place of the ground probe and the
    /// transitions, so nothing lands, jumps or counts air ticks, and clears
    /// flag 0x20000.
    pub fn rise_hold(&mut self) {
        self.landed = false;
        self.jumped = false;
        self.flags &= !TOUCHING;
    }

    /// Kind 13's slot 10 (`0x7FF789B583A0`): the spring's offset is `offset`
    /// and its speed 0, each on its grid (`0x7FF789B774B0`).
    pub fn pin_spring(&mut self, offset: f32) {
        self.spring.primary = offset;
        self.spring.speed = 0.0;
        self.spring.encode();
    }

    pub fn apply_fall(&mut self, gravity_scale: f64, dt: f64) {
        let added = (-WORLD_GRAVITY * gravity_scale) as f32 * dt as f32;
        self.fall += added;
        #[cfg(test)]
        {
            self.fall_added = Some(added);
        }
    }

    /// Update the gravity portion after the contact solver projects velocity.
    ///
    /// `0x7FF789B4F530` normalizes the accumulated fall, projects the new
    /// velocity onto it, then scales that unit vector by the projection.
    /// Keep both float32 products: the native unit vector is not exactly
    /// +/-1. Its `rsqrtps` estimate is CPU-specific, so a server and client
    /// with different estimates can still disagree in the last bits.
    pub fn finish_fall(&mut self, gravity_scale: f64, vertical: f64) {
        #[cfg(test)]
        {
            self.fall_projection = Some(FallProjection {
                accumulated: self.fall,
                added: self.fall_added.take(),
                gravity_scale,
                vertical,
                landed: self.landed,
                airborne: self.flags & IN_AIR != 0,
            });
        }
        let squared = self.fall * self.fall;
        let inverse = if squared > TINY_SQUARED {
            let estimate = reciprocal_sqrt_estimate(squared);
            (3.0 - (estimate * estimate) * squared) * (estimate * 0.5)
        } else {
            return;
        };
        let length = squared * inverse;
        if length.abs() > TINY {
            if self.landed {
                self.fall = 0.0;
            } else if self.flags & IN_AIR != 0 {
                let gravity = (-WORLD_GRAVITY * gravity_scale) as f32;
                let vertical = vertical as f32;
                if gravity * vertical < 0.0 {
                    self.fall = 0.0;
                } else {
                    let unit = self.fall * inverse;
                    let projection = vertical * unit;
                    if projection < length {
                        self.fall = unit * projection;
                    }
                }
            }
        }
    }
}

struct SpringAnchor {
    feet: f64,
    pogo: f64,
    spring: f32,
}

impl Spring {
    fn set_limits(&mut self, base: &MoverSettings) {
        let stand = base.stand_height + base.stand_pogo;
        let crouch = base.crouch_height + base.crouch_pogo;
        self.low = -stand.max(crouch) as f32;
        self.high = (2.0 * base.step_down) as f32;
    }

    fn settle(
        &mut self,
        start: SpringAnchor,
        feet: f64,
        pogo: f64,
        values: &MoverSettings,
        dt: f32,
    ) {
        let lifted = (start.pogo + f64::from(start.spring)) as f32;
        let rest = (start.feet + f64::from(lifted)) as f32 - feet as f32;
        let offset = (f64::from(rest) - pogo) as f32;
        let (primary, secondary, speed) = if offset.abs() > SPRING_REST {
            let omega = (values.pogo_frequency * f64::from(std::f32::consts::TAU)) as f32;
            let h = omega * dt;
            let pull = ((omega * omega) * dt) * offset;
            let damping = (((values.pogo_damping + values.pogo_damping) as f32 + h) * h) + 1.0;
            let mut speed = (self.speed - pull) / damping;
            let mut spring = speed * dt + offset;
            let moved = (spring - self.primary).abs();
            if spring.abs() <= SPRING_REST {
                spring = 0.0;
                speed = if dt > TINY { -(offset / dt) } else { 0.0 };
            }
            let secondary = if moved > self.secondary.abs() {
                0.0
            } else if self.secondary > 0.0 {
                self.secondary - moved
            } else if self.secondary < 0.0 {
                self.secondary + moved
            } else {
                self.secondary
            };
            (spring, secondary, speed)
        } else {
            (0.0, 0.0, 0.0)
        };
        self.primary = primary.max(self.low).min(self.high);
        self.secondary = secondary.max(self.low).min(self.high);
        self.encode();
        self.speed = snap(f64::from(speed)) as f32;
    }

    /// An offset's step on the 1/32768 grid of the range (0x7FF789B745E0),
    /// or None when the range is empty.
    fn step(&self, offset: f32) -> Option<u16> {
        let span = self.high - self.low;
        (span > TINY).then(|| {
            let fraction = ((offset - self.low) * (1.0 / span)).clamp(0.0, 1.0);
            (fraction * SPRING_STEPS + 0.5).floor() as u16
        })
    }

    fn encode(&mut self) {
        let (Some(primary), Some(secondary)) = (self.step(self.primary), self.step(self.secondary))
        else {
            self.primary = 0.0;
            self.secondary = 0.0;
            self.encoded = [0; 2];
            return;
        };
        let span = self.high - self.low;
        self.encoded = [primary, secondary];
        [self.primary, self.secondary] = self
            .encoded
            .map(|steps| span * (f32::from(steps) / SPRING_STEPS) + self.low);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fall_component_uses_the_native_unit_projection_products() {
        for fall in [-0.28_f32, -0.56, -0.84, -1.724_616_8, -3.5, -7.0] {
            let mut posture = Posture::new(&MoverSettings::default());
            posture.flags = FELL;
            posture.fall = fall;
            let vertical = fall * 0.5;
            let squared = fall * fall;
            let r = reciprocal_sqrt_estimate(squared);
            let inverse = (3.0 - (r * r) * squared) * (r * 0.5);
            let unit = fall * inverse;
            let expected = unit * (vertical * unit);
            posture.finish_fall(1.75, f64::from(vertical));
            assert_eq!(posture.fall.to_bits(), expected.to_bits(), "{fall}");

            posture.finish_fall(1.75, 1.0);
            assert_eq!(posture.fall, 0.0, "upward velocity clears the fall");
            posture.fall = fall;
            posture.flags = 0;
            posture.landed = true;
            posture.finish_fall(1.75, 0.0);
            assert_eq!(posture.fall, 0.0, "landing clears the fall");
        }
    }

    #[test]
    fn the_captured_apex_fall_keeps_its_exact_native_bits() {
        // PharahEmote run5, B73E00 hook5, frame5240: the preceding
        // server fall is zero; its new vertical speed is the old scalar
        // clamp's -0.044648438692092896. The Ryzen capture normalizes the
        // -0.28 gravity increment with rsqrtps estimate 3.5712890625.
        let mut posture = Posture::new(&MoverSettings::default());
        posture.flags = FELL;
        posture.apply_fall(1.75, 0.016);
        let squared = posture.fall * posture.fall;
        let r = reciprocal_sqrt_estimate(squared);
        let inverse = (3.0 - (r * r) * squared) * (r * 0.5);
        let unit = posture.fall * inverse;
        let vertical = f32::from_bits(0xbd36_e148);
        let expected = if r.to_bits() == 0x4064_9000 {
            f32::from_bits(0xbd36_e14a)
        } else {
            // Intel/AMD estimates need not match. Assert the formula
            // exactly instead: this CPU cannot replay the capture's last
            // bits without substituting the other CPU's rsqrt table.
            unit * (vertical * unit)
        };
        posture.finish_fall(1.75, f64::from(vertical));
        assert_eq!(posture.fall.to_bits(), expected.to_bits());
    }
}
