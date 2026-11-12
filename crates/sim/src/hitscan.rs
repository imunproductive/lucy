//! The reference hitscan rule for retail 104319.
//!
//! Ports target rewind and direct-hit selection from ow174/game/combat.py.
//! Target eligibility and damage delivery belong to the match. Capsule and
//! static intersections use the existing collision kernels.
//!
//! Beyond the reference (which hit every body as one capsule): a body with
//! hit shapes ([`crate::hitboxes`]) is hit by them, turned by its facing,
//! and a hit is critical when the shape it enters first is. Only bots have
//! shapes for now, posed each tick by their animation
//! ([`crate::bot_animation`]), and a rewound shot meets the shapes of the
//! retained tick nearest its view; a player's body keeps the capsule (the
//! heroes' shapes are a rest pose, which cannot follow a crouch or an
//! animation).

use std::{
    collections::{HashMap, VecDeque},
    f64::consts::PI,
    fs, io,
    path::Path,
    sync::Arc,
};

use orpheus_core::guid::parse_guid;
use orpheus_physics::{
    collision::index::IndexError,
    static_ray::{Vec3, capsule_entry_distance, segment_distance},
};
use orpheus_script::weapons::{DamageTags, HitDefinition};
use orpheus_wire::commands::Command;
use serde::Deserialize;

use crate::hitboxes::HitShape;

pub const DEFAULT_REACH: f64 = 200.0;
const HISTORY: usize = 64;
const HEAD_BELOW_EYE: f64 = 0.2;
const ANGLE_STEP: f64 = std::f64::consts::TAU / 65536.0;

/// The derived ray profiles file, in the runtime `data` folder (the
/// `orpheus-assets` derivation `game_ray_profiles`).
pub const RAY_PROFILES_FILE: &str = "game_ray_profiles_174.json";

/// The map mask of a shot or projectile whose volley names no ray profile,
/// which is nearly every hitscan weapon's (Soldier: 76's rifle, Tracer's
/// pistols, Cassidy's Peacekeeper, Widowmaker's rifle). Assumption: the
/// client fills such a request with `STU_8EFC337B`'s class defaults, whose
/// `m_category` packs to it; every 104319 profile's map mask is this one or
/// `0x1F9F` (refs report `WeaponRays.md`, IDA question 1).
pub const DEFAULT_MAP_MASK: u16 = 0x1FDF;

/// Each ray profile's map mask (`097`, `STU_8EFC337B` `m_category` packed):
/// a map triangle or prop shape stops a shot or projectile of the profile
/// when its mask shares a bit with it.
#[derive(Debug, Default)]
pub struct RayProfiles {
    masks: HashMap<u64, u16>,
}

#[derive(Deserialize)]
struct RawRayProfiles {
    profiles: HashMap<String, u16>,
}

