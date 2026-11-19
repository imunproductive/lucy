//! What the client does around its path query for Widowmaker's ledge pull
//! (mover behaviour kind 10, `STU_1F5ED928`; its path build `0x7FF789B5EC50`):
//! it lifts the start and goal into the air graph, refuses a goal the air
//! can't reach nearby (the query's mode-1 start `0x7FF78A627A60`), runs the
//! query ([`client_path`]) and turns its voxels into a polyline.
//!
//! - The goal (the hook's point) goes to the first air voxel above the floor
//!   under it: a voxel in the air drops up to 3 voxels until it leaves the
//!   air, one outside it climbs up to 4 until the next is air
//!   (`0x7FF789AED5F0(cursor, grid, 3, 3)`; when neither finds it the voxel
//!   stays), then it goes one voxel up (`0x7FF789AED860(cursor, grid, 1,
//!   1)`), and the goal is that voxel's point (`0x7FF78A61A7E0`).
//! - The start (the feet) goes through `0x7FF78A615C30` ([`lift`]): a voxel
//!   outside the air climbs the same way and then one up into it; when it
//!   can't, a search over the voxels around it (6 directions, each step 1,
//!   one more unless it goes up, one more into a voxel outside the air,
//!   expanding voxels at most `search` away) takes the nearest air voxel. The
//!   start is that voxel's point.
//! - The query's start (mode 1) refuses a start or goal outside the air (−4),
//!   a goal outside its window of 5 × 7 × 5 units (−2), and a goal two A*
//!   searches don't reach (−2): one over the air graph's regions (each
//!   cell's connected parts, linked to the next cell's) that expands nodes
//!   at most 48 m along (`0x7FF78A616DC0`, three cells), one over its units
//!   that expands nodes at most 4.9 m along (`0x7FF78A616A70`, half the
//!   window's largest side). Their costs (`0x7FF78A611E80`): the path runs
//!   through the middles of the links, an estimate of 1.1 × the distance
//!   from the next link's middle to the goal plus half of one minus the
//!   cosine between that link and the way to the goal.
//! - The polyline (`0x7FF789B57260`): the first voxel's point, then each
//!   voxel where the step direction changes and the last one, each at its
//!   voxel's middle (`0x7FF78A627930`); the first point is then the start.
//!
//! Every value is float32 in the client's order of operations; an `rsqrtps`
//! estimate is the correctly rounded value (as [`crate::contact`] does),
//! followed by the client's Newton step.

use super::{AiCells, DIRECTIONS, End, Graph, Node, OPPOSITE, Step, Vec3, Voxel, client_path};

/// The start search's open list room (`0x7FF78A614A60(.., 1024)`; a push
/// past it is dropped, `0x7FF78A61CFC0`).
const NODES: usize = 1024;
/// The query's window in units (x, y, z; `0x7FF789B51810(q, 2, 3, 2)`).
const WINDOW: [u8; 3] = [5, 7, 5];
/// What the path build's start search may expand (`0x7FF789B5EC50` sets the
/// config's third word to 2; the searches' own lifts use 8).
pub const START_SEARCH: u32 = 2;
const SEARCH_SEARCH: u32 = 8;
/// The client's "none" threshold for squared lengths (`1.1754944e-36`).
const TINY_SQUARED: f32 = 1.175_494_4e-36;

/// Why the query refused (the client's `out` codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// −4: the start or the goal isn't in the air graph.
    NotInAir,
    /// −2: the goal is outside the window or a search doesn't reach it.
    Unreachable,
    /// −1: the query's A* found no path.
    NoPath,
}

/// The path build's points: the lifted start, the lifted goal, and the
/// polyline (empty when the query refused).
#[derive(Clone, Debug, PartialEq)]
pub struct LedgePath {
    pub start: Vec3,
    pub goal: Vec3,
    pub path: Result<Vec<Vec3>, Refusal>,
}

