//! Target searches: `STUStatescriptStateTrackTargets` (OWLib's name, hash
//! 41E3C7A7). While the state runs it keeps the bodies within a sphere that
//! pass its filter and writes them to its output variables; its frames
//! carry only the dropped targets it still holds (see `m_93AB8C13`).
//!
//! What the fields mean, read from the 292 uses in the hero and bot bundles
//! (`game_graphs_174.json.gz`, `game_graphs_bots_174.json.gz`; 157 and 135):
//!
//! - `m_8DCA77B6`: the entity the sphere goes with (187 uses: always the
//!   owner). `m_05BE62DD` is then an offset in that entity's transform
//!   (the centre setup `0x7FF789BAA430`: the model's matrix, so it turns
//!   with the entity's yaw: forward `+z`, up `+y`): the Biotic Field
//!   (`0258` st0) and Soldier: 76's ally count (`0257` st6) use (0, 1, 0),
//!   a metre up, and quick melee (`0043` st5) adds (0, 0, 1.5) to the
//!   eye's height above the feet (`STU_21A13145`, the eye, less
//!   `STU_71A87E92`, the feet), 1.5 m ahead. With `m_0297060F` the offset
//!   also turns with the entity's pitch, about its eye height
//!   (`0x7FF789CAF3F0`): quick melee's centre goes 1.5 m along the aim
//!   from the eye. Without the entity, `m_05BE62DD` is a world point (an
//!   entity's position config); with `m_0297060F` it would be in the
//!   searcher's own transform (not modelled: finds nothing).
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
//!   - The client's filter test (`ScriptHost_EntityPassesFilter`
//!     0x7FF789C1DA20) also requires component 97 when a mask is set and
//!     the any-of mask isn't every kind (bodies have it), then tests the
//!     condition, then the tag lists (`0x7FF789C1EC70`: a tag of
//!     `m_9C7BD8B8`, every tag of `m_B486603E`, no tag of `m_8B2A4DF3`, as
//!     component 76 gives a body's tags). The server tests the condition
//!     (`m_condition`) with each candidate as every parameter, as
//!     FilterArray does (`vm/filter_array.rs`; the spawn room `0D62` reads
//!     it as parameter 8: its team and whether its feet are in the room's
//!     volume, `STU_73E1C5F0`), and the tag lists with the world's target
//!     tags ([`World::has_target_tag`]); a condition it can't evaluate or
//!     a tag the world can't tell leaves the search finding nothing.
//!
//! The rest of the class, from the client's search `0x7FF789BBE3B0` (run
//! by the begin `0x7FF789BA3570` and the update slot 14 `0x7FF789BA5590`;
//! bodies `0x7FF789BBACF0`, line of sight `0x7FF789BB3570`, frame writer
//! `0x7FF789BA5F80`):
//!
//! - When it searches: at its begin, then (slot 14, at a frame's last
//!   update) on the frames the gate `0x7FF789BB9460` lets through:
//!   `m_DD792FF0` 0 every frame; 1, 2, 3 or 4 every 3, 6, 12 or 30 frames,
//!   on the frames where `(frame + h) as i32 % n == 0`, `h` the FNV-1a hash
//!   of the four bytes of the entity id xor the node's `m_871A8203` (its
//!   rid). The spawn rooms' `0D62` searches use 3 (12 frames).
//! - `m_32AFE9AA`: each search tests the found bodies again and drops
//!   those that no longer pass; without it a found body stays until the
//!   state ends (quick melee: each body struck once a swing).
//! - `m_93AB8C13`: a dropped body stays a target for that many seconds;
//!   those waiting bodies, with the milliseconds since they were dropped,
//!   are the frame's `targets` list, so it is empty without it. Not
//!   modelled (finds nothing when set).
//! - `m_43478CAD`: the targets' order (with the scoring `m_84E5DE4E` and
//!   the limit `m_C39691D9`, `0x7FF789B9A0C0`); 0 keeps them in the order
//!   found. Others are not modelled (find nothing).
//! - `m_95FFC3C5`: a line-of-sight test of each new body (and, with
//!   `m_32AFE9AA`, the found ones): rays from `m_D59188D5` (else the
//!   centre) through the collision of `m_3D667CE3`'s profile (else
//!   `0690.002D`) to 0.3 and 0.6 of the body's height (or its hardpoints
//!   `0DC0.0001`/`0DC0.01CD`); `m_8D13DD07` adds a sweep first and
//!   `m_2F5FAF65` two rays to the sides. Not modelled: a search with it is
//!   an assumption ("no line-of-sight test"), as are its inputs.
//! - `m_1C2A9945`: bodies are tested where the searcher's player (a
//!   projectile's shooter's, component 50) saw them, rewound. The server
//!   tests them where the match places them now (assumption).
//! - `m_08285CD6`: the cone's direction leaves out the entity's pitch; a
//!   whole sphere has none. `m_E4A295CE`: a predicting client replays its
//!   recorded results after a rollback (`0x7FF789BA9250`); nothing for the
//!   server. `m_8CBE558B` gates debug drawing (slot 56 `0x7FF789BA1860`,
//!   read by the drawing slots 57-62 only); `m_4A6E0895` calls slot 64,
//!   which returns at once. No client code writes `m_24A4AB8A`.
//!
//! The server runs the forms whose set fields are only these: the spawn
//! rooms' `0D62` st0 and st3 (a 100 m sphere around the room's own
//! placeable, whole: no assumption), `0257` st6 and Reinhardt's `0095`
//! st18, both counting allies within 10 m for a criteria, the Biotic
//! Field's `0258` st0 and quick melee's `0043` st5 (the last four with
//! the line of sight and the rewind assumed). Any other set field (sweep
//! origins, scoring `m_84E5DE4E`, limits, the output `m_CEE20F6F` that
//! SetVars read) or an offset the server can't evaluate leaves the search
//! finding nothing, recorded as an assumption naming the field.
//!
//! Assumed, as nothing in the data says: a body counts when its capsule
//! (as the match places it) comes within the radius, as `heal_area`
//! measures; new bodies come after those already found, in the world's
//! order (the client appends them, `0x7FF789AF3D70`); the outputs are
//! written at the begin and whenever the set changes, and keep their last
//! value when the state ends.

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
    "m_2A0FB8BB",
    // The order of the targets; 0 keeps the order found.
    "m_43478CAD",
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

