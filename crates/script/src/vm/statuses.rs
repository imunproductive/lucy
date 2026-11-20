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
//! - A modifier scales the health changes of the entity whose state holds
//!   it. `m_F7872851` names the entity credited for what it changes, the
//!   instance's owner when unset (the state's init 0x7FF789C37C30 keeps it
//!   at `+0x248`; a value that names no entity credits no one). Each
//!   change a modifier makes goes to that entity as [`MODIFIED`]
//!   ([`Scaled::notices`]): Mercy's damage boost (`006C` st6 on the
//!   boosted body, `10DD` st4 on a chained one) names `#41`, its aura
//!   entry's applier (`vm/auras.rs`), and her weapon graph `006B` rid157
//!   counts the boost's extra damage into `048A` and `048C`; Baptiste's
//!   matrix (`1909` on the matrix entity) names his body, whose `1902`
//!   rid63 counts the same two stats (the "Damage Amplified" both heroes
//!   have: inference); Genji's Deflect (`03A3` st59, unset) is credited to
//!   Genji, whose rid140 plays its feedback on `m_E9517777`'s `19C4`. A
//!   holder that took the modifier would boost Mercy and Baptiste
//!   themselves (HealthAuras.md, "Damage Boost's `#41`").
//! - [`MODIFIED`]'s parameters, from its seven entries (`006B`, `03A3`,
//!   `0B9E`, `0CF5`, `1761`, `1902`, `1D02`): `074D` the change the
//!   modifier made (signed: `1902` counts only above 0, `1761` only below),
//!   `005C` the change's other party (the target of a dealt kind's, the
//!   attacker or healer of a taken kind's; `1902` tests it as an enemy),
//!   `076A` `m_E9517777`. `0013` (`006B`, `0B9E`) is not sent: what it
//!   carries is open. A modifier's part of a change is its addition, or
//!   for a multiplier the change it makes to what the additions left
//!   (assumption: the parts sum to the change).
//! - `m_50530EAB` is the value; `m_41B10FF2` 1 multiplies by it, 0 adds it.
//!   The modifiers of one kind combine as one plus the sum of the
//!   additions, times each multiplier, never below 0 (assumption, as the
//!   public rule: amplifications and reductions add up, categories
//!   multiply). `m_941C56C2`, true on the additive damage reductions alone
//!   (Roadhog's `0240` st15 -0.5, Nano Boost's `0B9F` st18 -0.5, Orisa's
//!   `0CF4` st4 -0.4, Bastion's `012F` st124 `#9959`), marks those that
//!   count toward the reduction cap: together they reduce by at most
//!   [`REDUCTION_CAP`], unless one alone reduces by more (assumption,
//!   the public rule; the client has no code for it: the arithmetic is
//!   server-only). The others add after the cap (Discord's `0178` st8
//!   +0.25 taken). The dealt kind acts on the attacker's hit, at the hit
//!   for a hitscan or a graph's change and as the projectile kept them at
//!   its launch for a projectile (the client copies the shooter's `0317`
//!   and `0319` into it at its creation, `sub_7FF789A32900`); the taken
//!   kind on the body's.
//! - Of the event's tags, one of `m_F60AA143` must be there when it lists
//!   any, none of `m_5CFFCBAE` may be (`0C72`: `293E`), and `m_029060AC` are
//!   added to them when it applies (a dealt one's for the target's taken
//!   ones).
//! - The party filter tests the event's other party (the query binds it
//!   as its candidate parameter, `0A4:006E`): `m_6AA5D70F` of a taken
//!   kind the attacker or healer, at the hit ([`PartyFilter`]; Genji's
//!   Deflect `03A3` st59: ×0 for the tags `03EF`/`0302`/`20EC` from an
//!   attacker in front of him; Mei's wall `0636` st5: ×0 from the maker's
//!   allies). A filter the server can't answer leaves the modifier out.
//!   `m_5E6F1B8B`, a dealt kind's filter on the target, is set nowhere in
//!   the data (`script query class StateCombatModFilter`: 0 of 34); a
//!   modifier with one is left out.
//! - Not read: `m_893AABFC` (the query doesn't read it).
//!
//! `STU_0DB9E66B` (31 nodes in 30 graphs, server-only: no registration of
//! its own, codec `none`) is the assist credit, the graphs' form of the
//! Workshop's Start Assist (Assisters, Targets: "the players whose
//! eliminations will grant assist credit to the assisters; allied: a
//! defensive assist, otherwise offensive"). `m_CABF569E` names the
//! assisters, `m_targets` the targets, `m_86DFF239` the kind, `m_43B917C6`
//! seconds the credit outlasts the state, `m_AFAB6CCD` a tag list (always
//! `2456.01C`, on the kind 1 nodes; not read). Kind 1 (11 nodes, each
//! with 3 s, 2 s in `0ECA`) is on enemies: Biotic Grenade's enemy aura
//! `0C72`, Zenyatta's Discord Orb `0176` st20, Reinhardt's `224E`; kind 2
//! (19 nodes, 0 s) on allies: Mercy's weapon graph `006B`, Harmony Orb `017B`,
//! the Biotic Field's heal `0B3C`, Biotic Grenade's ally aura `0B99`;
//! kind 3 only in `02D1`. So kind 1 is the offensive assist, kind 2 the
//! defensive one ([`ASSIST_OFFENSIVE`], [`ASSIST_DEFENSIVE`]; assumption,
//! from the targets' sides, `refs/reports/StatCredit.md`). The server
//! evaluates the assisters and targets when the state begins (assumption)
//! and the match credits the assist at a death ([`AssistLink`]).

