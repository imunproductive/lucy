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
//!   entity's position config); with `m_0297060F` it is turned in the
//!   searcher's own transform: into it, by its pitch about its eye height,
//!   and back (`0x7FF789BAA430`); its own eye (Nano Boost's `0B9E` st6,
//!   `STU_21A13145(owner())`) stays where it is.
//! - `m_radius`: the sphere's radius in metres (Biotic Field `#422` = 5,
//!   the radius the match's `heal_area` uses; `0257` st6 10). Sure.
//! - `m_A30B383E`: a cone's full angle in degrees (default 360: the whole
//!   sphere; Nano Boost `10 × #9602e`). Its direction (`0x7FF789BAA430`,
//!   `+0x10` of the shape) is `(sin y cos p, sin p, cos y cos p)` with the
//!   yaw `y` = `m_D45E22FA` and the pitch `p` = `m_E46A19A3` in degrees
//!   (both default 0), turned by the entity's model transform (no entity:
//!   the world's; 46 searches give `STU_C8C4C20D(owner())`, the owner's
//!   heading, as the yaw); with `m_08285CD6` the aim's pitch (component 28
//!   `+66`, positive down) is taken off `p`, so the cone follows the aim.
//!   A body in the sphere is in the cone when the point of its axis
//!   nearest the sphere's diameter along the direction is within half the
//!   angle of it, or that diameter crosses the axis ahead of the centre
//!   (`0x7FF789B9E660`). The half-angle scales `m_B9BEAC7F` (horizontal)
//!   and `m_3119AD61` (vertical, unset: 1) narrow or widen the cone
//!   (`0x7FF789BAA430` @0x7FF789BABB45: the cosines of the half-angle times
//!   each, clamped to `[0, π]`); the test takes the cosine between the two
//!   by how far the line to the body's axis point rises (its unit `|y|`).
//!   With `m_E474FFD6` (Rocket Punch's searches, `0D06` st6 and st72) the
//!   shape is a cylinder along the diameter instead: a body passes when its
//!   axis point is ahead of the centre and within `sin(half) × radius ×
//!   scale` of the diameter (`m_8862A75D`, the radius it lerps to, unset),
//!   the scale again between the two by the rise between the points
//!   (clamped to 1 m). The inner cone `m_544C3E6F` is not modelled.
//! - `m_84E5DE4E`: the targets' score, lowest first (`0x7FF789BA6E10`):
//!   `STU_B61DA2A5` sums its entries' terms, each with its `m_weight` (a
//!   weight within 0.004 of 0 counts nothing); a `STU_0BCD3EFA` term maps
//!   its measure from `m_ABC68B33` (min) to `m_2AFC083F` (max) onto
//!   `[0, 1]`: `STU_2FD4C482` the screen distance from the crosshair to
//!   the body's box (×√2) in a 90° view from the searcher's eye along its
//!   aim, `STU_AD7331B0` the body's distance from the centre; a body behind
//!   the view adds 100. Of the hero bundle's 30 searches with a scoring,
//!   24 (Nano Boost's among them) use `[2FD4C482 0..1 ×1, AD7331B0 0..40
//!   ×0.01]`, the ally nearest the crosshair, then the nearest; 2 add
//!   `STU_153DF0E5`, 2 use `STU_E3EF9AFE` and 2 `STU_7658FFA9`, which are
//!   not modelled.
//! - `m_C39691D9`: the most targets (default `0x7FFFFFFF`): a search adds
//!   at most that many new bodies, best first, and keeps the best that
//!   many (`0x7FF789BBE3B0`).
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
//! - `m_43478CAD`: the targets' order without a scoring (`0x7FF789BB9570`
//!   reads it only then): 0 keeps them in the order found, 1 puts the
//!   nearest to the centre first and 2 the nearest to the searcher's own
//!   position (the squared distance of each body's position; Rocket
//!   Punch's `0D06` st6 with one target). 3 (the view's) is not modelled
//!   (finds nothing). With a scoring it changes nothing.
//! - `m_95FFC3C5`: a line-of-sight test of each new body (and, with
//!   `m_32AFE9AA`, the found ones), `0x7FF789BB3570` (its arguments from
//!   the search `0x7FF789BBE3B0`): a body is seen when one of its rays
//!   meets nothing of the map (the scene query `0x7FF789C707F0` →
//!   `0x7FF789AFEEB0` with a filter of zero masks: the map's collision of
//!   the ray profile, a hit on the body itself not counting). The rays go
//!   from `m_D59188D5` (else the centre) through the profile of
//!   `m_3D667CE3` (its `m_C1903771`, else `002D.097`,
//!   `0x0690_0000_0000_002D`), in this order, to the body where the
//!   searcher's player saw it (`m_1C2A9945`):
//!   1. for a body that isn't dead (no health component, or
//!      `0x7FF789ADBF00`: maximum health set and health 0), its model's
//!      hardpoint `0DC0.0001`, when the model has it;
//!   2. the point 0.3 of its height up its axis;
//!   3. its model's hardpoint `0DC0.01CD`, else the point 0.6 up;
//!   4. with `m_2F5FAF65`, the two points 0.45 up and half its radius to
//!      each side of the ray to point 2 (the side is up × that ray).
//!
//!   `m_8D13DD07` first sweeps from the origin to point 2 (`0x7FF78AA64A40`
//!   with `0x7FF789B9D4B0`); anything it meets hides the body. A body with
//!   component 128 takes `0x7FF789CA8EC0` in place of rays 2-4 (no body
//!   the server lists has it [inference: its one factory, no hero data
//!   class]). The fields that override the point or add hardpoint lists
//!   (`m_48837057`, `m_294660A6`, `m_229DD034`, `m_BA6C9F75`,
//!   `m_9E1426F6`, `m_E8474EE8`, `m_C1B724B2`, `m_47AB1887`) are among
//!   those a search can't run with. The server has no hardpoints, so it
//!   casts rays 2, 0.6 up and 4: a body ray 2 sees is seen as the client
//!   sees it; any other answer is an assumption naming the hardpoints, as
//!   is a seen body of a search with a sweep, or a world with no map.
//! - `m_1C2A9945`: when the searcher has a player (component 43 → 29; a
//!   projectile through its shooter, component 50), bodies are tested
//!   where that player saw them, rewound (`0x7FF789CBB340`,
//!   `0x7FF789CC1AA0`). The world says where ([`World::seen_bodies`]): a
//!   player's body's world at the view of the command its script runs
//!   (`orpheus_sim::combat::TargetRewind`, the history the server's shots
//!   rewind with), a bot's and a map placeable's as listed (no player).
//!   The searcher's own body stays where it is (its player sees it
//!   there); a body that did not exist at the view is left out. A world
//!   that can't tell (a spawned entity's: the Biotic Field `0258`) tests
//!   them as listed, an assumption.
//! - `m_08285CD6`: the cone follows the aim's pitch (above); a whole
//!   sphere has no direction. `m_E4A295CE`: a predicting client replays its
//!   recorded results after a rollback (`0x7FF789BA9250`); nothing for the
//!   server. `m_8CBE558B` gates debug drawing (slot 56 `0x7FF789BA1860`,
//!   read by the drawing slots 57-62 only); `m_4A6E0895` calls slot 64,
//!   which returns at once. No client code writes `m_24A4AB8A`.
//!
//! The server runs the forms whose set fields are only these: the spawn
//! rooms' `0D62` st0 and st3 (a 100 m sphere around the room's own
//! placeable, whole: no assumption), `0257` st6 and Reinhardt's `0095`
//! st18, both counting allies within 10 m for a criteria, the Biotic
//! Field's `0258` st0, quick melee's `0043` st5, the aimed ally cones
//! (Nano Boost's `0B9E` st6) and the Training Partner console's `265F`
//! rid1 (the players within 1.75 m of it, in sight). Any other set field
//! (the output `m_CEE20F6F` that SetVars read, other scorings) or an
//! offset the server can't evaluate leaves the search finding nothing,
//! recorded as an assumption naming the field.
//!
//! Assumed, as nothing in the data says: a body counts when its capsule
//! (as the match places it, or where the searcher's player saw it) comes
//! within the radius, as `heal_area`
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
    "m_2F88C5F2",
    "m_544C3E6F",
    "m_762A56C0",
    "m_B3A77CD0",
    "m_16BDE607",
    "m_8862A75D",
    "m_48837057",
    "m_294660A6",
    "m_229DD034",
    "m_BA6C9F75",
    "m_9E1426F6",
    "m_E8474EE8",
    "m_C1B724B2",
    "m_93AB8C13",
    "m_CEE20F6F",
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

