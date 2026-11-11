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
//!   (0, 0, 1.5) to the eye's height above the feet, 1.5 m ahead
//!   (`combat::melee_states` in `orpheus_sim` reads that reach). Without
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
//! - `m_filter` (`STUConfigVarFilter`):
//!   - `m_66747E7D`, the team relation to the searching body: 1 = allies
//!     (the heals: the Biotic Field `0258`, Lúcio `0266`, Brigitte `12D8`,
//!     Baptiste `19A5`), 2 = enemies (Pharah `002B`, Reaper `0044`, Hanzo
//!     `0101`, Cassidy `022F`, Widowmaker `0067`), 0 = either. Fairly sure.
//!   - `m_89831602` bit 4: leaves the searching body out. Every search for
//!     an ally other than oneself has it (Mercy's beam `006E`, Zarya's
//!     barrier `025C`, Zenyatta's orb `02F3`, Ana `0B9E`, Brigitte's pack
//!     `0EDD`), the heals that also heal their caster don't (`0258`,
//!     `0266`, `19A5`, `12D8`). So `0257` st6 counts the allies near the
//!     caster, not the caster. Fairly sure. Bits 1 (quick melee, Reinhardt's
//!     charge) and 2 (Brigitte `12BE`, Moira `1007`) are open.
//!   - `m_84065FD1`: which kinds of entity: 1 in the heals and the ally
//!     counts, 64 in the melee and charge hits, `0x3FFFFFFF` all. Players'
//!     bodies and bots are taken to be of kinds 1 and 64. Fairly sure.
//!   - `m_4EA686DB`: exclusions; bit 16 is in nearly every search for
//!     heroes, heals and hits alike, and is taken to leave the dead out (as
//!     the match's heals and hits do). Other bits (4 in hits, `0x200000`
//!     in heals, 128, 256) are open.
//!   - `m_1E14CCA8` (team filter bits `0x800000 << team` in the objective
//!     graph `1225`, small values elsewhere), `m_27CF71D5`, the condition
//!     and the tag lists (as `STU_AC829876` has them) are not modelled.
//!
//! The server runs the forms whose set fields are only these (an entity
//! sphere with a vertical offset, or a world point; a whole sphere; a
//! filter of kinds 1 or 64 or all, exclusion bit 16, a team relation and
//! bit 4): `0257` st6 and Reinhardt's `0095` st18, both counting allies
//! within 10 m for a criteria. Any other set field (line of sight or
//! sweep origins, a second filter `m_3D667CE3` with collision assets,
//! scoring `m_84E5DE4E`, limits, the outputs `m_24A4AB8A` and `m_CEE20F6F`
//! that ModifyHealth's `m_B12BFE8B` and SetVars read) or an offset the
//! server can't evaluate (quick melee's needs the eye and the aim) leaves
//! the search finding nothing, recorded as an assumption naming the field.
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
    "m_D59188D5",
    "m_48837057",
    "m_294660A6",
    "m_3D667CE3",
    "m_229DD034",
    "m_BA6C9F75",
    "m_9E1426F6",
    "m_E8474EE8",
    "m_C1B724B2",
    "m_93AB8C13",
    "m_C39691D9",
    "m_CEE20F6F",
    "m_24A4AB8A",
    "m_84E5DE4E",
    "m_4A6E0895",
    "m_8CBE558B",
    "m_2A0FB8BB",
];

/// The filter's fields the server doesn't model.
const UNMODELLED_FILTER: &[&str] = &[
    "m_93FA1642",
    "m_1E14CCA8",
    "m_27CF71D5",
    "m_condition",
    "m_9C7BD8B8",
    "m_B486603E",
    "m_8B2A4DF3",
    "m_C1903771",
    "m_F61726FE",
    "m_B40362FC",
];

/// `m_84065FD1`: every kind.
const ALL_KINDS: u64 = 0x3fff_ffff;
/// `m_84065FD1` bits that take players' bodies and bots.
const BODY_KINDS: u64 = 1 | 64;
/// `m_4EA686DB`: the dead are left out.
const EXCLUDE_DEAD: u64 = 16;
/// `m_89831602`: the searching body is left out.
const EXCLUDE_SELF: u64 = 4;

