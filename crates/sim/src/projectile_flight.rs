//! Server-side projectile flight and splash for build 104319.
//!
//! Uses the swept target bodies and map ray from the match. Damage and
//! network delivery are separate so a predicted alias never hits twice.
//! The reference is ow174/game/combat.py (`_fly`), which flew every
//! projectile straight and ended it where it hit. Beyond it (no reference;
//! the values are the volley's, see [`ProjectileBallistics`]): a projectile
//! with gravity flies an arc, swept as one straight chord a tick at most,
//! and one that sticks stays on the body or surface it hit, moving with the
//! body, until its fuse ends and it explodes there. A body with hit shapes
//! is met by its shapes, as a shot meets it ([`TargetPose::entry`]): the
//! sweep and an explosion's distance; others by the reference's capsule. A
//! direct hit on the head is critical as a shot's is ([`TargetPose::head`];
//! the reference did so for Mei's icicle alone, `combat.py`
//! `projectile_headshot`): Hanzo's arrows and Genji's shuriken score
//! headshots, a rocket whose hit says 1 doesn't. A projectile meets bodies
//! as a sphere of its volley's radius ([`ProjectileBallistics::radius`],
//! 0.2 m for Helix Rockets) and surfaces as a point; the reference swept
//! points. An explosion also hurts and pushes its own shooter
//! ([`ProjectileFlight::own_blast`]), and reaches no body it can't see
//! past the map (the client's sight test of its own push; the reference's
//! splash went through walls). A flight ends at its volley's lifetime or
//! its range along its path, whichever comes first. A projectile with a
//! bounce block (`lucy_script::weapons::Bounce`: Moira's Biotic Orbs, Junkrat's
//! grenades) bounces off the map's surfaces as the client's does: its
//! velocity's part into the surface turns back, scaled by the
//! restitution, and it flies the rest of the tick from 1 mm off the
//! surface (the client's least push, `0x7FF789C87B80`; assumption: the
//! push it gives a projectile with a collision radius isn't modelled). A
//! bounce off a body isn't modelled: a body ends it, as before.

use crate::hitscan::{Impact, TargetPose};
use lucy_core::tick::TICK_PERIOD;
use lucy_physics::{collision::index::IndexError, static_ray::Vec3};
use lucy_script::{
    vm::CombatMod,
    weapons::{DamageTags, HitDefinition, ProjectileBallistics},
};

/// Where a sweep meets the map: how far along it, and the surface's
/// normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallHit {
    pub distance: f64,
    pub normal: Vec3,
}

/// A ray against the map's collision: from an origin along a unit
/// direction for a reach, with a ray profile whose map mask it meets the
/// map with (the volley's for the flight, the explosion's for its sight
/// test); where it first meets the map.
pub trait MapRay:
    FnMut(Vec3, Vec3, f64, Option<u64>) -> Result<Option<WallHit>, IndexError>
{
}

impl<T: FnMut(Vec3, Vec3, f64, Option<u64>) -> Result<Option<WallHit>, IndexError>> MapRay for T {}

/// How far back along its flight an explosion's sight rays start: 0.2 m
/// (`0x7FF789A433F0`: the projectile's position less 0.2 × its direction,
/// `0x7FF78B565B80`), so they leave a surface it met from the side it came.
const SIGHT_BACK: f64 = 0.2;

/// How far a bounce puts the projectile off the surface (the client's
/// least push, 0.001 m).
const BOUNCE_PUSH: f64 = 0.001;
/// The most bounces in one piece of a tick's flight (a projectile wedged
/// in a corner stops bouncing there for the tick).
const MAX_BOUNCES_A_PIECE: usize = 8;

pub struct ProjectileFlight {
    ballistics: ProjectileBallistics,
    position: Vec3,
    /// Where it flies (a unit vector): the launch direction, then the
    /// direction its last bounce left along.
    direction: Vec3,
    /// Its speed along `direction`, m/s: the volley's, then its last
    /// bounce's.
    speed: f64,
    /// The downward speed gravity added so far, m/s.
    fall: f64,
    remaining: f64,
    /// Seconds flown so far (its fuse included once it stuck).
    travelled: f64,
    /// Metres flown so far, along its path ([`ProjectileBallistics::range`]).
    flown: f64,
    last_tick: Option<u32>,
    stuck: Option<Stuck>,
    ended: bool,
    /// It ended with its explosion.
    exploded: bool,
    /// It hurt an enemy (`Some`, whether critically), for its shooter's
    /// shot stats ([`ProjectileFlight::strike`]).
    struck: Option<bool>,
    /// Its shooter's dealt modifiers at its launch, which its hits deal
    /// with ([`ProjectileFlight::launch`]); none before its first step.
    launched: Option<Vec<CombatMod>>,
    /// Its sweep passes the bodies ([`ProjectileFlight::pass_bodies`]).
    passes_bodies: bool,
    /// The surfaces it bounced off so far.
    bounces: u32,
    landing_armed: bool,
    /// The scale of its speed its entity's graphs set
    /// ([`ProjectileFlight::set_speed_scale`]); 1 otherwise.
    speed_scale: f64,
}

/// A projectile that stuck: to a body (`entity`) at `offset` from its feet,
/// or where it hit a surface; and the seconds its fuse has left.
#[derive(Clone, Copy, Debug)]
struct Stuck {
    entity: Option<u32>,
    offset: Vec3,
    fuse: f64,
}

/// One advance: the damage it did (the direct hit of a projectile that
/// stuck, or the direct hit and explosion of one that ended), and whether
/// it ended.
#[derive(Debug, Default, PartialEq)]
pub struct FlightStep {
    pub impacts: Vec<Impact>,
    pub end: Option<FlightEnd>,
}

#[derive(Debug, PartialEq)]
pub struct FlightEnd {
    pub point: Vec3,
    /// It ended with its explosion: an impact, sticking fuse, or a
    /// range/lifetime expiry whose authored explosion enables it.
    pub exploded: bool,
    /// Seconds from the launch to the end, a fuse included.
    pub travelled: f64,
    /// The velocity its explosion adds to its shooter's own body
    /// ([`ProjectileFlight::own_blast`]); the match fills it in.
    pub knockback: Option<Vec3>,
}

/// What an explosion does to its shooter's own body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnBlast {
    /// From the explosion to the body's surface.
    pub distance: f64,
    pub damage: f64,
    pub loadout: Option<u64>,
    /// The explosion's damage tags, which the body's damage message
    /// carries.
    pub tags: DamageTags,
    /// The way its damage travels: the line from the explosion to the
    /// body's centre ([`ProjectileFlight::blast_push`]'s).
    pub direction: Vec3,
    pub knockback: Option<Vec3>,
}

impl ProjectileFlight {
    pub fn new(
        ballistics: ProjectileBallistics,
        position: Vec3,
        direction: Vec3,
        fired_at: Option<u32>,
    ) -> Option<Self> {
        if !ballistics.speed.is_finite()
            || ballistics.speed <= 0.0
            || !ballistics.lifetime.is_finite()
            || ballistics.lifetime <= 0.0
            || !(ballistics.speed * ballistics.lifetime).is_finite()
            || !ballistics.gravity.is_finite()
            || ballistics
                .fuse
                .is_some_and(|fuse| !fuse.is_finite() || fuse < 0.0)
            || !position.into_iter().chain(direction).all(f64::is_finite)
            || (direction.iter().map(|value| value * value).sum::<f64>() - 1.0).abs() > 1e-6
        {
            return None;
        }
        Some(Self {
            remaining: ballistics.lifetime,
            speed: ballistics.speed,
            ballistics,
            position,
            direction,
            fall: 0.0,
            travelled: 0.0,
            flown: 0.0,
            last_tick: fired_at,
            stuck: None,
            ended: false,
            exploded: false,
            struck: None,
            launched: None,
            passes_bodies: false,
            bounces: 0,
            landing_armed: false,
            speed_scale: 1.0,
        })
    }