/// The filter's tag lists (any of, all of, none of), as
/// `0x7FF789C1EC70` tests them.
pub(super) const TAG_LISTS: [&str; 3] = ["m_9C7BD8B8", "m_B486603E", "m_8B2A4DF3"];

/// A body's filter word (see the module notes): its team's bit
/// `TEAM_BIT << team`, team 4 in a free-for-all, the kinds of a living body
/// and [`DEAD`] once it died.
const TEAM_BIT: u64 = 0x0080_0000;
const FREE_FOR_ALL_TEAM: u8 = 4;
const BODY_KINDS: u64 = 0x1_0041;
pub(crate) const DEAD: u64 = 0x10;

/// The line of sight (see the module notes): the switch, its origin, the
/// filter whose ray profile it casts with, the sweep before the rays and
/// the two rays to the sides.
const SIGHT: &str = "m_95FFC3C5";
const SIGHT_ORIGIN: &str = "m_D59188D5";
const SIGHT_FILTER: &str = "m_3D667CE3";
const SIGHT_SWEEP: &str = "m_8D13DD07";
const SIGHT_SIDES: &str = "m_2F5FAF65";
/// The ray profile of a line of sight whose filter names none
/// (`002D.097`, `0x7FF789BB3570`).
const SIGHT_PROFILE: u64 = 0x0690_0000_0000_002D;
/// The shares of a body's height its rays aim at: ray 2, ray 3 without the
/// hardpoint, and the side rays.
const SIGHT_LOW: f64 = 0.3;
const SIGHT_HIGH: f64 = 0.6;
const SIGHT_SIDE: f64 = 0.45;

