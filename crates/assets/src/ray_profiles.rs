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
//! The asset stores only the flags that differ from the profile's own
//! defaults for each of its two STUDominoFlags, and those defaults are the
//! profile's, not the flags class's (OWLib's, all on but `m_B97596D2`):
//! `m_category` has `m_E552CFAA` alone on, `m_CBCB2AB9` `m_27CD5BF8` alone.
//! Measured in the game (2026-11-19, owdbg probe at `0x7FF789AFF25B` in the
//! query `0x7FF789AFEEB0`, after both packs `0x7FF78A6B8FC0`: `r15` the
//! profile, `[rsi+0x1C8]` the loaded profile), profiles `0002`, `0017` and
//! `0018` (Widowmaker's Grappling Hook), which store no `m_category`, load
//! `m_E552CFAA` alone (map mask `0x0040`), and their `m_CBCB2AB9` loads
//! the stored flags plus `m_27CD5BF8`. Across the 66 profiles of 104319
//! the asset stores `m_category`'s `m_E552CFAA` only as 0 and its other
//! flags only as 1, `m_CBCB2AB9`'s `m_27CD5BF8` only as 0 and its others
//! only as 1: each stored flag is a change from those defaults (refs
//! report `Grapple.md` "DayGrapple3"). So a profile without an
//! `m_category`, `0001` among them (the client's profile for a volley that
//! names none), has map mask `0x0040`.

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
/// The one flag a ray profile's `m_category` has on by default.
const CATEGORY_DEFAULT: u32 = 0xE552_CFAA;

const SOURCE: &str = "Build 1.74.0.0.104319: every ray profile (097, STU_8EFC337B): its GUID -> its \
m_category (STU_933AD0A9) packed as the collision masks are, the map mask of a ray of that profile; \
a flag the asset leaves out takes the profile's default (m_E552CFAA alone on).";

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
            let stored = category
                .and_then(|category| category.stored(field))
                .and_then(|value| match value {
                    Value::Int(value) => Some(*value != 0),
                    Value::UInt(value) => Some(*value != 0),
                    _ => None,
                });
            Some(stored.unwrap_or(field == CATEGORY_DEFAULT))
        });
        profiles.insert(key(typed(index, RAY_PROFILE)), json!(mask));
    }
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "profiles": profiles}),
    )])
}