/// `0x7FF789B5EC50` up to the ledge probes: lift `feet` and `point`, ask the
/// query in mode 1 and turn its path into the polyline.
pub fn ledge_path(cells: &AiCells, feet: Vec3, point: Vec3) -> LedgePath {
    let (start, _) = lift(cells, feet, START_SEARCH);
    let goal = lift_goal(cells, point).floor();
    let path = mode_one(cells, start, goal).map(|steps| {
        let mut points = polyline(&steps);
        points[0] = start;
        points
    });
    LedgePath { start, goal, path }
}

/// The query in mode 1 (`0x7FF78A627A60`, then [`client_path`]).
pub fn mode_one(cells: &AiCells, start: Vec3, goal: Vec3) -> Result<Vec<Step>, Refusal> {
    let (from, to) = (Voxel::of(start), Voxel::of(goal));
    let (Some(first), Some(last)) = (cells.node(Graph::Air, from), cells.node(Graph::Air, to))
    else {
        return Err(Refusal::NotInAir);
    };
    let window = (0..3)
        .all(|axis| (last.unit[axis] - first.unit[axis]).abs() <= i32::from(WINDOW[axis] >> 1));
    if !window {
        return Err(Refusal::Unreachable);
    }
    if !reaches(cells, Level::Regions, start, goal) || !reaches(cells, Level::Units, start, goal) {
        return Err(Refusal::Unreachable);
    }
    client_path(
        cells,
        Graph::Air,
        End {
            node: first,
            sub: from.sub(),
        },
        End {
            node: last,
            sub: to.sub(),
        },
    )
    .ok_or(Refusal::NoPath)
}

/// `0x7FF789B57260`: the first step's point, then the point of each step
/// whose direction from the one before differs from the last kept
/// direction, and of the last step.
pub fn polyline(steps: &[Step]) -> Vec<Vec3> {
    let Some(first) = steps.first() else {
        return Vec::new();
    };
    let mut points = vec![step_point(first)];
    let mut kept = steps.get(1).map_or(13, |step| step.direction);
    for (at, step) in steps.iter().enumerate().skip(1) {
        if step.direction != kept || at == steps.len() - 1 {
            points.push(step_point(step));
            kept = step.direction;
        }
    }
    points
}

/// `0x7FF78A627930`: the middle of a step's voxel.
fn step_point(step: &Step) -> Vec3 {
    let corner = unit_corner(step.node.unit);
    let voxel = [step.sub >> 4, step.sub & 3, (step.sub >> 2) & 3].map(f32::from);
    let half = [
        super::VOXEL * 0.5,
        super::VOXEL_HEIGHT * 0.5,
        super::VOXEL * 0.5,
    ];
    let size = [super::VOXEL, super::VOXEL_HEIGHT, super::VOXEL];
    std::array::from_fn(|axis| (half[axis] + corner[axis]) + voxel[axis] * size[axis])
}

/// `0x7FF78A619C30`: a unit's lowest corner, from its cell's (the loader
/// `0x7FF78A61CB80` puts a cell's corner at its key times the cell's size).
fn unit_corner(unit: [i32; 3]) -> Vec3 {
    let origin = cell_origin(
        Node {
            unit,
            place: 0,
            entry: 0,
        }
        .key(),
    );
    let size = [super::VOXEL, super::VOXEL_HEIGHT, super::VOXEL];
    std::array::from_fn(|axis| ((unit[axis] & 15) * 4) as f32 * size[axis] + origin[axis])
}

fn cell_origin(key: u32) -> Vec3 {
    let field = |shift: u32, mask: u32, bias: i32| ((key >> shift) & mask) as i32 - bias;
    [
        field(0, 0xFFF, 2048) as f32 * super::CELL,
        field(24, 0xFF, 128) as f32 * super::CELL_HEIGHT,
        field(12, 0xFFF, 2048) as f32 * super::CELL,
    ]
}

/// Whether the air graph holds a voxel (the cursor's flag, `0x7FF78A5FC780`).
fn air(cells: &AiCells, voxel: Voxel) -> bool {
    cells.find(Graph::Air, voxel).is_some()
}

