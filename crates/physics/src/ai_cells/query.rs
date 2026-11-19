//! The client's path query (`0x7FF78A626BC0`) after its start: an A* voxel
//! by voxel in a window of units around the start, over one graph (mode 0
//! the walk graph, mode 1 the air, which Widowmaker's ledge pull and the
//! mover behaviour beside it use), as the client runs it, ties and all.
//!
//! What the client does before (mode 1's two searches that refuse a far or
//! cut-off goal, `0x7FF78A616DC0` and `0x7FF78A616A70`; mode 0's lift of
//! both points into the walk graph) is not here: the caller gives the start
//! and goal nodes and voxels.
//!
//! - The window: `0x7FF789B51810(query, 2, 3, 2)`, 5 × 7 × 5 units (x, y,
//!   z) centred on the start's, four entries a unit (`0x7FF78A627200` finds
//!   the unit beside, by a direction code `9 dx + 3 dz + dy + 13`). The
//!   goal's unit must be in it (`0x7FF78A626A00`).
//! - A window entry (1104 bytes, `0x7FF78A626750`) holds its node, entry
//!   and voxel mask, a record per voxel reached (`0x7FF78A628B80`: open and
//!   closed bits, the voxel, its parent record, its own index, the parent's
//!   unit and entry, g + h, g), open and closed masks and the search it
//!   belongs to; it is cleared when a search first uses it
//!   (`0x7FF78A628D30`).
//! - The open list: a binary heap of `{g + h, unit and entry, record}`, at
//!   most 2048 entries (`0x7FF789B51810` → `0x7FF78A626B60`); a push moves
//!   up past larger costs (`0x7FF78A6287F0`), a better cost of an open
//!   voxel moves its entry up (`0x7FF78A628590`), the pop sinks the hole to
//!   a leaf (the smaller child, the left on a tie, `0x7FF78A628E80`) and
//!   moves the last entry up from there (`0x7FF78A626790`).
//! - A voxel's neighbours (`0x7FF78A627070`): in its unit the voxels a
//!   horizontal step away (8 directions) and a layer up, down or level, in
//!   its mask; with the unit's vertical flag (set for mode 1's start, and
//!   copied to every unit reached from it) also straight up and down; then
//!   the same steps that leave the unit. A step into another unit fills the
//!   window's entries there from the current node's links in that
//!   direction (`0x7FF78A626020` air, `0x7FF78A6263A0` walk, in the order
//!   of the client's neighbour walk; a linked node the grid doesn't have
//!   ends the step); each of the four entries there whose mask has the
//!   voxel is a neighbour.
//! - Costs (`0x7FF78A626F30`): 1000, 1414 or 1732 by the axes the step
//!   changes; the estimate (`0x7FF78A628310`): 1000 × the voxels between,
//!   each unit between counting 4.
//! - The path (`0x7FF78A626830`): the records back from the goal's, each
//!   `{voxel, node, direction}` (the direction of the step from the one
//!   before, `0x7FF78A6275B0` and its table `0x7FF78BD2F880`; 26 for the
//!   first).

use super::{AiCells, Graph, Node};

/// The window's half sides in units (x, y, z): `0x7FF789B51810`'s
/// arguments as both callers give them.
const HALF: [i32; 3] = [2, 3, 2];
const SIDES: [usize; 3] = [5, 7, 5];
const SLOTS: usize = SIDES[0] * SIDES[1] * SIDES[2];
/// The open list's room (`0x7FF78A626B60(query + 8, 2048)`).
const HEAP: usize = 2048;
/// No record: the start's parent, and what a record update that changes
/// nothing returns.
const NONE: u32 = 31;
const UNCHANGED: u32 = 64;
/// `0x7FF78BD2F880`: the direction of a voxel step by `9 dx + 3 dz + dy +
/// 13`.
const STEP_DIRECTION: [u8; 27] = [
    24, 15, 19, 23, 14, 18, 22, 13, 17, 25, 16, 20, 21, 26, 4, 9, 0, 5, 12, 3, 8, 11, 2, 7, 10, 1,
    6,
];

