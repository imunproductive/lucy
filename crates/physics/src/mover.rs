//! Server-side movement for an authored 104319 character capsule.
//!
//! The client never sends its position and moves no body but its own, so the
//! server runs the PC client's character mover for every player (image base
//! 0x7FF788F10000), one tick per command frame (dt = 16 ms). Main tick
//! 0x7FF789B4B1D0:
//!
//! 1. The body turns to the command's yaw (0x7FF789B4C6A0); the pitch is held
//!    to +-89 degrees (0x7FF789B763B0; s16 to radians by the float at
//!    0x7FF78B524F6C, limit at 0x7FF78B523F94, back to s16 at 0x7FF78B524F70,
//!    rounded half away from zero), which sets flag 0x200000 for good
//!    (0x7FF789B4C8C0). Mods 28..31 hold the yaw and the pitch to their
//!    limits first and also set the flag. A running turn (kind 7,
//!    [`turn`]) turns the body by its rate instead.
//! 2. A held crouch button does not crouch again within 0.15 s of standing up
//!    (0x7FF789B73CA0, constant 0x7FF78B567978).
//! 3. The ground probe (0x7FF789B48990) at the feet before anything moves.
//! 4. Transitions (0x7FF789B4F780): in the air, ground and `v.n < 0.004`
//!    (0x7FF78B509D70) lands (0x7FF789AD0400), else the air ticks count up; on
//!    the ground, no ground is a fall (0x7FF789AD0370). The jump sets the
//!    vertical speed to jump_speed (0x7FF789B48880, air ticks reset at
//!    0x7FF789AD03B0), flags 0x4 | 0x400 | 0x1. Crouch 0x7FF789B4C3D0 if the
//!    crouched capsule fits.
//! 5. The move context (0x7FF789B49FA0): ground normal (or up), horizontal
//!    velocity, yaw axes, throttles s8 / 127, then mods 17..19 on the
//!    throttles (0x7FF789B43960).
//! 6. The pogo spring (0x7FF789B4F110).
//! 7. Velocity: friction (0x7FF789B43090; air 0x7FF789B416A0 plus quadratic
//!    drag toward terminal_velocity while falling), acceleration
//!    (0x7FF789B434A0), the direction's speed cap (0x7FF789B44090), gravity
//!    `-10 * gravity_scale` in the air (0x7FF789B42100; world gravity 10 m/s^2
//!    at 0x7FF78AA63BD0; every hero's scale is 1.75 and jump speed 6 m/s),
//!    then mods 32..35 (0x7FF789B41870).
//! 8. The move (0x7FF789B4D9C0, 0x7FF789ACF640; outer rounds 0x7FF789B4B683):
//!    on the ground the velocity follows the ground plane (0x7FF789B4DD81);
//!    the engine moves the capsule by (velocity + spring speed) * dt; the
//!    velocity loses what goes into the contact planes (0x7FF789CCAD50).
//! 9. Gravity's part of the velocity (state +832, 0x7FF789B4F530; summed at
//!    0x7FF789B42133, normalised with rsqrtps so it can differ in the last
//!    bits between CPUs); positions and velocities on the 1/1024 m grid
//!    (0x7FF789B77730), spring offsets as 1/32768 steps of their range
//!    (0x7FF789B745E0, 0x7FF789B74E40, 0x7FF789B77550). The tick ends at
//!    0x7FF789D11B60.
//!
//! The mover writes a movement state for other clients' animation
//! (0x7FF789B4DA26): this command's throttles and the throttles and tick of
//! the last command that had any. Other flags: 0x20000 the last move touched
//! something (0x7FF789ACF8AC), 0x80000 the probe met a surface that is not
//! ground. Mover +0x14DD is "landed this tick", state +40 the frame the body
//! last stood up in, state +24 the gravity scale of the last tick
//! (0x7FF789B4D869). A statescript velocity action (`STU_FF3471E9`,
//! [`Mover::act_on_velocity`]) gives the next pose one-pose mods 32..35 and
//! velocity changes (mover +0x7D0) after them; a rise of the vertical speed
//! over both takes the body off the ground. Not modelled: abilities that
//! take over the move (the action's channel 0 among them), moving
//! platforms, other bodies. Knockback is a velocity added at the next posed
//! frame ([`Mover::push`]), a reconstruction (leafall903/ORPHEUS (MIT)
//! `heavy_assault.py` `_push`), not the client's code. The capsule rests
//! 5 mm out of a wall (the engine's contact skin).

pub mod correction;
pub mod ground;
pub mod ground_walk;
pub mod kinematics;
pub mod landing;
pub mod modifiers;
mod motion;
pub mod move_to;
pub mod path_walk;
pub mod platform;
#[cfg(test)]
mod ported_tests;
pub(crate) mod posture;
pub mod rise;
pub mod settings;
pub mod trial;
pub mod turn;

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    collision::index::TriangleIndex,
    contact::body::CharacterBody,
    contact::cache::CapsulePose,
    contact::gjk::PointSetSize,
    contact::hull::HullContactError,
    contact::solver::clip_velocity,
    contact::world::ContactWorld,
    mover::ground::{GroundProbe, ProbePose, pogo, standing_body},
    mover::kinematics::{Ground, MotionInput, Poses, VelocityStep, poses},
    mover::modifiers::{
        ActiveModifiers, KNOCKBACK_SCALE, Modifier, ModifierKey, ModifierTimeline, TAKE_OFF,
        VelocityChange, VelocityElement, change_velocity, narrow,
    },
    mover::posture::{
        CROUCHED, FELL, IN_AIR, JUMPED, NOT_GROUND_UNDER, Posture, PostureStep, TOUCHING, snap,
    },
    mover::settings::MoverSettings,
    nav::Navigation,
};
use lucy_wire::{
    commands::Command,
    entity::{Movement, MovementParent, NO_INPUT, POSITION_UNITS, TURNING},
};
pub use move_to::{MoveEnd, MoveKey, MoveTo, Speed};
pub use path_walk::PathWalk;
pub use rise::{DeathZones, Rise};
pub use turn::Turn;

/// What a state of the body's graphs hands the mover while it runs: a
/// `MoveTo`'s move ([`move_to`], kind 1), a path walk ([`path_walk`],
/// kind 10), the Charge's turn ([`turn`], kind 7) or Meteor Strike's
/// flight ([`rise`], kind 13).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Behaviour {
    MoveTo(MoveTo),
    PathWalk(PathWalk),
    Turn(Turn),
    Rise(Rise),
}

