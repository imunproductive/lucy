//! Map AI spawners and the training bots' server-side patrol movement.
//!
//! The catalog and patrol behavior follow the Python reference.
//!
//! Research: the Practice Range has 15 spawners, 11 Training Bots on the red
//! team and 4 Friendly Bots on the blue one (retail's OW2 Practice Range
//! showed 15 bots too, each with a movement record in every frame, idle or
//! not). Graph 0B8E walks a route: variable #6947 (0D80/1B23) lists AI hint
//! points, #39 (0D80/0027) is the route index (2 to start); it waits 2 s,
//! then 1 s (two StateWaits), sets `#39 = (#39 + 1) % count(#6947)` and walks
//! to `#6947[#39]` (STU_ECAEE401 with move-to 194D744E and face-to AB9B30EF).
//! Graph 0B92 alternates firing and waiting phases of 1.75 to 2.5 s within
//! 0.4 m of spawn; its 0013.025 reset branch delays firing by 2.75 s. A bot's
//! health is the body's override of 0033's #199 (Training Bots 200, Friendly
//! Bots 225). The client's code for the AI's move-to, face-to and fire states
//! is a stub (one shared class, vtable 0x7FF78B6BC800): they ran on the
//! server only.
//!
//! Measured on the retail OW2 Practice Range's live traffic (its four walking
//! bots walk the far routes of these spawners): a bot sets off when its wait
//! ends whichever way it faces, keys held all the way (throttles 127; retail
//! sent 126-127 from the first tick of a walk to its last), and turns toward
//! the way it walks at 60 degrees a second until 0.5 s (28 to 33 ticks) after
//! it lets go. It speeds up at ground acceleration times run speed and lets go
//! where ground friction (base times run speed) stops it on its point (4.5
//! m/s^2, 4.5 m/s and 67.5 m/s^2 for the Training Bot body). The next walk
//! starts 1.2 s (75 ticks) after the last tick with keys. Unverified: retail
//! stopped 0.05-0.45 m short of the 1.74 points. The far runners (hero 016E,
//! body 14ED) use 1.74's values: 6 m/s, up at 66 m/s^2, down at 39 m/s^2;
//! OW2 put ordinary Training Bots on these routes (4.5 m/s) and changed three
//! routes. Bot n's entity is `0xC0001000 + n` (type 2, like the game mode
//! entity); its respawns take the next free ids up from there. The stand-in
//! controller of the bot in slot n (`scene.rs` `BotControllers`) is
//! `0xC0000800 + n`, below them, so the two never meet.

use std::{
    collections::{HashMap, VecDeque},
    f64::consts,
    fs, io,
    path::Path,
    sync::Arc,
};

use serde::Deserialize;

use orpheus_core::guid::parse_guid;
use orpheus_core::tick::TICK_PERIOD;
use orpheus_physics::contact::body::CharacterBody;
use orpheus_physics::contact::math::{add, sub};
use orpheus_physics::contact::world::ContactWorld;
use orpheus_physics::mover::ground::{pogo, standing_body, standing_origin};
use orpheus_physics::mover::settings::MoverSettings;
use orpheus_wire::entity::{Movement, NO_INPUT};

use crate::{
    bot_animation::{AnimRequest, BodyRig, BotAnimation},
    hitboxes::HitShape,
};

pub const BOT_ENTITY: u32 = 0xc000_1000;
pub const BOT_CONTROLLER_ENTITY: u32 = 0xc000_0800;
pub const DEAD_MOVEMENT_FLAG: u32 = 0x2000;
pub const SETTLE_TICKS: u32 = 63;
pub const BOT_RECORD_DELAY: u32 = 8;

const PATROL: u64 = 0x0580_0000_0000_0b8e;
const FIRING: u64 = 0x0580_0000_0000_0b92;
const FIRST_WAIT: f64 = 3.0;
const WALK_GAP: f64 = 1.2;
const ARRIVED: f64 = 0.05;
const MAX_DETOUR_SAMPLES: usize = 64;
const STEP_UP: f64 = 0.6;
const DROP: f64 = 4.0;
const MAX_SLOPE: f64 = 1.0;
const FORWARD: f64 = 127.0;
const YAW_UNITS: f64 = 65536.0;
const TURN_RATE: f64 = YAW_UNITS / 6.0;
const TURN_AFTER: f64 = 0.5;
const CATCH_UP: u32 = 625;
const EPSILON: f64 = 1.0e-9;
/// A training bot's eye over its feet, where its shots start (combat.py
/// `EYE`, which Python used for every bot), and where the head zone of a bot
/// body without hit shapes ends. Kept, not derived: the Training Bot body
/// (1403) authors a first-person eye of 2.0, the top of its capsule, which
/// is not where the reference aimed from (its tests pin 1.55); the other bot
/// bodies author none. Bodies with hit shapes (every bot body in the data)
/// take their critical zone from the shapes instead.
pub const BOT_EYE: f64 = 1.55;

