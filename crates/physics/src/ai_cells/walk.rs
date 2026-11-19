//! A walk over the client's walk graph ([`Graph::Walk`]): an A* over its
//! nodes (1 m units, each a connected part of the walkable voxels in it)
//! along the client's links, then the waypoints: the turns of the walk,
//! straight lines the walk graph shows clear and the caller's walk test
//! passes.
//!
//! The client's own query (`0x7FF78A626BC0` in mode 0) searches the same
//! graph voxel by voxel, but only in a window of 5 × 7 × 5 units around the
//! start (`0x7FF789B51810(query, 2, 3, 2)`): it is a short-range query. The
//! server's walks cross whole maps, so this search runs over the units
//! (assumption: retail's server-side AI search is unknown). A step costs
//! the distance between the units' points (the voxel nearest the unit's
//! middle), plus [`EDGE_COST`] when it leaves a unit with fewer than eight
//! horizontal neighbours, so walks keep a unit from walls and drops where
//! there is room (the same assumption as `nav`).

use std::{cmp::Ordering, collections::BinaryHeap};

use super::{AiCells, Graph, Node, VOXEL, VOXEL_HEIGHT, Vec3, Voxel};
use crate::nav::{Checks, EDGE_COST, FastMap, FastSet, SETUP_WORK, SNAP, spend};

/// What a node's expansion costs in work (the units of `nav::PathSearch`,
/// about a microsecond of a release build): 2 µs (measured 2026-11-19 over
/// 61 Rialto walks between walk voxels on a 16 m lattice, release build).
const EXPAND_WORK: u32 = 2;
/// What a straight line's test on the walk graph costs a quarter metre
/// (up to five voxel lookups of about 0.1 µs; assumption from the lookups'
/// cost, not measured alone).
const LINE_WORK_PER_SAMPLE: f32 = 0.5;

/// How many nodes a search expands before it gives up (an unreachable
/// goal on a big map; assumption: far more than any walk the server makes).
const EXPANSIONS: usize = 200_000;
/// How far up and down from a point's voxel its walk voxel is looked for, in
/// voxels: a point on the floor is in the voxel under the walk voxel or in
/// it.
const COLUMN: [i32; 8] = [0, 1, -1, 2, -2, 3, -3, 4];
/// How much a straight line's floor may rise or fall from one sample to the
/// next (a quarter metre apart), in voxels.
const LINE_STEP: i32 = 2;

/// A walk search run a bounded amount of work at a time, as
/// `nav::PathSearch`: the walk is the same however the work is split.
#[derive(Clone, Debug)]
pub struct WalkSearch {
    from: Vec3,
    to: Vec3,
    state: Option<State>,
}

#[derive(Clone, Debug)]
struct State {
    start: Node,
    goal: Node,
    /// The walk's first and last points.
    ends: [Vec3; 2],
    open: BinaryHeap<Open>,
    /// The best cost found to a node and the node it came from.
    best: FastMap<Node, (f32, Node)>,
    closed: FastSet<Node>,
    points: FastMap<Node, Vec3>,
    pushed: u32,
    scratch: Vec<Node>,
    answers: FastMap<[u32; 6], bool>,
    /// Once the A* is done: the walk's points, the waypoints so far, the
    /// point they reached and how far a line from there reaches so far.
    turns: Option<(Vec<Vec3>, Vec<Vec3>, usize, usize)>,
}

#[derive(Clone, Debug)]
struct Open {
    estimate: f32,
    order: u32,
    node: Node,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Open {}

impl Ord for Open {
    /// The lowest estimate first, then the first pushed.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then(other.order.cmp(&self.order))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl WalkSearch {
    pub fn new(from: Vec3, to: Vec3) -> Self {
        Self {
            from,
            to,
            state: None,
        }
    }

