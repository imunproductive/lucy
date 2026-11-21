//! Ray casts: the action `STU_08D6BA6E`, the state RayCast
//! (`STUStatescriptStateRayCast`, 019E0826) and the state TrackRay
//! (`STUStatescriptStateTrackRay`, 164EF13B). Every script world of a
//! match casts against the same map ([`World::map_rays`]).
//!
//! The action and RayCast end in the client's shared cast `0x7FF789B9DDA0`
//! (TrackRay builds the same scene query itself, base slot 55
//! `0x7FF789B98880`): the ray from a start along a direction (normalized
//! there) for a length, with a radius, a filter, an ignore list and
//! `m_type`. Type 0 is the scene query (`0x7FF789AFC340` builds it,
//! `0x7FF789C717C0` → `0x7FF789AFEEB0` runs it: the map's collision of the
//! filter's ray profile and the entities the filter passes, one sweep of
//! the radius). Type 1 walks a grid of the caster's level instead (component
//! 1 `+0x250` → `+1112` → `+0x1A0`; `0x7FF78A61D460` → `0x7FF78A61D4F0`,
//! cells of `+416` by `+420`, so likely the AI navigation volumes), and with
//! a radius of at least a cell casts more rays stepped sideways; a hit is
//! where the walk is blocked. Type 1 is not modelled (its only uses, the
//! four sideways checks of `0CD5`, can't run for other gaps).
//!
//! - The action (run `0x7FF789B95820`, `m_startPosWS`, `m_dirWS`,
//!   `m_castLength`, `m_castRadius`, `m_castFilter`, `m_ignoreEntities`)
//!   writes `m_out_HitSomething` always and `m_out_HitEntity` always (none
//!   on a miss), `m_out_HitPointWS` and `m_out_HitNormal` only on a hit.
//!   `m_2158DFDE` (a value of the hit's component 4) is set in no node and
//!   not written.
//! - RayCast (the same fields): its begin (`0x7FF789B940E0`) binds its
//!   outputs to the state (`0x7FF78AA82F90(state, variable, slot, no
//!   priority, 1, 0)`, slots 0-3: hit, entity, point, normal; slot 49
//!   `0x7FF789B94580` answers them from the state's block) without writing
//!   the variables' own values, and clears the block: no hit, no entity,
//!   point and normal zero; slot 14 (`0x7FF789B941F0`) casts again each
//!   update; a miss keeps the last point and normal and makes the entity
//!   none. Here the same bindings ([`LinkValue::Output`]), their values
//!   the block's; the end releases them and each variable is back at its
//!   own value. The owner's frames list the bindings, so his client keeps
//!   casting the ray into the variables itself: a frame that lists such a
//!   variable without them unbinds it there (`0x7FF78AAB7420`), and from
//!   then on only the server's frames change it, as restores that wake no
//!   watcher (df10 s2 cp4_bufslam: Doomfist's `0B27` st8 ground ray
//!   `#2413` turned false on his client only when the server's frame came,
//!   so st7 never watched `#10473e` and the slam flew 2 frames late).
//! - TrackRay (the winner's slot 55 `0x7FF78980FBB0` runs the base's cast
//!   and records it; slot 60 stores; slot 14 casts again each update): from
//!   `m_3BC31C57`, in the transform of entity `m_56B8547E` when it is a
//!   valid entity (its model matrix, vtable +456: the position, turned by
//!   the yaw), to `m_F5732403`: `STU_9E9028E6` = that start plus the world
//!   direction `m_direction` (not turned) times `m_6A79BD98` (clamped to
//!   1.19e-4..10000), or `STU_E5D454BA` = point `m_BD631144` in entity
//!   `m_entity`'s transform (a world point when the entity is invalid;
//!   `m_68B8830B` true: that entity is ignored too). It ignores its start
//!   entity, `m_ignoreEntities` and the entities of `m_7AB75223` (the
//!   query's second list, `+0xC0`, handed to the filter by
//!   `0x7FF78AA6F970`: the server reads it as more ignored entities); its
//!   radius is `m_57DAE1B5` and its filter `m_filter`, tested as the
//!   instigator `m_instigator` (default the script's own entity). A miss's
//!   point is the end and its normal points from the end back to the start.
//!   Its outputs are the fields of `m_target` (`STU_404DE6D3`, read from
//!   the variables they feed): `m_7BA44A69` hit, `m_692DFF72` entity,
//!   `m_750377AA` the hit entity as an array, `m_5ED803CF` the hit point (on
//!   a hit), `m_F2F8606D` the point, `m_A7C2505B` the normal.
//! - `m_3B0AD973` (TrackRay slot 59 `0x7FF789B938E0`, the action's
//!   argument 12) hands the query a callback (`off_7FF78B56F6D0`) whose one
//!   slot only notes that the host (vtable +560) agreed; after the query the
//!   host reports the instigator and the hit to itself (+552, when the hit
//!   lacks tags 58 and 715). The hit doesn't change. Its five uses are
//!   server-only TrackRays (Zarya `0241`, `157C`, Echo `1FBD`) whose
//!   `m_750377AA` feeds a ModifyHealth; the report's server side isn't in
//!   the client.
//! - `m_8CBE558B` changes nothing: the action passes it to the shared cast
//!   (argument 11), which never reads it (debug drawing, as TrackTargets').
//! - The caster isn't left out by itself: the action's and RayCast's ignore
//!   list is `m_ignoreEntities` alone (three uses list the owner there), and
//!   TrackRay's adds its start entity.
//!
//! What the server runs (assumptions, recorded): the map's collision (the
//! ray cache) masked by the filter's ray profile `m_C1903771` (`0690`, the
//! default profile when unset) and the bodies the world lists, as the match
//! places their capsules, that pass the filter's masks, relation words,
//! condition and tag lists as a target search tests them (`vm/targets.rs`),
//! the nearest first. The bodies only when the profile's rays test the
//! bodies' hit shapes ([`MapRays::meets_bodies`]): the client's walk tests a
//! body's hit shapes only then (`0x7FF789B01DD0`, `0x7FF789B00C50`), so the
//! rays of `0018` (Widowmaker's Grappling Hook), `00A1`, `0016` and the
//! other profiles without `m_0220680A` bit 0 pass every living body. A
//! body the ray starts in is not hit. A radius grows
//! the bodies' capsules; against the map the ray stays thin, where the
//! client sweeps the radius through the whole query (`0x7FF789AFEEB0`'s
//! `+0x1A0`). A hit on the map has no entity (the client gives the hit
//! object's entity, `+176`: a placeable's own collision would name it; the
//! ray cache keeps no placeables). Hardpoints (`m_F745334E`, `m_hardPoint`:
//! the model's bone matrix, vtable +448) and type 1 cast nothing: the
//! outputs stay as they were (recorded as an assumption naming the field).
//! So do a world with no map and a filter field the server doesn't model.

