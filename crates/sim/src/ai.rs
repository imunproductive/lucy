//! Practice vs AI: AI heroes played by the server.
//!
//! The Python reference server had no AI players; this is the Rust server's
//! own, after what public sources say of Overwatch 1's bots
//! (`docs/notes/game-rules.md` section 13) and what the ow1-preservation
//! project found running AI heroes against a 1.74 client
//! (`docs/notes/related-projects.md`, "AI heroes as server-driven players";
//! `docs/notes/ow1-preservation.md` sections 7 and 8). An AI is a player
//! without a client: the room gives it a seat and a real hero body (the
//! player body factory, its statescript, mover, combat, death and respawn),
//! and [`AiController`] makes the commands a client would send for it each
//! tick: walk the map's navigation toward the objective or an enemy, aim at
//! an enemy it can see, fire, now and then use an ability, and fall back to
//! its spawn when hurt.
//!
//! What the sources give:
//! - rule 123: the leader picks Easy, Medium or Hard, which deal 25%, 50%
//!   and 100% damage; bots never headshot; harder bots aim better. The
//!   damage is the data's: the hero's weapon graphs read it from the
//!   difficulty profile of the body's AI entity
//!   (`orpheus_script::ai_definitions`; Soldier: 76's rifle 4, 10 and 20
//!   against the player's 20);
//! - rule 124: the AI heroes are Roadhog, Zarya, Bastion, McCree (Cassidy),
//!   Mei, Reaper, Soldier: 76, Sombra, Torbjörn, Ana, Lúcio and Zenyatta
//!   (ow1-preservation's 2018 list has nine of them; the twelve are used);
//! - rule 127: bots jump only to get around and use no movement abilities
//!   outside spawn;
//! - ow1-preservation: six AIs walking the navigation from the attackers'
//!   spawn bunched within 0.4 m; spreading their goals over walkable cells
//!   6 m around the objective and leaving the spawn 1.5 s apart fixed it.
//!
//! The rest is an assumption, named where it is used: the aim error and the
//! reaction time per difficulty, the burst rhythm, the ranges, the health
//! at which an AI falls back and the ability it uses then.

use std::{cell::Cell, f64::consts::TAU};

use orpheus_core::tick::TICK_PERIOD;
use orpheus_physics::{mover::kinematics::poses, nav::Navigation};
use orpheus_script::ai_definitions::{EASY, HARD, MEDIUM};
use orpheus_wire::commands::{Command, FIRE, JUMP};

use crate::navigate;

/// Ability 2 (E) on the command (logical button 36).
pub const ABILITY_2: u32 = 0x10;
/// The angle units of a command: a turn is 65536.
const ANGLE_UNITS: f64 = 65536.0 / TAU;

/// Rule 124: the heroes the AI plays, by their 1.74 names.
pub const ROSTER: [&str; 12] = [
    "Roadhog",
    "Zarya",
    "Bastion",
    "McCree",
    "Mei",
    "Reaper",
    "Soldier: 76",
    "Sombra",
    "Torbjörn",
    "Ana",
    "Lúcio",
    "Zenyatta",
];