/// One end of a query: a node and the voxel in its unit (`y | z << 2 | x <<
/// 4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct End {
    pub node: Node,
    pub sub: u8,
}

/// A step of the client's path: the voxel, its node and the direction from
/// the voxel before (26 for the first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub sub: u8,
    pub node: Node,
    pub direction: u8,
}

#[derive(Clone, Copy, Default)]
struct Record {
    /// Bit 0 open, 1 closed, 2–7 the voxel, 8–13 the parent record, 14–19
    /// its own index, 20–30 the parent's unit.
    word: u32,
    /// The parent's entry in its unit.
    parent_layer: u32,
    total: u32,
    cost: u32,
}

#[derive(Clone)]
struct Entry {
    node: Option<Node>,
    word: u32,
    mask: u64,
    empty: bool,
    vertical: bool,
    search: u64,
    used: u32,
    open: u64,
    closed: u64,
    records: [Record; 64],
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            node: None,
            word: 0,
            mask: 0,
            empty: true,
            vertical: false,
            search: u64::MAX,
            used: 0,
            open: 0,
            closed: 0,
            records: [Record::default(); 64],
        }
    }
}

#[derive(Clone, Copy)]
struct Open {
    total: u32,
    slot: u16,
    layer: u8,
    record: u8,
}

struct Query<'a> {
    cells: &'a AiCells,
    graph: Graph,
    window: Vec<[Entry; 4]>,
    heap: Vec<Open>,
    goal: End,
    goal_word: u32,
    search: u64,
}

/// The client's path from `start` to `goal` over `graph`: `None` when the
/// goal's unit is outside the window (the client's −2) or the open list
/// empties first (−1).
pub fn client_path(cells: &AiCells, graph: Graph, start: End, goal: End) -> Option<Vec<Step>> {
    let offset: [i32; 3] = std::array::from_fn(|axis| goal.node.unit[axis] - start.node.unit[axis]);
    if (0..3).any(|axis| offset[axis].abs() > HALF[axis]) {
        return None;
    }
    let mut query = Query {
        cells,
        graph,
        window: vec![Default::default(); SLOTS],
        heap: Vec::new(),
        goal,
        goal_word: cells.entry_word(graph, goal.node),
        search: 1,
    };
    let centre = slot(HALF);
    let mask = cells.mask(graph, start.node);
    query.window[centre][0] = Entry {
        node: Some(start.node),
        word: cells.entry_word(graph, start.node),
        mask,
        empty: mask == 0,
        vertical: graph == Graph::Air,
        ..Default::default()
    };
    let estimate = query.estimate(start.node, start.sub);
    query.push(centre, 0, start.sub, None, 0x7FF, 4, estimate, 0);
    loop {
        let open = query.pop()?;
        let (slot, layer) = (open.slot as usize, open.layer as usize);
        let entry = &mut query.window[slot][layer];
        let record = &mut entry.records[open.record as usize];
        let sub = (record.word >> 2 & 63) as u8;
        if entry.node == Some(goal.node) && entry.word == query.goal_word && sub == goal.sub {
            return Some(query.path(open));
        }
        record.word = record.word & !1 | 2;
        entry.closed |= 1 << sub;
        entry.open &= !(1 << sub);
        let parent = *record;
        let steps = neighbours(entry, sub);
        query.relax(slot, layer, parent, &steps);
        if query.heap.is_empty() {
            return None;
        }
    }
}

/// The window slot of a unit offset from the window's corner.
fn slot([x, y, z]: [i32; 3]) -> usize {
    ((x as usize * SIDES[2]) + z as usize) * SIDES[1] + y as usize
}