/// A bot body's walk, from its mover (bots.py `mover_data`): run speed,
/// acceleration and braking, and the gravity scale its movement records
/// carry (mover.py `GRAVITY_SCALE`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotWalk {
    pub speed: f64,
    pub acceleration: f64,
    pub brake: f64,
    pub gravity: f64,
}

impl BotWalk {
    pub fn from_mover(mover: &MoverSettings) -> Self {
        Self {
            speed: mover.run_forward,
            acceleration: mover.accel_ground * mover.run_forward,
            brake: mover.decel_ground_base * mover.run_forward,
            gravity: mover.gravity_scale,
        }
    }
}

#[derive(Debug)]
pub struct BotSpawn {
    pub hero: u64,
    pub name: String,
    pub body: u64,
    pub team: u8,
    pub health: f64,
    pub position: [f64; 3],
    pub yaw: i32,
    pub firing: bool,
    pub route: Vec<[f64; 3]>,
    initial_index: i64,
}

#[derive(Debug, Default)]
pub struct BotCatalog {
    maps: HashMap<u64, Vec<Arc<BotSpawn>>>,
}

impl BotCatalog {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        match path.metadata() {
            Ok(metadata) if !metadata.is_file() => Ok(Self::default()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
            Ok(_) => Self::from_json(&fs::read_to_string(path)?),
        }
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let raw: RawTable = serde_json::from_str(text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut maps = HashMap::with_capacity(raw.maps.len());
        for (map, entry) in raw.maps {
            let mut spawns = Vec::new();
            for row in entry.spawners {
                let Some(hero) = raw.heroes.get(&row.hero) else {
                    continue;
                };
                if row.identifier.is_some() {
                    continue;
                }
                let mut patrol = false;
                let mut firing = false;
                for graph in row.graphs {
                    let id = guid(&graph)?;
                    patrol |= id == PATROL;
                    firing |= id == FIRING;
                }
                let route = if patrol {
                    row.vars
                        .route
                        .iter()
                        .filter_map(|point| entry.hints.get(point).copied())
                        .collect()
                } else {
                    Vec::new()
                };
                let team = u8::try_from(row.team)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                spawns.push(Arc::new(BotSpawn {
                    hero: guid(&row.hero)?,
                    name: hero.name.clone(),
                    body: guid(&hero.body)?,
                    team,
                    health: hero.health,
                    position: row.position,
                    yaw: yaw_of(row.rotation),
                    firing,
                    route,
                    initial_index: row.vars.index,
                }));
            }
            maps.insert(guid(&map)?, spawns);
        }
        Ok(Self { maps })
    }

    /// Entity IDs follow the authored order after excluding mode-specific
    /// spawners and unknown heroes. Body physics comes from the shared mover.
    pub fn make(&self, map_guid: u64, mover: impl Fn(u64) -> BotWalk) -> Vec<Bot> {
        self.maps
            .get(&map_guid)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, spawn)| {
                Bot::new(
                    BOT_ENTITY + index as u32,
                    Arc::clone(spawn),
                    mover(spawn.body),
                )
            })
            .collect()
    }
}

#[derive(Deserialize)]
struct RawTable {
    heroes: HashMap<String, RawHero>,
    maps: HashMap<String, RawMap>,
}

#[derive(Deserialize)]
struct RawHero {
    name: String,
    body: String,
    health: f64,
}

#[derive(Deserialize)]
struct RawMap {
    spawners: Vec<RawSpawner>,
    hints: HashMap<String, [f64; 3]>,
}

#[derive(Deserialize)]
struct RawSpawner {
    hero: String,
    team: i32,
    identifier: Option<String>,
    position: [f64; 3],
    rotation: [f64; 4],
    graphs: Vec<String>,
    #[serde(default)]
    vars: RawVars,
}

