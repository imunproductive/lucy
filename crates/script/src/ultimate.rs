//! The ultimate's charge: entity variable #342 against its cost #338, which
//! the body's graphs set. Graph `0B8B` (a SubScript of `004B`, server-only
//! nodes) charges it, as the retail server ran it: 5 points after each 1 s
//! Wait by itself, and the amount of each damage dealt (`0012.025`) and
//! healing done (`0044.025`) the server sends the body ([`DAMAGE_DEALT`],
//! [`HEALING_DONE`]), each times the ruleset scales it reads from the
//! body's entity variables (`docs/notes/behavior.md`, "Ultimate charge").
//!
//! What stays here is what no graph does: the messages' shape, and writing
//! the ruleset's "Ultimate Generation" settings onto the body's entity
//! variables (no graph writes #9532e, #12918e or #13022e; retail's server
//! does, an inference from the reads, GameSettings.md).

use crate::{
    rulesets::Rulesets,
    weapons::DamageTags,
    wire::{Value, Variables},
};

pub const ULT_CHARGE: u32 = 342;
pub const ULT_COST: u32 = 338;
/// `0012.025`: the body dealt damage: the amount (`0013`), the body hit
/// (`0015`) and the hit's tags (`003D`). `0B8B` rid0 charges it.
pub const DAMAGE_DEALT: u64 = 0x0240_0000_0000_0012;
/// `0044.025`: the body healed another: the amount (`0040`), the body
/// healed (`003F`) and its kind (`0041`). `0B8B` rid31 charges it.
pub const HEALING_DONE: u64 = 0x0240_0000_0000_0044;
/// The hero ruleset entries (`01C` indexes, schema `0A30…0007`, each 100% by
/// default) that `0B8B` reads as entity variables of the same id:
/// "Ultimate Generation" (both ways), "Ultimate Generation - Passive" (the
/// 1 s loop, rid4) and "Ultimate Generation - Combat" (rid23, rid32).
pub const ULTIMATE_GENERATION: u32 = 0x253C;
pub const PASSIVE_GENERATION: u32 = 0x3276;
pub const COMBAT_GENERATION: u32 = 0x32DE;

/// The ruleset's ultimate generation settings for the hero in the mode,
/// as entity variables for the body's spawn: the ones the schemas set.
pub fn generation_variables(rulesets: &Rulesets, hero: u64, mode: u64) -> Variables {
    [ULTIMATE_GENERATION, PASSIVE_GENERATION, COMBAT_GENERATION]
        .into_iter()
        .filter_map(|key| Some((key, rulesets.default(key, hero, mode)?)))
        .collect()
}

/// The message the body takes for `amount` it dealt to `target`: damage
/// with the hit's tags, or healing whose kind is the heal's tags
/// (assumption: `0B8B` leaves out kinds `0292` and `025A`, and `01C7`
/// tests `04AB`, all identifiers a health change's tags hold).
pub fn dealt_message(target: u32, amount: f64, tags: &DamageTags, heal: bool) -> (u64, Variables) {
    let (message, keys) = if heal {
        (HEALING_DONE, [0x40, 0x3f, 0x41])
    } else {
        (DAMAGE_DEALT, [0x13, 0x15, 0x3d])
    };
    let parameters = Variables::from([
        (keys[0], Value::Float(amount)),
        (keys[1], Value::Entity(target)),
        (keys[2], tags.to_value()),
    ]);
    (message, parameters)
}