use super::*;
use crate::weapons::DamageTags;

/// `STU_0DB9E66B` `m_86DFF239` of an offensive assist: the targets are
/// the assister's enemies, credited when one dies.
pub const ASSIST_OFFENSIVE: i64 = 1;
/// `STU_0DB9E66B` `m_86DFF239` of a defensive assist: the targets are the
/// assister's allies, credited when one of them eliminates an enemy.
pub const ASSIST_DEFENSIVE: i64 = 2;

/// The assist credit an assist state (`STU_0DB9E66B`) holds while it runs
/// and for its linger after ([`World::assist`]).
#[derive(Clone, Debug, PartialEq)]
pub struct AssistLink {
    /// `m_CABF569E`: who gets the credit (bodies, or the player entity of
    /// a field's owner).
    pub assisters: Vec<u32>,
    /// `m_targets` (the owner itself unevaluated).
    pub targets: Vec<u32>,
    /// `m_86DFF239`: [`ASSIST_OFFENSIVE`], [`ASSIST_DEFENSIVE`] or another.
    pub kind: i64,
    /// `m_43B917C6` in milliseconds: how long the credit outlasts the
    /// state.
    pub linger_ms: u64,
}

/// A combat modifier a `CombatModFilter` state holds.
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
    /// The entity whose state holds it, which it scales the changes of.
    pub owner: u32,
    /// The entity credited for what it changes (`m_F7872851`; the owner
    /// when unset; 0 when it names no entity), told by [`MODIFIED`].
    pub credit: u32,
    /// `m_E9517777` (`03A3` st59's `19C4`), handed to the credited
    /// entity with each change; null when unset.
    pub identifier: Value,
    /// `m_941C56C2`: an addition that counts toward [`REDUCTION_CAP`].
    pub capped: bool,
    /// Its filter on the event's other party (`m_6AA5D70F`), tested at
    /// the event by the graph that runs it.
    pub party: Option<PartyFilter>,
}

/// The state of a modifier with a party filter (`m_6AA5D70F`), which
/// [`Component::passes_party_filter`] tests on a health change's other
/// party: its instance and raw state on the entity that runs it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PartyFilter {
    pub instance: u16,
    pub state: u16,
}

/// The game message the entity a modifier credits takes for each change
/// the modifier made ([`Scaled::notices`]).
pub const MODIFIED: u64 = 0x0240_0000_0000_0d9a;
/// [`MODIFIED`]'s change the modifier made (`074D.026`).
const MODIFIED_CHANGE: u32 = 0x74d;
/// [`MODIFIED`]'s other party of the change (`005C.026`).
const MODIFIED_OTHER: u32 = 0x5c;
/// [`MODIFIED`]'s `m_E9517777` (`076A.026`).
const MODIFIED_IDENTIFIER: u32 = 0x76a;

/// What the modifiers make of a health change ([`combat_scale`]): its
/// scale and, per modifier that applied, its part of the change.
#[derive(Clone, Debug, PartialEq)]
pub struct Scaled {
    pub factor: f64,
    /// Each applied modifier's owner, credited entity, `m_E9517777` and
    /// part of the change per unit of the unscaled amount.
    parts: Vec<(u32, u32, Value, f64)>,
}

