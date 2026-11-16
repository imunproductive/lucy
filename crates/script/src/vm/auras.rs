//! Auras: a status a body carries, whose effect is a graph that runs on the
//! body (`053`, [`crate::graph::Aura`]).
//!
//! An aura comes on a body as a new root instance of its graph, whose
//! "aura applied" entry (`STUStatescriptEntryGameMessage` for message
//! `000F`) takes the instigator in `m_B60ABDA4` (`#40`) and the parameters
//! its rows name (`m_params`, keyed by `0A4` identifiers), and runs at once.
//! Biotic Grenade's `0267` runs `0C71`, whose entry takes `0A4:002A` to
//! `#499` and `0A4:0708` to `#18763`, then plays `0B99` on the instigator's
//! ally and `0C72` on its enemy (`STU_53F4CA65`), each with the
//! instigator. Those hold the status (`#8203e` heal boost or `#8202e`
//! anti-heal, the status icon over the nameplate), change the healing the
//! body takes (`CombatModFilter`), heal or damage it and end after their
//! `Wait` (4 s) with the instance (`STU_22926252`).
//!
//! The aura's instance is created with its applier, so its `#27` (the
//! creator, [`CREATOR`]) is the instigator, as are its children's: `0C72`
//! and `0B99` pick their amount by whether an AI controls `#27` (`0C72`
//! node 6: 60 damage, else an AI-scaled value; `0B99` node 7: 100
//! healing; `STU_D3EB8D87` of `STU_DB742151` of `#27`).
//!
//! Assumed, as nothing in the data says: the aura's own instance leaves the
//! body after the aura's `m_7DAE8F10` seconds when that is above 0 (0.1 s
//! for `0267`, enough to play its children), else when its graph exits; a
//! second application adds a second instance (the children it plays
//! replace the running ones, PlayScript policy 3); the entry's `m_FF703F20`
//! (`#41`) stays unset.

use super::*;

/// The game message an aura's graph takes when it comes on a body.
pub const AURA_APPLIED: u64 = 0x0240_0000_0000_000f;

impl Component {
    /// Put the aura `aura` (a `053` GUID) on this body, dealt by
    /// `instigator`, with the parameters an ApplyAura names (`0A4` GUID and
    /// value). Returns the aura's instance; none when the bundle lacks the
    /// aura or its graph.
    pub fn apply_aura(
        &mut self,
        aura: u64,
        instigator: u32,
        parameters: &[(u64, Value)],
        world: &mut impl World,
    ) -> Result<Option<u16>, InitFault> {
        let Some(found) = self.graphs.aura(aura).copied() else {
            return Ok(None);
        };
        let Some(graph) = found
            .graph
            .filter(|&graph| self.graphs.graph(graph).is_some())
        else {
            return Ok(None);
        };
        let id = u16::try_from(self.machine.next_id).map_err(|_| InitFault::InstanceIdExhausted)?;
        let mut created = false;
        self.run(world, |vm| {
            created = vm.create(id, graph, None, Variables::new(), instigator)?;
            if !created {
                return Ok(());
            }
            if found.seconds > 0.0 {
                let delay = (found.seconds * 1000.0).round() as u64;
                vm.post_after(id, delay, Event::ExitInstance)?;
            }
            vm.start_aura(id, instigator, parameters)?;
            vm.run()
        })?;
        Ok(created.then_some(id))
    }
}

impl<W: World> Vm<'_, '_, '_, W> {
    /// Each "aura applied" entry of the new instance takes the instigator
    /// and its parameters, then runs.
    fn start_aura(
        &mut self,
        id: u16,
        instigator: u32,
        parameters: &[(u64, Value)],
    ) -> Result<(), InitFault> {
        let graph = self.graph(id);
        for node in graph.nodes.iter().filter(|node| {
            !node.client_only
                && node.class == "STUStatescriptEntryGameMessage"
                && node
                    .field("m_gameMessage.m_gameMessage")
                    .and_then(asset_guid)
                    == Some(AURA_APPLIED)
        }) {
            for row in array_field(&node.fields, "m_params") {
                let Some(key) = row.get("m_B5051BCE").and_then(asset_guid) else {
                    continue;
                };
                if let Some((_, value)) = parameters.iter().find(|(name, _)| *name == key) {
                    self.bind_start_value(id, node, field(row, "m_out_Var"), value.clone())?;
                }
            }
            let source = field(&node.fields, "m_B60ABDA4");
            if expr::valid_entity(instigator) {
                self.bind_start_value(id, node, source, Value::Entity(instigator))?;
            }
            if !field(&node.fields, "m_FF703F20").is_null() {
                self.assume(
                    self.site(id, node),
                    "STUStatescriptEntryGameMessage m_FF703F20",
                    "unset",
                );
            }
            self.follow(id, node, "m_outPlug", None, 0)?;
        }
        Ok(())
    }
}
