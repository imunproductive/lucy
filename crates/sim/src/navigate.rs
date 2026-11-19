//! The server's one path query, for every body it walks without a client:
//! the Training Bots' patrols ([`crate::bots`], the points their graph
//! `0B8E`'s AI movement state `STU_ECAEE401` asks for) and the AI heroes of
//! Practice vs AI ([`crate::ai`]). Retail ran this natively: the AI
//! locomotion of component 88 runs navigation queries (`sub_7FF789AF0850`
//! from its callbacks `0x7FF789B07F20`/`0x7FF789B083E0`), and no graph
//! paths (`D:/Work/Coding/refs/reports/RustInsteadOfGraphs.md` §9).
//!
//! A walk is searched over the map's AI cell data, the client's own walk
//! grid ([`WalkSearch`], when the navigation has the cells attached, as
//! every derived map does), else over its tactical volumes
//! ([`Navigation::path_with`]); either way checked with the walker's own
//! standing capsule on the map's collision ([`walkable`]).
//!
//! A path can take tens of milliseconds (a long walk over a wide map, steps
//! refused and searched round), more than a tick, so the server searches a
//! walk a bounded amount at a time ([`PathQuery`]): a room's walkers share
//! [`TICK_WORK`] of path work a tick ([`PathBudget`]), and a search that
//! runs out of it goes on next tick, to the same walk.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use orpheus_physics::{
    ai_cells::WalkSearch,
    contact::body::CharacterBody,
    mover::ground::standing_origin,
    nav::{LAYER, Navigation, PathSearch},
};

/// The most a floor may rise or fall under one step of a straight walk
/// (the Training Bots' step up, `bots.rs`).
pub const STEP: f64 = 0.6;

/// The path work a room's walkers may do in a tick, in the units of
/// [`PathSearch::advance`] (about a microsecond of a release build): 3 ms of
/// the 16 ms tick, so a tick spends at most about 4 ms on paths with the
/// last step's overrun (measured 2026-11-19,
/// `D:/Work/Coding/refs/reports/Navigation.md`; assumption: room enough for
/// the rest of a tick).
pub const TICK_WORK: u32 = 3000;

/// The path work a room's walkers share in a tick ([`TICK_WORK`]): the
/// Training Bots' walks and the AI heroes'. A new tick has all of it again.
#[derive(Debug)]
pub struct PathBudget {
    per_tick: u32,
    tick: AtomicU32,
    left: AtomicU32,
}

impl Default for PathBudget {
    fn default() -> Self {
        Self::new(TICK_WORK)
    }
}

impl PathBudget {
    pub const fn new(per_tick: u32) -> Self {
        Self {
            per_tick,
            tick: AtomicU32::new(u32::MAX),
            left: AtomicU32::new(per_tick),
        }
    }

    /// Work without a limit (a walker alone in a test).
    pub const fn unlimited() -> Self {
        Self::new(u32::MAX)
    }

    /// The work left in `tick`.
    pub fn left(&self, tick: u32) -> u32 {
        self.spend(tick, |left| *left)
    }

    /// `run` spends from the work left in `tick`.
    fn spend<T>(&self, tick: u32, run: impl FnOnce(&mut u32) -> T) -> T {
        if self.tick.swap(tick, Relaxed) != tick {
            self.left.store(self.per_tick, Relaxed);
        }
        let mut left = self.left.load(Relaxed);
        let result = run(&mut left);
        self.left.store(left, Relaxed);
        result
    }
}

/// A walk being searched for, a tick's share of work at a time: the walk
/// [`path`] gives, however many ticks it takes.
#[derive(Clone, Debug)]
pub struct PathQuery {
    from: [f64; 3],
    to: [f64; 3],
    /// `None` until the first call, which picks the search the navigation
    /// has data for.
    search: Option<Search>,
}

/// The search over the map's AI cells, or over its tactical volumes when
/// the navigation has no cells attached.
#[derive(Clone, Debug)]
enum Search {
    Cells(WalkSearch),
    Volumes(PathSearch),
}

impl Search {
    fn new(navigation: &Navigation, from: [f64; 3], to: [f64; 3]) -> Self {
        if navigation.ai_cells().is_some() {
            Self::Cells(WalkSearch::new(to_nav(from), to_nav(to)))
        } else {
            Self::Volumes(PathSearch::new(to_nav(from), to_nav(to)))
        }
    }

    /// Search on while `work` is left: `None` while it goes on, then the
    /// walk's points (the first where it starts).
    fn advance(
        &mut self,
        navigation: &Navigation,
        work: &mut u32,
        mut walkable: impl FnMut([f64; 3], [f64; 3]) -> bool,
    ) -> Option<Option<Vec<[f32; 3]>>> {
        let walkable = |a, b| walkable(from_nav(a), from_nav(b));
        match self {
            Self::Cells(search) => {
                let cells = navigation.ai_cells().expect("picked with cells");
                search.advance(cells, work, walkable)
            }
            Self::Volumes(search) => search
                .advance(navigation, work, walkable)
                .map(|walk| walk.map(|walk| walk.waypoints)),
        }
    }
}

impl PathQuery {
    pub fn new(from: [f64; 3], to: [f64; 3]) -> Self {
        Self {
            from,
            to,
            search: None,
        }
    }

