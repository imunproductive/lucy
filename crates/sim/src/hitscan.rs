//! The reference hitscan rule for retail 104319.
//!
//! Ports target rewind and direct-hit selection from ow174/game/combat.py.
//! Target eligibility and damage delivery belong to the match. Capsule and
//! static intersections use the existing collision kernels.

use std::collections::VecDeque;

use orpheus_physics::{
    collision::index::IndexError,
    static_ray::{Vec3, capsule_entry_distance},
};
use orpheus_script::weapons::HitDefinition;
use orpheus_wire::commands::Command;

pub const DEFAULT_REACH: f64 = 200.0;
const HISTORY: usize = 64;
const HEAD_BELOW_EYE: f64 = 0.2;
const ANGLE_STEP: f64 = std::f64::consts::TAU / 65536.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetPose {
    pub feet: Vec3,
    pub height: f64,
    pub radius: f64,
    pub eye: f64,
}

impl TargetPose {
    pub(crate) fn segment(self) -> [Vec3; 2] {
        let [x, y, z] = self.feet;
        [
            [x, y + self.radius, z],
            [x, y + self.radius.max(self.height - self.radius), z],
        ]
    }
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
            (Some(&(earlier_tick, earlier)), Some(&(later_tick, later))) => {
                let share = -view.distance_to(earlier_tick)
                    / f64::from(later_tick.wrapping_sub(earlier_tick));
                let posture = if share < 0.5 { earlier } else { later };
                TargetPose {
                    feet: std::array::from_fn(|axis| {
                        earlier.feet[axis] + (later.feet[axis] - earlier.feet[axis]) * share
                    }),
                    height: posture.height,
                    eye: posture.eye,
                    ..current
                }
            }
            (Some(&(_, pose)), None) | (None, Some(&(_, pose))) => TargetPose {
                radius: current.radius,
                ..pose
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
}

impl HitscanShot {
    pub fn from_command(origin: Vec3, command: &Command, reach: f64, hit: HitDefinition) -> Self {
        Self {
            origin,
            direction: aim_direction(command),
            reach: if reach == 0.0 { DEFAULT_REACH } else { reach },
            hit,
        }
    }