#[derive(Default, Deserialize)]
struct RawVars {
    #[serde(default, rename = "0x0D80000000001B23")]
    route: Vec<String>,
    #[serde(default, rename = "0x0D80000000000027")]
    index: i64,
}

fn guid(text: &str) -> io::Result<u64> {
    parse_guid(text).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid bot catalog GUID {text:?}"),
        )
    })
}

fn s16(angle: f64) -> f64 {
    (angle + YAW_UNITS / 2.0).rem_euclid(YAW_UNITS) - YAW_UNITS / 2.0
}

fn yaw_of([_, y, _, w]: [f64; 4]) -> i32 {
    s16((y.atan2(w) * YAW_UNITS / consts::PI).round_ties_even()) as i32
}

fn heading([dx, dz]: [f64; 2]) -> f64 {
    s16((dx.atan2(dz) * YAW_UNITS / consts::TAU).round_ties_even())
}

fn throttles(yaw: f64, [dx, dz]: [f64; 2]) -> [i8; 2] {
    let (sin, cos) = (yaw * consts::TAU / YAW_UNITS).sin_cos();
    let right = -dx * cos + dz * sin;
    let forward = dx * sin + dz * cos;
    [
        (right * FORWARD).round_ties_even() as i8,
        (forward * FORWARD).round_ties_even() as i8,
    ]
}

fn planar_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

type Detour = [Option<[f64; 3]>; 2];

#[derive(Debug)]
struct PatrolPlans {
    first_index: usize,
    first: Detour,
    legs: Vec<Detour>,
}

#[derive(Debug)]
struct PatrolCollision {
    body: CharacterBody,
    lift: f32,
}

impl PatrolCollision {
    fn clear(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        self.body
            .sweep(
                standing_origin(from, self.lift),
                standing_origin(to, self.lift),
            )
            .is_ok_and(|hit| hit.is_none())
    }

    fn supported(
        &self,
        from: [f64; 3],
        to: [f64; 3],
        floor_tolerance: f64,
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) -> bool {
        let width = 2.0 * f64::from(self.body.radius());
        let samples = (planar_distance(from, to) / width).ceil() as usize;
        if samples == 0 || samples > MAX_DETOUR_SAMPLES || !self.clear(from, to) {
            return false;
        }
        (0..=samples).all(|sample| {
            let t = sample as f64 / samples as f64;
            let [x, y, z] = std::array::from_fn(|axis| from[axis] + (to[axis] - from[axis]) * t);
            ground(x, z, y + floor_tolerance, y - floor_tolerance).is_some()
        })
    }

    fn advance(&mut self, from: [f64; 3], to: [f64; 3], dt: f64) -> [f64; 3] {
        let start = standing_origin(from, self.lift);
        self.body.teleport(start);
        let delta = sub(standing_origin(to, self.lift), start);
        match self.body.move_capsule(delta, dt as f32) {
            Ok(moved) if moved.position == add(start, delta) => to,
            Ok(moved) => {
                let [x, y, z] = moved.position.map(f64::from);
                [x, y - f64::from(self.lift), z]
            }
            Err(_) => from,
        }
    }
}

#[derive(Debug)]
pub struct Bot {
    pub entity: u32,
    pub spawn: Arc<BotSpawn>,
    pub position: [f64; 3],
    pub yaw: f64,
    pub velocity: [f64; 3],
    pub dead: bool,
    walk: BotWalk,
    pace: f64,
    direction: [f64; 2],
    index: usize,
    wait: f64,
    target: Option<[f64; 3]>,
    waypoints: VecDeque<[f64; 3]>,
    collision: Option<PatrolCollision>,
    plans: Option<Arc<PatrolPlans>>,
    start: [f64; 3],
    keys: bool,
    turning: f64,
    held: [i8; 2],
    last_held: [i8; 2],
    input_tick: Option<u32>,
    moved_tick: Option<u32>,
    last_tick: Option<u32>,
    records_from: Option<u32>,
    /// Its body's animation, when the data has a rig for it.
    animation: Option<BotAnimation>,
}

