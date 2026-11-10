//! Navigation over a map's AI tactical volumes, the base for server-driven
//! AI heroes: which volume a point stands in, a route over the volumes'
//! neighbour graph, a path of waypoints on walkable floor, and spread-out
//! standing cells around a goal.
//!
//! The data is `data/navigation/<map>.json.gz`, derived from the map's chunk
//! 0x12 by `orpheus_assets` (format in `docs/dev/data-sources.md`). Each
//! volume is a box of voxels, 1 × 1.4 × 1 m (y counts layers of [`LAYER`]
//! metres), with a walkable mask, a floor point and the volumes it links
//! to. Links are the client's: two volumes whose cells touch but aren't
//! linked (a wall, a drop) are not crossed.
//!
//! The Python reference server had no AI navigation; the approach (A* over
//! the volume graph from the attacker spawn to a point, a cell per bot near
//! the goal, walkable voxel columns) follows the ow1-preservation project's
//! notes (`docs/notes/related-projects.md`, "AI bots and navigation").

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap},
    fs::File,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;
use serde::Deserialize;

pub type Vec3 = [f32; 3];
/// A voxel: x and z in metres, y in layers of [`LAYER`] metres.
pub type Cell = [i32; 3];

/// The height of a voxel layer.
pub const LAYER: f32 = 1.4;
/// Floors sit on a grid of a quarter layer (`docs/notes/ow1-preservation.md`:
/// "node y = floor rounded up to 0.35 m").
const FLOOR_STEP: f32 = 0.35;
/// How far a point off the walkable cells (a capture point's centre on a
/// prop, a spawn in a doorway) snaps to the nearest walkable cell.
pub const SNAP: f32 = 4.0;

/// A map's navigation file under the runtime folder.
pub fn file(runtime: &Path, map_guid: u64) -> PathBuf {
    runtime
        .join("data/navigation")
        .join(format!("{:04X}.json.gz", map_guid as u32))
}

/// The layer a height is in. Floor points sit on the 0.35 m grid with a
/// float error either way (9.7999 is the floor of layer 7), so the height
/// is rounded to that grid first.
pub fn layer_of(y: f32) -> i32 {
    ((y / FLOOR_STEP).round() as i32).div_euclid(4)
}

/// The voxel a point is in.
pub fn cell_of(point: Vec3) -> Cell {
    [
        point[0].floor() as i32,
        layer_of(point[1]),
        point[2].floor() as i32,
    ]
}

/// One tactical volume.
#[derive(Clone, Debug, Deserialize)]
pub struct Volume {
    /// The client's floor point, a walkable spot inside the volume.
    pub floor: Vec3,
    /// The first cell of the box.
    pub origin: Cell,
    /// The box's size in cells (x, layers, z).
    pub size: [i32; 3],
    /// The linked volumes.
    pub neighbours: Vec<usize>,
    /// The walkable mask, bit `z + Sz·(x + Sx·y)` of the cell `origin + (x,
    /// y, z)`, 32 cells a word.
    walkable: Vec<u32>,
}

impl Volume {
    fn bit(&self, cell: Cell) -> Option<usize> {
        let local = [0, 1, 2].map(|axis| cell[axis] - self.origin[axis]);
        if (0..3).any(|axis| !(0..self.size[axis]).contains(&local[axis])) {
            return None;
        }
        let [x, y, z] = local;
        Some((z + self.size[2] * (x + self.size[0] * y)) as usize)
    }

    pub fn walkable(&self, cell: Cell) -> bool {
        self.bit(cell)
            .is_some_and(|bit| self.walkable[bit / 32] >> (bit % 32) & 1 == 1)
    }

    /// A walkable cell whose eight same-layer neighbours are walkable in
    /// this volume too: away from walls and edges. The client ships the
    /// other walkable cells as a second mask (`m_63EE0EDB`).
    pub fn interior(&self, cell: Cell) -> bool {
        self.walkable(cell)
            && (-1..=1)
                .all(|dx| (-1..=1).all(|dz| self.walkable([cell[0] + dx, cell[1], cell[2] + dz])))
    }