impl Scaled {
    /// No modifier applies.
    pub const NONE: Self = Self {
        factor: 1.0,
        parts: Vec::new(),
    };

    /// `amount` scaled.
    pub fn apply(&self, amount: f64) -> f64 {
        amount * self.factor
    }

    /// The [`MODIFIED`] notices of a change of `amount` (unscaled) whose
    /// other party is `other`: one from each applied modifier's owner to
    /// the entity it credits, but for a part of 0 or no credited entity.
    pub fn notices(&self, amount: f64, other: u32) -> impl Iterator<Item = GameMessage> + '_ {
        self.parts
            .iter()
            .filter(move |&&(_, credit, _, part)| {
                expr::valid_entity(credit) && part * amount != 0.0
            })
            .map(move |(owner, credit, identifier, part)| {
                let mut parameters = Variables::from([
                    (MODIFIED_CHANGE, Value::Float(part * amount)),
                    (MODIFIED_OTHER, Value::Entity(other)),
                ]);
                if *identifier != Value::Null {
                    parameters.insert(MODIFIED_IDENTIFIER, identifier.clone());
                }
                GameMessage {
                    sender: *owner,
                    target: *credit,
                    message: MODIFIED,
                    parameters,
                }
            })
    }
}

/// The field of a taken kind's party filter.
pub(super) const PARTY_FILTER: &str = "m_6AA5D70F";

/// The most the additions marked `m_941C56C2` reduce by together: the
/// public rule, "the combined total of general damage taken reduction
/// effects is capped at 50%", a single larger one overriding it
/// (https://overwatch.weirdgloop.org/w/Damage_modification). Assumption:
/// neither the client (no code computes a health change) nor the data
/// gives a value.
pub const REDUCTION_CAP: f64 = 0.5;

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

    /// A projectile's speed scale state (`STU_D3F71FCF`) began: its scale
    /// `m_67E8FF48` goes to the world ([`World::set_projectile_speed`]).
    /// What the client does with it is an assumption (its component 50
    /// `+0x3AC`, no reader found; BioticOrb.md §6).
    pub(super) fn begin_projectile_speed(&mut self, key: StateKey, node: &Node) {
        let site = self.site(key.instance, node);
        let scale = self
            .evaluate(
                key.instance,
                node,
                field(&node.fields, "m_67E8FF48"),
                Some(key),
            )
            .map_or(1.0, |value| expr::to_float(&value));
        self.assume(site, &node.class, "the projectile's speed scale");
        if !self.world.set_projectile_speed(site, Some(scale)) {
            self.assume(
                site,
                &node.class,
                "no effect (the world flies no projectile)",
            );
        }
    }

    /// An assist state began: its credit goes to the world
    /// ([`World::assist`]) with the assisters and targets as they are now.
    pub(super) fn begin_assist(&mut self, key: StateKey, node: &Node) {
        let id = key.instance;
        let site = self.site(id, node);
        let linger = match field(&node.fields, "m_43B917C6") {
            Json::Null => 0.0,
            config => self
                .evaluate_quietly(id, node, config)
                .map_or(0.0, |value| expr::to_float(&value)),
        };
        let link = AssistLink {
            assisters: self.entity_list(id, node, "m_CABF569E"),
            targets: self.entity_list(id, node, "m_targets"),
            kind: field(&node.fields, "m_86DFF239").as_i64().unwrap_or(0),
            linger_ms: (linger.max(0.0) * 1000.0) as u64,
        };
        let now = self.machine.now_ms;
        if !self.world.assist(site, Some(link), now) {
            self.assume(site, &node.class, "no effect (the world keeps no assist)");
        }
    }

    /// An assist state ended: its credit lingers for its time.
    pub(super) fn end_assist(&mut self, key: StateKey, node: &Node) {
        let site = self.site(key.instance, node);
        let now = self.machine.now_ms;
        self.world.assist(site, None, now);
    }

    pub(super) fn begin_combat_mod(&mut self, key: StateKey, node: &Node) {
        let id = key.instance;
        let site = self.site(id, node);
        let modifier = match self.read_combat_mod(key, node) {
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
    /// doesn't apply (a dealt kind's filter on the target, `m_5E6F1B8B`)
    /// or a value it can't evaluate.
    fn read_combat_mod(&mut self, key: StateKey, node: &Node) -> Result<CombatMod, &'static str> {
        let id = key.instance;
        let fields = &node.fields;
        let kind = asset_guid(field(fields, "m_60703B1E")).ok_or("no effect (no kind)")?;
        // The client's query tests `m_6AA5D70F` for the taken kinds and
        // `m_5E6F1B8B` for the dealt ones, each alone.
        let taken = [DAMAGE_TAKEN, HEALING_TAKEN].contains(&kind);
        if !taken && !field(fields, "m_5E6F1B8B").is_null() {
            return Err("no effect (a filter on the target)");
        }
        let party = (taken && !field(fields, PARTY_FILTER).is_null()).then_some(PartyFilter {
            instance: id,
            state: key.raw,
        });
        let site = self.site(id, node);
        // The client's init: an evaluated value's entity (none for a null
        // value), the owner when unset or when it can't evaluate.
        let credit = match field(fields, "m_F7872851") {
            Json::Null => site.entity,
            credit => self
                .evaluate(id, node, credit, None)
                .map_or(site.entity, |credit| {
                    expr::to_entity(&credit)
                        .filter(|&credit| expr::valid_entity(credit))
                        .unwrap_or(0)
                }),
        };
        let identifier = match field(fields, "m_E9517777") {
            Json::Null => Value::Null,
            identifier => self
                .evaluate(id, node, identifier, None)
                .unwrap_or(Value::Null),
        };
        let value = self
            .evaluate(id, node, field(fields, "m_50530EAB"), None)
            .ok_or("no effect (an unevaluated value)")?;
        let capped = match field(fields, "m_941C56C2") {
            Json::Null => false,
            capped => self
                .evaluate(id, node, capped, None)
                .is_some_and(|capped| truthy(&capped)),
        };
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
            owner: site.entity,
            credit,
            identifier,
            capped,
            party,
        })
    }
}

