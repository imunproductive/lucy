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
//!
//! The client's own path query (`0x7FF78A626BC0`, Widowmaker's ledge pull
//! and the AI locomotion of component 88) searches the chunk's other
//! document, the AI cell data (`STU_3BC32A6E`): 16 m cells keyed as that
//! document's cell keys, each a sparse 64 × 64 × 64 voxel grid, with an A*
//! whose estimate is 1000 × the Manhattan distance in its nodes
//! (`D:/Work/Coding/refs/reports/Navigation.md`). That document isn't
//! decoded, so the server paths over these volumes instead (assumption:
//! the same walkable floor at 1 m instead of 0.25 m). Their heights are
//! coarse (a layer is 1.4 m), so [`Navigation::path_with`] lets the caller
//! test the steps between layers and the straight lines against the map's
//! collision.

use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashMap, HashSet},
    fs::File,
    hash::{BuildHasherDefault, Hasher},
    io::{self, BufReader, Read},
    mem,
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
/// What a step onto a cell next to a wall or an edge (a neighbour no body
/// can step to) costs on top of its length: walks keep a cell away from
/// walls where there is room, so a body's capsule doesn't catch on the
/// corners it turns (assumption; the client's step costs aren't decoded).
const EDGE_COST: f32 = 1.0;
/// How many times a walk is searched again without a step the caller
/// refused ([`Navigation::path_with`]; assumption: enough to get round a
/// railing or a corner the cells don't show, cheap enough for a tick).
const REPLANS: usize = 8;
/// How many different steps and lines one path asks the caller about
/// ([`Navigation::path_with`]); past it the cells alone decide. Bounds a
/// path's work where steps keep failing (assumption: enough for the walks
/// the tests make).
const CHECKS: usize = 256;
/// What a path search's steps cost in work ([`PathSearch::advance`]), in
/// units of about a microsecond of a release build, set at the dearest
/// places' cost (measured 2026-11-19 over every spawn → objective walk of
/// four PvP maps, `D:/Work/Coding/refs/reports/Navigation.md`): finding
/// the volumes and the route (up to 0.2 ms), a cell the A* expands (4 µs
/// on average, 6 on Rialto), a cell of a walk whose turns are found, and a
/// walk test (the caller's capsule check: 15–110 µs for the short steps
/// between layers, most of them, the dearest in Gibraltar's first spawn;
/// long lines up to 28 µs more a metre).
const SETUP_WORK: u32 = 200;
const EXPAND_WORK: u32 = 7;
const TURN_WORK: u32 = 1;
const CHECK_WORK: u32 = 80;
const CHECK_WORK_PER_METRE: f32 = 25.0;

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
    portals: FastMap<(usize, usize), Vec3>,
    /// The connected group of each volume (the lowest volume index in it).
    components: Vec<usize>,
    /// The volumes each walkable cell is walkable in, lowest index first.
    cells: FastMap<Cell, Vec<usize>>,
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
        let mut cells: FastMap<Cell, Vec<usize>> = FastMap::default();
        for (index, volume) in volumes.iter().enumerate() {
            for cell in volume.cells() {
                cells.entry(cell).or_default().push(index);
            }
        }
        let mut navigation = Self {
            components: components(&volumes),
            volumes,
            portals: FastMap::default(),
            cells,
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

    /// The walkable cell a point stands in: its own voxel, the one below (a
    /// body stands about 0.8 m above the floor, which can be the next layer
    /// up), or the one above when that cell's floor estimate is within a
    /// quarter layer over the point: floors are rounded up to the 0.35 m grid,
    /// so feet on a floor at 11.0 stand under the 11.2 the volume gives,
    /// which is the next layer's bottom (a Practice Range patrol).
    pub fn locate(&self, point: Vec3) -> Option<Spot> {
        let [x, layer, z] = cell_of(point);
        let find = |layer: i32, fits: &dyn Fn(&Volume) -> bool| {
            let cell = [x, layer, z];
            self.cells
                .get(&cell)
                .into_iter()
                .flatten()
                .copied()
                .find(|&volume| fits(&self.volumes[volume]))
                .map(|volume| Spot { volume, cell })
        };
        find(layer, &|_| true)
            .or_else(|| find(layer - 1, &|_| true))
            .or_else(|| {
                find(layer + 1, &|volume| {
                    volume.floor_y(layer + 1) - point[1] <= FLOOR_STEP + 1e-3
                })
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

    /// A walk from one point to another, the map's cells alone deciding
    /// ([`Self::path_with`] with every step and line walkable).
    pub fn path(&self, from: Vec3, to: Vec3) -> Option<Walk> {
        self.path_with(from, to, |_, _| true)
    }

    /// A walk from one point to another: both snap to walkable cells (within
    /// [`SNAP`] metres), the route picks the volumes, an A* over their cells
    /// finds the walk (a cell a step from a wall or an edge costs
    /// [`EDGE_COST`] more) and the waypoints keep only the turns (a straight
    /// line to a later cell replaces the cells between when it crosses only
    /// walkable, passable cells). `walkable` tells whether a body walks
    /// straight from one floor point to another (heights are the cells'
    /// floor estimates, a layer off on a slope): the A* asks it for each step
    /// to another layer, the waypoints for each line that replaces cells and
    /// for each step between neighbouring cells they keep. A kept step it
    /// refuses (a railing or a wall's corner inside the cells) is taken out
    /// and the A* runs again, at most [`REPLANS`] times, then as often over
    /// the route's volumes and those linked to them; the first walk is kept
    /// when none passes. `walkable` is asked once per pair of points and at
    /// most [`CHECKS`] times a path. The first and last waypoints are the points
    /// themselves on the floor when they stand on walkable cells, the
    /// snapped cells' centres otherwise. `None` when either point is off the
    /// map's walkable cells or no route joins them. The whole search at once
    /// ([`PathSearch`] runs it a bounded amount of work at a time).
    pub fn path_with(
        &self,
        from: Vec3,
        to: Vec3,
        mut walkable: impl FnMut(Vec3, Vec3) -> bool,
    ) -> Option<Walk> {
        let mut search = PathSearch::new(from, to);
        loop {
            let mut work = u32::MAX;
            if let Some(walk) = search.advance(self, &mut work, &mut walkable) {
                return walk;
            }
        }
    }

    /// The start of a search ([`Self::path_with`]): the snapped cells, the
    /// route, the walk's ends, the corridors and the first A*.
    fn begin(&self, from: Vec3, to: Vec3) -> Option<Searching> {
        let start = self.nearest(from, SNAP)?;
        let goal = self.nearest(to, SNAP)?;
        let route = self.route(start.volume, goal.volume)?;
        let end = |point: Vec3, spot: Spot| {
            let volume = &self.volumes[spot.volume];
            if self.locate(point) == Some(spot) {
                [point[0], volume.floor_y(spot.cell[1]), point[2]]
            } else {
                volume.centre(spot.cell)
            }
        };
        let ends = [end(from, start), end(to, goal)];
        // The route's volumes, then those and their linked ones: a way round
        // what the route's cells can't pass may lead through a volume beside.
        let mut narrow = vec![false; self.volumes.len()];
        for &volume in &route.volumes {
            narrow[volume] = true;
        }
        let mut wide = narrow.clone();
        for &volume in &route.volumes {
            for &next in &self.volumes[volume].neighbours {
                wide[next] = true;
            }
        }
        let stage = Stage::Cells(self.cell_search(&narrow, start.cell, goal.cell));
        Some(Searching {
            start: start.cell,
            goal: goal.cell,
            route,
            ends,
            corridors: [narrow, wide],
            corridor: 0,
            runs: 0,
            blocked: FastSet::default(),
            edges: FastMap::default(),
            answers: FastMap::default(),
            first: None,
            stage,
        })
    }

    /// The waypoints of a walk over `cells` from `ends[0]` to `ends[1]`
    /// ([`Self::path_with`]), found from `anchor` on into `waypoints`, and
    /// the first step between neighbouring cells the caller refuses into
    /// `refused`. `None` when the work ran out: the search goes on from
    /// `anchor` (the tests asked already are answered again for free).
    #[allow(clippy::too_many_arguments)]
    fn turns(
        &self,
        corridor: &[bool],
        cells: &[Cell],
        [_, last]: [Vec3; 2],
        waypoints: &mut Vec<Vec3>,
        refused: &mut Option<(Cell, Cell)>,
        anchor: &mut usize,
        checks: &mut Checks<'_, impl FnMut(Vec3, Vec3) -> bool>,
    ) -> Option<()> {
        let point = |index: usize| {
            if index + 1 == cells.len() {
                last
            } else {
                self.centre_in(corridor, cells[index])
            }
        };
        while *anchor + 1 < cells.len() {
            let at = waypoints[waypoints.len() - 1];
            let mut reach = *anchor + 1;
            while reach + 1 < cells.len()
                && self.clear(corridor, cells[*anchor], cells[reach + 1])
                && checks.ask(at, point(reach + 1))?
            {
                reach += 1;
            }
            if reach == *anchor + 1 && refused.is_none() && !checks.ask(at, point(reach))? {
                *refused = Some((cells[*anchor], cells[reach]));
            }
            if reach + 1 < cells.len() {
                waypoints.push(point(reach));
            }
            *anchor = reach;
        }
        waypoints.push(last);
        Some(())
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

    /// The volumes a cell is walkable in, lowest index first.
    fn walkable_in(&self, cell: Cell) -> &[usize] {
        self.cells.get(&cell).map_or(&[], Vec::as_slice)
    }

    /// The route volumes (`corridor[volume]`) that have a cell walkable.
    fn owners<'a>(&'a self, corridor: &'a [bool], cell: Cell) -> impl Iterator<Item = usize> + 'a {
        in_corridor(corridor, self.walkable_in(cell))
    }

    /// The floor centre of a cell of the route, in the first volume that has
    /// it.
    fn centre_in(&self, corridor: &[bool], cell: Cell) -> Vec3 {
        self.centre_among(corridor, cell, self.walkable_in(cell))
    }

    /// [`Self::centre_in`] with the volumes the cell is walkable in.
    fn centre_among(&self, corridor: &[bool], cell: Cell, volumes: &[usize]) -> Vec3 {
        let volume = in_corridor(corridor, volumes)
            .next()
            .expect("a cell of the walk is walkable in the route");
        self.volumes[volume].centre(cell)
    }

    /// Whether a body can step from a cell to the next one (sideways or
    /// diagonally, up to a layer up or down): both walkable in route volumes
    /// that are the same or linked, and for a diagonal step both corners
    /// open.
    fn passable(&self, corridor: &[bool], from: Cell, to: Cell) -> bool {
        self.passable_between(
            corridor,
            from,
            self.walkable_in(from),
            to,
            self.walkable_in(to),
        )
    }

    /// [`Self::passable`] with the volumes each cell is walkable in.
    fn passable_between(
        &self,
        corridor: &[bool],
        from: Cell,
        here: &[usize],
        to: Cell,
        there: &[usize],
    ) -> bool {
        let linked = in_corridor(corridor, here)
            .any(|a| in_corridor(corridor, there).any(|b| self.linked(a, b)));
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

    /// Whether a cell is a step from a wall or an edge: one of its eight
    /// neighbours is no cell a body can step to (a wall's corner counts).
    fn edge(&self, corridor: &[bool], cell: Cell) -> bool {
        let here = self.walkable_in(cell);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dz| {
                (dx, dz) != (0, 0)
                    && (-1..=1).all(|dl| {
                        let next = [cell[0] + dx, cell[1] + dl, cell[2] + dz];
                        let there = self.walkable_in(next);
                        there.is_empty()
                            || !self.passable_between(corridor, cell, here, next, there)
                    })
            })
        })
    }

    /// A new A* over the route's cells ([`Self::walk_cells`]).
    fn cell_search(&self, corridor: &[bool], start: Cell, goal: Cell) -> CellSearch {
        let goal_centre = self.centre_in(corridor, goal);
        let mut index = FastMap::default();
        index.insert(start, 0);
        CellSearch {
            goal_centre,
            index,
            cells: vec![start],
            cost: vec![0.0],
            previous: vec![usize::MAX],
            closed: vec![false],
            open: BinaryHeap::from([Open {
                estimate: distance(self.centre_in(corridor, start), goal_centre),
                node: 0,
            }]),
            expanding: None,
        }
    }

    /// The shortest walk over the route's cells (A*, 8 directions; a step
    /// to another layer only where the caller says a body takes it; no step
    /// in `blocked`): `Some(None)` when there is none, `None` when the work
    /// ran out. A cell whose expansion the work cut short is expanded again
    /// first next time: the steps it relaxed already change nothing, so the
    /// walk is the one a search without pauses finds. `edges` caches which
    /// cells are a step from a wall (per corridor).
    fn walk_cells(
        &self,
        corridor: &[bool],
        search: &mut CellSearch,
        goal: Cell,
        edges: &mut FastMap<Cell, bool>,
        blocked: &FastSet<(Cell, Cell)>,
        checks: &mut Checks<'_, impl FnMut(Vec3, Vec3) -> bool>,
    ) -> Option<Option<Vec<Cell>>> {
        let CellSearch {
            goal_centre,
            index,
            cells,
            cost,
            previous,
            closed,
            open,
            expanding,
        } = search;
        let found = loop {
            // Only an expansion and a walk test need work: the goal and the
            // closed cells come off the open list without.
            let node = match *expanding {
                Some(node) => node,
                None => {
                    let Some(&Open { node, .. }) = open.peek() else {
                        return Some(None);
                    };
                    if cells[node] == goal {
                        break node;
                    }
                    if !closed[node] && *checks.work == 0 {
                        return None;
                    }
                    open.pop();
                    if mem::replace(&mut closed[node], true) {
                        continue;
                    }
                    checks.spend(EXPAND_WORK);
                    *expanding = Some(node);
                    node
                }
            };
            let cell = cells[node];
            let here = self.walkable_in(cell);
            let centre = self.centre_among(corridor, cell, here);
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    for dl in -1..=1 {
                        let next = [cell[0] + dx, cell[1] + dl, cell[2] + dz];
                        let there = self.walkable_in(next);
                        if there.is_empty()
                            || !self.passable_between(corridor, cell, here, next, there)
                            || index.get(&next).is_some_and(|&slot| closed[slot])
                            || blocked.contains(&(cell, next))
                        {
                            continue;
                        }
                        let next_centre = self.centre_among(corridor, next, there);
                        if dl != 0 && !checks.ask(centre, next_centre)? {
                            continue;
                        }
                        let edge = if *edges
                            .entry(next)
                            .or_insert_with(|| self.edge(corridor, next))
                        {
                            EDGE_COST
                        } else {
                            0.0
                        };
                        let reached = cost[node] + distance(centre, next_centre) + edge;
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
                                estimate: reached + distance(next_centre, *goal_centre),
                                node: slot,
                            });
                        }
                    }
                }
            }
            *expanding = None;
        };
        let mut walk = vec![cells[found]];
        let mut node = found;
        while previous[node] != usize::MAX {
            node = previous[node];
            walk.push(cells[node]);
        }
        walk.reverse();
        Some(Some(walk))
    }

    /// Whether the straight line between two cells' centres crosses only
    /// passable cells of the route and stays within a layer of the line's
    /// height (so a line over a dip or a rise keeps the turn at its foot:
    /// the waypoints follow the floor's heights). The line visits every
    /// column it passes through (through a corner exactly, a diagonal step),
    /// each entered at the layer (the one below, the same or the one above)
    /// nearest the line's height there.
    fn clear(&self, corridor: &[bool], from: Cell, to: Cell) -> bool {
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
#[derive(Clone, Debug, PartialEq)]
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

/// The volumes of a list that are in the corridor.
fn in_corridor<'a>(corridor: &'a [bool], volumes: &'a [usize]) -> impl Iterator<Item = usize> + 'a {
    volumes
        .iter()
        .copied()
        .filter(move |&volume| corridor[volume])
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

/// FxHash (the Rust compiler's hasher) with a final mix: the searches' maps
/// are keyed by a few small integers, which SipHash made most of an A*
/// step's cost.
#[derive(Clone, Copy, Default)]
struct FastHasher(u64);

impl FastHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for FastHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.add(value.into());
    }

    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    fn finish(&self) -> u64 {
        let mut hash = self.0;
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
        hash ^ hash >> 33
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;
type FastSet<K> = HashSet<K, BuildHasherDefault<FastHasher>>;

/// A path query run a bounded amount of work at a time: the search of
/// [`Navigation::path_with`], kept between calls of [`Self::advance`], so a
/// server spreads a long path over its ticks. The walk is the one
/// [`Navigation::path_with`] finds, however the work is split.
#[derive(Clone, Debug)]
pub struct PathSearch {
    from: Vec3,
    to: Vec3,
    /// `None` until the first call has the work to begin.
    state: Option<Searching>,
}

impl PathSearch {
    pub fn new(from: Vec3, to: Vec3) -> Self {
        Self {
            from,
            to,
            state: None,
        }
    }

    /// Search on over `navigation` (always the same one) while `work` is
    /// left, spending it ([`SETUP_WORK`] and the other costs, about a
    /// microsecond of a release build a unit); a step runs while any work is
    /// left, so the last one can overrun it, by a walk test at most. `None`
    /// while the search goes on (no work left), then the walk once
    /// ([`Navigation::path_with`]; `walkable` is asked as there, each pair
    /// once whichever call asks it). Nothing is done without work.
    pub fn advance(
        &mut self,
        navigation: &Navigation,
        work: &mut u32,
        walkable: impl FnMut(Vec3, Vec3) -> bool,
    ) -> Option<Option<Walk>> {
        if self.state.is_none() {
            if *work == 0 {
                return None;
            }
            spend(work, SETUP_WORK);
            match navigation.begin(self.from, self.to) {
                Some(state) => self.state = Some(state),
                None => return Some(None),
            }
        }
        let Searching {
            start,
            goal,
            route,
            ends,
            corridors,
            corridor,
            runs,
            blocked,
            edges,
            answers,
            first,
            stage,
        } = self.state.as_mut()?;
        let mut checks = Checks {
            test: walkable,
            answers,
            work,
        };
        loop {
            let lane = &corridors[*corridor];
            match stage {
                Stage::Cells(search) => {
                    let found =
                        navigation.walk_cells(lane, search, *goal, edges, blocked, &mut checks)?;
                    if let Some(cells) = found {
                        checks.spend(TURN_WORK.saturating_mul(cells.len() as u32));
                        *stage = Stage::Turns {
                            cells,
                            waypoints: vec![ends[0]],
                            refused: None,
                            anchor: 0,
                        };
                        continue;
                    }
                }
                Stage::Turns {
                    cells,
                    waypoints,
                    refused,
                    anchor,
                } => {
                    navigation.turns(
                        lane,
                        cells,
                        *ends,
                        waypoints,
                        refused,
                        anchor,
                        &mut checks,
                    )?;
                    let turns = mem::take(waypoints);
                    let Some((a, b)) = *refused else {
                        return Some(Some(Walk {
                            route: route.clone(),
                            waypoints: turns,
                        }));
                    };
                    first.get_or_insert(turns);
                    blocked.insert((a, b));
                    blocked.insert((b, a));
                    *runs += 1;
                    if *runs <= REPLANS {
                        *stage = Stage::Cells(navigation.cell_search(lane, *start, *goal));
                        continue;
                    }
                }
            }
            // No walk over this corridor, or none it walks: the next one.
            if *corridor + 1 == corridors.len() {
                return Some(first.take().map(|waypoints| Walk {
                    route: route.clone(),
                    waypoints,
                }));
            }
            *corridor += 1;
            *runs = 0;
            blocked.clear();
            edges.clear();
            *stage = Stage::Cells(navigation.cell_search(&corridors[*corridor], *start, *goal));
        }
    }
}

/// A search begun ([`PathSearch`]).
#[derive(Clone, Debug)]
struct Searching {
    start: Cell,
    goal: Cell,
    route: Route,
    /// The walk's first and last points.
    ends: [Vec3; 2],
    /// The route's volumes, then those and the ones linked to them.
    corridors: [Vec<bool>; 2],
    corridor: usize,
    /// The A* runs over this corridor that ended with a refused step.
    runs: usize,
    /// The steps refused over this corridor.
    blocked: FastSet<(Cell, Cell)>,
    /// Which cells are a step from a wall, over this corridor.
    edges: FastMap<Cell, bool>,
    /// The caller's answers, by pair of points.
    answers: FastMap<[u32; 6], bool>,
    /// The waypoints of the first walk, kept when none passes.
    first: Option<Vec<Vec3>>,
    stage: Stage,
}

#[derive(Clone, Debug)]
enum Stage {
    /// An A* over the corridor's cells.
    Cells(CellSearch),
    /// The turns of the walk it found, up to `anchor`.
    Turns {
        cells: Vec<Cell>,
        waypoints: Vec<Vec3>,
        refused: Option<(Cell, Cell)>,
        anchor: usize,
    },
}

/// An A* over a corridor's cells ([`Navigation::walk_cells`]).
#[derive(Clone, Debug)]
struct CellSearch {
    goal_centre: Vec3,
    index: FastMap<Cell, usize>,
    cells: Vec<Cell>,
    cost: Vec<f32>,
    previous: Vec<usize>,
    closed: Vec<bool>,
    open: BinaryHeap<Open>,
    /// The cell being expanded when the work ran out.
    expanding: Option<usize>,
}

fn spend(work: &mut u32, cost: u32) {
    *work = work.saturating_sub(cost);
}

/// A caller's walk test asked at most once per pair of points and at most
/// [`CHECKS`] times in all, each asked while work is left.
struct Checks<'a, F> {
    test: F,
    answers: &'a mut FastMap<[u32; 6], bool>,
    work: &'a mut u32,
}

impl<F: FnMut(Vec3, Vec3) -> bool> Checks<'_, F> {
    fn spend(&mut self, cost: u32) {
        spend(self.work, cost);
    }

    /// The answer; `None` when it must be asked and no work is left.
    fn ask(&mut self, a: Vec3, b: Vec3) -> Option<bool> {
        let key = [a[0], a[1], a[2], b[0], b[1], b[2]].map(f32::to_bits);
        if let Some(&answer) = self.answers.get(&key) {
            return Some(answer);
        }
        if self.answers.len() >= CHECKS {
            return Some(true);
        }
        if *self.work == 0 {
            return None;
        }
        let length = (a[0] - b[0]).hypot(a[2] - b[2]);
        self.spend(CHECK_WORK + (length * CHECK_WORK_PER_METRE) as u32);
        let answer = (self.test)(a, b);
        self.answers.insert(key, answer);
        Some(answer)
    }
}

#[cfg(test)]
mod tests;
