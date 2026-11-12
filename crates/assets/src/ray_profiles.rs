//! `data/game_ray_profiles_174.json`: the map mask of every ray profile
//! (`097`, `STU_8EFC337B`). A weapon volley names its profile in
//! `m_C1903771`; the client's ray request takes the profile's `m_category`
//! (an STUDominoFlags, at +0x35), packed, as the mask of the static
//! traversal, and `m_CBCB2AB9` (+0x28) as the mask it tests the bodies'
//! shapes with (leafall903/ORPHEUS (MIT) `docs/COMMUNICATION_WHEEL.txt`:
//! `BEEEB0` packs +28 and +35 into request +2 and +0, `E122A0` copies +0
//! into the static traversal). A map triangle or prop shape stops the ray
//! when its mask shares a bit with the map mask. In 104319 every profile's
//! is `0x1FDF` or `0x1F9F` (refs report `WeaponRays.md`).

use std::path::Path;

use orpheus_stu::Value;
use serde_json::{Map, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{key, load, typed},
    props::pack_flags,
};

pub(crate) const PATH: &str = "data/game_ray_profiles_174.json";

const RAY_PROFILE: u16 = 0x097;
const PROFILE_CLASS: &str = "STU_8EFC337B";

const SOURCE: &str = "Build 1.74.0.0.104319: every ray profile (097, STU_8EFC337B): its GUID -> its \
m_category (STU_933AD0A9) packed as the collision masks are, the map mask of a ray of that profile.";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut profiles = Map::new();
    for (&index, asset) in &load(install, RAY_PROFILE)? {
        let root = asset
            .root()
            .filter(|root| root.class_name() == Some(PROFILE_CLASS))
            .ok_or_else(|| asset_error(format!("{index:012X}.097: not a {PROFILE_CLASS}")))?;
        let category = root
            .get("m_category")
            .and_then(|value| asset.resolve(value));
        let mask = pack_flags(|field| {
            category?
                .fields
                .iter()
                .find(|(hash, _)| *hash == field)
                .and_then(|(_, value)| match value {
                    Value::Int(value) => Some(*value != 0),
                    Value::UInt(value) => Some(*value != 0),
                    _ => None,
                })
        });
        profiles.insert(key(typed(index, RAY_PROFILE)), json!(mask));
    }
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "profiles": profiles}),
    )])
}