/// The statuses and combat modifiers a body's states hold now, for its
/// world to keep and the match's health code to read.
#[derive(Debug, Default)]
pub struct BodyEffects {
    statuses: Vec<(Site, i64)>,
    /// The modifiers its states hold.
    modifiers: Vec<(Site, CombatMod)>,
    /// The assist credits and, for one whose state ended, when its linger
    /// ends.
    assists: Vec<(Site, AssistLink, Option<u64>)>,
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

    /// [`World::assist`] at `now_ms`: a credit is kept while its state
    /// runs, and after it ends for its linger (none: gone at once).
    pub fn assist(&mut self, site: Site, link: Option<AssistLink>, now_ms: u64) -> bool {
        self.assists
            .retain(|(_, _, until)| until.is_none_or(|until| until >= now_ms));
        let running = self
            .assists
            .iter()
            .position(|(held, _, until)| *held == site && until.is_none());
        if let Some(index) = running {
            let linger = self.assists[index].1.linger_ms;
            if linger == 0 {
                self.assists.remove(index);
            } else {
                self.assists[index].2 = Some(now_ms + linger);
            }
        }
        if let Some(link) = link {
            self.assists.push((site, link, None));
        }
        true
    }

    /// The assist credits held at `now_ms`, the lingering ones included.
    pub fn assists(&self, now_ms: u64) -> impl Iterator<Item = &AssistLink> {
        self.assists
            .iter()
            .filter(move |(_, _, until)| until.is_none_or(|until| until >= now_ms))
            .map(|(_, link, _)| link)
    }

    /// The body takes no damage (a status of type 0 or 1).
    pub fn invulnerable(&self) -> bool {
        !self.statuses.is_empty()
    }

    /// The modifiers its states hold now, in the order they came.
    pub fn modifiers(&self) -> impl Iterator<Item = &CombatMod> {
        self.modifiers.iter().map(|(_, modifier)| modifier)
    }

    /// The party filters of the body's modifiers of `kind`, for the world
    /// that runs them to test on the event's other party
    /// ([`Component::passes_party_filter`]).
    pub fn party_filters(&self, kind: u64) -> impl Iterator<Item = PartyFilter> + '_ {
        self.modifiers()
            .filter(move |modifier| modifier.kind == kind)
            .filter_map(|modifier| modifier.party)
    }

    /// [`combat_scale`] of the body's modifiers, the party filters in
    /// `passed` passing.
    pub fn scale(&self, kind: u64, tags: &mut DamageTags, passed: &[PartyFilter]) -> Scaled {
        combat_scale(self.modifiers(), kind, tags, passed)
    }
}