    /// The walkable cells, layer by layer.
    pub fn cells(&self) -> impl Iterator<Item = Cell> + '_ {
        let [sx, sy, sz] = self.size;
        let [ox, oy, oz] = self.origin;
        (0..sy)
            .flat_map(move |y| {
                (0..sx).flat_map(move |x| (0..sz).map(move |z| [ox + x, oy + y, oz + z]))
            })
            .filter(|cell| self.walkable(*cell))
    }

    /// The floor height of a layer of this volume: the floor point's,
    /// shifted by whole layers. An estimate: the mask says only which layer
    /// a floor is in, so a slope within a layer reads flat.
    pub fn floor_y(&self, layer: i32) -> f32 {
        self.floor[1] + (layer - layer_of(self.floor[1])) as f32 * LAYER
    }

    /// The centre of a cell's floor.
    pub fn centre(&self, cell: Cell) -> Vec3 {
        [
            cell[0] as f32 + 0.5,
            self.floor_y(cell[1]),
            cell[2] as f32 + 0.5,
        ]
    }

    /// Whether a point is within `margin` metres of the box.
    fn near(&self, point: Vec3, margin: f32) -> bool {
        let scale = [1.0, LAYER, 1.0];
        (0..3).all(|axis| {
            let low = self.origin[axis] as f32 * scale[axis];
            let high = (self.origin[axis] + self.size[axis]) as f32 * scale[axis];
            point[axis] >= low - margin && point[axis] <= high + margin
        })
    }
}

/// A walkable cell and the volume it was found in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spot {
    pub volume: usize,
    pub cell: Cell,
}

/// The volumes from one to another and the length of the route through
/// them: floor point to the shared border's centre to the next floor point.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub volumes: Vec<usize>,
    pub length: f32,
}

/// A walk between two points: the route and the waypoints, on walkable
/// floor (each waypoint's height is its cell's floor estimate, see
/// [`Volume::floor_y`]), straight lines between them crossing only walkable
/// cells of the route's volumes.
#[derive(Clone, Debug, PartialEq)]
pub struct Walk {
    pub route: Route,
    pub waypoints: Vec<Vec3>,
}

impl Walk {
    pub fn length(&self) -> f32 {
        self.waypoints
            .windows(2)
            .map(|pair| distance(pair[0], pair[1]))
            .sum()
    }
}

/// A map's navigation.
#[derive(Debug)]
pub struct Navigation {
    volumes: Vec<Volume>,
    /// The centre of the border between two linked volumes, keyed by the
    /// lower index first.
    portals: HashMap<(usize, usize), Vec3>,
    /// The connected group of each volume (the lowest volume index in it).
    components: Vec<usize>,
}

#[derive(Deserialize)]
struct NavigationFile {
    volumes: Vec<Volume>,
}

