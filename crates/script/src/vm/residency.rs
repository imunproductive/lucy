//! Area residency `STU_96F571ED` (server-only graph `0F00` st12, the
//! capture area aura `02A2.053` on each body a capture point `0E8E`
//! lists): while it runs, `m_E2755955` says whether the entity
//! `m_6A78E231` (the body) stands in the volume of the entity
//! `m_2FA86F1A` (the point: the area its target tag instance names,
//! `CaptureAreas.md` §3).
//!
//! Client (image base 0x7FF788F10000; ModeGraphGaps.md "ModeGraph3"):
//! begin `0x7FF789C3BEF0` writes false, evaluates both entities and
//! registers the state with the first entity's mover (component 31,
//! `+144`, `sub_7FF789C47EF0`), keeping the area entity (`+24`) and an
//! inside flag (`+28`, false); the state's event slot `0x7FF789C3C7C0`
//! writes true on event 1 and false on event 2, which the mover raises as
//! the body comes into and leaves the area. Which point of the body the
//! mover tests was not read: the server tests the feet, which the areas'
//! boxes reach 0.3 to 1.3 m below (`CaptureAreas.md` §2) [assumption].

use super::*;

impl<W: World> Vm<'_, '_, '_, W> {
    /// The begin writes false, then tests at once and each update.
    pub(super) fn begin_area_residency(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        self.put(key.instance, node, "m_E2755955", Value::Bool(false))?;
        self.update_area_residency(key, node)?;
        self.register_tick(key, self.machine.now_ms);
        Ok(())
    }

    /// Whether the entity stands in the area now; an entity or area the
    /// world can't place writes nothing (reported).
    pub(super) fn update_area_residency(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        let mut entity = |name: &str| {
            self.evaluate_quietly(id, node, field(&node.fields, name))
                .as_ref()
                .and_then(expr::to_entity)
                .filter(|&entity| expr::valid_entity(entity))
        };
        let (Some(body), Some(area)) = (entity("m_6A78E231"), entity("m_2FA86F1A")) else {
            return self.put(id, node, "m_E2755955", Value::Bool(false));
        };
        let inside = self
            .world
            .entity_position(site, body)
            .and_then(|feet| self.world.in_entity_volume(site, area, feet));
        match inside {
            Some(inside) => self.put(id, node, "m_E2755955", Value::Bool(inside)),
            None => {
                self.report(site, Issue::UnsupportedField("area residency volume"));
                Ok(())
            }
        }
    }
}
