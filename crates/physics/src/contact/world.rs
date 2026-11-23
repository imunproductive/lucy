//! Ordered static proxies and scene sweeps for retail movement collision.
//!
//! Ports `engine_world`, `StaticWorld`, and `scene_sweep` from the Python
//! reference. The character body and its per-shape contact state live above
//! this immutable geometry.
//!
//! Client functions: fat AABB overlap 0x7FF789D110E0 (no sign bit in
//! `(a.high - b.low) | (b.high - a.low)`); shape AABB 0x7FF789D057B0
//! (polytope 0x7FF789CE94B0, capsule 0x7FF789CEBA90, sphere 0x7FF789CEBB20).
//! The probe's scene sweep 0x7FF789D25900 -> 0x7FF789D1D7E0 ->
//! 0x7FF789D22360 -> 0x7FF789D05490, with GJK ray casts per shape (polytope
//! 0x7FF789CE9A00, capsule 0x7FF789CEBEB0, sphere 0x7FF789CEC7F0; no shrink).
//! The broadphase hands the best fraction on as the next shape's limit, so a
//! later shape counts only when strictly closer. Unverified: the world's
//! shape order (chunk polytopes, meshes, then props in placeable order).
//!
//! Switched shapes ([`ContactWorld::with_switched`]): the spawn rooms' team
//! walls (graph `0FA8`, definitions `1C90`/`1C91`) are the models of
//! server-owned placeables that stand in every PvP doorway in pairs, one
//! stopping each team (their meshes' masks 0x88 and 0x90: team proxy 0x08
//! or 0x10, `lucy_assets` `map_areas`). The client turns a wall on for
//! the team it stops only while its graph's state 3 runs (after game
//! message `0845`); the match does the same through
//! [`ContactWorld::switch`]. A switched-off shape is out of every query;
//! a body drops its pairs with it at its next pair update.
//!
//! Moving shapes ([`StaticShape::moving`], [`ContactWorld::place`]): the
//! parts of an idle-animated server placeable's model that its animation
//! moves (Hanamura's cars `03B6`/`03B7`, whose bodies bob on their
//! springs). They stay out of the column grid: each query looks at where
//! each stands now. A move counts as a switch change, so a body rebuilds
//! its contacts with the moved shapes at its next pair update.

use std::collections::{BTreeSet, HashMap, hash_map::Entry};
use std::error::Error;
use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use crate::collision::cache::{CacheError, CollisionCache, Hull, Mesh, Placement, Shape};
use crate::collision::index::GroundHit;
use crate::collision::grid::{ColumnGrid, ColumnRange, GridBuilder};
use crate::contact::gjk::PointSetSize;
use crate::contact::gjk::sweep::{
    CastFrame, CastOptions, CastShape, MeshProbe, ShapeHit, mesh_sweep, shape_cast,
};
use crate::contact::math::{ContactFrame, Vec3, add, maxps, minps, mul, scale, sub};
use crate::contact::mesh::{ContactBounds, ContactMesh, ContactMeshError};
use crate::contact::round::{RoundObstacle, RoundShape};
use crate::contact::triangle::SKIN;
use crate::nav::FastSet;

const STATIC_COLUMN_WIDTH: f64 = 4.0;
const HUGE_PROXY_COLUMNS: u64 = 1024;
pub(crate) const PROXY_MARGIN: f32 = 0.1;
const SWEEP_REACH: f64 = 0.05;

#[derive(Debug)]
pub enum ContactWorldError {
    Cache(CacheError),
    Mesh(ContactMeshError),
    InvalidGeometry,
    IndexLimit,
}

impl fmt::Display for ContactWorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cache(error) => fmt::Display::fmt(error, f),
            Self::Mesh(error) => fmt::Display::fmt(error, f),
            Self::InvalidGeometry => f.write_str("invalid static contact geometry or frame"),
            Self::IndexLimit => {
                f.write_str("static contact proxies exceed the collision grid limit")
            }
        }
    }
}

impl Error for ContactWorldError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Cache(error) => Some(error),
            Self::Mesh(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CacheError> for ContactWorldError {
    fn from(value: CacheError) -> Self {
        Self::Cache(value)
    }
}

impl From<ContactMeshError> for ContactWorldError {
    fn from(value: ContactMeshError) -> Self {
        Self::Mesh(value)
    }
}

#[derive(Clone)]
pub enum StaticGeometry {
    Mesh(Arc<ContactMesh>),
    Hull(Arc<Hull>),
    Round { shape: RoundShape, mask: u16 },
}

impl StaticGeometry {
    pub fn mask(&self) -> u16 {
        match self {
            Self::Mesh(mesh) => mesh.mask,
            Self::Hull(hull) => hull.mask,
            Self::Round { mask, .. } => *mask,
        }
    }
}

pub struct StaticShape {
    geometry: StaticGeometry,
    placed: Placed,
    /// A moving shape's place now ([`ContactWorld::place`]); `placed` is
    /// where it started.
    moving: Option<RwLock<Placed>>,
    /// The group that switches it ([`ContactWorld::with_switched`]).
    switch: Option<usize>,
    /// The platform a mover standing on it rides ([`Platform`]).
    platform: Option<Platform>,
}

/// The entity and node of a server placeable's model that a mover
/// standing on one of its shapes rides (`crate::mover::platform`): the
/// client gives the ground shape's owner as the mover's parent when it has
/// component 77 and any of components 28, 39, 27 or 98, with the shape's
/// node (`0x7FF789B4A250`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Platform {
    pub entity: u32,
    /// The skeleton node's id, 0 for the entity's own frame.
    pub node: u32,
    /// The node's frame in the shape's: its rest place in the model (the
    /// shape's frame moves it with its bone), the translation times the
    /// placeable's scale; the identity for node 0.
    pub node_place: ContactFrame,
    /// The share of the platform's velocity the mover takes (component 77
    /// `m_2F528438`).
    pub factor: f32,
}

