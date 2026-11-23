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
    weapons::DamageTags,
    wire::{Value, Variables},
};

pub const ULT_CHARGE: u32 = 342;
pub const ULT_COST: u32 = 338;
/// `0012.025`: the body dealt damage: the amount (`0013`), the body hit
/// (`0015`), the hit's tags (`003D`) and what it took from the body's named
/// pools (`0624`). `0B8B` rid0 charges it; rid72 folds `0624` and rid73
/// takes what the pool `3EBB.01C` (Wrecking Ball's Adaptive Shield, `1566`
/// st6) lost off the amount, so damage that shield soaks charges nothing.
pub const DAMAGE_DEALT: u64 = 0x0240_0000_0000_0012;
/// `0624`'s item keys, as `0B8B` rid72 reads them: the pool's name
/// (`3EB8`, an identifier) and what it lost (`01F3`).
const DRAIN_POOL: i64 = 0x3eb8;
const DRAIN_AMOUNT: i64 = 0x01f3;
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

/// The ruleset's ultimate generation settings for the body's spawn, as
/// entity variables: the ones `ruleset` answers (the hero's and mode's
/// schemas, or a match's own ruleset, `GraphStore::ruleset_value`).
pub fn generation_variables(ruleset: impl Fn(u32) -> Option<Value>) -> Variables {
    [ULTIMATE_GENERATION, PASSIVE_GENERATION, COMBAT_GENERATION]
        .into_iter()
        .filter_map(|key| Some((key, ruleset(key)?)))
        .collect()
}

/// The message the body takes for `amount` it dealt to `target`: damage
/// with the hit's tags and what it took from the target's named pools
/// (`drained`, by name, in order: `crate::vitals::Damage::drained`), or
/// healing whose kind is the heal's tags (assumption: `0B8B` leaves out
/// kinds `0292` and `025A`, and `01C7` tests `04AB`, all identifiers a
/// health change's tags hold).
pub fn dealt_message(
    target: u32,
    amount: f64,
    tags: &DamageTags,
    heal: bool,
    drained: &[(u64, f64)],
) -> (u64, Variables) {
    let (message, keys) = if heal {
        (HEALING_DONE, [0x40, 0x3f, 0x41])
    } else {
        (DAMAGE_DEALT, [0x13, 0x15, 0x3d])
    };
    let mut parameters = Variables::from([
        (keys[0], Value::Float(amount)),
        (keys[1], Value::Entity(target)),
        (keys[2], tags.to_value()),
    ]);
    if !heal {
        let drains = drained
            .iter()
            .map(|&(pool, lost)| {
                Value::Map(vec![
                    (Value::Integer(DRAIN_POOL), Value::Asset(pool)),
                    (Value::Integer(DRAIN_AMOUNT), Value::Float(lost)),
                ])
            })
            .collect();
        parameters.insert(0x624, Value::Array(drains));
    }
    (message, parameters)
}