impl RayProfiles {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let invalid = |error: String| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: {error}", path.display()),
            )
        };
        let data = fs::read(path).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", path.display()))
        })?;
        let raw: RawRayProfiles =
            serde_json::from_slice(&data).map_err(|error| invalid(error.to_string()))?;
        let masks = raw
            .profiles
            .into_iter()
            .map(|(guid, mask)| {
                let guid = parse_guid(&guid).ok_or_else(|| invalid(format!("bad GUID {guid}")))?;
                Ok((guid, mask))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self { masks })
    }

    /// The map mask of a volley's profile: [`DEFAULT_MAP_MASK`] when it
    /// names none, or one the install lacks.
    pub fn map_mask(&self, profile: Option<u64>) -> u16 {
        profile
            .and_then(|profile| self.masks.get(&profile).copied())
            .unwrap_or(DEFAULT_MAP_MASK)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetPose {
    pub feet: Vec3,
    pub height: f64,
    pub radius: f64,
    pub eye: f64,
    /// The facing in radians: forward is `[sin, 0, cos]`.
    pub yaw: f64,
    /// The body's hit shapes in its frame; `None`: the capsule from the
    /// feet to `height`, critical from `eye` less 0.2 m up.
    pub shapes: Option<Arc<[HitShape]>>,
}

impl TargetPose {
    pub(crate) fn segment(&self) -> [Vec3; 2] {
        let [x, y, z] = self.feet;
        [
            [x, y + self.radius, z],
            [x, y + self.radius.max(self.height - self.radius), z],
        ]
    }

    /// A point of the body's frame in the world: turned by the facing about
    /// the feet (the frame's +Z to `[sin yaw, 0, cos yaw]`).
    fn place(&self, [x, y, z]: Vec3) -> Vec3 {
        let (sin, cos) = self.yaw.sin_cos();
        let [fx, fy, fz] = self.feet;
        [fx + x * cos + z * sin, fy + y, fz - x * sin + z * cos]
    }

    /// The body's shapes in the world: segment ends, radius, critical.
    fn placed(&self) -> impl Iterator<Item = (Vec3, Vec3, f64, bool)> + '_ {
        self.shapes
            .iter()
            .flat_map(|shapes| shapes.iter())
            .map(|shape| {
                (
                    self.place(shape.a),
                    self.place(shape.b),
                    shape.radius,
                    shape.critical,
                )
            })
    }

    /// Where a ray, or a sphere of radius `size` swept along it, first
    /// enters the body within `length`: the distance, and for a body with
    /// shapes whether the shape it enters is critical (the nearest shape;
    /// the earlier one of the list on a tie). `None` for the capsule, whose
    /// head zone the caller applies. A sphere meets a shape where its centre
    /// comes within the shape's radius plus `size`.
    pub(crate) fn entry(
        &self,
        origin: Vec3,
        direction: Vec3,
        length: f64,
        size: f64,
    ) -> Option<(f64, Option<bool>)> {
        if self.shapes.is_none() {
            let [a, b] = self.segment();
            return capsule_entry_distance(origin, direction, length, a, b, self.radius + size)
                .map(|distance| (distance, None));
        }
        self.placed()
            .filter_map(|(a, b, radius, critical)| {
                capsule_entry_distance(origin, direction, length, a, b, radius + size)
                    .map(|distance| (distance, Some(critical)))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
    }

    /// Whether a hit at height `impact_y` that entered the body as
    /// [`TargetPose::entry`] says (`entered`) struck the head: the entered
    /// shape's own flag, or on the capsule at or above the eye less 0.2 m
    /// (combat.py `_hitscan`).
    pub(crate) fn head(&self, entered: Option<bool>, impact_y: f64) -> bool {
        entered.unwrap_or_else(|| impact_y >= self.feet[1] + self.eye - HEAD_BELOW_EYE)
    }

    /// How far a point is from the body's surface, 0 inside it.
    pub(crate) fn surface_distance(&self, point: Vec3) -> f64 {
        if self.shapes.is_none() {
            let [a, b] = self.segment();
            return (segment_distance(point, a, b) - self.radius).max(0.0);
        }
        self.placed()
            .map(|(a, b, radius, _)| (segment_distance(point, a, b) - radius).max(0.0))
            .fold(f64::INFINITY, f64::min)
    }
}

/// The angle a `share` of the way from `from` to `to` along the shorter
/// turn.
fn turned(from: f64, to: f64, share: f64) -> f64 {
    let turn = (to - from + PI).rem_euclid(2.0 * PI) - PI;
    from + turn * share
}

#[derive(Clone, Copy, Debug)]
pub struct ViewFrame {
    command: u32,
    rewind: f64,
}

impl ViewFrame {
    pub fn from_command(command: &Command, quanta: u32) -> Self {
        assert!(quanta != 0);
        Self {
            command: command.frame,
            rewind: command.view_delay(quanta) * 1_000_000.0 / f64::from(quanta),
        }
    }

    /// The poses of a world tick, as a bot's shot sees them (combat.py
    /// `_shoot`: `view_tick = self.tick` without a command).
    pub fn at(tick: u32) -> Self {
        Self {
            command: tick,
            rewind: 0.0,
        }
    }

    fn distance_to(self, frame: u32) -> f64 {
        f64::from(frame.wrapping_sub(self.command) as i32) + self.rewind
    }
}

/// History belongs to one body generation. Replacements discard its poses
/// and use their own start tick to reject shots aimed before they existed.
pub struct TargetHistory {
    replacement: Option<u32>,
    poses: VecDeque<(u32, TargetPose)>,
}

impl TargetHistory {
    pub fn new(replacement: Option<u32>) -> Self {
        Self {
            replacement,
            poses: VecDeque::new(),
        }
    }

    pub fn record(&mut self, tick: u32, pose: TargetPose) {
        if let Some((last, latest)) = self.poses.back_mut() {
            match (tick.wrapping_sub(*last) as i32).cmp(&0) {
                std::cmp::Ordering::Less => return,
                std::cmp::Ordering::Equal => {
                    *latest = pose;
                    return;
                }
                std::cmp::Ordering::Greater => {}
            }
        }
        if self.poses.len() == HISTORY {
            self.poses.pop_front();
        }
        self.poses.push_back((tick, pose));
    }

    /// The body as a view saw it: its feet and facing between the two
    /// retained poses around the view's time, the posture (height, eye) and
    /// shapes of the nearer one (a bot's shapes are posed each tick,
    /// [`crate::bot_animation`]), the radius of the body now.
    pub fn at(&self, view: ViewFrame, current: TargetPose) -> Option<TargetPose> {
        if self
            .replacement
            .is_some_and(|tick| view.distance_to(tick) > 0.0)
        {
            return None;
        }
        let upper = self
            .poses
            .partition_point(|(tick, _)| view.distance_to(*tick) <= 0.0);
        let sampled = match (
            upper.checked_sub(1).and_then(|index| self.poses.get(index)),
            self.poses.get(upper),
        ) {
            (Some((earlier_tick, earlier)), Some((later_tick, later))) => {
                let share = -view.distance_to(*earlier_tick)
                    / f64::from(later_tick.wrapping_sub(*earlier_tick));
                let posture = if share < 0.5 { earlier } else { later };
                TargetPose {
                    feet: std::array::from_fn(|axis| {
                        earlier.feet[axis] + (later.feet[axis] - earlier.feet[axis]) * share
                    }),
                    height: posture.height,
                    eye: posture.eye,
                    yaw: turned(earlier.yaw, later.yaw, share),
                    shapes: posture.shapes.clone(),
                    ..current
                }
            }
            (Some((_, pose)), None) | (None, Some((_, pose))) => TargetPose {
                radius: current.radius,
                ..pose.clone()
            },
            (None, None) => current,
        };
        Some(sampled)
    }
}

pub struct HitscanShot {
    pub origin: Vec3,
    pub direction: Vec3,
    pub reach: f64,
    pub hit: HitDefinition,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Impact {
    pub entity: u32,
    pub distance: f64,
    pub damage: f64,
    pub critical: bool,
    pub loadout: Option<u64>,
    /// The health change's damage tags (the struck body's `0011.025`).
    pub tags: DamageTags,
    /// What the body takes as healing when it is an ally the hit strikes
    /// ([`HitDefinition::ally_heal`]); 0: an ally takes nothing.
    pub heal: f64,
}

impl HitscanShot {
    /// A shot along a direction from an origin: a player's aim, turned by
    /// his volley's spread, or a bot's yaw (combat.py `_shoot`).
    pub fn along(origin: Vec3, direction: Vec3, reach: f64, hit: HitDefinition) -> Self {
        Self {
            origin,
            direction,
            reach: if reach == 0.0 { DEFAULT_REACH } else { reach },
            hit,
        }
    }

    /// Find the first eligible body before any blocking map surface: the
    /// nearest entry into a capsule or, for a body with them, into one of
    /// its shapes. Iteration order breaks exact ties, as it does in the
    /// reference.
    pub fn resolve(
        &self,
        targets: impl IntoIterator<Item = (u32, TargetPose)>,
        wall_distance: impl FnOnce(Vec3, Vec3, f64) -> Result<Option<f64>, IndexError>,
    ) -> Result<Option<Impact>, IndexError> {
        if !self.hit.amount.is_finite() || self.hit.amount <= 0.0 {
            return Ok(None);
        }
        let mut nearest: Option<(f64, u32, TargetPose, Option<bool>)> = None;
        for (entity, pose) in targets {
            let length = nearest
                .as_ref()
                .map_or(self.reach, |(distance, ..)| *distance);
            if let Some((distance, critical)) = pose.entry(self.origin, self.direction, length, 0.0)
            {
                if nearest.is_none() || distance < length {
                    nearest = Some((distance, entity, pose, critical));
                    if distance == 0.0 {
                        break;
                    }
                }
            }
        }
        let Some((distance, entity, pose, critical)) = nearest else {
            return Ok(None);
        };
        if distance > 0.0
            && wall_distance(self.origin, self.direction, distance)?
                .is_some_and(|wall| wall < distance)
        {
            return Ok(None);
        }
        let head = pose.head(critical, self.origin[1] + self.direction[1] * distance);
        Ok(Some(Impact {
            entity,
            distance,
            damage: self.hit.damage_at(distance, head),
            critical: head && self.hit.critical > 1.0,
            loadout: self.hit.loadout,
            tags: self.hit.tags,
            heal: self.hit.ally_heal(),
        }))
    }
}

pub(crate) fn aim_direction(command: &Command) -> Vec3 {
    let (yaw, pitch) = command.aim();
    let (y_sin, y_cos) = (f64::from(yaw) * ANGLE_STEP).sin_cos();
    let (p_sin, p_cos) = (f64::from(pitch) * ANGLE_STEP).sin_cos();
    [y_sin * p_cos, -p_sin, y_cos * p_cos]
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_physics::collision::index::{SceneTriangle, TriangleIndex};
    use orpheus_script::weapons::{AllyHits, DamageFalloff};
    use orpheus_wire::commands::Click;

    const MOVER: u16 = 0x18;
    const RIFLE: HitDefinition = HitDefinition {
        amount: 20.0,
        loadout: Some(0x0b90_0000_0000_008d),
        critical: 2.0,
        falloff: Some(DamageFalloff {
            start: 30.0,
            end: 50.0,
            floor: 0.3,
        }),
        tags: DamageTags::NONE,
        allies: AllyHits::None,
    };

    fn standing(x: f64, z: f64) -> TargetPose {
        TargetPose {
            feet: [x, 0.0, z],
            height: 1.8,
            radius: 0.5,
            eye: 1.55,
            yaw: 0.0,
            shapes: None,
        }
    }

    #[test]
    fn selects_one_body_and_applies_map_occlusion_and_critical_damage() {
        let wall = |z: f64| {
            TriangleIndex::new([SceneTriangle {
                vertices: [[-5.0, 0.0, z], [5.0, 0.0, z], [0.0, 5.0, z]],
                mask: MOVER,
                surface: 3,
            }])
            .unwrap()
        };
        let blocked = wall(4.0);
        let behind = wall(11.0);
        let coincident = wall(9.5);
        let direct = HitscanShot::along(
            [0.0, 1.0, 0.0],
            aim_direction(&Command::default()),
            180.0,
            RIFLE,
        );
        let targets = [
            (1, standing(0.0, 12.0)),
            (2, standing(0.0, 10.0)),
            (3, standing(0.0, 10.0)),
        ];
        let resolve = |world: &TriangleIndex| {
            direct
                .resolve(targets.clone(), |origin, direction, distance| {
                    world
                        .cast_ray(origin, direction, distance, MOVER)
                        .map(|hit| hit.map(|hit| hit.distance))
                })
                .unwrap()
        };
        assert!(resolve(&blocked).is_none());
        let hit = resolve(&behind).unwrap();
        assert_eq!(
            hit,
            Impact {
                entity: 2,
                distance: 9.5,
                damage: 20.0,
                critical: false,
                loadout: RIFLE.loadout,
                tags: RIFLE.tags,
                heal: 0.0,
            }
        );
        assert_eq!(resolve(&coincident), Some(hit));
        let headshot = HitscanShot {
            origin: [0.0, 1.4, 0.0],
            ..direct
        };
        let hit = headshot
            .resolve(targets.clone(), |_, _, _| Ok(None))
            .unwrap()
            .unwrap();
        assert_eq!(hit.entity, 2);
        assert!(hit.critical);
        assert_eq!(hit.damage, 40.0);
        let inside = HitscanShot {
            origin: [0.0, 1.0, 10.0],
            ..headshot
        };
        assert_eq!(
            inside
                .resolve(targets.clone(), |_, _, _| panic!("inside capsule"))
                .unwrap()
                .unwrap()
                .distance,
            0.0
        );
        let short = HitscanShot {
            reach: 9.499,
            ..inside
        };
        let short = HitscanShot {
            origin: [0.0, 1.0, 0.0],
            ..short
        };
        assert!(
            short
                .resolve(targets, |_, _, _| panic!("out of range"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn uses_click_aim_and_interpolates_view_time_across_wrap() {
        let command = Command {
            frame: 1,
            yaw: 0,
            extra: 12,
            click: Some(Click {
                yaw: 16_384,
                pitch: 0,
                subframe: u8::MAX,
            }),
            ..Command::default()
        };
        let shot = HitscanShot::along([0.0, 1.0, 0.0], aim_direction(&command), 0.0, RIFLE);
        assert_eq!(shot.reach, DEFAULT_REACH);
        assert!((shot.direction[0] - 1.0).abs() < 1e-12 && shot.direction[2].abs() < 1e-12);
        let before = standing(8.0, 0.0);
        let crouched = TargetPose {
            height: 1.2,
            eye: 1.05,
            ..standing(12.0, 0.0)
        };
        let mut history = TargetHistory::new(Some(u32::MAX));
        history.record(u32::MAX, before);
        history.record(1, crouched.clone());
        history.record(0, standing(50.0, 0.0)); // a late sample cannot reorder the history
        let pose = history
            .at(ViewFrame::from_command(&command, 16_000), crouched.clone())
            .unwrap();
        assert_eq!(pose.feet, [10.5, 0.0, 0.0]);
        assert_eq!((pose.height, pose.eye), (1.2, 1.05));
        assert!(
            shot.resolve([(7, pose)], |_, _, _| Ok(None))
                .unwrap()
                .unwrap()
                .critical
        );
        let early = Command {
            extra: 48,
            ..command
        };
        assert!(
            history
                .at(ViewFrame::from_command(&early, 16_000), crouched)
                .is_none()
        );
    }

    #[test]
    fn history_clamps_to_retained_samples_and_keeps_current_radius() {
        let mut history = TargetHistory::new(None);
        for tick in 0..70 {
            history.record(tick, standing(f64::from(tick), 0.0));
        }
        let current = TargetPose {
            radius: 0.4,
            ..standing(100.0, 0.0)
        };
        let seen = |frame| {
            history
                .at(
                    ViewFrame::from_command(
                        &Command {
                            frame,
                            ..Command::default()
                        },
                        16_000,
                    ),
                    current.clone(),
                )
                .unwrap()
        };
        assert_eq!((seen(0).feet[0], seen(0).radius), (6.0, 0.4));
        assert_eq!(seen(69).feet, [69.0, 0.0, 0.0]);
        assert_eq!(seen(100).feet, [69.0, 0.0, 0.0]);
    }

    fn sphere(centre: Vec3, radius: f64, critical: bool) -> HitShape {
        HitShape {
            a: centre,
            b: centre,
            radius,
            critical,
        }
    }

    fn shaped(feet: Vec3, yaw: f64, shapes: &[HitShape]) -> TargetPose {
        TargetPose {
            feet,
            yaw,
            shapes: Some(shapes.into()),
            ..standing(0.0, 0.0)
        }
    }

    fn level(origin: Vec3, direction: Vec3) -> HitscanShot {
        HitscanShot::along(origin, direction, 180.0, RIFLE)
    }

    fn first(shot: &HitscanShot, targets: &[(u32, TargetPose)]) -> Option<Impact> {
        shot.resolve(targets.iter().cloned(), |_, _, _| Ok(None))
            .unwrap()
    }

    #[test]
    fn a_body_with_shapes_is_hit_by_them_turned_by_its_facing() {
        // An arm out to the body's right (+X), as the capsule would not
        // have it.
        let arm = [HitShape {
            a: [0.4, 1.2, 0.0],
            b: [0.9, 1.2, 0.0],
            radius: 0.1,
            critical: false,
        }];
        let shot = level([0.8, 1.2, -5.0], [0.0, 0.0, 1.0]);
        let facing_ahead = shaped([0.0; 3], 0.0, &arm);
        let hit = first(&shot, &[(1, facing_ahead)]).unwrap();
        assert!((hit.distance - 4.9).abs() < 1e-9, "{hit:?}");
        assert!(!hit.critical);
        // A quarter turn swings the arm to -Z: the same shot passes by,
        // where the capsule (radius 0.5) would not have hit either way.
        let turned = shaped([0.0; 3], std::f64::consts::FRAC_PI_2, &arm);
        assert!(first(&shot, &[(1, turned.clone())]).is_none());
        let along_the_arm = level([-5.0, 1.2, -0.8], [1.0, 0.0, 0.0]);
        let hit = first(&along_the_arm, &[(1, turned)]).unwrap();
        assert!((hit.distance - 4.9).abs() < 1e-9, "{hit:?}");
    }

    #[test]
    fn only_the_critical_shape_is_critical() {
        let body = [
            sphere([0.0, 1.2, 0.0], 0.4, false),
            sphere([0.0, 1.9, 0.0], 0.15, true),
        ];
        let target = [(1, shaped([0.0, 0.0, 10.0], 0.0, &body))];
        let at = |height| first(&level([0.0, height, 0.0], [0.0, 0.0, 1.0]), &target);
        let head = at(1.9).unwrap();
        assert!(head.critical);
        assert_eq!(head.damage, 40.0);
        assert!((head.distance - 9.85).abs() < 1e-9);
        // Above the capsule's head line (eye 1.55 less 0.2) but on the body.
        let chest = at(1.45).unwrap();
        assert!(!chest.critical);
        assert_eq!(chest.damage, 20.0);
        assert!(at(2.1).is_none());
    }

    #[test]
    fn the_nearest_shape_of_all_bodies_wins() {
        // A shield arm in front of the head: the shot enters the arm first.
        let body = [
            sphere([0.0, 1.6, 0.0], 0.2, true),
            sphere([0.0, 1.6, -0.5], 0.1, false),
        ];
        let near = shaped([0.0, 0.0, 10.0], 0.0, &body);
        let shot = level([0.0, 1.6, 0.0], [0.0, 0.0, 1.0]);
        let hit = first(&shot, &[(1, near.clone())]).unwrap();
        assert!((hit.distance - 9.4).abs() < 1e-9);
        assert!(!hit.critical);
        // Turned around, the arm is behind the head.
        let back = shaped([0.0, 0.0, 10.0], std::f64::consts::PI, &body);
        let hit = first(&shot, &[(1, back)]).unwrap();
        assert!((hit.distance - 9.8).abs() < 1e-9);
        assert!(hit.critical);
        // A capsule body in front of it takes the shot, by the eye rule: its
        // top cap (1.3 m up, 0.5 m round) 0.4 m before its axis at 1.6 m.
        let hit = first(&shot, &[(1, near), (2, standing(0.0, 8.0))]).unwrap();
        assert_eq!((hit.entity, hit.critical), (2, true));
        assert!((hit.distance - 7.6).abs() < 1e-9, "{hit:?}");
    }

    #[test]
    fn history_rewinds_the_facing_along_the_shorter_turn() {
        let body = [sphere([0.0, 1.0, 0.0], 0.3, false)];
        let mut history = TargetHistory::new(None);
        history.record(10, shaped([0.0; 3], 3.0, &body));
        history.record(12, shaped([2.0, 0.0, 0.0], -3.0, &body));
        let current = shaped([5.0, 0.0, 0.0], 1.0, &body);
        let seen = |frame| {
            let command = Command {
                frame,
                ..Command::default()
            };
            history
                .at(ViewFrame::from_command(&command, 16_000), current.clone())
                .unwrap()
        };
        let middle = seen(11);
        assert_eq!(middle.feet, [1.0, 0.0, 0.0]);
        // From 3.0 to -3.0 rad through pi, not through 0.
        assert!(
            (middle.yaw - std::f64::consts::PI).abs() < 1e-12,
            "{}",
            middle.yaw
        );
        assert_eq!(middle.shapes.as_deref(), Some(&body[..]));
        assert_eq!(seen(10).yaw, 3.0);
        assert_eq!(seen(20).yaw, -3.0);
    }

    /// A bot's shapes are posed each tick; a rewound shot meets the pose
    /// of the retained tick nearest its view, not the body's pose now.
    #[test]
    fn history_rewinds_the_pose() {
        let low = [sphere([0.0, 1.0, 0.0], 0.3, false)];
        let high = [sphere([0.0, 2.0, 0.0], 0.3, true)];
        let mut history = TargetHistory::new(None);
        history.record(10, shaped([0.0; 3], 0.0, &low));
        history.record(12, shaped([0.0; 3], 0.0, &high));
        let current = shaped([0.0; 3], 0.0, &[sphere([0.0, 3.0, 0.0], 0.3, false)]);
        let seen = |frame| {
            let command = Command {
                frame,
                ..Command::default()
            };
            history
                .at(ViewFrame::from_command(&command, 16_000), current.clone())
                .unwrap()
        };
        assert_eq!(seen(10).shapes.as_deref(), Some(&low[..]));
        assert_eq!(seen(12).shapes.as_deref(), Some(&high[..]));
        assert_eq!(seen(20).shapes.as_deref(), Some(&high[..]));
        let at = |height: f64, frame| {
            first(
                &level([0.0, height, -5.0], [0.0, 0.0, 1.0]),
                &[(1, seen(frame))],
            )
        };
        assert!(at(1.0, 10).is_some_and(|hit| !hit.critical));
        assert!(at(1.0, 12).is_none());
        assert!(at(2.0, 12).is_some_and(|hit| hit.critical));
        assert!(at(3.0, 12).is_none());
    }

    /// The reference's capsule rule (combat.py `ray_capsule`, the head from
    /// the eye less 0.2 m), which players' bodies keep, on a Practice Range
    /// bot's capsule. The bot itself is now hit by its shapes (below).
    #[test]
    #[ignore = "requires a derived 104319 Practice Range scene"]
    fn practice_capsule_hits_match_reference_geometry() {
        use crate::scene_assets::SceneAssets;
        use orpheus_content::maps::GameMap;

        let root = orpheus_testkit::runtime_dir();
        let scene = SceneAssets::load(&root)
            .unwrap()
            .prepare(GameMap::practice_range())
            .unwrap();
        let bot = scene.bots().iter().find(|bot| bot.spawn.team == 1).unwrap();
        let target = TargetPose {
            shapes: None,
            ..scene.bot_target(bot)
        };
        let trace = |yaw, distance, height, frame| {
            let [x, y, z] = target.feet;
            let origin = if yaw == 0 {
                [x, y + height, z - distance]
            } else {
                [x - distance, y + height, z]
            };
            let command = Command {
                yaw,
                frame,
                ..Command::default()
            };
            HitscanShot::along(origin, aim_direction(&command), 180.0, RIFLE)
                .resolve(
                    [(bot.entity, target.clone())],
                    |origin, direction, length| scene.hitscan_wall(origin, direction, length, None),
                )
                .unwrap()
        };
        assert!(trace(0, 8.0, 1.0, 1000).is_none());
        let chest = trace(16_384, 8.0, 1.0, 1000).unwrap();
        assert_eq!(
            (chest.entity, chest.distance, chest.damage),
            (bot.entity, 7.5, 20.0)
        );
        let far = trace(16_384, 40.0, 1.55, 1000).unwrap();
        assert!(far.critical);
        assert!((far.distance - 39.502_506_281_446_74).abs() < 1e-9);
        assert!((far.damage - 26.696_491_205_974_56).abs() < 1e-9);
    }

    /// A Practice Range Training Bot as its hit shapes place it, posed at
    /// the start of its clock (its idle pose, hovering 0.5 m up with its
    /// arms down, and the first frame of its bob), facing its yaw: a shot from
    /// its side passing under it 0.15 m above its feet misses, where its
    /// capsule (from the feet) took it; a shot at its 2 m eye hits the head
    /// and is critical; one at 1.5 m, where the rest pose had the head,
    /// hits the chest and is not.
    #[test]
    #[ignore = "requires a derived 104319 Practice Range scene"]
    fn a_practice_bot_is_hit_by_its_shapes() {
        use crate::scene_assets::SceneAssets;
        use orpheus_content::maps::GameMap;

        let scene = SceneAssets::load(orpheus_testkit::runtime_dir())
            .unwrap()
            .prepare(GameMap::practice_range())
            .unwrap();
        let bot = scene.bots().iter().find(|bot| bot.spawn.team == 1).unwrap();
        let target = scene.bot_target(bot);
        assert_eq!(target.shapes.as_ref().map(|shapes| shapes.len()), Some(7));
        let (sin, cos) = target.yaw.sin_cos();
        let forward = [sin, 0.0, cos];
        // The body's +X, to one side.
        let side = [cos, 0.0, -sin];
        let point = |ahead: f64, aside: f64, height: f64| -> Vec3 {
            std::array::from_fn(|axis| {
                target.feet[axis]
                    + forward[axis] * ahead
                    + side[axis] * aside
                    + if axis == 1 { height } else { 0.0 }
            })
        };
        let shoot = |pose: &TargetPose, origin: Vec3, direction: Vec3| {
            HitscanShot::along(origin, direction, 180.0, RIFLE)
                .resolve([(bot.entity, pose.clone())], |_, _, _| Ok(None))
                .unwrap()
        };
        let under = point(0.0, -8.0, 0.15);
        assert!(shoot(&target, under, side).is_none());
        let capsule = TargetPose {
            shapes: None,
            ..target.clone()
        };
        assert!(shoot(&capsule, under, side).is_some());
        let back = forward.map(|part| -part);
        let head = shoot(&target, point(8.0, 0.0, 2.0), back).unwrap();
        assert!(head.critical, "{head:?}");
        assert_eq!(head.damage, 40.0);
        // The head sphere: 0.44 m round, its centre 1.98 m up; the bot is
        // posed at the start of its clock, where the hover bob (`391F`)
        // leans the head 1.5 cm back from the idle pose (7.57 m).
        assert!((head.distance - 7.58).abs() < 0.01, "{head:?}");
        let chest = shoot(&target, point(8.0, 0.0, 1.5), back).unwrap();
        assert!(!chest.critical, "{chest:?}");
        assert_eq!(chest.damage, 20.0);
        // The chest sphere: 0.48 m round, its centre 1.37 m up.
        assert!((chest.distance - 7.54).abs() < 0.01, "{chest:?}");
    }
}