/// The filter's tag lists (any of, all of, none of), as
/// `0x7FF789C1EC70` tests them.
pub(super) const TAG_LISTS: [&str; 3] = ["m_9C7BD8B8", "m_B486603E", "m_8B2A4DF3"];

/// A body's filter word (see the module notes): its team's bit
/// `TEAM_BIT << team`, team 4 in a free-for-all, the kinds of a living body
/// and [`DEAD`] once it died.
const TEAM_BIT: u64 = 0x0080_0000;
const FREE_FOR_ALL_TEAM: u8 = 4;
const BODY_KINDS: u64 = 0x1_0041;
const DEAD: u64 = 0x10;

/// Fields the client reads that the server runs the search without, each
/// an assumption when set (see the module notes).
const ASSUMED: &[(&str, &str)] = &[
    ("m_95FFC3C5", "no line-of-sight test"),
    (
        "m_1C2A9945",
        "bodies where the match places them now, not rewound",
    ),
];

/// How often a search runs again (`m_DD792FF0`, gate `0x7FF789BB9460`):
/// every `n` frames for the values 1 to 4, else every frame.
const RATE: &str = "m_DD792FF0";
/// `m_32AFE9AA`: the found bodies are tested again at each search.
const RECHECK: &str = "m_32AFE9AA";
/// `m_0297060F`: an entity's offset turns with its pitch too.
const FOLLOW_PITCH: &str = "m_0297060F";
/// The found bodies' output.
const FOUND: &str = "m_8A2905A0";

/// One turn in the yaw and pitch units of [`World::entity_facing`].
const TURN: f64 = std::f64::consts::TAU / 65_536.0;

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

/// A `STUConfigVarFilter`'s masks, relation words and tag lists, the part
/// of it a body search tests (see the module notes); the ray casts
/// (`vm/rays.rs`) test bodies with it too.
pub(super) struct BodyFilter {
    masks: expr::FilterMasks,
    /// Any of, all of, none of (`TAG_LISTS`), each empty unless the
    /// caller allows it.
    tags: [Vec<u64>; 3],
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
        let mut tags: [Vec<u64>; 3] = Default::default();
        for (list, name) in tags.iter_mut().zip(TAG_LISTS) {
            if allowed.contains(&name) {
                *list = expr::filter_tag_list(field(filter, name)).ok_or(name)?;
            }
        }
        Ok(Self {
            masks: expr::FilterMasks::words(filter),
            tags,
        })
    }

    /// Whether `candidate` passes the masks and relation words, as
    /// `Filter_TestTeamMasks`; `hostile` and `searcher` are its relation to
    /// the searching entity (relations matter only when a word is set).
    fn passes(&self, candidate: &TargetCandidate, hostile: bool, searcher: bool) -> bool {
        self.masks
            .passes(candidate.filter_word(), !hostile || searcher, searcher)
    }

    fn relations(&self) -> bool {
        self.masks.relations()
    }

    /// Whether no body can pass: no any-of mask and no include word (the
    /// map-only filters of most ray casts).
    fn passes_none(&self) -> bool {
        self.masks.passes_none()
    }

    /// Whether `entity` carries a tag of the any-of list (when it has
    /// one), every tag of the all-of list and none of the none-of list,
    /// as the world gives its target tags; the list whose tag the world
    /// can't tell, as the error.
    fn tags_pass<W: World + ?Sized>(
        &self,
        world: &W,
        site: Site,
        entity: u32,
    ) -> Result<bool, Gap> {
        let [any, all, none] = &self.tags;
        let carries = |tags: &[u64], list: &'static str| -> Result<Vec<bool>, Gap> {
            tags.iter()
                .map(|&tag| {
                    world
                        .has_target_tag(site, entity, tag)
                        .ok_or(Gap::Filter(list))
                })
                .collect()
        };
        let any = carries(any, TAG_LISTS[0])?;
        let all = carries(all, TAG_LISTS[1])?;
        let none = carries(none, TAG_LISTS[2])?;
        Ok((any.is_empty() || any.contains(&true))
            && !all.contains(&false)
            && !none.contains(&true))
    }
}

