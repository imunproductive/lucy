//! `data/game_ray_profiles_174.json`: the map mask of every ray profile
//! (`097`, `STU_8EFC337B`). A weapon volley names its profile in
//! `m_C1903771`; the client's ray request takes the profile's `m_category`
//! (an STUDominoFlags, at +0x35), packed, as the mask of the static
//! traversal, and `m_CBCB2AB9` (+0x28) as the mask it tests the bodies'
//! shapes with (leafall903/ORPHEUS (MIT) `docs/COMMUNICATION_WHEEL.txt`:
//! `BEEEB0` packs +28 and +35 into request +2 and +0, `E122A0` copies +0
//! into the static traversal). A map triangle or prop shape stops the ray
//! when its mask shares a bit with the map mask.
//!
//! The data's packing is not always what the client loads: measured in the
//! game (2026-11-19, owdbg probe at `0x7FF789AFF25B` in the query
//! `0x7FF789AFEEB0`, after both packs `0x7FF78A6B8FC0`: `r15` the profile,
//! `[rsi+0x1DA]` the packed `m_category`, `[rsi+0x1C8]` the loaded
//! profile), profiles `0002`, `0017` and `0018` (Widowmaker's Grappling
//! Hook) load an `m_category` of `m_E552CFAA` alone, map mask `0x0040`,
//! where their data has no `m_category` (packed here as the class's
//! assumed defaults, `0x1FDF`). Those three take [`MEASURED`]; the others
//! keep the data's packing, unconfirmed (their loaded `m_CBCB2AB9` bytes
//! also differ from the data's, so the reading of explicit values is in
//! doubt too; refs report `Grapple.md` "DayGrapple2").

use std::path::Path;

use orpheus_stu::Value;
use serde_json::{Map, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{key, load, typed},
    props::pack_flags,
};

pub(crate) const PATH: &str = "data/game_ray_profiles_174.json";

/// Map masks measured in the game where the data's packing differs from
/// what the client loads (module docs): profile index, map mask.
const MEASURED: [(u64, u16); 3] = [(0x0002, 0x0040), (0x0017, 0x0040), (0x0018, 0x0040)];

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
        let packed = || {
            pack_flags(|field| {
                category?
                    .fields
                    .iter()
                    .find(|(hash, _)| *hash == field)
                    .and_then(|(_, value)| match value {
                        Value::Int(value) => Some(*value != 0),
                        Value::UInt(value) => Some(*value != 0),
                        _ => None,
                    })
            })
        };
        let mask = MEASURED
            .iter()
            .find(|(measured, _)| *measured == index)
            .map_or_else(packed, |&(_, mask)| mask);
        profiles.insert(key(typed(index, RAY_PROFILE)), json!(mask));
    }
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "profiles": profiles}),
    )])
}
