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
//! frame, and the movement records carry the parent (entity and node) with
//! that local position; the owner's client compares all of them exactly
//! (`0x7FF789B73E00`), so a different parent is a correction.
//!
//! The velocity stays in world axes ([`WORLD_AXES`]): the mover's init
//! (`0x7FF789AD0540` +0xE22) sets movement flag 0x1000000 and nothing in
//! the client clears it but a record or create without it. With it the
//! velocity getter and setter (`0x7FF789B73D60`, `0x7FF789B76A70`) and the
//! friction's add (`0x7FF789B6DA60`) take the stored numbers as they are,
//! so on a platform the stored velocity is the velocity relative to it, in
//! world axes, and the platform's own motion comes from the carry alone.
//! The reparent (`0x7FF789B4ADA0`) adds the old platform's velocity `p`
//! (`0x7FF789ACD630`: getter plus `p`, stored) and takes the new one's off
//! (`0x7FF789ACD520`: getter less `p`): a body leaving a platform keeps its
//! motion, unturned. Without the flag (the server's records before
//! 2026-11-24) the same code turns the numbers by the platform's rotation
//! and drops its velocity: a still rider stopped dead on a jump and a
//! moving one left turned by the platform's yaw.
//!
//! The parent's frame (`0x7FF789B71FD0`) is its component 1 transform as
//! posed for the tick (the tick's rewind scope poses it at the command's
//! view time), after the node's place in the model for a node (rows made
//! unit with `rsqrtps`), without scale. Within one tick (`0x7FF789B4B1D0`):
//! the stored local point placed by the frame is where the probe starts,
//! plus `dt · p` (`p` the platform velocity of the tick before); the
//! capsule stays where the last tick left it, and the sweep adds the carry
//! `D`, the local point's place now less the capsule's feet
//! (`0x7FF789B46560`: `dt · p` on the tick it takes a new platform, 0
//! without one); after the move the feet go back into the frame (its
//! inverse by cofactors, `0x7FF789884370`), on the 1/1024 grid. The
//! platform velocity `p` (`0x7FF789B6E4E0`) is the local point's move
//! between the platform posed at the view time and `dt` before it, divided
//! by `dt`, times component 77's factor; it is taken again every tick at
//! the reparent.
//!
//! The client takes `rsqrtps`/`rcpps` estimates for the frame's rows and
//! its inverse; the port takes them correctly rounded (they differ between
//! CPUs), with the client's Newton steps.

use super::{Mover, posture::snap};
use crate::contact::math::{ContactFrame, Vec3, add, scale, sub};
use crate::contact::world::{ContactWorld, Platform};

/// Movement flag 0x1000000: the velocity (and the facing) in world axes
/// whatever the parent. The client's mover init sets it on every mover
/// (`0x7FF789AD0540`); the records carry it so that the owner's client
/// keeps it ([`Mover::record`]).
pub const WORLD_AXES: u32 = 0x0100_0000;

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
    /// The velocity relative to the platform, in world axes.
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
    /// Put a riding mover's feet in the world where its platform stands
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
        self.pose.velocity = riding.velocity.map(f64::from);
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
            Some((riding, carry))
        });
        let same = |platform: &Platform| {
            found
                .is_some_and(|found| found.entity == platform.entity && found.node == platform.node)
        };
        if let Some((riding, carry)) = old.filter(|(riding, _)| same(&riding.platform)) {
            self.riding = Some(Riding { carry, ..riding });
            return start.carry;
        }
        // The old platform's velocity on, the new one's off.
        let raw = match old {
            Some((riding, carry)) => add(riding.velocity, carry),
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
        let velocity = sub(raw, carry);
        self.riding = Some(Riding {
            platform,
            local,
            velocity,
            carry,
        });
        self.pose.velocity = velocity.map(f64::from);
        scale(carry, dt)
    }

    /// Leave the platform outside the tick's probe (a behaviour or an
    /// authored motion taking over: the client's reparent to none from
    /// them): the feet where the platform has them now, its velocity added.
    pub(super) fn leave_platform(&mut self, dt: f32) {
        let Some(riding) = self.riding.take() else {
            return;
        };
        let world = self.body.world();
        let Some(matrix) = frame_now(world, &riding.platform) else {
            return;
        };
        let clock = self.platforms.as_deref();
        let carry = platform_velocity(clock, &riding.platform, &matrix, riding.local, dt);
        self.pose.position = matrix.place(riding.local).map(f64::from);
        self.pose.velocity = add(riding.velocity, carry).map(f64::from);
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

    /// The moved velocity on the grid, kept as it is: relative to the
    /// platform on one ([`WORLD_AXES`]).
    pub(super) fn settle_velocity(&mut self, velocity: [f64; 3]) {
        self.pose.velocity = velocity.map(snap);
        if let Some(riding) = self.riding.as_mut() {
            riding.velocity = self.pose.velocity.map(|axis| axis as f32);
        }
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
    }
}