    /// Search on over `cells` (always the same) while `work` is left: `None`
    /// while the search goes on, then the walk's points once (`None` when
    /// either point is more than [`SNAP`] from the walk graph or nothing
    /// links them). The first point is `from` on its floor when it stands on
    /// the walk graph, the nearest node's point otherwise; the last likewise
    /// for `to`. `walkable` is asked about each straight line that would
    /// replace units, once per pair and at most `nav::CHECKS` times.
    pub fn advance(
        &mut self,
        cells: &AiCells,
        work: &mut u32,
        walkable: impl FnMut(Vec3, Vec3) -> bool,
    ) -> Option<Option<Vec<Vec3>>> {
        if self.state.is_none() {
            if *work == 0 {
                return None;
            }
            spend(work, SETUP_WORK);
            let (Some((start, from)), Some((goal, to))) =
                (locate(cells, self.from), locate(cells, self.to))
            else {
                return Some(None);
            };
            let mut state = State {
                start,
                goal,
                ends: [from, to],
                open: BinaryHeap::new(),
                best: FastMap::default(),
                closed: FastSet::default(),
                points: FastMap::default(),
                pushed: 0,
                scratch: Vec::new(),
                answers: FastMap::default(),
                turns: None,
            };
            state.best.insert(start, (0.0, start));
            let estimate = distance(state.point(cells, start), to);
            state.push(estimate, start);
            self.state = Some(state);
        }
        let state = self.state.as_mut().expect("begun above");
        if state.turns.is_none() {
            let nodes = match state.search(cells, work)? {
                Some(nodes) => nodes,
                None => return Some(None),
            };
            let mut points: Vec<Vec3> =
                nodes.iter().map(|&node| state.point(cells, node)).collect();
            let last = points.len() - 1;
            points[0] = state.ends[0];
            if last > 0 {
                points[last] = state.ends[1];
            } else {
                points.push(state.ends[1]);
            }
            let first = points[0];
            state.turns = Some((points, vec![first], 0, 1));
        }
        let (points, waypoints, anchor, reach) = state.turns.as_mut().expect("set above");
        let mut checks = Checks {
            test: walkable,
            answers: &mut state.answers,
            work,
        };
        turns(cells, points, waypoints, anchor, reach, &mut checks)?;
        Some(Some(std::mem::take(waypoints)))
    }
}

impl State {
    /// A node's point, the floor of its voxel nearest the unit's middle.
    fn point(&mut self, cells: &AiCells, node: Node) -> Vec3 {
        *self
            .points
            .entry(node)
            .or_insert_with(|| middle(cells, node).floor())
    }

    fn push(&mut self, estimate: f32, node: Node) {
        self.open.push(Open {
            estimate,
            order: self.pushed,
            node,
        });
        self.pushed += 1;
    }

