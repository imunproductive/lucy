//! A mover standing on a moving platform (static reading of the 1.74
//! client, refs `MapEntities.md` "MovingMaps"; addresses at
//! 0x7FF788F10000).
//!
//! The ground probe gives the ground shape's owner as the mover's parent
//! when that entity has component 77 and any of components 28, 39, 27 or
//! 98 (`0x7FF789B4A250`; [`Platform`]), with the skeleton node the shape
//! hangs on (0 for none). The tick's reparent (`0x7FF789B4ADA0`) runs every
//! tick: the parent drops the first tick the probe finds no platform (a
//! jump's next tick, since the probe reaches nothing in the air). While it
//! has a parent the movement state keeps its position in the parent's
//! frame and its velocity relative to the platform in the frame's axes,
//! and the movement records carry the parent (entity and node) with those
//! local values; the owner's client compares all of them exactly
//! (`0x7FF789B73E00`), so a different parent is a correction.
//!
//! The parent's frame (`0x7FF789B71FD0`) is its component 1 transform as
//! posed for the tick (the tick's rewind scope poses it at the command's
//! view time), after the node's place in the model for a node (rows made
//! unit with `rsqrtps`), without scale. Within one tick (`0x7FF789B4B1D0`):
//! the stored local point placed by the frame is where the probe starts,
//! plus `dt · p` (`p` the platform velocity of the tick before); the
//! world velocity is `R · v + p`; the capsule stays where the last tick
//! left it, and the sweep adds the carry `D`, the local point's place now
//! less the capsule's feet (`0x7FF789B46560`: `dt · p` on the tick it
//! takes a new platform, 0 without one); after the move the feet go back
//! into the frame (its inverse by cofactors, `0x7FF789884370`) and the
//! velocity by the transposed rows less `p` (`0x7FF789B76A70`), both then
//! on the 1/1024 grid. The platform velocity `p` (`0x7FF789B6E4E0`) is the
//! local point's move between the platform posed at the view time and
//! `dt` before it, divided by `dt`, times component 77's factor; it is
//! taken again every tick at the reparent. The reparent moves the
//! velocity's numbers between frames without turning them: `v + Rᵀp` of
//! the old platform, then less `Rᵀp` of the new one, so landing on a
//! turned platform turns the world velocity by its turn.
//!
//! The client takes `rsqrtps`/`rcpps` estimates for the frame's rows and
//! its inverse; the port takes them correctly rounded (they differ between
//! CPUs), with the client's Newton steps.

use super::{Mover, posture::snap};
use crate::contact::math::{ContactFrame, Vec3, add, scale, sub};
use crate::contact::world::{ContactWorld, Platform};

/// The platform frames dt before the moving shapes' current poses, which
/// give the platform velocity: the match keeps the clocks it posed the
/// shapes at.
pub trait PlatformClock: Send + Sync {
    /// The frame of `platform` as the client posed it `back` seconds
    /// before the time the world's moving shapes stand at now; `None` for
    /// a platform that doesn't move.
    fn frame_before(&self, platform: &Platform, back: f32) -> Option<ContactFrame>;
}

/// A platform's frame as the mover takes it: three rows (the frame's
/// axes in the world) and its origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix {
    rows: [Vec3; 3],
    origin: Vec3,
}

impl Matrix {
    /// The frame of a platform shape's entity or node as `frame` gives it;
    /// a node's rows made unit as the client makes them.
    pub(crate) fn of(frame: ContactFrame, node: u32) -> Self {
        let [x, y, z, w] = frame.rotation;
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let rows = [
            [1.0 - (y * y2 + z * z2), x * y2 + w * z2, x * z2 - w * y2],
            [x * y2 - w * z2, 1.0 - (x * x2 + z * z2), y * z2 + w * x2],
            [x * z2 + w * y2, y * z2 - w * x2, 1.0 - (x * x2 + y * y2)],
        ];
        let rows = if node == 0 {
            rows
        } else {
            rows.map(|row| {
                let squared = (row[1] * row[1] + row[0] * row[0]) + row[2] * row[2];
                let r = estimate(squared);
                let unit = (3.0 - squared * (r * r)) * (r * 0.5);
                row.map(|value| value * unit)
            })
        };
        Self {
            rows,
            origin: frame.translation,
        }
    }

