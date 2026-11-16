//! Ray casts: the action `STU_08D6BA6E`, the state RayCast
//! (`STUStatescriptStateRayCast`, 019E0826) and the state TrackRay
//! (`STUStatescriptStateTrackRay`, 164EF13B).
//!
//! All three end in the client's shared cast `0x7FF789B9DDA0`: the ray
//! from a start along a direction (normalized there) for a length, with a
//! radius, a filter, an ignore list and `m_type`. Type 0 is the general
//! cast (`0x7FF789C717C0`: the scene's collision and the entities the
//! filter passes); type 1 another path through the scene's entity
//! (`0x7FF78A61D460`, stepped sideways by its +416); type 1 is not modelled.
//!
//! - The action (run `0x7FF789B95820`, `m_startPosWS`, `m_dirWS`,
//!   `m_castLength`, `m_castRadius`, `m_castFilter`, `m_ignoreEntities`)
//!   writes `m_out_HitSomething` always and `m_out_HitEntity` always (none
//!   on a miss), `m_out_HitPointWS` and `m_out_HitNormal` only on a hit.
//!   `m_2158DFDE` (a value of the hit's component 4) is set in no node and
//!   not written.
//! - RayCast (the same fields): its begin (`0x7FF789B940E0`) binds its
//!   outputs to the state (slot 49 `0x7FF789B94580` answers them: hit,
//!   entity, point, normal) and clears point and normal to zero; slot 14
//!   (`0x7FF789B941F0`) casts again each update; a miss keeps the last point
//!   and normal and makes the entity none. Here the outputs are written at
//!   the begin and whenever a cast changes them, and keep their last value
//!   when the state ends.
//! - TrackRay (base slot 55 `0x7FF789B98880` casts, slot 60 `0x7FF789B93A70`
//!   stores; slot 14 casts again each update): from `m_3BC31C57`, in the
//!   transform of entity `m_56B8547E` when set (else world), to `m_F5732403`:
//!   `STU_9E9028E6` = the start plus `m_direction` times `m_6A79BD98`
//!   (clamped to 1.19e-4..10000), or `STU_E5D454BA` = point `m_BD631144` in
//!   entity `m_entity`'s transform (`m_68B8830B` true: that entity is
//!   ignored too). It ignores its start entity and `m_ignoreEntities`; its
//!   radius is `m_57DAE1B5` and its filter `m_filter`. A miss's point is the
//!   end and its normal points from the end back to the start. Its outputs
//!   are the fields of `m_target` (`STU_404DE6D3`, read from the variables
//!   they feed): `m_7BA44A69` hit, `m_692DFF72` entity, `m_750377AA` the hit
//!   entity as an array, `m_5ED803CF` the hit point (on a hit), `m_F2F8606D`
//!   the point, `m_A7C2505B` the normal.
//!
//! What the server runs (assumptions, recorded): the map's collision (the
//! ray cache) masked by the filter's ray profile `m_C1903771` (`0690`, the
//! default profile when unset) and the bodies the world lists, as the match
//! places their capsules, that pass the filter's masks and relation words
//! as a target search tests them (`vm/targets.rs`), the nearest first. The
//! casting body itself is never hit, nor a body the ray starts in. A radius
//! grows the bodies' capsules; against the map the ray stays thin. A hit
//! on the map has no entity. An entity transform is only its position: the
//! forms with a sideways offset or direction, hardpoints (`m_F745334E`,
//! `m_hardPoint`), the second entity list `m_7AB75223`, the callback switch
//! `m_3B0AD973`, `m_8CBE558B`, type 1 and the filter fields a target search
//! doesn't model cast nothing: the outputs stay as they were (recorded as
//! an assumption naming the field). So does a world with no map.

use super::{
    targets::{BodyFilter, Gap},
    *,
};

const ACTION: &str = "STU_08D6BA6E";
const RAY_CAST: &str = "STUStatescriptStateRayCast";
const TRACK_RAY: &str = "STUStatescriptStateTrackRay";

/// The filter field a cast reads itself: the map's ray profile.
const PROFILE: &str = "m_C1903771";

/// TrackRay's `m_6A79BD98` clamp (base slot 55).
const MIN_LENGTH: f64 = 1.19e-4;
const MAX_LENGTH: f64 = 10_000.0;