/// `0x7FF789AED5F0(cursor, grid, up, down)`: a voxel in the air goes down up
/// to `down` voxels to the first outside it; one outside goes up to `up + 1`
/// voxels and stops under the first in it. `None` (the voxel stays) when
/// neither finds it.
fn settle(cells: &AiCells, voxel: Voxel, up: u32, down: u32) -> Option<Voxel> {
    let step = |voxel: Voxel, delta: i32| Voxel {
        y: voxel.y + delta,
        ..voxel
    };
    let mut at = voxel;
    if air(cells, voxel) {
        for _ in 0..down {
            at = step(at, -1);
            if !air(cells, at) {
                return Some(at);
            }
        }
    } else {
        for _ in 0..=up {
            let below = at;
            at = step(at, 1);
            if air(cells, at) {
                return Some(below);
            }
        }
    }
    None
}

/// The path build's goal (`0x7FF789B5EC50`): [`settle`] by 3 and 3, then
/// one voxel up.
pub fn lift_goal(cells: &AiCells, point: Vec3) -> Voxel {
    let voxel = Voxel::of(point);
    let voxel = settle(cells, voxel, 3, 3).unwrap_or(voxel);
    Voxel {
        y: voxel.y + 1,
        ..voxel
    }
}

/// `0x7FF78A615C30` (config `{3, 3, search}`, its byte 13 set): the point
/// of the air voxel a point lifts to, and that voxel when there is one (the
/// call's result). The point is the voxel the lift ended on even when it
/// failed.
pub fn lift(cells: &AiCells, point: Vec3, search: u32) -> (Vec3, Option<Voxel>) {
    let voxel = Voxel::of(point);
    let (voxel, lifted) = if air(cells, voxel) {
        (voxel, true)
    } else {
        match settle(cells, voxel, 3, 3) {
            Some(below) => (
                Voxel {
                    y: below.y + 1,
                    ..below
                },
                true,
            ),
            None => (voxel, false),
        }
    };
    let out = voxel.floor();
    if !cells.has_cell(voxel.key()) || (!lifted && search == 0) {
        return (out, None);
    }
    if lifted && air(cells, voxel) {
        return (out, Some(voxel));
    }
    if search == 0 {
        return (out, None);
    }
    match nearest_air(cells, voxel, search as f32) {
        Some(found) => (found.floor(), Some(found)),
        None => (out, None),
    }
}

/// `0x7FF78A615C30`'s search: from `voxel`, the six neighbours of each
/// voxel at most `reach` steps away (`0x7FF78A6199B0`, table
/// `0x7FF78BD2F420`: up, down, +x, −x, +z, −z), each step costing 1, the
/// estimate one more unless it goes up and one more into a voxel outside
/// the air; the first voxel taken from the open list that is in the air.
fn nearest_air(cells: &AiCells, voxel: Voxel, reach: f32) -> Option<Voxel> {
    const STEPS: [(usize, i32, bool); 6] = [
        (1, 1, true),
        (1, -1, false),
        (0, 1, false),
        (0, -1, false),
        (2, 1, false),
        (2, -1, false),
    ];
    struct Seen {
        voxel: Voxel,
        cost: f32,
        total: f32,
        open: bool,
        closed: bool,
        air: bool,
    }
    let mut seen = vec![Seen {
        voxel,
        cost: 0.0,
        total: 0.0,
        open: true,
        closed: false,
        air: air(cells, voxel),
    }];
    let mut index = std::collections::HashMap::from([(voxel, 0usize)]);
    let mut heap = Heap::default();
    heap.push(0);
    while let Some(at) = heap.pop(|id| seen[id].total) {
        seen[at].closed = true;
        if seen[at].air && cells.find(Graph::Air, seen[at].voxel).is_some() {
            return Some(seen[at].voxel);
        }
        if reach < seen[at].cost {
            continue;
        }
        for (axis, delta, free) in STEPS {
            let mut next = seen[at].voxel;
            match axis {
                0 => next.x += delta,
                1 => next.y += delta,
                _ => next.z += delta,
            }
            let id = *index.entry(next).or_insert_with(|| {
                seen.push(Seen {
                    voxel: next,
                    cost: 0.0,
                    total: 0.0,
                    open: false,
                    closed: false,
                    air: false,
                });
                seen.len() - 1
            });
            if seen[id].closed {
                continue;
            }
            let open = seen[id].open;
            if !open {
                seen[id].air = air(cells, next);
            }
            let cost = seen[at].cost + 1.0;
            let mut total = cost;
            if !free {
                total += 1.0;
            }
            if !seen[id].air {
                total += 1.0;
            }
            if !open || total < seen[id].total {
                seen[id].cost = cost;
                seen[id].total = total;
                if open {
                    heap.rise_item(id, |id| seen[id].total);
                } else if heap.len() < NODES {
                    seen[id].open = true;
                    heap.push(id);
                    heap.rise_last(|id| seen[id].total);
                }
            }
        }
    }
    None
}

