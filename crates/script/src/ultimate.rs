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
/// (`0015`), the hit's tags (`003D`) and whether it was critical (`0095`).
/// `0B8B` rid0 charges it; `01C7` rid1 plays the hit's sound for it (rid40
/// on `0095`: the critical effects, rid100/rid0/rid99 the others).
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

/// The ruleset's ultimate generation settings for the body's spawn, as
/// entity variables: the ones `ruleset` answers (the hero's and mode's
/// schemas, or a match's own ruleset, `GraphStore::ruleset_value`).
pub fn generation_variables(ruleset: impl Fn(u32) -> Option<Value>) -> Variables {
    [ULTIMATE_GENERATION, PASSIVE_GENERATION, COMBAT_GENERATION]
        .into_iter()
        .filter_map(|key| Some((key, ruleset(key)?)))
        .collect()
}

/// What a body dealt ([`dealt_message`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dealt {
    Damage { critical: bool },
    Healing,
}

/// The message the body takes for `amount` it dealt to `target`: damage
/// with the hit's tags and whether it was critical, or healing whose kind
/// is the heal's tags (assumption: `0B8B` leaves out kinds `0292` and
/// `025A`, and `01C7` tests `04AB`, all identifiers a health change's tags
/// hold). `01C7` also reads a `0724` with `0012.025` (#2101, which of its
/// hit sounds plays); what retail sent in it is not known, so it isn't
/// sent.
pub fn dealt_message(
    target: u32,
    amount: f64,
    tags: &DamageTags,
    dealt: Dealt,
) -> (u64, Variables) {
    match dealt {
        Dealt::Damage { critical } => (
            DAMAGE_DEALT,
            Variables::from([
                (0x13, Value::Float(amount)),
                (0x15, Value::Entity(target)),
                (0x3d, tags.to_value()),
                (0x95, Value::Bool(critical)),
            ]),
        ),
        Dealt::Healing => (
            HEALING_DONE,
            Variables::from([
                (0x40, Value::Float(amount)),
                (0x3f, Value::Entity(target)),
                (0x41, tags.to_value()),
            ]),
        ),
    }
}