impl Behaviour {
    /// The client's behaviour kind (the factory `0x7FF789B533F0`'s).
    fn kind(&self) -> u8 {
        match self {
            Self::MoveTo(_) => 1,
            Self::Turn(_) => 7,
            Self::PathWalk(_) => 10,
            Self::Rise(_) => 13,
        }
    }
}

const ABYSS: f64 = 100.0;
const EPSILON: f64 = 1.1754943508222875e-36;
/// Positions and velocities are on a 1/1024 grid (`STEP` in `ow174/game/mover.py`).
const STEP: f64 = 1.0 / POSITION_UNITS;

/// Project the owner's current movement record into the authored 13-bit sample.
pub fn sample_movement_flags(pose: &Movement) -> Option<u16> {
    // These modes require nested native movement records. Regular Rust poses
    // cannot enter them or enqueue the scheduled transition represented by bit 10.
    if pose.flags & (0x20 | 0x80) != 0 {
        return None;
    }
    let [x, y, z] = pose.velocity.map(|value| value as f32);
    if ![x, y, z].iter().all(|value| value.is_finite()) {
        return None;
    }
    let airborne = pose.flags & IN_AIR != 0;
    let vertical_threshold = 100.0 * f32::EPSILON;
    let moving = (x * x + y * y) + z * z > 10.0 * f32::EPSILON;
    Some(
        u16::from(airborne)
            | u16::from(!airborne) << 1
            | u16::from(y > vertical_threshold) << 2
            | u16::from(y < -vertical_threshold) << 3
            | u16::from(moving) << 4
            | u16::from(pose.throttles != [0; 2]) << 5
            | u16::from(pose.flags & CROUCHED != 0) << 6
            | u16::from(pose.flags & NOT_GROUND_UNDER != 0) << 9
            | u16::from(pose.flags & JUMPED != 0) << 11,
    )
}

/// The record landed and left the ground within its own move: airborne with
/// its air ticks at 0 (the client's sampler `0x7FF789C4D420` reads the
/// mover state's `+0x10` for this).
pub fn movement_restarted(pose: &Movement) -> bool {
    pose.flags & IN_AIR != 0 && pose.air_ticks == 0
}

/// The spring steps of a body on `base` at rest (spring offsets 0), for the
/// movement records of a body the server places without a mover (a training
/// bot on its patrol). Not 0: see `Posture::spring_steps`.
pub fn resting_spring(base: &MoverSettings) -> [i32; 2] {
    Posture::new(base).spring_steps().map(i32::from)
}

pub struct Mover {
    base: MoverSettings,
    category: u16,
    probe: GroundProbe,
    body: CharacterBody,
    posture: Posture,
    modifiers: ModifierTimeline,
    last_command: Option<Command>,
    last_input: Option<u32>,
    pose: Movement,
    /// The `MoveTo` move the body's graphs run ([`move_to`]).
    move_to: Option<move_to::Run>,
    /// The path walk the body's graphs run ([`path_walk`]).
    walk: Option<path_walk::Walk>,
    /// The turn the body's graphs run ([`turn`]).
    turn: Option<turn::Run>,
    /// Meteor Strike's flight the body's graphs run ([`rise`]), with the
    /// block of the frame before.
    rise: Option<(MoveKey, Rise)>,
    /// What kills a body that falls into it ([`rise::fatal_drop`]).
    zones: Option<Arc<dyn DeathZones>>,
    /// The map's navigation, whose AI cells a path walk searches.
    navigation: Option<Arc<Navigation>>,
    /// A dash or rewind the body's graphs started ([`motion`]).
    motion: Option<motion::Motion>,
    /// Recent feet positions for a rewind.
    history: motion::History,
    /// The velocity [`Mover::push`] adds at the next posed frame.
    push: Option<[f64; 3]>,
    /// The next pose's mods of velocity actions ([`Mover::act_on_velocity`]).
    once: Vec<Modifier>,
    /// The next pose's velocity changes with their priorities.
    velocity_changes: Vec<(f64, VelocityChange)>,
    /// The move-speed scale of the last move (mover `+0x4D8`).
    speed_scale: f64,
    /// The knockback scale of the last move (mod 26, mover `+0x4DC`).
    knockback_scale: f64,
    /// The platform it stands on, with its pose in the platform's frame
    /// ([`platform`]).
    riding: Option<platform::Riding>,
    /// The platforms' frames the tick before, for their velocity.
    platforms: Option<Arc<dyn platform::PlatformClock>>,
}

impl Mover {
    pub fn new(
        base: MoverSettings,
        world: Arc<ContactWorld>,
        position: [f64; 3],
        yaw: i32,
        category: u16,
    ) -> Result<Self, PointSetSize> {
        let position = position.map(snap);
        let posture = Posture::new(&base);
        let spring = posture.spring_steps().map(i32::from);
        Ok(Self {
            base,
            category,
            probe: GroundProbe::new(&base)?,
            body: standing_body(&base, world, position, category),
            posture,
            modifiers: ModifierTimeline::default(),
            last_command: None,
            last_input: None,
            pose: Movement {
                position,
                yaw,
                gravity: base.gravity_scale,
                spring,
                ..Movement::default()
            },
            move_to: None,
            walk: None,
            turn: None,
            rise: None,
            zones: None,
            navigation: None,
            motion: None,
            history: motion::History::with_capacity(motion::HISTORY),
            push: None,
            once: Vec::new(),
            velocity_changes: Vec::new(),
            speed_scale: 1.0,
            knockback_scale: 1.0,
            riding: None,
            platforms: None,
        })
    }

    /// The map's navigation, whose AI cells the body's path walks search
    /// ([`path_walk`]); without it a walk finds no path.
    pub fn with_navigation(mut self, navigation: Option<Arc<Navigation>>) -> Self {
        self.navigation = navigation;
        self
    }

    /// The zones that kill a body that falls into them, which Meteor
    /// Strike's flight and floor test ([`rise::fatal_drop`]).
    pub fn with_death_zones(mut self, zones: Option<Arc<dyn DeathZones>>) -> Self {
        self.zones = zones;
        self
    }

