//! Target searches: `STUStatescriptStateTrackTargets` (OWLib's name, hash
//! 41E3C7A7). While the state runs it keeps the bodies within a sphere that
//! pass its filter, writes them to its output variables and sends them in
//! its frames (the `targets` payload: each entity and its age).
//!
//! What the fields mean, read from the 292 uses in the hero and bot bundles
//! (`game_graphs_174.json.gz`, `game_graphs_bots_174.json.gz`; 157 and 135):
//!
//! - `m_8DCA77B6`: the entity the sphere goes with (187 uses: always the
//!   owner). `m_05BE62DD` is then an offset from that entity's feet: the
//!   Biotic Field (`0258` st0) and Soldier: 76's ally count (`0257` st6)
//!   use (0, 1, 0), a metre up, and quick melee (`0043` st5) adds
//!   (0, 0, 1.5) to the eye's height above the feet (`STU_21A13145`, the
//!   eye, less `STU_71A87E92`, the feet), 1.5 m ahead. Without
//!   the entity, `m_05BE62DD` is a world point (an entity's position
//!   config). Fairly sure.
//! - `m_radius`: the sphere's radius in metres (Biotic Field `#422` = 5,
//!   the radius the match's `heal_area` uses; `0257` st6 10). Sure.
//! - `m_A30B383E`: 360 in 47 uses, 90 or 180 or an expression in others: a
//!   cone's angle in degrees, 360 for the whole sphere. Fairly sure.
//! - `m_8A2905A0`: the found entities as an array (283 uses, `#375` in 143):
//!   ModifyHealth's `m_FA3ECC43` and ApplyAura's `m_targets` read it, `0043`
//!   counts it (opcode 18) and watches it grow. Sure.
//! - `m_1B89EA79`: their count (17 uses): `0257` and Reinhardt's `0095`
//!   test `#1192 > 0` before a criteria. Sure.
//! - `m_filter` (`STUConfigVarFilter`), tested as the client's
//!   `Filter_TestTeamMasks` 0x7FF789ADAAE0 (called by
//!   `ScriptHost_EntityPassesFilter` 0x7FF789C1DA20; HealthAuras.md,
//!   "Client tools pass 2 (2026-11-16)"): three masks against the
//!   candidate's filter word, and three relation words. A candidate passes
//!   when the any-of test passes, the all-of test passes and the none-of
//!   test doesn't hit:
//!   - `m_84065FD1` any-of: the word shares a bit with it, or it is
//!     `0x3FFFFFFF` (every kind); `m_1E14CCA8` all-of: the word has all its
//!     bits (0: no test); `m_4EA686DB` none-of: a bit in common fails the
//!     candidate.
//!   - The relation words, read only when one is set: bit 1 = friendly
//!     (not hostile, or the searcher itself), 2 = hostile, 4 = the searcher.
//!     `m_27CF71D5` include: a relation in it passes the any-of test;
//!     `m_66747E7D` require: bit 1 fails a hostile candidate, 2 a friendly
//!     one, 4 any but the searcher; `m_89831602` exclude: a relation in it
//!     fails the candidate. So quick melee (`0043` st5, exclude 1) finds
//!     enemies only, Brigitte (`12BE` st19, `0D00` st1, exclude 2) allies
//!     with herself, Moira (`1007` st17, exclude 6) allies without
//!     herself, and the ally counts (`0257` st6: require 1, exclude 4)
//!     allies without the caster.
//!   - Hostile is the enemy test of `STU_53F4CA65` (`sub_7FF789B2CF60`):
//!     the searcher counts as its owner when it has one (a projectile as
//!     its thrower), that owner is never hostile, and teams -1 and 4 (a
//!     free-for-all body, which lists no team) are hostile to everyone
//!     else. The client uses another test for candidates with word bit
//!     `0x10000000`; no body carries it.
//!   - The candidate's word is the client's component 25 `+0x58`; the
//!     server takes it to be the body's filter bits as it sends them
//!     (component 26, `orpheus_sim::scene::body_filter`): its team's bit
//!     `0x800000 << team` (team 4 in a free-for-all), the kinds `0x41` (1 a
//!     hero, 64 a body that can be hit) and 16 once dead. So `m_84065FD1` 1
//!     or 64 finds bodies, `m_4EA686DB` 16 leaves the dead out, the
//!     objective graph `1225`'s `m_1E14CCA8` `0x800000 << team` finds a
//!     team's bodies, and bits no body carries (none-of 4 in the hits,
//!     `0x200000` in heals) change nothing. Assumption: the two words are
//!     the same (`orpheus_sim::scene` module notes).
//!   - The client also requires component 97 when a mask is set and the
//!     any-of mask isn't every kind (bodies have it), and then tests the
//!     condition and the tag lists (component 76); the condition and the
//!     tag lists are not modelled.
//!
//! The server runs the forms whose set fields are only these (an entity
//! sphere with a vertical or forward offset, or a world point; a whole
//! sphere; the masks and relation words): `0257`
//! st6 and Reinhardt's `0095` st18, both counting allies within 10 m for a
//! criteria, and quick melee's `0043` st5, whose line of sight
//! (`m_D59188D5`, `m_3D667CE3`) isn't tested and whose output `m_24A4AB8A`
//! isn't written (assumptions, as the Python reference). A forward offset
//! goes along the entity's aim. Any other set field (sweep origins,
//! scoring `m_84E5DE4E`, limits, the output `m_CEE20F6F` that SetVars
//! read) or an offset the server can't evaluate leaves the search finding
//! nothing, recorded as an assumption naming the field.
//! The ten byte and enum fields after `m_2A0FB8BB` are in every use with
//! values whose meaning is open; they are not read.
//!
//! Assumed, as nothing in the data says: a body counts when its capsule
//! (as the match places it) comes within the radius, as `heal_area`
//! measures; new bodies come after those already found, in the world's
//! order; the search runs at the state's begin and each tick; the outputs
//! are written at the begin and whenever the set changes, and keep their
//! last value when the state ends; a target's age in the payload is the
//! milliseconds since it came in.