/// Which graph of the air the reach searches walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Level {
    /// The cells' regions (`0x7FF78A6183F0`, nodes `1 | region << 12 | key
    /// << 32`, links `0x7FF78A61BE70`).
    Regions,
    /// The units (`0x7FF78A617060`, the client's unit nodes, links
    /// [`AiCells::linked`]).
    Units,
}

impl Level {
    /// How far along the search expands a node: three cells for the regions
    /// (`0x7FF78A627A60`: a voxel × 64 × 3, 48 m), half the window's largest
    /// side for the units (4.9 m).
    fn reach(self) -> f32 {
        match self {
            Self::Regions => super::VOXEL * 64.0 * 3.0,
            Self::Units => {
                let unit = super::VOXEL * 4.0;
                let tallest = (f32::from(WINDOW[1]) * (super::VOXEL_HEIGHT * 4.0))
                    .max(f32::from(WINDOW[2]) * unit);
                (f32::from(WINDOW[0]) * unit).max(tallest) * 0.5
            }
        }
    }
}

/// A node of a reach search: its id, its word (the region's u64 or the
/// entry's u32) and where its cell is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Place {
    id: u64,
    word: u64,
    node: Option<Node>,
}

struct Visit {
    place: Place,
    point: Vec3,
    parent: Option<usize>,
    cost: f32,
    total: f32,
    open: bool,
    closed: bool,
}

/// `0x7FF78A616DC0` / `0x7FF78A616A70`: whether an A* from `start` over
/// `level`, expanding only nodes at most [`Level::reach`] along, takes the
/// goal's node from its open list.
pub(super) fn reaches(cells: &AiCells, level: Level, start: Vec3, goal: Vec3) -> bool {
    let reach = level.reach();
    let Some(target) = locate(cells, level, goal).map(|(_, place)| place.id) else {
        return false;
    };
    let Some((point, place)) = locate(cells, level, start) else {
        return false;
    };
    let mut visits = vec![Visit {
        place,
        point,
        parent: None,
        cost: 0.0,
        total: 0.0,
        open: true,
        closed: false,
    }];
    let (cost, estimate) = step_cost(&visits, 0, point, goal);
    visits[0].total = estimate + cost;
    let mut index = std::collections::HashMap::from([(place.id, 0usize)]);
    let mut heap = Heap::default();
    heap.push(0);
    let mut links = Vec::new();
    while let Some(at) = heap.pop(|id| visits[id].total) {
        visits[at].closed = true;
        if visits[at].place.id == target {
            return true;
        }
        if visits[at].cost > reach {
            continue;
        }
        neighbours(cells, level, visits[at].place, &mut links);
        for &(next, _) in &links {
            let id = match index.get(&next.id) {
                Some(&id) => id,
                None => {
                    visits.push(Visit {
                        place: next,
                        point: [0.0; 3],
                        parent: None,
                        cost: 0.0,
                        total: 0.0,
                        open: false,
                        closed: false,
                    });
                    index.insert(next.id, visits.len() - 1);
                    visits.len() - 1
                }
            };
            if visits[id].closed {
                continue;
            }
            let open = visits[id].open;
            if !open {
                visits[id].point = place_point(cells, level, next);
            }
            let (cost, estimate) = step_cost(&visits, at, visits[id].point, goal);
            let total = estimate + cost;
            if !open || total < visits[id].total {
                let visit = &mut visits[id];
                visit.cost = cost;
                visit.total = total;
                visit.parent = Some(at);
                visit.place.word = next.word;
                if open {
                    heap.rise_item(id, |id| visits[id].total);
                } else {
                    visits[id].open = true;
                    heap.push(id);
                    heap.rise_last(|id| visits[id].total);
                }
            }
        }
    }
    false
}