    /// Its shooter's dealt modifiers (`0317`, `0319`) as he held them at
    /// its launch: the client copies them into a projectile at its
    /// creation (`sub_7FF789A32900`, `refs/reports/HealthAuras.md` "Aura
    /// modifiers"), so a boost that ends in flight still counts for its
    /// hits, and one that begins doesn't. Kept once; later calls don't
    /// change them.
    pub fn launch(&mut self, modifiers: impl FnOnce() -> Vec<CombatMod>) {
        if self.launched.is_none() {
            self.launched = Some(modifiers());
        }
    }

    /// The modifiers [`Self::launch`] kept.
    pub fn launch_modifiers(&self) -> Option<&[CombatMod]> {
        self.launched.as_deref()
    }

    /// Whether it does damage where it hits or explodes.
    pub fn damaging(&self) -> bool {
        self.ballistics.damaging()
    }

    /// One of its impacts struck an enemy, critically or not; it hit with
    /// any, critically with any critical one.
    pub fn strike(&mut self, critical: bool) {
        self.struck = Some(self.struck.unwrap_or(false) || critical);
    }

    /// Whether it struck an enemy so far, and whether critically.
    pub fn struck(&self) -> Option<bool> {
        self.struck
    }

    /// Where it is now.
    pub fn position(&self) -> Vec3 {
        self.position
    }

    /// Its entity definition (`m_projectileEntity`), if the volley names
    /// one.
    pub fn definition(&self) -> Option<u64> {
        self.ballistics.definition
    }

    /// The direct hit of the projectile, if it has one.
    pub fn direct(&self) -> Option<HitDefinition> {
        self.ballistics.direct
    }

    /// The volley's ray profile, whose map mask the sweep meets the map with.
    pub fn ray_profile(&self) -> Option<u64> {
        self.ballistics.ray_profile
    }

    /// Its sweep passes the bodies: its ray profile meets none
    /// (`hitscan::RayProfiles::meets_bodies`). Its explosion still reaches
    /// them.
    pub fn pass_bodies(&mut self) {
        self.passes_bodies = true;
    }

    /// Its entity's graphs scale its speed (`STU_D3F71FCF`: a Biotic Orb
    /// flies at 0.275 while it has targets, assumption): its next steps
    /// fly that much slower; its bounces keep their own speed. Its
    /// lifetime runs as before.
    pub fn set_speed_scale(&mut self, scale: f64) {
        if scale.is_finite() && scale > 0.0 {
            self.speed_scale = scale;
        }
    }

    /// Advance the elapsed portion of a tick, even when it ends inside the
    /// tick due to range, lifetime or fuse. Static geometry wins only a
    /// closer hit.
    pub fn advance(
        &mut self,
        tick: u32,
        targets: &[(u32, TargetPose)],
        mut wall: impl MapRay,
    ) -> Result<FlightStep, IndexError> {
        let mut step = FlightStep::default();
        if self.ended {
            return Ok(step);
        }
        let elapsed = self.last_tick.map_or(1, |last| tick.saturating_sub(last));
        if elapsed == 0 {
            return Ok(step);
        }
        self.last_tick = Some(tick);
        let mut time = f64::from(elapsed) * TICK_PERIOD.as_secs_f64();
        if self.stuck.is_none() {
            time = self.fly(time, targets, &mut wall, &mut step)?;
        }
        if self.stuck.is_some() && step.end.is_none() {
            self.hold(time, targets, &mut wall, &mut step)?;
        }
        Ok(step)
    }

    /// Fly up to `time` seconds; what is left of them once it stuck. It
    /// ends where its lifetime or its range along its path runs out, a
    /// sweep cut there in proportion, as the client cuts its step
    /// (`0x7FF789C82C80`).
    fn fly(
        &mut self,
        time: f64,
        targets: &[(u32, TargetPose)],
        wall: &mut impl MapRay,
        step: &mut FlightStep,
    ) -> Result<f64, IndexError> {
        let duration = time.min(self.remaining);
        let gravity = self.ballistics.gravity;
        let profile = self.ballistics.ray_profile;
        // A straight flight sweeps the whole duration at once (combat.py
        // `_fly`); an arc a chord of at most one tick at a time.
        let pieces = if gravity == 0.0 {
            1
        } else {
            (duration / TICK_PERIOD.as_secs_f64()).ceil().max(1.0) as u32
        };
        let piece = duration / f64::from(pieces);
        let mut spent = 0.0;
        for _ in 0..pieces {
            // A bounce flies the rest of the piece from where it bounced.
            let mut left = piece;
            for _ in 0..=MAX_BOUNCES_A_PIECE {
                left = left.min(self.remaining.max(0.0));
                let (direction, mut reach) = self.chord(left);
                let mut span = left;
                let rest = self
                    .ballistics
                    .range
                    .map_or(f64::INFINITY, |range| (range - self.flown).max(0.0));
                let out_of_range = reach >= rest;
                if out_of_range {
                    span = if reach > 0.0 {
                        left * rest / reach
                    } else {
                        0.0
                    };
                    reach = rest;
                }
                let hit = if reach > 0.0 {
                    closest(
                        self.position,
                        direction,
                        reach,
                        self.ballistics.radius,
                        if self.passes_bodies { &[] } else { targets },
                        |origin, direction, reach| wall(origin, direction, reach, profile),
                    )?
                } else {
                    None
                };
                let (distance, seconds) = match hit {
                    Some((distance, _)) if gravity == 0.0 => {
                        (distance, distance / (self.speed * self.speed_scale))
                    }
                    Some((distance, _)) => (distance, span * distance / reach),
                    None => (reach, span),
                };
                for (position, direction) in self.position.iter_mut().zip(direction) {
                    *position += direction * distance;
                }
                self.fall += gravity * seconds;
                self.travelled += seconds;
                self.flown += distance;
                self.remaining -= seconds;
                spent += seconds;
                left -= seconds;
                match hit {
                    None if out_of_range => {
                        self.expire(targets, wall, step)?;
                        return Ok(0.0);
                    }
                    None => break,
                    Some((_, Met::Surface(normal))) if self.bounce(normal, direction) => {}
                    Some((distance, met)) => {
                        let struck = match met {
                            Met::Body(entity, entered) => Some((entity, entered)),
                            Met::Surface(_) => None,
                        };
                        self.hit(distance, struck, targets, wall, step)?;
                        return Ok((time - spent).max(0.0));
                    }
                }
                if left <= 0.0 {
                    break;
                }
            }
            if self.remaining <= 0.0 {
                self.expire(targets, wall, step)?;
                return Ok(0.0);
            }
        }
        if self.remaining <= 0.0 {
            self.expire(targets, wall, step)?;
        }
        Ok(0.0)
    }

    /// The straight sweep of its next `seconds`: along its flight, or the
    /// chord of its arc; its direction and length.
    fn chord(&self, seconds: f64) -> (Vec3, f64) {
        let gravity = self.ballistics.gravity;
        let speed = self.speed * self.speed_scale;
        if gravity == 0.0 {
            return (self.direction, speed * seconds);
        }
        let mut chord = self.direction.map(|part| part * speed * seconds);
        chord[1] -= (self.fall + 0.5 * gravity * seconds) * seconds;
        let length = chord.iter().map(|part| part * part).sum::<f64>().sqrt();
        (
            chord.map(|part| part / length.max(f64::MIN_POSITIVE)),
            length,
        )
    }