use super::*;

const CLASS: &str = "STUStatescriptStateTrackTargets";

/// The class's own fields the server doesn't model: set, the search finds
/// nothing.
const UNMODELLED: &[&str] = &[
    "m_47AB1887",
    "m_E46A19A3",
    "m_3119AD61",
    "m_B9BEAC7F",
    "m_D45E22FA",
    "m_2F88C5F2",
    "m_544C3E6F",
    "m_762A56C0",
    "m_B3A77CD0",
    "m_16BDE607",
    "m_8862A75D",
    "m_E474FFD6",
    "m_48837057",
    "m_294660A6",
    "m_229DD034",
    "m_BA6C9F75",
    "m_9E1426F6",
    "m_E8474EE8",
    "m_C1B724B2",
    "m_93AB8C13",
    "m_C39691D9",
    "m_CEE20F6F",
    "m_84E5DE4E",
    "m_4A6E0895",
    "m_8CBE558B",
    "m_2A0FB8BB",
];

/// The filter's fields the server doesn't model.
const UNMODELLED_FILTER: &[&str] = &[
    "m_93FA1642",
    "m_condition",
    "m_9C7BD8B8",
    "m_B486603E",
    "m_8B2A4DF3",
    "m_C1903771",
    "m_F61726FE",
    "m_B40362FC",
];

/// A body's filter word (see the module notes): its team's bit
/// `TEAM_BIT << team`, team 4 in a free-for-all, the kinds of a living body
/// and [`DEAD`] once it died.
const TEAM_BIT: u64 = 0x0080_0000;
const FREE_FOR_ALL_TEAM: u8 = 4;
const BODY_KINDS: u64 = 0x1_0041;
const DEAD: u64 = 0x10;

/// Fields set in quick melee's search (`0043` st5) that the server runs
/// without: the line of sight from `m_D59188D5` (the eye, `STU_21A13145`)
/// with the collision filter `m_3D667CE3` (`0690.002B`), which the Python
/// reference didn't test either, and the output `m_24A4AB8A` (`#23530`,
/// which only the ModifyHealth's unread `m_B12BFE8B` reads). Each set one
/// is an assumption.
const UNREAD: &[(&str, &str)] = &[
    ("m_D59188D5", "no line-of-sight test"),
    ("m_3D667CE3", "no line-of-sight test"),
    ("m_24A4AB8A", "not written"),
];

