//! The shockwave `STUStatescriptStateShockwave` (`STU_01D67FA0`;
//! Doomfist's slam `0B27` st44): a wave that grows to its radius, hitting
//! the bodies it reaches, and then finishes the state.
//!
//! Client (image base 0x7FF788F10000): the begin `0x7FF789BA2510` sets the
//! wave's reach (state `+660`) to 0 and, unless `m_04F3240D` holds, sends
//! the wave once (`0x7FF789BBD0D0`). The update `0x7FF789BA5270`
//! evaluates the radius `m_6A79BD98` and `m_speed`: the wave takes
//! `radius / max(speed, 1e-5)` seconds to reach its radius, no time when
//! `m_3418CEA4` holds; its reach is that share of the radius (all of it
//! when the time is under 0.1 ms). While the seconds since the begin
//! (`0x7FF78AA81D10`, ms × 0.001) are fewer it sends the wave again; after,
//! it sends it once more when `m_3418CEA4` holds and finishes the state
//! (`0x7FF78AA812E0`). The update also runs in the begin's frame, after
//! the begin has begun the subgraph's states (slot 14 runs from the
//! frame's update list): the owner's client never shows `0B27` st44 at the
//! end of the frame it begins in (df10 s1 `cp10_slam`, frame 6761), and
//! the slam's ModifyHealth st42 must be running when the wave's hits come
//! in. A host with flag `0x400` at `+792` never sends or finishes it (not
//! modelled).
//!
//! Sending the wave (`0x7FF789BBD0D0`), only with an output array
//! `m_8A2905A0`:
//!
//! - Its shape (`0x7FF789BA9A80`): the centre `m_05BE62DD` (a world point
//!   without the entity `m_8DCA77B6`), the flat direction `(sin y, 0,
//!   cos y)` of the yaw `m_D45E22FA` in degrees, the cosine of half the
//!   angle `m_A30B383E`, the reach as its radius, and an inner radius
//!   `reach - m_2C434213` when that is set. The bodies it finds
//!   (`0x7FF789BBACF0`, the same search and shape test as
//!   `STUStatescriptStateTrackTargets`, `vm/targets.rs`) pass its filter
//!   `m_filter`.
//! - Each such body not in the array yet: with `m_95FFC3C5`, the sight
//!   line `0x7FF789BB3570` from `m_D59188D5` (ray profile of
//!   `m_3D667CE3`, without the sweep, the given point or the side rays)
//!   must see it, keeping where its clear ray met the body; then the ground
//!   walk: with `m_7A61C4A9`, from that point back to the sight line's
//!   origin over the ground (`0x7FF789BAC810`,
//!   [`orpheus_physics::mover::ground_walk`], drop `m_FF3B7DF2`, climb
//!   `m_E81B2985`, probe `m_525E5A40`, the sample rays with the ray
//!   profile of `m_9A8CA980`); without it, a walk over the navigation
//!   voxels (`0x7FF78A61A9D0`), which the server doesn't have.
//! - The hits are added to the array after the bodies it held
//!   (`0x7FF789AF3D70`, ordered by `m_43478CAD` when it is not 0), which is
//!   written when there are new ones or it has no value. The subgraph's
//!   ModifyHealth (`0B27` st42) strikes each body that comes in.
//!
//! The server runs the form Doomfist's slam has (`m_7A61C4A9`,
//! `m_95FFC3C5`, no entity, no inner radius, no timed form `m_04F3240D`, no
//! rewind `m_1C2A9945`, order 0); any other form with an array hits nothing,
//! recorded as an assumption naming the field. Its sight line casts rays 2
//! and 3 of a TrackTargets' (0.3 and 0.6 of the body's height) for the
//! client's hardpoint ray first, recorded as an assumption. Reinhardt's
//! Earthshatter (`00BE`) is the timed form with the voxel walk; Meteor
//! Strike's (`0C51` st9) and Wrecking Ball's (`160A` st9) waves have no
//! array.