impl Navigation {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::read(BufReader::new(File::open(path)?))
    }

    /// A gzipped navigation file.
    pub fn read(input: impl Read) -> io::Result<Self> {
        let mut text = Vec::new();
        GzDecoder::new(input).read_to_end(&mut text)?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &[u8]) -> io::Result<Self> {
        let file: NavigationFile = serde_json::from_slice(text).map_err(invalid)?;
        Self::new(file.volumes)
    }

    fn new(volumes: Vec<Volume>) -> io::Result<Self> {
        for (index, volume) in volumes.iter().enumerate() {
            if volume.size.iter().any(|&side| side <= 0) {
                return Err(invalid(format!("volume {index} has an empty size")));
            }
            let cells: usize = volume.size.iter().map(|&side| side as usize).product();
            if volume.walkable.len() != cells.div_ceil(32) {
                return Err(invalid(format!(
                    "volume {index}'s mask doesn't fit its size"
                )));
            }
            if volume
                .neighbours
                .iter()
                .any(|&other| other >= volumes.len() || other == index)
            {
                return Err(invalid(format!("volume {index} has a bad link")));
            }
        }
        let mut navigation = Self {
            components: components(&volumes),
            volumes,
            portals: HashMap::new(),
        };
        for a in 0..navigation.volumes.len() {
            for &b in &navigation.volumes[a].neighbours {
                let key = (a.min(b), a.max(b));
                if !navigation.portals.contains_key(&key) {
                    let portal = navigation.border(key.0, key.1);
                    navigation.portals.insert(key, portal);
                }
            }
        }
        Ok(navigation)
    }

    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    /// The connected group a volume is in, named by its lowest volume.
    pub fn component(&self, volume: usize) -> usize {
        self.components[volume]
    }

    /// The walkable cell a point stands in: its own voxel, or the one below
    /// (a body stands about 0.8 m above the floor, which can be the next
    /// layer up).
    pub fn locate(&self, point: Vec3) -> Option<Spot> {
        let [x, layer, z] = cell_of(point);
        [layer, layer - 1].into_iter().find_map(|layer| {
            let cell = [x, layer, z];
            self.volumes
                .iter()
                .position(|volume| volume.walkable(cell))
                .map(|volume| Spot { volume, cell })
        })
    }

    /// The volume a point stands in (see [`Self::locate`]).
    pub fn volume_at(&self, point: Vec3) -> Option<usize> {
        self.locate(point).map(|spot| spot.volume)
    }

    /// The cell a point stands in, else the walkable cell whose floor centre
    /// is nearest within `radius` metres.
    pub fn nearest(&self, point: Vec3, radius: f32) -> Option<Spot> {
        if let Some(spot) = self.locate(point) {
            return Some(spot);
        }
        let mut best: Option<(f32, Spot)> = None;
        for (index, volume) in self.volumes.iter().enumerate() {
            if !volume.near(point, radius + LAYER) {
                continue;
            }
            for cell in volume.cells() {
                let gap = distance(volume.centre(cell), point);
                if gap <= radius && best.is_none_or(|(nearest, _)| gap < nearest) {
                    best = Some((
                        gap,
                        Spot {
                            volume: index,
                            cell,
                        },
                    ));
                }
            }
        }
        best.map(|(_, spot)| spot)
    }

    /// The shortest route between two volumes over their links (A*).
    pub fn route(&self, from: usize, to: usize) -> Option<Route> {
        let count = self.volumes.len();
        if from >= count || to >= count {
            return None;
        }
        let goal = self.volumes[to].floor;
        let mut cost = vec![f32::INFINITY; count];
        let mut previous = vec![usize::MAX; count];
        let mut closed = vec![false; count];
        let mut open = BinaryHeap::new();
        cost[from] = 0.0;
        open.push(Open {
            estimate: distance(self.volumes[from].floor, goal),
            node: from,
        });
        while let Some(Open { node, .. }) = open.pop() {
            if node == to {
                break;
            }
            if std::mem::replace(&mut closed[node], true) {
                continue;
            }
            for &next in &self.volumes[node].neighbours {
                let portal = self.portal(node, next);
                let reached = cost[node]
                    + distance(self.volumes[node].floor, portal)
                    + distance(portal, self.volumes[next].floor);
                if reached < cost[next] {
                    cost[next] = reached;
                    previous[next] = node;
                    open.push(Open {
                        estimate: reached + distance(self.volumes[next].floor, goal),
                        node: next,
                    });
                }
            }
        }
        if !cost[to].is_finite() {
            return None;
        }
        let mut volumes = vec![to];
        while volumes[volumes.len() - 1] != from {
            volumes.push(previous[volumes[volumes.len() - 1]]);
        }
        volumes.reverse();
        Some(Route {
            volumes,
            length: cost[to],
        })
    }

    /// A walk from one point to another: both snap to walkable cells (within
    /// [`SNAP`] metres), the route picks the volumes, an A* over their cells
    /// finds the walk and the waypoints keep only the turns (a straight line
    /// to a later cell replaces the cells between when it crosses only
    /// walkable, passable cells). The first and last waypoints are the
    /// points themselves on the floor when they stand on walkable cells, the
    /// snapped cells' centres otherwise. `None` when either point is off the
    /// map's walkable cells or no route joins them.
    pub fn path(&self, from: Vec3, to: Vec3) -> Option<Walk> {
        let start = self.nearest(from, SNAP)?;
        let goal = self.nearest(to, SNAP)?;
        let route = self.route(start.volume, goal.volume)?;
        let corridor = &route.volumes;
        let cells = self.walk_cells(corridor, start.cell, goal.cell)?;
        let end = |point: Vec3, spot: Spot| {
            let volume = &self.volumes[spot.volume];
            if self.locate(point) == Some(spot) {
                [point[0], volume.floor_y(spot.cell[1]), point[2]]
            } else {
                volume.centre(spot.cell)
            }
        };
        let mut waypoints = vec![end(from, start)];
        let mut anchor = 0;
        while anchor + 1 < cells.len() {
            let mut reach = anchor + 1;
            while reach + 1 < cells.len() && self.clear(corridor, cells[anchor], cells[reach + 1]) {
                reach += 1;
            }
            if reach + 1 < cells.len() {
                waypoints.push(self.centre_in(corridor, cells[reach]));
            }
            anchor = reach;
        }
        waypoints.push(end(to, goal));
        Some(Walk { route, waypoints })
    }

    /// Up to `count` standing cells around a point, spread apart: walkable
    /// cells away from walls and edges ([`Volume::interior`]; any walkable
    /// cell when there are none) whose floor centre is within `radius`
    /// metres of the point, in the volume group of the point's cell. The
    /// first is the one nearest the point, each next the one farthest from
    /// those already taken. Empty when the point is off the walkable cells.
    pub fn free_cells(&self, around: Vec3, radius: f32, count: usize) -> Vec<Vec3> {
        let Some(at) = self.nearest(around, SNAP) else {
            return Vec::new();
        };
        let group = self.components[at.volume];
        let mut interior = Vec::new();
        let mut walkable = Vec::new();
        for (index, volume) in self.volumes.iter().enumerate() {
            if self.components[index] != group || !volume.near(around, radius + LAYER) {
                continue;
            }
            for cell in volume.cells() {
                let centre = volume.centre(cell);
                if distance(centre, around) > radius {
                    continue;
                }
                if volume.interior(cell) {
                    interior.push(centre);
                }
                walkable.push(centre);
            }
        }
        let mut candidates = if interior.is_empty() {
            walkable
        } else {
            interior
        };
        let mut taken = Vec::new();
        while taken.len() < count && !candidates.is_empty() {
            let score = |candidate: Vec3| {
                if taken.is_empty() {
                    -distance(candidate, around)
                } else {
                    taken
                        .iter()
                        .map(|&other| distance(candidate, other))
                        .fold(f32::INFINITY, f32::min)
                }
            };
            let mut best = 0;
            for index in 1..candidates.len() {
                if score(candidates[index]) > score(candidates[best]) {
                    best = index;
                }
            }
            taken.push(candidates.swap_remove(best));
        }
        taken
    }

    fn portal(&self, a: usize, b: usize) -> Vec3 {
        self.portals[&(a.min(b), a.max(b))]
    }

    /// The centre of the border between two volumes: the mean of the
    /// midpoints between each cell of one and a touching cell of the other
    /// (sideways or the same column, up to a layer apart, or the same cell
    /// where they overlap). The midpoint of the floor points when no cells
    /// touch.
    fn border(&self, a: usize, b: usize) -> Vec3 {
        const TOUCHING: [(i32, i32); 5] = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)];
        let (first, second) = (&self.volumes[a], &self.volumes[b]);
        let mut sum = [0.0f64; 3];
        let mut pairs = 0u32;
        for cell in first.cells() {
            for (dx, dz) in TOUCHING {
                for dl in -1..=1 {
                    let other = [cell[0] + dx, cell[1] + dl, cell[2] + dz];
                    if !second.walkable(other) {
                        continue;
                    }
                    let mid = midpoint(first.centre(cell), second.centre(other));
                    for axis in 0..3 {
                        sum[axis] += f64::from(mid[axis]);
                    }
                    pairs += 1;
                }
            }
        }
        if pairs == 0 {
            return midpoint(first.floor, second.floor);
        }
        sum.map(|total| (total / f64::from(pairs)) as f32)
    }

    fn linked(&self, a: usize, b: usize) -> bool {
        a == b || self.volumes[a].neighbours.contains(&b)
    }

    /// The route volumes that have a cell walkable.
    fn owners<'a>(&'a self, corridor: &'a [usize], cell: Cell) -> impl Iterator<Item = usize> + 'a {
        corridor
            .iter()
            .copied()
            .filter(move |&volume| self.volumes[volume].walkable(cell))
    }

    /// The floor centre of a cell of the route, in the first volume that has
    /// it.
    fn centre_in(&self, corridor: &[usize], cell: Cell) -> Vec3 {
        let volume = self
            .owners(corridor, cell)
            .next()
            .expect("a cell of the walk is walkable in the route");
        self.volumes[volume].centre(cell)
    }

    /// Whether a body can step from a cell to the next one (sideways or
    /// diagonally, up to a layer up or down): both walkable in route volumes
    /// that are the same or linked, and for a diagonal step both corners
    /// open.
    fn passable(&self, corridor: &[usize], from: Cell, to: Cell) -> bool {
        let linked = self
            .owners(corridor, from)
            .any(|a| self.owners(corridor, to).any(|b| self.linked(a, b)));
        if !linked {
            return false;
        }
        if from[0] == to[0] || from[2] == to[2] {
            return true;
        }
        let open = |x: i32, z: i32| {
            [from[1], to[1]]
                .into_iter()
                .any(|layer| self.owners(corridor, [x, layer, z]).next().is_some())
        };
        open(to[0], from[2]) && open(from[0], to[2])
    }

    /// The shortest walk over the route's cells (A*, 8 directions).
    fn walk_cells(&self, corridor: &[usize], start: Cell, goal: Cell) -> Option<Vec<Cell>> {
        let goal_centre = self.centre_in(corridor, goal);
        let mut index = HashMap::from([(start, 0)]);
        let mut cells = vec![start];
        let mut cost = vec![0.0f32];
        let mut previous = vec![usize::MAX];
        let mut closed = vec![false];
        let mut open = BinaryHeap::from([Open {
            estimate: distance(self.centre_in(corridor, start), goal_centre),
            node: 0,
        }]);
        let found = loop {
            let Open { node, .. } = open.pop()?;
            if cells[node] == goal {
                break node;
            }
            if std::mem::replace(&mut closed[node], true) {
                continue;
            }
            let cell = cells[node];
            let centre = self.centre_in(corridor, cell);
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    for dl in -1..=1 {
                        let next = [cell[0] + dx, cell[1] + dl, cell[2] + dz];
                        if !self.passable(corridor, cell, next) {
                            continue;
                        }
                        let next_centre = self.centre_in(corridor, next);
                        let reached = cost[node] + distance(centre, next_centre);
                        let slot = *index.entry(next).or_insert_with(|| {
                            cells.push(next);
                            cost.push(f32::INFINITY);
                            previous.push(usize::MAX);
                            closed.push(false);
                            cells.len() - 1
                        });
                        if reached < cost[slot] {
                            cost[slot] = reached;
                            previous[slot] = node;
                            open.push(Open {
                                estimate: reached + distance(next_centre, goal_centre),
                                node: slot,
                            });
                        }
                    }
                }
            }
        };
        let mut walk = vec![cells[found]];
        let mut node = found;
        while previous[node] != usize::MAX {
            node = previous[node];
            walk.push(cells[node]);
        }
        walk.reverse();
        Some(walk)
    }

    /// Whether the straight line between two cells' centres crosses only
    /// passable cells of the route and stays within a layer of the line's
    /// height (so a line over a dip or a rise keeps the turn at its foot:
    /// the waypoints follow the floor's heights). The line visits every
    /// column it passes through (through a corner exactly, a diagonal step),
    /// each entered at the layer (the one below, the same or the one above)
    /// nearest the line's height there.
    fn clear(&self, corridor: &[usize], from: Cell, to: Cell) -> bool {
        let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
        let (nx, nz) = (dx.abs(), dz.abs());
        let (sx, sz) = (dx.signum(), dz.signum());
        let (mut ix, mut iz) = (0, 0);
        let mut at = from;
        while ix < nx || iz < nz {
            // Which grid line the line crosses next: x at (0.5 + ix) / nx of
            // the way, z at (0.5 + iz) / nz.
            let next_x = (1 + 2 * ix) * nz;
            let next_z = (1 + 2 * iz) * nx;
            let t = if next_x <= next_z {
                (1 + 2 * ix) as f32 / (2 * nx) as f32
            } else {
                (1 + 2 * iz) as f32 / (2 * nz) as f32
            };
            let (mut x, mut z) = (at[0], at[2]);
            if next_x <= next_z {
                x += sx;
                ix += 1;
            }
            if next_z <= next_x {
                z += sz;
                iz += 1;
            }
            let height = from[1] as f32 + (to[1] - from[1]) as f32 * t;
            let off = |cell: &Cell| (cell[1] as f32 - height).abs();
            let next = (at[1] - 1..=at[1] + 1)
                .map(|layer| [x, layer, z])
                .filter(|next| off(next) <= 1.0 && self.passable(corridor, at, *next))
                .min_by(|p, q| off(p).total_cmp(&off(q)));
            let Some(next) = next else {
                return false;
            };
            at = next;
        }
        at == to
    }
}

/// An open A* node, popped lowest estimate first.
#[derive(PartialEq)]
struct Open {
    estimate: f32,
    node: usize,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then(other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Each volume's connected group, named by its lowest volume.
fn components(volumes: &[Volume]) -> Vec<usize> {
    let mut group: Vec<usize> = (0..volumes.len()).collect();
    fn root(group: &mut [usize], mut node: usize) -> usize {
        while group[node] != node {
            group[node] = group[group[node]];
            node = group[node];
        }
        node
    }
    for (a, volume) in volumes.iter().enumerate() {
        for &b in &volume.neighbours {
            let (ra, rb) = (root(&mut group, a), root(&mut group, b));
            group[ra.max(rb)] = ra.min(rb);
        }
    }
    (0..volumes.len())
        .map(|node| root(&mut group, node))
        .collect()
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn midpoint(a: Vec3, b: Vec3) -> Vec3 {
    [0, 1, 2].map(|axis| (a[axis] + b[axis]) / 2.0)
}

fn invalid(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests;