/// `0x7FF78A611E80`: the cost to `next` from the node at `at` and the
/// estimate from there to `goal`.
fn step_cost(visits: &[Visit], at: usize, next: Vec3, goal: Vec3) -> (f32, f32) {
    let here = visits[at].point;
    let mut cost = visits[at].cost;
    let from = match visits[at].parent {
        Some(parent) => std::array::from_fn(|axis| (visits[parent].point[axis] + here[axis]) * 0.5),
        None => here,
    };
    let back = sub(here, from);
    cost += client_length((back[1] * back[1] + back[0] * back[0]) + back[2] * back[2]);
    let middle: Vec3 = std::array::from_fn(|axis| (here[axis] + next[axis]) * 0.5);
    let ahead = sub(middle, here);
    let (way, way_length) = normalise_yx(sub(goal, next));
    let (link, link_length) = normalise_yx(ahead);
    let mut estimate = 0.0;
    if link_length > 0.0 && way_length > 0.0 {
        cost += link_length;
        let cosine = (way[1] * link[1] + way[0] * link[0]) + way[2] * link[2];
        estimate += ((1.0 - cosine) * 0.5) * 1.0;
    }
    let rest = sub(goal, middle);
    let rest = client_length((rest[1] * rest[1] + rest[0] * rest[0]) + rest[2] * rest[2]);
    (cost, rest * 1.1 + estimate)
}

/// The node of `level` holding `point` after its lift (`0x7FF78A623800`
/// with the config `{3, 3, 8}`) and the lifted point.
fn locate(cells: &AiCells, level: Level, point: Vec3) -> Option<(Vec3, Place)> {
    let (point, voxel) = lift(cells, point, SEARCH_SEARCH);
    let voxel = voxel?;
    let hit = cells.find(Graph::Air, voxel)?;
    let node = cells.node(Graph::Air, voxel)?;
    let place = match level {
        Level::Units => Place {
            id: node.client_id(),
            word: u64::from(cells.entry_word(Graph::Air, node)),
            node: Some(node),
        },
        Level::Regions => {
            let block = cells.block(voxel.key())?;
            let region = block.u64(super::AIR.bricks, hit.brick as usize)? & 0xFF;
            region_place(cells, voxel.key(), region as u32)?
        }
    };
    Some((point, place))
}

fn region_place(cells: &AiCells, key: u32, region: u32) -> Option<Place> {
    let block = cells.block(key)?;
    let word = block.u64(9, region as usize)?;
    Some(Place {
        id: 1 | u64::from(region) << 12 | u64::from(key) << 32,
        word,
        node: None,
    })
}

/// The nodes a node of `level` links to, with each link's direction, in the
/// client's order.
fn neighbours(cells: &AiCells, level: Level, place: Place, out: &mut Vec<(Place, u8)>) {
    out.clear();
    match level {
        Level::Units => {
            let mut linked = Vec::new();
            cells.linked(Graph::Air, place.node.expect("a unit's node"), &mut linked);
            out.extend(linked.into_iter().map(|(node, direction)| {
                (
                    Place {
                        id: node.client_id(),
                        word: u64::from(cells.entry_word(Graph::Air, node)),
                        node: Some(node),
                    },
                    direction,
                )
            }));
        }
        Level::Regions => region_links(cells, place, out),
    }
}