    /// It met a surface of `normal` flying along `along` (where it is now):
    /// whether it bounces off (`Bounce::flies_on`). A bounce turns its
    /// velocity's part into the surface back, scaled by the restitution
    /// (`0x7FF789C87B80`: `v − (1 + e)(v·n)n`), and puts it 1 mm off the
    /// surface; the hit that doesn't bounce ends it there.
    fn bounce(&mut self, normal: Vec3, along: Vec3) -> bool {
        let Some(bounce) = self.ballistics.bounce else {
            return false;
        };
        // The surface's side toward the projectile.
        let normal = if dot(normal, along) > 0.0 {
            normal.map(|part| -part)
        } else {
            normal
        };
        self.bounces += 1;
        if !bounce.flies_on(bounce.surface(normal), self.bounces) {
            return false;
        }
        let mut velocity = self.direction.map(|part| part * self.speed);
        velocity[1] -= self.fall;
        let into = dot(velocity, normal);
        if into < 0.0 {
            let scale = (1.0 + bounce.restitution) * into;
            if bounce.tangent_deceleration > 0.0 || (bounce.tangent_scale - 1.0).abs() > 0.004 {
                let tangent =
                    std::array::from_fn::<_, 3, _>(|axis| velocity[axis] - into * normal[axis]);
                let tangent_speed = dot(tangent, tangent).sqrt();
                let retained = if tangent_speed > 0.0 {
                    let slowed = (tangent_speed
                        - bounce.tangent_deceleration.max(0.0) * TICK_PERIOD.as_secs_f64())
                    .max(0.0)
                        / tangent_speed;
                    if (bounce.tangent_scale - 1.0).abs() > 0.004 {
                        slowed * bounce.tangent_scale
                    } else {
                        slowed
                    }
                } else {
                    0.0
                };
                for axis in 0..3 {
                    velocity[axis] =
                        tangent[axis] * retained - bounce.restitution * into * normal[axis];
                }
            } else {
                for axis in 0..3 {
                    velocity[axis] -= scale * normal[axis];
                }
            }
        }
        let speed = dot(velocity, velocity).sqrt();
        if !speed.is_finite() {
            return false;
        }
        self.speed = speed;
        self.direction = if speed > 0.0 {
            velocity.map(|part| part / speed)
        } else {
            normal
        };
        self.fall = 0.0;
        for (position, normal) in self.position.iter_mut().zip(normal) {
            *position += BOUNCE_PUSH * normal;
        }
        self.arm_landing(normal);
        true
    }

    /// `0x7FF789C83CB0` arms only once, after the bounce, below the
    /// configured speed. Its anchor is the later of this command frame
    /// and launch + minimum age. `0x7FF789C7B1E0` offsets elapsed ticks by
    /// trunc((original lifetime - landing lifetime) / tick period).
    fn arm_landing(&mut self, normal: Vec3) {
        let (Some(landing), Some(bounce)) = (self.ballistics.landing, self.ballistics.bounce)
        else {
            return;
        };
        if self.landing_armed
            || !landing.surfaces[bounce.surface(normal) as usize]
            || self.speed > landing.speed
        {
            return;
        }
        self.landing_armed = true;
        let tick = TICK_PERIOD.as_secs_f64();
        let frame_age = (self.travelled / tick - 1e-9).ceil() * tick;
        let minimum_age = (landing.minimum_age / tick).trunc() * tick;
        let offset = ((self.ballistics.lifetime - landing.lifetime) / tick).trunc() * tick;
        let deadline = frame_age.max(minimum_age) + self.ballistics.lifetime - offset;
        self.remaining = self.remaining.min((deadline - self.travelled).max(0.0));
    }

    fn expire(
        &mut self,
        targets: &[(u32, TargetPose)],
        wall: &mut impl MapRay,
        step: &mut FlightStep,
    ) -> Result<(), IndexError> {
        if self
            .ballistics
            .blast
            .as_ref()
            .is_some_and(|blast| blast.on_expiry)
        {
            self.explode(targets, wall, step)
        } else {
            step.end = Some(self.finish());
            Ok(())
        }
    }

    /// What it hit `distance` into this sweep (the body and how it entered
    /// it, [`TargetPose::entry`]), now at its position: the direct hit's
    /// damage to a struck body, then it sticks, or it ends with its
    /// explosion. The direct hit falls off with the distance flown along
    /// its path (leafall903/ORPHEUS (MIT) `combat.py` `_fly`, commit
    /// b3c5138: Doomfist's 6 damage is 3.9 at 23 m and 1.8 at 35 m; there
    /// the seconds flown times the speed, which an arc or a slowed flight
    /// overstates), and scores its critical multiplier on the head.
    fn hit(
        &mut self,
        distance: f64,
        target: Option<Struck>,
        targets: &[(u32, TargetPose)],
        wall: &mut impl MapRay,
        step: &mut FlightStep,
    ) -> Result<(), IndexError> {
        let entity = target.map(|(entity, _)| entity);
        if let (Some((entity, entered)), Some(direct)) = (
            target,
            self.ballistics.direct.filter(|hit| hit.amount > 0.0),
        ) {
            let head =
                pose_of(targets, entity).is_some_and(|pose| pose.head(entered, self.position[1]));
            step.impacts.push(Impact {
                entity,
                distance,
                damage: direct.damage_at(self.flown, head),
                critical: head && direct.critical > 1.0,
                loadout: direct.loadout,
                tags: direct.tags,
                heal: direct.ally_heal(),
            });
        }
        let Some(fuse) = self.ballistics.fuse else {
            return self.explode(targets, wall, step);
        };
        let offset = entity
            .and_then(|entity| pose_of(targets, entity))
            .map_or([0.0; 3], |pose| {
                std::array::from_fn(|axis| self.position[axis] - pose.feet[axis])
            });
        self.stuck = Some(Stuck {
            entity,
            offset,
            fuse,
        });
        Ok(())
    }

    /// `time` seconds stuck: on the body where it still is a target, until
    /// the fuse ends.
    fn hold(
        &mut self,
        time: f64,
        targets: &[(u32, TargetPose)],
        wall: &mut impl MapRay,
        step: &mut FlightStep,
    ) -> Result<(), IndexError> {
        let Some(stuck) = &mut self.stuck else {
            return Ok(());
        };
        if let Some(pose) = stuck.entity.and_then(|entity| pose_of(targets, entity)) {
            self.position = std::array::from_fn(|axis| pose.feet[axis] + stuck.offset[axis]);
        }
        if time < stuck.fuse {
            stuck.fuse -= time;
            self.travelled += time;
            return Ok(());
        }
        self.travelled += stuck.fuse;
        stuck.fuse = 0.0;
        self.explode(targets, wall, step)
    }

    /// The explosion at its position (combat.py `_fly`: every target's
    /// distance to the capsule, here to its shapes when it has them, scales
    /// the damage), and the end. A body the explosion can't see takes
    /// nothing ([`Self::in_sight`]; beyond combat.py, which hurt bodies
    /// behind walls).
    fn explode(
        &mut self,
        targets: &[(u32, TargetPose)],
        wall: &mut impl MapRay,
        step: &mut FlightStep,
    ) -> Result<(), IndexError> {
        self.exploded = true;
        if let Some(blast) = self
            .ballistics
            .blast
            .as_ref()
            .filter(|blast| blast.hit.amount > 0.0 && blast.aura.is_none())
        {
            for (entity, pose) in targets {
                let entity = *entity;
                let distance = pose.surface_distance(self.position);
                let damage = blast.hit.amount * blast.multiplier(distance);
                if damage > 0.0 && self.in_sight(pose, wall)? {
                    step.impacts.push(Impact {
                        entity,
                        distance,
                        damage,
                        critical: false,
                        loadout: blast.hit.loadout,
                        tags: blast.hit.tags,
                        heal: 0.0,
                    });
                }
            }
        }
        step.end = Some(self.finish());
        Ok(())
    }

    fn finish(&mut self) -> FlightEnd {
        self.ended = true;
        FlightEnd {
            point: self.position,
            exploded: self.exploded,
            travelled: self.travelled,
            knockback: None,
        }
    }

    /// Its explosion on its shooter's own body at `own`, once it exploded
    /// and when it sees him ([`Self::in_sight`]): the blast's damage at the
    /// body's distance times the explosion's self-damage scale, and its
    /// push ([`Self::push_at`]) times the explosion's self-push scale
    /// ([`lucy_script::weapons::Blast::self_knockback`]; none without one),
    /// only while the body is inside the explosion's damage rings, the
    /// outer radius left out: the client's own push (`0x7FF789A433F0`)
    /// gives nothing past the last ring whose radius the distance is below.
    /// Assumption: the client measures that distance from the sight rays'
    /// origin to the body's axis less its radius; the server measures it
    /// as it does the damage's. Beyond Python, which never hurt or pushed
    /// the shooter.
    pub fn own_blast(
        &self,
        own: &TargetPose,
        mut wall: impl MapRay,
    ) -> Result<Option<OwnBlast>, IndexError> {
        let Some(blast) = self.ballistics.blast.as_ref().filter(|_| self.exploded) else {
            return Ok(None);
        };
        let distance = own.surface_distance(self.position);
        let amount = if blast.aura.is_some() {
            0.0
        } else {
            blast.hit.amount.max(0.0)
        };
        let damage = amount * blast.multiplier(distance) * blast.self_damage;
        let inside = blast.rings.iter().any(|&(radius, _)| distance < radius);
        let knockback = inside
            .then(|| self.push_at(own, blast.self_knockback))
            .flatten();
        if !(damage > 0.0 || knockback.is_some()) || !self.in_sight(own, &mut wall)? {
            return Ok(None);
        }
        Ok(Some(OwnBlast {
            distance,
            damage,
            loadout: blast.hit.loadout,
            tags: blast.hit.tags,
            direction: self.blast_line(own),
            knockback,
        }))
    }

