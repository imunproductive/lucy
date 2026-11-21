//! Pickups: the server-only pair a health pack and a hurt hero meet
//! through. Neither class has client code (`python tools/stu.py`: no
//! registration of their own, the base state's lifecycle, codec `none`), so
//! what they do is read from their fields and their only uses, which share
//! the pickup kind `m_1A69E234` (`STU_B9BE2BE1`, enum `AD46F1CE`, 0 in
//! both):
//!
//! - `STU_913C09E4` (`0046` rid11 st5, the health pack): while active, the
//!   entity `m_504473B0` (the pack, `owner()`) is a pickup of its kind: a
//!   sphere of `m_radius` (0.7) round `m_06B5A4F1` (its feet + 0.5 up) for
//!   the team `m_2D0C0615` (the team of its owner `#6396e`, else -1: any).
//!   The pack ends it as it is taken (rid19's out plug aborts st4/st5) and
//!   begins it again as it respawns (rid13).
//! - `STU_87621906` (`0033` rid22 st11, every hero's body graph, active
//!   while the body is alive and hurt, st10): writes into `m_0E739B87`
//!   (#1195) the pickup of its kind the body `m_05140BC3` (`owner()`)
//!   touches, which `0033` then sends `0643.025` every 0.5 s (rid23), and
//!   the pack takes as the body's request (rid12 EntryGameMessage `0643`,
//!   its sender into #8049): it heals him (st0 ModifyHealth #199) unless it
//!   is respawning (#467), he is dead (#478e), under anti-heal (tag `02FB`)
//!   or #11252e.
//!
//! The match keeps the pickups ([`World::set_pickup`]) and answers which
//! one a body touches ([`World::touched_pickup`]).

use super::*;

/// A pickup a `STU_913C09E4` state holds ([`World::set_pickup`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pickup {
    /// The entity a body that touches it names (`m_504473B0`).
    pub entity: u32,
    /// Its kind (`m_1A69E234`): a search finds only its own kind.
    pub kind: i64,
    /// Its sphere: centre and radius.
    pub centre: [f64; 3],
    pub radius: f64,
    /// The team whose bodies find it; -1: any team.
    pub team: i64,
}

impl Pickup {
    /// Whether the sphere touches a body's capsule: from `feet` up
    /// `height`, `radius` round.
    pub fn touches(&self, feet: [f64; 3], height: f64, radius: f64) -> bool {
        let [x, y, z] = self.centre;
        let low = feet[1] + radius;
        let high = (feet[1] + height - radius).max(low);
        let nearest = y.clamp(low, high);
        let (dx, dy, dz) = (x - feet[0], y - nearest, z - feet[2]);
        let reach = self.radius + radius;
        dx * dx + dy * dy + dz * dz <= reach * reach
    }
}

impl<W: World> Vm<'_, '_, '_, W> {
    /// `STU_913C09E4` begins: the world keeps its pickup until it ends. A
    /// world that keeps none leaves it, as an assumption.
    pub(super) fn begin_pickup(&mut self, key: StateKey, node: &Node) {
        let id = key.instance;
        let mut read = |name| self.evaluate(id, node, field(&node.fields, name), None);
        let (Some(entity), Some(kind), Some(centre), Some(radius), Some(team)) = (
            read("m_504473B0"),
            read("m_1A69E234"),
            read("m_06B5A4F1"),
            read("m_radius"),
            read("m_2D0C0615"),
        ) else {
            return;
        };
        let site = self.site(id, node);
        let (Some(entity), Value::Vector(centre)) = (expr::to_entity(&entity), centre) else {
            self.report(site, Issue::UnsupportedField("pickup entity or position"));
            return;
        };
        let pickup = Pickup {
            entity,
            kind: expr::to_int(&kind, -1),
            centre,
            radius: expr::to_float(&radius),
            team: expr::to_int(&team, -1),
        };
        if !self.world.set_pickup(site, Some(pickup)) {
            self.assume(site, &node.class, "the world keeps no pickup");
        }
    }

    pub(super) fn end_pickup(&mut self, key: StateKey, node: &Node) {
        self.world.set_pickup(self.site(key.instance, node), None);
    }

    /// `STU_87621906`, at its begin and each tick: the pickup the body
    /// touches now (none: Null) into its output, when it changed. A world
    /// that knows no pickups writes nothing, as an assumption.
    pub(super) fn search_pickup(
        &mut self,
        key: StateKey,
        node: &Node,
        begin: bool,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let mut read = |name| self.evaluate(id, node, field(&node.fields, name), None);
        let (Some(body), Some(kind)) = (read("m_05140BC3"), read("m_1A69E234")) else {
            return Ok(());
        };
        let site = self.site(id, node);
        let target = match expr::lvalue(field(&node.fields, "m_0E739B87")) {
            Ok(target) => target.map(|target| source_key(id, target)),
            Err(error) => {
                self.report(site, Issue::Expression(error));
                return Ok(());
            }
        };
        let found = match expr::to_entity(&body) {
            Some(body) => self
                .world
                .touched_pickup(site, body, expr::to_int(&kind, -1)),
            None => Some(None),
        };
        let Some(found) = found else {
            self.assume(site, &node.class, "the world knows no pickups");
            return Ok(());
        };
        let state = self.state_mut(key).expect("active pickup search");
        let StateKind::PickupSearch { found: known } = state.kind else {
            return Ok(());
        };
        if !begin && known == found {
            return Ok(());
        }
        state.kind = StateKind::PickupSearch { found };
        if let Some(target) = target {
            self.write(target, found.map_or(Value::Null, Value::Entity))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A health pack's sphere (0.7 m round its feet + 0.5 m) touches a
    /// standing body (1.8 m, 0.35 m round) beside it, not one a step away
    /// or one far above.
    #[test]
    fn a_pickup_touches_the_bodies_its_sphere_reaches() {
        let pack = Pickup {
            entity: 1,
            kind: 0,
            centre: [0.0, 0.5, 0.0],
            radius: 0.7,
            team: -1,
        };
        assert!(pack.touches([0.0, 0.0, 0.0], 1.8, 0.35));
        assert!(pack.touches([1.0, 0.0, 0.0], 1.8, 0.35));
        assert!(!pack.touches([1.1, 0.0, 0.0], 1.8, 0.35));
        assert!(!pack.touches([0.0, 1.6, 0.0], 1.8, 0.35));
    }
}