/// `0x7FF78A61BE70`: a region's outgoing link words lead to the next cell
/// in each direction, to each of its regions whose incoming words have the
/// same running number with the opposite direction.
fn region_links(cells: &AiCells, place: Place, out: &mut Vec<(Place, u8)>) {
    let key = (place.id >> 32) as u32;
    let Some(block) = cells.block(key) else {
        return;
    };
    let links = super::AIR.links;
    let at = ((place.word >> 22) & 0xFFFF) as usize;
    let Some(header) = block.u32(links, at) else {
        return;
    };
    let (skip, outgoing) = ((header & 0x7FF) as usize, ((header >> 11) & 0x7FF) as usize);
    let mut number = 0;
    for word in 0..outgoing {
        let Some(word) = block.u32(links, at + 1 + skip + word) else {
            return;
        };
        number += word >> 26;
        for direction in super::bits(word) {
            let [dx, dy, dz] = DIRECTIONS[direction];
            let next = ((key & 0xFFF) as i32 + dx) as u32 & 0xFFF
                | ((((key >> 12) & 0xFFF) as i32 + dz) as u32 & 0xFFF) << 12
                | (((key >> 24) as i32 + dy) as u32 & 0xFF) << 24;
            let Some(other) = cells.block(next) else {
                continue;
            };
            let count = other.word::<4>(156).map_or(0, u32::from_le_bytes) as usize;
            let back = OPPOSITE[direction];
            for region in 0..count {
                let Some(theirs) = other.u64(9, region) else {
                    break;
                };
                if incoming(other, ((theirs >> 22) & 0xFFFF) as usize, back, number) {
                    out.push((
                        Place {
                            id: 1 | (region as u64) << 12 | u64::from(next) << 32,
                            word: theirs,
                            node: None,
                        },
                        direction as u8,
                    ));
                }
            }
        }
    }
}

/// `0x7FF78A5FDE30`: whether a links record's incoming words reach `number`
/// with direction `back`.
fn incoming(block: &super::Block, at: usize, back: usize, number: u32) -> bool {
    let links = super::AIR.links;
    let Some(header) = block.u32(links, at) else {
        return false;
    };
    let first = at + 1 + (header & 0x7FF) as usize + ((header >> 11) & 0x7FF) as usize;
    let mut running = 0;
    (0..(header >> 22) as usize).any(|word| {
        let word = block.u32(links, first + word).unwrap_or(0);
        running += word >> 26;
        running == number && word & (1 << back) & 0x3FF_FFFF != 0
    })
}

/// A reach node's point: a unit's (`0x7FF78A619DF0`: its corner plus the
/// voxel its mask names in section 16, x and z at the voxel's middle, y 4 mm
/// above its floor), a region's that of its first unit's first entry
/// (`0x7FF78A613FE0`), else the region unit's corner (`0x7FF78A61A640`).
fn place_point(cells: &AiCells, level: Level, place: Place) -> Vec3 {
    match level {
        Level::Units => {
            let node = place.node.expect("a unit's node");
            unit_point(cells, node.key(), node.unit, place.word as u32)
        }
        Level::Regions => region_point(cells, place),
    }
}

fn unit_point(cells: &AiCells, key: u32, unit: [i32; 3], entry: u32) -> Vec3 {
    let origin = cell_origin(key);
    let (width, height) = (super::VOXEL, super::VOXEL_HEIGHT);
    let corner = [
        ((unit[0] & 15) * 4) as f32 * width + origin[0],
        ((unit[1] & 15) * 4) as f32 * height + origin[1],
        ((unit[2] & 15) * 4) as f32 * width + origin[2],
    ];
    let byte = cells
        .block(key)
        .and_then(|block| block.u8(16, (entry & 0xFFF) as usize))
        .unwrap_or(0);
    [
        (f32::from(byte & 3) * width + width * 0.5) + corner[0],
        (f32::from((byte >> 4) & 3) * height + 0.004) + corner[1],
        (f32::from((byte >> 2) & 3) * width + width * 0.5) + corner[2],
    ]
}

