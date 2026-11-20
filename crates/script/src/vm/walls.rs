//! The wall check `STU_FBFF5DE7` (Rocket Punch `0D06` st7, the punch
//! victim's `0D07` st20, Reinhardt's Charge `007D` st3, `1537` st26; codec
//! none): while it runs it writes `m_26DA7CF8` each update, true when a
//! trial move of the body ahead of it is turned by more than
//! 90° − `m_87A4D0AD` (or stopped). The punch's server-only rays st79/st80
//! run on it (st78) and end the dash at the wall (`rid16` into the impact
//! st9).
//!
//! Client (image base 0x7FF788F10000; `refs/reports/Doomfist.md` §6):
//! slot 12 `0x7FF789C3D3E0` writes false and puts the state on the update
//! list; slot 14 `0x7FF789C3D440` on the entity's mover (component 31)
//! writes `m_87A4D0AD` (degrees, as radians) > asin(cosine) of the trial
//! (`0x7FF789C4B360`, asin `0x7FF7894D7970`), the trial's direction
//! `m_B1003775` (default the entity's forward), flat and unit
//! (`xmmword_7FF78C151790` = up when too short), its distance
//! `m_3A5CB679`. Without a mover it writes false. The trial itself is
//! [`orpheus_physics::mover::trial`]; only its displacement form
//! (`m_9A978490` true: the punch and its victim) is ported. The velocity
//! form (Reinhardt, `1537`) writes nothing, recorded.

use super::*;

const WALL_CHECK: &str = "STU_FBFF5DE7";

impl<W: World> Vm<'_, '_, '_, W> {
    /// The begin writes false; the checks follow each update.
    pub(super) fn begin_wall_check(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        self.put(key.instance, node, "m_26DA7CF8", Value::Bool(false))?;
        self.register_tick(key, self.machine.now_ms);
        Ok(())
    }

    pub(super) fn update_wall_check(
        &mut self,
        key: StateKey,
        node: &Node,
    ) -> Result<(), InitFault> {
        let id = key.instance;
        let site = self.site(id, node);
        let fields = &node.fields;
        if !self.flag_or(id, node, "m_9A978490") {
            self.assume(
                site,
                &format!("{WALL_CHECK} velocity form"),
                "writes nothing",
            );
            return Ok(());
        }
        let direction = match field(fields, "m_B1003775") {
            config if config.is_null() => self.world.entity_yaw(site, site.entity).map(|yaw| {
                let radians = f64::from(yaw) * std::f64::consts::TAU / 65536.0;
                [radians.sin(), 0.0, radians.cos()]
            }),
            config => match self.evaluate_quietly(id, node, config) {
                Some(Value::Vector(vector)) => Some(vector),
                _ => None,
            },
        };
        let distance = self.config_number(id, node, field(fields, "m_3A5CB679"));
        let (Some(direction), Some(distance)) = (direction, distance) else {
            self.assume(site, &format!("{WALL_CHECK} fields"), "writes nothing");
            return Ok(());
        };
        let direction = flat_unit(direction);
        let Some(cosine) = self
            .world
            .trial_turn(site, site.entity, direction, distance)
        else {
            self.assume(site, &format!("{WALL_CHECK} mover"), "writes nothing");
            return Ok(());
        };
        let angle = self
            .config_number(id, node, field(fields, "m_87A4D0AD"))
            .unwrap_or(0.0) as f32;
        let hit = angle * 0.017_453_292 > (cosine as f32).asin();
        self.put(id, node, "m_26DA7CF8", Value::Bool(hit))
    }

    /// A bool field; false when unset or not a value.
    pub(super) fn flag_or(&mut self, id: u16, node: &Node, name: &str) -> bool {
        let config = field(&node.fields, name);
        !config.is_null()
            && self
                .evaluate_quietly(id, node, config)
                .is_some_and(|value| expr::truthy(&value))
    }
}

/// The flat part of `vector`, unit, or up when it is too short (as the
/// client's `0x7FF789C4B360` makes the direction).
fn flat_unit([x, _, z]: [f64; 3]) -> [f64; 3] {
    let (x, z) = (x as f32, z as f32);
    let squared = z * z + x * x;
    if squared < 1.175_494_4e-36 {
        return [0.0, 1.0, 0.0];
    }
    let length = squared.sqrt();
    [f64::from(x / length), 0.0, f64::from(z / length)]
}