/// `m_1C2A9945`: the bodies where the searcher's player saw them.
const REWIND: &str = "m_1C2A9945";

/// How often a search runs again (`m_DD792FF0`, gate `0x7FF789BB9460`):
/// every `n` frames for the values 1 to 4, else every frame.
const RATE: &str = "m_DD792FF0";
/// `m_32AFE9AA`: the found bodies are tested again at each search.
const RECHECK: &str = "m_32AFE9AA";
/// `m_0297060F`: an entity's offset turns with its pitch too.
const FOLLOW_PITCH: &str = "m_0297060F";
/// The found bodies' output.
const FOUND: &str = "m_8A2905A0";
/// The cone: its angle in degrees (360, or unset: a whole sphere), the
/// direction's yaw and pitch in degrees, and whether the pitch follows the
/// aim's.
const CONE_ANGLE: &str = "m_A30B383E";
const CONE_YAW: &str = "m_D45E22FA";
const CONE_PITCH: &str = "m_E46A19A3";
const FOLLOW_AIM: &str = "m_08285CD6";
/// The half-angle's horizontal and vertical scales, and the cylinder
/// shape (the module notes).
const CONE_SCALES: [&str; 2] = ["m_B9BEAC7F", "m_3119AD61"];
const CYLINDER: &str = "m_E474FFD6";
/// The scoring, the order without one, and the most targets kept.
const SCORING: &str = "m_84E5DE4E";
const ORDER: &str = "m_43478CAD";
const LIMIT: &str = "m_C39691D9";
/// What the score adds for a body it can't measure (`0x7FF789BA6E10`: a
/// missing entity, or one behind the view).
const UNSCORED: f64 = 100.0;

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
    /// The cone (the module notes); None: the whole sphere.
    cone: Option<Cone>,
    /// The scoring's terms (`m_84E5DE4E`): the lowest total comes first.
    scoring: Vec<ScoreTerm>,
    /// The order without a scoring (`m_43478CAD`).
    order: Order,
    /// The most targets kept (`m_C39691D9`).
    limit: Option<usize>,
}

/// A search's order without a scoring (`m_43478CAD`, see the module
/// notes).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Order {
    Found,
    NearCentre,
    NearSearcher,
}

/// What a `STU_0BCD3EFA` scoring term measures (`0x7FF789BA6E10`).
#[derive(Clone, Copy)]
enum Measure {
    /// `STU_2FD4C482`: how far the body's box is from the crosshair on the
    /// searcher's screen.
    Crosshair,
    /// `STU_AD7331B0`: how far the body is from the search's centre.
    Distance,
}

/// One term of a score: its measure mapped from `[min, max]` onto `[0, 1]`
/// (clamped), times its weight.
#[derive(Clone, Copy)]
struct ScoreTerm {
    measure: Measure,
    min: f64,
    max: f64,
    weight: f64,
}

/// What a search found: the bodies in the order kept, and each listed
/// body's score (empty without a scoring).
struct Found {
    entities: Vec<u32>,
    scores: Vec<(u32, f64)>,
}

impl Found {
    fn score(&self, entity: u32) -> f64 {
        self.scores
            .iter()
            .find(|(scored, _)| *scored == entity)
            .map_or(UNSCORED, |(_, score)| *score)
    }
}

/// A line of sight's rays as evaluated now (see the module notes).
struct Sight {
    origin: [f64; 3],
    profile: u64,
    sides: bool,
}

/// What a line of sight says of one body.
enum Sighting {
    /// Ray 2 sees it: the client sees it too.
    Seen,
    /// Another ray sees it, or none does; the client's hardpoint rays could
    /// answer otherwise.
    Guessed(bool),
}