/// Where a shape stands: its frame, the matrix the casts take, its fat
/// AABB.
#[derive(Clone, Copy)]
struct Placed {
    frame: Option<ContactFrame>,
    matrix: Option<CastFrame>,
    fat: Option<ContactBounds>,
}

impl Placed {
    fn new(
        geometry: &StaticGeometry,
        frame: Option<ContactFrame>,
    ) -> Result<Self, ContactWorldError> {
        if frame.is_some_and(|frame| {
            !frame
                .translation
                .into_iter()
                .chain(frame.rotation)
                .all(f32::is_finite)
        }) {
            return Err(ContactWorldError::InvalidGeometry);
        }
        let matrix = frame.map(CastFrame::from);
        let fat = shape_bounds(geometry, frame, matrix).map(|bounds| ContactBounds {
            low: sub(bounds.low, [PROXY_MARGIN; 3]),
            high: add(bounds.high, [PROXY_MARGIN; 3]),
        });
        if fat
            .is_some_and(|bounds| !valid_box(bounds.low.map(f64::from), bounds.high.map(f64::from)))
        {
            return Err(ContactWorldError::InvalidGeometry);
        }
        Ok(Self { frame, matrix, fat })
    }
}

impl StaticShape {
    pub fn new(
        geometry: StaticGeometry,
        frame: Option<ContactFrame>,
    ) -> Result<Self, ContactWorldError> {
        let placed = Placed::new(&geometry, frame)?;
        Ok(Self {
            geometry,
            placed,
            moving: None,
            switch: None,
            platform: None,
        })
    }

    /// This shape, which the match moves ([`ContactWorld::place`]): the
    /// animated parts of an idle-animated model.
    pub fn moving(self) -> Self {
        Self {
            moving: Some(RwLock::new(self.placed)),
            ..self
        }
    }

    /// This shape, part of a platform a mover rides ([`Platform`]).
    pub fn on_platform(self, platform: Platform) -> Self {
        Self {
            platform: Some(platform),
            ..self
        }
    }

    pub fn platform(&self) -> Option<Platform> {
        self.platform
    }

    pub fn is_moving(&self) -> bool {
        self.moving.is_some()
    }

    fn placed(&self) -> Placed {
        self.moving.as_ref().map_or(self.placed, |moving| {
            *moving.read().unwrap_or_else(PoisonError::into_inner)
        })
    }

    pub fn geometry(&self) -> &StaticGeometry {
        &self.geometry
    }

    pub fn frame(&self) -> Option<ContactFrame> {
        self.placed().frame
    }

    pub fn fat_bounds(&self) -> Option<ContactBounds> {
        self.placed().fat
    }
}

fn shape_bounds(
    geometry: &StaticGeometry,
    frame: Option<ContactFrame>,
    matrix: Option<CastFrame>,
) -> Option<ContactBounds> {
    let transform = matrix.unwrap_or_else(|| ContactFrame::IDENTITY.into());
    match geometry {
        StaticGeometry::Hull(hull) => {
            if hull.vertices.is_empty() {
                return None;
            }
            let mut low = [f32::MAX; 3];
            let mut high = [-f32::MAX; 3];
            for &point in &hull.vertices {
                let turned = transform.rotate_only(point);
                low = std::array::from_fn(|k| minps(low[k], turned[k]));
                high = std::array::from_fn(|k| maxps(high[k], turned[k]));
            }
            let translation = frame.map_or([0.0; 3], |frame| frame.translation);
            Some(ContactBounds {
                low: add(sub(low, [SKIN; 3]), translation),
                high: add(add([SKIN; 3], high), translation),
            })
        }
        StaticGeometry::Round {
            shape: RoundShape::Sphere(sphere),
            ..
        } => {
            let center = transform.transform(sphere.center);
            Some(ContactBounds {
                low: sub(center, [sphere.radius; 3]),
                high: add(center, [sphere.radius; 3]),
            })
        }
        StaticGeometry::Round {
            shape: RoundShape::Capsule(capsule),
            ..
        } => {
            let a = transform.transform(capsule.a);
            let b = transform.transform(capsule.b);
            Some(ContactBounds {
                low: std::array::from_fn(|k| minps(a[k], b[k]) - capsule.radius),
                high: std::array::from_fn(|k| maxps(a[k], b[k]) + capsule.radius),
            })
        }
        StaticGeometry::Mesh(mesh) => {
            let bounds = mesh.bounds()?;
            let low = sub(bounds.low, [SKIN; 3]);
            let high = add(bounds.high, [SKIN; 3]);
            let Some(frame) = frame else {
                return Some(ContactBounds { low, high });
            };
            Some(oriented_box(
                ContactBounds { low, high },
                frame,
                absolute_columns(transform),
            ))
        }
    }
}