    /// The aura its explosion puts on a body at `pose` (Biotic Grenade's
    /// [`lucy_script::weapons::Blast::aura`]), once it exploded: when the
    /// body is within its rings and in its sight ([`Self::in_sight`]).
    pub fn blast_aura(
        &self,
        pose: &TargetPose,
        mut wall: impl MapRay,
    ) -> Result<Option<u64>, IndexError> {
        let Some((blast, aura)) = self
            .ballistics
            .blast
            .as_ref()
            .filter(|_| self.exploded)
            .and_then(|blast| Some((blast, blast.aura?)))
        else {
            return Ok(None);
        };
        let reached = blast.multiplier(pose.surface_distance(self.position)) > 0.0
            && self.in_sight(pose, &mut wall)?;
        Ok(reached.then_some(aura))
    }

    /// Its explosion's push on a body at `pose`, once it exploded and when
    /// it sees the body ([`Self::in_sight`]): [`Self::push_at`], whole.
    pub fn blast_push(
        &self,
        pose: &TargetPose,
        mut wall: impl MapRay,
    ) -> Result<Option<Vec3>, IndexError> {
        let Some(push) = self.push_at(pose, 1.0) else {
            return Ok(None);
        };
        Ok(self.in_sight(pose, &mut wall)?.then_some(push))
    }

    /// Its explosion's push on a body at `pose` (none before it exploded),
    /// times `scale` (the explosion's self-push scale for its shooter, 1
    /// for others): the client's formula ([`lucy_script::weapons::Knockback::push`],
    /// `0x7FF789C7B3C0`) along the line from the explosion to the body's
    /// centre (half its height above the feet; along the projectile's
    /// flight when they meet), at the body's distance.
    fn push_at(&self, pose: &TargetPose, scale: f64) -> Option<Vec3> {
        let blast = self.ballistics.blast.as_ref().filter(|_| self.exploded)?;
        if scale <= 0.0 {
            return None;
        }
        let distance = pose.surface_distance(self.position);
        blast
            .knockback
            .as_ref()?
            .push(self.blast_line(pose), distance, scale)
    }