impl Bot {
    fn new(entity: u32, spawn: Arc<BotSpawn>, walk: BotWalk) -> Self {
        let index = if spawn.route.is_empty() {
            0
        } else {
            spawn.initial_index.rem_euclid(spawn.route.len() as i64) as usize
        };
        Self {
            entity,
            position: spawn.position,
            yaw: f64::from(spawn.yaw),
            velocity: [0.0; 3],
            dead: false,
            walk,
            pace: 0.0,
            direction: [0.0, 1.0],
            index,
            wait: FIRST_WAIT,
            target: None,
            waypoints: VecDeque::new(),
            collision: None,
            plans: None,
            start: spawn.position,
            keys: false,
            turning: 0.0,
            held: [0; 2],
            last_held: [0; 2],
            input_tick: None,
            moved_tick: None,
            last_tick: None,
            records_from: None,
            animation: None,
            spawn,
        }
    }

    /// The facing the clients are sent (its yaw in 1/65536 turns, rounded),
    /// in radians: forward is `[sin, 0, cos]`.
    pub fn facing(&self) -> f64 {
        self.yaw.round_ties_even() * consts::TAU / YAW_UNITS
    }

    /// Animate the body by `body`'s rig ([`crate::bot_animation`]).
    pub(crate) fn attach_animation(&mut self, body: Arc<BodyRig>) {
        self.animation = Some(BotAnimation::new(body));
    }

    /// Pose the body for `tick` (after it walked), with what its script
    /// asks; a body without a rig keeps its static shapes.
    pub(crate) fn animate(&mut self, tick: u32, arms_forward: bool, anim: Option<AnimRequest>) {
        let walking = self.walking();
        if let Some(animation) = &mut self.animation {
            animation.advance(tick, walking, arms_forward, anim);
        }
    }

    /// The body's shapes as posed last; `None` without a rig.
    pub(crate) fn posed_shapes(&self) -> Option<Arc<[HitShape]>> {
        self.animation.as_ref().map(BotAnimation::shapes)
    }

    pub(crate) fn attach_collision(
        &mut self,
        settings: &MoverSettings,
        world: Arc<ContactWorld>,
        category: u16,
        mut ground: impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) {
        self.collision = Some(PatrolCollision {
            body: standing_body(settings, world, self.position, category),
            lift: pogo(settings, false) as f32,
        });
        let count = self.spawn.route.len();
        if count != 0 {
            let first_index = (self.index + 1) % count;
            self.plans = Some(Arc::new(PatrolPlans {
                first_index,
                first: self.detour(self.position, self.spawn.route[first_index], &mut ground),
                legs: (0..count)
                    .map(|index| {
                        let from = self.spawn.route[(index + count - 1) % count];
                        self.detour(from, self.spawn.route[index], &mut ground)
                    })
                    .collect(),
            }));
        }
    }

    fn detour(
        &self,
        from: [f64; 3],
        target: [f64; 3],
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) -> Detour {
        let Some(collision) = &self.collision else {
            return [None; 2];
        };
        let same_floor = |point: [f64; 3]| (point[1] - from[1]).abs() <= ARRIVED;
        let distance = planar_distance(from, target);
        if distance <= ARRIVED || !same_floor(target) || collision.clear(from, target) {
            return [None; 2];
        }
        let hint = self
            .spawn
            .route
            .iter()
            .copied()
            .filter(|&point| {
                same_floor(point)
                    && planar_distance(from, point) > ARRIVED
                    && planar_distance(point, target) > ARRIVED
                    && collision.supported(from, point, STEP_UP, ground)
                    && collision.supported(point, target, STEP_UP, ground)
            })
            .min_by(|&a, &b| {
                let length = |point| planar_distance(from, point) + planar_distance(point, target);
                length(a).total_cmp(&length(b))
            });
        if hint.is_some() {
            return [hint, None];
        }

        // Try a body-width lane to either side. Each leg must fit the same
        // capsule and stay supported by the same floor as ordinary movement.
        let [x, _, z] = from;
        let [tx, _, tz] = target;
        let scale = 2.0 * f64::from(collision.body.radius()) / distance;
        for sign in [1.0, -1.0] {
            let offset = [(tz - z) * scale * sign, 0.0, (x - tx) * scale * sign];
            let side = |point: [f64; 3]| std::array::from_fn(|axis| point[axis] + offset[axis]);
            let [a, b] = [side(from), side(target)];
            if collision.supported(from, a, ARRIVED, ground)
                && collision.supported(a, b, ARRIVED, ground)
                && collision.supported(b, target, ARRIVED, ground)
            {
                return [Some(a), Some(b)];
            }
        }
        [None; 2]
    }