impl Sight {
    /// Whether `body` is in sight; None: the map can't answer.
    fn sees(&self, map: &dyn MapRays, body: &TargetCandidate) -> Option<Sighting> {
        let [x, y, z] = body.feet;
        let up = |share: f64| [x, y + body.height * share, z];
        let low = up(SIGHT_LOW);
        if self.clear(map, low)? {
            return Some(Sighting::Seen);
        }
        if self.clear(map, up(SIGHT_HIGH))? {
            return Some(Sighting::Guessed(true));
        }
        if self.sides {
            // Up × the ray to point 2, of unit length; along x without one.
            let [ahead_x, _, ahead_z] = sub(low, self.origin);
            let length = (ahead_x * ahead_x + ahead_z * ahead_z).sqrt();
            let side = if length > 0.0 {
                [ahead_z / length, 0.0, -ahead_x / length]
            } else {
                [1.0, 0.0, 0.0]
            };
            let middle = up(SIGHT_SIDE);
            let half = body.radius * 0.5;
            if self.clear(map, along(middle, side, half))?
                || self.clear(map, along(middle, side, -half))?
            {
                return Some(Sighting::Guessed(true));
            }
        }
        Some(Sighting::Guessed(false))
    }

    /// Whether the ray from the origin to `point` meets nothing of the map.
    fn clear(&self, map: &dyn MapRays, point: [f64; 3]) -> Option<bool> {
        let offset = sub(point, self.origin);
        let reach = dot(offset, offset).sqrt();
        if reach < 1e-6 {
            return Some(true);
        }
        let direction = offset.map(|value| value / reach);
        let hit = map.cast(self.origin, direction, reach, Some(self.profile))?;
        Some(hit.is_none_or(|hit| hit.distance >= reach))
    }
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
        let (found, limit) = match self.search_targets(key.instance, node) {
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
        // The client takes the bodies found before from the found output
        // itself (none without it), keeps them unless `m_32AFE9AA` tests
        // them again, and appends up to the limit's count of new ones (best
        // first); with a scoring, all of them then go best first, and the
        // limit cuts the rest (`0x7FF789BBE3B0`). The server keeps them in
        // the state, cleared at its end: the same while no other node
        // writes the output.
        let keep = !is_set(field(&node.fields, RECHECK));
        let listed = !field(&node.fields, FOUND).is_null();
        let limit = limit.unwrap_or(usize::MAX);
        let now = self.machine.now_ms;
        let state = self.state_mut(key).expect("active TrackTargets");
        let before: Vec<u32> = state.tracked.iter().map(|target| target.entity).collect();
        if !listed {
            state.tracked.clear();
        } else if !keep {
            state
                .tracked
                .retain(|target| found.entities.contains(&target.entity));
        }
        let new: Vec<u32> = found
            .entities
            .iter()
            .copied()
            .filter(|&entity| !state.tracked.iter().any(|target| target.entity == entity))
            .take(limit)
            .collect();
        state
            .tracked
            .extend(new.into_iter().map(|entity| TrackedTarget {
                entity,
                since_ms: now,
            }));
        if !found.scores.is_empty() {
            state
                .tracked
                .sort_by(|a, b| found.score(a.entity).total_cmp(&found.score(b.entity)));
        }
        state.tracked.truncate(limit);
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

    /// `bodies` where the searcher's player saw them (see the module
    /// notes): each but the searcher's own body moved to its seen place,
    /// or left out when it did not exist then. False: the world can't
    /// tell, and they stay as listed.
    fn rewind(&self, site: Site, bodies: &mut Vec<TargetCandidate>) -> bool {
        match self.world.seen_bodies(site) {
            None => false,
            Some(SeenBodies::Listed) => true,
            Some(SeenBodies::Rewound(seen)) => {
                bodies.retain_mut(|body| {
                    if body.entity == site.entity {
                        return true;
                    }
                    let Some(place) = seen.iter().find(|place| place.entity == body.entity) else {
                        return false;
                    };
                    body.feet = place.feet;
                    body.height = place.height;
                    body.radius = place.radius;
                    body.yaw = place.yaw;
                    true
                });
                true
            }
        }
    }

    /// The bodies the search finds now, best first with a scoring, else in
    /// the world's order; and the limit.
    fn search_targets(&mut self, id: u16, node: &Node) -> Result<(Found, Option<usize>), Gap> {
        let search = self.target_search(id, node)?;
        let site = self.site(id, node);
        let mut bodies = self.passing_bodies(site, &search.filter)?;
        if is_set(field(&node.fields, REWIND)) && !self.rewind(site, &mut bodies) {
            self.assume(
                site,
                &format!("{CLASS} {REWIND}"),
                "bodies where the match places them now, not rewound",
            );
        }
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
        let mut entities = Vec::new();
        for candidate in bodies.iter().filter(|candidate| {
            candidate.distance(centre) <= search.radius
                && search
                    .cone
                    .as_ref()
                    .is_none_or(|cone| in_cone(candidate, centre, cone, search.radius))
        }) {
            if self.filter_admits(id, node, filter_config, &search.filter, candidate.entity)? {
                entities.push(candidate.entity);
            }
        }
        if is_set(field(&node.fields, SIGHT)) {
            entities = self.in_sight(id, node, centre, &bodies, entities);
        }
        let mut found = Found {
            entities,
            scores: Vec::new(),
        };
        if !search.scoring.is_empty() {
            let view = if search
                .scoring
                .iter()
                .any(|term| matches!(term.measure, Measure::Crosshair))
            {
                Some(
                    self.world
                        .eye(site, site.entity)
                        .ok_or(Gap::Field(SCORING))?,
                )
            } else {
                None
            };
            found.scores = bodies
                .iter()
                .map(|body| (body.entity, score(&search.scoring, body, centre, view)))
                .collect();
            let mut ranked = std::mem::take(&mut found.entities);
            ranked.sort_by(|a, b| found.score(*a).total_cmp(&found.score(*b)));
            found.entities = ranked;
        } else if search.order != Order::Found {
            // Nearest first (`0x7FF789BB9570` cases 1 and 2: the squared
            // distance of each body's position from the centre, or from
            // the searcher's own).
            let from = match search.order {
                Order::NearSearcher => bodies
                    .iter()
                    .find(|body| body.entity == site.entity)
                    .map(|body| body.feet)
                    .or_else(|| self.world.entity_position(site, site.entity))
                    .ok_or(Gap::Field(ORDER))?,
                _ => centre,
            };
            found.scores = bodies
                .iter()
                .map(|body| {
                    let offset = sub(body.feet, from);
                    (body.entity, dot(offset, offset))
                })
                .collect();
            let mut ranked = std::mem::take(&mut found.entities);
            ranked.sort_by(|a, b| found.score(*a).total_cmp(&found.score(*b)));
            found.entities = ranked;
        }
        Ok((found, search.limit))
    }

    /// The bodies of `found` (all of `bodies`) the searcher sees, in order
    /// (`m_95FFC3C5`, see the module notes); each answer the client could
    /// give otherwise is an assumption. Without a line of sight the server
    /// can cast (an origin it can't evaluate, a profile it can't read, no
    /// map) it sees them all, also an assumption.
    fn in_sight(
        &mut self,
        id: u16,
        node: &Node,
        centre: [f64; 3],
        bodies: &[TargetCandidate],
        found: Vec<u32>,
    ) -> Vec<u32> {
        if found.is_empty() {
            return found;
        }
        let site = self.site(id, node);
        let sight = match self.sight(id, node, centre) {
            Ok(sight) => sight,
            Err(name) => {
                self.assume(site, &format!("{CLASS} {name}"), "no line-of-sight test");
                return found;
            }
        };
        let Some(map) = self.world.map_rays() else {
            self.assume(site, &format!("{CLASS} {SIGHT}"), "no line-of-sight test");
            return found;
        };
        let sweep = is_set(field(&node.fields, SIGHT_SWEEP));
        let mut seen = Vec::new();
        let mut assumed = Vec::new();
        for entity in found {
            let body = bodies
                .iter()
                .find(|body| body.entity == entity)
                .expect("a found body is listed");
            let visible = match sight.sees(map, body) {
                None => {
                    assumed.push((SIGHT, "no line-of-sight test"));
                    true
                }
                Some(Sighting::Seen) => true,
                Some(Sighting::Guessed(visible)) => {
                    assumed.push((
                        "m_95FFC3C5 hardpoints",
                        "rays to 0.3 and 0.6 of the height, no hardpoint 0DC0.0001/01CD",
                    ));
                    visible
                }
            };
            if visible {
                if sweep {
                    assumed.push((SIGHT_SWEEP, "no sweep before the rays"));
                }
                seen.push(entity);
            }
        }
        for (name, answer) in assumed {
            self.assume(site, &format!("{CLASS} {name}"), answer);
        }
        seen
    }

    /// The line of sight's origin, ray profile and side rays, or the field
    /// the server can't evaluate.
    fn sight(&mut self, id: u16, node: &Node, centre: [f64; 3]) -> Result<Sight, &'static str> {
        let fields = &node.fields;
        let origin = match field(fields, SIGHT_ORIGIN) {
            Json::Null => centre,
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Vector(origin)) => origin,
                _ => return Err(SIGHT_ORIGIN),
            },
        };
        let profile = match field(fields, SIGHT_FILTER) {
            Json::Null => None,
            filter => super::rays::profile(filter).map_err(|_| SIGHT_FILTER)?,
        };
        Ok(Sight {
            origin,
            profile: profile.unwrap_or(SIGHT_PROFILE),
            sides: is_set(field(fields, SIGHT_SIDES)),
        })
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
            None if follow_pitch => offset = self.searcher_offset(id, node, offset)?,
            None => (),
        }
        let radius = self
            .evaluate_quietly(id, node, field(fields, "m_radius"))
            .map(|radius| expr::to_float(&radius))
            .filter(|radius| radius.is_finite() && *radius >= 0.0)
            .ok_or(Gap::Field("m_radius"))?;
        let cone = self.cone(id, node, entity)?;
        let mut scoring = Vec::new();
        let mut order = Order::Found;
        match field(fields, SCORING) {
            Json::Null => {
                order = match field(fields, ORDER).as_i64().unwrap_or(0) {
                    0 => Order::Found,
                    1 => Order::NearCentre,
                    2 => Order::NearSearcher,
                    _ => return Err(Gap::Field(ORDER)),
                };
            }
            // No body can pass (no any-of mask nor include word: the
            // capture point `0E8E` st5's search for its floor ring by tag
            // `0302`): no score orders an empty list.
            _ if filter.passes_none() => {}
            config => self.score_terms(id, node, config, 1.0, &mut scoring)?,
        }
        let limit = match field(fields, LIMIT) {
            Json::Null => None,
            config => {
                let limit = self
                    .evaluate_quietly(id, node, config)
                    .map(|limit| expr::to_float(&limit))
                    .filter(|limit| limit.is_finite())
                    .ok_or(Gap::Field(LIMIT))?;
                Some(limit.max(0.0) as usize)
            }
        };
        Ok(Search {
            entity,
            offset,
            radius,
            filter,
            cone,
            scoring,
            order,
            limit,
        })
    }

    /// A config in degrees (unset: 0), as radians.
    fn radians(&mut self, id: u16, node: &Node, name: &'static str) -> Result<f64, Gap> {
        match field(&node.fields, name) {
            Json::Null => Ok(0.0),
            config => self
                .evaluate_quietly(id, node, config)
                .map(|degrees| expr::to_float(&degrees))
                .filter(|degrees| degrees.is_finite())
                .map(f64::to_radians)
                .ok_or(Gap::Field(name)),
        }
    }

    /// The cone (see the module notes), or None for the whole sphere.
    fn cone(&mut self, id: u16, node: &Node, entity: Option<u32>) -> Result<Option<Cone>, Gap> {
        let angle = match field(&node.fields, CONE_ANGLE) {
            Json::Null => return Ok(None),
            config => self
                .evaluate_quietly(id, node, config)
                .map(|angle| expr::to_float(&angle))
                .filter(|angle| angle.is_finite())
                .ok_or(Gap::Field(CONE_ANGLE))?,
        };
        if angle >= 360.0 {
            return Ok(None);
        }
        let site = self.site(id, node);
        let mut yaw = self.radians(id, node, CONE_YAW)?;
        let mut pitch = self.radians(id, node, CONE_PITCH)?;
        if let Some(entity) = entity {
            let turn = self
                .world
                .entity_yaw(site, entity)
                .ok_or(Gap::Field(CONE_YAW))?;
            yaw += f64::from(turn) * TURN;
        }
        if is_set(field(&node.fields, FOLLOW_AIM)) {
            let [_, aim] = self
                .world
                .entity_facing(site, entity.unwrap_or(site.entity))
                .ok_or(Gap::Field(FOLLOW_AIM))?;
            pitch -= f64::from(aim) * TURN;
        }
        // The half-angle unclamped for the cylinder's sine, each scaled
        // one clamped to `[0, π]` for its cosine (`0x7FF789BAA430`).
        let half = (angle * 0.5).to_radians();
        let mut scales = [1.0; 2];
        for (scale, name) in scales.iter_mut().zip(CONE_SCALES) {
            let config = field(&node.fields, name);
            if !config.is_null() {
                *scale = self
                    .evaluate_quietly(id, node, config)
                    .map(|value| expr::to_float(&value))
                    .filter(|value| value.is_finite())
                    .ok_or(Gap::Field(name))?;
            }
        }
        let cylinder = match field(&node.fields, CYLINDER) {
            Json::Null => false,
            config => self
                .evaluate_quietly(id, node, config)
                .map(|value| expr::truthy(&value))
                .ok_or(Gap::Field(CYLINDER))?,
        };
        let direction = [
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        ];
        Ok(Some(Cone {
            direction,
            cos: scales.map(|scale| (half * scale).clamp(0.0, std::f64::consts::PI).cos()),
            cylinder: cylinder.then(|| scales.map(|scale| half.sin() * scale)),
        }))
    }

    /// The terms of the scoring `config` (`0x7FF789BA6E10`): a list
    /// `STU_B61DA2A5` gives each entry its own weight `m_weight`; a term
    /// whose weight is about 0 counts nothing. Other classes are not
    /// modelled.
    fn score_terms(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        weight: f64,
        terms: &mut Vec<ScoreTerm>,
    ) -> Result<(), Gap> {
        if weight.abs() <= 0.004 {
            return Ok(());
        }
        let float = |vm: &mut Self, value: &Json| {
            vm.evaluate_quietly(id, node, value)
                .map(|value| expr::to_float(&value))
                .filter(|value| value.is_finite())
                .ok_or(Gap::Field(SCORING))
        };
        let measure = match field(config, "$").as_str() {
            Some("STU_B61DA2A5") => {
                let Json::Array(entries) = field(config, "m_503F0304") else {
                    return Err(Gap::Field(SCORING));
                };
                for entry in entries {
                    let weight = float(self, field(entry, "m_weight"))?;
                    self.score_terms(id, node, field(entry, "m_84E5DE4E"), weight, terms)?;
                }
                return Ok(());
            }
            Some("STU_2FD4C482") => Measure::Crosshair,
            Some("STU_AD7331B0") => Measure::Distance,
            _ => return Err(Gap::Field(SCORING)),
        };
        let min = float(self, field(config, "m_ABC68B33"))?;
        let max = float(self, field(config, "m_2AFC083F"))?;
        if max > min {
            terms.push(ScoreTerm {
                measure,
                min,
                max,
                weight,
            });
        }
        Ok(())
    }

    /// A world point `offset` turned with the searcher's pitch about its eye
    /// height, in its own transform (`m_0297060F` without an entity,
    /// `0x7FF789BAA430`): Nano Boost's eye stays where it is.
    fn searcher_offset(&mut self, id: u16, node: &Node, offset: [f64; 3]) -> Result<[f64; 3], Gap> {
        let site = self.site(id, node);
        let searcher = site.entity;
        let feet = self
            .world
            .entity_position(site, searcher)
            .ok_or(Gap::Field(FOLLOW_PITCH))?;
        let yaw = self
            .world
            .entity_yaw(site, searcher)
            .ok_or(Gap::Field(FOLLOW_PITCH))?;
        let (sin, cos) = (f64::from(yaw) * TURN).sin_cos();
        let [x, y, z] = std::array::from_fn(|axis| offset[axis] - feet[axis]);
        let local = [x * cos - z * sin, y, x * sin + z * cos];
        let turned = self.entity_offset(id, node, searcher, local, true)?;
        Ok(std::array::from_fn(|axis| feet[axis] + turned[axis]))
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
    /// search's own entity config, else of `owner()`, less its position).
    fn eye_height(&mut self, id: u16, node: &Node, entity: u32) -> Result<f64, Gap> {
        let config = match field(&node.fields, "m_8DCA77B6") {
            Json::Null => serde_json::json!({
                "$": "STU_21A13145",
                "m_entity": {"$": "STU_B5A0CAF0"},
            }),
            entity => serde_json::json!({"$": "STU_21A13145", "m_entity": entity}),
        };
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
            candidate: self.candidate.clone(),
            sender: self.sender,
            watcher: None,
            read_native: false,
            read_volatile: false,
            natives: Vec::new(),
            assumed: Vec::new(),
        };
        let result = expr::evaluate_in(config, node.configs(), &mut context);
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

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn along(point: [f64; 3], direction: [f64; 3], distance: f64) -> [f64; 3] {
    std::array::from_fn(|axis| point[axis] + direction[axis] * distance)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|axis| a[axis] * b[axis]).sum()
}

