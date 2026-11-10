//! Server-side movement for an authored 104319 character capsule.

pub mod correction;
pub mod ground;
pub mod kinematics;
pub mod modifiers;
pub(crate) mod posture;
pub mod settings;

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    collision::index::TriangleIndex,
    contact::body::CharacterBody,
    contact::gjk::PointSetSize,
    contact::hull::HullContactError,
    contact::solver::clip_velocity,
    contact::world::ContactWorld,
    mover::ground::{GroundProbe, ProbePose, pogo, standing_body},
    mover::kinematics::{Ground, MotionInput, Poses, VelocityStep, poses},
    mover::modifiers::{Modifier, ModifierKey, ModifierTimeline},
    mover::posture::{
        CROUCHED, FELL, IN_AIR, JUMPED, NOT_GROUND_UNDER, Posture, PostureStep, TOUCHING, snap,
    },
    mover::settings::MoverSettings,
};
use orpheus_wire::{
    commands::Command,
    entity::{Movement, NO_INPUT, POSITION_UNITS},
};

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
    /// The last pose was a world-target move (`Mover.driven` in
    /// `ow174/game/mover.py`).
    driven: bool,
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
        Ok(Self {
            base,
            category,
            probe: GroundProbe::new(&base)?,
            body: standing_body(&base, world, position, category),
            posture: Posture::new(&base),
            modifiers: ModifierTimeline::default(),
            last_command: None,
            last_input: None,
            pose: Movement {
                position,
                yaw,
                gravity: base.gravity_scale,
                ..Movement::default()
            },
            driven: false,
        })
    }

    pub fn pose(&self) -> Movement {
        self.pose
    }

    pub fn crouched(&self) -> bool {
        self.posture.crouched()
    }

    pub fn track_modifiers(&mut self, frame: u32, active: &BTreeMap<ModifierKey, Modifier>) {
        self.modifiers.track(frame, active);
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
    /// `move_to` (a feet target and a speed in metres per second) the body
    /// sweeps toward the target instead of following the input and the result
    /// is `Some(true)` once it arrived, `Some(false)` once it is blocked, and
    /// `None` while it is still on its way. A target the speed can't move a
    /// whole grid step this frame is ignored: no fractional progress is kept
    /// between poses. The first pose without a target after driven ones starts
    /// from rest.
    pub fn step_pose_to(
        &mut self,
        rays: &TriangleIndex,
        command: Command,
        dt: f64,
        move_to: Option<([f64; 3], f64)>,
    ) -> Result<Option<bool>, HullContactError> {
        let move_to = move_to.filter(|&(_, speed)| {
            (speed * dt).partial_cmp(&STEP) != Some(std::cmp::Ordering::Less)
        });
        if self.driven && move_to.is_none() {
            self.pose.velocity = [0.0; 3];
        }
        self.driven = move_to.is_some();
        let active = self.modifiers.at(command.frame);
        let settings = active.settings(&self.base);
        let input = MotionInput::from(&command);
        let probe = self.probe.probe(
            rays,
            self.body.world(),
            &settings,
            ProbePose {
                feet: self.pose.position,
                velocity: self.pose.velocity,
                spring: self.posture.spring_offset(),
                crouched: self.posture.crouched(),
                airborne: self.posture.ground() != Ground::Standing,
            },
            dt,
            self.category,
        )?;
        self.posture.advance(
            PostureStep {
                command: &command,
                base: &self.base,
                values: &settings,
                mods: &active,
                probe,
                dt,
                pitch_held: input.pitch_held,
                driven: move_to.is_some(),
            },
            &mut self.pose.position,
            &mut self.pose.velocity,
            |radius, top| self.body.try_resize(radius, top),
        )?;
        let outcome = match move_to {
            None => {
                let ground = self.posture.ground();
                let velocity = VelocityStep {
                    base: &self.base,
                    settings: &settings,
                    input,
                    ground,
                    crouched: self.posture.crouched(),
                    dt,
                }
                .advance(self.pose.velocity);
                if ground != Ground::Standing {
                    self.posture.apply_fall(settings.gravity_scale, dt);
                }
                let normal = probe.ground.map_or([0.0, 1.0, 0.0], |hit| hit.normal);
                let vertical =
                    self.move_body(rays, velocity, normal, ground == Ground::Standing, dt)?;
                self.posture.finish_fall(settings.gravity_scale, vertical);
                None
            }
            Some((target, speed)) => self.move_to(rays, dt, target, speed)?,
        };

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
        self.pose.flags = self.posture.flags;
        self.pose.air_ticks = self.posture.air_ticks;
        self.pose.crouch_frame = self.posture.crouch_end;
        self.pose.gravity = settings.gravity_scale;
        self.pose.spring = self.posture.spring_steps().map(i32::from);
        self.pose.spring_speed = f64::from(self.posture.spring_speed());
        self.pose.fall = self.posture.fall;
        Ok(outcome)
    }

    /// Sweep a speed-limited displacement toward a world feet position
    /// (`Mover._move_to` in `ow174/game/mover.py`): `Some(true)` arrived,
    /// `Some(false)` blocked (less than half a grid step of progress), `None`
    /// still moving.
    fn move_to(
        &mut self,
        rays: &TriangleIndex,
        dt: f64,
        target: [f64; 3],
        speed: f64,
    ) -> Result<Option<bool>, HullContactError> {
        let start = self.pose.position;
        let [dx, dy, dz] = std::array::from_fn(|axis| target[axis] - start[axis]);
        let distance = dx.hypot(dy).hypot(dz);
        if distance > STEP {
            let scale = (speed * dt / distance).min(1.0);
            let [x, y, z] = start;
            let [bx, by, bz] = self.body.position().map(f64::from);
            let lift = f64::from(self.lift());
            let displacement = [
                (x + dx * scale - bx) as f32,
                (y + dy * scale + lift - by) as f32,
                (z + dz * scale - bz) as f32,
            ];
            let movement = self.body.move_capsule(displacement, dt as f32)?;
            let (mut end, touching) = (movement.position, movement.touching);
            // The spring holds the capsule above the feet. On descent the
            // feet still stop at walkable ground even when the capsule could
            // move lower.
            let lift = self.lift();
            let feet = f64::from(end[1]) - f64::from(lift);
            if dy < 0.0 && feet < y {
                if let Some(ground) = rays
                    .ground(
                        f64::from(end[0]),
                        f64::from(end[2]),
                        y + STEP,
                        feet,
                        self.category,
                    )
                    .filter(|&hit| self.probe.walkable(hit))
                {
                    end[1] = (ground.y + f64::from(lift)) as f32;
                }
            }
            self.place(rays, end, touching);
        }
        let position = self.pose.position;
        let [rx, ry, rz] = std::array::from_fn(|axis| position[axis] - target[axis]);
        let remaining = rx.hypot(ry).hypot(rz);
        self.posture.fall = 0.0;
        if position[1] - start[1] > STEP && self.posture.flags & IN_AIR == 0 {
            self.posture.flags |= FELL;
            self.posture.air_ticks = 0;
        }
        if remaining <= STEP || distance - remaining < STEP / 2.0 {
            self.pose.velocity = [0.0; 3];
            self.driven = false;
            return Ok(Some(remaining <= STEP));
        }
        self.pose.velocity = std::array::from_fn(|axis| snap((position[axis] - start[axis]) / dt));
        Ok(None)
    }

    /// The capsule's lowest point above the feet: the pogo plus the spring
    /// offset (`Mover._lift` in `ow174/game/mover.py`).
    fn lift(&self) -> f32 {
        pogo(&self.base, self.posture.crouched()) as f32 + self.posture.spring_offset()
    }

    fn move_body(
        &mut self,
        rays: &TriangleIndex,
        velocity: [f64; 3],
        normal: [f64; 3],
        grounded: bool,
        dt: f64,
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
            ((0.0 * rise + x) * dt) + 0.0,
            ((rise + y) * dt) + 0.0,
            ((0.0 * rise + z) * dt) + 0.0,
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
        self.pose.velocity = [x, y, z].map(snap);
        Ok(y)
    }

    /// Put the feet under the moved capsule, on the position grid and no lower
    /// than the abyss floor, and the capsule back over them; true when the
    /// feet were past that floor (`Mover._place` in `ow174/game/mover.py`).
    fn place(&mut self, rays: &TriangleIndex, position: [f32; 3], touching: bool) -> bool {
        self.posture.flags = (self.posture.flags & !TOUCHING) | (u32::from(touching) * TOUCHING);
        let lift = self.lift();
        let [px, py, pz] = position;
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
    use orpheus_wire::{
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

    /// Drive toward `target` at 5 m/s until the move ends; the outcomes.
    fn drive(mover: &mut Mover, rays: &TriangleIndex, target: [f64; 3]) -> Vec<Option<bool>> {
        let mut outcomes = Vec::new();
        for frame in 1..=100 {
            let outcome = mover
                .step_pose_to(rays, idle(frame), 0.016, Some((target, 5.0)))
                .unwrap();
            outcomes.push(outcome);
            if outcome.is_some() {
                break;
            }
        }
        outcomes
    }

    #[test]
    fn world_target_moves_arrive_or_report_blocking() {
        let rays = TriangleIndex::new([]).unwrap();
        let target = [1.0, 0.0, 0.0];

        let mut open = mover_in(Vec::new());
        let outcomes = drive(&mut open, &rays, target);
        let (last, moving) = outcomes.split_last().unwrap();
        assert_eq!(*last, Some(true));
        assert!(moving.len() > 10 && moving.iter().all(Option::is_none));
        assert_eq!(open.pose().position, target);
        assert_eq!(open.pose().velocity, [0.0; 3]);
        assert!(!open.driven);

        // A sphere across the path stops the capsule short of the target.
        let wall = StaticGeometry::Round {
            shape: RoundShape::Sphere(Sphere {
                center: [0.7, 1.0, 0.0],
                radius: 0.3,
            }),
            mask: 0x18,
        };
        let mut blocked = mover_in(vec![StaticShape::new(wall, None).unwrap()]);
        let outcomes = drive(&mut blocked, &rays, target);
        assert_eq!(outcomes.last(), Some(&Some(false)));
        assert!(blocked.pose().position[0] < 0.4);
        assert_eq!(blocked.pose().velocity, [0.0; 3]);
        assert!(!blocked.driven);
    }

    #[test]
    fn undriven_poses_start_from_rest_and_slow_targets_are_ignored() {
        let rays = TriangleIndex::new([]).unwrap();
        let mut mover = mover_in(Vec::new());
        let target = [1.0, 0.0, 0.0];
        assert_eq!(
            mover
                .step_pose_to(&rays, idle(1), 0.016, Some((target, 5.0)))
                .unwrap(),
            None
        );
        assert!(mover.driven);
        // 0.08 m on the 1/1024 grid is 82 steps; the velocity is that over dt, on the grid.
        let x = mover.pose().position[0];
        assert_eq!(x, 82.0 / 1024.0);
        assert_eq!(mover.pose().velocity, [5125.0 / 1024.0, 0.0, 0.0]);
        // A speed below one grid step per frame keeps no fractional progress:
        // the pose runs as if it had no target, from rest.
        assert_eq!(
            mover
                .step_pose_to(&rays, idle(2), 0.016, Some((target, 0.01)))
                .unwrap(),
            None
        );
        assert!(!mover.driven);
        let pose = mover.pose();
        assert_eq!((pose.velocity[0], pose.velocity[2]), (0.0, 0.0));
        assert_eq!(pose.position[0], x);
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
                    mover
                        .step_pose_to(&rays, command, 0.016, Some(([1.0, 0.0, 0.0], 5.0)))
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

    #[test]
    #[ignore = "requires a derived 104319 Practice Range movement cache"]
    fn practice_range_player_movement_records_match_python() {
        let runtime = orpheus_testkit::runtime_dir();
        let cache =
            read_movement_cache(File::open(runtime.join("collision/0688.col")).unwrap()).unwrap();
        let ids = BTreeSet::new();
        let scene = CollisionScene::movement_probe(&cache, &ids).unwrap();
        let world = Arc::new(ContactWorld::from_movement_cache(cache, &ids).unwrap());
        let catalog_path = runtime.join("data/game_movers_174.json");
        let settings = MoverCatalog::load(&catalog_path).unwrap();
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
            let body = orpheus_core::guid::parse_guid(guid).unwrap();
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
