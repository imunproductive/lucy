//! Upright character capsule with ordered static pairs and bounded contacts.
//!
//! Ports `CharacterBody` from `ow174/game/contacts.py`. The immutable
//! world is shared. Candidate lifetimes and warm caches are per body.
//!
//! Client functions: the character move 0x7FF789CCABF0 -> 0x7FF789CC9320
//! (mover +0x14E4 holds the rounds the next move may take). Shape AABB
//! 0x7FF789CEBA90, swept AABB 0x7FF789CEBC60. The proxy's fat AABB follows
//! when the shape leaves it or it gets four margins too loose (0x7FF789CD65F0
//! -> 0x7FF789CDD820). New pairs 0x7FF789CF5800, each at the head of the
//! list; pair filter 0x7FF789D07730. Teleport 0x7FF789D11B60 ->
//! 0x7FF789D10BD0. Stance capsule change 0x7FF789CCAC00 -> 0x7FF789D072F0
//! (contacts stay). Stance fit test 0x7FF789CCAC10 -> 0x7FF789CCA300 with
//! overlap query 0x7FF789D24DC0: no shape within max(0.005, r - 0.02) of the
//! segment; per shape 0x7FF789D05710 (mesh triangles 0x7FF789CF44A0 ->
//! 0x7FF789CEDCF0, polytopes 0x7FF789CEA060, capsules 0x7FF789CDC480, spheres
//! 0x7FF789CDC7F0). Move toward a target 0x7FF789D102D0.
//!
//! Other characters ([`CharacterBody::set_peers`]): the client registers a
//! remote body's capsule as a proxy of its team's category (0x08, 0x10).
//! The local mover's mask (`0x7FF789B463F0`) is 0x1499, both team
//! categories, with its own team's bit cleared when component 31's byte
//! `+0x14DF` is 0 (`STU_6CC90892` clears it while active) or its protected
//! byte `+0x14DA` says so: that is how allies drop out (refs report
//! PlayerHurt.md, "Client tools pass 2 (2026-11-16)"). The server has no
//! such proxies in its static world: a move takes the capsules it is given
//! as round obstacles (the caller passes enemies only), as upstream's
//! `mccree.py` `step_rialto_pose` does with `contacts.py` `PeerShape`.

use std::{collections::HashSet, fmt, sync::Arc};

use crate::{
    contact::cache::{CapsulePose, LOOSE, ProxyContact, encloses},
    contact::gjk::{PointSetSize, SimplexCache, gjk, sweep::MeshProbe},
    contact::hull::HullContactError,
    contact::math::{TINY, Vec3, add, dot, maxps, minps, scale, sub},
    contact::mesh::ContactBounds,
    contact::round::{Capsule, RoundShape, splat_tiny},
    contact::solver::{Constraint, ContactPlane, project_target},
    contact::triangle::{SKIN, TOUCH, closest_parameters},
    contact::world::{ContactWorld, PROXY_MARGIN, StaticGeometry, StaticShape, WorldHit, overlaps},
};

const MAX_ROUNDS: u32 = 20;
const FIT_SHRINK: f32 = 0.02;
/// Another character's capsule takes both team proxy categories as its
/// mask (`MOVER` in `ow174/game/collision.py`).
const PEER_MASK: u16 = 0x18;

pub struct BodyMove<'a> {
    pub position: Vec3,
    pub planes: &'a [ContactPlane],
    pub touching: bool,
}

pub struct CharacterBody {
    world: Arc<ContactWorld>,
    pose: CapsulePose,
    category: u16,
    fat: ContactBounds,
    proxy_moved: bool,
    contacts: Vec<(u32, ProxyContact)>,
    pairs: HashSet<u32>,
    /// The capsules of the other characters that block the moves
    /// ([`CharacterBody::set_peers`]).
    peer_capsules: Vec<Capsule>,
    /// Their contacts during a move, with their fat AABBs.
    peers: Vec<(ContactBounds, ProxyContact)>,
    budget: u32,
    constraints: Vec<Constraint>,
    planes: Vec<ContactPlane>,
}