/// The window slot beside `slot` in direction `code` (`0x7FF78A627200`),
/// `None` outside the window.
fn beside(slot: usize, code: u8) -> Option<usize> {
    let (sy, sz) = (SIDES[1], SIDES[2]);
    let code = i32::from(code);
    let at = [
        (slot / (sz * sy)) as i32 + code / 9 % 3 - 1,
        (slot % sy) as i32 + code % 3 - 1,
        (slot % (sz * sy) / sy) as i32 + code / 3 % 3 - 1,
    ];
    (0..3)
        .all(|axis| (0..SIDES[axis] as i32).contains(&at[axis]))
        .then(|| self::slot(at))
}

/// A voxel's coordinates in its unit (x, y, z).
fn coordinates(sub: u8) -> [i32; 3] {
    [
        i32::from(sub >> 4),
        i32::from(sub & 3),
        i32::from(sub >> 2 & 3),
    ]
}

/// The steps between two voxels per axis, a step out of the unit (3 apart)
/// counting one the other way.
fn deltas(from: u8, to: u8) -> [i32; 3] {
    let (a, b) = (coordinates(from), coordinates(to));
    std::array::from_fn(|axis| match b[axis] - a[axis] {
        -3 => 1,
        3 => -1,
        delta => delta,
    })
}

/// The direction code of a step that leaves the unit (`0x7FF78A6276B0`):
/// per axis the unit step, `9 dx + 3 dz + dy + 13`.
fn code(from: u8, to: u8) -> u8 {
    let (a, b) = (coordinates(from), coordinates(to));
    let unit = |axis: usize| match b[axis] - a[axis] {
        -3 => 1,
        3 => -1,
        _ => 0,
    };
    (9 * unit(0) + 3 * unit(2) + unit(1) + 13) as u8
}

/// A voxel's neighbours as `(direction code, voxel)` (`0x7FF78A627070`):
/// those in its unit and its mask (code 13), then those beyond the unit.
fn neighbours(entry: &Entry, sub: u8) -> Vec<(u8, u8)> {
    let [x, y, z] = coordinates(sub);
    let mut inside = 0u64;
    let mut outside = 0u64;
    for dx in -1..=1 {
        for dz in -1..=1 {
            for dy in -1..=1 {
                let horizontal = dx != 0 || dz != 0;
                if !horizontal && !(entry.vertical && dy != 0) {
                    continue;
                }
                let at = [x + dx, y + dy, z + dz];
                let bit = 1u64 << (at[1] & 3 | (at[2] & 3) << 2 | (at[0] & 3) << 4);
                if at.iter().all(|value| (0..4).contains(value)) {
                    inside |= bit;
                } else {
                    outside |= bit;
                }
            }
        }
    }
    let mut steps = Vec::new();
    let mut take = |mut bits: u64, code: &dyn Fn(u8) -> u8| {
        while bits != 0 {
            let next = bits.trailing_zeros() as u8;
            bits &= bits - 1;
            steps.push((code(next), next));
        }
    };
    take(inside & entry.mask, &|_| 13);
    take(outside, &|next| code(sub, next));
    steps
}

