//! State and instance teardown.

use super::*;

impl<'graph, 'world, 'machine, W: World> Vm<'graph, 'world, 'machine, W> {
    pub(super) fn end(
        &mut self,
        key: StateKey,
        finished: bool,
        depth: usize,
    ) -> Result<(), InitFault> {
        if depth >= MAX_PLUG_DEPTH {
            return Err(InitFault::PlugDepth);
        }
        let Some(state) = self
            .state_mut(key)
            .filter(|state| state.active && !state.ending)
        else {
            return Ok(());
        };
        state.ending = true;
        let kind = state.kind;
        let id = key.instance;
        let node = self
            .graph(id)
            .state(usize::from(key.raw))
            .expect("active state node");
        self.machine
            .queues
            .retain(id, |_, event| event.state() != Some(key.raw));
        self.clear_tick(key);
        self.exit_subgraph(id, node, "m_subgraphPlug", depth + 1)?;
        match kind {
            StateKind::MovementSample(previous) => {
                self.update_movement_sample(key, node, previous, false)?;
            }
            StateKind::TargetTag {
                effect: Some(effect),
            } => {
                self.world
                    .set_target_tag_effect(self.site(id, node), effect, false);
            }
            StateKind::HealthPool { pool: Some(pool) } => {
                if let Some(current) = self.world.remove_health_pool(self.site(id, node), pool) {
                    self.publish_health_pool(key, node, current)?;
                }
            }
            StateKind::BooleanSwitch(branch) => {
                self.exit_subgraph(id, node, branch_path(branch), depth + 1)?
            }
            StateKind::DelayedSwitch {
                current,
                undecided: false,
            } => self.exit_subgraph(id, node, branch_path(current), depth + 1)?,
            StateKind::Switch { case } => {
                self.exit_subgraph(id, node, &case_path(case), depth + 1)?
            }
            StateKind::SubScript { child: Some(child) } => self.destroy(child)?,
            StateKind::MessageIdle => {
                self.exit_subgraph(id, node, "m_33DA5163", depth + 1)?;
                self.exit_subgraph(id, node, "m_452862E6", depth + 1)?;
            }
            StateKind::AiCandidate { selected: true } => {
                self.exit_subgraph(id, node, "m_6F92F4BD", depth + 1)?;
            }
            StateKind::LogicalButton { .. } => {
                self.exit_subgraph(id, node, "m_80AF45FB", depth + 1)?;
                self.exit_subgraph(id, node, "m_1C9EF058", depth + 1)?;
            }
            StateKind::Stack { .. } => {
                self.exit_subgraph(id, node, "m_topSubgraphPlug", depth + 1)?;
                self.exit_subgraph(id, node, "m_underSubgraphPlug", depth + 1)?;
                self.unlink_single_output(key, depth + 1)?;
            }
            StateKind::CosmeticEntity | StateKind::Anim => {
                self.unlink_single_output(key, depth + 1)?
            }
            StateKind::ChaseVar => self.finish_chase(key, depth + 1)?,
            StateKind::WeaponVolley { .. } => self.finish_volley(key, depth + 1)?,
            StateKind::Ability { flags } => {
                self.set_ability_flags(key, flags & !ABILITY_HELD);
                self.stop_ability(key, node, depth + 1)?;
                self.finish_cooldown(key)?;
            }
            StateKind::Defer => {
                if let Some(deferred) = self.machine.deferred.get_mut(&id) {
                    deferred.retain(|&(_, raw)| raw != key.raw);
                }
                self.exit_subgraph(id, node, "m_8E484B2E", depth + 1)?;
            }
            StateKind::DisableButton { button } => {
                self.end_disable_button(key, button, depth + 1)?;
            }
            StateKind::AiFire {
                button: Some(button),
            } => self.post_edge(ButtonEdge {
                button,
                pressed: false,
            })?,
            StateKind::MoveTo | StateKind::NavPath => {
                self.machine.movements.remove(&key);
            }
            StateKind::GrappleCast => {
                self.machine.grapples.remove(&key);
            }
            StateKind::AiMove => {
                self.machine.ai_moves.remove(&key);
            }
            StateKind::HeroSelect => {
                self.machine.hero_selects.remove(&key);
            }
            StateKind::Rewind => self.machine.motions.push(Motion::EndRewind { finished }),
            StateKind::Status => {
                self.world.set_status(self.site(id, node), None);
            }
            StateKind::CombatMod => {
                self.world.combat_mod(self.site(id, node), None);
            }
            StateKind::Assist => self.end_assist(key, node),
            StateKind::ApplyAura => self.end_auras(key),
            StateKind::Scope => self.end_scopes(key),
            StateKind::SendMessage => self.end_send_state(key, node)?,
            StateKind::StatTime => self.end_stat_time(key, node),
            _ => (),
        }
        let state = self.state_mut(key).expect("ending state");
        state.active = false;
        state.ending = false;
        state.pending_change = false;
        state.pending_timer = false;
        state.pending_check = false;
        state.delayed_timers = [false; 2];
        state.tracked.clear();
        if matches!(kind, StateKind::SubScript { .. }) {
            state.kind = StateKind::SubScript { child: None };
        }
        if matches!(kind, StateKind::HealthPool { .. }) {
            state.kind = StateKind::HealthPool { pool: None };
        }
        if matches!(kind, StateKind::TargetTag { .. }) {
            state.kind = StateKind::TargetTag { effect: None };
        }
        if matches!(kind, StateKind::LogicalButton { .. }) {
            state.kind = StateKind::LogicalButton { down: false };
        }
        if matches!(kind, StateKind::AiFire { .. }) {
            state.kind = StateKind::AiFire { button: None };
        }
        if matches!(kind, StateKind::Stack { .. }) {
            state.kind = StateKind::Stack {
                top: false,
                under: false,
            };
        }
        self.machine.watchers.retain(|_, watchers| {
            watchers.retain(|watcher| *watcher != key);
            !watchers.is_empty()
        });
        self.machine.foreign_watchers.retain(|_, watchers| {
            watchers.retain(|watcher| *watcher != key);
            !watchers.is_empty()
        });
        self.machine.native_watchers.retain(|_, watchers| {
            watchers.retain(|watcher| *watcher != key);
            !watchers.is_empty()
        });
        let ending = if finished {
            "m_onFinishedPlug"
        } else {
            "m_onAbortPlug"
        };
        self.follow(id, node, ending, Some(key), depth)?;
        self.follow(id, node, "m_onEndPlug", Some(key), depth)?;
        Ok(())
    }