impl fmt::Debug for CharacterBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CharacterBody")
            .field("position", &self.pose.position)
            .field("category", &self.category)
            .finish_non_exhaustive()
    }
}

impl CharacterBody {
    pub fn new(world: Arc<ContactWorld>, pose: CapsulePose, category: u16) -> Self {
        Self {
            world,
            pose,
            category,
            fat: expanded(pose.bounds(), PROXY_MARGIN),
            proxy_moved: true,
            contacts: Vec::new(),
            pairs: HashSet::new(),
            peer_capsules: Vec::new(),
            peers: Vec::new(),
            budget: MAX_ROUNDS,
            constraints: Vec::new(),
            planes: Vec::new(),
        }
    }

    pub fn position(&self) -> Vec3 {
        self.pose.position
    }

    /// The capsule where it stands now.
    pub fn pose(&self) -> CapsulePose {
        self.pose
    }

    /// The capsules of the other characters that block this body's moves
    /// from now on, in place of the last ones (none at first). Each move
    /// takes them as static obstacles, fresh contacts at the head of its
    /// contact list, ahead of the map's shapes, and drops them when it ends
    /// (upstream `mccree.py` `step_rialto_pose` adds its NPCs' capsules so
    /// for each pose and takes them out after it). A move drops one sooner
    /// once its fat AABB leaves the body's, as the map's. The ground probe,
    /// the sweeps and the stance fit test leave them out: the body cannot
    /// stand on another character, and nothing stops a stance change.
    pub fn set_peers(&mut self, peers: impl IntoIterator<Item = Capsule>) {
        self.peer_capsules.clear();
        self.peer_capsules.extend(peers);
    }

    pub fn radius(&self) -> f32 {
        self.pose.radius
    }

    pub fn world(&self) -> &ContactWorld {
        &self.world
    }

    pub fn fresh_at(&self, position: Vec3) -> Self {
        self.fresh(CapsulePose {
            position,
            ..self.pose
        })
    }

    /// A new body in this one's world, of its mask, with the capsule
    /// `pose` (a temporary character, `0x7FF789CCAB30`).
    pub fn fresh(&self, pose: CapsulePose) -> Self {
        Self::new(Arc::clone(&self.world), pose, self.category)
    }

    /// Query the same upright capsule and mask without changing its contact state.
    pub fn sweep(&self, start: Vec3, end: Vec3) -> Result<Option<WorldHit>, PointSetSize> {
        self.world.scene_sweep(MeshProbe {
            points: &[[0.0, self.pose.radius, 0.0], [0.0, self.pose.top, 0.0]],
            radius: self.pose.radius,
            start,
            end,
            category: self.category,
        })
    }

    pub fn teleport(&mut self, position: Vec3) {
        if self.pose.position != position {
            self.pose.position = position;
            self.move_proxy(self.pose.bounds());
        }
    }

    /// A new capsule size without the fit test, its contacts kept (the
    /// client's stance capsule change `0x7FF789CCAC00`, as a `MoveTo`'s
    /// free-spot search grows its capsule, `0x7FF789C4A8A0`).
    pub fn reshape(&mut self, radius: f32, top: f32) {
        self.pose = CapsulePose {
            radius,
            top,
            ..self.pose
        };
        self.move_proxy(self.pose.bounds());
    }

    pub fn try_resize(&mut self, radius: f32, top: f32) -> Result<bool, HullContactError> {
        if !self.fits(radius, top)? {
            return Ok(false);
        }
        self.pose = CapsulePose {
            radius,
            top,
            ..self.pose
        };
        self.move_proxy(self.pose.bounds());
        self.update_pairs();
        Ok(true)
    }