use super::{
    targets::{BodyFilter, Gap, TAG_LISTS, is_set},
    *,
};

const ACTION: &str = "STU_08D6BA6E";
const RAY_CAST: &str = "STUStatescriptStateRayCast";
const TRACK_RAY: &str = "STUStatescriptStateTrackRay";
/// RayCast's outputs and the slots its begin binds them with.
const RAY_CAST_OUTPUTS: [(&str, u8); 4] = [
    ("m_out_HitSomething", 0),
    ("m_out_HitEntity", 1),
    ("m_out_HitPointWS", 2),
    ("m_out_HitNormal", 3),
];

/// The filter field a cast reads itself: the map's ray profile.
const PROFILE: &str = "m_C1903771";
/// The filter fields a cast tests (beside its masks and words).
const FILTER_FIELDS: [&str; 5] = [
    PROFILE,
    "m_condition",
    TAG_LISTS[0],
    TAG_LISTS[1],
    TAG_LISTS[2],
];
/// The host's hit report switch (see the module notes).
const REPORT: &str = "m_3B0AD973";

/// TrackRay's `m_6A79BD98` clamp (base slot 55).
const MIN_LENGTH: f64 = 1.19e-4;
const MAX_LENGTH: f64 = 10_000.0;

/// Why a cast can't run.
pub(super) enum RayGap {
    Field(&'static str),
    Filter(&'static str),
    NoBodies,
    NoMap,
}

impl From<Gap> for RayGap {
    fn from(gap: Gap) -> Self {
        match gap {
            Gap::Field(name) => Self::Field(name),
            Gap::Filter(name) => Self::Filter(name),
            Gap::NoBodies => Self::NoBodies,
        }
    }
}

/// A cast as evaluated now.
pub(super) struct Ray {
    origin: [f64; 3],
    /// Unit length.
    direction: [f64; 3],
    reach: f64,
    radius: f64,
    /// The filter's masks and words, or the first field the server doesn't
    /// model (it matters only when the ray tests bodies).
    filter: Result<BodyFilter, &'static str>,
    /// The node's filter field (its condition is evaluated per body).
    filter_name: &'static str,
    profile: Option<u64>,
    ignore: Vec<u32>,
}

impl Ray {
    fn end(&self) -> [f64; 3] {
        std::array::from_fn(|axis| self.origin[axis] + self.direction[axis] * self.reach)
    }
}

/// What a cast met first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Hit {
    /// None: the map.
    pub(super) entity: Option<u32>,
    pub(super) point: [f64; 3],
    pub(super) normal: [f64; 3],
}

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// The action: cast once and write the outputs.
    pub(super) fn cast_action(&mut self, id: u16, node: &Node) -> Result<(), InitFault> {
        let Some(hit) = self.cast_node(id, node, ACTION)? else {
            return Ok(());
        };
        self.put(id, node, "m_out_HitSomething", Value::Bool(hit.is_some()))?;
        self.put(id, node, "m_out_HitEntity", hit_entity(hit.as_ref()))?;
        if let Some(hit) = hit {
            self.put(id, node, "m_out_HitPointWS", Value::Vector(hit.point))?;
            self.put(id, node, "m_out_HitNormal", Value::Vector(hit.normal))?;
        }
        Ok(())
    }

