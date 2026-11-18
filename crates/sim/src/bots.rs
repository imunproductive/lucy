//! Map AI spawners and the training bots' walker: the server's native
//! locomotion, which walks the bot to the point its patrol graph asks for.
//!
//! The catalog follows the Python reference.
//!
//! Research: the Practice Range has 15 spawners, 11 Training Bots on the red
//! team and 4 Friendly Bots on the blue one (retail's OW2 Practice Range
//! showed 15 bots too, each with a movement record in every frame, idle or
//! not). Graph 0B8E (`orpheus_script::bot::PATROL_GRAPH`) runs in the bot's
//! script and decides when and where it walks: variable #6947 (0D80/1B23)
//! lists AI hint points, #39 (0D80/0027) is the route index (2 to start); it
//! waits 2 s, then 1 s (two StateWaits), sets `#39 = (#39 + 1) %
//! count(#6947)` and asks its AI movement state (STU_ECAEE401, move-to
//! 194D744E and face-to AB9B30EF on `#6947[#39]`) for the walk; the walk's
//! end waits 1 s and asks for the next point. The walker takes that point
//! ([`Bot::step`]) and reports the walk's end, which the match hands back to
//! the script. Graph 0B92 alternates firing and waiting phases of 1.75 to
//! 2.5 s within 0.4 m of spawn; its 0013.025 reset branch delays firing by
//! 2.75 s. A bot's health is the body's override of 0033's #199 (Training
//! Bots 200, Friendly Bots 225). The client's code for the AI's move-to,
//! face-to and fire states is a stub (one shared class, vtable
//! 0x7FF78B6BC800): they ran on the server only.
//!
//! Measured on the retail OW2 Practice Range's live traffic (its four walking
//! bots walk the far routes of these spawners): a bot sets off when its wait
//! ends whichever way it faces, keys held all the way (throttles 127; retail
//! sent 126-127 from the first tick of a walk to its last), and turns toward
//! the way it walks at 60 degrees a second until 0.5 s (28 to 33 ticks) after
//! it lets go. It speeds up at ground acceleration times run speed and lets go
//! where ground friction (base times run speed) stops it on its point (4.5
//! m/s^2, 4.5 m/s and 67.5 m/s^2 for the Training Bot body). The turn rate is
//! the walker's own: 0B8E's movement state names no rate (its numbers
//! `m_BDDD203A`, `m_6F4283C8`, `m_888D9442` are not decoded). The next walk
//! started 1.2 s (75 ticks) after the last tick with keys; 0B8E's 1 s wait
//! after the walk's end gives about 1.07 s (the slide to the point, then 63
//! ticks): the 0.13 s left is OW2's or the native walk's (its end may come
//! later than our stop), not known. Unverified: retail
//! stopped 0.05-0.45 m short of the 1.74 points. The far runners (hero 016E,
//! body 14ED) use 1.74's values: 6 m/s, up at 66 m/s^2, down at 39 m/s^2;
//! OW2 put ordinary Training Bots on these routes (4.5 m/s) and changed three
//! routes. Bot n's entity is `0x80008800 + n`; its respawns take the next
//! free ids up from there. The stand-in controller of the bot in slot n
//! (`scene.rs` `bot_controller_create`) is `0x80008000 + n`, below them, so
//! the two never meet. Both are in id class 0 (`0x8…`), which the client
//! uses for entities the server creates (the map's placeables are
//! `0x80000000 | index`, at most `0x1250` in 1.74's maps).
//! ow1-preservation's Training Bots (`0x80000306`) took the client's own
//! bullets on their body (its run 343); ours, in class 2 (`0xC…`, the map's
//! own entities, such as the bots' spawn markers `0xC0000004…`), took the
//! client's Helix Rockets but not its hitscan shots (checked in the game,
//! 2026-11-12). The guess that the client's hitscan ray leaves out class-2
//! entities did not hold: in class 0 they were still shot through. The
//! cause was the body filter word's missing bit 0x10000 (`scene.rs`
//! module notes).

use std::{
    collections::{HashMap, VecDeque},
    f64::consts,
    fmt, fs, io,
    path::Path,
    sync::Arc,
};

use serde::Deserialize;

