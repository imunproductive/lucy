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
//! projectile is swept as a point (the ballistics have no radius to grow
//! the shapes by, and the reference swept points too). An explosion also
//! hurts and pushes its own shooter ([`ProjectileFlight::own_blast`]).

use crate::hitscan::{Impact, TargetPose};
use orpheus_core::tick::TICK_PERIOD;
use orpheus_physics::{collision::index::IndexError, static_ray::Vec3};
use orpheus_script::weapons::{HitDefinition, ProjectileBallistics};

pub struct ProjectileFlight {
    ballistics: ProjectileBallistics,
    position: Vec3,
    /// The launch direction (a unit vector).
    direction: Vec3,
    /// The downward speed gravity added so far, m/s.
    fall: f64,
    remaining: f64,
    travelled: f64,
    last_tick: Option<u32>,
    stuck: Option<Stuck>,
    ended: bool,
    /// It ended with its explosion.
    exploded: bool,
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
            ballistics,
            position,
            direction,
            fall: 0.0,
            travelled: 0.0,
            last_tick: fired_at,
            stuck: None,
            ended: false,
            exploded: false,
        })
    }

    /// The direct hit of the projectile, if it has one.
    pub fn direct(&self) -> Option<HitDefinition> {
        self.ballistics.direct
    }

    /// Advance the elapsed portion of a tick, even when it ends inside the
    /// tick due to range, lifetime or fuse. Static geometry wins only a
    /// closer hit.
    pub fn advance(
        &mut self,
        tick: u32,
        targets: &[(u32, TargetPose)],
        mut wall: impl FnMut(Vec3, Vec3, f64) -> Result<Option<f64>, IndexError>,
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
            self.hold(time, targets, &mut step);
        }
        Ok(step)
    }

    /// Fly up to `time` seconds; what is left of them once it stuck.
    fn fly(
        &mut self,
        time: f64,
        targets: &[(u32, TargetPose)],
        wall: &mut impl FnMut(Vec3, Vec3, f64) -> Result<Option<f64>, IndexError>,
        step: &mut FlightStep,
    ) -> Result<f64, IndexError> {
        let duration = time.min(self.remaining);
        let (speed, gravity) = (self.ballistics.speed, self.ballistics.gravity);
        // A straight flight sweeps the whole duration at once (combat.py
        // `_fly`); an arc a chord of at most one tick at a time.
        let pieces = if gravity == 0.0 {
            1
        } else {
            (duration / TICK_PERIOD.as_secs_f64()).ceil().max(1.0) as u32
        };
        let piece = duration / f64::from(pieces);
        let mut flown = 0.0;
        for _ in 0..pieces {
            let (direction, reach) = if gravity == 0.0 {
                (self.direction, speed * piece)
            } else {
                let mut chord = self.direction.map(|part| part * speed * piece);
                chord[1] -= (self.fall + 0.5 * gravity * piece) * piece;
                let length = chord.iter().map(|part| part * part).sum::<f64>().sqrt();
                (
                    chord.map(|part| part / length.max(f64::MIN_POSITIVE)),
                    length,
                )
            };
            let hit = if reach > 0.0 {
                closest(self.position, direction, reach, targets, wall)?
            } else {
                None
            };
            let (distance, seconds) = match hit {
                Some((distance, _)) if gravity == 0.0 => (distance, distance / speed),
                Some((distance, _)) => (distance, piece * distance / reach),
                None => (reach, piece),
            };
            for (position, direction) in self.position.iter_mut().zip(direction) {
                *position += direction * distance;
            }
            self.fall += gravity * seconds;
            self.travelled += seconds;
            self.remaining -= seconds;
            flown += seconds;
            if let Some((distance, target)) = hit {
                self.hit(distance, target, targets, step);
                return Ok((time - flown).max(0.0));
            }
        }
        if self.remaining <= 0.0 {
            step.end = Some(self.finish());
        }
        Ok(0.0)
    }

    /// What it hit `distance` into this sweep: the direct hit's damage to a
    /// struck body, then it sticks, or it ends with its explosion. The
    /// direct hit falls off with the distance flown, its seconds in flight
    /// times its speed (leafall903/ORPHEUS (MIT) `combat.py` `_fly`, commit
    /// b3c5138: Doomfist's 6 damage is 3.9 at 23 m and 1.8 at 35 m).
    fn hit(
        &mut self,
        distance: f64,
        target: Option<u32>,
        targets: &[(u32, TargetPose)],
        step: &mut FlightStep,
    ) {
        if let (Some(entity), Some(direct)) = (
            target,
            self.ballistics.direct.filter(|hit| hit.amount > 0.0),
        ) {
            step.impacts.push(Impact {
                entity,
                distance,
                damage: direct.damage_at(self.travelled * self.ballistics.speed, false),
                critical: false,
                loadout: direct.loadout,
                tags: direct.tags,
                heal: direct.ally_heal(),
            });
        }
        let Some(fuse) = self.ballistics.fuse else {
            self.explode(targets, step);
            return;
        };
        let offset = target
            .and_then(|entity| pose_of(targets, entity))
            .map_or([0.0; 3], |pose| {
                std::array::from_fn(|axis| self.position[axis] - pose.feet[axis])
            });
        self.stuck = Some(Stuck {
            entity: target,
            offset,
            fuse,
        });
    }

    /// `time` seconds stuck: on the body where it still is a target, until
    /// the fuse ends.
    fn hold(&mut self, time: f64, targets: &[(u32, TargetPose)], step: &mut FlightStep) {
        let Some(stuck) = &mut self.stuck else {
            return;
        };
        if let Some(pose) = stuck.entity.and_then(|entity| pose_of(targets, entity)) {
            self.position = std::array::from_fn(|axis| pose.feet[axis] + stuck.offset[axis]);
        }
        if time < stuck.fuse {
            stuck.fuse -= time;
            self.travelled += time;
            return;
        }
        self.travelled += stuck.fuse;
        stuck.fuse = 0.0;
        self.explode(targets, step);
    }

    /// The explosion at its position (combat.py `_fly`: every target's
    /// distance to the capsule, here to its shapes when it has them, scales
    /// the damage), and the end.
    fn explode(&mut self, targets: &[(u32, TargetPose)], step: &mut FlightStep) {
        self.exploded = true;
        if let Some(blast) = self
            .ballistics
            .blast
            .as_ref()
            .filter(|blast| blast.hit.amount > 0.0)
        {
            for (entity, pose) in targets {
                let entity = *entity;
                let distance = pose.surface_distance(self.position);
                let damage = blast.hit.amount * blast.multiplier(distance);
                if damage > 0.0 {
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
    }

    fn finish(&mut self) -> FlightEnd {
        self.ended = true;
        FlightEnd {
            point: self.position,
            travelled: self.travelled,
            knockback: None,
        }
    }

    /// Its explosion on its shooter's own body at `own`, once it exploded:
    /// the blast's damage at the body's distance times the explosion's
    /// self-damage scale, and its push. The push is the one
    /// leafall903/ORPHEUS (MIT) `heavy_assault.py` `_push` gives a melee
    /// victim: the horizontal speed away from the explosion in the ground
    /// plane (along the projectile's flight when the body's feet are right
    /// under it; `_push` takes the attacker's aim there) and the upward
    /// speed. None when it does neither. Beyond Python, which never hurt or
    /// pushed the shooter: every explosion reaches its shooter (assumption;
    /// which of its flags `m_43D015B3`, `m_7F5CC8D3`, `m_B15688ED`,
    /// `m_21CEF653`, `m_EEB99E41` would spare him isn't known, and Helix
    /// Rockets set all five).
    pub fn own_blast(&self, own: &TargetPose) -> Option<OwnBlast> {
        let blast = self.ballistics.blast.as_ref().filter(|_| self.exploded)?;
        let distance = own.surface_distance(self.position);
        let damage = blast.hit.amount.max(0.0) * blast.multiplier(distance) * blast.self_damage;
        let knockback = blast
            .knockback
            .as_ref()
            .and_then(|knockback| knockback.at(distance))
            .map(|(horizontal, upward)| {
                let away = [
                    own.feet[0] - self.position[0],
                    own.feet[2] - self.position[2],
                ];
                let [x, z] = [away, [self.direction[0], self.direction[2]]]
                    .into_iter()
                    .find_map(|[x, z]| {
                        let length = x.hypot(z);
                        (length > 0.0).then(|| [x / length, z / length])
                    })
                    .unwrap_or_default();
                [horizontal * x, upward, horizontal * z]
            });
        (damage > 0.0 || knockback.is_some()).then_some(OwnBlast {
            distance,
            damage,
            loadout: blast.hit.loadout,
            knockback,
        })
    }
}

/// The closest body or surface along a sweep: its distance and the body.
fn closest(
    origin: Vec3,
    direction: Vec3,
    reach: f64,
    targets: &[(u32, TargetPose)],
    wall: &mut impl FnMut(Vec3, Vec3, f64) -> Result<Option<f64>, IndexError>,
) -> Result<Option<(f64, Option<u32>)>, IndexError> {
    let mut closest = None;
    for (entity, target) in targets {
        if let Some((distance, _)) = target.entry(origin, direction, reach) {
            let entity = *entity;
            if closest.is_none_or(|(best, _)| distance < best) {
                closest = Some((distance, Some(entity)));
            }
        }
    }
    if let Some(distance) =
        wall(origin, direction, reach)?.filter(|distance| (0.0..=reach).contains(distance))
    {
        if closest.is_none_or(|(best, _)| distance < best) {
            closest = Some((distance, None));
        }
    }
    Ok(closest)
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
    use orpheus_physics::collision::index::{SceneTriangle, TriangleIndex};
    use orpheus_script::weapons::{
        AllyHits, Blast, DamageFalloff, DamageTags, HitDefinition, Knockback, STICK_FUSE,
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

    /// Soldier: 76's Helix Rocket as graph `0254` node 35 has it.
    fn helix() -> ProjectileBallistics {
        ProjectileBallistics {
            speed: 50.0,
            lifetime: 2.0,
            definition: Some(0x0400_0000_0000_072c),
            direct: Some(hit(40.0)),
            blast: Some(Blast {
                hit: hit(80.0),
                rings: vec![(1.0, 1.0), (3.0, 0.5)],
                self_damage: 0.5,
                knockback: Some(Knockback {
                    horizontal: 4.0,
                    upward: 6.0,
                    horizontal_rings: vec![(0.5, 1.0), (3.0, 0.75)],
                    upward_rings: vec![(0.5, 1.0), (3.0, 0.25)],
                }),
            }),
            gravity: 0.0,
            fuse: None,
            unmodelled: Vec::new(),
        }
    }

    /// Tracer's Pulse Bomb as graph `0034` state 0 has it in the Practice
    /// Range: 15 m/s, 5 damage where it sticks, 350 within 2 m falling to
    /// ×0.2 at 4 m. Its gravity (30) is left out where a test aims straight.
    fn pulse_bomb(gravity: f64) -> ProjectileBallistics {
        ProjectileBallistics {
            speed: 15.0,
            lifetime: 5.0,
            definition: Some(0x0400_0000_0000_0045),
            direct: Some(hit(5.0)),
            blast: Some(Blast {
                hit: hit(350.0),
                rings: vec![(2.0, 1.0), (4.0, 0.2)],
                self_damage: 0.0,
                knockback: None,
            }),
            gravity,
            fuse: Some(STICK_FUSE),
            unmodelled: Vec::new(),
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

    fn no_wall(_: Vec3, _: Vec3, _: f64) -> Result<Option<f64>, IndexError> {
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

    fn cast(
        world: &TriangleIndex,
    ) -> impl FnMut(Vec3, Vec3, f64) -> Result<Option<f64>, IndexError> + '_ {
        |origin, direction, reach| {
            world
                .cast_ray(origin, direction, reach, 0x18)
                .map(|hit| hit.map(|hit| hit.distance))
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
                .advance(1000, &targets, |_, _, _| panic!("duplicate tick"))
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
                .advance(1005, &targets, |_, _, _| panic!("already ended"))
                .unwrap(),
            FlightStep::default()
        );
    }

    #[test]
    fn walls_win_closer_impact_and_last_partial_tick_is_swept() {
        let world = wall_at(1.0);
        let mut flight =
            ProjectileFlight::new(helix(), [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        let step = flight
            .advance(1002, &[(1, target(0.0, 3.0))], cast(&world))
            .unwrap();
        assert_eq!(step.end.as_ref().unwrap().point, [0.0, 1.0, 1.0]);
        // The splash reaches the capsule beyond the wall.
        assert_eq!(damage(&step), [(1, 70.0)]);
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

    /// A Helix Rocket fired straight down from the eye bursts on the floor
    /// at its shooter's feet: he takes half its 80 and goes up at 6 m/s
    /// (no way away from it in the ground plane, nor along its flight).
    /// Fired level at a wall 1 m ahead, it pushes him back from the wall,
    /// by the knockback rings at his distance. A rocket that runs out of
    /// time doesn't explode and does nothing to him.
    #[test]
    fn a_helix_hurts_and_pushes_its_own_shooter() {
        let own = target(0.0, 0.0);
        let mut flight =
            ProjectileFlight::new(helix(), [0.0, 1.55, 0.0], [0.0, -1.0, 0.0], Some(1000)).unwrap();
        let step = flight.advance(1002, &[], cast(&floor())).unwrap();
        assert!(step.end.unwrap().point[1].abs() < 1e-9);
        let blast = flight.own_blast(&own).unwrap();
        assert!(blast.distance < 1e-9);
        assert_eq!(
            (blast.damage, blast.loadout, blast.knockback),
            (40.0, Some(LOADOUT), Some([0.0, 6.0, 0.0]))
        );

        let mut flight =
            ProjectileFlight::new(helix(), [0.0, 1.55, 0.0], [0.0, 0.0, 1.0], Some(1000)).unwrap();
        let step = flight.advance(1002, &[], cast(&wall_at(1.0))).unwrap();
        assert_eq!(step.end.unwrap().point, [0.0, 1.55, 1.0]);
        let blast = flight.own_blast(&own).unwrap();
        // From the capsule's top end, 1.5 m up, less its radius.
        assert!((blast.distance - (1.0f64.hypot(0.05) - 0.5)).abs() < 1e-12);
        assert_eq!(blast.damage, 40.0);
        let (back, up) = helix()
            .blast
            .unwrap()
            .knockback
            .unwrap()
            .at(blast.distance)
            .unwrap();
        assert!(back < 4.0 && up < 6.0);
        assert_eq!(blast.knockback, Some([0.0, up, -back]));

        let short = ProjectileBallistics {
            lifetime: 0.01,
            ..helix()
        };
        let mut flight =
            ProjectileFlight::new(short, [0.0, 1.55, 0.0], [0.0, 0.0, 1.0], None).unwrap();
        assert!(flight.advance(1000, &[], no_wall).unwrap().end.is_some());
        assert_eq!(flight.own_blast(&own), None);
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
    #[test]
    fn a_body_with_shapes_is_swept_and_blasted_by_its_shapes() {
        use crate::hitboxes::HitShape;
        let head = HitShape {
            a: [0.0, 1.0, 0.0],
            b: [0.0, 1.0, 0.0],
            radius: 0.3,
            critical: true,
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
                    |_, _, _| panic!("a stuck projectile sweeps nothing"),
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
    /// reaches a body behind the wall; a bomb that hits nothing in its
    /// lifetime ends without one.
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
        let mut steps = (1000..1200).map(|tick| {
            flight
                .advance(tick, std::slice::from_ref(&bot), cast(&world))
                .unwrap()
        });
        let step = steps
            .find(|step| step.end.is_some() || !step.impacts.is_empty())
            .unwrap();
        assert_eq!(damage(&step), [(1, 350.0)]);
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
            lifetime: 500.0 / 80.0,
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
            gravity: 0.0,
            fuse: None,
            unmodelled: Vec::new(),
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
}