    pub(super) fn exit_subgraph(
        &mut self,
        id: u16,
        node: &Node,
        path: &str,
        depth: usize,
    ) -> Result<(), InitFault> {
        let Some(plug) = node.field(path) else {
            return Ok(());
        };
        for raw in raw_indices(array_field(plug, SUBGRAPH_STATES)) {
            self.end(StateKey { instance: id, raw }, false, depth)?;
            self.drop_begin(id, raw);
        }
        Ok(())
    }

    pub(super) fn drop_begin(&mut self, id: u16, raw: u16) {
        let now = self.machine.now_ms;
        self.machine.queues.retain(id, |due, event| {
            due != now || !matches!(event, Event::Begin(state) if *state == raw)
        });
    }

    pub(super) fn exit_instance(&mut self, id: u16) -> Result<(), InitFault> {
        let parent = self.machine.snapshot.instances[&id].parent;
        self.destroy(id)?;
        let Some((instance, raw)) = parent else {
            return Ok(());
        };
        let key = StateKey { instance, raw };
        let Some(state) = self.state_mut(key).filter(|state| {
            state.active && !state.ending && state.kind == StateKind::SubScript { child: Some(id) }
        }) else {
            return Ok(());
        };
        state.kind = StateKind::SubScript { child: None };
        let node = self
            .graph(instance)
            .state(usize::from(raw))
            .expect("active subscript");
        if !configured(node, "m_24AB04BB") {
            self.post(instance, Event::Finish(raw))?;
        }
        Ok(())
    }