fn region_point(cells: &AiCells, place: Place) -> Vec3 {
    let key = (place.id >> 32) as u32;
    let region = (place.id >> 12) & 0xF_FFFF;
    let word = place.word;
    let unit = [(word >> 10) & 15, (word >> 14) & 15, (word >> 18) & 15].map(|value| value as i32);
    if let Some(block) = cells.block(key) {
        let layout = &super::AIR;
        let mut entry = None;
        block.unit_entries(layout, super::unit_voxel(unit), |hit, word| {
            let brick = block.u64(layout.bricks, hit.brick as usize).unwrap_or(0);
            if brick & 0xFF == region {
                entry = Some(word);
            }
            entry.is_some()
        });
        if let Some(entry) = entry {
            return unit_point(cells, key, unit, entry);
        }
    }
    let origin = cell_origin(key);
    [
        ((word >> 8) & 0x3C) as f32 * super::VOXEL + origin[0],
        ((word >> 12) & 0x3C) as f32 * super::VOXEL_HEIGHT + origin[1],
        ((word >> 16) & 0x3C) as f32 * super::VOXEL + origin[2],
    ]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

/// The correctly rounded `rsqrtss` estimate.
fn estimate(value: f32) -> f32 {
    if value == 0.0 {
        f32::INFINITY
    } else {
        (1.0 / f64::from(value).sqrt()) as f32
    }
}

/// A length from its square as the client's scalar code takes it: none
/// below `1.18e-36`, else `x × ((3 − x r²) × r / 2)`.
pub(crate) fn client_length(squared: f32) -> f32 {
    if squared > TINY_SQUARED {
        let r = estimate(squared);
        squared * ((3.0 - squared * (r * r)) * (r * 0.5))
    } else {
        0.0
    }
}

/// `0x7FF78A611E80`'s normalisation (sum `(y² + x²) + z²`): the unit
/// vector and the length, −1 below `1.18e-36`.
fn normalise_yx(vector: Vec3) -> (Vec3, f32) {
    let squared = (vector[1] * vector[1] + vector[0] * vector[0]) + vector[2] * vector[2];
    if squared < TINY_SQUARED {
        return (vector, -1.0);
    }
    let r = estimate(squared);
    let factor = (3.0 - (r * r) * squared) * (r * 0.5);
    (vector.map(|axis| axis * factor), factor * squared)
}

/// The client's binary heaps of open nodes keyed by a float (`0x7FF78A5FE940`
/// push, `0x7FF78A5FE900` pop, `0x7FF78A5FE870` decrease; the start
/// search's `0x7FF78A61CFC0`, `0x7FF78A61DB40`, `0x7FF78A61CF00` are the
/// same): a new or decreased item rises past larger keys (an equal key stops
/// it); the pop sinks the hole to a leaf along the smaller child (the left
/// on a tie) and the last item rises from there.
#[derive(Default)]
struct Heap {
    items: Vec<usize>,
}

impl Heap {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn push(&mut self, item: usize) {
        self.items.push(item);
    }

    fn rise_last(&mut self, key: impl Fn(usize) -> f32) {
        let at = self.items.len() - 1;
        let item = self.items[at];
        self.rise(at, item, &key);
    }

    fn rise_item(&mut self, item: usize, key: impl Fn(usize) -> f32) {
        if let Some(at) = self.items.iter().position(|&other| other == item) {
            self.rise(at, item, &key);
        }
    }

    fn rise(&mut self, mut at: usize, item: usize, key: &impl Fn(usize) -> f32) {
        let value = key(item);
        while at > 0 {
            let parent = (at - 1) / 2;
            if value >= key(self.items[parent]) {
                break;
            }
            self.items[at] = self.items[parent];
            at = parent;
        }
        self.items[at] = item;
    }

    fn pop(&mut self, key: impl Fn(usize) -> f32) -> Option<usize> {
        let top = *self.items.first()?;
        let last = self.items.pop().expect("not empty");
        let count = self.items.len();
        if count == 0 {
            return Some(top);
        }
        let mut hole = 0;
        let mut child = 1;
        while child < count {
            if child + 1 < count && key(self.items[child + 1]) < key(self.items[child]) {
                child += 1;
            }
            self.items[hole] = self.items[child];
            hole = child;
            child = 2 * child + 1;
        }
        self.rise(hole, last, &key);
        Some(top)
    }
}