    /// The clock of the platforms it may ride, for their velocity
    /// ([`platform::PlatformClock`]); without it they carry it with no
    /// velocity of their own.
    pub fn with_platform_clock(mut self, clock: Option<Arc<dyn platform::PlatformClock>>) -> Self {
        self.platforms = clock;
        self
    }

    /// Its pose in the world (feet and velocity in world terms, on a
    /// platform too).
    pub fn pose(&self) -> Movement {
        self.pose
    }

    /// Its movement state as the records carry it: flag
    /// [`platform::WORLD_AXES`] as the client's mover has it from its init,
    /// and on a platform the platform as its parent with the feet in its
    /// frame and the velocity relative to it ([`platform`]).
    pub fn record(&self) -> Movement {
        let flags = self.pose.flags | platform::WORLD_AXES;
        let Some(riding) = self.riding else {
            return Movement { flags, ..self.pose };
        };
        Movement {
            position: riding.local.map(f64::from),
            velocity: riding.velocity.map(f64::from),
            flags,
            parent: Some(MovementParent {
                entity: riding.platform.entity,
                node: riding.platform.node,
            }),
            ..self.pose
        }
    }

    /// The move-speed scale (mod 0) its last move used, as the client's
    /// mover keeps it at `+0x4D8` (`STU_ED807E07`); 1 before the first.
    pub fn speed_scale(&self) -> f64 {
        self.speed_scale
    }

    /// The knockback scale (mod 26, the client's mover `+0x4DC`, written
    /// only by its update `0x7FF789B4CBA0`, default 1, at least 0) its last
    /// move used: what `STU_ABE57332` answers for its body (Rocket Punch's
    /// victim graph `0D07` scales its push by it). The abilities that hold
    /// mod 26 at 0 (Mei's Cryo-Freeze, D.Va's Defense Matrix:
    /// OwnerPrediction.md "DaySpawnRoom2") make it 0.
    pub fn knockback_scale(&self) -> f64 {
        self.knockback_scale
    }

    pub fn crouched(&self) -> bool {
        self.posture.crouched()
    }

    /// Whether its last pose stands on the ground (no airborne flag).
    pub fn on_ground(&self) -> bool {
        self.posture.ground() == Ground::Standing
    }

    /// The target of the `MoveTo` move the body runs (the client's kind-1
    /// block `+0x20`), if any.
    pub fn move_target(&self) -> Option<[f64; 3]> {
        self.move_to.as_ref().map(move_to::Run::target)
    }

    /// Whether its last pose landed it on the ground, from a jump or a fall.
    pub fn landed(&self) -> bool {
        self.posture.landed()
    }

    /// Whether its last pose jumped it off the ground (the jump button).
    pub fn jumped(&self) -> bool {
        self.posture.jumped()
    }

    /// The body's capsule where it stands now: its lowest point at the feet
    /// plus the lift (the pogo plus the spring offset), its stance's radius
    /// and segment. The client places another player's proxy so from his
    /// records (`Posture::spring_steps`).
    pub fn capsule(&self) -> CapsulePose {
        self.body.pose()
    }

    /// The body as its statescript's wall checks move it on trial
    /// ([`trial::TrialMove`]).
    pub fn trial(&self) -> trial::TrialMove {
        trial::TrialMove::new(self.body.shared_world(), self.body.pose(), self.category)
            .with_death_zones(self.zones.clone())
    }

    /// The other characters that block the body's moves from now on
    /// ([`CharacterBody::set_peers`]), each a capsule as [`Mover::capsule`]
    /// gives it.
    pub fn set_peers(&mut self, peers: &[CapsulePose]) {
        self.body
            .set_peers(peers.iter().map(|peer| peer.world_capsule()));
    }

    pub fn track_modifiers(&mut self, frame: u32, active: &BTreeMap<ModifierKey, Modifier>) {
        self.modifiers.track(frame, active);
    }

    /// Add `velocity` to the body's at its next posed frame, as a knockback
    /// (leafall903/ORPHEUS (MIT) `heavy_assault.py` `_push`): each part in
    /// float32, and an upward push of a body on the ground takes it off
    /// (FELL, its air ticks from 0), so the frame flies it instead of
    /// holding it to the ground. Pushes before that frame add up.
    pub fn push(&mut self, velocity: [f64; 3]) {
        if !velocity.iter().all(|part| part.is_finite()) {
            return;
        }
        let pending = self.push.get_or_insert([0.0; 3]);
        for (pending, part) in pending.iter_mut().zip(velocity) {
            *pending += part;
        }
    }

    fn take_push(&mut self) {
        let Some(push) = self.push.take() else {
            return;
        };
        for (velocity, part) in self.pose.velocity.iter_mut().zip(push) {
            *velocity = f64::from((*velocity + part) as f32);
        }
        if push[1] > 0.0 && self.posture.flags & IN_AIR == 0 {
            self.posture.flags |= FELL;
            self.posture.air_ticks = 0;
        }
    }

    /// One element of a statescript velocity action for the next pose
    /// (0x7FF789C3B350): a mod of its own, or a velocity change made from
    /// the body's velocity now (channels 1 and 4 read it).
    pub fn act_on_velocity(&mut self, element: VelocityElement) {
        match element {
            VelocityElement::Modifier(modifier) => self.once.push(modifier),
            VelocityElement::Vector {
                channel,
                operation,
                value,
                priority,
            } => {
                if let Some(change) =
                    VelocityChange::of(channel, operation, value, self.pose.velocity)
                {
                    self.velocity_changes.push((priority, change));
                }
            }
        }
    }

    /// Simulate the newest command and any reconstructible poses before it.
    /// The callback can advance the body's statescript at each posed frame.
    pub fn step(
        &mut self,
        rays: &TriangleIndex,
        newest: Command,
        dt: f64,
        after_pose: impl FnMut(&mut Self, Command),
    ) -> Result<(), HullContactError> {
        let mut pending = self.pending_poses(newest);
        self.step_pending(rays, &mut pending, dt, usize::MAX, after_pose)?;
        Ok(())
    }

    pub fn pending_poses(&self, newest: Command) -> Poses {
        poses(self.last_command, newest)
    }

    pub fn step_pending(
        &mut self,
        rays: &TriangleIndex,
        pending: &mut Poses,
        dt: f64,
        limit: usize,
        mut after_pose: impl FnMut(&mut Self, Command),
    ) -> Result<usize, HullContactError> {
        self.step_pending_with(pending, limit, |mover, command| {
            mover.step_pose_to(rays, command, dt, None)?;
            after_pose(mover, command);
            Ok(())
        })
    }

