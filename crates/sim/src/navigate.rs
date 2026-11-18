//! The server's one path query, for every body it walks without a client:
//! the Training Bots' patrols ([`crate::bots`], the points their graph
//! `0B8E`'s AI movement state `STU_ECAEE401` asks for) and the AI heroes of
//! Practice vs AI ([`crate::ai`]). Retail ran this natively: the AI
//! locomotion of component 88 runs navigation queries (`sub_7FF789AF0850`
//! from its callbacks `0x7FF789B07F20`/`0x7FF789B083E0`), and no graph
//! paths (`D:/Work/Coding/refs/reports/RustInsteadOfGraphs.md` §9).
//!
//! A walk is the map's navigation's ([`Navigation::path_with`], over the AI
//! tactical volumes), checked with the walker's own standing capsule on the
//! map's collision ([`walkable`]): the volumes' cells are a metre wide and
//! their heights a layer (1.4 m) coarse, so a straight line they find clear
//! can graze a wall's corner, cross a railing or step down a ledge, and a
//! step to another layer can be a ledge. The client's own path query
//! searches the chunk's finer AI cell data instead (`orpheus_physics::nav`
//! module notes).

use orpheus_physics::{
    contact::body::CharacterBody,
    mover::ground::standing_origin,
    nav::{LAYER, Navigation},
};

/// The most a floor may rise or fall under one step of a straight walk
/// (the Training Bots' step up, `bots.rs`).
pub const STEP: f64 = 0.6;

/// The points to walk through from `from` to `to`: the turns of the map's
/// navigation walk between them, then its end (`to` on the floor when it
/// stands on walkable cells, else the nearest cell's centre). A turn or line
/// is kept only where `walkable` says a body walks it straight. `None`
/// without navigation, or when it has no walk between the points.
pub fn path(
    navigation: Option<&Navigation>,
    from: [f64; 3],
    to: [f64; 3],
    mut walkable: impl FnMut([f64; 3], [f64; 3]) -> bool,
) -> Option<Vec<[f64; 3]>> {
    let walk = navigation?.path_with(to_nav(from), to_nav(to), |a, b| {
        walkable(from_nav(a), from_nav(b))
    })?;
    let mut points: Vec<[f64; 3]> = walk.waypoints.into_iter().map(from_nav).collect();
    // The first is where it stands (on the floor), unless that is off the
    // walkable cells and it snapped to a cell.
    if flat_distance(points[0], from) < 1.0e-3 {
        points.remove(0);
    }
    Some(points)
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
    let raise = |[x, y, z]: [f64; 3]| standing_origin([x, y + hump, z], lift);
    body.sweep(raise(from), raise(to))
        .is_ok_and(|hit| hit.is_none())
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