use orpheus_core::guid::parse_guid;
use orpheus_core::tick::TICK_PERIOD;
use orpheus_physics::contact::body::CharacterBody;
use orpheus_physics::contact::cache::CapsulePose;
use orpheus_physics::contact::math::{add, sub};
use orpheus_physics::contact::world::ContactWorld;
use orpheus_physics::mover::ground::{pogo, standing_body, standing_capsule, standing_origin};
use orpheus_physics::mover::settings::MoverSettings;
use orpheus_physics::{
    collision::index::TriangleIndex,
    mover::{Mover, resting_spring, sample_movement_flags},
};
use orpheus_script::bot::{BOT_MOVEMENT_SAMPLE, PATROL_GRAPH, Patrol};
use orpheus_script::vm::Motion;
use orpheus_wire::commands::Command;
use orpheus_wire::entity::{Movement, NO_INPUT};

use crate::{
    bot_animation::{AnimRequest, BodyRig, BotAnimation},
    hitboxes::HitShape,
    replication::start_motions,
};

pub const BOT_ENTITY: u32 = 0x8000_8800;
pub const BOT_CONTROLLER_ENTITY: u32 = 0x8000_8000;
pub const DEAD_MOVEMENT_FLAG: u32 = 0x2000;
pub const BOT_RECORD_DELAY: u32 = 8;

const PATROL: u64 = 0x0580_0000_0000_0000 | PATROL_GRAPH as u64;
const FIRING: u64 = 0x0580_0000_0000_0b92;
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
/// carry (mover.py `GRAVITY_SCALE`); and the spring steps of its records,
/// the body's at rest (`resting_spring`; see `Bot::movement`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BotWalk {
    pub speed: f64,
    pub acceleration: f64,
    pub brake: f64,
    pub gravity: f64,
    pub spring: [i32; 2],
}

