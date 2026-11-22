//! Reinhardt's Charge's turn `STU_C168636D` (`007D` st6 under the charge's
//! MovementMod st36: 240, 240 and #34534 = 30; st7 under the crash's st5:
//! 0, 0, 0; codec none): a mover behaviour of kind 7
//! ([`orpheus_physics::mover::turn`], which has the client's code). Its
//! begin hands the mover its block (slot 56 `0x7FF789C40000`: the three
//! fields in degrees, each 0 when unset or no number); the mover turns the
//! body by the side keys while it runs, in place of the aim. With an output
//! (`m_9F17FE89`, st6's #85) the state writes the mover's rate each update
//! (radians a second; his client's #85 was the rate of the same frame,
//! Practice Range session r0, 2026-11-22): the charge's search cones
//! (st52, st74) and its lean (st96's data flow) read it.

use super::*;
use orpheus_physics::mover::Turn;

impl<W: World + ?Sized> Vm<'_, '_, '_, W> {
    pub(super) fn begin_turn(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let [acceleration, deceleration, greatest] = ["m_717EAFE7", "m_34A1CCB7", "m_6716919E"]
            .map(|name| match field(&node.fields, name) {
                config if config.is_null() => 0.0,
                config => self.config_number(id, node, config).unwrap_or(0.0),
            });
        self.machine.movements.insert(
            key,
            Behaviour::Turn(Turn::from_degrees(acceleration, deceleration, greatest)),
        );
        if !field(&node.fields, "m_9F17FE89").is_null() {
            self.register_tick(key, self.machine.now_ms);
        }
        Ok(())
    }

    /// The mover's rate to the output, once its turn runs.
    pub(super) fn update_turn(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let site = self.site(key.instance, node);
        match self.world.turn_rate(site, site.entity) {
            Some(rate) => self.put(key.instance, node, "m_9F17FE89", Value::Float(rate.into())),
            None => Ok(()),
        }
    }
}