/// ow1-preservation: each AI of a team leaves its spawn 1.5 s after the one
/// before.
const STAGGER: f64 = 1.5;
/// ow1-preservation: the AIs of a team stand on cells spread 6 m around the
/// objective.
const SPREAD: f32 = 6.0;
/// A waypoint counts as reached this close, horizontally (assumption).
const ARRIVE: f64 = 0.8;
/// A standing cell counts as reached this close (assumption).
const HOLD: f64 = 1.2;
/// How far an AI sees and shoots an enemy (assumption: Soldier: 76's
/// damage falls off from 30 m).
pub const ENGAGE_RANGE: f64 = 35.0;
/// A walk is kept until it ends, the AI is stuck or its goal moves this
/// far; a goal that moves (a hunted enemy) gets a new walk at most this
/// often (assumption).
const REPATH: f64 = 3.0;
const GOAL_MOVED: f64 = 2.0;
/// At most this many new walks a tick for a room's AIs: a path over the
/// navigation is an A* over the cells of its route (a few milliseconds in
/// a release build, a hundred times that in a debug one), so AIs that set
/// off together take turns (assumption).
pub const PATHS_PER_TICK: usize = 1;
/// No progress over this much of its body's walking, by this much, means
/// stuck: jump and step aside for this much of its walking, to one side
/// and to the other the next time, then find a new path (assumption; rule
/// 127, bots jump to get around).
const STUCK: f64 = 1.0;
const STUCK_DISTANCE: f64 = 0.4;
const SIDESTEP: f64 = 0.5;
/// Falls back to its spawn below this share of its health, until it has
/// this much again or the time is up (assumption; rule 128, bots retreat).
pub const RETREAT_BELOW: f64 = 0.3;
const RECOVERED: f64 = 0.7;
const RETREAT: f64 = 6.0;
/// At most one ability this often: rule 127 keeps bots off their movement
/// abilities, so an AI uses its ability 2 when it falls back (Soldier: 76's
/// Biotic Field, Roadhog's Take a Breather, Mei's Cryo-Freeze...)
/// (assumption).
const ABILITY_COOLDOWN: f64 = 12.0;
/// Fire in bursts: this long on, then this long off (assumption).
const BURST: f64 = 0.6;
const BURST_PAUSE: f64 = 0.3;
/// Sidestep while holding a spot in a fight, changing side this often
/// (assumption).
const STRAFE: f64 = 0.8;
/// Where on a body an AI aims: rule 123, bots never headshot, so the
/// centre of the body, not the head.
const CHEST: f64 = 0.55;

/// Game ticks in `seconds`.
pub fn ticks(seconds: f64) -> u32 {
    (seconds / TICK_PERIOD.as_secs_f64()).round() as u32
}

/// The AI level of a Practice vs AI match (rule 123).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    /// The card's AI level, 1 to 3 (`game_queues_174.json` `ai_teams`).
    pub fn from_level(level: u8) -> Option<Self> {
        match level {
            1 => Some(Self::Easy),
            2 => Some(Self::Medium),
            3 => Some(Self::Hard),
            _ => None,
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "easy" | "1" => Some(Self::Easy),
            "medium" | "2" => Some(Self::Medium),
            "hard" | "3" => Some(Self::Hard),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }

    /// The difficulty identifier of its AI profiles
    /// (`orpheus_script::ai_definitions`).
    pub fn identifier(self) -> u64 {
        match self {
            Self::Easy => EASY,
            Self::Medium => MEDIUM,
            Self::Hard => HARD,
        }
    }

    /// The largest error of an aim, in radians each way (rule 123: harder
    /// bots aim better; the values are an assumption).
    pub fn aim_error(self) -> f64 {
        match self {
            Self::Easy => 0.09,
            Self::Medium => 0.055,
            Self::Hard => 0.03,
        }
    }

    /// How long an AI takes from seeing an enemy to its first shot
    /// (assumption).
    pub fn reaction(self) -> f64 {
        match self {
            Self::Easy => 0.7,
            Self::Medium => 0.45,
            Self::Hard => 0.25,
        }
    }
}

/// A small deterministic generator (SplitMix64): the room seeds each AI.
#[derive(Clone, Debug)]
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// One of `count` (0 for none).
    pub fn below(&mut self, count: usize) -> usize {
        if count == 0 {
            0
        } else {
            (self.next_u64() % count as u64) as usize
        }
    }

    /// In [-1, 1).
    pub fn signed(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// The heroes of `count` AIs of a team: roster heroes in a random order,
/// each once (Quick Play's limit of one per hero and team) and none a
/// player of the team already plays (`taken`), again from the start of the
/// roster when the team needs more.
pub fn pick_heroes(count: usize, taken: &[&str], random: &mut Random) -> Vec<&'static str> {
    let mut free: Vec<&'static str> = ROSTER
        .iter()
        .copied()
        .filter(|hero| !taken.contains(hero))
        .collect();
    let mut picks = Vec::with_capacity(count);
    while picks.len() < count {
        if free.is_empty() {
            free = ROSTER.to_vec();
        }
        picks.push(free.remove(random.below(free.len())));
    }
    picks
}

/// Another player as an AI sees him this tick: his living body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub slot: usize,
    pub team: u8,
    pub feet: [f64; 3],
    /// The body's height, standing or crouched.
    pub height: f64,
}