    /// A RayCast or TrackRay state's begin: cast, and again each tick when
    /// the form runs.
    pub(super) fn begin_ray(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        if node.class == RAY_CAST {
            // The begin binds the outputs to the state's block and clears
            // it: no hit, no entity, point and normal zero.
            for (name, slot) in RAY_CAST_OUTPUTS {
                let value = match slot {
                    0 => Value::Bool(false),
                    1 => hit_entity(None),
                    _ => Value::Vector([0.0; 3]),
                };
                self.bind_ray_output(key, node, name, slot, value, depth)?;
            }
        }
        if self.update_ray(key, node)? {
            self.register_tick(key, self.machine.now_ms);
        }
        Ok(())
    }

    /// Cast and write what changed; false when the form can't run.
    pub(super) fn update_ray(&mut self, key: StateKey, node: &Node) -> Result<bool, InitFault> {
        let id = key.instance;
        if node.class == RAY_CAST {
            let Some(hit) = self.cast_node(id, node, RAY_CAST)? else {
                return Ok(false);
            };
            let (point, normal) = match hit {
                Some(hit) => (
                    Some(Value::Vector(hit.point)),
                    Some(Value::Vector(hit.normal)),
                ),
                None => (None, None),
            };
            let values = [
                Some(Value::Bool(hit.is_some())),
                Some(hit_entity(hit.as_ref())),
                point,
                normal,
            ];
            for ((name, _), value) in RAY_CAST_OUTPUTS.into_iter().zip(values) {
                if let Some(value) = value {
                    self.set_ray_output(key, node, name, value)?;
                }
            }
            return Ok(true);
        }
        let site = self.site(id, node);
        let cast = match self.track_ray(id, node) {
            Ok(ray) => self.cast(id, node, &ray).map(|hit| (ray, hit)),
            Err(gap) => Err(gap),
        };
        let (ray, hit) = match cast {
            Ok(cast) => cast,
            Err(gap) => {
                self.ray_gap(site, TRACK_RAY, gap);
                return Ok(false);
            }
        };
        self.ray_assumptions(site, TRACK_RAY, node, &ray);
        let end = ray.end();
        let (point, normal) = match &hit {
            Some(hit) => (hit.point, hit.normal),
            None => (
                end,
                unit(std::array::from_fn(|axis| ray.origin[axis] - end[axis])),
            ),
        };
        let target = field(&node.fields, "m_target");
        let outputs = [
            ("m_7BA44A69", Some(Value::Bool(hit.is_some()))),
            ("m_692DFF72", Some(hit_entity(hit.as_ref()))),
            (
                "m_750377AA",
                Some(Value::Array(
                    hit.iter()
                        .filter_map(|hit| hit.entity)
                        .map(Value::Entity)
                        .collect(),
                )),
            ),
            (
                "m_5ED803CF",
                hit.as_ref().map(|hit| Value::Vector(hit.point)),
            ),
            ("m_F2F8606D", Some(Value::Vector(point))),
            ("m_A7C2505B", Some(Value::Vector(normal))),
        ];
        for (name, value) in outputs {
            if let Some(value) = value {
                self.put_config(id, node, field(target, name), value)?;
            }
        }
        Ok(true)
    }

