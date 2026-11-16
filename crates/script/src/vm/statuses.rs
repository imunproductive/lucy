//! Statuses and combat modifiers a body's graphs put on it while a state
//! runs: `STU_9028E035` (a status by `m_type`) and
//! `STUStatescriptStateCombatModFilter` (a scale on the damage or healing
//! the body deals or takes). The match's health code reads them
//! ([`World::set_status`], [`World::combat_mod`]).
//!
//! `STU_9028E035` `m_type`, read from its 38 uses in the hero and bot
//! bundles (no other field is set):
//!
//! - 0, in 26: the spawn room (`0C8E` st11, while the body stands in its
//!   team's room), Reaper's Wraith Form (`0020`, `00F2`), Mei's Cryo-Freeze
//!   (`05FE` st49), D.Va's ejection (`026A`), Sombra's Translocator
//!   (`0C19`). Strong guess: invulnerable, the body takes no damage.
//! - 1, in 12: Tracer's Recall (`001A` st2), Doomfist's Meteor Strike
//!   (`0C51` st81), Moira's Fade (`0F7C`), Echo's Duplicate (`19EB` st8,
//!   st93), the Deathmatch and Team Deathmatch bodies (`1058`, `1066` st3).
//!   Guess: phased out, untargetable as well as invulnerable. The server
//!   treats it as invulnerable too (assumption).
//!
//! `CombatModFilter`: `m_60703B1E` names what it scales (`0D8`
//! identifiers). The client's query (0x7FF789C375A0, HealthAuras.md
//! "Client tools pass (2026-11-16)") pairs the kinds {`0318`, `031A`}, whose
//! state tests its filter `m_6AA5D70F` on one party of the event, and
//! {`0317`, `0319`}, whose state tests `m_5E6F1B8B` on the other. With the
//! data that gives `0317` the damage dealt, `0318` the damage taken, `0319`
//! the healing dealt, `031A` the healing taken (Biotic Grenade's enemy aura
//! `0C72` st8 sets 0, the anti-heal; its ally aura `0B99` st4 1.5, the heal
//! boost; `0318` 0.5 on several damage reductions). Which party is the
//! source and which the target is not shown; the server keeps its choice:
//! a "taken" modifier acts on the body whose state holds it.
//! `m_50530EAB` is the value; `m_41B10FF2` 1 multiplies by it, 0 adds it
//! (-0.5 and -0.4 in `0240` and `0CF4`). Of the event's tags, one of
//! `m_F60AA143` must be there when it lists any, none of `m_5CFFCBAE` may
//! be (`0C72`: `293E`), and `m_029060AC` are added to them when it applies.
//! What `m_941C56C2` and `m_E9517777` do is not known.

use super::*;
use crate::weapons::DamageTags;

/// A combat modifier a `CombatModFilter` state holds on its body.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatMod {
    /// What it scales (`m_60703B1E`, a `0D8` GUID).
    pub kind: u64,
    /// `m_50530EAB`.
    pub value: f64,
    /// `m_41B10FF2`: multiplies (true) or adds.
    pub multiplies: bool,
    /// The tags one of which the event must have when it lists any
    /// (`m_F60AA143`).
    pub required: Vec<u64>,
    /// The damage tags it leaves alone (`m_5CFFCBAE`).
    pub exempt: Vec<u64>,
    /// The tags it adds to the event when it applies (`m_029060AC`).
    pub added: Vec<u64>,
}

/// `m_60703B1E` of a modifier of the healing the body takes.
pub const HEALING_TAKEN: u64 = 0x0d80_0000_0000_031a;
/// `m_60703B1E` of a modifier of the damage the body takes.
pub const DAMAGE_TAKEN: u64 = 0x0d80_0000_0000_0318;

/// `STU_9028E035` `m_type` 0: invulnerable (strong guess).
pub const INVULNERABLE: i64 = 0;
/// `STU_9028E035` `m_type` 1: phased out (guess).
pub const PHASED_OUT: i64 = 1;

