//! The map's placed entities that carry target tags, as the scene tag
//! queries find them (`STU_160F2578`, `STU_42578342`, `STU_EB216A0B`,
//! `STU_B3E67A36`; [`crate::expr`]'s `tag_query`).
//!
//! The client walks every entity of its pool for a query (0x7FF789BA15F0)
//! and tests each one's tags (0x7FF789CC5030: component 76's own list,
//! less the suppressed, plus the added). The worlds know the map's
//! placeables (`0x80000000 | flat`, `lucy_assets`' `map_areas`
//! `tagged`) and, of the bodies, only their own: a tag that some body
//! definition carries could also find bodies, so a query for one is left
//! unanswered.
//!
//! A placeable's tags are those its instance data
//! (`STUTargetTagInstanceData.m_targetTags`) and its definition
//! (`STUTargetTagComponent.m_targetTags`) name together. Assumption: the
//! client's component 76 list holds both; how it builds the list from the
//! two was not read (upstream leafall903/ORPHEUS `scene.py`
//! `static_tag_targets` refuses a placeable with both).

use std::collections::BTreeSet;

use crate::graph::GraphStore;

/// A placed entity with its target tags.
#[derive(Clone, Debug, PartialEq)]
pub struct TaggedEntity {
    pub entity: u32,
    pub position: [f64; 3],
    /// Its placement's rotation, a unit quaternion (x, y, z, w).
    pub rotation: [f64; 4],
    /// Tag GUIDs as the graphs name them (type `06C`).
    pub tags: BTreeSet<u64>,
}

impl TaggedEntity {
    /// Where it faces: its placement's forward axis, +z turned by its
    /// rotation (a body's facing is +z turned by its yaw, `expr::aim`).
    pub fn forward(&self) -> [f64; 3] {
        let [x, y, z, w] = self.rotation;
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ]
    }
}

/// The map's tagged entities in one mode, in placeable order.
#[derive(Debug, Default)]
pub struct MapTags {
    entities: Vec<TaggedEntity>,
    /// The tags some body definition of the bundle carries.
    body_tags: BTreeSet<u64>,
}

impl MapTags {
    pub fn new(entities: Vec<TaggedEntity>, graphs: &GraphStore) -> Self {
        let body_tags = graphs
            .bodies()
            .values()
            .filter_map(|body| body.target_tags.as_ref())
            .flatten()
            .copied()
            .collect();
        Self {
            entities,
            body_tags,
        }
    }

    /// The entities that carry `tag`, in placeable order; None when a body
    /// definition carries it too (the bodies' tags are not listed here).
    pub fn with_tag(&self, tag: u64) -> Option<Vec<u32>> {
        if self.body_may_carry(tag) {
            return None;
        }
        Some(self.placed_with_tag(tag))
    }

    /// The placeables that carry `tag`, in placeable order, whether or not
    /// bodies may carry it too.
    pub fn placed_with_tag(&self, tag: u64) -> Vec<u32> {
        self.entities
            .iter()
            .filter(|entity| entity.tags.contains(&tag))
            .map(|entity| entity.entity)
            .collect()
    }

    /// Whether some body definition of the bundle carries `tag`.
    pub fn body_may_carry(&self, tag: u64) -> bool {
        self.body_tags.contains(&tag)
    }

    /// Every listed entity, in placeable order.
    pub fn entities(&self) -> impl Iterator<Item = u32> + '_ {
        self.entities.iter().map(|entity| entity.entity)
    }

    /// A listed entity.
    pub fn get(&self, entity: u32) -> Option<&TaggedEntity> {
        self.entities.iter().find(|tagged| tagged.entity == entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_lists_the_placeables_that_carry_it_unless_a_body_may() {
        let tagged = |entity, tags: &[u64]| TaggedEntity {
            entity,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            tags: tags.iter().copied().collect(),
        };
        let tags = MapTags {
            entities: vec![
                tagged(0x8000_0010, &[1, 2]),
                tagged(0x8000_0020, &[2]),
                tagged(0x8000_0030, &[3]),
            ],
            body_tags: BTreeSet::from([3]),
        };
        assert_eq!(tags.with_tag(2), Some(vec![0x8000_0010, 0x8000_0020]));
        assert_eq!(tags.with_tag(4), Some(Vec::new()));
        assert_eq!(tags.with_tag(3), None);
        assert_eq!(
            tags.get(0x8000_0020).map(|entity| entity.entity),
            Some(0x8000_0020)
        );
        assert!(tags.get(0x8000_0040).is_none());
    }
}