    /// Like [`Mover::step_pending`], but `each` runs the pose itself (with
    /// [`Mover::step_pose_to`]), so it can drive it toward a world target
    /// (`Mover.step` with `simulate_pose` in `ow174/game/mover.py`, as
    /// `Player._step_body_pose` in `ow174/game/match.py` uses it).
    pub fn step_pending_with(
        &mut self,
        pending: &mut Poses,
        limit: usize,
        mut each: impl FnMut(&mut Self, Command) -> Result<(), HullContactError>,
    ) -> Result<usize, HullContactError> {
        let mut stepped = 0;
        for command in pending.by_ref().take(limit) {
            each(self, command)?;
            stepped += 1;
        }
        if stepped != 0 && pending.len() == 0 {
            self.last_command = Some(pending.newest());
        }
        Ok(stepped)
    }

    /// One posed frame (`Mover.step_pose` in `ow174/game/mover.py`). With
    /// `behaviour` (a state's [`Behaviour`]) a `MoveTo` drives the body's
    /// velocity toward its target instead of following the input, a path
    /// walk sets it along its path; a new key begins a new behaviour, none
    /// drops the running one. The result is the behaviour's end at the
    /// frame it ends (`None` while it runs, and on frames after its end
    /// before the state lets it go). A dash or rewind the body's graphs
    /// started ([`Mover::dash`], [`Mover::rewind`]) takes the pose before
    /// either.
    pub fn step_pose_to(
        &mut self,
        rays: &TriangleIndex,
        command: Command,
        dt: f64,
        behaviour: Option<(MoveKey, &Behaviour)>,
    ) -> Result<Option<MoveEnd>, HullContactError> {
        let rewinding = self.rewinding();
        self.take_push();
        self.begin_move(behaviour)?;
        let driven = self.move_to.as_ref().is_some_and(move_to::Run::drives);
        let rising = self.rise.map(|(_, rise)| rise);
        let once = std::mem::take(&mut self.once);
        let mut changes = std::mem::take(&mut self.velocity_changes);
        changes.sort_by(|(left, _), (right, _)| left.total_cmp(right));
        let changes: Vec<_> = changes.into_iter().map(|(_, change)| change).collect();
        let active = self.modifiers.at(command.frame).with_once(&once);
        let settings = active.settings(&self.base);
        self.speed_scale = active.speed_scale();
        self.knockback_scale = active.intent(KNOCKBACK_SCALE, 1.0).max(0.0);
        let mut input = MotionInput::new(&command, &active);
        if let Some(turn) = self.turn.as_mut() {
            // Kind 7's slot 7 in place of the aim: the body turns by the
            // rate and keeps its pitch.
            input.yaw = turn.step(self.pose.yaw, command.right, dt as f32);
            input.pitch = self.pose.pitch as i16;
            input.aim_held = false;
        }
        if driven || rising.is_some() || self.motion.is_some() {
            // Assumption: a behaviour that takes over the move, or an
            // authored motion, leaves the platform (the client's behaviour
            // kinds keep their own probe; not read).
            self.leave_platform(dt as f32);
        }
        let mut carry = [0.0f32; 3];
        let normal = if driven {
            // Kind 1's slot 8: no ground probe and no transitions, the up
            // as the ground's normal; slot 10: no spring.
            self.posture
                .hold(command.frame, &self.base, input.aim_held, |radius, top| {
                    self.body.try_resize(radius, top)
                })?;
            [0.0, 1.0, 0.0]
        } else if rising.is_some() {
            // Kind 13's slot 8: no ground probe and no transitions.
            self.posture.rise_hold();
            [0.0, 1.0, 0.0]
        } else {
            let start = self.ride_in_world(dt as f32);
            let [sx, sy, sz] = start.shift.map(f64::from);
            let [x, y, z] = self.pose.position;
            let probe = self.probe.probe(
                rays,
                self.body.world(),
                &settings,
                ProbePose {
                    feet: [x + sx, y + sy, z + sz],
                    velocity: self.pose.velocity,
                    spring: self.posture.spring_offset(),
                    crouched: self.posture.crouched(),
                    airborne: self.posture.ground() != Ground::Standing,
                },
                dt,
                self.category,
            )?;
            carry = self.reparent(probe.ground.and(probe.shape), start, dt as f32);
            self.posture.advance(
                PostureStep {
                    command: &command,
                    base: &self.base,
                    values: &settings,
                    mods: &active,
                    probe,
                    dt,
                    aim_held: input.aim_held,
                    driven: rewinding,
                },
                &mut self.pose.position,
                &mut self.pose.velocity,
                |radius, top| self.body.try_resize(radius, top),
            )?;
            probe.ground.map_or([0.0, 1.0, 0.0], |hit| hit.normal)
        };
        let outcome = if self.step_motion(rays, normal, dt)? {
            None
        } else if driven {
            self.drive(rays, &active, &settings, &changes, dt)?
        } else {
            let ground = self.posture.ground();
            let walked = self.walk.as_mut().map(|walk| {
                let cells = self.navigation.as_deref().and_then(Navigation::ai_cells);
                walk.step(cells, self.pose.position, input.forward(), dt)
            });
            // Kind 10's slot 11 replaces the velocity step (gravity
            // included) while it walks or has walked.
            let (before, stepped) = match walked {
                Some(path_walk::Velocity::Set(velocity)) => (velocity, false),
                Some(path_walk::Velocity::Keep) => (self.pose.velocity, false),
                None | Some(path_walk::Velocity::Default) => (
                    VelocityStep {
                        base: &self.base,
                        settings: &settings,
                        input,
                        ground,
                        crouched: self.posture.crouched(),
                        dt,
                    }
                    .advance(self.pose.velocity),
                    true,
                ),
            };
            let velocity =
                change_velocity(active.limit_velocity(before, settings.jump_speed), &changes);
            if stepped && ground != Ground::Standing {
                self.posture.apply_fall(settings.gravity_scale, dt);
            }
            if narrow(velocity[1] - before[1]) > TAKE_OFF {
                self.posture.take_off();
            }
            let grounded = self.posture.ground() == Ground::Standing;
            let vertical = match rising {
                Some(rise) => self.rise_to(rise, velocity, command.frame, dt)?,
                None => self.move_body(rays, velocity, normal, grounded, dt, carry)?,
            };
            self.posture.finish_fall(settings.gravity_scale, vertical);
            match self.walk.as_mut() {
                Some(walk) => walk.take_end(),
                None => self.move_to.as_mut().and_then(move_to::Run::take_end),
            }
        };
        if !self.rewinding() {
            self.record_history();
        }

        if input.throttles != [0; 2] {
            self.last_input = Some(command.frame);
            self.pose.input_throttles = input.throttles;
        }
        self.pose.frame = Some(command.frame);
        self.pose.input_frame = match self.last_input {
            Some(frame) if frame == command.frame => None,
            frame => Some(frame.unwrap_or(NO_INPUT)),
        };
        self.pose.throttles = input.throttles;
        self.pose.yaw = input.yaw;
        self.pose.pitch = i32::from(input.pitch);
        self.pose.flags = self.posture.flags
            | self.move_to.as_ref().map_or(0, move_to::Run::flag)
            | self.walk.as_ref().map_or(0, path_walk::Walk::flag)
            | self.turn.map_or(0, |_| TURNING);
        self.pose.move_to = self.move_to.as_ref().and_then(move_to::Run::record);
        self.pose.turn_rate = self.turn.as_ref().map(turn::Run::rate);
        self.pose.nav_path = self.walk.as_ref().map(path_walk::Walk::record);
        self.pose.air_ticks = self.posture.air_ticks;
        self.pose.crouch_frame = self.posture.crouch_end;
        self.pose.gravity = settings.gravity_scale;
        self.pose.spring = self.posture.spring_steps().map(i32::from);
        self.pose.spring_speed = f64::from(self.posture.spring_speed());
        self.pose.fall = self.posture.fall;
        Ok(outcome)
    }