impl Seen {
    /// Where an AI aims at him.
    pub fn chest(&self) -> [f64; 3] {
        let [x, y, z] = self.feet;
        [x, y + self.height * CHEST, z]
    }
}

/// Where the match sends an AI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Goal {
    /// Stay where it is: hero select, the attackers' setup, a round's end.
    Hold,
    /// The objective the round is played for.
    Objective([f64; 3]),
    /// No objective (Deathmatch, Elimination, Skirmish): the nearest enemy.
    Hunt,
}

/// What an AI knows this tick.
pub struct Senses<'a> {
    pub tick: u32,
    pub team: u8,
    /// Its living body's feet; `None` while it is dead or has no body.
    pub feet: Option<[f64; 3]>,
    /// Its body: a new one (a respawn, a hero switch) starts afresh.
    pub body: Option<u32>,
    /// Its eyes above its feet.
    pub eye: f64,
    /// Its health as a share of its most (1 when unknown).
    pub health: f64,
    /// The other players' living bodies.
    pub others: &'a [Seen],
    pub goal: Goal,
    /// Its team's spawn, where it falls back to.
    pub home: [f64; 3],
    pub navigation: Option<&'a Navigation>,
    /// The new walks its room's AIs may still make this tick
    /// ([`PATHS_PER_TICK`]); a new walk takes one.
    pub paths: &'a Cell<usize>,
    /// Whether nothing of the map stands between two points.
    pub sight: &'a dyn Fn([f64; 3], [f64; 3]) -> bool,
    /// Whether its body walks straight from one floor point to another
    /// (`PreparedScene::walk_test`): its walks are made with it
    /// ([`crate::navigate`]).
    pub walkable: &'a dyn Fn([f64; 3], [f64; 3]) -> bool,
}

/// The enemy an AI shoots at: the nearest living enemy within `range` it
/// can see from `eye`. The current target stays chosen while it remains a
/// candidate, unless another is less than half as far.
pub fn choose_target(
    eye: [f64; 3],
    team: u8,
    others: &[Seen],
    current: Option<usize>,
    range: f64,
    visible: impl Fn(&Seen) -> bool,
) -> Option<usize> {
    let candidates: Vec<(f64, &Seen)> = others
        .iter()
        .filter(|other| other.team != team)
        .map(|other| (distance(eye, other.chest()), other))
        .filter(|&(gap, other)| gap <= range && visible(other))
        .collect();
    let nearest = candidates
        .iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .copied()?;
    match candidates
        .iter()
        .find(|(_, other)| Some(other.slot) == current)
    {
        Some(&(gap, kept)) if nearest.0 * 2.0 >= gap => Some(kept.slot),
        _ => Some(nearest.1.slot),
    }
}

/// The command angles (yaw, pitch) that look from `eye` at `point`: the
/// forward axis is (sin yaw, 0, cos yaw), and the pitch is positive looking
/// down (`orpheus_wire::commands`).
pub fn aim_at(eye: [f64; 3], point: [f64; 3]) -> (i32, i32) {
    let [dx, dy, dz] = [0, 1, 2].map(|axis| point[axis] - eye[axis]);
    let yaw = dx.atan2(dz);
    let pitch = (-dy).atan2(dx.hypot(dz));
    (
        (yaw * ANGLE_UNITS).round() as i32,
        (pitch * ANGLE_UNITS).round() as i32,
    )
}

/// The throttles (forward, right) that walk along the ground direction
/// `(dx, dz)` while looking at `yaw`: the right axis is (-cos yaw, 0,
/// sin yaw) (`orpheus_wire::commands`).
pub fn throttles(yaw: i32, direction: [f64; 2]) -> (i8, i8) {
    let [dx, dz] = direction;
    let length = dx.hypot(dz);
    if length < 1e-6 {
        return (0, 0);
    }
    let (sin, cos) = (f64::from(yaw) / ANGLE_UNITS).sin_cos();
    let forward = (dx * sin + dz * cos) / length;
    let right = (-dx * cos + dz * sin) / length;
    let most = forward.abs().max(right.abs());
    let scale = |value: f64| (value / most * 127.0).round() as i8;
    (scale(forward), scale(right))
}