/// What `modifiers` make of a health change of `kind` with `tags` (see the
/// module notes): of those of that kind whose required tags (if any) meet
/// the tags, that leave none of them alone and whose party filter (if
/// any) is in `passed`, in the order they came, one plus the additions
/// (those marked `m_941C56C2` together reducing by at most
/// [`REDUCTION_CAP`] unless one alone reduces by more), never below 0,
/// times the multipliers; with each one's part of the change. Each that
/// applies adds its tags to `tags`.
pub fn combat_scale<'a>(
    modifiers: impl IntoIterator<Item = &'a CombatMod>,
    kind: u64,
    tags: &mut DamageTags,
    passed: &[PartyFilter],
) -> Scaled {
    let mut applied: Vec<&CombatMod> = Vec::new();
    for modifier in modifiers
        .into_iter()
        .filter(|modifier| modifier.kind == kind)
    {
        let required = modifier.required.is_empty()
            || modifier
                .required
                .iter()
                .any(|&tag| tags.iter().any(|held| held == tag));
        if !required
            || tags.iter().any(|tag| modifier.exempt.contains(&tag))
            || modifier
                .party
                .is_some_and(|filter| !passed.contains(&filter))
        {
            continue;
        }
        for &tag in &modifier.added {
            tags.push(tag);
        }
        applied.push(modifier);
    }
    let additions = || applied.iter().filter(|modifier| !modifier.multiplies);
    let capped: f64 = additions()
        .filter(|modifier| modifier.capped)
        .map(|modifier| modifier.value)
        .sum();
    let deepest = additions()
        .filter(|modifier| modifier.capped)
        .fold(0.0_f64, |deepest, modifier| deepest.min(modifier.value));
    let kept = f64::max(capped, deepest.min(-REDUCTION_CAP));
    let added = additions()
        .filter(|modifier| !modifier.capped)
        .map(|modifier| modifier.value)
        .sum::<f64>()
        + kept;
    let sum = f64::max(1.0 + added, 0.0);
    // An addition's part is its value, the capped ones' cut to what the
    // cap kept and all of them to what the floor at 0 kept; a multiplier's
    // the change it makes to what came before it.
    let ratio = |kept: f64, raw: f64| if raw == 0.0 { 0.0 } else { kept / raw };
    let floor = ratio(sum - 1.0, added);
    let cut = ratio(kept, capped);
    let mut product = 1.0;
    let parts = applied
        .iter()
        .map(|modifier| {
            let part = if modifier.multiplies {
                let part = sum * product * (modifier.value - 1.0);
                product *= modifier.value;
                part
            } else if modifier.capped {
                modifier.value * cut * floor
            } else {
                modifier.value * floor
            };
            (
                modifier.owner,
                modifier.credit,
                modifier.identifier.clone(),
                part,
            )
        })
        .collect();
    Scaled {
        factor: f64::max(sum * product, 0.0),
        parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An assist credit holds while its state runs and, once it ends, for
    /// its linger (Discord's 3 s), then goes; one without a linger goes as
    /// its state ends.
    #[test]
    fn an_assist_credit_outlasts_its_state_by_its_linger() {
        let site = |node| Site {
            entity: 0xa000_0001,
            instance: 1,
            graph: 0x0176,
            node: Some(node),
        };
        let link = |linger_ms| AssistLink {
            assisters: vec![0xa000_0001],
            targets: vec![0xa001_0001],
            kind: ASSIST_OFFENSIVE,
            linger_ms,
        };
        let mut effects = BodyEffects::default();
        effects.assist(site(40), Some(link(3000)), 1000);
        effects.assist(site(41), Some(link(0)), 1000);
        assert_eq!(effects.assists(1500).count(), 2);
        effects.assist(site(40), None, 2000);
        effects.assist(site(41), None, 2000);
        assert_eq!(
            effects.assists(5000).cloned().collect::<Vec<_>>(),
            [link(3000)]
        );
        assert_eq!(effects.assists(5001).count(), 0);
    }
}
