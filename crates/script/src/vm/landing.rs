//! The placement selector `STU_4C96470A` in its landing form (Seismic Slam
//! in the air, `0B27` st6; codec: one byte): while its gate `m_B02B1FD7`
//! (`#1321`) holds, each update searches where along the view the body can
//! land and writes the point (`m_30710B76`, `#431`) and whether it found
//! one (`m_C3240FFC`, `#1627`); the slam's transition (`#1627 && #1321`)
//! then begins its `MoveTo` (st9) to the point.
//!
//! Client (image base 0x7FF788F10000; `refs/reports/Doomfist.md` §5,
//! §10): the winning registration #1707 (vtable 0x7FF78B52E920), its update
//! `0x7FF789B93C50` → `0x7FF789BA8E90` (a kept result per frame) → compute
//! `0x7FF789BC6BA0`. A found point clears the destination entity
//! (`m_1549F0F9`) unless the ground under it is an entity's (the server's
//! search meets the static map only), writes the point and true; a miss
//! writes false and keeps the last point. The search is
//! [`orpheus_physics::mover::landing`]. The selector's other form (a
//! summoner's placement, confirmed by its input plug `m_6647ED22`) runs as
//! a plain state, recorded.

use super::*;
use orpheus_physics::mover::landing::LandingSearch;

const SELECTOR: &str = "STU_4C96470A";
/// The landing form's fields: its search's length (`m_6386E7B4`) and gate.
const LENGTH: &str = "m_6386E7B4";
const GATE: &str = "m_B02B1FD7";

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    /// The landing form ticks while it runs; the other form is plain.
    pub(super) fn begin_landing(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        if field(&node.fields, LENGTH).is_null() || field(&node.fields, GATE).is_null() {
            let site = self.site(id, node);
            self.assume(
                site,
                SELECTOR,
                "a placement without a landing search does nothing",
            );
            return Ok(());
        }
        if !self.flag_or(id, node, "m_A6BB0869") {
            return Ok(());
        }
        self.register_tick(key, self.machine.now_ms);
        self.update_landing(key, node)
    }

    /// One search while the gate holds.
    pub(super) fn update_landing(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        if !self.flag_or(id, node, GATE) {
            return Ok(());
        }
        let site = self.site(id, node);
        let Some(search) = self.landing_search(id, node) else {
            self.assume(site, &format!("{SELECTOR} fields"), "finds no landing");
            return Ok(());
        };
        let profile = match rays::profile(field(&node.fields, "m_castFilter")) {
            Ok(profile) => profile,
            Err(_) => {
                self.assume(
                    site,
                    &format!("{SELECTOR} m_castFilter"),
                    "the default profile",
                );
                None
            }
        };
        let Some(point) = self
            .world
            .landing_point(site, site.entity, &search, profile)
        else {
            self.assume(site, &format!("{SELECTOR} map"), "finds no landing");
            return Ok(());
        };
        if let Some(point) = point {
            self.put(id, node, "m_2FECD1E3", Value::Entity(0))?;
            self.put(id, node, "m_1549F0F9", Value::Entity(0))?;
            self.put(id, node, "m_30710B76", Value::Vector(point))?;
        }
        self.put(id, node, "m_C3240FFC", Value::Bool(point.is_some()))
    }

    /// The search from the body's eye along its view, with the node's
    /// numbers. None: a field isn't a number, or the body has no eye.
    fn landing_search(&mut self, id: u16, node: &Node) -> Option<LandingSearch> {
        let site = self.site(id, node);
        let (origin, facing) = self.world.eye(site, site.entity)?;
        let number =
            |vm: &mut Self, name: &str| vm.config_number(id, node, field(&node.fields, name));
        // `0x7FF789BAE800`: with `m_B221D18D`, no ground above
        // `m_6E8CA10B` (the slam: his eye's height).
        let ceiling = if self.flag_or(id, node, "m_B221D18D") {
            Some(number(self, "m_6E8CA10B")?)
        } else {
            None
        };
        Some(LandingSearch {
            origin,
            aim: expr::aim(facing),
            length: number(self, LENGTH)?,
            radius: number(self, "m_3656A811")?,
            step: number(self, "m_B3094B0E")?,
            slope: number(self, "m_2ADABA5F")?,
            spacing: number(self, "m_B42B2BEB")?,
            clearance: number(self, "m_60B6AF94")?,
            reach_height: number(self, "m_15F880A4")?,
            ceiling,
        })
    }
}