    /// Find the first eligible capsule before any blocking map surface.
    /// Iteration order breaks exact ties, as it does in the reference.
    pub fn resolve(
        &self,
        targets: impl IntoIterator<Item = (u32, TargetPose)>,
        wall_distance: impl FnOnce(Vec3, Vec3, f64) -> Result<Option<f64>, IndexError>,
    ) -> Result<Option<Impact>, IndexError> {
        if !self.hit.amount.is_finite() || self.hit.amount <= 0.0 {
            return Ok(None);
        }
        let mut nearest = None;
        for (entity, pose) in targets {
            let [a, b] = pose.segment();
            let length = nearest.map_or(self.reach, |(distance, _, _)| distance);
            if let Some(distance) =
                capsule_entry_distance(self.origin, self.direction, length, a, b, pose.radius)
            {
                if nearest.is_none() || distance < length {
                    nearest = Some((distance, entity, pose));
                    if distance == 0.0 {
                        break;
                    }
                }
            }
        }
        let Some((distance, entity, pose)) = nearest else {
            return Ok(None);
        };
        if distance > 0.0
            && wall_distance(self.origin, self.direction, distance)?
                .is_some_and(|wall| wall < distance)
        {
            return Ok(None);
        }
        let impact_y = self.origin[1] + self.direction[1] * distance;
        let head = impact_y >= pose.feet[1] + pose.eye - HEAD_BELOW_EYE;
        Ok(Some(Impact {
            entity,
            distance,
            damage: self.hit.damage_at(distance, head),
            critical: head && self.hit.critical > 1.0,
            loadout: self.hit.loadout,
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
    use orpheus_script::weapons::DamageFalloff;
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
    };

    fn standing(x: f64, z: f64) -> TargetPose {
        TargetPose {
            feet: [x, 0.0, z],
            height: 1.8,
            radius: 0.5,
            eye: 1.55,
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
        let direct = HitscanShot::from_command([0.0, 1.0, 0.0], &Command::default(), 180.0, RIFLE);
        let targets = [
            (1, standing(0.0, 12.0)),
            (2, standing(0.0, 10.0)),
            (3, standing(0.0, 10.0)),
        ];
        let resolve = |world: &TriangleIndex| {
            direct
                .resolve(targets, |origin, direction, distance| {
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
                loadout: RIFLE.loadout
            }
        );
        assert_eq!(resolve(&coincident), Some(hit));
        let headshot = HitscanShot {
            origin: [0.0, 1.4, 0.0],
            ..direct
        };
        let hit = headshot
            .resolve(targets, |_, _, _| Ok(None))
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
                .resolve(targets, |_, _, _| panic!("inside capsule"))
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
        let shot = HitscanShot::from_command([0.0, 1.0, 0.0], &command, 0.0, RIFLE);
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
        history.record(1, crouched);
        history.record(0, standing(50.0, 0.0)); // a late sample cannot reorder the history
        let pose = history
            .at(ViewFrame::from_command(&command, 16_000), crouched)
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
                    current,
                )
                .unwrap()
        };
        assert_eq!((seen(0).feet[0], seen(0).radius), (6.0, 0.4));
        assert_eq!(seen(69).feet, [69.0, 0.0, 0.0]);
        assert_eq!(seen(100).feet, [69.0, 0.0, 0.0]);
    }

    #[test]
    #[ignore = "requires a prepared 104319 Practice Range scene"]
    fn practice_capsule_hits_and_health_notices_match_reference_geometry() {
        use crate::scene_assets::SceneAssets;
        use orpheus_content::maps::GameMap;
        use orpheus_core::bits::BitReader;
        use orpheus_script::vitals::{Death, Vitals};
        use orpheus_wire::{
            combat_messages::CombatNotice,
            jam::codec::{Schemas, Value},
            messages::{read_channel, write_channel},
        };

        let root = orpheus_testkit::runtime_dir();
        let scene = SceneAssets::load(&root)
            .unwrap()
            .prepare(GameMap::practice_range())
            .unwrap();
        let schemas = Schemas::load(root.join("data/game_schemas_174.json")).unwrap();
        let bot = scene.bots().iter().find(|bot| bot.spawn.team == 1).unwrap();
        let target = scene.bot_target(bot);
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
            HitscanShot::from_command(origin, &command, 180.0, RIFLE)
                .resolve([(bot.entity, target)], |origin, direction, length| {
                    scene.hitscan_wall(origin, direction, length)
                })
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

        let mut vitals = Vitals::new(bot.spawn.health, 0.0, 0.0);
        let source = 0xa000_0101;
        for index in 0..5 {
            let frame = 1000 + index * 7;
            let hit = trace(16_384, 8.0, 1.55, frame).unwrap();
            let result = vitals.damage(
                hit.damage,
                Death {
                    time: f64::from(frame) * 0.016,
                    frame,
                    killer: source,
                },
            );
            assert_eq!((result.dealt, result.killed), (40.0, index == 4));
            assert_eq!(vitals.total(), 160.0 - f64::from(index) * 40.0);
            let notice = CombatNotice::Hit {
                target: hit.entity,
                dealt: result.dealt,
                critical: hit.critical,
                killed: result.killed,
                shot_frame: frame,
            };
            let mut bits = orpheus_core::bits::BitWriter::new();
            write_channel(&mut bits, &[(index, notice.encode(&schemas).unwrap())], &[]).unwrap();
            let mut reader = BitReader::new(bits.as_bytes());
            let channel = read_channel(&mut reader, &schemas).unwrap();
            let message = &channel.reliable[0].message;
            assert_eq!(message.fields[&0x84], Value::Unsigned(u64::from(frame)));
            assert_eq!(notice.required_identity(), Some(bot.entity));
            assert!(vitals.changed_component().is_some());
            vitals.clear_changed();
        }
        assert_eq!(vitals.death().unwrap().killer, source);
    }
}