/// The waypoint to walk to from `feet`: those within [`ARRIVE`] metres are
/// passed, the last within [`HOLD`], and any it has walked past (beyond it
/// on the way from the waypoint before: a body that takes its commands far
/// apart overshoots, and turning back would overshoot again); `None` once
/// the last is passed.
pub fn steer(feet: [f64; 3], waypoints: &[[f64; 3]], next: &mut usize) -> Option<[f64; 3]> {
    while let Some(&point) = waypoints.get(*next) {
        let last = *next + 1 == waypoints.len();
        let reach = if last { HOLD } else { ARRIVE };
        let walked_past = next
            .checked_sub(1)
            .and_then(|before| waypoints.get(before))
            .is_some_and(|&from| {
                let way = [point[0] - from[0], point[2] - from[2]];
                (feet[0] - point[0]) * way[0] + (feet[2] - point[2]) * way[1] > 0.0
            });
        if flat_distance(feet, point) > reach && !walked_past {
            return Some(point);
        }
        *next += 1;
    }
    None
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn flat_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn to_nav(point: [f64; 3]) -> [f32; 3] {
    point.map(|value| value as f32)
}

fn from_nav(point: [f32; 3]) -> [f64; 3] {
    point.map(f64::from)
}

/// A walk being followed.
#[derive(Clone, Debug)]
struct Walking {
    /// Where it leads.
    goal: [f64; 3],
    waypoints: Vec<[f64; 3]>,
    next: usize,
    /// When it was made.
    made: u32,
}

/// The enemy being shot at.
#[derive(Clone, Copy, Debug)]
struct Aim {
    slot: usize,
    /// The first tick it may shoot at him (its reaction time).
    from: u32,
}

/// One AI's decisions, tick after tick.
#[derive(Clone, Debug)]
pub struct AiController {
    difficulty: Difficulty,
    random: Random,
    /// Its place among its team's AIs, and their number: which standing
    /// cell it takes and when it leaves the spawn.
    rank: usize,
    team_size: usize,
    body: Option<u32>,
    walk: Option<Walking>,
    /// The objective and the standing cell it took there.
    spot: Option<([f64; 3], [f64; 3])>,
    /// When it may start walking, once it has somewhere to go.
    leaves: Option<u32>,
    target: Option<Aim>,
    /// It falls back until this tick.
    retreat: Option<u32>,
    ability_ready: u32,
    /// Where it was when it last made progress, and the frames its body
    /// walked since.
    progress: Option<([f64; 3], u32)>,
    /// Its last command, and the frames of walking it gave its body.
    last: Option<Command>,
    walked: u32,
    /// Stepping aside after it got stuck: the frames of walking left, and
    /// the side; the side of the next time.
    sidestep: Option<(u32, i8)>,
    sidestep_side: i8,
    strafe: i8,
    strafe_until: u32,
    /// The current burst: firing until `burst.0`, then pausing until
    /// `burst.1`.
    burst: (u32, u32),
    yaw: i32,
    pitch: i32,
}

impl AiController {
    pub fn new(difficulty: Difficulty, rank: usize, team_size: usize, seed: u64) -> Self {
        Self {
            difficulty,
            random: Random::new(seed),
            rank,
            team_size: team_size.max(1),
            body: None,
            walk: None,
            spot: None,
            leaves: None,
            target: None,
            retreat: None,
            ability_ready: 0,
            progress: None,
            last: None,
            walked: 0,
            sidestep: None,
            sidestep_side: 127,
            strafe: 127,
            strafe_until: 0,
            burst: (0, 0),
            yaw: 0,
            pitch: 0,
        }
    }

    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// The player it shoots at, if any.
    pub fn target(&self) -> Option<usize> {
        self.target.map(|aim| aim.slot)
    }

    /// Whether it is falling back to its spawn.
    pub fn retreating(&self) -> bool {
        self.retreat.is_some()
    }

    /// This tick's command.
    pub fn command(&mut self, senses: &Senses<'_>) -> Command {
        let tick = senses.tick;
        let mut command = Command {
            frame: tick,
            yaw: self.yaw,
            pitch: self.pitch,
            ..Command::default()
        };
        let Some(feet) = senses.feet else {
            // Dead or not yet in the world: no input, as a client's.
            self.target = None;
            self.walk = None;
            self.last = None;
            self.walked = 0;
            return command;
        };
        if senses.body != self.body {
            // A new body at the spawn: everything starts again.
            self.body = senses.body;
            self.walk = None;
            self.leaves = None;
            self.retreat = None;
            self.progress = None;
            self.sidestep = None;
            self.last = None;
            self.target = None;
        }
        let eye = [feet[0], feet[1] + senses.eye, feet[2]];

        // Fall back when hurt, once in a while with its ability.
        match self.retreat {
            None if senses.health < RETREAT_BELOW => {
                self.retreat = Some(tick + ticks(RETREAT));
                self.walk = None;
                if tick >= self.ability_ready {
                    self.ability_ready = tick + ticks(ABILITY_COOLDOWN);
                    command.buttons |= ABILITY_2;
                }
            }
            Some(until) if senses.health >= RECOVERED || tick >= until => {
                self.retreat = None;
                self.walk = None;
            }
            _ => {}
        }

        // Whom to shoot.
        let sight = senses.sight;
        let chosen = choose_target(
            eye,
            senses.team,
            senses.others,
            self.target.map(|aim| aim.slot),
            ENGAGE_RANGE,
            |other| sight(eye, other.chest()),
        );
        self.target = chosen.map(|slot| match self.target {
            Some(aim) if aim.slot == slot => aim,
            _ => Aim {
                slot,
                from: tick + ticks(self.difficulty.reaction()),
            },
        });
        let target = self
            .target
            .and_then(|aim| senses.others.iter().find(|other| other.slot == aim.slot));

        // Where to go.
        let destination = if self.retreat.is_some() {
            Some(senses.home)
        } else {
            match senses.goal {
                Goal::Hold => None,
                Goal::Objective(objective) => Some(self.spot(senses.navigation, objective)),
                Goal::Hunt => target.map(|enemy| enemy.feet).or_else(|| {
                    senses
                        .others
                        .iter()
                        .filter(|other| other.team != senses.team)
                        .min_by(|a, b| distance(feet, a.feet).total_cmp(&distance(feet, b.feet)))
                        .map(|enemy| enemy.feet)
                }),
            }
        };
        let leaves = match destination {
            Some(_) => *self
                .leaves
                .get_or_insert(tick + ticks(STAGGER * self.rank as f64)),
            None => u32::MAX,
        };
        // Stepping aside after it got stuck, for that much of its walking.
        let aside = match self.sidestep {
            Some((left, side)) if left > self.walked => {
                self.sidestep = Some((left - self.walked, side));
                Some(side)
            }
            _ => {
                self.sidestep = None;
                None
            }
        };
        let heading = match destination {
            Some(goal) if tick >= leaves && aside.is_none() => self.next_point(senses, feet, goal),
            Some(_) => None,
            None => {
                self.walk = None;
                None
            }
        };

        // Where to look.
        match target {
            Some(enemy) => {
                let (yaw, pitch) = aim_at(eye, enemy.chest());
                let error = self.difficulty.aim_error() * ANGLE_UNITS;
                command.yaw = yaw + (self.random.signed() * error).round() as i32;
                command.pitch = pitch + (self.random.signed() * error).round() as i32;
            }
            None => {
                if let Some(point) = heading {
                    let (yaw, _) = aim_at(eye, point);
                    command.yaw = yaw;
                    command.pitch = 0;
                }
            }
        }

        // How to move.
        if let Some(side) = aside {
            command.right = side;
        } else if let Some(point) = heading {
            let direction = [point[0] - feet[0], point[2] - feet[2]];
            (command.forward, command.right) = throttles(command.yaw, direction);
            if self.stuck(feet) {
                // Jump and step aside; a new walk from there.
                command.buttons |= JUMP;
                (command.forward, command.right) = (0, self.sidestep_side);
                self.sidestep = Some((ticks(SIDESTEP), self.sidestep_side));
                self.sidestep_side = -self.sidestep_side;
                self.walk = None;
            }
        } else {
            self.progress = None;
            if target.is_some() && destination.is_some() {
                // Holding its spot in a fight: sidestep.
                if tick >= self.strafe_until {
                    self.strafe = if self.random.below(2) == 0 { 127 } else { -127 };
                    self.strafe_until = tick + ticks(STRAFE);
                }
                command.right = self.strafe;
            }
        }

        // When to fire.
        if self.target.is_some_and(|aim| tick >= aim.from) {
            if tick >= self.burst.1 {
                self.burst = (tick + ticks(BURST), tick + ticks(BURST + BURST_PAUSE));
            }
            if tick < self.burst.0 {
                command.buttons |= FIRE;
            }
        }

        self.yaw = command.yaw;
        self.pitch = command.pitch;
        // The frames between two commands are the body's last input held
        // (`orpheus_physics::mover::kinematics::poses`): how far this one
        // walks it.
        let walking = |pose: &Command| pose.forward != 0 || pose.right != 0;
        self.walked = poses(self.last, command).filter(walking).count() as u32;
        self.last = Some(command);
        command
    }

    /// Its standing cell at the objective: the cells spread around it, one
    /// per AI of the team, its own by rank. A new objective: a new walk.
    fn spot(&mut self, navigation: Option<&Navigation>, objective: [f64; 3]) -> [f64; 3] {
        if let Some((at, cell)) = self.spot {
            if flat_distance(at, objective) < GOAL_MOVED {
                return cell;
            }
        }
        let cells = navigation
            .map(|navigation| navigation.free_cells(to_nav(objective), SPREAD, self.team_size))
            .unwrap_or_default();
        let cell = cells
            .get(self.rank % cells.len().max(1))
            .map_or(objective, |&cell| from_nav(cell));
        self.spot = Some((objective, cell));
        self.walk = None;
        cell
    }

    /// The point to walk to next on the way to `goal`; `None` once there
    /// or at the end of its walk (a goal off the walkable cells ends the
    /// walk at the nearest one). A new walk ([`crate::navigate::path`], the
    /// path query the Training Bots use too) when there is none, the goal
    /// moved (at most every [`REPATH`] seconds while the walk goes on) or
    /// the AI was pushed off the end of its walk, as the room's paths this
    /// tick allow; until then the old walk, if any.
    fn next_point(
        &mut self,
        senses: &Senses<'_>,
        feet: [f64; 3],
        goal: [f64; 3],
    ) -> Option<[f64; 3]> {
        let tick = senses.tick;
        let stale = self.walk.as_ref().is_none_or(|walk| {
            let ended = walk.next >= walk.waypoints.len();
            let moved = flat_distance(walk.goal, goal) >= GOAL_MOVED
                && (ended || tick >= walk.made + ticks(REPATH));
            let pushed = ended
                && walk
                    .waypoints
                    .last()
                    .is_some_and(|&end| flat_distance(feet, end) > HOLD + ARRIVE);
            moved || pushed
        });
        if stale {
            if flat_distance(feet, goal) <= HOLD {
                self.walk = None;
                return None;
            }
            if senses.paths.get() > 0 {
                senses.paths.set(senses.paths.get() - 1);
                let waypoints = navigate::path(senses.navigation, feet, goal, |from, to| {
                    (senses.walkable)(from, to)
                })
                .unwrap_or_else(|| vec![goal]);
                self.walk = Some(Walking {
                    goal,
                    waypoints,
                    next: 0,
                    made: tick,
                });
            }
        }
        let walk = self.walk.as_mut()?;
        steer(feet, &walk.waypoints, &mut walk.next)
    }

    /// Whether its body's last second of walking moved it less than
    /// [`STUCK_DISTANCE`]; progress starts the count again. The count is of
    /// the frames its commands walked its body, not of ticks: a server
    /// behind on its ticks has its body take fewer, farther apart commands,
    /// which is no reason to think it stuck.
    fn stuck(&mut self, feet: [f64; 3]) -> bool {
        let walked = self.walked;
        match &mut self.progress {
            Some((at, _)) if flat_distance(*at, feet) >= STUCK_DISTANCE => {
                self.progress = Some((feet, 0));
                false
            }
            Some((_, frames)) => {
                *frames += walked;
                if *frames >= ticks(STUCK) {
                    self.progress = Some((feet, 0));
                    return true;
                }
                false
            }
            None => {
                self.progress = Some((feet, 0));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
