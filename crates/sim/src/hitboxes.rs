//! The shapes a shot hits on each body (`game_hitboxes_174.json`, derived
//! by `orpheus-assets`, whose `hitboxes` module says where they come from):
//! spheres and capsules in the body's frame at rest, its origin at the feet,
//! Y up and +Z forward, each critical or not.
//!
//! New: the Python reference hit every body as one capsule from the feet to
//! the mover's height (combat.py `_targets`, `ray_capsule`) and called a
//! hit critical at or above the eye less `HEAD_BELOW`.

use std::{collections::HashMap, fs, io, path::Path, sync::Arc};

use orpheus_core::guid::parse_guid;
use orpheus_physics::static_ray::Vec3;
use serde::Deserialize;

/// A hit shape in the body's frame: a capsule from `a` to `b`, a sphere
/// when they are the same point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitShape {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f64,
    /// A hit on it is critical (the body's damage zones list its zone).
    pub critical: bool,
}

/// Every body's [`HitShape`]s.
#[derive(Debug, Default)]
pub struct HitboxCatalog {
    by_body: HashMap<u64, Arc<[HitShape]>>,
}

#[derive(Deserialize)]
struct RawBody {
    shapes: Vec<RawShape>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum RawShape {
    Sphere {
        centre: Vec3,
        radius: f64,
        critical: bool,
    },
    Capsule {
        a: Vec3,
        b: Vec3,
        radius: f64,
        critical: bool,
    },
}

impl HitboxCatalog {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_json(&fs::read_to_string(path)?)
    }

    pub fn from_json(text: &str) -> io::Result<Self> {
        let invalid = |error: String| io::Error::new(io::ErrorKind::InvalidData, error);
        let raw: HashMap<String, RawBody> =
            serde_json::from_str(text).map_err(|error| invalid(error.to_string()))?;
        let mut by_body = HashMap::new();
        for (body, raw) in raw {
            let guid =
                parse_guid(&body).ok_or_else(|| invalid(format!("invalid body {body:?}")))?;
            let shapes: Arc<[HitShape]> = raw
                .shapes
                .into_iter()
                .map(|shape| match shape {
                    RawShape::Sphere {
                        centre,
                        radius,
                        critical,
                    } => HitShape {
                        a: centre,
                        b: centre,
                        radius,
                        critical,
                    },
                    RawShape::Capsule {
                        a,
                        b,
                        radius,
                        critical,
                    } => HitShape {
                        a,
                        b,
                        radius,
                        critical,
                    },
                })
                .collect();
            if !shapes.is_empty() {
                by_body.insert(guid, shapes);
            }
        }
        Ok(Self { by_body })
    }

    /// The body's shapes; `None` for a body without any, which keeps the
    /// capsule.
    pub fn for_body(&self, body: u64) -> Option<Arc<[HitShape]>> {
        self.by_body.get(&body).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_spheres_and_capsules_and_skips_bodies_without_shapes() {
        let catalog = HitboxCatalog::from_json(
            r#"{
                "0x0400000000001403": {"name": "Training Bot", "model": "0x0D0000000000312E", "shapes": [
                    {"kind": "sphere", "centre": [0.0, 1.5, 0.0], "radius": 0.44, "critical": true},
                    {"kind": "capsule", "a": [0.5, 0.9, 0.0], "b": [0.5, 0.9, 0.8], "radius": 0.1, "critical": false}
                ]},
                "0x0400000000000001": {"name": "Tracer", "shapes": []}
            }"#,
        )
        .unwrap();
        let shapes = catalog.for_body(0x0400_0000_0000_1403).unwrap();
        assert_eq!(
            *shapes,
            [
                HitShape {
                    a: [0.0, 1.5, 0.0],
                    b: [0.0, 1.5, 0.0],
                    radius: 0.44,
                    critical: true
                },
                HitShape {
                    a: [0.5, 0.9, 0.0],
                    b: [0.5, 0.9, 0.8],
                    radius: 0.1,
                    critical: false
                },
            ]
        );
        assert!(catalog.for_body(0x0400_0000_0000_0001).is_none());
        assert!(catalog.for_body(0x0400_0000_0000_0002).is_none());
    }
}