    /// The behaviour the state hands over this frame: a new key begins it
    /// (a `MoveTo`'s or a path walk's begin takes a body on the ground off
    /// it and sets the air ticks, a `MoveTo`'s lateral count, to 0), none
    /// lets it go.
    fn begin_move(
        &mut self,
        request: Option<(MoveKey, &Behaviour)>,
    ) -> Result<(), HullContactError> {
        let running = |key: MoveKey, kind: u8| {
            request
                .is_some_and(|(requested, behaviour)| requested == key && behaviour.kind() == kind)
        };
        if let Some(run) = self.move_to.take_if(|run| !running(run.key, 1)) {
            self.pose.velocity = run.let_go(self.pose.velocity).map(snap);
        }
        // Kind 10's end (slot 5 `0x7FF789B53A40`) only clears its flag and
        // record; kind 7's (slot 5 `0x7FF789B53A10`) its flag.
        self.walk.take_if(|walk| !running(walk.key, 10));
        self.turn.take_if(|turn| !running(turn.key, 7));
        // Kind 13 begins and ends with nothing (slots 4, 5); each frame's
        // block replaces the last.
        self.rise = match request {
            Some((key, Behaviour::Rise(rise))) => Some((key, *rise)),
            _ => None,
        };
        let Some((key, behaviour)) = request else {
            return Ok(());
        };
        let begins = match behaviour {
            Behaviour::MoveTo(params) if self.move_to.is_none() => {
                let run = move_to::Run::start(
                    key,
                    *params,
                    move_to::Body {
                        feet: self.pose.position,
                        body: &self.body,
                        base: &self.base,
                        crouched: self.posture.crouched(),
                    },
                )?;
                let drives = run.drives();
                self.move_to = Some(run);
                drives
            }
            Behaviour::PathWalk(params) if self.walk.is_none() => {
                self.walk = Some(path_walk::Walk::start(key, *params, self.pose.position));
                true
            }
            Behaviour::Turn(params) if self.turn.is_none() => {
                self.turn = Some(turn::Run::start(key, *params));
                false
            }
            _ => false,
        };
        if begins {
            if self.posture.flags & IN_AIR == 0 {
                self.posture.take_off();
            }
            self.posture.air_ticks = 0;
        }
        Ok(())
    }

    /// A frame the move drives ([`move_to`]): its velocity, the velocity
    /// mods only on the frame it ends, and no gravity; the capsule moves by
    /// the velocity alone (the pogo spring is the behaviour's to skip), and
    /// the velocity loses what goes into the contact planes. The move's
    /// check after the capsule moved (slot 18) ends the frame, and gravity's
    /// part of the velocity follows the new velocity (0x7FF789B4F530).
    fn drive(
        &mut self,
        rays: &TriangleIndex,
        active: &ActiveModifiers,
        settings: &MoverSettings,
        changes: &[VelocityChange],
        dt: f64,
    ) -> Result<Option<MoveEnd>, HullContactError> {
        let run = self.move_to.as_mut().expect("a driving move");
        let mut velocity = run.step(
            self.pose.position,
            self.pose.velocity,
            &mut self.posture.air_ticks,
            dt,
        );
        if !run.drives() {
            velocity = change_velocity(
                active.limit_velocity(velocity, settings.jump_speed),
                changes,
            );
        }
        let [x, y, z] = velocity.map(|axis| axis as f32);
        let dt = dt as f32;
        let movement = self.body.move_capsule([x * dt, y * dt, z * dt], dt)?;
        let velocity = clip_velocity([x, y, z], movement.planes).map(f64::from);
        let (position, touching) = (movement.position, movement.touching);
        self.place(rays, position, touching);
        self.posture
            .finish_fall(settings.gravity_scale, velocity[1]);
        self.pose.velocity = velocity.map(snap);
        let run = self.move_to.as_mut().expect("a driving move");
        run.after_move(self.pose.position);
        Ok(run.take_end())
    }

    /// A frame Meteor Strike's flight moves ([`rise`]): the spring pinned
    /// to the pogo below the feet (slot 10), the capsule put where the
    /// search finds (slot 16) and the feet under it; the velocity stays as
    /// the step left it. The velocity's vertical part, as
    /// [`Mover::move_body`] gives it.
    fn rise_to(
        &mut self,
        rise: Rise,
        velocity: [f64; 3],
        frame: u32,
        dt: f64,
    ) -> Result<f64, HullContactError> {
        self.posture
            .pin_spring(-(pogo(&self.base, self.posture.crouched()) as f32));
        let travel = velocity.map(|axis| axis as f32);
        let at = rise.search(
            &mut self.body,
            self.category,
            self.zones.as_deref(),
            travel,
            dt as f32,
            frame,
        )?;
        let lift = self.lift();
        self.pose.position =
            [f64::from(at[0]), f64::from(at[1] - lift), f64::from(at[2])].map(snap);
        let [x, y, z] = self.pose.position;
        self.body
            .teleport([x as f32, (y + f64::from(lift)) as f32, z as f32]);
        self.pose.velocity = velocity.map(snap);
        Ok(self.pose.velocity[1])
    }