    /// The rows turning `v`: `(v.y M1 + v.x M0) + v.z M2`.
    pub(crate) fn turn(&self, [x, y, z]: Vec3) -> Vec3 {
        let [m0, m1, m2] = self.rows;
        add(add(scale(m1, y), scale(m0, x)), scale(m2, z))
    }

    /// A local point in the world (`0x7FF789B44500`).
    pub(crate) fn place(&self, point: Vec3) -> Vec3 {
        add(self.turn(point), self.origin)
    }

    /// A world vector in the rows' axes, by the transposed rows
    /// (`0x7FF789B76A70`): `(M_i.y v.y + M_i.x v.x) + M_i.z v.z`.
    pub(crate) fn turn_back(&self, [x, y, z]: Vec3) -> Vec3 {
        self.rows
            .map(|row| (row[1] * y + row[0] * x) + row[2] * z)
    }

    /// Its inverse by cofactors (`0x7FF789884260`, `0x7FF789884370`).
    pub(crate) fn inverse(&self) -> Self {
        let [r0, r1, r2] = self.rows;
        let cofactors = [cross(r1, r2), cross(r2, r0), cross(r0, r1)];
        let c0 = cofactors[0];
        let det = (r0[1] * c0[1] + r0[0] * c0[0]) + r0[2] * c0[2];
        let r = reciprocal(det);
        let inverse = (2.0 - det * r) * r;
        let [c0, c1, c2] = cofactors.map(|c| scale(c, inverse));
        let rows = [
            [c0[0], c1[0], c2[0]],
            [c0[1], c1[1], c2[1]],
            [c0[2], c1[2], c2[2]],
        ];
        let partial = Self {
            rows,
            origin: [0.0; 3],
        };
        Self {
            rows,
            origin: partial.turn(self.origin).map(|value| -value),
        }
    }
}

/// The parent a mover rides and what it keeps in its frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Riding {
    pub(crate) platform: Platform,
    /// The feet in the platform's frame, on the 1/1024 grid.
    pub(crate) local: Vec3,
    /// The velocity relative to the platform, in its axes.
    pub(crate) velocity: Vec3,
    /// The platform velocity (`p`) the tick took.
    pub(crate) carry: Vec3,
}

/// The platform's frame now, if it still stands.
pub(crate) fn frame_now(world: &ContactWorld, platform: &Platform) -> Option<Matrix> {
    let shape = world.platform_shape(platform.entity, platform.node)?;
    world
        .platform_frame(shape)
        .map(|frame| Matrix::of(frame, platform.node))
}

/// The platform velocity of `local` on `platform` (see the module notes),
/// `dt` the tick's length.
pub(crate) fn platform_velocity(
    clock: Option<&dyn PlatformClock>,
    platform: &Platform,
    now: &Matrix,
    local: Vec3,
    dt: f32,
) -> Vec3 {
    if platform.factor == 0.0 {
        return [0.0; 3];
    }
    let Some(before) = clock.and_then(|clock| clock.frame_before(platform, dt)) else {
        return [0.0; 3];
    };
    let before = Matrix::of(before, platform.node);
    let moved = sub(now.place(local), before.place(local));
    scale(moved, platform.factor).map(|value| value / dt)
}

/// A riding mover's start of the tick: the carry to its platform's place
/// now and the probe's shift (`dt · p`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RideStart {
    pub(crate) carry: Vec3,
    pub(crate) shift: Vec3,
}

impl Mover {
    /// Put a riding mover's pose in the world where its platform stands
    /// now (see the module notes); a platform gone drops it, the pose as
    /// it was.
    pub(super) fn ride_in_world(&mut self, dt: f32) -> RideStart {
        let Some(riding) = self.riding else {
            return RideStart::default();
        };
        let Some(matrix) = frame_now(self.body.world(), &riding.platform) else {
            self.riding = None;
            return RideStart::default();
        };
        let placed = matrix.place(riding.local);
        let feet = self.pose.position.map(|axis| axis as f32);
        self.pose.position = placed.map(f64::from);
        self.pose.velocity = add(matrix.turn(riding.velocity), riding.carry).map(f64::from);
        RideStart {
            carry: sub(placed, feet),
            shift: scale(riding.carry, dt),
        }
    }