/// Why a search finds nothing.
enum Gap {
    /// A field the server doesn't model is set, or doesn't evaluate.
    Field(&'static str),
    /// A filter field the server doesn't model.
    Filter(&'static str),
    /// The world lists no bodies.
    NoBodies,
}

#[derive(Clone, Copy, PartialEq)]
enum Relation {
    Any,
    Allies,
    Enemies,
}

/// A search's sphere and filter, as evaluated now.
struct Search {
    entity: Option<u32>,
    offset: [f64; 3],
    radius: f64,
    relation: Relation,
    exclude_dead: bool,
    exclude_self: bool,
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
        let candidates = self.world.target_candidates(site).ok_or(Gap::NoBodies)?;
        let centre = match search.entity {
            Some(entity) => {
                let body = candidates
                    .iter()
                    .find(|candidate| candidate.entity == entity)
                    .ok_or(Gap::Field("m_8DCA77B6"))?;
                std::array::from_fn(|axis| body.feet[axis] + search.offset[axis])
            }
            None => search.offset,
        };
        let owner = candidates
            .iter()
            .find(|candidate| candidate.entity == site.entity);
        let ally = |candidate: &TargetCandidate| {
            candidate.entity == site.entity
                || owner.is_some_and(|owner| owner.team.is_some() && owner.team == candidate.team)
        };
        if search.relation != Relation::Any && owner.is_none() {
            return Err(Gap::Filter("m_66747E7D"));
        }
        Ok(candidates
            .iter()
            .filter(|candidate| !(search.exclude_dead && candidate.dead))
            .filter(|candidate| !(search.exclude_self && candidate.entity == site.entity))
            .filter(|candidate| match search.relation {
                Relation::Any => true,
                Relation::Allies => ally(candidate),
                Relation::Enemies => !ally(candidate),
            })
            .filter(|candidate| candidate.distance(centre) <= search.radius)
            .map(|candidate| candidate.entity)
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
        let filter = field(fields, "m_filter");
        if let Some(name) = UNMODELLED_FILTER
            .iter()
            .find(|name| is_set(field(filter, name)))
        {
            return Err(Gap::Filter(name));
        }
        let number = |name: &str| field(filter, name).as_u64().unwrap_or(0);
        let kinds = number("m_84065FD1");
        if kinds != ALL_KINDS && kinds & BODY_KINDS == 0 {
            return Err(Gap::Filter("m_84065FD1"));
        }
        let excluded = number("m_4EA686DB");
        if excluded & !EXCLUDE_DEAD != 0 {
            return Err(Gap::Filter("m_4EA686DB"));
        }
        let flags = number("m_89831602");
        if flags & !EXCLUDE_SELF != 0 {
            return Err(Gap::Filter("m_89831602"));
        }
        let relation = match number("m_66747E7D") {
            0 => Relation::Any,
            1 => Relation::Allies,
            2 => Relation::Enemies,
            _ => return Err(Gap::Filter("m_66747E7D")),
        };
        let entity = match field(fields, "m_8DCA77B6") {
            Json::Null => None,
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Entity(entity)) if expr::valid_entity(entity) => Some(entity),
                _ => return Err(Gap::Field("m_8DCA77B6")),
            },
        };
        let Some(Value::Vector(offset)) =
            self.evaluate_quietly(id, node, field(fields, "m_05BE62DD"))
        else {
            return Err(Gap::Field("m_05BE62DD"));
        };
        // An offset across the entity's frame turns with it, by a rule
        // nothing in the data shows.
        if entity.is_some() && (offset[0] != 0.0 || offset[2] != 0.0) {
            return Err(Gap::Field("m_05BE62DD"));
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
            relation,
            exclude_dead: excluded & EXCLUDE_DEAD != 0,
            exclude_self: flags & EXCLUDE_SELF != 0,
        })
    }

    /// A config's value now, or None where the server can't evaluate it; a
    /// search that can't run finds nothing instead of stopping the body.
    fn evaluate_quietly(&mut self, id: u16, node: &Node, config: &Json) -> Option<Value> {
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
fn is_set(value: &Json) -> bool {
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