/// The closest points of segments `p1 q1` and `p2 q2` (Ericson, Real-Time
/// Collision Detection 5.1.9), as the client's `0x7FF789CD8720` gives them.
fn closest_points(p1: [f64; 3], q1: [f64; 3], p2: [f64; 3], q2: [f64; 3]) -> [[f64; 3]; 2] {
    const EPSILON: f64 = 1e-12;
    let (d1, d2, r) = (sub(q1, p1), sub(q2, p2), sub(p1, p2));
    let (a, e, f) = (dot(d1, d1), dot(d2, d2), dot(d2, r));
    let (s, t) = if a <= EPSILON && e <= EPSILON {
        (0.0, 0.0)
    } else if a <= EPSILON {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = dot(d1, r);
        if e <= EPSILON {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = dot(d1, d2);
            let denominator = a * e - b * b;
            let s = if denominator == 0.0 {
                0.0
            } else {
                ((b * f - c * e) / denominator).clamp(0.0, 1.0)
            };
            let t = (b * s + f) / e;
            if t < 0.0 {
                ((-c / a).clamp(0.0, 1.0), 0.0)
            } else if t > 1.0 {
                (((b - c) / a).clamp(0.0, 1.0), 1.0)
            } else {
                (s, t)
            }
        }
    };
    [along(p1, d1, s), along(p2, d2, t)]
}