    /// Whether its explosion sees a body at `pose`, as the client's own push
    /// tests its shooter (`0x7FF789A433F0`): a ray of the explosion's sight
    /// profile ([`lucy_script::weapons::Blast::sight`]) from 0.2 m back along
    /// its flight ([`SIGHT_BACK`]) to the body's centre (half its height
    /// above the feet) meets no map collision, or failing that one to the
    /// point of the body's axis at the origin's height (between its feet and
    /// its top). An explosion without the test sees every body.
    /// Assumption: the server's damage, other bodies' pushes and auras take
    /// the same test as that push (no client code deals them; walls stopping
    /// explosions is the public rule).
    fn in_sight(&self, pose: &TargetPose, wall: &mut impl MapRay) -> Result<bool, IndexError> {
        let Some(profile) = self.ballistics.blast.as_ref().and_then(|blast| blast.sight) else {
            return Ok(true);
        };
        let heading = self.heading();
        let origin: Vec3 =
            std::array::from_fn(|axis| self.position[axis] - SIGHT_BACK * heading[axis]);
        let [x, y, z] = pose.feet;
        let centre = [x, y + pose.height / 2.0, z];
        let level = [x, origin[1].clamp(y, y + pose.height), z];
        for point in [centre, level] {
            let line: Vec3 = std::array::from_fn(|axis| point[axis] - origin[axis]);
            let reach = dot(line, line).sqrt();
            if reach < 1e-6 {
                return Ok(true);
            }
            let direction = line.map(|part| part / reach);
            if wall(origin, direction, reach, Some(profile))?
                .is_none_or(|hit| hit.distance >= reach)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The unit direction it flies in now: its launch's or its last
    /// bounce's, bent by the speed gravity added (the client's sight origin
    /// steps back along its velocity, `0x7FF789A433F0`).
    fn heading(&self) -> Vec3 {
        let mut velocity = self
            .direction
            .map(|part| part * self.speed * self.speed_scale);
        velocity[1] -= self.fall;
        let length = dot(velocity, velocity).sqrt();
        if length > 0.0 {
            velocity.map(|part| part / length)
        } else {
            self.direction
        }
    }

    /// The ability its explosion is (its blast's health change's loadout).
    pub fn blast_loadout(&self) -> Option<u64> {
        self.ballistics.blast.as_ref()?.hit.loadout
    }

    /// The unit line from where it is to the centre of a body at `pose`
    /// (half its height above the feet); along its flight when they meet.
    fn blast_line(&self, pose: &TargetPose) -> Vec3 {
        let centre = [pose.feet[0], pose.feet[1] + pose.height / 2.0, pose.feet[2]];
        let line: [f64; 3] = std::array::from_fn(|axis| centre[axis] - self.position[axis]);
        [line, self.direction]
            .into_iter()
            .find_map(|[x, y, z]| {
                let length = x.hypot(y).hypot(z);
                (length > 0.0).then_some([x / length, y / length, z / length])
            })
            .unwrap_or_default()
    }
}

/// A body a sweep struck, and how it entered it ([`TargetPose::entry`]).
type Struck = (u32, Option<bool>);

/// What a sweep met first: a body and how it entered it, or a surface of
/// the map and its normal.
#[derive(Clone, Copy, Debug)]
enum Met {
    Body(u32, Option<bool>),
    Surface(Vec3),
}

/// The closest body or surface along a sweep: its distance, and what it
/// met. Bodies meet a sphere of radius `size`, surfaces its centre.
fn closest(
    origin: Vec3,
    direction: Vec3,
    reach: f64,
    size: f64,
    targets: &[(u32, TargetPose)],
    mut wall: impl FnMut(Vec3, Vec3, f64) -> Result<Option<WallHit>, IndexError>,
) -> Result<Option<(f64, Met)>, IndexError> {
    let mut closest = None;
    for (entity, target) in targets {
        if let Some((distance, entered)) = target.entry(origin, direction, reach, size) {
            if closest.is_none_or(|(best, _)| distance < best) {
                closest = Some((distance, Met::Body(*entity, entered)));
            }
        }
    }
    if let Some(hit) =
        wall(origin, direction, reach)?.filter(|hit| (0.0..=reach).contains(&hit.distance))
    {
        if closest.is_none_or(|(best, _)| hit.distance < best) {
            closest = Some((hit.distance, Met::Surface(hit.normal)));
        }
    }
    Ok(closest)
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn pose_of(targets: &[(u32, TargetPose)], entity: u32) -> Option<&TargetPose> {
    targets
        .iter()
        .find(|(target, _)| *target == entity)
        .map(|(_, pose)| pose)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucy_physics::collision::index::{SceneTriangle, TriangleIndex};
    use lucy_script::weapons::{
        AllyHits, BLAST_SIGHT_PROFILE, Blast, DamageFalloff, DamageTags, HitDefinition, Knockback,
        STICK_FUSE,
    };

    const LOADOUT: u64 = 0x0b90_0000_0000_008e;
    const TICK: f64 = 0.016;

    fn hit(amount: f64) -> HitDefinition {
        HitDefinition {
            amount,
            loadout: Some(LOADOUT),
            critical: 1.0,
            falloff: None,
            tags: DamageTags::NONE,
            allies: AllyHits::None,
        }
    }

    /// Soldier: 76's Helix Rocket as graph `0254` node 35 has it, swept as
    /// a point (its 0.2 m radius: `a_projectile_meets_bodies_by_its_radius`).
    fn helix() -> ProjectileBallistics {
        ProjectileBallistics {
            speed: 50.0,
            lifetime: 2.0,
            range: None,
            definition: Some(0x0400_0000_0000_072c),
            direct: Some(hit(40.0)),
            blast: Some(Blast {
                hit: hit(80.0),
                on_expiry: false,
                rings: vec![(1.0, 1.0), (3.0, 0.5)],
                self_damage: 0.5,
                aura: None,
                sight: Some(BLAST_SIGHT_PROFILE),
                self_knockback: 1.0,
                knockback: Some(Knockback {
                    away: 4.0,
                    upward: 6.0,
                    lift: 0.0,
                    away_rings: vec![(0.5, 1.0), (3.0, 0.75)],
                    upward_rings: vec![(0.5, 1.0), (3.0, 0.25)],
                }),
            }),
            radius: 0.0,
            gravity: 0.0,
            fuse: None,
            bounce: None,
            unmodelled: Vec::new(),
            landing: None,
            ray_profile: None,
        }
    }

    /// Tracer's Pulse Bomb as graph `0034` state 0 has it in the Practice
    /// Range: 15 m/s, 5 damage where it sticks, 350 within 2 m falling to
    /// ×0.2 at 4 m. Its gravity (30) is left out where a test aims straight.
    fn pulse_bomb(gravity: f64) -> ProjectileBallistics {
        ProjectileBallistics {
            speed: 15.0,
            lifetime: 5.0,
            range: None,
            definition: Some(0x0400_0000_0000_0045),
            direct: Some(hit(5.0)),
            blast: Some(Blast {
                hit: hit(350.0),
                on_expiry: false,
                rings: vec![(2.0, 1.0), (4.0, 0.2)],
                self_damage: 0.0,
                aura: None,
                sight: Some(BLAST_SIGHT_PROFILE),
                self_knockback: 1.0,
                knockback: None,
            }),
            radius: 0.0,
            gravity,
            fuse: Some(STICK_FUSE),
            bounce: None,
            landing: None,
            unmodelled: Vec::new(),
            ray_profile: None,
        }
    }

    fn target(x: f64, z: f64) -> TargetPose {
        TargetPose {
            feet: [x, 0.0, z],
            height: 2.0,
            radius: 0.5,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        }
    }

    fn no_wall(_: Vec3, _: Vec3, _: f64, _: Option<u64>) -> Result<Option<WallHit>, IndexError> {
        Ok(None)
    }

    fn damage(step: &FlightStep) -> Vec<(u32, f64)> {
        step.impacts
            .iter()
            .map(|impact| (impact.entity, impact.damage))
            .collect()
    }

    /// A wall across the flight at `z`, from x −50 to 50 and y 0 to 50.
    fn wall_at(z: f64) -> TriangleIndex {
        let corner = |x: f64, y: f64| [x, y, z];
        TriangleIndex::new([
            SceneTriangle {
                vertices: [corner(-50.0, 0.0), corner(50.0, 0.0), corner(50.0, 50.0)],
                mask: 0x18,
                surface: 0,
            },
            SceneTriangle {
                vertices: [corner(-50.0, 0.0), corner(50.0, 50.0), corner(-50.0, 50.0)],
                mask: 0x18,
                surface: 0,
            },
        ])
        .unwrap()
    }

    /// A floor at y 0, x and z from −50 to 50.
    fn floor() -> TriangleIndex {
        TriangleIndex::new([
            SceneTriangle {
                vertices: [[-50.0, 0.0, -50.0], [50.0, 0.0, 50.0], [50.0, 0.0, -50.0]],
                mask: 0x18,
                surface: 0,
            },
            SceneTriangle {
                vertices: [[-50.0, 0.0, -50.0], [-50.0, 0.0, 50.0], [50.0, 0.0, 50.0]],
                mask: 0x18,
                surface: 0,
            },
        ])
        .unwrap()
    }

    fn cast(world: &TriangleIndex) -> impl MapRay + '_ {
        |origin, direction, reach, _| {
            world.cast_ray(origin, direction, reach, 0x18).map(|hit| {
                hit.map(|hit| WallHit {
                    distance: hit.distance,
                    normal: hit.normal,
                })
            })
        }
    }

    #[test]
    fn helix_sweeps_missed_ticks_and_applies_direct_damage_before_splash() {
        let targets = [
            (1, target(0.0, 3.0)),
            (2, target(2.5, 2.5)),
            (3, target(4.0, 2.5)),
        ];
        let mut flight =
            ProjectileFlight::new(helix(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], None).unwrap();
        assert_eq!(
            flight.advance(1000, &targets, no_wall).unwrap(),
            FlightStep::default()
        );
        assert_eq!(
            flight
                .advance(1000, &targets, |_, _, _, _| panic!("duplicate tick"))
                .unwrap(),
            FlightStep::default()
        );
        let step = flight.advance(1004, &targets, no_wall).unwrap();
        let end = step.end.as_ref().unwrap();
        assert_eq!(end.point, [0.0, 1.0, 2.5]);
        assert!((end.travelled - 0.05).abs() < 1e-12);
        assert_eq!(damage(&step), [(1, 40.0), (1, 80.0), (2, 60.0)]);
        assert!(
            step.impacts
                .iter()
                .all(|impact| impact.loadout == Some(LOADOUT) && !impact.critical)
        );
        assert_eq!(
            flight
                .advance(1005, &targets, |_, _, _, _| panic!("already ended"))
                .unwrap(),
            FlightStep::default()
        );
    }

    /// A Helix Rocket (radius 0.2 m) flying 0.6 m beside a body's axis
    /// strikes its capsule (radius 0.5) where the rocket's surface meets
    /// it; as a point it would pass by. A wall stops its centre. One whose
    /// sweep passes the bodies flies through one to the wall behind it.
    #[test]
    fn a_projectile_meets_bodies_by_its_radius() {
        let targets = [(1, target(0.6, 3.0))];
        let rocket = ProjectileBallistics {
            radius: 0.2,
            ..helix()
        };
        let mut point =
            ProjectileFlight::new(helix(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        assert!(
            point
                .advance(1004, &targets, no_wall)
                .unwrap()
                .impacts
                .is_empty()
        );
        let mut flight =
            ProjectileFlight::new(rocket.clone(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000))
                .unwrap();
        let step = flight.advance(1004, &targets, no_wall).unwrap();
        let end = step.end.as_ref().unwrap();
        // The centres are 0.7 m apart where they meet: 0.6 across, so
        // sqrt(0.49 - 0.36) along the flight before the body's axis.
        let met = 3.0 - (0.7f64 * 0.7 - 0.6 * 0.6).sqrt();
        assert!((end.point[2] - met).abs() < 1e-9, "{:?}", end.point);
        assert_eq!(step.impacts[0].entity, 1);
        assert_eq!(step.impacts[0].damage, 40.0);

        let world = wall_at(2.0);
        let mut walled =
            ProjectileFlight::new(rocket, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        let step = walled
            .advance(1004, &[(1, target(0.0, 2.8))], cast(&world))
            .unwrap();
        // The sphere would meet the capsule at 2.1, the wall stops its
        // centre at 2.0, and the wall hides the body from the explosion.
        assert_eq!(step.end.as_ref().unwrap().point, [0.0, 1.0, 2.0]);
        assert!(step.impacts.is_empty(), "{step:?}");

        // Through the body on its axis to the wall 1 m behind it; the
        // explosion there still reaches the body, 0.5 m from its surface.
        let world = wall_at(4.0);
        let mut passing =
            ProjectileFlight::new(helix(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        passing.pass_bodies();
        let step = passing
            .advance(1006, &[(1, target(0.0, 3.0))], cast(&world))
            .unwrap();
        assert_eq!(step.end.as_ref().unwrap().point, [0.0, 1.0, 4.0]);
        assert_eq!(damage(&step), [(1, 80.0)]);
    }

    /// The wall wins over the body behind it, and stops the explosion: the
    /// body beyond it takes nothing (where Python's splash reached it for
    /// 70), the one beside the burst on this side its 80.
    #[test]
    fn walls_win_closer_impact_and_last_partial_tick_is_swept() {
        let world = wall_at(1.0);
        let mut flight =
            ProjectileFlight::new(helix(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        let step = flight
            .advance(
                1002,
                &[(1, target(0.0, 3.0)), (2, target(1.2, 0.2))],
                cast(&world),
            )
            .unwrap();
        assert_eq!(step.end.as_ref().unwrap().point, [0.0, 1.0, 1.0]);
        assert_eq!(damage(&step), [(2, 80.0)]);
        let short = ProjectileBallistics {
            lifetime: 0.01,
            ..helix()
        };
        let mut flight =
            ProjectileFlight::new(short.clone(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], None).unwrap();
        let step = flight
            .advance(1000, &[(1, target(0.0, 1.0))], no_wall)
            .unwrap();
        let end = step.end.as_ref().unwrap();
        assert_eq!((end.point[2], end.travelled), (0.5, 0.01));
        assert_eq!(step.impacts[0].damage, 40.0);
        let mut flight =
            ProjectileFlight::new(short, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], None).unwrap();
        let step = flight
            .advance(1000, &[(1, target(1.0, 1.0))], no_wall)
            .unwrap();
        let end = step.end.as_ref().unwrap();
        assert_eq!((end.point[2], end.travelled), (0.5, 0.01));
        assert!(step.impacts.is_empty());
    }

    /// Moira's Biotic Orb (`10E4` rid19: 20 m/s, 99 bounces off every kind
    /// of surface, restitution 1) fired at a wall 2 m ahead comes back at
    /// full speed: 10 ticks (0.16 s) fly 2 m to the wall, then 1.2 m back
    /// from 1 mm off it. A block whose floors don't bounce (Sombra's
    /// Translocator `0C19` st5) ends on the floor; one with no bounces left
    /// ends at the wall.
    #[test]
    fn a_bouncing_projectile_comes_back_off_the_wall() {
        use lucy_script::weapons::Bounce;
        let orb = |bounce| ProjectileBallistics {
            speed: 20.0,
            lifetime: 7.0,
            range: None,
            definition: Some(0x0400_0000_0000_1e61),
            direct: None,
            blast: None,
            radius: 0.0,
            gravity: 0.0,
            fuse: None,
            bounce: Some(bounce),
            landing: None,
            unmodelled: Vec::new(),
            ray_profile: None,
        };
        let moira = Bounce {
            count: 99,
            bounces: [true; 3],
            ends: [false; 3],
            restitution: 1.0,
            tangent_deceleration: 0.0,
            tangent_scale: 1.0,
            ceiling: -std::f64::consts::FRAC_1_SQRT_2,
            floor: std::f64::consts::FRAC_1_SQRT_2,
        };
        let wall = wall_at(2.0);
        let mut flight =
            ProjectileFlight::new(orb(moira), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000))
                .unwrap();
        assert_eq!(
            flight.advance(1010, &[], cast(&wall)).unwrap(),
            FlightStep::default()
        );
        assert!(
            (flight.position[2] - (2.0 - 0.001 - 1.2)).abs() < 1e-6,
            "{:?}",
            flight.position
        );
        assert!((flight.direction[2] + 1.0).abs() < 1e-12);
        assert!((flight.speed - 20.0).abs() < 1e-9);

        let translocator = Bounce {
            bounces: [true, true, false],
            ..moira
        };
        let mut flight = ProjectileFlight::new(
            orb(translocator),
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            Some(1000),
        )
        .unwrap();
        let end = flight.advance(1010, &[], cast(&floor())).unwrap().end;
        assert!(end.unwrap().point[1].abs() < 1e-6);

        let spent = Bounce { count: 0, ..moira };
        let mut flight =
            ProjectileFlight::new(orb(spent), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000))
                .unwrap();
        let end = flight.advance(1010, &[], cast(&wall)).unwrap().end;
        assert!((end.unwrap().point[2] - 2.0).abs() < 1e-6);
    }

    /// A Helix Rocket fired straight down from the eye bursts on the floor
    /// at its shooter's feet: he takes half its 80 and goes up at the line's
    /// vertical part × 6 m/s (the client's push, `Knockback::push`), times
    /// the explosion's self-push scale (Pharah's 1.875; without the flag,
    /// none, the damage all the same). Fired level at a
    /// wall 1 m ahead, it pushes him back from the wall and down along the
    /// line from the burst to his centre, plus up, by the knockback rings
    /// at his distance; a body behind that wall it doesn't reach. A rocket
    /// that runs out of time doesn't explode and does nothing to him.
    #[test]
    fn a_helix_hurts_and_pushes_its_own_shooter() {
        let own = target(0.0, 0.0);
        let at_feet = |self_knockback| {
            let mut rocket = helix();
            rocket.blast.as_mut().unwrap().self_knockback = self_knockback;
            let mut flight =
                ProjectileFlight::new(rocket, [0.0, 1.55, 0.0], [0.0, -1.0, 0.0], Some(1000))
                    .unwrap();
            let step = flight.advance(1002, &[], cast(&floor())).unwrap();
            assert!(step.end.unwrap().point[1].abs() < 1e-9);
            flight.own_blast(&own, cast(&floor())).unwrap().unwrap()
        };
        let blast = at_feet(1.0);
        assert!(blast.distance < 1e-9);
        // Straight up the line's vertical part × 6 (`m_10399AE7`), times the
        // self-push scale; the push away has no ground part there.
        assert_eq!(
            (blast.damage, blast.loadout, blast.knockback),
            (40.0, Some(LOADOUT), Some([0.0, 6.0, 0.0]))
        );
        assert_eq!(at_feet(1.875).knockback, Some([0.0, 11.25, 0.0]));
        let unpushed = at_feet(0.0);
        assert_eq!((unpushed.damage, unpushed.knockback), (40.0, None));

        let wall = wall_at(1.0);
        let mut flight =
            ProjectileFlight::new(helix(), [0.0, 1.55, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        let step = flight.advance(1002, &[], cast(&wall)).unwrap();
        assert_eq!(step.end.unwrap().point, [0.0, 1.55, 1.0]);
        let blast = flight.own_blast(&own, cast(&wall)).unwrap().unwrap();
        // From the capsule's top end, 1.5 m up, less its radius.
        assert!((blast.distance - (1.0f64.hypot(0.05) - 0.5)).abs() < 1e-12);
        assert_eq!(blast.damage, 40.0);
        // The line from the burst (1.55 m up, 1 m ahead) to his centre (1 m
        // up): pushed back along the ground; its downward part is floored at
        // `m_583C208B` (0) while the upward ring holds.
        let length = 0.55f64.hypot(1.0);
        let line = [0.0, -0.55 / length, -1.0 / length];
        let expected = helix()
            .blast
            .unwrap()
            .knockback
            .unwrap()
            .push(line, blast.distance, 1.0);
        assert_eq!(blast.knockback, expected);
        let [x, y, z] = expected.unwrap();
        assert!(x == 0.0 && y == 0.0 && z < 0.0);
        assert_eq!(
            flight.own_blast(&target(0.0, 1.5), cast(&wall)).unwrap(),
            None
        );

        let short = ProjectileBallistics {
            lifetime: 0.01,
            ..helix()
        };
        let mut flight =
            ProjectileFlight::new(short, [0.0, 1.55, 0.0], [0.0, 0.0, 1.0], None).unwrap();
        assert!(flight.advance(1000, &[], no_wall).unwrap().end.is_some());
        assert_eq!(flight.own_blast(&own, no_wall).unwrap(), None);
    }

    /// Fired level from 1.5 m at 20 m/s under 9.8 m/s² (Junkrat's and
    /// Hanzo's arc), the projectile lands where and when the parabola meets
    /// the floor, sweeping the ticks it missed.
    #[test]
    fn an_arc_lands_where_its_parabola_meets_the_ground() {
        let grenade = ProjectileBallistics {
            speed: 20.0,
            gravity: 9.8,
            ..helix()
        };
        let ground = floor();
        let mut flight =
            ProjectileFlight::new(grenade, [0.0, 1.5, 0.0], [1.0, 0.0, 0.0], Some(1000)).unwrap();
        let mut end = None;
        let mut tick = 1000;
        while end.is_none() {
            tick += 3;
            end = flight.advance(tick, &[], cast(&ground)).unwrap().end;
        }
        let end = end.unwrap();
        let fall = (2.0 * 1.5 / 9.8f64).sqrt();
        assert!((end.travelled - fall).abs() < 1e-3, "{}", end.travelled);
        assert!((end.point[0] - 20.0 * fall).abs() < 0.02, "{:?}", end.point);
        assert!(end.point[1].abs() < 1e-9 && end.point[2] == 0.0);
        assert_eq!(tick, 1000 + 3 * (fall / (3.0 * TICK)).ceil() as u32);
    }

    /// The arc dips onto a short target that a straight shot passes over.
    #[test]
    fn an_arc_hits_a_target_below_its_line_of_fire() {
        let short = TargetPose {
            feet: [10.0, 0.0, 0.0],
            height: 0.9,
            radius: 0.3,
            eye: 0.7,
            yaw: 0.0,
            shapes: None,
        };
        let fly = |gravity| {
            let ballistics = ProjectileBallistics {
                speed: 20.0,
                lifetime: 0.7,
                gravity,
                blast: None,
                ..helix()
            };
            let mut flight =
                ProjectileFlight::new(ballistics, [0.0, 1.5, 0.0], [1.0, 0.0, 0.0], Some(1000))
                    .unwrap();
            (1001..1100)
                .map(|tick| {
                    flight
                        .advance(tick, &[(7, short.clone())], no_wall)
                        .unwrap()
                })
                .find(|step| step.end.is_some())
                .unwrap()
        };
        let straight = fly(0.0);
        assert!(straight.impacts.is_empty());
        assert!((straight.end.unwrap().point[0] - 14.0).abs() < 1e-9);
        let arc = fly(9.8);
        assert_eq!(damage(&arc), [(7, 40.0)]);
        let point = arc.end.unwrap().point;
        assert!(point[0] > 9.0 && point[0] < 10.0, "{point:?}");
        assert!(point[1] > 0.0 && point[1] < 0.9, "{point:?}");
    }

    /// A body with hit shapes is met by them: a bolt passing 0.4 m from its
    /// one 0.3 m sphere flies by, where the capsule (0.5 m) caught it; one
    /// at the sphere stops on its surface, and its blast there is at 0 m.
    /// A direct hit that scores headshots (2, as Genji's shuriken) doubles
    /// on a critical shape, and on the capsule at or above the eye less
    /// 0.2 m (1.35 m up), not below; the Helix Rocket's (1) never does.
    #[test]
    fn a_body_with_shapes_is_swept_and_blasted_by_its_shapes() {
        use crate::hitboxes::HitShape;
        let head = HitShape {
            a: [0.0, 1.0, 0.0],
            b: [0.0, 1.0, 0.0],
            radius: 0.3,
            critical: true,
            fixed: true,
        };
        let shaped = TargetPose {
            shapes: Some(vec![head].into()),
            ..target(5.0, 0.0)
        };
        let fly = |z, body: &TargetPose| {
            let mut flight =
                ProjectileFlight::new(helix(), [0.0, 1.0, z], [1.0, 0.0, 0.0], Some(1000)).unwrap();
            (1001..1200)
                .map(|tick| flight.advance(tick, &[(4, body.clone())], no_wall).unwrap())
                .find(|step| step.end.is_some())
                .unwrap()
        };
        assert_eq!(damage(&fly(0.4, &target(5.0, 0.0))), [(4, 40.0), (4, 80.0)]);
        let by = fly(0.4, &shaped);
        assert!(by.impacts.is_empty());
        assert!((by.end.unwrap().point[0] - 100.0).abs() < 1e-9);
        let struck = fly(0.0, &shaped);
        assert_eq!(damage(&struck), [(4, 40.0), (4, 80.0)]);
        assert!((struck.end.unwrap().point[0] - 4.7).abs() < 1e-9);
        assert!(struck.impacts.iter().all(|impact| !impact.critical));
        let shuriken = |y, body: &TargetPose| {
            let ballistics = ProjectileBallistics {
                direct: Some(HitDefinition {
                    critical: 2.0,
                    ..hit(30.0)
                }),
                blast: None,
                ..helix()
            };
            let mut flight =
                ProjectileFlight::new(ballistics, [0.0, y, 0.0], [1.0, 0.0, 0.0], Some(1000))
                    .unwrap();
            let step = (1001..1200)
                .map(|tick| flight.advance(tick, &[(4, body.clone())], no_wall).unwrap())
                .find(|step| step.end.is_some())
                .unwrap();
            let [impact] = step.impacts[..] else {
                panic!("{step:?}");
            };
            (impact.damage, impact.critical)
        };
        assert_eq!(shuriken(1.0, &shaped), (60.0, true));
        assert_eq!(shuriken(1.4, &target(5.0, 0.0)), (60.0, true));
        assert_eq!(shuriken(1.0, &target(5.0, 0.0)), (30.0, false));
    }

    /// The Pulse Bomb sticks to the bot it hits (5 damage), rides along as
    /// the bot moves, and explodes a second later where the bot then is:
    /// 350 to the bot it is on, ×0.6 to a body 3 m away, nothing to the one
    /// it passed.
    #[test]
    fn a_sticking_projectile_rides_its_target_and_explodes_after_its_fuse() {
        let mut flight =
            ProjectileFlight::new(pulse_bomb(0.0), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(999))
                .unwrap();
        let mut bot = target(0.0, 3.0);
        let behind = target(-3.0, 0.0);
        let mut stuck = None;
        for tick in 1000..1100 {
            let step = flight
                .advance(tick, &[(1, bot.clone()), (2, behind.clone())], no_wall)
                .unwrap();
            assert!(step.end.is_none());
            if !step.impacts.is_empty() {
                assert_eq!(damage(&step), [(1, 5.0)]);
                stuck = Some(tick);
                break;
            }
        }
        // 2.5 m at 15 m/s: in the 11th tick.
        let stuck = stuck.unwrap();
        assert_eq!(stuck, 1010);
        let mut tick = stuck;
        let step = loop {
            tick += 1;
            bot.feet[0] += 0.05;
            let near = target(bot.feet[0] + 3.5, 2.5);
            let step = flight
                .advance(
                    tick,
                    &[(1, bot.clone()), (2, behind.clone()), (3, near)],
                    |_, _, _, profile| {
                        // Only its explosion's sight rays.
                        assert_eq!(profile, Some(BLAST_SIGHT_PROFILE));
                        Ok(None)
                    },
                )
                .unwrap();
            if step.end.is_some() {
                break step;
            }
            assert!(step.impacts.is_empty());
        };
        let end = step.end.as_ref().unwrap();
        let fuse_end = 2.5 / 15.0 + STICK_FUSE;
        assert!((end.travelled - fuse_end).abs() < 1e-9);
        assert_eq!(tick, 999 + (fuse_end / TICK).ceil() as u32);
        assert_eq!(end.point, [bot.feet[0], 1.0, 2.5]);
        let hits = damage(&step);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0], (1, 350.0));
        assert_eq!(hits[1].0, 3);
        assert!((hits[1].1 - 210.0).abs() < 1e-9);
    }

    /// It sticks to a surface too, damages nothing there, and its explosion
    /// reaches a body on its side of the wall, not the one behind it; a
    /// bomb that hits nothing in its lifetime ends without one.
    #[test]
    fn a_sticking_projectile_holds_on_a_surface_and_a_miss_ends_quietly() {
        let world = wall_at(1.0);
        let mut flight = ProjectileFlight::new(
            pulse_bomb(30.0),
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            Some(999),
        )
        .unwrap();
        let bot = (1, target(0.0, 3.0));
        let bots = [bot.clone(), (2, target(1.5, 0.0))];
        let mut steps = (1000..1200).map(|tick| flight.advance(tick, &bots, cast(&world)).unwrap());
        let step = steps
            .find(|step| step.end.is_some() || !step.impacts.is_empty())
            .unwrap();
        assert_eq!(damage(&step), [(2, 350.0)]);
        // It met the wall after 1/15 s, 1/15 m lower (the chord within 1 mm).
        let end = step.end.unwrap();
        let flight = 1.0 / 15.0;
        assert!((end.travelled - (flight + STICK_FUSE)).abs() < 1e-9);
        assert!((end.point[2] - 1.0).abs() < 1e-9);
        assert!((end.point[1] - (1.0 - 15.0 * flight * flight)).abs() < 1e-3);
        let quick = ProjectileBallistics {
            lifetime: 0.1,
            ..pulse_bomb(0.0)
        };
        let mut flight =
            ProjectileFlight::new(quick, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(999)).unwrap();
        let step = (1000..1100)
            .map(|tick| {
                flight
                    .advance(tick, std::slice::from_ref(&bot), no_wall)
                    .unwrap()
            })
            .find(|step| step.end.is_some())
            .unwrap();
        assert!(step.impacts.is_empty());
        assert!((step.end.unwrap().travelled - 0.1).abs() < 1e-12);
    }

    /// Upstream `test_hand_cannon_direct_damage_uses_the_authored_falloff`
    /// (b3c5138): a Hand Cannon pellet (graph `0D0A` state 2: 80 m/s, 500 m
    /// reach, 6 damage falling from 15 m to ×0.3 at 30 m) does 6.0, 3.9 and
    /// 1.8 to a body 5, 23 and 35 m ahead, whose capsule it meets 0.5 m
    /// short.
    #[test]
    fn a_direct_hit_falls_off_with_the_distance_flown() {
        let hand_cannon = ProjectileBallistics {
            speed: 80.0,
            lifetime: 10.0,
            range: Some(500.0),
            definition: None,
            direct: Some(HitDefinition {
                falloff: Some(DamageFalloff {
                    start: 15.0,
                    end: 30.0,
                    floor: 0.3,
                }),
                ..hit(6.0)
            }),
            blast: None,
            radius: 0.0,
            gravity: 0.0,
            fuse: None,
            bounce: None,
            landing: None,
            unmodelled: Vec::new(),
            ray_profile: None,
        };
        for (ahead, expected) in [(5.0, 6.0), (23.0, 3.9), (35.0, 1.8)] {
            let mut flight = ProjectileFlight::new(
                hand_cannon.clone(),
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                Some(999),
            )
            .unwrap();
            let body = [(1, target(0.0, ahead))];
            let step = (1000..1100)
                .map(|tick| flight.advance(tick, &body, no_wall).unwrap())
                .find(|step| step.end.is_some())
                .unwrap();
            let [(1, damage)] = damage(&step)[..] else {
                panic!("one direct hit: {step:?}")
            };
            assert!((damage - expected).abs() < 0.02, "{ahead} m: {damage}");
        }
    }

    /// A volley's range ends its flight along its path (the client's step
    /// `0x7FF789C82C80`): 10 m at 20 m/s is half a second flying straight,
    /// longer for a projectile its graphs slow (0.275, a Biotic Orb's) and
    /// for an arc, whose path is longer than the line to where it ends.
    #[test]
    fn a_range_ends_the_flight_along_its_path() {
        let ranged = |gravity, scale: f64, up: f64| {
            let ballistics = ProjectileBallistics {
                speed: 20.0,
                lifetime: 5.0,
                range: Some(10.0),
                gravity,
                ..helix()
            };
            let direction = [0.0, up.sin(), up.cos()];
            let mut flight =
                ProjectileFlight::new(ballistics, [0.0; 3], direction, Some(1000)).unwrap();
            flight.set_speed_scale(scale);
            let end = (1001..2000)
                .find_map(|tick| flight.advance(tick, &[], no_wall).unwrap().end)
                .unwrap();
            (end, flight.flown)
        };
        let (straight, flown) = ranged(0.0, 1.0, 0.0);
        assert!(!straight.exploded);
        assert!((flown - 10.0).abs() < 1e-9 && (straight.point[2] - 10.0).abs() < 1e-9);
        assert!((straight.travelled - 0.5).abs() < 1e-9);
        let (slowed, _) = ranged(0.0, 0.275, 0.0);
        assert!((slowed.point[2] - 10.0).abs() < 1e-9);
        assert!((slowed.travelled - 0.5 / 0.275).abs() < 1e-9);
        let (arc, flown) = ranged(9.8, 1.0, std::f64::consts::FRAC_PI_4);
        assert!((flown - 10.0).abs() < 1e-9);
        let line = arc.point.iter().map(|part| part * part).sum::<f64>().sqrt();
        assert!(line < 10.0 && arc.travelled > 0.5, "{arc:?}");
    }

    /// Sigma's `187F` expiry flag runs the same native area hit as an
    /// impact. The ray misses the bot, but its burst reaches the body.
    #[test]
    fn an_expiry_blast_reaches_a_body_beside_the_flight() {
        for range_end in [false, true] {
            let mut ballistics = helix();
            ballistics.blast.as_mut().unwrap().on_expiry = true;
            if range_end {
                ballistics.range = Some(27.5);
            } else {
                ballistics.lifetime = 0.55;
            }
            let mut flight =
                ProjectileFlight::new(ballistics, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000))
                    .unwrap();
            let targets = [(1, target(1.0, 27.5))];
            let step = (1001..1100)
                .map(|tick| flight.advance(tick, &targets, no_wall).unwrap())
                .find(|step| step.end.is_some())
                .unwrap();
            assert_eq!(damage(&step), [(1, 80.0)]);
            let end = step.end.unwrap();
            assert!(end.exploded);
            assert!((end.travelled - 0.55).abs() < 1e-9);
        }
    }

    /// Junkrat `022B`: a slow floor bounce arms a 0.2 s lifetime no
    /// earlier than launch + 0.5 s, not the volley's 10 s failsafe.
    #[test]
    fn a_slow_floor_bounce_arms_the_grenades_short_lifetime() {
        use lucy_script::weapons::{Bounce, Landing};
        let mut grenade = helix();
        grenade.speed = 10.0;
        grenade.lifetime = 10.0;
        grenade.gravity = 12.25;
        grenade.direct = None;
        grenade.blast.as_mut().unwrap().on_expiry = true;
        grenade.bounce = Some(Bounce {
            count: 100,
            bounces: [true; 3],
            ends: [false; 3],
            restitution: 0.5,
            tangent_deceleration: 300.0,
            tangent_scale: 0.75,
            ceiling: -std::f64::consts::FRAC_1_SQRT_2,
            floor: 0.5,
        });
        grenade.landing = Some(Landing {
            surfaces: [false, false, true],
            speed: 8.5,
            lifetime: f64::from(0.2f32),
            minimum_age: 0.5,
        });
        let mut flight = ProjectileFlight::new(
            grenade.clone(),
            [0.0, 0.1, 0.0],
            [0.0, -1.0, 0.0],
            Some(1000),
        )
        .unwrap();
        let step = (1001..1700)
            .map(|tick| flight.advance(tick, &[], cast(&floor())).unwrap())
            .find(|step| step.end.is_some())
            .unwrap();
        let end = step.end.unwrap();
        assert!(flight.landing_armed && end.exploded);
        // Client integer ticks: floor(0.5 / .016) = 31; the elapsed
        // offset floor((10 - f32(.2)) / .016) = 612. Deadline:
        // 31*.016 + 10 - 612*.016 = .704 s.
        assert!((end.travelled - 0.704).abs() < 1e-9, "{end:?}");

        // The first oblique bounce still exceeds the speed gate:
        // tangent (20 - 300*.016)*.75 = 11.4; normal 15*.5 = 7.5.
        grenade.speed = 25.0;
        let mut fast =
            ProjectileFlight::new(grenade, [0.0; 3], [0.0, -0.6, 0.8], Some(1000)).unwrap();
        assert!(fast.bounce([0.0, 1.0, 0.0], [0.0, -0.6, 0.8]));
        fast.arm_landing([0.0, 1.0, 0.0]);
        assert!(!fast.landing_armed);
        assert!((fast.speed - (11.4f64.powi(2) + 7.5f64.powi(2)).sqrt()).abs() < 1e-9);
    }
}
