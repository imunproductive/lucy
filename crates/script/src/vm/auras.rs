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
//! The entry's `m_FF703F20` takes the message's second entity (`+156`;
//! the client's dispatch `0x7FF78AAA55F0` writes `+152` to `m_B60ABDA4`,
//! `+156` to `m_FF703F20` and the host's vtable `+480` of `+156` to
//! `m_44D0D060`). A message the client sends (SendGameMessage's execute
//! `0x7FF789815F80`) has the sending entity at `+152` and at `+156` the
//! action's `m_8B34E9C3`, else the sending instance's `#27`, else the
//! sender (setters `0x7FF78AAB7E60`, `0x7FF78AAB7E30`; `#27` read by
//! `0x7FF78AAA2E50`). The server's aura message is taken to follow the same
//! rule: `+156` is the ApplyAura instance's `#27`, [`AuraChange::Apply`]'s
//! `creator`. The graphs agree: Mercy's damage boost `006C` (on the
//! beamed body, put there by her weapon `006B`) keeps `#40` and `#41` as
//! Mercy; its Valkyrie chain `10DD` (put on a nearby ally by `006C`) has
//! `#40` the beamed body (its beam starts there) and `#41` Mercy (`#1900e`,
//! "boosted by", is `#40` in `006C` and `#41` in `10DD`); Nano Boost's
//! `0B9F` credits Ana's stats `0413`/`0422` to `#41`. `m_44D0D060` (`#42`
//! in `10DD`) stays unset: what the host's slot gives is open.
//!
//! Assumed, as nothing in the data says: the aura's own instance leaves the
//! body after the aura's `m_7DAE8F10` seconds when that is above 0 (0.1 s
//! for `0267`, enough to play its children), else when its graph exits; a
//! second application adds a second instance (the children it plays
//! replace the running ones, PlayScript policy 3).
//!
//! ApplyAura (`STU_431F3FCC`, 59 nodes in 46 graphs) is server-only: no
//! registration of its own (the client runs the base state, codec none)
//! and no client reader of the aura's fields (`tools/who_reads.py
//! STU_76F2C784 m_5A4051BC`, `m_14AA219A`). So its rule is the server's
//! own; modelled here, as an assumption, after the data: while the state
//! runs, each body its `m_targets` (an entity or an array, mostly a
//! TrackTargets' output) lists gets the aura `m_A76A8CF6` (`STU_0E766B33`)
//! once, dealt by `m_3273B2AB` (the owner in its two uses; unset: the
//! script's entity), with its `m_params` rows (`STU_814D595E`: a `0A4`
//! identifier and a value, evaluated when the body comes in). The aura's
//! instance leaves the body when the body leaves the list or the state
//! ends, or earlier by the aura's own rule above. The data supports the
//! "while listed" part: the auras of 28 uses have 0 s and graphs that never
//! end (Biotic Field `0258` st7 → `025F` → `0B3C`: a heal rate while the
//! body is in the field), and Nano Boost's (`0B9E` st12 → `0269`) has
//! 99999 s, so the state's end must take it off. `m_6D7701B7`
//! (`STU_2B2E2F6D` of the owner, kind 2; `0CCC`, `1100`) is not read.
//! The aura goes to the target's own component through the match
//! ([`World::change_aura`]); the same body can be the target.

use super::*;

/// The game message an aura's graph takes when it comes on a body.
pub const AURA_APPLIED: u64 = 0x0240_0000_0000_000f;

/// The ApplyAura activation that put an aura on a body: its entity,
/// instance, state and activation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AuraSource {
    pub entity: u32,
    pub instance: u16,
    pub state: u16,
    pub activation: u16,
}

/// An aura an ApplyAura state puts on a body or takes off it
/// ([`World::change_aura`]), or one a hit puts on it (no source). The
/// match hands it to the target's component ([`Component::change_aura`]).
#[derive(Clone, Debug, PartialEq)]
pub enum AuraChange {
    Apply {
        source: Option<AuraSource>,
        target: u32,
        /// The aura (`053` GUID).
        aura: u64,
        instigator: u32,
        /// The entity the aura entry's `m_FF703F20` takes: the applying
        /// instance's creator (`#27`), else the instigator.
        creator: u32,
        /// The parameters (`0A4` GUID and value).
        parameters: Vec<(u64, Value)>,
    },
    Remove {
        source: AuraSource,
        target: u32,
    },
}

impl AuraChange {
    /// The body it is for.
    pub fn target(&self) -> u32 {
        match *self {
            Self::Apply { target, .. } | Self::Remove { target, .. } => target,
        }
    }
}

