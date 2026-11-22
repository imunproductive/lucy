//! Widowmaker's Grappling Hook (graph `005A`): the cast `STU_4BBC5D09` and
//! the kind-10 mover behaviour state `STU_1F5ED928`. The pull between them
//! is a `MoveTo` (`lucy_physics::mover::move_to`).
//!
//! The cast (registration #1708, the winner over the base #595; codec
//! `start`, the milliseconds since it began):
//!
//! - Its begin (slot 23 `0x7FF789BA1AA0`) writes the hook's start and way
//!   from the view (`0x7FF789BABC90`: with neither `m_D750D816` nor
//!   `m_F0F18008` set, the view's position and forward from
//!   `0x7FF789C2D280` and the eye height (`0x7FF789CAF3F0`: the mover's
//!   stand or crouch eye, times the body's scale) along the body's up)
//!   into `m_5ED803CF` (#431) and `m_64801077` (#433), the hardpoint's point
//!   (slot 56, `m_F745334E`) into `m_F2F8606D` (#432), no entity into
//!   `m_1FFD7AC0` (#38), and the aim assist's outcome into `m_9169D109`
//!   (#1221), `m_58B4C4D7` (#1220) and `m_EA134F86` (#1219): a cone search
//!   of `m_3D5CAF44` degrees (`10 * #9585e`, a hero setting no client sets
//!   by default) and `m_7B36ED50` metres (`0x7FF789B68FF0`). Then it runs
//!   its update once. #431 and #432 are its bound outputs: the begin binds
//!   them to the state (`0x7FF78AA83570` → `0x7FF78AA83430`: the value into
//!   the variable, then the binding `0x7FF78AAB2B80` with slot 0 and 1,
//!   priority 0, at or above).
//! - Its update (slot 14 `0x7FF789BA3AB0`, once a frame) casts from #431
//!   along #433 for the range (`m_2C40C158` when #1221 holds, else
//!   `m_2AFC083F`) with filter `m_66B658A5`, and again from the view now to
//!   where the hook is: #431 plus #433 times the seconds since the begin
//!   times `m_speed`. That second cast meeting something ends the cast on
//!   it; else the hook reaching the first cast's hit (or the range) ends it
//!   on the first cast's outcome. The hit counts when the hit object's
//!   entity passes `m_6F7A45D9` (`0x7FF789BB8730`; none passes).
//! - Its end (slot 24 `0x7FF789BA35C0`; not for the end codes 5 to 7 a
//!   teardown uses) writes the hit into `m_5ED803CF` and `m_F2F8606D`
//!   through their bindings (`0x7FF78AA848F0`: the value, then the
//!   binding's release; a variable no longer bound to the state is not
//!   written), its normal into `m_D7A2A03B` (#434) and `m_13BE2D3D`, the
//!   world point into `m_835453C7`, the hit entity and bone into
//!   `m_1FFD7AC0` (#38) and `m_86CF0002` (#8695) (entity-local when the
//!   entity has component 1) and true into `m_9E1A7C58` (#435); a miss only
//!   writes false there. The state's end releases what is still bound.
//!
//! The owner frames carry #431 and #432 with their bindings: a frame that
//! listed them without (as the server sent them before) unbound the cast on
//! the client, so its own end left #431 at the start, at her eye, and
//! `005A` rid100 (`dist(#1175, #431) >= 2`, #1175 her feet) took the miss
//! branch: the pull began only with the server's `MoveTo` an owner frame's
//! round trip later (`refs/reports/Grapple.md`, "DayFirstGrapple2").
//!
//! What the server does (assumptions, recorded): the view is the body's
//! eye (its feet and eye height) looking along its command's yaw and pitch;
//! the hardpoint's point is that eye (the server has no hardpoints); the
//! aim assist finds nothing (a cone above 0 is recorded); the casts are the
//! map and body casts of `vm/rays.rs` (a map hit has no entity, so it
//! counts and its point is a world point); the update runs on each frame
//! from the begin's, the seconds being whole frames; `m_783CC3B5` (extra
//! casts around the way) and `m_89460C8D` are unset in the data and not
//! read.
//!
//! `STU_1F5ED928` (the pull's last 0.2 s, a child of the mover behaviour
//! base `STU_1C2FB897`): its begin (`0x7FF789C407D0`) hands the mover a
//! behaviour of kind 10 (slot 55; vtable `0x7FF78B56A568`) with the body's
//! position, the point `m_03AD31B9` and the seconds `m_710CF237` (slot 56
//! `0x7FF789C39370`): `lucy_physics::mover::path_walk`, which asks the
//! client's path query for a way through the air from the feet to the point
//! and walks the body along it in those seconds (a hook at a ledge's top
//! walks her up onto it). The state finishes on the update that sees the
//! walk done or no path found (slot 14 `0x7FF789C390C0`: the record's
//! state is 2), which the mover reports as the move's end.

use super::{
    rays::{Hit, RayGap},
    targets::{BodyFilter, Gap, TAG_LISTS, is_set},
    *,
};