    /// The capsule's lowest point above the feet: the pogo plus the spring
    /// offset (`Mover._lift` in `ow174/game/mover.py`).
    fn lift(&self) -> f32 {
        pogo(&self.base, self.posture.crouched()) as f32 + self.posture.spring_offset()
    }

    /// Move the capsule by the velocity (along the ground when grounded)
    /// and the spring for `dt`, plus `carry` (the platform's carry,
    /// [`platform`]; the client's sweep displacement `ctx+112`).
    fn move_body(
        &mut self,
        rays: &TriangleIndex,
        velocity: [f64; 3],
        normal: [f64; 3],
        grounded: bool,
        dt: f64,
        carry: [f32; 3],
    ) -> Result<f64, HullContactError> {
        let [x, y, z] = velocity;
        let velocity = if grounded && (x * x + y * y) + z * z >= EPSILON {
            along_ground([x, z], normal)
        } else {
            velocity
        };
        let [x, y, z] = velocity.map(|axis| axis as f32);
        let dt = dt as f32;
        let rise = self.posture.spring_speed();
        let displacement = [
            ((0.0 * rise + x) * dt) + carry[0],
            ((rise + y) * dt) + carry[1],
            ((0.0 * rise + z) * dt) + carry[2],
        ];
        let movement = self.body.move_capsule(displacement, dt)?;
        let [mut x, mut y, mut z] = clip_velocity([x, y, z], movement.planes).map(f64::from);
        if grounded {
            let squared = (x * x + y * y) + z * z;
            let flat = x * x + z * z;
            if squared >= EPSILON && flat >= EPSILON {
                let factor = (squared / flat).sqrt();
                (x, y, z) = (x * factor, 0.0, z * factor);
            }
        }
        let (position, touching) = (movement.position, movement.touching);
        if self.place(rays, position, touching) {
            y = 0.0;
        }
        self.settle_velocity([x, y, z]);
        Ok(y)
    }

    /// Put the feet under the moved capsule, on the position grid and no lower
    /// than the abyss floor, and the capsule back over them; true when the
    /// feet were past that floor (`Mover._place` in `ow174/game/mover.py`).
    /// On a platform the grid is the platform's frame's ([`platform`]).
    fn place(&mut self, rays: &TriangleIndex, position: [f32; 3], touching: bool) -> bool {
        self.posture.flags = (self.posture.flags & !TOUCHING) | (u32::from(touching) * TOUCHING);
        let lift = self.lift();
        let [px, py, pz] = position;
        if let Some(feet) = self.place_on_platform([px, py - lift, pz]) {
            let [x, y, z] = feet;
            self.pose.position = feet.map(f64::from);
            self.body.teleport([x, y + lift, z]);
            return false;
        }
        let feet = f64::from(py - lift);
        let floor = rays.lowest() - ABYSS;
        self.pose.position = [f64::from(px), feet.max(floor), f64::from(pz)].map(snap);
        let [px, py, pz] = self.pose.position;
        self.body
            .teleport([px as f32, (py + f64::from(lift)) as f32, pz as f32]);
        feet < floor
    }
}