impl Component {
    /// Put the aura `aura` (a `053` GUID) on this body, dealt by
    /// `instigator` for `creator` ([`AuraChange::Apply`]), with the
    /// parameters an ApplyAura names (`0A4` GUID and value). Returns the
    /// aura's instance; none when the bundle lacks the aura or its graph.
    pub fn apply_aura(
        &mut self,
        aura: u64,
        instigator: u32,
        creator: u32,
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
            vm.start_aura(id, instigator, creator, parameters)?;
            vm.run()
        })?;
        Ok(created.then_some(id))
    }

    /// Apply an aura change for this body: put the aura on it
    /// ([`Self::apply_aura`]), keeping which ApplyAura put it there, or
    /// take off the aura that ApplyAura put there (its instance, unless it
    /// already left). Returns whether anything changed.
    pub fn change_aura(
        &mut self,
        change: &AuraChange,
        world: &mut impl World,
    ) -> Result<bool, InitFault> {
        match change {
            AuraChange::Apply {
                source,
                aura,
                instigator,
                creator,
                parameters,
                ..
            } => {
                let created = self.apply_aura(*aura, *instigator, *creator, parameters, world)?;
                if let (Some(source), Some(id)) = (source, created) {
                    self.machine.applied_auras.insert(*source, id);
                }
                Ok(created.is_some())
            }
            AuraChange::Remove { source, .. } => {
                let Some(id) = self.machine.applied_auras.remove(source) else {
                    return Ok(false);
                };
                if !self.machine.snapshot.instances.contains_key(&id) {
                    return Ok(false);
                }
                self.run(world, |vm| {
                    vm.post(id, Event::ExitInstance)?;
                    vm.run()
                })?;
                Ok(true)
            }
        }
    }
}

impl<W: World> Vm<'_, '_, '_, W> {
    /// Each "aura applied" entry of the new instance takes the instigator,
    /// the creator and its parameters, then runs.
    fn start_aura(
        &mut self,
        id: u16,
        instigator: u32,
        creator: u32,
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
            let creator = if expr::valid_entity(creator) {
                creator
            } else {
                instigator
            };
            if expr::valid_entity(creator) {
                let target = field(&node.fields, "m_FF703F20");
                self.bind_start_value(id, node, target, Value::Entity(creator))?;
            }
            if !field(&node.fields, "m_44D0D060").is_null() {
                self.assume(
                    self.site(id, node),
                    "STUStatescriptEntryGameMessage m_44D0D060",
                    "unset",
                );
            }
            self.follow(id, node, "m_outPlug", None, 0)?;
        }
        Ok(())
    }

    /// ApplyAura's begin and tick: the aura on each body that came into
    /// `m_targets`, off each body that left it.
    pub(super) fn refresh_auras(&mut self, key: StateKey, node: &Node) -> Result<(), InitFault> {
        let id = key.instance;
        let listed = self.listed_entities(key, node, "m_targets");
        let state = self.state(key).expect("active ApplyAura");
        let source = AuraSource {
            entity: self.machine.snapshot.entity,
            instance: id,
            state: key.raw,
            activation: state.activation,
        };
        let left: Vec<u32> = state
            .tracked
            .iter()
            .map(|seen| seen.entity)
            .filter(|entity| !listed.contains(entity))
            .collect();
        let came: Vec<u32> = listed
            .into_iter()
            .filter(|&entity| !state.tracked.iter().any(|seen| seen.entity == entity))
            .collect();
        if left.is_empty() && came.is_empty() {
            return Ok(());
        }
        let site = self.site(id, node);
        let mut taken = true;
        for &target in &left {
            taken &= self
                .world
                .change_aura(site, AuraChange::Remove { source, target });
        }
        let mut applied = Vec::new();
        if !came.is_empty()
            && let Some(Value::Asset(aura)) =
                self.evaluate(id, node, field(&node.fields, "m_A76A8CF6"), Some(key))
        {
            let instigator = match field(&node.fields, "m_3273B2AB") {
                Json::Null => None,
                config => self
                    .evaluate(id, node, config, Some(key))
                    .and_then(|value| expr::to_entity(&value)),
            }
            .unwrap_or(source.entity);
            let creator = match self.machine.value(VarKey::Instance(id, CREATOR), None) {
                Value::Null => self.source_of(id),
                written => expr::to_entity(&written).unwrap_or(0),
            };
            if !field(&node.fields, "m_6D7701B7").is_null() {
                self.assume(site, "STU_431F3FCC m_6D7701B7", "not read");
            }
            let mut parameters = Vec::new();
            for row in array_field(&node.fields, "m_params") {
                let Some(name) = row.get("m_B5051BCE").and_then(asset_guid) else {
                    continue;
                };
                if let Some(value) = self.evaluate(id, node, field(row, "m_value"), Some(key)) {
                    parameters.push((name, value));
                }
            }
            for &target in &came {
                taken &= self.world.change_aura(
                    site,
                    AuraChange::Apply {
                        source: Some(source),
                        target,
                        aura,
                        instigator,
                        creator,
                        parameters: parameters.clone(),
                    },
                );
            }
            applied = came;
        }
        if !taken {
            self.assume(site, &node.class, "no effect (the world applies no aura)");
        }
        let now = self.machine.now_ms;
        let state = self.state_mut(key).expect("active ApplyAura");
        state.tracked.retain(|seen| !left.contains(&seen.entity));
        state
            .tracked
            .extend(applied.into_iter().map(|entity| TrackedTarget {
                entity,
                since_ms: now,
            }));
        Ok(())
    }

    /// ApplyAura ended, or its instance went: its auras come off every body
    /// it put them on.
    pub(super) fn end_auras(&mut self, key: StateKey) {
        let Some(state) = self.state(key) else {
            return;
        };
        let source = AuraSource {
            entity: self.machine.snapshot.entity,
            instance: key.instance,
            state: key.raw,
            activation: state.activation,
        };
        let targets: Vec<u32> = state.tracked.iter().map(|seen| seen.entity).collect();
        let node = self
            .graph(key.instance)
            .state(usize::from(key.raw))
            .expect("ApplyAura");
        let site = self.site(key.instance, node);
        for target in targets {
            self.world
                .change_aura(site, AuraChange::Remove { source, target });
        }
    }
}