    /// The action's or RayCast's cast (they share their fields): Some(hit)
    /// when it runs, None (recorded) when it can't.
    fn cast_node(
        &mut self,
        id: u16,
        node: &Node,
        class: &'static str,
    ) -> Result<Option<Option<Hit>>, InitFault> {
        let site = self.site(id, node);
        let cast = match self.node_ray(id, node) {
            Ok(ray) => self.cast(id, node, &ray).map(|hit| (ray, hit)),
            Err(gap) => Err(gap),
        };
        Ok(match cast {
            Ok((ray, hit)) => {
                self.ray_assumptions(site, class, node, &ray);
                Some(hit)
            }
            Err(gap) => {
                self.ray_gap(site, class, gap);
                None
            }
        })
    }

    pub(super) fn ray_gap(&mut self, site: Site, class: &str, gap: RayGap) {
        match gap {
            RayGap::Field(name) => self.assume(site, &format!("{class} {name}"), "casts nothing"),
            RayGap::Filter(name) => {
                self.assume(site, &format!("{class} filter.{name}"), "casts nothing")
            }
            RayGap::NoBodies => self.assume(
                site,
                &format!("{class} bodies"),
                "casts nothing (the world lists no bodies)",
            ),
            RayGap::NoMap => self.assume(
                site,
                &format!("{class} map"),
                "casts nothing (the world has no map)",
            ),
        }
    }

    fn ray_assumptions(&mut self, site: Site, class: &str, node: &Node, ray: &Ray) {
        self.assume(
            site,
            class,
            "casts against the map and the listed bodies' capsules",
        );
        if ray.radius > 0.0 {
            self.assume(
                site,
                &format!("{class} radius"),
                "grows the bodies; a thin ray against the map",
            );
        }
        if is_set(field(&node.fields, REPORT)) {
            self.assume(
                site,
                &format!("{class} {REPORT}"),
                "the host's hit report is not made; the hit is the nearest",
            );
        }
    }

    /// The nearest thing the ray meets: the map or a body that passes (none
    /// when the ray's profile meets no body).
    pub(super) fn cast(&mut self, id: u16, node: &Node, ray: &Ray) -> Result<Option<Hit>, RayGap> {
        let site = self.site(id, node);
        let map_rays = self.world.map_rays().ok_or(RayGap::NoMap)?;
        let meets_bodies = map_rays.meets_bodies(ray.profile);
        let map = map_rays
            .cast(ray.origin, ray.direction, ray.reach, ray.profile)
            .ok_or(RayGap::NoMap)?;
        let map_hit = map.map(|hit| Hit {
            entity: None,
            point: hit.point,
            normal: hit.normal,
        });
        // The filter's body fields matter only to rays that test bodies.
        if !meets_bodies {
            return Ok(map_hit);
        }
        let filter = ray.filter.as_ref().map_err(|name| RayGap::Filter(name))?;
        let bodies = self.passing_bodies(site, filter)?;
        let reach = map.map_or(ray.reach, |hit| hit.distance);
        let mut met: Vec<(f64, [f64; 3], u32)> = bodies
            .iter()
            .filter(|body| !ray.ignore.contains(&body.entity))
            .filter_map(|body| {
                let (distance, normal) =
                    body.ray_hit(ray.origin, ray.direction, reach, ray.radius)?;
                (map.is_none() || distance < reach).then_some((distance, normal, body.entity))
            })
            .collect();
        met.sort_by(|a, b| a.0.total_cmp(&b.0));
        let filter_config = field(&node.fields, ray.filter_name);
        for (distance, normal, entity) in met {
            if self.filter_admits(id, node, filter_config, filter, entity)? {
                return Ok(Some(Hit {
                    entity: Some(entity),
                    point: std::array::from_fn(|axis| {
                        ray.origin[axis] + ray.direction[axis] * distance
                    }),
                    normal,
                }));
            }
        }
        Ok(map_hit)
    }