const CAST: &str = "STU_4BBC5D09";
/// Widowmaker's Grappling Hook graph: its `MoveTo` aborts when the body
/// passes its target (the behaviour block's `+153`, `0x7FF789C43A60`).
pub(super) const GRAPPLING_HOOK: u32 = 0x005A;
const PATH: &str = "STU_1F5ED928";
/// The fields the server reads for the hook's start from the view; any
/// other way of starting it is not modelled.
const START_FIELDS: [&str; 4] = ["m_D750D816", "m_F0F18008", "m_705D6A01", "m_794A5BCA"];
/// The cast's bound outputs and their slots: the hook's start (#431) and
/// the hardpoint's point (#432).
const OUTPUTS: [(&str, u8); 2] = [("m_5ED803CF", 0), ("m_F2F8606D", 1)];

/// A running cast: where it started and what it met.
#[derive(Clone, Debug)]
pub(super) struct Cast {
    origin: [f64; 3],
    direction: [f64; 3],
    /// The hit the end writes (mover `+0x200`), when one was met.
    hit: Option<Hit>,
}

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// The cast's begin.
    pub(super) fn begin_grapple(
        &mut self,
        key: StateKey,
        node: &Node,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        if let Some(name) = START_FIELDS
            .into_iter()
            .find(|name| is_set(field(&node.fields, name)))
        {
            self.assume(site, &format!("{CAST} {name}"), "casts nothing");
            return Ok(());
        }
        let Some((origin, direction)) = self.view(site) else {
            self.assume(site, &format!("{CAST} view"), "casts nothing (no eye)");
            return Ok(());
        };
        let cone = self
            .config_number(id, node, field(&node.fields, "m_3D5CAF44"))
            .unwrap_or(0.0);
        if cone > 0.0 {
            self.assume(
                site,
                &format!("{CAST} m_3D5CAF44"),
                "the aim assist finds nothing",
            );
        }
        self.assume(site, CAST, "the hook flies from the eye, one cast a frame");
        for (name, slot) in OUTPUTS {
            self.bind_output(key, node, name, slot, Value::Vector(origin), depth)?;
        }
        for (name, value) in [
            ("m_64801077", Value::Vector(direction)),
            ("m_EA134F86", Value::Vector([0.0; 3])),
            ("m_58B4C4D7", Value::Vector([0.0; 3])),
            ("m_9169D109", Value::Bool(false)),
            ("m_1FFD7AC0", Value::Entity(0)),
        ] {
            self.put(id, node, name, value)?;
        }
        self.machine.grapples.insert(
            key,
            Cast {
                origin,
                direction,
                hit: None,
            },
        );
        self.register_tick(key, self.machine.now_ms);
        self.tick_grapple(key, depth)
    }

    /// `0x7FF78AA83430`: the value into the variable `name` names, then
    /// the variable bound to the state with `slot`.
    fn bind_output(
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
        let old = self.machine.value(target, None);
        self.set_base(target, value.clone());
        self.insert_link(
            target,
            VariableLink {
                state: key,
                priority: LINK_PRIORITY,
                output: LinkValue::Output { slot, value },
                stack: false,
            },
            depth,
        )?;
        self.notify_value(target, &old, None)
    }

    /// The variable an output field names.
    pub(super) fn output_key(&mut self, id: u16, node: &Node, name: &str) -> Option<VarKey> {
        match expr::lvalue(field(&node.fields, name)) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(self.site(id, node), Issue::Expression(error));
                None
            }
        }
    }

    /// The cast's update: the hook flies on, or the cast ends on what it met.
    pub(super) fn tick_grapple(&mut self, key: StateKey, depth: usize) -> Result<(), InitFault> {
        let Some(mut cast) = self.machine.grapples.get(&key).cloned() else {
            return Ok(());
        };
        let id = key.instance;
        let node = self.graph(id).state(usize::from(key.raw)).expect("grapple");
        let site = self.site(id, node);
        let started = self
            .state(key)
            .map_or(self.machine.now_ms, |state| state.start_ms);
        let assisted = self
            .evaluate_quietly(id, node, field(&node.fields, "m_9169D109"))
            .is_some_and(|value| truthy(&value));
        let long = field(&node.fields, "m_2C40C158");
        let range = if assisted && is_set(long) {
            long
        } else {
            field(&node.fields, "m_2AFC083F")
        };
        let (Some(range), Some(speed)) = (
            self.config_number(id, node, range),
            self.config_number(id, node, field(&node.fields, "m_speed")),
        ) else {
            self.assume(site, &format!("{CAST} range"), "casts nothing");
            self.machine.grapples.remove(&key);
            return Ok(());
        };
        let reached = match self.hook_cast(id, node, cast.origin, cast.direction, range) {
            Ok(Some(hit)) => {
                cast.hit = Some(hit);
                distance(cast.origin, hit.point)
            }
            Ok(None) => range,
            Err(gap) => {
                self.ray_gap(site, CAST, gap);
                self.machine.grapples.remove(&key);
                return Ok(());
            }
        };
        let flown = (self.machine.now_ms - started) as f64 * 0.001 * speed;
        let tip: [f64; 3] =
            std::array::from_fn(|axis| cast.origin[axis] + cast.direction[axis] * flown);
        let eye = self.view(site).map(|(eye, _)| eye);
        let met = match eye {
            Some(eye) if distance(eye, tip) > 1e-9 => {
                let way = std::array::from_fn(|axis| (tip[axis] - eye[axis]) / distance(eye, tip));
                match self.hook_cast(id, node, eye, way, distance(eye, tip)) {
                    Ok(met) => met,
                    Err(gap) => {
                        self.ray_gap(site, CAST, gap);
                        None
                    }
                }
            }
            _ => None,
        };
        let outcome = match met {
            Some(hit) => Some(Some(hit)),
            None if flown < reached => None,
            None => Some(cast.hit),
        };
        let Some(hit) = outcome else {
            self.machine.grapples.insert(key, cast);
            return Ok(());
        };
        let hit = match hit {
            Some(hit) if self.hook_holds(id, node, hit)? => Some(hit),
            _ => None,
        };
        self.write_hook(key, node, hit, depth)?;
        self.machine.grapples.remove(&key);
        self.clear_tick(key);
        self.post(id, Event::Finish(key.raw))
    }

    /// One cast of the hook with its filter: what it met first.
    fn hook_cast(
        &mut self,
        id: u16,
        node: &Node,
        origin: [f64; 3],
        direction: [f64; 3],
        reach: f64,
    ) -> Result<Option<Hit>, RayGap> {
        let ray = Self::ray(
            origin,
            direction,
            reach,
            0.0,
            node,
            "m_66B658A5",
            Vec::new(),
        )?;
        self.cast(id, node, &ray)
    }

    /// `0x7FF789BB8730`: the map counts; a body when it passes `m_6F7A45D9`.
    fn hook_holds(&mut self, id: u16, node: &Node, hit: Hit) -> Result<bool, InitFault> {
        let Some(entity) = hit.entity else {
            return Ok(true);
        };
        let config = field(&node.fields, "m_6F7A45D9");
        if !is_set(config) {
            return Ok(true);
        }
        let site = self.site(id, node);
        let allowed = ["m_condition", TAG_LISTS[0], TAG_LISTS[1], TAG_LISTS[2]];
        let holds = BodyFilter::read(config, &allowed)
            .map_err(Gap::Filter)
            .and_then(|filter| self.filter_admits(id, node, config, &filter, entity));
        match holds {
            Ok(holds) => Ok(holds),
            Err(_) => {
                self.assume(site, &format!("{CAST} m_6F7A45D9"), "a body hit holds");
                Ok(true)
            }
        }
    }

    /// The end's outputs: a hit goes into the bound outputs as their
    /// bindings are released (`0x7FF78AA848F0`), then into the rest.
    fn write_hook(
        &mut self,
        key: StateKey,
        node: &Node,
        hit: Option<Hit>,
        depth: usize,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let Some(hit) = hit else {
            return self.put(id, node, "m_9E1A7C58", Value::Bool(false));
        };
        let point = Value::Vector(hit.point);
        for (name, _) in OUTPUTS {
            if let Some(target) = self.output_key(id, node, name) {
                self.remove_link(key, target, Some(point.clone()), depth)?;
            }
        }
        for (name, value) in [
            ("m_835453C7", point),
            ("m_9E1A7C58", Value::Bool(true)),
            ("m_D7A2A03B", Value::Vector(hit.normal)),
            ("m_13BE2D3D", Value::Vector(hit.normal)),
            ("m_1FFD7AC0", Value::Entity(hit.entity.unwrap_or(0))),
            ("m_86CF0002", Value::Integer(0)),
        ] {
            self.put(id, node, name, value)?;
        }
        Ok(())
    }

    /// The view the hook starts from: the eye and the aim.
    fn view(&mut self, site: Site) -> Option<([f64; 3], [f64; 3])> {
        let (eye, facing) = self.world.eye(site, site.entity)?;
        Some((eye, expr::aim(facing)))
    }

    /// `STU_1F5ED928`'s begin: the walk it hands the mover. A point or
    /// seconds that aren't a vector and a number hand nothing (the state
    /// then runs until its parent ends it).
    pub(super) fn begin_nav_path(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        self.machine.movements.remove(&key);
        let point = self.evaluate(id, node, field(&node.fields, "m_03AD31B9"), None);
        let seconds = match field(&node.fields, "m_710CF237") {
            Json::Null => Some(0.0),
            config => self.config_number(id, node, config),
        };
        match (point, seconds) {
            (Some(Value::Vector(point)), Some(seconds))
                if point.iter().all(|axis| axis.is_finite()) =>
            {
                self.machine
                    .movements
                    .insert(key, Behaviour::PathWalk(PathWalk { point, seconds }));
            }
            _ => self.assume(
                self.site(id, node),
                PATH,
                "hands the mover nothing (no point or seconds)",
            ),
        }
        Ok(())
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3)
        .map(|axis| (a[axis] - b[axis]).powi(2))
        .sum::<f64>()
        .sqrt()
}