impl<W: World> Vm<'_, '_, '_, W> {
    pub(super) fn begin_status(&mut self, key: StateKey, node: &Node) {
        let site = self.site(key.instance, node);
        let status = field(&node.fields, "m_type").as_i64().unwrap_or(0);
        if !self.world.set_status(site, Some(status)) {
            self.assume(site, &node.class, "no effect (the world keeps no status)");
        }
    }

    pub(super) fn begin_combat_mod(&mut self, key: StateKey, node: &Node) {
        let id = key.instance;
        let site = self.site(id, node);
        let Some(modifier) = self.read_combat_mod(id, node) else {
            self.assume(site, &node.class, "no effect (an unread form)");
            return;
        };
        if !self.world.combat_mod(site, Some(modifier)) {
            self.assume(site, &node.class, "no effect (the world keeps no modifier)");
        }
    }

    /// The state's modifier; none for a form the server doesn't read: a
    /// filter on the event's party (`m_6AA5D70F`, `m_5E6F1B8B`).
    fn read_combat_mod(&mut self, id: u16, node: &Node) -> Option<CombatMod> {
        let kind = asset_guid(field(&node.fields, "m_60703B1E"))?;
        if !field(&node.fields, "m_6AA5D70F").is_null()
            || !field(&node.fields, "m_5E6F1B8B").is_null()
        {
            return None;
        }
        let value = self.evaluate(id, node, field(&node.fields, "m_50530EAB"), None)?;
        let tags = |name| {
            array_field(&node.fields, name)
                .iter()
                .filter_map(asset_guid)
                .collect()
        };
        Some(CombatMod {
            kind,
            value: expr::to_float(&value),
            multiplies: field(&node.fields, "m_41B10FF2").as_i64().unwrap_or(0) != 0,
            required: tags("m_F60AA143"),
            exempt: tags("m_5CFFCBAE"),
            added: tags("m_029060AC"),
        })
    }
}

/// The statuses and combat modifiers a body's states hold now, for its
/// world to keep and the match's health code to read.
#[derive(Debug, Default)]
pub struct BodyEffects {
    statuses: Vec<(Site, i64)>,
    modifiers: Vec<(Site, CombatMod)>,
}

impl BodyEffects {
    /// [`World::set_status`]: the statuses the server applies (types 0 and
    /// 1) are kept; others are not.
    pub fn set_status(&mut self, site: Site, status: Option<i64>) -> bool {
        self.statuses.retain(|(held, _)| *held != site);
        match status {
            Some(status @ (INVULNERABLE | PHASED_OUT)) => {
                self.statuses.push((site, status));
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    /// [`World::combat_mod`]: the modifiers of the healing the body takes
    /// are kept; the others, guesses, are not.
    pub fn combat_mod(&mut self, site: Site, modifier: Option<CombatMod>) -> bool {
        self.modifiers.retain(|(held, _)| *held != site);
        match modifier {
            Some(modifier) if modifier.kind == HEALING_TAKEN => {
                self.modifiers.push((site, modifier));
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    /// The body takes no damage (a status of type 0 or 1).
    pub fn invulnerable(&self) -> bool {
        !self.statuses.is_empty()
    }

    /// What a heal with `tags` is scaled by: each healing modifier whose
    /// required tags (if any) meet the tags and that doesn't leave one of
    /// them alone, a multiplier times it, an addition plus it, in the order
    /// they came, each adding its tags for the next; never below 0.
    pub fn healing_scale(&self, tags: &DamageTags) -> f64 {
        let mut tags = *tags;
        let mut scale = 1.0;
        for (_, modifier) in &self.modifiers {
            let required = modifier.required.is_empty()
                || modifier
                    .required
                    .iter()
                    .any(|&tag| tags.iter().any(|held| held == tag));
            if !required || tags.iter().any(|tag| modifier.exempt.contains(&tag)) {
                continue;
            }
            if modifier.multiplies {
                scale *= modifier.value;
            } else {
                scale += modifier.value;
            }
            for &tag in &modifier.added {
                tags.push(tag);
            }
        }
        f64::max(scale, 0.0)
    }
}