    /// The action's and RayCast's ray.
    fn node_ray(&mut self, id: u16, node: &Node) -> Result<Ray, RayGap> {
        let fields = &node.fields;
        if field(fields, "m_type").as_u64().unwrap_or(0) != 0 {
            return Err(RayGap::Field("m_type"));
        }
        let origin = self
            .vector(id, node, field(fields, "m_startPosWS"))
            .ok_or(RayGap::Field("m_startPosWS"))?;
        let direction = self
            .vector(id, node, field(fields, "m_dirWS"))
            .and_then(checked_unit)
            .ok_or(RayGap::Field("m_dirWS"))?;
        let reach = self
            .config_number(id, node, field(fields, "m_castLength"))
            .filter(|reach| *reach >= 0.0)
            .ok_or(RayGap::Field("m_castLength"))?;
        let radius = self.radius(id, node, field(fields, "m_castRadius"), "m_castRadius")?;
        let ignore = self.ignored(id, node, field(fields, "m_ignoreEntities"))?;
        Self::ray(
            origin,
            direction,
            reach,
            radius,
            node,
            "m_castFilter",
            ignore,
        )
    }

    /// TrackRay's ray.
    fn track_ray(&mut self, id: u16, node: &Node) -> Result<Ray, RayGap> {
        let fields = &node.fields;
        if !field(fields, "m_F745334E").is_null() {
            return Err(RayGap::Field("m_F745334E"));
        }
        let site = self.site(id, node);
        let instigator = field(fields, "m_instigator");
        if !instigator.is_null()
            && self.evaluate_quietly(id, node, instigator) != Some(Value::Entity(site.entity))
        {
            return Err(RayGap::Field("m_instigator"));
        }
        let mut ignore = self.ignored(id, node, field(fields, "m_ignoreEntities"))?;
        let second = field(fields, "m_7AB75223");
        if !second.is_null() {
            ignore.extend(self.entities_of(id, node, second, "m_7AB75223")?);
        }
        let start = field(fields, "m_3BC31C57");
        let start = if start.is_null() {
            [0.0; 3]
        } else {
            self.vector(id, node, start)
                .ok_or(RayGap::Field("m_3BC31C57"))?
        };
        let origin = match self.entity(id, node, field(fields, "m_56B8547E"), "m_56B8547E")? {
            Some(entity) => {
                ignore.push(entity);
                self.in_entity(id, node, entity, start, "m_3BC31C57")?
            }
            None => start,
        };
        let end = field(fields, "m_F5732403");
        let end = match field(end, "$").as_str() {
            Some("STU_9E9028E6") => {
                // A world direction: slot 55 adds it to the start as it is.
                let direction = self
                    .vector(id, node, field(end, "m_direction"))
                    .ok_or(RayGap::Field("m_F5732403.m_direction"))?;
                let length = self
                    .config_number(id, node, field(end, "m_6A79BD98"))
                    .ok_or(RayGap::Field("m_F5732403.m_6A79BD98"))?
                    .clamp(MIN_LENGTH, MAX_LENGTH);
                std::array::from_fn(|axis| origin[axis] + direction[axis] * length)
            }
            Some("STU_E5D454BA") => {
                if !field(end, "m_hardPoint").is_null() {
                    return Err(RayGap::Field("m_F5732403.m_hardPoint"));
                }
                let point = field(end, "m_BD631144");
                let point = if point.is_null() {
                    [0.0; 3]
                } else {
                    self.vector(id, node, point)
                        .ok_or(RayGap::Field("m_F5732403.m_BD631144"))?
                };
                let ignored = self.flag(id, node, field(end, "m_68B8830B"))?;
                match self.entity(id, node, field(end, "m_entity"), "m_F5732403.m_entity")? {
                    Some(entity) => {
                        if ignored {
                            ignore.push(entity);
                        }
                        self.in_entity(id, node, entity, point, "m_F5732403.m_BD631144")?
                    }
                    None => point,
                }
            }
            _ => return Err(RayGap::Field("m_F5732403")),
        };
        let delta: [f64; 3] = std::array::from_fn(|axis| end[axis] - origin[axis]);
        let reach = length(delta);
        let direction = checked_unit(delta).ok_or(RayGap::Field("m_F5732403"))?;
        let radius = self.radius(id, node, field(fields, "m_57DAE1B5"), "m_57DAE1B5")?;
        Self::ray(origin, direction, reach, radius, node, "m_filter", ignore)
    }