    /// The A*: the walk's nodes from the start to the goal, `Some(None)` when
    /// there is none, `None` when the work ran out.
    fn search(&mut self, cells: &AiCells, work: &mut u32) -> Option<Option<Vec<Node>>> {
        let goal_point = self.ends[1];
        loop {
            if *work == 0 {
                return None;
            }
            let Some(Open { node, .. }) = self.open.pop() else {
                return Some(None);
            };
            if !self.closed.insert(node) {
                continue;
            }
            if node == self.goal {
                let mut nodes = vec![node];
                while nodes[nodes.len() - 1] != self.start {
                    nodes.push(self.best[&nodes[nodes.len() - 1]].1);
                }
                nodes.reverse();
                return Some(Some(nodes));
            }
            if self.closed.len() > EXPANSIONS {
                return Some(None);
            }
            spend(work, EXPAND_WORK);
            let mut next = std::mem::take(&mut self.scratch);
            cells.neighbours(Graph::Walk, node, &mut next);
            let mut sides = 0u16;
            for other in &next {
                let [dx, _, dz] = [0, 1, 2].map(|axis| other.unit[axis] - node.unit[axis]);
                if dx != 0 || dz != 0 {
                    sides |= 1 << ((dx + 1) * 3 + dz + 1);
                }
            }
            let edge = if sides.count_ones() < 8 {
                EDGE_COST
            } else {
                0.0
            };
            let here = self.point(cells, node);
            let cost = self.best[&node].0;
            for &other in &next {
                if self.closed.contains(&other) {
                    continue;
                }
                let there = self.point(cells, other);
                let reached = cost + distance(here, there) + edge;
                if self
                    .best
                    .get(&other)
                    .is_some_and(|&(best, _)| best <= reached)
                {
                    continue;
                }
                self.best.insert(other, (reached, node));
                self.push(reached + distance(there, goal_point), other);
            }
            self.scratch = next;
        }
    }
}

/// The waypoints of a walk through `points`, from `anchor` (with its line
/// reaching `reach` so far) on into `waypoints`: from each waypoint the
/// farthest later point a straight line reaches (clear on the walk graph and
/// passed by the walk test), else the next point. `None` when the work ran
/// out: the search goes on from there.
fn turns(
    cells: &AiCells,
    points: &[Vec3],
    waypoints: &mut Vec<Vec3>,
    anchor: &mut usize,
    reach: &mut usize,
    checks: &mut Checks<'_, impl FnMut(Vec3, Vec3) -> bool>,
) -> Option<()> {
    let last = points.len() - 1;
    while *anchor < last {
        let at = waypoints[waypoints.len() - 1];
        while *reach < last {
            let (clear, cost) = clear(cells, at, points[*reach + 1]);
            // The line's own cost after the walk test, so a call with work
            // left always gets its answer.
            let passes = clear && checks.ask(at, points[*reach + 1])?;
            spend(checks.work, cost);
            if !passes {
                break;
            }
            *reach += 1;
        }
        waypoints.push(points[*reach]);
        *anchor = *reach;
        *reach = *anchor + 1;
    }
    Some(())
}

/// Whether the walk graph has floor all along the straight line from `a`
/// to `b`, and the test's work: a walk voxel every quarter metre, each within [`LINE_STEP`]
/// voxels of the one before, the last within as much of `b`'s height.
fn clear(cells: &AiCells, a: Vec3, b: Vec3) -> (bool, u32) {
    let samples = ((a[0] - b[0]).hypot(a[2] - b[2]) / VOXEL).ceil().max(1.0) as usize;
    let cost = (samples as f32 * LINE_WORK_PER_SAMPLE).ceil() as u32;
    let mut y = Voxel::of(lifted(a)).y;
    for sample in 1..=samples {
        let t = sample as f32 / samples as f32;
        let column = Voxel::of([a[0] + (b[0] - a[0]) * t, 0.0, a[2] + (b[2] - a[2]) * t]);
        let Some(found) = (0..=LINE_STEP)
            .flat_map(|dy| [dy, -dy])
            .map(|dy| Voxel {
                y: y + dy,
                ..column
            })
            .find(|&voxel| cells.find(Graph::Walk, voxel).is_some())
        else {
            return (false, cost);
        };
        y = found.y;
    }
    ((y - Voxel::of(lifted(b)).y).abs() <= LINE_STEP, cost)
}

/// A floor point raised half a voxel, into the walk voxel above the floor.
fn lifted([x, y, z]: Vec3) -> Vec3 {
    [x, y + VOXEL_HEIGHT * 0.5, z]
}

/// The walk node under a point and the walk's end there: the point on its
/// floor when its own column has a walk voxel near its height, else the
/// nearest node's point within [`SNAP`].
fn locate(cells: &AiCells, point: Vec3) -> Option<(Node, Vec3)> {
    let in_column = |voxel: Voxel| {
        COLUMN.into_iter().find_map(|dy| {
            let voxel = Voxel {
                y: voxel.y + dy,
                ..voxel
            };
            cells.node(Graph::Walk, voxel).map(|node| (node, voxel))
        })
    };
    let at = Voxel::of(lifted(point));
    if let Some((node, voxel)) = in_column(at) {
        return Some((node, [point[0], voxel.floor()[1], point[2]]));
    }
    let reach = (SNAP / VOXEL) as i32;
    let mut best: Option<(i32, Node, Voxel)> = None;
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let far = dx * dx + dz * dz;
            if far > reach * reach || best.is_some_and(|(near, ..)| near <= far) {
                continue;
            }
            if let Some((node, voxel)) = in_column(Voxel {
                x: at.x + dx,
                z: at.z + dz,
                ..at
            }) {
                best = Some((far, node, voxel));
            }
        }
    }
    best.map(|(_, node, voxel)| (node, voxel.floor()))
}

/// The voxel of a node nearest its unit's middle (lowest first).
fn middle(cells: &AiCells, node: Node) -> Voxel {
    let mask = cells.mask(Graph::Walk, node);
    let off = |value: u32| (2 * value as i32 - 3).abs();
    let sub = (0..64)
        .filter(|&sub| mask & 1 << sub != 0)
        .min_by_key(|&sub| (off(sub >> 4 & 3) + off(sub >> 2 & 3), sub & 3))
        .unwrap_or(0);
    node.voxel(sub)
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}