/// Why a cast can't run.
enum RayGap {
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
struct Ray {
    origin: [f64; 3],
    /// Unit length.
    direction: [f64; 3],
    reach: f64,
    radius: f64,
    filter: BodyFilter,
    profile: Option<u64>,
    ignore: Vec<u32>,
}

impl Ray {
    fn end(&self) -> [f64; 3] {
        std::array::from_fn(|axis| self.origin[axis] + self.direction[axis] * self.reach)
    }
}

/// What a cast met first.
struct Hit {
    /// None: the map.
    entity: Option<u32>,
    point: [f64; 3],
    normal: [f64; 3],
}

impl<W: World> Vm<'_, '_, '_, W> {
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
    pub(super) fn begin_ray(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        if node.class == RAY_CAST {
            // The begin clears the bound point and normal.
            for name in ["m_out_HitPointWS", "m_out_HitNormal"] {
                self.put(key.instance, node, name, Value::Vector([0.0; 3]))?;
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
            self.put(id, node, "m_out_HitSomething", Value::Bool(hit.is_some()))?;
            self.put(id, node, "m_out_HitEntity", hit_entity(hit.as_ref()))?;
            if let Some(hit) = hit {
                self.put(id, node, "m_out_HitPointWS", Value::Vector(hit.point))?;
                self.put(id, node, "m_out_HitNormal", Value::Vector(hit.normal))?;
            }
            return Ok(true);
        }
        let site = self.site(id, node);
        let cast = match self.track_ray(id, node) {
            Ok(ray) => self.cast(site, &ray).map(|hit| (ray, hit)),
            Err(gap) => Err(gap),
        };
        let (ray, hit) = match cast {
            Ok(cast) => cast,
            Err(gap) => {
                self.ray_gap(site, TRACK_RAY, gap);
                return Ok(false);
            }
        };
        self.ray_assumptions(site, TRACK_RAY, &ray);
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
            Ok(ray) => self.cast(site, &ray).map(|hit| (ray, hit)),
            Err(gap) => Err(gap),
        };
        Ok(match cast {
            Ok((ray, hit)) => {
                self.ray_assumptions(site, class, &ray);
                Some(hit)
            }
            Err(gap) => {
                self.ray_gap(site, class, gap);
                None
            }
        })
    }