    pub(super) fn ray(
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
        radius: f64,
        node: &Node,
        filter_name: &'static str,
        ignore: Vec<u32>,
    ) -> Result<Ray, RayGap> {
        let filter = field(&node.fields, filter_name);
        Ok(Ray {
            origin,
            direction,
            reach,
            radius,
            filter: BodyFilter::read(filter, &FILTER_FIELDS),
            filter_name,
            profile: profile(filter)?,
            ignore,
        })
    }

    /// `offset` in `entity`'s transform: its position plus the offset
    /// turned by its yaw (its model matrix, as a target search's centre).
    fn in_entity(
        &mut self,
        id: u16,
        node: &Node,
        entity: u32,
        offset: [f64; 3],
        name: &'static str,
    ) -> Result<[f64; 3], RayGap> {
        let site = self.site(id, node);
        let position = self
            .world
            .entity_position(site, entity)
            .ok_or(RayGap::Field(name))?;
        let turned = self
            .entity_offset(id, node, entity, offset, false)
            .map_err(|_| RayGap::Field(name))?;
        Ok(std::array::from_fn(|axis| position[axis] + turned[axis]))
    }

    /// An entity config: None when unset or not a valid entity (the client
    /// then takes the point as a world point).
    fn entity(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        name: &'static str,
    ) -> Result<Option<u32>, RayGap> {
        if config.is_null() {
            return Ok(None);
        }
        match self.evaluate_quietly(id, node, config) {
            Some(Value::Entity(entity)) => Ok(expr::valid_entity(entity).then_some(entity)),
            Some(Value::Null) => Ok(None),
            _ => Err(RayGap::Field(name)),
        }
    }

    /// The entities of an ignore list (each config an entity or an array of
    /// them).
    fn ignored(&mut self, id: u16, node: &Node, list: &Json) -> Result<Vec<u32>, RayGap> {
        let mut ignore = Vec::new();
        for config in list.as_array().into_iter().flatten() {
            ignore.extend(self.entities_of(id, node, config, "m_ignoreEntities")?);
        }
        Ok(ignore)
    }

    /// The valid entities of one config: an entity or an array of them.
    fn entities_of(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        name: &'static str,
    ) -> Result<Vec<u32>, RayGap> {
        let value = self
            .evaluate_quietly(id, node, config)
            .ok_or(RayGap::Field(name))?;
        let values = match value {
            Value::Array(values) => values,
            value => vec![value],
        };
        let mut entities = Vec::new();
        for value in values {
            match value {
                Value::Entity(entity) if expr::valid_entity(entity) => entities.push(entity),
                Value::Entity(_) | Value::Null => {}
                _ => return Err(RayGap::Field(name)),
            }
        }
        Ok(entities)
    }

    pub(super) fn flag(&mut self, id: u16, node: &Node, config: &Json) -> Result<bool, RayGap> {
        if config.is_null() {
            return Ok(false);
        }
        self.evaluate_quietly(id, node, config)
            .map(|value| truthy(&value))
            .ok_or(RayGap::Field("flag"))
    }

    fn vector(&mut self, id: u16, node: &Node, config: &Json) -> Option<[f64; 3]> {
        match self.evaluate_quietly(id, node, config)? {
            Value::Vector(vector) if vector.iter().all(|value| value.is_finite()) => Some(vector),
            _ => None,
        }
    }