    pub(super) fn destroy(&mut self, id: u16) -> Result<(), InitFault> {
        let mut pending = vec![(id, false)];
        while let Some((id, children_done)) = pending.pop() {
            if !children_done {
                self.spend("instance teardown")?;
                for state in self.machine.snapshot.instances[&id].states.values_mut() {
                    state.ending = true;
                }
                pending.push((id, true));
                pending.extend(self.machine.snapshot.instances.iter().rev().filter_map(
                    |(&child, instance)| {
                        instance
                            .parent
                            .is_some_and(|(parent, _)| parent == id)
                            .then_some((child, false))
                    },
                ));
                continue;
            }
            let outputs: Vec<_> = self.machine.snapshot.instances[&id]
                .states
                .iter()
                .filter_map(|(&raw, state)| {
                    (state.linked_variable.is_some()
                        || state.ability_stack.is_some()
                        || matches!(
                            state.kind,
                            StateKind::WeaponVolley { .. }
                                | StateKind::HealthPool { pool: Some(_) }
                                | StateKind::TargetTag { effect: Some(_) }
                        )
                        || (state.kind == StateKind::ApplyAura && !state.tracked.is_empty())
                        || (state.kind == StateKind::Scope && !state.scopes.is_empty()))
                    .then_some((raw, state.kind))
                })
                .collect();
            for (raw, kind) in outputs {
                let key = StateKey { instance: id, raw };
                if let StateKind::TargetTag {
                    effect: Some(effect),
                } = kind
                {
                    let node = self.graph(id).state(usize::from(raw)).expect("target tag");
                    self.world
                        .set_target_tag_effect(self.site(id, node), effect, false);
                } else if let StateKind::HealthPool { pool: Some(pool) } = kind {
                    let node = self.graph(id).state(usize::from(raw)).expect("health pool");
                    let site = self.site(id, node);
                    self.world.remove_health_pool(site, pool);
                    self.publish_health_totals(site)?;
                } else if matches!(kind, StateKind::WeaponVolley { .. }) {
                    self.finish_volley(key, 0)?;
                } else if kind == StateKind::ChaseVar {
                    self.finish_chase(key, 0)?;
                } else if kind == StateKind::ApplyAura {
                    self.end_auras(key);
                } else if kind == StateKind::Scope {
                    self.end_scopes(key);
                } else if matches!(kind, StateKind::Ability { .. }) {
                    self.unlink_ability_stack(key, 0)?;
                    self.finish_cooldown(key)?;
                } else {
                    self.unlink_single_output(key, 0)?;
                }
            }
            // Button blocks and held AI buttons let go once the instance is
            // gone (their `on_end` in `ow174/game/script/nodes.py`).
            let controls: Vec<_> = self.machine.snapshot.instances[&id]
                .states
                .iter()
                .filter(|(_, state)| state.active)
                .filter_map(|(&raw, state)| match state.kind {
                    StateKind::DisableButton { .. } | StateKind::AiFire { button: Some(_) } => {
                        Some((StateKey { instance: id, raw }, state.kind))
                    }
                    _ => None,
                })
                .collect();
            if self.machine.snapshot.instances[&id]
                .states
                .values()
                .any(|state| state.active && state.kind == StateKind::Rewind)
            {
                self.machine
                    .motions
                    .push(Motion::EndRewind { finished: false });
            }
            // Its persistent senders let their message go
            // ([`Self::end_send_state`]).
            let senders: Vec<u16> = self.machine.snapshot.instances[&id]
                .states
                .iter()
                .filter(|(_, state)| {
                    state.active
                        && state.kind == StateKind::SendMessage
                        && !state.tracked.is_empty()
                })
                .map(|(&raw, _)| raw)
                .collect();
            for raw in senders {
                let node = self.graph(id).state(usize::from(raw)).expect("sender");
                self.end_send_state(StateKey { instance: id, raw }, node)?;
            }
            self.machine.snapshot.instances.shift_remove(&id);
            self.machine.queues.remove(id);
            self.machine.ticking.remove(&id);
            self.machine.deferred.remove(&id);
            self.machine.joins.remove(&id);
            self.machine.movements.retain(|key, _| key.instance != id);
            self.machine.grapples.retain(|key, _| key.instance != id);
            self.machine
                .hero_selects
                .retain(|key, _| key.instance != id);
            self.machine
                .ability_presses
                .retain(|key, _| key.instance != id);
            self.machine.watchers.retain(|key, watchers| {
                if matches!(key, VarKey::Instance(instance, _) if *instance == id) {
                    return false;
                }
                watchers.retain(|watcher| watcher.instance != id);
                !watchers.is_empty()
            });
            self.machine.foreign_watchers.retain(|_, watchers| {
                watchers.retain(|watcher| watcher.instance != id);
                !watchers.is_empty()
            });
            self.machine.native_watchers.retain(|_, watchers| {
                watchers.retain(|watcher| watcher.instance != id);
                !watchers.is_empty()
            });
            for (key, kind) in controls {
                match kind {
                    StateKind::DisableButton { button } => {
                        self.end_disable_button(key, button, 0)?
                    }
                    StateKind::AiFire {
                        button: Some(button),
                    } => self.post_edge(ButtonEdge {
                        button,
                        pressed: false,
                    })?,
                    _ => unreachable!("collected control state"),
                }
            }
        }
        Ok(())
    }
}