impl BotWalk {
    pub fn from_mover(mover: &MoverSettings) -> Self {
        Self {
            speed: mover.run_forward,
            acceleration: mover.accel_ground * mover.run_forward,
            brake: mover.decel_ground_base * mover.run_forward,
            gravity: mover.gravity_scale,
            spring: resting_spring(mover),
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
    /// The patrol's hint points ([`PATROL_GRAPH`]'s #6947); empty for a
    /// spawner without the patrol.
    pub route: Vec<[f64; 3]>,
    /// The patrol's first route index (#39).
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
    /// The point its script asked for when it last set off ([`Self::step`]).
    goal: Option<[f64; 3]>,
    /// The point it walks to now: the goal or a detour's corner on the way.
    target: Option<[f64; 3]>,
    waypoints: VecDeque<[f64; 3]>,
    collision: Option<PatrolCollision>,
    start: [f64; 3],
    keys: bool,
    turning: f64,
    held: [i8; 2],
    last_held: [i8; 2],
    input_tick: Option<u32>,
    last_tick: Option<u32>,
    records_from: Option<u32>,
    /// Its body's animation, when the data has a rig for it.
    animation: Option<BotAnimation>,
    /// Its flight after a knockback ([`Bot::knock`]).
    flight: Option<BotFlight>,
}

/// A knocked bot's body on a mover of its own (the body's
/// `STUCharacterMoverComponent`, as a player's), until it stands still on
/// the ground again.
struct BotFlight {
    mover: Mover,
    /// The last tick it flew.
    last_tick: u32,
}

impl fmt::Debug for BotFlight {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BotFlight")
            .field("pose", &self.mover.pose())
            .field("last_tick", &self.last_tick)
            .finish()
    }
}

impl Bot {
    fn new(entity: u32, spawn: Arc<BotSpawn>, walk: BotWalk) -> Self {
        Self {
            entity,
            position: spawn.position,
            yaw: f64::from(spawn.yaw),
            velocity: [0.0; 3],
            dead: false,
            walk,
            pace: 0.0,
            direction: [0.0, 1.0],
            goal: None,
            target: None,
            waypoints: VecDeque::new(),
            collision: None,
            start: spawn.position,
            keys: false,
            turning: 0.0,
            held: [0; 2],
            last_held: [0; 2],
            input_tick: None,
            last_tick: None,
            records_from: None,
            animation: None,
            flight: None,
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
    ) {
        self.collision = Some(PatrolCollision {
            body: standing_body(settings, world, self.position, category),
            lift: pogo(settings, false) as f32,
        });
    }

    /// The overrides its patrol graph starts with, for a spawner that lists
    /// it ([`BotSpawn::route`]).
    pub fn patrol(&self) -> Option<Patrol<'_>> {
        (!self.spawn.route.is_empty()).then(|| Patrol {
            route: &self.spawn.route,
            index: self.spawn.initial_index,
        })
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

    /// A knockback (`push`, metres a second) throws the body: it leaves its
    /// patrol and flies on a mover of its own (`mover`, made at its feet
    /// when it isn't flying yet) until it stands still on the ground
    /// ([`Self::fly`]). Pushes before its next frame add up
    /// ([`Mover::push`]).
    pub(crate) fn knock(
        &mut self,
        push: [f64; 3],
        tick: u32,
        mover: impl FnOnce() -> Option<Mover>,
    ) {
        if let Some(flying) = self.take_off(tick, mover) {
            flying.push(push);
        }
    }

    /// The dashes, rewinds and velocity actions its script began by the end
    /// of tick `began` (Pharah's Jump Jet `STU_FF3471E9` on a hero body):
    /// they act on its body's mover from its next frame on, as on a
    /// player's ([`start_motions`]), so it flies ([`Self::knock`]).
    pub(crate) fn start_motions(
        &mut self,
        motions: Vec<Motion>,
        began: u32,
        mover: impl FnOnce() -> Option<Mover>,
    ) {
        if motions.is_empty() {
            return;
        }
        if let Some(flying) = self.take_off(began, mover) {
            start_motions(flying, motions, TICK_PERIOD.as_secs_f64());
        }
    }

    /// The mover it flies on, made at its feet (`mover`) if it walked until
    /// now, as of `tick`; none once dead.
    fn take_off(&mut self, tick: u32, mover: impl FnOnce() -> Option<Mover>) -> Option<&mut Mover> {
        if self.dead {
            return None;
        }
        if self.flight.is_none() {
            self.flight = Some(BotFlight {
                mover: mover()?,
                last_tick: tick,
            });
            // The point it was walking to, still asked for, sets it off
            // again once it lands (assumption: the native walk resumes; the
            // script's movement state stays on).
            self.goal = None;
            self.keys = false;
            self.held = [0; 2];
            self.turning = 0.0;
            self.stand();
        }
        self.flight.as_mut().map(|flight| &mut flight.mover)
    }

    /// A knocked body's frames up to `tick` on its mover, with no input; it
    /// lands once its mover has it on the ground and still. Then its walk
    /// goes on from there ([`Self::step`]).
    pub(crate) fn fly(&mut self, tick: u32, rays: &TriangleIndex) {
        let Some(flight) = &mut self.flight else {
            return;
        };
        let seconds = TICK_PERIOD.as_secs_f64();
        let frames = tick.saturating_sub(flight.last_tick).min(CATCH_UP);
        for frame in tick - frames + 1..=tick {
            let command = Command {
                frame,
                ..Command::default()
            };
            if flight
                .mover
                .step_pose_to(rays, command, seconds, None)
                .is_err()
            {
                break;
            }
        }
        flight.last_tick = tick;
        let pose = flight.mover.pose();
        self.position = pose.position;
        self.velocity = pose.velocity;
        let [vx, _, vz] = pose.velocity;
        if self.movement_sample() & 1 == 0 && vx == 0.0 && vz == 0.0 {
            self.flight = None;
            self.velocity = [0.0; 3];
            if let Some(collision) = &mut self.collision {
                collision.body = collision
                    .body
                    .fresh_at(standing_origin(self.position, collision.lift));
            }
        }
    }

    /// Its 13 movement bits for its script's movement sampler
    /// (`STU_96F1DBCF`): its mover's while it flies, else on the ground
    /// ([`BOT_MOVEMENT_SAMPLE`]).
    pub fn movement_sample(&self) -> u16 {
        self.flight
            .as_ref()
            .and_then(|flight| sample_movement_flags(&flight.mover.pose()))
            .unwrap_or(BOT_MOVEMENT_SAMPLE)
    }

    /// Walk to `goal`, the point its script asks for now
    /// ([`crate::bot_script::BotScripts::move_goal`]), through the ticks up
    /// to `tick`: a new point sets it off from where it stands. The floor
    /// provider can give a surface Y within the top and bottom probe bounds;
    /// a missing surface uses the authored hint heights. A knocked body
    /// flies instead ([`Self::fly`]); its walk goes on once it lands, with
    /// no catch-up for the flight. The tick its walk ended (it stands still
    /// on the point), for the script.
    pub fn step(
        &mut self,
        tick: u32,
        goal: Option<[f64; 3]>,
        mut ground: impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) -> Option<u32> {
        if self.dead {
            return None;
        }
        if self.flight.is_some() {
            self.last_tick = Some(tick);
            return None;
        }
        let ticks = self
            .last_tick
            .map_or(1, |last| tick.saturating_sub(last))
            .min(CATCH_UP);
        if ticks == 0 {
            return None;
        }
        self.last_tick = Some(tick);
        let seconds = TICK_PERIOD.as_secs_f64();
        let mut ended = None;
        for at in tick - (ticks - 1)..=tick {
            if self.tick(at, seconds, goal, &mut ground) {
                ended = Some(at);
            }
        }
        ended
    }

    /// One tick of the walk; true when the walk ended in it.
    fn tick(
        &mut self,
        tick: u32,
        dt: f64,
        goal: Option<[f64; 3]>,
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) -> bool {
        self.held = [0; 2];
        if goal != self.goal {
            self.goal = goal;
            if let Some(goal) = goal {
                self.set_off(goal, ground);
            }
        }
        let walking = self.target.is_some();
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
        walking && self.target.is_none()
    }

    /// Set off for `goal` from where it stands, around what blocks the
    /// straight way ([`Self::detour`], from the route point it stands on).
    fn set_off(
        &mut self,
        goal: [f64; 3],
        ground: &mut impl FnMut(f64, f64, f64, f64) -> Option<f64>,
    ) {
        let from = self
            .spawn
            .route
            .iter()
            .copied()
            .find(|&point| {
                planar_distance(self.position, point) <= ARRIVED
                    && (self.position[1] - point[1]).abs() <= STEP_UP
            })
            .unwrap_or(self.position);
        let detour = self.detour(from, goal, ground);
        self.waypoints.clear();
        self.waypoints.extend(detour.into_iter().flatten());
        self.waypoints.push_back(goal);
        self.target = self.waypoints.pop_front();
        self.start = self.position;
        self.keys = true;
        self.turning = 0.0;
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

    /// Its capsule where it is now, as other characters meet it, on its
    /// body's mover `settings`: standing on its patrol (its spring at rest,
    /// as its records say: `Bot::movement`), its mover's while it flies.
    /// None once it is dead: a dead body blocks nobody.
    pub fn capsule(&self, settings: &MoverSettings) -> Option<CapsulePose> {
        if self.dead {
            return None;
        }
        Some(self.flight.as_ref().map_or_else(
            || standing_capsule(settings, self.position),
            |flight| flight.mover.capsule(),
        ))
    }

    pub fn died(&mut self) {
        self.dead = true;
        self.flight = None;
        self.keys = false;
        self.held = [0; 2];
        self.stand();
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

    /// Whether the datagram of `tick` carries the bot's movement record:
    /// every one, once a respawned bot's create had time to arrive
    /// (`records_from`). Retail's Practice Range sent every bot's record in
    /// every frame, idle or not (module notes). Python (bots.py
    /// `record_due`) sent a resting bot's about once a second, so a bot
    /// that respawned stayed unseen until its turn came (the client places
    /// a mover only from its movement records), while the server already
    /// let it be hit; and the client may leave a body whose newest record
    /// is that old out of its own hit tests (assumption, see `scene.rs`).
    pub fn record_due(&self, tick: u32) -> bool {
        self.records_from.is_none_or(|from| tick >= from)
    }

    /// Its movement record for `tick`; while it flies, its mover's pose
    /// (flags, air time, fall, spring) with its own facing and no input.
    ///
    /// On its patrol its spring is at rest. Python (bots.py `Bot.movement`)
    /// sent the spring steps as 0, the bottom of their range, -(stance 2.0 +
    /// pogo 0.5): the client put the bot's collision capsule 2.5 m too low,
    /// its top 0.5 m under the bot's feet (`Posture::spring_steps` in
    /// `orpheus-physics`), so players walked through the bots. (The client's
    /// own shots passing through them had another cause: `scene.rs` module
    /// notes.)
    /// Upstream measured that capsule on its NPCs and sent them their rest
    /// steps (leafall903/ORPHEUS (MIT) `bots.py` `Bot.movement`, "Wire step
    /// zero decodes to the bottom of the spring range").
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
        if let Some(flight) = &self.flight {
            return Movement {
                yaw: s16(self.yaw.round_ties_even()) as i32,
                throttles: self.held,
                input_frame,
                input_throttles: self.last_held,
                frame: None,
                crouch_frame: None,
                ..flight.mover.pose()
            };
        }
        Movement {
            position: self.position,
            yaw: s16(self.yaw.round_ties_even()) as i32,
            velocity: self.velocity,
            flags: if self.dead { DEAD_MOVEMENT_FLAG } else { 0 },
            throttles: self.held,
            input_frame,
            input_throttles: self.last_held,
            gravity: self.walk.gravity,
            spring: self.walk.spring,
            ..Movement::default()
        }
    }
}

#[cfg(test)]
mod tests;