use super::{
    rays,
    targets::{BodyFilter, Cone, Gap, TAG_LISTS, in_cone, is_set},
    *,
};
use orpheus_physics::mover::ground_walk::GroundWalk;

const CLASS: &str = "STUStatescriptStateShockwave";
const FOUND: &str = "m_8A2905A0";
/// The ray profile of the ground walk's first and last casts
/// (`0016.097`, `0x7FF789BAC810`).
const WALK_PROFILE: u64 = 0x0690_0000_0000_0016;
/// Fields a form the server runs leaves unset.
const UNMODELLED: &[&str] = &[
    "m_8DCA77B6",
    "m_04F3240D",
    "m_2C434213",
    "m_1C2A9945",
    "m_43478CAD",
];
/// Fields a form the server runs sets.
const REQUIRED: &[&str] = &["m_7A61C4A9", "m_95FFC3C5"];

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    pub(super) fn begin_shockwave(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        if !self.wave_flag(key.instance, node, "m_04F3240D") {
            self.send_wave(key, node, 0.0)?;
        } else {
            self.assume(
                self.site(key.instance, node),
                &format!("{CLASS} m_04F3240D"),
                "the wave timed from its begin",
            );
        }
        self.register_tick(key, self.machine.now_ms);
        // The update after the subgraph's begins, which are queued too.
        self.post(key.instance, Event::Timer(key.raw, 0))
    }

    /// The wave grows and is sent, or has reached its radius: the state
    /// finishes.
    pub(super) fn update_shockwave(&mut self, key: StateKey) -> Result<(), InitFault> {
        let Some(state) = self.state(key) else {
            return Ok(());
        };
        let start_ms = state.start_ms;
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("shockwave");
        let mut float = |name| {
            self.evaluate(id, node, field(&node.fields, name), None)
                .map_or(0.0, |value| expr::to_float(&value) as f32)
        };
        let radius = float("m_6A79BD98");
        let speed = float("m_speed").max(0.000_01);
        let instant = self.wave_flag(id, node, "m_3418CEA4");
        let seconds = if instant { 0.0 } else { radius / speed };
        let elapsed = self.machine.now_ms.saturating_sub(start_ms) as f32 * 0.001;
        let share = if seconds <= 0.000_1 {
            1.0
        } else {
            elapsed / seconds
        };
        let reach = f64::from(share.clamp(0.0, 1.0) * radius);
        if elapsed < seconds {
            return self.send_wave(key, node, reach);
        }
        if instant {
            self.send_wave(key, node, reach)?;
        }
        self.post(id, Event::Finish(key.raw))
    }

    fn wave_flag(&mut self, id: u16, node: &Node, name: &str) -> bool {
        !field(&node.fields, name).is_null()
            && self
                .evaluate(id, node, field(&node.fields, name), None)
                .is_some_and(|value| truthy(&value))
    }

    /// The hit pass (see the module notes) at `reach`: the new hits added
    /// to the output array.
    fn send_wave(&mut self, key: StateKey, node: &Node, reach: f64) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        let output = match expr::lvalue(field(&node.fields, FOUND)) {
            Ok(Some(output)) => source_key(id, output),
            Ok(None) => return Ok(()),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(());
            }
        };
        let current = self.machine.value(output, None);
        let mut found: Vec<Value> = match &current {
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        };
        let held: Vec<u32> = found
            .iter()
            .filter_map(|item| match *item {
                Value::Entity(entity) => Some(entity),
                _ => None,
            })
            .collect();
        let hits = match self.wave_hits(id, node, reach, &held) {
            Ok(hits) => hits,
            Err(name) => {
                self.assume(site, &format!("{CLASS} {name}"), "hits nothing");
                return Ok(());
            }
        };
        if hits.is_empty() && current != Value::Null {
            return Ok(());
        }
        found.extend(hits.into_iter().map(Value::Entity));
        self.write(output, Value::Array(found))
    }

    /// The bodies the wave hits now that `held` doesn't list, in the
    /// world's order; or the field of a form the server can't run.
    fn wave_hits(
        &mut self,
        id: u16,
        node: &Node,
        reach: f64,
        held: &[u32],
    ) -> Result<Vec<u32>, &'static str> {
        let fields = &node.fields;
        if let Some(name) = UNMODELLED.iter().find(|name| is_set(field(fields, name))) {
            return Err(name);
        }
        if let Some(name) = REQUIRED.iter().find(|name| !is_set(field(fields, name))) {
            return Err(name);
        }
        let site = self.site(id, node);
        let filter_config = field(fields, "m_filter");
        let filter = BodyFilter::read(
            filter_config,
            &["m_condition", TAG_LISTS[0], TAG_LISTS[1], TAG_LISTS[2]],
        )?;
        let centre = match self.evaluate_quietly(id, node, field(fields, "m_05BE62DD")) {
            Some(Value::Vector(centre)) => centre,
            _ => return Err("m_05BE62DD"),
        };
        let degrees = |vm: &mut Self, name| match field(fields, name) {
            Json::Null => Ok(0.0),
            config => vm
                .evaluate_quietly(id, node, config)
                .map(|value| expr::to_float(&value))
                .filter(|value| value.is_finite())
                .ok_or(name),
        };
        let yaw = degrees(self, "m_D45E22FA")?.to_radians();
        let half = (degrees(self, "m_A30B383E")? * 0.5).to_radians();
        let cone = Cone {
            direction: [yaw.sin(), 0.0, yaw.cos()],
            cos: [half.cos(); 2],
            cylinder: None,
        };
        let bodies = self.passing_bodies(site, &filter).map_err(gap_field)?;
        let mut candidates = Vec::new();
        for body in bodies.iter().filter(|body| {
            !held.contains(&body.entity)
                && body.distance(centre) <= reach
                && in_cone(body, centre, &cone, reach)
        }) {
            if self
                .filter_admits(id, node, filter_config, &filter, body.entity)
                .map_err(gap_field)?
            {
                candidates.push(body);
            }
        }
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let sight = self.sight(id, node, centre)?;
        let ray_profile = match field(fields, "m_9A8CA980") {
            Json::Null => None,
            filter => rays::profile(filter).map_err(|_| "m_9A8CA980")?,
        };
        let float = |vm: &mut Self, name| {
            vm.evaluate_quietly(id, node, field(fields, name))
                .map(|value| expr::to_float(&value))
                .filter(|value| value.is_finite())
                .ok_or(name)
        };
        let drop = float(self, "m_FF3B7DF2")?;
        let climb = float(self, "m_E81B2985")?;
        let probe = float(self, "m_525E5A40")?;
        let map = self.world.map_rays().ok_or("m_95FFC3C5")?;
        let mut hits = Vec::new();
        for body in candidates {
            let Some(from) = sight.seen_point(map, body).ok_or("m_95FFC3C5")? else {
                continue;
            };
            let walk = GroundWalk {
                feet: body.feet,
                from,
                to: sight.origin,
                drop,
                climb,
                probe,
            };
            if self
                .world
                .ground_walk(site, &walk, Some(WALK_PROFILE), ray_profile)
                .ok_or("m_7A61C4A9")?
            {
                hits.push(body.entity);
            }
        }
        self.assume(
            site,
            &format!("{CLASS} m_95FFC3C5 hardpoints"),
            "rays to 0.3 and 0.6 of the height, no hardpoint 0DC0.0001/01CD",
        );
        Ok(hits)
    }
}

fn gap_field(gap: Gap) -> &'static str {
    match gap {
        Gap::Field(name) | Gap::Filter(name) => name,
        Gap::NoBodies => "bodies",
    }
}