fn along_ground([x, z]: [f64; 2], [nx, ny, nz]: [f64; 3]) -> [f64; 3] {
    let speed = (x * x + z * z).sqrt();
    let along = x * nx + z * nz;
    let projected = [x - nx * along, -ny * along, z - nz * along];
    let (px, pz) = (-z, x);
    let length = (px * px + pz * pz).sqrt();
    if length * length < EPSILON || speed <= 0.0 {
        return projected;
    }
    let (px, pz) = (px / length, pz / length);
    let direction = [-pz * ny, pz * nx - px * nz, px * ny];
    let [dx, dy, dz] = direction;
    let norm = (dx * dx + dy * dy + dz * dz).sqrt();
    if norm <= 0.0 {
        return projected;
    }
    let sign = if (dx * projected[0] + dy * projected[1]) + dz * projected[2] < 0.0 {
        -norm
    } else {
        norm
    };
    direction.map(|axis| axis / sign * speed)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs::{self, File},
    };

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{
        collision::cache::read_movement_cache,
        collision::scene::CollisionScene,
        contact::round::{RoundShape, Sphere},
        contact::world::{StaticGeometry, StaticShape},
        mover::modifiers::Operation,
        mover::settings::MoverCatalog,
    };
    use lucy_wire::{
        commands::{CROUCH, JUMP},
        entity::movement_record,
    };

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

    fn idle(frame: u32) -> Command {
        Command {
            frame,
            ..Command::default()
        }
    }

    const MOVE: MoveKey = MoveKey {
        instance: 1,
        raw_state: 0,
        activation: 1,
    };

    /// A move to `destination` at 5 m/s with the grapple's other numbers
    /// (`005A` st1: 100 speeds a second, 0.3 s across, 1 m and 0.3 s stuck).
    fn move_to(destination: [f64; 3]) -> MoveTo {
        MoveTo {
            destination,
            surface: [0.0; 3],
            free_spot: false,
            speed: Speed::MetresPerSecond(5.0),
            acceleration: 100.0,
            slow_radius: 0.0,
            lateral_seconds: 0.3,
            stuck_distance: 1.0,
            stuck_seconds: 0.3,
            exit_horizontal: 0.0,
            exit_vertical: 0.0,
            exit_total: 0.0,
            exit_on_end: false,
            abort_past: false,
        }
    }

    /// Run `params` until the move ends; the ends, one a frame.
    fn drive(mover: &mut Mover, rays: &TriangleIndex, params: &MoveTo) -> Vec<Option<MoveEnd>> {
        let mut outcomes = Vec::new();
        for frame in 1..=100 {
            let outcome = mover
                .step_pose_to(
                    rays,
                    idle(frame),
                    0.016,
                    Some((MOVE, &Behaviour::MoveTo(*params))),
                )
                .unwrap();
            outcomes.push(outcome);
            if outcome.is_some() {
                break;
            }
        }
        outcomes
    }

    /// At 5 m/s the body goes 0.08 m a frame; the frame that would pass the
    /// target lands on it, and the next one arrives with the velocity held
    /// to the exit limits (0).
    #[test]
    fn a_move_lands_on_its_target_and_arrives_the_frame_after() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        let target = [1.0, 0.0, 0.0];
        let outcomes = drive(&mut mover, &rays, &move_to(target));
        // 12 frames of 0.08 m, the 13th lands, the 14th arrives.
        assert_eq!(outcomes.len(), 14);
        assert_eq!(outcomes.last(), Some(&Some(MoveEnd::Arrived)));
        assert_eq!(mover.pose().position, target);
        assert_eq!(mover.pose().velocity, [0.0; 3]);
    }

    /// With the exit limits unset (10000) the landing frame keeps the
    /// velocity, so the body goes about 0.04 m past the target (positions
    /// on the pose's grid): a grapple's move (`abort_past`) blocks on that
    /// frame, after the move; another arrives the frame after and keeps
    /// going.
    #[test]
    fn a_move_without_exit_limits_passes_its_target_and_a_grapple_blocks_there() {
        let rays = TriangleIndex::new([]).unwrap();
        let free = |abort_past| MoveTo {
            exit_horizontal: 10_000.0,
            exit_vertical: 10_000.0,
            exit_total: 10_000.0,
            abort_past,
            ..move_to([1.0, 0.0, 0.0])
        };
        let mut grapple = mover_in(Vec::new());
        let outcomes = drive(&mut grapple, &rays, &free(true));
        assert_eq!(outcomes.len(), 13);
        assert_eq!(outcomes.last(), Some(&Some(MoveEnd::Blocked)));
        assert_eq!(grapple.pose().velocity, [5.0, 0.0, 0.0]);
        let past = grapple.pose().position[0];
        assert!((past - 1.04).abs() < 0.005, "{past}");

        let mut other = mover_in(Vec::new());
        let outcomes = drive(&mut other, &rays, &free(false));
        assert_eq!(outcomes.len(), 14);
        assert_eq!(outcomes.last(), Some(&Some(MoveEnd::Arrived)));
        assert_eq!(other.pose().velocity, [5.0, 0.0, 0.0]);
        let past = other.pose().position[0];
        assert!((past - 1.12).abs() < 0.005, "{past}");
    }

    /// A body held within the stuck distance of its last checkpoint for the
    /// stuck time blocks when the target is farther than that distance.
    #[test]
    fn a_move_held_by_a_wall_blocks_after_its_stuck_time() {
        let rays = TriangleIndex::new([]).unwrap();
        let wall = StaticGeometry::Round {
            shape: RoundShape::Sphere(Sphere {
                center: [0.7, 1.0, 0.0],
                radius: 0.3,
            }),
            mask: 0x18,
        };
        let mut mover = mover_in(vec![StaticShape::new(wall, None).unwrap()]);
        let outcomes = drive(&mut mover, &rays, &move_to([3.0, 0.0, 0.0]));
        assert_eq!(outcomes.last(), Some(&Some(MoveEnd::Blocked)));
        // The checkpoint is the start (never 1 m away): 0.3 s is 19 frames.
        assert_eq!(outcomes.len(), 19);
        assert!(mover.pose().position[0] < 0.4);
    }

    /// The velocity across the direction goes over `m_7F3716A6` seconds in
    /// equal parts: 3 m/s over three frames leaves 2, 1 and 0 (the target
    /// far enough that the sideways drift doesn't turn the direction).
    #[test]
    fn a_move_drops_the_velocity_across_it_in_equal_parts() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        let params = MoveTo {
            lateral_seconds: 0.048,
            ..move_to([1000.0, 0.0, 0.0])
        };
        mover.push([0.0, 0.0, 3.0]);
        let mut across = Vec::new();
        for frame in 1..=4 {
            mover
                .step_pose_to(
                    &rays,
                    idle(frame),
                    0.016,
                    Some((MOVE, &Behaviour::MoveTo(params))),
                )
                .unwrap();
            across.push(mover.pose().velocity[2]);
        }
        assert_eq!(across, [2.0, 1.0, 0.0, 0.0]);
    }

    /// A key the mover hasn't seen begins a new move; no request lets the
    /// move go and the pose runs as usual (here: falls).
    #[test]
    fn a_new_key_begins_a_move_and_none_lets_it_go() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        let params = Behaviour::MoveTo(move_to([0.0, 0.0, 5.0]));
        mover
            .step_pose_to(&rays, idle(1), 0.016, Some((MOVE, &params)))
            .unwrap();
        assert_eq!(mover.pose().velocity, [0.0, 0.0, 5.0]);
        mover.step_pose_to(&rays, idle(2), 0.016, None).unwrap();
        assert!(mover.move_to.is_none());
        assert!(mover.pose().velocity[1] < 0.0);
        let again = MoveKey {
            activation: 2,
            ..MOVE
        };
        mover
            .step_pose_to(&rays, idle(3), 0.016, Some((again, &params)))
            .unwrap();
        assert_eq!(mover.move_to.as_ref().map(|run| run.key), Some(again));
    }

    #[test]
    fn pending_poses_step_through_the_callback_and_keep_the_newest() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        let mut frames = Vec::new();
        for newest in [5, 8] {
            let mut pending = mover.pending_poses(idle(newest));
            mover
                .step_pending_with(&mut pending, usize::MAX, |mover, command| {
                    frames.push(command.frame);
                    let params = Behaviour::MoveTo(move_to([1.0, 0.0, 0.0]));
                    mover
                        .step_pose_to(&rays, command, 0.016, Some((MOVE, &params)))
                        .map(drop)
                })
                .unwrap();
        }
        assert_eq!(frames, [5, 6, 7, 8]);
        assert_eq!(mover.last_command.map(|command| command.frame), Some(8));
        assert!(mover.pose().position[0] > 0.3);
    }

    #[test]
    fn owner_movement_sample_covers_fresh_and_live_poses() {
        let mut pose = Movement::default();
        assert_eq!(sample_movement_flags(&pose), Some(1 << 1));

        pose.frame = Some(20);
        assert_eq!(sample_movement_flags(&pose), Some(1 << 1));
        pose.flags = CROUCHED | NOT_GROUND_UNDER;
        assert_eq!(
            sample_movement_flags(&pose),
            Some((1 << 1) | (1 << 6) | (1 << 9))
        );
        pose.flags = JUMPED;
        pose.velocity = [0.0, 2.0, 0.0];
        pose.throttles = [0, -127];
        assert_eq!(
            sample_movement_flags(&pose),
            Some(1 | (1 << 2) | (1 << 4) | (1 << 5) | (1 << 11))
        );
        pose.flags = 0x800000;
        pose.velocity[1] = -2.0;
        pose.throttles = [0; 2];
        assert_eq!(sample_movement_flags(&pose), Some(1 | (1 << 3) | (1 << 4)));
        pose.flags |= 0x80;
        assert_eq!(sample_movement_flags(&pose), None);
    }

    // The historical Python oracle used a scalar clamp, not B4F530's
    // float32 normalization. Keep that oracle for the complete wire record;
    // separately check the actual native fall at every pose, without a
    // CPU-specific whole-record hash or a tolerance.
    fn python_projected_fall(fall: f32, input: &posture::FallProjection) -> f32 {
        if fall.abs() <= f32::EPSILON {
            return fall;
        }
        if input.landed {
            return 0.0;
        }
        if input.airborne {
            let gravity = f64::from((-kinematics::WORLD_GRAVITY * input.gravity_scale) as f32);
            if gravity * input.vertical < 0.0 {
                return 0.0;
            }
            if input.vertical * 1.0_f64.copysign(f64::from(fall)) < f64::from(fall.abs()) {
                return input.vertical as f32;
            }
        }
        fall
    }

    fn native_projected_fall(fall: f32, input: &posture::FallProjection) -> f32 {
        let squared = fall * fall;
        if squared <= crate::contact::math::TINY {
            return fall;
        }
        #[cfg(target_arch = "x86_64")]
        let estimate = lucy_simd::reciprocal_sqrt_estimate(squared);
        #[cfg(not(target_arch = "x86_64"))]
        let estimate = (1.0 / f64::from(squared).sqrt()) as f32;
        let inverse = (estimate * 0.5) * (3.0 - squared * (estimate * estimate));
        let length = squared * inverse;
        if length.abs() <= f32::EPSILON {
            return fall;
        }
        if input.landed {
            return 0.0;
        }
        if input.airborne {
            let vertical = input.vertical as f32;
            let gravity = (-kinematics::WORLD_GRAVITY * input.gravity_scale) as f32;
            if gravity * vertical < 0.0 {
                return 0.0;
            }
            let unit = fall * inverse;
            let projected = vertical * unit;
            if projected < length {
                return unit * projected;
            }
        }
        fall
    }

    #[test]
    #[ignore = "requires a derived 104319 Practice Range movement cache"]
    fn practice_range_player_movement_records_match_python() {
        let runtime = lucy_testkit::runtime_dir();
        let cache =
            read_movement_cache(File::open(runtime.join("collision/0688.col")).unwrap()).unwrap();
        let ids = BTreeSet::new();
        let scene = CollisionScene::movement_probe(&cache, &ids).unwrap();
        let world = Arc::new(ContactWorld::from_movement_cache(cache, &ids).unwrap());
        let catalog_path = runtime.join("data/game_movers_174.json");
        // mover.py `SOLDIER_BODY`, the fallback body.
        let settings = MoverCatalog::load(&catalog_path, 0x0400_0000_0000_03cf).unwrap();
        let records: BTreeMap<String, serde_json::Value> =
            serde_json::from_slice(&fs::read(catalog_path).unwrap()).unwrap();
        assert_eq!(records.len(), 38);
        let sprint = BTreeMap::from([(
            ModifierKey {
                instance: 1,
                state: 1,
                slot: 0,
            },
            Modifier {
                index: 0,
                operation: Operation::Multiply,
                value: 1.5,
                priority: 0.0,
            },
        )]);
        let released = BTreeMap::new();
        let mut combined = Sha256::new();
        for (i, guid) in records.keys().enumerate() {
            let body = lucy_core::guid::parse_guid(guid).unwrap();
            let spawn = if i % 3 == 2 {
                [45.5, 2.5, 34.75]
            } else {
                [54.56507, 1.0, 42.14788]
            };
            let category = [0x08, 0x10, 0x18][i % 3];
            let mut mover = Mover::new(
                *settings.for_body(Some(body)),
                Arc::clone(&world),
                spawn,
                -31010,
                category,
            )
            .unwrap();
            let first = 4000 + i as u32 * 500;
            let mut digest = Sha256::new();
            let mut python_fall = 0.0_f32;
            let mut native_fall = 0.0_f32;
            let mut visited = 0;
            for n in 0..304 {
                if (20..24).contains(&n) || (80..118).contains(&n) || (210..278).contains(&n) {
                    continue;
                }
                let jump = (24..29).contains(&n) || (138..153).contains(&n);
                let crouch = (45..75).contains(&n) || (121..136).contains(&n);
                let command = Command {
                    frame: first + n,
                    forward: [127, -127, 58, 0][(n as usize / 16) % 4],
                    right: [0, 0, -127, 111][(n as usize / 20) % 4],
                    yaw: (n as i32 / 12) * 4096 - 31010,
                    pitch: (n as i32 % 28) * 2500 - 32000,
                    buttons: (u32::from(jump) * JUMP) | (u32::from(crouch) * CROUCH),
                    ..Command::default()
                };
                mover
                    .step(scene.triangles(), command, 0.016, |mover, pose| {
                        visited += 1;
                        let record = movement_record(&mover.pose(), Some(pose.frame + 2)).unwrap();
                        digest.update((record.bit_len() as u32).to_le_bytes());
                        digest.update(record.as_bytes());
                        let active = if (12..35).contains(&((pose.frame - first) % 72)) {
                            &sprint
                        } else {
                            &released
                        };
                        mover.track_modifiers(pose.frame, active);
                    })
                    .unwrap();
            }
            assert_eq!(visited, 236);
            combined.update(body.to_le_bytes());
            combined.update(digest.finalize());
        }
        assert_eq!(
            format!("{:x}", combined.finalize()),
            "e2fcdb617ec2f5429ea6c54616de11ac20352e46801f0c9f51664818a13b44cf"
        );
    }
}