/// Why a search finds nothing.
pub(super) enum Gap {
    /// A field the server doesn't model is set, or doesn't evaluate.
    Field(&'static str),
    /// A filter field the server doesn't model.
    Filter(&'static str),
    /// The world lists no bodies.
    NoBodies,
}

/// A search's sphere and filter, as evaluated now.
struct Search {
    entity: Option<u32>,
    offset: [f64; 3],
    radius: f64,
    filter: BodyFilter,
}

/// A `STUConfigVarFilter`'s masks and relation words, the part of it a
/// body search tests (see the module notes); the ray casts
/// (`vm/rays.rs`) test bodies with it too.
pub(super) struct BodyFilter {
    masks: expr::FilterMasks,
}

impl BodyFilter {
    /// The filter's masks and words, or the first filter field the server
    /// doesn't model ([`UNMODELLED_FILTER`] less `allowed`, which the
    /// caller reads itself). A missing filter passes no body.
    pub(super) fn read(filter: &Json, allowed: &[&str]) -> Result<Self, &'static str> {
        if let Some(name) = UNMODELLED_FILTER
            .iter()
            .find(|name| !allowed.contains(name) && is_set(field(filter, name)))
        {
            return Err(name);
        }
        Ok(Self {
            masks: expr::FilterMasks::words(filter),
        })
    }

    /// Whether `candidate` passes the masks and relation words, as
    /// `Filter_TestTeamMasks`; `hostile` and `searcher` are its relation to
    /// the searching entity (relations matter only when a word is set).
    fn passes(&self, candidate: &TargetCandidate, hostile: bool, searcher: bool) -> bool {
        self.masks
            .passes(filter_word(candidate), !hostile || searcher, searcher)
    }

    fn relations(&self) -> bool {
        self.masks.relations()
    }

    /// Whether no body can pass: no any-of mask and no include word (the
    /// map-only filters of most ray casts).
    fn passes_none(&self) -> bool {
        self.masks.passes_none()
    }
}

/// A body's filter word, as the match sends it (see the module notes).
fn filter_word(candidate: &TargetCandidate) -> u64 {
    let team = candidate.team.unwrap_or(FREE_FOR_ALL_TEAM);
    let dead = if candidate.dead { DEAD } else { 0 };
    TEAM_BIT << team | BODY_KINDS | dead
}

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    /// The search at the state's begin; a form the server runs searches
    /// again each tick.
    pub(super) fn begin_track_targets(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        if self.track_targets(key, node, true)? {
            self.register_tick(key, self.machine.now_ms);
        }
        Ok(())
    }

    /// Search and keep the result; true when the form runs.
    pub(super) fn track_targets(
        &mut self,
        key: StateKey,
        node: &Node,
        begin: bool,
    ) -> Result<bool, InitFault> {
        let site = self.site(key.instance, node);
        let found = match self.search_targets(key.instance, node) {
            Ok(found) => found,
            Err(gap) => {
                match gap {
                    Gap::Field(name) => {
                        self.assume(site, &format!("{CLASS} {name}"), "finds no targets")
                    }
                    Gap::Filter(name) => self.assume(
                        site,
                        &format!("{CLASS} m_filter.{name}"),
                        "finds no targets",
                    ),
                    Gap::NoBodies => self.assume(
                        site,
                        &format!("{CLASS} bodies"),
                        "finds no targets (the world lists none)",
                    ),
                }
                return Ok(false);
            }
        };
        self.assume(site, CLASS, "a sphere search (flag fields unread)");
        for (name, answer) in UNREAD {
            if is_set(field(&node.fields, name)) {
                self.assume(site, &format!("{CLASS} {name}"), answer);
            }
        }
        let now = self.machine.now_ms;
        let state = self.state_mut(key).expect("active TrackTargets");
        let before: Vec<u32> = state.tracked.iter().map(|target| target.entity).collect();
        state
            .tracked
            .retain(|target| found.contains(&target.entity));
        for entity in found {
            if !state.tracked.iter().any(|target| target.entity == entity) {
                state.tracked.push(TrackedTarget {
                    entity,
                    since_ms: now,
                });
            }
        }
        let after: Vec<u32> = state.tracked.iter().map(|target| target.entity).collect();
        if begin || after != before {
            self.write_targets(key.instance, node, &after)?;
        }
        Ok(true)
    }

    fn write_targets(&mut self, id: u16, node: &Node, entities: &[u32]) -> Result<(), InitFault> {
        let outputs = [
            (
                "m_8A2905A0",
                Value::Array(
                    entities
                        .iter()
                        .map(|&entity| Value::Entity(entity))
                        .collect(),
                ),
            ),
            ("m_1B89EA79", Value::Integer(entities.len() as i64)),
        ];
        for (name, value) in outputs {
            match expr::lvalue(field(&node.fields, name)) {
                Ok(Some(variable)) => self.write(source_key(id, variable), value)?,
                Ok(None) => (),
                Err(error) => self.report(self.site(id, node), Issue::Expression(error)),
            }
        }
        Ok(())
    }

    /// The bodies the search finds now, in the world's order.
    fn search_targets(&mut self, id: u16, node: &Node) -> Result<Vec<u32>, Gap> {
        let search = self.target_search(id, node)?;
        let site = self.site(id, node);
        let bodies = self.passing_bodies(site, &search.filter)?;
        let centre = match search.entity {
            Some(entity) => {
                let candidates = self.world.target_candidates(site).ok_or(Gap::NoBodies)?;
                let body = candidates
                    .iter()
                    .find(|candidate| candidate.entity == entity)
                    .ok_or(Gap::Field("m_8DCA77B6"))?;
                std::array::from_fn(|axis| body.feet[axis] + search.offset[axis])
            }
            None => search.offset,
        };
        Ok(bodies
            .iter()
            .filter(|candidate| candidate.distance(centre) <= search.radius)
            .map(|candidate| candidate.entity)
            .collect())
    }

