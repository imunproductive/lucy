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
//! boost; Nano Boost's `0B9F` st18 -0.5 taken and st19 +0.5 dealt). The
//! client never computes a health change: its only use of the modifiers is
//! a projectile's look (HealthAuras.md "Aura modifiers (2026-11-18)").
//!
//! - `m_F7872851` is the entity that holds the modifier, the instance's
//!   owner when unset (the state's init 0x7FF789C37C30 keeps it at
//!   `+0x248`). The server keeps a modifier only on the body whose graph
//!   runs the state; one held for another entity (`#41` in `006C` and
//!   `10DD`, a projectile's owner in Baptiste's `1909`) is not applied.
//! - `m_50530EAB` is the value; `m_41B10FF2` 1 multiplies by it, 0 adds it.
//!   The modifiers of one kind combine as one plus the sum of the
//!   additions, times each multiplier, never below 0 (assumption, as the
//!   public rule: amplifications and reductions add up, categories
//!   multiply). The dealt kind acts on the attacker's hit (at the hit, not
//!   at a projectile's launch), the taken kind on the body's.
//! - Of the event's tags, one of `m_F60AA143` must be there when it lists
//!   any, none of `m_5CFFCBAE` may be (`0C72`: `293E`), and `m_029060AC` are
//!   added to them when it applies (a dealt one's for the target's taken
//!   ones).
//! - Not read: the party filters (a modifier with one is left out),
//!   `m_941C56C2` (true on the additive reductions `0240`, `0B9F`, `0CF4`,
//!   `012F`: strong guess, they count toward the 50 % reduction cap),
//!   `m_E9517777` (`03A3`'s `19C4`, an identifier the query hands back) and
//!   `m_893AABFC` (the query doesn't read it).

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

/// `m_60703B1E` of a modifier of the damage the body deals.
pub const DAMAGE_DEALT: u64 = 0x0d80_0000_0000_0317;
/// `m_60703B1E` of a modifier of the damage the body takes.
pub const DAMAGE_TAKEN: u64 = 0x0d80_0000_0000_0318;
/// `m_60703B1E` of a modifier of the healing the body gives.
pub const HEALING_DEALT: u64 = 0x0d80_0000_0000_0319;
/// `m_60703B1E` of a modifier of the healing the body takes.
pub const HEALING_TAKEN: u64 = 0x0d80_0000_0000_031a;

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
        let modifier = match self.read_combat_mod(id, node) {
            Ok(modifier) => modifier,
            Err(form) => {
                self.assume(site, &node.class, form);
                return;
            }
        };
        if !self.world.combat_mod(site, Some(modifier)) {
            self.assume(site, &node.class, "no effect (the world keeps no modifier)");
        }
    }

    /// The state's modifier, or why it has no effect: a form the server
    /// doesn't apply (a filter on the event's other party, `m_6AA5D70F` or
    /// `m_5E6F1B8B`; a holder, `m_F7872851`, other than the state's own
    /// entity) or a value it can't evaluate.
    fn read_combat_mod(&mut self, id: u16, node: &Node) -> Result<CombatMod, &'static str> {
        let fields = &node.fields;
        let kind = asset_guid(field(fields, "m_60703B1E")).ok_or("no effect (no kind)")?;
        if !field(fields, "m_6AA5D70F").is_null() || !field(fields, "m_5E6F1B8B").is_null() {
            return Err("no effect (a party filter)");
        }
        let holder = field(fields, "m_F7872851");
        if !holder.is_null() {
            let site = self.site(id, node);
            let holder = self
                .evaluate(id, node, holder, None)
                .and_then(|holder| expr::to_entity(&holder));
            if holder != Some(site.entity) {
                return Err("no effect (held for another entity)");
            }
        }
        let value = self
            .evaluate(id, node, field(fields, "m_50530EAB"), None)
            .ok_or("no effect (an unevaluated value)")?;
        let tags = |name| {
            array_field(fields, name)
                .iter()
                .filter_map(asset_guid)
                .collect()
        };
        Ok(CombatMod {
            kind,
            value: expr::to_float(&value),
            multiplies: field(fields, "m_41B10FF2").as_i64().unwrap_or(0) != 0,
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

    /// [`World::combat_mod`]: the modifiers of the four kinds are kept.
    pub fn combat_mod(&mut self, site: Site, modifier: Option<CombatMod>) -> bool {
        self.modifiers.retain(|(held, _)| *held != site);
        match modifier {
            Some(modifier)
                if [DAMAGE_DEALT, DAMAGE_TAKEN, HEALING_DEALT, HEALING_TAKEN]
                    .contains(&modifier.kind) =>
            {
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

    /// The modifiers held now, in the order they came.
    pub fn modifiers(&self) -> impl Iterator<Item = &CombatMod> {
        self.modifiers.iter().map(|(_, modifier)| modifier)
    }

    /// [`combat_scale`] of the body's modifiers.
    pub fn scale(&self, kind: u64, tags: &mut DamageTags) -> f64 {
        combat_scale(self.modifiers(), kind, tags)
    }
}

/// What `modifiers` scale a health change of `kind` with `tags` by (see the
/// module notes): of those of that kind whose required tags (if any) meet
/// the tags and that leave none of them alone, in the order they came, one
/// plus the additions, never below 0, times the multipliers. Each that
/// applies adds its tags to `tags`.
pub fn combat_scale<'a>(
    modifiers: impl IntoIterator<Item = &'a CombatMod>,
    kind: u64,
    tags: &mut DamageTags,
) -> f64 {
    let mut sum = 1.0;
    let mut product = 1.0;
    for modifier in modifiers
        .into_iter()
        .filter(|modifier| modifier.kind == kind)
    {
        let required = modifier.required.is_empty()
            || modifier
                .required
                .iter()
                .any(|&tag| tags.iter().any(|held| held == tag));
        if !required || tags.iter().any(|tag| modifier.exempt.contains(&tag)) {
            continue;
        }
        if modifier.multiplies {
            product *= modifier.value;
        } else {
            sum += modifier.value;
        }
        for &tag in &modifier.added {
            tags.push(tag);
        }
    }
    f64::max(f64::max(sum, 0.0) * product, 0.0)
}