impl Query<'_> {
    /// `0x7FF78A628310`: 1000 × the voxels from `sub` of `node` to the
    /// goal's, a unit between counting 4.
    fn estimate(&self, node: Node, sub: u8) -> u32 {
        let (here, there) = (coordinates(sub), coordinates(self.goal.sub));
        let voxels: i32 = (0..3)
            .map(|axis| {
                let units = self.goal.node.unit[axis] - node.unit[axis];
                let voxels = there[axis] - here[axis];
                if node == self.goal.node || units == 0 {
                    voxels.abs()
                } else {
                    4 * units.abs() + voxels * units.signum()
                }
            })
            .sum();
        1000 * voxels as u32
    }

    /// `0x7FF78A628D30` with `0x7FF78A628B80`: reach `sub` of the entry at
    /// `slot`, `layer` from `parent` (a record of the entry at
    /// `parent_slot`, `parent_layer`).
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        slot: usize,
        layer: usize,
        sub: u8,
        parent: Option<Record>,
        parent_slot: u32,
        parent_layer: u32,
        estimate: u32,
        cost: u32,
    ) {
        let entry = &mut self.window[slot][layer];
        if entry.search != self.search {
            entry.search = self.search;
            entry.used = 0;
            entry.records = [Record::default(); 64];
            entry.closed = 0;
            entry.open = 0;
        }
        let bit = 1u64 << sub;
        let was_open = entry.open & bit != 0;
        let parent_index = parent.map_or(NONE, |parent| parent.word >> 14 & 63);
        let total = estimate.wrapping_add(cost);
        let index = if was_open {
            let found = (0..64).find(|&index| {
                let word = entry.records[index].word;
                word & 1 != 0 && (word & 0xFF) >> 2 == u32::from(sub)
            });
            match found {
                Some(index) if entry.records[index].total > total => {
                    let record = &mut entry.records[index];
                    record.word &= 0x800F_FFFF;
                    record.parent_layer &= !3;
                    record.total = total;
                    record.word |= (parent_slot & 0x7FF) << 20;
                    record.cost = cost;
                    record.parent_layer |= parent_layer & 3;
                    record.word = record.word & 0xFFFF_C0FF | parent_index << 8;
                    index as u32
                }
                _ => UNCHANGED,
            }
        } else {
            let index = entry.used;
            let record = &mut entry.records[index as usize];
            record.parent_layer &= !3;
            record.word = record.word & 0x800F_FF03
                | (u32::from(sub) & 63 | (parent_slot & 0x7FF) << 18) << 2
                | 1;
            record.parent_layer |= parent_layer & 3;
            record.word = parent_index << 8 | record.word & 0xFFFF_C0FF;
            record.cost = cost;
            record.total = total;
            record.word = record.word & !0xFC000 | (index << 14 & 0xFC000);
            entry.used += 1;
            entry.open |= bit;
            index
        };
        if index == UNCHANGED {
            return;
        }
        let open = Open {
            total,
            slot: slot as u16,
            layer: layer as u8,
            record: index as u8,
        };
        if was_open {
            // `0x7FF78A628590`: the first entry of that record moves up.
            if let Some(at) = self.heap.iter().position(|other| {
                other.slot == open.slot && other.layer == open.layer && other.record == open.record
            }) {
                self.rise(at, open);
            }
        } else if self.heap.len() < HEAP {
            self.heap.push(open);
            self.rise(self.heap.len() - 1, open);
        }
    }

    /// Move `open` up from `at` past the larger costs and put it there.
    fn rise(&mut self, mut at: usize, open: Open) {
        while at > 0 {
            let parent = (at - 1) / 2;
            if self.heap[parent].total <= open.total {
                break;
            }
            self.heap[at] = self.heap[parent];
            at = parent;
        }
        self.heap[at] = open;
    }

    /// The lowest entry (`0x7FF78A626BC0`'s pop with `0x7FF78A628E80` and
    /// `0x7FF78A626790`).
    fn pop(&mut self) -> Option<Open> {
        let top = *self.heap.first()?;
        let last = self.heap.pop().expect("not empty");
        if self.heap.is_empty() {
            return Some(top);
        }
        let count = self.heap.len();
        let mut hole = 0;
        let mut child = 1;
        while child < count {
            if child + 1 < count && self.heap[child].total > self.heap[child + 1].total {
                child += 1;
            }
            self.heap[hole] = self.heap[child];
            hole = child;
            child = 2 * child + 1;
        }
        self.rise(hole, last);
        Some(top)
    }

    /// `0x7FF78A6288E0`: reach each neighbour of the voxel `parent` holds,
    /// in the entry at `slot`, `layer`.
    fn relax(&mut self, slot: usize, layer: usize, parent: Record, steps: &[(u8, u8)]) {
        let from = (parent.word >> 2 & 63) as u8;
        for &(code, sub) in steps {
            let Some(target) = beside(slot, code) else {
                continue;
            };
            if !self.fill(slot, layer, code, target) {
                continue;
            }
            for other in 0..4 {
                let entry = &self.window[target][other];
                let reachable = entry.search == self.search
                    && entry.closed & 1 << sub == 0
                    && (entry.vertical || !entry.empty)
                    && entry.mask & 1 << sub != 0;
                if !reachable {
                    continue;
                }
                let node = entry.node.expect("filled");
                let deltas = deltas(from, sub);
                let (estimate, cost) = if deltas.iter().all(|delta| delta.abs() <= 1) {
                    let step = match deltas.iter().map(|delta| delta.abs()).sum::<i32>() {
                        1 => 1000,
                        2 => 1414,
                        _ => 1732,
                    };
                    (self.estimate(node, sub), parent.cost + step)
                } else {
                    (u32::MAX, 0)
                };
                self.push(
                    target,
                    other,
                    sub,
                    Some(parent),
                    slot as u32,
                    layer as u32,
                    estimate,
                    cost,
                );
            }
        }
    }

    /// `0x7FF78A626020` / `0x7FF78A6263A0`: the window's entries at
    /// `target` get the nodes the entry at `slot`, `layer` links to in
    /// direction `code` (a node already there this search stays, a new one
    /// takes the first entry not used this search). False when a linked node
    /// isn't in the grid.
    fn fill(&mut self, slot: usize, layer: usize, code: u8, target: usize) -> bool {
        let entry = &self.window[slot][layer];
        let node = entry.node.expect("searched entries have nodes");
        let vertical = entry.vertical;
        let mut linked = Vec::new();
        self.cells.linked(self.graph, node, &mut linked);
        for (next, direction) in linked {
            let [dx, dy, dz] = super::DIRECTIONS[direction as usize];
            if (9 * dx + 3 * dz + dy + 13) as u8 != code {
                continue;
            }
            let entries = &mut self.window[target];
            if entries
                .iter()
                .any(|entry| entry.node == Some(next) && entry.search == self.search)
            {
                continue;
            }
            let Some(free) = entries.iter().position(|entry| entry.search != self.search) else {
                continue;
            };
            let Some(next) = self.cells.node_at(self.graph, next.unit, next.place) else {
                return false;
            };
            let mask = self.cells.mask(self.graph, next);
            let entry = &mut entries[free];
            entry.vertical = vertical;
            entry.node = Some(next);
            entry.word = self.cells.entry_word(self.graph, next);
            entry.mask = mask;
            entry.empty = mask == 0;
            entry.search = self.search;
            entry.used = 0;
            entry.records = [Record::default(); 64];
            entry.closed = 0;
            entry.open = 0;
        }
        true
    }

    /// `0x7FF78A626830`: the records back from `last`'s to the start's.
    fn path(&self, last: Open) -> Vec<Step> {
        let mut steps = Vec::new();
        let (mut slot, mut layer, mut index) = (
            last.slot as usize,
            last.layer as usize,
            u32::from(last.record),
        );
        loop {
            let entry = &self.window[slot][layer];
            let record = entry.records[index as usize];
            steps.push(Step {
                sub: (record.word >> 2 & 63) as u8,
                node: entry.node.expect("searched entries have nodes"),
                direction: 0,
            });
            layer = (record.parent_layer & 3) as usize;
            slot = (record.word >> 20 & 0x7FF) as usize;
            index = record.word >> 8 & 63;
            if index == NONE {
                break;
            }
        }
        steps.reverse();
        for at in 1..steps.len() {
            let deltas = deltas(steps[at - 1].sub, steps[at].sub);
            steps[at].direction = if deltas.iter().all(|delta| delta.abs() <= 1) {
                STEP_DIRECTION[(9 * deltas[0] + 3 * deltas[2] + deltas[1] + 13) as usize]
            } else {
                26
            };
        }
        steps[0].direction = 26;
        steps
    }
}