/// A search's cone (see the module notes): its unit direction, the cosines
/// of its half-angle scaled horizontally and vertically, and for the
/// cylinder shape (`m_E474FFD6`) the sine of the half-angle times each
/// scale (times the radius, the most a body's axis may lie off the
/// diameter).
struct Cone {
    direction: [f64; 3],
    cos: [f64; 2],
    cylinder: Option<[f64; 2]>,
}

/// Whether a body within the sphere is in the cone (the client's shape test
/// `0x7FF789B9E660`): the point of its axis nearest the sphere's diameter
/// along the cone's direction is within the half-angle of it, the cosine
/// taken between the horizontal and the vertical one by the unit line's
/// `|y|`, or that diameter crosses the axis ahead of the centre; for the
/// cylinder, the point is ahead of the centre and within the sine's share
/// of the radius of the diameter, taken between the two by the rise
/// between the points (clamped to 1).
fn in_cone(body: &TargetCandidate, centre: [f64; 3], cone: &Cone, radius: f64) -> bool {
    // The client's test of a half-angle wider than the whole sphere.
    const WHOLE: f64 = -0.9999;
    if cone.cos.iter().all(|&cos| cos <= WHOLE) {
        return true;
    }
    let direction = cone.direction;
    let [x, y, z] = body.feet;
    let low = y + body.radius.min(body.height * 0.5);
    let high = y + (body.height - body.radius).max(body.height * 0.5);
    let [axis, diameter] = closest_points(
        [x, low, z],
        [x, high, z],
        along(centre, direction, radius),
        along(centre, direction, -radius),
    );
    let offset = sub(axis, centre);
    let off = sub(axis, diameter);
    let ahead = dot(offset, direction);
    if off == [0.0; 3] && ahead > 0.0 {
        return true;
    }
    if let Some([horizontal, vertical]) = cone.cylinder {
        if ahead < 0.0 {
            return false;
        }
        let rise = off[1].abs().clamp(0.0, 1.0);
        let most = (horizontal + (vertical - horizontal) * rise) * radius;
        return dot(off, off).sqrt() <= most;
    }
    let length = dot(offset, offset).sqrt();
    let unit = if length < 1e-18 {
        [0.0, 0.0, 1.0]
    } else {
        offset.map(|value| value / length)
    };
    let [horizontal, vertical] = cone.cos;
    dot(unit, direction) >= horizontal + (vertical - horizontal) * unit[1].abs().clamp(0.0, 1.0)
}

