//! The target tags a body's own states add and suppress (target tag state
//! `STU_E56BA8D8`), kept the same way by every body's world: a player's or
//! an AI hero's (`crate::player::PlayerWorld`) and a training bot's
//! (`crate::bot::BotWorld`). What they hold goes to the clients as the
//! body's component 76 (AllyBars.md "Client tools pass 2": the client's
//! effective tags are the authored ones and field 0, less field 1).

use std::collections::BTreeMap;

use crate::vm::TargetTagEffect;

/// The tags a body's states add and suppress, as tag GUIDs in order: what
/// component 76 carries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetTagLists {
    pub added: Vec<u64>,
    pub suppressed: Vec<u64>,
}

#[derive(Default)]
struct Counts {
    grants: u32,
    suppressions: u32,
}

/// How many of a body's running states grant and suppress each tag.
#[derive(Default)]
pub struct TargetTags {
    counts: BTreeMap<u64, Counts>,
    /// A tag joined or left a list since [`Self::take_change`].
    changed: bool,
}

impl TargetTags {
    /// A state's effect on one of the body's own tags began (`active`) or
    /// ended; false when it can't be taken (an end with nothing begun, or a
    /// grant while a suppression holds the tag).
    pub fn set(&mut self, effect: TargetTagEffect, active: bool) -> bool {
        let counts = if active {
            self.counts.entry(effect.tag).or_default()
        } else {
            let Some(counts) = self.counts.get_mut(&effect.tag) else {
                return false;
            };
            counts
        };
        // The authored removal can mask existing grants. A later grant's
        // precedence over an already-active suppression is still unknown.
        if active && effect.present && counts.suppressions != 0 {
            return false;
        }
        let count = if effect.present {
            &mut counts.grants
        } else {
            &mut counts.suppressions
        };
        let next = if active {
            count.checked_add(1)
        } else {
            count.checked_sub(1)
        };
        let Some(next) = next else {
            return false;
        };
        // A tag joins or leaves a list only when its count leaves or
        // reaches zero.
        self.changed |= (*count == 0) != (next == 0);
        *count = next;
        if counts.grants == 0 && counts.suppressions == 0 {
            self.counts.remove(&effect.tag);
        }
        true
    }

    /// Whether the body carries `tag` by its states: false while one
    /// suppresses it, true while one only grants it; None when no state
    /// touches it (the authored tags decide).
    pub fn has(&self, tag: u64) -> Option<bool> {
        self.counts.get(&tag).map(|counts| counts.suppressions == 0)
    }

    /// The tags the states add and suppress now, when a tag came or went
    /// since the last call. A suppression wins on the client as in
    /// [`Self::has`] (`0x7FF789AF2980`).
    pub fn take_change(&mut self) -> Option<TargetTagLists> {
        if !std::mem::take(&mut self.changed) {
            return None;
        }
        let mut lists = TargetTagLists::default();
        for (&tag, counts) in &self.counts {
            if counts.grants != 0 {
                lists.added.push(tag);
            }
            if counts.suppressions != 0 {
                lists.suppressed.push(tag);
            }
        }
        Some(lists)
    }
}