    /// The reparent (`0x7FF789B4ADA0`): the platform under the feet
    /// (`ground`, the probe's ground shape) becomes the parent, none drops
    /// it; the carry the sweep takes this tick.
    pub(super) fn reparent(&mut self, ground: Option<u32>, start: RideStart, dt: f32) -> Vec3 {
        let world = self.body.world();
        let found = ground.and_then(|shape| world.shapes()[shape as usize].platform());
        let clock = self.platforms.as_deref();
        let old = self.riding.and_then(|riding| {
            let matrix = frame_now(world, &riding.platform)?;
            let carry = platform_velocity(clock, &riding.platform, &matrix, riding.local, dt);
            Some((riding, matrix, carry))
        });
        let same = |platform: &Platform| {
            found.is_some_and(|found| found.entity == platform.entity && found.node == platform.node)
        };
        if let Some((riding, matrix, carry)) = old.filter(|(riding, ..)| same(&riding.platform)) {
            self.riding = Some(Riding { carry, ..riding });
            self.pose.velocity = add(matrix.turn(riding.velocity), carry).map(f64::from);
            return start.carry;
        }
        // The velocity's numbers move between the frames unturned.
        let raw = match old {
            Some((riding, matrix, carry)) => add(riding.velocity, matrix.turn_back(carry)),
            None => self.pose.velocity.map(|axis| axis as f32),
        };
        self.riding = None;
        self.pose.velocity = raw.map(f64::from);
        let Some(platform) = found else {
            return [0.0; 3];
        };
        let Some(matrix) = frame_now(world, &platform) else {
            return [0.0; 3];
        };
        let local = matrix
            .inverse()
            .place(self.pose.position.map(|axis| axis as f32));
        let carry = platform_velocity(clock, &platform, &matrix, local, dt);
        let velocity = sub(raw, matrix.turn_back(carry));
        self.riding = Some(Riding {
            platform,
            local,
            velocity,
            carry,
        });
        self.pose.velocity = add(matrix.turn(velocity), carry).map(f64::from);
        scale(carry, dt)
    }

    /// A riding mover's moved feet (world) back in its platform's frame on
    /// the grid; their place in the world, or `None` when it rides none.
    pub(super) fn place_on_platform(&mut self, feet: Vec3) -> Option<Vec3> {
        let riding = self.riding.as_mut()?;
        let matrix = frame_now(self.body.world(), &riding.platform)?;
        let local = matrix.inverse().place(feet);
        riding.local = local.map(|axis| snap(f64::from(axis)) as f32);
        Some(matrix.place(riding.local))
    }

    /// The moved velocity (world): on a platform, relative to it in its
    /// axes, on the grid; in the world otherwise.
    pub(super) fn settle_velocity(&mut self, velocity: [f64; 3]) {
        let world = velocity.map(|axis| axis as f32);
        let settled = self.riding.as_mut().and_then(|riding| {
            let matrix = frame_now(self.body.world(), &riding.platform)?;
            let local = matrix.turn_back(sub(world, riding.carry));
            riding.velocity = local.map(|axis| snap(f64::from(axis)) as f32);
            Some(add(matrix.turn(riding.velocity), riding.carry))
        });
        self.pose.velocity = match settled {
            Some(world) => world.map(f64::from),
            None => velocity.map(snap),
        };
    }
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The correctly rounded `rsqrtss` estimate.
fn estimate(value: f32) -> f32 {
    (1.0 / f64::from(value).sqrt()) as f32
}

/// The correctly rounded `rcpss` estimate.
fn reciprocal(value: f32) -> f32 {
    (1.0 / f64::from(value)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame turned about y and moved: its matrix places a point as the
    /// frame does, and the inverse takes it back.
    #[test]
    fn a_matrix_places_and_its_inverse_takes_back() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let frame = ContactFrame {
            translation: [3.0, 1.0, -2.0],
            rotation: [0.0, half, 0.0, half],
        };
        let matrix = Matrix::of(frame, 0);
        let point = [1.0, 2.0, 0.5];
        let placed = matrix.place(point);
        let expected = frame.place(point);
        for axis in 0..3 {
            assert!((placed[axis] - expected[axis]).abs() < 1e-5, "{placed:?}");
        }
        let back = matrix.inverse().place(placed);
        for axis in 0..3 {
            assert!((back[axis] - point[axis]).abs() < 1e-5, "{back:?}");
        }
        let turned = matrix.turn([0.0, 0.0, 1.0]);
        let local = matrix.turn_back(turned);
        assert!((local[2] - 1.0).abs() < 1e-6 && local[0].abs() < 1e-6);
    }
}
