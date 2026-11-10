//! Float32 character capsule contacts from the build 104319 movement engine.
//!
//! Client addresses in these modules are for the PC 1.74 client (build
//! 104319) at image base 0x7FF788F10000. The client's mover asks its physics
//! engine to move the capsule (0x7FF789CCABF0). The engine does not push the
//! capsule out of the world itself: it keeps contacts between the capsule and
//! the world's shapes (map triangle mesh, convex polytopes, props), each
//! contact keeps manifolds (a normal and up to four points with their
//! separation), and the character's solver turns each manifold into a plane.
//! Every value is float32, each operation rounded the way the SSE code rounds
//! it. The CPU's rcpps/rsqrtps estimates are not reproducible across CPUs;
//! the port uses the correctly rounded value, and the client's Newton
//! refinement step follows either way.

pub mod body;
pub mod cache;
pub mod gjk;
pub mod hull;
pub mod manifolds;
pub mod math;
pub mod mesh;
pub mod round;
pub mod solver;
pub mod triangle;
pub mod world;