pub(crate) fn absolute_columns(matrix: CastFrame) -> [Vec3; 3] {
    matrix
        .columns()
        .map(|column| column.map(|x| maxps(0.0 - x, x)))
}

pub(crate) fn oriented_box(
    bounds: ContactBounds,
    frame: ContactFrame,
    [c0, c1, c2]: [Vec3; 3],
) -> ContactBounds {
    let ContactBounds { low, high } = bounds;
    let center = frame.place(scale(add(low, high), 0.5));
    let [hx, hy, hz] = scale(sub(high, low), 0.5);
    let extent = add(add(scale(c1, hy), scale(c0, hx)), scale(c2, hz));
    ContactBounds {
        low: sub(center, extent),
        high: add(extent, center),
    }
}

fn valid_box(low: [f64; 3], high: [f64; 3]) -> bool {
    (0..3).all(|k| low[k].is_finite() && high[k].is_finite() && low[k] <= high[k])
}

fn columns(low: [f64; 3], high: [f64; 3]) -> Option<ColumnRange> {
    ColumnRange::for_width([low[0], low[2]], [high[0], high[2]], STATIC_COLUMN_WIDTH)
}

pub(crate) fn overlaps(low: [f64; 3], high: [f64; 3], bounds: ContactBounds) -> bool {
    (0..3).all(|k| {
        !(high[k] - f64::from(bounds.low[k])).is_sign_negative()
            && !(f64::from(bounds.high[k]) - low[k]).is_sign_negative()
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldHit {
    pub hit: ShapeHit,
    pub shape: u32,
    pub triangle: Option<u32>,
}

pub struct ContactWorld {
    shapes: Vec<StaticShape>,
    grid: ColumnGrid,
    /// The moving shapes ([`StaticShape::moving`]), out of the grid: every
    /// query looks at each where it stands now.
    moving: Vec<u32>,
    /// Whether each switched group is on.
    switches: Box<[AtomicBool]>,
    /// Each switched group's shapes.
    groups: Box<[Range<u32>]>,
    /// Moved on by every switch change and every move of a moving shape,
    /// so bodies redo their pairs.
    epoch: AtomicU64,
}

impl ContactWorld {
    /// Keep the supplied proxy order. It also determines equal-fraction ties.
    pub fn new(shapes: Vec<StaticShape>) -> Result<Self, ContactWorldError> {
        Self::build(shapes, 0)
    }

    fn build(shapes: Vec<StaticShape>, groups: usize) -> Result<Self, ContactWorldError> {
        let mut grid = GridBuilder::with_large_limit(HUGE_PROXY_COLUMNS);
        let mut moving = Vec::new();
        let mut ranges = vec![0..0; groups];
        for (index, shape) in shapes.iter().enumerate() {
            let id = u32::try_from(index).map_err(|_| ContactWorldError::IndexLimit)?;
            if let Some(group) = shape.switch {
                let range = &mut ranges[group];
                if range.start == range.end {
                    *range = id..id;
                }
                range.end = id + 1;
            }
            if shape.is_moving() {
                moving.push(id);
                continue;
            }
            let Some(bounds) = shape.placed.fat else {
                continue;
            };
            let range = columns(bounds.low.map(f64::from), bounds.high.map(f64::from))
                .ok_or(ContactWorldError::InvalidGeometry)?;
            grid.insert(id, range)
                .map_err(|_| ContactWorldError::IndexLimit)?;
        }
        Ok(Self {
            shapes,
            grid: grid.build(),
            moving,
            switches: (0..groups).map(|_| AtomicBool::new(false)).collect(),
            groups: ranges.into(),
            epoch: AtomicU64::new(0),
        })
    }

    /// This world with groups of shapes the match switches on and off,
    /// after its own shapes in group order: group `n` is `groups[n]`, off
    /// at first (see the module notes).
    pub fn with_switched(self, groups: Vec<Vec<StaticShape>>) -> Result<Self, ContactWorldError> {
        let count = groups.len();
        let mut shapes = self.shapes;
        for (group, members) in groups.into_iter().enumerate() {
            shapes.extend(members.into_iter().map(|shape| StaticShape {
                switch: Some(group),
                ..shape
            }));
        }
        Self::build(shapes, count)
    }

    /// The ids of a switched group's shapes, in its order.
    pub fn group_shapes(&self, group: usize) -> Range<u32> {
        self.groups[group].clone()
    }

    /// Put moving shapes ([`StaticShape::moving`]) at new frames; whether
    /// any moved. Bodies take the new places at their next pair update.
    pub fn place(
        &self,
        moves: impl IntoIterator<Item = (u32, ContactFrame)>,
    ) -> Result<bool, ContactWorldError> {
        let mut moved = false;
        for (id, frame) in moves {
            let shape = self
                .shapes
                .get(id as usize)
                .ok_or(ContactWorldError::InvalidGeometry)?;
            let cell = shape
                .moving
                .as_ref()
                .ok_or(ContactWorldError::InvalidGeometry)?;
            if shape.frame() == Some(frame) {
                continue;
            }
            let placed = Placed::new(&shape.geometry, Some(frame))?;
            *cell.write().unwrap_or_else(PoisonError::into_inner) = placed;
            moved = true;
        }
        if moved {
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
        Ok(moved)
    }

    /// Turn a switched group on or off; whether that changed it.
    pub fn switch(&self, group: usize, on: bool) -> bool {
        let changed = self.switches[group].swap(on, Ordering::AcqRel) != on;
        if changed {
            self.epoch.fetch_add(1, Ordering::AcqRel);
        }
        changed
    }

    /// Whether a switched group is on.
    pub fn is_on(&self, group: usize) -> bool {
        self.switches[group].load(Ordering::Acquire)
    }

    /// Counts the switch changes and the moves of moving shapes.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    /// Whether a shape is in the world now: not switched, or switched on.
    pub(crate) fn shape_on(&self, id: u32) -> bool {
        self.shapes[id as usize]
            .switch
            .is_none_or(|group| self.is_on(group))
    }

    /// Consume the movement cache and retain only the selected placement groups.
    pub fn from_movement_cache(
        cache: CollisionCache,
        identifiers: &BTreeSet<u64>,
    ) -> Result<Self, ContactWorldError> {
        cache.header.check_movement()?;
        let groups = cache.header.placement_groups(identifiers);
        let mut shapes = Vec::new();
        for hull in cache.hulls {
            shapes.push(StaticShape::new(
                StaticGeometry::Hull(Arc::new(hull)),
                None,
            )?);
        }
        for mesh in cache.meshes {
            let mesh = Arc::new(ContactMesh::new(mesh, [1.0; 3], u16::MAX)?);
            shapes.push(StaticShape::new(StaticGeometry::Mesh(mesh), None)?);
        }
        let mut scaled = HashMap::new();
        let mut raw_meshes = HashMap::<(u32, usize), Arc<Mesh>>::new();
        for placement in cache.placements {
            if !groups.contains(&placement.group) {
                continue;
            }
            let model = cache
                .models
                .get(placement.model as usize)
                .ok_or(ContactWorldError::InvalidGeometry)?;
            let frame = ContactFrame {
                translation: placement.translation,
                rotation: placement.rotation,
            };
            for (slot, shape) in model.iter().enumerate() {
                let key = (placement.model, slot, placement.scale.map(f32::to_bits));
                let geometry = match scaled.entry(key) {
                    Entry::Occupied(found) => found.into_mut(),
                    Entry::Vacant(slot_entry) => {
                        slot_entry.insert(placed_geometry(shape, placement, slot, &mut raw_meshes)?)
                    }
                };
                shapes.push(StaticShape::new(geometry.clone(), Some(frame))?);
            }
        }
        Self::new(shapes)
    }

    pub fn shapes(&self) -> &[StaticShape] {
        &self.shapes
    }

    /// Find shapes whose fat bounds overlap the box, in proxy order.
    pub fn query(&self, bounds: ContactBounds) -> Vec<u32> {
        self.query_precise(bounds.low.map(f64::from), bounds.high.map(f64::from))
    }

    fn query_precise(&self, low: [f64; 3], high: [f64; 3]) -> Vec<u32> {
        if !valid_box(low, high) {
            return Vec::new();
        }
        let Some(range) = columns(low, high) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let mut seen = FastSet::default();
        let on = |shape: &StaticShape| shape.switch.is_none_or(|group| self.is_on(group));
        self.grid.visit(range.expanded(1), |id| {
            let shape = &self.shapes[id as usize];
            if shape
                .placed
                .fat
                .is_some_and(|bounds| overlaps(low, high, bounds))
                && on(shape)
                && seen.insert(id)
            {
                found.push(id);
            }
        });
        for &id in &self.moving {
            let shape = &self.shapes[id as usize];
            if shape
                .fat_bounds()
                .is_some_and(|bounds| overlaps(low, high, bounds))
                && on(shape)
            {
                found.push(id);
            }
        }
        found.sort_unstable();
        found
    }

    /// Sweep the probe through all applicable static shapes, in proxy order.
    pub fn scene_sweep(&self, probe: MeshProbe<'_>) -> Result<Option<WorldHit>, PointSetSize> {
        if probe.points.is_empty() || probe.points.len() > usize::from(u8::MAX) {
            return Err(PointSetSize);
        }
        let translation = sub(probe.end, probe.start);
        let extent = probe
            .points
            .iter()
            .flatten()
            .map(|lane| lane.abs())
            .fold(0.0, f32::max);
        let reach = (f64::from(extent) + f64::from(probe.radius)) + SWEEP_REACH;
        let low = std::array::from_fn(|k| f64::from(probe.start[k].min(probe.end[k])) - reach);
        let high = std::array::from_fn(|k| f64::from(probe.start[k].max(probe.end[k])) + reach);
        let b = CastShape {
            points: probe.points,
            radius: maxps(SKIN, probe.radius),
            frame: Some(
                ContactFrame {
                    translation: probe.start,
                    ..ContactFrame::IDENTITY
                }
                .into(),
            ),
        };
        let mut nearest = None;
        let mut max_fraction = 1.0;
        for id in self.query_precise(low, high) {
            let shape = &self.shapes[id as usize];
            if shape.geometry.mask() & probe.category == 0 {
                continue;
            }
            let placed = shape.placed();
            let options = CastOptions {
                translation,
                max_fraction,
                hit_touching: false,
                shrink: false,
            };
            let cast = |points, radius| {
                shape_cast(
                    CastShape {
                        points,
                        radius,
                        frame: placed.matrix,
                    },
                    b,
                    options,
                )
            };
            let result = match &shape.geometry {
                StaticGeometry::Mesh(mesh) => mesh_sweep(
                    mesh,
                    placed.frame.unwrap_or(ContactFrame::IDENTITY),
                    probe,
                    max_fraction,
                )?
                .map(|hit| (hit.hit, Some(hit.triangle))),
                StaticGeometry::Hull(hull) => cast(&hull.vertices, SKIN)?.map(|hit| (hit, None)),
                StaticGeometry::Round {
                    shape: RoundShape::Capsule(capsule),
                    ..
                } => cast(&[capsule.a, capsule.b], capsule.radius)?.map(|hit| (hit, None)),
                StaticGeometry::Round {
                    shape: RoundShape::Sphere(sphere),
                    ..
                } => cast(&[sphere.center], sphere.radius)?.map(|hit| (hit, None)),
            };
            if let Some((hit, triangle)) = result {
                max_fraction = hit.fraction;
                nearest = Some(WorldHit {
                    hit,
                    shape: id,
                    triangle,
                });
            }
        }
        Ok(nearest)
    }

    /// The highest ground under `(x, z)` between `top` and `bottom` that a
    /// vertical ray meets among the switched shapes standing now (the
    /// server placeables' models, which the triangle index of the map
    /// lacks), with the shape; meshes and polytopes (the client's probe
    /// rays meet every shape of its physics world).
    pub fn ground(
        &self,
        x: f64,
        z: f64,
        top: f64,
        bottom: f64,
        category: u16,
    ) -> Option<(GroundHit, u32)> {
        if !(bottom <= top) {
            return None;
        }
        let mut best: Option<(GroundHit, u32)> = None;
        for id in self.query_precise([x, bottom, z], [x, top, z]) {
            let shape = &self.shapes[id as usize];
            if shape.switch.is_none() || shape.geometry.mask() & category == 0 {
                continue;
            }
            let frame = shape.frame().unwrap_or(ContactFrame::IDENTITY);
            let length = top - bottom;
            let found = match &shape.geometry {
                StaticGeometry::Mesh(mesh) => mesh_ground(mesh, frame, [x, top, z], length, category),
                StaticGeometry::Hull(hull) => hull_ground(hull, frame, [x, top, z], length),
                StaticGeometry::Round { .. } => None,
            };
            let Some((height, normal, surface, mask)) = found else {
                continue;
            };
            if best.is_some_and(|(hit, _)| hit.y >= height) {
                continue;
            }
            let [nx, ny, nz] = normal;
            best = Some((
                GroundHit {
                    y: height,
                    normal,
                    plane_offset: (nx * x + ny * height) + nz * z,
                    mask,
                    surface,
                },
                id,
            ));
        }
        best
    }

    /// The frame of the platform a shape belongs to as it stands now: the
    /// shape's frame after its node's place ([`Platform::node_place`]).
    pub fn platform_frame(&self, shape: u32) -> Option<ContactFrame> {
        let shape = self.shapes.get(shape as usize)?;
        let platform = shape.platform?;
        let frame = shape.frame().unwrap_or(ContactFrame::IDENTITY);
        Some(compose(frame, platform.node_place))
    }

    /// The standing shape of a platform (`entity`, `node`), if any.
    pub fn platform_shape(&self, entity: u32, node: u32) -> Option<u32> {
        (0..self.shapes.len() as u32).find(|&id| {
            self.shape_on(id)
                && self.shapes[id as usize]
                    .platform
                    .is_some_and(|platform| platform.entity == entity && platform.node == node)
        })
    }
}

/// `outer` after `inner`: `inner`'s place carried by `outer`.
pub fn compose(outer: ContactFrame, inner: ContactFrame) -> ContactFrame {
    let [ax, ay, az, aw] = outer.rotation;
    let [bx, by, bz, bw] = inner.rotation;
    ContactFrame {
        translation: outer.place(inner.translation),
        rotation: [
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz,
        ],
    }
}

/// A downward ray from `origin` over `length` against a mesh in `frame`:
/// the highest triangle of the category it meets, as (height, world
/// normal, surface, mask).
fn mesh_ground(
    mesh: &ContactMesh,
    frame: ContactFrame,
    origin: [f64; 3],
    length: f64,
    category: u16,
) -> Option<(f64, [f64; 3], u16, u16)> {
    let (start, direction) = local_ray(frame, origin, length);
    let end: [f64; 3] = std::array::from_fn(|k| start[k] + direction[k]);
    let low = std::array::from_fn(|k| start[k].min(end[k]) as f32);
    let high = std::array::from_fn(|k| start[k].max(end[k]) as f32);
    let mut best: Option<(f64, [f64; 3], u16, u16)> = None;
    for number in mesh.query(low, high, category) {
        let triangle = mesh.triangle(number)?;
        if triangle.mask & category == 0 {
            continue;
        }
        let [a, b, c] = triangle.corners().map(|corner| corner.map(f64::from));
        let Some(fraction) = ray_triangle(start, direction, a, b, c) else {
            continue;
        };
        if best.is_some_and(|(found, ..)| found <= fraction) {
            continue;
        }
        let normal = rotate64(frame.rotation.map(f64::from), triangle.normal().map(f64::from));
        best = Some((fraction, normal, triangle.surface, triangle.mask));
    }
    best.map(|(fraction, normal, surface, mask)| {
        (origin[1] - fraction * length, unit64(normal), surface, mask)
    })
}

/// A downward ray from `origin` over `length` against a polytope in
/// `frame` (planes `n · p <= w` inside): where it enters, as (height,
/// world normal of the face it enters, surface, mask).
fn hull_ground(
    hull: &Hull,
    frame: ContactFrame,
    origin: [f64; 3],
    length: f64,
) -> Option<(f64, [f64; 3], u16, u16)> {
    let (start, direction) = local_ray(frame, origin, length);
    let (mut enter, mut exit) = (0.0f64, 1.0f64);
    let mut face = None;
    for plane in &hull.planes {
        let normal = [plane[0], plane[1], plane[2]].map(f64::from);
        let along = dot64(normal, direction);
        let outside = dot64(normal, start) - f64::from(plane[3]);
        if along.abs() < 1e-12 {
            if outside > 0.0 {
                return None;
            }
            continue;
        }
        let fraction = -outside / along;
        if along < 0.0 {
            if fraction > enter {
                enter = fraction;
                face = Some(normal);
            }
        } else if fraction < exit {
            exit = fraction;
        }
        if enter > exit {
            return None;
        }
    }
    let normal = face?;
    let normal = unit64(rotate64(frame.rotation.map(f64::from), normal));
    Some((origin[1] - enter * length, normal, 0, hull.mask))
}

/// A world ray straight down from `origin` over `length` in a frame's
/// local space: its start and its whole travel.
fn local_ray(frame: ContactFrame, origin: [f64; 3], length: f64) -> ([f64; 3], [f64; 3]) {
    let [x, y, z, w] = frame.rotation.map(f64::from);
    let back = [-x, -y, -z, w];
    let offset = std::array::from_fn(|k| origin[k] - f64::from(frame.translation[k]));
    (rotate64(back, offset), rotate64(back, [0.0, -length, 0.0]))
}

/// The fraction of `travel` from `start` at which it crosses a triangle,
/// from either side.
fn ray_triangle(
    start: [f64; 3],
    travel: [f64; 3],
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
) -> Option<f64> {
    let sub = |p: [f64; 3], q: [f64; 3]| -> [f64; 3] { std::array::from_fn(|k| p[k] - q[k]) };
    let (ab, ac) = (sub(b, a), sub(c, a));
    let p = cross64(travel, ac);
    let det = dot64(ab, p);
    if det.abs() < 1e-15 {
        return None;
    }
    let to = sub(start, a);
    let u = dot64(to, p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross64(to, ab);
    let v = dot64(travel, q) / det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let fraction = dot64(ac, q) / det;
    (0.0..=1.0).contains(&fraction).then_some(fraction)
}

fn dot64(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross64(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit64(v: [f64; 3]) -> [f64; 3] {
    let length = dot64(v, v).sqrt();
    v.map(|value| value / length)
}

fn rotate64([x, y, z, w]: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let t = cross64([x, y, z], v).map(|value| value * 2.0);
    let u = cross64([x, y, z], t);
    std::array::from_fn(|k| v[k] + w * t[k] + u[k])
}

fn placed_geometry(
    shape: &Shape,
    placement: Placement,
    slot: usize,
    raw_meshes: &mut HashMap<(u32, usize), Arc<Mesh>>,
) -> Result<StaticGeometry, ContactWorldError> {
    match shape {
        Shape::Mesh { mask, mesh } => {
            let raw = raw_meshes
                .entry((placement.model, slot))
                .or_insert_with(|| Arc::new(mesh.clone()));
            Ok(StaticGeometry::Mesh(Arc::new(ContactMesh::new(
                Arc::clone(raw),
                placement.scale,
                *mask,
            )?)))
        }
        _ => Ok(scaled_solid(shape, placement.scale).expect("only solid shapes remain")),
    }
}

/// A model's polytope, capsule or sphere scaled as the movement cache
/// scales a placed prop's ([`ContactWorld::from_movement_cache`]), for a
/// [`StaticShape`] in the prop's frame; `None` for a mesh (a
/// [`ContactMesh`] scales it).
pub fn scaled_solid(shape: &Shape, scale: [f32; 3]) -> Option<StaticGeometry> {
    match shape {
        Shape::Mesh { .. } => None,
        Shape::Hull(hull) => {
            let mut hull = hull.clone();
            if scale != [1.0; 3] {
                hull.centroid = mul(hull.centroid, scale);
                for vertex in &mut hull.vertices {
                    *vertex = mul(*vertex, scale);
                }
                for plane in &mut hull.planes {
                    plane[3] *= scale[0];
                }
            }
            Some(StaticGeometry::Hull(Arc::new(hull)))
        }
        Shape::Capsule { .. } | Shape::Sphere { .. } => {
            let placement = Placement {
                model: 0,
                group: 0,
                translation: [0.0; 3],
                scale,
                rotation: [0.0, 0.0, 0.0, 1.0],
            };
            let obstacle = RoundObstacle::from_placement(shape, placement)?;
            Some(StaticGeometry::Round {
                shape: obstacle.shape,
                mask: obstacle.mask,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::cache::Triangle;
    use crate::contact::round::{Capsule, Sphere};
    use serde_json::json;

    const DISC: [Vec3; 5] = [
        [0.4, 0.015, 0.0],
        [0.0, 0.015, 0.4],
        [-0.4, 0.015, 0.0],
        [0.0, 0.015, -0.4],
        [0.0; 3],
    ];

    fn cube() -> Hull {
        Hull {
            mask: 1,
            centroid: [-2.0, 0.6, 0.0],
            vertices: vec![
                [-2.3, 0.4, -0.3],
                [-2.3, 0.4, 0.3],
                [-2.3, 0.8, -0.3],
                [-2.3, 0.8, 0.3],
                [-1.7, 0.4, -0.3],
                [-1.7, 0.4, 0.3],
                [-1.7, 0.8, -0.3],
                [-1.7, 0.8, 0.3],
            ],
            planes: vec![],
            faces: vec![],
            half_edges: vec![],
        }
    }

    fn sphere(center: Vec3, radius: f32, mask: u16) -> StaticGeometry {
        StaticGeometry::Round {
            shape: RoundShape::Sphere(Sphere { center, radius }),
            mask,
        }
    }

    #[test]
    fn scene_cast_dispatches_round_and_hull_shapes_and_preserves_first_tie() {
        let geometry = [
            sphere([0.0, 1.0, 0.0], 0.4, 1),
            sphere([0.0, 1.0, 0.0], 0.4, 1),
            StaticGeometry::Round {
                shape: RoundShape::Capsule(Capsule {
                    a: [2.0, 0.5, 0.0],
                    b: [2.0, 1.0, 0.0],
                    radius: 0.3,
                }),
                mask: 2,
            },
            StaticGeometry::Hull(Arc::new(cube())),
        ];
        let world = ContactWorld::new(
            geometry
                .into_iter()
                .map(|geometry| StaticShape::new(geometry, None).unwrap())
                .collect(),
        )
        .unwrap();
        let sweep = |x, category| {
            world
                .scene_sweep(MeshProbe {
                    points: &DISC,
                    radius: 0.015,
                    start: [x, 2.0, 0.0],
                    end: [x, -1.0, 0.0],
                    category,
                })
                .unwrap()
        };
        for (x, category, shape, expected) in [
            (
                0.0,
                1,
                0,
                [0x3e4b_17e5, 0, 0x3fb3_3333, 0, 0, 0x3f80_0000, 0],
            ),
            (
                2.0,
                2,
                2,
                [0x3e6d_3a06, 0x4000_0000, 0x3fa6_6666, 0, 0, 0x3f80_0000, 0],
            ),
            (
                -2.0,
                1,
                3,
                [
                    0x3eca_3d71,
                    0xc000_0000,
                    0x3f4f_5c28,
                    0,
                    0,
                    0x3f7f_ffff,
                    0x8000_0000,
                ],
            ),
        ] {
            let result = sweep(x, category).unwrap();
            assert_eq!((result.shape, result.triangle), (shape, None));
            assert_eq!(result.hit.fraction.to_bits(), expected[0]);
            assert_eq!(result.hit.point.map(f32::to_bits), expected[1..4]);
            assert_eq!(result.hit.normal.map(f32::to_bits), expected[4..7]);
        }
        assert!(sweep(0.0, 2).is_none());
    }

    #[test]
    fn broadphase_keeps_oversized_proxies_and_signed_zero_overlap_rule() {
        let world = ContactWorld::new(vec![
            StaticShape::new(sphere([0.0; 3], 150.0, 1), None).unwrap(),
            StaticShape::new(sphere([100.0, 0.0, 0.0], 0.2, 1), None).unwrap(),
        ])
        .unwrap();
        let q = ContactBounds {
            low: [99.0, -1.0, -1.0],
            high: [101.0, 1.0, 1.0],
        };
        assert_eq!(world.query(q), [0, 1]);
        assert_eq!(
            world.query(ContactBounds {
                low: [-180.0; 3],
                high: [180.0; 3]
            }),
            [0, 1]
        );
        assert!(
            world
                .query(ContactBounds {
                    low: [200.0; 3],
                    high: [201.0; 3]
                })
                .is_empty()
        );
        let zero = ContactBounds {
            low: [-1.0; 3],
            high: [-0.0, 1.0, 1.0],
        };
        assert!(!overlaps([0.0; 3], [1.0; 3], zero));
        assert!(overlaps([-0.0, 0.0, 0.0], [1.0; 3], zero));
    }

    #[test]
    fn a_moving_shape_is_found_where_it_was_last_placed() {
        let world = ContactWorld::new(vec![
            StaticShape::new(sphere([0.0; 3], 0.5, 1), None).unwrap(),
        ])
        .unwrap()
        .with_switched(vec![vec![
            StaticShape::new(sphere([0.0; 3], 0.5, 1), Some(ContactFrame::IDENTITY))
                .unwrap()
                .moving(),
        ]])
        .unwrap();
        world.switch(0, true);
        let at = |x: f32| ContactBounds {
            low: [x - 0.1, -0.1, -0.1],
            high: [x + 0.1, 0.1, 0.1],
        };
        assert_eq!(world.group_shapes(0), 1..2);
        assert_eq!(world.query(at(0.0)), [0, 1]);
        let epoch = world.epoch();
        let frame = ContactFrame {
            translation: [30.0, 0.0, 0.0],
            ..ContactFrame::IDENTITY
        };
        assert!(world.place([(1, frame)]).unwrap());
        assert_eq!(world.epoch(), epoch + 1);
        assert_eq!(world.query(at(0.0)), [0]);
        assert_eq!(world.query(at(30.0)), [1]);
        // The same place again moves nothing; a fixed shape can't be moved.
        assert!(!world.place([(1, frame)]).unwrap());
        assert_eq!(world.epoch(), epoch + 1);
        assert!(world.place([(0, frame)]).is_err());
    }

    #[test]
    fn movement_cache_keeps_shape_order_groups_and_model_scale_reuse() {
        let mesh = Mesh {
            vertices: vec![[0.0; 3], [0.0, 0.0, 2.0], [2.0, 0.0, 0.0]],
            triangles: vec![Triangle {
                corners: [0, 1, 2],
                wings: [-1; 3],
                mask: 1,
                surface: 0,
            }],
        };
        let placement = |group, x| Placement {
            model: 0,
            group,
            translation: [x, 0.0, 0.0],
            scale: [1.25, 0.75, 1.5],
            rotation: ContactFrame::IDENTITY.rotation,
        };
        let cache = CollisionCache {
            header: serde_json::from_value(json!({
                "meshes": [[3, 1]], "hulls": 1, "models": 1, "placements": 2,
                "identifiers": ["AB"],
            }))
            .unwrap(),
            meshes: vec![mesh.clone()],
            hulls: vec![cube()],
            models: vec![vec![Shape::Mesh { mesh, mask: 1 }]],
            placements: vec![placement(0, 5.0), placement(1, 10.0)],
        };
        let core = ContactWorld::from_movement_cache(cache.clone(), &BTreeSet::new()).unwrap();
        let mode = ContactWorld::from_movement_cache(cache, &BTreeSet::from([0xab])).unwrap();
        assert_eq!((core.shapes().len(), mode.shapes().len()), (3, 4));
        let [hull, map, prop_a, prop_b] = mode.shapes() else {
            panic!("expected all four shapes");
        };
        assert!(matches!(hull.geometry(), StaticGeometry::Hull(_)));
        assert!(matches!(map.geometry(), StaticGeometry::Mesh(_)));
        assert!(hull.frame().is_none() && map.frame().is_none());
        let (StaticGeometry::Mesh(a), StaticGeometry::Mesh(b)) =
            (prop_a.geometry(), prop_b.geometry())
        else {
            panic!("expected both mesh props");
        };
        assert!(Arc::ptr_eq(a, b));
        assert_eq!(prop_b.frame().unwrap().translation, [10.0, 0.0, 0.0]);
    }
}