/// Whether a running search searches again now (the client's slot 14,
/// `0x7FF789BA5590`): at a frame's last update, on the frames the gate
/// `0x7FF789BB9460` gives for its rate `m_DD792FF0` (see the module notes).
fn search_due(now_ms: u64, frame: u32, site: Site, node: &Node) -> bool {
    if now_ms % FRAME_MS != FRAME_MS - 1 {
        return false;
    }
    let every: i32 = match field(&node.fields, RATE).as_u64() {
        Some(1) => 3,
        Some(2) => 6,
        Some(3) => 12,
        Some(4) => 30,
        _ => return true,
    };
    let phase = fnv1a(site.entity ^ u32::from(node.rid));
    (frame.wrapping_add(phase) as i32) % every == 0
}

/// 32-bit FNV-1a over the four little-endian bytes of `value`.
fn fnv1a(value: u32) -> u32 {
    value.to_le_bytes().iter().fold(0x811C_9DC5, |hash, &byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    })
}

/// A body's filter word, as the match sends it (see the module notes).
pub(crate) fn filter_word(candidate: &TargetCandidate) -> u64 {
    let team = candidate.team.unwrap_or(FREE_FOR_ALL_TEAM);
    let dead = if candidate.dead { DEAD } else { 0 };
    TEAM_BIT << team | BODY_KINDS | dead
}

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    /// The search at the state's begin; a form the server runs searches
    /// again on the frames its rate gives.
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

    /// Search (at the begin, or on a frame [`search_due`] lets through) and
    /// keep the result; true when the form runs.
    pub(super) fn track_targets(
        &mut self,
        key: StateKey,
        node: &Node,
        begin: bool,
    ) -> Result<bool, InitFault> {
        let site = self.site(key.instance, node);
        if !begin && !search_due(self.machine.now_ms, self.machine.frame, site, node) {
            return Ok(true);
        }
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
        for (name, answer) in ASSUMED {
            if is_set(field(&node.fields, name)) {
                self.assume(site, &format!("{CLASS} {name}"), answer);
            }
        }
        // The client takes the bodies found before from the found output
        // itself (none without it), keeps them unless `m_32AFE9AA` tests
        // them again, and appends the new ones. The server keeps them in
        // the state, cleared at its end: the same while no other node
        // writes the output.
        let keep = !is_set(field(&node.fields, RECHECK));
        let listed = !field(&node.fields, FOUND).is_null();
        let now = self.machine.now_ms;
        let state = self.state_mut(key).expect("active TrackTargets");
        let before: Vec<u32> = state.tracked.iter().map(|target| target.entity).collect();
        if !listed {
            state.tracked.clear();
        } else if !keep {
            state
                .tracked
                .retain(|target| found.contains(&target.entity));
        }
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
        // The sphere goes with a listed body, else with any entity the
        // world places (a spawned Biotic Field's own search).
        let centre = match search.entity {
            Some(entity) => {
                let candidates = self.world.target_candidates(site).ok_or(Gap::NoBodies)?;
                // A body is where the match placed it; another entity (the
                // spawn room's placeable) where the world puts it.
                let feet = candidates
                    .iter()
                    .find(|candidate| candidate.entity == entity)
                    .map(|body| body.feet)
                    .or_else(|| self.world.entity_position(site, entity))
                    .ok_or(Gap::Field("m_8DCA77B6"))?;
                std::array::from_fn(|axis| feet[axis] + search.offset[axis])
            }
            None => search.offset,
        };
        let filter_config = field(&node.fields, "m_filter");
        let mut found = Vec::new();
        for candidate in bodies
            .iter()
            .filter(|candidate| candidate.distance(centre) <= search.radius)
        {
            if self.filter_admits(id, node, filter_config, &search.filter, candidate.entity)? {
                found.push(candidate.entity);
            }
        }
        Ok(found)
    }

    /// Whether `entity` passes the parts of a filter the client tests after
    /// its masks (`ScriptHost_EntityPassesFilter`, see the module notes):
    /// the condition (`m_condition` of `filter_config`, with the entity as
    /// every parameter) and the tag lists. A condition the server can't
    /// evaluate, or a tag the world can't tell, is the error.
    pub(super) fn filter_admits(
        &mut self,
        id: u16,
        node: &Node,
        filter_config: &Json,
        filter: &BodyFilter,
        entity: u32,
    ) -> Result<bool, Gap> {
        let condition = field(filter_config, "m_condition");
        if is_set(condition) {
            let outer = self.candidate.replace(Value::Entity(entity));
            let holds = self.evaluate_quietly(id, node, condition);
            self.candidate = outer;
            match holds {
                Some(value) if truthy(&value) => {}
                Some(_) => return Ok(false),
                None => return Err(Gap::Filter("m_condition")),
            }
        }
        let site = self.site(id, node);
        filter.tags_pass(&*self.world, site, entity)
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
        // An owner that is a controller (a spawned entity's) counts as the
        // body it possesses.
        let owner = candidates
            .iter()
            .find(|candidate| candidate.entity == owner)
            .or_else(|| {
                candidates
                    .iter()
                    .find(|candidate| candidate.controller == Some(owner))
            });
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
            .cloned()
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
        let filter = BodyFilter::read(
            field(fields, "m_filter"),
            &["m_condition", TAG_LISTS[0], TAG_LISTS[1], TAG_LISTS[2]],
        )
        .map_err(Gap::Filter)?;
        let entity = match field(fields, "m_8DCA77B6") {
            Json::Null => None,
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Entity(entity)) if expr::valid_entity(entity) => Some(entity),
                _ => return Err(Gap::Field("m_8DCA77B6")),
            },
        };
        // Unset (the spawn rooms' `0D62`): no offset, the STU default.
        let mut offset = match field(fields, "m_05BE62DD") {
            Json::Null => [0.0; 3],
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Vector(offset)) => offset,
                _ => return Err(Gap::Field("m_05BE62DD")),
            },
        };
        let follow_pitch = is_set(field(fields, FOLLOW_PITCH));
        match entity {
            Some(entity) => offset = self.entity_offset(id, node, entity, offset, follow_pitch)?,
            // In the searcher's own transform: not modelled.
            None if follow_pitch => return Err(Gap::Field(FOLLOW_PITCH)),
            None => (),
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

    /// `offset` in `entity`'s transform (see the module notes): turned by
    /// its yaw (forward `+z`, sideways `+x`, as a rotation about the up
    /// axis), and first, with `follow_pitch`, by its pitch about its eye
    /// height (a positive pitch looks down, [`expr::aim`]).
    pub(super) fn entity_offset(
        &mut self,
        id: u16,
        node: &Node,
        entity: u32,
        [x, y, z]: [f64; 3],
        follow_pitch: bool,
    ) -> Result<[f64; 3], Gap> {
        let site = self.site(id, node);
        let (mut up, mut ahead) = (y, z);
        if follow_pitch && (y != 0.0 || z != 0.0) {
            let [_, pitch] = self
                .world
                .entity_facing(site, entity)
                .ok_or(Gap::Field(FOLLOW_PITCH))?;
            if pitch != 0 {
                let eye = self.eye_height(id, node, entity)?;
                let (sin, cos) = (f64::from(pitch) * TURN).sin_cos();
                up = (y - eye) * cos - z * sin + eye;
                ahead = (y - eye) * sin + z * cos;
            }
        }
        if x == 0.0 && ahead == 0.0 {
            return Ok([0.0, up, 0.0]);
        }
        let yaw = self
            .world
            .entity_yaw(site, entity)
            .ok_or(Gap::Field("m_05BE62DD"))?;
        let (sin, cos) = (f64::from(yaw) * TURN).sin_cos();
        Ok([x * cos + ahead * sin, up, ahead * cos - x * sin])
    }

    /// How high `entity`'s eye is over its feet (`STU_21A13145` of the
    /// search's own entity config, less its position).
    fn eye_height(&mut self, id: u16, node: &Node, entity: u32) -> Result<f64, Gap> {
        let config = serde_json::json!({
            "$": "STU_21A13145",
            "m_entity": field(&node.fields, "m_8DCA77B6"),
        });
        let site = self.site(id, node);
        let feet = self
            .world
            .entity_position(site, entity)
            .ok_or(Gap::Field(FOLLOW_PITCH))?;
        match self.evaluate_quietly(id, node, &config) {
            Some(Value::Vector(eye)) => Ok(eye[1] - feet[1]),
            _ => Err(Gap::Field(FOLLOW_PITCH)),
        }
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
            graphs: self.graphs,
            world: self.world,
            site,
            reads: Vec::new(),
            foreign_reads: Vec::new(),
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