    fn ray_gap(&mut self, site: Site, class: &str, gap: RayGap) {
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

    fn ray_assumptions(&mut self, site: Site, class: &str, ray: &Ray) {
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
    }

    /// The nearest thing the ray meets: the map or a body that passes.
    fn cast(&mut self, site: Site, ray: &Ray) -> Result<Option<Hit>, RayGap> {
        let bodies = self.passing_bodies(site, &ray.filter)?;
        let map = self
            .world
            .map_ray(site, ray.origin, ray.direction, ray.reach, ray.profile)
            .ok_or(RayGap::NoMap)?;
        let mut best = map.map(|hit| {
            (
                hit.distance,
                Hit {
                    entity: None,
                    point: hit.point,
                    normal: hit.normal,
                },
            )
        });
        for body in bodies {
            if body.entity == site.entity || ray.ignore.contains(&body.entity) {
                continue;
            }
            let reach = best.as_ref().map_or(ray.reach, |(distance, _)| *distance);
            if let Some((distance, normal)) =
                body.ray_hit(ray.origin, ray.direction, reach, ray.radius)
                && best.as_ref().is_none_or(|(nearest, _)| distance < *nearest)
            {
                let point =
                    std::array::from_fn(|axis| ray.origin[axis] + ray.direction[axis] * distance);
                best = Some((
                    distance,
                    Hit {
                        entity: Some(body.entity),
                        point,
                        normal,
                    },
                ));
            }
        }
        Ok(best.map(|(_, hit)| hit))
    }

    /// The action's and RayCast's ray.
    fn node_ray(&mut self, id: u16, node: &Node) -> Result<Ray, RayGap> {
        let fields = &node.fields;
        if field(fields, "m_type").as_u64().unwrap_or(0) != 0 {
            return Err(RayGap::Field("m_type"));
        }
        for name in ["m_8CBE558B", "m_3B0AD973"] {
            if self.flag(id, node, field(fields, name))? {
                return Err(RayGap::Field(name));
            }
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
        let filter = field(fields, "m_castFilter");
        let ignore = self.ignored(id, node, field(fields, "m_ignoreEntities"))?;
        Ok(Ray {
            origin,
            direction,
            reach,
            radius,
            filter: BodyFilter::read(filter, &[PROFILE]).map_err(RayGap::Filter)?,
            profile: profile(filter)?,
            ignore,
        })
    }

    /// TrackRay's ray.
    fn track_ray(&mut self, id: u16, node: &Node) -> Result<Ray, RayGap> {
        let fields = &node.fields;
        for name in ["m_F745334E", "m_7AB75223"] {
            if !field(fields, name).is_null() {
                return Err(RayGap::Field(name));
            }
        }
        if self.flag(id, node, field(fields, "m_3B0AD973"))? {
            return Err(RayGap::Field("m_3B0AD973"));
        }
        let site = self.site(id, node);
        let instigator = field(fields, "m_instigator");
        if !instigator.is_null()
            && self.evaluate_quietly(id, node, instigator) != Some(Value::Entity(site.entity))
        {
            return Err(RayGap::Field("m_instigator"));
        }
        let mut ignore = self.ignored(id, node, field(fields, "m_ignoreEntities"))?;
        let start = field(fields, "m_3BC31C57");
        let start = if start.is_null() {
            [0.0; 3]
        } else {
            self.vector(id, node, start)
                .ok_or(RayGap::Field("m_3BC31C57"))?
        };
        let origin_entity = self.entity(id, node, field(fields, "m_56B8547E"), "m_56B8547E")?;
        let origin = match origin_entity {
            Some(entity) => {
                ignore.push(entity);
                self.in_entity(site, entity, start)
                    .ok_or(RayGap::Field("m_3BC31C57"))?
            }
            None => start,
        };
        let end = field(fields, "m_F5732403");
        let end = match field(end, "$").as_str() {
            Some("STU_9E9028E6") => {
                let direction = self
                    .vector(id, node, field(end, "m_direction"))
                    .ok_or(RayGap::Field("m_F5732403.m_direction"))?;
                // A direction in the entity's transform turns with it by a
                // rule not modelled; a vertical one doesn't turn.
                if origin_entity.is_some() && (direction[0] != 0.0 || direction[2] != 0.0) {
                    return Err(RayGap::Field("m_F5732403.m_direction"));
                }
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
                match self.entity(id, node, field(end, "m_entity"), "m_F5732403.m_entity")? {
                    Some(entity) => {
                        if self.flag(id, node, field(end, "m_68B8830B"))? {
                            ignore.push(entity);
                        }
                        self.in_entity(site, entity, point)
                            .ok_or(RayGap::Field("m_F5732403.m_BD631144"))?
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
        let filter = field(fields, "m_filter");
        Ok(Ray {
            origin,
            direction,
            reach,
            radius,
            filter: BodyFilter::read(filter, &[PROFILE]).map_err(RayGap::Filter)?,
            profile: profile(filter)?,
            ignore,
        })
    }

    /// A point in `entity`'s transform: its position plus a vertical
    /// offset. None for a sideways offset (it turns with the entity) or an
    /// entity the world doesn't place.
    fn in_entity(&self, site: Site, entity: u32, offset: [f64; 3]) -> Option<[f64; 3]> {
        if offset[0] != 0.0 || offset[2] != 0.0 {
            return None;
        }
        let [x, y, z] = self.world.entity_position(site, entity)?;
        Some([x, y + offset[1], z])
    }

    /// An entity config: None when unset.
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
            Some(Value::Entity(entity)) if expr::valid_entity(entity) => Ok(Some(entity)),
            _ => Err(RayGap::Field(name)),
        }
    }

    /// The entities of an ignore list (each config an entity or an array of
    /// them; invalid ones are left out).
    fn ignored(&mut self, id: u16, node: &Node, list: &Json) -> Result<Vec<u32>, RayGap> {
        let mut ignore = Vec::new();
        for config in list.as_array().into_iter().flatten() {
            let value = self
                .evaluate_quietly(id, node, config)
                .ok_or(RayGap::Field("m_ignoreEntities"))?;
            let values = match value {
                Value::Array(values) => values,
                value => vec![value],
            };
            for value in values {
                match value {
                    Value::Entity(entity) if expr::valid_entity(entity) => ignore.push(entity),
                    Value::Entity(_) | Value::Null => {}
                    _ => return Err(RayGap::Field("m_ignoreEntities")),
                }
            }
        }
        Ok(ignore)
    }

    fn flag(&mut self, id: u16, node: &Node, config: &Json) -> Result<bool, RayGap> {
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

    fn config_number(&mut self, id: u16, node: &Node, config: &Json) -> Option<f64> {
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
    fn put(&mut self, id: u16, node: &Node, name: &str, value: Value) -> Result<(), InitFault> {
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
}

/// The hit entity as the outputs hold it: none (0) for a miss or the map.
fn hit_entity(hit: Option<&Hit>) -> Value {
    Value::Entity(hit.and_then(|hit| hit.entity).unwrap_or(0))
}

/// The filter's ray profile (`m_C1903771`, `STU_20DB8AB5`): None when unset.
fn profile(filter: &Json) -> Result<Option<u64>, RayGap> {
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
