//! `data/game_ray_profiles_174.json`: what a ray of each ray profile
//! (`097`, `STU_8EFC337B`) meets. A weapon volley names its profile in
//! `m_C1903771`; the client's ray query (`0x7FF789AFEEB0`) packs the
//! profile's `m_category` (an STUDominoFlags, at +0x35) into the query's
//! `+0` and `m_CBCB2AB9` (+0x28) into its `+2`, and copies
//! `m_0220680A & 3` (+0x42) into its `+0x18`. Each profile gives:
//!
//! - `map`: `m_category` packed, the mask a map triangle or prop shape
//!   must share a bit with to stop the ray;
//! - `body`: `m_CBCB2AB9` packed, the categories of an entity's physics
//!   shapes the ray meets. The client's physics shape filter
//!   (`0x7FF789D077A0`) passes a shape when the query's `+2` shares a bit
//!   with the shape's category (`+0x90`) and its `+0` with the shape's mask
//!   (`+0x92`). The query's static ray takes `body & 0x81` (the map and the
//!   props: categories 1 and 0x80); its entity walk tests an entity's model
//!   bodies with the whole mask (`0x7FF789B01DD0`, `0x7FF789B00C50` →
//!   `0x7FF789CFAD50`). The categories are the model parts' (`props.rs`):
//!   1 and 0x80 static and kinematic parts, 0x100 the other parts and the
//!   ragdolls, 0x800 sensors, 0x1000 moving placeables; a living body's
//!   one shape in the physics world is a 0x2000 capsule (refs report
//!   `HitPredict.md` "Night (2026-11-17)"), a category no profile holds;
//! - `hit_shapes`: `m_0220680A` bit 0, whether the ray tests the bodies'
//!   hit shapes (the model's motion type 1 parts, kept out of the physics
//!   world): the entity walk accepts a model with hit shapes when the
//!   query's `+0x18` has bit 0 (or the entity is in the query's second
//!   list), and then tests them (`0x7FF78AA3E2E0`) with no category: a hit
//!   shape has none (its holder `STU_00066F4C` has no STUDominoFlags).
//!   Without it a living hero or bot is not met at all.
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
//! the stored flags plus `m_27CD5BF8` (body masks `0x1983`, `0x1081`,
//! `0x1081` in the client's packing, [`pack_client_flags`]). Across the 66
//! profiles of 104319 the asset stores `m_category`'s `m_E552CFAA` only as
//! 0 and its other flags only as 1, `m_CBCB2AB9`'s `m_27CD5BF8` only as 0
//! and its others only as 1: each stored flag is a change from those
//! defaults (refs report `Grapple.md` "DayGrapple3"). So a profile without
//! an `m_category`, `0001` among them (the client's profile for a volley
//! that names none), has map mask `0x0040`. `m_0220680A` is stored as 1 or
//! 9 and never as 0, so a profile that leaves it out has 0.

use std::path::Path;

use orpheus_stu::{Object, Value};
use serde_json::{Map, json};

use crate::{
    Install, Output, Result, asset_error,
    game_queues::{key, load, typed},
    props::pack_client_flags,
};

pub(crate) const PATH: &str = "data/game_ray_profiles_174.json";

const RAY_PROFILE: u16 = 0x097;
const PROFILE_CLASS: &str = "STU_8EFC337B";
/// The one flag a ray profile's `m_category` has on by default.
const CATEGORY_DEFAULT: u32 = 0xE552_CFAA;
/// The one flag a ray profile's `m_CBCB2AB9` has on by default.
const BODY_DEFAULT: u32 = 0x27CD_5BF8;
/// `m_0220680A`: bit 0 makes the ray test the bodies' hit shapes.
const QUERY_FLAGS: u32 = 0x0220_680A;

const SOURCE: &str = "Build 1.74.0.0.104319: every ray profile (097, STU_8EFC337B): its GUID -> \
{map: m_category (STU_933AD0A9) packed as the client packs it, the map mask of a ray of that \
profile; body: m_CBCB2AB9 packed so, the physics shape categories it meets; hit_shapes: \
m_0220680A bit 0, whether it tests the bodies' hit shapes}; a flag the asset leaves out takes the \
profile's default (m_category: m_E552CFAA alone on; m_CBCB2AB9: m_27CD5BF8 alone; m_0220680A: 0).";

pub(crate) fn derive(install: &Install, _out: &Path) -> Result<Vec<Output>> {
    let mut profiles = Map::new();
    for (&index, asset) in &load(install, RAY_PROFILE)? {
        let root = asset
            .root()
            .filter(|root| root.class_name() == Some(PROFILE_CLASS))
            .ok_or_else(|| asset_error(format!("{index:012X}.097: not a {PROFILE_CLASS}")))?;
        let member = |name| root.get(name).and_then(|value| asset.resolve(value));
        let query_flags = root.stored(QUERY_FLAGS).and_then(number).unwrap_or(0);
        profiles.insert(
            key(typed(index, RAY_PROFILE)),
            json!({
                "map": pack(member("m_category"), CATEGORY_DEFAULT),
                "body": pack(member("m_CBCB2AB9"), BODY_DEFAULT),
                "hit_shapes": query_flags & 1 != 0,
            }),
        );
    }
    Ok(vec![Output::json(
        PATH,
        &json!({"source": SOURCE, "profiles": profiles}),
    )])
}

/// A profile's STUDominoFlags member packed, each flag the asset leaves out
/// at the member's default (`default` alone on).
fn pack(member: Option<&Object>, default: u32) -> u16 {
    pack_client_flags(|field| {
        member
            .and_then(|member| member.stored(field))
            .and_then(number)
            .map_or(field == default, |value| value != 0)
    })
}

fn number(value: &Value) -> Option<u64> {
    match value {
        Value::Int(value) => u64::try_from(*value).ok(),
        Value::UInt(value) => Some(*value),
        _ => None,
    }
}