    pub(super) fn config_number(&mut self, id: u16, node: &Node, config: &Json) -> Option<f64> {
        self.evaluate_quietly(id, node, config)
            .map(|value| expr::to_float(&value))
            .filter(|value| value.is_finite())
    }

    /// A cast radius: 0 when unset.
    fn radius(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        name: &'static str,
    ) -> Result<f64, RayGap> {
        if config.is_null() {
            return Ok(0.0);
        }
        self.config_number(id, node, config)
            .map(|radius| radius.max(0.0))
            .ok_or(RayGap::Field(name))
    }

    /// Write output field `name` of the node.
    pub(super) fn put(
        &mut self,
        id: u16,
        node: &Node,
        name: &str,
        value: Value,
    ) -> Result<(), InitFault> {
        self.put_config(id, node, field(&node.fields, name), value)
    }

    /// Write `value` to the variable `config` names, when it changes it.
    fn put_config(
        &mut self,
        id: u16,
        node: &Node,
        config: &Json,
        value: Value,
    ) -> Result<(), InitFault> {
        match expr::lvalue(config) {
            Ok(Some(variable)) => {
                let key = source_key(id, variable);
                if self.machine.value(key, None) != value {
                    self.write(key, value)?;
                }
            }
            Ok(None) => (),
            Err(error) => self.report(self.site(id, node), Issue::Expression(error)),
        }
        Ok(())
    }

    /// A RayCast's begin binds output `name` with `slot` to the state.
    fn bind_ray_output(
        &mut self,
        key: StateKey,
        node: &Node,
        name: &str,
        slot: u8,
        value: Value,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(target) = self.output_key(key.instance, node, name) else {
            return Ok(());
        };
        self.insert_link(
            target,
            VariableLink {
                state: key,
                priority: LINK_PRIORITY,
                output: LinkValue::Output { slot, value },
                stack: false,
            },
            depth,
        )
    }

    /// A cast's new value of RayCast output `name`: into its binding, when
    /// it changes it.
    fn set_ray_output(
        &mut self,
        key: StateKey,
        node: &Node,
        name: &str,
        value: Value,
    ) -> Result<(), InitFault> {
        let Some(target) = self.output_key(key.instance, node, name) else {
            return Ok(());
        };
        let old = self.machine.value(target, None);
        let Some(bound) = self
            .machine
            .links
            .get_mut(&target)
            .into_iter()
            .flatten()
            .find_map(|entry| match &mut entry.output {
                LinkValue::Output { value, .. } if entry.state == key => Some(value),
                _ => None,
            })
        else {
            return Ok(());
        };
        if *bound == value {
            return Ok(());
        }
        *bound = value;
        self.notify_value(target, &old, None)
    }
}

/// The hit entity as the outputs hold it: none (0) for a miss or the map.
fn hit_entity(hit: Option<&Hit>) -> Value {
    Value::Entity(hit.and_then(|hit| hit.entity).unwrap_or(0))
}

/// The filter's ray profile (`m_C1903771`, `STU_20DB8AB5`): None when unset.
pub(super) fn profile(filter: &Json) -> Result<Option<u64>, RayGap> {
    let config = field(filter, PROFILE);
    if config.is_null() {
        return Ok(None);
    }
    if field(config, "$").as_str() != Some("STU_20DB8AB5")
        || !field(config, "m_resourceKey").is_null()
    {
        return Err(RayGap::Filter(PROFILE));
    }
    match field(config, PROFILE) {
        Json::Null => Ok(None),
        Json::String(guid) => guid
            .strip_prefix("0x")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .map(Some)
            .ok_or(RayGap::Filter(PROFILE)),
        _ => Err(RayGap::Filter(PROFILE)),
    }
}

fn length(vector: [f64; 3]) -> f64 {
    vector.iter().map(|value| value * value).sum::<f64>().sqrt()
}

/// `vector` scaled to unit length; None when it has none.
fn checked_unit(vector: [f64; 3]) -> Option<[f64; 3]> {
    let length = length(vector);
    (length > 1e-9).then(|| vector.map(|value| value / length))
}

/// `vector` scaled to unit length, zero when it has none.
fn unit(vector: [f64; 3]) -> [f64; 3] {
    checked_unit(vector).unwrap_or([0.0; 3])
}