    fn planned_detour(&self, previous: usize) -> Detour {
        let Some(plans) = &self.plans else {
            return [None; 2];
        };
        if self.index == plans.first_index && self.position == self.spawn.position {
            return plans.first;
        }
        let from = self.spawn.route[previous];
        if planar_distance(self.position, from) <= ARRIVED
            && (self.position[1] - from[1]).abs() <= STEP_UP
        {
            plans.legs[self.index]
        } else {
            [None; 2]
        }
    }

    /// The floor provider can give a surface Y within the top and bottom
    /// probe bounds. A missing surface uses the authored hint heights.
    pub fn step(&mut self, tick: u32, mut ground: impl FnMut(f64, f64, f64, f64) -> Option<f64>) {
        if self.spawn.route.is_empty() || self.dead {
            return;
        }
        let ticks = self
            .last_tick
            .map_or(1, |last| tick.saturating_sub(last))
            .min(CATCH_UP);
        if ticks == 0 {
            return;
        }
        self.last_tick = Some(tick);
        let seconds = TICK_PERIOD.as_secs_f64();
        for at in tick - (ticks - 1)..=tick {
            self.tick(at, seconds, &mut ground);
        }
    }

    fn tick(
        &mut self,
        tick: u32,
        dt: f64,
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) {
        let before = (self.position, self.yaw.round_ties_even());
        self.held = [0; 2];
        if !self.keys {
            self.wait -= dt;
            if self.target.is_none() && self.wait <= EPSILON {
                let previous = self.index;
                self.index = (self.index + 1) % self.spawn.route.len();
                let goal = self.spawn.route[self.index];
                let detour = self.planned_detour(previous);
                self.waypoints.extend(detour.into_iter().flatten());
                self.target = Some(self.waypoints.pop_front().unwrap_or(goal));
                if self.target != Some(goal) {
                    self.waypoints.push_back(goal);
                }
                self.start = self.position;
                self.keys = true;
                self.turning = 0.0;
            }
        }
        if self.keys {
            self.walk(tick, dt, ground);
        } else {
            if self.target.is_some() {
                self.slide(dt, ground);
            }
            if self.turning > EPSILON {
                self.turning -= dt;
                self.turn(dt);
            }
        }
        if (self.position, self.yaw.round_ties_even()) != before {
            self.moved_tick = Some(tick);
        }
    }

    fn walk(
        &mut self,
        tick: u32,
        dt: f64,
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) {
        let mut offset = self.to_target();
        let mut distance = offset[0].hypot(offset[1]);
        if distance <= ARRIVED {
            if let Some(goal) = self.waypoints.pop_front() {
                self.target = Some(goal);
                self.start = self.position;
                offset = self.to_target();
                distance = offset[0].hypot(offset[1]);
            }
        }
        if distance <= ARRIVED {
            self.keys = false;
            self.wait = WALK_GAP;
            self.stand();
            return;
        }
        self.direction = offset.map(|axis| axis / distance);
        self.turn(dt);
        self.pace = self.walk.speed.min(self.pace + self.walk.acceleration * dt);
        self.held = throttles(self.yaw, self.direction);
        self.last_held = self.held;
        self.input_tick = Some(tick);
        let mut stride = self.pace * dt;
        if self.waypoints.is_empty() {
            let slide = self.slide_length(dt);
            if distance - stride <= slide {
                stride = (distance - slide).max(0.0);
                self.keys = false;
                self.wait = WALK_GAP;
                self.turning = TURN_AFTER;
            }
        }
        self.advance(stride, distance, dt, ground);
    }

    fn slide(&mut self, dt: f64, ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>) {
        self.pace = (self.pace - self.walk.brake * dt).max(0.0);
        let [dx, dz] = self.to_target();
        let distance = dx.hypot(dz);
        if self.pace > 0.0
            && distance > EPSILON
            && self.advance(self.pace * dt, distance, dt, ground) < distance - EPSILON
        {
            return;
        }
        self.stand();
    }

    fn stand(&mut self) {
        self.pace = 0.0;
        self.velocity = [0.0; 3];
        self.target = None;
        self.waypoints.clear();
    }

    fn to_target(&self) -> [f64; 2] {
        let [tx, _, tz] = self.target.expect("a walking bot has a target");
        [tx - self.position[0], tz - self.position[2]]
    }