    /// Search on with the work left in the room's `tick`: `None` while the
    /// search goes on, then what [`path`] gives (at once without
    /// navigation).
    pub fn advance(
        &mut self,
        navigation: Option<&Navigation>,
        budget: &PathBudget,
        tick: u32,
        walkable: impl FnMut([f64; 3], [f64; 3]) -> bool,
    ) -> Option<Option<Vec<[f64; 3]>>> {
        let Some(navigation) = navigation else {
            return Some(None);
        };
        let search = self
            .search
            .get_or_insert_with(|| Search::new(navigation, self.from, self.to));
        let walk = budget.spend(tick, |work| search.advance(navigation, work, walkable))?;
        Some(walk.map(|walk| points(walk, self.from)))
    }
}

/// The points to walk through from `from` to `to`: the turns of the map's
/// walk between them, then its end (`to` on the floor when it stands on the
/// walkable grid, else the nearest walkable point). A turn or line is kept
/// only where `walkable` says a body walks it straight. `None` without
/// navigation, or when it has no walk between the points. The whole search
/// at once ([`PathQuery`] spreads it over ticks).
pub fn path(
    navigation: Option<&Navigation>,
    from: [f64; 3],
    to: [f64; 3],
    mut walkable: impl FnMut([f64; 3], [f64; 3]) -> bool,
) -> Option<Vec<[f64; 3]>> {
    let navigation = navigation?;
    let mut search = Search::new(navigation, from, to);
    loop {
        let mut work = u32::MAX;
        if let Some(walk) = search.advance(navigation, &mut work, &mut walkable) {
            return walk.map(|walk| points(walk, from));
        }
    }
}

fn points(walk: Vec<[f32; 3]>, from: [f64; 3]) -> Vec<[f64; 3]> {
    let mut points: Vec<[f64; 3]> = walk.into_iter().map(from_nav).collect();
    // The first is where it stands (on the floor), unless that is off the
    // walkable grid and it snapped.
    if flat_distance(points[0], from) < 1.0e-3 {
        points.remove(0);
    }
    points
}

/// Whether a body with this standing capsule (`lift` above its feet) walks
/// straight from one floor point to another, as a walker on the floor
/// would: both points have floor under them within a layer of their heights
/// (navigation heights are estimates, rounded up: the floor at or just
/// under the height first, else the one above); stepping along the line a
/// body width
/// at a time, each step finds floor at most [`STEP`] above the last or at
/// most [`STEP`] plus a 45° slope below it, and the last ends within
/// [`STEP`] of the far point's floor; and the capsule sweeps from one point
/// to the other without hitting the map, lifted by as much as the floor
/// rises over the straight line between them (at most [`STEP`]: the line
/// can't cut through a hump). `ground(x, z, top, bottom)` is the highest
/// floor between the two heights, if any.
pub fn walkable(
    body: &CharacterBody,
    lift: f32,
    ground: &impl Fn(f64, f64, f64, f64) -> Option<f64>,
    from: [f64; 3],
    to: [f64; 3],
) -> bool {
    let reach = f64::from(LAYER);
    let settle = |[x, y, z]: [f64; 3]| {
        ground(x, z, y + STEP, y - reach)
            .or_else(|| ground(x, z, y + reach, y + STEP))
            .map(|y| [x, y, z])
    };
    let (Some(from), Some(to)) = (settle(from), settle(to)) else {
        return false;
    };
    let width = 2.0 * f64::from(body.radius());
    let samples = (flat_distance(from, to) / width).ceil().max(1.0) as usize;
    let mut floor = from[1];
    let mut hump = 0.0f64;
    for sample in 1..=samples {
        let t = sample as f64 / samples as f64;
        let x = from[0] + (to[0] - from[0]) * t;
        let z = from[2] + (to[2] - from[2]) * t;
        let Some(next) = ground(x, z, floor + STEP, floor - STEP - width) else {
            return false;
        };
        floor = next;
        hump = hump.max(floor - (from[1] + (to[1] - from[1]) * t));
    }
    if (floor - to[1]).abs() > STEP || hump > STEP {
        return false;
    }
    // The sweep a piece at a time: a short sweep's bounds meet few of the
    // map's shapes, and the first hit ends the test.
    let [start, end] = [from, to].map(|[x, y, z]| standing_origin([x, y + hump, z], lift));
    let pieces = (flat_distance(from, to) / SWEEP_PIECE).ceil().max(1.0) as usize;
    let at = |piece: usize| {
        let t = piece as f32 / pieces as f32;
        [0, 1, 2].map(|axis| start[axis] + (end[axis] - start[axis]) * t)
    };
    (0..pieces).all(|piece| {
        body.sweep(at(piece), at(piece + 1))
            .is_ok_and(|hit| hit.is_none())
    })
}

/// The length of the pieces [`walkable`] sweeps the capsule over: a sweep
/// over a long line meets every shape in its bounds (measured 2026-11-19:
/// 2 m pieces gave the same answers over 106 000 walk tests, 18% faster).
const SWEEP_PIECE: f64 = 2.0;

fn flat_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

fn to_nav(point: [f64; 3]) -> [f32; 3] {
    point.map(|value| value as f32)
}

fn from_nav(point: [f32; 3]) -> [f64; 3] {
    point.map(f64::from)
}