/// A body's score: each term's measure mapped onto `[0, 1]` times its
/// weight, summed; [`UNSCORED`] for a term that can't measure it.
fn score(
    terms: &[ScoreTerm],
    body: &TargetCandidate,
    centre: [f64; 3],
    view: Option<([f64; 3], [i32; 2])>,
) -> f64 {
    terms
        .iter()
        .map(|term| {
            let measure = match term.measure {
                Measure::Distance => {
                    let offset = sub(body.feet, centre);
                    dot(offset, offset).sqrt()
                }
                Measure::Crosshair => match view.and_then(|view| crosshair_distance(body, view)) {
                    Some(distance) => distance * std::f64::consts::SQRT_2,
                    None => return UNSCORED,
                },
            };
            ((measure - term.min) / (term.max - term.min)).clamp(0.0, 1.0) * term.weight
        })
        .sum()
}

/// How far the crosshair is from a body's box on the searcher's screen
/// (`0x7FF789BA6E10` with `STU_2FD4C482`): the view from its eye along its
/// aim with a 90° field both ways (`0x7FF78A9884E0`), the screen from 0 to
/// 1 each way, the crosshair in its middle; the box spans the projections
/// of the body's feet a radius to the view's left and of its top a radius
/// to the right. None when either point is behind the eye. Assumption: the
/// viewport the client projects into (`0x7FF78A987F80`, `+0x1F0`) is that
/// unit square; only the order of the scores matters, and the term weighs
/// against the distance's 1 %.
fn crosshair_distance(
    body: &TargetCandidate,
    (eye, [yaw, pitch]): ([f64; 3], [i32; 2]),
) -> Option<f64> {
    let (yaw_sin, yaw_cos) = (f64::from(yaw) * TURN).sin_cos();
    let (pitch_sin, pitch_cos) = (f64::from(pitch) * TURN).sin_cos();
    // A positive pitch looks down.
    let forward = [yaw_sin * pitch_cos, -pitch_sin, yaw_cos * pitch_cos];
    let right = [yaw_cos, 0.0, -yaw_sin];
    let up = [yaw_sin * pitch_sin, pitch_cos, yaw_cos * pitch_sin];
    let project = |point: [f64; 3]| {
        let offset = sub(point, eye);
        let depth = dot(offset, forward);
        (depth > 0.0).then(|| {
            [
                0.5 + 0.5 * dot(offset, right) / depth,
                0.5 - 0.5 * dot(offset, up) / depth,
            ]
        })
    };
    let [x, y, z] = body.feet;
    let low = project(along(body.feet, right, -body.radius))?;
    let high = project(along([x, y + body.height, z], right, body.radius))?;
    let squared: f64 = (0..2)
        .map(|axis| {
            let nearest = 0.5_f64.clamp(low[axis].min(high[axis]), low[axis].max(high[axis]));
            (0.5 - nearest).powi(2)
        })
        .sum();
    Some(squared.sqrt())
}