    fn slide_length(&self, dt: f64) -> f64 {
        let slow = self.walk.brake * dt;
        let mut pace = self.pace - slow;
        let mut length = 0.0;
        if slow <= 0.0 && pace > 0.0 {
            return f64::INFINITY;
        }
        for _ in 0..CATCH_UP {
            if pace <= 0.0 {
                return length;
            }
            length += pace * dt;
            pace -= slow;
        }
        // Invalid or extremely weak braking must not stall the tick.
        f64::INFINITY
    }

    fn turn(&mut self, dt: f64) {
        let left = s16(heading(self.direction) - self.yaw);
        let units = TURN_RATE * dt;
        self.yaw = s16(self.yaw + left.clamp(-units, units));
    }

    fn advance(
        &mut self,
        mut stride: f64,
        distance: f64,
        dt: f64,
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) -> f64 {
        let [x, y, z] = self.position;
        let [dx, dz] = self.direction;
        let probe = stride.min(distance);
        let probe_y = self.floor(x + dx * probe, y, z + dz * probe, ground);
        let rise = (probe_y - y).abs();
        if 0.0 < rise && rise <= MAX_SLOPE * probe {
            stride *= probe / probe.hypot(rise);
        }
        let reuse_probe = stride == probe && stride < distance;
        let [nx, nz] = if stride >= distance {
            stride = distance;
            let [tx, _, tz] = self.target.expect("a walking bot has a target");
            [tx, tz]
        } else {
            [x + dx * stride, z + dz * stride]
        };
        let ny = if reuse_probe {
            probe_y
        } else {
            self.floor(nx, y, nz, ground)
        };
        let intended = [nx, ny, nz];
        let [nx, ny, nz] = self.collision.as_mut().map_or(intended, |collision| {
            collision.advance(self.position, intended, dt)
        });
        self.position = [nx, ny, nz];
        self.velocity = [(nx - x) / dt, (ny - y) / dt, (nz - z) / dt];
        if self.position == intended {
            stride
        } else {
            ((nx - x) * dx + (nz - z) * dz).max(0.0)
        }
    }

    fn floor(
        &self,
        x: f64,
        y: f64,
        z: f64,
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) -> f64 {
        if let Some(height) = ground(x, z, y + STEP_UP, y - DROP) {
            return height;
        }
        let [sx, sy, sz] = self.start;
        let [tx, ty, tz] = self.target.expect("a walking bot has a target");
        let whole = (tx - sx).hypot(tz - sz);
        let gone = if whole <= 0.0 {
            1.0
        } else {
            ((x - sx).hypot(z - sz) / whole).min(1.0)
        };
        sy + (ty - sy) * gone
    }

    pub fn walking(&self) -> bool {
        self.target.is_some()
    }

    pub fn died(&mut self, tick: u32) {
        self.dead = true;
        self.keys = false;
        self.held = [0; 2];
        self.stand();
        self.moved_tick = Some(tick);
    }

    pub fn respawned(&self, entity: u32, records_from: u32) -> Self {
        let mut fresh = Self::new(entity, Arc::clone(&self.spawn), self.walk);
        fresh.collision = self.collision.as_ref().map(|collision| PatrolCollision {
            body: collision
                .body
                .fresh_at(standing_origin(fresh.position, collision.lift)),
            lift: collision.lift,
        });
        fresh.records_from = Some(records_from);
        fresh.animation = self.animation.as_ref().map(BotAnimation::fresh);
        fresh
    }

    pub fn record_due(&self, tick: u32, every: u32) -> bool {
        if self.records_from.is_some_and(|from| tick < from) {
            return false;
        }
        if self.walking()
            || self
                .moved_tick
                .is_some_and(|moved| tick.saturating_sub(moved) < SETTLE_TICKS)
        {
            return true;
        }
        let every = every.max(1);
        let period = (SETTLE_TICKS / every).max(1);
        (tick / every) % period == self.entity % period
    }

    pub fn movement(&self, tick: u32) -> Movement {
        let input_frame = if self.held != [0; 2] {
            None
        } else {
            match self.input_tick {
                None => Some(NO_INPUT),
                Some(frame) if frame < tick => Some(frame),
                Some(_) => None,
            }
        };
        Movement {
            position: self.position,
            yaw: s16(self.yaw.round_ties_even()) as i32,
            velocity: self.velocity,
            flags: if self.dead { DEAD_MOVEMENT_FLAG } else { 0 },
            throttles: self.held,
            input_frame,
            input_throttles: self.last_held,
            gravity: self.walk.gravity,
            ..Movement::default()
        }
    }
}

#[cfg(test)]
mod tests;