    /// The bodies the world lists that pass `filter` for a search on
    /// `site`, in the world's order.
    pub(super) fn passing_bodies(
        &mut self,
        site: Site,
        filter: &BodyFilter,
    ) -> Result<Vec<TargetCandidate>, Gap> {
        if filter.passes_none() {
            return Ok(Vec::new());
        }
        // The searcher counts as its owner in the enemy test, as in
        // `STU_53F4CA65`.
        let owner = self
            .world
            .related_entity(site, EntityRelation::Owner, site.entity)
            .filter(|&owner| expr::valid_entity(owner))
            .unwrap_or(site.entity);
        let candidates = self.world.target_candidates(site).ok_or(Gap::NoBodies)?;
        let owner = candidates
            .iter()
            .find(|candidate| candidate.entity == owner);
        if filter.relations() && owner.is_none() {
            return Err(Gap::Filter("m_66747E7D"));
        }
        let hostile = |candidate: &TargetCandidate| {
            owner.is_some_and(|owner| {
                candidate.entity != owner.entity
                    && (owner.team.is_none() || owner.team != candidate.team)
            })
        };
        Ok(candidates
            .iter()
            .filter(|candidate| {
                filter.passes(
                    candidate,
                    hostile(candidate),
                    candidate.entity == site.entity,
                )
            })
            .copied()
            .collect())
    }

    /// The form's sphere and filter, or the first part the server can't run.
    fn target_search(&mut self, id: u16, node: &Node) -> Result<Search, Gap> {
        let fields = &node.fields;
        if let Some(name) = UNMODELLED.iter().find(|name| is_set(field(fields, name))) {
            return Err(Gap::Field(name));
        }
        let angle = field(fields, "m_A30B383E");
        if is_set(angle)
            && self
                .evaluate_quietly(id, node, angle)
                .is_none_or(|angle| expr::to_float(&angle) < 360.0)
        {
            return Err(Gap::Field("m_A30B383E"));
        }
        let filter = BodyFilter::read(field(fields, "m_filter"), &[]).map_err(Gap::Filter)?;
        let entity = match field(fields, "m_8DCA77B6") {
            Json::Null => None,
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Entity(entity)) if expr::valid_entity(entity) => Some(entity),
                _ => return Err(Gap::Field("m_8DCA77B6")),
            },
        };
        let Some(Value::Vector(mut offset)) =
            self.evaluate_quietly(id, node, field(fields, "m_05BE62DD"))
        else {
            return Err(Gap::Field("m_05BE62DD"));
        };
        if let Some(entity) = entity {
            // A sideways offset turns with the entity by a rule nothing in
            // the data shows. A forward one (quick melee's 1.5 m) goes along
            // where the entity looks, its pitch included, as the Python
            // reference aimed quick melee (`combat.py` `_melee`: the eye
            // plus the aim times the reach); assumption.
            if offset[0] != 0.0 {
                return Err(Gap::Field("m_05BE62DD"));
            }
            if offset[2] != 0.0 {
                let facing = self
                    .world
                    .entity_facing(self.site(id, node), entity)
                    .ok_or(Gap::Field("m_05BE62DD"))?;
                let aim = expr::aim(facing);
                let ahead = offset[2];
                offset = [aim[0] * ahead, offset[1] + aim[1] * ahead, aim[2] * ahead];
            }
        }
        let radius = self
            .evaluate_quietly(id, node, field(fields, "m_radius"))
            .map(|radius| expr::to_float(&radius))
            .filter(|radius| radius.is_finite() && *radius >= 0.0)
            .ok_or(Gap::Field("m_radius"))?;
        Ok(Search {
            entity,
            offset,
            radius,
            filter,
        })
    }

    /// A config's value now, or None where the server can't evaluate it; a
    /// search that can't run finds nothing instead of stopping the body.
    pub(super) fn evaluate_quietly(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
    ) -> Option<Value> {
        let site = self.site(id, node);
        let mut context = InstanceContext {
            machine: self.machine,
            world: self.world,
            site,
            reads: Vec::new(),
            parameters: self.parameters.as_deref(),
            candidate: self.candidate.as_ref(),
            watcher: None,
            read_native: false,
            assumed: Vec::new(),
        };
        let result = expr::evaluate(config, &mut context);
        for (class, answer) in context.assumed {
            self.assume(site, &class, answer);
        }
        result.ok()
    }
}

/// A config that says something: not null, and not a literal zero or false.
pub(super) fn is_set(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(value) => *value,
        Json::Number(value) => value.as_f64() != Some(0.0),
        Json::Array(items) => !items.is_empty(),
        Json::Object(_) => {
            !matches!(
                field(value, "$").as_str(),
                Some("STUConfigVarInt" | "STUConfigVarFloat" | "STUConfigVarBool")
            ) || is_set(field(value, "m_value"))
        }
        Json::String(text) => !text.is_empty(),
    }
}