    pub fn fits(&self, radius: f32, top: f32) -> Result<bool, HullContactError> {
        if radius
            .partial_cmp(&SKIN)
            .is_none_or(std::cmp::Ordering::is_lt)
        {
            return Ok(false);
        }
        let reach = maxps(TOUCH, radius - FIT_SHRINK);
        let capsule = CapsulePose {
            radius,
            top,
            ..self.pose
        }
        .world_capsule();
        let bounds = segment_bounds(capsule.a, capsule.b, reach);
        for id in self.world.query(bounds) {
            let shape = &self.world.shapes()[id as usize];
            if shape.geometry().mask() & self.category != 0
                && overlaps_segment(shape, [capsule.a, capsule.b], reach, self.category)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn move_capsule(
        &mut self,
        displacement: Vec3,
        dt: f32,
    ) -> Result<BodyMove<'_>, HullContactError> {
        let rounds = self.budget;
        let (current, passes_total) = self.move_rounds(displacement, dt, rounds)?;
        self.budget = if passes_total > (rounds * 100).min(500) {
            (self.budget / 2).max(1)
        } else {
            (self.budget * 2).min(MAX_ROUNDS)
        };
        self.planes.clear();
        let mut touching = false;
        for constraint in &self.constraints {
            if constraint.impulse > 0.0 {
                touching = true;
                if constraint.impulse < f32::MAX {
                    self.planes.push(constraint.plane);
                }
            }
        }
        Ok(BodyMove {
            position: current,
            planes: &self.planes,
            touching,
        })
    }

    /// A move of at most `rounds` rounds that leaves the round budget as it
    /// is: a temporary character's, whose caller names the rounds (a
    /// `MoveTo`'s free spot passes 5, `0x7FF789C4A8A0`). Where it stops.
    pub fn move_fixed(
        &mut self,
        displacement: Vec3,
        rounds: u32,
    ) -> Result<Vec3, HullContactError> {
        Ok(self.move_rounds(displacement, 0.0, rounds)?.0)
    }

    /// The character move `0x7FF789CC9320`: up to `rounds` rounds (its
    /// request's `+0x2C`), each projecting the target onto the contact
    /// planes and moving there, until a round moves less than the skin
    /// ([`TOUCH`], the request's `+0x28`). Where it stops and the solver
    /// passes it took.
    fn move_rounds(
        &mut self,
        displacement: Vec3,
        dt: f32,
        rounds: u32,
    ) -> Result<(Vec3, u32), HullContactError> {
        let mut current = self.pose.position;
        let target = add(add(scale([0.0; 3], dt), current), displacement);
        let mut passes_total = 0;
        self.peers.clear();
        for &capsule in &self.peer_capsules {
            let geometry = StaticGeometry::Round {
                shape: RoundShape::Capsule(capsule),
                mask: PEER_MASK,
            };
            let Ok(shape) = StaticShape::new(geometry, None) else {
                continue;
            };
            if let Some(fat) = shape.fat_bounds() {
                self.peers
                    .push((fat, ProxyContact::new(&shape, self.category)));
            }
        }
        for _ in 0..rounds {
            self.update_pairs();
            self.constraints.clear();
            let peers = self.peers.iter_mut().map(|(_, contact)| contact);
            for contact in peers.chain(self.contacts.iter_mut().map(|(_, contact)| contact)) {
                let sign = contact.normal_sign();
                let manifolds = contact.manifolds(self.pose)?;
                for (normal, points) in manifolds.iter() {
                    let normal = if sign > 0.0 {
                        normal
                    } else {
                        normal.map(|axis| -axis)
                    };
                    let deepest = points[1..]
                        .iter()
                        .fold(points[0].separation, |depth, point| {
                            minps(depth, point.separation)
                        });
                    let offset = dot(normal, current) - deepest;
                    self.constraints
                        .push(ContactPlane { normal, offset }.into());
                }
            }
            let (projected, passes) = project_target(target, &mut self.constraints);
            passes_total += passes;
            let moved = self.set_position(projected)?;
            let step = sub(moved, current);
            current = moved;
            if dot(step, step) < TOUCH * TOUCH {
                break;
            }
        }
        self.peers.clear();
        Ok((current, passes_total))
    }

    fn move_proxy(&mut self, bounds: ContactBounds) {
        if encloses(self.fat, bounds) && encloses(expanded(bounds, LOOSE), self.fat) {
            return;
        }
        self.fat = expanded(bounds, PROXY_MARGIN);
        self.proxy_moved = true;
    }

    fn update_pairs(&mut self) {
        if !std::mem::take(&mut self.proxy_moved) {
            return;
        }
        let before = self.contacts.len();
        for id in self.world.query(self.fat) {
            let shape = &self.world.shapes()[id as usize];
            if shape.geometry().mask() & self.category != 0 && self.pairs.insert(id) {
                self.contacts
                    .push((id, ProxyContact::new(shape, self.category)));
            }
        }
        let new = self.contacts.len() - before;
        if new != 0 {
            self.contacts[before..].reverse();
            self.contacts.rotate_right(new);
        }
    }

    fn set_position(&mut self, target: Vec3) -> Result<Vec3, HullContactError> {
        let swept = self.pose.swept_bounds(target);
        self.move_proxy(swept);
        self.update_pairs();
        let translation = sub(target, self.pose.position);
        let low = self.fat.low.map(f64::from);
        let high = self.fat.high.map(f64::from);
        self.peers
            .retain(|(bounds, _)| overlaps(low, high, *bounds));
        self.contacts.retain(|(id, _)| {
            let bounds = self.world.shapes()[*id as usize].fat_bounds();
            let keep = bounds.is_some_and(|bounds| overlaps(low, high, bounds));
            if !keep {
                self.pairs.remove(id);
            }
            keep
        });
        let mut fraction = 1.0;
        if translation != [0.0; 3] {
            let peers = self.peers.iter_mut().map(|(_, contact)| contact);
            for contact in peers.chain(self.contacts.iter_mut().map(|(_, contact)| contact)) {
                if let Some(candidate) = contact.cast(self.pose, swept, translation)? {
                    fraction = minps(fraction, candidate);
                }
            }
        }
        let moved = add(scale(translation, fraction), self.pose.position);
        if moved.into_iter().all(f32::is_finite) {
            self.teleport(moved);
        }
        Ok(self.pose.position)
    }
}

fn expanded(bounds: ContactBounds, margin: f32) -> ContactBounds {
    ContactBounds {
        low: sub(bounds.low, [margin; 3]),
        high: add(bounds.high, [margin; 3]),
    }
}

fn segment_bounds(a: Vec3, b: Vec3, reach: f32) -> ContactBounds {
    ContactBounds {
        low: std::array::from_fn(|k| minps(a[k], b[k]) - reach),
        high: std::array::from_fn(|k| maxps(a[k], b[k]) + reach),
    }
}

fn overlaps_segment(
    shape: &StaticShape,
    segment: [Vec3; 2],
    reach: f32,
    category: u16,
) -> Result<bool, HullContactError> {
    let [a, b] = shape
        .frame()
        .map_or(segment, |frame| segment.map(|point| frame.local(point)));
    let near = |vertices: &[Vec3]| {
        gjk(vertices, &[a, b], SimplexCache::EMPTY, None).map(|distance| distance.distance < reach)
    };
    let overlaps = match shape.geometry() {
        StaticGeometry::Mesh(mesh) => {
            let bounds = segment_bounds(a, b, reach);
            for id in mesh.query(bounds.low, bounds.high, category) {
                let triangle = mesh.triangle(id).expect("candidate from same mesh");
                if near(triangle.corners())? {
                    return Ok(true);
                }
            }
            false
        }
        StaticGeometry::Hull(hull) => near(&hull.vertices)?,
        StaticGeometry::Round {
            shape: RoundShape::Capsule(capsule),
            ..
        } => {
            let total = capsule.radius + reach;
            let u = sub(capsule.b, capsule.a);
            let v = sub(b, a);
            if !splat_tiny(dot(u, u)) && !splat_tiny(dot(v, v)) {
                let d = sub(capsule.a, a);
                dot(d, d) < total
            } else {
                let (first, second) = closest_parameters(capsule.a, capsule.b, a, b);
                let d = sub(second, first);
                dot(d, d) < total * total
            }
        }
        StaticGeometry::Round {
            shape: RoundShape::Sphere(sphere),
            ..
        } => {
            let axis = sub(b, a);
            let along = dot(sub(sphere.center, a), axis);
            let length = dot(axis, axis);
            let point =
                if along <= 0.0 || length.partial_cmp(&TINY) != Some(std::cmp::Ordering::Greater) {
                    a
                } else if length <= along {
                    b
                } else {
                    add(scale(axis, along / length), a)
                };
            let d = sub(sphere.center, point);
            let total = reach + sphere.radius;
            dot(d, d) < total * total
        }
    };
    Ok(overlaps)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{
        collision::cache::{Mesh, Triangle},
        contact::mesh::ContactMesh,
        contact::solver::clip_velocity,
    };

    fn room() -> Arc<ContactWorld> {
        let mesh = Mesh {
            vertices: vec![
                [-10.0, 0.0, -10.0],
                [-10.0, 0.0, 10.0],
                [10.0, 0.0, -10.0],
                [10.0, 0.0, 10.0],
                [-1.8, 0.0, -10.0],
                [-1.8, 4.0, -10.0],
                [-1.8, 0.0, 10.0],
                [-1.8, 4.0, 10.0],
                [-1.8, 0.0, 2.0],
                [-1.8, 4.0, 2.0],
                [10.0, 0.0, 2.0],
                [10.0, 4.0, 2.0],
                [3.0, 1.3, -2.0],
                [5.0, 1.3, -2.0],
                [3.0, 1.3, 2.0],
                [5.0, 1.3, 2.0],
            ],
            triangles: (0..16)
                .step_by(4)
                .flat_map(|first| {
                    [
                        Triangle {
                            corners: [first, first + 1, first + 2],
                            wings: [-1, first + 3, -1],
                            mask: 0x18,
                            surface: 0,
                        },
                        Triangle {
                            corners: [first + 3, first + 2, first + 1],
                            wings: [-1, first, -1],
                            mask: 0x18,
                            surface: 0,
                        },
                    ]
                })
                .collect(),
        };
        let geometry =
            StaticGeometry::Mesh(Arc::new(ContactMesh::new(mesh, [1.0; 3], 0x18).unwrap()));
        Arc::new(ContactWorld::new(vec![StaticShape::new(geometry, None).unwrap()]).unwrap())
    }

    #[test]
    fn capsule_projects_to_floor_and_corner_and_checks_standing_clearance() {
        let pose = CapsulePose {
            position: [0.0, 0.1, 0.0],
            radius: 0.5,
            top: 1.0,
        };
        let mut body = CharacterBody::new(room(), pose, 0x18);
        let mut digest = Sha256::new();
        for input in [
            [-0.85, -0.15, 0.8],
            [-0.8, 0.0, 0.9],
            [-0.9, 0.0, 1.0],
            [0.0; 3],
            [0.5, 0.0, -0.6],
            [-0.75, 0.0, 0.8],
            [0.1, 0.25, -0.3],
            [0.1, -0.4, -0.3],
        ] {
            let result = body.move_capsule(input, 0.016).unwrap();
            for axis in result.position {
                digest.update(axis.to_le_bytes());
            }
            digest.update([u8::from(result.touching), result.planes.len() as u8]);
            for plane in result.planes {
                for axis in plane.normal.into_iter().chain([plane.offset]) {
                    digest.update(axis.to_le_bytes());
                }
            }
            for axis in clip_velocity(input, result.planes) {
                digest.update(axis.to_le_bytes());
            }
        }
        assert_eq!(
            format!("{:x}", digest.finalize()),
            "2ff511de4f4f90c94e11a261e6cf92577172eef72c9f606e99c5896336a4ccbf"
        );
        body.teleport([4.0, 0.0, 0.0]);
        assert!(!body.fits(0.5, 1.0).unwrap());
        assert!(body.try_resize(0.5, 0.55).unwrap());
        assert!(!body.try_resize(0.5, 1.0).unwrap());
        assert!(!body.fits(0.001, 1.0).unwrap());
    }
}
